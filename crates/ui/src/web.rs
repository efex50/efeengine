//! The web backend: the [`UiTree`] as DOM elements under the `#ui` layer element (wasm32 only).
//! Design and consequences: `docs/ui/API.md` sections 5, 6, 10 and 12; the pure parts
//! (element mapping, class list, declarations, diff) are in [`crate::dom_plan`].
//!
//! - One element per node, found by the `data-efui-id` attribute (`WidgetId::to_bits`). Inline
//!   style = the resolved normal style; hover, pressed, focus and disabled come from the theme
//!   stylesheet (`<style id="efui-theme">`).
//! - One delegated listener per event type on the layer element (no closure per widget). The
//!   closures live in the backend and are removed again on drop.
//! - Only the DOM thread (main) touches this; the backend is not `Send`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use wasm_bindgen::{Clamped, JsCast, closure::Closure};
use web_sys::{
    CanvasRenderingContext2d, Document, Element, Event, EventTarget, HtmlCanvasElement, HtmlElement, HtmlInputElement,
    ImageData, KeyboardEvent, PointerEvent, Text,
};

use crate::backend::{UiBackend, UiBackendCx};
use crate::capture::UiCapture;
use crate::css;
use crate::dom_plan::{self, Decl};
use crate::id::{Owner, WidgetId};
use crate::journal::UiChange;
use crate::node::{ImageFit, KindTag, NodeKind};
use crate::signal::{UiSignal, UiSignalKind, UiValue};
use crate::theme::Theme;
use crate::tree::UiTree;

const ID_ATTR: &str = "data-efui-id";
/// Marks the layer element; core's key handling ignores events from inside it.
const LAYER_ATTR: &str = "data-efui-layer";
const DEFAULT_LAYER_ID: &str = "ui";

#[derive(Default)]
struct Shared {
    signals: Vec<UiSignal>,
    kinds: HashMap<u64, KindTag>,
    hover: HashSet<u64>,
    /// The pointer is over something inside the layer element (pointerover / pointerout).
    over: bool,
}

struct Entry {
    el: HtmlElement,
    /// Checkbox: the `<input>`.
    input: Option<HtmlInputElement>,
    /// Checkbox: the label span.
    label_span: Option<HtmlElement>,
    /// Button: the text node shown while the node has no children.
    label_text: Option<Text>,
    /// The inline declarations last written.
    decls: Vec<Decl>,
    /// The resolved direction last used for this node's children.
    dir: crate::style::Direction,
    /// Modal layer root: the full-window backdrop element and its declarations.
    backdrop: Option<(HtmlElement, Vec<Decl>)>,
}

struct Listener {
    target: EventTarget,
    kind: &'static str,
    closure: Closure<dyn FnMut(Event)>,
}

pub struct WebBackend {
    inner: Option<Inner>,
}

struct Inner {
    document: Document,
    root: HtmlElement,
    theme_style: HtmlElement,
    theme_written: bool,
    entries: HashMap<WidgetId, Entry>,
    shared: Rc<RefCell<Shared>>,
    listeners: Vec<Listener>,
}

impl WebBackend {
    pub fn new() -> Self {
        Self { inner: None }
    }
}

impl Default for WebBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn warn(msg: &str) {
    web_sys::console::warn_1(&format!("efeengine-ui: {msg}").into());
}

fn find_or_create_root(document: &Document) -> Option<HtmlElement> {
    let cfg = efeengine_core::engine::config::startup_config();
    let id = cfg.ui_layer_id.as_deref().unwrap_or(DEFAULT_LAYER_ID);
    if let Some(el) = document.get_element_by_id(id) {
        return el.dyn_into().ok();
    }
    let el: HtmlElement = document.create_element("div").ok()?.dyn_into().ok()?;
    el.set_id(id);
    let _ = el.style().set_property("position", "absolute");
    let _ = el.style().set_property("inset", "0");
    let _ = el.style().set_property("pointer-events", "none");
    let canvas_id = cfg.canvas_id.as_deref().unwrap_or("canvas");
    let parent = document
        .get_element_by_id(canvas_id)
        .and_then(|c| c.parent_element())
        .or_else(|| document.body().map(Into::into))?;
    parent.append_child(&el).ok()?;
    Some(el)
}

fn head_style(document: &Document, id: &str) -> Option<HtmlElement> {
    if let Some(el) = document.get_element_by_id(id) {
        return el.dyn_into().ok();
    }
    let el: HtmlElement = document.create_element("style").ok()?.dyn_into().ok()?;
    el.set_id(id);
    document.head()?.append_child(&el).ok()?;
    Some(el)
}

impl UiBackend for WebBackend {
    fn init(&mut self, _cx: &mut UiBackendCx) {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            warn("no document, the web backend does nothing");
            return;
        };
        let Some(root) = find_or_create_root(&document) else {
            warn("could not find or create the ui layer element");
            return;
        };
        let (Some(base_style), Some(theme_style)) = (head_style(&document, "efui-base"), head_style(&document, "efui-theme")) else {
            warn("could not create the stylesheets");
            return;
        };
        base_style.set_text_content(Some(css::base_stylesheet()));
        let _ = root.set_attribute(LAYER_ATTR, "");
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut inner = Inner {
            document,
            root,
            theme_style,
            theme_written: false,
            entries: HashMap::new(),
            shared,
            listeners: Vec::new(),
        };
        inner.attach_listeners();
        self.inner = Some(inner);
    }

    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], theme: &Theme, cx: &mut UiBackendCx) {
        if let Some(inner) = &mut self.inner {
            inner.sync(tree, changes, theme, cx);
        }
    }

    fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
        if let Some(inner) = &self.inner {
            out.append(&mut inner.shared.borrow_mut().signals);
        }
    }

    fn capture(&self) -> UiCapture {
        let Some(inner) = &self.inner else { return UiCapture::default() };
        let pointer = inner.shared.borrow().over && inner.root.matches(":hover").unwrap_or(true);
        let keyboard = inner.document.active_element().is_some_and(|a| {
            inner.root.contains(Some(&a))
                && matches!(a.tag_name().to_ascii_lowercase().as_str(), "input" | "button" | "textarea" | "select")
        });
        UiCapture { pointer, keyboard }
    }
}

/// The widget element at or above `target` (up to the layer element) whose kind satisfies `pred`.
fn widget_at(shared: &Shared, root: &Element, target: Option<EventTarget>, pred: impl Fn(KindTag) -> bool) -> Option<(WidgetId, Element)> {
    let mut cur = target?.dyn_into::<Element>().ok();
    while let Some(el) = cur {
        if el == *root {
            return None;
        }
        if let Some(bits) = el.get_attribute(ID_ATTR).and_then(|s| s.parse::<u64>().ok()) {
            if shared.kinds.get(&bits).is_some_and(|k| pred(*k)) {
                return Some((WidgetId::from_bits(bits), el));
            }
        }
        cur = el.parent_element();
    }
    None
}

fn signal(widget: WidgetId, kind: UiSignalKind) -> UiSignal {
    UiSignal { widget, key: None, owner: Owner::Game, kind }
}

fn input_of(target: Option<EventTarget>) -> Option<HtmlInputElement> {
    target?.dyn_into().ok()
}

impl Inner {
    fn listen(&mut self, kind: &'static str, mut f: impl FnMut(&Shared, &Element, Event) -> Option<Vec<UiSignal>> + 'static) {
        let shared = self.shared.clone();
        let root: Element = self.root.clone().into();
        let closure = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
            let mut sh = shared.borrow_mut();
            if let Some(mut sigs) = f(&sh, &root, e) {
                sh.signals.append(&mut sigs);
            }
        });
        let target: EventTarget = self.root.clone().into();
        if target.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref()).is_ok() {
            self.listeners.push(Listener { target, kind, closure });
        } else {
            warn(&format!("could not add the {kind} listener"));
        }
    }

    fn attach_listeners(&mut self) {
        use KindTag::*;
        self.listen("click", |sh, root, e| {
            let (id, el) = widget_at(sh, root, e.target(), |k| matches!(k, Button | Checkbox))?;
            // A mouse or touch click (detail > 0) gives the focus back to the page, so the game
            // keeps its keys; a keyboard activation (detail 0) keeps it for keyboard navigation.
            if e.dyn_ref::<web_sys::MouseEvent>().is_some_and(|m| m.detail() > 0)
                && let Some(active) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.active_element())
                && el.contains(Some(&active))
                && let Ok(h) = active.dyn_into::<HtmlElement>()
            {
                let _ = h.blur();
            }
            (sh.kinds.get(&id.to_bits()) == Some(&Button)).then(|| vec![signal(id, UiSignalKind::Click)])
        });
        self.listen("change", |sh, root, e| {
            let input = input_of(e.target())?;
            if input.type_() != "checkbox" {
                return None;
            }
            let (id, _) = widget_at(sh, root, e.target(), |k| k == Checkbox)?;
            Some(vec![signal(id, UiSignalKind::Changed(UiValue::Bool(input.checked())))])
        });
        self.listen("input", |sh, root, e| {
            let input = input_of(e.target())?;
            match input.type_().as_str() {
                "range" => {
                    let (id, _) = widget_at(sh, root, e.target(), |k| k == Slider)?;
                    let v = input.value().parse::<f32>().ok()?;
                    Some(vec![signal(id, UiSignalKind::Changed(UiValue::F32(v)))])
                }
                "text" | "password" => {
                    let (id, _) = widget_at(sh, root, e.target(), |k| k == TextInput)?;
                    Some(vec![signal(id, UiSignalKind::Changed(UiValue::Text(input.value())))])
                }
                _ => None,
            }
        });
        self.listen("keydown", |sh, root, e| {
            let ke = e.clone().dyn_into::<KeyboardEvent>().ok()?;
            if ke.key() != "Enter" || ke.is_composing() {
                return None;
            }
            let input = input_of(e.target())?;
            let (id, _) = widget_at(sh, root, e.target(), |k| k == TextInput)?;
            Some(vec![signal(id, UiSignalKind::Submit(input.value()))])
        });
        for (kind, focus) in [("focusin", true), ("focusout", false)] {
            self.listen(kind, move |sh, root, e| {
                let (id, _) = widget_at(sh, root, e.target(), |k| matches!(k, Button | Checkbox | Slider | TextInput))?;
                Some(vec![signal(id, UiSignalKind::Focus(focus))])
            });
        }
        // Hover: pointerover / pointerout bubble; every hover-enabled widget element above the
        // target that the pointer did not already come from (or go to) gets one signal.
        for (kind, enter) in [("pointerover", true), ("pointerout", false)] {
            self.listen(kind, move |sh, root, e| {
                let related = e.dyn_ref::<PointerEvent>().and_then(|p| p.related_target()).and_then(|t| t.dyn_into::<Element>().ok());
                let mut out = Vec::new();
                let mut cur = e.target().and_then(|t| t.dyn_into::<Element>().ok());
                while let Some(el) = cur {
                    if el == *root {
                        break;
                    }
                    if let Some(bits) = el.get_attribute(ID_ATTR).and_then(|s| s.parse::<u64>().ok())
                        && sh.hover.contains(&bits)
                        && !related.as_ref().is_some_and(|r| el.contains(Some(r)))
                    {
                        out.push(signal(WidgetId::from_bits(bits), UiSignalKind::Hover(enter)));
                    }
                    cur = el.parent_element();
                }
                if out.is_empty() { None } else { Some(out) }
            });
        }
        // Pointer-over-widget flag for `capture()`.
        let shared = self.shared.clone();
        let root: Element = self.root.clone().into();
        for kind in ["pointerover", "pointerout", "pointerup", "pointercancel"] {
            let shared = shared.clone();
            let root = root.clone();
            let closure = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
                let over = match kind {
                    "pointerover" => e.target().and_then(|t| t.dyn_into::<Element>().ok()).is_some_and(|t| t != root),
                    "pointerout" => {
                        let rel = e.dyn_ref::<PointerEvent>().and_then(|p| p.related_target()).and_then(|t| t.dyn_into::<Element>().ok());
                        rel.is_some_and(|r| r != root && root.contains(Some(&r)))
                    }
                    // A finished touch leaves no pointer behind.
                    _ => {
                        if e.dyn_ref::<PointerEvent>().is_some_and(|p| p.pointer_type() == "touch") {
                            false
                        } else {
                            return;
                        }
                    }
                };
                // The listener only runs from the layer element; borrow failure cannot happen
                // because the other listeners release their borrow before returning.
                if let Ok(mut sh) = shared.try_borrow_mut() {
                    sh.over = over;
                }
            });
            let target: EventTarget = self.root.clone().into();
            if target.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref()).is_ok() {
                self.listeners.push(Listener { target, kind, closure });
            }
        }
    }

    // ----- sync ------------------------------------------------------------------------------

    fn sync(&mut self, tree: &UiTree, changes: &[UiChange], theme: &Theme, cx: &mut UiBackendCx) {
        let mut restyle_all = false;
        if !self.theme_written {
            self.theme_style.set_text_content(Some(&css::theme_state_stylesheet(theme)));
            self.theme_written = true;
        }
        let mut dirty_parents: Vec<WidgetId> = Vec::new();
        for ch in changes {
            match *ch {
                UiChange::Created(id) => {
                    if tree.get(id).is_none() {
                        continue;
                    }
                    self.create(tree, id, cx);
                    match tree.parent(id) {
                        Some(p) => dirty_parents.push(p),
                        None => self.attach_layer(tree, id),
                    }
                }
                UiChange::Removed(id) => self.remove(id),
                UiChange::Moved { parent, .. } => dirty_parents.push(parent),
                UiChange::Kind(id) => self.apply_kind(tree, id, cx),
                UiChange::Style(id) | UiChange::Classes(id) => {
                    let old = self.entries.get(&id).map(|e| e.dir);
                    self.restyle(tree, id);
                    // A direction change moves every child's Fill mapping: restyle them too.
                    if old != self.entries.get(&id).map(|e| e.dir) {
                        for c in tree.children(id) {
                            self.restyle(tree, *c);
                        }
                    }
                }
                UiChange::Visible(id) => self.apply_visible(tree, id),
                UiChange::Enabled(id) => {
                    for n in tree.subtree(id) {
                        self.apply_disabled(tree, n);
                        self.restyle(tree, n);
                    }
                }
                UiChange::HoverSignals(id) => {
                    let bits = id.to_bits();
                    let mut sh = self.shared.borrow_mut();
                    if tree.get(id).is_some_and(|n| n.hover_signals()) {
                        sh.hover.insert(bits);
                    } else {
                        sh.hover.remove(&bits);
                    }
                }
                UiChange::Layer(id) => {
                    self.restyle(tree, id);
                    self.sync_backdrop(tree, id);
                }
                UiChange::Theme => {
                    self.theme_style.set_text_content(Some(&css::theme_state_stylesheet(theme)));
                    restyle_all = true;
                }
                UiChange::Font(_) => {}
            }
        }
        if restyle_all {
            let ids: Vec<WidgetId> = self.entries.keys().copied().collect();
            for id in ids {
                self.restyle(tree, id);
            }
        }
        dirty_parents.sort();
        dirty_parents.dedup();
        for p in dirty_parents {
            self.place_children(tree, p);
        }
    }

    fn make_element(&self, tag: &str) -> Option<HtmlElement> {
        self.document.create_element(tag).ok()?.dyn_into().ok()
    }

    fn create(&mut self, tree: &UiTree, id: WidgetId, cx: &UiBackendCx) {
        let Some(node) = tree.get(id) else { return };
        let Some(el) = self.make_element(dom_plan::element_tag(node.kind())) else {
            warn("could not create an element");
            return;
        };
        let mut entry = Entry {
            el: el.clone(),
            input: None,
            label_span: None,
            label_text: None,
            decls: Vec::new(),
            dir: crate::style::Direction::Column,
            backdrop: None,
        };
        match node.kind() {
            NodeKind::Button { .. } => {
                let _ = el.set_attribute("type", "button");
                entry.label_text = Some(self.document.create_text_node(""));
            }
            NodeKind::Checkbox { .. } => {
                if let (Some(input), Some(span)) = (self.make_element("input"), self.make_element("span")) {
                    let _ = input.set_attribute("type", "checkbox");
                    let _ = el.append_child(&input);
                    let _ = el.append_child(&span);
                    entry.input = input.dyn_into().ok();
                    entry.label_span = Some(span);
                }
            }
            NodeKind::Slider { .. } => {
                let _ = el.set_attribute("type", "range");
            }
            NodeKind::TextInput { .. } => {
                let _ = el.set_attribute("type", "text");
            }
            NodeKind::Image { .. } => {}
            _ => {}
        }
        let bits = id.to_bits();
        let _ = el.set_attribute(ID_ATTR, &bits.to_string());
        {
            let mut sh = self.shared.borrow_mut();
            sh.kinds.insert(bits, node.tag());
            if node.hover_signals() {
                sh.hover.insert(bits);
            }
        }
        self.entries.insert(id, entry);
        self.apply_kind(tree, id, cx);
        self.apply_visible(tree, id);
        self.apply_disabled(tree, id);
        self.restyle(tree, id);
        self.sync_backdrop(tree, id);
    }

    fn attach_layer(&mut self, tree: &UiTree, id: WidgetId) {
        let Some(e) = self.entries.get(&id) else { return };
        let _ = self.root.append_child(&e.el);
        self.sync_backdrop(tree, id);
    }

    fn remove(&mut self, id: WidgetId) {
        if let Some(e) = self.entries.remove(&id) {
            e.el.remove();
            if let Some((b, _)) = e.backdrop {
                b.remove();
            }
        }
        let bits = id.to_bits();
        let mut sh = self.shared.borrow_mut();
        sh.kinds.remove(&bits);
        sh.hover.remove(&bits);
    }

    /// Orders the DOM children of `parent` like `tree.children(parent)`. A button without
    /// children shows its label text instead; checkboxes, sliders, inputs and images have no
    /// DOM children from the tree.
    fn place_children(&mut self, tree: &UiTree, parent: WidgetId) {
        let Some(pe) = self.entries.get(&parent) else { return };
        let kind = tree.kind(parent);
        if !matches!(kind, Some(NodeKind::Container | NodeKind::Scroll | NodeKind::Button { .. })) {
            return;
        }
        let mut wanted: Vec<web_sys::Node> = tree
            .children(parent)
            .iter()
            .filter_map(|c| self.entries.get(c))
            .map(|e| e.el.clone().into())
            .collect();
        if wanted.is_empty() {
            if let Some(t) = &pe.label_text {
                wanted.push(t.clone().into());
            }
        }
        let el = pe.el.clone();
        let mut cursor = el.first_child();
        for w in &wanted {
            if cursor.as_ref() == Some(w) {
                cursor = w.next_sibling();
            } else {
                let _ = el.insert_before(w, cursor.as_ref());
            }
        }
        while let Some(c) = cursor {
            cursor = c.next_sibling();
            let _ = el.remove_child(&c);
        }
    }

    fn apply_kind(&mut self, tree: &UiTree, id: WidgetId, cx: &UiBackendCx) {
        let (Some(e), Some(kind)) = (self.entries.get(&id), tree.kind(id)) else { return };
        match kind {
            NodeKind::Text { text } => {
                if e.el.text_content().as_deref() != Some(text) {
                    e.el.set_text_content(Some(text));
                }
            }
            NodeKind::Button { label } => {
                if let Some(t) = &e.label_text {
                    if t.data() != *label {
                        t.set_data(label);
                    }
                }
            }
            NodeKind::Checkbox { label, checked } => {
                if let Some(i) = &e.input {
                    i.set_checked(*checked);
                }
                if let Some(s) = &e.label_span {
                    if s.text_content().as_deref() != Some(label) {
                        s.set_text_content(Some(label));
                    }
                }
            }
            NodeKind::Slider { min, max, step, value } => {
                if let Ok(i) = e.el.clone().dyn_into::<HtmlInputElement>() {
                    i.set_min(&min.to_string());
                    i.set_max(&max.to_string());
                    i.set_step(&if *step > 0.0 { step.to_string() } else { "any".to_string() });
                    i.set_value(&value.to_string());
                }
            }
            NodeKind::TextInput { value, placeholder, max_len, password } => {
                if let Ok(i) = e.el.clone().dyn_into::<HtmlInputElement>() {
                    i.set_type(if *password { "password" } else { "text" });
                    i.set_placeholder(placeholder);
                    if *max_len > 0 {
                        i.set_max_length(*max_len as i32);
                    } else {
                        let _ = i.remove_attribute("maxlength");
                    }
                    // Not while it already shows the value: a write would move the caret.
                    if i.value() != *value {
                        i.set_value(value);
                    }
                }
            }
            NodeKind::Image { texture, fit } => {
                let style = e.el.style();
                let _ = style.set_property(
                    "object-fit",
                    match fit {
                        ImageFit::Contain => "contain",
                        ImageFit::Cover => "cover",
                        ImageFit::Fill => "fill",
                        ImageFit::None => "none",
                    },
                );
                if let Some((_, img)) = cx.images.iter().find(|(h, _)| h == texture) {
                    draw_image(&e.el, img.size(), img.rgba8());
                }
            }
            NodeKind::Container | NodeKind::Scroll => {}
        }
    }

    fn apply_visible(&mut self, tree: &UiTree, id: WidgetId) {
        let (Some(e), Some(n)) = (self.entries.get(&id), tree.get(id)) else { return };
        set_flag(&e.el, "hidden", !n.visible());
        if let Some((b, _)) = &e.backdrop {
            set_flag(b, "hidden", !n.visible());
        }
    }

    fn apply_disabled(&mut self, tree: &UiTree, id: WidgetId) {
        let Some(e) = self.entries.get(&id) else { return };
        let off = dom_plan::effectively_disabled(tree, id);
        set_flag(&e.el, css::DISABLED_ATTR, off);
        match tree.kind(id) {
            Some(NodeKind::Button { .. } | NodeKind::Slider { .. } | NodeKind::TextInput { .. }) => set_flag(&e.el, "disabled", off),
            Some(NodeKind::Checkbox { .. }) => {
                if let Some(i) = &e.input {
                    set_flag(i, "disabled", off);
                }
            }
            _ => {}
        }
    }

    /// Writes the class attribute and the changed inline properties of one node.
    fn restyle(&mut self, tree: &UiTree, id: WidgetId) {
        let Some(e) = self.entries.get_mut(&id) else { return };
        let _ = e.el.set_attribute("class", &dom_plan::class_attr(tree, id));
        let new = dom_plan::node_decls(tree, id);
        apply_decls(&e.el, &e.decls, &new);
        e.decls = new;
        e.dir = dom_plan::direction_of(tree, id);
    }

    /// A modal layer root gets a transparent full-window element below it that takes the
    /// pointer events (a layer root itself lets them through); other layers have none.
    fn sync_backdrop(&mut self, tree: &UiTree, id: WidgetId) {
        let Some(layer) = tree.get(id).and_then(|n| n.layer().copied()) else { return };
        let Some(el) = self.make_element("div") else { return };
        let Some(e) = self.entries.get_mut(&id) else { return };
        if !layer.modal {
            if let Some((b, _)) = e.backdrop.take() {
                b.remove();
            }
            return;
        }
        let new = dom_plan::backdrop_decls(&layer);
        let hidden = tree.get(id).is_some_and(|n| !n.visible());
        match &mut e.backdrop {
            Some((b, old)) => {
                apply_decls(b, old, &new);
                *old = new;
                set_flag(b, "hidden", hidden);
            }
            None => {
                let _ = el.set_attribute("class", "efui efui-backdrop");
                apply_decls(&el, &[], &new);
                set_flag(&el, "hidden", hidden);
                let _ = self.root.insert_before(&el, Some(&e.el));
                e.backdrop = Some((el, new));
            }
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        for l in self.listeners.drain(..) {
            let _ = l.target.remove_event_listener_with_callback(l.kind, l.closure.as_ref().unchecked_ref());
        }
        // The elements and stylesheets stay: the page owns the layer element.
    }
}

fn set_flag(el: &Element, name: &str, on: bool) {
    if on {
        let _ = el.set_attribute(name, "");
    } else {
        let _ = el.remove_attribute(name);
    }
}

fn apply_decls(el: &HtmlElement, old: &[Decl], new: &[Decl]) {
    let (set, remove) = dom_plan::style_diff(old, new);
    let style = el.style();
    for (k, v) in set {
        let _ = style.set_property(k, &v);
    }
    for k in remove {
        let _ = style.remove_property(k);
    }
}

fn draw_image(el: &HtmlElement, (w, h): (u32, u32), rgba: &[u8]) {
    let Ok(canvas) = el.clone().dyn_into::<HtmlCanvasElement>() else { return };
    canvas.set_width(w);
    canvas.set_height(h);
    let Some(ctx) = canvas.get_context("2d").ok().flatten().and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok()) else {
        return;
    };
    match ImageData::new_with_u8_clamped_array_and_sh(Clamped(rgba), w, h) {
        Ok(data) => {
            let _ = ctx.put_image_data(&data, 0.0, 0.0);
        }
        Err(_) => warn("could not build ImageData"),
    }
}

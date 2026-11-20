//! Bindings: read-only functions from game state to one node property (API.md section 8).

use std::sync::{Arc, Mutex};

use std::any::Any;

use crate::id::WidgetId;
use crate::node::ImageId;
use crate::signal::UiValue;
use crate::tree::UiTree;

/// What a binding function may read: a list of host-provided values, looked up by type. The
/// adapter fills it (the engine adapter provides its resources, world and time) and offers
/// typed accessors through an extension trait.
#[derive(Clone, Copy)]
pub struct BindCx<'a> {
    items: &'a [&'a dyn Any],
}

impl<'a> BindCx<'a> {
    pub fn new(items: &'a [&'a dyn Any]) -> Self {
        Self { items }
    }

    /// The first provided value of type `T`.
    pub fn get<T: 'static>(&self) -> Option<&'a T> {
        self.items.iter().find_map(|i| i.downcast_ref::<T>())
    }
}

/// A read-only function from game state to a value of type `T`. `None` (resource or component
/// missing) leaves the property as it is. `Send + Sync`.
pub struct Bind<T> {
    f: Arc<dyn Fn(&BindCx) -> Option<T> + Send + Sync>,
}

impl<T> Clone for Bind<T> {
    fn clone(&self) -> Self {
        Self { f: self.f.clone() }
    }
}

impl<T: 'static> Bind<T> {
    /// From anything the [`BindCx`] reads.
    pub fn with(f: impl Fn(&BindCx) -> Option<T> + Send + Sync + 'static) -> Self {
        Self { f: Arc::new(f) }
    }

    /// Evaluates the binding.
    pub fn eval(&self, cx: &BindCx) -> Option<T> {
        (self.f)(cx)
    }

    #[doc(hidden)]
    pub fn erase(self, wrap: fn(T) -> BoundValue) -> Erased {
        Arc::new(move |cx| (self.f)(cx).map(wrap))
    }
}

pub type Erased = Arc<dyn Fn(&BindCx) -> Option<BoundValue> + Send + Sync>;

/// A text given at node creation: a fixed string or a binding.
pub enum TextSrc {
    Static(String),
    Bound(Bind<String>),
}

impl TextSrc {
    #[doc(hidden)]
    pub fn split(self) -> (String, Option<Bind<String>>) {
        match self {
            TextSrc::Static(s) => (s, None),
            TextSrc::Bound(b) => (String::new(), Some(b)),
        }
    }
}

impl From<&str> for TextSrc {
    fn from(s: &str) -> Self {
        TextSrc::Static(s.to_string())
    }
}

impl From<String> for TextSrc {
    fn from(s: String) -> Self {
        TextSrc::Static(s)
    }
}

impl From<Bind<String>> for TextSrc {
    fn from(b: Bind<String>) -> Self {
        TextSrc::Bound(b)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BindTarget {
    Text,
    Value,
    Checked,
    Visible,
    Enabled,
    Class,
    Image,
}

/// A value a binding produced.
#[derive(Clone, PartialEq, Debug)]
pub enum BoundValue {
    Text(String),
    F32(f32),
    Bool(bool),
    Image(ImageId),
}

/// A changed binding result, to apply to the tree.
#[derive(Clone, PartialEq, Debug)]
pub struct BindUpdate {
    pub widget: WidgetId,
    pub target: BindTarget,
    pub value: BoundValue,
}

impl BindUpdate {
    /// Writes the value into the tree (a normal change: it reaches the journal only if the
    /// node actually changes). `false` if the id is stale or the value does not fit.
    pub fn apply(&self, tree: &mut UiTree) -> bool {
        let id = self.widget;
        match (self.target, &self.value) {
            (BindTarget::Text, BoundValue::Text(t)) => tree.set_text(id, t.clone()),
            (BindTarget::Value, BoundValue::F32(v)) => tree.set_value(id, UiValue::F32(*v)),
            (BindTarget::Checked, BoundValue::Bool(b)) => tree.set_value(id, UiValue::Bool(*b)),
            (BindTarget::Visible, BoundValue::Bool(b)) => tree.set_visible(id, *b),
            (BindTarget::Enabled, BoundValue::Bool(b)) => tree.set_enabled(id, *b),
            (BindTarget::Class, BoundValue::Text(c)) => tree.set_class(id, c),
            (BindTarget::Image, BoundValue::Image(t)) => tree.set_image(id, *t),
            _ => false,
        }
    }
}

struct Entry {
    id: WidgetId,
    target: BindTarget,
    eval: Erased,
    last: Mutex<Option<BoundValue>>,
}

/// All bindings, keyed by widget. Kept next to the [`UiTree`]; the adapter evaluates it once
/// per frame.
#[derive(Default)]
pub struct UiBindings {
    entries: Vec<Entry>,
}

impl UiBindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `true` if `id` has any binding.
    pub fn is_bound(&self, id: WidgetId) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// Drops all bindings of a widget.
    pub fn remove(&mut self, id: WidgetId) {
        self.entries.retain(|e| e.id != id);
    }

    #[doc(hidden)]
    pub fn add(&mut self, id: WidgetId, target: BindTarget, eval: Erased) {
        // A second binding of the same property replaces the first.
        self.entries.retain(|e| !(e.id == id && e.target == target));
        self.entries.push(Entry { id, target, eval, last: Mutex::new(None) });
    }

    /// Takes the bindings registered on the tree since the last call and forgets the bindings
    /// of removed widgets.
    pub fn absorb(&mut self, tree: &mut UiTree) {
        for id in tree.take_removed() {
            self.remove(id);
        }
        for p in std::mem::take(&mut tree.pending_binds) {
            if tree.contains(p.id) {
                self.add(p.id, p.target, p.eval);
            }
        }
    }

    /// Evaluates every binding and returns the ones whose value changed since the last
    /// evaluation (the first evaluation always reports).
    ///
    /// Takes `&self` (the last values sit behind a mutex per entry), so an adapter can evaluate
    /// through a shared borrow.
    pub fn evaluate(&self, cx: &BindCx) -> Vec<BindUpdate> {
        let mut out = Vec::new();
        for e in &self.entries {
            let Some(v) = (e.eval)(cx) else { continue };
            let mut last = e.last.lock().unwrap_or_else(|p| p.into_inner());
            if last.as_ref() != Some(&v) {
                *last = Some(v.clone());
                out.push(BindUpdate { widget: e.id, target: e.target, value: v });
            }
        }
        out
    }
}

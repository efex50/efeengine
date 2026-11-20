//! The retained tree (API.md sections 2 to 4): nodes, layers, keys, owners, journal.

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut, RangeInclusive};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::bindings::{Bind, BindTarget, BoundValue, Erased, TextSrc};
use crate::id::{Owner, WidgetId};
use crate::journal::UiChange;
use crate::node::{ClassId, ImageFit, ImageId, Layer, Node, NodeKind, clamp_slider};
use crate::signal::UiValue;
use crate::style::*;
use crate::theme::{Theme, WidgetState};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TreeError {
    #[error("stale or unknown widget id")]
    Stale,
    #[error("key `{0}` is already used by another node")]
    KeyTaken(String),
    #[error("a node cannot be moved into itself or its own subtree")]
    Cycle,
    #[error("a layer root cannot be moved")]
    LayerRoot,
    #[error("the new kind has a different node kind than the node")]
    KindMismatch,
}

/// A font loaded through [`UiTree::load_font`].
#[derive(Clone, Debug)]
pub struct FontData {
    pub name: Arc<str>,
    pub bytes: Arc<[u8]>,
}

struct Slot {
    generation: u32,
    node: Option<Node>,
}

#[doc(hidden)]
pub struct PendingBind {
    pub id: WidgetId,
    pub target: BindTarget,
    pub eval: Erased,
}

/// The UI tree resource: nodes, layers, keys, theme, fonts and the change journal.
pub struct UiTree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    len: usize,
    next_seq: u64,
    keys: HashMap<Arc<str>, WidgetId>,
    class_ids: HashMap<Arc<str>, ClassId>,
    class_names: Vec<Arc<str>>,
    theme: Theme,
    fonts: Vec<FontData>,
    owner: Owner,
    journal: Vec<UiChange>,
    /// Nodes created since the last `take_changes`.
    fresh: HashSet<WidgetId>,
    removed_log: Vec<WidgetId>,
    #[doc(hidden)]
    pub pending_binds: Vec<PendingBind>,
}

impl Default for UiTree {
    fn default() -> Self {
        Self::new()
    }
}

impl UiTree {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
            next_seq: 0,
            keys: HashMap::new(),
            class_ids: HashMap::new(),
            class_names: Vec::new(),
            theme: Theme::dark(),
            fonts: Vec::new(),
            owner: Owner::Game,
            journal: Vec::new(),
            fresh: HashSet::new(),
            removed_log: Vec::new(),
            pending_binds: Vec::new(),
        }
    }

    // ----- reads -----------------------------------------------------------------------------

    /// The node of `id`, or `None` if the id is stale.
    pub fn get(&self, id: WidgetId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation { slot.node.as_ref() } else { None }
    }

    fn get_mut(&mut self, id: WidgetId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation { slot.node.as_mut() } else { None }
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        self.get(id).is_some()
    }

    /// Number of live nodes (layer roots included).
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Every live node with its id, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (WidgetId, &Node)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.node.as_ref().map(|n| (WidgetId { index: i as u32, generation: s.generation }, n))
        })
    }

    pub fn parent(&self, id: WidgetId) -> Option<WidgetId> {
        self.get(id)?.parent
    }

    /// The children in order (empty for a stale id).
    pub fn children(&self, id: WidgetId) -> &[WidgetId] {
        self.get(id).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    pub fn kind(&self, id: WidgetId) -> Option<&NodeKind> {
        self.get(id).map(|n| &n.kind)
    }

    /// The text a node shows: `Text` text, `Button` / `Checkbox` label, `TextInput` value.
    pub fn text_of(&self, id: WidgetId) -> Option<&str> {
        match &self.get(id)?.kind {
            NodeKind::Text { text } => Some(text),
            NodeKind::Button { label } | NodeKind::Checkbox { label, .. } => Some(label),
            NodeKind::TextInput { value, .. } => Some(value),
            _ => None,
        }
    }

    /// The value of a `Checkbox` (`Bool`), `Slider` (`F32`) or `TextInput` (`Text`).
    pub fn value(&self, id: WidgetId) -> Option<UiValue> {
        match &self.get(id)?.kind {
            NodeKind::Checkbox { checked, .. } => Some(UiValue::Bool(*checked)),
            NodeKind::Slider { value, .. } => Some(UiValue::F32(*value)),
            NodeKind::TextInput { value, .. } => Some(UiValue::Text(value.clone())),
            _ => None,
        }
    }

    pub fn visible(&self, id: WidgetId) -> Option<bool> {
        self.get(id).map(|n| n.visible)
    }

    pub fn enabled(&self, id: WidgetId) -> Option<bool> {
        self.get(id).map(|n| n.enabled)
    }

    /// `true` if the node and all its ancestors are visible.
    pub fn is_shown(&self, id: WidgetId) -> bool {
        self.chain_all(id, |n| n.visible)
    }

    /// `true` if the node and all its ancestors are enabled.
    pub fn is_enabled(&self, id: WidgetId) -> bool {
        self.chain_all(id, |n| n.enabled)
    }

    fn chain_all(&self, id: WidgetId, f: impl Fn(&Node) -> bool) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            let Some(n) = self.get(c) else { return false };
            if !f(n) {
                return false;
            }
            cur = n.parent;
        }
        true
    }

    pub fn owner_of(&self, id: WidgetId) -> Option<&Owner> {
        self.get(id).map(|n| &n.owner)
    }

    /// The root layer node a node lives under (itself for a layer root).
    pub fn layer_of(&self, id: WidgetId) -> Option<WidgetId> {
        let mut cur = id;
        loop {
            let n = self.get(cur)?;
            match n.parent {
                Some(p) => cur = p,
                None => return Some(cur),
            }
        }
    }

    /// The layer roots, lowest z first (creation order for equal z).
    pub fn layers(&self) -> Vec<WidgetId> {
        let mut v: Vec<(i32, u64, WidgetId)> = self.iter().filter_map(|(id, n)| n.layer.map(|l| (l.z, n.seq, id))).collect();
        v.sort_by_key(|(z, seq, _)| (*z, *seq));
        v.into_iter().map(|(_, _, id)| id).collect()
    }

    /// The layer roots shown in `window`, lowest z first.
    pub fn layers_in(&self, window: u32) -> Vec<WidgetId> {
        self.layers().into_iter().filter(|id| self.layer_window(*id) == Some(window)).collect()
    }

    /// The window a layer root is shown in (`None` if `id` is not a layer root).
    pub fn layer_window(&self, id: WidgetId) -> Option<u32> {
        self.get(id)?.layer.map(|l| l.window)
    }

    /// The window the node `id` is shown in: that of the layer it lives on.
    pub fn window_of(&self, id: WidgetId) -> Option<u32> {
        self.layer_window(self.layer_of(id)?)
    }

    /// Every window that has a layer, ascending.
    pub fn layer_windows(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.iter().filter_map(|(_, n)| n.layer.map(|l| l.window)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The journal entries that would build the tree from nothing: what a backend that joins
    /// late (a new window's) is synced with. Parents come before their children.
    pub fn snapshot_changes(&self) -> Vec<UiChange> {
        let mut out = vec![UiChange::Theme];
        out.extend((0..self.fonts().len()).map(|i| UiChange::Font(crate::style::FontId(i as u32))));
        for root in self.layers() {
            out.extend(self.subtree(root).into_iter().map(UiChange::Created));
        }
        out
    }

    /// `true` if a visible modal layer exists (in any window).
    pub fn modal_active(&self) -> bool {
        self.iter().any(|(id, n)| n.layer.is_some_and(|l| l.modal) && n.visible && self.is_shown(id))
    }

    /// `true` if a visible modal layer sits above the layer `id` lives on (higher `z`, same window).
    pub fn is_below_modal(&self, id: WidgetId) -> bool {
        let Some(own) = self.layer_of(id).and_then(|l| self.get(l)).and_then(|n| n.layer) else { return false };
        self.iter().any(|(m, n)| n.layer.is_some_and(|l| l.modal && l.z > own.z && l.window == own.window) && self.is_shown(m))
    }

    /// All nodes under `id`, depth first, `id` itself first (empty for a stale id).
    pub fn subtree(&self, id: WidgetId) -> Vec<WidgetId> {
        let mut out = Vec::new();
        if !self.contains(id) {
            return out;
        }
        let mut stack = vec![id];
        while let Some(c) = stack.pop() {
            out.push(c);
            if let Some(n) = self.get(c) {
                stack.extend(n.children.iter().rev().copied());
            }
        }
        out
    }

    /// Ids of all nodes owned by `owner`.
    pub fn nodes_of(&self, owner: &Owner) -> Vec<WidgetId> {
        self.iter().filter(|(_, n)| &n.owner == owner).map(|(id, _)| id).collect()
    }

    // ----- keys ------------------------------------------------------------------------------

    /// The node with string key `key`.
    pub fn find(&self, key: &str) -> Option<WidgetId> {
        let id = self.keys.get(key).copied()?;
        // Invariant: `remove` drops the key of every node it frees, so the table never holds a
        // dead id. Checked anyway: a stale entry must never be handed out.
        debug_assert!(self.contains(id), "key table holds a dead id for `{key}`");
        self.contains(id).then_some(id)
    }

    pub fn key_of(&self, id: WidgetId) -> Option<&str> {
        self.get(id)?.key.as_deref()
    }

    /// Gives a node a key, unique in the tree (replaces its old key). Setting the key a node
    /// already has is fine.
    pub fn set_key(&mut self, id: WidgetId, key: &str) -> Result<(), TreeError> {
        if !self.contains(id) {
            return Err(TreeError::Stale);
        }
        if let Some(&other) = self.keys.get(key) {
            return if other == id { Ok(()) } else { Err(TreeError::KeyTaken(key.to_string())) };
        }
        let key: Arc<str> = key.into();
        let node = self.get_mut(id).expect("checked");
        if let Some(old) = node.key.replace(key.clone()) {
            self.keys.remove(&old);
        }
        self.keys.insert(key, id);
        Ok(())
    }

    pub fn clear_key(&mut self, id: WidgetId) {
        if let Some(old) = self.get_mut(id).and_then(|n| n.key.take()) {
            self.keys.remove(&old);
        }
    }

    // ----- owners ----------------------------------------------------------------------------

    /// The owner given to nodes created from now on.
    pub fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Sets the owner of nodes created from now on; returns the previous one.
    pub fn set_owner(&mut self, owner: Owner) -> Owner {
        std::mem::replace(&mut self.owner, owner)
    }

    /// Runs `f` with `owner` as the current owner.
    pub fn with_owner<R>(&mut self, owner: Owner, f: impl FnOnce(&mut UiTree) -> R) -> R {
        let prev = self.set_owner(owner);
        let r = f(self);
        self.owner = prev;
        r
    }

    /// Removes every node of `owner` (with their subtrees); returns how many nodes went.
    pub fn remove_owner(&mut self, owner: &Owner) -> usize {
        let before = self.len;
        for id in self.nodes_of(owner) {
            self.remove(id);
        }
        before - self.len
    }

    // ----- classes ---------------------------------------------------------------------------

    /// The id of class `name`, interning it on first use.
    pub fn class_id(&mut self, name: &str) -> ClassId {
        if let Some(&id) = self.class_ids.get(name) {
            return id;
        }
        let id = ClassId(self.class_names.len() as u32);
        let name: Arc<str> = name.into();
        self.class_names.push(name.clone());
        self.class_ids.insert(name, id);
        id
    }

    pub fn class_name(&self, id: ClassId) -> Option<&str> {
        self.class_names.get(id.0 as usize).map(|s| &**s)
    }

    /// The class names of a node, in order.
    pub fn class_names(&self, id: WidgetId) -> Vec<&str> {
        self.get(id).map(|n| n.classes.iter().filter_map(|c| self.class_name(*c)).collect()).unwrap_or_default()
    }

    pub fn has_class(&self, id: WidgetId, name: &str) -> bool {
        self.class_names(id).contains(&name)
    }

    // ----- journal ---------------------------------------------------------------------------

    fn push(&mut self, change: UiChange) {
        if let Some(id) = change.widget()
            && !matches!(change, UiChange::Created(_) | UiChange::Removed(_))
            && self.fresh.contains(&id)
        {
            return;
        }
        self.journal.push(change);
    }

    /// The changes since the last [`take_changes`](Self::take_changes).
    pub fn changes(&self) -> &[UiChange] {
        &self.journal
    }

    /// Takes the journal (the UI plugin does this once per frame before `sync`).
    ///
    /// `Moved` entries are coalesced here: one per node (the last one's place in the journal),
    /// rewritten to the node's final parent and index, so every `index` is valid against the
    /// final tree whatever moves, inserts and removals came after the entry was written (see
    /// [`UiChange`]).
    pub fn take_changes(&mut self) -> Vec<UiChange> {
        self.fresh.clear();
        let mut journal = std::mem::take(&mut self.journal);
        let mut last: HashMap<WidgetId, usize> = HashMap::new();
        for (i, c) in journal.iter().enumerate() {
            if let UiChange::Moved { id, .. } = c {
                last.insert(*id, i);
            }
        }
        if !last.is_empty() {
            let mut i = 0;
            journal.retain_mut(|c| {
                let keep = match c {
                    UiChange::Moved { id, parent, index } => {
                        if last.get(id) != Some(&i) {
                            false
                        } else if let Some(p) = self.get(*id).and_then(|n| n.parent) {
                            *parent = p;
                            *index = self.get(p).and_then(|pn| pn.children.iter().position(|c| c == id)).unwrap_or(0) as u32;
                            true
                        } else {
                            false
                        }
                    }
                    _ => true,
                };
                i += 1;
                keep
            });
        }
        journal
    }

    /// Whether the bindings system has anything to absorb (new registrations or removed ids).
    #[doc(hidden)]
    pub fn has_binding_work(&self) -> bool {
        !self.pending_binds.is_empty() || !self.removed_log.is_empty()
    }

    /// Ids removed since the last call (the bindings system drops their bindings).
    #[doc(hidden)]
    pub fn take_removed(&mut self) -> Vec<WidgetId> {
        std::mem::take(&mut self.removed_log)
    }

    // ----- creation --------------------------------------------------------------------------

    fn alloc(&mut self, mut node: Node) -> WidgetId {
        self.len += 1;
        node.seq = self.next_seq;
        self.next_seq += 1;
        if let Some(i) = self.free.pop() {
            let slot = &mut self.slots[i as usize];
            slot.node = Some(node);
            WidgetId { index: i, generation: slot.generation }
        } else {
            self.slots.push(Slot { generation: 0, node: Some(node) });
            WidgetId { index: self.slots.len() as u32 - 1, generation: 0 }
        }
    }

    /// Creates (or updates) the layer `name`: a root container with key `name`. An existing
    /// layer of that name keeps its id and children and gets the new `Layer` data.
    pub fn layer(&mut self, name: &str, layer: Layer) -> WidgetId {
        if let Some(id) = self.find(name) {
            if self.get(id).is_some_and(|n| n.layer.is_some()) {
                self.set_layer(id, layer);
                return id;
            }
            panic!("UiTree::layer: key `{name}` is used by a node that is not a layer");
        }
        let mut node = Node::new(NodeKind::Container, None, self.owner.clone());
        node.layer = Some(layer);
        let id = self.alloc(node);
        self.set_key(id, name).expect("key checked free");
        self.fresh.insert(id);
        self.journal.push(UiChange::Created(id));
        id
    }

    /// Creates (or updates) the layer `name` and shows it in `window` (`Layer::window`). The
    /// window may be opened later: the layer simply has no UI until it exists.
    pub fn layer_in(&mut self, window: impl Into<u32>, name: &str, layer: Layer) -> WidgetId {
        self.layer(name, layer.window(window))
    }

    /// Changes the `Layer` data of a layer root.
    pub fn set_layer(&mut self, id: WidgetId, layer: Layer) -> bool {
        match self.get_mut(id) {
            Some(n) if n.layer.is_some() => {
                if n.layer != Some(layer) {
                    n.layer = Some(layer);
                    self.push(UiChange::Layer(id));
                }
                true
            }
            _ => false,
        }
    }

    pub fn layer_data(&self, id: WidgetId) -> Option<Layer> {
        self.get(id)?.layer
    }

    /// Creates a child of `parent` (appended). One `Created` journal entry.
    pub fn try_create(&mut self, parent: WidgetId, kind: NodeKind) -> Result<WidgetId, TreeError> {
        if !self.contains(parent) {
            return Err(TreeError::Stale);
        }
        let id = self.alloc(Node::new(kind, Some(parent), self.owner.clone()));
        self.get_mut(parent).expect("checked").children.push(id);
        self.fresh.insert(id);
        self.journal.push(UiChange::Created(id));
        Ok(id)
    }

    /// Like [`try_create`](Self::try_create), returning a builder handle. Panics if `parent` is
    /// stale (a bug in the calling code).
    pub fn create(&mut self, parent: WidgetId, kind: NodeKind) -> NodeRef<'_> {
        let id = self.try_create(parent, kind).expect("UiTree::create: stale parent id");
        NodeRef { tree: self, id }
    }

    /// A handle for an existing node, to use the chained setters.
    pub fn node(&mut self, id: WidgetId) -> Option<NodeRef<'_>> {
        self.contains(id).then_some(NodeRef { tree: self, id })
    }

    pub fn container(&mut self, parent: WidgetId) -> NodeRef<'_> {
        self.create(parent, NodeKind::Container)
    }

    pub fn row(&mut self, parent: WidgetId) -> NodeRef<'_> {
        self.create(parent, NodeKind::Container).direction(Direction::Row)
    }

    pub fn column(&mut self, parent: WidgetId) -> NodeRef<'_> {
        self.create(parent, NodeKind::Container).direction(Direction::Column)
    }

    pub fn scroll(&mut self, parent: WidgetId) -> NodeRef<'_> {
        self.create(parent, NodeKind::Scroll)
    }

    /// A text node; `text` is a `&str`, `String` or a [`Bind<String>`].
    pub fn text(&mut self, parent: WidgetId, text: impl Into<TextSrc>) -> NodeRef<'_> {
        let (initial, bind) = text.into().split();
        let id = self.try_create(parent, NodeKind::Text { text: initial }).expect("UiTree::text: stale parent id");
        if let Some(b) = bind {
            self.bind_text(id, b);
        }
        NodeRef { tree: self, id }
    }

    /// A GPU view drawn by the painter registered under `painter` (API.md section 17).
    pub fn gpu_view(&mut self, parent: WidgetId, painter: &str) -> NodeRef<'_> {
        self.create(parent, NodeKind::gpu_view(painter))
    }

    pub fn image(&mut self, parent: WidgetId, texture: ImageId, fit: ImageFit) -> NodeRef<'_> {
        self.create(parent, NodeKind::Image { texture, fit })
    }

    pub fn button(&mut self, parent: WidgetId, label: impl Into<TextSrc>) -> NodeRef<'_> {
        let (initial, bind) = label.into().split();
        let id = self.try_create(parent, NodeKind::Button { label: initial }).expect("UiTree::button: stale parent id");
        if let Some(b) = bind {
            self.bind_text(id, b);
        }
        NodeRef { tree: self, id }
    }

    pub fn checkbox(&mut self, parent: WidgetId, label: impl Into<String>, checked: bool) -> NodeRef<'_> {
        self.create(parent, NodeKind::Checkbox { label: label.into(), checked })
    }

    pub fn slider(&mut self, parent: WidgetId, range: RangeInclusive<f32>, value: f32) -> NodeRef<'_> {
        self.create(parent, NodeKind::slider(*range.start(), *range.end(), value))
    }

    pub fn text_input(&mut self, parent: WidgetId, value: impl Into<String>) -> NodeRef<'_> {
        self.create(parent, NodeKind::text_input(value))
    }

    // ----- removal and moves -----------------------------------------------------------------

    /// Removes a node and its subtree. Returns `false` for a stale id.
    pub fn remove(&mut self, id: WidgetId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let mut order = self.subtree(id);
        order.reverse(); // children before parents
        let gone: HashSet<WidgetId> = order.iter().copied().collect();
        // Detach from the parent.
        if let Some(p) = self.get(id).and_then(|n| n.parent)
            && let Some(pn) = self.get_mut(p)
        {
            pn.children.retain(|c| *c != id);
        }
        // Drop journal entries about removed nodes (or moved under them).
        let created_now: HashSet<WidgetId> = gone.iter().copied().filter(|i| self.fresh.contains(i)).collect();
        self.journal.retain(|c| {
            let about = c.widget().is_some_and(|w| gone.contains(&w));
            let under = matches!(c, UiChange::Moved { parent, .. } if gone.contains(parent));
            !(about || under)
        });
        for &n in &order {
            let slot = &mut self.slots[n.index as usize];
            let node = slot.node.take().expect("subtree node alive");
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(n.index);
            self.len -= 1;
            if let Some(k) = node.key {
                self.keys.remove(&k);
            }
            self.fresh.remove(&n);
            self.removed_log.push(n);
            if !created_now.contains(&n) {
                self.journal.push(UiChange::Removed(n));
            }
        }
        true
    }

    /// Moves `id` under `parent` at `index` (clamped to the end). Fails for stale ids, layer
    /// roots and moves into the node's own subtree. No entry if nothing changes.
    pub fn move_to(&mut self, id: WidgetId, parent: WidgetId, index: usize) -> Result<(), TreeError> {
        let node = self.get(id).ok_or(TreeError::Stale)?;
        if !self.contains(parent) {
            return Err(TreeError::Stale);
        }
        if node.layer.is_some() {
            return Err(TreeError::LayerRoot);
        }
        // `parent` must not be inside id's subtree.
        let mut cur = Some(parent);
        while let Some(c) = cur {
            if c == id {
                return Err(TreeError::Cycle);
            }
            cur = self.get(c).and_then(|n| n.parent);
        }
        let old_parent = node.parent.expect("non-layer node has a parent");
        let old_index = self.get(old_parent).and_then(|p| p.children.iter().position(|c| *c == id)).expect("child listed");
        self.get_mut(old_parent).expect("alive").children.remove(old_index);
        let new_children = &mut self.get_mut(parent).expect("alive").children;
        let index = index.min(new_children.len());
        new_children.insert(index, id);
        self.get_mut(id).expect("alive").parent = Some(parent);
        if old_parent != parent || old_index != index {
            self.push(UiChange::Moved { id, parent, index: index as u32 });
        }
        Ok(())
    }

    // ----- setters (one journal entry each, none if nothing changes) -------------------------

    /// Sets the text of a `Text`, the label of a `Button` / `Checkbox` or the value of a
    /// `TextInput`. Returns `false` if the node has no text.
    pub fn set_text(&mut self, id: WidgetId, text: impl Into<String>) -> bool {
        let text = text.into();
        let Some(node) = self.get_mut(id) else { return false };
        let slot = match &mut node.kind {
            NodeKind::Text { text } => text,
            NodeKind::Button { label } | NodeKind::Checkbox { label, .. } => label,
            NodeKind::TextInput { .. } => return self.set_value(id, UiValue::Text(text)),
            _ => return false,
        };
        if *slot != text {
            *slot = text;
            self.push(UiChange::Kind(id));
        }
        true
    }

    /// Sets the value of a `Checkbox` (`Bool`), `Slider` (`F32`, clamped) or `TextInput`
    /// (`Text`, cut to `max_len`). Returns `false` if the type does not fit the node.
    pub fn set_value(&mut self, id: WidgetId, value: impl Into<UiValue>) -> bool {
        let Some(node) = self.get_mut(id) else { return false };
        match sanitize(&node.kind, value.into()) {
            Some(v) => {
                if apply_value(&mut node.kind, v) {
                    self.push(UiChange::Kind(id));
                }
                true
            }
            None => false,
        }
    }

    /// Writes a value reported by a backend into the node WITHOUT a journal entry (the backend
    /// already shows it). Returns the sanitized value, or `None` if it does not fit the node.
    /// Used by the UI plugin before game systems run.
    pub fn write_back(&mut self, id: WidgetId, value: UiValue) -> Option<UiValue> {
        let node = self.get_mut(id)?;
        let v = sanitize(&node.kind, value)?;
        apply_value(&mut node.kind, v.clone());
        Some(v)
    }

    /// Replaces the kind data. The node kind (`KindTag`) must stay the same (a different kind
    /// means remove and create).
    pub fn set_kind(&mut self, id: WidgetId, mut kind: NodeKind) -> Result<(), TreeError> {
        kind.normalize();
        let node = self.get_mut(id).ok_or(TreeError::Stale)?;
        if node.kind.tag() != kind.tag() {
            return Err(TreeError::KindMismatch);
        }
        if node.kind != kind {
            node.kind = kind;
            self.push(UiChange::Kind(id));
        }
        Ok(())
    }

    /// Changes the texture of an `Image` (fit unchanged).
    pub fn set_image(&mut self, id: WidgetId, texture: ImageId) -> bool {
        let Some(node) = self.get_mut(id) else { return false };
        let NodeKind::Image { texture: t, .. } = &mut node.kind else { return false };
        if *t != texture {
            *t = texture;
            self.push(UiChange::Kind(id));
        }
        true
    }

    /// Edits the kind data in place (placeholder, max_len, step, ...); one `Kind` entry if it
    /// changed. The closure must not change the kind itself (it is checked: a different tag
    /// restores the old kind and returns `false`).
    pub fn edit_kind(&mut self, id: WidgetId, f: impl FnOnce(&mut NodeKind)) -> bool {
        let Some(node) = self.get_mut(id) else { return false };
        let old = node.kind.clone();
        f(&mut node.kind);
        if node.kind.tag() != old.tag() {
            node.kind = old;
            return false;
        }
        node.kind.normalize();
        if node.kind != old {
            self.push(UiChange::Kind(id));
        }
        true
    }

    pub fn set_visible(&mut self, id: WidgetId, visible: bool) -> bool {
        let Some(n) = self.get_mut(id) else { return false };
        if n.visible != visible {
            n.visible = visible;
            self.push(UiChange::Visible(id));
        }
        true
    }

    pub fn set_enabled(&mut self, id: WidgetId, enabled: bool) -> bool {
        let Some(n) = self.get_mut(id) else { return false };
        if n.enabled != enabled {
            n.enabled = enabled;
            self.push(UiChange::Enabled(id));
        }
        true
    }

    /// Opts the node in or out of `Hover` signals. One `HoverSignals` entry if the flag changed
    /// (a node created this frame is covered by `Created`).
    pub fn set_hover_signals(&mut self, id: WidgetId, on: bool) -> bool {
        let Some(n) = self.get_mut(id) else { return false };
        if n.hover_signals != on {
            n.hover_signals = on;
            self.push(UiChange::HoverSignals(id));
        }
        true
    }

    /// Replaces the classes with one class (an empty name clears them).
    pub fn set_class(&mut self, id: WidgetId, name: &str) -> bool {
        if name.is_empty() { self.set_classes(id, &[]) } else { self.set_classes(id, &[name]) }
    }

    /// Replaces the classes, in order.
    pub fn set_classes(&mut self, id: WidgetId, names: &[&str]) -> bool {
        if !self.contains(id) {
            return false;
        }
        let ids: SmallVec<[ClassId; 2]> = names.iter().map(|n| self.class_id(n)).collect();
        let n = self.get_mut(id).expect("checked");
        if n.classes != ids {
            n.classes = ids;
            self.push(UiChange::Classes(id));
        }
        true
    }

    pub fn add_class(&mut self, id: WidgetId, name: &str) -> bool {
        let mut names: Vec<String> = self.class_names(id).into_iter().map(String::from).collect();
        if !self.contains(id) {
            return false;
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.set_classes(id, &refs)
    }

    pub fn remove_class(&mut self, id: WidgetId, name: &str) -> bool {
        let names: Vec<String> = self.class_names(id).into_iter().filter(|n| *n != name).map(String::from).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.set_classes(id, &refs)
    }

    /// Replaces the inline style.
    pub fn set_style(&mut self, id: WidgetId, style: Style) -> bool {
        self.edit_style(id, |s| *s = style)
    }

    /// Edits the inline style; one `Style` entry if it changed.
    pub fn edit_style(&mut self, id: WidgetId, f: impl FnOnce(&mut Style)) -> bool {
        let Some(n) = self.get_mut(id) else { return false };
        let old = n.style;
        f(&mut n.style);
        if n.style != old {
            self.push(UiChange::Style(id));
        }
        true
    }

    /// A guard that derefs to the node's inline `Style` and writes one `Style` entry when it
    /// drops, if the style changed. `None` for a stale id.
    pub fn style_mut(&mut self, id: WidgetId) -> Option<StyleMut<'_>> {
        let old = self.get(id)?.style;
        Some(StyleMut { tree: self, id, old })
    }

    // ----- theme and fonts -------------------------------------------------------------------

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Replaces the theme; one `Theme` journal entry (backends restyle everything).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.journal.push(UiChange::Theme);
    }

    /// The style of `id` for `state`: theme, classes, inline style and state styles (see
    /// [`Theme::resolve`]). A node that is disabled (own flag or an ancestor's) gets the
    /// `disabled` state on top of `state`.
    pub fn resolve(&self, id: WidgetId, mut state: WidgetState) -> Option<Style> {
        let node = self.get(id)?;
        state.disabled |= !self.is_enabled(id);
        Some(self.theme.resolve(node.kind.tag(), node.classes.iter().filter_map(|c| self.class_name(*c)), &node.style, state))
    }

    /// Registers a font; one `Font` journal entry. Loading a font under a name that exists
    /// returns a new id (the old one stays valid).
    pub fn load_font(&mut self, name: &str, bytes: impl Into<Arc<[u8]>>) -> FontId {
        let id = FontId(self.fonts.len() as u32);
        self.fonts.push(FontData { name: name.into(), bytes: bytes.into() });
        self.journal.push(UiChange::Font(id));
        id
    }

    pub fn font(&self, id: FontId) -> Option<&FontData> {
        self.fonts.get(id.0 as usize)
    }

    /// The newest font loaded under `name`.
    pub fn font_by_name(&self, name: &str) -> Option<FontId> {
        self.fonts.iter().rposition(|f| &*f.name == name).map(|i| FontId(i as u32))
    }

    pub fn fonts(&self) -> &[FontData] {
        &self.fonts
    }

    // ----- bindings (stored in `UiBindings`, see bindings.rs) ---------------------------------

    fn bind(&mut self, id: WidgetId, target: BindTarget, eval: Erased) {
        if self.contains(id) {
            self.pending_binds.push(PendingBind { id, target, eval });
        }
    }

    /// Binds the text / label of a node to game state.
    pub fn bind_text(&mut self, id: WidgetId, b: Bind<String>) {
        self.bind(id, BindTarget::Text, b.erase(BoundValue::Text));
    }

    /// Binds a slider value.
    pub fn bind_value(&mut self, id: WidgetId, b: Bind<f32>) {
        self.bind(id, BindTarget::Value, b.erase(BoundValue::F32));
    }

    /// Binds a checkbox's checked state.
    pub fn bind_checked(&mut self, id: WidgetId, b: Bind<bool>) {
        self.bind(id, BindTarget::Checked, b.erase(BoundValue::Bool));
    }

    pub fn bind_visible(&mut self, id: WidgetId, b: Bind<bool>) {
        self.bind(id, BindTarget::Visible, b.erase(BoundValue::Bool));
    }

    pub fn bind_enabled(&mut self, id: WidgetId, b: Bind<bool>) {
        self.bind(id, BindTarget::Enabled, b.erase(BoundValue::Bool));
    }

    /// Binds the (single) class of a node.
    pub fn bind_class(&mut self, id: WidgetId, b: Bind<String>) {
        self.bind(id, BindTarget::Class, b.erase(BoundValue::Text));
    }

    pub fn bind_image(&mut self, id: WidgetId, b: Bind<ImageId>) {
        self.bind(id, BindTarget::Image, b.erase(BoundValue::Image));
    }
}

/// Checks `v` fits `kind` and normalizes it: slider clamped (NaN refused), text cut to `max_len`.
fn sanitize(kind: &NodeKind, v: UiValue) -> Option<UiValue> {
    match (kind, v) {
        (NodeKind::Checkbox { .. }, UiValue::Bool(b)) => Some(UiValue::Bool(b)),
        (NodeKind::Slider { min, max, .. }, UiValue::F32(x)) if !x.is_nan() => Some(UiValue::F32(clamp_slider(*min, *max, x))),
        (NodeKind::TextInput { max_len, .. }, UiValue::Text(t)) => {
            let t = if *max_len > 0 && t.chars().count() > *max_len as usize { t.chars().take(*max_len as usize).collect() } else { t };
            Some(UiValue::Text(t))
        }
        _ => None,
    }
}

/// Stores a sanitized value; `true` if it differed.
fn apply_value(kind: &mut NodeKind, v: UiValue) -> bool {
    match (kind, v) {
        (NodeKind::Checkbox { checked, .. }, UiValue::Bool(b)) => std::mem::replace(checked, b) != b,
        (NodeKind::Slider { value, .. }, UiValue::F32(x)) => std::mem::replace(value, x) != x,
        (NodeKind::TextInput { value, .. }, UiValue::Text(t)) => {
            if *value != t {
                *value = t;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Guard from [`UiTree::style_mut`].
pub struct StyleMut<'a> {
    tree: &'a mut UiTree,
    id: WidgetId,
    old: Style,
}

impl Deref for StyleMut<'_> {
    type Target = Style;
    fn deref(&self) -> &Style {
        &self.tree.get(self.id).expect("alive").style
    }
}

impl DerefMut for StyleMut<'_> {
    fn deref_mut(&mut self) -> &mut Style {
        &mut self.tree.get_mut(self.id).expect("alive").style
    }
}

impl Drop for StyleMut<'_> {
    fn drop(&mut self) {
        if self.tree.get(self.id).is_some_and(|n| n.style != self.old) {
            self.tree.push(UiChange::Style(self.id));
        }
    }
}

/// A node being built (or edited): chained setters, then `.id()`.
pub struct NodeRef<'a> {
    tree: &'a mut UiTree,
    id: WidgetId,
}

macro_rules! def_noderef_style {
    ($($f:ident: $t:ty = $a:ty;)*) => {
        impl NodeRef<'_> {
            $(pub fn $f(self, v: $a) -> Self {
                self.tree.edit_style(self.id, |s| s.$f = Some(v.into()));
                self
            })*
        }
    };
}
style_fields!(def_noderef_style);

impl NodeRef<'_> {
    pub fn id(&self) -> WidgetId {
        self.id
    }

    /// Adds a class. Panics are never used here: an unknown class just has no style.
    pub fn class(self, name: &str) -> Self {
        self.tree.add_class(self.id, name);
        self
    }

    pub fn classes(self, names: &[&str]) -> Self {
        self.tree.set_classes(self.id, names);
        self
    }

    /// Sets the key. Panics if another node has it (use [`UiTree::set_key`] to handle that).
    pub fn key(self, key: &str) -> Self {
        if let Err(e) = self.tree.set_key(self.id, key) {
            panic!("NodeRef::key(`{key}`): {e}");
        }
        self
    }

    pub fn visible(self, v: bool) -> Self {
        self.tree.set_visible(self.id, v);
        self
    }

    pub fn enabled(self, v: bool) -> Self {
        self.tree.set_enabled(self.id, v);
        self
    }

    /// A `GpuView` sends `Pointer` signals (no effect on other kinds).
    pub fn pointer_signals(self) -> Self {
        self.tree.edit_kind(self.id, |k| {
            if let NodeKind::GpuView { pointer, .. } = k {
                *pointer = true;
            }
        });
        self
    }

    /// Sends `Hover` signals for this node.
    pub fn hover_signals(self) -> Self {
        self.tree.set_hover_signals(self.id, true);
        self
    }

    /// Sets the inline style as a whole.
    pub fn style(self, style: Style) -> Self {
        self.tree.set_style(self.id, style);
        self
    }

    pub fn placeholder(self, p: &str) -> Self {
        self.tree.edit_kind(self.id, |k| {
            if let NodeKind::TextInput { placeholder, .. } = k {
                *placeholder = p.to_string();
            }
        });
        self
    }

    pub fn max_len(self, n: u32) -> Self {
        self.tree.edit_kind(self.id, |k| {
            if let NodeKind::TextInput { max_len, .. } = k {
                *max_len = n;
            }
        });
        self
    }

    pub fn password(self, on: bool) -> Self {
        self.tree.edit_kind(self.id, |k| {
            if let NodeKind::TextInput { password, .. } = k {
                *password = on;
            }
        });
        self
    }

    /// Slider step (0 = continuous).
    pub fn step(self, s: f32) -> Self {
        self.tree.edit_kind(self.id, |k| {
            if let NodeKind::Slider { step, .. } = k {
                *step = s.max(0.0);
            }
        });
        self
    }

    /// Creates a child of this node (builders on a handle keep nesting short).
    pub fn child(&mut self, kind: NodeKind) -> NodeRef<'_> {
        self.tree.create(self.id, kind)
    }
}

//! The headless model (API.md section 14): no widgets; records the journal, checks it is
//! consistent and injects signals. Adapters wrap it in their `UiBackend`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::capture::UiCapture;
use crate::id::WidgetId;
use crate::journal::UiChange;
use crate::signal::UiSignal;
use crate::tree::UiTree;

#[derive(Default)]
struct State {
    /// Every change handed to `sync`, in order.
    recorded: Vec<UiChange>,
    /// The size of each `sync` batch (one per frame with changes).
    batches: Vec<usize>,
    errors: Vec<String>,
    inject: Vec<UiSignal>,
    capture: UiCapture,
    size: (f32, f32),
    scale: f32,
}

/// Test access to the headless model. Also a resource (`Send + Sync`), inserted by
/// `UiPlugin::new()`.
#[derive(Clone, Default)]
pub struct HeadlessHandle {
    state: Arc<Mutex<State>>,
}

impl HeadlessHandle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues a signal; the next frame's input stage delivers it (after the plugin's checks).
    pub fn inject(&self, signal: UiSignal) {
        self.lock().inject.push(signal);
    }

    /// Everything `sync` received so far (since the last [`clear`](Self::clear)).
    pub fn changes(&self) -> Vec<UiChange> {
        self.lock().recorded.clone()
    }

    /// The sizes of the `sync` batches (one entry per frame that had changes).
    pub fn batches(&self) -> Vec<usize> {
        self.lock().batches.clone()
    }

    /// Takes and clears the recorded changes.
    pub fn take_changes(&self) -> Vec<UiChange> {
        std::mem::take(&mut self.lock().recorded)
    }

    /// Consistency violations found by `sync` (empty = fine).
    pub fn errors(&self) -> Vec<String> {
        self.lock().errors.clone()
    }

    pub fn clear(&self) {
        let mut s = self.lock();
        s.recorded.clear();
        s.batches.clear();
        s.errors.clear();
    }

    /// What `capture()` reports from now on.
    pub fn set_capture(&self, capture: UiCapture) {
        self.lock().capture = capture;
    }

    /// The last size and scale given to `resize`.
    pub fn size(&self) -> ((f32, f32), f32) {
        let s = self.lock();
        (s.size, s.scale)
    }

    /// The pending (not yet delivered) injected signals.
    pub fn pending(&self) -> Vec<UiSignal> {
        self.lock().inject.clone()
    }
}

/// The model behind a headless backend. Checks per `sync`: a created id is new and alive with an existing parent; a
/// removed id is known and gone from the tree; every other change names a known, live id (a
/// moved node's parent must exist and hold it at `index`).
pub struct HeadlessModel {
    handle: HeadlessHandle,
    known: HashSet<WidgetId>,
}

impl HeadlessModel {
    /// A model and the handle to reach it.
    pub fn new() -> (Self, HeadlessHandle) {
        let handle = HeadlessHandle::default();
        (Self { handle: handle.clone(), known: HashSet::new() }, handle)
    }
}

impl HeadlessModel {
    /// Records the window size and scale.
    pub fn init(&mut self, logical_size: (f32, f32), scale_factor: f32) {
        let mut s = self.handle.lock();
        s.size = logical_size;
        s.scale = scale_factor;
    }

    /// Checks the batch against the tree and records it.
    pub fn sync(&mut self, tree: &UiTree, changes: &[UiChange]) {
        let mut errors = Vec::new();
        for c in changes {
            match *c {
                UiChange::Created(id) => {
                    if !self.known.insert(id) {
                        errors.push(format!("Created({id:?}) twice"));
                    }
                    match tree.get(id) {
                        None => errors.push(format!("Created({id:?}) but the node is gone")),
                        Some(n) => {
                            if let Some(p) = n.parent()
                                && !tree.contains(p)
                            {
                                errors.push(format!("Created({id:?}): parent {p:?} does not exist"));
                            }
                        }
                    }
                }
                UiChange::Removed(id) => {
                    if !self.known.remove(&id) {
                        errors.push(format!("Removed({id:?}) of an unknown id"));
                    }
                    if tree.contains(id) {
                        errors.push(format!("Removed({id:?}) but the node is alive"));
                    }
                }
                UiChange::Moved { id, parent, index } => {
                    self.check_live(tree, id, "Moved", &mut errors);
                    if !tree.contains(parent) {
                        errors.push(format!("Moved({id:?}): parent {parent:?} does not exist"));
                    } else if tree.children(parent).get(index as usize) != Some(&id) {
                        errors.push(format!("Moved({id:?}): index {index} is not the node's position in {parent:?}"));
                    }
                }
                UiChange::Kind(id) => self.check_live(tree, id, "Kind", &mut errors),
                UiChange::Style(id) => self.check_live(tree, id, "Style", &mut errors),
                UiChange::Classes(id) => self.check_live(tree, id, "Classes", &mut errors),
                UiChange::Visible(id) => self.check_live(tree, id, "Visible", &mut errors),
                UiChange::Enabled(id) => self.check_live(tree, id, "Enabled", &mut errors),
                UiChange::HoverSignals(id) => self.check_live(tree, id, "HoverSignals", &mut errors),
                UiChange::Layer(id) => self.check_live(tree, id, "Layer", &mut errors),
                UiChange::Theme => {}
                UiChange::Font(id) => {
                    if tree.font(id).is_none() {
                        errors.push(format!("Font({id:?}) is not loaded"));
                    }
                }
            }
        }
        let mut s = self.handle.lock();
        s.recorded.extend_from_slice(changes);
        s.batches.push(changes.len());
        s.errors.extend(errors);
    }

    /// Moves the injected signals into `out`.
    pub fn drain_signals(&mut self, out: &mut Vec<UiSignal>) {
        out.append(&mut self.handle.lock().inject);
    }

    pub fn capture(&self) -> UiCapture {
        self.handle.lock().capture
    }

    pub fn resize(&mut self, logical: (f32, f32), scale: f32) {
        let mut s = self.handle.lock();
        s.size = logical;
        s.scale = scale;
    }
}

impl HeadlessModel {
    fn check_live(&self, tree: &UiTree, id: WidgetId, what: &str, errors: &mut Vec<String>) {
        if !self.known.contains(&id) {
            errors.push(format!("{what}({id:?}) of an unknown id"));
        }
        if !tree.contains(id) {
            errors.push(format!("{what}({id:?}) names a removed node"));
        }
    }
}

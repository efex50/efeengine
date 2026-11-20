//! The change journal (API.md section 13).

use crate::id::WidgetId;
use crate::style::FontId;

/// What changed since the last [`UiTree::take_changes`](crate::UiTree::take_changes). The journal
/// names the change, the backend reads the new state from the tree.
///
/// Coalescing rules (applied by the tree): a node created this frame gets no further property
/// entries (`Created` already covers its final state); a node created and removed in the same
/// frame leaves no entry at all; removing a node drops its earlier entries.
///
/// Placement rule (`Created` and `Moved`): a backend applies the journal against the FINAL tree
/// of the frame, so positions are always read from it. `Created` puts the node under its parent
/// as it is in the tree now. `Moved { id, parent, index }` is coalesced by
/// [`UiTree::take_changes`](crate::UiTree::take_changes): at most one entry per node, naming the
/// node's final parent and its final index in `tree.children(parent)` (so
/// `tree.children(parent)[index] == id` always holds when `sync` runs). The entries are NOT a
/// script of intermediate moves: a backend that wants to be exact simply orders the children of
/// every parent named by a `Created` or `Moved` entry like `tree.children(parent)`. The headless
/// backend checks the index against the final tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiChange {
    Created(WidgetId),
    /// One entry for every node of a removed subtree, children before parents.
    Removed(WidgetId),
    Moved { id: WidgetId, parent: WidgetId, index: u32 },
    /// Kind data changed: text, label, value, checked, placeholder, image, ...
    Kind(WidgetId),
    Style(WidgetId),
    Classes(WidgetId),
    Visible(WidgetId),
    Enabled(WidgetId),
    /// The node's `hover_signals` flag changed (a backend starts or stops watching pointer
    /// enter / leave for it). Read the flag from the node.
    HoverSignals(WidgetId),
    /// The `Layer` data (z, modal, anchor, offset) of a layer root changed.
    Layer(WidgetId),
    Theme,
    Font(FontId),
    /// Drop the keyboard focus of the text input in a window (`Some(window)`) or in every
    /// window (`None`): see [`UiTree::blur`](crate::UiTree::blur). Names no node. A backend
    /// that has a focused text input there unfocuses it and reports `Focus(false)`.
    Blur(Option<u32>),
}

impl UiChange {
    /// The node the change is about.
    pub fn widget(&self) -> Option<WidgetId> {
        match *self {
            UiChange::Created(id)
            | UiChange::Removed(id)
            | UiChange::Kind(id)
            | UiChange::Style(id)
            | UiChange::Classes(id)
            | UiChange::Visible(id)
            | UiChange::Enabled(id)
            | UiChange::HoverSignals(id)
            | UiChange::Layer(id) => Some(id),
            UiChange::Moved { id, .. } => Some(id),
            UiChange::Theme | UiChange::Font(_) | UiChange::Blur(_) => None,
        }
    }
}

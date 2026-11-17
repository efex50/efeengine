//! The change journal (API.md section 13).

use crate::id::WidgetId;
use crate::style::FontId;

/// What changed since the last [`UiTree::take_changes`](crate::UiTree::take_changes). The journal
/// names the change, the backend reads the new state from the tree.
///
/// Coalescing rules (applied by the tree): a node created this frame gets no further property
/// entries (`Created` already covers its final state); a node created and removed in the same
/// frame leaves no entry at all; removing a node drops its earlier entries.
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
    /// The `Layer` data (z, modal, anchor, offset) of a layer root changed.
    Layer(WidgetId),
    Theme,
    Font(FontId),
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
            | UiChange::Layer(id) => Some(id),
            UiChange::Moved { id, .. } => Some(id),
            UiChange::Theme | UiChange::Font(_) => None,
        }
    }
}

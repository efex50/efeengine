//! A wrapper that gives the children of a row / column / stack a stable identity.
//!
//! iced matches widget state (`Tree`) to widgets by position, so a child inserted before
//! another one, or a reorder, would hand its state (focus, caret, selection, scroll offset) to
//! the wrong node. `Keyed` keeps the keys of the children next to the state tree of the
//! container it wraps and, in `diff`, permutes the child trees so that every key keeps its own
//! tree; keys that are new get a fresh (empty) tree, keys that are gone drop theirs. Then the
//! inner container does its ordinary positional diff against the aligned trees.
//!
//! The wrapper is one extra tree level: `tree.children[0]` is the wrapped container's tree.

use iced_core::layout::{self, Layout};
use iced_core::widget::{Operation, Tree, tree};
use iced_core::{Element, Event, Length, Rectangle, Shell, Size, Vector, Widget, mouse, overlay, renderer};

struct KeyState(Vec<u64>);

pub struct Keyed<'a, M, T, R> {
    keys: Vec<u64>,
    inner: Element<'a, M, T, R>,
}

impl<'a, M, T, R> Keyed<'a, M, T, R> {
    /// `keys[i]` is the identity of the `i`-th child of `inner` (same order and length).
    pub fn new(keys: Vec<u64>, inner: Element<'a, M, T, R>) -> Self {
        Self { keys, inner }
    }
}

/// Moves `old_children` (keyed by `old_keys`) into the order of `new_keys`; unknown keys get an
/// empty tree. Pure, so it is unit-tested.
pub fn permute<X>(old_keys: &[u64], old: Vec<X>, new_keys: &[u64], fresh: impl Fn() -> X) -> Vec<X> {
    let mut pool: Vec<Option<X>> = old.into_iter().map(Some).collect();
    new_keys
        .iter()
        .map(|k| match old_keys.iter().position(|o| o == k) {
            Some(i) => pool.get_mut(i).and_then(Option::take).unwrap_or_else(&fresh),
            None => fresh(),
        })
        .collect()
}

impl<M, T, R> Widget<M, T, R> for Keyed<'_, M, T, R>
where
    R: iced_core::Renderer,
{
    fn size(&self) -> Size<Length> {
        self.inner.size()
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyState(self.keys.clone()))
    }

    fn diff(&mut self, t: &mut Tree) {
        let old_keys = std::mem::take(&mut t.state.downcast_mut::<KeyState>().0);
        if t.children.len() != 1 {
            t.children = vec![Tree::new(&self.inner)];
        } else if t.children[0].tag == self.inner.tag() && old_keys.len() == t.children[0].children.len() {
            let old = std::mem::take(&mut t.children[0].children);
            t.children[0].children = permute(&old_keys, old, &self.keys, Tree::empty);
        }
        t.state.downcast_mut::<KeyState>().0.clone_from(&self.keys);
        t.children[0].diff(&mut self.inner);
    }

    fn layout(&mut self, t: &mut Tree, renderer: &R, limits: &layout::Limits) {
        self.inner.layout(&mut t.children[0], renderer, limits);
        t.size = t.children[0].size;
    }

    fn draw(
        &self,
        t: &Tree,
        renderer: &mut R,
        theme: &T,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let Some((l, c)) = layout.iter(&t.children).next() {
            self.inner.draw(c, renderer, theme, style, l, cursor, viewport);
        }
    }

    fn operate(&mut self, t: &mut Tree, layout: Layout, viewport: &Rectangle, renderer: &R, op: &mut dyn Operation) {
        if let Some((l, c)) = layout.iter_mut(&mut t.children).next() {
            self.inner.operate(c, l, viewport, renderer, op);
        }
    }

    fn update(
        &mut self,
        t: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &R,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Some((l, c)) = layout.iter_mut(&mut t.children).next() {
            self.inner.update(c, event, l, cursor, renderer, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        t: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        match layout.iter(&t.children).next() {
            Some((l, c)) => self.inner.mouse_interaction(c, l, cursor, viewport, renderer),
            None => mouse::Interaction::None,
        }
    }

    fn overlay<'b>(
        &'b mut self,
        t: &'b mut Tree,
        layout: Layout,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, M, T, R>> {
        match layout.iter_mut(&mut t.children).next() {
            Some((l, c)) => self.inner.overlay(c, l, renderer, viewport, translation, window),
            None => Vec::new(),
        }
    }
}

impl<M, T, R> iced_core::widget::Meta for Keyed<'_, M, T, R> {}

#[cfg(test)]
mod tests {
    use super::permute;

    #[test]
    fn keys_keep_their_state_across_insert_and_reorder() {
        // state value = the key it was created for
        let old = vec!["a", "b", "c"];
        let ok = [1, 2, 3];
        // insert a new key at the front, drop 2, reorder 3 before 1
        let out = permute(&ok, old, &[9, 3, 1], || "new");
        assert_eq!(out, vec!["new", "c", "a"]);
    }

    #[test]
    fn duplicate_keys_do_not_panic() {
        let out = permute(&[1, 1], vec!["x", "y"], &[1, 1, 1], || "n");
        assert_eq!(out, vec!["x", "n", "n"]);
    }
}

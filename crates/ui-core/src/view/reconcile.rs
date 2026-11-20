//! The reconciler: turns an `El` tree into retained-tree operations (API.md section 16.2).
//!
//! It diffs against the tree itself (not against the previous `El`), so edits made behind the
//! view's back are overwritten and a value the plugin wrote back from a signal counts as the
//! current state.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::id::WidgetId;
use crate::tree::UiTree;
use crate::view::el::{El, Handlers};

pub type Table<Msg> = HashMap<WidgetId, Handlers<Msg>>;

fn join(prefix: &str, key: &str) -> String {
    let mut s = String::with_capacity(prefix.len() + 1 + key.len());
    s.push_str(prefix);
    s.push('/');
    s.push_str(key);
    s
}

/// Panics (before anything is applied) if two keyed elements get the same tree key, or a slot
/// has children.
pub fn validate<Msg>(el: &El<Msg>, prefix: &str, seen: &mut HashSet<String>) {
    let own;
    let child_prefix = match &el.key {
        Some(k) => {
            own = join(prefix, k);
            if !seen.insert(own.clone()) {
                panic!(
                    "UiView: duplicate key `{own}`; keys must be unique among the keyed elements that share the same nearest keyed ancestor"
                );
            }
            own.as_str()
        }
        None => prefix,
    };
    if el.slot && !el.children.is_empty() {
        panic!("UiView: slot `{child_prefix}` cannot have children (manage them with the retained API)");
    }
    for c in &el.children {
        validate(c, child_prefix, seen);
    }
}

/// Reconciles `root` as the only child of `layer`. `prefix` is the view name (the layer key).
pub fn reconcile_root<Msg: Clone>(tree: &mut UiTree, layer: WidgetId, prefix: &str, root: El<Msg>, table: &mut Table<Msg>) {
    reconcile_children(tree, layer, vec![root], prefix, table);
}

fn reconcile_children<Msg: Clone>(tree: &mut UiTree, parent: WidgetId, els: Vec<El<Msg>>, prefix: &str, table: &mut Table<Msg>) {
    let cur: Vec<WidgetId> = tree.children(parent).to_vec();
    let mut matched: Vec<Option<WidgetId>> = vec![None; els.len()];
    let mut claimed: HashSet<WidgetId> = HashSet::new();
    let mut misplaced: Vec<WidgetId> = Vec::new();

    for (i, e) in els.iter().enumerate() {
        let cand = match &e.key {
            Some(k) => match tree.find(&join(prefix, k)) {
                Some(id) if tree.parent(id) == Some(parent) => Some(id),
                // The key lives under another parent now: that node is replaced (ids are only
                // kept for moves inside one parent).
                Some(id) => {
                    misplaced.push(id);
                    None
                }
                None => None,
            },
            None => cur.get(i).copied().filter(|id| tree.key_of(*id).is_none()),
        };
        if let Some(id) = cand
            && tree.kind(id).is_some_and(|k| k.tag() == e.kind.tag())
            && claimed.insert(id)
        {
            matched[i] = Some(id);
        }
    }
    for id in misplaced {
        tree.remove(id);
    }
    for id in cur {
        if !claimed.contains(&id) {
            tree.remove(id);
        }
    }

    for (i, e) in els.into_iter().enumerate() {
        let id = match matched[i] {
            Some(id) => id,
            None => {
                let id = tree.try_create(parent, e.kind.clone()).expect("UiView: parent alive");
                if let Some(k) = &e.key {
                    let full = join(prefix, k);
                    if let Err(err) = tree.set_key(id, &full) {
                        panic!("UiView: cannot use key `{full}`: {err}");
                    }
                }
                id
            }
        };
        if tree.children(parent).get(i) != Some(&id) {
            let _ = tree.move_to(id, parent, i);
        }
        reconcile_node(tree, id, e, prefix, table);
    }
}

fn reconcile_node<Msg: Clone>(tree: &mut UiTree, id: WidgetId, el: El<Msg>, prefix: &str, table: &mut Table<Msg>) {
    let El { kind, classes, style, key, visible, enabled, slot, children, handlers } = el;
    if tree.kind(id) != Some(&kind) {
        let _ = tree.set_kind(id, kind);
    }
    let names: SmallVec<[&str; 4]> = classes.iter().map(String::as_str).collect();
    tree.set_classes(id, &names);
    tree.set_style(id, style);
    tree.set_visible(id, visible);
    tree.set_enabled(id, enabled);
    if !handlers.is_empty() {
        table.insert(id, handlers);
    }
    if slot {
        return;
    }
    let child_prefix;
    let child_prefix = match &key {
        Some(k) => {
            child_prefix = join(prefix, k);
            child_prefix.as_str()
        }
        None => prefix,
    };
    reconcile_children(tree, id, children, child_prefix, table);
}


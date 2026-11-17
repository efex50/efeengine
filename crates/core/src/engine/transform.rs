//! Built-in motion and hierarchy systems: velocity integration (`FixedUpdate`) and transform
//! propagation (`PostUpdate`). Registered by `Engine::with_config`.

use std::collections::HashMap;

use glam::Affine2;
use hecs::Entity;

use crate::engine::components::{Children, GlobalTransform, Parent, Transform, Velocity};
use crate::engine::render::RenderWarnings;
use crate::engine::schedule::{Access, ParCtx};

/// What [`velocity_system`] declares.
pub(crate) fn velocity_access() -> Access {
    Access::new().named("velocity").read::<Velocity>().write::<Transform>()
}

/// `FixedUpdate` (parallel): `position += linear * dt`, `rotation += angular * dt` with the fixed delta.
pub fn velocity_system(cx: &ParCtx<'_>) {
    let dt = cx.time.fixed_delta();
    cx.par_for_each::<(&mut Transform, &Velocity), _>(1024, move |(t, v)| {
        t.position += v.linear * dt;
        t.rotation += v.angular * dt;
    });
}

/// What [`propagate_transforms`] declares.
pub(crate) fn propagate_access() -> Access {
    Access::new()
        .named("propagate_transforms")
        .read::<Transform>()
        .read::<Parent>()
        .write::<GlobalTransform>()
        .write::<Children>()
        .res_mut::<RenderWarnings>()
}

/// `PostUpdate` (parallel): compute every `GlobalTransform` from `Transform` and the `Parent` chain
/// (`global = parent_global * local`), inserting `GlobalTransform` where it is missing, and
/// rebuild `Children`.
///
/// A parent counts only if the entity is alive and has a `Transform`; otherwise the child is a
/// root. A cycle is broken by making the entity the walk meets first (lowest id order) a root;
/// it is logged once (`RenderWarnings::hierarchy_cycle`).
pub fn propagate_transforms(cx: &ParCtx<'_>) {
    // nodes: every entity with a Transform, sorted by id for a deterministic result
    let mut nodes: Vec<(Entity, Affine2, Option<Entity>)> = cx
        .query::<(Entity, &Transform, Option<&Parent>)>()
        .iter()
        .map(|(e, t, p)| (e, t.affine(), p.and_then(|p| p.entity())))
        .collect();
    nodes.sort_by_key(|(e, _, _)| e.id());
    let index: HashMap<Entity, usize> = nodes.iter().enumerate().map(|(i, (e, _, _))| (*e, i)).collect();
    // effective parent index (stale or Transform-less parents are roots)
    let mut parent: Vec<Option<usize>> = nodes.iter().map(|(_, _, p)| p.and_then(|p| index.get(&p).copied())).collect();

    // 0 = new, 1 = on the current walk, 2 = done
    let mut state = vec![0u8; nodes.len()];
    let mut global: Vec<Affine2> = vec![Affine2::IDENTITY; nodes.len()];
    let mut cycle = false;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..nodes.len() {
        if state[start] == 2 {
            continue;
        }
        stack.clear();
        let mut cur = Some(start);
        while let Some(i) = cur {
            match state[i] {
                2 => break,
                1 => {
                    // back on this walk: a cycle. `i` becomes a root, so it must be resolved
                    // first: move it to the top of the stack.
                    parent[i] = None;
                    cycle = true;
                    if let Some(pos) = stack.iter().position(|&n| n == i) {
                        stack.remove(pos);
                        stack.push(i);
                    }
                    break;
                }
                _ => {
                    state[i] = 1;
                    stack.push(i);
                    cur = parent[i];
                }
            }
        }
        // resolve from the top of the chain down
        while let Some(i) = stack.pop() {
            let local = nodes[i].1;
            global[i] = match parent[i] {
                Some(p) => global[p] * local,
                None => local,
            };
            state[i] = 2;
        }
    }
    if cycle {
        let mut warn = cx.res_mut::<RenderWarnings>();
        if !warn.hierarchy_cycle {
            warn.hierarchy_cycle = true;
            crate::global_warn("a Parent cycle was found; it was broken by treating one entity of it as a root");
        }
    }

    // write GlobalTransform: update in place, insert (through commands) where missing
    for (i, (e, _, _)) in nodes.iter().enumerate() {
        let g = GlobalTransform::from_affine(global[i]);
        match cx.query_one::<&mut GlobalTransform>(*e).get() {
            Ok(slot) => *slot = g,
            Err(_) => cx.commands().insert(*e, g),
        }
    }

    // rebuild Children (children are visited in id order, so each list is sorted)
    let mut kids: Vec<Vec<Entity>> = vec![Vec::new(); nodes.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            kids[*p].push(nodes[i].0);
        }
    }
    let stale: Vec<Entity> = cx
        .query::<(Entity, &Children)>()
        .iter()
        .filter(|(e, _)| index.get(e).is_none_or(|&i| kids[i].is_empty()))
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        cx.commands().remove::<Children>(e);
    }
    for (i, list) in kids.into_iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let p = nodes[i].0;
        match cx.query_one::<&mut Children>(p).get() {
            Ok(c) => {
                if c.entities() != list.as_slice() {
                    c.set(list);
                }
            }
            Err(_) => cx.commands().insert(p, Children::new(list)),
        }
    }
}

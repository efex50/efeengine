//! Built-in motion and hierarchy systems: velocity integration (`FixedUpdate`) and transform
//! propagation (`PostUpdate`). Registered by `Engine::with_config`.

use std::collections::HashMap;

use glam::Affine2;
use hecs::Entity;

use crate::engine::components::{Children, GlobalTransform, Parent, Transform, Velocity};
use crate::engine::render::RenderWarnings;
use crate::engine::schedule::Ctx;

/// `FixedUpdate`: `position += linear * dt`, `rotation += angular * dt` with the fixed delta.
pub fn velocity_system(cx: &mut Ctx<'_>) {
    let dt = cx.time.fixed_delta();
    for (t, v) in cx.world.query_mut::<(&mut Transform, &Velocity)>() {
        t.position += v.linear * dt;
        t.rotation += v.angular * dt;
    }
}

/// `PostUpdate`: compute every `GlobalTransform` from `Transform` and the `Parent` chain
/// (`global = parent_global * local`), inserting `GlobalTransform` where it is missing, and
/// rebuild `Children`.
///
/// A parent counts only if the entity is alive and has a `Transform`; otherwise the child is a
/// root. A cycle is broken by making the entity the walk meets first (lowest id order) a root;
/// it is logged once (`RenderWarnings::hierarchy_cycle`).
pub fn propagate_transforms(cx: &mut Ctx<'_>) {
    // nodes: every entity with a Transform, sorted by id for a deterministic result
    let mut nodes: Vec<(Entity, Affine2, Option<Entity>)> = cx
        .world
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
    if cycle && !cx.resources.get::<RenderWarnings>().is_some_and(|w| w.hierarchy_cycle) {
        cx.resources.get_or_insert_with(RenderWarnings::default).hierarchy_cycle = true;
        crate::global_warn("a Parent cycle was found; it was broken by treating one entity of it as a root");
    }

    // write GlobalTransform: update in place, insert where missing
    let mut missing: Vec<(Entity, GlobalTransform)> = Vec::new();
    for (i, (e, _, _)) in nodes.iter().enumerate() {
        let g = GlobalTransform::from_affine(global[i]);
        match cx.world.query_one_mut::<&mut GlobalTransform>(*e) {
            Ok(slot) => *slot = g,
            Err(_) => missing.push((*e, g)),
        }
    }
    for (e, g) in missing {
        let _ = cx.world.insert_one(e, g);
    }

    // rebuild Children (children are visited in id order, so each list is sorted)
    let mut kids: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            kids.entry(nodes[*p].0).or_default().push(nodes[i].0);
        }
    }
    let stale: Vec<Entity> = cx
        .world
        .query::<(Entity, &Children)>()
        .iter()
        .filter(|(e, _)| !kids.contains_key(e))
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        let _ = cx.world.remove_one::<Children>(e);
    }
    for (p, list) in kids {
        match cx.world.query_one_mut::<&mut Children>(p) {
            Ok(c) => {
                if c.0 != list {
                    c.0 = list;
                }
            }
            Err(_) => {
                let _ = cx.world.insert_one(p, Children(list));
            }
        }
    }
}

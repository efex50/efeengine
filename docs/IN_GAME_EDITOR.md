# In-game editor

Status: step 1 of 4 done (modes, hierarchy, read-only inspector). This replaces the separate editor
app: [`EDITOR.md`](EDITOR.md), `crates/editor` and `crates/editor-link` are deprecated (kept for their
ideas; nothing new goes there).

## Idea

The editor is a plugin that runs inside the game, on top of the Debug menu (F5). The game, the
editor and the world are one process: no protocol, no second window of state. Games enable it in
development only:

```toml
efeengine-ui = { path = "...", features = ["editor"] }   # off by default
```

```rust
engine.add_plugin(UiPlugin::new())?;
engine.add_plugin(DebugMenuPlugin)?;
engine.add_plugin(EditorPlugin::new())?;   // adds the "Editor" tab; .start_in(EditorMode::Play) to not freeze at start
```

Code: `crates/ui/src/editor/` (`mod.rs` plugin, resources, systems; `mode.rs` play states; `panel.rs`
the tab). The Debug menu got one extension point for it: `DebugExtraTabs` (a tab = title + view
function + wake check) and `DebugMsg::Ext` (tag + payload). The view still rebuilds only when
something it shows changed; the editor writes its resources only on a real change.

## Modes

| Mode | Game systems | World |
|---|---|---|
| Edit (default at start) | frozen | can be inspected (edited in step 2) |
| Play | run | a snapshot was taken when Play started from Edit |
| Pause | frozen again | kept as it is |
| Step (from Pause) | run for one frame, then frozen | advances one frame |
| Stop (or Edit button) | frozen | restored from the snapshot, unless "Keep changes on Stop" |

Frozen = `set_system_enabled(false)` on every system of `PreUpdate`, `FixedUpdate`, `Update`,
`PostUpdate` that is not: global (`Engine::set_global_system`), `propagate_transforms` /
`render_extract`, or owned by a plugin whose name starts with `efeengine` (UI, Debug menu, the
editor). `script_system` and `velocity_system` are frozen like any game system. Play re-enables
exactly the systems Edit froze (a system the game had disabled stays disabled). Transitions run
in `Commands::engine_task`, at the end of a stage, so snapshot and flags change at one point.
Step overshoots by the rest of the frame it was requested in (PostUpdate systems run twice).
Programmatic control: `cx.send(EditorRequest::Play)`; state: `EditorState` resource.

Play-mode fidelity is the `snapshot()` / `restore()` of `docs/SCENES.md` (S3): only registered
components (`#[script_component]`, mod components) and resources registered with
`register_snapshot_resource` survive Stop. `Name` is Rust-only, so the plugin puts it back by
`EditorId` (entities without an `EditorId` lose it). Entity ids change on Stop; the selection
follows its `EditorId`.

## Core API (`efeengine_core::engine::inspect`, methods on `Engine`)

- `editable_entities() -> Vec<EntityRow>`: what a scene would capture, in tree order (roots by
  `EditorId`, then entity bits; children after their parent). `EntityRow { entity, name, editor_id,
  parent, scene_owned, depth, children }`. Name: `Name`, else `<first component> #<index>`, else
  `Entity(<index>)`. A parent outside the list, or a cycle, makes roots.
- `inspect_entity(e) -> Option<Vec<InspectedComponent>>`: registry-known components, fields in
  declared order as `SceneValue` (the scene codec), `readonly` flag.
- `set_component_field(e, component, field, SceneValue) -> Result<(), SceneError>`: one field
  through the same codec; nothing is written on error. (Used by the inspector in step 2.)

## Panels (step 1)

- Toolbar: Edit | Play | Pause | Step | Stop (current mode highlighted), status line, "Keep changes".
- Hierarchy: filter box, tree indented by `Parent`, click selects (`EditorSelection`).
- Inspector: component headers, `field: value` rows, read-only for now.
- World: a rectangle around the selected sprite (`DebugDraw2D`).
- Data refresh: `EditorPanel` is read at most every 0.25 s while the tab shows, and at once after a
  selection or mode change or when the tab opens.

## Roadmap

Step 1: modes, hierarchy, read-only inspector
- [x] `Engine::editable_entities`, `inspect_entity`, `set_component_field` (+ `tests/inspect.rs`)
- [x] `DebugExtraTabs` extension point in the Debug menu
- [x] `EditorPlugin`, feature `editor`: Edit / Play / Pause / Step / Stop, keep changes
- [x] Hierarchy (tree, filter, select), read-only inspector, selection outline
- [x] Tests (core, UI, headless example `examples/editor_demo`), screenshot

Step 2: editing
- [ ] Editable inspector fields (`set_component_field`), add / remove component, rename
- [ ] Spawn / delete / duplicate / reparent entities (EditorId allocation)
- [ ] Undo / redo command log
- [ ] Save / load scene (`save_scene`, `load_scene_file`), dirty marker

Step 3: viewport
- [ ] World picking (click selects), gizmos (move / rotate / scale), editor camera (pan, zoom)
- [ ] Undo for gizmo drags, snapping

Step 4: scenes and prefabs
- [ ] Prefabs (`docs/SCENES.md` S5): instantiate, edit, apply / revert overrides
- [ ] Scene system list (S4.3): per-scene enabled systems, global systems
- [ ] Hot reload of scripts and mods while in Edit

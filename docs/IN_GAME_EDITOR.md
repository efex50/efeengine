# In-game editor

Status: steps 1 and 2 of 4 done (modes, hierarchy, editable inspector, entity actions, undo / redo, scene files). This replaces the separate editor
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

Code: `crates/ui/src/editor/` (`mod.rs` plugin, resources, systems; `mode.rs` play states; `edit.rs`
edits and the undo log; `files.rs` scene files; `panel.rs` the tab). `examples/editor_demo` is the
native test bed; it loads `scenes/demo.ron` (a test keeps it equal to the demo world), so Save /
Load can be tried (F5, tab Editor). The Debug menu got one extension point for it: `DebugExtraTabs` (a tab = title + view
function + wake check) and `DebugMsg::Ext` (tag + payload). The view still rebuilds only when
something it shows changed; the editor writes its resources only on a real change.

## Modes

| Mode | Game systems | World |
|---|---|---|
| Edit (default at start) | frozen | can be inspected and edited |
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
  through the same codec; nothing is written on error.
- `InspectedComponent::entity_fields`: names of the fields that hold an entity reference
  (read-only in the UI).
- `addable_components(e)`, `add_component_default(e, name)`, `remove_component(e, name)`: what the
  component picker lists, and add / remove through the registry. The new component is `Default`
  for `Transform`, `Sprite` and `Camera`, zeroed bytes for everything else. Read-only components
  (`GlobalTransform`) are refused; `EditorId` is not offered by `addable_components`
  (`UNMANAGED_COMPONENTS`).
- `next_editor_id()`, `ensure_editor_id(e)`: unique `EditorId` allocation (largest + 1).
- `new_editor_entity()`: `Transform` + `EditorId` + `Name` "entity N", owned by the active scene
  when one is loaded (`spawn_scene_owned`), else a free entity.
- `entity_tree(e)`, `capture_entity_data(e) -> EntityData`, `spawn_entity_data(&[EntityData])`,
  `duplicate_entity(e)`, `delete_entity_tree(e)`: copies of entities (registry components, `Name`,
  scene owner tag) that can be spawned again with references among them remapped; duplicate gets
  a new `EditorId`; delete takes the descendants and returns the copies (undo).
- `clear_editable_world()`: unload the scene and despawn everything `editable_entities` lists.
- `capture_world_scene(name)` / `save_world_scene(path)` (in `scene.rs`): every editable entity
  (not only scene-owned ones), with ids 1, 2, ... in `EditorId` order and entity fields renumbered
  to them, so `Parent` survives a file round trip.

## Panels

- Toolbar: Edit | Play | Pause | Step | Stop (current mode highlighted) | Undo | Redo, status line,
  "Keep changes".
- Scene row: `Scene: <name>` (`*` while there are unsaved edits, `(none)` without a scene), Save,
  Save As, Load, New, a path field (Enter loads), the `*.ron` files of `scenes/` (first four, a
  click loads) and a message line for the last result or error (also logged). The name is the
  file's stem, else `Engine::current_scene`.
- Hierarchy: filter box, New / Duplicate / Delete, tree indented by `Parent`, click selects
  (`EditorSelection`).
- Inspector: a Name input, "Add component" (a list of what the entity lacks), then per component a
  header with an `x` button and `field: widget` rows.
- World: a rectangle around the selected sprite (`DebugDraw2D`).
- Data refresh: `EditorPanel` is read at most every 0.25 s while the tab shows, and at once after a
  selection, mode or edit. The tab's own state (`EditorUi`: drafts, errors, status, scene path,
  dirty flag) is written only on a real change; an idle editor still rebuilds nothing.

## Editing (step 2)

Fields. The widget follows the value: `Bool` is a checkbox (writes at once), a number is a text
input, a list (`position`, `scale`, `color`) is one input per element. Enter or losing focus
commits the text; Escape drops it. The text is parsed by the shape of the current value (`Float`,
`Int`, `UInt`) and written with `set_component_field` (so ranges are checked by the codec); a
text that does not parse or fit stays in the field with the `wire-error` style, the world keeps
its value and the message line says why. Read-only components, entity references (`Parent.entity`)
and `EditorId` are plain text. Editing is allowed in every mode: in Play it is live and Stop takes
it back unless "Keep changes" is on.

Components. "Add component" lists registry components the entity lacks (no read-only ones, no
`EditorId`, no `Parent`). `x` removes one; it is not shown for read-only components
(`GlobalTransform`; `Children` is Rust-only and never listed) nor for `EditorId`, which the editor
needs to find the entity again (undo, selection after Stop, scene files).

Entities. New makes `Transform` + `EditorId` + `Name` "entity N" (scene-owned when a scene is
loaded) and selects it. Duplicate copies the registry components and `Name` ("<name> copy") with
a new `EditorId`; `Parent` is copied (the copy is a sibling), children are not. Delete removes the
entity and its descendants. Rename is the Name input of the inspector.

Undo / redo. Every edit is recorded in `EditorHistory` (a resource, 500 entries) with what
reverses it: field set (old and new value), add / remove component (with the removed values),
rename, new / duplicate (undo deletes it, redo spawns the same copy again), delete (undo spawns the
tree again with the same `EditorIds` and rebuilt `Parent` links). Commands name entities by
`EditorId`; an entity without one gets one the first time it is edited. A new edit clears the redo
side. The log is cleared when Stop restores the snapshot and when a scene is loaded or New is
pressed.

| Shortcut | Action |
|---|---|
| Ctrl+Z | Undo |
| Ctrl+Shift+Z, Ctrl+Y | Redo |

Shortcuts work only while the Editor tab shows and never while an editor text input has focus
(the plugin tracks the focus of its inputs, and also honours the UI's keyboard capture), so
Ctrl+Z inside a text field belongs to the field.

Scene files. Save writes to the path the scene was loaded from or last saved to (kept in
`EditorUi::path`, the editor plugin, not the engine); Save As writes to the path field (`.ron` is
added when it has no extension, directories are created); Load reads, parses and validates the
file first (nothing changes when it fails), then replaces the editable world (`clear_editable_world`
+ `load_scene`); New unloads the scene and empties the world. All four need Edit mode (the buttons
are disabled in Play / Pause and the message line says so). Paths are relative to the current
directory. The file is `Engine::save_world_scene` (every editable entity, with ids so `Parent`
survives) plus a sidecar `<file>.names` (`<EditorId>TAB<name>` lines): `Name` is Rust-only and a
scene file cannot hold it. Public helpers: `editor::save_scene_with_names`,
`editor::load_scene_with_names(engine, path, replace)`, `editor::list_scenes`.

Known limitations. Names are saved only for entities with an `EditorId`. Entity references other
than `Parent` are not editable. Undoing the removal of a component that holds an entity reference
(`Parent`) restores the old raw reference, which is stale if its target was deleted and brought
back in between. Reparenting is not done yet. The Debug menu window now opens at 640x600.

## Roadmap

Step 1: modes, hierarchy, read-only inspector (done)
- [x] `Engine::editable_entities`, `inspect_entity`, `set_component_field` (+ `tests/inspect.rs`)
- [x] `DebugExtraTabs` extension point in the Debug menu
- [x] `EditorPlugin`, feature `editor`: Edit / Play / Pause / Step / Stop, keep changes
- [x] Hierarchy (tree, filter, select), read-only inspector, selection outline
- [x] Tests (core, UI, headless example `examples/editor_demo`), screenshot

Step 2: editing
- [x] Editable inspector fields (`set_component_field`), add / remove component, rename
- [x] Spawn / delete / duplicate entities (EditorId allocation); reparent is left for step 3
- [x] Undo / redo command log (Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y, toolbar)
- [x] Save / load scene (`save_world_scene`, `load_scene`, names sidecar), dirty marker, `scenes/` list

Step 3: viewport
- [ ] World picking (click selects), gizmos (move / rotate / scale), editor camera (pan, zoom)
- [ ] Undo for gizmo drags, snapping

Step 4: scenes and prefabs
- [ ] Prefabs (`docs/SCENES.md` S5): instantiate, edit, apply / revert overrides
- [ ] Scene system list (S4.3): per-scene enabled systems, global systems
- [ ] Hot reload of scripts and mods while in Edit

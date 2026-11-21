# In-game editor

Status: steps 1 to 3 of 4 done (modes, hierarchy, editable inspector, entity actions, undo / redo, scene files, viewport: editor camera, picking, gizmos, reparenting, separate editor window). This replaces the separate editor
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
engine.add_plugin(EditorPlugin::new())?;   // adds the "Editor" tab; .start_in(EditorMode::Play) to not freeze at start,
                                           // .separate_window(true) for layout B, .scenes_dir("levels") for the Load list
```

Code: `crates/ui/src/editor/` (`mod.rs` plugin, resources, systems; `mode.rs` play states; `edit.rs`
edits and the undo log; `files.rs` scene files; `panel.rs` the tab; `viewport.rs` camera, picking, gizmos). `examples/editor_demo` is the
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
`register_snapshot_resource` survive Stop. `Name` is part of a snapshot and of scene files (`SceneEntity::name`), so it survives Stop for every
entity. Entity ids change on Stop; the selection follows its `EditorId`. The editor camera is an
`EditorOnly` entity that snapshots never touch.

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
- `EntityData::scene.name` carries the `Name`; `entity_field_names(component)` lists a component's entity-reference
  fields (the editor keeps references that point outside a copied group by `EditorId`).
- `new_editor_entity()`: `Transform` + `EditorId` + `Name` "entity N", owned by the active scene
  when one is loaded (`spawn_scene_owned`), else a free entity.
- `entity_tree(e)`, `capture_entity_data(e) -> EntityData`, `spawn_entity_data(&[EntityData])`,
  `duplicate_entity(e)`, `delete_entity_tree(e)`: copies of entities (registry components, `Name`,
  scene owner tag) that can be spawned again with references among them remapped; duplicate gets
  a new `EditorId`; delete takes the descendants and returns the copies (undo).
- `clear_editable_world()`: unload the scene and despawn everything `editable_entities` lists.
- `EditorOnly` (marker component): the entity is a tool's own (the editor camera): `captured_entities`
  (capture, snapshot, restore, `editable_entities`, `clear_editable_world`) skips it and the render extract
  prefers an `EditorOnly` camera over the game's cameras of its window. `DebugDraw2D::set_window(w)`
  sends the shapes pushed after it to window `w`'s frame (`WindowFrame::d2.draw`).
- `capture_world_scene(name)` / `save_world_scene(path)` (in `scene.rs`): every editable entity
  (not only scene-owned ones), with ids 1, 2, ... in `EditorId` order and entity fields renumbered
  to them, so `Parent` survives a file round trip.

## Panels

- Toolbar: Edit | Play | Pause | Step | Stop (current mode highlighted) | Undo | Redo, status line,
  "Keep changes". Viewport row: Move (W) | Rotate (E) | Scale (R), Grid (G) checkbox, grid step input,
  Detach / Attach.
- Scene row: `Scene: <name>` (`*` while there are unsaved edits, `(none)` without a scene), Save,
  Save As, Load, New, a path field (Enter loads), the `*.ron` files of `scenes/` (first four, a
  click loads) and a message line for the last result or error (also logged). The name is the
  file's stem, else `Engine::current_scene`.
- Hierarchy: filter box, New / Duplicate / Delete / Unparent, tree indented by `Parent`, click selects
  (`EditorSelection`, shared with viewport picking; the row of a picked entity is highlighted, the
  list is not scrolled to it).
- Inspector: a Name input, a "Parent" row (Change opens the list of possible parents plus "none"), "Add component" (a list of what the entity lacks), then per component a
  header with an `x` button and `field: widget` rows.
- World: the grid, a handle (small circle) for every entity without a sprite, a rectangle around the
  selected sprite and the gizmo (`DebugDraw2D`, in the window the editor camera draws).
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
survives, and `name: Some("...")` for every entity that has a `Name`; there is no sidecar file).
The Load list shows the `*.ron` files of the plugin's scenes directory (`EditorPlugin::scenes_dir`,
default `scenes`). `editor::list_scenes(dir)` is public.

Known limitations (steps 1 and 2). Entity references other than `Parent` are not editable. The
Debug menu window opens at 640x600.

## Viewport (step 3)

Editor camera. `EditorCamera` (resource, `.entity`) is an entity with `Camera`, `Transform` and `EditorOnly`:
never captured by scenes or snapshots, not in the hierarchy, not in the undo log, never restored.
Its position and zoom are kept in the plugin (it is written back to the entity every frame, so a
game system that moves every `Transform` cannot move it) and persist across Edit / Play. It starts
where the game's first active 2D camera is (position, zoom, clear colour); Home goes back there.
The game's cameras are never modified: the render extract prefers the `EditorOnly` camera of a
window, and the editor turns its `active` flag on and off.

Layouts.

| | A (default) | B (`separate_window(true)`, or the Detach button) |
|---|---|---|
| Main window | editor camera in Edit and Paused, game camera in Play | game camera in every mode |
| Editor camera | active only outside Play | active all the time, in its own window |
| Editor panels | Editor tab of the Debug menu (its own window, F5) | the panel view `editor_panel` in the editor window (top left, 540x660); the Debug menu keeps its own window and its Editor tab shows only a note and Attach |
| Gizmos, grid, outline | main window | editor window |
| Close the editor window | n/a | same as Attach |

Without a second window (SDL mode, a custom renderer, a backend that cannot open one) `Detach` and
`separate_window(true)` log a warning, show the reason in the message line and stay in layout A.
The window (and the layer placement) follow `EditorView::window`.

Controls (only when the editor camera is the one drawing the window, the pointer is in that
window and not over UI, and for keys no text input has focus and the window has keyboard focus):

| Input | Action |
|---|---|
| Middle drag, or Space + left drag | pan |
| Wheel | zoom around the cursor (x1.1 per line, 0.02 to 64 pixels per unit) |
| F | frame the selection (a sprite fills about half the window; other entities are centred) |
| Home | back to the starting position and zoom |
| Left click | select the topmost entity under the cursor; empty space clears the selection |
| W / E / R | Move / Rotate / Scale gizmo |
| G | grid on / off |
| Ctrl (held while dragging) | snap: position to the grid step, rotation to 15 degrees, scale to 0.25 |
| Shift (held while scaling) | uniform scale |
| Escape (during a drag) | cancel the drag |
| Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y | undo / redo (Editor tab showing, or the editor window focused) |

Picking. Sprites are tested by their real quad (`Sprite` size, or the texture size for
`auto_size`, times the entity's world transform, rotation and scale included); invisible
(`Visibility`) and zero-size sprites are skipped; among overlapping sprites the topmost wins (render
order: `layer`, then entity id). An entity without a sprite is picked by a handle, a circle of 9
pixels at its position (drawn in the viewport); handles win over sprites under them. Only entities
the hierarchy lists can be picked. A click on a gizmo handle of the selection starts a drag instead.

Gizmos. Drawn with `DebugDraw2D` in screen-constant size. Move: centre square (free) and X / Y
arrows (red / green, world axes). Rotate: a ring of 64 pixels. Scale: the four corners of the
sprite's quad (a box of 40 pixels for an entity without a sprite). A drag writes the entity's local
`Transform` live every frame; the pointer is always converted through the parent's world transform,
so a child moves in world space and keeps a correct local `Transform`; children follow through the
normal transform propagation. The whole drag is one undo entry (start to end `Transform`); a drag
that ends where it began records nothing. Grid step: the toolbar input (default 16, positive
numbers only).

Reparenting. The inspector's Parent row (Change: pick an entity or none) and the hierarchy's
Unparent button. The entity keeps its world position, rotation and scale (the local `Transform` is
recomputed through the new parent). It is refused for the entity itself or one of its
descendants, a dead parent and a parent without `Transform`. Undoable; the parents are kept by
`EditorId`. Limitation: a non-uniform scaled and rotated parent can need a shear that `Transform`
cannot hold; the position is exact, the shape may differ.

Undo of references. A removed component that holds an entity reference (`Parent`) remembers the
target by `EditorId` and finds it again at undo time; a deleted or duplicated group does the same
for references that leave the group. So undoing in any order after the target was deleted and
brought back (new entity bits) still links to the right entity.

Tests: `crates/ui/src/editor/tests_viewport.rs` (the camera math is also unit-tested in
`viewport.rs`), `crates/core/tests/{scene,windows}.rs`. `tests_viewport.rs` renders the world
through the real sprite renderer offscreen (`cargo test ... screenshots_of_the_viewport --
--nocapture` prints PNGs base64 encoded).

Known limitations (step 3). The hierarchy is not scrolled to a picked entity. Handles are drawn for
every entity without a sprite, which is noisy in worlds with many logic entities. The editor camera
does not rotate. 3D cameras are not handled (the viewport is 2D). Layout B places the panels
at a fixed size (540x660) in the top left corner.

## Roadmap

Step 1: modes, hierarchy, read-only inspector (done)
- [x] `Engine::editable_entities`, `inspect_entity`, `set_component_field` (+ `tests/inspect.rs`)
- [x] `DebugExtraTabs` extension point in the Debug menu
- [x] `EditorPlugin`, feature `editor`: Edit / Play / Pause / Step / Stop, keep changes
- [x] Hierarchy (tree, filter, select), read-only inspector, selection outline
- [x] Tests (core, UI, headless example `examples/editor_demo`), screenshot

Step 2: editing
- [x] Editable inspector fields (`set_component_field`), add / remove component, rename
- [x] Spawn / delete / duplicate entities (EditorId allocation); reparent came in step 3
- [x] Undo / redo command log (Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y, toolbar)
- [x] Save / load scene (`save_world_scene`, `load_scene`), dirty marker, `scenes/` list

Step 3: viewport (done)
- [x] Editor camera (`EditorOnly`, pan, zoom, frame, home), layouts A and B (`separate_window`, Detach / Attach)
- [x] World picking (click selects), gizmos (move / rotate / scale), grid and snapping
- [x] Undo for gizmo drags, reparenting (inspector picker, Unparent), child gizmos in world space
- [x] Step-2 leftovers: names in the scene file (no `.names` sidecar), undo resolves references by `EditorId`, configurable scenes directory

Step 4: scenes and prefabs
- [ ] Prefabs (`docs/SCENES.md` S5): instantiate, edit, apply / revert overrides
- [ ] Scene system list (S4.3): per-scene enabled systems, global systems
- [ ] Hot reload of scripts and mods while in Edit

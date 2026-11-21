# In-game editor

Status: all 4 steps done (modes, hierarchy, editable inspector, entity actions, undo / redo, scene files, viewport: editor camera, picking, gizmos, reparenting, separate editor window, prefabs, the scene system list). This replaces the separate editor
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

## Using the editor in your game

1. **Feature flag.** Depend on `efeengine-ui` with the `editor` feature, in development builds only
   (it is off by default); gate the plugin the same way:

   ```toml
   efeengine-ui = { path = "...", features = ["editor"] }
   ```

2. **Plugin setup.** After `UiPlugin` and `DebugMenuPlugin`, before the world is built (the editor
   freezes the game on its first frame, so systems must exist by then):

   ```rust
   engine.add_plugin(UiPlugin::new())?;
   engine.add_plugin(DebugMenuPlugin)?;
   engine.add_plugin(
       EditorPlugin::new()
           .scenes_dir("scenes")      // the Load list (default "scenes")
           .prefabs_dir("prefabs")    // prefab files (default "prefabs", or the dir the engine already has)
           .separate_window(false),   // true: layout B, the editor camera and panels in their own window
   )?;
   ```

   Then press F5 and open the "Editor" tab. Games that load their own scenes should call
   `engine.set_prefab_dir(..)` themselves (scene files with `prefab:` entries need it even without the editor).

3. **Make things editable.** The inspector shows what the type registry knows: Rust components
   marked `#[script_component]` (and registered with `engine.register::<T>()`) and mod components.
   Fields are numbers, bools and fixed arrays of them; a field marked `#[script(entity)]` is an
   entity reference (shown read-only). Anything else (plain Rust components, `Name` aside) is
   invisible to the editor, scenes and snapshots.

4. **Survive Stop.** Play takes a snapshot; Stop restores it. Components are covered by (3).
   Resources only if registered with `engine.register_snapshot_resource::<T>()` (`T: Clone`),
   otherwise they keep their played value (`Engine::snapshot_gaps()` lists them).

5. **`EditorId` and `Name`.** Every editable entity gets an `EditorId` (a registered component; the
   editor adds one the first time an entity is edited, or you set them in code) and may have a
   `Name` (the hierarchy label, kept in scene files). Entities spawned by scene files are owned by
   the scene (a scene switch removes them); entities the game spawns with `world.spawn` are
   listed too, saved too, but never removed by a switch.

6. **Scenes, prefabs, systems.** Scene files live in the scenes directory (`Save As` writes
   them, `Engine::load_scene_file` loads them); prefab files in the prefabs directory. A scene's
   `systems` list is edited in the Systems panel. Mark engine-level systems that edit mode must
   not freeze with `engine.set_global_system(name)`.

Controls (also listed with their details below):

| Where | Input | Action |
|---|---|---|
| Debug menu | F5 | open / close the window, tab "Editor" |
| Toolbar | Edit, Play, Pause, Step, Stop | play states; Stop restores the world |
| Anywhere in the tab | Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | undo / redo (not while typing in a field) |
| Viewport | middle drag, or Space + left drag | pan |
| Viewport | wheel | zoom around the cursor |
| Viewport | left click, drag a gizmo handle | select, move / rotate / scale (Ctrl snaps, Shift = uniform scale, Escape cancels) |
| Viewport | W / E / R | Move / Rotate / Scale tool |
| Viewport | F, Home | frame the selection, back to the start |
| Viewport | G, H | grid on / off, handles of sprite-less entities on / off |
| Left column | tabs Hierarchy, Prefabs, Systems | entity tree, prefab list, the scene's system list |

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
- Prefabs (`engine::prefab`, `docs/SCENES.md` S5; see "Prefabs" below): `Prefab`, `PrefabInstance`,
  `set_prefab_dir`, `register_prefab`, `find_prefab` / `load_prefab`, `save_prefab`, `remove_prefab`,
  `prefab_names`, `place_prefab`, `capture_prefab`, `link_prefab`, `prefab_instance`,
  `set_prefab_instance`, `prefab_instance_root`, `prefab_instance_entities`, `unpack_prefab`,
  `prefab_overrides`, `prefab_base_field`, `apply_prefab_to_world`. `EntityRow::prefab` names the
  prefab an instance root belongs to; `EntityData` keeps the link, so undo of a delete restores it.
- Scene systems (`engine::scene`): `registered_systems() -> Vec<SystemInfo>` (name, owner plugin, stage,
  enabled, global, listed), `scene_system_entries()` (the list as written, `plugin:` entries
  unexpanded), `scene_entries_with(name, on)`, `set_scene_system_entries(entries, apply_flags)`.
  The active scene now remembers its `systems` and `requires` as written, so `capture_world_scene`
  (Save) writes them back (before, a saved scene lost its `systems` list).
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
- Left column: three tabs, Hierarchy | Prefabs | Systems (`EditorUi::left`).
- Hierarchy: filter box, New / Duplicate / Delete / Unparent, tree indented by `Parent`, click selects
  (`EditorSelection`, shared with viewport picking; the row of a picked entity is highlighted, the
  list is not scrolled to it). The root of a prefab instance shows `[PrefabName]`.
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
| Editor panels | Editor tab of the Debug menu (its own window, F5) | the panel view `editor_panel` in the editor window (top left, sized to the window, see step 4); the Debug menu keeps its own window and its Editor tab shows only a note and Attach |
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

Known limitations (step 3). The editor camera does not rotate. 3D cameras are not handled (the viewport
is 2D). (The hierarchy scrolling, the handle noise and the fixed layout-B panel size were taken up in step 4.)

## Prefabs and the scene system list (step 4)

Prefabs. A prefab is a file `<prefabs dir>/<Name>.ron` (default `prefabs/`, `EditorPlugin::prefabs_dir`
or `Engine::set_prefab_dir`): a tree of entities written with the scene codec; the first entity is
the root, every entity has a local `id`, a child holds `Parent { entity: <parent's local id> }`. Names are
letters, digits, `_` and `-`. A scene file places one with
`(prefab: Some("Crate"), overrides: { "Transform": { "position": [3.0, 4.0] }, "#2:Sprite": { "color": [..] } })`:
overrides are `component -> field -> value` of the root, and `#<local id>:<component>` for another
entity of the prefab (a component the prefab lacks is added); `name` is the root's name; `id` is the
root's scene id (so other entities can still refer to it). They are validated with the scene: an unknown prefab, component or field, or a value that does not fit gives
an error naming the scene, the entry and the field, and nothing is loaded. A prefab file that is broken is an
error naming the file. An override for a local id the prefab no longer has is ignored with a warning. The
prefab is read again on every load, so editing it changes every placement and the overrides stay.
Prefabs registered with `Engine::register_prefab` (no files, for tests and wasm builds) win over files.
`Engine::load_scene` returns every spawned entity (a placement adds its whole tree, root first).

Instances. Each entity placed from a prefab carries a `PrefabInstance` (the prefab as it was read, and the
entity's local id). Overrides are not stored: they are the fields whose value differs from that
prefab, so any edit (inspector, gizmo) counts as an override, and a field set back to the prefab's value is
not one. Saving (`capture_world_scene`, so the editor's Save) writes a complete instance as one
`prefab:` entry with only the differing fields (and the root's `EditorId`, so the hierarchy order is stable).
An instance that is no longer complete is written as plain entities, losing the link: a member deleted, a
component removed, a child renamed, an entity added under a member, or a non-root member referenced from
outside the instance. Entity references inside an instance are compared in local ids; entity references are
never listed as overrides. Snapshots (Play / Stop) and restart snapshots keep the links.

Editor. The Prefabs tab: a name field with "Save as prefab" (Edit mode; the selected entity and its
children become the prefab, the entity becomes an instance, a new prefab starts at the origin; saving over an existing name rewrites it), and the
list of prefab files; a click places a copy at the centre of the editor camera and selects its root (placing at the cursor is not offered). The
inspector of an instance shows `Prefab: <name> (root)`, "Apply to prefab" (Edit mode; writes the whole instance back, the root's own
transform excepted; other instances in the open world follow: a field that still held the old prefab value takes the new one, overrides stay; instances whose entities or
parents differ between the two versions are left until the scene is loaded again, and the message line counts them) and "Unpack" (breaks the link of
the whole instance). A field that differs from its prefab has a `*` after its name and a "revert" button that
writes the prefab's value back. Every one of these is an undo step: placing, Save as prefab / Apply (restores the old file, or removes a new one, and the links and values other instances took), Unpack, revert. Editing a
prefab through the editor needs its file to be writable; a read-only file or a missing directory gives a message.

Systems. The Systems tab lists every registered system (name; plugin; "global" or "engine" for the read-only
ones: global systems, `propagate_transforms` / `render_extract` and `efeengine*` plugins) with a checkbox for
the active scene's `systems` list. Checking adds the name; unchecking removes it, or, for a system that is only
covered by a `plugin:<name>` entry, replaces that entry by the plugin's other systems. The list is written to
the scene file on Save (with `requires`) and the scene is marked dirty. In Edit and Pause only the list
changes (the game stays frozen; flags are applied the next time the scene is loaded); in Play the enabled flags follow at once, like a
scene switch. Needs a loaded scene (the boxes are disabled without one: Save As of a New scene does not create one). Undoable.

Viewport polish. H / the "Handles (H)" checkbox hide the handles of entities without a sprite (they are
not drawn and not picked; a selected entity keeps its gizmo; `EditorView::handles`). In layout B the panel is 45% of the editor
window (between 420 and 760 pixels, never wider than the window) and as tall as it, docked top left, and follows
a resize of the window (`EditorView::window_size`, `panel::panel_size`).

Tests: `crates/core/tests/prefab.rs` (12 tests: S5.4 two placements with different overrides, trees and `#id:`
overrides, validation errors, propagation, save round trip, incomplete instances, apply, snapshots, registered
prefabs, the scene system list), `crates/ui/src/editor/tests_step4.rs`, `examples/editor_demo/tests/editor.rs`
(the demo's `prefabs/Crate.ron` and `scenes/demo.ron` are kept equal to what the code builds).
Screenshots: `cargo test -p efeengine-ui --features editor,fallback --lib editor_step4_screenshot` with `EFE_DEBUG_MENU_SHOTS=<dir>`
writes `step4_prefabs.png`, `step4_prefab_inspector.png`, `step4_systems.png`.

Known limitations (step 4). The hierarchy is not scrolled to a picked entity: the UI toolkit has no scroll offset or
scroll-to-key API (a `scroll` only has `follow_end`); adding one means a new `NodeKind::Scroll` field handled by the fallback, iced and web
backends. Prefabs do not nest (a prefab file cannot contain a `prefab:` entry; a nested instance inside a captured tree is saved as plain
entities), child names cannot be overridden (a renamed child unpacks on save), components the new prefab version adds are not added to
live instances, and instances edited while the file changes on disk are compared with the version they were placed from. There is no "revert all". A scene
created with New has no `systems` list until it is saved and loaded.

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

Step 4: scenes and prefabs (done)
- [x] Prefabs (`docs/SCENES.md` S5): instantiate, edit, apply / revert overrides, unpack
- [x] Scene system list (S4.3): per-scene enabled systems, global systems read-only
- [x] Polish: handles toggle, layout-B panel sized to its window, `editor_demo` prefab
- [ ] Not done: scroll the hierarchy to a picked entity (needs a UI scroll-to API), hot reload of scripts and mods while in Edit

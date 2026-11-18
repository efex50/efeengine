# Editor: design and roadmap

Status: planned. Nothing of the editor exists yet. Roadmap items: 7.2 (schema export), 7.4 (editor protocol), 7.5 (edit and play modes), 7.6 (hot-patching), 7.7 (seamless restart), and the editor panels in 6.4. Scenes (S1–S3) are done and are the editor's document format; see [`SCENES.md`](SCENES.md).

## Goals

- Edit scenes and components visually, then run the game with the edits.
- Write game logic as normal Rust systems. Edit mode and play mode both work without the editor knowing anything about game types.
- Keep the editor and game separate processes. Rust has no stable ABI, so the editor never loads game code (this is already decided in Phase 7).

Not goals for the first version: a 3D level builder, asset import pipelines, a visual scripting graph, multiplayer editing.

## Architecture

```text
+---------------------+        local socket         +------------------------------+
| editor (iced app)   | <-------------------------> | game process                 |
| panels, undo UI,    |   requests / events / polls  | engine + efeengine-editor-link|
| build runner        |                              | (plugin, debug builds only)  |
+---------------------+                              +------------------------------+
        |  cargo build / run                                  |
        +-----------------------------------------------------+
```

- **Game side: `efeengine-editor-link`**, a plugin. It opens a localhost socket when the game starts with the editor flag, reads requests once per frame at a fixed point, applies them through `Commands` (so changes land at a stage boundary like any other system), and answers polls.
- **Editor side: `efeengine-editor`**, an iced application (Phase 6 decided iced for the editor too). It owns no game state: the game's world is the only source of truth, and the editor keeps a cache of what it last received.
- **Protocol:** length-prefixed messages with `serde` encoding. JSON while we debug (readable in logs), a binary encoding later if profiling says so. Every message carries a protocol version; the handshake refuses a mismatch with a clear error. Entity references use the raw `u64` bits from `entity_to_bits`, so a stale entity fails cleanly.
- **Security:** localhost only, and the link plugin is absent from release builds unless the game opts in. Mods cannot open the socket.

### Modes

The editor uses the scene system's own mechanisms rather than new ones.

| Mode | Systems running | World | On stop or save |
|---|---|---|---|
| **Edit** | only systems marked `global` (rendering, transform propagation, the editor link, gizmos) | editable; edits are real world changes | `capture_scene` → save to the scene file |
| **Play** | everything the scene activated | changes are live; the world moves | `restore(snapshot)` back to the state at Play, unless "keep changes" |

- **Entering edit mode** disables every non-global system with `set_system_enabled` (the switching logic from S2 already does this per scene; edit mode is "all game systems off"). Leaving it re-enables what the active scene lists.
- **Play** takes `snapshot()` (S3), then re-enables the scene's systems. **Stop** calls `restore()`.
- The scene file is the saved document. Only `SceneOwned` entities are written (S2 gap fixed in S3), so editor-spawned helpers must not carry that component.

### Identity across restore

`restore` gives entities new ids (documented in S3). The editor therefore never keeps a raw entity id across a restore. Selection and the hierarchy use the scene's stable entity index (the `SceneEntity` position in the file, or an explicit `EditorId` component that the editor adds when it spawns or first selects an entity). The link maps that id to the live entity on each request.

## Schema and inspectors

The inspector needs to know every component's fields and types without linking game code:

- **7.2 schema export:** the game writes its component schema (names, fields, types, layout) from the type registry, next to the script API index. The editor reads this file at connect time.
- **Editing a field:** the editor sends `set_field(entity, component, field, value)`. The link applies it with `Commands`, using the byte-level `insert_raw` / set path from the registry.
- **Field types:** numbers, `Vec2/Vec3/Vec4`, quaternions, colours, `Entity` references (shown as a picker), strings and enums. **Blocker:** `ScriptType` does not support `bool` fields (found while writing the scene tests). Add `bool` before the inspector ships, or booleans show as read-only.
- **Rust-only components** (`#[derive(Component)]` without `ScriptType`) are not in the schema and are not editable at first.

## Panels

| Panel | Shows | First usable in |
|---|---|---|
| Hierarchy | entities in the scene, with names (`Name` component), parent/child (`Parent`, `Children`) | E1 |
| Inspector | components of the selected entity, schema-driven | E1 read-only, E4 editable |
| Stats | FPS, frame time, entity count, system timings (from 6.4) | E1 |
| Log | the engine logger, filterable | E1 |
| Systems | system names per stage, enabled flags, `global` marker (S2 API) | E2 |
| Scene | current scene, `requires`, `systems`, save and load | E2 |
| Errors | `cargo build` output and game load errors | E5 |
| Script and mods | loaded modules, budgets, traps (6.4) | later |

## Play-mode fidelity: what a snapshot misses

`restore()` covers entities, components, system flags and current-scene ownership. It does **not** cover:

- **Resources** (for example a `Score` resource). A play session that changes a resource then stops will leave it changed. Fix: register resources in the type registry so the snapshot can record them (a new step in phase E3; decide the list of snapshotted resources per game).
- **Events** in flight.
- **Change ticks** (recorded, never rewound; harmless for the editor).
- **Engine state outside the world**: windows, audio, timers. Play mode does not restore them.

The editor shows which of these a session changed, so a user is not surprised.

## Build and run

- The editor runs `cargo build` (with the game's `Cargo.toml`), streams the output to the Errors panel, and starts the game process with the editor flag.
- A **doctor** step checks the Rust toolchain and target setup, like `build.py doctor`.
- Hot-patching (7.6) and seamless restart (7.7) are later phases here, on top of working edit and play.

## Roadmap

Each phase ends with something a user can see. Phases map onto the existing Phase 7 items and 6.4.

### E0: foundations (7.2, prerequisites)

- [x] E0.1 **[S] Schema export.** `EFE_API_EXPORT` also writes the component schema (7.2). _Done when_ `Transform` and a game component appear in the exported file.
  - _Done: `crates/core/src/engine/schema.rs` (format in its docs; `<path>.efschema.json` next to the index), tests in `crates/core/tests/api_index.rs`._
- [x] E0.2 **[S] `bool` in `ScriptType`.** Needed for inspector editing and for scene files with flags.
  - _Done: `ScriptBool` (one-byte Pod wrapper, `Prim::Bool`) and `set_bool`/`field_bool`/checked `set_field` in `crates/core/src/engine/types.rs`; scene writes accept 0/1 in `scene.rs`._
- [x] E0.3 **[S] Stable editor ids.** An `EditorId` component (or use of the scene index), with the rule that editor selection never stores a raw entity id.
  _Done: `EditorId` in crates/core/src/engine/components.rs, `Engine::entity_by_editor_id` (O(n)) and `SceneError::DuplicateEditorId` in crates/core/src/engine/scene.rs; tests in crates/core/tests/snapshot.rs._
- [x] E0.4 **[S] Resource registration for snapshots.** Resources a game opts in to are recorded and restored by `snapshot()` / `restore()`.
  _Done: `Engine::register_snapshot_resource::<T: Clone>`, `Engine::snapshot_gaps`, `Snapshot::{resources, gaps}` in crates/core/src/engine/scene.rs; tests in crates/core/tests/snapshot.rs._
- [ ] E0.5 **Prerequisite: iced platform check (6.1, 6.2, 6.3).** The editor UI is iced. Panels depend on the UI pass and input ownership.
  - _Note (deviation): the editor is a standalone iced application in its own window (`crates/editor`, package `efeengine-editor`, native only), not embedded in the game's wgpu surface, so the 6.2/6.3 prerequisite (UI pass and input ownership inside the game surface) does not apply. This box stays open until the embedded viewport (E7.1) needs it._

### E1: link and read-only viewer (7.4, 6.4 stats/log)

- [x] E1.1 **[N] `efeengine-editor-link` plugin.** Localhost socket, versioned handshake, request drain at a fixed point per frame, `Commands` for every change.
  _Done: `crates/editor-link` (`EditorLink::from_env()`, off unless `EFE_EDITOR=1`, port `EFE_EDITOR_PORT` default 7878, 127.0.0.1 only); drain system in `PostUpdate` at `order::LATE + 100`. Reads only._
- [x] E1.2 **[S] Protocol messages.** Entity list, component values (polled on request), logs, stats; no mutation yet.
  _Done: `crates/editor-link/src/protocol.rs` (messages, framing), `src/handler.rs` (requests, log ring buffer); tests in `crates/editor-link/tests/protocol.rs`. `editor_id` is filled from the `EditorId` component and `components` accepts an editor id (the decimal `EditorId.id`) since E2; the lookup in the handler repeats `Engine::entity_by_editor_id` over `ctx.world` because a system has no `Engine`._
- [x] E1.3 **[N] Editor app skeleton.** Connects, shows Hierarchy, Inspector (read-only), Stats and Log panels.
  _Done: `crates/editor` (`src/main.rs` window and panels; `src/worker.rs` connection thread with automatic reconnect; `src/conn.rs` reconnect state machine; `src/model.rs` formatting and input checks). Run `cargo run -p efeengine-editor -- --port 7878` against a game started with `EFE_EDITOR=1`. Polling: stats about 3 times a second, open entity every 0.5 s, lists and log every second. Not run against `script_demo` in this step (no display); the worker is tested against a fake game and the link tests run over real sockets. Also shows Scene (E2.2) and Systems (E2.3) panels and an Edit/Play switch._
- _Done when_ the editor shows the live entity list, one entity's components and the log of a running `script_demo`, and disconnecting and reconnecting works.

### E2: edit mode and scene save (7.5 edit mode)

- [x] E2.1 **Edit mode.** Disable all non-global systems; re-enable the scene's on exit. Uses `set_global_system` and `set_system_enabled`.
  _Done: request `set_mode` (`crates/editor-link/src/handler.rs`, behaviour documented in `src/protocol.rs`; tests in `crates/editor-link/tests/protocol.rs`). Deviations: (1) it needed a small core addition, `Commands::engine_task` / `EngineCommand::Task` (`crates/core/src/engine/commands.rs`, run in `Engine::apply_commands`) and `Engine::scene_systems()` (`scene.rs`), because a system only has `Ctx`, not `Engine`. (2) Leaving edit mode restores the enabled flags recorded on entry first, then enables the scene's listed systems, so systems that no scene lists are not left disabled; a `load_scene` made in edit mode re-disables non-global systems and updates the recorded flags. The link's own `editor_link_drain` is marked global._
- [x] E2.2 **Save.** Save the scene through `save_scene` (S1); load a scene from the Scene panel (`load_scene`, S2).
  _Done: requests `save_scene` / `load_scene` / `scene` in `crates/editor-link/src/handler.rs` (paths relative to `EditorLink::with_project_dir`, else `EFE_EDITOR_PROJECT`, else the working directory; absolute paths, `..`, empty paths rejected as `bad_path`; engine failures reply `scene_error`); Scene panel in `crates/editor/src/main.rs`. Symlinks inside the project directory are not resolved or checked._
- [x] E2.3 **Systems panel.** Lists systems and their enabled and global flags.
  _Done: request `systems` (`handler.rs`), panel in `crates/editor/src/main.rs`, formatting in `crates/editor/src/model.rs`._
- _Done when_ you can open a scene, see its entities, save it, and reopen it with the same content.

### E3: play and stop (7.5 play mode)

- [x] E3.1 **Play.** `snapshot()` on Play, scene systems on, world live.
  - _Done: `set_mode` in `crates/editor-link/src/handler.rs` (snapshot kept in `LinkState`); tests in `crates/editor-link/tests/protocol.rs`. `script_demo` now adds `EditorLink::from_env()`._
- [x] E3.2 **Stop.** `restore()` on Stop, with the "keep changes" option.
  - _Done: `set_mode edit {keep_changes}` and the strict `stop {keep_changes}` request (`no_snapshot` error when no play session); editor checkbox in `crates/editor/src/main.rs`._
- [x] E3.3 **Fidelity report.** Show what the snapshot does not cover in this game (resources not registered, events, engine state).
  - _Done: `fidelity` request (`crates/editor-link/src/protocol.rs`, `handler.rs`); the editor's Play panel filters engine-internal resources in `crates/editor/src/model.rs` (`game_gaps`)._
- _Done when_ you press play, move an entity, press stop, and get the scene back.

### E4: live editing (7.4 editing, 6.4 inspector)

- [x] E4.1 **Set field.** `set_field` through the link; works in edit and play modes.
  - _Done: `Request::SetField` in `crates/editor-link/src/protocol.rs`, `set_field` / `prim_bytes` in `crates/editor-link/src/handler.rs` (engine task, byte-level write by schema type; errors `bad_value`, `unsupported_type`, `unknown_field`, `unknown_entity`, `readonly`; reply carries the old value). Core helper `Engine::types_and_world_mut`. Tests in `crates/editor-link/tests/protocol.rs`._
- [x] E4.2 **Spawn and despawn** entities from the editor, tagged `SceneOwned` so they are saved.
  - _Done: `Request::Spawn` / `Request::Despawn` and `spawn` / `despawn` in `crates/editor-link/src/handler.rs`; core `Engine::spawn_scene_owned` and `is_scene_owned` in `crates/core/src/engine/scene.rs`. Editor id auto-assigned (max + 1), duplicates rejected, despawn replies `owned` and the components it had. Editor: "Add entity" and "Delete entity" in `crates/editor/src/main.rs`._
- [x] E4.3 **Inspector editing** for every schema type, using the field-type table above.
  - _Done: `crates/editor/src/edit.rs` (schema parse, `input_kind`, `parse_scalar`, `parse_vector`) and `field_row` in `crates/editor/src/main.rs`: number and vector inputs, bool checkbox, entity picker over the Hierarchy, read-only with a note otherwise. Submit is Enter, or the user moving to another input, the selection or a mode button (iced's `text_input` has no blur event at the pinned rev). Server errors show in the Inspector. Strings and enums do not exist in the registry yet._
- [x] E4.4 **Undo and redo** for edits in edit mode (the editor records the old value and sends the inverse).
  - _Done: `inverse`, `UndoStack`, `history_blocked` in `crates/editor/src/edit.rs`; Undo/Redo buttons and Ctrl+Z / Ctrl+Shift+Z (also Ctrl+Y) in `crates/editor/src/main.rs`. Disabled in play mode with the reason shown; cleared when play starts, on scene load and on reconnect. Limit: an entity with no `EditorId` is addressed by raw bits, so its history is only good until it is respawned._
- _Done when_ a value changed in play mode is still set after stop with "keep changes", and an edit in edit mode undoes cleanly.

### E5: build and run (7.5 toolchain)

- [ ] E5.1 **Build runner.** `cargo build` from the editor, errors in the Errors panel.
- [ ] E5.2 **Doctor.** Toolchain and target checks with clear messages.
- [ ] E5.3 **Run.** Start and stop the game process from the editor.
- _Done when_ a broken Rust file shows its error in the editor, and fixing it and rebuilding starts the game.

### E6: fast iteration (7.6, 7.7)

- [ ] E6.1 **Hot-patching (7.6).** Systems run through Subsecond; a system body edit applies in under a second. Native first.
- [ ] E6.2 **Seamless restart (7.7).** For layout changes that cannot be patched: snapshot, rebuild, restart, restore with fields mapped by name and new fields defaulted.
- _Done when_ editing a system body changes the running game in under a second, and adding a field to a component keeps the scene and the camera position.

### E7: viewport and gizmos (later)

- [ ] E7.1 **Viewport.** The game's own window next to the editor at first. Embedding works on Windows and X11 only; Wayland and the web are later.
- [ ] E7.2 **Gizmos.** Move, rotate and scale handles in edit mode, drawn by the game (rendering stays in the game process).
- [ ] E7.3 **Prefab editing** (with S5 from `SCENES.md`).

## Open questions

- **Viewport:** is a separate window acceptable for the first release? Recommendation: yes; embedding waits until the editor is useful.
- **Protocol encoding:** JSON first, then binary. Recommendation: measure before switching.
- **Web play:** the game can run in a browser, but the editor cannot attach to it (no socket). Recommendation: out of scope; web builds run without the editor.
- **Resource snapshots:** which resources does a game opt in to? Recommendation: opt-in per resource in E0.4, so the snapshot cost stays visible.

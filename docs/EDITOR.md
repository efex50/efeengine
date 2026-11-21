# Editor: design and roadmap

> **Deprecated:** the separate editor app (`crates/editor`, `crates/editor-link`) is replaced by the in-game editor, see [`IN_GAME_EDITOR.md`](IN_GAME_EDITOR.md). Kept for the ideas (modes, play-mode fidelity).

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

- **Game side: `efeengine-editor-link`**, a plugin. It listens on a local socket (a Unix domain socket file, or a Windows named pipe) when the game starts with the editor flag, reads requests once per frame at a fixed point, applies them through `Commands` (so changes land at a stage boundary like any other system), and answers polls.
- **Editor side: `efeengine-editor`**, an iced application (Phase 6 decided iced for the editor too). It owns no game state: the game's world is the only source of truth, and the editor keeps a cache of what it last received.
- **Protocol:** length-prefixed messages with `serde` encoding. JSON while we debug (readable in logs), a binary encoding later if profiling says so. Every message carries a protocol version; the handshake refuses a mismatch with a clear error. Entity references use the raw `u64` bits from `entity_to_bits`, so a stale entity fails cleanly.
- **Security:** a local socket, private to the user (no port, not reachable over the network or by other users: the socket file is mode 0600 in a 0700 directory), and the link plugin is absent from release builds unless the game opts in. Mods cannot open the socket.

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
- **Rebuild and restart** (E6.2) snapshots the running game, stops it, builds and starts it again with `EFE_RESTORE_SNAPSHOT` set; see E6.2.
- Hot-patching (7.6) is E6.1.

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
  _Done: `crates/editor-link` (`EditorLink::from_env()`, off unless `EFE_EDITOR=1`; transport changed to a local socket in E10.1); drain system in `PostUpdate` at `order::LATE + 100`. Reads only._
- [x] E1.2 **[S] Protocol messages.** Entity list, component values (polled on request), logs, stats; no mutation yet.
  _Done: `crates/editor-link/src/protocol.rs` (messages, framing), `src/handler.rs` (requests, log ring buffer); tests in `crates/editor-link/tests/protocol.rs`. `editor_id` is filled from the `EditorId` component and `components` accepts an editor id (the decimal `EditorId.id`) since E2; the lookup in the handler repeats `Engine::entity_by_editor_id` over `ctx.world` because a system has no `Engine`._
- [x] E1.3 **[N] Editor app skeleton.** Connects, shows Hierarchy, Inspector (read-only), Stats and Log panels.
  _Done: `crates/editor` (`src/main.rs` window and panels; `src/worker.rs` connection thread with automatic reconnect; `src/conn.rs` reconnect state machine; `src/model.rs` formatting and input checks). Run `cargo run -p efeengine-editor -- --socket <name>` against a game started with `EFE_EDITOR=1 EFE_EDITOR_SOCKET=<name>` (E10.1). Polling: stats about 3 times a second, open entity every 0.5 s, lists and log every second. Not run against `script_demo` in this step (no display); the worker is tested against a fake game and the link tests run over real sockets. Also shows Scene (E2.2) and Systems (E2.3) panels and an Edit/Play switch._
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

- [x] E5.1 **Build runner.** `cargo build` from the editor, errors in the Errors panel.
  - _Done: `crates/editor/src/build.rs` (command lines, `classify`, `location`, `OutputBuffer` bounded to 2000 lines, `BuildState`), `runner.rs` (`run_build` reads stdout and stderr line by line on a worker thread), the Errors panel and header status in `main.rs`, and `cli.rs` (`--package`, `--example`, `--project-dir` inside the repo, existing directories only). Without `--package` Build and Run are disabled with a hint. `crates/editor/tests/build_runner.rs` builds a broken and a good throwaway project. Not wired: `./build.py editor` and `cargo xtask editor` still start the game themselves and do not pass `--package`._
- [x] E5.2 **Doctor.** Toolchain and target checks with clear messages.
  - _Done: `crates/editor/src/doctor.rs` (`run_checks` takes the command runner and file test, `report_lines` and `format_report`, same `ok`/`MISS`/`fix:` layout as `./build.py doctor`); the Doctor button runs it on the worker thread. Checks: cargo, rustc, SDL3 via pkg-config (not on Windows), workspace Cargo.toml. It does not check nightly, wasm or cmake._
- [x] E5.3 **Run.** Start and stop the game process from the editor.
  - _Done: `GameHolder` in `crates/editor/src/runner.rs` (kills the child on Drop), Run builds first and starts `target/<profile>/<package>` directly with `EFE_EDITOR=1` and `EFE_EDITOR_SOCKET`; Stop also cancels a build. A game that exits on its own shows its status in the header. Only checked with stand-in processes in unit tests, not with a real game window._
- _Done when_ a broken Rust file shows its error in the editor, and fixing it and rebuilding starts the game.

### E6: fast iteration (7.6, 7.7)

- [ ] E6.1 **Hot-patching (7.6).** Systems run through Subsecond; a system body edit applies in under a second. Native first.
  - _Spike result (2026-11-18), not done, left unticked._ Tried, in a scratch project outside the repo: `subsecond` 0.7.10 and `dioxus-devtools` 0.7.10 (`serve` feature) fetch and compile on the stable and nightly toolchains here (7 s and 43 s); `dx` (`cargo install dioxus-cli --locked`, dioxus-cli 0.7.10) installs from crates.io (about 10 min on 8 cores). Minimal wiring: the game calls `dioxus_devtools::connect_subsecond()` once and wraps code in `subsecond::call(|| ...)`; the dev side is `dx serve --hot-patch --platform linux --package <p> --bin <p> --features <f> --interactive false`, which owns the build (it captures the rustc and linker arguments of a "fat" first build, then builds thin patch objects and sends a jump table over a websocket). The linker capture needs the tip crate to be rebuilt by dx, so touch it first.
    - _Worked:_ a toy binary with `subsecond::call(|| println!("msg = alpha"))` in a loop printed `beta` after editing the string; `dx` reported the patch build at 96 to 136 ms and the new text appeared within the 1 s polling step of my check (edit to applied: about 1 s at most, not measured finer). `script_demo` (`SDL3` window on this Wayland session, real wgpu device) built and ran under `dx serve --hot-patch` after the touch, and `dx` reported a patch in 2.1 s.
    - _Did not work:_ in the engine, a patched system body did not change the running game: after editing the format string inside a `subsecond::call` closure wrapped around the body of `log_progress` the old text kept printing for 30 s. The same happened in the toy when the `call` closure only called a free function (`subsecond::call(|| tick(n))`, with `tick` and the function it calls edited). Cause not found. Suspects, not checked: `HotFn` reaches a closure that captures state only through the jump-table entry of `HotFunction::call_it::<F>`, which may not be re-emitted in the patch for engine closures (captured `ctx`, `&mut` state); `script_demo` is a `cdylib`+`rlib` library with a separate bin, and dx's workspace-crate replay handled that crate (the log showed a patch built from `lib.rs`) but the patch may not contain the closure; or the system runs where the jump table is not consulted.
    - _Why it stays open:_ engine systems are boxed closures called through the schedule, so patching them needs the call wrapper inside the user's system body (a `#[hot]`-style macro or a `hotpatch::call(|| ..)` convention), a `hotpatch` cargo feature that pulls `subsecond` and `dioxus-devtools` into `efeengine-core` (and a large dependency tree into `Cargo.lock`), and a decision on who owns the build: `dx serve` replaces the editor's own build runner (E5) for the hot loop. I removed my wiring (feature, wrapper, `script_demo` change) because it was not shown to work. Estimate: one to two days to find why the engine closure is not patched (read `subsecond` `HotFn::try_call` and dx's patch symbol selection, test closures with captured state in the toy), then half a day for the feature, a `hotpatch::call` helper and the editor launching `dx serve` instead of `cargo build` in the hot loop; wasm is out of scope (subsecond on wasm needs `dx`'s own wasm build). Scratch projects and the `dx` binary were in the session scratchpad and are not kept.
- [x] E6.2 **Seamless restart (7.7).** For layout changes that cannot be patched: snapshot, rebuild, restart, restore with fields mapped by name and new fields defaulted.
  - _Done: the game writes its world on a `restart_snapshot {path}` request (`Engine::restart_snapshot`, RON with entity ids and scene ownership, under the project directory, default `target/efe-restart.ron`). The editor's "Rebuild and restart" button (`crates/editor/src/restart.rs` state machine: Idle, Snapshotting, Restarting) requests it, then the runner stops the game, builds and starts the binary with `EFE_RESTORE_SNAPSHOT=<absolute path>` (`runner::Handle::restart`, `build::with_restore`). `EditorLink::from_env` reads that variable and restores on the game's second frame (after its own setup) with `Engine::restore_lenient` (`crates/core/src/engine/scene.rs`), a separate tolerant path; `load_scene` and `restore` keep the strict decoder. Lenient rules: a field the type no longer has is dropped with a warning; a field missing from the snapshot is zero with a warning (the registry holds no per-field defaults, so "default" means zero and a new `scale` field starts at 0, not 1: give such fields a meaningful zero or set them in the game's setup); a value that does not fit the field's type is zero with a warning; a component the game no longer registers is skipped with a warning and the entity keeps its others; references between entities are remapped; the active scene's entities become owned by the scene the new run loaded. Enum or variant names do not exist in the registry (only numbers and bools), so there is nothing to map there. System enabled flags, resources, the editor mode and events are not carried over: the new run's setup decides them. A snapshot taken in play mode restores the play state. If the build fails the snapshot file is kept and the button retries with it. Warnings go to the game log (visible in the editor's log panel)._
  - _Verified: `crates/core/tests/restart_restore.rs` (7 tests: missing field zero, unknown field dropped, unknown component skipped, camera-like entity keeps position, rotation and scale, entity references remapped, active-scene ownership, wrong-typed value, bad format leaves the world, strict `load_scene` still rejects unknown fields, RON round trip); `crates/editor-link/tests/protocol.rs::restart_snapshot_is_written_and_restored_on_the_next_start` (a headless engine writes the file over the socket, a second engine with `with_restore` restores it on frame 2 and logs the dropped field); `restart.rs` state-machine unit tests; `runner.rs` test that a restart kills the old process, builds and starts the new one with `EFE_RESTORE_SNAPSHOT` set (shell stand-ins). Not verified: a real game window and a real rebuild with a changed component from the editor GUI, the iced button itself, Windows._
- _Done when_ editing a system body changes the running game in under a second, and adding a field to a component keeps the scene and the camera position.

### E7: viewport and gizmos (later)

- [x] E7.1 **Viewport.** The game's own window next to the editor at first. Embedding works on Windows and X11 only; Wayland and the web are later.
  - _Done: separate window, not embedded. `crates/editor/src/place.rs`: pure `game_rect` (right of the editor, else left, else clamped to the screen edge; shrinks to the screen), `session_kind`, `place_game` / `place_with_retry` (X11 through `xdotool`; Windows through PowerShell and user32; Wayland or no display returns "placement not supported on this session"). The editor starts the game with `EFE_WINDOW_TITLE=efe-game-<editor pid>`; the link sets that as the game's main window title on its first frame (`crates/editor-link/src/lib.rs`), and after `GameStarted` the editor retries placement on a background thread for 10 s and shows the result as a notice. Settings panel checkbox "Place game window next to editor" (default on); every failure is a notice, never a crash. Verified: unit tests for the placement math, session detection and xdotool output parsing. NOT verified: no real window was ever moved. This machine is a Wayland session without `xdotool`, so the X11 path and the Windows PowerShell script have not been run at all, and the Wayland path was only checked through `session_kind`. Placement happens once per game start (also after Rebuild and restart); the game does not follow the editor window when it is moved afterwards, and window decorations are not accounted for._
- [x] E7.2 **Gizmos.** Move, rotate and scale handles in edit mode, drawn by the game (rendering stays in the game process).
  - _Done: protocol version 2 adds `select {entity|editor_id, gizmo}` and `clear_selection` (reply `selected`); the link stores the result in the `Selection` resource (`crates/editor-link/src/gizmo.rs`). In edit mode the link's per-frame system pushes the handles of the selected entity (needs a `Transform`; the world pose comes from `GlobalTransform` when present) into the engine's existing `DebugDraw2D`: arrows along the world axes for Move, a ring plus a tick for Rotate, axis lines with square ends for Scale, sized 80 screen pixels using the 2D camera zoom. Editor: a Gizmo panel (`crates/editor/src/gizmo.rs`, `main.rs`) with the Move / Rotate / Scale mode, the numeric fields of the mode's `Transform` field (X, Y; a third axis would show as Z, but `Transform` is 2D today) and nudge buttons (-1, -0.1, +0.1, +1) that send `set_field` through the undo stack: one press is one `set_field` and one undo step. Selecting an entity in the Hierarchy selects it in the game too (also after reconnect). Verified: protocol round-trip and wire-shape tests, an end-to-end test that `select` stores the entity and a failed select keeps the old one, unit tests of the handle geometry and of drawing only for a live entity with a `Transform`, unit tests of the nudge arithmetic and that each nudge is one undo entry. NOT verified: nobody ran the game with the editor and looked at the handles (no screenshot was taken), so colours, size and position on screen are unchecked. NOT done: dragging the handles with the mouse in the game window. The game window owns the mouse and keyboard, and forwarding input from the editor needs the input ownership work this document defers; until then the numeric fields and nudge buttons are the way to move, rotate and scale. 3D gizmos do not exist (`DebugDraw2D` is 2D)._
- [ ] E7.3 **Prefab editing** (with S5 from `SCENES.md`).
  - _Note: waits for S5 (prefabs) in `SCENES.md`, which is not built. Not started._

## Projects and pages (E8, E9)

The editor opens on a **home page** and works on one **project** at a time, the way an IDE does. A project is a folder with a `.efproj` file (design in `PROJECT_FILE.md`). Pages are addressed by path, like web pages:

| Path | Page | Purpose |
|---|---|---|
| `/home` | Home | recent projects, New, Open, Settings |
| `/project/new` | New project | create a project folder from a template |
| `/project/open` | Open project | pick a `.efproj` by path or from recents |
| `/project/delete` | Delete project | remove a project folder after typed confirmation |
| `/settings` | Settings | editor settings, stored per user |
| `/editor` | Editor | the current panels (E0–E7), bound to the open project |

Rules:
- `/editor` needs an open project; without one the editor goes to `/home`.
- Build, Run and Restart use the project's own package and paths, not command-line flags. The `--package` / `--example` flags keep working as a shortcut that opens a project for that game.
- Recent projects and settings live in the user config directory, not in the project folder.

### E8: project file

- [x] E8.1 **[M] Format and parser.** `.efproj` XML (design: `PROJECT_FILE.md`) parsed and written by a new crate `crates/project` (`efeengine-project`); unknown elements are errors with the line number; a round-trip test. _Done:_ `crates/project` parses and writes `.efproj`; tests cover the canonical file round-tripping byte for byte, and each parse error (wrong or missing Sdk, missing Name, unknown element with line number, unknown attribute, absolute path, `..`, EngineVersion 2).
- [x] E8.2 **[S] Scaffold.** Creating a project writes `<name>.efproj`, a `Cargo.toml` with a git dependency on the engine (pinned to a commit), `src/main.rs`, `scenes/start.ron`, and nothing else. The engine is fetched from GitHub by cargo. _Done:_ `create()` writes the four files, refuses a non-empty folder, rejects bad names; tests check the files and the project passing `check`; an ignored test creates a project and runs `cargo check` on it against this checkout (passes). The scene is checked as format-1 RON text only (loading needs the engine). The settings page that supplies the engine path is E9.6. Changed in E9.8: `create(parent, name)` takes no engine path; the pin comes from `crates/project/build.rs` (`git rev-parse HEAD`, else `branch = "main"`). The commit must be pushed to GitHub: uncommitted local engine changes are not in new projects. The ignored `cargo check` test was removed.
- [x] E8.3 **[S] Validation.** `efeengine-project::check` reports missing package, missing start scene, scene or mod files that do not exist, and a `Cargo.toml` package that does not match. _Done:_ `check()` returns every issue with its field; tests cover package mismatch, missing start scene, missing scene file, missing mod file, missing `Cargo.toml`, a bad or missing `.efproj`, and a valid project with none.
- _Done when_ a project can be created on disk, read back, and checked with a clear message for each broken field.

### E9: pages

- [x] E9.1 **[M] Router.** Pages are an enum with a path; `/home` is the start page; navigation is a message, not a window; back and forward keep history. _Done:_ `crates/editor/src/route.rs`: `Route` with `path()` / `parse()` (unknown path gives Home), `resolve` (`/editor` without a project or `--package` goes to Home; delete without a folder goes to Home) and `Nav` history (navigate, back, forward; a new page drops forward history; a back/forward entry is redirected again). Navigation is an iced `Message::Navigate`/`Back`/`Forward`, no window. Verified: unit tests for the parse/path round trip, the redirect rule, back/forward and redirect of a history entry. NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked.
- [x] E9.2 **[S] Home page.** Recent projects (open or remove from the list), buttons New, Open, Settings. _Done:_ Home page in `main.rs`: recent list (project name read from the `.efproj`, path), Open, Remove (list only, never files), Delete (to `/project/delete`), missing projects show "missing" and cannot be opened; buttons New, Open, Settings. Verified: the recent list logic (cap, order, removal) by config tests, name reading by `projects` tests. NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked.
- [x] E9.3 **[M] New project page.** Name, parent folder, template (the only template is the default). Creates the project (E8.2) and opens it. _Done:_ New project page: name, parent folder (default current directory), engine path from settings (unset: message and Create disabled). Create calls `efeengine_project::create`, loads and opens the project, goes to `/editor`; errors show on the page. Template choice is not shown (only one template exists). Verified: `create` and `load_project` by their tests (E8, `projects::tests`); the page's own message handling is not unit tested. NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked. (E9.8: the parent folder and engine path fields were removed; projects go to the projects folder.)
- [x] E9.4 **[S] Open project page.** Path field with a browse button, and the recent list. Opens after validation (E8.3); a broken project shows its messages and does not open. _Done:_ Open page: path field takes a `.efproj` or a folder (`efeengine_project::find`), runs `check_project`, lists every issue and does not open on any; on success adds to recents (saved), opens, goes to `/editor`. Recents are buttons that fill the field. No browse button (`rfd` is not a dependency): plain text field. Verified: `projects::tests::open_resolves_file_or_folder_and_checks` (file, folder, empty, missing, non-efproj, broken project). NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked. (E9.8: the path field was removed; the page lists the projects of the projects folder.)
- [x] E9.5 **[S] Delete project page.** Shows the project folder path and its size, requires typing the project name, refuses if the game is running, and deletes the whole folder. It deletes only folders that contain a valid `.efproj`. _Done:_ Delete page: folder, size in bytes (walk capped at 100 000 entries, says so), typed exact project name enables Delete. `projects::delete_project` refuses while any game or build runs in this editor session (it cannot tell which project a game belongs to), refuses symlinks, a folder without its own valid `.efproj`, a wrong name; then `remove_dir_all` on that folder only; the project leaves recents and the editor returns to `/home`. Verified: unit tests for no name, wrong name, game running, no `.efproj` (also one only in a parent), invalid `.efproj`, symlink (unix), success leaving a sibling, and the size cap; all in temp dirs. NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked. (E9.8: the folder path is no longer shown; the page shows the name and size.)
- [x] E9.6 **[S] Settings page.** Engine path, place game window (E7.1), cargo profile default. Stored in the user config file; a bad config file is reported and replaced with defaults. _Done:_ `config.rs`: JSON in `$XDG_CONFIG_HOME/efeengine-editor/config.json` (or `~/.config`, `%APPDATA%`); fields engine_path, place_game_window, cargo_release, recent (max 10). Missing file gives defaults; a bad file is kept as `config.json.bad`, defaults are written and a notice shown; save errors are notices. Settings page with plain checkboxes (no `rfd`). Verified: tests for missing file, bad file (5 shapes), round trip, save failure, recent cap and order, config dir rules. NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked. (E9.8: the engine path setting was removed; an old `engine_path` in the file is ignored; E10.1 removed the port setting, an old `default_port` is ignored the same way.)
- [x] E9.7 **[M] Editor page.** The existing panels move under `/editor` and take the open project's paths and package. _Done:_ The panels live under `Route::Editor`, with a header bar (current path, Home, back, forward). Build, Run, Restart and Doctor use the open project's package and folder; `EFE_START_SCENE` (the project's StartScene) is set for the game, but the game side does not read it yet. `--package`/`--example`/`--project-dir` still work: the editor starts on `/editor`, and uses the project found from `--project-dir` only if its package matches, otherwise runs without a project as before. Opening another project while a game or build runs is refused. Verified: builds, clippy, existing editor tests pass. NOT verified: no GUI run; the editor was never started, so no page was looked at on screen and the iced layout (spacing, widths, the 720 px page column, colours) is unchecked.
- [x] E9.8 **[M] Internal paths and toolchain gate.** The engine location and the projects folder are not user settings and no path is shown in the UI. Build, Run and Rebuild and restart first check that the Rust toolchain runs. _Done:_ the engine is a git dependency (E8.2), so there is no engine path; `config.projects_dir` (default `<home>/efeengine-projects`, `%USERPROFILE%` on Windows, created at the first project) holds the projects. New project asks for a name only. Open project lists the sub-folders of the projects folder that hold a `.efproj` (`projects::list_projects`); a broken one shows "needs attention" and its issues on Details. Home, Recent and Delete show names; Delete shows the name and a KB/MB size and keeps all its checks. `doctor::toolchain_gate` (cargo and rustc; rustup is only the installer) runs on the runner thread before a build; if missing, nothing is built and the editor shows "Rust is not installed. Install it from rustup.rs, then press Build again." plus the note that the first build downloads the engine from GitHub (network needed), with an "Open rustup.rs" button (`xdg-open`, `open`, `cmd /C start`; failure is a notice). Nothing is downloaded or installed. Build and Run still use the project's package and folder as the cargo directory; the folder is not shown. `--project-dir` still works. Tests: config without `engine_path` loads clean, project listing, gate messages, delete refuses without the typed name. Not checked: a run of the GUI, and the git fetch of a scaffolded project (no network use).
- _Done when_ a user can create a project, close the editor, reopen it from recents, change a setting, and delete a project, each with its page.

### E10: transport

- [x] E10.1 **Local socket bridge.** The editor-game link uses a local socket instead of a localhost TCP port: no port to clash with other programs, and not reachable by other local users.
  - _Done: `crates/editor-link/src/socket.rs` on the `interprocess` crate (2.4.7, published 2026-10-23, pinned with `=`): a Unix domain socket file on Linux and macOS, a named pipe `\\.\pipe\<name>` on Windows. The editor picks the name (`efe-editor-<pid>`, or `--socket <name>`) and passes it to the game in `EFE_EDITOR_SOCKET` (`EditorLink::new(name)`, `from_env`); the game listens, the editor connects and keeps its reconnect backoff. Unix socket files live in `$XDG_RUNTIME_DIR/efeengine/`, else `<temp dir>/efeengine-<uid>/` (directory 0700, socket file 0600); a stale file is removed before binding, a live one gives "in use". A name containing `/` is used as a file path (tests). If binding fails the game logs the error and runs without the link. `PROTOCOL_VERSION` is 3 (framing and messages unchanged). Removed: TCP, `--port`, `EFE_EDITOR_PORT`, `parse_port`, `default_port` and the Settings port field (an old `default_port` in the config file loads without a notice); `build.py editor` and `cargo xtask editor` use a socket name from their own process id. Verified: the editor-link protocol tests run over real Unix sockets (unique name per test), plus new tests for two servers with different names not interfering, a live name refused, a stale socket file replaced, file mode 0600 and directory mode 0700; the editor worker is tested against a fake game over a socket. Also run by hand: `script_demo` with `EFE_EDITOR=1 EFE_EDITOR_SOCKET=<name>` on this Wayland session created the socket (mode 0600) and answered `hello` (protocol 3) and `stats` to a small Python client over the socket. NOT verified: the editor window was not started against the game, so the editor-to-real-game handshake is untested (the worker is tested only against a fake game); no Windows run (the named pipe path compiles only in theory here, and its timeouts are emulated with nonblocking reads); macOS not run. Unix socket paths are limited to about 104 to 108 bytes._
- _Done when_ a game and the editor connect with no TCP port open.

## Open questions

- **Viewport:** is a separate window acceptable for the first release? Recommendation: yes; embedding waits until the editor is useful.
- **Protocol encoding:** JSON first, then binary. Recommendation: measure before switching.
- **Web play:** the game can run in a browser, but the editor cannot attach to it (no socket). Recommendation: out of scope; web builds run without the editor.
- **Resource snapshots:** which resources does a game opt in to? Recommendation: opt-in per resource in E0.4, so the snapshot cost stays visible.

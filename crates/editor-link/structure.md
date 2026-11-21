# efeengine-editor-link

The in-game localhost IPC plugin and wire protocol (design: `docs/EDITOR.md`, phases E1, E2, E4, E6.2, E7.2). Lets an external editor inspect a running game, switch between Edit and Play modes, save/load scenes, edit component fields, spawn/despawn entities, take restart snapshots, and select entities for in-game gizmo drawing. Disabled unless `EFE_EDITOR=1` and `EFE_EDITOR_SOCKET` are set.

## At a glance
| | |
|---|---|
| Path | `crates/editor-link` (package `efeengine-editor-link`) |
| Kind | library (rlib) |
| Depends on (workspace) | `efeengine-core` (`crates/core`: `Engine`, `Plugin`, `Ctx`, `World`, `Resources`, `TypeRegistry`, `DebugDraw2D`, `Logger`, `RestartSnapshot`) |
| External deps | `serde` (derive), `serde_json`; `cfg(not(target_family = "wasm"))`: `interprocess` (`=2.4.7`); `cfg(unix)`: `libc` |
| Used by | `efeengine-editor` (`crates/editor`), `examples/script_demo` |
| Targets | native (local socket listener and drain system) and `wasm32-unknown-unknown` (`EditorLink::build` is a no-op; `socket` is native-only) |
| Tests | unit tests in `src/` (4: `gizmo.rs` (3: gizmo geometry and `DebugDraw2D` drawing) and `handler.rs` (1: `resolve_project_path`)); `tests/protocol.rs` (22: message round-trips, handshake, `FrameReader`, live socket requests against `Engine`, Unix socket permissions/naming) |

## Layout
```text
crates/editor-link
├── Cargo.toml
├── structure.md
├── src
│   ├── lib.rs        `EditorLink` plugin (`new`, `from_env`, `off`, `with_project_dir`, `with_restore`), `handshake`, native listener/accept/serve threads, per-frame `PostUpdate` drain system (`editor_link_drain`) and `restore_snapshot`
│   ├── protocol.rs   `PROTOCOL_VERSION` (`3`), `MAX_FRAME` (1 MiB), `ClientMessage`, `ServerMessage`, `Request`, `Reply`, `GizmoMode`, `Mode`, info structs, `write_frame`/`read_frame`/`send`/`recv`, incremental `FrameReader`
│   ├── socket.rs     `#[cfg(not(target_family = "wasm"))]` local IPC (`new_name`, `listen`, `accept`, `connect`, `Conn` with nonblocking read/write timeout wrappers, Unix domain sockets in `$XDG_RUNTIME_DIR/efeengine/` or `<tmp>/efeengine-<uid>/` with `0700`/`0600` modes, Windows named pipes)
│   ├── handler.rs    `handle` (immediate vs `ctx.commands.engine_task` dispatch), `LinkState`, `RingLogger` (1000-line ring buffer) + `recent_logs`, `resolve_project_path`, `NOT_COVERED`, schema/field JSON-to-bytes conversion, edit/mode/scene/snapshot handlers
│   └── gizmo.rs      `Selection` resource, `HANDLE_PIXELS`, `Part`, `parts` (Move/Rotate/Scale geometry), `draw_selection` (`DebugDraw2D` in edit mode, scaled by active orthographic `Camera` zoom)
└── tests
    └── protocol.rs   wire serialization, `FrameReader`, and end-to-end socket + `Engine` tests
```

## Architecture & protocol
- **Framing & handshake (`protocol.rs`, `lib.rs`)**: Length-prefixed (`u32` big-endian, max 1 MiB) UTF-8 JSON frames. Clients must open with `ClientMessage::Hello { protocol: PROTOCOL_VERSION, client }`; mismatches or non-hello first messages receive `ServerMessage::Error` and close.
- **Threading & frame scheduling (`lib.rs`)**: On native, `EditorLink` installs `RingLogger` over `engine.logger` and the global logger, spawns an accept thread (up to `MAX_CLIENTS = 4`) and per-connection reader threads pushing into a bounded `sync_channel` (`QUEUE = 256`). A global system (`editor_link_drain`, `Stage::PostUpdate`, `order::LATE + 100`) drains requests once per frame, updates `EFE_WINDOW_TITLE` if set, optionally restores `EFE_RESTORE_SNAPSHOT` on frame 2 via `Engine::restore_lenient`, and draws gizmos via `gizmo::draw_selection` while in `Mode::Edit`.
- **Requests & modes (`protocol.rs`, `handler.rs`)**:
  - Immediate (`handle_now`): `Stats`, `Schema`, `Logs`, `Entities`, `Components`.
  - Deferred (`engine_task`): `Systems`, `Scene`, `SetMode` / `Stop` (with `Engine::snapshot`/`restore`), `Fidelity`, `SaveScene` / `LoadScene` / `RestartSnapshot` (paths validated by `resolve_project_path`), `SetField`, `Spawn`, `Despawn`.
  - Gizmo selection (`Select`, `ClearSelection`): updates the `Selection` resource directly in `handle`.

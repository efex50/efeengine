# efeengine-editor

Standalone native `iced` editor application (`efe-editor`) for efeengine projects and running games (design: `docs/EDITOR.md`, phases E1 to E7.2 and E9). Owns no game state itself; communicates with the game process over `efeengine-editor-link` and manages project lifecycle via `efeengine-project` and `cargo` subprocesses. Pure state machines and helpers live in the library target so they are tested without a window, while `main.rs` holds the `iced` UI.

## At a glance
| | |
|---|---|
| Path | `crates/editor` (package `efeengine-editor`, lib `efeengine_editor`, bin `efe-editor`) |
| Kind | library (`rlib`) + binary (`efe-editor`) |
| Depends on (workspace) | `efeengine-editor-link` (`crates/editor-link`), `efeengine-project` (`crates/project`) |
| External deps | `serde_json`, `iced` (git rev `e5c350f2`, feature `tokio`) |
| Used by | `xtask` (`editor_session` builds and launches `efe-editor`) |
| Targets | native only |
| Tests | unit tests in `src/` (81 across 14 modules, including 6 Unix-only tests in `runner.rs`); `tests/build_runner.rs` (2: offline `cargo build` against throwaway projects for diagnostics and success) |

## Layout
```text
crates/editor
├── Cargo.toml
├── structure.md
├── src
│   ├── lib.rs        re-exports all 14 pure/background modules tested without an `iced` window
│   ├── main.rs       `efe-editor` `iced::application` (`Editor`, `Message`, `Pages`, multi-page router UI, hierarchy, inspector, gizmo nudge, logs, systems, errors, build/run/restart controls)
│   ├── cli.rs        `Cli`, `parse_cli`, `workspace_root`, `resolve_project_dir` (`--socket`, `--package`, `--example`, `--project-dir` constrained inside workspace root)
│   ├── config.rs     `Config` (`projects_dir`, `place_game_window`, `cargo_release`, `recent` capped at `MAX_RECENT = 10`), JSON load/save with `.bad` backup recovery, XDG/Windows path helpers
│   ├── route.rs      `Route` (`Home`, `NewProject`, `OpenProject`, `DeleteProject`, `Settings`, `Editor`), `resolve`, and browser-style `Nav` history stack
│   ├── projects.rs   `OpenProject`, `Listed`, `load_project`, `list_projects`, `dir_size`, `human_size`, `delete_project` (safety checks over `.efproj` and symlinks)
│   ├── build.rs      `CommandSpec`, `build_command`, `target_dir`, `game_binary`, `run_command`, `with_restore`, `OutputBuffer` (`MAX_LINES = 2000`), rustc diagnostic/location parsing, `BuildStatus`/`RunStatus`/`BuildState`
│   ├── doctor.rs     `CheckResult`, `Gate`, `run_checks`, `toolchain_gate`, `gate_message`, `report_lines`/`format_report`, `open_url`, `system_exec`, `doctor`
│   ├── runner.rs     background build/run/doctor thread (`Handle`, `spawn`, `run_build`, `GameHolder` killing child processes on stop/drop)
│   ├── restart.rs    `Phase`, `Action`, `Restart` state machine for seamless rebuild-and-restart via `SNAPSHOT_PATH` (`"target/efe-restart.ron"`)
│   ├── conn.rs       `ConnState` (`Connecting`, `Connected`, `Retrying`) and exponential `backoff` (0.5 s to 5 s)
│   ├── worker.rs     background editor-link client thread (`Handle`, `Event`, `spawn`, `dedupe` merging identical read requests while preserving mutations)
│   ├── model.rs      pure UI formatting and validation (`entity_label`, `sort_entities`, `format_value`, `component_rows`, `system_sections`, `log_label`, `log_visible`, `game_gaps`, `fidelity_lines`, `validate_scene_path`)
│   ├── edit.rs       schema parsing (`parse_schema`, `input_kind`), typed scalar/vector parsing (`parse_scalar`, `parse_vector`, `edit_texts`), `Target`/`Op`/`Entry`, `inverse`, bounded `UndoStack` (`MAX_HISTORY = 200`), `is_mutation`
│   ├── gizmo.rs      `MODES`, `STEPS`, `label`, `field`, `axes`, and rounded `nudge` arithmetic for `Transform` edits
│   └── place.rs      `Rect`, `Session`, `game_rect`, `session_kind`, `place_game`/`place_with_retry` (X11 via `xdotool`, Windows via PowerShell/user32; Wayland returns `UNSUPPORTED`)
└── tests
    └── build_runner.rs  end-to-end `run_build` + `BuildState` tests with real `cargo` invocations on temp crates
```

## Architecture
- **Background threads (`worker.rs`, `runner.rs`, `place.rs`)**: `main.rs` polls `worker::Handle` (socket connection + request/reply queue to the running game) and `runner::Handle` (Cargo builds, child game process ownership via `GameHolder`, and `doctor` checks) on an `iced::time` tick, plus one-shot placement threads (`place_with_retry`) when `place_game` is enabled.
- **Pure logic boundary (`lib.rs`)**: CLI parsing, user config, page routing, project management, build/restart state machines, connection backoff, inspector schema/input parsing, undo/redo inverses, gizmo nudging, and window placement geometry are isolated from `iced` widgets and covered by unit tests.

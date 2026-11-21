# efeengine-project

The `.efproj` project file library (design: `docs/PROJECT_FILE.md`). Handles XML parsing and canonical writing, ancestor search, validation against `Cargo.toml` and scene/mod files, and scaffolding new game project directories with no dependency on the engine or the editor.

## At a glance
| | |
|---|---|
| Path | `crates/project` (package `efeengine-project`) |
| Kind | library (rlib) |
| Depends on (workspace) | nothing |
| External deps | `quick-xml` (strict XML parsing), `toml` (`Cargo.toml` package name validation) |
| Used by | `efeengine-editor` (`crates/editor`) |
| Targets | native (uses `std::fs` and `std::env`) |
| Tests | unit test in `src/create.rs` (1: engine git selector formatting); `tests/project.rs` (19: canonical round-trip, XML/path/version validation, `check`, `create`, `find`) |

## Layout
```text
crates/project
├── Cargo.toml
├── build.rs          records workspace `git rev-parse HEAD` (or `EFE_ENGINE_REV` override) as compile-time `EFE_ENGINE_REV`
├── structure.md
├── src
│   ├── lib.rs        `SDK` (`"efeengine/1"`), `ENGINE_VERSION` (`1`), `Project`, `ProjectError`, `find`, `path_problem`, re-exports
│   ├── format.rs     `parse` (strict XML parser with 1-based line errors and path checks) and `write` (canonical 2-space XML formatter)
│   ├── check.rs      `CheckIssue`, `check` (find and parse `*.efproj` in a folder), `check_project` (verify `Cargo.toml` `[package].name`, `StartScene`, `Scene` and `Mod` files)
│   └── create.rs     `ENGINE_GIT_URL` and `create` (scaffolds `<name>.efproj`, `Cargo.toml`, `src/main.rs`, `scenes/start.ron`)
└── tests
    └── project.rs    integration tests for `parse`, `write`, `check`, `create`, and `find`
```

## Public API & behavior
- **Format (`format.rs`, `lib.rs`)**: `Project { name, package, example, start_scene, engine_version, scenes, mods }`. `parse` enforces `Sdk="efeengine/1"`, `EngineVersion == 1`, rejects unknown elements/attributes/entities/CDATA, rejects duplicate or empty required properties (`Name`, `Package`, `StartScene`), and checks `StartScene`, `<Scene Include>`, and `<Mod Include>` with `path_problem` (relative paths staying inside the project directory, no `..`, leading `/` or `\`, or Windows drive prefixes). `write` emits canonical UTF-8 XML with fixed element ordering.
- **Discovery & validation (`lib.rs`, `check.rs`)**: `find(from)` walks up ancestor directories returning the first sorted `*.efproj` file. `check(dir)` / `check_project(project, dir)` collect all `CheckIssue`s across `Cargo.toml` (`[package].name` must match `project.package`), `StartScene` membership in `scenes`, and existence of listed `scenes` and `mods` files on disk.
- **Scaffolding (`create.rs`, `build.rs`)**: `create(parent, name)` validates project names (`[a-z][a-z0-9_]{0,63}`), refuses existing non-empty directories (`ProjectError::NotEmpty`), and writes `<name>.efproj`, `Cargo.toml` (depending on `efeengine-core` from `ENGINE_GIT_URL` pinned to `EFE_ENGINE_REV` at runtime/compile time or `branch = "main"`), `src/main.rs`, and `scenes/start.ron` (format 1).

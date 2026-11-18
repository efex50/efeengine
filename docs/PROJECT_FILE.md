# Project file (`.efproj`): design

Status: designed, not built. Roadmap items E8.1–E8.3 in `EDITOR.md`.

## Why an XML project file

The format follows the SDK-style `.csproj` idea: one small file names what the project is and what it contains, and everything else is derived from it. A project is a folder; the `.efproj` is its entry point.

- Readable and diffable in git.
- Lists are explicit (`Include` items), so a file that is not listed is not part of the project.
- XML so it can be extended with new items and properties without breaking old readers, which skip unknown properties only where the schema marks them as optional.

It is not Cargo's format. The Rust crate stays a normal `Cargo.toml`; the `.efproj` says which game, scenes and mods belong to the editor's project.

## Layout on disk

```text
coin_dash/
  coin_dash.efproj        the project file (entry point)
  Cargo.toml              the game package (git dependency on the engine)
  src/main.rs             the game
  scenes/start.ron        scene files listed in the project
```

## Format

```xml
<?xml version="1.0" encoding="utf-8"?>
<Project Sdk="efeengine/1">
  <PropertyGroup>
    <Name>coin_dash</Name>
    <Package>coin_dash</Package>
    <Example></Example>
    <StartScene>scenes/start.ron</StartScene>
    <EngineVersion>1</EngineVersion>
  </PropertyGroup>

  <ItemGroup>
    <Scene Include="scenes/start.ron" />
    <Scene Include="scenes/level_1.ron" />
    <Mod Include="mods/inventory.efmod" />
  </ItemGroup>
</Project>
```

Rules:
- `Sdk` is required and must be `efeengine/1`. It is the format version. A different value is an error that names the version.
- `PropertyGroup` holds single values. `Name`, `Package` and `StartScene` are required. `Example` is optional; when set, the project builds `examples/<Example>` and not the package root.
- `EngineVersion` is required and must match the engine's format version (1 today).
- `ItemGroup` holds lists. `Scene` and `Mod` items take a path relative to the project folder. Paths must stay inside the project folder: absolute paths and `..` are errors.
- Unknown elements and unknown attributes are errors, with the line number. There is no silent ignore, so typos are caught.
- Element and attribute names are case-sensitive.
- The file is written with two-space indentation and a trailing newline, so saving an unchanged project changes nothing.

## Validation (`check`)

`efeengine-project::check` reports, per field:
- `Package` names no package in the project's `Cargo.toml`, or the names differ.
- `StartScene` is not in the `Scene` items.
- A listed `Scene` or `Mod` file does not exist.
- The folder has no `Cargo.toml`.

Opening a project that fails `check` shows every message and does not open it.

## Crate

`crates/project` (`efeengine-project`), no dependency on the editor or the engine. It parses with `quick-xml` (already in `Cargo.lock`), has a `Project` struct, `parse`, `write`, `check`, and `find` (walks up from a path to the nearest `.efproj`).

## What the editor does with it

- Build, Run and Restart use `Package` (or `Example`), not command-line flags.
- The game gets `StartScene` as its first scene.
- Recent projects are stored as paths in the user config, never inside the project. The UI shows project names only, never paths.
- New projects are created in the projects folder (default `<home>/efeengine-projects`, kept in the user config) and listed from there.
- The scaffold's `Cargo.toml` depends on `efeengine-core` from `https://github.com/efex50/efeengine.git`, pinned with `rev = "<commit>"`. The commit is the engine checkout's `HEAD` when `efeengine-project` was built (`crates/project/build.rs`, overridable with `EFE_ENGINE_REV`); without git at build time it falls back to `branch = "main"`. The commit must be pushed to GitHub, and uncommitted local engine changes are not in new projects. The first build downloads the engine, so it needs network access.
- Build and Run first check that `cargo` and `rustc` run; if not, they do not start and the editor says to install Rust from rustup.rs.

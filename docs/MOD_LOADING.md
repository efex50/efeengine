# Loading scripts and mods

A practical guide to what the engine does when you load code. Source:
`crates/core/src/engine/scripting/{load,packages,reload,cache,mods}.rs` and `engine.rs`.
Format of a package: [`crates/efevm/MOD_PACKAGE.md`](../crates/efevm/MOD_PACKAGE.md). Design:
`crates/efescript/docs/e10-design.md`. Tests: `crates/core/tests/{load,reload,packages}.rs`.

## Game scripts and mods

| | Game script | Mod |
|---|---|---|
| Inputs | `.efa` source, `.efvm` bytecode | `.efmod` package, mod folder (`mod.toml`) |
| Who wrote it | you | a third party |
| Checks | none: it may import any registered namespace | game check, id clash, dependencies, **policy**, asset check |
| Can provide functions | no (`ScriptError::NotAMod`) | yes (`provide`, section 13) |
| Reloadable by `reload_mod` | no | yes |

A script you ask for by path is game code and is trusted. A mod only *requests* namespaces
(`[permissions]` in `mod.toml`); your `ModPolicy` decides what it *gets*:

```rust
engine.set_mod_policy(|r: &ModRequest<'_>| match r.manifest.id.as_str() {
    "inventory" => Ok(Grant { namespaces: vec!["engine.log".into()], ..Grant::default() }),
    other => Err(format!("{other} is not welcome")), // becomes ModError::Denied
});
```

The grant is intersected with the request and covers the whole package. A mod's imports (and
the events it uses) must lie inside the grant or in the namespaces of its declared
dependencies, else `ModError::NotGranted`. `Grant` also carries the VM `budget` per frame and
`memory_pages`. The default policy (`DefaultPolicy`) grants nothing, and nothing is loaded
unless you register a source or call `load`.

## `Engine::load` and `load_input`

```rust
engine.load("scripts/mover.efa")?;       // native: dispatch on the path
engine.load("mods/inventory.efmod")?;    // native
engine.load("mods/inventory")?;          // a folder with mod.toml
engine.load_input(LoadInput::Package { label: "inventory.efmod", bytes: &bytes })?; // every target
```

`load` reads the path and calls `load_input`. The `LoadInput` kinds:

- `Efa { name, files, root }`: game script, compiled through the cache (feature `efa`).
- `Efvm { name, bytes }`: game module.
- `Package { label, bytes }`: a mod from `.efmod` bytes.
- `Folder { label, files, assets }`: a mod from source; `files` is any `efa::asm::FileSource`
  (`FsSource`, `MemSource`, your own), `assets` the asset paths relative to `[build] assets`.

The result is `Loaded::Module(id)` or `Loaded::Package(id)`. `init` runs right away. Without
the `efa` feature, `.efa` and folders give `LoadError::CompilerDisabled`.

## Mod sources

```rust
pub trait ModSource {
    fn list(&mut self) -> Vec<ModItem>;                       // every mod, now
    fn changed(&mut self) -> Vec<String> { vec![] }           // labels that changed (hot reload)
}
pub struct ModItem { pub label: String, pub body: ModBody }   // Package(Vec<u8>) | Folder { files, assets }
```

`DirSource::new(dir)` (native) lists `*.efmod` files and sub-folders with a `mod.toml`; its
`changed` compares file length and modification time. It is never installed for you:

```rust
engine.add_mod_source(DirSource::new("mods"));
```

A custom source (a download, a workshop) only has to turn what it has into `ModItem`s:

```rust
struct Workshop { cache: Vec<(String, Vec<u8>)>, dirty: Vec<String> }

impl ModSource for Workshop {
    fn list(&mut self) -> Vec<ModItem> {
        self.cache.iter().map(|(name, bytes)| ModItem { label: name.clone(), body: ModBody::Package(bytes.clone()) }).collect()
    }
    fn changed(&mut self) -> Vec<String> { std::mem::take(&mut self.dirty) }
}
engine.add_mod_source(Workshop { cache: downloaded, dirty: vec![] });
```

`label` is only a name for reports; the mod's identity is its manifest id. A folder source
should implement `FileSource::list` (`FsSource` and `MemSource` do) so the engine can hash it;
without it every load of the folder counts as changed.

## What `load_mods` does

`engine.load_mods() -> LoadReport` (`loaded`, `failed`, `skipped`; `reload_mods` also fills
`reloaded` and `unloaded`):

1. **List** every source, in registration order.
2. **Read manifests** only (`read_package`, or `prepare_mod` for folders) and hash the content.
   Unreadable items go to `failed`. An item whose id is loaded with the same hash goes to
   `skipped`; with another hash it fails with `AlreadyLoaded` (see below).
3. **Resolve** all items and the loaded mods with one `efemod::resolve`: missing or
   mismatched dependencies, duplicate ids and cycles are `ModError::Resolve`.
4. In dependency order, per item: **compile** a folder (through the cache, against the game
   index plus the indexes of its dependencies), then game check, id clash and dependency check,
   **grant** (policy, then imports against the grant), **check assets** (every handler's
   `check`; any `Err` refuses the whole mod, `ModError::Asset`), **instantiate** (link, register
   what it provides) and **commit** (handlers' `commit`; unhandled assets stay in
   `PackageInfo.assets`).
5. A mod whose dependency failed is reported as `ResolveProblem::DependencyFailed`.
   `init` of every loaded mod runs after all loads.

A failed item changes nothing: no namespace, no handler commit, no module id.

## Dependencies

```toml
[mod]
id = "shop"
version = "1.2.0"

[dependencies]
inventory = "^1"          # a loaded mod within that range; its namespace needs no grant
```

A dependency must be a loaded package in range. A folder mod is compiled against the
dependency's index, so it can `import inventory.add_item`.

## `[meta]` and `AssetHandler`

`[meta]` in `mod.toml` is a free-form table (at most 64 KiB, depth 16) kept in the manifest as
JSON: `engine.package(id).unwrap().manifest.meta`. Assets are any files under `[build] assets`.
An `AssetHandler` takes over the assets it `matches` (first handler wins):

```rust
struct Items(Vec<String>);
impl AssetHandler for Items {
    fn matches(&self, path: &str) -> bool { path.ends_with("items.json") }
    fn check(&mut self, _m: &Manifest, path: &str, bytes: &[u8]) -> Result<(), String> {
        std::str::from_utf8(bytes).map(|_| ()).map_err(|e| format!("{path}: {e}")) // before anything loads
    }
    fn commit(&mut self, id: ModuleId, m: &Manifest, path: &str, bytes: &[u8], res: &mut Resources) {
        res.insert(/* put the items where the game reads them */);
    }
    fn unload(&mut self, id: ModuleId, res: &mut Resources) { /* take them out again */ }
}
engine.add_asset_handler(Items(vec![]));
```

`unload` runs when the mod unloads and before the `commit` of a hot reload or reload. Names
`manifest.json`, `module.efvm` and `engine/*` are reserved.

## Content hash and `AlreadyLoaded`

Loading a mod records `PackageInfo.hash` (blake3, 64 hex characters) and `PackageInfo.source`
(the label). A package hashes its `.efmod` bytes; a folder hashes every file of the folder,
sorted by path, each as its path and its bytes. If a mod with the same id is loaded,
`load`, `load_input` and `load_mods` fail with

```
ModError::AlreadyLoaded { id, module, same_contents }
// "mod `x` is already loaded"
// "mod `x` is already loaded (contents changed; use reload_mod)"
```

`load_mods` does not report an identical mod as a failure: it lands in `LoadReport::skipped`,
so calling it twice is harmless.

## Reload and unload

```rust
match engine.reload_mod("mods/inventory")? {          // or reload_mod_input(LoadInput) on web
    Reloaded::Unchanged(id) => {}                      // same hash: nothing happened
    Reloaded::Replaced { id, dependents } => {}        // new module id; dependents reloaded
    Reloaded::Loaded(id) => {}                         // it was not loaded: a plain load
}
let report = engine.reload_mods();   // new: load; changed: replace; same: skipped; gone: unloaded
let gone = engine.unload_mods();     // UnloadReport { unloaded, failed }
```

**Replacing a mod.** The mod and every loaded dependent are unloaded (dependents first, handlers'
`unload` called), the new version loads, then each dependent loads again against it (a folder
dependent is recompiled, a package dependent loads from the module the engine retained).
Module ids change (an id is never reused) and `init` runs again for each of them.

**Failure and rollback.** A new version that does not build fails before anything is touched.
Any later failure (a dependent no longer links, the policy refuses) unloads what was loaded in
the attempt and loads the previous versions back. The error is
`ModError::ReloadFailed { id, cause, restored }`: `restored == true` means the old versions run
again; `false` means the restore failed too, the mod is left unloaded and the log says why.
A game module that uses the mod cannot be put back (the engine keeps no source for it), so the
reload is refused up front and nothing changes.

**`reload_mods`.** Re-lists every source and classifies each item by id and hash, with one
dependency resolve. Mods that came from a source and are no longer listed are unloaded with
their dependents. All changed mods are swapped together, so a mod and its dependent can change
in step (an API break plus the matching update). Limit: a new mod that a changed mod needs
but that itself needs another changed mod cannot be ordered, and the changed mod fails.

**`unload_mods`.** Every mod, latest loaded first, with handlers' `unload`. Game modules stay
loaded; a mod that a game module still imports from stays too, listed in `failed`.

**Hot reload.** The script system asks each source `changed()` at most every 0.25 s of engine
time, reloads the changed loaded mods *in place* (module ids stay, `init` runs again, a failure keeps
the old version; an incompatible change is refused there, which `reload_mod` can do) and rebuilds dependent folder mods whose bytes change. Queue the explicit
version from a system with `ctx.commands.reload_mods()` (script_demo: press R) and `ctx.commands.unload_mods()`
(coin_dash_mods: press U); the engine logs both reports.

**Game scripts from source.** `engine.watch_source(id, "scripts/main.efa")` (native) watches every `.efa` file under the
root's directory: a change recompiles the root through the cache and swaps the module in place (same module id,
`init` runs again, `Script.state` resets, a compile error is logged and the old version keeps running). The world
is kept; the module's memory is not (see `examples/coin_dash_scripts`).

## `ScriptCache`

`engine.cache()` is the engine-owned cache; configure it with `EngineConfig.script_cache`:

```rust
EngineConfig { script_cache: CacheConfig { disk: CacheDir::Path("cache".into()), memory: true, max_disk_bytes: 64 << 20 }, ..Default::default() }
```

`CacheDir::Platform` (default: `$XDG_CACHE_HOME` or `~/.cache`, `Library/Caches`, `%LOCALAPPDATA%`,
under `efeengine/<game name>`, resolved on first use), `CacheDir::Off`, `CacheDir::Path(p)`.

- **Memory** (all targets): skips decoding and verifying the same bytes twice. Key: blake3 of
  the bytes; a hit is confirmed by comparing the full bytes. Capped at 64 MiB, cleared past it.
- **Disk** (native only; web is memory only): stores compile results of `.efa` and mod
  folders as `<base>/efa/v{CACHE_FORMAT}/<xx>/<hash>.{efvm,deps}`. Key: blake3 of the cache
  format, the `efa` version, the root file's name and contents, and the API index hash.
  `.deps` lists every file the compile read with its hash.
- **A hit is checked:** each file in `.deps` is re-read and re-hashed, the `.efvm` is decoded
  and verified, and its recorded index hash must equal the current one. Any mismatch is a miss
  and the stale entry is deleted. Linking and the policy always run after a hit. Writes go to
  a temp file, then `rename`. `max_disk_bytes` is enforced once per engine, oldest first.
- `engine.cache().stats()` returns `CacheStats` (`mem_hits`, `decodes`, `disk_hits`,
  `compiles`, ...); `clear_memory()` and `clear_disk()` reset it.

## Errors

`load` and `load_input` return `LoadError`: `Io`, `Compile(Diagnostics)` (a `.efa` script),
`Script(ScriptError)`, `Mod(ModError)`, `CompilerDisabled`, `UnknownKind(path)`, `NotAMod(name)`
(`reload_mod` of a game module). Mod failures are `ModError`:

| Variant | Meaning |
|---|---|
| `Package` | container, manifest, module decode or verify |
| `Build` | folder: `mod.toml`, limits (`BuildLimits`), compile diagnostics as text |
| `Resolve` | missing or wrong-version dependency, duplicate id, cycle, failed dependency |
| `GameMismatch` | the manifest names another game |
| `IdClash` | id equals or prefixes a registered namespace |
| `Denied` | the policy refused |
| `NotGranted` | imports or events outside the grant and dependencies |
| `Asset` | reserved name, or an `AssetHandler::check` failed |
| `Script` | name taken, link errors, struct mismatch, `Needed`, ... |
| `AlreadyLoaded` | a mod with that id is loaded (`same_contents` tells if identical) |
| `ReloadFailed` | `reload_mod` failed; `restored` tells if the old version is back |

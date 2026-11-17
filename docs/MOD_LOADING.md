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
dependencies, else `ModError::NotGranted`. `Grant` also carries the VM `budget` per frame,
`memory_pages`, and the component limits `max_component_types` (16) and `max_component_size`
(256 bytes), see [Mod components](#mod-components). The default policy (`DefaultPolicy`) grants nothing, and nothing is loaded
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

## Mod components

A mod can declare components of its own in `[meta]`. They are real archetype columns (plain
bytes, `Pod`), so scripts and Rust use them like built-in ones.

```toml
[[meta.component]]
name = "Health"
doc = "optional"
fields = [ { name = "hp", type = "f32" }, { name = "max", type = "f32" }, { name = "pos", type = "f32", count = 2 } ]
tracked = true   # optional, default true: change ticks on
```

- **Fields.** `type` is a primitive name: `u8 i8 u16 i16 u32 i32 u64 i64 f32 f64 bool entity`;
  `count` (default 1) makes an array. The layout follows `repr(C)`: each field is aligned to the
  size of its type, the component is aligned to its largest field and its size is padded to
  that. At least one field; field names are identifiers and unique; unknown keys are errors.
- **Name.** The component is registered as `mod:<mod id>/<Name>` (owner `mod:<mod id>`, id
  `fnv1a32(name)`), in the `TypeRegistry`, the API index and the world, before the mod's `init`
  runs. `StructDesc::validate` accepts this form (`efevm::api::is_type_name`); it cannot appear
  in a `&Name` function parameter and `.efa` code cannot spell it, so scripts get the id with
  `engine.world.component_id("mod:inventory/Health")` (put it in a global in `init`). An `.efa`
  `component` block is not supported.
- **Declaration errors** fail the load before anything is touched, as
  `ModError::Component { id, component, reason }`: bad type, duplicate field, no fields, a
  duplicate or non-identifier name, a name that is already registered, a component over 64 KiB,
  more than 256 components or 64 fields.
- **Limits.** `Grant::max_component_types` (default 16) and `Grant::max_component_size` (default
  256 bytes) are checked after the policy answers; `ModRequest::components` lists what the mod
  declares (`name`, `size()`, `align()`, `desc`) so a policy can decide or raise the limits.
- **Scripts.** `engine.world.get/set/has/insert/remove/changed/query` work on the id exactly as
  for Rust components, including the buffered `set` and a module seeing its own sets. They can be
  put on any entity, including game entities that keep their typed components.
- **Rust.** `ctx.dyn_get(e, id) -> Result<DynRef, CompError>` (bytes, shared borrow of the
  column) and `ctx.dyn_get_mut` (stamps the change); `engine.types().view(world, e, id)`
  gives a `DynView` with `field::<T>("name")`, `field_f32("hp")` and `DynComponent { desc, bytes }`
  underneath; `view_mut(..).set_field("hp", 1.0f32)` writes. `TypeEntry::read/write/insert/remove/has`
  work on them (so `Commands::insert_raw` too). Parallel systems cannot declare a mod component
  in `Access`: `ParCtx::dyn_get` reads it through a run-time checked shared borrow (a conflicting
  write would panic; only exclusive systems write them, so treat them as read-only there).
- **Entities of a mod.** An entity spawned by a script (`engine.world.spawn`) carries a Rust-only
  untracked `SpawnedBy(module)` component (not visible to scripts).
- **Unload** (`unload_mods`, a mod dropped by `reload_mods`, `Engine::unload_module`): first
  every entity with `SpawnedBy(module)` is despawned (collected in query order, then despawned),
  then each component is unregistered, which removes it from all remaining entities, and its type
  leaves the registry and the index. Loading the mod again registers a new generation.
- **Replacing a mod** (`reload_mod`, `reload_mods`): the old module's spawned entities are
  despawned (its `init` runs again and spawns anew). A component with an identical layout (same
  size, alignment, field names, types, counts and `tracked`) in the new version is kept as it
  is: data, ticks and registration stay. If the layout changed it is *migrated* after the whole
  swap succeeded: every entity that has it (world order) gets the new layout, where a field with
  the same name, primitive type and count keeps its bytes and every other field is zero.
  Components the new version does not declare are removed from all entities. If the swap fails
  and the previous versions are restored, the component data was never touched. The spawned
  entities of the failed attempt's old version are gone (they are despawned before the new
  version loads), and `init` of the restored version spawns them again.
- **Hot reload in place** (watched file or source, module id kept): changing the code is fine,
  the entities stay. Changing the declared components is refused (`ModError::Component`, the old
  version keeps running); a source reload then queues `reload_mods`, which replaces the mod with
  the migration above, and a watched package or file needs `Engine::reload_mod`.
- **Determinism.** Spawn, despawn and migration order comes from query order and sorted maps,
  never from hash iteration; one thread and four threads give identical worlds.

## The `.efapi` of a mod

A mod's API fragment is built from its declaration, without running the game: its namespace
(`<id>`, the manifest's `api_version` and doc), the functions it provides, its events, its own
structs, and every `[[meta.component]]` as a struct `mod:<id>/<Name>` owned by `mod:<id>` with its
component id. It is the same data the engine registers when the mod loads (a test compares them),
built by `efemod::api_fragment(&manifest, &module)`.

- `efescript pack DIR` writes `<out>.efapi` next to the `.efmod`; `cargo xtask mods <example>` writes
  one beside each package in `target/mods/<example>/`.
- `cargo xtask mod-api examples/<example>/mods/<name> [-o FILE] [--api GAME.efapi]` writes just the
  fragment (default `<name>.efapi`). Inside an example it builds against that example's exported
  game index and the sibling mods it depends on; elsewhere pass `--api`.
- `cargo xtask check-scripts <example>`: a script with `; needs-mod: NAME` as one of its leading
  comment lines is checked against the game index merged with the fragment of
  `examples/<example>/mods/NAME` (and the fragments of the mods that one depends on), using
  `ApiIndex::merge`. If no such mod folder exists, or it does not build, the command fails and says so.
- Use a fragment the same way as any `--api` input of `efescript` and the LSP: merge it with the
  game's `.efapi` (`--api game.efapi --api mod.efapi`).

Mods register at any time, also after the game loop started: the lock of
[SYSTEMS.md](SYSTEMS.md) is for the game's own registration.

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
Module ids change (an id is never reused) and `init` runs again for each of them. The entities
the old modules spawned are despawned; mod components are kept or migrated (see
[Mod components](#mod-components)).

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

**`unload_mods`.** Every mod, latest loaded first, with handlers' `unload`, its spawned entities
despawned and its components removed. Game modules stay loaded; a mod that a game module still imports from stays too, listed in `failed`.

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
| `Component` | a declared mod component is invalid, over the grant's limits, or a hot reload in place would change the declarations |

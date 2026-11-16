# Plan: one engine loading API, engine-owned caches, pluggable mod loading (E10)

## Context
Today the engine loads only compiled bytecode, and it does so through several separate functions:
- `load_module(name, bytes)`, `load_module_file(path)`, `load_package(bytes)`, `load_packages`, `load_mods_dir` in `crates/core/src/engine.rs:318-431`.
- Core depends on efevm and efemod only. It cannot compile `.efa` source.
- `.efa` gets compiled only by the `efescript` CLI, which xtask runs once per script as a subprocess.
- Nothing is cached anywhere, and xtask rebuilds everything on every run.
- script_demo loads `mover.efvm` by a hard-coded path. The game binary never loads mods.

The user wants the following:
1. One engine-level function that loads a `.efa`, `.efvm` or `.efmod` file, or a mod folder.
2. Runtime `.efa` compilation **always available**, shipped games included.
3. A cache **owned by the engine**, on **disk and in memory**. On web, memory only.
4. Third-party mod loading that is **opt-in and pluggable**. The game developer brings their own system for finding mods (a folder, Steam Workshop, a download, …); the engine provides the pipeline that loads them.
5. Packages that can carry **custom metadata and custom assets** defined by the game.

## 1. Lean compiler crate `crates/efa`
- Move `asm/*` and `link/*` out of efescript unchanged.
  - `FsSource` stays native-only.
- Also move the in-memory half of `pack.rs`:
  - `read_mod_toml`, the `inside` path check, `mod_index`;
  - a new `build_mod(src: &dyn FileSource, api, &BuildLimits) -> BuiltMod { manifest, module, assets }`.
- Dependencies: efevm, efemod (without `write`), toml.
  - It must never enable `efemod[write]`. Feature unification would otherwise ship the LZMA encoder in every game.
- What stays in efescript: CLI, LSP, debugger, `host_std`, bundle, audio, `pack_dir` and `.efmod` writing.
  - efescript adds `pub use efa::{asm, link}`.
- Core gets a feature `efa = ["dep:efa"]`, on by default.
  - Without the feature, loading `.efa` or a folder returns `LoadError::CompilerDisabled`.
- `BuildLimits` caps total bytes, file count, `use` depth and nesting, because mod-folder source is untrusted.

## 2. Engine API (new `crates/core/src/engine/scripting/load.rs`)
```rust
pub enum LoadInput<'a> {
    Efa    { name: &'a str, files: &'a dyn FileSource, root: &'a str },               // game script, trusted
    Efvm   { name: &'a str, bytes: &'a [u8] },                                        // trusted
    Package{ label: &'a str, bytes: &'a [u8] },                                       // policy path
    Folder { label: &'a str, files: &'a dyn FileSource, assets: &'a [String] },       // policy path
}
pub enum Loaded { Module(ModuleId), Package(ModuleId) }
pub enum LoadError { Io(String), Compile(Diagnostics), Script(ScriptError), Mod(ModError), CompilerDisabled, UnknownKind }

impl Engine {
    #[cfg(native)] pub fn load(&mut self, path: impl AsRef<Path>) -> Result<Loaded, LoadError>; // by extension / mod.toml
    pub fn load_input(&mut self, input: LoadInput) -> Result<Loaded, LoadError>;            // web + custom systems
    pub fn add_mod_source(&mut self, s: impl ModSource + 'static);
    pub fn set_mod_policy(&mut self, p: impl ModPolicy + 'static);                           // already exists
    pub fn add_asset_handler(&mut self, h: impl AssetHandler + 'static);
    pub fn load_mods(&mut self) -> LoadReport;  // every registered source, one dependency resolve
    pub fn cache(&mut self) -> &mut ScriptCache;
}
```
- **Trust:** a `.efa` or `.efvm` that the game asks for by path is game code. It runs with no policy, as `load_module` does today. Packages and mod folders always go through:
  - the game check
  - the id-clash check
  - dependency checks
  - the policy and its grant
- **Removed** (no backward compatibility): `load_module_file`, `load_mods_dir`, `load_packages`, the public `load_package`. Unload, reload and `package()` stay.

## 3. Pluggable third-party mods
```rust
pub trait ModSource { fn list(&mut self) -> Vec<ModItem>; fn changed(&mut self) -> Vec<String> { vec![] } }
pub struct ModItem { pub label: String, pub body: ModBody }
pub enum ModBody { Package(Vec<u8>), Folder { files: Box<dyn FileSource>, assets: Vec<String> } }
```
- The engine ships `DirSource::new(dir)` as a ready-made source. It is native only and lists `*.efmod` files and folders that contain `mod.toml`. **It is never installed automatically.**
- With no source registered, `load_mods()` loads nothing. `DefaultPolicy` still grants nothing.
- How `load_mods()` works:
  1. Collect the items from every source.
  2. Read just the manifests, so it can resolve before any compiling.
  3. Run `efemod::resolve` across all sources.
  4. In dependency order, compile each folder against the game index merged with the indexes of its loaded dependencies. Then grant, check assets, instantiate and commit.
- A folder's `[build] api` setting is ignored at runtime, and it is an error if it names a path.
- `changed()` drives hot reload of a mod and the mods that depend on it.

## 4. Custom metadata and assets
- **Metadata:** `Manifest.meta: Option<Value>` in `crates/efemod/src/manifest.rs`.
  - Source: a free-form `[meta]` table in `mod.toml`, converted from TOML to JSON and kept in `manifest.json`.
  - Caps: 64 KiB and nesting depth 16.
  - The game reads it as `engine.package(id).manifest.meta`.
- **Asset handlers:**
  ```rust
  pub trait AssetHandler {
      fn matches(&self, path: &str) -> bool;
      fn check(&mut self, m: &Manifest, path: &str, bytes: &[u8]) -> Result<(), String>;   // before instantiate
      fn commit(&mut self, id: ModuleId, m: &Manifest, path: &str, bytes: &[u8], res: &mut Resources); // after success
      fn unload(&mut self, id: ModuleId, res: &mut Resources);
  }
  ```
  - If any `check` fails, the whole package is rejected with `ModError::Asset{path, reason}`, the same all-or-nothing rule as permissions.
  - `commit` runs only after a successful load, so a failed load leaves nothing behind.
  - Assets no handler matches stay in `PackageInfo.assets`.
  - Reserved paths: `manifest.json`, `module.efvm`, and `engine/*`.

## 5. `ScriptCache` (new `scripting/cache.rs`, settings in `EngineConfig.script_cache`)
- **Memory cache:** skips decoding and verifying the same bytes twice.
  - It is keyed by a blake3 hash, and a hit is confirmed by comparing the full bytes.
  - Linking and the policy always run again.
- **Disk cache:** stores the `.efa` and mod-folder compile results.
  - **Key:** a blake3 hash of the root file's path and contents, the merged API index hash, `CACHE_FORMAT` and the `efa` crate version.
  - **Dependency list:** each entry stores the `.efvm` plus a list of every file the compile read and its hash. It is recorded by a `FileSource` wrapper, since the `use` graph is only known after parsing. Any changed file counts as a miss.
  - **Hits are still checked:** every hit goes through decode, verify and link, and its section-12 index hash must equal the current one.
  - **Layout:** `<cache_dir>/efa/v{N}/<xx>/<hash>.{efvm,deps}`.
  - **Writes:** to a `.tmp` file, then renamed into place, so a crash never leaves a half-written entry.
  - **Size cap:** `max_disk_bytes`, enforced at startup by deleting the oldest entries first.
- **Config:** `CacheConfig { disk: Option<PathBuf> /* default: platform cache dir/<game> */, memory: bool, max_disk_bytes }`.
  - On web, `disk` is always `None`.
  - New dependency: `blake3`, which is pure Rust and builds for wasm. FNV is too weak here, because a crafted collision would let a mod skip verification.

## 6. Demo, xtask, tests, docs
- **script_demo:**
  - `load_scripts` becomes `engine.load(dir.join("mover.efa"))`, with the source shipped next to the binary.
  - `main` registers `DirSource(target/debug/mods)` and a demo policy, then calls `load_mods()`, so inventory and shop run in the real game and their logs show.
  - It logs the load report and each package's `meta`.
  - The inventory mod gets a `[meta]` block and an `items.json` handler that logs the items.
- **xtask:**
  - `build` stops assembling scripts. It copies the `.efa` files and still exports the index (for the language server) and packs the mods.
  - A new `xtask check-scripts` assembles every script against the index, for CI.
- **Tests:** a new file `crates/core/tests/load.rs`:
  - each input kind is handled correctly
  - a memory-cache hit skips decoding (counted)
  - disk cache: a hit, then a miss after editing a file the script uses
  - changing the API index invalidates cached entries
  - a folder mod that depends on a packaged mod
  - a rejected asset rejects the whole package, and nothing is committed
  - `[meta]` survives the round trip through `manifest.json`
  - nothing loads when no mod source is registered
  - inputs over `BuildLimits` are refused

  The existing `packages_e2e` and `e2e` tests are moved onto the new API.
- **Docs:**
  - a new E10 "Engine loading and caches" in `crates/efescript/ROADMAP.md`, before E9, which stays last
  - one line in the root `ROADMAP.md`
  - `MOD_PACKAGE.md` (`meta`, reserved `engine/`)
  - `e8-design.md`
  - the `structure.md` files

## Execution (an Opus middle manager, using the model split)
Each step touches its own files:
1. **Opus:** final trait and type spec. Doc only.
2. **Sonnet:** crate split (`crates/efa`, efescript re-exports, Cargo files) and the wasm check.
3. **Haiku or Gemini:** `Manifest.meta` and the `[meta]` parsing.
4. **Sonnet:** `ScriptCache`.
5. **Sonnet:** `load.rs`, `ModSource`/`DirSource`/`AssetHandler`, the pipeline, and the `engine.rs` changes.
6. **Haiku:** script_demo and xtask.
7. **Sonnet:** tests, and a fuzz target for `build_mod`.
8. **Haiku:** docs and roadmaps.
9. **Gemini:** review of the cache and the trust boundary.

## Risks
- **Untrusted source reaches the compiler.** Mitigated by `BuildLimits` plus fuzzing, because a panic aborts on wasm.
- **Cache poisoning.** Mitigated by re-checking every hit and comparing the index hash. Someone who can write to the cache folder could still put other code that passes verification in its place. For a trusted game script that is no worse than editing the `.efa` file itself.
- **Bigger game builds.** Shipping the compiler and `toml` makes builds bigger; measure the wasm build size.
- **The encoder leaking into games.** If any crate turns on `efemod[write]`, the LZMA encoder gets compiled into games; check with `cargo tree -e features`.

## Verification
- `cargo test -p efevm -p efemod -p efa -p efescript -p efeengine-core -p script_demo` passes. The baseline is 1134 tests plus the new ones.
- `cargo check -p efa -p efeengine-core --target wasm32-unknown-unknown` passes.
- `cargo check --workspace --all-targets` passes.
- `cargo tree -p script_demo -e features | grep lzma` finds no encoder.
- `cargo xtask build script_demo native`, then run `target/debug/native`. The logs should show:
  - `mover.efa` compiled and loaded; on a second run, a cache hit
  - the inventory and shop mods loaded, with `engine.log` lines from inventory
  - the `items.json` handler output
- Editing `mods/inventory/src/items.efa` while the game runs reloads it.

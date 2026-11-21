# efemod

`efemod` reads and checks mod packages (`.efmod`), the archive format the game loads mods from. The format is specified in `crates/efevm/MOD_PACKAGE.md`. A package is a restricted 7z archive holding `manifest.json`, `module.efvm` and optional `assets/` files. The crate also holds the SemVer version and requirement types, the dependency resolver that decides load order, and `[[meta.component]]` component layout/API-fragment helpers. It does no I/O: the engine or the packer hands it bytes.

## At a glance
| | |
|---|---|
| Path | `crates/efemod` |
| Kind | library (`efemod`, `edition = "2024"`, `version = "0.1.0"`, `#![forbid(unsafe_code)]`) |
| Depends on (workspace) | `efevm` |
| Key external deps | `sevenz-rust2` 0.25 (`default-features = false`, decoder only), `crc32fast` 1.5.5, `lzma-rust2` 0.25 (`default-features = false`, `std`, optional: `encoder` enabled by `write`) |
| Used by | `efeengine-core` (decoder without `write`, re-exported as `efeengine_core::efemod`; `dev-dependencies` enable `write`), `efa` (reader only, no `write`), `efescript` (`write` feature: `pack`, `inspect`, `api-export`), `examples/coin_dash_mods` (`dev-dependencies` with `write`), and `examples/coin_dash_wasm` (non-wasm target `dependencies` with `write`) |
| Features | `write` (off by default: `Package::write`, `container::write`, LZMA2 encoder from `lzma-rust2/encoder`); self dev-dependency `efemod = { path = ".", features = ["write"] }` so tests always build the writer while normal dependents do not |
| Targets | `wasm32-unknown-unknown` as well as native; no `std::fs`, no threads |
| Tests | 4 integration test files under `tests/` (`container.rs`, `package.rs`, `resolve.rs`, `version.rs`) |

## Layout
```text
crates/efemod/
├── Cargo.toml          package efemod; feature `write`; self dev-dependency
├── src/
│   ├── lib.rs          `#![forbid(unsafe_code)]`; module list, crate docs, re-exports (`Package`, `check_module`, `Manifest`, `Permissions`, `MAX_META_BYTES`, `MAX_META_DEPTH`, `Version`, `VersionReq`, `VersionError`, `PackageError`, `ReadLimits`, `resolve`, `Candidate`, `Dependency`, `Resolution`, `ResolveProblem`, `api_fragment`)
│   ├── components.rs   `[[meta.component]]` parsing & `repr(C)` layout (`ModComponent`, `ComponentError`, `component_name`, `parse_components_raw`, `api_fragment`, caps `MAX_COMPONENTS_PER_MOD`, `MAX_FIELDS`, `MAX_COMPONENT_BYTES`)
│   ├── container.rs    7z profile: `Entry`, `read` (pre-checks, then `sevenz-rust2`, CRC per entry), `precheck`, `validate_entry_name`, `write` (feature `write`)
│   ├── limits.rs       `ReadLimits` (MOD_PACKAGE section 6): file, entry, header, dictionary, name and per-entry sizes; `HARD` caps; `clamped()`
│   ├── error.rs        `PackageError`: `TooLarge`, `Container`, `BadEntryName`, `Missing`, `Crc`, `Manifest`, `Module`, `UndeclaredImport`
│   ├── manifest.rs     `Manifest` (with `meta`, `display_name`, `from_json`, `to_json`, `validate`, `namespace_doc`) and `Permissions`; `MAX_META_BYTES`, `MAX_META_DEPTH`
│   ├── package.rs      `Package` (`manifest`, decoded `module`, `module_bytes`, `assets`, `ignored`), `Package::read`, `write` (feature `write`), `index`, `asset`; `check_module`
│   ├── version.rs      `Version` (SemVer, no build metadata), `PreId`, `VersionReq` (Cargo's matching rules, no wildcards), `VersionError`
│   └── resolve.rs      `resolve()` over `Candidate`s: duplicate ids, missing and mismatched deps, cycles, failed deps; topological load order
└── tests/
    ├── container.rs    7z profile: signature and header CRCs, header walk, limits, names, coders, round trips (uses `write`)
    ├── package.rs      read/write of packages, manifest rules, `meta` limits and round trips, `check_module`, assets, the module index, never-panic mutations
    ├── version.rs      SemVer parse and order, requirement parse and match
    └── resolve.rs      resolver problems, cycles, priority between problems, load order
```

## Main concepts
- **Package format.** `Package::read(bytes, &ReadLimits)` checks in the order of MOD_PACKAGE section 7 and returns a `Package { manifest, module, module_bytes, assets, ignored }`. `manifest.json` and `module.efvm` are required; `assets/<path>` entries go into `assets` (keyed without the prefix); any other entry goes into `ignored`. The module is decoded and verified by `efevm::Module::decode`, then `check_module` runs.
- **Container checks before the decoder** (`container.rs`). `sevenz-rust2` is only given bytes that already passed: the start-header CRC, a plain (not encoded) header of at most 4 MiB, at most 4096 files, folders and streams, one folder per packed stream with one coder, and only COPY or LZMA2 (dictionary at most 64 MiB). Entry names follow the section 2 rules. Each decoded entry is CRC-checked. Every read is bounded by `ReadLimits`, so a hostile archive cannot make the crate allocate more than the caps.
- **ReadLimits** (`limits.rs`). Defaults are `ReadLimits::HARD`: 64 MiB file, 4096 entries, 4 MiB header, 64 MiB dictionary, 240-byte names with 16 segments, 64 KiB manifest, 16 MiB module, 32 MiB per asset, 128 MiB total. `clamped()` lowers any field to the hard cap, so an embedder can tighten the limits but never raise them.
- **Manifest** (`manifest.rs`). `Manifest { id, version, name, description, authors, license, game, api_version, dependencies, permissions, meta }`. `id` matches `[a-z][a-z0-9_]{0,63}` and is not reserved (`engine`, `std`, `efe`). `Permissions` are requests (`namespaces` as prefixes, at most 64; `budget` in VM units per frame; `memory_pages`); the game's policy decides what is granted. `meta` holds optional free-form JSON object metadata capped at `MAX_META_BYTES` (64 KiB canonical JSON) and `MAX_META_DEPTH` (16). `check_module` enforces that the module does not import from its own namespace and that every imported namespace is a requested prefix or a dependency's namespace.
- **Mod components & API fragment** (`components.rs`). `parse_components_raw(mod_id, meta)` parses `[[meta.component]]` into `ModComponent { name, tracked, desc }` with `repr(C)` field offsets, full struct name `mod:<id>/<Name>`, owner `mod:<id>`, and FNV-1a 32-bit `id()`, bounded by `MAX_COMPONENTS_PER_MOD` (256), `MAX_FIELDS` (64), and `MAX_COMPONENT_BYTES` (64 KiB). `api_fragment(manifest, module)` builds the mod's `efevm::ApiIndex` from `module`'s interface plus each `[[meta.component]]` struct carrying its component id.
- **Writer** (`write` feature only). `Package::write(manifest, module_bytes, assets)` validates the manifest and asset names, then calls `container::write`: one folder per non-empty entry, LZMA2 at preset 6 with the dictionary fitted to the entry (COPY when it is not larger), a plain header with sizes, CRCs and names. Same inputs give the same bytes. Only tools/packers/examples enable this feature (`efescript`, `examples/coin_dash_wasm`, test dev-deps); the engine runtime builds the reader alone.
- **Versions** (`version.rs`). `Version` is SemVer with a pre-release list and no build metadata; a numeric pre-release identifier must fit `u64`. `VersionReq` parses comparators (`=`, `>`, `>=`, `<`, `<=`, `~`, `^`) and matches with Cargo's semantics. Wildcards are rejected. Length and comparator-count caps come from MOD_PACKAGE section 5.
- **Resolution** (`resolve.rs`). `resolve(&[Candidate], &[(id, &Version)])` is pure. A `Candidate` is `{ id, version, dependencies }`; `loaded` lists packages already present. The result `Resolution { order, problems }` gives the indices that can load, dependencies first (topological, ties broken by id), and one `ResolveProblem` per failed candidate: `DuplicateId`, `Missing`, `Version`, `Cycle` (with the cycle path), or `DependencyFailed`. Each candidate appears in at most one of the two lists.
- **Module index.** `Package::index()` builds the mod's own `efevm::ApiIndex` from the module's interface (section 13) with the manifest's id, `api_version` and namespace doc. `api_fragment` extends that with `[[meta.component]]` descriptors for build tools and scripts.
- **Errors.** Every failure is a `PackageError` variant; `Module` wraps `efevm::LoadError`. Messages name the entry or the manifest key path. Nothing panics on malformed input (`tests/package.rs` mutates manifests and archives in a never-panic loop).

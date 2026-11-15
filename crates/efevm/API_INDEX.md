# The API index file (`.efapi`), format version 1

The API index lists everything a script can call or see: host-function namespaces,
host functions with parameter names, types and docs, and the byte layouts of structs and
engine components. It is the "PDB" of a game built with efeengine: a mod developer, the
assembler, the language server and the debugger need this file and nothing else.

The index is not a second table. It is a snapshot of `efevm::HostRegistry`, the only way
to make a host function callable (`HostRegistry::imports()` is the only producer of the
`Imports` an `Instance` links against). Whatever is callable is therefore in the index, and
what is in the index is exactly what was registered.

Producers: `HostRegistry::index()` → `ApiIndex`, `ApiIndex::to_json()` → text.
Consumers: `ApiIndex::from_json(&str)`. Both live in `efevm::api` and do no I/O; callers
read and write the file.

## File

- Name: `<game>.efapi` (the std library of the efescript CLI: `std.efapi`).
- Encoding: UTF-8 JSON, one object. The writer emits keys in the order shown below, two-space
  indentation, `\n` line ends, and a final newline, so the same registry always produces the
  same bytes.
- Readers must ignore unknown keys (forward compatibility) but must reject an unknown
  `format` or a `version` greater than they support.

```json
{
  "format": "efapi",
  "version": 1,
  "game": "script_demo",
  "hash": "fnv1a64:9f0c2b5e17a4d3c8",
  "namespaces": [
    { "name": "engine.world", "version": 1, "owner": "engine", "doc": "Entities and components." }
  ],
  "types": [
    {
      "name": "Transform", "owner": "engine", "doc": "Position, rotation and scale.",
      "size": 20, "align": 4, "component": 2166136261,
      "fields": [
        { "name": "position", "type": "f32", "count": 2, "offset": 0, "doc": "" },
        { "name": "rotation", "type": "f32", "count": 1, "offset": 8, "doc": "Radians." },
        { "name": "scale", "type": "f32", "count": 2, "offset": 12, "doc": "" }
      ]
    }
  ],
  "functions": [
    {
      "namespace": "engine.world", "name": "get", "owner": "engine",
      "doc": "Copy component `component` of `entity` to `out`; 0 ok, 1 stale entity, 2 no such component.",
      "params": [
        { "name": "entity", "type": "entity" },
        { "name": "component", "type": "i32" },
        { "name": "out", "type": "ptr" }
      ],
      "results": [ { "name": "status", "type": "i32" } ],
      "signature": "(i64, i32, i64) -> i32"
    }
  ]
}
```

### Keys

| Key | Meaning |
|---|---|
| `format` | always `"efapi"` |
| `version` | format version, `1` |
| `game` | name of the game (or `"std"`); informational, not hashed |
| `hash` | `"fnv1a64:"` + 16 lower-case hex digits, see below |
| `namespaces` | sorted by `name` |
| `types` | sorted by `name`; `component` is a number for engine components, `null` for plain structs |
| `functions` | sorted by `(namespace, name)` |
| `signature` | the lowered VM signature, `(params) -> result` or `(params) -> (r1, r2)` or `(params)`; derived from `params`/`results`, written for humans and checked by the reader |

Names: a namespace is identifiers joined by dots (`std`, `engine.world`, `inventory`); a
function, struct or field name is an identifier `[A-Za-z_][A-Za-z0-9_]*`. A script imports
`namespace.name`; the last dot separates the function name.

### Parameter types

| Spelling | VM slots | Meaning |
|---|---|---|
| `i32` `i64` `f32` `f64` | one | plain values |
| `bool` | `i32` | 0 = false |
| `entity` | `i64` | `Entity::to_bits`; a stale or forged handle fails with a status code, never a panic |
| `ptr` | `i64` | untyped guest address |
| `str` | `i64, i64` | pointer and length of UTF-8 text in guest memory (parameters only) |
| `bytes` | `i64, i64` | pointer and length of a byte buffer in guest memory (parameters only) |
| `&Name` | `i64` | pointer to a struct with the registered layout `Name` (parameters only) |

Results may only be `i32 i64 f32 f64 bool entity ptr`.

### Field types

`u8 i8 u16 i16 u32 i32 u64 i64 f32 f64 bool entity`, each aligned to its own size;
`count` > 1 means a fixed array (`Vec2` = `f32` × 2). Fields never overlap and lie within
`size`; `align` is a power of two dividing `size`. Padding is not listed.

## Hash

`hash` is 64-bit FNV-1a over the bytes of the *canonical body*: the same JSON text the
writer produces, with the `game` and `hash` lines left out (so the hash depends only on the
API, not on the game's name). Readers recompute it and reject a file whose hash does not
match (it was edited by hand or truncated). FNV-1a is a change detector, not a security
measure.

## How modules use the hash (`.efvm` section 12 "api")

A module assembled against an index records it in section 12 of the `.efvm` format
(SPEC.md section 9.1): the index hash (`u64`, 8 bytes little-endian) and, for every
namespace it imports from, `(name, version)`. At link time:

- every import is resolved by `namespace.name` and checked against the lowered signature
  (`missing import`, `signature mismatch` errors name the function and both signatures);
- every recorded namespace must exist with the same version (`namespace engine.world is
  version 2, module was built against version 1`);
- a different index hash alone is not an error: if all imports and versions match, the
  module links (functions were added or docs changed). `Imports::check_module` lists every
  incompatibility at once, by name, and says which hash the module was built against.

## Versioning

Additive changes (new keys) keep `version` 1. A change that old readers would misread
bumps it. Namespace versions are owned by the namespace owner and are bumped on an
incompatible change to any function in the namespace.

## Reader and registry details (recorded during implementation)

- The reader accepts a file if its `hash` matches either the definition above (the file text
  with the top-level `game` and `hash` lines removed) or the hash of its canonical
  re-serialisation. The first covers keys a newer version-1 writer may add; the second lets a
  reformatted file with the same content load. It rejects duplicate keys and any `version`
  other than 1, and sorts what it reads instead of requiring sorted input.
- `HostRegistry::unregister_owner` returns `Result<usize, RegistryError>`: it refuses
  (`StructInUse`) and removes nothing when another owner still uses one of the owner's structs.
- `Imports::check_module` appends `LinkError::ApiHash { module, host }` after real problems,
  to say which index the module was built against; `Instance::new` never fails with it.

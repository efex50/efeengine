# Scenes: design and roadmap

Status: planned. Nothing in the engine implements scenes yet. Roadmap items: 3.7 (load/save), 7.3 (snapshot), 7.7 (seamless restart). This document is the detailed plan for them.

## What a scene is

A scene is a named, versioned file that describes one playable state of the world:

- **entities**: each with its components, written with registry names (`Transform`, `mod:dash/Coin`), so the file is readable and survives code reordering;
- **active systems**: the systems that run while this scene is loaded (see below);
- **start settings**: anything the scene needs before its first frame (for example the camera it renders through, or the window it targets).

Scenes are data. Rust and script code decide what happens inside them.

## Should a scene say which systems it activates?

**Yes.** Reasons:

1. Today the only way to switch behaviour is `set_system_enabled(name, bool)` called from game code. Each scene change then needs hand-written toggles, and a forgotten toggle leaves the previous level's logic running.
2. The list is data the editor can show and edit (7.5), and the loader can check it against the registry, the same way `cargo xtask check-scripts` checks `needs-mod`.
3. Scenes that share systems (for example a menu and a level both using the UI system) just list them; no code is repeated.

The scene should not carry logic. It only chooses from systems the game and mods already register.

### Proposed data model

```text
Scene {
    format: 1,                        // versioned, see Phase S1
    name: "level_1",
    requires: ["coin_dash_mods"],     // mods/plugins that must be loaded; error if missing
    systems: ["coin_collect", "hud"], // systems to enable while this scene is active
    entities: [ ... ],                // components by registry name; an entity may also carry `id` and `name`
}
```

- `SceneEntity::name` (optional) is the entity's `Name` component (Rust-only, so it is not one of `components`): capture, snapshot, restart snapshot and `save_world_scene` write it, load and restore put it back. Format stays 1 (the field is optional, older files load unchanged).
- `systems` names systems, or whole plugins with `plugin:<name>`, which the engine already tags on every system.
- Systems the **project** marks `global` (set in `project.toml`, for example the engine's transform propagation and the renderer) are never toggled by scenes.
- A system that no scene lists and is not global is left as the game set it. This keeps existing code working.

### Switching rules

`Engine::load_scene(name)` runs at a frame boundary, through `ctx.schedule()` so it is safe from inside a system:

1. Despawn every entity owned by the current scene (ownership is a component, `SceneOwned(scene_id)`, the same pattern as `SpawnedBy` for mods).
2. Disable the systems the current scene listed that the new scene does not list.
3. Enable the systems the new scene lists.
4. Spawn the new scene's entities, tagged `SceneOwned`.
5. Emit a `SceneLoaded` event, so scripts and Rust systems can react.

A failed load (unknown system, missing mod, bad component) must leave the old scene running and report the error.

### Open questions

- Should a scene also list **resources** (for example a `Score` resource) to reset on load? Recommendation: yes, but only resources that the scene's `init` declares; do this in phase S3, not earlier.
- Should scenes stack (a pause menu over a level)? Recommendation: not in the first version. Start with one active scene, and add a stack later if the game needs it.
- Keep the system list as plain names, or allow run conditions in the file? Recommendation: names only. Run conditions stay in Rust, where they are typed.

## Roadmap

### Phase S1: format and load/save (roadmap 3.7)

- [x] S1.1 **[S] Scene struct and versioned RON file.** Add `serde` derives (`serde` is already a dependency of `crates/core`) and a RON reader and writer. Fields as above; `format` is required.
  _Done: `crates/core/src/engine/scene.rs` (`Scene`, `SceneEntity`, `SceneValue`, RON via the `ron` crate)._
- [x] S1.2 **[S] Component codec through the type registry.** Read and write each component by registry name, using the field layout from 3.4 (`TypeRegistry`, `ScriptType`). Mod components use their `mod:<mod>/<Component>` name.
  _Done: `crates/core/src/engine/scene.rs` (`encode_component` / `decode_component` over `TypeRegistry`)._
- [x] S1.3 **[S] Save.** `Engine::save_scene(path)` writes the current scene's `SceneOwned` entities. Save output is stable (sorted entities and fields) so files diff cleanly.
  _Done: `Engine::save_scene` / `capture_scene` in `crates/core/src/engine/scene.rs` (all registered, non-readonly components; sorted; `SceneOwned` comes in S2)._
- [x] S1.4 **[S] Load and validate.** Parse, check `requires`, check every `systems` entry exists, and check each component against the registry. Errors name the scene, entity index and field.
  _Done: `Engine::load_scene_str` / `load_scene_file` / `validate_scene` in `crates/core/src/engine/scene.rs`._
- [x] S1.5 **Test.** A scene round-trips byte for byte (`tests/scene.rs`), and a bad file returns an error without changing the world.
  _Done: `crates/core/tests/scene.rs`._

_Done when_: a scene file with two entities and two systems loads, runs, saves, and loads again with identical output.

### Phase S2: switching (no snapshots yet)

- [x] S2.1 **[S] `SceneOwned` component and ownership.** Tag entities spawned by a scene; the same rule as `SpawnedBy` for mods.
  _Done: `SceneOwned` in `crates/core/src/engine/scene.rs` (not in the TypeRegistry, so never saved)._
- [x] S2.2 **[S] `Engine::load_scene` and unload.** Implements the six switching steps above, applied at a frame boundary.
  _Done: `Engine::load_scene` / `unload_scene` in `crates/core/src/engine/scene.rs`; deferred via `Commands::load_scene` (`crates/core/src/engine/commands.rs`)._
- [x] S2.3 **[S] `systems` and `global` in `project.toml`.** Systems are toggled through the existing `set_system_enabled`, not new scheduler code.
  _Done (deviation): no project.toml yet; `Engine::set_global_system` in `crates/core/src/engine/scene.rs`._
- [x] S2.4 **[S] `SceneLoaded` / `SceneUnloaded` events.** Rust and script code can react to a switch.
  _Done: Rust events `SceneLoaded` / `SceneUnloaded` in `crates/core/src/engine/scene.rs` (no script-side event yet)._
- [x] S2.5 **Test.** Switching A → B leaves no entity from A, disables only A's systems, and a failed load keeps A running (`tests/scene_switch.rs`).
  _Done: `crates/core/tests/scene_switch.rs`._

_Done when_: a game with two scenes changes between them, with the correct systems running in each, from a script call.

### Phase S3: snapshots (roadmap 7.3)

- [x] S3.1 **[S] `Engine::snapshot()` / `restore()`.** A snapshot is a scene-format value kept in memory: the same data as a scene file, plus the world's tick and the enabled-system set.
  _Done: `Engine::snapshot` / `restore`, `Snapshot` in `crates/core/src/engine/scene.rs` (all captured entities with ownership, system flags per stage, tick, current scene)._
- [x] S3.2 **[S] Restore keeps identity.** Restoring maps entities by a stable id stored in the scene, so references between entities still resolve.
  _Done: entity `id` on `SceneEntity` plus `remap_entities` in `crates/core/src/engine/scene.rs` (respawn all, then rewrite Entity-typed fields)._
- [x] S3.3 **Test.** Snapshot, change the world, restore: the world equals the snapshot, including system enabled flags.
  _Done: `crates/core/tests/snapshot.rs`._

_Done when_: a snapshot restores a running world exactly (roadmap 7.3).

### Phase S4: editor support (roadmap 7.5, 7.7)

- [ ] S4.1 **Edit and play modes.** The editor takes a snapshot on play and restores it on stop, unless "keep changes" is chosen.
- [ ] S4.2 **Seamless restart.** When a rebuild changes a component layout, the editor snapshots, restarts, and restores with fields mapped by name (roadmap 7.7).
- [ ] S4.3 **Edit the system list.** The editor shows the systems a scene lists, with the registered names, and writes changes back to the scene file.

### Phase S5: prefabs and reuse (after S4)

- [ ] S5.1 **[S] Prefab files.** A prefab is a named bundle of components (for example `Coin`, `Player`), stored as a file with the same component codec as scenes (S1.2).
- [ ] S5.2 **[S] Placing prefabs.** A scene entry can say `prefab: "Coin"` with optional field overrides. Overrides are checked against the registry at load time.
- [ ] S5.3 **[S] Prefab edits propagate.** Changing a prefab file updates every scene entity placed from it on the next load. Overrides stay.
- [ ] S5.4 **Test.** Placing one prefab twice with different overrides gives two entities with the right values (`tests/prefab.rs`).

_Done when_: a level places the same prefab ten times, and editing the prefab changes all ten after a reload.

### Deferred: scenes that include scenes

Not planned yet. The idea is a scene file that includes another scene (a HUD, a level's player setup), with the included entities owned by the parent and its systems added to the parent's list. Start it only when a real project duplicates setup code across scenes. It needs a rule for system ownership when two included scenes list the same system.

## Not in scope (first version)

- Streaming or background loading of scenes.
- Scene stacking (see open questions).
- Networked scene sync.
- Scenes that include scenes (deferred, see above). Prefabs are in phase S5.

# Systems: order, enabled, run conditions, late add and remove

Source: `crates/core/src/engine/{schedule,commands,plugin}.rs`, `engine.rs`. Tests:
`crates/core/tests/{systems,order,parallel}.rs`. How par systems run side by side is in
[THREADING.md](THREADING.md).

## The rule

**Register API in setup or `Plugin::build`; gate late features with `enabled` / `run_if`.**

API means everything scripts and mods can see: host functions, script events, structs, components
and plugins. It is registered between `Engine::new` and the start of the game loop. Once
`game_loop()` starts (or the first `frame` / `run_frames` / `tick` runs, for engines driven by hand)
the registration is closed:

| Closed after start (returns an error) | Still open |
|---|---|
| `register_host_fn`, `register_component`, `register_component_owned`, `register_struct`, `register_event` (script events), `add_plugin` | `add_system`, `add_par_system`, `insert_resource`, `add_event` (Rust events), `set_renderer`, `set_system_enabled`, `remove_system` |

The error says: "API registration is closed once the game loop starts; register in setup or
Plugin::build (mods declare theirs in mod.toml)" (`RegistryError::Closed`, `TypeError::Closed`,
`PluginError::Closed`). `Engine::api_open()` tells which state the engine is in.

Why: the API index (`.efapi`) is written when the loop starts, and scripts are compiled against it.
An index that can grow while the game runs would make that file a lie. A feature that appears late
(a menu, a debug tool, a boss fight) is a system that exists from the start and is switched on:

```rust
engine.add_system(Stage::Update, boss_ai).name("boss_ai").enabled(false);
// later, from anywhere:
engine.set_system_enabled("boss_ai", true)?;
```

Mods are exempt: what a mod declares (provides, events, structs, `[[meta.component]]`) is
registered when the mod loads, at any time, and removed when it unloads or reloads. They do not go
through the locked functions. `Engine::reopen_api()` (`#[doc(hidden)]`) opens registration again
for a future editor or hot reload of game code; a shipped game does not call it.

## Order

A stage runs its systems sorted by `(order, registration index)` (constants in `order`, default
`order::USER`), then batches them by declared access. See THREADING.md for the batches.

## Enabled and run conditions

```rust
engine.add_system(Stage::Update, f).name("f").order(10).enabled(false);
engine.add_system(Stage::Update, g).run_if(|rc| rc.res::<Paused>().is_some_and(|p| !p.0));
```

- `.enabled(bool)` (default `true`) is the switch; `Engine::set_system_enabled(name, bool)` flips
  it later and `is_system_enabled(name)` reads it. Several systems may share a name: the setter
  changes all of them, the getter says whether any is enabled. An unknown name is
  `ScheduleError::UnknownSystem`. The name is the one from `.name("..")`, else the closure's type name.
- `.run_if(cond)`: `cond: Fn(&RunCtx) -> bool + Send + Sync`. `RunCtx` is read-only: `world`,
  `types`, `resources` (`rc.res::<T>()`), `time`, `input`, `gamepads`, `window`. It runs on the main
  thread, every time the stage runs (every fixed step in `FixedUpdate`), in sorted system order,
  before the stage's batches are formed.
- A disabled system, or one whose condition is false, is **skipped when batches are formed**: it
  does not split a batch of par systems around it (`Engine::batch_count` counts enabled systems;
  conditions are not evaluated there).
- It does not advance its **change tick**: `last_run` stays where it was. When it runs again it sees
  every change (`Changed<T>`, `Added<T>`) since it last ran, once. That is the same as Bevy.
- `system_names(stage)` lists disabled systems too; `system_states(stage)` gives `(name, enabled)`.

## Late add and remove

`add_system` / `add_par_system` work at any time (also after the loop started, also from a plugin
or from outside the schedule). A late system gets the next registration index, so with an equal
order number it runs after the systems added before it. A `Startup` system added after `Startup`
ran never runs. `Engine::remove_system(name) -> Result<usize, ScheduleError>` removes every system
of that name from all stages and returns how many it removed.

## Deferred edits from inside a system

A running system cannot touch the schedule it is part of. It queues edits instead:

```rust
engine.add_system(Stage::Update, |cx| {
    cx.schedule().add_system(Stage::Update, |_| {}).name("later").order(5);
    cx.schedule().set_enabled("boss_ai", true);
    cx.schedule().remove("tutorial");
});
```

`ctx.schedule()` ([`ScheduleCommands`]) offers `add_system`, `add_par_system`, `set_enabled` and
`remove`; `add_*` return a handle with `.order/.name/.enabled/.run_if`. The edits are applied at
the end of the current stage, after the world `Commands`, in queue order. A system added this way
first runs the next time its stage runs (a later stage of the same frame already sees it). Names
that match nothing are logged and skipped. Late-added systems must be `Send` (the queue is part of
`Commands`, which par systems fill too).

Par systems queue through `cx.commands().schedule()` with the same methods. Their queues merge in
sorted system order, like their world commands, so the result is the same for every thread count.

## Components

```rust
#[script_component]                 // repr(C) + Clone, Copy, Pod, Zeroable, ScriptType
#[derive(Default, Debug)]
pub struct Patrol { pub origin: Vec2, pub range: f32 }

#[script_component(name = "Hp", tracked)]   // other script name, change tracking on
pub struct Health { pub hp: f32 }

#[script_component(readonly)]       // scripts read it; set / insert / remove give status 5
pub struct Score { pub value: i64 }

#[derive(Component)]                // Rust-only, optional: only adds engine metadata
#[component(tracked)]
pub struct Target(pub Entity);

engine.register::<Patrol>()?;       // one call for both kinds
```

`register` records the plugin being built as the owner (else `engine`) and fails with
`TypeError::Closed` once the game has started. Plain types still work as components without any
macro. A `readonly` component is checked when a script runs (status 5), not by the assembler.

## Imports

Three layers, all public:

- `use efeengine_core::prelude::*;` is the everyday set (engine, config, systems, components,
  `Entity`/`World`, `Vec2`.., `Res`, `Time`, common keys, log functions).
- Domain modules at the crate root are extra front doors for rarer things: `render` (renderer
  authors: `Renderer`, `Gpu`, `RenderFrame`, `wgpu`, `bytemuck`, ...), `input`, `scripting`, `mods`,
  `ecs`, `math` (all of glam), `cache`, `log`, `thread`. Import them where needed, e.g.
  `use efeengine_core::render::*;`.
- `efeengine_core::engine::...` is the full internals (`engine::window`, `messages`, `types`,
  `schedule`, ...), as public as the code makes it. The facades hide nothing.

The crate root also re-exports `hecs`, `glam`, `wgpu`, `bytemuck`, `efevm`, `efemod`, `efa`. The
full list is in `crates/core/structure.md`.

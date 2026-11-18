# Multiplayer: design and roadmap

Status: planned. Nothing of multiplayer exists yet. This document is the plan; nothing here is built.

## The idea

Systems are tagged by where they run, and the build decides which ones exist:

- `#[server_system]`: only in server builds. Authoritative logic: spawning, damage, scoring, validating input.
- `#[client_system]`: only in client builds. Rendering, sound, UI, camera, prediction display.
- `#[hybrid_system]`: in both. Shared game rules (movement, collision, cooldowns) that the client predicts and the server confirms.

A headless server build has no client systems at compile time, so client-only code cannot leak into the server or slow it down.

**Verdict:** this is the right shape, with three things to add. The compile-time split alone is not enough, the engine's dependencies must be split first, and hybrid systems need a deterministic rule.

## What the engine has and lacks today

Has, and useful here:
- A typed world (hecs fork) with **change ticks** (`Changed<T>`, 3.x). Replication can send only what changed since a client's last acknowledged tick.
- A **type registry** with stable names and byte layouts (3.4, `#[script_component]`). This is the wire format for components.
- **Snapshot and restore** (S3). Prediction rollback can start from this.
- **FixedUpdate** and fixed-delta systems (3.6). Simulation runs at a fixed rate, independent of the frame rate.
- **Parallel execution that gives the same world for any thread count** (2.8, tested). Same inputs give the same result on server and client.
- **Scenes** (S1–S2): both sides can load the same scene file by name.

Lacks, and must be added:
- **The engine links SDL3 and wgpu on native unconditionally** (`crates/core/Cargo.toml`: `sdl3`, `sdl3-sys`, `wgpu`, not optional). A headless server build cannot be made today. This is milestone M0.
- **Render and window components** (`Sprite`, `Camera`, `TextureHandle`) live in the core crate with no feature gate.
- **No networking, no replicated-component marker, no stable network ids.**

## The side model

### Features

```toml
[features]
default = ["client"]
client = ["dep:sdl3", "dep:wgpu", ...]   # window, input, render, audio, UI
server = []                              # headless: no window, no GPU
```

- `cargo build -p mygame --no-default-features --features server` builds a headless server. A CI test checks that the server's dependency tree has no `sdl3` or `wgpu`.
- A game that wants both builds with both features. The `hybrid` side is always present.
- The engine's own render and window code sits behind `client`. Components that only make sense on screen (`Sprite`, `Camera`) are behind it too, so a server build cannot reference them.

### Macros

```rust
#[server_system(order = order::UPDATE)]
fn apply_damage(hits: Query<&Hit>, ...) { ... }

#[client_system]
fn draw_health_bars(...) { ... }

#[hybrid_system(fixed)]
fn move_players(input: Res<PlayerInput>, ...) { ... }
```

Behaviour:
- Each macro emits the function plus a `SystemDesc` (name, side, order, access). It emits nothing for a side that is not compiled, so the function does not exist in that build.
- Registration is **explicit**, not link-time collection. The game lists its systems once:
  ```rust
  register_systems!(engine, side = Server);   // expands to the server and hybrid entries
  ```
  Explicit lists avoid `inventory`-style linker tricks, which do not work reliably on wasm. A test checks that every tagged function appears in the list.
- A hybrid system must not check the side with `cfg`. It reads the **`Role` resource** (`Role::Server`, `Role::Client`) for the few places where behaviour differs (for example, the client skips audio). Keeping one code path is what makes prediction match the server.

### Authority

Compile-time side and runtime authority are different things. The server is authoritative in every build that runs it; a client only predicts. The rules:

1. Only `server_system` and `hybrid_system` may write replicated components on the server.
2. Client input is data. The server validates it (range, rate, cooldown) before a hybrid system uses it.
3. A client never sends positions or health as truth. It sends inputs, and the server replies with state.

## Networking model

**Recommended: server-authoritative state replication with client-side prediction.** The server runs the simulation, clients send inputs, the server sends changed components, and the client predicts its own avatar and corrects from the server.

Not recommended for the first version: **lockstep** (every peer simulates the same inputs). It needs bit-exact determinism across native and wasm and across machines, which f32 does not guarantee. Prediction with server correction tolerates small differences; lockstep does not.

Components:
- **`NetId`**: a stable network id per replicated entity, assigned by the server (not the hecs id). Same role as `EditorId`.
- **`#[replicated]`**: a registry flag on a component type. The registry's byte layout is the wire format; a component's fields are written in declared order, little-endian.
- **Delta snapshots**: each client has a last acknowledged tick; the server sends the components whose `Changed<T>` tick is newer than that, plus despawns.
- **Input**: each client sends its inputs tagged with the tick they apply to. The server keeps a small input buffer per client.
- **Prediction and reconciliation**: the client runs the hybrid systems ahead of the server, keeps a history of its own states, and on a server correction it restores the state for that tick (`restore` from a partial snapshot) and replays the later inputs.
- **Interpolation**: remote entities are shown a little in the past, between two received states, so motion looks smooth.

## Transport

- **Native dev transport:** UDP with a small reliable layer, or TCP for the first prototype. Loopback tests first, then real sockets.
- **Browser clients:** browsers cannot open raw UDP. The web build needs WebSocket or WebTransport, which means the server needs a second transport. Plan it, do not build it first.
- **Simulated network:** a test layer that adds latency, jitter and loss, so prediction is tested without a real network.

## Roadmap

Each phase ends with something that runs. Phases M0–M1 do not need a network.

### M0: headless builds (prerequisite)

- [ ] M0.1 **[S] Feature split.** `client` and `server` features in `crates/core`. SDL3, wgpu, window, render and audio code behind `client`. _Done when_ `cargo check -p efeengine-core --no-default-features --features server` passes.
- [ ] M0.2 **[S] Render and window components behind `client`.** `Sprite`, `Camera`, `TextureHandle`, `Visibility` are gated; the server build does not reference them. Engine systems that need them are gated the same way.
- [ ] M0.3 **[S] CI dependency test.** A script or test that asserts the server build's dependency tree has no `sdl3` or `wgpu`.
- [ ] M0.4 **[S] `Role` resource.** `Role::Server`, `Role::Client`, set by the game at startup from its build side.
- _Done when_ a headless example (no window, no GPU) runs a fixed-step simulation and prints the world's state each second.

### M1: system sides (the macros)

- [ ] M1.1 **[S] `#[server_system]`, `#[client_system]`, `#[hybrid_system]`** in `efeengine-macros`. Each emits the function and a `SystemDesc` only for the sides it applies to.
- [ ] M1.2 **[S] `register_systems!`** explicit registration, with a test that every tagged function is listed.
- [ ] M1.3 **Compile-fail tests (trybuild).** A client-only component used in a server system fails to compile; a hybrid system that calls a client-only function fails to compile.
- [ ] M1.4 **[S] Server example.** The same game built with `--features server` has no client systems in its binary's system list.
- _Done when_ a game with one of each macro builds in three configurations (client, server, both), and the server build's system list has no client systems.

### M2: replication (no network yet)

- [ ] M2.1 **[S] `NetId` and `#[replicated]`.** Stable ids assigned by the server; replicated components are marked in the registry.
- [ ] M2.2 **[M] Delta encoding.** A serializer that writes the components changed since a tick, and a decoder that applies them, using `Changed<T>` and the registry layout. Despawns are included.
- [ ] M2.3 **Tests.** Encode then decode gives the same world; a second delta after no change is empty; a despawn is replicated.
- _Done when_ a server world and a client world stay equal over 1000 simulated ticks with random changes.

### M3: transport

- [ ] M3.1 **[M] Native transport.** UDP with a reliable channel for state and an unreliable one for inputs, or TCP for the prototype. Connection handshake with protocol version (as the editor link does).
- [ ] M3.2 **[S] Simulated network.** Latency, jitter and packet loss in tests.
- [ ] M3.3 **[S] Join and leave.** Client connects, receives a full snapshot, then deltas; disconnects clean up its entities.
- _Done when_ two clients and one headless server run on localhost and both see the same world.

### M4: inputs and ticks

- [ ] M4.1 **[S] Input messages tagged by tick.** Client sends inputs for future ticks; server buffers them.
- [ ] M4.2 **[S] Server validation.** Hybrid and server systems read validated inputs only (rate, range, cooldown checks).
- _Done when_ a client's input moves its avatar on the server with the correct tick.

### M5: prediction and reconciliation

- [ ] M5.1 **[M] Client prediction.** The client runs hybrid systems ahead, keeping a state history per tick.
- [ ] M5.2 **[M] Reconciliation.** On a server correction, restore the state for that tick (partial snapshot), replay later inputs.
- [ ] M5.3 **[S] Interpolation** for remote entities.
- _Done when_ with 150 ms simulated latency and 5% loss, a client's own avatar follows its input without visible snapping, and remote avatars move smoothly.

### M6: rooms and scenes

- [ ] M6.1 **[S] Scene sync.** Server and clients load the same scene by name (S1–S2); the server sends the scene name and requirements on join.
- [ ] M6.2 **[S] Rooms.** One server, several independent worlds (rooms), each with its own set of clients.
- _Done when_ two rooms run on one headless server without seeing each other's entities.

### M7: hardening

- [ ] M7.1 **[S] Bandwidth budget.** Per-client byte budget per tick; priority for nearby entities.
- [ ] M7.2 **[M] Load test.** A headless server with N simulated bots, reporting tick time and bandwidth.
- [ ] M7.3 **[S] Browser transport.** WebSocket or WebTransport so a web client can join (see Transport).
- _Done when_ a headless server holds 50 bots at the target tick rate on one core, and a browser client joins a native server.

## Dependencies on the roadmap

- Phase 2 (engine loop, FixedUpdate) and 3.x (change ticks, type registry): done.
- S1–S3 (scenes and snapshots): done. M5 relies on partial snapshots, which need a small extension to S3.
- Phase 4 (renderer): only the `client` side. M0 must not wait for it.
- The editor (E-phases) is independent; the editor link is a client-side tool and stays behind `client`.

## Open questions

- **Authority model:** server-authoritative with prediction (recommended), or lockstep? Lockstep needs determinism across platforms; decide after M5 measurements.
- **Partial snapshot cost:** full-world snapshot per tick is too expensive for prediction. Recommendation: snapshot only components marked `#[predicted]`.
- **Web clients:** do browser players need to be in the first release? If not, M7.3 can wait.
- **Hosting:** dedicated server only, or also listen-server (one client hosts)? Recommendation: dedicated first; a listen-server is the same binary with the `client` and `server` features both on.

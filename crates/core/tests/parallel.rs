//! Determinism of hybrid parallel scheduling (docs/PARALLEL_SPEC.md section 8): the same game
//! run with `Threads::Single` and `Threads::Fixed(4)` must give a byte-identical world.
//!
//! The game: ~2k entities, par systems (the built-in velocity system plus four custom ones on
//! disjoint and overlapping components, one with `res_mut`, one that spawns and despawns through
//! `ParCommands`), exclusive systems, and three script modules (two of them `set` the same
//! `Transform`s every frame and all spawn entities from their pools, one is a `Script` entry).

use efeengine_core::efevm::ValType::{F32, I64};
use efeengine_core::efevm::*;
use efeengine_core::prelude::*;

const FRAMES: usize = 300;
const ENTITIES: usize = 2000;
const REPEATS: usize = 20;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
struct Health {
    hp: f32,
    regen: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
struct Heat {
    value: f32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
struct Tag {
    n: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
struct Life {
    ttl: i32,
    _pad: u32,
}

/// Shared by `life` (par, `res_mut`) and the exclusive systems.
#[derive(Default)]
struct Stats {
    frames: u64,
    spawned: u64,
    despawned: u64,
}

// ---- script modules ----

fn import(mb: &mut ModuleBuilder, e: &Engine, ns: &str, name: &str) -> u32 {
    let f = e.api_index().function(ns, name).unwrap_or_else(|| panic!("{ns}.{name} is not in the index")).func_type();
    mb.import(ns, name, f)
}

fn tid(e: &Engine, name: &str) -> i32 {
    e.types().by_name(name).unwrap().id() as i32
}

/// `frame(dt)`: set `Transform.position.x = x` of every entity in `targets`.
fn setter(e: &Engine, targets: &[Entity], x: f32) -> Module {
    let mut mb = ModuleBuilder::new();
    let get = import(&mut mb, e, "engine.world", "get");
    let set = import(&mut mb, e, "engine.world", "set");
    mb.memory(1, None);
    let t = tid(e, "Transform");
    let mut f = FunctionBuilder::new(64);
    for ent in targets {
        let bits = entity_to_bits(*ent) as i64;
        f.const_i64(10, bits);
        f.const_i32(11, t);
        f.const_i64(12, 1024);
        f.call(get, 10);
        f.const_i64(13, 0);
        f.const_f32(14, x);
        f.store(Op::F32Store, 14, 13, 1024);
        f.const_i64(10, bits);
        f.const_i32(11, t);
        f.const_i64(12, 1024);
        f.call(set, 10);
    }
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

/// `frame(dt)`: `n` times spawn an entity and insert `Life { ttl }` into it.
fn spawner(e: &Engine, n: usize, ttl: i32) -> Module {
    let mut mb = ModuleBuilder::new();
    let spawn = import(&mut mb, e, "engine.world", "spawn");
    let insert = import(&mut mb, e, "engine.world", "insert");
    mb.memory(1, None);
    let life = tid(e, "Life");
    let mut f = FunctionBuilder::new(64);
    f.const_i64(13, 0);
    f.const_i32(14, ttl);
    f.store(Op::I32Store, 14, 13, 1024);
    for _ in 0..n {
        f.call(spawn, 20);
        f.mov(30, 20);
        f.const_i32(31, life);
        f.const_i64(32, 1024);
        f.call(insert, 30);
    }
    f.ret(0);
    let id = mb.add_func(FuncType::new([F32], []), f);
    mb.export_func("frame", id);
    mb.finish().unwrap()
}

/// `entry(entity, dt, state) -> state + 1`
fn counter() -> Module {
    let mut mb = ModuleBuilder::new();
    let mut f = FunctionBuilder::new(16);
    f.const_i64(10, 1);
    f.op3(Op::I64Add, 11, 2, 10);
    f.mov(0, 11);
    f.ret(0);
    let id = mb.add_func(FuncType::new([I64, F32, I64], [I64]), f);
    mb.export_func("entry", id);
    mb.finish().unwrap()
}

// ---- the game ----

/// A tiny deterministic generator for the initial world.
fn lcg(s: &mut u32) -> f32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    (*s >> 8) as f32 / (1u32 << 24) as f32
}

fn build(threads: Threads) -> Engine {
    build_with(threads, false)
}

/// `mixed`: give the custom Update systems explicit order numbers (not their registration order).
fn build_with(threads: Threads, mixed: bool) -> Engine {
    let cfg = EngineConfig { fixed_dt: 1.0 / 60.0, threads, ..EngineConfig::headless() };
    let mut e = Engine::with_config(cfg).expect("headless engine");
    for r in [
        e.register_component::<Health>(),
        e.register_component::<Heat>(),
        e.register_component::<Tag>(),
        e.register_component::<Life>(),
    ] {
        r.expect("register");
    }
    e.insert_resource(Stats::default());

    let mut seed = 12345u32;
    let mut targets = Vec::new();
    for i in 0..ENTITIES {
        let pos = Vec2::new(lcg(&mut seed) * 100.0, lcg(&mut seed) * 100.0);
        let vel = Velocity { linear: Vec2::new(lcg(&mut seed) - 0.5, lcg(&mut seed) - 0.5) * 10.0, angular: 0.0 };
        let hp = Health { hp: 50.0 + lcg(&mut seed) * 50.0, regen: lcg(&mut seed) };
        let ent = e.world_mut().spawn((Transform::from_position(pos), vel, hp, Heat::default(), Tag { n: i as u32, _pad: 0 }));
        if i % 4 == 0 {
            let ttl = 50 + (lcg(&mut seed) * 300.0) as i32;
            e.world_mut().insert_one(ent, Life { ttl, _pad: 0 }).unwrap();
        }
        if i < 5 {
            targets.push(ent);
        }
    }

    // par: disjoint from `tag` (batched together), overlaps `health` (heat <-> health)
    e.add_par_system(
        Stage::Update,
        Access::new().named("heat").write::<Heat>().read::<Health>(),
        |ctx| {
            ctx.par_for_each::<(&mut Heat, &Health), _>(256, |(h, hp)| {
                h.value = h.value * 0.99 + hp.hp * 0.01;
            });
        },
    ).order(if mixed { 30 } else { 0 });
    e.add_par_system(
        Stage::Update,
        Access::new().named("tag").write::<Tag>().read::<Transform>(),
        |ctx| {
            ctx.par_for_each::<(&mut Tag, &Transform), _>(256, |(tag, t)| {
                tag.n = ((t.position.x * 10.0) as u32) ^ tag.n.rotate_left(1);
            });
        },
    ).order(if mixed { 10 } else { 0 });
    e.add_par_system(
        Stage::Update,
        Access::new().named("health").write::<Health>().read::<Heat>().read::<Velocity>(),
        |ctx| {
            ctx.par_for_each::<(&mut Health, &Heat, &Velocity), _>(256, |(hp, h, v)| {
                hp.hp = (hp.hp + hp.regen * 0.1 - h.value * 0.001 - v.linear.length() * 0.001).clamp(0.0, 100.0);
            });
        },
    ).order(if mixed { 20 } else { 0 });
    // par with a resource and structural changes through commands
    e.add_par_system(
        Stage::Update,
        Access::new().named("life").write::<Life>().res_mut::<Stats>(),
        |ctx| {
            let mut stats = ctx.res_mut::<Stats>();
            stats.frames += 1;
            let mut dying = Vec::new();
            for (ent, life) in ctx.query::<(Entity, &mut Life)>().iter() {
                life.ttl -= 1;
                if life.ttl <= 0 {
                    dying.push(ent);
                }
            }
            for ent in dying {
                ctx.commands().despawn(ent);
                stats.despawned += 1;
            }
            let ttl = 10 + (stats.spawned % 17) as i32;
            ctx.commands().spawn((
                Transform::from_position(Vec2::splat(stats.spawned as f32)),
                Life { ttl, _pad: 0 },
                Tag { n: stats.spawned as u32, _pad: 0 },
            ));
            stats.spawned += 1;
        },
    ).order(if mixed { -5 } else { 0 });
    // exclusive systems
    e.add_system(Stage::Update, |ctx| {
        let frames = ctx.resources.get::<Stats>().map(|s| s.frames).unwrap_or(0);
        if frames % 25 == 0 {
            ctx.spawn((Transform::from_position(Vec2::splat(frames as f32)), Velocity::default(), Health { hp: 1.0, regen: 0.5 }));
        }
    });
    e.add_system(Stage::PostUpdate, |ctx| {
        let frames = ctx.resources.get::<Stats>().map(|s| s.frames).unwrap_or(0);
        if frames % 10 == 0 {
            for h in ctx.world.query_mut::<&mut Health>() {
                h.hp += 1.0;
            }
        }
    });

    // scripts: `a` and `b` set the same Transforms (write conflicts) and all spawn
    e.load_module("a", &setter(&e, &targets, 3.0).encode()).unwrap();
    e.load_module("b", &setter(&e, &targets, 7.0).encode()).unwrap();
    e.load_module("sp1", &spawner(&e, 2, 20).encode()).unwrap();
    e.load_module("sp2", &spawner(&e, 3, 25).encode()).unwrap();
    let c = e.load_module("count", &counter().encode()).unwrap();
    for _ in 0..3 {
        e.world_mut().spawn((Script::new(c, "entry"),));
    }
    e
}

// ---- the world hash ----

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf29ce484222325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 = (self.0 ^ *x as u64).wrapping_mul(0x100000001b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

fn hash_world(e: &Engine) -> u64 {
    let world = e.world();
    let mut ents: Vec<Entity> = world.iter().map(|r| r.entity()).collect();
    ents.sort_by_key(|x| entity_to_bits(*x));
    let mut entries: Vec<_> = e.types().entries().iter().filter(|t| t.is_component()).collect();
    entries.sort_by_key(|t| t.id());
    let mut h = Fnv::new();
    let mut buf = Vec::new();
    h.u64(ents.len() as u64);
    for ent in ents {
        h.u64(entity_to_bits(ent));
        for t in &entries {
            if t.has(world, ent) {
                buf.clear();
                buf.resize(t.size() as usize, 0);
                t.read(world, ent, &mut buf).unwrap();
                h.u64(t.id() as u64);
                h.bytes(&buf);
            }
        }
        if let Ok(s) = world.get::<&Script>(ent) {
            h.u64(s.state as u64);
        }
    }
    h.0
}

/// Run the game; returns the world hash, the summed write conflicts and the entity count.
fn run(threads: Threads) -> (u64, u64, usize, u64) {
    run_with(threads, false)
}

fn run_with(threads: Threads, mixed: bool) -> (u64, u64, usize, u64) {
    let mut e = build_with(threads, mixed);
    let mut conflicts = 0;
    for _ in 0..FRAMES {
        e.run_frames(1, 1.0 / 60.0);
        conflicts += e.script_stats().write_conflicts;
    }
    let errs: Vec<_> = e.scripting().log_lines().filter(|l| l.level == LogLevel::Error).map(|l| l.text.clone()).collect();
    assert!(errs.is_empty(), "{errs:?}");
    let (spawned, despawned) = {
        let s = e.resources().get::<Stats>().unwrap();
        (s.spawned, s.despawned)
    };
    assert!(spawned > 0 && despawned > 0, "commands spawned and despawned entities");
    (hash_world(&e), conflicts, e.world().len() as usize, e.script_stats().spawn_overflows)
}

#[test]
fn single_and_fixed_4_give_the_same_world_every_time() {
    let (single, conflicts, n, _) = run(Threads::Single);
    assert!(n >= 1000, "{n} entities");
    assert!(conflicts > 0, "the two setter modules must conflict (got {conflicts})");
    let mut seen = Vec::new();
    for i in 0..REPEATS {
        let (h, c, n2, _) = run(Threads::Fixed(4));
        seen.push(h);
        assert_eq!(n2, n, "run {i}: entity count");
        assert_eq!(c, conflicts, "run {i}: write conflicts");
        assert_eq!(h, single, "run {i}: world hash differs from Single");
    }
    assert_eq!(seen.len(), REPEATS);
}

#[test]
fn the_hash_changes_when_the_world_changes() {
    // guards the hash itself: a different thread count must not matter, a different game must
    let (a, ..) = run(Threads::Single);
    let mut e = build(Threads::Single);
    e.run_frames(FRAMES, 1.0 / 60.0);
    let t = e.world().iter().find_map(|r| r.get::<&Transform>().map(|_| r.entity())).unwrap();
    e.world_mut().get::<&mut Transform>(t).unwrap().position.x += 1.0;
    assert_ne!(hash_world(&e), a);
}

#[test]
fn mixed_orders_are_deterministic_across_thread_counts() {
    let (plain, ..) = run_with(Threads::Single, false);
    let (single, ..) = run_with(Threads::Single, true);
    assert_ne!(single, plain, "explicit orders change the result");
    for i in 0..5 {
        let (h, ..) = run_with(Threads::Fixed(4), true);
        assert_eq!(h, single, "run {i}");
    }
}

//! The engine's host functions (owner `engine`, version 1), registered in the one
//! `HostRegistry<ScriptHost>`. Registering is what makes them callable *and* what puts them in
//! the API index; there is no other list. Status codes of `engine.world`: 0 ok, 1 stale or
//! forged entity, 2 component missing, 3 unknown component id, 4 wrong size, -1 other.

use efevm::{ApiType as T, Caller, EventRef, GuestSlice, HostFn, HostOk, HostRegistry, RegistryError, Results, Trap};
use hecs::{Entity, World};

use super::{LogLevel, ScriptEvent, ScriptHost};
use crate::engine::input::{KeyCode, MouseButton};
use crate::engine::types::{CompError, entity_from_bits, entity_to_bits};

pub(crate) const OWNER: &str = "engine";

const OK: i32 = 0;
const STALE: i32 = 1;
const MISSING: i32 = 2;
const UNKNOWN_ID: i32 = 3;
const WRONG_SIZE: i32 = 4;

fn status_of(e: &CompError) -> i32 {
    match e {
        CompError::NoEntity => STALE,
        CompError::Missing(_) => MISSING,
        CompError::UnknownId(_) | CompError::NotComponent(_) => UNKNOWN_ID,
        CompError::WrongSize { .. } => WRONG_SIZE,
    }
}

fn alive(world: &World, bits: u64) -> Option<Entity> {
    entity_from_bits(bits).filter(|e| world.contains(*e))
}

fn status(res: &mut Results<'_>, v: i32) -> Result<HostOk, Trap> {
    res.set_i32(0, v)?;
    Ok(HostOk::Continue)
}

fn read_name(c: &Caller<'_, ScriptHost>, s: GuestSlice) -> Result<String, Trap> {
    c.read_str(s)
}

pub(crate) fn register_engine_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    for (ns, doc) in [
        ("engine.world", "Entities and their components. Entity handles are u64 values; a stale or forged handle gives status 1, never a crash."),
        ("engine.input", "Keyboard and mouse state of the current frame."),
        ("engine.time", "Frame timing."),
        ("engine.log", "Write to the engine log."),
    ] {
        r.define_namespace(ns, 1, OWNER, doc)?;
    }
    // Version 2: `export` is gone. A mod's functions are declared in its module (section 13);
    // this namespace is how a mod reads the assets of its own package.
    r.define_namespace("engine.mods", 2, OWNER, "Mod support. A mod declares what it provides in its module (section 13), so no function is needed to share one; `asset_len` and `asset_read` read the assets of the calling mod's own package, `meta_f64` a number of its `[meta]` table.")?;
    // Version 2: events are typed. A module lists the events it uses in its events section and
    // passes their ids; the payload is the registered struct (the string-named, raw-bytes
    // `send`/`count`/`read` of version 1 are gone).
    r.define_namespace("engine.events", 2, OWNER, "Typed script events. A module lists the events it sends or reads in its events section (an event's id is its index there); `send`, `count` and `read` take that id, and the payload is the event's registered struct. Events sent in one frame are readable during the next.")?;
    r.define_namespace("engine.script", 1, OWNER, "Per-entity scripts. The `Script` component is Rust-only, so this is how a script attaches one of its own entry functions to an entity.")?;
    r.define_namespace("engine.random", 1, OWNER, "Pseudo-random numbers (xorshift64*), one stream per module. Not for security. The stream starts the same every run; `seed` changes it.")?;
    r.define_namespace("engine.math", 1, OWNER, "Math the instruction set lacks (it has sqrt, abs, floor and friends, but no trigonometry).")?;
    world_api(r)?;
    script_api(r)?;
    random_api(r)?;
    math_api(r)?;
    input_api(r)?;
    time_api(r)?;
    events_api(r)?;
    log_api(r)?;
    mods_api(r)?;
    Ok(())
}

fn world_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.world";
    r.register(
        HostFn::new(NS, "spawn")
            .doc("Take a new empty entity from the calling module's pre-reserved pool and return its handle; it is valid at once and comes alive when the frame's commands apply (add components with insert). When the pool is empty (more spawns in one frame than reserved; the pool grows for the next frame) the null handle 0 is returned, which every function treats as a stale entity.")
            .result("entity", T::Entity),
        |c, _a, res| {
            let bits = c.host().spawn().map_or(0, entity_to_bits);
            res.set_entity(0, bits)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "despawn")
            .doc("Queue despawning `entity` (applied after the stage). Status: 0 ok, 1 stale entity.")
            .param("entity", T::Entity)
            .result("status", T::I32),
        |c, a, res| {
            let bits = a.entity(0)?;
            let h = c.host();
            match alive(h.world(), bits) {
                Some(e) => {
                    h.commands.despawn(e);
                    status(res, OK)
                }
                None => status(res, STALE),
            }
        },
    )?;
    r.register(
        HostFn::new(NS, "alive")
            .doc("1 if `entity` exists (spawned and not despawned), else 0.")
            .param("entity", T::Entity)
            .result("alive", T::Bool),
        |c, a, res| {
            let bits = a.entity(0)?;
            let h = c.host();
            res.set_bool(0, alive(h.world(), bits).is_some())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "get")
            .doc("Copy component `component` (an id from component_id or the index) of `entity` to `out`, which must have room for the component's size. Reads the start-of-frame value, except that a module sees its own earlier `set`s of this frame. Status: 0 ok, 1 stale entity, 2 component missing, 3 unknown component id.")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .param("out", T::Ptr)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, id, out) = (a.entity(0)?, a.i32(1)? as u32, a.ptr(2)?);
            let bytes = {
                let h = c.host();
                let Some(e) = alive(h.world(), bits) else { return status(res, STALE) };
                match h.read_component(e, id) {
                    Ok(buf) => buf,
                    Err(err) => return status(res, status_of(&err)),
                }
            };
            c.write_bytes(out, &bytes)?;
            status(res, OK)
        },
    )?;
    r.register(
        HostFn::new(NS, "set")
            .doc("Overwrite, in place, the existing component `component` of `entity` with the bytes at `src` (the component's size). Buffered: applied after all scripts of the frame ran, in module order (a later module wins over an earlier one); the calling module reads its own write back at once, other modules see the old value until next frame. Status: 0 ok, 1 stale entity, 2 component missing, 3 unknown component id.")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .param("src", T::Ptr)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, id, src) = (a.entity(0)?, a.i32(1)? as u32, a.ptr(2)?);
            let (e, size) = {
                let h = c.host();
                let Some(e) = alive(h.world(), bits) else { return status(res, STALE) };
                let Ok(entry) = h.types().component(id) else { return status(res, UNKNOWN_ID) };
                if !entry.has(h.world(), e) {
                    return status(res, MISSING);
                }
                (e, entry.size())
            };
            let bytes = c.read_bytes(GuestSlice::new(src, size as u64))?;
            match c.host().set_component(e, id, &bytes) {
                Ok(()) => status(res, OK),
                Err(err) => status(res, status_of(&err)),
            }
        },
    )?;
    r.register(
        HostFn::new(NS, "has")
            .doc("1 if `entity` is alive and has component `component`, else 0 (also for an unknown component id).")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .result("has", T::Bool),
        |c, a, res| {
            let (bits, id) = (a.entity(0)?, a.i32(1)? as u32);
            let h = c.host();
            let has = match (alive(h.world(), bits), h.types().component(id)) {
                (Some(e), Ok(entry)) => entry.has(h.world(), e),
                _ => false,
            };
            res.set_bool(0, has)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "insert")
            .doc("Queue adding (or replacing) component `component` on `entity` from the bytes at `src` (the component's size); applied after the stage. Status: 0 ok, 1 stale entity, 3 unknown component id.")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .param("src", T::Ptr)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, id, src) = (a.entity(0)?, a.i32(1)? as u32, a.ptr(2)?);
            let (e, size) = {
                let h = c.host();
                let Some(e) = alive(h.world(), bits) else { return status(res, STALE) };
                let Ok(entry) = h.types().component(id) else { return status(res, UNKNOWN_ID) };
                (e, entry.size())
            };
            let bytes = c.read_bytes(GuestSlice::new(src, size as u64))?;
            c.host().commands.insert_raw(e, id, bytes);
            status(res, OK)
        },
    )?;
    r.register(
        HostFn::new(NS, "remove")
            .doc("Queue removing component `component` from `entity`; applied after the stage (a missing component is ignored then). Status: 0 ok, 1 stale entity, 3 unknown component id.")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, id) = (a.entity(0)?, a.i32(1)? as u32);
            let h = c.host();
            let Some(e) = alive(h.world(), bits) else { return status(res, STALE) };
            if h.types().component(id).is_err() {
                return status(res, UNKNOWN_ID);
            }
            h.commands.remove_raw(e, id);
            status(res, OK)
        },
    )?;
    r.register(
        HostFn::new(NS, "query")
            .doc("Write the handles (u64, 8 bytes each) of entities that have component `component` to `out`, at most `cap` of them, and return how many were written; -1 for an unknown component id.")
            .param("component", T::I32)
            .param("out", T::Ptr)
            .param("cap", T::I64)
            .result("count", T::I64),
        |c, a, res| {
            let (id, out, cap) = (a.i32(0)? as u32, a.ptr(1)?, a.i64(2)?);
            // A negative `cap` counts as 0 (nothing is written). For `cap > 0` the whole
            // `out .. out + cap*8` range must be inside guest memory, even if no entity
            // matches, so a bad `out` traps the same way every time.
            if cap > 0 {
                let need = (cap as u64).checked_mul(8).ok_or(Trap::OutOfBounds)?;
                match out.checked_add(need) {
                    Some(end) if out <= i64::MAX as u64 && end <= c.memory().len() => {}
                    _ => return Err(Trap::OutOfBounds),
                }
            }
            let bytes = {
                let h = c.host();
                let Ok(entry) = h.types().component(id) else {
                    res.set_i64(0, -1)?;
                    return Ok(HostOk::Continue);
                };
                let cap = usize::try_from(cap).unwrap_or(0);
                let mut v: Vec<u8> = Vec::new();
                for er in h.world().iter() {
                    if v.len() / 8 >= cap {
                        break;
                    }
                    let e = er.entity();
                    if entry.has(h.world(), e) {
                        v.extend_from_slice(&entity_to_bits(e).to_le_bytes());
                    }
                }
                v
            };
            if !bytes.is_empty() {
                c.write_bytes(out, &bytes)?;
            }
            res.set_i64(0, (bytes.len() / 8) as i64)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "component_id")
            .doc("The id of the component named `name` (`Transform`, `Velocity`, ...), or -1 if there is no such component. The same ids are in the API index.")
            .param("name", T::Str)
            .result("id", T::I32),
        |c, a, res| {
            let name = read_name(c, a.str(0)?)?;
            let id = c.host().types().by_name(&name).filter(|t| t.is_component()).map_or(-1, |t| t.id() as i32);
            res.set_i32(0, id)?;
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

fn script_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.script";
    r.register(
        HostFn::new(NS, "attach")
            .doc("Queue attaching a `Script` to `entity`: every frame the engine calls the calling module's exported function `entry(entity: i64, dt: f32, state: i64) -> i64` for it, the result being the next `state` (0 at first). Applied after the stage, replaces an existing `Script`. Status: 0 ok, 1 stale entity, 2 the module has no export `entry` of that signature, 3 no module is running.")
            .param("entity", T::Entity)
            .param("entry", T::Str)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, name) = (a.entity(0)?, read_name(c, a.str(1)?)?);
            let h = c.host();
            let Some(e) = alive(h.world(), bits) else { return status(res, STALE) };
            let Some(id) = h.current else { return status(res, 3) };
            let ok = h.slot(id).is_some_and(|m| {
                m.module.export_func(&name).and_then(|f| m.module.func_type(f)).is_some_and(|t| {
                    *t == efevm::FuncType::new([efevm::ValType::I64, efevm::ValType::F32, efevm::ValType::I64], [efevm::ValType::I64])
                })
            });
            if !ok {
                return status(res, 2);
            }
            h.commands.insert(e, super::Script::new(id, name));
            status(res, OK)
        },
    )?;
    Ok(())
}

fn random_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.random";
    r.register(
        HostFn::new(NS, "f32").doc("A pseudo-random number in `0.0 <= x < 1.0`.").result("x", T::F32),
        |c, _a, res| {
            let bits = c.host().next_random() >> 40; // 24 bits: exact in an f32
            res.set_f32(0, bits as f32 / (1u32 << 24) as f32)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "range")
            .doc("A pseudo-random integer in `0 <= x < n` (0 when `n <= 0`).")
            .param("n", T::I64)
            .result("x", T::I64),
        |c, a, res| {
            let n = a.i64(0)?;
            let x = if n <= 0 { 0 } else { (c.host().next_random() % n as u64) as i64 };
            res.set_i64(0, x)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "seed").doc("Restart the stream from `seed` (the same seed gives the same numbers).").param("seed", T::I64),
        |c, a, _res| {
            let seed = a.i64(0)? as u64;
            c.host().seed_random(seed);
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

fn math_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.math";
    r.register(
        HostFn::new(NS, "sin").doc("Sine of `x` (radians).").param("x", T::F32).result("y", T::F32),
        |_c, a, res| {
            res.set_f32(0, a.f32(0)?.sin())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "cos").doc("Cosine of `x` (radians).").param("x", T::F32).result("y", T::F32),
        |_c, a, res| {
            res.set_f32(0, a.f32(0)?.cos())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "atan2")
            .doc("The angle in radians of the vector (`x`, `y`), in `-pi ..= pi`.")
            .param("y", T::F32)
            .param("x", T::F32)
            .result("angle", T::F32),
        |_c, a, res| {
            res.set_f32(0, a.f32(0)?.atan2(a.f32(1)?))?;
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

fn input_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.input";
    const NAMES: &str = "Keys are named like the engine's KeyCode variants, ignoring case: A..Z, Key0..Key9 (or \"0\"..\"9\"), F1..F24, Up, Down, Left, Right, Space, Enter, Escape, Tab, Backspace, LShift, RShift, LCtrl, RCtrl, LAlt, RAlt, Numpad0..Numpad9, Minus, Equal, Comma, Period, ...; an unknown name is never down.";
    r.register(
        HostFn::new(NS, "key_down")
            .doc(format!("1 while the key `name` is held. {NAMES}"))
            .param("name", T::Str)
            .result("down", T::Bool),
        |c, a, res| {
            let name = read_name(c, a.str(0)?)?;
            let v = KeyCode::from_name(&name).is_some_and(|k| c.host().input().pressed(k));
            res.set_bool(0, v)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "key_pressed")
            .doc("1 during the frame the key `name` went down (same key names as key_down).")
            .param("name", T::Str)
            .result("pressed", T::Bool),
        |c, a, res| {
            let name = read_name(c, a.str(0)?)?;
            let v = KeyCode::from_name(&name).is_some_and(|k| c.host().input().just_pressed(k));
            res.set_bool(0, v)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "key_released")
            .doc("1 during the frame the key `name` went up (same key names as key_down).")
            .param("name", T::Str)
            .result("released", T::Bool),
        |c, a, res| {
            let name = read_name(c, a.str(0)?)?;
            let v = KeyCode::from_name(&name).is_some_and(|k| c.host().input().just_released(k));
            res.set_bool(0, v)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "mouse_x")
            .doc("Mouse x position in window pixels (0 if the pointer has not been seen).")
            .result("x", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().input().mouse_pos().map_or(0.0, |p| p.0))?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "mouse_y")
            .doc("Mouse y position in window pixels (0 if the pointer has not been seen).")
            .result("y", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().input().mouse_pos().map_or(0.0, |p| p.1))?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "mouse_down")
            .doc("1 while mouse button `button` is held: 0 left, 1 middle, 2 right, 3 back, 4 forward, 5..255 other.")
            .param("button", T::I32)
            .result("down", T::Bool),
        |c, a, res| {
            let b = match a.i32(0)? {
                0 => Some(MouseButton::Left),
                1 => Some(MouseButton::Middle),
                2 => Some(MouseButton::Right),
                3 => Some(MouseButton::Back),
                4 => Some(MouseButton::Forward),
                n @ 5..=255 => Some(MouseButton::Other(n as u8)),
                _ => None,
            };
            res.set_bool(0, b.is_some_and(|b| c.host().input().mouse_pressed(b)))?;
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

fn time_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.time";
    r.register(
        HostFn::new(NS, "delta").doc("Seconds since the previous frame.").result("seconds", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().time().delta())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "elapsed").doc("Seconds since the engine started.").result("seconds", T::F64),
        |c, _a, res| {
            res.set_f64(0, c.host().time().elapsed())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "frame").doc("Number of frames run so far.").result("frame", T::I64),
        |c, _a, res| {
            res.set_i64(0, c.host().time().frame() as i64)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "fixed_delta").doc("Length of a fixed update step in seconds.").result("seconds", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().time().fixed_delta())?;
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

/// The event `ev` of the calling module's section 14 (the module whose code runs now). An id
/// outside the list traps with a message naming the module and the id.
fn event_of(c: &mut Caller<'_, ScriptHost>, func: &str, ev: i32) -> Result<EventRef, Trap> {
    let h = c.host();
    let slot = h
        .current
        .and_then(|id| h.slot(id))
        .ok_or_else(|| Trap::Host(format!("engine.events.{func}: no module is running")))?;
    let events = &slot.module.data().events;
    usize::try_from(ev).ok().and_then(|i| events.get(i)).cloned().ok_or_else(|| {
        Trap::Host(format!(
            "engine.events.{func}: module `{}` has no event with id {ev} (its section 14 lists {} event{})",
            slot.name,
            events.len(),
            if events.len() == 1 { "" } else { "s" }
        ))
    })
}

fn events_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.events";
    r.register(
        HostFn::new(NS, "send")
            .doc("Queue the event `ev` (an id of the calling module's events section) for the next frame; copies the registered payload struct's size in bytes from `payload` (an event without payload ignores `payload`: pass 0). At most 4096 events are queued per frame, further sends are dropped, counted and logged once per frame. An unknown `ev` or an out-of-bounds `payload` traps.")
            .param("ev", T::I32)
            .param("payload", T::Ptr),
        |c, a, _res| {
            let ev = event_of(c, "send", a.i32(0)?)?;
            let ptr = a.ptr(1)?;
            let size = ev.payload_size() as u64;
            let payload = if size == 0 {
                Vec::new()
            } else {
                // Bounds first, so a bogus size never allocates.
                if ptr > i64::MAX as u64 || ptr.checked_add(size).is_none_or(|end| end > c.memory().len()) {
                    return Err(Trap::OutOfBounds);
                }
                c.read_bytes(GuestSlice::new(ptr, size))?
            };
            c.host().queue_event(ScriptEvent { event: ev.qualified(), payload });
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "count")
            .doc("Number of events `ev` (an id of the calling module's events section) readable this frame, that is, sent during the previous frame (or by Rust). An unknown `ev` traps.")
            .param("ev", T::I32)
            .result("count", T::I32),
        |c, a, res| {
            let ev = event_of(c, "count", a.i32(0)?)?;
            let name = ev.qualified();
            let n = c.host().events_named(&name).count();
            res.set_i32(0, n.min(i32::MAX as usize) as i32)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "read")
            .doc("Copy the payload of the `index`-th (0-based, in send order) event `ev` of this frame to `out` (the registered payload struct's size in bytes; nothing for an event without payload) and return 0, or return -1 if there is no such event (negative or too large `index`). An unknown `ev` or an out-of-bounds `out` traps.")
            .param("ev", T::I32)
            .param("index", T::I32)
            .param("out", T::Ptr)
            .result("status", T::I32),
        |c, a, res| {
            let ev = event_of(c, "read", a.i32(0)?)?;
            let index = a.i32(1)?;
            let out = a.ptr(2)?;
            let name = ev.qualified();
            let size = ev.payload_size() as u64;
            if size > 0 && (out > i64::MAX as u64 || out.checked_add(size).is_none_or(|end| end > c.memory().len())) {
                return Err(Trap::OutOfBounds);
            }
            let data = usize::try_from(index).ok().and_then(|i| c.host().events_named(&name).nth(i).map(<[u8]>::to_vec));
            match data {
                None => res.set_i32(0, -1)?,
                Some(d) => {
                    if size > 0 {
                        c.write_bytes(out, &d)?;
                    }
                    res.set_i32(0, 0)?;
                }
            }
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

fn log_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.log";
    for (name, level, doc) in [
        ("info", LogLevel::Info, "Log `msg` at info level."),
        ("warn", LogLevel::Warn, "Log `msg` at warning level."),
        ("error", LogLevel::Error, "Log `msg` at error level."),
    ] {
        r.register(HostFn::new(NS, name).doc(doc).param("msg", T::Str), move |c, a, _res| {
            let msg = c.read_str(a.str(0)?)?;
            let h = c.host();
            let who = h.current.and_then(|id| h.module_name(id)).unwrap_or("?").to_string();
            h.log(level, format!("[{who}] {msg}"));
            Ok(HostOk::Continue)
        })?;
    }
    Ok(())
}

/// The asset `path` of the package of the module that is running now.
fn own_asset<'a>(h: &'a ScriptHost, path: &str) -> Option<&'a [u8]> {
    let id = h.current?;
    h.slot(id)?.package.as_ref()?.assets.get(path).map(Vec::as_slice)
}

fn mods_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.mods";
    r.register(
        HostFn::new(NS, "asset_len")
            .doc("Length in bytes of the asset `path` (relative to the package's assets/ folder, e.g. \"items.json\") of the calling mod's own package; -1 if there is no such asset or the caller is not a package.")
            .param("path", T::Str)
            .result("len", T::I64),
        |c, a, res| {
            let path = read_name(c, a.str(0)?)?;
            let len = own_asset(c.host(), &path).map_or(-1, |d| d.len() as i64);
            res.set_i64(0, len)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "meta_f64")
            .doc("A number from the `[meta]` table of the calling mod's own manifest: `key` is a dotted path (`difficulty.speed`); returns `default` if the caller is not a package, the key is missing, or the value is not a number (a boolean is 1 or 0).")
            .param("key", T::Str)
            .param("default", T::F64)
            .result("value", T::F64),
        |c, a, res| {
            let key = read_name(c, a.str(0)?)?;
            let default = a.f64(1)?;
            let h = c.host();
            let meta = h.current.and_then(|id| h.slot(id)).and_then(|m| m.package.as_ref()).and_then(|p| p.manifest.meta.as_ref());
            let mut v = meta;
            for part in key.split('.') {
                v = v.and_then(|v| v.get(part));
            }
            let value = match v {
                Some(efevm::api::json::Value::Num(n)) => n.parse::<f64>().unwrap_or(default),
                Some(efevm::api::json::Value::Bool(b)) => f64::from(*b),
                _ => default,
            };
            res.set_f64(0, value)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "asset_read")
            .doc("Copy the asset `path` of the calling mod's own package into `buf`, as much as fits (at most buf.len bytes); returns the asset's full length, or -1 if there is no such asset or the caller is not a package.")
            .param("path", T::Str)
            .param("buf", T::Bytes)
            .result("len", T::I64),
        |c, a, res| {
            let path = read_name(c, a.str(0)?)?;
            let buf = a.bytes(1)?;
            if buf.ptr > i64::MAX as u64 || buf.len > i64::MAX as u64 {
                return Err(Trap::OutOfBounds);
            }
            let found = own_asset(c.host(), &path).map(|d| {
                let n = buf.len.min(d.len() as u64) as usize;
                (d.len(), d[..n].to_vec())
            });
            match found {
                None => res.set_i64(0, -1)?,
                Some((full, head)) => {
                    c.write_bytes(buf.ptr, &head)?;
                    res.set_i64(0, full as i64)?;
                }
            }
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

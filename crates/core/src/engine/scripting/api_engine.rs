//! The engine's host functions (owner `engine`, version 1), registered in the one
//! `HostRegistry<ScriptHost>`. Registering is what makes them callable *and* what puts them in
//! the API index; there is no other list. Status codes of `engine.world`: 0 ok, 1 stale or
//! forged entity, 2 component missing, 3 unknown component id, 4 wrong size, -1 other.

use efevm::{ApiType as T, Caller, GuestSlice, HostFn, HostOk, HostRegistry, RegistryError, Results, Trap};
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
        ("engine.events", "Named byte-payload events; events sent in one frame are readable during the next."),
        ("engine.log", "Write to the engine log."),
        ("engine.mods", "Mod support: share functions with other modules."),
    ] {
        r.define_namespace(ns, 1, OWNER, doc)?;
    }
    world_api(r)?;
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
            .doc("Reserve a new empty entity and return its handle; it is valid at once and comes alive when the frame's commands apply (add components with insert).")
            .result("entity", T::Entity),
        |c, _a, res| {
            let h = c.host();
            let e = h.commands.spawn_empty(&h.world);
            res.set_entity(0, entity_to_bits(e))?;
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
            match alive(&h.world, bits) {
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
            res.set_bool(0, alive(&h.world, bits).is_some())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "get")
            .doc("Copy component `component` (an id from component_id or the index) of `entity` to `out`, which must have room for the component's size. Status: 0 ok, 1 stale entity, 2 component missing, 3 unknown component id.")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .param("out", T::Ptr)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, id, out) = (a.entity(0)?, a.i32(1)? as u32, a.ptr(2)?);
            let bytes = {
                let h = c.host();
                let Some(e) = alive(&h.world, bits) else { return status(res, STALE) };
                let Ok(entry) = h.types.component(id) else { return status(res, UNKNOWN_ID) };
                let mut buf = vec![0u8; entry.size() as usize];
                if let Err(err) = entry.read(&h.world, e, &mut buf) {
                    return status(res, status_of(&err));
                }
                buf
            };
            c.write_bytes(out, &bytes)?;
            status(res, OK)
        },
    )?;
    r.register(
        HostFn::new(NS, "set")
            .doc("Overwrite, in place, the existing component `component` of `entity` with the bytes at `src` (the component's size). Status: 0 ok, 1 stale entity, 2 component missing, 3 unknown component id.")
            .param("entity", T::Entity)
            .param("component", T::I32)
            .param("src", T::Ptr)
            .result("status", T::I32),
        |c, a, res| {
            let (bits, id, src) = (a.entity(0)?, a.i32(1)? as u32, a.ptr(2)?);
            let (e, size) = {
                let h = c.host();
                let Some(e) = alive(&h.world, bits) else { return status(res, STALE) };
                let Ok(entry) = h.types.component(id) else { return status(res, UNKNOWN_ID) };
                if !entry.has(&h.world, e) {
                    return status(res, MISSING);
                }
                (e, entry.size())
            };
            let bytes = c.read_bytes(GuestSlice::new(src, size as u64))?;
            let h = c.host();
            let Ok(entry) = h.types.component(id) else { return status(res, UNKNOWN_ID) };
            match entry.write(&mut h.world, e, &bytes) {
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
            let has = match (alive(&h.world, bits), h.types.component(id)) {
                (Some(e), Ok(entry)) => entry.has(&h.world, e),
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
                let Some(e) = alive(&h.world, bits) else { return status(res, STALE) };
                let Ok(entry) = h.types.component(id) else { return status(res, UNKNOWN_ID) };
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
            let Some(e) = alive(&h.world, bits) else { return status(res, STALE) };
            if h.types.component(id).is_err() {
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
                let Ok(entry) = h.types.component(id) else {
                    res.set_i64(0, -1)?;
                    return Ok(HostOk::Continue);
                };
                let cap = usize::try_from(cap).unwrap_or(0);
                let mut v: Vec<u8> = Vec::new();
                for er in h.world.iter() {
                    if v.len() / 8 >= cap {
                        break;
                    }
                    let e = er.entity();
                    if entry.has(&h.world, e) {
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
            let id = c.host().types.by_name(&name).filter(|t| t.is_component()).map_or(-1, |t| t.id() as i32);
            res.set_i32(0, id)?;
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
            let v = KeyCode::from_name(&name).is_some_and(|k| c.host().input.pressed(k));
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
            let v = KeyCode::from_name(&name).is_some_and(|k| c.host().input.just_pressed(k));
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
            let v = KeyCode::from_name(&name).is_some_and(|k| c.host().input.just_released(k));
            res.set_bool(0, v)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "mouse_x")
            .doc("Mouse x position in window pixels (0 if the pointer has not been seen).")
            .result("x", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().input.mouse_pos().map_or(0.0, |p| p.0))?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "mouse_y")
            .doc("Mouse y position in window pixels (0 if the pointer has not been seen).")
            .result("y", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().input.mouse_pos().map_or(0.0, |p| p.1))?;
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
            res.set_bool(0, b.is_some_and(|b| c.host().input.mouse_pressed(b)))?;
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
            res.set_f32(0, c.host().time.delta())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "elapsed").doc("Seconds since the engine started.").result("seconds", T::F64),
        |c, _a, res| {
            res.set_f64(0, c.host().time.elapsed())?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "frame").doc("Number of frames run so far.").result("frame", T::I64),
        |c, _a, res| {
            res.set_i64(0, c.host().time.frame() as i64)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "fixed_delta").doc("Length of a fixed update step in seconds.").result("seconds", T::F32),
        |c, _a, res| {
            res.set_f32(0, c.host().time.fixed_delta())?;
            Ok(HostOk::Continue)
        },
    )?;
    Ok(())
}

fn events_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    const NS: &str = "engine.events";
    r.register(
        HostFn::new(NS, "send")
            .doc("Send an event called `name` with a byte payload; it is readable during the next frame (also from Rust as ScriptEvent).")
            .param("name", T::Str)
            .param("data", T::Bytes),
        |c, a, _res| {
            let name = c.read_str(a.str(0)?)?;
            let data = c.read_bytes(a.bytes(1)?)?;
            c.host().send_event(ScriptEvent { name, data });
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "count")
            .doc("Number of events called `name` readable this frame.")
            .param("name", T::Str)
            .result("count", T::I64),
        |c, a, res| {
            let name = c.read_str(a.str(0)?)?;
            let n = c.host().events_in.iter().filter(|e| e.name == name).count();
            res.set_i64(0, n as i64)?;
            Ok(HostOk::Continue)
        },
    )?;
    r.register(
        HostFn::new(NS, "read")
            .doc("Copy the payload of event number `index` (0-based) called `name` into `buf`, as much as fits; returns the full payload length, or -1 if there is no such event.")
            .param("name", T::Str)
            .param("index", T::I64)
            .param("buf", T::Bytes)
            .result("len", T::I64),
        |c, a, res| {
            let name = c.read_str(a.str(0)?)?;
            let index = a.i64(1)?;
            let buf = a.bytes(2)?;
            if buf.ptr > i64::MAX as u64 || buf.len > i64::MAX as u64 {
                return Err(Trap::OutOfBounds);
            }
            let data = usize::try_from(index)
                .ok()
                .and_then(|i| c.host().events_in.iter().filter(|e| e.name == name).nth(i).map(|e| e.data.clone()));
            match data {
                None => res.set_i64(0, -1)?,
                Some(d) => {
                    let n = buf.len.min(d.len() as u64) as usize;
                    c.write_bytes(buf.ptr, &d[..n])?;
                    res.set_i64(0, d.len() as i64)?;
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

fn mods_api(r: &mut HostRegistry<ScriptHost>) -> Result<(), RegistryError> {
    r.register(
        HostFn::new("engine.mods", "export")
            .doc("Call from `init`: offer the exported function `func` of this module to other modules as `<module name>.<func>`, with `doc` as its documentation. It is registered (and appears in the API index) when init returns. Status: 0 ok, -1 refused (not in init, no such function, bad name or empty doc).")
            .param("func", T::Str)
            .param("doc", T::Str)
            .result("status", T::I32),
        |c, a, res| {
            let func = c.read_str(a.str(0)?)?;
            let doc = c.read_str(a.str(1)?)?;
            let s = c.host().request_export(&func, &doc);
            status(res, s)
        },
    )?;
    Ok(())
}

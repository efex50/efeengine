//! The trampoline of a declared provide and the copy ABI across mods (e8-design sections 6
//! and 7).
//!
//! Memories are separate, so a pointer cannot cross: `str`, `bytes` and `&Struct` arguments are
//! copied into the provider's scratch region before it runs, and `bytes` and struct arguments
//! are copied back after a normal return. The borrow structure forces three phases: read from
//! the caller's memory (`Caller`), run in the provider (`ScriptHost::run_provide`, which owns
//! the provider's instance while it runs), write back into the caller's memory.

use efevm::{ApiType, Args, Caller, GuestSlice, HostOk, Results, Trap};

use super::host::{CopyIn, ProvideCall, RunError};
use super::{ModuleId, ScriptHost};

/// How one parameter is passed.
enum Kind {
    Scalar(usize),
    Str,
    Bytes,
    /// `&S` with `S.size`.
    Struct(u32),
}

/// What the trampoline needs from the provider's current module, copied out of the host so the
/// caller's memory can be read afterwards.
struct Info {
    module: String,
    func: u32,
    params: Vec<(String, Kind)>,
    scratch: (u64, u64),
}

fn align8(v: u64) -> u64 {
    (v + 7) & !7
}

fn info(host: &ScriptHost, id: ModuleId, namespace: &str, name: &str) -> Result<Info, Trap> {
    let slot = host.slot_checked(id).map_err(|e| Trap::Host(format!("module {namespace}: {e}")))?;
    if !slot.loaded {
        return Err(Trap::Host(format!("module {} unloaded", slot.name)));
    }
    let iface = slot.module.data().interface.as_ref();
    let Some((iface, p)) = iface.and_then(|i| i.provide(name).map(|p| (i, p))) else {
        return Err(Trap::Host(format!("`{namespace}.{name}` is no longer provided by module {}", slot.name)));
    };
    let mut params = Vec::with_capacity(p.params.len());
    for param in &p.params {
        let kind = match &param.ty {
            ApiType::Str => Kind::Str,
            ApiType::Bytes => Kind::Bytes,
            ApiType::Struct(s) => match iface.struct_desc(s) {
                Some(d) => Kind::Struct(d.size),
                None => return Err(Trap::Host(format!("`{namespace}.{name}`: struct `{s}` has no layout in module {}", slot.name))),
            },
            t => Kind::Scalar(t.lower().len()),
        };
        params.push((param.name.clone(), kind));
    }
    let scratch = iface.scratch.map_or((0, 0), |(o, n)| (o, n as u64));
    Ok(Info { module: slot.name.clone(), func: p.func, params, scratch })
}

/// A guest `i64` pointer or length: negative values are out of bounds.
fn nonneg(v: u64) -> Option<u64> {
    (v <= i64::MAX as u64).then_some(v)
}

/// The host function behind provide `name` of the mod `id` (namespace `namespace`).
pub(crate) fn call(
    c: &mut Caller<'_, ScriptHost>,
    id: ModuleId,
    namespace: &str,
    name: &str,
    a: &Args<'_>,
    r: &mut Results<'_>,
) -> Result<HostOk, Trap> {
    let q = format!("{namespace}.{name}");
    let info = info(c.host(), id, namespace, name)?;
    let raw = a.raw();
    let slots: usize = info.params.iter().map(|(_, k)| match k {
        Kind::Scalar(n) => *n,
        Kind::Str | Kind::Bytes => 2,
        Kind::Struct(_) => 1,
    }).sum();
    if raw.len() != slots {
        return Err(Trap::Host(format!("`{q}` was called with {} argument slots, it takes {slots}", raw.len())));
    }

    // Phase 1: copy in, left to right, from the scratch start.
    let (start, size) = info.scratch;
    let end = start + size;
    let mut cursor = start;
    let mut out_args: Vec<u64> = Vec::with_capacity(raw.len());
    let mut copies: Vec<CopyIn> = Vec::new();
    let mut back_to: Vec<u64> = Vec::new();
    let (mut bytes_in, mut bytes_out) = (0u64, 0u64);
    let mut s = 0usize;
    for (pname, kind) in &info.params {
        let (ptr, len, utf8, back) = match kind {
            Kind::Scalar(n) => {
                out_args.extend_from_slice(&raw[s..s + n]);
                s += n;
                continue;
            }
            Kind::Str => (raw[s], raw[s + 1], true, false),
            Kind::Bytes => (raw[s], raw[s + 1], false, true),
            Kind::Struct(n) => (raw[s], *n as u64, false, true),
        };
        let (Some(ptr), Some(len)) = (nonneg(ptr), nonneg(len)) else {
            return Err(Trap::Host(format!("argument `{pname}` of `{q}`: negative pointer or length")));
        };
        let slots = if matches!(kind, Kind::Struct(_)) { 1 } else { 2 };
        s += slots;
        if len == 0 {
            // An empty range still has to start inside the caller's memory (the same check as
            // below; `read_bytes` accepts `ptr == memory size` with length 0).
            c.read_bytes(GuestSlice::new(ptr, 0))
                .map_err(|_| Trap::Host(format!("argument `{pname}` of `{q}`: {len} bytes at address {ptr} are out of bounds")))?;
            out_args.push(cursor);
            if slots == 2 {
                out_args.push(0);
            }
            continue;
        }
        let at = align8(cursor);
        let needs = at.saturating_add(len).saturating_sub(start);
        if at.saturating_add(len) > end {
            return Err(Trap::Host(format!(
                "argument `{pname}` of `{q}` needs {needs} bytes of scratch, the mod has {size}"
            )));
        }
        let data = c
            .read_bytes(GuestSlice::new(ptr, len))
            .map_err(|_| Trap::Host(format!("argument `{pname}` of `{q}`: {len} bytes at address {ptr} are out of bounds")))?;
        if utf8 && std::str::from_utf8(&data).is_err() {
            return Err(Trap::Host(format!("argument `{pname}` of `{q}` is not valid UTF-8")));
        }
        out_args.push(at);
        if slots == 2 {
            out_args.push(len);
        }
        bytes_in += len;
        if back {
            bytes_out += len;
            back_to.push(ptr);
        }
        copies.push(CopyIn { dst: at, data, back });
        cursor = at + len;
    }

    // Phase 2: run in the provider.
    let units = 1 + (bytes_in + bytes_out) / 256;
    let pc = ProvideCall { func: info.func, args: out_args, copies, units };
    let (results, back) = match c.host().run_provide(id, &pc) {
        Ok(v) => v,
        Err(RunError::OutOfBudget) => return Err(Trap::Host(format!("out of budget in `{q}`"))),
        Err(RunError::Yielded) => return Err(Trap::Host(format!("`{q}` yielded"))),
        Err(e) => return Err(Trap::Host(e.to_string())),
    };

    // Phase 3: copy back into the caller (nothing is copied after a trap).
    for (ptr, data) in back_to.iter().zip(&back) {
        c.write_bytes(*ptr, data).map_err(|e| Trap::Host(format!("copying the result of `{q}` back: {e}")))?;
    }
    for (slot, v) in r.raw().iter_mut().zip(results) {
        *slot = v;
    }
    Ok(HostOk::Continue)
}

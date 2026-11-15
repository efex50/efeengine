//! The `Script` component and the script system.

use efevm::{FuncType, Module, ValType};
use hecs::Entity;

use super::host::{LogLevel, RunError};
use super::{ModuleId, Scripting};
use crate::engine::schedule::Ctx;
use crate::engine::types::entity_to_bits;

/// Attaches a script to an entity: every frame the script system calls
/// `entry(entity: entity, dt: f32, state: i64) -> i64` of `module` and stores the result in
/// `state`. Rust-only component (not visible to scripts).
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub module: ModuleId,
    /// Name of an exported function of the module.
    pub entry: String,
    /// Carried from call to call; the entry's return value. Reset to 0 when the module is
    /// hot-reloaded.
    pub state: i64,
}

impl Script {
    pub fn new(module: ModuleId, entry: impl Into<String>) -> Self {
        Script { module, entry: entry.into(), state: 0 }
    }
}

enum Sig {
    Missing,
    Mismatch(String),
    Ok,
}

fn check_sig(module: &Module, name: &str, params: &[ValType], results: &[ValType]) -> Sig {
    let Some(idx) = module.export_func(name) else { return Sig::Missing };
    match module.func_type(idx) {
        Some(t) if *t == FuncType::new(params.iter().copied(), results.iter().copied()) => Sig::Ok,
        Some(t) => Sig::Mismatch(format!(
            "`{name}` is {t}, expected {}",
            FuncType::new(params.iter().copied(), results.iter().copied())
        )),
        None => Sig::Missing,
    }
}

/// The script system: first system of `Stage::Update`.
pub(crate) fn script_system(cx: &mut Ctx<'_>) {
    let scripting = &mut *cx.scripting;
    if !scripting.is_active() {
        return;
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let reloaded = scripting.check_reload(cx.time.elapsed());
        if !reloaded.is_empty() {
            for s in cx.world.query_mut::<&mut Script>() {
                if reloaded.contains(&s.module) {
                    s.state = 0;
                }
            }
        }
    }
    let events: Vec<super::ScriptEvent> = cx.events::<super::ScriptEvent>().iter().cloned().collect();
    let scripting = &mut *cx.scripting;
    scripting.enter(cx.world, cx.types, cx.time, cx.input, events);
    scripting.run_frame();
    scripting.leave(cx.world, cx.commands);
    let out = scripting.take_events();
    for ev in out {
        cx.send(ev);
    }
}

impl Scripting {
    /// Run `init` of every module that has not had it yet (and register what it exports).
    pub(crate) fn run_inits(&mut self) {
        for idx in 0..self.host.modules.len() {
            let slot = &mut self.host.modules[idx];
            if !slot.loaded || !slot.pending_init {
                continue;
            }
            slot.pending_init = false;
            slot.remaining = slot.budget;
            slot.exports.clear();
            let (name, sig) = (slot.name.clone(), check_sig(&slot.module, "init", &[], &[]));
            let id = ModuleId(idx as u32);
            match sig {
                Sig::Missing => continue,
                Sig::Mismatch(m) => {
                    self.log(LogLevel::Warn, format!("[{name}] init is not run: {m}"));
                    continue;
                }
                Sig::Ok => {}
            }
            self.host.in_init = true;
            let r = self.host.run_export(id, "init", &[]);
            self.host.in_init = false;
            match r {
                Ok(_) => self.register_exports(id),
                Err(e) => self.report(id, "init", e),
            }
        }
    }

    fn report(&mut self, id: ModuleId, what: &str, e: RunError) {
        let name = self.host.module_name(id).unwrap_or("?").to_string();
        match e {
            RunError::OutOfBudget => {
                let budget = self.host.modules.get(id.0 as usize).map_or(0, |m| m.budget);
                self.log(LogLevel::Warn, format!("[{name}] `{what}` ran out of its budget of {budget} units; the rest of this module is skipped this frame"));
            }
            other => self.log(LogLevel::Error, format!("[{name}] `{what}` failed: {other}")),
        }
    }

    /// One frame of scripting, with the world inside the host: budgets reset, pending
    /// `init`s, then per loaded module `frame(dt)` and the `entry` of each of its `Script`s.
    pub(crate) fn run_frame(&mut self) {
        for s in &mut self.host.modules {
            s.remaining = s.budget;
        }
        self.run_inits();
        for s in &mut self.host.modules {
            s.remaining = s.budget;
        }
        let dt = self.host.time.delta();
        let mut jobs: Vec<(u32, Entity)> =
            self.host.world.query::<(Entity, &Script)>().iter().map(|(e, s)| (s.module.0, e)).collect();
        jobs.sort_by_key(|j| j.0);

        for idx in 0..self.host.modules.len() {
            if !self.host.modules[idx].loaded {
                continue;
            }
            let id = ModuleId(idx as u32);
            let name = self.host.modules[idx].name.clone();
            let mut exhausted = false;

            let frame_sig = check_sig(&self.host.modules[idx].module, "frame", &[ValType::F32], &[]);
            match frame_sig {
                Sig::Ok => {
                    if let Err(e) = self.host.run_export(id, "frame", &[dt.to_bits() as u64]) {
                        exhausted = matches!(e, RunError::OutOfBudget);
                        self.report(id, "frame", e);
                    }
                }
                Sig::Mismatch(m) => self.warn_once(format!("frame:{idx}"), format!("[{name}] frame is not run: {m}")),
                Sig::Missing => {}
            }

            for &(_, e) in jobs.iter().filter(|j| j.0 == idx as u32) {
                if exhausted {
                    break;
                }
                let Some((entry, state)) = self.host.world.get::<&Script>(e).ok().map(|s| (s.entry.clone(), s.state)) else {
                    continue;
                };
                match check_sig(&self.host.modules[idx].module, &entry, &[ValType::I64, ValType::F32, ValType::I64], &[ValType::I64]) {
                    Sig::Ok => {}
                    Sig::Missing => {
                        self.warn_once(format!("entry:{idx}:{entry}"), format!("[{name}] has no exported function `{entry}`; scripts using it are skipped"));
                        continue;
                    }
                    Sig::Mismatch(m) => {
                        self.warn_once(format!("entry:{idx}:{entry}"), format!("[{name}] entry {m} (entry functions take (entity, dt: f32, state: i64) -> i64); scripts using it are skipped"));
                        continue;
                    }
                }
                let args = [entity_to_bits(e), dt.to_bits() as u64, state as u64];
                match self.host.run_export(id, &entry, &args) {
                    Ok(r) => {
                        if let (Some(&v), Ok(mut s)) = (r.first(), self.host.world.get::<&mut Script>(e)) {
                            s.state = v as i64;
                        }
                    }
                    Err(err) => {
                        exhausted = matches!(err, RunError::OutOfBudget);
                        self.report(id, &entry, err);
                    }
                }
            }
        }

        // Scripts that point at a module that is not loaded.
        for &(m, _) in &jobs {
            if !self.host.modules.get(m as usize).is_some_and(|s| s.loaded) {
                self.warn_once(format!("gone:{m}"), format!("scripts refer to module id {m}, which is not loaded; they are skipped"));
            }
        }
    }
}

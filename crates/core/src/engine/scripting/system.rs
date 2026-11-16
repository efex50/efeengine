//! The `Script` component and the script system.

use std::sync::Arc;

use efevm::{FuncType, Module, ValType};
use hecs::Entity;

use super::host::{self, LogLevel, ModuleSlot, RunError};
use super::{LogLine, ModuleId, ScriptEvent, ScriptHost, ScriptStats, Scripting};
use crate::engine::parallel::Executor;
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

/// The script system: first system of `Stage::Update`. Exclusive; the lanes run in parallel
/// inside it (through the engine's executor).
pub(crate) fn script_system(cx: &mut Ctx<'_>) {
    let scripting = &mut *cx.scripting;
    if !scripting.is_active() {
        return;
    }
    {
        let reloaded = scripting.check_reload(cx.time.elapsed(), cx.resources);
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
    scripting.run_frame(cx.exec);
    scripting.leave(cx.world, cx.commands);
    let out = scripting.take_events();
    for ev in out {
        cx.send(ev);
    }
}

/// The lanes of `modules`: connected components of the "imports a function of a namespace owned
/// by" relation between loaded modules (docs/PARALLEL_SPEC.md 7.2). Members ascend by id, lanes
/// ascend by their smallest id. Modules call each other only through provides, which run the
/// other module's instance inside the caller's call, so a lane is closed under calls.
fn compute_lanes(modules: &[ModuleSlot], registry: &efevm::HostRegistry<ScriptHost>) -> Vec<Vec<usize>> {
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let n = modules.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let by_name: std::collections::HashMap<&str, usize> =
        modules.iter().enumerate().filter(|(_, m)| m.loaded).map(|(i, m)| (m.name.as_str(), i)).collect();
    for (i, m) in modules.iter().enumerate().filter(|(_, m)| m.loaded) {
        for imp in &m.module.data().imports {
            let Some(owner) = registry.namespace(&imp.namespace).map(|ns| ns.owner.as_str()) else { continue };
            if let Some(&j) = by_name.get(owner) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    // the smaller id stays the root, so a lane is found by its smallest member
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
    }
    let mut lanes: Vec<Vec<usize>> = Vec::new();
    let mut lane_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in (0..n).filter(|&i| modules[i].loaded) {
        let r = find(&mut parent, i);
        let l = *lane_of.entry(r).or_insert_with(|| {
            lanes.push(Vec::new());
            lanes.len() - 1
        });
        lanes[l].push(i);
    }
    lanes
}

impl ScriptHost {
    fn report(&mut self, id: ModuleId, what: &str, e: RunError) {
        let name = self.module_name(id).unwrap_or("?").to_string();
        match e {
            RunError::OutOfBudget => {
                let budget = self.slot(id).map_or(0, |m| m.budget);
                self.log(LogLevel::Warn, format!("[{name}] `{what}` ran out of its budget of {budget} units; the rest of this module is skipped this frame"));
            }
            other => self.log(LogLevel::Error, format!("[{name}] `{what}` failed: {other}")),
        }
    }

    /// Run one lane: budgets reset, pending `init`s of its modules, then (with `frame`) per
    /// module, by id, `frame(dt)` and the `entry` of each of its `Script`s.
    fn run_lane(&mut self, lane: &[usize], frame: bool) {
        for &idx in lane {
            if let Some(s) = self.modules[idx].as_mut() {
                s.remaining = s.budget;
            }
        }
        for &idx in lane {
            let id = ModuleId(idx as u32);
            let slot = self.modules[idx].as_mut().expect("lane member");
            if !slot.pending_init {
                continue;
            }
            slot.pending_init = false;
            slot.remaining = slot.budget;
            let (name, sig) = (slot.name.clone(), check_sig(&slot.module, "init", &[], &[]));
            match sig {
                Sig::Missing => continue,
                Sig::Mismatch(m) => {
                    self.log(LogLevel::Warn, format!("[{name}] init is not run: {m}"));
                    continue;
                }
                Sig::Ok => {}
            }
            if let Err(e) = self.run_export(id, "init", &[]) {
                self.report(id, "init", e);
            }
        }
        if !frame {
            return;
        }
        for &idx in lane {
            self.modules[idx].as_mut().expect("lane member").remaining = self.modules[idx].as_ref().expect("lane member").budget;
        }
        let view = self.frame.clone();
        let dt = view.time.delta();
        for &idx in lane {
            let id = ModuleId(idx as u32);
            let name = self.modules[idx].as_ref().expect("lane member").name.clone();
            let mut exhausted = false;

            let frame_sig = check_sig(&self.modules[idx].as_ref().expect("lane member").module, "frame", &[ValType::F32], &[]);
            match frame_sig {
                Sig::Ok => {
                    if let Err(e) = self.run_export(id, "frame", &[dt.to_bits() as u64]) {
                        exhausted = matches!(e, RunError::OutOfBudget);
                        self.report(id, "frame", e);
                    }
                }
                Sig::Mismatch(m) => self.warn_once(format!("frame:{idx}"), format!("[{name}] frame is not run: {m}")),
                Sig::Missing => {}
            }

            for &(_, e) in view.jobs.iter().filter(|j| j.0 == idx as u32) {
                if exhausted {
                    break;
                }
                let Some((entry, state)) = view.world.get::<&Script>(e).ok().map(|s| (s.entry.clone(), s.state)) else {
                    continue;
                };
                let sig = check_sig(
                    &self.modules[idx].as_ref().expect("lane member").module,
                    &entry,
                    &[ValType::I64, ValType::F32, ValType::I64],
                    &[ValType::I64],
                );
                match sig {
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
                match self.run_export(id, &entry, &args) {
                    Ok(r) => {
                        if let Some(&v) = r.first() {
                            self.script_state.push((e, v as i64));
                        }
                    }
                    Err(err) => {
                        exhausted = matches!(err, RunError::OutOfBudget);
                        self.report(id, &entry, err);
                    }
                }
            }
        }
    }
}

impl Scripting {
    /// The lanes of the loaded modules (docs/PARALLEL_SPEC.md 7.2): modules joined by provides
    /// (one imports a namespace the other owns) share a lane; every other module is alone.
    /// Ascending by smallest module id; members ascending.
    pub fn lanes(&self) -> Vec<Vec<ModuleId>> {
        compute_lanes(&self.modules, &self.registry).into_iter().map(|l| l.into_iter().map(|i| ModuleId(i as u32)).collect()).collect()
    }

    /// Run `init` of every module that has not had it yet (outside a frame; the lanes still run
    /// through the executor and merge the same way).
    pub(crate) fn run_inits(&mut self, exec: &Executor) {
        self.run_phase(exec, false);
    }

    /// One frame of scripting, with the world in the snapshot: budgets reset, pending `init`s,
    /// then per loaded module `frame(dt)` and the `entry` of each of its `Script`s. Lanes run
    /// as tasks of `exec` and their buffers merge in lane order afterwards.
    pub(crate) fn run_frame(&mut self, exec: &Executor) {
        self.run_phase(exec, true);
    }

    fn run_phase(&mut self, exec: &Executor, frame: bool) {
        let mut view = self.frame.take().expect("Scripting::enter before running scripts");
        view.module_names = self.modules.iter().map(|m| m.name.clone()).collect();
        if frame {
            let mut jobs: Vec<(u32, Entity)> = view.world.query::<(Entity, &Script)>().iter().map(|(e, s)| (s.module.0, e)).collect();
            jobs.sort_by_key(|j| j.0);
            view.jobs = jobs;
        }
        // Spawn pools, reserved here in module-id order so the ids are the same for every
        // thread count (reservation is atomic, so reserving inside a lane would depend on timing).
        let initial: Vec<usize> = self.modules.iter().map(|m| if m.loaded { m.pool_size } else { 0 }).collect();
        for (m, &n) in self.modules.iter_mut().zip(&initial) {
            m.spawn_pool = (0..n).map(|_| view.world.reserve_entity()).collect();
        }
        let lanes = compute_lanes(&self.modules, &self.registry);
        let n = self.modules.len();
        let mut master: Vec<Option<ModuleSlot>> = std::mem::take(&mut self.modules).into_iter().map(Some).collect();
        let jobs = view.jobs.clone();
        let arc = Arc::new(view);
        let mut hosts: Vec<ScriptHost> = lanes
            .iter()
            .map(|lane| {
                let mut slots: Vec<Option<ModuleSlot>> = (0..n).map(|_| None).collect();
                for &i in lane {
                    slots[i] = master[i].take();
                }
                ScriptHost::lane(arc.clone(), slots)
            })
            .collect();
        {
            let mut tasks: Vec<_> = hosts
                .iter_mut()
                .zip(&lanes)
                .map(|(h, lane)| move || h.run_lane(lane, frame))
                .collect();
            let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks.iter_mut().map(|t| t as &mut (dyn FnMut() + Send)).collect();
            exec.run(&mut refs);
        }

        // Take everything back out of the lane hosts and drop them (releases the Arc).
        struct LaneOut {
            writes: Vec<(Entity, u32, Vec<u8>)>,
            write_index: std::collections::HashMap<(Entity, u32), usize>,
            commands: crate::engine::commands::Commands,
            script_state: Vec<(Entity, i64)>,
            events_out: Vec<ScriptEvent>,
            dropped: usize,
            logs: Vec<LogLine>,
            warns: Vec<(String, String)>,
            overflow: std::collections::BTreeMap<ModuleId, u64>,
        }
        let mut outs: Vec<LaneOut> = Vec::with_capacity(hosts.len());
        for (host, lane) in hosts.into_iter().zip(&lanes) {
            let ScriptHost { frame, mut modules, writes, write_index, commands, script_state, events_out, dropped_events, logs, warns, overflow, current: _ } = host;
            drop(frame);
            for &i in lane {
                master[i] = modules[i].take();
            }
            outs.push(LaneOut { writes, write_index, commands, script_state, events_out, dropped: dropped_events, logs, warns, overflow });
        }
        let mut view = match Arc::try_unwrap(arc) {
            Ok(v) => v,
            Err(_) => panic!("script lanes still hold the frame snapshot after the script phase (a lane host leaked)"),
        };
        self.modules = master.into_iter().map(|s| s.expect("every module slot returns from its lane")).collect();

        // Merge, in lane order.
        let types = view.types.clone();
        let mut stats = ScriptStats { lanes: lanes.len(), ..Default::default() };
        let mut written: std::collections::HashMap<(Entity, u32), usize> = std::collections::HashMap::new();
        for (li, out) in outs.iter().enumerate() {
            for (i, (e, c, bytes)) in out.writes.iter().enumerate() {
                if out.write_index.get(&(*e, *c)) != Some(&i) {
                    continue; // overwritten later in this lane: only the last write counts
                }
                match written.insert((*e, *c), li) {
                    Some(prev) if prev != li => {
                        stats.write_conflicts += 1;
                        ::log::debug!("script write conflict on {e:?} component {c:#010x}: lane {li} overwrites lane {prev}");
                    }
                    _ => {}
                }
                if let Ok(entry) = types.component(*c) {
                    if let Err(err) = entry.write(&mut view.world, *e, bytes) {
                        ::log::debug!("script write on {e:?} component {c:#010x} failed at the merge: {err}");
                    }
                }
            }
        }
        for out in &outs {
            for &(e, v) in &out.script_state {
                if let Ok(mut s) = view.world.get::<&mut Script>(e) {
                    s.state = v;
                }
            }
        }
        for out in &mut outs {
            self.pending.append(&mut out.commands);
        }
        for out in &mut outs {
            stats.dropped_events += out.dropped;
            for ev in out.events_out.drain(..) {
                if self.events_out.len() < host::MAX_EVENTS {
                    self.events_out.push(ev);
                } else {
                    stats.dropped_events += 1;
                }
            }
            for l in out.logs.drain(..) {
                self.log(l.level, l.text);
            }
            for (k, t) in out.warns.drain(..) {
                self.warn_once(k, t);
            }
            stats.spawn_overflows += out.overflow.values().sum::<u64>();
        }
        if stats.write_conflicts > 0 {
            self.log(LogLevel::Warn, format!("{} script write conflicts this frame (several modules set the same component of the same entity; the later module won)", stats.write_conflicts));
        }
        // Unused pool entities are despawned (after everything else); the pool follows demand.
        let mut overflow: std::collections::BTreeMap<ModuleId, u64> = Default::default();
        for out in &outs {
            for (&id, &c) in &out.overflow {
                *overflow.entry(id).or_insert(0) += c;
            }
        }
        for (idx, slot) in self.modules.iter_mut().enumerate() {
            if !slot.loaded {
                continue;
            }
            let left = std::mem::take(&mut slot.spawn_pool);
            let used = initial[idx] - left.len();
            let demand = used + overflow.get(&ModuleId(idx as u32)).copied().unwrap_or(0) as usize;
            slot.pool_size = (2 * demand).clamp(host::MIN_POOL, host::MAX_POOL);
            for e in left {
                self.pending.despawn(e);
            }
        }
        self.stats = stats;
        // Scripts that point at a module that is not loaded.
        for &(m, _) in &jobs {
            if !self.modules.get(m as usize).is_some_and(|s| s.loaded) {
                self.warn_once(format!("gone:{m}"), format!("scripts refer to module id {m}, which is not loaded; they are skipped"));
            }
        }
        self.frame = Some(view);
    }
}

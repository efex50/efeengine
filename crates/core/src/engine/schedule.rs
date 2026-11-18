//! System schedule: stages and the per-system context.

use std::sync::Arc;

use hecs::{Component, DynamicBundle, Entity, Query, QueryBorrow, Tick, World};

use crate::engine::{
    WindowState, Windows,
    events::{EventRegistry, Events},
    input::{GamepadState, InputState},
    resources::{Res, Resources},
    time::Time,
    scripting::Scripting,
    types::TypeRegistry,
};
pub use crate::engine::commands::{Commands, EngineCommand, EngineCommands, ParCommands};
pub use crate::engine::parallel::{Access, ParCtx};
use crate::engine::parallel::{Executor, ParOutput};
use crate::engine::order;

/// When a system runs. See the frame order in `docs/design-io-and-loop.md` section 9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Once, at the start of the first frame.
    Startup,
    PreUpdate,
    /// Zero or more times per frame from the fixed-step accumulator.
    FixedUpdate,
    Update,
    PostUpdate,
    RenderExtract,
    /// Once, when the engine is killed.
    Shutdown,
}

impl Stage {
    pub(crate) const COUNT: usize = 7;

    fn index(self) -> usize {
        self as usize
    }
}

/// Borrowed view of the engine handed to every system.
pub struct Ctx<'a> {
    /// The game world. Query it freely; queue structural changes through `commands`
    /// (or [`Ctx::spawn`]) so they are applied after the stage.
    pub world: &'a mut World,
    /// Registered script-visible types (components and structs).
    pub types: &'a TypeRegistry,
    pub resources: &'a mut Resources,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub gamepads: &'a GamepadState,
    /// The main window's state (same as `windows.main()`).
    pub window: &'a WindowState,
    /// Every window's state.
    pub windows: &'a Windows,
    pub commands: &'a mut Commands,
    /// The scripting runtime (host registry, loaded modules). Systems may register host
    /// functions or load modules; the script system itself runs first in `Stage::Update`.
    pub scripting: &'a mut Scripting,
    events: &'a mut EventRegistry,
    /// The engine's executor (the script system runs its lanes through it).
    pub(crate) exec: &'a Executor,
    last_run: Tick,
    this_run: Tick,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(
        world: &'a mut World,
        types: &'a TypeRegistry,
        resources: &'a mut Resources,
        time: &'a Time,
        input: &'a InputState,
        gamepads: &'a GamepadState,
        windows: &'a Windows,
        commands: &'a mut Commands,
        scripting: &'a mut Scripting,
        events: &'a mut EventRegistry,
        exec: &'a Executor,
    ) -> Self {
        Self {
            world,
            types,
            resources,
            time,
            input,
            gamepads,
            window: windows.main(),
            windows,
            commands,
            scripting,
            events,
            exec,
            last_run: Tick::new(0),
            this_run: Tick::new(0),
        }
    }

    /// The change tick this system last ran at (`Tick::new(0)` before its first run, so the
    /// first run sees everything as added and changed). Only meaningful inside a system run
    /// by the schedule.
    pub fn last_run(&self) -> Tick {
        self.last_run
    }

    /// The change tick of this run: what `&mut T` / `Mut<T>` writes through [`Ctx::query`]
    /// stamp, and what the world's [`change_tick`](World::change_tick) equals while the system
    /// runs.
    pub fn this_run(&self) -> Tick {
        self.this_run
    }

    /// Query the world with this system's change ticks, so `Changed<T>` / `Added<T>` filters
    /// compare against its last run and `&mut T` stamps its tick:
    ///
    /// ```ignore
    /// for (e, t, _) in ctx.query::<(Entity, &Transform, Changed<Transform>)>().iter() { /* ... */ }
    /// ```
    ///
    /// Only components registered with `track_changes` report changes (`Transform` and
    /// `GlobalTransform` do); filters on an untracked component match everything.
    pub fn query<Q: Query>(&self) -> QueryBorrow<'_, Q> {
        self.world.query::<Q>().with_ticks(self.last_run, self.this_run)
    }

    /// Whether component `T` of `e` was added or changed since this system's last run.
    /// `true` for a component that does not track changes; `false` if `e` or `T` is missing.
    pub fn changed<T: Component>(&self, e: Entity) -> bool {
        self.world.is_changed::<T>(e, self.last_run).unwrap_or(false)
    }

    /// Input filtered by the UI capture flags ([`InputState::game_input`]).
    pub fn game_input(&self) -> crate::engine::input::GameInput<'_> {
        self.input.game_input()
    }

    /// Whether resource `T` was inserted or changed since this system's last run (`false` if
    /// missing). Changes are `ResMut` mutable derefs (parallel systems) and `Resources::get_mut`
    /// / `insert` (exclusive code).
    pub fn res_changed<T: Send + Sync + 'static>(&self) -> bool {
        self.resources.is_changed::<T>(self.last_run, self.this_run)
    }

    /// The bytes of the mod-defined component `type_id` (an id from the [`TypeRegistry`], e.g.
    /// `types.by_name("mod:coins/Health")`) of `e`. Holds a shared column borrow until dropped.
    /// Use [`TypeRegistry::view`] for field access by name.
    pub fn dyn_get(&self, e: Entity, type_id: u32) -> Result<hecs::DynRef<'_>, crate::engine::types::CompError> {
        self.types.component(type_id)?.dyn_ref(self.world, e)
    }

    /// Like [`dyn_get`](Self::dyn_get), for writing; stamps the component changed (if it tracks
    /// changes) with the world's current tick.
    pub fn dyn_get_mut(&mut self, e: Entity, type_id: u32) -> Result<hecs::DynRefMut<'_>, crate::engine::types::CompError> {
        self.types.component(type_id)?.dyn_ref_mut(self.world, e)
    }

    /// Events of type `T` readable this frame (empty if none were ever sent).
    pub fn events<T: Send + Sync + 'static>(&self) -> &Events<T> {
        self.events.get_or_empty::<T>()
    }

    /// Queue edits of the schedule: add, enable/disable or remove systems. They are applied at
    /// the end of the current stage, after the world commands, in queue order (see
    /// [`ScheduleCommands`]).
    pub fn schedule(&mut self) -> ScheduleCommands<'_> {
        ScheduleCommands { ops: self.commands.sched_ops() }
    }

    /// Spawn an entity with `bundle`: the entity is reserved now (the handle is valid for
    /// further commands at once) and gets its components when the stage's commands apply.
    pub fn spawn(&mut self, bundle: impl DynamicBundle) -> Entity {
        self.commands.spawn(self.world, bundle)
    }

    /// Send an event; readable by every system during the next frame.
    pub fn send<T: Send + Sync + 'static>(&mut self, ev: T) {
        self.events.send(ev);
    }

    /// Send an event that is readable **now**: by every system that runs later in this frame
    /// (and by the same system if it reads after sending). It stays readable until the next
    /// frame's event update drops it, like any readable event; it is never delivered twice.
    /// Use it for events produced by an input-like system early in the frame (for example UI
    /// signals); normal game events should use [`send`](Self::send).
    pub fn send_now<T: Send + Sync + 'static>(&mut self, ev: T) {
        self.events.get_mut::<T>().extend_readable(std::iter::once(ev));
    }

    /// [`send_now`](Self::send_now) for several events, in order.
    pub fn send_batch_now<T: Send + Sync + 'static>(&mut self, evs: impl IntoIterator<Item = T>) {
        self.events.get_mut::<T>().extend_readable(evs);
    }

    /// Send the registered script event `event` (`namespace.name`) with `payload` (exactly the
    /// size of its payload struct; empty for an event without payload). It is refused, and not
    /// queued, if the event is not registered or the size is wrong
    /// ([`ScriptEventError`](crate::engine::scripting::ScriptEventError)). Readable by scripts
    /// (`engine.events.read`) and by Rust systems ([`script_events`](Self::script_events))
    /// during the next frame. Rust sends do not count against the scripts' 4096 per frame cap.
    pub fn send_script_event(&mut self, event: &str, payload: &[u8]) -> Result<(), crate::engine::scripting::ScriptEventError> {
        self.scripting.check_event(event, payload.len())?;
        self.events.send(crate::engine::scripting::ScriptEvent { event: event.to_string(), payload: payload.to_vec() });
        Ok(())
    }

    /// The payloads of the script events `event` (`namespace.name`) readable this frame, in
    /// send order (sent by scripts or by Rust last frame). Entries whose size is not the
    /// registered payload size are skipped.
    pub fn script_events<'b>(&'b self, event: &'b str) -> impl Iterator<Item = &'b [u8]> + 'b {
        let size = self.scripting.event_payload_size(event);
        self.events::<crate::engine::scripting::ScriptEvent>()
            .iter()
            .filter(move |e| e.event == event && Some(e.payload.len()) == size)
            .map(|e| e.payload.as_slice())
    }
}

/// Read-only view handed to a system's run condition ([`SystemEntry::run_if`]).
pub struct RunCtx<'a> {
    pub world: &'a World,
    pub types: &'a TypeRegistry,
    pub resources: &'a Resources,
    pub time: &'a Time,
    pub input: &'a InputState,
    pub gamepads: &'a GamepadState,
    pub window: &'a WindowState,
    pub windows: &'a Windows,
}

impl RunCtx<'_> {
    /// Shared access to the resource `T` (`None` if absent or currently mutably borrowed).
    pub fn res<T: Send + Sync + 'static>(&self) -> Option<Res<'_, T>> {
        self.resources.get::<T>()
    }
}

/// A run condition: evaluated on the main thread, in sorted system order, before a stage's
/// batches are formed; `false` skips the system for this run.
pub type RunIf = Box<dyn Fn(&RunCtx<'_>) -> bool + Send + Sync>;

/// A boxed exclusive system function (runs alone, on the calling thread).
pub type System = Box<dyn FnMut(&mut Ctx<'_>)>;

/// A boxed parallel system function (see [`ParCtx`]).
pub type ParSystem = Box<dyn FnMut(&ParCtx<'_>) + Send>;

enum Entry {
    Exclusive(System),
    Par { access: Access, f: ParSystem },
}

/// One registered system: its function plus ordering metadata.
struct Sys {
    kind: Entry,
    order: i32,
    /// Global registration index (monotonic over all stages); the tie-breaker for equal orders.
    seq: u64,
    name: &'static str,
    /// Plugin that was being built when the system was added.
    owner: Option<Arc<str>>,
    /// Change tick of this system's previous run (`Tick::new(0)` before the first).
    last_run: Tick,
    /// Disabled systems are skipped (and keep their `last_run`).
    enabled: bool,
    run_if: Option<RunIf>,
}

impl Sys {
    fn is_exclusive(&self) -> bool {
        matches!(self.kind, Entry::Exclusive(_))
    }
}

/// One batch of a stage: the entries `start..end` (of the sorted list). An exclusive batch is a
/// single exclusive system; a parallel batch is a run of par systems whose accesses do not conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchInfo {
    pub start: usize,
    pub end: usize,
    pub exclusive: bool,
}

#[derive(Default)]
struct StageSystems {
    entries: Vec<Sys>,
    batches: Vec<BatchInfo>,
    /// Entries were added, removed or reordered since the last sort.
    dirty: bool,
    /// `batches` is valid for the `active` set it was built with.
    batches_valid: bool,
    /// The active set (enabled and run condition true) per sorted entry the batches were built for.
    built_for: Vec<bool>,
}

impl StageSystems {
    /// Sort by `(order, registration index)` if entries changed.
    fn ensure_sorted(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.batches_valid = false;
        self.entries.sort_by_key(|s| (s.order, s.seq));
        #[cfg(debug_assertions)]
        self.diagnose();
    }

    /// Active set from the `enabled` flags only (no run conditions).
    fn enabled_set(&self) -> Vec<bool> {
        self.entries.iter().map(|s| s.enabled).collect()
    }

    /// Rebuild the batches for `active` unless they are already built for it. Walking the sorted
    /// list, an active par system joins the current batch if it does not conflict with any active
    /// member; an active exclusive system always gets a batch of its own; inactive systems are
    /// skipped (they never split a batch). No look-ahead.
    fn ensure_batches(&mut self, active: &[bool]) {
        if self.batches_valid && self.built_for == active {
            return;
        }
        self.batches_valid = true;
        self.built_for.clear();
        self.built_for.extend_from_slice(active);
        self.batches.clear();
        for i in 0..self.entries.len() {
            if !active[i] {
                // Trailing inactive entries stay inside a parallel batch (run skips them).
                if let Some(b) = self.batches.last_mut() {
                    if !b.exclusive && b.end == i {
                        b.end = i + 1;
                    }
                }
                continue;
            }
            let joins = match (&self.entries[i].kind, self.batches.last()) {
                (Entry::Par { access, .. }, Some(b)) if !b.exclusive => {
                    (b.start..b.end).filter(|&j| active[j]).all(|j| match &self.entries[j].kind {
                        Entry::Par { access: other, .. } => !access.conflicts(other),
                        Entry::Exclusive(_) => false,
                    })
                }
                _ => false,
            };
            if joins {
                self.batches.last_mut().expect("joins implies a batch").end = i + 1;
            } else {
                self.batches.push(BatchInfo { start: i, end: i + 1, exclusive: self.entries[i].is_exclusive() });
            }
        }
    }

    /// Debug note for systems of different plugins that share an order number and conflict:
    /// their relative order then depends on plugin registration order.
    #[cfg(debug_assertions)]
    fn diagnose(&self) {
        let mut i = 0;
        while i < self.entries.len() {
            let mut j = i + 1;
            while j < self.entries.len() && self.entries[j].order == self.entries[i].order {
                j += 1;
            }
            for a in i..j {
                for b in a + 1..j {
                    let (x, y) = (&self.entries[a], &self.entries[b]);
                    if x.owner == y.owner {
                        continue;
                    }
                    let conflict = match (&x.kind, &y.kind) {
                        (Entry::Par { access: p, .. }, Entry::Par { access: q, .. }) => p.conflicts(q),
                        _ => true,
                    };
                    if conflict {
                        log::debug!(
                            "systems `{}` ({}) and `{}` ({}) share order {} and conflict; they run in registration order",
                            x.name,
                            x.owner.as_deref().unwrap_or("<game>"),
                            y.name,
                            y.owner.as_deref().unwrap_or("<game>"),
                            x.order
                        );
                    }
                }
            }
            i = j;
        }
    }
}

/// Handle to the system that was just added; sets its order and name. Dropping it is fine.
pub struct SystemEntry<'a> {
    stage: &'a mut StageSystems,
    index: usize,
}

impl<'a> SystemEntry<'a> {
    /// Run position inside the stage: lower numbers run first, ties keep registration order
    /// (see [`order`](crate::engine::order)). Default: [`order::USER`](crate::engine::order::USER).
    pub fn order(self, order: i32) -> Self {
        self.stage.entries[self.index].order = order;
        self.stage.dirty = true;
        self
    }

    /// Whether the system runs (default `true`). A disabled system is skipped, does not split
    /// batches and does not advance its change tick: when re-enabled it sees every change since
    /// it last ran. Toggle later with [`Engine::set_system_enabled`](crate::Engine::set_system_enabled).
    pub fn enabled(self, enabled: bool) -> Self {
        self.stage.entries[self.index].enabled = enabled;
        self.stage.batches_valid = false;
        self
    }

    /// Run only while `cond` returns true. Evaluated every frame (every fixed step for
    /// `FixedUpdate`) on the main thread, in sorted system order, before the stage's batches
    /// are formed; a system skipped by its condition behaves like a disabled one.
    pub fn run_if(self, cond: impl Fn(&RunCtx<'_>) -> bool + Send + Sync + 'static) -> Self {
        self.stage.entries[self.index].run_if = Some(Box::new(cond));
        self.stage.batches_valid = false;
        self
    }

    /// The name shown in debug messages and access-check panics.
    pub fn name(self, name: &'static str) -> Self {
        let e = &mut self.stage.entries[self.index];
        e.name = name;
        if let Entry::Par { access, .. } = &mut e.kind {
            access.set_name(name);
        }
        self
    }
}

/// Systems per stage, sorted by `(order, registration index)` and grouped into batches.
pub struct Schedule {
    stages: [StageSystems; Stage::COUNT],
    next_seq: u64,
    owner: Option<Arc<str>>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    pub fn new() -> Self {
        Self { stages: std::array::from_fn(|_| StageSystems::default()), next_seq: 0, owner: None }
    }

    /// The plugin owner recorded on systems added now (`None` outside `Plugin::build`).
    pub(crate) fn current_owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// Set the plugin owner recorded on systems added from now on; returns the previous one.
    pub(crate) fn swap_owner(&mut self, owner: Option<Arc<str>>) -> Option<Arc<str>> {
        std::mem::replace(&mut self.owner, owner)
    }

    fn push(&mut self, stage: Stage, kind: Entry, name: &'static str) -> SystemEntry<'_> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let st = &mut self.stages[stage.index()];
        st.entries.push(Sys { kind, order: order::USER, seq, name, owner: self.owner.clone(), last_run: Tick::new(0), enabled: true, run_if: None });
        st.dirty = true;
        let index = st.entries.len() - 1;
        SystemEntry { stage: st, index }
    }

    /// Add an exclusive system (default name `name`, order [`order::USER`]).
    pub fn add(&mut self, stage: Stage, system: System, name: &'static str) -> SystemEntry<'_> {
        self.push(stage, Entry::Exclusive(system), name)
    }

    /// Add a parallel system with its declared access.
    pub fn add_par(&mut self, stage: Stage, mut access: Access, system: ParSystem, name: &'static str) -> SystemEntry<'_> {
        access.set_default_name(name);
        let name = access.name_or(name);
        self.push(stage, Entry::Par { access, f: system }, name)
    }

    pub fn len(&self, stage: Stage) -> usize {
        self.stages[stage.index()].entries.len()
    }

    /// Indices of `stage`'s entries in execution order (without mutating).
    fn sorted_indices(&self, stage: Stage) -> Vec<usize> {
        let st = &self.stages[stage.index()];
        let mut idx: Vec<usize> = (0..st.entries.len()).collect();
        if st.dirty {
            idx.sort_by_key(|&i| (st.entries[i].order, st.entries[i].seq));
        }
        idx
    }

    /// System names of `stage` in execution order.
    pub fn system_names(&self, stage: Stage) -> Vec<&str> {
        let st = &self.stages[stage.index()];
        self.sorted_indices(stage).into_iter().map(|i| st.entries[i].name).collect()
    }

    /// Owning plugin (if any) of each system of `stage`, in execution order.
    pub fn system_owners(&self, stage: Stage) -> Vec<Option<&str>> {
        let st = &self.stages[stage.index()];
        self.sorted_indices(stage).into_iter().map(|i| st.entries[i].owner.as_deref()).collect()
    }

    /// `(name, enabled)` of each system of `stage` in execution order. Disabled systems are
    /// listed (`system_names` lists them too).
    pub fn system_states(&self, stage: Stage) -> Vec<(&str, bool)> {
        let st = &self.stages[stage.index()];
        self.sorted_indices(stage).into_iter().map(|i| (st.entries[i].name, st.entries[i].enabled)).collect()
    }

    /// The batches of `stage` over the enabled systems (run conditions are not evaluated here;
    /// sorts first if needed).
    pub(crate) fn batches(&mut self, stage: Stage) -> Vec<BatchInfo> {
        let st = &mut self.stages[stage.index()];
        st.ensure_sorted();
        let active = st.enabled_set();
        st.ensure_batches(&active);
        st.batches.clone()
    }

    /// Enable or disable every system named `name` in one stage; returns how many matched.
    pub(crate) fn set_enabled_in(&mut self, stage: Stage, name: &str, enabled: bool) -> usize {
        let st = &mut self.stages[stage.index()];
        let mut n = 0;
        for e in st.entries.iter_mut().filter(|e| e.name == name) {
            e.enabled = enabled;
            n += 1;
        }
        st.batches_valid = false;
        n
    }

    /// Enable or disable every system named `name` (in all stages); returns how many matched.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> usize {
        let mut n = 0;
        for st in &mut self.stages {
            for e in st.entries.iter_mut().filter(|e| e.name == name) {
                e.enabled = enabled;
                n += 1;
            }
            st.batches_valid = false;
        }
        n
    }

    /// Whether any system named `name` is enabled; `None` if there is none.
    pub fn is_enabled(&self, name: &str) -> Option<bool> {
        let mut found = false;
        let mut any = false;
        for e in self.stages.iter().flat_map(|st| st.entries.iter()).filter(|e| e.name == name) {
            found = true;
            any |= e.enabled;
        }
        found.then_some(any)
    }

    /// Remove every system named `name` (in all stages); returns how many were removed.
    pub fn remove(&mut self, name: &str) -> usize {
        let mut n = 0;
        for st in &mut self.stages {
            let before = st.entries.len();
            st.entries.retain(|e| e.name != name);
            let removed = before - st.entries.len();
            if removed > 0 {
                n += removed;
                st.dirty = true;
            }
        }
        n
    }

    /// Apply queued edits in order. Unknown names are logged and skipped.
    pub(crate) fn apply_ops(&mut self, ops: Vec<ScheduleOp>) {
        for op in ops {
            match op {
                ScheduleOp::Add { stage, kind, name, order, enabled, run_if } => {
                    let kind = match kind {
                        NewKind::Exclusive(f) => Entry::Exclusive(f),
                        NewKind::Par { mut access, f } => {
                            access.set_name(name);
                            Entry::Par { access, f }
                        }
                    };
                    let entry = self.push(stage, kind, name);
                    let e = &mut entry.stage.entries[entry.index];
                    e.order = order;
                    e.enabled = enabled;
                    e.run_if = run_if;
                }
                ScheduleOp::SetEnabled(name, on) => {
                    if self.set_enabled(&name, on) == 0 {
                        log::warn!("schedule: set_enabled({name}): no such system");
                    }
                }
                ScheduleOp::Remove(name) => {
                    if self.remove(&name) == 0 {
                        log::warn!("schedule: remove({name}): no such system");
                    }
                }
            }
        }
    }

    /// Number of batches of `stage`.
    #[doc(hidden)]
    pub fn batch_count(&mut self, stage: Stage) -> usize {
        self.batches(stage).len()
    }

    pub(crate) fn run(&mut self, stage: Stage, cx: &mut Ctx<'_>, exec: &Executor) {
        let st = &mut self.stages[stage.index()];
        st.ensure_sorted();
        // Run conditions: main thread, sorted order, before any batch is formed.
        let active: Vec<bool> = {
            let rc = RunCtx {
                world: &*cx.world,
                types: cx.types,
                resources: &*cx.resources,
                time: cx.time,
                input: cx.input,
                gamepads: cx.gamepads,
                window: cx.window,
                windows: cx.windows,
            };
            st.entries.iter().map(|e| e.enabled && e.run_if.as_ref().is_none_or(|c| c(&rc))).collect()
        };
        st.ensure_batches(&active);
        let StageSystems { entries, batches, .. } = st;
        for b in batches.iter() {
            let members = &mut entries[b.start..b.end];
            let act = &active[b.start..b.end];
            if b.exclusive {
                let Entry::Exclusive(f) = &mut members[0].kind else { unreachable!("exclusive batch holds an exclusive system") };
                let this_run = cx.world.increment_change_tick();
                cx.last_run = members[0].last_run;
                cx.this_run = this_run;
                cx.resources.set_tick(this_run);
                f(cx);
                members[0].last_run = this_run;
                continue;
            }
            // Parallel batch: every active member gets its own ParCtx over the shared world.
            let mut members: Vec<&mut Sys> = members.iter_mut().zip(act).filter(|(_, a)| **a).map(|(m, _)| m).collect();
            let mut outs: Vec<ParOutput> = (0..members.len()).map(|_| ParOutput::default()).collect();
            // Ticks are assigned in sorted member order, before anything runs, so they never
            // depend on completion order (the 1 vs N thread determinism contract).
            let ticks: Vec<(Tick, Tick)> =
                members.iter().map(|m| (m.last_run, cx.world.increment_change_tick())).collect();
            {
                let world: &World = &*cx.world;
                let resources: &Resources = &*cx.resources;
                let events: &EventRegistry = &*cx.events;
                let (types, time, input, gamepads, window) = (cx.types, cx.time, cx.input, cx.gamepads, cx.windows);
                let mut tasks: Vec<_> = members
                    .iter_mut()
                    .zip(outs.iter_mut())
                    .zip(ticks.iter().copied())
                    .map(|((entry, out), (last_run, this_run))| {
                        let Entry::Par { access, f } = &mut entry.kind else { unreachable!("parallel batch holds par systems") };
                        let access: &Access = access;
                        move || {
                            let pcx = ParCtx::new(world, types, time, input, gamepads, window, resources, events, access, exec, last_run, this_run);
                            f(&pcx);
                            *out = pcx.into_output();
                        }
                    })
                    .collect();
                let mut refs: Vec<&mut (dyn FnMut() + Send)> =
                    tasks.iter_mut().map(|t| t as &mut (dyn FnMut() + Send)).collect();
                exec.run(&mut refs);
            }
            for (m, (_, this_run)) in members.iter_mut().zip(&ticks) {
                m.last_run = *this_run;
            }
            // Merge in sorted (execution) order, not completion order.
            for out in &mut outs {
                cx.commands.append(&mut out.commands);
                for send in out.sends.drain(..) {
                    send(&mut *cx.events);
                }
            }
        }
    }
}

/// What a deferred `add_system` carries.
pub(crate) enum NewKind {
    /// `Send` so the queue ([`Commands`]) stays `Send` (parallel systems queue into it too).
    Exclusive(Box<dyn FnMut(&mut Ctx<'_>) + Send>),
    Par { access: Access, f: ParSystem },
}

/// A queued schedule edit, applied at the end of the stage (see [`ScheduleCommands`]).
pub(crate) enum ScheduleOp {
    Add { stage: Stage, kind: NewKind, name: &'static str, order: i32, enabled: bool, run_if: Option<RunIf> },
    SetEnabled(String, bool),
    Remove(String),
}

impl std::fmt::Debug for ScheduleOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleOp::Add { stage, name, .. } => write!(f, "Add({stage:?}, {name})"),
            ScheduleOp::SetEnabled(n, on) => write!(f, "SetEnabled({n}, {on})"),
            ScheduleOp::Remove(n) => write!(f, "Remove({n})"),
        }
    }
}

/// Deferred schedule edits ([`Ctx::schedule`], [`ParCommands`]): applied at the end of the
/// current stage, after the world commands, in queue order (parallel systems' queues merge in
/// sorted system order, like commands). A system added this way gets the next registration
/// index, so with an equal order number it runs after the earlier ones, from the next run of
/// its stage on (a `Startup` system added late never runs). Names that match nothing are
/// logged and skipped. Systems added late must be `Send` (they sit in the command queue).
pub struct ScheduleCommands<'a> {
    pub(crate) ops: &'a mut Vec<ScheduleOp>,
}

/// Handle to a queued `add_system`: set its order, name, enabled flag and run condition.
pub struct PendingSystem<'a> {
    ops: &'a mut Vec<ScheduleOp>,
}

impl PendingSystem<'_> {
    fn last(&mut self) -> (&mut i32, &mut &'static str, &mut bool, &mut Option<RunIf>) {
        match self.ops.last_mut() {
            Some(ScheduleOp::Add { order, name, enabled, run_if, .. }) => (order, name, enabled, run_if),
            _ => unreachable!("PendingSystem points at its Add op"),
        }
    }
    pub fn order(mut self, o: i32) -> Self {
        *self.last().0 = o;
        self
    }
    pub fn name(mut self, n: &'static str) -> Self {
        *self.last().1 = n;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        *self.last().2 = on;
        self
    }
    pub fn run_if(mut self, cond: impl Fn(&RunCtx<'_>) -> bool + Send + Sync + 'static) -> Self {
        *self.last().3 = Some(Box::new(cond));
        self
    }
}

impl ScheduleCommands<'_> {
    /// Queue adding an exclusive system to `stage`.
    pub fn add_system(&mut self, stage: Stage, f: impl FnMut(&mut Ctx<'_>) + Send + 'static) -> PendingSystem<'_> {
        let name = std::any::type_name_of_val(&f);
        self.ops.push(ScheduleOp::Add { stage, kind: NewKind::Exclusive(Box::new(f)), name, order: order::USER, enabled: true, run_if: None });
        PendingSystem { ops: &mut *self.ops }
    }

    /// Queue adding a parallel system to `stage`.
    pub fn add_par_system(&mut self, stage: Stage, access: Access, f: impl FnMut(&ParCtx<'_>) + Send + 'static) -> PendingSystem<'_> {
        let name = std::any::type_name_of_val(&f);
        self.ops.push(ScheduleOp::Add { stage, kind: NewKind::Par { access, f: Box::new(f) }, name, order: order::USER, enabled: true, run_if: None });
        PendingSystem { ops: &mut *self.ops }
    }

    /// Queue enabling or disabling every system named `name`.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        self.ops.push(ScheduleOp::SetEnabled(name.to_string(), enabled));
    }

    /// Queue removing every system named `name`.
    pub fn remove(&mut self, name: &str) {
        self.ops.push(ScheduleOp::Remove(name.to_string()));
    }
}

/// The schedule edit methods of [`Engine`](crate::Engine) failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("no system named `{0}`")]
    UnknownSystem(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;
    struct R;

    fn par(s: &mut Schedule, a: Access) {
        s.add_par(Stage::Update, a, Box::new(|_| {}), "p");
    }
    fn excl(s: &mut Schedule) {
        s.add(Stage::Update, Box::new(|_| {}), "x");
    }
    fn shape(s: &mut Schedule) -> Vec<(usize, usize, bool)> {
        s.batches(Stage::Update).iter().map(|b| (b.start, b.end, b.exclusive)).collect()
    }

    #[test]
    fn disjoint_par_systems_share_a_batch() {
        let mut s = Schedule::new();
        par(&mut s, Access::new().write::<A>());
        par(&mut s, Access::new().write::<B>());
        par(&mut s, Access::new().res_mut::<R>());
        assert_eq!(shape(&mut s), vec![(0, 3, false)]);
    }

    #[test]
    fn read_read_does_not_conflict() {
        let mut s = Schedule::new();
        par(&mut s, Access::new().read::<A>());
        par(&mut s, Access::new().read::<A>());
        assert_eq!(shape(&mut s), vec![(0, 2, false)]);
    }

    #[test]
    fn conflicting_systems_keep_order_in_separate_batches() {
        let mut s = Schedule::new();
        par(&mut s, Access::new().write::<A>()); // 0
        par(&mut s, Access::new().read::<A>()); // 1 conflicts with 0
        par(&mut s, Access::new().write::<B>()); // 2 joins 1 (no look-ahead)
        par(&mut s, Access::new().read::<B>()); // 3 conflicts with 2
        assert_eq!(shape(&mut s), vec![(0, 1, false), (1, 3, false), (3, 4, false)]);
    }

    #[test]
    fn exclusive_system_closes_a_batch() {
        let mut s = Schedule::new();
        par(&mut s, Access::new().read::<A>());
        excl(&mut s);
        par(&mut s, Access::new().read::<B>());
        par(&mut s, Access::new().read::<A>());
        excl(&mut s);
        assert_eq!(shape(&mut s), vec![(0, 1, false), (1, 2, true), (2, 4, false), (4, 5, true)]);
        assert_eq!(s.len(Stage::Update), 5);
    }

    #[test]
    fn stages_are_independent() {
        let mut s = Schedule::new();
        par(&mut s, Access::new().write::<A>());
        s.add_par(Stage::PostUpdate, Access::new().write::<A>(), Box::new(|_| {}), "p");
        assert_eq!(s.batches(Stage::PostUpdate).len(), 1);
        assert_eq!(s.batches(Stage::Update).len(), 1);
    }
}

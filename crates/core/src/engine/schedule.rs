//! System schedule: stages and the per-system context.

use std::sync::Arc;

use hecs::{DynamicBundle, Entity, World};

use crate::engine::{
    WindowState,
    events::{EventRegistry, Events},
    input::{GamepadState, InputState},
    resources::Resources,
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
    pub window: &'a WindowState,
    pub commands: &'a mut Commands,
    /// The scripting runtime (host registry, loaded modules). Systems may register host
    /// functions or load modules; the script system itself runs first in `Stage::Update`.
    pub scripting: &'a mut Scripting,
    events: &'a mut EventRegistry,
    /// The engine's executor (the script system runs its lanes through it).
    pub(crate) exec: &'a Executor,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(
        world: &'a mut World,
        types: &'a TypeRegistry,
        resources: &'a mut Resources,
        time: &'a Time,
        input: &'a InputState,
        gamepads: &'a GamepadState,
        window: &'a WindowState,
        commands: &'a mut Commands,
        scripting: &'a mut Scripting,
        events: &'a mut EventRegistry,
        exec: &'a Executor,
    ) -> Self {
        Self { world, types, resources, time, input, gamepads, window, commands, scripting, events, exec }
    }

    /// Events of type `T` readable this frame (empty if none were ever sent).
    pub fn events<T: Send + Sync + 'static>(&self) -> &Events<T> {
        self.events.get_or_empty::<T>()
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
    /// Entries were added or reordered since the last sort.
    dirty: bool,
}

impl StageSystems {
    /// Sort by `(order, registration index)` and rebuild the batches: walking the sorted list,
    /// a par system joins the current batch if it does not conflict with any member; an
    /// exclusive system always gets a batch of its own. No look-ahead.
    fn ensure_sorted(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.entries.sort_by_key(|s| (s.order, s.seq));
        #[cfg(debug_assertions)]
        self.diagnose();
        self.batches.clear();
        for i in 0..self.entries.len() {
            let joins = match (&self.entries[i].kind, self.batches.last()) {
                (Entry::Par { access, .. }, Some(b)) if !b.exclusive => {
                    self.entries[b.start..b.end].iter().all(|m| match &m.kind {
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

    /// Set the plugin owner recorded on systems added from now on; returns the previous one.
    pub(crate) fn swap_owner(&mut self, owner: Option<Arc<str>>) -> Option<Arc<str>> {
        std::mem::replace(&mut self.owner, owner)
    }

    fn push(&mut self, stage: Stage, kind: Entry, name: &'static str) -> SystemEntry<'_> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let st = &mut self.stages[stage.index()];
        st.entries.push(Sys { kind, order: order::USER, seq, name, owner: self.owner.clone() });
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

    /// The batches of `stage` (sorts first if needed).
    pub(crate) fn batches(&mut self, stage: Stage) -> Vec<BatchInfo> {
        let st = &mut self.stages[stage.index()];
        st.ensure_sorted();
        st.batches.clone()
    }

    /// Number of batches of `stage`.
    #[doc(hidden)]
    pub fn batch_count(&mut self, stage: Stage) -> usize {
        self.batches(stage).len()
    }

    pub(crate) fn run(&mut self, stage: Stage, cx: &mut Ctx<'_>, exec: &Executor) {
        self.stages[stage.index()].ensure_sorted();
        let StageSystems { entries, batches, .. } = &mut self.stages[stage.index()];
        for b in batches.iter() {
            let members = &mut entries[b.start..b.end];
            if b.exclusive {
                let Entry::Exclusive(f) = &mut members[0].kind else { unreachable!("exclusive batch holds an exclusive system") };
                f(cx);
                continue;
            }
            // Parallel batch: every member gets its own ParCtx over the shared world.
            let mut outs: Vec<ParOutput> = (0..members.len()).map(|_| ParOutput::default()).collect();
            {
                let world: &World = &*cx.world;
                let resources: &Resources = &*cx.resources;
                let events: &EventRegistry = &*cx.events;
                let (types, time, input, gamepads, window) = (cx.types, cx.time, cx.input, cx.gamepads, cx.window);
                let mut tasks: Vec<_> = members
                    .iter_mut()
                    .zip(outs.iter_mut())
                    .map(|(entry, out)| {
                        let Entry::Par { access, f } = &mut entry.kind else { unreachable!("parallel batch holds par systems") };
                        let access: &Access = access;
                        move || {
                            let pcx = ParCtx::new(world, types, time, input, gamepads, window, resources, events, access, exec);
                            f(&pcx);
                            *out = pcx.into_output();
                        }
                    })
                    .collect();
                let mut refs: Vec<&mut (dyn FnMut() + Send)> =
                    tasks.iter_mut().map(|t| t as &mut (dyn FnMut() + Send)).collect();
                exec.run(&mut refs);
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

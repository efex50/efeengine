//! `UiTest`: a headless engine with the UI plugin, driven like a user would (API.md section 14).
//!
//! Everything the helpers do goes through the same path a real backend uses: a signal queued in
//! the headless backend, delivered at `order::INPUT + 10` after the plugin's checks (disabled
//! and invisible widgets send nothing, value changes are written back into the tree).

use efeengine_core::engine::resources::Res;
use efeengine_core::prelude::*;

use crate::capture::UiCapture;
use crate::headless::HeadlessHandle;
use crate::id::WidgetId;
use crate::journal::UiChange;
use crate::plugin::UiPlugin;
use crate::signal::{UiSignal, UiSignalKind, UiValue};
use crate::tree::UiTree;

pub struct UiTest {
    /// The engine, for resources, systems and `step`s of your own.
    pub engine: Engine,
    ui: HeadlessHandle,
}

impl UiTest {
    /// A headless engine (60 Hz fixed step) with `UiPlugin::new()`, then `setup`.
    pub fn new(setup: impl FnOnce(&mut Engine)) -> Self {
        Self::with_plugin(UiPlugin::new(), setup)
    }

    /// Like [`new`](Self::new) with your own plugin value (for example one with a theme).
    /// It must use the headless backend.
    pub fn with_plugin(plugin: UiPlugin, setup: impl FnOnce(&mut Engine)) -> Self {
        let mut engine = Engine::with_config(EngineConfig { fixed_dt: 1.0 / 60.0, ..EngineConfig::headless() }).expect("headless engine");
        engine.add_plugin(plugin).expect("UiPlugin");
        setup(&mut engine);
        let ui = engine.resources().get::<HeadlessHandle>().expect("UiTest needs the headless backend").clone();
        Self { engine, ui }
    }

    /// Runs `n` frames. Panics if the headless backend found an inconsistent journal.
    pub fn step(&mut self, n: usize) {
        self.engine.run_frames(n, 1.0 / 60.0);
        let errors = self.ui.errors();
        assert!(errors.is_empty(), "headless UI backend found journal errors: {errors:#?}");
    }

    /// The headless backend's handle (recorded journal, injection, capture).
    pub fn backend(&self) -> &HeadlessHandle {
        &self.ui
    }

    pub fn ui(&self) -> Res<'_, UiTree> {
        self.engine.resources().get::<UiTree>().expect("UiTree")
    }

    pub fn ui_mut(&mut self) -> &mut UiTree {
        self.engine.resources_mut().get_mut::<UiTree>().expect("UiTree")
    }

    pub fn resource<T: Send + Sync + 'static>(&self) -> Res<'_, T> {
        self.engine.resources().get::<T>().unwrap_or_else(|| panic!("no resource `{}`", std::any::type_name::<T>()))
    }

    pub fn resource_mut<T: Send + Sync + 'static>(&mut self) -> &mut T {
        self.engine.resources_mut().get_mut::<T>().unwrap_or_else(|| panic!("no resource `{}`", std::any::type_name::<T>()))
    }

    pub fn capture(&self) -> UiCapture {
        self.engine.input().capture()
    }

    /// The journal entries the backend received so far.
    pub fn changes(&self) -> Vec<UiChange> {
        self.ui.changes()
    }

    /// The id for `key`. Panics if there is none.
    pub fn id(&self, key: &str) -> WidgetId {
        self.ui().find(key).unwrap_or_else(|| panic!("no widget with key `{key}`"))
    }

    /// The text a widget shows (text, label or input value).
    pub fn text(&self, key: &str) -> String {
        let ui = self.ui();
        ui.text_of(self.id(key)).unwrap_or_else(|| panic!("widget `{key}` has no text")).to_string()
    }

    pub fn value(&self, key: &str) -> Option<UiValue> {
        self.ui().value(self.id(key))
    }

    /// Queues an arbitrary signal kind for `key` (delivered by the next frame).
    pub fn signal(&mut self, key: &str, kind: UiSignalKind) {
        let id = self.id(key);
        let s = UiSignal::new(&self.ui(), id, kind);
        self.ui.inject(s);
    }

    pub fn click(&mut self, key: &str) {
        self.signal(key, UiSignalKind::Click);
    }

    pub fn set_value(&mut self, key: &str, value: f32) {
        self.signal(key, UiSignalKind::Changed(UiValue::F32(value)));
    }

    pub fn set_checked(&mut self, key: &str, checked: bool) {
        self.signal(key, UiSignalKind::Changed(UiValue::Bool(checked)));
    }

    /// Appends `text` to the input's value (as already typed in earlier queued calls).
    pub fn type_text(&mut self, key: &str, text: &str) {
        let id = self.id(key);
        let mut cur = self.ui().text_of(id).unwrap_or("").to_string();
        for s in self.ui.pending() {
            if s.widget == id
                && let UiSignalKind::Changed(UiValue::Text(t)) = s.kind
            {
                cur = t;
            }
        }
        cur.push_str(text);
        self.signal(key, UiSignalKind::Changed(UiValue::Text(cur)));
    }

    /// Presses Enter in the input (with the value as typed so far).
    pub fn submit(&mut self, key: &str) {
        let id = self.id(key);
        let mut cur = self.ui().text_of(id).unwrap_or("").to_string();
        for s in self.ui.pending() {
            if s.widget == id
                && let UiSignalKind::Changed(UiValue::Text(t)) = s.kind
            {
                cur = t;
            }
        }
        self.signal(key, UiSignalKind::Submit(cur));
    }

    pub fn focus(&mut self, key: &str, focused: bool) {
        self.signal(key, UiSignalKind::Focus(focused));
    }

    pub fn hover(&mut self, key: &str, hovered: bool) {
        self.signal(key, UiSignalKind::Hover(hovered));
    }
}

// Lets `#[derive(ScriptType)]` (which names `::efeengine_core::...`) work inside this crate.
extern crate self as efeengine_core;

#[allow(unused)]
pub mod engine;
pub mod prelude;

#[allow(unused)]
pub use prelude::*;
pub use engine::*;
/// The ECS used for the game world (ROADMAP Phase 3), re-exported as-is.
pub use hecs;
/// Math types used by components, re-exported as-is.
pub use glam;
/// The VM crate; its `StructDesc` / `FieldDesc` / `Prim` describe script-visible layouts.
pub use efevm;





#[cfg(target_family = "wasm")]
mod wasm{

    use wasm_bindgen::prelude::{Closure, wasm_bindgen};
    use wgpu::web_sys;
    #[wasm_bindgen(js_namespace = console)]
    unsafe extern "C" {
        pub fn log(s: &str);
        pub fn warn(s: &str);
        pub fn error(s: &str);
    }

    #[wasm_bindgen]
    pub fn zort() -> wasm_bindgen::JsValue{


        web_sys::console::log_1(&"safasfi".into());
        let closure = Closure::wrap(Box::new(move |message: String| {
            web_sys::console::log_1(&message.into());
        }) as Box<dyn FnMut(String)>);

        // 2. Extract the JS function wrapper
        let js_func = closure.as_ref().clone();

        // 3. IMPORTANT: Forget the closure to keep it alive
        // If you don't do this, Rust will drop the 'closure' variable 
        // at the end of this scope, and the JS function will become invalid.
        closure.forget();

        js_func
    }
}



#[cfg(test)]
mod tests{
    use serde_json::json;

    use crate::engine::messages::Message;

    /// JS sends messages in serde's default (externally tagged) shape;
    /// pin it so a refactor can't silently break `send_message` from JS.
    #[test]
    fn message_json_shape(){
        let shape = |m: &Message| serde_json::to_value(m).unwrap();
        assert_eq!(shape(&Message::Start), json!("Start"));
        assert_eq!(shape(&Message::Say("hi".into())), json!({ "Say": "hi" }));
        assert_eq!(shape(&Message::SetThreads(4)), json!({ "SetThreads": 4 }));

        let parsed: Message = serde_json::from_value(json!({ "ChangeTitle": "x" })).unwrap();
        assert!(matches!(parsed, Message::ChangeTitle(t) if t == "x"));
    }
}

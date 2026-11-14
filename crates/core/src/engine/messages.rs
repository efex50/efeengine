use serde::{Deserialize, Serialize};
#[cfg(target_family = "wasm")]
use ts_rs::TS;
use once_cell::sync::Lazy;

use crate::LogMsg;

/// Global engine message queue: an unbounded multi-producer channel.
/// Anything (JS, the window backend, scripts) can send; the engine drains it once per tick.
pub static MESSAGES: Lazy<(flume::Sender<Message>, flume::Receiver<Message>)> =
    Lazy::new(flume::unbounded);

/// Get a sender handle that can be cloned and moved to other threads.
pub fn message_sender() -> flume::Sender<Message> {
    MESSAGES.0.clone()
}

/// Queue a message for the engine to handle on its next tick.
pub fn push_message(message: Message) {
    // The receiver lives in a static and is never dropped, so sending cannot fail.
    let _ = MESSAGES.0.send(message);
}

/// Take every message that is queued right now.
/// Messages sent while the returned iterator is being consumed are kept for the next call.
pub(crate) fn drain_messages() -> flume::Drain<'static, Message> {
    MESSAGES.1.drain()
}



#[cfg_attr(target_family = "wasm", derive(TS))]
#[derive(Debug, Serialize, Deserialize)]
pub enum Message{
    SetFrameRate(f32),
    Start,
    Stop,
    Kill,
    Log(LogMsg),
    Say(String),
    ChangeTitle(String),
    #[cfg(target_family = "wasm")]
    SetCanvasId(String),
    // for development
    Null,
}



#[cfg(target_family = "wasm")]
mod wasm_message_handler{
    use wasm_bindgen::prelude::*;
    use super::*;
    #[wasm_bindgen]
    pub fn send_message(message:JsValue) -> Result<(), JsValue>{
        
        let message: Message = serde_wasm_bindgen::from_value(message)?;
        push_message(message);
        Ok(())
    }
}
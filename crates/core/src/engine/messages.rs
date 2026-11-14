use serde::{Deserialize, Serialize};
#[cfg(target_family = "wasm")]
use ts_rs::TS;
use once_cell::sync::Lazy;

use crate::{LogMsg, engine, window::events::WindowEvents};

/// Global engine message queue: an unbounded multi-producer channel.
/// Anything (JS, the window backend, scripts, worker threads) can send;
/// the engine drains it once per tick.
pub type MessageVecType = Lazy<(flume::Sender<Message>,flume::Receiver<Message>)>;

pub static MESSAGE_SYSTEM:MessageVecType = Lazy::new(flume::unbounded);

/// Get a sender handle that can be cloned and moved to other threads.
pub fn message_sender() -> flume::Sender<Message> {
    MESSAGE_SYSTEM.0.clone()
}

/// Queue a message for the engine to handle on its next tick.
pub fn send_message(msg:Message){
    // The receiver lives in a static and is never dropped, so sending cannot fail.
    let _ = MESSAGE_SYSTEM.0.send(msg);
}
/// Take every message that is queued right now.
/// Messages sent while the returned iterator is being consumed are kept for the next call.
pub fn recv_messages() -> flume::Drain<'static, engine::messages::Message>{
    let a = MESSAGE_SYSTEM.1.drain();
    return a
}


#[derive(Debug, Serialize, Deserialize)]
pub enum Message{
    SetFrameRate(f32),
    /// Resize the global thread pool (clamped to >= 1).
    SetThreads(usize),
    WindowEvent(WindowEvents),
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
        super::send_message(message);
        Ok(())
    }
}
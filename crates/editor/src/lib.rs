//! The efeengine editor (docs/EDITOR.md, phases E1 and E2): a standalone iced application that
//! shows and edits a running game over the editor-link protocol. The window and widgets live in
//! `main.rs`; everything testable without a window is here.

pub mod conn;
pub mod edit;
pub mod model;
pub mod worker;

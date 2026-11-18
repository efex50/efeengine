//! The efeengine editor (docs/EDITOR.md, phases E1 to E7.2): a standalone iced application that
//! shows and edits a running game over the editor-link protocol, and builds and runs it. The
//! window and widgets live in `main.rs`; everything testable without a window is here.

pub mod build;
pub mod cli;
pub mod config;
pub mod conn;
pub mod doctor;
pub mod edit;
pub mod gizmo;
pub mod model;
pub mod place;
pub mod projects;
pub mod restart;
pub mod route;
pub mod runner;
pub mod worker;

//! efevm: register-based bytecode VM (see SPEC.md).
#![forbid(unsafe_code)]

pub mod binary;
pub mod builder;
pub mod error;
pub mod module;
pub mod op;
pub mod trap;
pub mod types;

mod verify;
pub mod imports;
pub mod instance;
pub mod interp;
pub mod memory;

pub use binary::DecodeError;
pub use builder::{FunctionBuilder, Label, ModuleBuilder};
pub use imports::{Caller, HostFunc, HostOk, Imports};
pub use instance::{FiberId, FiberStatus, Instance};
pub use memory::Memory;
pub use error::{BuildError, CallError, LinkError, LoadError, SpawnError};
pub use module::{
    BrTable, DataSegment, Export, ExportKind, FuncLines, Function, Global, Import, Inst,
    LineEntry, Lines, MemoryType, Module, ModuleData, Names,
};
pub use op::{Op, Shape};
pub use trap::{Exit, Trap};
pub use types::{FuncType, Limits, PAGE_SIZE, ValType};
pub use verify::VerifyError;

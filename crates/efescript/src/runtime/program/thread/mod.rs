pub mod runtime;
pub mod syscall;
pub mod opcode;
pub mod opcode_funs;

use efepages::page::Page;

use super::{PFlags, PRegisters};
use crate::runtime::host::Host;


/// Everything a thread needs from its runtime while it executes.
/// Passed as an argument to every tick, so threads hold no pointers into the runtime.
pub struct Ctx<'a> {
    pub page: &'a mut Page,
    pub host: &'a mut dyn Host,
}

#[derive(Debug, Default)]
pub struct PThread{
    /// id of the thread
    pub id:u32,
    pub counter:usize,
    pub registers:PRegisters,
    pub flags:PFlags,
    /// lowest valid stack address (pop/ret below it is a fault)
    pub stack_base:usize,
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickSignal{
    /// an instruction ran
    Continue,
    /// the end-of-program byte (0) was reached
    End,
    /// the program called syscall Finish with this code
    Finish(u8),
    /// the program did something invalid; the message says what
    Fault(String),
}

pub mod init;
pub mod thread;
mod runtime;


use efepages::page::Page;
use thread::PThread;
use crate::runtime::host::{Host, StdHost};




#[derive(Debug,Default)]
pub struct PRegisters{
    /// accumulator     d:1
    pub ra:usize,
    /// base            d:2
    pub rb:usize, 
    /// counter         d:3
    pub rc:usize,
    /// data            d:4
    pub rd:usize,
    /// stack base pointer     d:5 
    pub rbp:usize,
    /// stack pointer           d:6
    pub rsp:usize,
    //other registers
    pub r1:usize,
    pub r2:usize,
    pub r3:usize,
    pub r4:usize,
    pub r5:usize,
    pub r6:usize,
    

}

#[derive(Debug,Default)]
pub struct PFlags{
    carry       :bool,
    equals      :bool,
    greater     :bool,
    lesser      :bool,
    greatereq   :bool,
    lessereq    :bool,
    zero        :bool,
    negative    :bool,
    sign        :bool,
}
impl PFlags {
    pub fn reset(&mut self){
        *self = Self::default();
    }
}





pub struct ProgramRuntime{
    pub counter:usize,
    pub program:Page,
    pub program_size:usize,
    pub threads:Vec<PThread>,
    pub stdin:Vec<u8>,
    pub stderr:Vec<u8>,
    pub stdout:Vec<u8>,
    /// output / sound sink (StdHost unless `with_host` was used)
    pub host:Box<dyn Host>,
}

impl Default for ProgramRuntime {
    fn default() -> Self {
        Self{
            counter:0,
            program:Page::default(),
            program_size:0,
            threads:Vec::new(),
            stdin:Vec::new(),
            stderr:Vec::new(),
            stdout:Vec::new(),
            host:Box::new(StdHost),
        }
    }
}

impl std::fmt::Debug for ProgramRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramRuntime")
            .field("counter", &self.counter)
            .field("program", &self.program)
            .field("program_size", &self.program_size)
            .field("threads", &self.threads)
            .field("host", &"<dyn Host>")
            .finish()
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunResult{
    /// the program called syscall Finish with this code
    Finished(u8),
    /// the end-of-program byte was reached
    EndOfProgram,
    /// `run_with_limit` executed its maximum number of instructions
    LimitReached,
    /// the program did something invalid (bad opcode, unknown syscall, stack underflow, ...)
    Fault(String),
}

#[cfg(test)]
mod tests;

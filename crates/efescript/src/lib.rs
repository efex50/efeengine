pub mod prelude;

pub(crate)mod ops;
pub(crate)mod macros;
pub(crate)mod funs;
pub(crate)mod syscalls;
pub(crate)mod instruction;
pub mod nasm_efe;
pub(crate)mod runtime;
pub mod compiler;
pub mod debugger;
pub mod asm;
pub mod host_std;
pub mod disasm;
pub mod run_v2;



proc_macros::max_addr!();
proc_macros::stack_page_count!();



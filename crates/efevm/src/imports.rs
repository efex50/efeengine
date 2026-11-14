//! Host function imports.

use crate::instance::FiberId;
use crate::memory::Memory;
use crate::trap::Trap;
use crate::types::FuncType;
use std::collections::HashMap;
use std::sync::Arc;

/// What a host function asks the VM to do after it returns successfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostOk {
    Continue,
    /// Write the results, then suspend the fiber with `Exit::Yield`.
    Yield,
}

/// Access handed to host functions: host state, memory and globals.
pub struct Caller<'a, H> {
    host: &'a mut H,
    memory: &'a mut Memory,
    globals: &'a mut [u64],
    mutable: &'a [bool],
    gmask: &'a [u64],
    fiber: FiberId,
}

impl<'a, H> Caller<'a, H> {
    pub(crate) fn new(
        host: &'a mut H,
        memory: &'a mut Memory,
        globals: &'a mut [u64],
        mutable: &'a [bool],
        gmask: &'a [u64],
        fiber: FiberId,
    ) -> Self {
        Caller { host, memory, globals, mutable, gmask, fiber }
    }

    /// The fiber that is making this host call.
    pub fn fiber(&self) -> FiberId {
        self.fiber
    }

    pub fn host(&mut self) -> &mut H {
        self.host
    }

    pub fn memory(&self) -> &Memory {
        self.memory
    }

    pub fn memory_mut(&mut self) -> &mut Memory {
        self.memory
    }

    pub fn read(&self, addr: u64, buf: &mut [u8]) -> Result<(), Trap> {
        self.memory.read(addr, buf)
    }

    pub fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), Trap> {
        self.memory.write(addr, data)
    }

    pub fn global(&self, idx: u32) -> Option<u64> {
        self.globals.get(idx as usize).copied()
    }

    /// `Trap::Host` on a bad index or an immutable global. The upper 32 bits of the value are
    /// cleared when the global is `i32` or `f32`.
    pub fn set_global(&mut self, idx: u32, v: u64) -> Result<(), Trap> {
        let i = idx as usize;
        match (self.globals.get_mut(i), self.mutable.get(i)) {
            (Some(slot), Some(true)) => {
                *slot = v & self.gmask.get(i).copied().unwrap_or(u64::MAX);
                Ok(())
            }
            (Some(_), _) => Err(Trap::Host(format!("global {idx} is immutable"))),
            _ => Err(Trap::Host(format!("no global {idx}"))),
        }
    }
}

/// A host function: `(caller, args, results)`.
pub type HostFunc<H> =
    Arc<dyn Fn(&mut Caller<'_, H>, &[u64], &mut [u64]) -> Result<HostOk, Trap> + Send + Sync>;

/// Host functions available to a module, keyed by (namespace, name).
pub struct Imports<H> {
    pub(crate) funcs: HashMap<(String, String), (FuncType, HostFunc<H>)>,
}

impl<H> Imports<H> {
    pub fn new() -> Self {
        Imports { funcs: HashMap::new() }
    }

    /// Registers (or replaces) a host function.
    pub fn func(
        &mut self,
        namespace: &str,
        name: &str,
        ty: FuncType,
        f: impl Fn(&mut Caller<'_, H>, &[u64], &mut [u64]) -> Result<HostOk, Trap> + Send + Sync + 'static,
    ) -> &mut Self {
        self.funcs.insert((namespace.to_string(), name.to_string()), (ty, Arc::new(f)));
        self
    }
}

impl<H> Default for Imports<H> {
    fn default() -> Self {
        Self::new()
    }
}

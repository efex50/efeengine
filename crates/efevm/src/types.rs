//! Value types, function types and embedder limits.

/// A value type. Registers are untyped 64-bit slots; types matter only at function boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ValType {
    I32 = 0,
    I64 = 1,
    F32 = 2,
    F64 = 3,
}

impl ValType {
    /// Decodes the binary-format byte.
    pub fn from_byte(b: u8) -> Option<ValType> {
        match b {
            0 => Some(ValType::I32),
            1 => Some(ValType::I64),
            2 => Some(ValType::F32),
            3 => Some(ValType::F64),
            _ => None,
        }
    }
}

/// A function type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

impl FuncType {
    pub fn new(
        params: impl IntoIterator<Item = ValType>,
        results: impl IntoIterator<Item = ValType>,
    ) -> Self {
        FuncType { params: params.into_iter().collect(), results: results.into_iter().collect() }
    }
}

/// Size of a memory page in bytes.
pub const PAGE_SIZE: u64 = 65536;

/// Resource limits chosen by the embedder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_memory_pages: u32,
    pub max_call_depth: u32,
    /// Register slots per fiber, all frames together.
    pub max_registers: u32,
    pub max_fibers: u32,
}

/// Defaults: 1024 pages (64 MiB), call depth 4096, 262144 registers per fiber, 256 fibers.
impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_memory_pages: 1024,
            max_call_depth: 4096,
            max_registers: 262_144,
            max_fibers: 256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let t = FuncType::new([ValType::I64, ValType::I64], []);
        assert_eq!(t.params.len(), 2);
        assert_eq!(ValType::from_byte(3), Some(ValType::F64));
        assert_eq!(ValType::from_byte(4), None);
        assert_eq!(Limits::default().max_memory_pages, 1024);
    }
}

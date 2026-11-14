//! Error types for loading, linking, building and spawning.

use crate::binary::DecodeError;
use crate::op::Op;
use crate::verify::VerifyError;
use std::fmt;

/// Failure to turn bytes or a builder into a verified module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Decode { offset: u64, error: DecodeError },
    /// `func` is in the module-function index space (imports first).
    Verify { func: Option<u32>, inst: Option<u32>, error: VerifyError },
    Link(LinkError),
    Build(BuildError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    MissingImport { namespace: String, name: String },
    ImportTypeMismatch { namespace: String, name: String },
    MemoryTooLarge { min_pages: u32, cap: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    ImportAfterFunction,
    UnboundLabel,
    LabelBoundTwice,
    UndefinedFunction(u32),
    WrongShape(Op),
    /// More than 65535 registers requested.
    TooManyRegisters,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    NoSuchExport,
    NotAFunction,
    ImportEntry,
    ArgCount { expected: u32, got: u32 },
    TooManyFibers,
    StackOverflow,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Decode { offset, error } => write!(f, "decode error at byte {offset}: {error}"),
            LoadError::Verify { func, inst, error } => {
                write!(f, "verification failed")?;
                if let Some(func) = func {
                    write!(f, " in function {func}")?;
                }
                if let Some(inst) = inst {
                    write!(f, " at instruction {inst}")?;
                }
                write!(f, ": {error}")
            }
            LoadError::Link(e) => write!(f, "link error: {e}"),
            LoadError::Build(e) => write!(f, "build error: {e}"),
        }
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::MissingImport { namespace, name } => write!(f, "missing import {namespace}.{name}"),
            LinkError::ImportTypeMismatch { namespace, name } => {
                write!(f, "import {namespace}.{name} has a different type than the host function")
            }
            LinkError::MemoryTooLarge { min_pages, cap } => {
                write!(f, "module needs {min_pages} memory pages, limit is {cap}")
            }
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::ImportAfterFunction => f.write_str("import declared after a function"),
            BuildError::UnboundLabel => f.write_str("label used but never bound"),
            BuildError::LabelBoundTwice => f.write_str("label bound twice"),
            BuildError::UndefinedFunction(i) => write!(f, "function {i} declared but not defined"),
            BuildError::WrongShape(op) => write!(f, "wrong builder method for {}", op.name()),
            BuildError::TooManyRegisters => f.write_str("too many registers (limit is 65535)"),
        }
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::NoSuchExport => f.write_str("no such export"),
            SpawnError::NotAFunction => f.write_str("export is not a function"),
            SpawnError::ImportEntry => f.write_str("cannot spawn an imported (host) function"),
            SpawnError::ArgCount { expected, got } => {
                write!(f, "expected {expected} arguments, got {got}")
            }
            SpawnError::TooManyFibers => f.write_str("too many fibers"),
            SpawnError::StackOverflow => f.write_str("register stack exhausted"),
        }
    }
}

impl std::error::Error for LoadError {}
impl std::error::Error for LinkError {}
impl std::error::Error for BuildError {}
impl std::error::Error for SpawnError {}

//! AST of an `.efa` module. The authoritative description of the syntax is
//! `crates/efescript/docs/efa-syntax.md`; every node here maps to a production there.
//!
//! Design rules:
//! - Every node carries a [`Span`] (1-based line and column, counted in chars; `len` in chars).
//! - Instructions are generic (`mnemonic` text plus operands). The parser only does syntax;
//!   the lowering decides what each operand means from the mnemonic and the operand position.
//! - Integer literals are `i128` so both the signed and the unsigned 64-bit range fit.
//!   Float literals keep their source text (underscores removed, sign included) so that the
//!   lowering can parse them directly as `f32` or `f64` without double rounding.
//! - Names that the parser cannot classify (register, label, function, global, const, data
//!   symbol) are all [`Operand::Name`]; the lowering resolves them by position.

/// A source range on ONE line. `line` and `column` are 1-based and count chars (a tab is one
/// column). `len` is in chars. `line == 0` means "no location".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub line: u32,
    pub column: u32,
    pub len: u32,
}

impl Span {
    pub const fn new(line: u32, column: u32, len: u32) -> Span {
        Span { line, column, len }
    }

    /// The "no location" span (`line == 0`).
    pub const fn none() -> Span {
        Span { line: 0, column: 0, len: 0 }
    }

    /// The span that starts at `self` and ends where `other` ends. When both are on the same
    /// line and `other` does not end before `self` starts, `len` covers both; otherwise the
    /// result is `self` unchanged.
    pub fn join(self, other: Span) -> Span {
        if self.line == other.line && self.line != 0 {
            let end = other.column.saturating_add(other.len);
            if end >= self.column {
                return Span { line: self.line, column: self.column, len: end - self.column };
            }
        }
        self
    }
}

/// A whole `.efa` file: its top-level items in source order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Module {
    pub items: Vec<Item>,
}

/// One top-level item.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Import(ImportDecl),
    Memory(MemoryDecl),
    Const(ConstDecl),
    Global(GlobalDecl),
    Table(TableDecl),
    Data(DataDecl),
    Func(FuncDecl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Import(x) => x.span,
            Item::Memory(x) => x.span,
            Item::Const(x) => x.span,
            Item::Global(x) => x.span,
            Item::Table(x) => x.span,
            Item::Data(x) => x.span,
            Item::Func(x) => x.span,
        }
    }
}

/// A name with its position. Declared names have no dots; an import's qualified name
/// (`std.println`) and the data symbol `NAME.len` are the dotted ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// Value types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    I32,
    I64,
    F32,
    F64,
}

impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::F32 => "f32",
            Ty::F64 => "f64",
        }
    }

    pub fn from_name(s: &str) -> Option<Ty> {
        match s {
            "i32" => Some(Ty::I32),
            "i64" => Some(Ty::I64),
            "f32" => Some(Ty::F32),
            "f64" => Some(Ty::F64),
            _ => None,
        }
    }
}

/// A type written in the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeRef {
    pub ty: Ty,
    pub span: Span,
}

/// `import NAMESPACE.NAME(type, ...) -> type` (or `-> (type, type)`).
#[derive(Clone, Debug, PartialEq)]
pub struct ImportDecl {
    /// Whole statement.
    pub span: Span,
    /// The dotted name exactly as written, e.g. `std.println`.
    pub qualified: Ident,
    pub params: Vec<TypeRef>,
    pub results: Vec<TypeRef>,
}

impl ImportDecl {
    /// Splits the qualified name at its LAST dot: `("std", "println")`. `None` without a dot.
    pub fn split(&self) -> Option<(&str, &str)> {
        self.qualified.name.rsplit_once('.')
    }
}

/// `memory MIN [MAX]`. The operands are integers or `const` names (checked by the lowering).
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryDecl {
    pub span: Span,
    pub min: Operand,
    pub max: Option<Operand>,
}

/// `const NAME = literal`.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    pub span: Span,
    pub name: Ident,
    pub value: Operand,
}

/// `[export] global [mut] NAME: type = value`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlobalDecl {
    pub span: Span,
    pub exported: bool,
    pub mutable: bool,
    pub name: Ident,
    pub ty: TypeRef,
    pub init: Operand,
}

/// `table [f1, null, f2]`.
#[derive(Clone, Debug, PartialEq)]
pub struct TableDecl {
    pub span: Span,
    pub entries: Vec<TableEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TableEntry {
    /// A function or import (`std.println`) name.
    Func(Ident),
    /// The word `null`: an unset entry.
    Null(Span),
}

/// The data directive of a [`DataDecl`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DataKind {
    Db,
    Dw,
    Dd,
    Dq,
    Resb,
}

impl DataKind {
    pub fn name(self) -> &'static str {
        match self {
            DataKind::Db => "db",
            DataKind::Dw => "dw",
            DataKind::Dd => "dd",
            DataKind::Dq => "dq",
            DataKind::Resb => "resb",
        }
    }

    pub fn from_name(s: &str) -> Option<DataKind> {
        match s {
            "db" => Some(DataKind::Db),
            "dw" => Some(DataKind::Dw),
            "dd" => Some(DataKind::Dd),
            "dq" => Some(DataKind::Dq),
            "resb" => Some(DataKind::Resb),
            _ => None,
        }
    }
}

/// `NAME db "text", 10, 0xff` / `NAME dw 1, 2` / `NAME resb 64`.
/// For `resb` the parser still stores the operands it found (the lowering demands exactly one).
#[derive(Clone, Debug, PartialEq)]
pub struct DataDecl {
    pub span: Span,
    pub name: Ident,
    pub kind: DataKind,
    pub kind_span: Span,
    pub values: Vec<Operand>,
}

/// `[export] fn NAME(params) -> results { body }`.
#[derive(Clone, Debug, PartialEq)]
pub struct FuncDecl {
    /// The header line (from `export` or `fn` to the opening brace).
    pub span: Span,
    pub exported: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub results: Vec<TypeRef>,
    pub body: Vec<Stmt>,
    /// The closing brace.
    pub end: Span,
}

/// `name: type`. A parameter written `rK: type` where K is its position is "unnamed" (the
/// lowering decides; the parser just stores the name).
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeRef,
}

/// One statement of a function body.
#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub span: Span,
    pub kind: StmtKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// `name:`
    Label(Ident),
    /// `reg a, b, c`
    Reg(Vec<Ident>),
    /// `nregs N`
    Nregs(Operand),
    /// An instruction.
    Inst(Inst),
}

/// A generic instruction: `mnemonic operand, operand, ...`.
#[derive(Clone, Debug, PartialEq)]
pub struct Inst {
    /// From the mnemonic to the end of the statement (the lowering uses `line`/`column` as the
    /// position of every instruction it emits for this statement).
    pub span: Span,
    pub mnemonic: String,
    pub mnemonic_span: Span,
    pub operands: Vec<Operand>,
}

/// An instruction operand.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    /// Any name: register, label, function, import (`std.println`), global, const, data
    /// symbol (`msg`, `msg.len`). Resolved by the lowering from the operand position.
    Name(Ident),
    Int(i128, Span),
    /// Source text of a float literal: `1.5`, `-0.25`, `1e9`, `inf`, `-inf`, `nan`, `nan:0x7fc00001`.
    /// Underscores are removed. The sign is part of the text.
    Float(String, Span),
    /// A character literal (an integer, its Unicode scalar value).
    Char(char, Span),
    /// A string literal (unescaped bytes); only meaningful in `db`.
    Str(Vec<u8>, Span),
    /// Call sugar: `f(a, b) -> x`, `f(a) -> (x, y)`, `f()`. Used by `call` and (with an index
    /// operand as callee) `call_indirect`.
    Call(Box<CallSugar>),
    /// A bracketed label list: `[a, b, c]` (for `br_table`).
    Labels(Vec<Ident>, Span),
    /// A parenthesised tuple `(a, b)` (for `ret`).
    Tuple(Vec<Operand>, Span),
    /// `(T, ...) -> R` or `(T, ...) -> (R, ...)`: a function signature (for `call_indirect`).
    Signature(Signature),
}

impl Operand {
    pub fn span(&self) -> Span {
        match self {
            Operand::Name(i) => i.span,
            Operand::Int(_, s)
            | Operand::Float(_, s)
            | Operand::Char(_, s)
            | Operand::Str(_, s)
            | Operand::Labels(_, s)
            | Operand::Tuple(_, s) => *s,
            Operand::Call(c) => c.span,
            Operand::Signature(sig) => sig.span,
        }
    }
}

/// `callee(args) -> results`.
#[derive(Clone, Debug, PartialEq)]
pub struct CallSugar {
    /// From the callee to the end of the results (or the closing parenthesis).
    pub span: Span,
    /// A name (function, import, or for `call_indirect` a register) or, for `call_indirect`, an
    /// integer/char/const index.
    pub callee: Operand,
    pub args: Vec<Operand>,
    /// Empty when there is no `->`. The entries are registers (the lowering checks).
    pub results: Vec<Operand>,
}

/// `(T, ...) -> R` / `(T, ...) -> (R, ...)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub span: Span,
    pub params: Vec<TypeRef>,
    pub results: Vec<TypeRef>,
}

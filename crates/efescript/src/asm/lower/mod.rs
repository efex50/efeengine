//! Lowering: AST to `efevm::ModuleBuilder` to a verified `efevm::Module`.
//!
//! The rules (name resolution, immediates, hidden temporaries, call windows, nregs, builder
//! call order, data layout, names and lines) are those of `docs/efa-syntax.md` sections 4 to 9.
//! This file does the module level (declarations, data, globals, memory, exports, names and
//! the final verification); `func.rs` lowers function bodies; `value.rs` types immediates.
//! It never panics; it collects as many diagnostics as it can.

use std::collections::HashMap;

use efevm::{FuncType, LoadError, ModuleBuilder, Op, ValType};

use super::ast::{
    self, DataDecl, DataKind, FuncDecl, GlobalDecl, Ident, ImportDecl, Item, Operand, Span,
    TableEntry, Ty,
};
use super::diag::{did_you_mean, Diagnostic};
use value::{raw_reg, type_val, Konst, Val};

mod func;
#[cfg(test)]
mod tests;
mod value;

/// Reserved words: they cannot be declared as names of anything.
const RESERVED: &[&str] = &[
    "import", "memory", "const", "global", "table", "fn", "export", "mut", "reg", "nregs", "null",
    "db", "dw", "dd", "dq", "resb", "i32", "i64", "f32", "f64", "inf", "nan",
];

/// First data address and alignment (efa-syntax.md section 8).
const DATA_START: u64 = 1024;
const PAGE: u64 = 65536;

type Res<T> = Result<T, Diagnostic>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum VKind {
    Const,
    Data,
    DataLen,
}

/// A `const`, a data symbol or a `NAME.len`.
struct ValueInfo {
    pub val: Val,
    pub span: Span,
    pub kind: VKind,
}

/// A callable: an import or a function.
struct Sig {
    pub index: u32,
    pub params: Vec<Ty>,
    pub results: Vec<Ty>,
    pub span: Span,
}

struct GlobalInfo {
    pub index: u32,
    pub ty: Ty,
    pub span: Span,
}

/// Module-level name tables shared by every function body.
struct Env<'a> {
    file: &'a str,
    lines: Vec<&'a str>,
    pub funcs: HashMap<String, Sig>,
    pub func_names: Vec<String>,
    pub globals: HashMap<String, GlobalInfo>,
    pub global_names: Vec<String>,
    pub values: HashMap<String, ValueInfo>,
    pub value_names: Vec<String>,
    pub ops: HashMap<&'static str, Op>,
}

fn idx(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn val_type(t: Ty) -> ValType {
    match t {
        Ty::I32 => ValType::I32,
        Ty::I64 => ValType::I64,
        Ty::F32 => ValType::F32,
        Ty::F64 => ValType::F64,
    }
}

fn func_type(params: &[ast::TypeRef], results: &[ast::TypeRef]) -> FuncType {
    FuncType::new(
        params.iter().map(|t| val_type(t.ty)),
        results.iter().map(|t| val_type(t.ty)),
    )
}

fn func_type_of(params: &[Ty], results: &[Ty]) -> FuncType {
    FuncType::new(params.iter().map(|t| val_type(*t)), results.iter().map(|t| val_type(*t)))
}

impl<'a> Env<'a> {
    fn new(file: &'a str, src: &'a str) -> Env<'a> {
        Env {
            file,
            lines: src.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect(),
            funcs: HashMap::new(),
            func_names: Vec::new(),
            globals: HashMap::new(),
            global_names: Vec::new(),
            values: HashMap::new(),
            value_names: Vec::new(),
            ops: Op::ALL.iter().map(|o| (o.name(), *o)).collect(),
        }
    }

    fn err(&self, span: Span, msg: impl Into<String>) -> Diagnostic {
        let text = if span.line == 0 {
            ""
        } else {
            self.lines.get((span.line as usize).saturating_sub(1)).copied().unwrap_or("")
        };
        Diagnostic::with_line(self.file, text, span, msg)
    }

    fn file(&self) -> &str {
        self.file
    }

    fn first_defined(&self, d: Diagnostic, first: Span) -> Diagnostic {
        if first.line == 0 {
            d
        } else {
            d.with_note(format!("first defined at line {}", first.line))
        }
    }

    /// Checks a declared name: no dots, not reserved, not `r<digits>`.
    fn check_name(&self, id: &Ident, what: &str, diags: &mut Vec<Diagnostic>) -> bool {
        let bad = if id.name.contains('.') {
            Some(format!("`{}` is not a valid {what} name: declared names have no dots", id.name))
        } else if RESERVED.contains(&id.name.as_str()) {
            Some(format!("`{}` is a reserved word and cannot be a {what} name", id.name))
        } else if raw_reg(&id.name).is_some() {
            Some(format!("`{}` is a raw register name and cannot be a {what} name", id.name))
        } else {
            None
        };
        match bad {
            Some(m) => {
                diags.push(self.err(id.span, m));
                false
            }
            None => true,
        }
    }

    fn define_value(
        &mut self,
        name: &str,
        span: Span,
        val: Val,
        kind: VKind,
        diags: &mut Vec<Diagnostic>,
    ) {
        if let Some(prev) = self.values.get(name) {
            let d = self.err(span, format!("duplicate definition of `{name}`"));
            diags.push(self.first_defined(d, prev.span));
            return;
        }
        self.value_names.push(name.to_string());
        self.values.insert(name.to_string(), ValueInfo { val, span, kind });
    }

    fn define_func(&mut self, name: &str, sig: Sig, diags: &mut Vec<Diagnostic>) {
        if let Some(prev) = self.funcs.get(name) {
            let d = self.err(sig.span, format!("duplicate definition of function `{name}`"));
            diags.push(self.first_defined(d, prev.span));
            return;
        }
        self.func_names.push(name.to_string());
        self.funcs.insert(name.to_string(), sig);
    }

    /// An integer literal, character or `const` name that holds an integer.
    fn const_int(&self, op: &Operand) -> Res<i128> {
        match op {
            Operand::Int(v, _) => Ok(*v),
            Operand::Char(c, _) => Ok(*c as i128),
            Operand::Name(id) => match self.values.get(&id.name) {
                Some(ValueInfo { val: Val::Int(v), kind: VKind::Const, .. }) => Ok(*v),
                Some(ValueInfo { kind: VKind::Const, .. }) => {
                    Err(self.err(id.span, format!("expected an integer, `{}` is a float", id.name)))
                }
                Some(_) => Err(self.err(
                    id.span,
                    format!("expected an integer or a `const`, `{}` is a data symbol", id.name),
                )),
                None => Err(self.unknown_value(id)),
            },
            other => Err(self.err(
                other.span(),
                format!("expected an integer or a `const`, found {}", value::describe(other)),
            )),
        }
    }

    fn unknown_value(&self, id: &Ident) -> Diagnostic {
        let d = self.err(id.span, format!("unknown constant `{}`", id.name));
        match did_you_mean(&id.name, self.value_names.iter()) {
            Some(s) => d.with_note(format!("did you mean `{s}`?")),
            None => d,
        }
    }

    fn unknown_function(&self, id: &Ident) -> Diagnostic {
        let d = self.err(id.span, format!("unknown function `{}`", id.name));
        match did_you_mean(&id.name, self.func_names.iter()) {
            Some(s) => d.with_note(format!("did you mean `{s}`?")),
            None => d,
        }
    }

    fn unknown_global(&self, id: &Ident) -> Diagnostic {
        let d = self.err(id.span, format!("unknown global `{}`", id.name));
        match did_you_mean(&id.name, self.global_names.iter()) {
            Some(s) => d.with_note(format!("did you mean `{s}`?")),
            None => d,
        }
    }

    /// An integer-like or float value of a data / global / table position.
    fn value_of(&self, op: &Operand) -> Res<Val> {
        match op {
            Operand::Int(v, _) => Ok(Val::Int(*v)),
            Operand::Char(c, _) => Ok(Val::Int(*c as i128)),
            Operand::Float(t, _) => Ok(Val::Float(t.clone())),
            Operand::Name(id) => match self.values.get(&id.name) {
                Some(v) => Ok(v.val.clone()),
                None => Err(self.unknown_value(id)),
            },
            other => Err(self.err(
                other.span(),
                format!("expected a literal or a constant, found {}", value::describe(other)),
            )),
        }
    }
}

fn align8(n: u64) -> u64 {
    n.saturating_add(7) & !7
}

struct DataPlan<'a> {
    decl: &'a DataDecl,
    addr: u64,
}

/// The size in bytes a data item occupies (depends only on the count and kind of its values).
fn data_size(env: &Env, d: &DataDecl, diags: &mut Vec<Diagnostic>) -> u64 {
    let n = d.values.len() as u64;
    match d.kind {
        DataKind::Resb => {
            let count = match d.values.as_slice() {
                [one] => env.const_int(one).and_then(|v| {
                    u64::try_from(v)
                        .ok()
                        .filter(|c| *c <= u64::from(u32::MAX))
                        .ok_or_else(|| {
                            env.err(one.span(), format!("`resb` size {v} is out of range"))
                        })
                }),
                _ => Err(env.err(
                    d.kind_span,
                    format!("`resb` takes exactly one size, found {}", d.values.len()),
                )),
            };
            match count {
                Ok(c) => c,
                Err(e) => {
                    diags.push(e);
                    0
                }
            }
        }
        _ => {
            if d.values.is_empty() {
                diags.push(env.err(
                    d.kind_span,
                    format!("`{}` needs at least one value", d.kind.name()),
                ));
            }
            match d.kind {
                DataKind::Db => d
                    .values
                    .iter()
                    .map(|v| match v {
                        Operand::Str(b, _) => b.len() as u64,
                        _ => 1,
                    })
                    .sum(),
                DataKind::Dw => 2 * n,
                DataKind::Dd => 4 * n,
                _ => 8 * n,
            }
        }
    }
}

/// The bytes of a non-`resb` data item.
fn data_bytes(env: &Env, d: &DataDecl, diags: &mut Vec<Diagnostic>) -> Vec<u8> {
    let mut out = Vec::new();
    for v in &d.values {
        if let Err(e) = data_value(env, d.kind, v, &mut out) {
            diags.push(e);
        }
    }
    out
}

fn data_value(env: &Env, kind: DataKind, op: &Operand, out: &mut Vec<u8>) -> Res<()> {
    if let Operand::Str(bytes, span) = op {
        if kind != DataKind::Db {
            return Err(env.err(*span, format!("a string is only valid in `db`, not `{}`", kind.name())));
        }
        out.extend_from_slice(bytes);
        return Ok(());
    }
    let val = env.value_of(op)?;
    let span = op.span();
    let ranged = |lo: i128, hi: i128, what: &str| -> Res<i128> {
        match &val {
            Val::Float(_) => Err(env.err(span, "float literal in an integer position")),
            Val::Int(n) if (lo..=hi).contains(n) => Ok(*n),
            Val::Int(n) => Err(env.err(span, format!("integer {n} does not fit in {what}"))),
        }
    };
    match kind {
        DataKind::Db => out.push(ranged(-128, 255, "a byte (-128..=255)")? as u8),
        DataKind::Dw => {
            out.extend_from_slice(&(ranged(-32768, 65535, "16 bits")? as u16).to_le_bytes())
        }
        DataKind::Dd | DataKind::Dq => {
            let wide = kind == DataKind::Dq;
            let ty = match (&val, wide) {
                (Val::Float(_), false) => Ty::F32,
                (Val::Float(_), true) => Ty::F64,
                (Val::Int(_), false) => Ty::I32,
                (Val::Int(_), true) => Ty::I64,
            };
            match type_val(ty, &val).map_err(|m| env.err(span, m))? {
                Konst::I32(v) => out.extend_from_slice(&(v as u32).to_le_bytes()),
                Konst::F32(f) => out.extend_from_slice(&f.to_bits().to_le_bytes()),
                Konst::I64(v) => out.extend_from_slice(&(v as u64).to_le_bytes()),
                Konst::F64(f) => out.extend_from_slice(&f.to_bits().to_le_bytes()),
            }
        }
        DataKind::Resb => {}
    }
    Ok(())
}

/// The initial value of a global of type `ty`.
fn global_init(env: &Env, g: &GlobalDecl) -> Res<Konst> {
    let val = env.value_of(&g.init)?;
    type_val(g.ty.ty, &val).map_err(|m| env.err(g.init.span(), m))
}

pub(super) fn lower(
    m: &ast::Module,
    src: &str,
    file: &str,
) -> Result<efevm::Module, Vec<Diagnostic>> {
    let mut env = Env::new(file, src);
    let mut diags: Vec<Diagnostic> = Vec::new();

    let mut consts = Vec::new();
    let mut imports: Vec<&ImportDecl> = Vec::new();
    let mut funcs: Vec<&FuncDecl> = Vec::new();
    let mut globals: Vec<&GlobalDecl> = Vec::new();
    let mut tables = Vec::new();
    let mut datas: Vec<&DataDecl> = Vec::new();
    let mut mems = Vec::new();
    for item in &m.items {
        match item {
            Item::Import(x) => imports.push(x),
            Item::Memory(x) => mems.push(x),
            Item::Const(x) => consts.push(x),
            Item::Global(x) => globals.push(x),
            Item::Table(x) => tables.push(x),
            Item::Data(x) => datas.push(x),
            Item::Func(x) => funcs.push(x),
        }
    }

    // Constants.
    for c in &consts {
        env.check_name(&c.name, "constant", &mut diags);
        let val = match &c.value {
            Operand::Int(v, _) => Some(Val::Int(*v)),
            Operand::Char(ch, _) => Some(Val::Int(*ch as i128)),
            Operand::Float(t, _) => Some(Val::Float(t.clone())),
            other => {
                diags.push(env.err(
                    other.span(),
                    format!(
                        "a `const` value must be one integer, float or character literal, found {}",
                        value::describe(other)
                    ),
                ));
                None
            }
        };
        if let Some(val) = val {
            env.define_value(&c.name.name, c.name.span, val, VKind::Const, &mut diags);
        }
    }

    // Imports, then functions: one index space.
    let mut import_list: Vec<(String, String, FuncType)> = Vec::new();
    for imp in &imports {
        let (ns, name) = match imp.split() {
            Some((a, b)) if !a.is_empty() && !b.is_empty() => (a.to_string(), b.to_string()),
            _ => {
                diags.push(env.err(
                    imp.qualified.span,
                    format!(
                        "import `{}` needs a namespace: write `namespace.name`",
                        imp.qualified.name
                    ),
                ));
                (String::new(), imp.qualified.name.clone())
            }
        };
        let sig = Sig {
            index: idx(import_list.len()),
            params: imp.params.iter().map(|t| t.ty).collect(),
            results: imp.results.iter().map(|t| t.ty).collect(),
            span: imp.qualified.span,
        };
        env.define_func(&imp.qualified.name, sig, &mut diags);
        import_list.push((ns, name, func_type(&imp.params, &imp.results)));
    }
    let nimports = import_list.len();
    for (k, f) in funcs.iter().enumerate() {
        env.check_name(&f.name, "function", &mut diags);
        let sig = Sig {
            index: idx(nimports + k),
            params: f.params.iter().map(|p| p.ty.ty).collect(),
            results: f.results.iter().map(|t| t.ty).collect(),
            span: f.name.span,
        };
        env.define_func(&f.name.name, sig, &mut diags);
    }

    // Data layout (a pre-pass: sizes depend only on counts and kinds).
    let mut plans: Vec<DataPlan> = Vec::new();
    let mut cursor = DATA_START;
    for d in &datas {
        env.check_name(&d.name, "data", &mut diags);
        let len = data_size(&env, d, &mut diags);
        let addr = align8(cursor);
        cursor = addr.saturating_add(len);
        let span = d.name.span;
        env.define_value(&d.name.name, span, Val::Int(i128::from(addr)), VKind::Data, &mut diags);
        let len_name = format!("{}.len", d.name.name);
        if !env.values.contains_key(&len_name) {
            env.value_names.push(len_name.clone());
            env.values.insert(
                len_name,
                ValueInfo { val: Val::Int(i128::from(len)), span, kind: VKind::DataLen },
            );
        }
        plans.push(DataPlan { decl: d, addr });
    }
    let data_end = if plans.is_empty() { 0 } else { cursor };

    // Globals.
    let mut global_plans: Vec<(&GlobalDecl, Konst)> = Vec::new();
    for g in &globals {
        env.check_name(&g.name, "global", &mut diags);
        let k = match global_init(&env, g) {
            Ok(k) => k,
            Err(e) => {
                diags.push(e);
                Konst::zero(g.ty.ty)
            }
        };
        if let Some(prev) = env.globals.get(&g.name.name) {
            let d = env.err(g.name.span, format!("duplicate definition of global `{}`", g.name.name));
            diags.push(env.first_defined(d, prev.span));
        } else {
            env.global_names.push(g.name.name.clone());
            env.globals.insert(
                g.name.name.clone(),
                GlobalInfo {
                    index: idx(global_plans.len()),
                    ty: g.ty.ty,
                    span: g.name.span,
                },
            );
        }
        global_plans.push((g, k));
    }

    // Table.
    let mut table: Option<Vec<Option<u32>>> = None;
    for (n, t) in tables.iter().enumerate() {
        if n > 0 {
            let d = env.err(t.span, "duplicate `table` declaration");
            let first = tables.first().map(|f| f.span).unwrap_or_default();
            diags.push(env.first_defined(d, first));
            continue;
        }
        let mut entries = Vec::new();
        for e in &t.entries {
            match e {
                TableEntry::Null(_) => entries.push(None),
                TableEntry::Func(id) => match env.funcs.get(&id.name) {
                    Some(s) => entries.push(Some(s.index)),
                    None => {
                        diags.push(env.unknown_function(id));
                        entries.push(None);
                    }
                },
            }
        }
        table = Some(entries);
    }

    // Memory.
    let mut memory: (u32, Option<u32>) = (1, None);
    if mems.is_empty() {
        match u32::try_from(data_end.div_ceil(PAGE).max(1)) {
            Ok(p) => memory = (p, None),
            Err(_) => diags.push(env.err(Span::none(), "the data does not fit in any memory")),
        }
    }
    for (n, mem) in mems.iter().enumerate() {
        if n > 0 {
            let d = env.err(mem.span, "duplicate `memory` declaration");
            let first = mems.first().map(|f| f.span).unwrap_or_default();
            diags.push(env.first_defined(d, first));
            continue;
        }
        let to_u32 = |op: &Operand| -> Res<u32> {
            let v = env.const_int(op)?;
            u32::try_from(v).map_err(|_| {
                env.err(op.span(), format!("memory size {v} must be a non-negative number that fits in u32"))
            })
        };
        let min = match to_u32(&mem.min) {
            Ok(v) => v,
            Err(e) => {
                diags.push(e);
                continue;
            }
        };
        let max = match &mem.max {
            Some(op) => match to_u32(op) {
                Ok(v) if v < min => {
                    diags.push(env.err(op.span(), format!("memory maximum {v} is below the minimum {min}")));
                    None
                }
                Ok(v) => Some(v),
                Err(e) => {
                    diags.push(e);
                    None
                }
            },
            None => None,
        };
        if data_end > u64::from(min) * PAGE {
            diags.push(env.err(
                mem.span,
                format!(
                    "the data needs {} bytes but `memory {}` provides only {}",
                    data_end,
                    min,
                    u64::from(min) * PAGE
                ),
            ));
        }
        memory = (min, max);
    }

    // Exports: unique across functions and globals.
    let mut export_names: HashMap<String, Span> = HashMap::new();
    let mut func_exports: Vec<(String, u32)> = Vec::new();
    let mut global_exports: Vec<(String, u32)> = Vec::new();
    let mut note_export = |name: &Ident, diags: &mut Vec<Diagnostic>| -> bool {
        if let Some(prev) = export_names.get(&name.name) {
            let d = env.err(name.span, format!("duplicate export name `{}`", name.name));
            diags.push(env.first_defined(d, *prev));
            return false;
        }
        export_names.insert(name.name.clone(), name.span);
        true
    };
    for (k, f) in funcs.iter().enumerate() {
        let first = env.funcs.get(&f.name.name).is_some_and(|s| s.span == f.name.span);
        if f.exported && first && note_export(&f.name, &mut diags) {
            func_exports.push((f.name.name.clone(), idx(nimports + k)));
        }
    }
    for (k, (g, _)) in global_plans.iter().enumerate() {
        let first = env.globals.get(&g.name.name).is_some_and(|s| s.span == g.name.span);
        if g.exported && first && note_export(&g.name, &mut diags) {
            global_exports.push((g.name.name.clone(), idx(k)));
        }
    }

    // ---- Builder, in the documented call order. ----
    let mut mb = ModuleBuilder::new();
    for (ns, name, ty) in &import_list {
        mb.import(ns, name, ty.clone());
    }
    for f in &funcs {
        let params: Vec<Ty> = f.params.iter().map(|p| p.ty.ty).collect();
        let results: Vec<Ty> = f.results.iter().map(|t| t.ty).collect();
        mb.declare_func(func_type_of(&params, &results));
    }
    for (g, k) in &global_plans {
        match k {
            Konst::I32(v) => mb.global_i32(g.mutable, *v),
            Konst::I64(v) => mb.global_i64(g.mutable, *v),
            Konst::F32(v) => mb.global_f32(g.mutable, *v),
            Konst::F64(v) => mb.global_f64(g.mutable, *v),
        };
    }
    if let Some(t) = table {
        mb.table(t);
    }
    mb.memory(memory.0, memory.1);
    for p in &plans {
        if p.decl.kind == DataKind::Resb {
            continue;
        }
        let bytes = data_bytes(&env, p.decl, &mut diags);
        if !bytes.is_empty() {
            mb.data(p.addr, &bytes);
        }
    }
    let file_id = mb.file(file);
    let mut span_tables: Vec<Vec<Span>> = Vec::new();
    let mut reg_names: Vec<Vec<(u16, String)>> = Vec::new();
    for (k, f) in funcs.iter().enumerate() {
        let out = func::lower_func(&env, &mut mb, file_id, f, &mut diags);
        mb.define(idx(nimports + k), out.body);
        span_tables.push(out.spans);
        reg_names.push(out.reg_names);
    }
    for (name, i) in &func_exports {
        mb.export_func(name, *i);
    }
    for (name, i) in &global_exports {
        mb.export_global(name, *i);
    }
    for (i, (ns, name, _)) in import_list.iter().enumerate() {
        mb.func_name(idx(i), &format!("{ns}.{name}"));
    }
    for (k, f) in funcs.iter().enumerate() {
        mb.func_name(idx(nimports + k), &f.name.name);
    }
    for (k, names) in reg_names.iter().enumerate() {
        for (r, name) in names {
            mb.reg_name(idx(nimports + k), *r, name);
        }
    }

    if !diags.is_empty() {
        return Err(diags);
    }
    match mb.finish() {
        Ok(module) => Ok(module),
        Err(e) => Err(vec![map_load_error(&env, &e, nimports, &funcs, &span_tables)]),
    }
}

/// Maps a load error back to the source: a verification error to the statement that produced
/// the instruction, everything else to a location-less diagnostic.
fn map_load_error(
    env: &Env,
    e: &LoadError,
    nimports: usize,
    funcs: &[&FuncDecl],
    spans: &[Vec<Span>],
) -> Diagnostic {
    if let LoadError::Verify { func: Some(f), inst, error } = e {
        let k = (*f as usize).checked_sub(nimports);
        let span = match (k, inst) {
            (Some(k), Some(i)) => spans.get(k).and_then(|t| t.get(*i as usize)).copied(),
            (Some(k), None) => funcs.get(k).map(|d| d.name.span),
            _ => None,
        };
        if let Some(span) = span {
            return env.err(span, error.to_string());
        }
    }
    Diagnostic::without_location(env.file(), e.to_string())
}

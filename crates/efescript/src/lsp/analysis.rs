//! AST and index queries behind definition, completion and hover.

use crate::asm::ast::*;
use efevm::api::ApiIndex;
use efevm::Op;

pub fn enclosing_func(m: &Module, line: u32) -> Option<&FuncDecl> {
    m.items.iter().find_map(|i| match i {
        Item::Func(f) if f.span.line <= line && line <= f.end.line => Some(f),
        _ => None,
    })
}

fn line_mnemonic(f: &FuncDecl, line: u32) -> Option<&str> {
    f.body.iter().find_map(|s| match &s.kind {
        StmtKind::Inst(i) if i.mnemonic_span.line == line => Some(i.mnemonic.as_str()),
        _ => None,
    })
}

fn find_label(f: &FuncDecl, word: &str) -> Option<Span> {
    f.body.iter().find_map(|s| match &s.kind {
        StmtKind::Label(i) if i.name == word => Some(i.span),
        _ => None,
    })
}

fn find_reg(f: &FuncDecl, word: &str) -> Option<Span> {
    f.params.iter().find(|p| p.name.name == word).map(|p| p.name.span).or_else(|| {
        f.body.iter().find_map(|s| match &s.kind {
            StmtKind::Reg(v) => v.iter().find(|i| i.name == word).map(|i| i.span),
            _ => None,
        })
    })
}

/// The declaration span of `word` used on `line`.
pub fn definition(m: &Module, line: u32, word: &str) -> Option<Span> {
    if let Some(f) = enclosing_func(m, line) {
        let label_first = line_mnemonic(f, line).is_some_and(|mn| mn.starts_with("jmp") || mn.starts_with("br"));
        let found = if label_first {
            find_label(f, word).or_else(|| find_reg(f, word))
        } else {
            find_reg(f, word).or_else(|| find_label(f, word))
        };
        if found.is_some() {
            return found;
        }
    }
    m.items.iter().find_map(|i| match i {
        Item::Func(f) if f.name.name == word => Some(f.name.span),
        Item::Import(d) if d.qualified.name == word => Some(d.qualified.span),
        Item::Const(c) if c.name.name == word => Some(c.name.span),
        Item::Global(g) if g.name.name == word => Some(g.name.span),
        Item::Data(d) if d.name.name == word || word.strip_suffix(".len") == Some(d.name.name.as_str()) => {
            Some(d.name.span)
        }
        _ => None,
    })
}

/// A completion candidate from the module: (label, kind number, detail).
pub fn local_symbols(m: &Module, line: u32) -> Vec<(String, u32, String)> {
    let mut v = Vec::new();
    if let Some(f) = enclosing_func(m, line) {
        for p in &f.params {
            v.push((p.name.name.clone(), 6, format!("parameter: {}", p.ty.ty.name())));
        }
        for s in &f.body {
            match &s.kind {
                StmtKind::Label(i) => v.push((i.name.clone(), 18, "label".into())),
                StmtKind::Reg(r) => v.extend(r.iter().map(|i| (i.name.clone(), 6, "register".to_string()))),
                _ => {}
            }
        }
    }
    for i in &m.items {
        match i {
            Item::Func(f) => v.push((f.name.name.clone(), 3, "function".into())),
            Item::Import(d) => v.push((d.qualified.name.clone(), 3, "import".into())),
            Item::Const(c) => v.push((c.name.name.clone(), 21, "const".into())),
            Item::Global(g) => v.push((g.name.name.clone(), 6, format!("global: {}", g.ty.ty.name()))),
            Item::Data(d) => {
                v.push((d.name.name.clone(), 6, format!("data ({})", d.kind.name())));
                v.push((format!("{}.len", d.name.name), 21, "length of data".into()));
            }
            _ => {}
        }
    }
    v
}

/// Struct constants of the index: (label, detail, documentation).
pub fn struct_constants(ix: &ApiIndex) -> Vec<(String, String, String)> {
    let mut v = Vec::new();
    for t in &ix.types {
        v.push((format!("{}.size", t.name), format!("size of {} in bytes: {}", t.name, t.size), t.doc.clone()));
        v.push((format!("{}.align", t.name), format!("alignment of {}: {}", t.name, t.align), String::new()));
        if let Some(id) = t.component {
            v.push((format!("{}.id", t.name), format!("component id of {}: {}", t.name, id), String::new()));
        }
        for f in &t.fields {
            v.push((
                format!("{}.{}", t.name, f.name),
                format!("offset of {}.{} ({}): {}", t.name, f.name, f.ty.name(), f.offset),
                f.doc.clone(),
            ));
        }
    }
    v
}

/// Markdown hover text for `word`.
pub fn hover_text(ix: Option<&ApiIndex>, word: &str) -> Option<String> {
    if let Some(ix) = ix {
        if let Some((ns, name)) = word.rsplit_once('.') {
            if let Some(f) = ix.function(ns, name) {
                let ver = ix.namespace(ns).map(|n| format!(", namespace `{}` version {}", n.name, n.version)).unwrap_or_default();
                return Some(format!(
                    "```\n{}{}\n```\n\n{}\n\nowner: `{}`{}",
                    f.qualified(),
                    f.signature_text(),
                    f.doc,
                    f.owner,
                    ver
                ));
            }
            if let Some(t) = ix.struct_type(ns) {
                let doc = |d: &str| if d.is_empty() { String::new() } else { format!("\n\n{d}") };
                match name {
                    "size" => return Some(format!("```\n{ns}.size = {}\n```\n\nsize of `{ns}` in bytes{}", t.size, doc(&t.doc))),
                    "align" => return Some(format!("```\n{ns}.align = {}\n```\n\nalignment of `{ns}`", t.align)),
                    "id" if t.component.is_some() => {
                        return Some(format!("```\n{ns}.id = {}\n```\n\ncomponent id of `{ns}`", t.component.unwrap_or(0)))
                    }
                    _ => {}
                }
                if let Some(f) = t.field_named(name) {
                    let count = if f.count > 1 { format!(" x{}", f.count) } else { String::new() };
                    return Some(format!(
                        "```\n{ns}.{name} = {}\n```\n\nfield `{name}: {}{count}` of `{ns}` at offset {}, size {}{}",
                        f.offset,
                        f.ty.name(),
                        f.offset,
                        f.size(),
                        doc(&f.doc)
                    ));
                }
            }
        }
    }
    let op = Op::ALL.iter().find(|o| o.name() == word)?;
    Some(format!("```\n{} {}\n```\n\ninstruction, opcode 0x{:03X}", op.name(), op.shape().operands(), op.code()))
}

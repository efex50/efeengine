use std::collections::HashMap;

use crate::compiler::ast::lexer::LineColumn;
use crate::ops::OpCodes;

use super::nasm_funs::{static_bytes, static_fixed};
use super::{AsmInner, AsmInstruction, AsmItem, AsmOperand, Imm, ParseErr, PreCompile};

/// Jump and call instructions: their operand is a label (or a register).
pub(crate) fn is_jump(op: &OpCodes) -> bool {
    matches!(
        op,
        OpCodes::Jmp
            | OpCodes::Je
            | OpCodes::Jne
            | OpCodes::Jgt
            | OpCodes::Jlt
            | OpCodes::Jle
            | OpCodes::Jge
            | OpCodes::Jz
            | OpCodes::Jnz
            | OpCodes::Js
            | OpCodes::Call
    )
}

#[derive(Debug)]
enum SymKind {
    /// address inside the code
    Code(usize),
    /// offset inside the data block and its length
    Data { off: usize, len: usize },
}

type Resolver<'a> = &'a dyn Fn(&str, bool, &LineColumn) -> Result<u32, ParseErr>;

fn emit_imm(imm: &Imm, resolve: Resolver, out: &mut Vec<u8>) -> Result<(), ParseErr> {
    match imm {
        Imm::Num(n) => out.extend(static_bytes(*n)),
        Imm::Sym { name, len, pos } => out.extend(static_fixed(resolve(name, *len, pos)?)),
    }
    Ok(())
}

fn emit_item(item: &AsmItem, resolve: Resolver, out: &mut Vec<u8>) -> Result<(), ParseErr> {
    match item {
        AsmItem::Reg(r) => out.push(r.to_operand() as u8),
        AsmItem::Imm(i) => emit_imm(i, resolve, out)?,
    }
    Ok(())
}

fn emit_operand(op: &AsmOperand, resolve: Resolver, out: &mut Vec<u8>) -> Result<(), ParseErr> {
    match op {
        AsmOperand::Null => {}
        AsmOperand::Reg(r) => out.push(r.to_operand() as u8),
        AsmOperand::Imm(i) => emit_imm(i, resolve, out)?,
        AsmOperand::Ptr(tag, inner) => {
            out.push(tag.clone() as u8);
            match inner {
                AsmInner::One(a) => emit_item(a, resolve, out)?,
                AsmInner::Sum(a, b) | AsmInner::Sub(a, b) => {
                    emit_item(a, resolve, out)?;
                    out.push(if matches!(inner, AsmInner::Sum(..)) { b'+' } else { b'-' });
                    emit_item(b, resolve, out)?;
                }
            }
            out.push(tag.clone() as u8);
        }
    }
    Ok(())
}

fn emit_instruction(i: &AsmInstruction, resolve: Resolver) -> Result<Vec<u8>, ParseErr> {
    let mut out = vec![i.opcode.clone() as u8];
    emit_operand(&i.left, resolve, &mut out)?;
    emit_operand(&i.right, resolve, &mut out)?;
    Ok(out)
}

/// The assembled program: after the sizing pass every symbol address is known.
#[derive(Debug)]
pub(crate) struct Data {
    code: Vec<AsmInstruction>,
    symbols: HashMap<String, SymKind>,
    code_len: usize,
    data: Vec<u8>,
    has_data: bool,
}

impl Data {
    /// Sizing pass: instruction sizes do not depend on symbol values, so one walk
    /// gives the address of every label and the offset of every data definition.
    pub(crate) fn from_pre_compile(a: Vec<PreCompile>) -> Result<Self, ParseErr> {
        let mut code = Vec::new();
        let mut symbols: HashMap<String, SymKind> = HashMap::new();
        let mut cur = 0usize;
        let mut data = Vec::new();
        let mut has_data = false;

        let dummy = |_: &str, _: bool, _: &LineColumn| Ok(0u32);
        for x in a {
            match x {
                PreCompile::I(inst) => {
                    cur += emit_instruction(&inst, &dummy)?.len();
                    code.push(inst);
                }
                PreCompile::L(name, pos) => {
                    if symbols.contains_key(&name) {
                        return Err(ParseErr::DuplicateSymbol(name, pos));
                    }
                    symbols.insert(name, SymKind::Code(cur));
                }
                PreCompile::D(def) => {
                    if symbols.contains_key(&def.name) {
                        return Err(ParseErr::DuplicateSymbol(def.name, def.pos));
                    }
                    symbols.insert(
                        def.name,
                        SymKind::Data { off: data.len(), len: def.bytes.len() },
                    );
                    data.extend(def.bytes);
                    has_data = true;
                }
            }
        }

        Ok(Self { code, symbols, code_len: cur, data, has_data })
    }

    fn resolve(&self, name: &str, len: bool, pos: &LineColumn) -> Result<u32, ParseErr> {
        let value = match self.symbols.get(name) {
            None => return Err(ParseErr::UndefinedSymbol(name.to_string(), pos.clone())),
            Some(SymKind::Code(addr)) => {
                if len {
                    return Err(ParseErr::UndefinedSymbol(format!("{name}.len"), pos.clone()));
                }
                *addr
            }
            Some(SymKind::Data { off, len: l }) => {
                if len {
                    *l
                } else {
                    self.code_len + 1 + off
                }
            }
        };
        u32::try_from(value).map_err(|_| ParseErr::ProgramTooLarge)
    }

    /// Emit pass.
    pub(crate) fn to_program(&self) -> Result<Vec<u8>, ParseErr> {
        let resolver = |n: &str, l: bool, p: &LineColumn| self.resolve(n, l, p);
        let mut p = Vec::with_capacity(self.code_len + 1 + self.data.len());
        for i in &self.code {
            p.extend(emit_instruction(i, &resolver)?);
        }
        debug_assert_eq!(p.len(), self.code_len);
        if self.has_data {
            p.push(0);
            p.extend_from_slice(&self.data);
        }
        Ok(p)
    }
}

use num_traits::FromPrimitive;

use crate::instruction::{Operands, PtrInner, Registers};
use crate::nasm_efe::OperandType;
use crate::runtime::SimpleOperands;
use crate::runtime::parse_register_type_to_op;
use efepages::page::Page;

pub enum NumToStr{
    I(isize),
    U(usize)
}

/// Largest single memory access a syscall or the decoder will perform (guards against huge allocations).
pub(crate) const MAX_ACCESS: usize = 1 << 28;

/// Bounds-checked read: never panics.
pub(crate) fn mem_read(page: &Page, addr: usize, len: usize) -> Result<Vec<u8>, String> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if len > MAX_ACCESS {
        return Err(format!("memory read of {len} bytes is too large"));
    }
    if addr.checked_add(len).is_none() {
        return Err(format!("memory read out of range at {addr:#x}"));
    }
    Ok(page.read(addr, len))
}

/// Bounds-checked write: never panics.
pub(crate) fn mem_write(page: &mut Page, addr: usize, data: Vec<u8>) -> Result<(), String> {
    if data.is_empty() {
        return Ok(());
    }
    if data.len() > MAX_ACCESS {
        return Err(format!("memory write of {} bytes is too large", data.len()));
    }
    match addr.checked_add(data.len()) {
        // the last page of the address space cannot be addressed by the page store
        Some(end) if end < usize::MAX - (1 << 20) => {}
        _ => return Err(format!("memory write out of range at {addr:#x}")),
    }
    page.write(addr, data);
    Ok(())
}

pub(crate) fn read_u8(page: &Page, pos: &mut usize) -> Result<u8, String> {
    let b = mem_read(page, *pos, 1)?[0];
    *pos = pos.wrapping_add(1);
    Ok(b)
}

/// Static operand body: `len` then `len` big-endian bytes (len 0..=8). `pos` is after the tag.
pub(crate) fn decode_static(page: &Page, pos: &mut usize) -> Result<usize, String> {
    let len = read_u8(page, pos)? as usize;
    if len > 8 {
        return Err(format!("static operand of {len} bytes (max 8) at {:#x}", pos.wrapping_sub(1)));
    }
    let bytes = mem_read(page, *pos, len)?;
    *pos = pos.wrapping_add(len);
    let mut v: u64 = 0;
    for b in bytes {
        v = (v << 8) | b as u64;
    }
    Ok(v as usize)
}

enum Item {
    Reg(Registers),
    Static(usize),
}

fn decode_item(page: &Page, pos: &mut usize) -> Result<Item, String> {
    let at = *pos;
    let tag = read_u8(page, pos)?;
    let ty = OperandType::from_u8(tag)
        .ok_or_else(|| format!("bad operand byte {tag:#04x} in pointer at {at:#x}"))?;
    match SimpleOperands::from_operand(ty.clone()) {
        SimpleOperands::Reg => Ok(Item::Reg(Registers::from_operand(ty))),
        SimpleOperands::Static => Ok(Item::Static(decode_static(page, pos)?)),
        _ => Err(format!("bad operand byte {tag:#04x} in pointer at {at:#x}")),
    }
}

fn is_close(open: &OperandType, tag: u8) -> bool {
    let o = open.clone() as u8;
    tag == o || (*open == OperandType::Pointer && tag == OperandType::BYTEPTR as u8)
}

/// Decodes a pointer operand whose opening tag byte (`open`) was already consumed.
/// `pos` ends after the closing tag.
pub(crate) fn decode_pointer(page: &Page, pos: &mut usize, open: OperandType) -> Result<Operands, String> {
    let first = decode_item(page, pos)?;
    let at = *pos;
    let next = read_u8(page, pos)?;
    let inner = if is_close(&open, next) {
        match first {
            Item::Reg(r) => PtrInner::Reg(r),
            Item::Static(u) => PtrInner::Static(u),
        }
    } else {
        let sum = match next {
            b'+' => true,
            b'-' => false,
            _ => return Err(format!("bad pointer operator byte {next:#04x} at {at:#x}")),
        };
        let second = decode_item(page, pos)?;
        let at = *pos;
        let close = read_u8(page, pos)?;
        if !is_close(&open, close) {
            return Err(format!("pointer is not closed (byte {close:#04x}) at {at:#x}"));
        }
        match (first, second, sum) {
            (Item::Reg(a), Item::Reg(b), true)  => PtrInner::SumReg(a, b),
            (Item::Reg(a), Item::Reg(b), false) => PtrInner::ExtReg(a, b),
            (Item::Reg(a), Item::Static(u), true)  => PtrInner::Sum(a, u),
            (Item::Reg(a), Item::Static(u), false) => PtrInner::Ext(a, u),
            (Item::Static(u), Item::Reg(a), true)  => PtrInner::Sum(a, u),
            (Item::Static(u), Item::Reg(a), false) => PtrInner::Extr(u, a),
            (Item::Static(a), Item::Static(b), true)  => PtrInner::Static(a.wrapping_add(b)),
            (Item::Static(a), Item::Static(b), false) => PtrInner::Static(a.wrapping_sub(b)),
        }
    };
    Ok(match open {
        OperandType::BYTEPTR => Operands::BYTEPTR(inner),
        OperandType::WORDPTR => Operands::WORDPTR(inner),
        OperandType::DWORDPTR => Operands::DWORDPTR(inner),
        OperandType::QWORDPTR => Operands::QWORDPTR(inner),
        _ => Operands::Pointer(inner),
    })
}

/// Decodes one operand starting at `pos`; `pos` ends after it.
pub(crate) fn decode_operand(page: &Page, pos: &mut usize) -> Result<Operands, String> {
    let at = *pos;
    let tag = read_u8(page, pos)?;
    let ty = OperandType::from_u8(tag)
        .ok_or_else(|| format!("bad operand byte {tag:#04x} at {at:#x}"))?;
    match SimpleOperands::from_operand(ty.clone()) {
        SimpleOperands::Reg => parse_register_type_to_op(ty)
            .ok_or_else(|| format!("bad operand byte {tag:#04x} at {at:#x}")),
        SimpleOperands::Static => Ok(Operands::Static(decode_static(page, pos)?)),
        SimpleOperands::Ptr => decode_pointer(page, pos, ty),
        SimpleOperands::Nop => Ok(Operands::Null),
        SimpleOperands::String | SimpleOperands::PushPop => {
            Err(format!("unsupported operand byte {tag:#04x} at {at:#x}"))
        }
    }
}

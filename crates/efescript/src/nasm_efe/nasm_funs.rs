use crate::{
    compiler::ast::lexer::{LexerTokens, LineColumn},
    funs::trim_zeroes,
    instruction::{Operands, PtrInner, Registers},
    ops::OpCodes,
};

use super::{AsmInner, AsmItem, Imm, OperandType, ParseErr};


/// Static operand with the usual trimmed encoding: `00 len bytes..` (0 = `00 00`).
pub(crate) fn static_bytes(v: u64) -> Vec<u8> {
    let l = trim_zeroes(v.to_be_bytes().to_vec());
    let mut out = vec![OperandType::Static as u8, l.len() as u8];
    out.extend(l);
    out
}

/// Static operand for a symbol: fixed 4 byte big-endian value, `00 04 b3 b2 b1 b0`.
pub(crate) fn static_fixed(v: u32) -> Vec<u8> {
    let mut out = vec![OperandType::Static as u8, 4];
    out.extend_from_slice(&v.to_be_bytes());
    out
}

fn ptr_inner_bytes(inner: &PtrInner) -> Vec<u8> {
    let reg = |r: &Registers| vec![r.to_operand() as u8];
    let num = |n: &usize| static_bytes(*n as u64);
    let mut out = Vec::new();
    match inner {
        PtrInner::Static(s) => out.extend(num(s)),
        PtrInner::Reg(r) => out.extend(reg(r)),
        PtrInner::Sum(r, s) => {
            out.extend(reg(r));
            out.push(b'+');
            out.extend(num(s));
        }
        PtrInner::Ext(r, s) => {
            out.extend(reg(r));
            out.push(b'-');
            out.extend(num(s));
        }
        PtrInner::Extr(s, r) => {
            out.extend(num(s));
            out.push(b'-');
            out.extend(reg(r));
        }
        PtrInner::SumReg(a, b) => {
            out.extend(reg(a));
            out.push(b'+');
            out.extend(reg(b));
        }
        PtrInner::ExtReg(a, b) => {
            out.extend(reg(a));
            out.push(b'-');
            out.extend(reg(b));
        }
    }
    out
}

fn no_symbols(pos: &LineColumn) -> ParseErr {
    ParseErr::WrongPtrInner("symbols are not allowed here".to_string(), pos.clone())
}

/// register or plain number; symbols are an error (`PtrInner` cannot hold them)
fn plain_item(i: AsmItem) -> Result<Result<Registers, u64>, ParseErr> {
    match i {
        AsmItem::Reg(r) => Ok(Ok(r)),
        AsmItem::Imm(Imm::Num(n)) => Ok(Err(n)),
        AsmItem::Imm(Imm::Sym { pos, .. }) => Err(no_symbols(&pos)),
    }
}

impl PtrInner {
    /// Builds a pointer inner from the tokens between `[` and `]`.
    pub fn from_tokens(t: Vec<LexerTokens>) -> Result<Self, ParseErr> {
        let inner = super::parse_ptr_inner_tokens(t)?;
        Ok(match inner {
            AsmInner::One(a) => match plain_item(a)? {
                Ok(r) => PtrInner::Reg(r),
                Err(n) => PtrInner::Static(n as usize),
            },
            AsmInner::Sum(a, b) => match (plain_item(a)?, plain_item(b)?) {
                (Ok(r1), Ok(r2)) => PtrInner::SumReg(r1, r2),
                (Ok(r), Err(n)) | (Err(n), Ok(r)) => PtrInner::Sum(r, n as usize),
                (Err(n1), Err(n2)) => PtrInner::Static(n1.wrapping_add(n2) as usize),
            },
            AsmInner::Sub(a, b) => match (plain_item(a)?, plain_item(b)?) {
                (Ok(r1), Ok(r2)) => PtrInner::ExtReg(r1, r2),
                (Ok(r), Err(n)) => PtrInner::Ext(r, n as usize),
                (Err(n), Ok(r)) => PtrInner::Extr(n as usize, r),
                (Err(n1), Err(n2)) => PtrInner::Static(n1.wrapping_sub(n2) as usize),
            },
        })
    }
}

impl Registers {
    pub fn from_str<S:Into<String>>(data:S) -> Option<Self>{
        let d:String = data.into();
        let d = d.to_lowercase();
        match d.as_str() {
            "ra"  => Some(Self::RA),
            "rb"  => Some(Self::RB),
            "rc"  => Some(Self::RC),
            "rd"  => Some(Self::RD),
            "rsp" => Some(Self::RSP),
            "rbp" => Some(Self::RBP),
            "r1"  => Some(Self::R1),
            "r2"  => Some(Self::R2),
            "r3"  => Some(Self::R3),
            "r4"  => Some(Self::R4),
            "r5"  => Some(Self::R5),
            "r6"  => Some(Self::R6),
            "al"  => Some(Self::AL),
            "ah"  => Some(Self::AH),
            "bl"  => Some(Self::BL),
            "bh"  => Some(Self::BH),
            "cl"  => Some(Self::CL),
            "ch"  => Some(Self::CH),
            "dl"  => Some(Self::DL),
            "dh"  => Some(Self::DH),

            _ => None
        }
    
    }
    pub fn to_operand(&self) -> OperandType{
        match self {
            Registers::RA  => OperandType::RA,
            Registers::RB  => OperandType::RB,
            Registers::RC  => OperandType::RC,
            Registers::RD  => OperandType::RD,
            Registers::RBP => OperandType::RBP,
            Registers::RSP => OperandType::RSP,
            Registers::R1  => OperandType::R1,
            Registers::R2  => OperandType::R2,
            Registers::R3  => OperandType::R3,
            Registers::R4  => OperandType::R4,
            Registers::R5  => OperandType::R5,
            Registers::R6  => OperandType::R6,
            Registers::AL  => OperandType::AL,
            Registers::AH  => OperandType::AH,
            Registers::BL  => OperandType::BL,
            Registers::BH  => OperandType::BH,
            Registers::CL  => OperandType::CL,
            Registers::CH  => OperandType::CH,
            Registers::DL  => OperandType::DL,
            Registers::DH  => OperandType::DH,
        }
    }
    pub fn from_operand(o:OperandType) -> Self{
        match o {
            OperandType::RA => Self::RA,
            OperandType::RB => Self::RB,
            OperandType::RC => Self::RC,
            OperandType::RD => Self::RD,
            OperandType::RBP => Self::RBP,
            OperandType::RSP => Self::RSP,
            OperandType::R1 => Self::R1,
            OperandType::R2 => Self::R2,
            OperandType::R3 => Self::R3,
            OperandType::R4 => Self::R4,
            OperandType::R5 => Self::R5,
            OperandType::R6 => Self::R6,
            OperandType::AL => Self::AL,
            OperandType::AH => Self::AH,
            OperandType::BL => Self::BL,
            OperandType::BH => Self::BH,
            OperandType::CL => Self::CL,
            OperandType::CH => Self::CH,
            OperandType::DL => Self::DL,
            OperandType::DH => Self::DH,
            _=> todo!(),
        }
    }
}

pub(crate) fn parse_register_type_to_op(a:OperandType) -> Option<Operands>{
    match a {
        OperandType::RA => Some(Operands::RA),
        OperandType::RB => Some(Operands::RB),
        OperandType::RC => Some(Operands::RC),
        OperandType::RD => Some(Operands::RD),
        OperandType::RBP => Some(Operands::RBP),
        OperandType::RSP => Some(Operands::RSP),
        OperandType::R1 => Some(Operands::R1),
        OperandType::R2 => Some(Operands::R2),
        OperandType::R3 => Some(Operands::R3),
        OperandType::R4 => Some(Operands::R4),
        OperandType::R5 => Some(Operands::R5),
        OperandType::R6 => Some(Operands::R6),
        
        OperandType::AL => Some(Operands::AL),
        OperandType::AH => Some(Operands::AH),
        OperandType::BL => Some(Operands::BL),
        OperandType::BH => Some(Operands::BH),
        OperandType::CL => Some(Operands::CL),
        OperandType::CH => Some(Operands::CH),
        OperandType::DL => Some(Operands::DL),
        OperandType::DH => Some(Operands::DH),
        
        _ => None
    }
}


impl Operands{
    pub fn to_int(&self) -> Vec<u8>{
        let typed = |tag: OperandType, inner: &PtrInner| {
            // pointers end with the same operand code they start with
            let mut v = vec![tag.clone() as u8];
            v.extend(ptr_inner_bytes(inner));
            v.push(tag as u8);
            v
        };
        let reg = |t: OperandType| vec![t as u8];
        match self {
            Operands::Static(l) => static_bytes(*l as u64),
            Operands::RA => reg(OperandType::RA),
            Operands::RB => reg(OperandType::RB),
            Operands::RC => reg(OperandType::RC),
            Operands::RD => reg(OperandType::RD),
            Operands::RBP => reg(OperandType::RBP),
            Operands::RSP => reg(OperandType::RSP),
            Operands::R1 => reg(OperandType::R1),
            Operands::R2 => reg(OperandType::R2),
            Operands::R3 => reg(OperandType::R3),
            Operands::R4 => reg(OperandType::R4),
            Operands::R5 => reg(OperandType::R5),
            Operands::R6 => reg(OperandType::R6),
            Operands::AL => reg(OperandType::AL),
            Operands::AH => reg(OperandType::AH),
            Operands::BL => reg(OperandType::BL),
            Operands::BH => reg(OperandType::BH),
            Operands::CL => reg(OperandType::CL),
            Operands::CH => reg(OperandType::CH),
            Operands::DL => reg(OperandType::DL),
            Operands::DH => reg(OperandType::DH),
            Operands::Pointer(a) => typed(OperandType::Pointer, a),
            Operands::BYTEPTR(a) => typed(OperandType::BYTEPTR, a),
            Operands::WORDPTR(a) => typed(OperandType::WORDPTR, a),
            Operands::DWORDPTR(a) => typed(OperandType::DWORDPTR, a),
            Operands::QWORDPTR(a) => typed(OperandType::QWORDPTR, a),
            Operands::Null => reg(OperandType::NULL),
            Operands::Label(_) => reg(OperandType::NULL),
        }
    }
}


pub fn parse_operand_type(a:OperandType) -> Option<Operands>{
    match a {
        OperandType::RA => Some(Operands::RA),
        OperandType::RB => Some(Operands::RB),
        OperandType::RC => Some(Operands::RC),
        OperandType::RD => Some(Operands::RD),
        OperandType::RBP => Some(Operands::RBP),
        OperandType::RSP => Some(Operands::RSP),
        OperandType::R1 => Some(Operands::R1),
        OperandType::R2 => Some(Operands::R2),
        OperandType::R3 => Some(Operands::R3),
        OperandType::R4 => Some(Operands::R4),
        OperandType::R5 => Some(Operands::R5),
        OperandType::R6 => Some(Operands::R6),
        OperandType::AL => Some(Operands::AL),
        OperandType::AH => Some(Operands::AH),
        OperandType::BL => Some(Operands::BL),
        OperandType::BH => Some(Operands::BH),
        OperandType::CL => Some(Operands::CL),
        OperandType::CH => Some(Operands::CH),
        OperandType::DL => Some(Operands::DL),
        OperandType::DH => Some(Operands::DH),
        
        _ => None
    }
}



pub fn parse_opcode<S:Into<String>>(a:S) -> Option<OpCodes>{
    let a:String = a.into();
    match a.as_str() {
        "mov" => Some(OpCodes::Mov),
        "cmp" => Some(OpCodes::Cmp),
        "test" => Some(OpCodes::Test),
        "add8" => Some(OpCodes::Add8),
        "add16" => Some(OpCodes::Add16),
        "add32" => Some(OpCodes::Add32),
        "add64" => Some(OpCodes::Add64),
        "sub8" => Some(OpCodes::Sub8),
        "sub16" => Some(OpCodes::Sub16),
        "sub32" => Some(OpCodes::Sub32),
        "sub64" => Some(OpCodes::Sub64),
        "push8" => Some(OpCodes::Push8),
        "push16" => Some(OpCodes::Push16),
        "push32" => Some(OpCodes::Push32),
        "push64" => Some(OpCodes::Push64),
        "pop8" => Some(OpCodes::Pop8),
        "pop16" => Some(OpCodes::Pop16),
        "pop32" => Some(OpCodes::Pop32),
        "pop64" => Some(OpCodes::Pop64),
        "syscall" => Some(OpCodes::SysCall),
        "call" => Some(OpCodes::Call),
        "ret" => Some(OpCodes::Ret),
        "jmp" => Some(OpCodes::Jmp),
        "jz" => Some(OpCodes::Jz),
        "jnz" => Some(OpCodes::Jnz),
        "je" => Some(OpCodes::Je),
        "jne" => Some(OpCodes::Jne),
        "jgt" => Some(OpCodes::Jgt),
        "jlt" => Some(OpCodes::Jlt),
        "jge" => Some(OpCodes::Jge),
        "jle" => Some(OpCodes::Jle),
        "js" => Some(OpCodes::Js),
        "lea" => Some(OpCodes::Lea),
        "or" => Some(OpCodes::Or),
        "nor" => Some(OpCodes::Nor),
        "xor" => Some(OpCodes::Xor),
        "and" => Some(OpCodes::And),
        "nand" => Some(OpCodes::Nand),
        "nop" => Some(OpCodes::Nop),
        _ => None
    }
}


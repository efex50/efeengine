use efepages::page::Page;

use crate::{
    instruction::{Operands, PtrInner, Registers},
    runtime::{data_funs::{mem_read, mem_write}, program::PRegisters},
};

use super::PThread;

fn be_to_usize(bytes: &[u8]) -> usize {
    let mut v: u64 = 0;
    for b in bytes {
        v = (v << 8) | *b as u64;
    }
    v as usize
}

impl PThread {
    pub(super) fn get_op_data(&self, prg: &Page, a: &Operands) -> Result<usize, String>{
        let r = &self.registers;
        Ok(match a {
            Operands::Static(s) => *s,
            Operands::RA => r.ra,
            Operands::RB => r.rb,
            Operands::RC => r.rc,
            Operands::RD => r.rd,
            Operands::RBP => r.rbp,
            Operands::RSP => r.rsp,
            Operands::R1 =>  r.r1,
            Operands::R2 =>  r.r2,
            Operands::R3 =>  r.r3,
            Operands::R4 =>  r.r4,
            Operands::R5 =>  r.r5,
            Operands::R6 =>  r.r6,
            Operands::AL => r.ra & 0xff,
            Operands::AH => (r.ra & 0xff00) >> 8,
            Operands::BL => r.rb & 0xff,
            Operands::BH => (r.rb & 0xff00) >> 8,
            Operands::CL => r.rc & 0xff,
            Operands::CH => (r.rc & 0xff00) >> 8,
            Operands::DL => r.rd & 0xff,
            Operands::DH => (r.rd & 0xff00) >> 8,

            Operands::BYTEPTR(p) => {
                let addr = get_pointer_inner_addr(p, r);
                be_to_usize(&mem_read(prg, addr, 1)?)
            },
            Operands::WORDPTR(p) => {
                let addr = get_pointer_inner_addr(p, r);
                be_to_usize(&mem_read(prg, addr, 2)?)
            },
            Operands::DWORDPTR(p) => {
                let addr = get_pointer_inner_addr(p, r);
                be_to_usize(&mem_read(prg, addr, 4)?)
            },
            Operands::QWORDPTR(p) => {
                let addr = get_pointer_inner_addr(p, r);
                be_to_usize(&mem_read(prg, addr, 8)?)
            },
            Operands::Pointer(p) => get_pointer_inner_addr(p, r),
            n => return Err(format!("operand {n:?} cannot be read")),
        })
    }

    pub(super) fn write_op_data(&mut self, prg: &mut Page, a: &Operands, v: usize) -> Result<(), String>{
        fn low(reg: usize, v: usize) -> usize { (reg & !0xff) | (v & 0xff) }
        fn high(reg: usize, v: usize) -> usize { (reg & !0xff00) | ((v & 0xff) << 8) }
        match a {
            Operands::RA => self.registers.ra = v,
            Operands::RB => self.registers.rb = v,
            Operands::RC => self.registers.rc = v,
            Operands::RD => self.registers.rd = v,
            Operands::RBP => self.registers.rbp = v,
            Operands::RSP => self.registers.rsp = v,
            Operands::R1 =>  self.registers.r1 = v,
            Operands::R2 =>  self.registers.r2 = v,
            Operands::R3 =>  self.registers.r3 = v,
            Operands::R4 =>  self.registers.r4 = v,
            Operands::R5 =>  self.registers.r5 = v,
            Operands::R6 =>  self.registers.r6 = v,
            Operands::AL => self.registers.ra = low(self.registers.ra, v),
            Operands::AH => self.registers.ra = high(self.registers.ra, v),
            Operands::BL => self.registers.rb = low(self.registers.rb, v),
            Operands::BH => self.registers.rb = high(self.registers.rb, v),
            Operands::CL => self.registers.rc = low(self.registers.rc, v),
            Operands::CH => self.registers.rc = high(self.registers.rc, v),
            Operands::DL => self.registers.rd = low(self.registers.rd, v),
            Operands::DH => self.registers.rd = high(self.registers.rd, v),
            Operands::BYTEPTR(p) | Operands::Pointer(p) => {
                let addr = get_pointer_inner_addr(p, &self.registers);
                mem_write(prg, addr, (v as u8).to_be_bytes().to_vec())?;
            },
            Operands::WORDPTR(p) => {
                let addr = get_pointer_inner_addr(p, &self.registers);
                mem_write(prg, addr, (v as u16).to_be_bytes().to_vec())?;
            },
            Operands::DWORDPTR(p) => {
                let addr = get_pointer_inner_addr(p, &self.registers);
                mem_write(prg, addr, (v as u32).to_be_bytes().to_vec())?;
            },
            Operands::QWORDPTR(p) => {
                let addr = get_pointer_inner_addr(p, &self.registers);
                mem_write(prg, addr, (v as u64).to_be_bytes().to_vec())?;
            },
            n => return Err(format!("operand {n:?} cannot be written")),
        }
        Ok(())
    }
}


/// Effective address of a memory operand (`lea`).
pub(super) fn effective_address(a: &Operands, regs: &PRegisters) -> Option<usize> {
    match a {
        Operands::Pointer(p) | Operands::BYTEPTR(p) | Operands::WORDPTR(p) |
        Operands::DWORDPTR(p) | Operands::QWORDPTR(p) => Some(get_pointer_inner_addr(p, regs)),
        _ => None,
    }
}

fn get_pointer_inner_addr(a:&PtrInner,regs:&PRegisters) -> usize{
    match a {
        PtrInner::Static(a) => *a,
        PtrInner::Reg(registers) => regtype_to_reg_data(registers,regs),
        PtrInner::Sum(registers, a) => regtype_to_reg_data(registers,regs).wrapping_add(*a),
        PtrInner::Ext(registers, a) => regtype_to_reg_data(registers,regs).wrapping_sub(*a),
        PtrInner::Extr(a, registers) => a.wrapping_sub(regtype_to_reg_data(registers,regs)),
        PtrInner::SumReg(registers, registers1) => regtype_to_reg_data(registers,regs).wrapping_add(regtype_to_reg_data(registers1,regs)),
        PtrInner::ExtReg(registers, registers1) => regtype_to_reg_data(registers,regs).wrapping_sub(regtype_to_reg_data(registers1,regs)),
    }
}


fn regtype_to_reg_data(a:&Registers,regs:&PRegisters) -> usize{
    match a {
        Registers::RA => regs.ra,
        Registers::RB => regs.rb,
        Registers::RC => regs.rc,
        Registers::RD => regs.rd,
        Registers::RBP => regs.rbp,
        Registers::RSP => regs.rsp,
        Registers::R1 => regs.r1,
        Registers::R2 => regs.r2,
        Registers::R3 => regs.r3,
        Registers::R4 => regs.r4,
        Registers::R5 => regs.r5,
        Registers::R6 => regs.r6,
        Registers::AL => regs.ra & 0xff,
        Registers::AH => (regs.ra & 0xff00) >> 8,
        Registers::BL => regs.rb & 0xff,
        Registers::BH => (regs.rb & 0xff00) >> 8,
        Registers::CL => regs.rc & 0xff,
        Registers::CH => (regs.rc & 0xff00) >> 8,
        Registers::DL => regs.rd & 0xff,
        Registers::DH => (regs.rd & 0xff00) >> 8,
    }
}

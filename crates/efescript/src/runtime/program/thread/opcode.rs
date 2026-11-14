use crate::{ops::OpCodes, runtime::{data_funs::{mem_read, mem_write}, Instruction}};
use super::{opcode_funs::effective_address, Ctx, PThread, TickSignal};


impl PThread{

    fn push_bytes(&mut self, ctx: &mut Ctx, bytes: Vec<u8>) -> Result<(), String>{
        let len = bytes.len();
        mem_write(ctx.page, self.registers.rsp, bytes)?;
        self.registers.rsp = self.registers.rsp.wrapping_add(len);
        Ok(())
    }

    fn pop_bytes(&mut self, ctx: &mut Ctx, len: usize) -> Result<Vec<u8>, String>{
        let rsp = self.registers.rsp;
        if rsp < self.stack_base.saturating_add(len) {
            return Err("stack underflow (pop/ret on an empty stack)".to_string());
        }
        let d = mem_read(ctx.page, rsp - len, len)?;
        self.registers.rsp = rsp - len;
        Ok(d)
    }

    fn pop_value(&mut self, ctx: &mut Ctx, len: usize) -> Result<usize, String>{
        let d = self.pop_bytes(ctx, len)?;
        let mut v: u64 = 0;
        for b in d { v = (v << 8) | b as u64; }
        Ok(v as usize)
    }

    pub(super) fn handle_opcodes(&mut self, ctx: &mut Ctx, i:&Instruction) -> Result<TickSignal, String>{
        // helper closures can't borrow self twice, so use small macros
        macro_rules! rd { ($o:expr) => { self.get_op_data(ctx.page, $o)? }; }
        macro_rules! wr { ($o:expr, $v:expr) => {{ let v = $v; self.write_op_data(ctx.page, $o, v)? }}; }
        macro_rules! bin { ($f:expr) => {{
            let r = rd!(&i.operandr);
            let l = rd!(&i.operandl);
            let f: fn(usize, usize) -> usize = $f;
            wr!(&i.operandl, f(l, r));
        }}; }
        macro_rules! jump_if { ($c:expr) => {{
            if $c {
                self.counter = rd!(&i.operandl);
            }
        }}; }

        match i.opcode {
            OpCodes::Mov => { let r = rd!(&i.operandr); wr!(&i.operandl, r); },
            OpCodes::Add8  => bin!(|l, r| (l as u8).wrapping_add(r as u8) as usize),
            OpCodes::Add16 => bin!(|l, r| (l as u16).wrapping_add(r as u16) as usize),
            OpCodes::Add32 => bin!(|l, r| (l as u32).wrapping_add(r as u32) as usize),
            OpCodes::Add64 => bin!(|l, r| (l as u64).wrapping_add(r as u64) as usize),
            OpCodes::Sub8  => bin!(|l, r| (l as u8).wrapping_sub(r as u8) as usize),
            OpCodes::Sub16 => bin!(|l, r| (l as u16).wrapping_sub(r as u16) as usize),
            OpCodes::Sub32 => bin!(|l, r| (l as u32).wrapping_sub(r as u32) as usize),
            OpCodes::Sub64 => bin!(|l, r| (l as u64).wrapping_sub(r as u64) as usize),
            OpCodes::Or   => bin!(|l, r| l | r),
            OpCodes::Xor  => bin!(|l, r| l ^ r),
            OpCodes::And  => bin!(|l, r| l & r),
            OpCodes::Nand => bin!(|l, r| !(l & r)),
            OpCodes::Nor  => bin!(|l, r| !(l | r)),
            OpCodes::Lea => {
                let addr = effective_address(&i.operandr, &self.registers)
                    .ok_or_else(|| "lea needs a memory operand".to_string())?;
                wr!(&i.operandl, addr);
            },
            OpCodes::Push8 => { let d = rd!(&i.operandl); self.push_bytes(ctx, vec![d as u8])?; },
            OpCodes::Push16 => { let d = rd!(&i.operandl) as u16; self.push_bytes(ctx, d.to_be_bytes().to_vec())?; },
            OpCodes::Push32 => { let d = rd!(&i.operandl) as u32; self.push_bytes(ctx, d.to_be_bytes().to_vec())?; },
            OpCodes::Push64 => { let d = rd!(&i.operandl) as u64; self.push_bytes(ctx, d.to_be_bytes().to_vec())?; },
            OpCodes::Pop8  => { let d = self.pop_value(ctx, 1)?; wr!(&i.operandl, d); },
            OpCodes::Pop16 => { let d = self.pop_value(ctx, 2)?; wr!(&i.operandl, d); },
            OpCodes::Pop32 => { let d = self.pop_value(ctx, 4)?; wr!(&i.operandl, d); },
            OpCodes::Pop64 => { let d = self.pop_value(ctx, 8)?; wr!(&i.operandl, d); },
            OpCodes::Cmp => {
                let first = rd!(&i.operandl);
                let sec = rd!(&i.operandr);
                self.flags.reset();
                self.flags.equals = first == sec;
                self.flags.zero = first == sec;
                self.flags.greater = first > sec;
                self.flags.lesser = first < sec;
                self.flags.greatereq = first >= sec;
                self.flags.lessereq = first <= sec;
            },
            OpCodes::Test => {
                let first = rd!(&i.operandl);
                self.flags.reset();
                self.flags.zero = first == 0;
                self.flags.greater = first > 0;
                let top = first >> (usize::BITS - 1) == 1;
                self.flags.negative = top;
                self.flags.sign = top;
            },
            OpCodes::Jmp => jump_if!(true),
            OpCodes::Call => {
                let addr = rd!(&i.operandl);
                let ret = self.counter.to_be_bytes().to_vec();
                self.push_bytes(ctx, ret)?;
                self.counter = addr;
            },
            OpCodes::Ret => {
                let d = self.pop_value(ctx, std::mem::size_of::<usize>())?;
                self.counter = d;
            },
            OpCodes::SysCall => return Ok(self.handle_syscalls(ctx)),
            OpCodes::Nop => (),
            OpCodes::Je  => jump_if!(self.flags.equals),
            OpCodes::Jne => jump_if!(!self.flags.equals),
            OpCodes::Jgt => jump_if!(self.flags.greater),
            OpCodes::Jlt => jump_if!(self.flags.lesser),
            OpCodes::Jle => jump_if!(self.flags.lessereq),
            OpCodes::Jge => jump_if!(self.flags.greatereq),
            OpCodes::Jz  => jump_if!(self.flags.zero),
            OpCodes::Jnz => jump_if!(!self.flags.zero),
            OpCodes::Js  => jump_if!(self.flags.sign),
        };
        Ok(TickSignal::Continue)
    }
}

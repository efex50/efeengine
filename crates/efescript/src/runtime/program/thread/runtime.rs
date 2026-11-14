use efepages::page::Page;
use num_traits::FromPrimitive;

use crate::{
    instruction::Operands,
    ops::OpCodes,
    runtime::{data_funs::{decode_operand, read_u8}, Instruction},
};

use super::{Ctx, PThread, TickSignal};


impl PThread{
    /// Decodes the instruction at `self.counter` and advances `self.counter` past it.
    /// `Ok(None)` is the end-of-program byte.
    fn get_instruction(&mut self, prg: &Page) -> Result<Option<Instruction>, String>{
        let start = self.counter;
        let mut pos = start;
        let op = read_u8(prg, &mut pos)?;
        if op == 0 {return Ok(None);}
        let code = OpCodes::from_u8(op)
            .ok_or_else(|| format!("unknown opcode {op:#04x} at address {start:#x}"))?;
        let (operandl, operandr) = match code {
            OpCodes::Mov |
            OpCodes::Add8 | OpCodes::Add16 | OpCodes::Add32 | OpCodes::Add64 |
            OpCodes::Sub8 | OpCodes::Sub16 | OpCodes::Sub32 | OpCodes::Sub64 |
            OpCodes::Or | OpCodes::Xor | OpCodes::And | OpCodes::Nand | OpCodes::Nor |
            OpCodes::Lea |
            OpCodes::Cmp  => {
                let l = decode_operand(prg, &mut pos)?;
                let r = decode_operand(prg, &mut pos)?;
                (l, r)
            },
            OpCodes::Test |
            OpCodes::Call |
            OpCodes::Push8 | OpCodes::Push16 | OpCodes::Push32 | OpCodes::Push64 |
            OpCodes::Pop8 | OpCodes::Pop16 | OpCodes::Pop32 | OpCodes::Pop64 |
            OpCodes::Jmp | OpCodes::Je | OpCodes::Jne | OpCodes::Jgt | OpCodes::Jlt |
            OpCodes::Jle | OpCodes::Jge | OpCodes::Jz | OpCodes::Js | OpCodes::Jnz => {
                (decode_operand(prg, &mut pos)?, Operands::Null)
            },
            OpCodes::Ret | OpCodes::Nop | OpCodes::SysCall => (Operands::Null, Operands::Null),
        };
        if code == OpCodes::Lea {
            match operandr {
                Operands::Pointer(_) | Operands::BYTEPTR(_) | Operands::WORDPTR(_) |
                Operands::DWORDPTR(_) | Operands::QWORDPTR(_) => (),
                _ => return Err(format!("lea at {start:#x} needs a memory operand")),
            }
        }
        self.counter = pos;
        Ok(Some(Instruction{ opcode:code, operandl, operandr }))
    }

    /// runs the program until end, finish or fault
    pub fn start(&mut self, ctx: &mut Ctx) -> TickSignal{
        loop {
            match self.tick(ctx) {
                TickSignal::Continue => (),
                other => return other,
            }
        }
    }

    /// process 1 instruction
    pub fn tick(&mut self, ctx: &mut Ctx) -> TickSignal{
        let i = match self.get_instruction(ctx.page) {
            Ok(Some(i)) => i,
            Ok(None) => return TickSignal::End,
            Err(e) => return TickSignal::Fault(e),
        };
        match self.handle_opcodes(ctx, &i) {
            Ok(sig) => sig,
            Err(e) => TickSignal::Fault(e),
        }
    }
}

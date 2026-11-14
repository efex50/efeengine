use std::io::BufRead;

use num_traits::FromPrimitive;

use crate::{
    funs::as_usize,
    runtime::data_funs::{mem_read, mem_write, NumToStr},
    syscalls::SysCalls,
};

use super::{Ctx, PThread, TickSignal};

impl PThread{

    /// pushes `data` at rsp (the "write to stack" convention of the string syscalls)
    fn write_to_stack(&mut self, ctx: &mut Ctx, data: Vec<u8>) -> Result<(), String>{
        let len = data.len();
        mem_write(ctx.page, self.registers.rsp, data)?;
        self.registers.rsp = self.registers.rsp.wrapping_add(len);
        Ok(())
    }

    pub(super) fn handle_syscalls(&mut self, ctx: &mut Ctx) -> TickSignal{
        match self.run_syscall(ctx) {
            Ok(sig) => sig,
            Err(e) => TickSignal::Fault(e),
        }
    }

    fn run_syscall(&mut self, ctx: &mut Ctx) -> Result<TickSignal, String>{
        let number = self.registers.ra;
        let syscall = SysCalls::from_usize(number)
            .ok_or_else(|| format!("unknown syscall {number}"))?;
        match syscall {
            SysCalls::Printchar => {
                let c = u32::try_from(self.registers.rb).ok().and_then(char::from_u32)
                    .ok_or_else(|| format!("Printchar: {:#x} is not a valid char", self.registers.rb))?;
                let mut buf = [0u8; 4];
                ctx.host.write_out(c.encode_utf8(&mut buf).as_bytes());
            },
            SysCalls::Print => {
                let mut bytes = mem_read(ctx.page, self.registers.rb, self.registers.rc)?;
                bytes.push(b' ');
                ctx.host.write_out(&bytes);
            },
            SysCalls::Println => {
                let mut bytes = mem_read(ctx.page, self.registers.rb, self.registers.rc)?;
                bytes.push(b'\n');
                ctx.host.write_out(&bytes);
            },
            SysCalls::Finish => {
                return Ok(TickSignal::Finish(self.registers.rb as u8));
            },
            SysCalls::NumToString => {
                let r = self.registers.rb;
                let i = if self.registers.rc == 1 { NumToStr::I(r as isize) } else { NumToStr::U(r) };
                let s = match self.registers.rd {
                    1 => match i {NumToStr::I(s) => format!("{:x}",s), NumToStr::U(u) => format!("{:x}",u)},
                    2 => match i {NumToStr::I(s) => format!("{:b}",s), NumToStr::U(u) => format!("{:b}",u)},
                    3 => match i {NumToStr::I(s) => format!("{:o}",s), NumToStr::U(u) => format!("{:o}",u)},
                    _ => match i {NumToStr::I(s) => format!("{}",s), NumToStr::U(u) => format!("{}",u)},
                };
                let s = s.into_bytes();
                self.registers.rc = s.len();
                self.registers.rb = self.registers.rsp;
                self.write_to_stack(ctx, s)?;
            },
            SysCalls::StringToNum => {
                let bytes = mem_read(ctx.page, self.registers.rb, self.registers.rc)?;
                let str = String::from_utf8(bytes)
                    .map_err(|_| "StringToNum: the string is not valid UTF-8".to_string())?;
                let num = as_usize(str.clone())
                    .map_err(|_| format!("StringToNum: {str:?} is not a number"))?;
                self.registers.ra = num;
            },
            SysCalls::Readln => {
                let mut line = Vec::new();
                std::io::stdin().lock().read_until(b'\n', &mut line)
                    .map_err(|e| format!("Readln: {e}"))?;
                if line.last() == Some(&b'\n') { line.pop(); }
                if line.last() == Some(&b'\r') { line.pop(); }
                self.registers.rb = self.registers.rsp;
                self.registers.rc = line.len();
                self.write_to_stack(ctx, line)?;
            },
            SysCalls::ReadCon => {
                let brk = self.registers.rd as u8;
                let mut a = Vec::new();
                std::io::stdin().lock().read_until(brk, &mut a)
                    .map_err(|e| format!("ReadCon: {e}"))?;
                self.registers.rb = self.registers.rsp;
                self.registers.rc = a.len();
                self.write_to_stack(ctx, a)?;
            },
            SysCalls::ReadFs => {
                let bytes = mem_read(ctx.page, self.registers.rb, self.registers.rc)?;
                let pstr = String::from_utf8(bytes)
                    .map_err(|_| "ReadFs: the path is not valid UTF-8".to_string())?;
                let d = std::fs::read(&pstr).map_err(|e| format!("ReadFs: {pstr}: {e}"))?;
                self.write_to_stack(ctx, d)?;
            },
            SysCalls::WriteFs => {
                let to_write = mem_read(ctx.page, self.registers.rb, self.registers.rc)?;
                let bytes = mem_read(ctx.page, self.registers.rd, self.registers.r1)?;
                let pstr = String::from_utf8(bytes)
                    .map_err(|_| "WriteFs: the path is not valid UTF-8".to_string())?;
                std::fs::write(&pstr, to_write).map_err(|e| format!("WriteFs: {pstr}: {e}"))?;
            },
            SysCalls::PlaySound => {
                let result = mem_read(ctx.page, self.registers.rb, self.registers.rc)
                    .and_then(|b| String::from_utf8(b).map_err(|_| "the path is not valid UTF-8".to_string()))
                    .and_then(|path| ctx.host.play_sound(&path));
                match result {
                    Ok(()) => self.registers.ra = 0,
                    Err(e) => {
                        eprintln!("efescript: PlaySound failed: {e}");
                        self.registers.ra = 1;
                    }
                }
            },
            SysCalls::HeapAlloc | SysCalls::Write | SysCalls::Flush | SysCalls::RaylibEx1 => {
                return Err(format!("syscall {number} is not implemented"));
            },
        };
        Ok(TickSignal::Continue)
    }

}

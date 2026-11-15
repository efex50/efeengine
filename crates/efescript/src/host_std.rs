//! The `std` host namespace for efescript v2 programs (design: crates/efescript/docs/e2-design.md section 8).

use std::io::BufRead;
use efevm::{Caller, FuncType, HostOk, Imports, Trap, ValType};
use crate::prelude::Host;

/// Host state for programs that import `std.*`: wraps an output/sound `Host`, an input
/// source for `read_line`, and the exit code requested by `std.exit`.
pub struct StdEnv<H: Host> {
    /// Output and sound host.
    pub host: H,
    input: Box<dyn BufRead>,
    exit_code: Option<i32>,
}

impl<H: Host> StdEnv<H> {
    /// No input: `read_line` sees end of input.
    pub fn new(host: H) -> Self {
        Self::with_input(host, Vec::new())
    }

    /// `read_line` reads the process stdin.
    pub fn with_stdin(host: H) -> Self {
        Self {
            host,
            input: Box::new(std::io::BufReader::new(std::io::stdin())),
            exit_code: None,
        }
    }

    /// `read_line` reads these bytes (for tests).
    pub fn with_input(host: H, input: impl Into<Vec<u8>>) -> Self {
        Self {
            host,
            input: Box::new(std::io::Cursor::new(input.into())),
            exit_code: None,
        }
    }

    /// The code passed to `std.exit`, if it was called.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}

/// The signatures of every `std` function, in table order: (name, params, results).
pub const STD_FUNCS: &[(&str, &[ValType], &[ValType])] = &[
    ("print", &[ValType::I64, ValType::I64], &[]),
    ("println", &[ValType::I64, ValType::I64], &[]),
    ("print_i64", &[ValType::I64], &[]),
    ("print_f64", &[ValType::F64], &[]),
    ("print_char", &[ValType::I32], &[]),
    ("read_line", &[ValType::I64, ValType::I64], &[ValType::I64]),
    ("play_sound", &[ValType::I64, ValType::I64], &[ValType::I32]),
    ("exit", &[ValType::I32], &[]),
];

fn func_type(idx: usize) -> FuncType {
    FuncType::new(STD_FUNCS[idx].1.iter().copied(), STD_FUNCS[idx].2.iter().copied())
}

fn get_arg(args: &[u64], index: usize) -> Result<u64, Trap> {
    match args.get(index) {
        Some(&v) => Ok(v),
        None => Err(Trap::Host("missing argument".into())),
    }
}

fn set_res(res: &mut [u64], index: usize, val: u64) -> Result<(), Trap> {
    match res.get_mut(index) {
        Some(slot) => {
            *slot = val;
            Ok(())
        }
        None => Err(Trap::Host("missing result slot".into())),
    }
}

fn read_guest_bytes<H: Host>(
    caller: &Caller<'_, StdEnv<H>>,
    raw_ptr: u64,
    raw_len: u64,
) -> Result<Vec<u8>, Trap> {
    let ptr = raw_ptr as i64;
    let len = raw_len as i64;
    if ptr < 0 || len < 0 {
        return Err(Trap::OutOfBounds);
    }
    let ptr = ptr as u64;
    let len = len as u64;
    let mem_len = caller.memory().len();
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return Err(Trap::OutOfBounds),
    };
    if end > mem_len {
        return Err(Trap::OutOfBounds);
    }
    let ulen = match usize::try_from(len) {
        Ok(u) => u,
        Err(_) => return Err(Trap::OutOfBounds),
    };
    let mut buf = vec![0u8; ulen];
    caller.read(ptr, &mut buf)?;
    Ok(buf)
}

/// Registers every `std` function on a new `Imports`.
pub fn std_imports<H: Host + 'static>() -> Imports<StdEnv<H>> {
    let mut imports = Imports::new();
    add_std_imports(&mut imports);
    imports
}

/// Registers every `std` function on an existing `Imports`.
pub fn add_std_imports<H: Host + 'static>(imports: &mut Imports<StdEnv<H>>) {
    imports.func("std", STD_FUNCS[0].0, func_type(0), |caller, args, _| {
        let raw_ptr = get_arg(args, 0)?;
        let raw_len = get_arg(args, 1)?;
        let bytes = read_guest_bytes(caller, raw_ptr, raw_len)?;
        caller.host().host.write_out(&bytes);
        Ok(HostOk::Continue)
    });

    imports.func("std", STD_FUNCS[1].0, func_type(1), |caller, args, _| {
        let raw_ptr = get_arg(args, 0)?;
        let raw_len = get_arg(args, 1)?;
        let bytes = read_guest_bytes(caller, raw_ptr, raw_len)?;
        let host = &mut caller.host().host;
        host.write_out(&bytes);
        host.write_out(b"\n");
        Ok(HostOk::Continue)
    });

    imports.func("std", STD_FUNCS[2].0, func_type(2), |caller, args, _| {
        let val = get_arg(args, 0)? as i64;
        let s = val.to_string();
        caller.host().host.write_out(s.as_bytes());
        Ok(HostOk::Continue)
    });

    imports.func("std", STD_FUNCS[3].0, func_type(3), |caller, args, _| {
        let bits = get_arg(args, 0)?;
        let v = f64::from_bits(bits);
        let s = format!("{}", v);
        caller.host().host.write_out(s.as_bytes());
        Ok(HostOk::Continue)
    });

    imports.func("std", STD_FUNCS[4].0, func_type(4), |caller, args, _| {
        let val = get_arg(args, 0)? as u32;
        let c = match char::from_u32(val) {
            Some(ch) => ch,
            None => return Err(Trap::Host("std.print_char: invalid character".into())),
        };
        let mut buf = [0u8; 4];
        caller.host().host.write_out(c.encode_utf8(&mut buf).as_bytes());
        Ok(HostOk::Continue)
    });

    imports.func("std", STD_FUNCS[5].0, func_type(5), |caller, args, res| {
        let raw_ptr = get_arg(args, 0)?;
        let raw_cap = get_arg(args, 1)?;
        let ptr = raw_ptr as i64;
        let cap = raw_cap as i64;
        if ptr < 0 || cap < 0 {
            return Err(Trap::OutOfBounds);
        }
        let mut line = Vec::new();
        match caller.host().input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => {
                set_res(res, 0, (-1i64) as u64)?;
                Ok(HostOk::Continue)
            }
            Ok(_) => {
                if line.ends_with(b"\n") {
                    line.pop();
                    if line.ends_with(b"\r") {
                        line.pop();
                    }
                }
                let cap_usize = match usize::try_from(cap) {
                    Ok(c) => c,
                    Err(_) => usize::MAX,
                };
                let to_write = line.len().min(cap_usize);
                caller.write(ptr as u64, &line[..to_write])?;
                set_res(res, 0, to_write as u64)?;
                Ok(HostOk::Continue)
            }
        }
    });

    imports.func("std", STD_FUNCS[6].0, func_type(6), |caller, args, res| {
        let raw_ptr = get_arg(args, 0)?;
        let raw_len = get_arg(args, 1)?;
        let bytes = read_guest_bytes(caller, raw_ptr, raw_len)?;
        let path = match std::str::from_utf8(&bytes) {
            Ok(p) => p,
            Err(_) => {
                set_res(res, 0, 1)?;
                return Ok(HostOk::Continue);
            }
        };
        let code: u32 = match caller.host().host.play_sound(path) {
            Ok(()) => 0,
            Err(_) => 1,
        };
        set_res(res, 0, code as u64)?;
        Ok(HostOk::Continue)
    });

    imports.func("std", STD_FUNCS[7].0, func_type(7), |caller, args, _| {
        let raw_code = get_arg(args, 0)?;
        let code = raw_code as u32 as i32;
        caller.host().exit_code = Some(code);
        Ok(HostOk::Yield)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::CaptureHost;
    use efevm::{Exit, FiberId, FunctionBuilder, Instance, Limits, ModuleBuilder};

    fn import_std(mb: &mut ModuleBuilder, name: &str) -> u32 {
        let (_, p, r) = STD_FUNCS.iter().find(|(n, _, _)| *n == name).unwrap();
        mb.import("std", name, FuncType::new(p.iter().copied(), r.iter().copied()))
    }

    fn run_main(
        data: Option<(u64, &[u8])>,
        nregs: u16,
        results: &[ValType],
        body: impl FnOnce(&mut ModuleBuilder, &mut FunctionBuilder),
        env: &mut StdEnv<CaptureHost>,
    ) -> (Exit, Instance<StdEnv<CaptureHost>>, FiberId) {
        let mut mb = ModuleBuilder::new();
        let mut fb = FunctionBuilder::new(nregs);
        body(&mut mb, &mut fb);
        let f = mb.add_func(FuncType::new([], results.iter().copied()), fb);
        mb.export_func("main", f);
        mb.memory(1, None);
        if let Some((offset, bytes)) = data {
            mb.data(offset, bytes);
        }
        let module = mb.finish().unwrap();
        let mut inst = Instance::new(&module, &std_imports::<CaptureHost>(), Limits::default()).unwrap();
        let fiber = inst.spawn("main", &[]).unwrap();
        let exit = inst.resume(fiber, env, 1000);
        (exit, inst, fiber)
    }

    #[test]
    fn test_println() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, _, _) = run_main(Some((1024, b"hi")), 2, &[], |mb, fb| {
            let p = import_std(mb, "println");
            fb.const_i64(0, 1024);
            fb.const_i64(1, 2);
            fb.call(p, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Done);
        assert_eq!(env.host.out, b"hi\n");
    }

    #[test]
    fn test_print_i64() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, _, _) = run_main(None, 1, &[], |mb, fb| {
            let p = import_std(mb, "print_i64");
            fb.const_i64(0, -42);
            fb.call(p, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Done);
        assert_eq!(env.host.out, b"-42");
    }

    #[test]
    fn test_print_char() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, _, _) = run_main(None, 1, &[], |mb, fb| {
            let p = import_std(mb, "print_char");
            fb.const_i32(0, 0xE9);
            fb.call(p, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Done);
        assert_eq!(env.host.out, "é".as_bytes());

        let mut env2 = StdEnv::new(CaptureHost::new());
        let (exit2, _, _) = run_main(None, 1, &[], |mb, fb| {
            let p = import_std(mb, "print_char");
            fb.const_i32(0, 0xD800);
            fb.call(p, 0);
            fb.ret(0);
        }, &mut env2);
        assert_eq!(exit2, Exit::Trap(Trap::Host("std.print_char: invalid character".into())));
    }

    #[test]
    fn test_read_line() {
        let mut mb = ModuleBuilder::new();
        let rl = import_std(&mut mb, "read_line");
        let mut fb = FunctionBuilder::new(2);
        fb.const_i64(0, 1024);
        fb.const_i64(1, 16);
        fb.call(rl, 0);
        fb.ret(0);
        let f = mb.add_func(FuncType::new([], [ValType::I64]), fb);
        mb.export_func("main", f);
        mb.memory(1, None);
        let module = mb.finish().unwrap();

        let mut env = StdEnv::with_input(CaptureHost::new(), "abc\r\nxyz");
        let mut inst = Instance::new(&module, &std_imports::<CaptureHost>(), Limits::default()).unwrap();

        // Call 1: read first line "abc"
        let fib1 = inst.spawn("main", &[]).unwrap();
        assert_eq!(inst.resume(fib1, &mut env, 1000), Exit::Done);
        assert_eq!(inst.results(fib1).unwrap(), &[3]);
        let mut buf = [0u8; 3];
        inst.memory().read(1024, &mut buf).unwrap();
        assert_eq!(&buf, b"abc");

        // Call 2: read second line "xyz"
        let fib2 = inst.spawn("main", &[]).unwrap();
        assert_eq!(inst.resume(fib2, &mut env, 1000), Exit::Done);
        assert_eq!(inst.results(fib2).unwrap(), &[3]);
        inst.memory().read(1024, &mut buf).unwrap();
        assert_eq!(&buf, b"xyz");

        // Call 3: end of input with nothing read -> -1
        let fib3 = inst.spawn("main", &[]).unwrap();
        assert_eq!(inst.resume(fib3, &mut env, 1000), Exit::Done);
        assert_eq!(inst.results(fib3).unwrap(), &[(-1i64) as u64]);

        // Truncation by cap
        let mut mb2 = ModuleBuilder::new();
        let rl2 = import_std(&mut mb2, "read_line");
        let mut fb2 = FunctionBuilder::new(2);
        fb2.const_i64(0, 1024);
        fb2.const_i64(1, 2);
        fb2.call(rl2, 0);
        fb2.ret(0);
        let f2 = mb2.add_func(FuncType::new([], [ValType::I64]), fb2);
        mb2.export_func("main", f2);
        mb2.memory(1, None);
        let mut env2 = StdEnv::with_input(CaptureHost::new(), "hello\n");
        let mut inst2 = Instance::new(&mb2.finish().unwrap(), &std_imports::<CaptureHost>(), Limits::default()).unwrap();
        let fib4 = inst2.spawn("main", &[]).unwrap();
        assert_eq!(inst2.resume(fib4, &mut env2, 1000), Exit::Done);
        assert_eq!(inst2.results(fib4).unwrap(), &[2]);
        let mut buf2 = [0u8; 2];
        inst2.memory().read(1024, &mut buf2).unwrap();
        assert_eq!(&buf2, b"he");
    }

    #[test]
    fn test_play_sound() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, inst, fib) = run_main(Some((1024, b"test.wav")), 2, &[ValType::I32], |mb, fb| {
            let ps = import_std(mb, "play_sound");
            fb.const_i64(0, 1024);
            fb.const_i64(1, 8);
            fb.call(ps, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Done);
        assert_eq!(inst.results(fib).unwrap(), &[0]);
        assert_eq!(env.host.sounds, vec!["test.wav".to_string()]);
    }

    #[test]
    fn test_exit() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, _, _) = run_main(None, 1, &[], |mb, fb| {
            let ex = import_std(mb, "exit");
            fb.const_i32(0, 42);
            fb.call(ex, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Yield);
        assert_eq!(env.exit_code(), Some(42));
    }

    #[test]
    fn test_print_out_of_bounds() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, _, _) = run_main(None, 2, &[], |mb, fb| {
            let p = import_std(mb, "print");
            fb.const_i64(0, 65530);
            fb.const_i64(1, 10);
            fb.call(p, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Trap(Trap::OutOfBounds));
    }

    #[test]
    fn test_print_f64_and_print() {
        let mut env = StdEnv::new(CaptureHost::new());
        let (exit, _, _) = run_main(Some((1024, b"ok")), 2, &[], |mb, fb| {
            let pf = import_std(mb, "print_f64");
            let p = import_std(mb, "print");
            fb.const_f64(0, 3.5);
            fb.call(pf, 0);
            fb.const_i64(0, 1024);
            fb.const_i64(1, 2);
            fb.call(p, 0);
            fb.ret(0);
        }, &mut env);
        assert_eq!(exit, Exit::Done);
        assert_eq!(env.host.out, b"3.5ok");

        let env_stdin = StdEnv::with_stdin(CaptureHost::new());
        assert_eq!(env_stdin.exit_code(), None);
    }
}

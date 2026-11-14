//! Runtime unit tests with hand-written bytecode (no assembler involved).

use std::{cell::RefCell, rc::Rc};

use super::{ProgramRuntime, RunResult};
use crate::runtime::host::CaptureHost;

// registers
const RA: u8 = 1;
const RB: u8 = 2;
const RC: u8 = 3;
const RD: u8 = 4;
const RSP: u8 = 6;
const AL: u8 = 13;
const AH: u8 = 14;
const BL: u8 = 15;
const DH: u8 = 20;
// pointer tags
const PTR: u8 = 0x16;
const BYTEPTR: u8 = 0x17;
const WORDPTR: u8 = 0x18;
// opcodes
const MOV: u8 = 1;
const ADD8: u8 = 2;
const ADD64: u8 = 5;
const SUB8: u8 = 6;
const LEA: u8 = 15;
const PUSH64: u8 = 23;
const POP64: u8 = 27;
const CMP: u8 = 28;
const TEST: u8 = 29;
const JMP: u8 = 30;
const JZ: u8 = 37;
const JNZ: u8 = 38;
const JS: u8 = 39;
const RET: u8 = 41;
const SYSCALL: u8 = 42;

/// minimal static operand
fn imm(n: u64) -> Vec<u8> {
    let b: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
    let mut v = vec![0, b.len() as u8];
    v.extend(b);
    v
}
/// fixed 4 byte static (what the assembler emits for symbols)
fn st4(n: u32) -> Vec<u8> {
    let mut v = vec![0, 4];
    v.extend(n.to_be_bytes());
    v
}
fn ins(op: u8, ops: &[&[u8]]) -> Vec<u8> {
    let mut v = vec![op];
    for o in ops {
        v.extend_from_slice(o);
    }
    v
}
fn cat(parts: Vec<Vec<u8>>) -> Vec<u8> {
    parts.into_iter().flatten().collect()
}
fn mov_imm(reg: u8, n: u64) -> Vec<u8> {
    ins(MOV, &[&[reg], &imm(n)])
}
fn syscall(n: u64) -> Vec<u8> {
    cat(vec![mov_imm(RA, n), vec![SYSCALL]])
}
fn finish_with_rb() -> Vec<u8> {
    syscall(5050)
}
/// code + end marker + data; returns the program and the address of the data
fn with_data(code: Vec<u8>, data: &[u8]) -> Vec<u8> {
    let mut p = code;
    p.push(0);
    p.extend_from_slice(data);
    p
}

fn run(prog: Vec<u8>) -> (RunResult, ProgramRuntime, Rc<RefCell<CaptureHost>>) {
    let cap = CaptureHost::shared();
    let mut rt = ProgramRuntime::with_host(Box::new(cap.clone()));
    rt.load_from_vec(prog);
    let r = rt.run_with_limit(100_000);
    (r, rt, cap)
}
fn regs(rt: &ProgramRuntime) -> &super::PRegisters {
    &rt.threads[0].registers
}

fn is_fault(r: &RunResult, needle: &str) -> bool {
    matches!(r, RunResult::Fault(m) if m.contains(needle))
}

#[test]
fn println_is_captured_with_newline() {
    // layout is size independent because the address is a fixed 4 byte static
    let build = |addr: u32| {
        cat(vec![
            ins(MOV, &[&[RA], &imm(2)]),
            ins(MOV, &[&[RB], &st4(addr)]),
            ins(MOV, &[&[RC], &imm(5)]),
            vec![SYSCALL],
        ])
    };
    let addr = build(0).len() as u32 + 1;
    let (r, _rt, cap) = run(with_data(build(addr), b"hello"));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(cap.borrow().out, b"hello\n");
}

#[test]
fn print_has_trailing_space_and_printchar_works() {
    let build = |addr: u32| {
        cat(vec![
            ins(MOV, &[&[RB], &st4(addr)]),
            ins(MOV, &[&[RC], &imm(2)]),
            syscall(1),
            mov_imm(RB, 'x' as u64),
            syscall(0),
        ])
    };
    let addr = build(0).len() as u32 + 1;
    let (_r, _rt, cap) = run(with_data(build(addr), b"hi"));
    assert_eq!(cap.borrow().out_string(), "hi x");
}

#[test]
fn finish_returns_exit_code() {
    let prog = cat(vec![mov_imm(RB, 42), finish_with_rb()]);
    let (r, _, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::Finished(42));
}

#[test]
fn run_with_limit_stops_infinite_loop() {
    let (r, _, _) = run(vec![30, 0, 0]);
    assert_eq!(r, RunResult::LimitReached);
}

#[test]
fn unknown_opcode_is_a_fault() {
    let (r, _, _) = run(vec![200, 0]);
    assert!(is_fault(&r, "unknown opcode"), "{r:?}");
}

#[test]
fn bad_operand_byte_is_a_fault() {
    let (r, _, _) = run(vec![MOV, 0x7f, RA, 0]);
    assert!(is_fault(&r, "bad operand"), "{r:?}");
}

#[test]
fn unknown_syscall_is_a_fault() {
    let (r, _, _) = run(cat(vec![syscall(99), vec![0]]));
    assert!(is_fault(&r, "unknown syscall 99"), "{r:?}");
}

#[test]
fn unimplemented_syscalls_are_faults() {
    for n in [9u64, 10, 11, 2550] {
        let (r, _, _) = run(cat(vec![syscall(n), vec![0]]));
        assert!(is_fault(&r, "not implemented"), "syscall {n}: {r:?}");
    }
}

#[test]
fn play_sound_records_path_and_sets_ra_zero() {
    let path = b"examples/ominous_worse.ogg";
    let build = |addr: u32| {
        cat(vec![
            ins(MOV, &[&[RB], &st4(addr)]),
            ins(MOV, &[&[RC], &imm(path.len() as u64)]),
            syscall(12),
        ])
    };
    let addr = build(0).len() as u32 + 1;
    let (r, rt, cap) = run(with_data(build(addr), path));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(cap.borrow().sounds, vec!["examples/ominous_worse.ogg".to_string()]);
    assert_eq!(regs(&rt).ra, 0);
}

#[test]
fn play_sound_bad_utf8_path_fails_without_panic() {
    let build = |addr: u32| {
        cat(vec![
            ins(MOV, &[&[RB], &st4(addr)]),
            ins(MOV, &[&[RC], &imm(2)]),
            syscall(12),
        ])
    };
    let addr = build(0).len() as u32 + 1;
    let (r, rt, cap) = run(with_data(build(addr), &[0xff, 0xfe]));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).ra, 1);
    assert!(cap.borrow().sounds.is_empty());
}

#[test]
fn std_host_play_sound_without_feature_errs() {
    #[cfg(not(feature = "audio"))]
    {
        use crate::runtime::host::{Host, StdHost};
        let e = StdHost.play_sound("x.ogg").unwrap_err();
        assert!(e.contains("audio"));
    }
}

#[test]
fn first_static_in_pointer_reads_len_bytes() {
    // 4 byte static with leading zeros: [0x00001234]; the old decoder read only the first byte
    let prog = cat(vec![
        ins(MOV, &[&[BYTEPTR], &st4(0x1234), &[BYTEPTR], &imm(0x55)]),
        ins(MOV, &[&[RB], &[BYTEPTR], &st4(0x1234), &[BYTEPTR]]),
        // 8 byte static address, and a 2 byte one
        ins(MOV, &[&[RC], &[BYTEPTR], &[0, 8, 0, 0, 0, 0, 0, 0, 0x12, 0x34], &[BYTEPTR]]),
        ins(MOV, &[&[RD], &[WORDPTR], &imm(0x1234), &[WORDPTR]]),
    ]);
    let (r, rt, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).rb, 0x55);
    assert_eq!(regs(&rt).rc, 0x55);
    assert_eq!(regs(&rt).rd, 0x5500);
}

#[test]
fn ext_reg_subtracts() {
    let prog = cat(vec![
        mov_imm(RB, 10),
        mov_imm(RC, 3),
        ins(MOV, &[&[RA], &[PTR], &[RB], b"-", &[RC], &[BYTEPTR]]),
    ]);
    let (r, rt, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).ra, 7);
}

#[test]
fn sub_registers_in_pointers_are_masked() {
    let prog = cat(vec![
        mov_imm(RA, 0x0102),
        mov_imm(RB, 0x1000),
        ins(MOV, &[&[RC], &[PTR], &[RB], b"+", &[AL], &[BYTEPTR]]),
        ins(MOV, &[&[RD], &[PTR], &[RB], b"+", &[AH], &[BYTEPTR]]),
    ]);
    let (_, rt, _) = run(with_data(prog, b""));
    assert_eq!(regs(&rt).rc, 0x1002);
    assert_eq!(regs(&rt).rd, 0x1001);
}

#[test]
fn sub_register_writes_are_masked() {
    let prog = cat(vec![
        mov_imm(RA, 0x1234),
        ins(MOV, &[&[AL], &imm(0x1ff)]),
        mov_imm(RB, 0x1234),
        ins(MOV, &[&[BL], &imm(0x1ff)]),
        mov_imm(RC, 0x1234),
        ins(MOV, &[&[AH], &imm(0x1ff)]), // AH of ra
        mov_imm(RD, 0x1234),
        ins(MOV, &[&[DH], &imm(0x1ff)]),
    ]);
    let (r, rt, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).rb, 0x12ff);
    assert_eq!(regs(&rt).rd, 0xff34);
    // ra: AL write then AH write
    assert_eq!(regs(&rt).ra, 0xffff);
}

#[test]
fn arithmetic_wraps_instead_of_panicking() {
    let prog = cat(vec![
        mov_imm(RA, 0xff),
        ins(ADD8, &[&[RA], &imm(1)]),
        mov_imm(RB, 0),
        ins(SUB8, &[&[RB], &imm(1)]),
        mov_imm(RC, u64::MAX),
        ins(ADD64, &[&[RC], &imm(2)]),
        // pointer arithmetic wraps too: usize::MAX + 2 = 1
        mov_imm(RD, u64::MAX),
        ins(MOV, &[&[RSP], &[PTR], &[RD], b"+", &imm(2), &[BYTEPTR]]),
    ]);
    let (r, rt, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).ra, 0);
    assert_eq!(regs(&rt).rb, 0xff);
    assert_eq!(regs(&rt).rc, 1);
    assert_eq!(regs(&rt).rsp, 1);
}

#[test]
fn push_pop_roundtrip() {
    let prog = cat(vec![
        mov_imm(RA, 0xdead_beef),
        ins(PUSH64, &[&[RA]]),
        ins(POP64, &[&[RB]]),
    ]);
    let (r, rt, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).rb, 0xdead_beef);
}

#[test]
fn pop_and_ret_on_empty_stack_fault() {
    let (r, _, _) = run(with_data(ins(POP64, &[&[RA]]), b""));
    assert!(is_fault(&r, "stack underflow"), "{r:?}");
    let (r, _, _) = run(with_data(vec![RET], b""));
    assert!(is_fault(&r, "stack underflow"), "{r:?}");
}

/// builds `<setup>; <jump op> L; mov rb, 1; L: finish(rb)`; exit code 1 = jump not taken, 0 = taken
fn jump_probe(setup: Vec<u8>, jump_op: u8) -> RunResult {
    let build = |l: u32| {
        cat(vec![
            setup.clone(),
            mov_imm(RB, 0),
            ins(jump_op, &[&st4(l)]),
            mov_imm(RB, 1),
        ])
    };
    let l = build(0).len() as u32;
    let prog = cat(vec![build(l), finish_with_rb()]);
    run(with_data(prog, b"")).0
}

#[test]
fn cmp_sets_zero_flag_for_jz_jnz() {
    let eq = cat(vec![mov_imm(RA, 5), ins(CMP, &[&[RA], &imm(5)])]);
    let ne = cat(vec![mov_imm(RA, 5), ins(CMP, &[&[RA], &imm(6)])]);
    assert_eq!(jump_probe(eq.clone(), JZ), RunResult::Finished(0));
    assert_eq!(jump_probe(eq, JNZ), RunResult::Finished(1));
    assert_eq!(jump_probe(ne.clone(), JZ), RunResult::Finished(1));
    assert_eq!(jump_probe(ne, JNZ), RunResult::Finished(0));
}

#[test]
fn test_sets_zero_and_sign() {
    let zero = cat(vec![mov_imm(RA, 0), ins(TEST, &[&[RA]])]);
    let neg = cat(vec![mov_imm(RA, 1 << 63), ins(TEST, &[&[RA]])]);
    let pos = cat(vec![mov_imm(RA, 1), ins(TEST, &[&[RA]])]);
    assert_eq!(jump_probe(zero, JZ), RunResult::Finished(0));
    assert_eq!(jump_probe(neg.clone(), JS), RunResult::Finished(0));
    assert_eq!(jump_probe(neg, JZ), RunResult::Finished(1));
    assert_eq!(jump_probe(pos, JS), RunResult::Finished(1));
}

#[test]
fn lea_loads_effective_address() {
    let prog = cat(vec![
        mov_imm(RB, 100),
        mov_imm(RC, 5),
        ins(LEA, &[&[RA], &[BYTEPTR], &[RB], b"+", &imm(8), &[BYTEPTR]]),
        ins(LEA, &[&[RD], &[PTR], &[RB], b"-", &[RC], &[BYTEPTR]]),
        ins(LEA, &[&[RSP], &[WORDPTR], &imm(0x1234), &[WORDPTR]]),
    ]);
    let (r, rt, _) = run(with_data(prog, b""));
    assert_eq!(r, RunResult::EndOfProgram);
    assert_eq!(regs(&rt).ra, 108);
    assert_eq!(regs(&rt).rd, 95);
    assert_eq!(regs(&rt).rsp, 0x1234);
}

#[test]
fn runtime_can_move_after_load() {
    let build = |addr: u32| {
        cat(vec![
            ins(MOV, &[&[RB], &st4(addr)]),
            ins(MOV, &[&[RC], &imm(2)]),
            syscall(2),
        ])
    };
    let addr = build(0).len() as u32 + 1;
    let cap = CaptureHost::shared();
    let mut rt = ProgramRuntime::with_host(Box::new(cap.clone()));
    rt.load_from_vec(with_data(build(addr), b"ok"));
    // move it around (heap, vec) before running
    let boxed = Box::new(rt);
    let mut v = vec![*boxed];
    let rt = &mut v[0];
    assert_eq!(rt.run(), RunResult::EndOfProgram);
    assert_eq!(cap.borrow().out, b"ok\n");
}

mod common;
use common::{exit_code, run, run_limit};
use efescript::prelude::*;

/// Program that compares `a` with `b`, jumps with `jump` and exits with 1 if taken, 0 if not.
fn jump_program(jump: &str, a: u64, b: u64) -> String {
    format!(
        "mov r1, {a}\nmov r2, {b}\ncmp r1, r2\n{jump} _taken\n\
         mov ra, 5050\nmov rb, 0\nsyscall\n\
         label _taken\nmov ra, 5050\nmov rb, 1\nsyscall\n"
    )
}

#[test]
fn cmp_jumps_taken_and_not_taken() {
    // (jump, a, b, taken)
    let cases = [
        ("je", 3, 3, true), ("je", 3, 4, false),
        ("jne", 3, 4, true), ("jne", 3, 3, false),
        ("jgt", 4, 3, true), ("jgt", 3, 3, false), ("jgt", 3, 4, false),
        ("jlt", 3, 4, true), ("jlt", 3, 3, false), ("jlt", 4, 3, false),
        ("jge", 3, 3, true), ("jge", 4, 3, true), ("jge", 3, 4, false),
        ("jle", 3, 3, true), ("jle", 3, 4, true), ("jle", 4, 3, false),
    ];
    for (j, a, b, taken) in cases {
        assert_eq!(exit_code(&jump_program(j, a, b)), taken as u8, "{j} {a} {b}");
    }
}

#[test]
fn test_jumps_taken_and_not_taken() {
    let prog = |jump: &str, v: &str| {
        format!(
            "mov r1, {v}\ntest r1\n{jump} _t\nmov ra, 5050\nmov rb, 0\nsyscall\n\
             label _t\nmov ra, 5050\nmov rb, 1\nsyscall\n"
        )
    };
    assert_eq!(exit_code(&prog("jz", "0")), 1);
    assert_eq!(exit_code(&prog("jz", "5")), 0);
    assert_eq!(exit_code(&prog("jnz", "5")), 1);
    assert_eq!(exit_code(&prog("jnz", "0")), 0);
    assert_eq!(exit_code(&prog("js", "0x8000000000000000")), 1);
    assert_eq!(exit_code(&prog("js", "5")), 0);
}

#[test]
fn jump_through_register_and_label_immediate() {
    let src = "mov r1, _t\njmp r1\nmov ra, 5050\nmov rb, 9\nsyscall\n\
               label _t\nmov ra, 5050\nmov rb, 4\nsyscall\n";
    assert_eq!(exit_code(src), 4);
}

#[test]
fn loop_counts_to_n() {
    let src = "mov r1, 0\nlabel _l\nadd64 r1, 1\ncmp r1, 250\njne _l\n\
               mov ra, 5050\nmov rb, r1\nsyscall\n";
    assert_eq!(exit_code(src), 250);
    // a longer one printed as a number
    let src = "mov r1, 0\nlabel _l\nadd64 r1, 1\ncmp r1, 1000\njne _l\n\
               mov rb, r1\nmov rc, 0\nmov rd, 0\nmov ra, 3\nsyscall\nmov ra, 2\nsyscall\n";
    assert_eq!(run(src), (RunResult::EndOfProgram, "1000\n".to_string()));
}

#[test]
fn nested_calls() {
    let src = "mov r1, 0\ncall _a\n mov ra, 5050\nmov rb, r1\nsyscall\n\
               label _a\nadd64 r1, 1\ncall _b\nadd64 r1, 1\nret\n\
               label _b\nadd64 r1, 10\ncall _c\nadd64 r1, 10\nret\n\
               label _c\nadd64 r1, 100\nret\n";
    assert_eq!(exit_code(src), 122);
}

fn sum_program(n: u64) -> String {
    format!(
        "mov r1, {n}\ncall _sum\n\
         mov rb, r2\nmov rc, 0\nmov rd, 0\nmov ra, 3\nsyscall\nmov ra, 2\nsyscall\n\
         mov ra, 5050\nmov rb, 0\nsyscall\n\
         label _sum\ncmp r1, 0\njne _rec\nmov r2, 0\nret\n\
         label _rec\npush64 r1\nsub64 r1, 1\ncall _sum\npop64 r1\nadd64 r2, r1\nret\n"
    )
}

#[test]
fn recursion_depth_1000() {
    let (res, out) = run(&sum_program(1000));
    assert_eq!(res, RunResult::Finished(0));
    assert_eq!(out, "500500\n");
}

#[test]
fn recursion_depth_20000() {
    let (res, out) = run(&sum_program(20000));
    assert_eq!(res, RunResult::Finished(0));
    assert_eq!(out, "200010000\n");
}

#[test]
fn infinite_loop_hits_the_limit() {
    let (res, _) = run_limit("label _l\njmp _l\n", 10_000);
    assert_eq!(res, RunResult::LimitReached);
}

#[test]
fn ret_without_call_is_a_fault() {
    let (res, _) = run("ret\n");
    assert!(matches!(res, RunResult::Fault(_)), "{res:?}");
}

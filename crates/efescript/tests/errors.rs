mod common;
use common::run;
use efescript::nasm_efe::ParseErr;
use efescript::prelude::*;

fn err(src: &str) -> ParseErr {
    parse_str(src).expect_err("must not assemble")
}

#[test]
fn undefined_symbol() {
    match err("mov ra, 1\njmp nowhere\n") {
        ParseErr::UndefinedSymbol(name, pos) => {
            assert_eq!(name, "nowhere");
            assert_eq!(pos.line, 2);
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn undefined_data_symbol() {
    assert!(matches!(err("mov rb, nothing\n"), ParseErr::UndefinedSymbol(..)));
}

#[test]
fn duplicate_symbol() {
    match err("label a\nnop\nlabel a\n") {
        ParseErr::DuplicateSymbol(name, pos) => {
            assert_eq!(name, "a");
            assert_eq!(pos.line, 3);
        }
        e => panic!("{e:?}"),
    }
    // a label and a data name share one namespace
    assert!(matches!(err("a db 1\nlabel a\n"), ParseErr::DuplicateSymbol(..)));
}

#[test]
fn bad_operand() {
    let e = err("mov ra, ,\n");
    let pos = match &e {
        ParseErr::WrongToken(p)
        | ParseErr::WrongOperand(_, p)
        | ParseErr::Expected(_, _, p)
        | ParseErr::BadNumber(_, p)
        | ParseErr::WrongPtrInner(_, p) => p,
        e => panic!("{e:?}"),
    };
    assert_eq!(pos.line, 1);
}

#[test]
fn bad_number() {
    assert!(matches!(err("mov ra, 0xZZ\n"), ParseErr::BadNumber(..) | ParseErr::WrongOperand(..) | ParseErr::WrongToken(..)));
}

#[test]
fn unterminated_string() {
    match err("nop\nmsg db \"oops\n") {
        ParseErr::BadString(_, pos) => assert_eq!(pos.line, 2),
        e => panic!("{e:?}"),
    }
}

#[test]
fn unknown_mnemonic() {
    match err("nop\nfrobnicate ra, rb\n") {
        ParseErr::WrongOpCode(name, pos) => {
            assert_eq!(name, "frobnicate");
            assert_eq!(pos.line, 2);
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn missing_file_is_an_io_error() {
    assert!(matches!(parse_from_file("/definitely/not/here.efe"), Err(ParseErr::Io(_))));
}

#[test]
fn garbage_input_never_panics() {
    for src in ["", "\n\n", "mov", "mov ra", "[", "]]", "db", "label", "x:", "\"", "mov ra, [", ";", "call", "push64"] {
        let _ = parse_str(src); // Ok or Err, but no panic
    }
}

#[test]
fn unknown_syscall_is_a_fault() {
    for n in [255u64, 13, 99999] {
        let (res, out) = run(&format!("mov ra, {n}\nsyscall\n"));
        assert!(matches!(res, RunResult::Fault(_)), "{n}: {res:?}");
        assert_eq!(out, "");
    }
}

#[test]
fn heap_alloc_is_a_fault() {
    let (res, _) = run("mov ra, 9\nmov rb, 16\nsyscall\n");
    assert!(matches!(res, RunResult::Fault(_)), "{res:?}");
}

#[test]
fn bad_memory_and_stack_faults_do_not_panic() {
    // println of a huge length, pop on an empty stack, invalid Printchar
    for src in [
        "mov ra, 2\nmov rb, 0\nmov rc, 0xffffffffffff\nsyscall\n",
        "pop64 ra\n",
        "mov ra, 0\nmov rb, 0xd800\nsyscall\n",
    ] {
        let (res, _) = run(src);
        assert!(matches!(res, RunResult::Fault(_)), "{src}: {res:?}");
    }
}

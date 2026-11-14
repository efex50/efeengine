mod common;
use common::run;
use efescript::prelude::*;

#[test]
fn hello() {
    let (res, out) = run(include_str!("../examples/hello.efe"));
    assert_eq!(res, RunResult::EndOfProgram);
    assert_eq!(out, "hello world!\n");
}

#[test]
fn strings() {
    let (res, out) = run(include_str!("../examples/strings.efe"));
    assert_eq!(res, RunResult::EndOfProgram);
    assert_eq!(out, "hello\nh-e-l-l-o-\n104\nçay\n4\n");
}

#[test]
fn jumps() {
    let (res, out) = run(include_str!("../examples/jumps.efe"));
    assert_eq!(res, RunResult::Finished(0));
    assert!(!out.contains("FAIL"), "{out}");
    let expected: String = ["je", "jne", "jgt", "jlt", "jge", "jle", "jz", "jnz", "js"]
        .iter()
        .map(|j| format!("{j} taken: ok\n{j} not taken: ok\n"))
        .collect();
    assert_eq!(out, expected);
}

#[test]
fn recursion() {
    let (res, out) = run(include_str!("../examples/recursion.efe"));
    assert_eq!(res, RunResult::Finished(0));
    assert_eq!(out, "sum(10) = 55\nsum(100) = 5050\nfib(10) = 55\n");
}

#[test]
fn call() {
    let (res, out) = run(include_str!("../examples/call.efe"));
    assert_eq!(res, RunResult::Finished(5));
    assert_eq!(out, "in _test\nback in _main\n");
}

#[test]
fn heap_is_a_fault_not_a_panic() {
    let (res, out) = run(include_str!("../examples/heap.efe"));
    assert!(matches!(res, RunResult::Fault(_)), "{res:?}");
    assert_eq!(out, "");
}

#[test]
fn every_example_assembles() {
    for src in [
        include_str!("../examples/hello.efe"),
        include_str!("../examples/strings.efe"),
        include_str!("../examples/jumps.efe"),
        include_str!("../examples/recursion.efe"),
        include_str!("../examples/sound.efe"),
        include_str!("../examples/call.efe"),
        include_str!("../examples/test.efe"),
        include_str!("../examples/heap.efe"),
        include_str!("../examples/create_e.efe"),
    ] {
        assert!(parse_str(src).is_ok());
    }
}

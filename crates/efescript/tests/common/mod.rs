#![allow(dead_code)]
use efescript::prelude::*;

/// Assembles `src`, runs it with a capture host and returns the result and the output.
pub fn run(src: &str) -> (RunResult, String) {
    run_limit(src, 10_000_000)
}

pub fn run_limit(src: &str, limit: u64) -> (RunResult, String) {
    let cap = CaptureHost::shared();
    let mut rt = ProgramRuntime::with_host(Box::new(cap.clone()));
    rt.load_from_vec(parse_str(src).expect("program must assemble"));
    let res = rt.run_with_limit(limit);
    let out = cap.borrow().out_string();
    (res, out)
}

/// Runs a program and returns only the exit code (panics if it did not Finish).
pub fn exit_code(src: &str) -> u8 {
    match run(src).0 {
        RunResult::Finished(c) => c,
        other => panic!("expected Finished, got {other:?}"),
    }
}

use std::path::PathBuf;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_efescript");
const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn efescript(args: &[&str]) -> Output {
    Command::new(BIN).args(args).current_dir(ROOT).output().expect("spawn efescript")
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("efescript-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn runs_hello() {
    let o = efescript(&["examples/hello.efe"]);
    assert_eq!(String::from_utf8_lossy(&o.stdout), "hello world!\n");
    assert_eq!(o.status.code(), Some(0));
}

#[test]
fn missing_file_exits_2() {
    let o = efescript(&["examples/does_not_exist.efe"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
}

#[test]
fn assembly_error_exits_2() {
    let src = tmp("bad.efe");
    std::fs::write(&src, "jmp nowhere\n").unwrap();
    let o = efescript(&[src.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn output_file_roundtrip() {
    let out = tmp("hello.efec");
    let _ = std::fs::remove_file(&out);
    let o = efescript(&["examples/hello.efe", "-o", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty(), "-o must not run the program");
    assert!(out.exists());
    let o = efescript(&[out.to_str().unwrap()]);
    assert_eq!(String::from_utf8_lossy(&o.stdout), "hello world!\n");
    assert_eq!(o.status.code(), Some(0));
}

#[test]
fn finish_code_is_the_exit_code() {
    let src = tmp("seven.efe");
    std::fs::write(&src, "mov ra, 5050\nmov rb, 7\nsyscall\n").unwrap();
    let o = efescript(&[src.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(7));
}

#[test]
fn runtime_fault_exits_3() {
    let o = efescript(&["examples/heap.efe"]);
    assert_eq!(o.status.code(), Some(3));
}

#[test]
fn no_out_efec_is_written_to_the_working_directory() {
    let before = std::fs::metadata(PathBuf::from(ROOT).join("out.efec")).ok();
    let _ = efescript(&["examples/hello.efe"]);
    let after = std::fs::metadata(PathBuf::from(ROOT).join("out.efec")).ok();
    assert!(before.is_none() && after.is_none(), "out.efec must not be created in the crate dir");
}

mod common;
use common::run;
use efescript::prelude::*;

fn println_of(name: &str) -> String {
    format!("mov ra, 2\nmov rb, {name}\nmov rc, {name}.len\nsyscall\n")
}

#[test]
fn escapes_and_raw_bytes_print_exactly() {
    let src = format!(
        "m db \"a\\tb\\\\c\\\"d\\x41\\0\", 10, 0x42, 0b101000, 0o103\n{}",
        println_of("m")
    );
    let (res, out) = run(&src);
    assert_eq!(res, RunResult::EndOfProgram);
    // a TAB b \ c " d A NUL LF B ( C  + Println newline
    assert_eq!(out, "a\tb\\c\"dA\0\nB(C\n");
}

#[test]
fn newline_escape() {
    let src = format!("m db \"x\\ny\"\n{}", println_of("m"));
    assert_eq!(run(&src).1, "x\ny\n");
}

#[test]
fn len_is_the_byte_length_for_utf8() {
    // "çay" is 4 bytes; print the length as a number
    let src = "m db \"çay\"\nmov rb, m.len\nmov rc, 0\nmov rd, 0\nmov ra, 3\nsyscall\nmov ra, 2\nsyscall\n";
    assert_eq!(run(src).1, "4\n");
    // and the string itself comes out byte-exact
    let src = format!("m db \"çay\"\n{}", println_of("m"));
    assert_eq!(run(&src).1, "çay\n");
}

#[test]
fn strings_before_and_after_the_code() {
    let src = format!(
        "first db \"one\"\n{}{}second db \"two!\"\n",
        println_of("first"),
        println_of("second")
    );
    assert_eq!(run(&src), (RunResult::EndOfProgram, "one\ntwo!\n".to_string()));
}

#[test]
fn byte_access_through_pointers() {
    // [msg], [msg+reg], [reg+msg] read single bytes
    let src = "m db \"AbC\"\n\
        mov rb, 0\nmov bl, BYTEPTR [m]\nmov ra, 0\nsyscall\n\
        mov r1, 2\nmov rb, 0\nmov bl, BYTEPTR [m+r1]\nmov ra, 0\nsyscall\n\
        mov rb, 0\nmov bl, BYTEPTR [r1+m]\nmov ra, 0\nsyscall\n";
    assert_eq!(run(src).1, "ACC");
}

#[test]
fn data_is_not_executed_when_the_code_ends() {
    // the data bytes would be valid-looking opcodes; the zero marker stops the run first
    let src = "mov ra, 0\nmov rb, 65\nsyscall\nd db 1, 2, 3, 4, 5, 6, 7, 8\n";
    assert_eq!(run(src), (RunResult::EndOfProgram, "A".to_string()));
}

#[test]
fn data_placed_in_the_middle_does_not_move_code() {
    let src = format!("{}d db \"mid\"\n{}", println_of("d"), println_of("d"));
    assert_eq!(run(&src).1, "mid\nmid\n");
}

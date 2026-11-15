use efescript::host_std::StdEnv;
use efescript::prelude::*;

const USAGE: &str = r#"usage: efescript <file.efe|file.efec> [-o out] [-d] [-r]
       efescript run   <file.efa|file.efvm> [--budget N]
       efescript build <file.efa> -o <out.efvm>
       efescript dis   <file.efvm|file.efa> [-o out.efa]
       efescript <file.efa> [--budget N]      (shorthand for run)

  v2 programs: run exits with main's code or the std.exit code, 2 for a load or
  assembly error, 3 for a trap or an exhausted --budget. The OS takes exit codes
  modulo 256.

  efescript prog.efe            assemble and run
  efescript prog.efec           run precompiled bytes
  efescript prog.efe -o out     assemble to the file `out`, do not run
  -d    print the runtime state after the run
  -r    print the memory usage after the run

exit code: the Finish code of the program (0 at the end of the program),
2 for a missing file or an assembly error, 3 for a runtime error
"#;

fn fail(msg: String) -> ! {
    eprintln!("efescript: {msg}");
    std::process::exit(2);
}

fn v2_fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

/// Splits `args` into the positional file, `-o` and `--budget`.
fn v2_args(args: &[String], allow_budget: bool, allow_out: bool) -> (Option<String>, Option<String>, Option<u64>) {
    let (mut file, mut out, mut budget) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" if allow_out => match it.next() {
                Some(o) => out = Some(o.clone()),
                None => v2_fail("error: -o needs a file name"),
            },
            "--budget" if allow_budget => match it.next().and_then(|n| n.parse::<u64>().ok()) {
                Some(n) => budget = Some(n),
                None => v2_fail("error: --budget needs a number"),
            },
            _ if a.starts_with('-') && a.len() > 1 => v2_fail(format!("error: unknown or misplaced option `{a}`")),
            _ if file.is_none() => file = Some(a.clone()),
            _ => v2_fail(format!("error: unexpected argument `{a}`")),
        }
    }
    (file, out, budget)
}

fn v2_run(args: &[String]) -> ! {
    use efescript::run_v2;
    let (file, _, budget) = v2_args(args, true, false);
    let Some(file) = file else { v2_fail("error: run needs a file (.efa or .efvm)") };
    let prog = run_v2::load_path(&file).unwrap_or_else(|e| v2_fail(e));
    let mut env = StdEnv::with_stdin(StdHost);
    let outcome = run_v2::run(&prog, &mut env, budget);
    {
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    if let Some(e) = outcome.error {
        eprintln!("{e}");
    }
    std::process::exit(outcome.exit_code);
}

fn v2_build(args: &[String]) -> ! {
    use efescript::run_v2;
    let (file, out, _) = v2_args(args, false, true);
    let Some(file) = file else { v2_fail("error: build needs a .efa file") };
    let Some(out) = out else { v2_fail("error: build needs -o <out.efvm>") };
    let prog = run_v2::load_path(&file).unwrap_or_else(|e| v2_fail(e));
    std::fs::write(&out, prog.module.encode()).unwrap_or_else(|e| v2_fail(format!("error: {out}: {e}")));
    std::process::exit(0);
}

fn v2_dis(args: &[String]) -> ! {
    use efescript::run_v2;
    let (file, out, _) = v2_args(args, false, true);
    let Some(file) = file else { v2_fail("error: dis needs a .efvm or .efa file") };
    let prog = run_v2::load_path(&file).unwrap_or_else(|e| v2_fail(e));
    let text = efescript::disasm::disassemble(&prog.module);
    match out {
        Some(o) => std::fs::write(&o, text).unwrap_or_else(|e| v2_fail(format!("error: {o}: {e}"))),
        None => {
            use std::io::Write;
            let mut so = std::io::stdout();
            let _ = so.write_all(text.as_bytes());
            let _ = so.flush();
        }
    }
    std::process::exit(0);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("run") => v2_run(&argv[1..]),
        Some("build") => v2_build(&argv[1..]),
        Some("dis") => v2_dis(&argv[1..]),
        Some(f) if f.ends_with(".efa") => v2_run(&argv),
        _ => {}
    }

    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    let (mut dump, mut ram) = (false, false);

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-d" => dump = true,
            "-r" => ram = true,
            "-o" => match args.next() {
                Some(o) => out = Some(o),
                None => fail("-o needs a file name".to_string()),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ if file.is_none() => file = Some(a),
            _ => fail(format!("unexpected argument `{a}`")),
        }
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        std::process::exit(1);
    };

    let raw = std::fs::read(&file).unwrap_or_else(|e| fail(format!("{file}: {e}")));
    let bytes = if file.ends_with(".efec") {
        raw
    } else {
        let src = String::from_utf8(raw).unwrap_or_else(|_| fail(format!("{file}: not valid UTF-8")));
        parse_str(src).unwrap_or_else(|e| fail(format!("{file}: {e:?}")))
    };

    if let Some(o) = out {
        std::fs::write(&o, &bytes).unwrap_or_else(|e| fail(format!("{o}: {e}")));
        return;
    }

    let mut p = ProgramRuntime::new();
    p.load_from_vec(bytes);
    let result = p.run();
    if dump {
        println!("{:?}", p);
    }
    if ram {
        println!("ram usage :{:?} bytes", p.program.page.len() * efepages::vars::PAGE_SIZE);
    }
    let code = match result {
        RunResult::Finished(c) => c as i32,
        RunResult::EndOfProgram | RunResult::LimitReached => 0,
        RunResult::Fault(_) => 3,
    };
    std::process::exit(code);
}

#[allow(warnings)]
mod tests{
    use std::io::{stdout, BufWriter, Write};

    #[test]
    fn _flush(){
        let mut b = BufWriter::new(stdout());
        b.write("sa".as_bytes());
        println!("sa yazıldı");
        b.write("as".as_bytes());
        println!("as yazıldı");
        println!("b flush'landı");
        write!(b,"lan");
        b.write("\n".as_bytes());
        b.flush().unwrap();
    }


    #[test]
    fn vecflush(){
        let a = vec![b'a';10];
        let mut buf = BufWriter::new(stdout());
        buf.write(&a);
        buf.flush();
    }

}
#[allow(warnings)]
mod grok{
    use std::io::{self, Write};

    #[test]
    fn grok() -> io::Result<()> {
        let mut buffer = io::BufWriter::new(io::stdout());

        // Write some data to the buffer
        writeln!(buffer, "Hello, world!")?;

        // Flush the buffer to ensure the "Hello, world!" is actually printed
        buffer.flush()?;

        let a =10;


        Ok(())
    }


}
use efescript::asm::AsmOptions;
use efescript::host_std::{std_registry, StdEnv, StdOptions};
use efescript::prelude::StdHost;
use efevm::api::ApiIndex;

const USAGE: &str = r#"usage: efescript run   <file.efa|file.efvm> [--budget N] [--api FILE.efapi] [--fs-root DIR]
       efescript build <file.efa> -o <out.efvm> [--api FILE.efapi]
       efescript dis   <file.efvm|file.efa> [-o out.efa]
       efescript bundle <file.efa|file.efvm> -o <out> [--runner PATH]
       efescript api   <file.efapi>
       efescript api-export [-o FILE.efapi]
       efescript lsp   [--api FILE.efapi]     (language server on stdin/stdout)
       efescript debug <file.efa|file.efvm> [--api FILE.efapi] [--fs-root DIR]
                                             (debugger; commands on stdin, `help` lists them)
       efescript <file.efa> [--budget N]      (shorthand for run)

  v2 programs: run exits with main's code or the std.exit code, 2 for a load or
  assembly error, 3 for a trap or an exhausted --budget. The OS takes exit codes
  modulo 256.

  --api FILE.efapi assembles against an API index (the file a game exports, see
  `api-export`): imports are checked by name and signature, `import ns.name` may leave
  out the signature, struct constants (Transform.size, Transform.pos, ...) are defined,
  and the module records the index hash and namespace versions. `run` links against the
  std host only, so it can run such a module only when it imports nothing but std.

  --fs-root DIR lets std.fs read and write files below DIR (default: std.fs is denied).

  api prints the namespaces, types and functions of an index file (exit 2 when it
  cannot be read). api-export writes the index of the std host (default: stdout).

  bundle writes a standalone executable: a copy of the runner with the compiled
  module appended. The runner is --runner PATH, else efescript-runner next to this
  executable, else this executable. An executable with an embedded module runs it
  and ignores the command line.
"#;

fn fail(msg: String) -> ! {
    eprintln!("efescript: {msg}");
    std::process::exit(2);
}

fn v2_fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

/// The options of `run`, `build`, `dis` and `api-export`.
#[derive(Default)]
struct V2Args {
    file: Option<String>,
    out: Option<String>,
    budget: Option<u64>,
    api: Option<String>,
    fs_root: Option<String>,
}

/// Which options a subcommand accepts.
#[derive(Clone, Copy, Default)]
struct Allow {
    budget: bool,
    out: bool,
    api: bool,
    fs_root: bool,
}

fn v2_args(args: &[String], allow: Allow) -> V2Args {
    let mut r = V2Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" if allow.out => match it.next() {
                Some(o) => r.out = Some(o.clone()),
                None => v2_fail("error: -o needs a file name"),
            },
            "--budget" if allow.budget => match it.next().and_then(|n| n.parse::<u64>().ok()) {
                Some(n) => r.budget = Some(n),
                None => v2_fail("error: --budget needs a number"),
            },
            "--api" if allow.api => match it.next() {
                Some(f) => r.api = Some(f.clone()),
                None => v2_fail("error: --api needs a .efapi file"),
            },
            "--fs-root" if allow.fs_root => match it.next() {
                Some(d) => r.fs_root = Some(d.clone()),
                None => v2_fail("error: --fs-root needs a directory"),
            },
            _ if a.starts_with('-') && a.len() > 1 => v2_fail(format!("error: unknown or misplaced option `{a}`")),
            _ if r.file.is_none() => r.file = Some(a.clone()),
            _ => v2_fail(format!("error: unexpected argument `{a}`")),
        }
    }
    r
}

/// Reads and checks an `.efapi` file; exits 2 with a message on failure.
fn read_index(path: &str) -> ApiIndex {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| v2_fail(format!("error: {path}: {e}")));
    ApiIndex::from_json(&text).unwrap_or_else(|e| v2_fail(format!("error: {path}: {e}")))
}

fn v2_run(args: &[String]) -> ! {
    use efescript::run_v2;
    let a = v2_args(args, Allow { budget: true, api: true, fs_root: true, ..Allow::default() });
    let Some(file) = a.file else { v2_fail("error: run needs a file (.efa or .efvm)") };
    let index = a.api.as_deref().map(read_index);
    let prog = run_v2::load_path_with(&file, &AsmOptions { api: index.as_ref() }).unwrap_or_else(|e| v2_fail(e));
    let opts = StdOptions { fs_root: a.fs_root.map(Into::into) };
    let mut env = StdEnv::with_stdin(StdHost);
    let outcome = run_v2::run_with(&prog, &mut env, a.budget, &opts);
    {
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    if let Some(e) = outcome.error {
        eprintln!("{e}");
    }
    std::process::exit(outcome.exit_code);
}

/// `efescript debug`: the line debugger over the std host. Debugger commands come from
/// stdin, so the program's `std.read_line` sees end of input.
fn v2_debug(args: &[String]) -> ! {
    use efescript::{debugger::Debugger, run_v2};
    let a = v2_args(args, Allow { api: true, fs_root: true, ..Allow::default() });
    let Some(file) = a.file else { v2_fail("error: debug needs a file (.efa or .efvm)") };
    let index = a.api.as_deref().map(read_index);
    let prog = run_v2::load_path_with(&file, &AsmOptions { api: index.as_ref() }).unwrap_or_else(|e| v2_fail(e));
    let opts = StdOptions { fs_root: a.fs_root.map(Into::into) };
    let reg = std_registry::<StdHost>(&opts);
    // Show host calls by their index names: the given index, else the std one.
    let shown = index.unwrap_or_else(|| reg.index());
    let mut dbg = Debugger::new(prog.module.clone(), &reg.imports(), "main", Some(shown)).unwrap_or_else(|e| v2_fail(format!("error: {e}")));
    if let Some((name, text)) = &prog.source {
        dbg.add_source(name, text);
    }
    let mut env = StdEnv::new(StdHost);
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut out = std::io::stdout();
    if let Err(e) = dbg.run_script(&mut env, &mut input, &mut out) {
        v2_fail(format!("error: {e}"));
    }
    std::process::exit(0);
}

fn v2_build(args: &[String]) -> ! {
    use efescript::run_v2;
    let a = v2_args(args, Allow { out: true, api: true, ..Allow::default() });
    let Some(file) = a.file else { v2_fail("error: build needs a .efa file") };
    let Some(out) = a.out else { v2_fail("error: build needs -o <out.efvm>") };
    let index = a.api.as_deref().map(read_index);
    let prog = run_v2::load_path_with(&file, &AsmOptions { api: index.as_ref() }).unwrap_or_else(|e| v2_fail(e));
    std::fs::write(&out, prog.module.encode()).unwrap_or_else(|e| v2_fail(format!("error: {out}: {e}")));
    std::process::exit(0);
}

/// The text `efescript api` prints.
fn api_text(ix: &ApiIndex) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    let game = if ix.game.is_empty() { "(unnamed)" } else { ix.game.as_str() };
    let _ = writeln!(o, "API index of {game}, hash fnv1a64:{:016x}", ix.hash());
    let _ = writeln!(o, "\nnamespaces:");
    for n in &ix.namespaces {
        let _ = writeln!(o, "  {} v{} (owner {})", n.name, n.version, n.owner);
        if !n.doc.is_empty() {
            let _ = writeln!(o, "      {}", n.doc);
        }
    }
    let _ = writeln!(o, "\ntypes:");
    for t in &ix.types {
        let comp = t.component.map(|c| format!(", component id {c}")).unwrap_or_default();
        let _ = writeln!(o, "  {} (size {}, align {}{comp}, owner {})", t.name, t.size, t.align, t.owner);
        if !t.doc.is_empty() {
            let _ = writeln!(o, "      {}", t.doc);
        }
        for f in &t.fields {
            let count = if f.count > 1 { format!(" x{}", f.count) } else { String::new() };
            let doc = if f.doc.is_empty() { String::new() } else { format!("  {}", f.doc) };
            let _ = writeln!(o, "      +{:<3} {}: {}{count}{doc}", f.offset, f.name, f.ty);
        }
    }
    let _ = writeln!(o, "\nfunctions:");
    for f in &ix.functions {
        let _ = writeln!(o, "  {}{}", f.qualified(), f.signature_text());
        let _ = writeln!(o, "      {}", f.doc);
    }
    o
}

fn v2_api(args: &[String]) -> ! {
    let a = v2_args(args, Allow::default());
    let Some(file) = a.file else { v2_fail("error: api needs a .efapi file") };
    let ix = read_index(&file);
    write_stdout(api_text(&ix).as_bytes());
    std::process::exit(0);
}

fn v2_api_export(args: &[String]) -> ! {
    let a = v2_args(args, Allow { out: true, ..Allow::default() });
    if let Some(extra) = a.file {
        v2_fail(format!("error: unexpected argument `{extra}`"));
    }
    let json = std_registry::<StdHost>(&StdOptions::default()).index().to_json();
    match a.out {
        Some(o) => std::fs::write(&o, json).unwrap_or_else(|e| v2_fail(format!("error: {o}: {e}"))),
        None => write_stdout(json.as_bytes()),
    }
    std::process::exit(0);
}

fn write_stdout(bytes: &[u8]) {
    use std::io::Write;
    let mut so = std::io::stdout();
    let _ = so.write_all(bytes);
    let _ = so.flush();
}

fn v2_dis(args: &[String]) -> ! {
    use efescript::run_v2;
    let a = v2_args(args, Allow { out: true, ..Allow::default() });
    let (file, out) = (a.file, a.out);
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

fn v2_bundle(args: &[String]) -> ! {
    use efescript::{bundle, run_v2};
    let (mut file, mut out, mut runner) = (None::<String>, None::<String>, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => match it.next() {
                Some(o) => out = Some(o.clone()),
                None => v2_fail("error: -o needs a file name"),
            },
            "--runner" => match it.next() {
                Some(r) => runner = Some(r.clone()),
                None => v2_fail("error: --runner needs a path"),
            },
            _ if a.starts_with('-') && a.len() > 1 => v2_fail(format!("error: unknown or misplaced option `{a}`")),
            _ if file.is_none() => file = Some(a.clone()),
            _ => v2_fail(format!("error: unexpected argument `{a}`")),
        }
    }
    let Some(file) = file else { v2_fail(format!("error: bundle needs a .efa or .efvm file\n{USAGE}")) };
    let Some(out) = out else { v2_fail(format!("error: bundle needs -o <out>\n{USAGE}")) };
    let prog = run_v2::load_path(&file).unwrap_or_else(|e| v2_fail(e));
    let module = prog.module.encode();
    let runner_path: std::path::PathBuf = match runner {
        Some(r) => r.into(),
        None => {
            let me = std::env::current_exe().unwrap_or_else(|e| v2_fail(format!("error: cannot locate the executable: {e}")));
            let name = format!("efescript-runner{}", std::env::consts::EXE_SUFFIX);
            match me.parent().map(|d| d.join(&name)) {
                Some(c) if c.is_file() => c,
                _ => me,
            }
        }
    };
    let runner_len = std::fs::read(&runner_path)
        .map(|b| bundle::strip_payload(&b).len())
        .unwrap_or_else(|e| v2_fail(format!("error: {}: {e}", runner_path.display())));
    bundle::write_bundle(&runner_path, &module, std::path::Path::new(&out)).unwrap_or_else(|e| v2_fail(e));
    eprintln!("bundled {out} (runner {runner_len} bytes + module {} bytes)", module.len());
    std::process::exit(0);
}

fn main() {
    if let Some(code) = efescript::run_v2::run_embedded() {
        std::process::exit(code);
    }
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("run") => v2_run(&argv[1..]),
        Some("build") => v2_build(&argv[1..]),
        Some("dis") => v2_dis(&argv[1..]),
        Some("api") => v2_api(&argv[1..]),
        Some("debug") => v2_debug(&argv[1..]),
        Some("api-export") => v2_api_export(&argv[1..]),
        Some("bundle") => v2_bundle(&argv[1..]),
        Some("lsp") => {
            let a = v2_args(&argv[1..], Allow { api: true, ..Allow::default() });
            if let Some(extra) = a.file {
                v2_fail(format!("error: unexpected argument `{extra}`"));
            }
            std::process::exit(efescript::lsp::run_stdio_code(a.api.map(Into::into)));
        }
        Some(f) if f.ends_with(".efa") => v2_run(&argv),
        _ => {}
    }

    match argv.first() {
        Some(f) if f.ends_with(".efe") || f.ends_with(".efec") => {
            eprintln!("efescript: the legacy .efe/.efec format was removed; use .efa (see docs/efa-syntax.md)");
            std::process::exit(2);
        }
        Some(f) if f == "-h" || f == "--help" => {
            println!("{USAGE}");
            return;
        }
        Some(f) => fail(format!("unknown command or file type `{f}`\n{USAGE}")),
        None => {
            eprintln!("{USAGE}");
            std::process::exit(1);
        }
    }
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
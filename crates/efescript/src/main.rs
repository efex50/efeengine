use efescript::prelude::*;

const USAGE: &str = r#"usage: efescript <file.efe|file.efec> [-o out] [-d] [-r]

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

fn main() {
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
//! The efescript v2 assembler: `.efa` text to an `efevm` module.
//!
//! The language is specified in `crates/efescript/docs/efa-syntax.md` (built on
//! `docs/e2-design.md`). Pipeline: [`lexer::lex`] -> `parser::parse` -> `lower::lower`.
//! Lexer and parser diagnostics are merged (sorted by position); lowering only runs when there
//! are none.

pub mod ast;
pub mod diag;
pub mod lexer;
mod lower;
mod parser;

pub use diag::{Diagnostic, Diagnostics};

fn sort_diags(d: &mut [Diagnostic]) {
    d.sort_by_key(|x| (x.line, x.column));
}

/// At most this many diagnostics are returned; the last one says that more were found.
const MAX_DIAGS: usize = 100;

fn cap_diags(d: &mut Vec<Diagnostic>) {
    if d.len() > MAX_DIAGS {
        let file = d.first().map(|x| x.file.clone()).unwrap_or_default();
        d.truncate(MAX_DIAGS - 1);
        d.push(Diagnostic::without_location(
            &file,
            format!("too many errors; stopping after {MAX_DIAGS}"),
        ));
    }
}

/// Parse only (for tools). Returns the lexer and parser diagnostics, sorted by position, on error.
pub fn parse(src: &str, file: &str) -> Result<ast::Module, Diagnostics> {
    let (tokens, mut diags) = lexer::lex(src, file);
    let (module, parse_diags) = parser::parse(&tokens, src, file);
    diags.extend(parse_diags);
    if diags.is_empty() {
        Ok(module)
    } else {
        sort_diags(&mut diags);
        cap_diags(&mut diags);
        Err(Diagnostics(diags))
    }
}

/// Assemble `.efa` source to a verified module. `file` is used in diagnostics and the lines section.
pub fn assemble(src: &str, file: &str) -> Result<efevm::Module, Diagnostics> {
    let module = parse(src, file)?;
    match lower::lower(&module, src, file) {
        Ok(m) => Ok(m),
        Err(mut diags) => {
            sort_diags(&mut diags);
            cap_diags(&mut diags);
            Err(Diagnostics(diags))
        }
    }
}

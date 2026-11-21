# calc

`calc` is an expression front end: a char-based lexer producing spanned tokens, a Pratt parser that accepts tokens from any `TokenSource`, an infix printer, an evaluator with checked `i128` arithmetic, a constant folder, and fixed-width narrowing helpers for a VM. It is used by `efa` for constant expressions in `.efa` assembly and also ships an optional `cli` binary.

## At a glance
| | |
|---|---|
| Path | `crates/calc` |
| Kind | library (`calc` 1.1.0, edition 2024) + binary `cli` (`src/bin/cli.rs`, requires feature `cli`) |
| Depends on (workspace) | none |
| Used by | `efa` (`crates/efa/Cargo.toml:11`, `default-features = false`, constant expression parsing & evaluation in `asm::parser`, `asm::ast`, `asm::lower::consts`); tested by `xtask` |
| Key external deps | `clap` 4.6.8 (`derive`, optional, CLI only) |
| Features | `default = ["cli"]`; `cli = ["dep:clap"]` |
| Targets | Platform-independent library (used on native and `wasm32-unknown-unknown` via `efa` with `default-features = false`); `cli` binary is native |
| Tests | 24 integration tests in `tests/calc.rs` + 1 doc test in `src/lib.rs` |

## Layout
```text
crates/calc/
├── Cargo.toml          # package `calc` (1.1.0, edition 2024), optional `clap` 4.6.8, `cli` feature & bin
├── README.md           # quick usage overview and CLI example
├── tests/calc.rs       # 24 integration tests covering lexing, parsing, eval, simplify, narrowing, printing
└── src/
    ├── lib.rs          # re-exports public items + `parse_value()` + 1 doc test
    ├── error.rs        # `Span`, `CalcError`, `CalcErrorKind` (`Display` + `std::error::Error`, `render(src)`)
    ├── lexer.rs        # `Token`, `TokenKind`, `TokenSource`, `TokenIter`, `Lexer`, `lex()`
    ├── ast.rs          # `Expr`, `ExprKind`, `UnOp`, `BinOp` (+ `precedence`, `symbol`), `Display` (infix printer)
    ├── parser.rs       # `Parser` (Pratt, depth limit 200), `parse()`, `parse_tokens()`
    ├── eval.rs         # `Value`, `IntWidth`, `Env`, `NoEnv`, `Vars`, `eval()`, `simplify()`
    └── bin/cli.rs      # `cli` binary (`-s/--string`, `-v/--var`, `--simplify`, `--print`)
```

## Public API
Errors (`error.rs`):
- `Span { pub start: usize, pub end: usize }` (byte offsets; `Span::new`, `to`).
- `CalcError { pub kind: CalcErrorKind, pub span: Span }` (`new`, `with_span`, `render(&str) -> String`, `Display`, `std::error::Error`).
- `CalcErrorKind`: `UnexpectedToken(String)`, `UnexpectedEnd`, `UnclosedBracket` (span = opening bracket), `InvalidNumber(String)`, `UnknownVariable(String)`, `UnknownFunction(String)`, `DivisionByZero`, `Overflow`, `TypeMismatch(String)`, `TooDeep` (nesting over `MAX_DEPTH = 200`).

Lexing (`lexer.rs`):
- `lex(&str) -> Result<Vec<Token>, CalcError>`; `Token { pub kind: TokenKind, pub span: Span }`; `Token::new(kind, start, end)`.
- `TokenKind`: `Int(i128)`, `Float(f64)`, `Ident(String)`, `Plus`, `Minus`, `Star`, `Slash`, `Percent`, `Shl`, `Shr`, `Amp`, `Pipe`, `Caret`, `Tilde`, `Bang`, `EqEq`, `NotEq`, `Lt`, `Le`, `Gt`, `Ge`, `AndAnd`, `OrOr`, `LParen`, `RParen`, `Comma`, `Dot`; `TokenKind::describe(&self) -> String`.
- `trait TokenSource { fn next_token(&mut self) -> Result<Option<Token>, CalcError>; }`, implemented by `Lexer<'a>` (`Lexer::new`) and `TokenIter<I: Iterator<Item = Token>>`.
- Literals: `0x`/`0b`/`0o` (either case), `_` separators, decimal, `1.5`, `1e3`/`1.5E-2`; `12abc` and out-of-range/non-finite numbers produce `InvalidNumber` covering the whole literal. `.` is its own token (`1.x` lexes as `Int`, `Dot`, `Ident`). Negative literals are parsed as unary minus on a positive literal.

Parsing (`parser.rs`):
- `parse(&str) -> Result<Expr, CalcError>` and `parse_tokens(&[Token]) -> Result<Expr, CalcError>` (both reject trailing tokens via `finish()`).
- `Parser::new(&mut dyn TokenSource)` with `parse_expr()`, `peek()`, `finish()`: parse one expression and stop at the first token that cannot continue it (used by `efa` for comma-separated operands).
- Precedence, tightest first: postfix (`.field`, `name(args)`), unary `- ! ~`, `* / %`, `+ -`, `<< >>`, `&`, `^`, `|`, comparisons (`== != < <= > >=`), `&&`, `||`. All binary operators are left associative (`a < b < c` parses as `(a < b) < c` and fails type checking). Calls are only allowed on a plain `Var` name.
- `true`/`false` parse as `ExprKind::Bool`.

AST (`ast.rs`):
- `Expr { pub kind: ExprKind, pub span: Span }`; `PartialEq` compares structure and ignores spans.
- `ExprKind`: `Int(i128)`, `Float(f64)`, `Bool(bool)`, `Var(String)`, `Unary(UnOp, Box<Expr>)`, `Binary(BinOp, Box<Expr>, Box<Expr>)`, `Field(Box<Expr>, String)`, `Call(String, Vec<Expr>)`, `Paren(Box<Expr>)`.
- `Expr::dotted_path()` returns `Some("a.b.c")` for a `Var` followed by `Field`s.
- `UnOp` (`Neg`, `Not`, `BitNot`) and `BinOp` (`Add`, `Sub`, `Mul`, `Div`, `Rem`, `Shl`, `Shr`, `BitAnd`, `BitOr`, `BitXor`, `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`, `And`, `Or`, with `precedence()` and `symbol()`).
- `impl Display for Expr` is the infix printer; `parse(&e.to_string()) == Ok(e)` holds. Parentheses already in the tree (`Paren`) are preserved; hand-built trees receive the parentheses needed by precedence (including negative literals in postfix position).

Evaluation (`eval.rs`, `lib.rs`):
- `Value::{Int(i128), Float(f64), Bool(bool)}` (`From` for `i128`, `i64`, `i32`, `u64`, `f64`, `bool`; `Display`, `to_expr`, `type_name`).
- `trait Env { fn var(&self, &str) -> Option<Value>; fn call(&self, &str, &[Value]) -> Option<Result<Value, CalcError>>; }`. `a.b.c` is looked up as one name via `var("a.b.c")`. Errors returned from `call` get the call node's span. Implementations: `NoEnv` and `Vars { pub map: HashMap<String, Value> }` (`new`, `set`, `unset`).
- `eval(&Expr, &dyn Env) -> Result<Value, CalcError>`: checked `+ - * / %` and negation (`Overflow`), `/` and `%` by zero (`DivisionByZero`, also for floats), shift amount outside `0..128` (`Overflow`), int/float promotion to `f64`, `% << >> & | ^ ~` ints only, comparisons give `Bool` (`==`/`!=` also work on two bools), `&& || !` require bools (`&&`/`||` short-circuit), anything else `TypeMismatch`.
- `simplify(&Expr, &dyn Env) -> Result<Expr, CalcError>`: folds constant subtrees, substitutes known variables, calls `Env::call` when all arguments are constant (so functions are assumed pure), keeps unknown names. Identities: `x+0`, `0+x`, `x-0`, `x*1`, `1*x`, `x/1`, `x*0`, `0*x` (with an integer literal), `false && _`, `true || _`. No reassociation (`x + 1 + 2` stays as is; `x + (1 + 2)` becomes `x + 3`).
- `IntWidth::{I8, I16, I32, I64, U8, U16, U32, U64}` (`bits`, `signed`, `range`, `from_name`); `Value::to_int_of(width) -> Result<i128, CalcError>` (`Overflow` out of range, `TypeMismatch` for float/bool; errors carry `Span::default()`, attach a span with `CalcError::with_span`); `Value::to_f32` (`Overflow` if a finite value becomes non-finite) and `Value::to_f64`.
- `parse_value(&str) -> Result<Value, CalcError>`: parse and evaluate with `NoEnv`.

## CLI (`src/bin/cli.rs`)
`-s/--string EXPR` (or piped stdin), `-v/--var name=value` (repeatable, value parsed with `parse_value`), `--simplify` (fold constants and print infix), `--print infix|debug` (print the parsed/simplified expression instead of evaluating, repeatable). Default: evaluate and print the value. CLI/var/stdin errors exit 2; expression parse/simplify/eval errors print `CalcError::render` to stderr and exit 1.

## Dropped from the old API
`Tree`, `TreeType`, `TreeTokens`, `Number`, `Symbols`, `FixType`, prefix/postfix parsing and printing, notation auto-detection, `util::split_str`, `util::as_i128`, and the placeholder `add()`. Prefix/postfix were removed because the current grammar (unary operators, comparisons, calls) has no unambiguous prefix or postfix form worth maintaining.

## Tests
`cargo test -p calc`: 24 integration tests in `tests/calc.rs` plus 1 doc test in `src/lib.rs`.

# calc

`calc` is an arithmetic expression library: it tokenizes a string, parses it (infix, prefix or postfix notation) into a binary tree of `TreeType` nodes, prints the tree back in any of those notations, and can substitute variables and fold constants. Its README describes it as a library and client for efescript, but no workspace crate depends on it. It ships an optional `cli` binary that wraps the library.

## At a glance
| | |
|---|---|
| Path | `crates/calc` |
| Kind | library (+ binary `cli` behind the `cli` feature) |
| Depends on (workspace) | none |
| Used by | no workspace crate (checked: no `Cargo.toml` in `crates/` or `examples/` depends on `calc`). README says "client for efescript", but `crates/efescript/Cargo.toml` has no such dependency. |
| Key external deps | `clap` 4.5 with `derive` (optional; only for the CLI argument parser) |
| Targets | Native. Only `std` is used by the library, so it should build for other targets; not checked. The CLI reads stdin via `IsTerminal`. |
| Features | `default = ["cli"]`; `cli = ["dep:clap"]`. The `cli` bin has `required-features = ["cli"]`. |

## Layout
```text
crates/calc/
├── Cargo.toml          # package `calc` 1.1.0, edition 2024; optional clap; bin `cli`
├── README.md           # one line: "calc - a mathematical expression evaluater - parser - simplifier library and client for efescript"
└── src/
    ├── lib.rs          # `pub mod tree; pub mod types; pub mod util;` + placeholder `add()` and its test
    ├── types.rs        # TreeTokens (lexer tokens), Number (i128 | f64) with arithmetic, Symbols, TreeType (tree nodes), FixType
    ├── util.rs         # as_i128 (radix-prefixed integer parsing), check_fix_type (notation detection), split_str (tokenizer splitter)
    ├── tree.rs         # Tree: variables, parse_auto / parse_str, prefix/infix/postfix parsers, printers, simplify, is_number
    └── bin/
        └── cli.rs      # `cli` binary: clap flags -s/-v/--simplify/--space/--print
```

## Main concepts
- **`Number`** (`types.rs:23-27`): `Number(i128)` for integers and `Float(f64)` for floats. `Add`, `Sub`, `Mul`, `Div` are implemented; any mixed int/float operation yields `Float`. Integer division truncates. `as_str()` formats the value for output.
- **`TreeTokens`** (`types.rs:4-20`): lexer output: `Variable(String)`, `Number(Number)`, `Plus`, `Sub`, `Mul`, `Div`, `BracO`, `BracC`.
- **`TreeType`** (`types.rs:159-170`): tree nodes. Leaves are `Variable(String)` and `Number(Number)`; branches are `Plus`, `Sub`, `Mul`, `Div` (each `Box<TreeType>, Box<TreeType>`) and `Brac(Box<TreeType>)`.
- **`Symbols`** (`types.rs:132-154`): the operator subset used by the parser, with `TryFrom<TreeTokens>`.
- **`FixType`** (`types.rs:172-177`): `Infix`, `Prefix`, `Postfix`; selects the parser and the printer behaviour.
- **Tokenizing**: `util::split_str` splits on operator and bracket symbols and discards the separator characters (`,` and space for parsing). `Tree::tokenize` (`tree.rs:251`) maps each piece to a token: operators and brackets by text; otherwise `as_i128` (accepts `0b`, `0o`, `0x` prefixes), then `f64`, else `Variable`. Unknown text silently becomes a variable.
- **Notation detection**: `util::check_fix_type` (`util.rs:71`) looks at the first and last tokens to guess the notation; `Tree::parse_auto` uses it. It returns `Err` for an empty string.
- **Parsing**: `parse_infix` (`tree.rs:192`) is a Pratt parser: `+`/`-` bind with powers (1,2), `*`/`/` with (3,4), parentheses recurse. `parse_prefix` (`tree.rs:98`) and `parse_postfix` (`tree.rs:140`) are recursive descent over the token list. Results are stored in `Tree.inner: Option<Box<TreeType>>`.
- **Printing**: `prefixprint`, `infixprint`, `postfixprint` (`tree.rs:276-302`) call `print_inner`, which adds parentheses in infix mode where precedence requires them. The `separator` given to `Tree::new` is used only in prefix/postfix output.
- **Variables**: `Tree.variables: HashMap<String, Number>`; `set_var` / `unset_var` manage them. Variables are not substituted in the printed output; they are only used by `simplify` and `is_number`.
- **Simplify**: `Tree::simplify` (`tree.rs:406`) folds sub-trees whose leaves are all numbers or known variables into one `Number`, and applies local rewrites (`simplify_plus`, `simplify_sub`, `simplify_mul`) that merge constants across nested operations. Partial results are kept as a tree.
- **Evaluating**: `Tree::is_number` (`tree.rs:415`) returns the value when the root is a number or a known variable.
- **CLI** (`src/bin/cli.rs`):
  - `-s, --string <EXPR>`: the expression; otherwise read from piped stdin (if neither, the CLI exits with a Turkish usage error).
  - `-v, --var key=value` (repeatable): sets a variable; integers are tried first, then floats.
  - `--simplify`: calls `simplify` before output.
  - `--space`: uses a space as the printing separator instead of `,`.
  - `--print {infix,prefix,postfix,debug}` (repeatable): prints in the chosen notation or the `Debug` tree, then exits with status 1 (see gaps).
  - Without `--print`: prints the value if the tree is a number, otherwise the infix form.

```rust
use calc::tree::Tree;

let mut t = Tree::new(',');             // separator used when printing prefix/postfix
t.parse_auto("(x + 2) * 3").unwrap();   // infix, detected automatically
t.set_var("x", 4i128);
t.simplify();                           // folds to 18
assert_eq!(t.is_number().map(|n| n.as_str()), Some("18".to_string()));
println!("{}", t.infixprint().unwrap());
```

## Public API
- `calc::tree::Tree` - parsed expression plus variables.
  - `Tree::new(separator: impl Into<char>) -> Tree`
  - `set_var(name, value) -> Option<Number>` / `unset_var(name) -> Option<Number>`
  - `parse_auto(str) -> Result<(), ()>` - detect notation and parse.
  - `parse_str(&mut self, String, FixType) -> Result<(), ()>` - parse with an explicit notation (`parse_str` is `pub`).
  - `prefixprint()`, `infixprint()`, `postfixprint()` -> `Result<String, ()>`
  - `simplify()`, `is_number() -> Option<Number>`
  - `pub inner: Option<Box<TreeType>>`, `pub variables: HashMap<String, Number>`
- `calc::types`: `Number` (with `From<i128>`, `From<f64>`, `as_str`), `TreeType`, `TreeTokens`, `Symbols`, `FixType`.
- `calc::util::as_i128(impl Into<String>) -> Result<i128, ()>` - integer parsing with `0b`/`0o`/`0x` prefixes.
- `calc::util::check_fix_type` is `pub(crate)`; `split_str` is `pub(crate)`.
- `calc` lib also exports a placeholder `add(u64, u64) -> u64` (`lib.rs:6-8`), unrelated to the calculator.
- Binary `cli` (feature `cli`).

## Status and known gaps
- `Symbols::Brac => todo!()` in `maketree` (`tree.rs:94`). The branch is not reached by the current parsers (they never pass `Brac` to `maketree`), but it would panic if it were.
- Division by zero panics: `Number` integer division `a/b` (`types.rs:110`) is Rust integer division, so `1/0` panics. Float division by zero yields `inf` (`types.rs:101`).
- `parse_str` with `FixType::Postfix` computes `tokens.len() - 1` (`tree.rs:77`); an empty token list underflows and panics. `parse_postfix` indexes `tokens[*index]` without a bounds check (`tree.rs:141`).
- Trailing tokens are ignored: `parse_infix_main` (`tree.rs:187-190`) never checks that all tokens were consumed, so `parse_auto("1 2")` succeeds with `1` (verified).
- Unknown words become variables without error (`tree.rs:267-268`), so typos are not reported at parse time.
- `parse_prefix` accepts a leading `)` as a no-op (`tree.rs:107-109`) and `BracO` handling is partial.
- `simplify` only folds constants and partially merges constants across nested `+`, `-`, `*`; it does not do full algebra (`/` is folded only when both sides are numbers, `tree.rs:484-491`).
- CLI: after `--print`, the program calls `std::process::exit(1)` (`cli.rs:100`), so the "=> Hesaplanan Değer" line (`cli.rs:104-108`) is unreachable and the exit status is always 1 in that mode.
- CLI error messages are in Turkish; the source is in English with Turkish strings in `cli.rs`.
- Dead code: `util::_check_fix_type` (`util.rs:28-68`) is unused; `calc::add` placeholder in `lib.rs`.
- README is one line and has no usage documentation.
- Tests: 3 unit tests pass (`cargo test -p calc`); they cover the tokenizer and placeholder only, not parsing or simplification.

use crate::compiler::ast::lexer::{LexerTokenType, LexerTokens, LineColumn};
use crate::funs::as_u64;
use crate::instruction::{Instruction, Operands, Registers};
use crate::nasm_efe::nasm_funs::parse_opcode;
use crate::ops::OpCodes;
use num_derive::{FromPrimitive, ToPrimitive};

pub mod nasm_compiler;
pub mod nasm_funs;


// ---------------------------------------------------------------------------
// Assembler intermediate representation
// ---------------------------------------------------------------------------

/// An immediate: a plain number or a reference to a symbol (label or data name).
#[derive(Debug, PartialEq)]
pub(crate) enum Imm {
    Num(u64),
    /// `len == false`: the symbol's address, `len == true`: `NAME.len`
    Sym { name: String, len: bool, pos: LineColumn },
}

/// One side of a pointer expression.
#[derive(Debug, PartialEq)]
pub(crate) enum AsmItem {
    Reg(Registers),
    Imm(Imm),
}

/// The part between `[` and `]`: `item`, `item + item` or `item - item`.
#[derive(Debug, PartialEq)]
pub(crate) enum AsmInner {
    One(AsmItem),
    Sum(AsmItem, AsmItem),
    Sub(AsmItem, AsmItem),
}

#[derive(Debug, PartialEq)]
pub(crate) enum AsmOperand {
    Reg(Registers),
    Imm(Imm),
    /// the operand type is the pointer kind (`BYTEPTR`, `WORDPTR`, ...)
    Ptr(OperandType, AsmInner),
    Null,
}

#[derive(Debug)]
pub(crate) struct AsmInstruction {
    pub opcode: OpCodes,
    pub left: AsmOperand,
    pub right: AsmOperand,
    #[allow(dead_code)]
    pub pos: LineColumn,
}

#[derive(Debug)]
pub(crate) struct DataDef {
    pub name: String,
    #[allow(dead_code)]
    pub pos: LineColumn,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum PreCompile {
    I(AsmInstruction),
    L(String, LineColumn),
    D(DataDef),
}


#[allow(dead_code)]
impl Instruction {
    pub fn get_len(&self) -> usize {
        let mut len = 1;
        if self.operandl != Operands::Null {
            len += self.operandl.to_int().len();
        }
        if self.operandr != Operands::Null {
            len += self.operandr.to_int().len();
        }
        len
    }

    pub fn get_program(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.push(self.opcode.clone() as u8);
        if self.operandl != Operands::Null {
            v.append(&mut self.operandl.to_int());
        }
        if self.operandr != Operands::Null {
            v.append(&mut self.operandr.to_int());
        }
        v
    }
}


#[allow(unused)]
#[derive(Debug, Clone, PartialEq)]
pub enum ParseErr {
    NoData,
    /// unexpected token at the position
    WrongToken(LineColumn),
    /// unknown instruction / statement
    WrongOpCode(String, LineColumn),
    WrongOperand(String, LineColumn),
    /// missing or invalid label / data name
    ErrLabelName(LineColumn),
    EOF,
    WrongPtrInner(String, LineColumn),
    /// (symbol, position) a symbol was used but never defined
    UndefinedSymbol(String, LineColumn),
    /// (symbol, position of the second definition)
    DuplicateSymbol(String, LineColumn),
    /// malformed string literal (message, position)
    BadString(String, LineColumn),
    /// malformed or out of range number (text, position)
    BadNumber(String, LineColumn),
    /// (what was expected, what was found, position)
    Expected(String, String, LineColumn),
    /// code plus data does not fit in the 32 bit symbol values
    ProgramTooLarge,
    /// file could not be read
    Io(String),
}

impl std::fmt::Display for ParseErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseErr::NoData => write!(f, "no data"),
            ParseErr::WrongToken(p) => write!(f, "{p}: unexpected token"),
            ParseErr::WrongOpCode(s, p) => write!(f, "{p}: unknown instruction or statement `{s}`"),
            ParseErr::WrongOperand(s, p) => write!(f, "{p}: invalid operand `{s}`"),
            ParseErr::ErrLabelName(p) => write!(f, "{p}: expected a valid label or data name"),
            ParseErr::EOF => write!(f, "unexpected end of file"),
            ParseErr::WrongPtrInner(m, p) => write!(f, "{p}: invalid pointer: {m}"),
            ParseErr::UndefinedSymbol(s, p) => write!(f, "{p}: undefined symbol `{s}`"),
            ParseErr::DuplicateSymbol(s, p) => write!(f, "{p}: symbol `{s}` is already defined"),
            ParseErr::BadString(m, p) => write!(f, "{p}: bad string literal: {m}"),
            ParseErr::BadNumber(s, p) => write!(f, "{p}: bad number `{s}`"),
            ParseErr::Expected(what, found, p) => write!(f, "{p}: expected {what}, found {found}"),
            ParseErr::ProgramTooLarge => write!(f, "program is too large (addresses must fit in 32 bits)"),
            ParseErr::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ParseErr {}


// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

const PTR_KEYWORDS: [&str; 4] = ["byteptr", "wordptr", "dwordptr", "qwordptr"];

fn ptr_type(lower: &str) -> Option<OperandType> {
    match lower {
        "byteptr" => Some(OperandType::BYTEPTR),
        "wordptr" => Some(OperandType::WORDPTR),
        "dwordptr" => Some(OperandType::DWORDPTR),
        "qwordptr" => Some(OperandType::QWORDPTR),
        _ => None,
    }
}

/// Valid label / data names: letters, digits and `_`, not starting with a digit,
/// and not a mnemonic, register or keyword.
pub(crate) fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if first.is_ascii_digit() || !(first.is_alphanumeric() || first == '_') {
        return false;
    }
    if !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    let lower = s.to_lowercase();
    !(parse_opcode(&lower).is_some()
        || Registers::from_str(&lower).is_some()
        || lower == "label"
        || lower == "db"
        || PTR_KEYWORDS.contains(&lower.as_str()))
}

fn expected(what: &str, tok: &LexerTokens) -> ParseErr {
    if let LexerTokenType::StrErr(m) = &tok.token_type {
        return ParseErr::BadString(m.clone(), tok.pos.start.clone());
    }
    ParseErr::Expected(what.to_string(), tok.token_type.describe(), tok.pos.start.clone())
}

struct Parser {
    toks: Vec<LexerTokens>,
    i: usize,
}

impl Parser {
    fn new(mut toks: Vec<LexerTokens>) -> Self {
        let need_eof = !matches!(toks.last().map(|t| &t.token_type), Some(LexerTokenType::EOF));
        if need_eof {
            let pos = toks
                .last()
                .map(|t| t.pos.end.clone())
                .unwrap_or(LineColumn { line: 1, column: 1 });
            toks.push(LexerTokens {
                token_type: LexerTokenType::EOF,
                pos: crate::compiler::ast::lexer::Position { start: pos.clone(), end: pos },
            });
        }
        Self { toks, i: 0 }
    }

    fn peek(&self) -> &LexerTokens {
        &self.toks[self.i.min(self.toks.len() - 1)]
    }

    fn peek_type_at(&self, n: usize) -> &LexerTokenType {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].token_type
    }

    /// returns the current token and advances (the final EOF is never passed)
    fn next(&mut self) -> LexerTokens {
        let t = self.peek().clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn expect_eol(&mut self) -> Result<(), ParseErr> {
        match self.peek().token_type {
            LexerTokenType::EOL | LexerTokenType::EOF | LexerTokenType::SemiColon => Ok(()),
            _ => Err(expected("end of line", self.peek())),
        }
    }

    fn expect_comma(&mut self) -> Result<(), ParseErr> {
        let t = self.next();
        if t.token_type == LexerTokenType::Punc {
            Ok(())
        } else {
            Err(expected("`,`", &t))
        }
    }

    /// `name` or `name.len`; the name token is already consumed
    fn symbol_after(&mut self, name: String, pos: LineColumn) -> Result<Imm, ParseErr> {
        let mut len = false;
        if *self.peek_type_at(0) == LexerTokenType::Dot {
            self.next();
            let t = self.next();
            match &t.token_type {
                LexerTokenType::Ident(s) if s.eq_ignore_ascii_case("len") => len = true,
                _ => return Err(expected("`len` after `.`", &t)),
            }
        }
        Ok(Imm::Sym { name, len, pos })
    }

    /// register, number or symbol from an identifier / number token
    fn item_from(&mut self, tok: LexerTokens) -> Result<Option<AsmItem>, ParseErr> {
        match tok.token_type {
            LexerTokenType::Number(n) => Ok(Some(AsmItem::Imm(Imm::Num(n as u64)))),
            LexerTokenType::Ident(s) => {
                let lower = s.to_lowercase();
                if let Some(r) = Registers::from_str(&lower) {
                    return Ok(Some(AsmItem::Reg(r)));
                }
                if s.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    return match as_u64(&s) {
                        Some(n) => Ok(Some(AsmItem::Imm(Imm::Num(n)))),
                        None => Err(ParseErr::BadNumber(s, tok.pos.start)),
                    };
                }
                if !is_valid_name(&s) {
                    // ptr keywords, mnemonics and odd characters are not operands here
                    return Ok(None);
                }
                let imm = self.symbol_after(s, tok.pos.start)?;
                Ok(Some(AsmItem::Imm(imm)))
            }
            _ => Ok(None),
        }
    }

    /// one item of a pointer expression
    fn ptr_item(&mut self) -> Result<AsmItem, ParseErr> {
        let tok = self.next();
        if let LexerTokenType::StrErr(m) = &tok.token_type {
            return Err(ParseErr::BadString(m.clone(), tok.pos.start.clone()));
        }
        let pos = tok.pos.start.clone();
        let desc = tok.token_type.describe();
        match self.item_from(tok)? {
            Some(i) => Ok(i),
            None => Err(ParseErr::WrongPtrInner(
                format!("expected a register, number or symbol, found {desc}"),
                pos,
            )),
        }
    }

    /// parses `item [+|- item] ]` (the `[` is already consumed)
    fn ptr_inner_body(&mut self, open: LineColumn) -> Result<AsmInner, ParseErr> {
        if self.peek().token_type == LexerTokenType::RSPar {
            return Err(ParseErr::WrongPtrInner("No address in pointer".to_string(), open));
        }
        let first = self.ptr_item()?;
        let t = self.next();
        let inner = match t.token_type {
            LexerTokenType::RSPar => return Ok(AsmInner::One(first)),
            LexerTokenType::Plus | LexerTokenType::Minus => {
                let second = self.ptr_item()?;
                if t.token_type == LexerTokenType::Plus {
                    AsmInner::Sum(first, second)
                } else {
                    AsmInner::Sub(first, second)
                }
            }
            _ => {
                return Err(ParseErr::WrongPtrInner(
                    format!("expected `]`, `+` or `-`, found {}", t.token_type.describe()),
                    t.pos.start,
                ))
            }
        };
        let close = self.next();
        if close.token_type != LexerTokenType::RSPar {
            return Err(ParseErr::WrongPtrInner(
                format!("expected `]`, found {}", close.token_type.describe()),
                close.pos.start,
            ));
        }
        Ok(inner)
    }

    /// a general operand: register, number (also negative), symbol or typed pointer
    fn operand(&mut self) -> Result<AsmOperand, ParseErr> {
        let tok = self.next();
        match &tok.token_type {
            LexerTokenType::Minus => {
                let n = self.next();
                let v = match &n.token_type {
                    LexerTokenType::Number(v) => *v as u64,
                    LexerTokenType::Ident(s) if s.chars().next().map_or(false, |c| c.is_ascii_digit()) => {
                        as_u64(s).ok_or_else(|| ParseErr::BadNumber(s.clone(), n.pos.start.clone()))?
                    }
                    _ => return Err(expected("a number after `-`", &n)),
                };
                Ok(AsmOperand::Imm(Imm::Num(v.wrapping_neg())))
            }
            LexerTokenType::Ident(s) => {
                if let Some(pt) = ptr_type(&s.to_lowercase()) {
                    let brl = self.next();
                    if brl.token_type != LexerTokenType::LSPar {
                        return Err(expected("`[`", &brl));
                    }
                    let inner = self.ptr_inner_body(brl.pos.start)?;
                    return Ok(AsmOperand::Ptr(pt, inner));
                }
                let pos = tok.pos.start.clone();
                let name = s.clone();
                match self.item_from(tok.clone())? {
                    Some(AsmItem::Reg(r)) => Ok(AsmOperand::Reg(r)),
                    Some(AsmItem::Imm(i)) => Ok(AsmOperand::Imm(i)),
                    None => Err(ParseErr::WrongOperand(name, pos)),
                }
            }
            LexerTokenType::Number(_) => match self.item_from(tok.clone())? {
                Some(AsmItem::Imm(i)) => Ok(AsmOperand::Imm(i)),
                _ => Err(expected("an operand", &tok)),
            },
            _ => Err(expected("an operand", &tok)),
        }
    }

    /// register or label (symbol); everything else is an error
    fn jump_operand(&mut self) -> Result<AsmOperand, ParseErr> {
        let tok = self.peek().clone();
        let op = self.operand()?;
        match op {
            AsmOperand::Reg(_) | AsmOperand::Imm(Imm::Sym { .. }) => Ok(op),
            _ => Err(expected("a label or register", &tok)),
        }
    }

    fn statement(&mut self, ident: String, pos: LineColumn, prog: &mut Vec<PreCompile>) -> Result<(), ParseErr> {
        let lower = ident.to_lowercase();

        if lower == "label" {
            let name = self.next();
            match name.token_type {
                LexerTokenType::Ident(n) if is_valid_name(&n) => prog.push(PreCompile::L(n, name.pos.start)),
                _ => return Err(ParseErr::ErrLabelName(name.pos.start)),
            }
            return self.expect_eol();
        }

        if *self.peek_type_at(0) == LexerTokenType::Colon {
            if !is_valid_name(&ident) {
                return Err(ParseErr::ErrLabelName(pos));
            }
            self.next();
            prog.push(PreCompile::L(ident, pos));
            // another statement may follow on the same line
            return Ok(());
        }

        if let Some(opcode) = parse_opcode(&lower) {
            let (left, right) = match opcode {
                OpCodes::Mov
                | OpCodes::Add8
                | OpCodes::Add16
                | OpCodes::Add32
                | OpCodes::Add64
                | OpCodes::Sub8
                | OpCodes::Sub16
                | OpCodes::Sub32
                | OpCodes::Sub64
                | OpCodes::Or
                | OpCodes::Xor
                | OpCodes::And
                | OpCodes::Lea
                | OpCodes::Cmp
                | OpCodes::Nand
                | OpCodes::Nor => {
                    let l = self.operand()?;
                    self.expect_comma()?;
                    let r = self.operand()?;
                    (l, r)
                }
                OpCodes::Push8
                | OpCodes::Push16
                | OpCodes::Push32
                | OpCodes::Push64
                | OpCodes::Pop8
                | OpCodes::Pop16
                | OpCodes::Pop32
                | OpCodes::Pop64
                | OpCodes::Test => (self.operand()?, AsmOperand::Null),
                _ if nasm_compiler::is_jump(&opcode) => (self.jump_operand()?, AsmOperand::Null),
                _ => (AsmOperand::Null, AsmOperand::Null),
            };
            prog.push(PreCompile::I(AsmInstruction { opcode, left, right, pos }));
            return self.expect_eol();
        }

        if let LexerTokenType::Ident(d) = self.peek_type_at(0) {
            if d.eq_ignore_ascii_case("db") {
                if !is_valid_name(&ident) {
                    return Err(ParseErr::ErrLabelName(pos));
                }
                self.next();
                let mut bytes = Vec::new();
                loop {
                    let t = self.next();
                    match &t.token_type {
                        LexerTokenType::Str(b) => bytes.extend_from_slice(b),
                        LexerTokenType::Number(n) => {
                            let b = u8::try_from(*n)
                                .map_err(|_| ParseErr::BadNumber(n.to_string(), t.pos.start.clone()))?;
                            bytes.push(b);
                        }
                        LexerTokenType::Ident(s) if s.chars().next().map_or(false, |c| c.is_ascii_digit()) => {
                            let b = as_u64(s)
                                .and_then(|v| u8::try_from(v).ok())
                                .ok_or_else(|| ParseErr::BadNumber(s.clone(), t.pos.start.clone()))?;
                            bytes.push(b);
                        }
                        _ => return Err(expected("a string literal or byte value (0-255)", &t)),
                    }
                    if *self.peek_type_at(0) == LexerTokenType::Punc {
                        self.next();
                    } else {
                        break;
                    }
                }
                prog.push(PreCompile::D(DataDef { name: ident, pos, bytes }));
                return self.expect_eol();
            }
        }

        Err(ParseErr::WrongOpCode(ident, pos))
    }

    fn program(&mut self) -> Result<Vec<PreCompile>, ParseErr> {
        let mut prog = Vec::new();
        loop {
            let tok = self.next();
            match tok.token_type {
                LexerTokenType::EOL | LexerTokenType::SemiColon => {}
                LexerTokenType::EOF => return Ok(prog),
                LexerTokenType::Ident(ident) => self.statement(ident, tok.pos.start, &mut prog)?,
                LexerTokenType::StrErr(m) => return Err(ParseErr::BadString(m, tok.pos.start)),
                _ => return Err(ParseErr::WrongToken(tok.pos.start)),
            }
        }
    }
}

/// Parses the tokens between `[` and `]` of a pointer.
pub(crate) fn parse_ptr_inner_tokens(t: Vec<LexerTokens>) -> Result<AsmInner, ParseErr> {
    let open = t
        .first()
        .map(|x| x.pos.start.clone())
        .unwrap_or(LineColumn { line: 1, column: 1 });
    let close_pos = t.last().map(|x| x.pos.end.clone()).unwrap_or(open.clone());
    let mut toks = t;
    toks.push(LexerTokens {
        token_type: LexerTokenType::RSPar,
        pos: crate::compiler::ast::lexer::Position { start: close_pos.clone(), end: close_pos },
    });
    let mut p = Parser::new(toks);
    let inner = p.ptr_inner_body(open)?;
    // nothing may follow the closing bracket we appended
    if p.peek().token_type != LexerTokenType::EOF {
        return Err(ParseErr::WrongPtrInner(
            format!("unexpected {} in pointer", p.peek().token_type.describe()),
            p.peek().pos.start.clone(),
        ));
    }
    Ok(inner)
}


pub(crate) fn parse_str_to_instructions(str: String) -> Result<Vec<PreCompile>, ParseErr> {
    let tokens = LexerTokens::trim_spaces(LexerTokens::string_to_token(&str));
    Parser::new(tokens).program()
}


// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, ToPrimitive, FromPrimitive, Clone)]
pub(crate) enum OperandType {
    Static   = 0,
    RA       = 1,
    RB,
    RC,
    RD,
    RBP,
    RSP      = 6,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6       = 12,
    AL,
    AH,
    BL,
    BH,
    CL,
    CH,
    DL,
    DH,
    Pointer  = 0x16,
    BYTEPTR,
    WORDPTR,
    DWORDPTR,
    QWORDPTR,
    String,
    NULL     = 0xff,
}


// ---------------------------------------------------------------------------

/// Assembles source text into program bytes (code, then `0x00` and the data of every
/// `db` definition if there is any).
pub fn parse_str<S: Into<String>>(str: S) -> Result<Vec<u8>, ParseErr> {
    let mut src: String = str.into();
    if src.starts_with('\u{feff}') {
        src.remove(0);
    }
    let inst = parse_str_to_instructions(src)?;
    // one sizing pass (symbol addresses), one emit pass
    let data = nasm_compiler::Data::from_pre_compile(inst)?;
    data.to_program()
}

pub fn parse_from_file<S: Into<String>>(f: S) -> Result<Vec<u8>, ParseErr> {
    let path_str = f.into();
    let v = std::fs::read(std::path::Path::new(&path_str))
        .map_err(|e| ParseErr::Io(format!("cannot read `{path_str}`: {e}")))?;
    let text = String::from_utf8(v)
        .map_err(|_| ParseErr::Io(format!("`{path_str}` is not valid UTF-8")))?;
    parse_str(text)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruction::PtrInner;

    const RA: u8 = OperandType::RA as u8;
    const RB: u8 = OperandType::RB as u8;
    const RC: u8 = OperandType::RC as u8;
    const RBP: u8 = OperandType::RBP as u8;
    const BYTEPTR: u8 = OperandType::BYTEPTR as u8;
    const QWORDPTR: u8 = OperandType::QWORDPTR as u8;

    fn op(o: OpCodes) -> u8 {
        o as u8
    }

    fn asm(s: &str) -> Vec<u8> {
        parse_str(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    fn lc(line: u64, column: u64) -> LineColumn {
        LineColumn { line, column }
    }

    fn inner_of(src: &str) -> Result<PtrInner, ParseErr> {
        // tokens between the brackets of `[ ... ]`
        let toks = LexerTokens::trim_spaces(LexerTokens::str_to_token(src));
        let toks: Vec<_> = toks
            .into_iter()
            .filter(|t| t.token_type != LexerTokenType::EOF)
            .collect();
        PtrInner::from_tokens(toks)
    }

    #[test]
    fn label_test() {
        let s = "label sadfsafasf".to_string();
        assert!(parse_str_to_instructions(s).is_ok());
    }

    // ---- bug: PtrInner::from_tokens used t[0] for the right item ----

    #[test]
    fn ptr_inner_from_tokens_right_item() {
        assert_eq!(inner_of("rbp+8").unwrap(), PtrInner::Sum(Registers::RBP, 8));
        assert_eq!(inner_of("8+rbp").unwrap(), PtrInner::Sum(Registers::RBP, 8));
        assert_eq!(inner_of("rb-rc").unwrap(), PtrInner::ExtReg(Registers::RB, Registers::RC));
        assert_eq!(inner_of("rb+rc").unwrap(), PtrInner::SumReg(Registers::RB, Registers::RC));
        assert_eq!(inner_of("rbp-8").unwrap(), PtrInner::Ext(Registers::RBP, 8));
        assert_eq!(inner_of("8-rbp").unwrap(), PtrInner::Extr(8, Registers::RBP));
        assert_eq!(inner_of("8").unwrap(), PtrInner::Static(8));
        assert_eq!(inner_of("0x20b").unwrap(), PtrInner::Static(0x20b));
        assert_eq!(inner_of("rsp").unwrap(), PtrInner::Reg(Registers::RSP));
        assert_eq!(inner_of("3+4").unwrap(), PtrInner::Static(7));
        assert!(inner_of("").is_err());
        assert!(inner_of("rbp+").is_err());
        assert!(inner_of("foo").is_err());
    }

    #[test]
    fn ptr_bytes_right_item() {
        assert_eq!(
            asm("mov QWORDPTR [rbp+8], ra"),
            vec![op(OpCodes::Mov), QWORDPTR, RBP, b'+', 0, 1, 8, QWORDPTR, RA]
        );
        assert_eq!(
            asm("mov ra, BYTEPTR [8+rbp]"),
            vec![op(OpCodes::Mov), RA, BYTEPTR, 0, 1, 8, b'+', RBP, BYTEPTR]
        );
        assert_eq!(
            asm("mov ra, BYTEPTR [rb-rc]"),
            vec![op(OpCodes::Mov), RA, BYTEPTR, RB, b'-', RC, BYTEPTR]
        );
    }

    // ---- bug: 0x..b literals ----

    #[test]
    fn hex_literals_containing_b() {
        assert_eq!(asm("mov ra, 0x20b"), vec![op(OpCodes::Mov), RA, 0, 2, 0x02, 0x0b]);
        assert_eq!(asm("mov ra, 0x0b"), vec![op(OpCodes::Mov), RA, 0, 1, 0x0b]);
        assert_eq!(asm("mov ra, 0b101"), vec![op(OpCodes::Mov), RA, 0, 1, 5]);
        assert_eq!(asm("mov ra, 0o17"), vec![op(OpCodes::Mov), RA, 0, 1, 15]);
        assert_eq!(asm("mov ra, 0XfF"), vec![op(OpCodes::Mov), RA, 0, 1, 255]);
        assert_eq!(asm("mov ra, 0"), vec![op(OpCodes::Mov), RA, 0, 0]);
        assert!(matches!(parse_str("mov ra, 0xzz"), Err(ParseErr::BadNumber(..))));
        assert!(matches!(
            parse_str("mov ra, 0x1ffffffffffffffff"),
            Err(ParseErr::BadNumber(..))
        ));
    }

    // ---- bug: decimal numbers ----

    #[test]
    fn decimal_everywhere() {
        assert_eq!(asm("push32 5"), vec![op(OpCodes::Push32), 0, 1, 5]);
        assert_eq!(asm("push8 300"), vec![op(OpCodes::Push8), 0, 2, 1, 44]);
        assert_eq!(
            asm("mov BYTEPTR [8], 1"),
            vec![op(OpCodes::Mov), BYTEPTR, 0, 1, 8, BYTEPTR, 0, 1, 1]
        );
        assert_eq!(
            asm("mov ra, BYTEPTR [rbp+8]"),
            vec![op(OpCodes::Mov), RA, BYTEPTR, RBP, b'+', 0, 1, 8, BYTEPTR]
        );
        assert_eq!(asm("cmp ra, 10"), vec![op(OpCodes::Cmp), RA, 0, 1, 10]);
        // 20 digits: does not fit in i64 but fits in u64
        assert_eq!(
            asm("mov ra, 18446744073709551615"),
            vec![op(OpCodes::Mov), RA, 0, 8, 255, 255, 255, 255, 255, 255, 255, 255]
        );
    }

    // ---- bug: negative immediates ----

    #[test]
    fn negative_immediates() {
        assert_eq!(
            asm("mov ra, -1"),
            vec![op(OpCodes::Mov), RA, 0, 8, 255, 255, 255, 255, 255, 255, 255, 255]
        );
        assert_eq!(
            asm("mov ra, -2"),
            vec![op(OpCodes::Mov), RA, 0, 8, 255, 255, 255, 255, 255, 255, 255, 254]
        );
        assert_eq!(
            asm("mov ra, -0x10"),
            vec![op(OpCodes::Mov), RA, 0, 8, 255, 255, 255, 255, 255, 255, 255, 0xf0]
        );
        assert_eq!(asm("mov ra, -0"), vec![op(OpCodes::Mov), RA, 0, 0]);
        assert!(parse_str("mov ra, -").is_err());
        assert!(parse_str("mov ra, -rb").is_err());
    }

    // ---- bug: lea and js ----

    #[test]
    fn lea_and_js_assemble() {
        assert_eq!(
            asm("lea ra, QWORDPTR [rbp+8]"),
            vec![op(OpCodes::Lea), RA, QWORDPTR, RBP, b'+', 0, 1, 8, QWORDPTR]
        );
        assert_eq!(
            asm("js _x\nlabel _x"),
            vec![op(OpCodes::Js), 0, 4, 0, 0, 0, 7]
        );
        assert_eq!(
            asm("_x:\nJS _x"),
            vec![op(OpCodes::Js), 0, 4, 0, 0, 0, 0]
        );
    }

    // ---- bug: comments after any token ----

    #[test]
    fn comments_anywhere() {
        let plain = asm("mov ra, 1\nsyscall\nret\n");
        assert_eq!(asm("mov ra, 1 ; one\nsyscall; two\nret ;three"), plain);
        // a comment ends the line, so the operand after it is missing
        assert!(parse_str("mov ra,; comment\n1\n").is_err());
        assert_eq!(asm("; only a comment"), Vec::<u8>::new());
        assert_eq!(asm("nop ; \"unterminated string in comment"), vec![op(OpCodes::Nop)]);
        assert_eq!(asm("label a ; c\njmp a ;c"), vec![op(OpCodes::Jmp), 0, 4, 0, 0, 0, 0]);
        assert_eq!(asm("push8 1;x"), vec![op(OpCodes::Push8), 0, 1, 1]);
        assert_eq!(asm("mov ra, BYTEPTR [rbp] ;x"), asm("mov ra, BYTEPTR [rbp]"));
    }

    // ---- bug: jmp register / invalid jump operands ----

    #[test]
    fn jump_register_and_invalid_operands() {
        assert_eq!(asm("jmp ra"), vec![op(OpCodes::Jmp), RA]);
        assert_eq!(asm("call rb"), vec![op(OpCodes::Call), RB]);
        assert_eq!(asm("jne rc"), vec![op(OpCodes::Jne), RC]);
        for bad in ["jmp 5", "jmp [ra]", "jmp BYTEPTR [ra]", "jmp", "jmp \"x\"", "jmp ,", "jmp mov"] {
            assert!(parse_str(bad).is_err(), "{bad}");
        }
        assert!(matches!(parse_str("jmp 5"), Err(ParseErr::Expected(_, _, _))));
        assert_eq!(parse_str("jmp"), Err(ParseErr::Expected(
            "an operand".into(), "end of file".into(), lc(1, 4))));
    }

    // ---- label resolution: fixed 4 byte symbol values ----

    fn nops(n: usize) -> String {
        "nop\n".repeat(n)
    }

    fn target(bytes: &[u8], at: usize) -> u32 {
        assert_eq!(&bytes[at..at + 3], &[op(OpCodes::Jmp), 0, 4], "jump header");
        u32::from_be_bytes(bytes[at + 3..at + 7].try_into().unwrap())
    }

    #[test]
    fn label_backward_and_forward_boundaries() {
        for addr in [254usize, 255, 256, 257, 65534, 65535, 65536, 65537] {
            // backward: nops, label, jmp back to it
            let bytes = asm(&format!("{}label a\njmp a\n", nops(addr)));
            assert_eq!(bytes.len(), addr + 7);
            assert_eq!(target(&bytes, addr), addr as u32, "backward {addr}");

            // forward: jmp over padding to a label (the label address is 7 + pad)
            let pad = addr - 7;
            let bytes = asm(&format!("jmp a\n{}a:\nnop\n", nops(pad)));
            assert_eq!(target(&bytes, 0), addr as u32, "forward {addr}");
            assert_eq!(bytes.len(), addr + 1);
            assert_eq!(bytes[addr], op(OpCodes::Nop));
        }
    }

    #[test]
    fn label_sizes_do_not_depend_on_distance() {
        // the size of every jump is 7 regardless of the value
        assert_eq!(asm("jmp a\na:").len(), 7);
        assert_eq!(asm(&format!("jmp a\n{}a:", nops(70000))).len(), 7 + 70000);
        assert_eq!(asm("call a\nje a\nlabel a").len(), 14);
    }

    #[test]
    fn label_as_immediate_and_colon_syntax() {
        assert_eq!(
            asm("mov ra, _f\n_f: ret"),
            vec![op(OpCodes::Mov), RA, 0, 4, 0, 0, 0, 8, op(OpCodes::Ret)]
        );
        // a statement may follow a label on the same line
        assert_eq!(asm("a: nop\njmp a"), vec![op(OpCodes::Nop), op(OpCodes::Jmp), 0, 4, 0, 0, 0, 0]);
        // both label syntaxes, backward + forward in one program
        assert_eq!(
            asm("start:\njmp end\nlabel mid\njmp start\nend:"),
            vec![
                op(OpCodes::Jmp), 0, 4, 0, 0, 0, 14,
                op(OpCodes::Jmp), 0, 4, 0, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn undefined_and_duplicate_symbols() {
        assert_eq!(
            parse_str("nop\njmp foo"),
            Err(ParseErr::UndefinedSymbol("foo".into(), lc(2, 5)))
        );
        assert_eq!(
            parse_str("mov ra, foo"),
            Err(ParseErr::UndefinedSymbol("foo".into(), lc(1, 9)))
        );
        assert_eq!(
            parse_str("mov ra, BYTEPTR [foo]"),
            Err(ParseErr::UndefinedSymbol("foo".into(), lc(1, 18)))
        );
        assert_eq!(
            parse_str("a:\nnop\nlabel a"),
            Err(ParseErr::DuplicateSymbol("a".into(), lc(3, 7)))
        );
        assert_eq!(
            parse_str("a: nop\na: nop"),
            Err(ParseErr::DuplicateSymbol("a".into(), lc(2, 1)))
        );
        assert_eq!(
            parse_str("a db 1\nlabel a"),
            Err(ParseErr::DuplicateSymbol("a".into(), lc(2, 7)))
        );
        assert_eq!(
            parse_str("a db 1\na db 2"),
            Err(ParseErr::DuplicateSymbol("a".into(), lc(2, 1)))
        );
        // .len is only valid for db names
        assert!(matches!(parse_str("a:\nmov ra, a.len"), Err(ParseErr::UndefinedSymbol(..))));
        // the message mentions line and column
        let msg = parse_str("nop\njmp foo").unwrap_err().to_string();
        assert!(msg.contains("line 2") && msg.contains("column 5") && msg.contains("foo"), "{msg}");
    }

    #[test]
    fn bad_names() {
        for bad in ["label mov", "label ra", "label 1a", "label", "label db", "mov: nop", "label a-b", "1: nop"] {
            assert!(parse_str(bad).is_err(), "{bad}");
        }
        assert!(matches!(parse_str("label"), Err(ParseErr::ErrLabelName(_))));
    }

    // ---- db ----

    #[test]
    fn db_basic_layout() {
        let src = "mov rb, msg\nmov rc, msg.len\nmsg db \"hi\", 10\n";
        assert_eq!(
            asm(src),
            vec![
                op(OpCodes::Mov), RB, 0, 4, 0, 0, 0, 17,
                op(OpCodes::Mov), RC, 0, 4, 0, 0, 0, 3,
                0, // end-of-code marker
                b'h', b'i', 10,
            ]
        );
    }

    #[test]
    fn db_position_in_source_does_not_matter() {
        let a = asm("msg db \"ab\"\nmov rb, msg\nnop");
        let b = asm("mov rb, msg\nnop\nmsg db \"ab\"");
        assert_eq!(a, b);
        // code = 8 + 1 = 9, marker at 9, data starts at 10
        assert_eq!(a, vec![op(OpCodes::Mov), RB, 0, 4, 0, 0, 0, 10, op(OpCodes::Nop), 0, b'a', b'b']);
    }

    #[test]
    fn db_definition_order_and_items() {
        let bytes = asm("nop\nfirst db 1, 2, 0xff, \"ab\"\nsecond db \"\\n\\t\\r\\0\\\\\\\"\\x41\", 0b11, 0o7\nthird db \"é\"");
        assert_eq!(
            bytes,
            vec![
                op(OpCodes::Nop),
                0,
                1, 2, 0xff, b'a', b'b',
                b'\n', b'\t', b'\r', 0, b'\\', b'"', 0x41, 3, 7,
                0xc3, 0xa9,
            ]
        );
    }

    #[test]
    fn db_symbol_values() {
        let src = "mov rb, first\nmov rc, first.len\nmov ra, second\nmov rd, second.len\n\
                   first db \"abc\"\nsecond db 1,2\nempty db \"\"\nmov r1, empty.len";
        let bytes = asm(src);
        // 5 movs of 8 bytes = 40; marker at 40; data at 41
        assert_eq!(bytes.len(), 40 + 1 + 5);
        let val = |at: usize| u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap());
        assert_eq!(val(0), 41); // first
        assert_eq!(val(8), 3); // first.len
        assert_eq!(val(16), 44); // second
        assert_eq!(val(24), 2); // second.len
        assert_eq!(val(32), 0); // empty.len
        assert_eq!(bytes[40], 0);
        assert_eq!(&bytes[41..], b"abc\x01\x02");
    }

    #[test]
    fn db_symbols_inside_pointers() {
        let bytes = asm("mov ra, BYTEPTR [msg]\nmov rb, BYTEPTR [rbp+msg]\nmov rc, BYTEPTR [msg+rbp]\nmsg db 7");
        // instruction sizes: 1+1+(1+6+1)=10, then 1+1+(1+1+1+6+1)=12, 12 -> 34, marker, data at 35
        let t = |a: u8| vec![0, 4, 0, 0, 0, a];
        let mut want = vec![op(OpCodes::Mov), RA, BYTEPTR];
        want.extend(t(35));
        want.push(BYTEPTR);
        want.extend([op(OpCodes::Mov), RB, BYTEPTR, RBP, b'+']);
        want.extend(t(35));
        want.push(BYTEPTR);
        want.extend([op(OpCodes::Mov), RC, BYTEPTR]);
        want.extend(t(35));
        want.extend([b'+', RBP, BYTEPTR]);
        want.extend([0, 7]);
        assert_eq!(bytes, want);
        // NAME.len in a pointer
        let bytes = asm("mov ra, BYTEPTR [msg.len]\nmsg db 1,2,3");
        assert_eq!(&bytes[3..9], &[0, 4, 0, 0, 0, 3]);
    }

    #[test]
    fn db_errors() {
        assert!(matches!(parse_str("a db"), Err(ParseErr::Expected(..))));
        assert!(matches!(parse_str("a db 1,"), Err(ParseErr::Expected(..))));
        assert!(matches!(parse_str("a db 256"), Err(ParseErr::BadNumber(..))));
        assert!(matches!(parse_str("a db 0x100"), Err(ParseErr::BadNumber(..))));
        assert!(matches!(parse_str("a db -1"), Err(ParseErr::Expected(..))));
        assert!(matches!(parse_str("a db foo"), Err(ParseErr::Expected(..))));
        assert!(matches!(parse_str("a db \"x\" \"y\""), Err(ParseErr::Expected(..))));
        assert!(matches!(parse_str("mov db 1"), Err(ParseErr::WrongOperand(..))));
        assert!(matches!(parse_str("ra db 1"), Err(ParseErr::ErrLabelName(_))));
    }

    #[test]
    fn bad_strings() {
        assert_eq!(
            parse_str("a db \"abc"),
            Err(ParseErr::BadString("unterminated string literal".into(), lc(1, 6)))
        );
        assert!(matches!(parse_str("a db \"a\\qb\""), Err(ParseErr::BadString(..))));
        assert!(matches!(parse_str("a db \"a\\x4\""), Err(ParseErr::BadString(..))));
        assert!(matches!(parse_str("a db \"a\\x4g\""), Err(ParseErr::BadString(..))));
        assert!(matches!(parse_str("\"loose\""), Err(ParseErr::WrongToken(_))));
        assert!(matches!(parse_str("mov ra, \"x\""), Err(ParseErr::Expected(..))));
    }

    // ---- other operand / statement errors, display ----

    #[test]
    fn errors_have_positions() {
        assert_eq!(
            parse_str("nop\nfoo ra"),
            Err(ParseErr::WrongOpCode("foo".into(), lc(2, 1)))
        );
        assert_eq!(
            parse_str("mov ra rb"),
            Err(ParseErr::Expected("`,`".into(), "`rb`".into(), lc(1, 8)))
        );
        assert_eq!(
            parse_str("mov ra, 1 2"),
            Err(ParseErr::Expected("end of line".into(), "number 2".into(), lc(1, 11)))
        );
        assert_eq!(
            parse_str("mov ra,"),
            Err(ParseErr::Expected("an operand".into(), "end of file".into(), lc(1, 8)))
        );
        assert!(matches!(parse_str("mov ra, BYTEPTR rb"), Err(ParseErr::Expected(..))));
        assert!(matches!(parse_str("mov ra, BYTEPTR []"), Err(ParseErr::WrongPtrInner(..))));
        assert!(matches!(parse_str("mov ra, BYTEPTR [ra+rb+rc]"), Err(ParseErr::WrongPtrInner(..))));
        assert!(matches!(parse_str("mov ra, BYTEPTR [ra"), Err(ParseErr::WrongPtrInner(..))));
        assert!(matches!(parse_str("mov ra, BYTEPTR [BYTEPTR]"), Err(ParseErr::WrongPtrInner(..))));
        assert!(matches!(parse_str("mov ra, mov"), Err(ParseErr::WrongOperand(..))));
        for e in [
            ParseErr::NoData,
            ParseErr::EOF,
            ParseErr::ProgramTooLarge,
            ParseErr::Io("x".into()),
            ParseErr::WrongToken(lc(1, 1)),
        ] {
            let _: &dyn std::error::Error = &e;
            assert!(!e.to_string().is_empty());
        }
        let m = parse_str("mov ra rb").unwrap_err().to_string();
        assert!(m.contains("line 1") && m.contains("expected") && m.contains("found"), "{m}");
    }

    #[test]
    fn existing_program_shapes() {
        assert_eq!(
            asm("mov ra, 2\nmov rb, rbp\nsyscall"),
            vec![op(OpCodes::Mov), RA, 0, 1, 2, op(OpCodes::Mov), RB, RBP, op(OpCodes::SysCall)]
        );
        assert_eq!(asm("test r2\nnop\nret"), vec![op(OpCodes::Test), OperandType::R2 as u8, 255, op(OpCodes::Ret)]);
        assert_eq!(asm("MOV Ra , 1"), vec![op(OpCodes::Mov), RA, 0, 1, 1]);
        assert_eq!(asm("\r\nnop\r\n"), vec![op(OpCodes::Nop)]);
        assert_eq!(asm("\u{feff}nop"), vec![op(OpCodes::Nop)]);
    }

    // ---- files ----

    #[test]
    fn parse_from_file_errors() {
        assert!(matches!(
            parse_from_file("/definitely/not/here.efe"),
            Err(ParseErr::Io(_))
        ));
        let dir = std::env::temp_dir();
        let bad = dir.join(format!("efescript_nonutf8_{}.efe", std::process::id()));
        std::fs::write(&bad, [0x6e, 0xff, 0xfe, 0x70]).unwrap();
        let r = parse_from_file(bad.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&bad);
        assert!(matches!(r, Err(ParseErr::Io(_))));

        let good = dir.join(format!("efescript_good_{}.efe", std::process::id()));
        std::fs::write(&good, "nop\n").unwrap();
        let r = parse_from_file(good.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&good);
        assert_eq!(r, Ok(vec![op(OpCodes::Nop)]));
        // a directory is an Io error too
        assert!(matches!(parse_from_file(dir.to_string_lossy().to_string()), Err(ParseErr::Io(_))));
    }

    #[test]
    fn shipped_examples_assemble() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("efe") {
                let r = parse_from_file(p.to_string_lossy().to_string());
                assert!(r.is_ok(), "{}: {:?}", p.display(), r);
            }
        }
    }

    // ---- garbage never panics ----

    #[test]
    fn garbage_never_panics() {
        let inputs: &[&str] = &[
            "", " ", "\n\n\n", "\t", ";", ";;;", "\"", "\"\"", "\"\\", "\"\\x", "\"\\x4", "'", "`",
            "[", "]", "[]", "][", ",", ",,", ":", "::", ".", "..", "+", "-", "mov", "mov ra", "mov ra,",
            "mov ,", "mov ,ra", "mov [", "mov ra, [", "mov ra, [rbp]", "mov ra, BYTEPTR", "mov ra, BYTEPTR [",
            "mov ra, BYTEPTR [rbp", "mov ra, BYTEPTR [rbp+", "mov ra, BYTEPTR [rbp+]", "mov ra, BYTEPTR [+rbp]",
            "mov ra, BYTEPTR [-]", "mov ra, BYTEPTR [rbp++1]", "mov ra, BYTEPTR [.len]", "mov ra, BYTEPTR [a.]",
            "mov ra, BYTEPTR [a.len.len]", "mov ra, a.", "mov ra, a.b", "mov ra, .len", "mov ra, 1.5",
            "mov ra, --1", "mov ra, -+1", "mov ra, 99999999999999999999999999", "mov ra, -99999999999999999999",
            "mov ra, 0x", "mov ra, 0b", "mov ra, 0o8", "push", "push32", "push32 rb rc", "pop8 ,", "test",
            "jmp", "jmp jmp", "jmp ,", "jmp [", "call", "call 1", "ret ret", "syscall 1", "nop nop",
            "label", "label 5", "label \"x\"", "label a b", "label a:", "a:", "a::", "a: a:", ": a", "a :",
            "db", "db 1", "a db", "a db ,", "a db \"", "a db 1 2", "a db \"x\" 1", "a db 300", "a db -1",
            "a db a", "a b c d e f", "label db", "db db db", "mov: mov", "ra: nop", "\u{0}", "\u{feff}",
            "é", "𐰦", "mov ra, é", "é: nop", "é db 1", "mov ra, \"é\"", "\r", "\r\n\r\n", "\r;\r",
            "mov ra, 1 ; \"", "\"a\nb\"", "jmp a\n", "mov ra, a.len\n", "a db 1\nmov ra, a.len.len",
            "a db 1\nmov ra, BYTEPTR [a+a]", "a db 1\nmov ra, BYTEPTR [a-a]", "mov QWORDPTR [ra], QWORDPTR [rb]",
            "lea", "lea ra", "lea ra,", "js", "js js", "cmp 1,2", "mov 1, 2", "mov \"a\", \"b\"",
            "BYTEPTR [ra]", "[ra]", "ra", "1", "-1", "+1", "0x1", "mov ra, ra, ra", "mov ra,, rb",
        ];
        for s in inputs {
            let _ = parse_str(*s);
        }
        // byte-level mutation sweep over a valid program
        let base = "label a\nmov ra, BYTEPTR [rbp+8] ; c\njmp a\nmsg db \"hi\\n\", 1\nmov rb, msg.len\n";
        let chars: Vec<char> = base.chars().collect();
        for i in 0..chars.len() {
            for rep in ['"', '[', ']', ',', ';', ':', '.', '-', '+', '\\', '\n', ' ', 'x', '9'] {
                let mut c = chars.clone();
                c[i] = rep;
                let _ = parse_str(c.iter().collect::<String>());
                let mut c = chars.clone();
                c.remove(i);
                let _ = parse_str(c.iter().collect::<String>());
                let mut c = chars.clone();
                c.insert(i, rep);
                let _ = parse_str(c.iter().collect::<String>());
            }
            let _ = parse_str(chars[..i].iter().collect::<String>());
        }
    }
}

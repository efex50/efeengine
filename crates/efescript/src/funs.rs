/// Splits an optional radix prefix (`0b`, `0x`, `0o`, case-insensitive) from the digits.
fn split_radix(a: &str) -> (u32, &str) {
    let lower_prefix = a.get(..2).map(|p| p.to_ascii_lowercase());
    match lower_prefix.as_deref() {
        Some("0b") => (2, &a[2..]),
        Some("0x") => (16, &a[2..]),
        Some("0o") => (8, &a[2..]),
        _ => (10, a),
    }
}

/// Parses a number literal: decimal, `0b..`, `0x..`, `0o..`. No sign, no whitespace.
fn parse_u64_literal(a: &str) -> Option<u64> {
    let (radix, digits) = split_radix(a);
    // from_str_radix accepts a leading `+`, which is not a literal
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()
}

pub fn as_usize<S:Into<String>>(a:S) -> Result<usize,()>{
    let a:String = a.into();
    parse_u64_literal(&a).and_then(|n| usize::try_from(n).ok()).ok_or(())
}
#[allow(unused)]
pub fn as_u8<S:Into<String>>(a:S) -> Option<u8>{
    let a:String = a.into();
    parse_u64_literal(&a).and_then(|n| u8::try_from(n).ok())
}
#[allow(unused)]
pub fn as_u16<S:Into<String>>(a:S) -> Option<u16>{
    let a:String = a.into();
    parse_u64_literal(&a).and_then(|n| u16::try_from(n).ok())
}
#[allow(unused)]
pub fn as_u32<S:Into<String>>(a:S) -> Option<u32>{
    let a:String = a.into();
    parse_u64_literal(&a).and_then(|n| u32::try_from(n).ok())
}
#[allow(unused)]
pub fn as_u64<S:Into<String>>(a:S) -> Option<u64>{
    let a:String = a.into();
    parse_u64_literal(&a)
}


pub fn trim_zeroes(a:Vec<u8>) -> Vec<u8>{
    let mut hit = false;
    let mut buf = Vec::new();
    for x in a{
        if x != 0{
            hit = true;
        }
        if hit{
            buf.push(x);
        }
    };
    buf
}

pub fn get_db_data(a:&mut impl Iterator<Item = u8>){
    let mut v = Vec::new();
    while let Some(d) = a.next() {
        v.push(d);
    }
    println!("{}",String::from_utf8_lossy(&v));
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_and_decimal() {
        assert_eq!(as_usize("0x20b"), Ok(0x20b));
        assert_eq!(as_usize("0x0b"), Ok(0x0b));
        assert_eq!(as_usize("0X1F"), Ok(0x1f));
        assert_eq!(as_usize("0b101"), Ok(5));
        assert_eq!(as_usize("0o17"), Ok(15));
        assert_eq!(as_usize("42"), Ok(42));
        assert_eq!(as_usize("0"), Ok(0));
        assert_eq!(as_u8("0xff"), Some(255));
        assert_eq!(as_u8("256"), None);
        assert_eq!(as_u16("0x10b"), Some(0x10b));
        assert_eq!(as_u32("0xb0b0"), Some(0xb0b0));
        assert_eq!(as_u64("0xffffffffffffffff"), Some(u64::MAX));
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", "0x", "0b2", "x10", "+5", "-5", "1x", "0xg", "99999999999999999999999", "a0b"] {
            assert!(as_usize(s).is_err(), "{s:?}");
        }
    }
}

//! Formatting bytes for the clipboard / export, and parsing pasted data.

use std::fmt::Write;

use crate::encoding::TextEncoding;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
    HexSpaced,
    HexCompact,
    Text,
    CArray,
    RustArray,
    PythonBytes,
    CSharpArray,
    JavaArray,
    GoSlice,
    JsArray,
    Base64,
    HexDump,
}

impl CopyFormat {
    pub const ALL: [Self; 12] = [
        Self::HexSpaced,
        Self::HexCompact,
        Self::Text,
        Self::CArray,
        Self::RustArray,
        Self::PythonBytes,
        Self::CSharpArray,
        Self::JavaArray,
        Self::GoSlice,
        Self::JsArray,
        Self::Base64,
        Self::HexDump,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::HexSpaced => "Hex (spaced)",
            Self::HexCompact => "Hex (compact)",
            Self::Text => "Text",
            Self::CArray => "C array",
            Self::RustArray => "Rust array",
            Self::PythonBytes => "Python bytes",
            Self::CSharpArray => "C# array",
            Self::JavaArray => "Java array",
            Self::GoSlice => "Go slice",
            Self::JsArray => "JavaScript Uint8Array",
            Self::Base64 => "Base64",
            Self::HexDump => "Hex dump",
        }
    }

    pub fn format(
        self,
        bytes: &[u8],
        base_offset: usize,
        uppercase: bool,
        encoding: TextEncoding,
    ) -> String {
        let hx = |b: u8| hex_byte(b, uppercase);
        match self {
            Self::HexSpaced => join_bytes(bytes, " ", |b| hx(b).to_string()),
            Self::HexCompact => bytes.iter().map(|&b| hx(b)).collect(),
            Self::Text => encoding.decode_lossy(bytes),
            Self::CArray => format!(
                "const unsigned char data[{}] = {{\n{}\n}};",
                bytes.len(),
                array_body(bytes, |b| format!("0x{}", hx(b)))
            ),
            Self::RustArray => format!(
                "const DATA: [u8; {}] = [\n{}\n];",
                bytes.len(),
                array_body(bytes, |b| format!("0x{}", hx(b)))
            ),
            Self::PythonBytes => {
                let mut s = String::from("b\"");
                for &b in bytes {
                    let _ = write!(s, "\\x{}", hex_byte(b, false));
                }
                s.push('"');
                s
            }
            Self::CSharpArray => format!(
                "byte[] data = new byte[] {{\n{}\n}};",
                array_body(bytes, |b| format!("0x{}", hx(b)))
            ),
            Self::JavaArray => format!(
                "byte[] data = new byte[] {{\n{}\n}};",
                array_body(bytes, |b| if b < 0x80 {
                    format!("0x{}", hx(b))
                } else {
                    format!("(byte) 0x{}", hx(b))
                })
            ),
            Self::GoSlice => format!(
                "data := []byte{{\n{},\n}}",
                array_body(bytes, |b| format!("0x{}", hx(b)))
            ),
            Self::JsArray => format!(
                "const data = new Uint8Array([\n{}\n]);",
                array_body(bytes, |b| format!("0x{}", hx(b)))
            ),
            Self::Base64 => base64_encode(bytes),
            Self::HexDump => hex_dump(bytes, base_offset, uppercase, encoding),
        }
    }
}

pub fn hex_byte(b: u8, uppercase: bool) -> &'static str {
    const UPPER: &str = "000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F\
202122232425262728292A2B2C2D2E2F303132333435363738393A3B3C3D3E3F\
404142434445464748494A4B4C4D4E4F505152535455565758595A5B5C5D5E5F\
606162636465666768696A6B6C6D6E6F707172737475767778797A7B7C7D7E7F\
808182838485868788898A8B8C8D8E8F909192939495969798999A9B9C9D9E9F\
A0A1A2A3A4A5A6A7A8A9AAABACADAEAFB0B1B2B3B4B5B6B7B8B9BABBBCBDBEBF\
C0C1C2C3C4C5C6C7C8C9CACBCCCDCECFD0D1D2D3D4D5D6D7D8D9DADBDCDDDEDF\
E0E1E2E3E4E5E6E7E8E9EAEBECEDEEEFF0F1F2F3F4F5F6F7F8F9FAFBFCFDFEFF";
    const LOWER: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\
202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f\
404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f\
606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f\
808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f\
a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf\
c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedf\
e0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff";
    let table = if uppercase { UPPER } else { LOWER };
    let i = b as usize * 2;
    &table[i..i + 2]
}

pub fn to_hex(bytes: &[u8], uppercase: bool) -> String {
    bytes.iter().map(|&b| hex_byte(b, uppercase)).collect()
}

fn join_bytes(bytes: &[u8], sep: &str, f: impl Fn(u8) -> String) -> String {
    bytes.iter().map(|&b| f(b)).collect::<Vec<_>>().join(sep)
}

fn array_body(bytes: &[u8], f: impl Fn(u8) -> String) -> String {
    bytes
        .chunks(16)
        .map(|chunk| format!("    {}", join_bytes(chunk, ", ", &f)))
        .collect::<Vec<_>>()
        .join(",\n")
}

fn hex_dump(bytes: &[u8], base: usize, uppercase: bool, enc: TextEncoding) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let off = base + i * 16;
        let _ = write!(out, "{off:08X}  ");
        for j in 0..16 {
            match chunk.get(j) {
                Some(&b) => {
                    out.push_str(hex_byte(b, uppercase));
                    out.push(' ');
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.push('|');
        for &b in chunk {
            out.push(enc.decode(b).unwrap_or('.'));
        }
        out.push_str("|\n");
    }
    out
}

/// Parses hex text such as `DE AD BE EF`, `0xde,0xad`, `\xde\xad` or `deadbeef`.
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let cleaned = input
        .replace("\\x", " ")
        .replace("0x", " ")
        .replace("0X", " ");
    let mut out = Vec::new();
    for token in cleaned.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '{' | '}' | '[' | ']' | '"')
    }) {
        if token.is_empty() {
            continue;
        }
        if let Some(bad) = token.chars().find(|c| !c.is_ascii_hexdigit()) {
            return Err(format!("invalid hex character '{bad}'"));
        }
        if token.len() == 1 {
            out.push(u8::from_str_radix(token, 16).unwrap_or(0));
            continue;
        }
        if token.len() % 2 != 0 {
            return Err(format!("odd number of hex digits in '{token}'"));
        }
        for i in (0..token.len()).step_by(2) {
            out.push(u8::from_str_radix(&token[i..i + 2], 16).map_err(|e| e.to_string())?);
        }
    }
    if out.is_empty() {
        return Err("no hex bytes found".into());
    }
    Ok(out)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = match chunk.len() {
            3 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32,
            2 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8,
            _ => (chunk[0] as u32) << 16,
        };
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in input.chars() {
        if c.is_whitespace() || c == '=' {
            continue;
        }
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => return Err(format!("invalid base64 character '{c}'")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(
            parse_hex("DE AD be ef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(parse_hex("0xde, 0xad").unwrap(), vec![0xde, 0xad]);
        assert_eq!(parse_hex("\\x01\\x02").unwrap(), vec![1, 2]);
        assert_eq!(parse_hex("deadbeef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(parse_hex("1 2 a").unwrap(), vec![1, 2, 10]);
        assert!(parse_hex("xyz").is_err());
        assert!(parse_hex("abc").is_err());
        assert_eq!(to_hex(&[0xab, 0x01], true), "AB01");
        assert_eq!(hex_byte(0xff, false), "ff");
    }

    #[test]
    fn base64() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            let enc = base64_encode(s.as_bytes());
            assert_eq!(base64_decode(&enc).unwrap(), s.as_bytes());
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
    }

    #[test]
    fn formats() {
        let b = [0x00, 0xff];
        assert_eq!(
            CopyFormat::HexSpaced.format(&b, 0, true, TextEncoding::Ascii),
            "00 FF"
        );
        assert_eq!(
            CopyFormat::PythonBytes.format(&b, 0, true, TextEncoding::Ascii),
            "b\"\\x00\\xff\""
        );
        assert!(
            CopyFormat::CArray
                .format(&b, 0, true, TextEncoding::Ascii)
                .contains("data[2]")
        );
        assert!(
            CopyFormat::HexDump
                .format(b"ABC", 0x10, true, TextEncoding::Ascii)
                .starts_with("00000010  41 42 43")
        );
    }
}

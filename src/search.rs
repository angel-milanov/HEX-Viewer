//! Byte pattern search with wildcards, case-insensitive text and numeric
//! values.

use memchr::memmem;
use serde::{Deserialize, Serialize};

use crate::export::parse_hex;
use crate::values::NumType;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchKind {
    #[default]
    Hex,
    Text,
    Value,
}

impl SearchKind {
    pub const ALL: [Self; 3] = [Self::Hex, Self::Text, Self::Value];

    pub fn name(self) -> &'static str {
        match self {
            Self::Hex => "Hex",
            Self::Text => "Text",
            Self::Value => "Value",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextSearchEncoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl TextSearchEncoding {
    pub const ALL: [Self; 3] = [Self::Utf8, Self::Utf16Le, Self::Utf16Be];

    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8 / ASCII",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
        }
    }
}

/// Everything needed to turn user input into a [`Pattern`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuerySpec {
    pub kind: SearchKind,
    pub text_encoding: TextSearchEncoding,
    pub case_sensitive: bool,
    pub num_type: NumType,
    pub big_endian: bool,
}

impl Default for QuerySpec {
    fn default() -> Self {
        Self {
            kind: SearchKind::Hex,
            text_encoding: TextSearchEncoding::Utf8,
            case_sensitive: true,
            num_type: NumType::U32,
            big_endian: false,
        }
    }
}

impl QuerySpec {
    pub fn build(&self, input: &str) -> Result<Pattern, String> {
        if input.is_empty() {
            return Err("enter a search pattern".into());
        }
        match self.kind {
            SearchKind::Hex => Pattern::from_hex(input),
            SearchKind::Text => Ok(Pattern::from_text(
                input,
                self.text_encoding,
                self.case_sensitive,
            )),
            SearchKind::Value => self
                .num_type
                .encode(input, self.big_endian)
                .map(|b| Pattern::exact(&b)),
        }
    }

    /// Converts replacement input into literal bytes.
    pub fn replacement_bytes(&self, input: &str) -> Result<Vec<u8>, String> {
        match self.kind {
            SearchKind::Hex => {
                if input.trim().is_empty() {
                    Ok(Vec::new())
                } else {
                    parse_hex(input)
                }
            }
            SearchKind::Text => Ok(encode_text(input, self.text_encoding)),
            SearchKind::Value => self.num_type.encode(input, self.big_endian),
        }
    }
}

pub fn encode_text(s: &str, enc: TextSearchEncoding) -> Vec<u8> {
    match enc {
        TextSearchEncoding::Utf8 => s.as_bytes().to_vec(),
        TextSearchEncoding::Utf16Le => s.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        TextSearchEncoding::Utf16Be => s.encode_utf16().flat_map(u16::to_be_bytes).collect(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Elem {
    /// Matches when `byte & mask == value`.
    Masked { value: u8, mask: u8 },
    /// Matches either byte (used for case-insensitive ASCII letters).
    Either(u8, u8),
}

impl Elem {
    #[inline]
    fn matches(self, b: u8) -> bool {
        match self {
            Elem::Masked { value, mask } => b & mask == value,
            Elem::Either(x, y) => b == x || b == y,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    elems: Vec<Elem>,
}

impl Pattern {
    pub fn exact(bytes: &[u8]) -> Self {
        Self {
            elems: bytes
                .iter()
                .map(|&b| Elem::Masked {
                    value: b,
                    mask: 0xff,
                })
                .collect(),
        }
    }

    /// Parses hex with optional `?` wildcards, e.g. `4D 5A ?? ?0`.
    pub fn from_hex(input: &str) -> Result<Self, String> {
        let mut nibbles: Vec<Option<u8>> = Vec::new();
        for token in input.split(|c: char| c.is_whitespace() || c == ',') {
            let token = token
                .strip_prefix("0x")
                .or_else(|| token.strip_prefix("0X"))
                .unwrap_or(token);
            let mut tok: Vec<Option<u8>> = Vec::new();
            for c in token.chars() {
                match c {
                    '?' | '*' => tok.push(None),
                    c => match c.to_digit(16) {
                        Some(d) => tok.push(Some(d as u8)),
                        None => return Err(format!("invalid hex character '{c}'")),
                    },
                }
            }
            if tok.len() == 1 {
                tok.insert(0, Some(0));
            }
            if !tok.len().is_multiple_of(2) {
                return Err(format!("odd number of hex digits in '{token}'"));
            }
            nibbles.extend(tok);
        }
        if nibbles.is_empty() {
            return Err("enter a hex pattern".into());
        }
        let elems = nibbles
            .chunks(2)
            .map(|pair| {
                let (hi, hm) = pair[0].map_or((0, 0), |v| (v << 4, 0xf0));
                let (lo, lm) = pair[1].map_or((0, 0), |v| (v, 0x0f));
                Elem::Masked {
                    value: hi | lo,
                    mask: hm | lm,
                }
            })
            .collect();
        Ok(Self { elems })
    }

    pub fn from_text(input: &str, enc: TextSearchEncoding, case_sensitive: bool) -> Self {
        let bytes = encode_text(input, enc);
        let step = if enc == TextSearchEncoding::Utf8 {
            1
        } else {
            2
        };
        let letter_pos = match enc {
            TextSearchEncoding::Utf16Be => 1,
            _ => 0,
        };
        let elems = bytes
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                let is_letter_byte = i % step == letter_pos
                    && b.is_ascii_alphabetic()
                    && (step == 1 || bytes[i - letter_pos + (1 - letter_pos)] == 0);
                if !case_sensitive && is_letter_byte {
                    Elem::Either(b.to_ascii_lowercase(), b.to_ascii_uppercase())
                } else {
                    Elem::Masked {
                        value: b,
                        mask: 0xff,
                    }
                }
            })
            .collect();
        Self { elems }
    }

    pub fn len(&self) -> usize {
        self.elems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }

    fn exact_bytes(&self) -> Option<Vec<u8>> {
        self.elems
            .iter()
            .map(|e| match *e {
                Elem::Masked { value, mask: 0xff } => Some(value),
                _ => None,
            })
            .collect()
    }

    /// Index and element of the first element suitable for a memchr scan.
    fn anchor(&self) -> Option<(usize, Elem)> {
        self.elems
            .iter()
            .copied()
            .enumerate()
            .find(|(_, e)| matches!(e, Elem::Masked { mask: 0xff, .. } | Elem::Either(..)))
    }

    #[inline]
    pub fn matches_at(&self, data: &[u8], pos: usize) -> bool {
        data.len() >= pos + self.elems.len()
            && self
                .elems
                .iter()
                .zip(&data[pos..])
                .all(|(e, &b)| e.matches(b))
    }

    /// First match starting at or after `from`.
    pub fn find_forward(&self, data: &[u8], from: usize) -> Option<usize> {
        let n = self.elems.len();
        if self.is_empty() || from >= data.len() || data.len() - from < n {
            return None;
        }
        if let Some(exact) = self.exact_bytes() {
            return memmem::find(&data[from..], &exact).map(|p| p + from);
        }
        let Some((ai, anchor)) = self.anchor() else {
            // All-wildcard pattern: checks each position linearly.
            return (from..=data.len() - n).find(|&p| self.matches_at(data, p));
        };
        let mut search_from = from + ai;
        let last_anchor = data.len() - n + ai;
        while search_from <= last_anchor {
            let hay = &data[search_from..=last_anchor];
            let hit = match anchor {
                Elem::Masked { value, .. } => memchr::memchr(value, hay),
                Elem::Either(a, b) => memchr::memchr2(a, b, hay),
            }?;
            let pos = search_from + hit - ai;
            if self.matches_at(data, pos) {
                return Some(pos);
            }
            search_from += hit + 1;
        }
        None
    }

    /// Last match starting strictly before `before`.
    pub fn find_backward(&self, data: &[u8], before: usize) -> Option<usize> {
        let n = self.elems.len();
        if n == 0 || data.len() < n || before == 0 {
            return None;
        }
        let last_start = (before - 1).min(data.len() - n);
        if let Some(exact) = self.exact_bytes() {
            return memmem::rfind(&data[..last_start + n], &exact);
        }
        let Some((ai, anchor)) = self.anchor() else {
            return (0..=last_start).rev().find(|&p| self.matches_at(data, p));
        };
        let mut end = last_start + ai + 1;
        while end > ai {
            let hay = &data[ai..end];
            let hit = match anchor {
                Elem::Masked { value, .. } => memchr::memrchr(value, hay),
                Elem::Either(a, b) => memchr::memrchr2(a, b, hay),
            }?;
            let pos = hit;
            if self.matches_at(data, pos) {
                return Some(pos);
            }
            end = ai + hit;
        }
        None
    }

    /// All non-overlapping matches, up to `limit`. Returns `(hits, truncated)`.
    pub fn find_all(&self, data: &[u8], limit: usize) -> (Vec<usize>, bool) {
        let mut hits = Vec::new();
        let mut pos = 0;
        let step = self.elems.len().max(1);
        while let Some(p) = self.find_forward(data, pos) {
            if hits.len() >= limit {
                return (hits, true);
            }
            hits.push(p);
            pos = p + step;
        }
        (hits, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parse_and_wildcards() {
        let p = Pattern::from_hex("4D 5A ?? 00").unwrap();
        assert_eq!(p.len(), 4);
        let data = b"xxMZ\x90\x00yyMZ\x01\x01";
        assert_eq!(p.find_forward(data, 0), Some(2));
        assert_eq!(p.find_forward(data, 3), None);
        assert!(Pattern::from_hex("4G").is_err());
        assert!(Pattern::from_hex("123").is_err());
        assert_eq!(
            Pattern::from_hex("0x41 0x42").unwrap(),
            Pattern::exact(b"AB")
        );
        assert_eq!(Pattern::from_hex("4142").unwrap(), Pattern::exact(b"AB"));
    }

    #[test]
    fn nibble_wildcards() {
        let p = Pattern::from_hex("?1").unwrap();
        assert_eq!(p.find_forward(&[0x00, 0x20, 0xA1], 0), Some(2));
        let p = Pattern::from_hex("A?").unwrap();
        assert_eq!(p.find_backward(&[0xA1, 0x00, 0xAF, 0x00], 4), Some(2));
    }

    #[test]
    fn case_insensitive_text() {
        let p = Pattern::from_text("hello", TextSearchEncoding::Utf8, false);
        assert_eq!(p.find_forward(b"say HeLLo there", 0), Some(4));
        let p = Pattern::from_text("Hi", TextSearchEncoding::Utf16Le, false);
        assert_eq!(p.find_forward(b"..h\0I\0", 0), Some(2));
        let p = Pattern::from_text("Hi", TextSearchEncoding::Utf16Be, false);
        assert_eq!(p.find_forward(b".\0h\0I", 0), Some(1));
    }

    #[test]
    fn forward_backward_exact() {
        let data = b"abcabcabc";
        let p = Pattern::exact(b"abc");
        assert_eq!(p.find_forward(data, 1), Some(3));
        assert_eq!(p.find_backward(data, 6), Some(3));
        assert_eq!(p.find_backward(data, 3), Some(0));
        assert_eq!(p.find_backward(data, 0), None);
        assert_eq!(p.find_all(data, 10), (vec![0, 3, 6], false));
        assert_eq!(p.find_all(data, 2), (vec![0, 3], true));
    }

    #[test]
    fn backward_with_anchor() {
        let data = b"aXbaYbaZb";
        let p = Pattern::from_hex("61 ?? 62").unwrap();
        assert_eq!(p.find_backward(data, 9), Some(6));
        assert_eq!(p.find_backward(data, 6), Some(3));
        assert_eq!(p.find_backward(data, 1), Some(0));
    }

    #[test]
    fn value_search() {
        let spec = QuerySpec {
            kind: SearchKind::Value,
            num_type: NumType::U16,
            big_endian: true,
            ..Default::default()
        };
        let p = spec.build("0x1234").unwrap();
        assert_eq!(p.find_forward(&[0, 0x12, 0x34], 0), Some(1));
    }
}

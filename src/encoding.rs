//! Single-byte character encodings used by the text column of the hex view.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextEncoding {
    #[default]
    Ascii,
    Windows1252,
    Cp437,
    Ebcdic,
}

impl TextEncoding {
    pub const ALL: [Self; 4] = [Self::Ascii, Self::Windows1252, Self::Cp437, Self::Ebcdic];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ascii => "ASCII",
            Self::Windows1252 => "Windows-1252",
            Self::Cp437 => "CP437 (DOS)",
            Self::Ebcdic => "EBCDIC (CP037)",
        }
    }

    /// Returns the printable character for `b`, or `None` if the byte is a
    /// control/unmapped code that should be rendered as a placeholder.
    #[inline]
    pub fn decode(self, b: u8) -> Option<char> {
        self.table()[b as usize]
    }

    fn table(self) -> &'static [Option<char>; 256] {
        static TABLES: [OnceLock<[Option<char>; 256]>; 4] = [
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
            OnceLock::new(),
        ];
        TABLES[self as usize].get_or_init(|| std::array::from_fn(|b| self.decode_uncached(b as u8)))
    }

    fn decode_uncached(self, b: u8) -> Option<char> {
        match self {
            Self::Ascii => (0x20..0x7f).contains(&b).then_some(b as char),
            Self::Windows1252 => match b {
                0x20..=0x7e => Some(b as char),
                0x80..=0x9f => {
                    let c = CP1252_HIGH[(b - 0x80) as usize];
                    (c != '\0').then_some(c)
                }
                0xa0 | 0xad => None,
                0xa1..=0xff => Some(b as char),
                _ => None,
            },
            Self::Cp437 => match b {
                0x00 | 0x20 | 0xff => None,
                0x01..=0x1f => CP437_LOW.chars().nth(b as usize - 1),
                0x21..=0x7e => Some(b as char),
                0x7f => Some('⌂'),
                0x80..=0xfe => CP437_HIGH.chars().nth(b as usize - 0x80),
            },
            Self::Ebcdic => {
                let c = EBCDIC_037.chars().nth(b as usize).unwrap_or('\0');
                (c != '\0').then_some(c)
            }
        }
    }

    /// Maps a typed character back to a byte in this encoding.
    pub fn encode(self, c: char) -> Option<u8> {
        if c == ' ' {
            return Some(match self {
                Self::Ebcdic => 0x40,
                _ => 0x20,
            });
        }
        (0..=255u8).find(|&b| self.decode(b) == Some(c))
    }

    /// Encodes a string, replacing unmappable characters with `?`.
    pub fn encode_str(self, s: &str) -> Vec<u8> {
        let fallback = self.encode('?').unwrap_or(b'?');
        s.chars()
            .map(|c| self.encode(c).unwrap_or(fallback))
            .collect()
    }

    /// Decodes bytes into a display string, using `.` for unprintable bytes.
    pub fn decode_lossy(self, bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|&b| match b {
                b'\n' | b'\t' if self != Self::Ebcdic => b as char,
                _ => self.decode(b).unwrap_or('.'),
            })
            .collect()
    }
}

const CP1252_HIGH: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', //
    '\0', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];

const CP437_LOW: &str = "☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼";

const CP437_HIGH: &str = concat!(
    "ÇüéâäàåçêëèïîìÄÅ",
    "ÉæÆôöòûùÿÖÜ¢£¥₧ƒ",
    "áíóúñÑªº¿⌐¬½¼¡«»",
    "░▒▓│┤╡╢╖╕╣║╗╝╜╛┐",
    "└┴┬├─┼╞╟╚╔╩╦╠═╬╧",
    "╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀",
    "αßΓπΣσµτΦΘΩδ∞φε∩",
    "≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}",
);

/// EBCDIC code page 037; `\0` marks control codes.
const EBCDIC_037: &str = concat!(
    "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    " \0âäàáãåçñ¢.<(+|",
    "&éêëèíîïìß!$*);¬",
    "-/ÂÄÀÁÃÅÇÑ¦,%_>?",
    "øÉÊËÈÍÎÏÌ`:#@'=\"",
    "Øabcdefghi«»ðýþ±",
    "°jklmnopqrªºæ¸Æ¤",
    "µ~stuvwxyz¡¿ÐÝÞ®",
    "^£¥·©§¶¼½¾[]¯¨´×",
    "{ABCDEFGHI\0ôöòóõ",
    "}JKLMNOPQR¹ûüùúÿ",
    "\\÷STUVWXYZ²ÔÖÒÓÕ",
    "0123456789³ÛÜÙÚ\0",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_have_expected_sizes() {
        assert_eq!(CP437_LOW.chars().count(), 31);
        assert_eq!(CP437_HIGH.chars().count(), 128);
        assert_eq!(EBCDIC_037.chars().count(), 256);
    }

    #[test]
    fn ascii_roundtrip() {
        for b in 0x20..0x7f {
            let c = TextEncoding::Ascii.decode(b).unwrap();
            assert_eq!(TextEncoding::Ascii.encode(c), Some(b));
        }
        assert_eq!(TextEncoding::Ascii.decode(0x00), None);
        assert_eq!(TextEncoding::Ascii.decode(0x80), None);
    }

    #[test]
    fn ebcdic_letters() {
        assert_eq!(TextEncoding::Ebcdic.decode(0xc1), Some('A'));
        assert_eq!(TextEncoding::Ebcdic.decode(0x81), Some('a'));
        assert_eq!(TextEncoding::Ebcdic.decode(0xf0), Some('0'));
        assert_eq!(TextEncoding::Ebcdic.encode('Z'), Some(0xe9));
        assert_eq!(TextEncoding::Ebcdic.encode(' '), Some(0x40));
    }

    #[test]
    fn cp437_glyphs() {
        assert_eq!(TextEncoding::Cp437.decode(0x01), Some('☺'));
        assert_eq!(TextEncoding::Cp437.decode(0xdb), Some('█'));
        assert_eq!(TextEncoding::Cp437.encode('█'), Some(0xdb));
    }
}

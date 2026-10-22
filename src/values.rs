//! Numeric value encoding/decoding shared by search, the data inspector and
//! the go-to dialog.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NumType {
    U8,
    I8,
    U16,
    I16,
    #[default]
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

impl NumType {
    pub const ALL: [Self; 10] = [
        Self::U8,
        Self::I8,
        Self::U16,
        Self::I16,
        Self::U32,
        Self::I32,
        Self::U64,
        Self::I64,
        Self::F32,
        Self::F64,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::U8 => "uint8",
            Self::I8 => "int8",
            Self::U16 => "uint16",
            Self::I16 => "int16",
            Self::U32 => "uint32",
            Self::I32 => "int32",
            Self::U64 => "uint64",
            Self::I64 => "int64",
            Self::F32 => "float32",
            Self::F64 => "float64",
        }
    }

    pub fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }

    /// Parses `text` and encodes it into bytes with the given endianness.
    pub fn encode(self, text: &str, big_endian: bool) -> Result<Vec<u8>, String> {
        let t = text.trim().replace('_', "");
        macro_rules! int {
            ($ty:ty) => {{
                let v = parse_int::<$ty>(&t)?;
                if big_endian {
                    v.to_be_bytes().to_vec()
                } else {
                    v.to_le_bytes().to_vec()
                }
            }};
        }
        macro_rules! float {
            ($ty:ty) => {{
                let v: $ty = t
                    .parse()
                    .map_err(|_| format!("'{text}' is not a valid number"))?;
                if big_endian {
                    v.to_be_bytes().to_vec()
                } else {
                    v.to_le_bytes().to_vec()
                }
            }};
        }
        Ok(match self {
            Self::U8 => int!(u8),
            Self::I8 => int!(i8),
            Self::U16 => int!(u16),
            Self::I16 => int!(i16),
            Self::U32 => int!(u32),
            Self::I32 => int!(i32),
            Self::U64 => int!(u64),
            Self::I64 => int!(i64),
            Self::F32 => float!(f32),
            Self::F64 => float!(f64),
        })
    }

    /// Decodes the leading bytes of `bytes`; `None` if there aren't enough.
    pub fn decode(self, bytes: &[u8], big_endian: bool) -> Option<String> {
        let b = bytes.get(..self.size())?;
        macro_rules! conv {
            ($ty:ty) => {{
                let arr = b.try_into().ok()?;
                if big_endian {
                    <$ty>::from_be_bytes(arr)
                } else {
                    <$ty>::from_le_bytes(arr)
                }
            }};
        }
        Some(match self {
            Self::U8 => b[0].to_string(),
            Self::I8 => (b[0] as i8).to_string(),
            Self::U16 => conv!(u16).to_string(),
            Self::I16 => conv!(i16).to_string(),
            Self::U32 => conv!(u32).to_string(),
            Self::I32 => conv!(i32).to_string(),
            Self::U64 => conv!(u64).to_string(),
            Self::I64 => conv!(i64).to_string(),
            Self::F32 => format_float(conv!(f32) as f64, true),
            Self::F64 => format_float(conv!(f64), false),
        })
    }
}

pub fn format_float(v: f64, single: bool) -> String {
    if v.is_nan() || v.is_infinite() {
        return v.to_string();
    }
    let a = v.abs();
    if a != 0.0 && !(1e-6..1e15).contains(&a) {
        format!("{v:e}")
    } else if single {
        format!("{}", v as f32)
    } else {
        format!("{v}")
    }
}

/// Parses an integer that may be written in decimal, `0x` hex, `0o` octal or
/// `0b` binary, with an optional leading minus sign.
pub fn parse_int<T>(text: &str) -> Result<T, String>
where
    T: TryFrom<i128>,
{
    let v = parse_i128(text)?;
    T::try_from(v).map_err(|_| format!("{text} is out of range"))
}

pub fn parse_i128(text: &str) -> Result<i128, String> {
    let t = text.trim().replace('_', "");
    let (neg, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(&t)),
    };
    let lower = body.to_ascii_lowercase();
    let (radix, digits) = if let Some(h) = lower.strip_prefix("0x") {
        (16, h)
    } else if let Some(o) = lower.strip_prefix("0o") {
        (8, o)
    } else if let Some(b) = lower.strip_prefix("0b") {
        (2, b)
    } else if let Some(h) = lower.strip_suffix('h') {
        (16, h)
    } else {
        (10, lower.as_str())
    };
    if digits.is_empty() {
        return Err("enter a number".into());
    }
    let v = i128::from_str_radix(digits, radix)
        .map_err(|_| format!("'{text}' is not a valid number"))?;
    Ok(if neg { -v } else { v })
}

/// Converts an IEEE 754 half-precision value to f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f32;
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        0x1f if frac == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// Converts an f32 to IEEE 754 half precision (round-to-nearest-even).
pub fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let mid = 1 << (shift - 1);
        let round = rem > mid || (rem == mid && half & 1 == 1);
        return sign | (half + round as u32) as u16;
    }
    let half = ((e as u32) << 10) | (mant >> 13);
    let rem = mant & 0x1fff;
    let round = rem > 0x1000 || (rem == 0x1000 && half & 1 == 1);
    sign | (half + round as u32) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_ints() {
        assert_eq!(
            NumType::U16.encode("0x1234", false).unwrap(),
            vec![0x34, 0x12]
        );
        assert_eq!(NumType::U16.encode("4660", true).unwrap(), vec![0x12, 0x34]);
        assert_eq!(NumType::I8.encode("-1", false).unwrap(), vec![0xff]);
        assert!(NumType::U8.encode("256", false).is_err());
        assert_eq!(
            NumType::I32
                .decode(&[0xff, 0xff, 0xff, 0xff], false)
                .unwrap(),
            "-1"
        );
        assert_eq!(NumType::U32.decode(&[0, 0, 1, 0], true).unwrap(), "256");
        assert!(NumType::U64.decode(&[0; 4], false).is_none());
    }

    #[test]
    fn floats() {
        let b = NumType::F32.encode("1.5", false).unwrap();
        assert_eq!(NumType::F32.decode(&b, false).unwrap(), "1.5");
        let b = NumType::F64.encode("-2.25", true).unwrap();
        assert_eq!(NumType::F64.decode(&b, true).unwrap(), "-2.25");
    }

    #[test]
    fn parse_radix() {
        assert_eq!(parse_i128("0x10").unwrap(), 16);
        assert_eq!(parse_i128("10h").unwrap(), 16);
        assert_eq!(parse_i128("0b101").unwrap(), 5);
        assert_eq!(parse_i128("-0o17").unwrap(), -15);
        assert_eq!(parse_i128("1_000").unwrap(), 1000);
        assert!(parse_i128("zz").is_err());
    }

    #[test]
    fn half_precision() {
        for v in [0.0f32, 1.0, -2.5, 65504.0, 0.000061035156] {
            assert_eq!(f16_to_f32(f32_to_f16(v)), v);
        }
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert!(f16_to_f32(0x7e00).is_nan());
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
    }
}

//! Byte-wise transformations applied to a selection.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransformOp {
    #[default]
    Xor,
    And,
    Or,
    Add,
    Subtract,
    Not,
    Negate,
    ShiftLeft,
    ShiftRight,
    RotateLeft,
    RotateRight,
    ReverseBits,
    ReverseBytes,
    Swap16,
    Swap32,
    Swap64,
    Random,
    Increment,
}

impl TransformOp {
    pub const ALL: [Self; 18] = [
        Self::Xor,
        Self::And,
        Self::Or,
        Self::Add,
        Self::Subtract,
        Self::Not,
        Self::Negate,
        Self::ShiftLeft,
        Self::ShiftRight,
        Self::RotateLeft,
        Self::RotateRight,
        Self::ReverseBits,
        Self::ReverseBytes,
        Self::Swap16,
        Self::Swap32,
        Self::Swap64,
        Self::Random,
        Self::Increment,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Xor => "XOR with key",
            Self::And => "AND with key",
            Self::Or => "OR with key",
            Self::Add => "Add key (mod 256)",
            Self::Subtract => "Subtract key (mod 256)",
            Self::Not => "NOT (invert bits)",
            Self::Negate => "Negate (two's complement)",
            Self::ShiftLeft => "Shift left",
            Self::ShiftRight => "Shift right",
            Self::RotateLeft => "Rotate left",
            Self::RotateRight => "Rotate right",
            Self::ReverseBits => "Reverse bits in each byte",
            Self::ReverseBytes => "Reverse byte order",
            Self::Swap16 => "Swap endianness (16-bit)",
            Self::Swap32 => "Swap endianness (32-bit)",
            Self::Swap64 => "Swap endianness (64-bit)",
            Self::Random => "Fill with random bytes",
            Self::Increment => "Fill with increasing sequence",
        }
    }

    pub fn needs_key(self) -> bool {
        matches!(
            self,
            Self::Xor | Self::And | Self::Or | Self::Add | Self::Subtract
        )
    }

    pub fn needs_bits(self) -> bool {
        matches!(
            self,
            Self::ShiftLeft | Self::ShiftRight | Self::RotateLeft | Self::RotateRight
        )
    }

    /// Applies the operation in place. `key` must be non-empty for keyed ops.
    pub fn apply(self, data: &mut [u8], key: &[u8], bits: u32) {
        let bits = bits % 8;
        let keyed = |data: &mut [u8], f: fn(u8, u8) -> u8| {
            if key.is_empty() {
                return;
            }
            for (b, &k) in data.iter_mut().zip(key.iter().cycle()) {
                *b = f(*b, k);
            }
        };
        match self {
            Self::Xor => keyed(data, |a, k| a ^ k),
            Self::And => keyed(data, |a, k| a & k),
            Self::Or => keyed(data, |a, k| a | k),
            Self::Add => keyed(data, u8::wrapping_add),
            Self::Subtract => keyed(data, u8::wrapping_sub),
            Self::Not => data.iter_mut().for_each(|b| *b = !*b),
            Self::Negate => data.iter_mut().for_each(|b| *b = b.wrapping_neg()),
            Self::ShiftLeft => data.iter_mut().for_each(|b| *b <<= bits),
            Self::ShiftRight => data.iter_mut().for_each(|b| *b >>= bits),
            Self::RotateLeft => data.iter_mut().for_each(|b| *b = b.rotate_left(bits)),
            Self::RotateRight => data.iter_mut().for_each(|b| *b = b.rotate_right(bits)),
            Self::ReverseBits => data.iter_mut().for_each(|b| *b = b.reverse_bits()),
            Self::ReverseBytes => data.reverse(),
            Self::Swap16 => data.chunks_exact_mut(2).for_each(<[u8]>::reverse),
            Self::Swap32 => data.chunks_exact_mut(4).for_each(<[u8]>::reverse),
            Self::Swap64 => data.chunks_exact_mut(8).for_each(<[u8]>::reverse),
            Self::Random => fill_random(data),
            Self::Increment => {
                let start = key.first().copied().unwrap_or(0);
                for (i, b) in data.iter_mut().enumerate() {
                    *b = start.wrapping_add(i as u8);
                }
            }
        }
    }
}

/// Fills `data` with pseudo-random bytes (xorshift64*, time-seeded).
/// Not suitable for cryptographic use.
pub fn fill_random(data: &mut [u8]) {
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64)
        | 1;
    for chunk in data.chunks_mut(8) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let v = state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
}

/// Repeats `pattern` to fill `len` bytes.
pub fn repeat_pattern(pattern: &[u8], len: usize) -> Vec<u8> {
    if pattern.is_empty() {
        return vec![0; len];
    }
    pattern.iter().copied().cycle().take(len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_ops() {
        let mut d = [0x0f, 0xf0, 0xff];
        TransformOp::Xor.apply(&mut d, &[0xff], 0);
        assert_eq!(d, [0xf0, 0x0f, 0x00]);
        TransformOp::Add.apply(&mut d, &[0x01, 0x02], 0);
        assert_eq!(d, [0xf1, 0x11, 0x01]);
    }

    #[test]
    fn bit_ops() {
        let mut d = [0b1000_0001];
        TransformOp::RotateLeft.apply(&mut d, &[], 1);
        assert_eq!(d, [0b0000_0011]);
        TransformOp::ReverseBits.apply(&mut d, &[], 0);
        assert_eq!(d, [0b1100_0000]);
        TransformOp::ShiftRight.apply(&mut d, &[], 6);
        assert_eq!(d, [0b11]);
    }

    #[test]
    fn swaps() {
        let mut d = [1, 2, 3, 4, 5];
        TransformOp::Swap16.apply(&mut d, &[], 0);
        assert_eq!(d, [2, 1, 4, 3, 5]);
        TransformOp::ReverseBytes.apply(&mut d, &[], 0);
        assert_eq!(d, [5, 3, 4, 1, 2]);
    }

    #[test]
    fn patterns() {
        assert_eq!(repeat_pattern(&[1, 2], 5), vec![1, 2, 1, 2, 1]);
        assert_eq!(repeat_pattern(&[], 2), vec![0, 0]);
        let mut d = [0u8; 300];
        TransformOp::Increment.apply(&mut d, &[0xfe], 0);
        assert_eq!(&d[..3], &[0xfe, 0xff, 0x00]);
    }
}

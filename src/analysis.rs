//! Checksums, statistics, string extraction, file-type detection and
//! minimap summaries. Everything here is pure and safe to run on a
//! background thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::export::to_hex;

// ---------------------------------------------------------------------------
// Background jobs

/// A computation running on a worker thread.
pub struct Job<T> {
    rx: mpsc::Receiver<T>,
    cancel: Arc<AtomicBool>,
}

pub enum JobPoll<T> {
    Pending,
    Done(T),
    Failed,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(ctx: &egui::Context, f: impl FnOnce(&AtomicBool) -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = f(&flag);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self { rx, cancel }
    }

    pub fn poll(&self) -> JobPoll<T> {
        match self.rx.try_recv() {
            Ok(v) => JobPoll::Done(v),
            Err(mpsc::TryRecvError::Empty) => JobPoll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => JobPoll::Failed,
        }
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Size of the work units between cancellation checks.
const CHUNK: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Checksums & hashes

#[derive(Clone, Debug)]
pub struct HashResult {
    pub range: (usize, usize),
    pub version: u64,
    pub values: Vec<(&'static str, String)>,
}

pub fn compute_hashes(data: &[u8], cancel: &AtomicBool) -> Option<Vec<(&'static str, String)>> {
    let mut crc32 = crc32fast::Hasher::new();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut adler = Adler32::default();
    let mut crc16 = 0xffffu16;
    let mut sum8 = 0u8;
    let mut sum16 = 0u16;
    let mut sum32 = 0u32;
    let mut xor8 = 0u8;

    for chunk in data.chunks(CHUNK) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        crc32.update(chunk);
        md5.update(chunk);
        sha1.update(chunk);
        sha256.update(chunk);
        sha512.update(chunk);
        adler.update(chunk);
        crc16 = crc16_ccitt(crc16, chunk);
        for &b in chunk {
            sum8 = sum8.wrapping_add(b);
            sum16 = sum16.wrapping_add(b as u16);
            sum32 = sum32.wrapping_add(b as u32);
            xor8 ^= b;
        }
    }

    Some(vec![
        ("Sum-8", format!("{sum8:02X}")),
        ("Sum-16", format!("{sum16:04X}")),
        ("Sum-32", format!("{sum32:08X}")),
        ("XOR-8", format!("{xor8:02X}")),
        ("CRC-16/CCITT", format!("{crc16:04X}")),
        ("CRC-32", format!("{:08X}", crc32.finalize())),
        ("Adler-32", format!("{:08X}", adler.finish())),
        ("MD5", to_hex(&md5.finalize(), false)),
        ("SHA-1", to_hex(&sha1.finalize(), false)),
        ("SHA-256", to_hex(&sha256.finalize(), false)),
        ("SHA-512", to_hex(&sha512.finalize(), false)),
    ])
}

struct Adler32 {
    a: u32,
    b: u32,
}

impl Default for Adler32 {
    fn default() -> Self {
        Self { a: 1, b: 0 }
    }
}

impl Adler32 {
    fn update(&mut self, data: &[u8]) {
        const MOD: u32 = 65521;
        for chunk in data.chunks(5552) {
            for &byte in chunk {
                self.a += byte as u32;
                self.b += self.a;
            }
            self.a %= MOD;
            self.b %= MOD;
        }
    }

    fn finish(&self) -> u32 {
        (self.b << 16) | self.a
    }
}

fn crc16_ccitt(mut crc: u16, data: &[u8]) -> u16 {
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// Statistics

#[derive(Clone, Debug)]
pub struct Stats {
    pub range: (usize, usize),
    pub version: u64,
    pub histogram: Box<[u64; 256]>,
    pub total: u64,
    pub entropy: f64,
    pub zeros: u64,
    pub printable: u64,
    pub whitespace: u64,
    pub control: u64,
    pub high: u64,
    pub mean: f64,
}

pub fn compute_histogram(data: &[u8], cancel: &AtomicBool) -> Option<Box<[u64; 256]>> {
    let mut hist = Box::new([0u64; 256]);
    for chunk in data.chunks(CHUNK) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        for &b in chunk {
            hist[b as usize] += 1;
        }
    }
    Some(hist)
}

pub fn entropy_of(hist: &[u64; 256], total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let t = total as f64;
    hist.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / t;
            -p * p.log2()
        })
        .sum()
}

pub fn compute_stats(
    data: &[u8],
    range: (usize, usize),
    version: u64,
    cancel: &AtomicBool,
) -> Option<Stats> {
    let histogram = compute_histogram(data, cancel)?;
    let total = data.len() as u64;
    let sum_range = |r: std::ops::RangeInclusive<usize>| -> u64 { histogram[r].iter().sum() };
    let whitespace = [0x09usize, 0x0a, 0x0d, 0x20]
        .iter()
        .map(|&i| histogram[i])
        .sum();
    let weighted: f64 = histogram
        .iter()
        .enumerate()
        .map(|(i, &c)| i as f64 * c as f64)
        .sum();
    Some(Stats {
        range,
        version,
        entropy: entropy_of(&histogram, total),
        zeros: histogram[0],
        printable: sum_range(0x21..=0x7e),
        whitespace,
        control: sum_range(0x01..=0x1f) + histogram[0x7f]
            - histogram[0x09]
            - histogram[0x0a]
            - histogram[0x0d],
        high: sum_range(0x80..=0xff),
        mean: if total > 0 {
            weighted / total as f64
        } else {
            0.0
        },
        total,
        histogram,
    })
}

// ---------------------------------------------------------------------------
// String extraction

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringKind {
    Ascii,
    Utf16Le,
}

#[derive(Clone, Debug)]
pub struct FoundString {
    pub offset: usize,
    pub byte_len: usize,
    pub kind: StringKind,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct StringsResult {
    pub version: u64,
    pub items: Vec<FoundString>,
    pub truncated: bool,
}

fn is_string_byte(b: u8) -> bool {
    (0x20..0x7f).contains(&b) || b == b'\t'
}

pub fn extract_strings(
    data: &[u8],
    min_len: usize,
    utf16: bool,
    limit: usize,
    cancel: &AtomicBool,
) -> Option<(Vec<FoundString>, bool)> {
    let min_len = min_len.max(1);
    let mut out = Vec::new();

    // ASCII runs
    let mut start = None;
    for (i, &b) in data.iter().enumerate() {
        if i % CHUNK == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        if is_string_byte(b) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take()
            && i - s >= min_len
        {
            out.push(make_ascii(data, s, i));
        }
    }
    if let Some(s) = start
        && data.len() - s >= min_len
    {
        out.push(make_ascii(data, s, data.len()));
    }

    if utf16 {
        for parity in 0..2 {
            let mut start = None;
            let mut i = parity;
            while i + 1 < data.len() {
                if i % CHUNK < 2 && cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let ok = is_string_byte(data[i]) && data[i + 1] == 0;
                if ok {
                    start.get_or_insert(i);
                } else if let Some(s) = start.take()
                    && (i - s) / 2 >= min_len
                {
                    out.push(make_utf16(data, s, i));
                }
                i += 2;
            }
            if let Some(s) = start
                && (i - s) / 2 >= min_len
            {
                out.push(make_utf16(data, s, i));
            }
        }
        out.sort_by_key(|s| s.offset);
    }

    let truncated = out.len() > limit;
    out.truncate(limit);
    Some((out, truncated))
}

const MAX_STRING_PREVIEW: usize = 256;

fn make_ascii(data: &[u8], s: usize, e: usize) -> FoundString {
    let shown = &data[s..e.min(s + MAX_STRING_PREVIEW)];
    FoundString {
        offset: s,
        byte_len: e - s,
        kind: StringKind::Ascii,
        text: shown.iter().map(|&b| b as char).collect(),
    }
}

fn make_utf16(data: &[u8], s: usize, e: usize) -> FoundString {
    let shown = &data[s..e.min(s + MAX_STRING_PREVIEW * 2)];
    FoundString {
        offset: s,
        byte_len: e - s,
        kind: StringKind::Utf16Le,
        text: shown.iter().step_by(2).map(|&b| b as char).collect(),
    }
}

// ---------------------------------------------------------------------------
// Minimap

#[derive(Clone, Copy, Debug, Default)]
pub struct BlockInfo {
    /// Shannon entropy normalised to `0..=1`.
    pub entropy: f32,
    pub zero: f32,
    pub ascii: f32,
    pub high: f32,
}

#[derive(Clone, Debug)]
pub struct MinimapData {
    pub version: u64,
    pub blocks: Vec<BlockInfo>,
}

pub const MINIMAP_BLOCKS: usize = 2048;

pub fn compute_minimap(data: &[u8], version: u64, cancel: &AtomicBool) -> Option<MinimapData> {
    let n = MINIMAP_BLOCKS.min(data.len().max(1));
    let block = data.len().div_ceil(n).max(1);
    let mut blocks = Vec::with_capacity(n);
    for chunk in data.chunks(block) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // Sample large blocks to bound the cost on huge files.
        let sample = &chunk[..chunk.len().min(64 * 1024)];
        let mut hist = [0u64; 256];
        for &b in sample {
            hist[b as usize] += 1;
        }
        let total = sample.len() as u64;
        let t = total.max(1) as f32;
        let ascii: u64 = hist[0x20..0x7f].iter().sum::<u64>() + hist[9] + hist[10] + hist[13];
        let high: u64 = hist[0x80..].iter().sum();
        let max_entropy = (total as f64).log2().clamp(1.0, 8.0);
        blocks.push(BlockInfo {
            entropy: (entropy_of(&hist, total) / max_entropy).clamp(0.0, 1.0) as f32,
            zero: hist[0] as f32 / t,
            ascii: ascii as f32 / t,
            high: high as f32 / t,
        });
    }
    Some(MinimapData { version, blocks })
}

// ---------------------------------------------------------------------------
// Comparison

/// First differing offset at or after `from`, treating length differences as
/// differences.
pub fn next_difference(a: &[u8], b: &[u8], from: usize) -> Option<usize> {
    let common = a.len().min(b.len());
    let mut i = from;
    while i < common {
        let end = (i + 4096).min(common);
        if a[i..end] != b[i..end] {
            return (i..end).find(|&j| a[j] != b[j]);
        }
        i = end;
    }
    let max = a.len().max(b.len());
    (from.max(common) < max).then_some(from.max(common))
}

pub fn prev_difference(a: &[u8], b: &[u8], before: usize) -> Option<usize> {
    let common = a.len().min(b.len());
    let max = a.len().max(b.len());
    if before > common && common < max {
        return Some(before - 1);
    }
    let mut i = before.min(common);
    while i > 0 {
        let start = i.saturating_sub(4096);
        if a[start..i] != b[start..i] {
            return (start..i).rev().find(|&j| a[j] != b[j]);
        }
        i = start;
    }
    None
}

pub fn count_differences(a: &[u8], b: &[u8]) -> usize {
    let common = a.len().min(b.len());
    let mut n = a.len().max(b.len()) - common;
    for (ca, cb) in a[..common].chunks(4096).zip(b[..common].chunks(4096)) {
        if ca != cb {
            n += ca.iter().zip(cb).filter(|(x, y)| x != y).count();
        }
    }
    n
}

// ---------------------------------------------------------------------------
// File type detection

pub fn detect_file_type(data: &[u8]) -> Option<&'static str> {
    const SIGS: &[(usize, &[u8], &str)] = &[
        (0, b"\x89PNG\r\n\x1a\n", "PNG image"),
        (0, b"\xff\xd8\xff", "JPEG image"),
        (0, b"GIF87a", "GIF image"),
        (0, b"GIF89a", "GIF image"),
        (0, b"BM", "BMP image"),
        (0, b"II*\x00", "TIFF image"),
        (0, b"MM\x00*", "TIFF image"),
        (0, b"\x00\x00\x01\x00", "ICO icon"),
        (0, b"%PDF", "PDF document"),
        (0, b"PK\x03\x04", "ZIP archive (or DOCX/JAR/APK)"),
        (0, b"PK\x05\x06", "ZIP archive (empty)"),
        (0, b"Rar!\x1a\x07", "RAR archive"),
        (0, b"7z\xbc\xaf\x27\x1c", "7-Zip archive"),
        (0, b"\x1f\x8b", "GZIP compressed"),
        (0, b"BZh", "BZIP2 compressed"),
        (0, b"\xfd7zXZ\x00", "XZ compressed"),
        (0, b"\x28\xb5\x2f\xfd", "Zstandard compressed"),
        (0, b"\x04\x22\x4d\x18", "LZ4 frame"),
        (0, b"\x7fELF", "ELF executable"),
        (0, b"MZ", "DOS/Windows executable (MZ)"),
        (0, b"\xca\xfe\xba\xbe", "Mach-O universal / Java class"),
        (0, b"\xcf\xfa\xed\xfe", "Mach-O 64-bit"),
        (0, b"\xce\xfa\xed\xfe", "Mach-O 32-bit"),
        (0, b"\x00asm", "WebAssembly module"),
        (0, b"dex\n", "Android DEX"),
        (0, b"SQLite format 3\x00", "SQLite database"),
        (0, b"OggS", "OGG media"),
        (0, b"fLaC", "FLAC audio"),
        (0, b"ID3", "MP3 audio (ID3)"),
        (0, b"\x1aE\xdf\xa3", "Matroska/WebM video"),
        (4, b"ftyp", "MP4/QuickTime media"),
        (0, b"wOFF", "WOFF font"),
        (0, b"wOF2", "WOFF2 font"),
        (0, b"\x00\x01\x00\x00\x00", "TrueType font"),
        (0, b"OTTO", "OpenType font"),
        (
            0,
            b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1",
            "MS Office legacy (OLE2)",
        ),
        (0, b"{\\rtf", "RTF document"),
        (0, b"<?xml", "XML document"),
        (0, b"\xef\xbb\xbf", "UTF-8 text (BOM)"),
        (0, b"\xff\xfe", "UTF-16 LE text (BOM)"),
        (0, b"\xfe\xff", "UTF-16 BE text (BOM)"),
        (0, b"#!", "Script (shebang)"),
        (257, b"ustar", "TAR archive"),
        (
            0,
            b"\x4c\x00\x00\x00\x01\x14\x02\x00",
            "Windows shortcut (LNK)",
        ),
        (0, b"MSCF", "Microsoft Cabinet"),
        (0, b"!<arch>\n", "Unix archive (ar/.deb)"),
        (0, b"\xed\xab\xee\xdb", "RPM package"),
        (0, b"8BPS", "Photoshop document"),
        (0, b"RIFF", "RIFF container (WAV/AVI/WEBP)"),
    ];
    if data.starts_with(b"RIFF") && data.len() >= 12 {
        match &data[8..12] {
            b"WAVE" => return Some("WAV audio"),
            b"AVI " => return Some("AVI video"),
            b"WEBP" => return Some("WEBP image"),
            _ => {}
        }
    }
    SIGS.iter()
        .find(|(off, sig, _)| data.get(*off..*off + sig.len()) == Some(*sig))
        .map(|&(_, _, name)| name)
        .or_else(|| {
            let head = &data[..data.len().min(4096)];
            (!head.is_empty()
                && head
                    .iter()
                    .all(|&b| is_string_byte(b) || b == b'\n' || b == b'\r'))
            .then_some("Plain text")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn known_hashes() {
        let h = compute_hashes(b"abc", &no_cancel()).unwrap();
        let get = |n: &str| h.iter().find(|(k, _)| *k == n).unwrap().1.clone();
        assert_eq!(get("CRC-32"), "352441C2");
        assert_eq!(get("MD5"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(get("SHA-1"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            get("SHA-256"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(get("Adler-32"), "024D0127");
        assert_eq!(get("CRC-16/CCITT"), "514A");
    }

    #[test]
    fn entropy_bounds() {
        let zeros = vec![0u8; 1000];
        let s = compute_stats(&zeros, (0, 1000), 0, &no_cancel()).unwrap();
        assert_eq!(s.entropy, 0.0);
        assert_eq!(s.zeros, 1000);
        let all: Vec<u8> = (0..=255).collect();
        let s = compute_stats(&all, (0, 256), 0, &no_cancel()).unwrap();
        assert!((s.entropy - 8.0).abs() < 1e-9);
    }

    #[test]
    fn strings_extraction() {
        let data = b"\x00\x01hello\x00\x01\x02w\x00o\x00r\x00l\x00d\x00\xff";
        let (s, truncated) = extract_strings(data, 4, true, 100, &no_cancel()).unwrap();
        assert!(!truncated);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "hello");
        assert_eq!(s[0].offset, 2);
        assert_eq!(s[1].text, "world");
        assert_eq!(s[1].offset, 10);
        assert_eq!(s[1].kind, StringKind::Utf16Le);
    }

    #[test]
    fn differences() {
        let a = b"abcdef";
        let b = b"abXdeY12";
        assert_eq!(next_difference(a, b, 0), Some(2));
        assert_eq!(next_difference(a, b, 3), Some(5));
        assert_eq!(next_difference(a, b, 6), Some(6));
        assert_eq!(prev_difference(a, b, 5), Some(2));
        assert_eq!(prev_difference(a, b, 2), None);
        assert_eq!(count_differences(a, b), 4);
        assert_eq!(next_difference(a, a, 0), None);
    }

    #[test]
    fn file_types() {
        assert_eq!(
            detect_file_type(b"\x89PNG\r\n\x1a\n...."),
            Some("PNG image")
        );
        assert_eq!(detect_file_type(b"hello world\n"), Some("Plain text"));
        assert_eq!(detect_file_type(b"\x00\x13\x37"), None);
    }

    #[test]
    fn minimap_blocks() {
        let data = vec![0u8; 10_000];
        let m = compute_minimap(&data, 1, &no_cancel()).unwrap();
        assert_eq!(m.blocks.len(), 2000);
        assert!(m.blocks.iter().all(|b| b.zero == 1.0));
    }
}

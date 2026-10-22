//! Data inspector: interprets the bytes at the cursor as many types and lets
//! the user edit them in place.

use egui::{Grid, RichText, TextEdit, Ui};

use crate::values::{NumType, f16_to_f32, f32_to_f16, format_float, parse_int};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Binary,
    Num(NumType),
    U24,
    I24,
    F16,
    Ascii,
    Utf8,
    Utf16,
    Uleb128,
    Sleb128,
    Unix32,
    Unix64,
    FileTime,
    DosDateTime,
    Guid,
}

pub const FIELDS: &[Field] = &[
    Field::Binary,
    Field::Num(NumType::U8),
    Field::Num(NumType::I8),
    Field::Num(NumType::U16),
    Field::Num(NumType::I16),
    Field::U24,
    Field::I24,
    Field::Num(NumType::U32),
    Field::Num(NumType::I32),
    Field::Num(NumType::U64),
    Field::Num(NumType::I64),
    Field::F16,
    Field::Num(NumType::F32),
    Field::Num(NumType::F64),
    Field::Ascii,
    Field::Utf8,
    Field::Utf16,
    Field::Uleb128,
    Field::Sleb128,
    Field::Unix32,
    Field::Unix64,
    Field::FileTime,
    Field::DosDateTime,
    Field::Guid,
];

impl Field {
    pub fn name(self) -> &'static str {
        match self {
            Field::Binary => "binary",
            Field::Num(t) => t.name(),
            Field::U24 => "uint24",
            Field::I24 => "int24",
            Field::F16 => "float16",
            Field::Ascii => "char",
            Field::Utf8 => "UTF-8",
            Field::Utf16 => "UTF-16",
            Field::Uleb128 => "ULEB128",
            Field::Sleb128 => "SLEB128",
            Field::Unix32 => "time_t (32)",
            Field::Unix64 => "time_t (64)",
            Field::FileTime => "FILETIME",
            Field::DosDateTime => "DOS date/time",
            Field::Guid => "GUID",
        }
    }

    pub fn editable(self) -> bool {
        matches!(
            self,
            Field::Binary | Field::Num(_) | Field::U24 | Field::I24 | Field::F16 | Field::Ascii
        )
    }

    pub fn decode(self, b: &[u8], be: bool) -> Option<String> {
        let arr = |n: usize| -> Option<Vec<u8>> {
            let mut v = b.get(..n)?.to_vec();
            if be {
                v.reverse();
            }
            Some(v)
        };
        let le_u64 = |n: usize| -> Option<u64> {
            let v = arr(n)?;
            Some(v.iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64))
        };
        match self {
            Field::Binary => b.first().map(|v| format!("{v:08b}")),
            Field::Num(t) => t.decode(b, be),
            Field::U24 => le_u64(3).map(|v| v.to_string()),
            Field::I24 => le_u64(3).map(|v| (((v as i32) << 8) >> 8).to_string()),
            Field::F16 => le_u64(2).map(|v| format_float(f16_to_f32(v as u16) as f64, true)),
            Field::Ascii => b.first().map(|&v| match v {
                0x20..=0x7e => format!("'{}'", v as char),
                0 => "NUL".into(),
                b'\t' => "'\\t'".into(),
                b'\n' => "'\\n'".into(),
                b'\r' => "'\\r'".into(),
                _ => format!("\\x{v:02X}"),
            }),
            Field::Utf8 => decode_utf8(b),
            Field::Utf16 => decode_utf16(b, be),
            Field::Uleb128 => decode_leb128(b, false),
            Field::Sleb128 => decode_leb128(b, true),
            Field::Unix32 => le_u64(4).map(|v| format_unix(v as u32 as i64)),
            Field::Unix64 => le_u64(8).map(|v| format_unix(v as i64)),
            Field::FileTime => le_u64(8).map(|v| {
                if v == 0 {
                    return "(zero)".into();
                }
                let secs = (v / 10_000_000) as i64 - 11_644_473_600;
                format_unix(secs)
            }),
            Field::DosDateTime => {
                let time = u16::from_le_bytes(b.get(..2)?.try_into().ok()?);
                let date = u16::from_le_bytes(b.get(2..4)?.try_into().ok()?);
                let (y, mo, d) = (1980 + (date >> 9) as u32, (date >> 5) & 0xf, date & 0x1f);
                let (h, mi, s) = (time >> 11, (time >> 5) & 0x3f, (time & 0x1f) * 2);
                if !(1..=12).contains(&mo) || d == 0 || h > 23 || mi > 59 || s > 59 {
                    return Some("(invalid)".into());
                }
                Some(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"))
            }
            Field::Guid => {
                let g = b.get(..16)?;
                let (d1, d2, d3) = if be {
                    (
                        u32::from_be_bytes(g[0..4].try_into().ok()?),
                        u16::from_be_bytes(g[4..6].try_into().ok()?),
                        u16::from_be_bytes(g[6..8].try_into().ok()?),
                    )
                } else {
                    (
                        u32::from_le_bytes(g[0..4].try_into().ok()?),
                        u16::from_le_bytes(g[4..6].try_into().ok()?),
                        u16::from_le_bytes(g[6..8].try_into().ok()?),
                    )
                };
                Some(format!(
                    "{{{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
                    g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
                ))
            }
        }
    }

    /// Encodes user input for editable fields.
    pub fn encode(self, text: &str, be: bool) -> Result<Vec<u8>, String> {
        let order = |mut v: Vec<u8>| {
            if be {
                v.reverse();
            }
            v
        };
        match self {
            Field::Binary => {
                let t: String = text
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '_')
                    .collect();
                u8::from_str_radix(t.trim_start_matches("0b"), 2)
                    .map(|v| vec![v])
                    .map_err(|_| "expected up to 8 binary digits".into())
            }
            Field::Num(t) => t.encode(text, be),
            Field::U24 => {
                let v: u32 = parse_int(text)?;
                if v > 0xff_ffff {
                    return Err("value out of range for uint24".into());
                }
                Ok(order(v.to_le_bytes()[..3].to_vec()))
            }
            Field::I24 => {
                let v: i32 = parse_int(text)?;
                if !(-0x80_0000..0x80_0000).contains(&v) {
                    return Err("value out of range for int24".into());
                }
                Ok(order(v.to_le_bytes()[..3].to_vec()))
            }
            Field::F16 => {
                let v: f32 = text
                    .trim()
                    .parse()
                    .map_err(|_| "not a number".to_string())?;
                Ok(order(f32_to_f16(v).to_le_bytes().to_vec()))
            }
            Field::Ascii => {
                let t = text.trim_matches('\'');
                let mut chars = t.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii() => Ok(vec![c as u8]),
                    _ => Err("enter a single ASCII character".into()),
                }
            }
            _ => Err("not editable".into()),
        }
    }
}

fn decode_utf8(b: &[u8]) -> Option<String> {
    let first = *b.first()?;
    let n = match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Some("(invalid)".into()),
    };
    let s = std::str::from_utf8(b.get(..n)?).ok();
    Some(match s.and_then(|s| s.chars().next()) {
        Some(c) => describe_char(c, n),
        None => "(invalid)".into(),
    })
}

fn decode_utf16(b: &[u8], be: bool) -> Option<String> {
    let unit = |i: usize| -> Option<u16> {
        let p: [u8; 2] = b.get(i..i + 2)?.try_into().ok()?;
        Some(if be {
            u16::from_be_bytes(p)
        } else {
            u16::from_le_bytes(p)
        })
    };
    let u0 = unit(0)?;
    let units: Vec<u16> = if (0xd800..0xdc00).contains(&u0) {
        match unit(2) {
            Some(u1) => vec![u0, u1],
            None => vec![u0],
        }
    } else {
        vec![u0]
    };
    Some(match char::decode_utf16(units.iter().copied()).next() {
        Some(Ok(c)) => describe_char(c, units.len() * 2),
        _ => "(invalid)".into(),
    })
}

fn describe_char(c: char, n: usize) -> String {
    let shown = if c.is_control() { ' ' } else { c };
    if c.is_control() {
        format!("U+{:04X} ({n} B)", c as u32)
    } else {
        format!("U+{:04X} '{shown}' ({n} B)", c as u32)
    }
}

fn decode_leb128(b: &[u8], signed: bool) -> Option<String> {
    let mut result: u128 = 0;
    let mut shift = 0;
    for (i, &byte) in b.iter().take(10).enumerate() {
        result |= ((byte & 0x7f) as u128) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            let n = i + 1;
            return Some(if signed {
                let mut v = result as i128;
                if shift < 128 && byte & 0x40 != 0 {
                    v |= -1i128 << shift;
                }
                format!("{v} ({n} B)")
            } else {
                format!("{result} ({n} B)")
            });
        }
    }
    (!b.is_empty()).then(|| "(unterminated)".into())
}

/// Formats Unix seconds as an ISO-like UTC timestamp.
pub fn format_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if !(1..=9999).contains(&y) {
        return "(out of range)".into();
    }
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// UI

struct Editing {
    offset: usize,
    index: usize,
    buf: String,
}

#[derive(Default)]
pub struct InspectorState {
    editing: Option<Editing>,
    error: Option<String>,
}

pub fn inspector_ui(
    ui: &mut Ui,
    state: &mut InspectorState,
    bytes: &[u8],
    offset: usize,
    big_endian: &mut bool,
    read_only: bool,
) -> Option<Vec<u8>> {
    let mut edit = None;
    ui.label(RichText::new(format!("Offset 0x{offset:X} ({offset})")).strong());
    ui.horizontal(|ui| {
        ui.label("Byte order:");
        ui.selectable_value(big_endian, false, "Little endian");
        ui.selectable_value(big_endian, true, "Big endian");
    });

    if let Some(&byte) = bytes.first() {
        ui.horizontal(|ui| {
            ui.label("Bits:");
            ui.spacing_mut().item_spacing.x = 2.0;
            for bit in (0..8).rev() {
                let set = byte & (1 << bit) != 0;
                let resp = ui
                    .add_enabled(
                        !read_only,
                        egui::Button::new(RichText::new(if set { "1" } else { "0" }).monospace())
                            .selected(set)
                            .min_size(egui::vec2(16.0, 0.0)),
                    )
                    .on_hover_text(format!("Bit {bit} (click to toggle)"));
                if resp.clicked() {
                    edit = Some(vec![byte ^ (1 << bit)]);
                }
                if bit == 4 {
                    ui.add_space(4.0);
                }
            }
        });
    }
    ui.separator();

    if bytes.is_empty() {
        ui.weak("Cursor is at end of file.");
        return edit;
    }

    if state.editing.as_ref().is_some_and(|e| e.offset != offset) || read_only {
        state.editing = None;
        state.error = None;
    }

    Grid::new("inspector_grid")
        .num_columns(2)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (i, &field) in FIELDS.iter().enumerate() {
                let Some(value) = field.decode(bytes, *big_endian) else {
                    continue;
                };
                ui.label(RichText::new(field.name()).weak());
                match state.editing.as_mut().filter(|e| e.index == i) {
                    Some(editing) => {
                        let resp = ui.add(
                            TextEdit::singleline(&mut editing.buf)
                                .desired_width(170.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if !resp.has_focus() && !resp.lost_focus() {
                            resp.request_focus();
                        }
                        let (enter, escape) = ui.input(|inp| {
                            (
                                inp.key_pressed(egui::Key::Enter),
                                inp.key_pressed(egui::Key::Escape),
                            )
                        });
                        if enter {
                            match field.encode(editing.buf.trim(), *big_endian) {
                                Ok(b) => {
                                    edit = Some(b);
                                    state.editing = None;
                                    state.error = None;
                                }
                                Err(e) => state.error = Some(e),
                            }
                        } else if escape || (resp.lost_focus() && state.error.is_none()) {
                            state.editing = None;
                            state.error = None;
                        }
                    }
                    None => {
                        let resp = ui.add(
                            egui::Label::new(RichText::new(&value).monospace())
                                .sense(egui::Sense::click()),
                        );
                        let can_edit = field.editable() && !read_only;
                        let resp = resp.on_hover_text(if can_edit {
                            "Double-click to edit, right-click to copy"
                        } else {
                            "Right-click to copy"
                        });
                        if resp.double_clicked() && can_edit {
                            let buf = match field {
                                Field::Ascii => bytes
                                    .first()
                                    .filter(|b| b.is_ascii_graphic() || **b == b' ')
                                    .map(|&b| (b as char).to_string())
                                    .unwrap_or_default(),
                                _ => value.clone(),
                            };
                            state.editing = Some(Editing {
                                offset,
                                index: i,
                                buf,
                            });
                            state.error = None;
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Copy value").clicked() {
                                ui.ctx().copy_text(value.clone());
                                ui.close();
                            }
                        });
                    }
                }
                ui.end_row();
            }
        });

    if let Some(err) = &state.error {
        ui.colored_label(ui.visuals().error_fg_color, err);
    }
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(format_unix(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_unix(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(format_unix(-86_400), "1969-12-31 00:00:00 UTC");
        // 2009-07-25 20:59:59 UTC as FILETIME
        let ft: u64 = 128_930_291_990_000_000;
        assert_eq!(
            Field::FileTime.decode(&ft.to_le_bytes(), false).unwrap(),
            "2009-07-25 20:59:59 UTC"
        );
    }

    #[test]
    fn integers_24() {
        assert_eq!(
            Field::U24.decode(&[0x01, 0x02, 0x03], false).unwrap(),
            "197121"
        );
        assert_eq!(
            Field::U24.decode(&[0x01, 0x02, 0x03], true).unwrap(),
            "66051"
        );
        assert_eq!(Field::I24.decode(&[0xff, 0xff, 0xff], false).unwrap(), "-1");
        assert_eq!(
            Field::I24.encode("-2", false).unwrap(),
            vec![0xfe, 0xff, 0xff]
        );
        assert_eq!(Field::U24.encode("0x010203", true).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn leb128() {
        assert_eq!(
            decode_leb128(&[0xe5, 0x8e, 0x26], false).unwrap(),
            "624485 (3 B)"
        );
        assert_eq!(
            decode_leb128(&[0xc0, 0xbb, 0x78], true).unwrap(),
            "-123456 (3 B)"
        );
        assert_eq!(decode_leb128(&[0x80], false).unwrap(), "(unterminated)");
    }

    #[test]
    fn chars_and_guid() {
        assert_eq!(
            Field::Utf8.decode("é".as_bytes(), false).unwrap(),
            "U+00E9 'é' (2 B)"
        );
        assert_eq!(
            Field::Utf16
                .decode(&[0x3d, 0xd8, 0x00, 0xde], false)
                .unwrap(),
            "U+1F600 '😀' (4 B)"
        );
        let g = [
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(
            Field::Guid.decode(&g, false).unwrap(),
            "{00112233-4455-6677-8899-AABBCCDDEEFF}"
        );
        assert_eq!(
            Field::Binary.encode("1010 0101", false).unwrap(),
            vec![0xa5]
        );
        assert_eq!(Field::Ascii.encode("A", false).unwrap(), vec![b'A']);
    }

    #[test]
    fn dos_datetime() {
        // 2020-05-17 13:45:30
        let time: u16 = (13 << 11) | (45 << 5) | 15;
        let date: u16 = (40 << 9) | (5 << 5) | 17;
        let mut b = time.to_le_bytes().to_vec();
        b.extend(date.to_le_bytes());
        assert_eq!(
            Field::DosDateTime.decode(&b, false).unwrap(),
            "2020-05-17 13:45:30"
        );
    }
}

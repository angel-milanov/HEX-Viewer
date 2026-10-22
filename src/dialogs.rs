//! Modal-ish tool windows: go to, select range, fill, insert and transform.

use egui::{Align2, Key, RichText, TextEdit, TextStyle, Ui, Window};

use crate::app::{APP_NAME, HexApp, plural};
use crate::commands::SHORTCUT_HELP;
use crate::export::parse_hex;
use crate::transform::{TransformOp, fill_random, repeat_pattern};

#[derive(Default)]
pub struct GotoDialog {
    input: String,
    hex: bool,
    extend: bool,
    error: Option<String>,
    focus: bool,
}

#[derive(Default)]
pub struct RangeDialog {
    start: String,
    end: String,
    use_length: bool,
    hex: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum PatternKind {
    #[default]
    Hex,
    Text,
    Random,
}

#[derive(Default)]
pub struct FillDialog {
    pattern: String,
    kind: PatternKind,
    error: Option<String>,
}

#[derive(Default)]
pub struct InsertDialog {
    count: String,
    hex_count: bool,
    pattern: String,
    kind: PatternKind,
    error: Option<String>,
}

#[derive(Default)]
pub struct TransformDialog {
    op: TransformOp,
    key: String,
    bits: u32,
    error: Option<String>,
}

#[derive(Default)]
pub struct Dialogs {
    pub goto: Option<GotoDialog>,
    pub range: Option<RangeDialog>,
    pub fill: Option<FillDialog>,
    pub insert: Option<InsertDialog>,
    pub transform: Option<TransformDialog>,
    pub shortcuts: bool,
    pub about: bool,
}

impl Dialogs {
    pub fn open_goto(&mut self) {
        self.goto = Some(GotoDialog {
            hex: true,
            focus: true,
            ..Default::default()
        });
    }

    pub fn open_select_range(&mut self, sel: Option<(usize, usize)>) {
        let (start, end) = sel.map_or((String::new(), String::new()), |(s, e)| {
            (format!("{s:X}"), format!("{:X}", e - 1))
        });
        self.range = Some(RangeDialog {
            start,
            end,
            hex: true,
            ..Default::default()
        });
    }

    pub fn open_fill(&mut self) {
        self.fill = Some(FillDialog {
            pattern: "00".into(),
            ..Default::default()
        });
    }

    pub fn open_insert(&mut self) {
        self.insert = Some(InsertDialog {
            count: "16".into(),
            pattern: "00".into(),
            ..Default::default()
        });
    }

    pub fn open_transform(&mut self) {
        self.transform = Some(TransformDialog {
            key: "FF".into(),
            bits: 1,
            ..Default::default()
        });
    }
}

/// Parses an offset. `0x`/`h` force hex, `#` forces decimal, otherwise the
/// default radix is used. Returns `(sign, value)` where sign is `+`/`-` for
/// relative input.
fn parse_offset(input: &str, hex_default: bool) -> Result<(Option<char>, usize), String> {
    let t = input.trim().replace(['_', ' '], "");
    if t.is_empty() {
        return Err("enter an offset".into());
    }
    let (sign, rest) = match t.chars().next() {
        Some(c @ ('+' | '-')) => (Some(c), &t[1..]),
        _ => (None, t.as_str()),
    };
    let (radix, digits) =
        if let Some(r) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
            (16, r)
        } else if let Some(r) = rest.strip_suffix(['h', 'H']) {
            (16, r)
        } else if let Some(r) = rest.strip_prefix('#') {
            (10, r)
        } else {
            (if hex_default { 16 } else { 10 }, rest)
        };
    usize::from_str_radix(digits, radix)
        .map(|v| (sign, v))
        .map_err(|_| {
            format!(
                "'{input}' is not a valid {} number",
                if radix == 16 { "hex" } else { "decimal" }
            )
        })
}

fn pattern_bytes(kind: PatternKind, text: &str, len: usize) -> Result<Vec<u8>, String> {
    match kind {
        PatternKind::Hex => {
            let p = parse_hex(text)?;
            Ok(repeat_pattern(&p, len))
        }
        PatternKind::Text => {
            if text.is_empty() {
                return Err("enter some text".into());
            }
            Ok(repeat_pattern(text.as_bytes(), len))
        }
        PatternKind::Random => {
            let mut v = vec![0; len];
            fill_random(&mut v);
            Ok(v)
        }
    }
}

fn pattern_kind_ui(ui: &mut Ui, kind: &mut PatternKind) {
    ui.horizontal(|ui| {
        ui.selectable_value(kind, PatternKind::Hex, "Hex pattern");
        ui.selectable_value(kind, PatternKind::Text, "Text");
        ui.selectable_value(kind, PatternKind::Random, "Random");
    });
}

fn error_label(ui: &mut Ui, err: &Option<String>) {
    if let Some(e) = err {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
}

/// OK / Cancel row; Enter confirms and Escape cancels.
fn ok_cancel(ui: &mut Ui, ok_label: &str) -> (bool, bool) {
    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    let mut ok = enter;
    let mut cancel = esc;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ok |= ui.button(ok_label).clicked();
        cancel |= ui.button("Cancel").clicked();
    });
    (ok, cancel)
}

fn dialog_window(title: &str) -> Window<'_> {
    Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, [0.0, 90.0])
}

impl HexApp {
    pub(crate) fn dialogs_ui(&mut self, ctx: &egui::Context) {
        self.goto_dialog(ctx);
        self.range_dialog(ctx);
        self.fill_dialog(ctx);
        self.insert_dialog(ctx);
        self.transform_dialog(ctx);
        self.help_windows(ctx);
    }

    fn goto_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialogs.goto else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.dialogs.goto = None;
            return;
        };
        let mut done = false;
        dialog_window("Go to offset").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let resp = ui.add(
                    TextEdit::singleline(&mut d.input)
                        .font(TextStyle::Monospace)
                        .hint_text("offset, +rel or -rel")
                        .desired_width(200.0),
                );
                if d.focus {
                    resp.request_focus();
                    d.focus = false;
                }
                ui.selectable_value(&mut d.hex, true, "Hex");
                ui.selectable_value(&mut d.hex, false, "Dec");
            });
            ui.checkbox(&mut d.extend, "Extend selection");
            ui.weak(format!(
                "Current: 0x{:X} ({}) · File size: 0x{:X}",
                tab.view.cursor,
                tab.view.cursor,
                tab.doc.len()
            ));
            error_label(ui, &d.error);
            let (ok, cancel) = ok_cancel(ui, "Go");
            if cancel {
                done = true;
            } else if ok {
                match parse_offset(&d.input, d.hex) {
                    Ok((sign, v)) => {
                        let len = tab.doc.len();
                        let cur = tab.view.cursor;
                        let target = match sign {
                            Some('+') => cur.saturating_add(v),
                            Some(_) => cur.saturating_sub(v),
                            None => v,
                        };
                        if target > len {
                            d.error = Some(format!("Offset is beyond end of file (0x{len:X})"));
                        } else {
                            tab.view.move_to(target, d.extend, len);
                            tab.view.center_cursor = true;
                            done = true;
                        }
                    }
                    Err(e) => d.error = Some(e),
                }
            }
        });
        if done {
            self.dialogs.goto = None;
        }
    }

    fn range_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialogs.range else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.dialogs.range = None;
            return;
        };
        let mut done = false;
        dialog_window("Select range").show(ctx, |ui| {
            egui::Grid::new("range_grid").num_columns(2).show(ui, |ui| {
                ui.label("Start");
                ui.add(
                    TextEdit::singleline(&mut d.start)
                        .font(TextStyle::Monospace)
                        .desired_width(160.0),
                );
                ui.end_row();
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut d.use_length, false, "End");
                    ui.selectable_value(&mut d.use_length, true, "Length");
                });
                ui.add(
                    TextEdit::singleline(&mut d.end)
                        .font(TextStyle::Monospace)
                        .desired_width(160.0),
                );
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut d.hex, true, "Hex");
                ui.selectable_value(&mut d.hex, false, "Decimal");
                ui.weak(if d.use_length {
                    ""
                } else {
                    "(end is inclusive)"
                });
            });
            error_label(ui, &d.error);
            let (ok, cancel) = ok_cancel(ui, "Select");
            if cancel {
                done = true;
            } else if ok {
                let len = tab.doc.len();
                let parsed = parse_offset(&d.start, d.hex).and_then(|(_, s)| {
                    parse_offset(&d.end, d.hex)
                        .map(|(_, e)| (s, if d.use_length { s + e } else { e + 1 }))
                });
                match parsed {
                    Ok((s, e)) if s < e && e <= len => {
                        tab.view.select(s, e, len);
                        done = true;
                    }
                    Ok(_) => {
                        d.error = Some(format!("Range must be within 0..0x{len:X} and non-empty"))
                    }
                    Err(e) => d.error = Some(e),
                }
            }
        });
        if done {
            self.dialogs.range = None;
        }
    }

    fn fill_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialogs.fill else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.dialogs.fill = None;
            return;
        };
        let Some((s, e)) = tab.view.selection(tab.doc.len()) else {
            self.dialogs.fill = None;
            return;
        };
        let mut done = false;
        let mut status = None;
        dialog_window("Fill selection").show(ctx, |ui| {
            ui.label(format!(
                "Fill 0x{s:X}–0x{:X} ({})",
                e - 1,
                plural(e - s, "byte")
            ));
            pattern_kind_ui(ui, &mut d.kind);
            if d.kind != PatternKind::Random {
                ui.add(
                    TextEdit::singleline(&mut d.pattern)
                        .font(TextStyle::Monospace)
                        .desired_width(260.0),
                );
            }
            error_label(ui, &d.error);
            let (ok, cancel) = ok_cancel(ui, "Fill");
            if cancel {
                done = true;
            } else if ok {
                match pattern_bytes(d.kind, &d.pattern, e - s) {
                    Ok(bytes) => {
                        tab.doc.replace(s, e - s, bytes, false);
                        status = Some(format!("Filled {}", plural(e - s, "byte")));
                        done = true;
                    }
                    Err(err) => d.error = Some(err),
                }
            }
        });
        if done {
            self.dialogs.fill = None;
        }
        if let Some(s) = status {
            self.set_status(s);
        }
    }

    fn insert_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialogs.insert else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.dialogs.insert = None;
            return;
        };
        let mut done = false;
        let mut status = None;
        dialog_window("Insert bytes").show(ctx, |ui| {
            ui.label(format!("Insert at 0x{:X}", tab.view.cursor));
            ui.horizontal(|ui| {
                ui.label("Count");
                ui.add(
                    TextEdit::singleline(&mut d.count)
                        .font(TextStyle::Monospace)
                        .desired_width(120.0),
                );
                ui.selectable_value(&mut d.hex_count, false, "Dec");
                ui.selectable_value(&mut d.hex_count, true, "Hex");
            });
            pattern_kind_ui(ui, &mut d.kind);
            if d.kind != PatternKind::Random {
                ui.add(
                    TextEdit::singleline(&mut d.pattern)
                        .font(TextStyle::Monospace)
                        .desired_width(260.0),
                );
            }
            error_label(ui, &d.error);
            let (ok, cancel) = ok_cancel(ui, "Insert");
            if cancel {
                done = true;
            } else if ok {
                let result = parse_offset(&d.count, d.hex_count).and_then(|(_, n)| {
                    if n == 0 || n > 1 << 30 {
                        Err("count must be between 1 and 1 GiB".to_string())
                    } else {
                        pattern_bytes(d.kind, &d.pattern, n)
                    }
                });
                match result {
                    Ok(bytes) => {
                        let at = tab.view.cursor;
                        let n = bytes.len();
                        tab.doc.insert(at, bytes, false);
                        let len = tab.doc.len();
                        tab.view.select(at, at + n, len);
                        status = Some(format!("Inserted {}", plural(n, "byte")));
                        done = true;
                    }
                    Err(e) => d.error = Some(e),
                }
            }
        });
        if done {
            self.dialogs.insert = None;
        }
        if let Some(s) = status {
            self.set_status(s);
        }
    }

    fn transform_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialogs.transform else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.dialogs.transform = None;
            return;
        };
        let len = tab.doc.len();
        let (s, e) = tab.view.selection(len).unwrap_or((0, len));
        let mut done = false;
        let mut status = None;
        dialog_window("Transform bytes").show(ctx, |ui| {
            ui.label(if tab.view.selection(len).is_some() {
                format!(
                    "Selection 0x{s:X}–0x{:X} ({})",
                    e.saturating_sub(1),
                    plural(e - s, "byte")
                )
            } else {
                format!("Entire file ({})", plural(len, "byte"))
            });
            egui::ComboBox::from_id_salt("transform_op")
                .width(260.0)
                .selected_text(d.op.name())
                .show_ui(ui, |ui| {
                    for op in TransformOp::ALL {
                        ui.selectable_value(&mut d.op, op, op.name());
                    }
                });
            if d.op.needs_key() {
                ui.horizontal(|ui| {
                    ui.label("Key (hex)");
                    ui.add(
                        TextEdit::singleline(&mut d.key)
                            .font(TextStyle::Monospace)
                            .desired_width(200.0),
                    );
                });
                ui.weak("Multi-byte keys are repeated across the range.");
            }
            if d.op.needs_bits() {
                ui.horizontal(|ui| {
                    ui.label("Bits");
                    ui.add(egui::DragValue::new(&mut d.bits).range(1..=7));
                });
            }
            if d.op == TransformOp::Increment {
                ui.horizontal(|ui| {
                    ui.label("Start value (hex)");
                    ui.add(
                        TextEdit::singleline(&mut d.key)
                            .font(TextStyle::Monospace)
                            .desired_width(60.0),
                    );
                });
            }
            error_label(ui, &d.error);
            let (ok, cancel) = ok_cancel(ui, "Apply");
            if cancel {
                done = true;
            } else if ok && e > s {
                let key = if d.op.needs_key() || d.op == TransformOp::Increment {
                    match parse_hex(&d.key) {
                        Ok(k) => k,
                        Err(err) => {
                            d.error = Some(format!("Key: {err}"));
                            return;
                        }
                    }
                } else {
                    Vec::new()
                };
                let mut bytes = tab.doc.bytes()[s..e].to_vec();
                d.op.apply(&mut bytes, &key, d.bits);
                tab.doc.replace(s, e - s, bytes, false);
                status = Some(format!(
                    "{} applied to {}",
                    d.op.name(),
                    plural(e - s, "byte")
                ));
                done = true;
            } else if ok {
                done = true;
            }
        });
        if done {
            self.dialogs.transform = None;
        }
        if let Some(s) = status {
            self.set_status(s);
        }
    }

    fn help_windows(&mut self, ctx: &egui::Context) {
        let mut open = self.dialogs.shortcuts;
        Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("shortcut_grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (keys, what) in SHORTCUT_HELP {
                            ui.label(RichText::new(*keys).monospace().strong());
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
            });
        self.dialogs.shortcuts = open && !ctx.input(|i| i.key_pressed(Key::Escape));

        let mut open = self.dialogs.about;
        Window::new(format!("About {APP_NAME}"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(RichText::new(APP_NAME).strong());
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(6.0);
                    ui.label("A fast, cross-platform hex editor built with Rust and egui.");
                    ui.add_space(6.0);
                    ui.hyperlink_to("egui", "https://github.com/emilk/egui");
                });
            });
        self.dialogs.about = open && !ctx.input(|i| i.key_pressed(Key::Escape));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets() {
        assert_eq!(parse_offset("1F", true).unwrap(), (None, 0x1f));
        assert_eq!(parse_offset("10", false).unwrap(), (None, 10));
        assert_eq!(parse_offset("0x10", false).unwrap(), (None, 16));
        assert_eq!(parse_offset("10h", false).unwrap(), (None, 16));
        assert_eq!(parse_offset("#10", true).unwrap(), (None, 10));
        assert_eq!(parse_offset("+20", true).unwrap(), (Some('+'), 0x20));
        assert_eq!(parse_offset("-0x5", true).unwrap(), (Some('-'), 5));
        assert!(parse_offset("xyz", true).is_err());
        assert!(parse_offset("", true).is_err());
    }

    #[test]
    fn patterns() {
        assert_eq!(
            pattern_bytes(PatternKind::Hex, "AB CD", 3).unwrap(),
            vec![0xab, 0xcd, 0xab]
        );
        assert_eq!(
            pattern_bytes(PatternKind::Text, "xy", 3).unwrap(),
            b"xyx".to_vec()
        );
        assert_eq!(pattern_bytes(PatternKind::Random, "", 7).unwrap().len(), 7);
        assert!(pattern_bytes(PatternKind::Hex, "zz", 3).is_err());
    }
}

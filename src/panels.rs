//! Right-hand tool panels.

use egui::{
    Color32, CornerRadius, Grid, ProgressBar, Rect, RichText, ScrollArea, Sense, TextEdit,
    TextStyle, Ui, pos2, vec2,
};

use crate::analysis::{
    HashResult, Job, StringKind, StringsResult, compute_hashes, compute_stats, count_differences,
    extract_strings,
};
use crate::app::{HexApp, group_digits, human_size, plural};
use crate::commands::Command;
use crate::export::to_hex;
use crate::inspector::inspector_ui;
use crate::settings::SidePanel;

const MAX_STRINGS: usize = 100_000;

impl HexApp {
    pub(crate) fn side_panel(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for p in SidePanel::ALL {
                ui.selectable_value(&mut self.settings.side_panel, p, p.name());
            }
        });
        ui.separator();
        if self.tabs.is_empty() {
            ui.weak("No file open.");
            return;
        }
        match self.settings.side_panel {
            SidePanel::Inspector => {
                ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.inspector_panel(ui));
            }
            SidePanel::Bookmarks => self.bookmarks_panel(ui),
            SidePanel::SearchResults => self.search_panel(ui),
            SidePanel::Strings => self.strings_panel(ui),
            SidePanel::Checksums => {
                ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.checksums_panel(ui));
            }
            SidePanel::Statistics => {
                ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.statistics_panel(ui));
            }
            SidePanel::Compare => self.compare_panel(ui),
        }
    }

    fn inspector_panel(&mut self, ui: &mut Ui) {
        let tab = &mut self.tabs[self.active];
        let cursor = tab.view.cursor;
        let len = tab.doc.len();
        let bytes = tab.doc.bytes()[cursor.min(len)..(cursor + 16).min(len)].to_vec();
        let edit = inspector_ui(
            ui,
            &mut tab.inspector,
            &bytes,
            cursor,
            &mut self.settings.inspector_big_endian,
            tab.doc.read_only,
        );
        if let Some(new) = edit {
            tab.doc.overwrite(cursor, new, false);
        }
    }

    fn bookmarks_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("➕ Add at cursor")
                .on_hover_text("Ctrl+B")
                .clicked()
            {
                self.pending.push(Command::ToggleBookmark);
            }
            if ui
                .button("⏶")
                .on_hover_text("Previous (Shift+F2)")
                .clicked()
            {
                self.pending.push(Command::PrevBookmark);
            }
            if ui.button("⏷").on_hover_text("Next (F2)").clicked() {
                self.pending.push(Command::NextBookmark);
            }
        });
        ui.separator();
        let tab = &mut self.tabs[self.active];
        if tab.doc.bookmarks.is_empty() {
            ui.weak("No bookmarks. Select bytes and press Ctrl+B.");
            return;
        }
        let mut remove = None;
        let mut goto = None;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (i, bm) in tab.doc.bookmarks.iter_mut().enumerate() {
                ui.push_id(i, |ui| {
                    ui.horizontal(|ui| {
                        ui.color_edit_button_srgb(&mut bm.color);
                        ui.add(
                            TextEdit::singleline(&mut bm.name)
                                .desired_width(ui.available_width() - 40.0),
                        );
                        if ui.small_button("🗑").on_hover_text("Delete").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(28.0);
                        if ui
                            .link(RichText::new(format!("0x{:X}", bm.offset)).monospace())
                            .on_hover_text("Go to bookmark")
                            .clicked()
                        {
                            goto = Some((bm.offset, bm.len));
                        }
                        ui.weak(plural(bm.len, "byte"));
                    });
                });
                ui.separator();
            }
        });
        if let Some(i) = remove {
            tab.doc.bookmarks.remove(i);
        }
        if let Some((o, n)) = goto {
            let len = tab.doc.len();
            tab.view.select(o, o + n, len);
            tab.view.center_cursor = true;
        }
    }

    fn search_panel(&mut self, ui: &mut Ui) {
        let tab = &mut self.tabs[self.active];
        let s = &tab.search;
        if s.hits.is_empty() {
            ui.weak("No results. Use Find (Ctrl+F) and press “Find all”.");
            return;
        }
        ui.label(format!(
            "{}{} for “{}”",
            plural(s.hits.len(), "match"),
            if s.truncated { " (limit reached)" } else { "" },
            s.query
        ));
        if s.version != tab.doc.version {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Document changed since the search.",
            );
        }
        ui.separator();
        let row_h = ui.text_style_height(&TextStyle::Monospace) + 4.0;
        let data = tab.doc.bytes();
        let mut clicked = None;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            row_h,
            s.hits.len(),
            |ui, range| {
                for i in range {
                    let off = s.hits[i];
                    let preview_end = (off + s.len.min(12)).min(data.len());
                    let preview = to_hex(data.get(off..preview_end).unwrap_or(&[]), true);
                    let text = RichText::new(format!("{off:>10X}  {preview}")).monospace();
                    if ui.selectable_label(s.current == Some(i), text).clicked() {
                        clicked = Some(i);
                    }
                }
            },
        );
        if let Some(i) = clicked {
            let off = tab.search.hits[i];
            let n = tab.search.len;
            let len = tab.doc.len();
            tab.search.current = Some(i);
            tab.view.select(off, off + n, len);
            tab.view.cursor = off;
            tab.view.anchor = Some(off + n.max(1) - 1);
            tab.view.center_cursor = true;
        }
    }

    fn strings_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label("Min length");
            ui.add(egui::DragValue::new(&mut self.settings.strings_min_len).range(1..=256));
            ui.checkbox(&mut self.settings.strings_utf16, "UTF-16LE");
        });
        let min_len = self.settings.strings_min_len;
        let utf16 = self.settings.strings_utf16;
        let tab = &mut self.tabs[self.active];
        ui.horizontal(|ui| {
            let running = tab.strings_job.is_some();
            if ui
                .add_enabled(!running, egui::Button::new("Extract strings"))
                .clicked()
            {
                let data = tab.doc.snapshot();
                let version = tab.doc.version;
                tab.strings_job = Some(Job::spawn(&ctx, move |cancel| {
                    extract_strings(&data, min_len, utf16, MAX_STRINGS, cancel).map(
                        |(items, truncated)| StringsResult {
                            version,
                            items,
                            truncated,
                        },
                    )
                }));
            }
            if running {
                ui.spinner();
                if ui.button("Cancel").clicked() {
                    tab.strings_job = None;
                }
            }
        });
        let Some(result) = &tab.strings else {
            ui.weak("Finds runs of printable characters, like the Unix `strings` tool.");
            return;
        };
        ui.add(
            TextEdit::singleline(&mut tab.strings_filter)
                .hint_text("Filter…")
                .desired_width(f32::INFINITY),
        );
        let filter = tab.strings_filter.to_lowercase();
        let items: Vec<usize> = (0..result.items.len())
            .filter(|&i| filter.is_empty() || result.items[i].text.to_lowercase().contains(&filter))
            .collect();
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{}{}",
                plural(items.len(), "string"),
                if result.truncated {
                    " (limit reached)"
                } else {
                    ""
                }
            ));
            if result.version != tab.doc.version {
                ui.colored_label(ui.visuals().warn_fg_color, "stale");
            }
        });
        ui.separator();
        let row_h = ui.text_style_height(&TextStyle::Monospace) + 4.0;
        let mut clicked = None;
        ScrollArea::both()
            .auto_shrink(false)
            .show_rows(ui, row_h, items.len(), |ui, range| {
                for &i in &items[range] {
                    let s = &result.items[i];
                    let tag = match s.kind {
                        StringKind::Ascii => "A",
                        StringKind::Utf16Le => "U",
                    };
                    let text =
                        RichText::new(format!("{:>8X} {tag} {}", s.offset, s.text)).monospace();
                    if ui
                        .add(egui::Button::new(text).frame(false).truncate())
                        .on_hover_text(format!(
                            "{} at 0x{:X}",
                            plural(s.byte_len, "byte"),
                            s.offset
                        ))
                        .clicked()
                    {
                        clicked = Some((s.offset, s.byte_len));
                    }
                }
            });
        if let Some((o, n)) = clicked {
            let len = tab.doc.len();
            tab.view.select(o, o + n, len);
            tab.view.cursor = o;
            tab.view.anchor = Some(o + n - 1);
            tab.view.center_cursor = true;
        }
    }

    fn scope_selector(&mut self, ui: &mut Ui) -> (usize, usize) {
        let tab = &self.tabs[self.active];
        let len = tab.doc.len();
        let sel = tab.view.selection(len);
        ui.horizontal(|ui| {
            ui.label("Scope:");
            ui.selectable_value(&mut self.analysis_use_selection, false, "Entire file");
            ui.add_enabled_ui(sel.is_some(), |ui| {
                ui.selectable_value(&mut self.analysis_use_selection, true, "Selection");
            });
        });
        match sel {
            Some(r) if self.analysis_use_selection => r,
            _ => (0, len),
        }
    }

    fn checksums_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let range = self.scope_selector(ui);
        let tab = &mut self.tabs[self.active];
        ui.horizontal(|ui| {
            let running = tab.hash_job.is_some();
            if ui
                .add_enabled(!running, egui::Button::new("Compute"))
                .clicked()
            {
                let data = tab.doc.snapshot();
                let version = tab.doc.version;
                tab.hash_job = Some(Job::spawn(&ctx, move |cancel| {
                    compute_hashes(&data[range.0..range.1], cancel).map(|values| HashResult {
                        range,
                        version,
                        values,
                    })
                }));
            }
            ui.weak(format!(
                "0x{:X}–0x{:X} ({})",
                range.0,
                range.1.saturating_sub(1),
                human_size(range.1 - range.0)
            ));
            if running {
                ui.spinner();
            }
        });
        let Some(h) = &tab.hashes else {
            ui.weak("CRC-16/32, Adler-32, MD5, SHA-1, SHA-256, SHA-512 and simple sums.");
            return;
        };
        if h.version != tab.doc.version || h.range != range {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Results are for a different range or an older version.",
            );
        }
        ui.separator();
        Grid::new("hash_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for (name, value) in &h.values {
                    ui.label(RichText::new(*name).weak());
                    let resp = ui
                        .add(
                            egui::Label::new(RichText::new(value).monospace())
                                .wrap()
                                .sense(Sense::click()),
                        )
                        .on_hover_text("Click to copy");
                    if resp.clicked() {
                        ui.ctx().copy_text(value.clone());
                    }
                    ui.end_row();
                }
            });
    }

    fn statistics_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let range = self.scope_selector(ui);
        let log_scale = &mut self.histogram_log;
        let tab = &mut self.tabs[self.active];
        ui.horizontal(|ui| {
            let running = tab.stats_job.is_some();
            if ui
                .add_enabled(!running, egui::Button::new("Analyze"))
                .clicked()
            {
                let data = tab.doc.snapshot();
                let version = tab.doc.version;
                tab.stats_job = Some(Job::spawn(&ctx, move |cancel| {
                    compute_stats(&data[range.0..range.1], range, version, cancel)
                }));
            }
            if running {
                ui.spinner();
            }
        });
        let Some(st) = &tab.stats else {
            ui.weak("Byte histogram, Shannon entropy and character class breakdown.");
            return;
        };
        if st.version != tab.doc.version || st.range != range {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Results are for a different range or an older version.",
            );
        }
        ui.separator();
        ui.label(RichText::new("Entropy").strong());
        ui.add(
            ProgressBar::new((st.entropy / 8.0) as f32)
                .text(format!("{:.4} bits/byte", st.entropy))
                .fill(entropy_color(st.entropy)),
        );
        ui.weak(entropy_hint(st.entropy));
        ui.add_space(6.0);

        let pct = |n: u64| {
            if st.total == 0 {
                0.0
            } else {
                n as f64 * 100.0 / st.total as f64
            }
        };
        Grid::new("stats_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(RichText::new(k).weak());
                    ui.label(RichText::new(v).monospace());
                    ui.end_row();
                };
                row("Bytes", group_digits(st.total as usize));
                row("Mean value", format!("{:.3}", st.mean));
                row(
                    "Distinct values",
                    st.histogram.iter().filter(|&&c| c > 0).count().to_string(),
                );
                row(
                    "Zero bytes",
                    format!("{} ({:.2}%)", st.zeros, pct(st.zeros)),
                );
                row(
                    "Printable ASCII",
                    format!("{} ({:.2}%)", st.printable, pct(st.printable)),
                );
                row(
                    "Whitespace",
                    format!("{} ({:.2}%)", st.whitespace, pct(st.whitespace)),
                );
                row(
                    "Control",
                    format!("{} ({:.2}%)", st.control, pct(st.control)),
                );
                row(
                    "High (0x80–0xFF)",
                    format!("{} ({:.2}%)", st.high, pct(st.high)),
                );
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Histogram").strong());
            ui.checkbox(log_scale, "Log scale");
        });
        draw_histogram(ui, &st.histogram, *log_scale);

        ui.add_space(8.0);
        ui.label(RichText::new("Most common bytes").strong());
        let mut order: Vec<usize> = (0..256).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(st.histogram[i]));
        Grid::new("top_bytes")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for &b in order.iter().take(10).filter(|&&b| st.histogram[b] > 0) {
                    let ch = if (0x20..0x7f).contains(&b) {
                        format!("'{}'", b as u8 as char)
                    } else {
                        String::new()
                    };
                    ui.label(RichText::new(format!("0x{b:02X} {ch}")).monospace());
                    ui.label(RichText::new(st.histogram[b].to_string()).monospace());
                    ui.label(format!("{:.2}%", pct(st.histogram[b])));
                    ui.end_row();
                }
            });
    }

    fn compare_panel(&mut self, ui: &mut Ui) {
        let active_id = self.tabs[self.active].id;
        let others: Vec<(u64, String)> = self
            .tabs
            .iter()
            .filter(|t| t.id != active_id)
            .map(|t| (t.id, t.doc.title.clone()))
            .collect();
        if others.is_empty() {
            ui.weak("Open a second file to compare it byte-by-byte with this one.");
            return;
        }
        let tab_idx = self.active;
        let current = self.tabs[tab_idx].compare_with;
        let selected_name = current
            .and_then(|id| others.iter().find(|(i, _)| *i == id))
            .map_or("(none)".to_string(), |(_, n)| n.clone());
        ui.horizontal(|ui| {
            ui.label("Compare with");
            egui::ComboBox::from_id_salt("compare_with")
                .selected_text(selected_name)
                .show_ui(ui, |ui| {
                    let tab = &mut self.tabs[tab_idx];
                    ui.selectable_value(&mut tab.compare_with, None, "(none)");
                    for (id, name) in &others {
                        ui.selectable_value(&mut tab.compare_with, Some(*id), name);
                    }
                });
        });
        let Some(other) = self.compare_data() else {
            return;
        };
        ui.checkbox(&mut self.highlight_diffs, "Highlight differences");
        ui.horizontal(|ui| {
            if ui.button("⏶ Previous").on_hover_text("Shift+F7").clicked() {
                self.pending.push(Command::PrevDifference);
            }
            if ui.button("⏷ Next").on_hover_text("F7").clicked() {
                self.pending.push(Command::NextDifference);
            }
        });
        let other_version = self
            .tabs
            .iter()
            .find(|t| Some(t.id) == self.tabs[tab_idx].compare_with)
            .map_or(0, |t| t.doc.version);
        let tab = &mut self.tabs[tab_idx];
        let a = tab.doc.bytes();
        ui.separator();
        Grid::new("compare_grid").num_columns(2).show(ui, |ui| {
            ui.weak("This file");
            ui.label(group_digits(a.len()));
            ui.end_row();
            ui.weak("Other file");
            ui.label(group_digits(other.len()));
            ui.end_row();
            ui.weak("Differences");
            match tab.diff_count {
                Some((v1, v2, n)) if v1 == tab.doc.version && v2 == other_version => {
                    ui.label(if n == 0 {
                        "identical".to_string()
                    } else {
                        plural(n, "byte")
                    });
                }
                _ => {
                    if ui.button("Count").clicked() {
                        let n = count_differences(a, &other);
                        tab.diff_count = Some((tab.doc.version, other_version, n));
                    }
                }
            }
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.weak("Differing bytes are highlighted in red. Comparison is positional (no alignment).");
    }
}

fn entropy_color(e: f64) -> Color32 {
    let t = (e / 8.0) as f32;
    Color32::from_rgb(40, 140, 220).lerp_to_gamma(Color32::from_rgb(230, 70, 50), t)
}

fn entropy_hint(e: f64) -> &'static str {
    match e {
        e if e < 1.0 => "Very low — mostly repeated values or padding.",
        e if e < 4.5 => "Low — text or structured data.",
        e if e < 7.0 => "Medium — executable code or mixed data.",
        e if e < 7.9 => "High — compressed data likely.",
        _ => "Very high — encrypted or well-compressed data likely.",
    }
}

fn draw_histogram(ui: &mut Ui, hist: &[u64; 256], log: bool) {
    let width = ui.available_width().max(256.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 120.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), ui.visuals().extreme_bg_color);
    let max = hist.iter().copied().max().unwrap_or(0).max(1) as f64;
    let scale = |v: u64| -> f32 {
        if v == 0 {
            0.0
        } else if log {
            ((v as f64).ln_1p() / max.ln_1p()) as f32
        } else {
            (v as f64 / max) as f32
        }
    };
    let bw = rect.width() / 256.0;
    for (i, &v) in hist.iter().enumerate() {
        let h = scale(v) * (rect.height() - 2.0);
        if h <= 0.0 {
            continue;
        }
        let x = rect.left() + i as f32 * bw;
        let color = match i {
            0 => Color32::from_gray(140),
            0x20..=0x7e => Color32::from_rgb(70, 140, 240),
            0x80..=0xff => Color32::from_rgb(235, 140, 50),
            _ => Color32::from_rgb(170, 100, 230),
        };
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x, rect.bottom() - h),
                pos2(x + bw.max(1.0), rect.bottom()),
            ),
            CornerRadius::ZERO,
            color,
        );
    }
    if let Some(pos) = resp.hover_pos() {
        let i = (((pos.x - rect.left()) / bw) as usize).min(255);
        let total: u64 = hist.iter().sum();
        let pct = if total > 0 {
            hist[i] as f64 * 100.0 / total as f64
        } else {
            0.0
        };
        let ch = if (0x20..0x7f).contains(&i) {
            format!(" '{}'", i as u8 as char)
        } else {
            String::new()
        };
        resp.on_hover_text_at_pointer(format!("0x{i:02X}{ch}: {} ({pct:.3}%)", hist[i]));
    }
}

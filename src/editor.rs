//! The hex view widget: virtualized rendering of the offset / hex / text
//! columns, mouse and keyboard editing, scrollbar and minimap.

use egui::{
    Align2, Color32, CornerRadius, Event, EventFilter, FontId, Id, Key, Modifiers, Pos2, Rect,
    Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::analysis::{BlockInfo, MinimapData};
use crate::commands::{Command, PasteMode};
use crate::document::Document;
use crate::export::{CopyFormat, hex_byte};
use crate::settings::{MinimapMode, Settings};

pub fn view_id() -> Id {
    Id::unique("hexeditor_main_view")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Hex,
    Text,
}

/// Per-tab cursor, selection and scroll state.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub cursor: usize,
    /// 0 = editing the high nibble, 1 = low nibble.
    pub nibble: u8,
    pub anchor: Option<usize>,
    pub pane: Pane,
    pub top_row: usize,
    pub h_offset: f32,
    pub scroll_to_cursor: bool,
    /// Center the cursor row when scrolling it into view.
    pub center_cursor: bool,
    /// Layout of the last frame (used by commands like page up/down).
    pub visible_rows: usize,
    pub bytes_per_row: usize,
    scroll_accum: f32,
    drag_origin: Option<usize>,
    vscroll_grab: Option<f32>,
    hscroll_grab: Option<f32>,
}

impl ViewState {
    /// Inclusive selection as a half-open `(start, end)` range.
    pub fn selection(&self, len: usize) -> Option<(usize, usize)> {
        let a = self.anchor?;
        let start = a.min(self.cursor);
        let end = (a.max(self.cursor) + 1).min(len);
        (start < end).then_some((start, end))
    }

    /// Selection, or the single byte under the cursor.
    pub fn selection_or_cursor(&self, len: usize) -> Option<(usize, usize)> {
        self.selection(len)
            .or_else(|| (self.cursor < len).then_some((self.cursor, self.cursor + 1)))
    }

    pub fn move_to(&mut self, pos: usize, extend: bool, len: usize) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = pos.min(len);
        self.nibble = 0;
        self.scroll_to_cursor = true;
    }

    /// Selects `start..end` (half-open). An empty range just moves the cursor.
    pub fn select(&mut self, start: usize, end: usize, len: usize) {
        let start = start.min(len);
        let end = end.min(len);
        if end > start {
            self.anchor = Some(start);
            self.cursor = end - 1;
        } else {
            self.anchor = None;
            self.cursor = start;
        }
        self.nibble = 0;
        self.scroll_to_cursor = true;
    }

    pub fn clamp(&mut self, len: usize) {
        self.cursor = self.cursor.min(len);
        if let Some(a) = self.anchor
            && a >= len
        {
            self.anchor = if len == 0 { None } else { Some(len - 1) };
        }
    }
}

// ---------------------------------------------------------------------------
// Editing operations shared with menu commands

/// Deletes the selection, or the byte under the cursor.
pub fn delete_forward(doc: &mut Document, view: &mut ViewState) -> bool {
    let len = doc.len();
    let Some((s, e)) = view.selection_or_cursor(len) else {
        return false;
    };
    let ok = doc.delete(s, e - s);
    if ok {
        view.select(s, s, doc.len());
    }
    ok
}

/// Deletes the selection, or the byte before the cursor (insert mode only;
/// in overwrite mode the cursor just moves left).
pub fn backspace(doc: &mut Document, view: &mut ViewState, insert_mode: bool) -> bool {
    if view.selection(doc.len()).is_some() {
        return delete_forward(doc, view);
    }
    if view.cursor == 0 {
        return false;
    }
    if insert_mode {
        let pos = view.cursor - 1;
        let ok = doc.delete(pos, 1);
        if ok {
            view.select(pos, pos, doc.len());
        }
        ok
    } else {
        view.move_to(view.cursor - 1, false, doc.len());
        doc.break_merge();
        true
    }
}

/// Writes pasted bytes: replaces the selection in insert mode, overwrites
/// from the selection start / cursor in overwrite mode.
pub fn write_bytes(
    doc: &mut Document,
    view: &mut ViewState,
    bytes: Vec<u8>,
    insert_mode: bool,
) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let n = bytes.len();
    let sel = view.selection(doc.len());
    let start = sel.map_or(view.cursor, |(s, _)| s);
    let ok = match (insert_mode, sel) {
        (true, Some((s, e))) => doc.replace(s, e - s, bytes, false),
        (true, None) => doc.insert(start, bytes, false),
        (false, _) => doc.overwrite(start, bytes, false),
    };
    if ok {
        view.select(start, start + n, doc.len());
    }
    ok
}

/// Removes the selection before typing (insert mode) or collapses it to its
/// start (overwrite mode).
fn prepare_typing(doc: &mut Document, view: &mut ViewState, insert_mode: bool) {
    if let Some((s, e)) = view.selection(doc.len()) {
        if insert_mode {
            doc.delete(s, e - s);
        }
        view.anchor = None;
        view.cursor = s;
        view.nibble = 0;
        doc.break_merge();
    }
}

fn type_nibble(doc: &mut Document, view: &mut ViewState, digit: u8, insert_mode: bool) {
    prepare_typing(doc, view, insert_mode);
    let pos = view.cursor;
    if view.nibble == 0 {
        if insert_mode || pos >= doc.len() {
            doc.insert(pos, vec![digit << 4], true);
        } else {
            let old = doc.get(pos).unwrap_or(0);
            doc.overwrite(pos, vec![(digit << 4) | (old & 0x0f)], true);
        }
        view.nibble = 1;
    } else {
        let old = doc.get(pos).unwrap_or(0);
        doc.overwrite(pos, vec![(old & 0xf0) | digit], true);
        view.nibble = 0;
        view.cursor = (pos + 1).min(doc.len());
    }
    view.scroll_to_cursor = true;
}

fn type_byte(doc: &mut Document, view: &mut ViewState, byte: u8, insert_mode: bool) {
    prepare_typing(doc, view, insert_mode);
    let pos = view.cursor;
    if insert_mode || pos >= doc.len() {
        doc.insert(pos, vec![byte], true);
    } else {
        doc.overwrite(pos, vec![byte], true);
    }
    view.cursor = (pos + 1).min(doc.len());
    view.nibble = 0;
    view.scroll_to_cursor = true;
}

// ---------------------------------------------------------------------------
// Layout

struct Metrics {
    cw: f32,
    row_h: f32,
    text_dy: f32,
    bpr: usize,
    group: usize,
    digits: usize,
    hex_x: f32,
    text_x: f32,
    content_w: f32,
    show_text: bool,
}

impl Metrics {
    fn new(settings: &Settings, cw: f32, glyph_h: f32, avail_w: f32, len: usize) -> Self {
        let group = settings.group_size.max(1);
        let digits = settings.offset_base.digits(len);
        let pad = cw * 0.5;
        let hex_x = pad + (digits as f32 + 2.0) * cw;
        let show_text = settings.show_text_column;
        let bpr = if settings.bytes_per_row == 0 {
            let per_byte = 3.0 * cw + cw / group as f32 + if show_text { cw } else { 0.0 };
            let fixed = hex_x + if show_text { 2.0 * cw } else { 0.0 } + pad;
            let fit = ((avail_w - fixed) / per_byte).floor().max(1.0) as usize;
            let unit = if fit >= 8 { group.max(8).min(fit) } else { 1 };
            (fit / unit * unit).clamp(1, 256)
        } else {
            settings.bytes_per_row.clamp(1, 256)
        };
        let row_h = (glyph_h * 1.3).round();
        let mut m = Self {
            cw,
            row_h,
            text_dy: ((row_h - glyph_h) * 0.5).round(),
            bpr,
            group,
            digits,
            hex_x,
            text_x: 0.0,
            content_w: 0.0,
            show_text,
        };
        let hex_end = m.col_hex_x(bpr - 1) + 2.0 * cw;
        m.text_x = hex_end + 2.0 * cw;
        m.content_w = if show_text {
            m.text_x + bpr as f32 * cw + pad
        } else {
            hex_end + pad
        };
        m
    }

    fn col_hex_x(&self, col: usize) -> f32 {
        self.hex_x + col as f32 * 3.0 * self.cw + (col / self.group) as f32 * self.cw
    }

    fn col_text_x(&self, col: usize) -> f32 {
        self.text_x + col as f32 * self.cw
    }

    /// Background rectangle of a hex cell, extended to touch the next cell.
    fn hex_cell_rect(&self, ox: f32, y: f32, col: usize) -> Rect {
        let x0 = ox + self.col_hex_x(col) - self.cw * 0.5;
        let x1 = if col + 1 < self.bpr {
            ox + self.col_hex_x(col + 1) - self.cw * 0.5
        } else {
            x0 + 3.0 * self.cw
        };
        Rect::from_min_max(pos2(x0, y), pos2(x1, y + self.row_h))
    }

    /// Maps a point to `(column, pane, nibble)`.
    fn hit_col(&self, x: f32) -> (usize, Pane, u8) {
        if self.show_text && x >= self.text_x - self.cw {
            let col = ((x - self.text_x) / self.cw).floor().max(0.0) as usize;
            return (col.min(self.bpr - 1), Pane::Text, 0);
        }
        let mut col = 0;
        for c in 0..self.bpr {
            if self.col_hex_x(c) - self.cw * 0.5 <= x {
                col = c;
            } else {
                break;
            }
        }
        let nibble = u8::from(x >= self.col_hex_x(col) + self.cw);
        (col, Pane::Hex, nibble)
    }
}

// ---------------------------------------------------------------------------
// Colors

struct Palette {
    bg: Color32,
    header_bg: Color32,
    text: Color32,
    weak: Color32,
    separator: Color32,
    selection: Color32,
    cursor: Color32,
    modified: Color32,
    match_bg: Color32,
    current_match_bg: Color32,
    selection_match_bg: Color32,
    diff_bg: Color32,
    row_highlight: Color32,
    dark: bool,
}

impl Palette {
    fn new(v: &egui::Visuals) -> Self {
        let dark = v.dark_mode;
        Self {
            bg: v.extreme_bg_color,
            header_bg: v.faint_bg_color,
            text: v.text_color(),
            weak: v.weak_text_color(),
            separator: v.widgets.noninteractive.bg_stroke.color,
            selection: v
                .selection
                .bg_fill
                .gamma_multiply(if dark { 0.8 } else { 0.6 }),
            cursor: if dark {
                Color32::from_rgb(255, 196, 64)
            } else {
                Color32::from_rgb(214, 110, 0)
            },
            modified: if dark {
                Color32::from_rgb(255, 100, 100)
            } else {
                Color32::from_rgb(205, 25, 25)
            },
            match_bg: Color32::from_rgba_unmultiplied(255, 210, 0, if dark { 60 } else { 110 }),
            current_match_bg: Color32::from_rgba_unmultiplied(
                255,
                140,
                0,
                if dark { 150 } else { 170 },
            ),
            selection_match_bg: Color32::from_rgba_unmultiplied(
                80,
                200,
                255,
                if dark { 45 } else { 70 },
            ),
            diff_bg: Color32::from_rgba_unmultiplied(255, 60, 60, if dark { 70 } else { 90 }),
            row_highlight: if dark {
                Color32::from_rgba_unmultiplied(255, 255, 255, 7)
            } else {
                Color32::from_rgba_unmultiplied(0, 0, 0, 10)
            },
            dark,
        }
    }

    fn byte(&self, b: u8, colored: bool) -> Color32 {
        let dark = self.dark;
        let rgb = Color32::from_rgb;
        if !colored {
            return if b == 0 { self.weak } else { self.text };
        }
        match b {
            0x00 => self.weak,
            0xff => {
                if dark {
                    rgb(255, 135, 100)
                } else {
                    rgb(195, 65, 35)
                }
            }
            0x09 | 0x0a | 0x0d | 0x20 => {
                if dark {
                    rgb(110, 210, 140)
                } else {
                    rgb(20, 135, 60)
                }
            }
            0x21..=0x7e => {
                if dark {
                    rgb(125, 180, 255)
                } else {
                    rgb(20, 85, 195)
                }
            }
            0x01..=0x1f | 0x7f => {
                if dark {
                    rgb(200, 150, 255)
                } else {
                    rgb(125, 55, 185)
                }
            }
            _ => {
                if dark {
                    rgb(235, 200, 110)
                } else {
                    rgb(150, 105, 0)
                }
            }
        }
    }

    fn block(&self, b: &BlockInfo, mode: MinimapMode) -> Color32 {
        match mode {
            MinimapMode::Entropy => {
                let t = b.entropy;
                let stops = [
                    (0.0, Color32::from_rgb(20, 30, 90)),
                    (0.45, Color32::from_rgb(30, 150, 110)),
                    (0.75, Color32::from_rgb(230, 200, 40)),
                    (1.0, Color32::from_rgb(230, 50, 40)),
                ];
                let mut c = stops[0].1;
                for w in stops.windows(2) {
                    let (t0, c0) = w[0];
                    let (t1, c1) = w[1];
                    if t >= t0 {
                        c = c0.lerp_to_gamma(c1, ((t - t0) / (t1 - t0)).clamp(0.0, 1.0));
                    }
                }
                c
            }
            MinimapMode::ByteClass => {
                let zero = if self.dark {
                    [35.0, 35.0, 40.0]
                } else {
                    [235.0, 235.0, 240.0]
                };
                let ascii = [60.0, 130.0, 240.0];
                let high = [235.0, 120.0, 40.0];
                let other = [170.0, 90.0, 220.0];
                let rest = (1.0 - b.zero - b.ascii - b.high).max(0.0);
                let mix = |i: usize| {
                    (zero[i] * b.zero + ascii[i] * b.ascii + high[i] * b.high + other[i] * rest)
                        .clamp(0.0, 255.0) as u8
                };
                Color32::from_rgb(mix(0), mix(1), mix(2))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Widget

pub struct HexViewArgs<'a> {
    pub settings: &'a mut Settings,
    pub insert_mode: bool,
    pub search_hits: &'a [usize],
    pub search_len: usize,
    pub current_hit: Option<usize>,
    pub compare: Option<&'a [u8]>,
    pub minimap: Option<&'a MinimapData>,
}

const HIT: u8 = 1;
const CURRENT_HIT: u8 = 2;
const SEL_MATCH: u8 = 4;

pub fn hex_view(
    ui: &mut Ui,
    doc: &mut Document,
    view: &mut ViewState,
    args: HexViewArgs<'_>,
) -> Vec<Command> {
    let mut cmds = Vec::new();
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());

    let font = FontId::monospace(args.settings.font_size);
    let (cw, glyph_h) = ui
        .ctx()
        .fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));

    let sb_w = 14.0;
    let mm_w = if args.settings.show_minimap {
        44.0
    } else {
        0.0
    };
    let area = Rect::from_min_max(full.min, pos2(full.max.x - sb_w - mm_w, full.max.y));
    let mut len = doc.len();
    let m = Metrics::new(args.settings, cw, glyph_h, area.width(), len);
    view.bytes_per_row = m.bpr;

    let header_h = if args.settings.show_header {
        m.row_h + 4.0
    } else {
        0.0
    };
    let needs_hscroll = m.content_w > area.width() + 0.5;
    let hscroll_h = if needs_hscroll { 10.0 } else { 0.0 };
    let header_rect = Rect::from_min_size(area.min, vec2(area.width(), header_h));
    let rows_rect = Rect::from_min_max(
        pos2(area.min.x, area.min.y + header_h),
        pos2(area.max.x, area.max.y - hscroll_h),
    );
    let visible_rows = ((rows_rect.height() / m.row_h).floor() as usize).max(1);
    view.visible_rows = visible_rows;
    let max_h_offset = (m.content_w - area.width()).max(0.0);

    // ---- interaction ------------------------------------------------------
    let id = view_id();
    let resp = ui.interact(area, id, Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
        resp.request_focus();
    }
    if ui.memory(|mem| mem.focused().is_none()) {
        ui.memory_mut(|mem| mem.request_focus(id));
    }
    let focused = resp.has_focus();
    if focused {
        ui.memory_mut(|mem| {
            mem.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
    }

    let (scroll, zoom, primary_pressed, secondary_pressed, primary_down, mods, pointer_pos) = ui
        .input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.pointer.primary_pressed(),
                i.pointer.secondary_pressed(),
                i.pointer.primary_down(),
                i.modifiers,
                i.pointer.interact_pos(),
            )
        });

    let total_rows = |len: usize| len / m.bpr + 1;
    let hovered_any = ui.rect_contains_pointer(full);

    if hovered_any {
        if zoom != 1.0 {
            args.settings.font_size = (args.settings.font_size * zoom).clamp(8.0, 40.0);
        }
        if scroll.y != 0.0 {
            view.scroll_accum -= scroll.y;
            let rows = (view.scroll_accum / m.row_h).trunc();
            if rows != 0.0 {
                view.scroll_accum -= rows * m.row_h;
                view.top_row = (view.top_row as i64 + rows as i64).max(0) as usize;
            }
        }
        if scroll.x != 0.0 {
            view.h_offset -= scroll.x;
        }
    }

    let ox_of = |h: f32| area.left() - h;
    let hit_test = |view: &ViewState, pos: Pos2, len: usize| -> (usize, Pane, u8) {
        let row_f = ((pos.y - rows_rect.top()) / m.row_h).floor();
        let row = (view.top_row as i64 + row_f as i64).max(0) as usize;
        let (col, pane, nibble) = m.hit_col(pos.x - ox_of(view.h_offset));
        let off = (row * m.bpr + col).min(len);
        (off, pane, nibble)
    };

    if let Some(pos) = pointer_pos
        && rows_rect.contains(pos)
        && resp.hovered()
    {
        if primary_pressed {
            let (off, pane, nibble) = hit_test(view, pos, len);
            view.pane = if m.show_text { pane } else { Pane::Hex };
            if mods.shift {
                view.anchor.get_or_insert(view.cursor);
                view.cursor = off;
            } else {
                view.anchor = None;
                view.cursor = off;
                view.drag_origin = Some(off);
            }
            view.nibble = if pane == Pane::Hex && off < len {
                nibble
            } else {
                0
            };
            doc.break_merge();
        } else if secondary_pressed {
            let (off, pane, _) = hit_test(view, pos, len);
            let inside = view
                .selection(len)
                .is_some_and(|(s, e)| off >= s && off < e);
            if !inside {
                view.anchor = None;
                view.cursor = off;
                view.nibble = 0;
                view.pane = if m.show_text { pane } else { Pane::Hex };
            }
        }
    }
    if primary_down {
        if let (Some(origin), Some(pos)) = (view.drag_origin, pointer_pos) {
            // Auto-scroll while dragging past the edges.
            if pos.y < rows_rect.top() {
                view.top_row = view.top_row.saturating_sub(1);
                ui.ctx().request_repaint();
            } else if pos.y > rows_rect.bottom() {
                view.top_row += 1;
                ui.ctx().request_repaint();
            }
            let clamped = pos2(
                pos.x,
                pos.y.clamp(rows_rect.top(), rows_rect.bottom() - 1.0),
            );
            let (off, _, _) = hit_test(view, clamped, len);
            if off != origin || view.anchor.is_some() {
                view.anchor = Some(origin);
                view.cursor = off;
                view.nibble = 0;
            }
        }
    } else {
        view.drag_origin = None;
    }

    // ---- keyboard -----------------------------------------------------------
    if focused {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => handle_key(
                    doc,
                    view,
                    key,
                    modifiers,
                    args.insert_mode,
                    m.show_text,
                    &mut cmds,
                ),
                Event::Text(text) => {
                    if doc.read_only {
                        if text.chars().any(|c| !c.is_control()) {
                            cmds.push(Command::ReadOnlyNotice);
                        }
                        continue;
                    }
                    for c in text.chars() {
                        match view.pane {
                            Pane::Hex => {
                                if let Some(d) = c.to_digit(16) {
                                    type_nibble(doc, view, d as u8, args.insert_mode);
                                }
                            }
                            Pane::Text => {
                                if let Some(b) = args.settings.text_encoding.encode(c) {
                                    type_byte(doc, view, b, args.insert_mode);
                                }
                            }
                        }
                    }
                }
                Event::Copy => cmds.push(Command::Copy),
                Event::Cut => cmds.push(Command::Cut),
                Event::Paste(text) => cmds.push(Command::PasteText(text)),
                _ => {}
            }
        }
        len = doc.len();
    }
    view.clamp(len);

    // ---- scrolling ----------------------------------------------------------
    let rows = total_rows(len);
    let max_top = rows.saturating_sub(visible_rows);
    if view.scroll_to_cursor {
        let cursor_row = view.cursor / m.bpr;
        if view.center_cursor {
            view.top_row = cursor_row.saturating_sub(visible_rows / 2);
        } else if cursor_row < view.top_row {
            view.top_row = cursor_row;
        } else if cursor_row >= view.top_row + visible_rows {
            view.top_row = cursor_row + 1 - visible_rows;
        }
        let col = view.cursor % m.bpr;
        let (x0, x1) = match view.pane {
            Pane::Hex => (m.col_hex_x(col) - m.cw, m.col_hex_x(col) + 3.0 * m.cw),
            Pane::Text => (m.col_text_x(col) - m.cw, m.col_text_x(col) + 2.0 * m.cw),
        };
        if x0 < view.h_offset {
            view.h_offset = x0;
        } else if x1 > view.h_offset + area.width() {
            view.h_offset = x1 - area.width();
        }
        view.scroll_to_cursor = false;
        view.center_cursor = false;
    }
    view.top_row = view.top_row.min(max_top);
    view.h_offset = view.h_offset.clamp(0.0, max_h_offset);

    // ---- painting -----------------------------------------------------------
    let pal = Palette::new(ui.visuals());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::ZERO, pal.bg);
    let ox = ox_of(view.h_offset);
    let data = doc.bytes();
    let selection = view.selection(len);
    let cursor_col = view.cursor % m.bpr;
    let settings = &*args.settings;
    let upper = settings.uppercase_hex;

    if args.settings.show_header {
        painter.rect_filled(header_rect, CornerRadius::ZERO, pal.header_bg);
        let y = header_rect.top() + 2.0 + m.text_dy;
        let label = format!("Offset({})", settings.offset_base.suffix());
        let label = if label.len() > m.digits + 1 {
            settings.offset_base.suffix().to_string()
        } else {
            label
        };
        painter.text(
            pos2(ox + m.cw * 0.5, y),
            Align2::LEFT_TOP,
            label,
            font.clone(),
            pal.weak,
        );
        for col in 0..m.bpr {
            let color = if col == cursor_col {
                pal.cursor
            } else {
                pal.weak
            };
            let text = if m.bpr <= 256 {
                hex_byte(col as u8, upper)
            } else {
                ""
            };
            painter.text(
                pos2(ox + m.col_hex_x(col), y),
                Align2::LEFT_TOP,
                text,
                font.clone(),
                color,
            );
            if m.show_text {
                let digit = &hex_byte((col % 16) as u8, upper)[1..];
                painter.text(
                    pos2(ox + m.col_text_x(col), y),
                    Align2::LEFT_TOP,
                    digit,
                    font.clone(),
                    color,
                );
            }
        }
        painter.hline(
            area.x_range(),
            header_rect.bottom() - 0.5,
            Stroke::new(1.0, pal.separator),
        );
    }

    // Column separators
    let sep_stroke = Stroke::new(1.0, pal.separator);
    let sep1 = ox + m.hex_x - m.cw;
    painter.vline(sep1, rows_rect.y_range(), sep_stroke);
    if m.show_text {
        painter.vline(ox + m.text_x - m.cw, rows_rect.y_range(), sep_stroke);
    }

    let rows_to_draw = visible_rows + 1;
    let win_start = view.top_row * m.bpr;
    let win_end = ((view.top_row + rows_to_draw) * m.bpr).min(len);
    let mut flags = vec![0u8; win_end.saturating_sub(win_start)];

    if args.search_len > 0 {
        let first = args
            .search_hits
            .partition_point(|&h| h + args.search_len <= win_start);
        for &h in &args.search_hits[first..] {
            if h >= win_end {
                break;
            }
            let bit = if Some(h) == args.current_hit {
                CURRENT_HIT
            } else {
                HIT
            };
            for o in h.max(win_start)..(h + args.search_len).min(win_end) {
                flags[o - win_start] |= bit;
            }
        }
    }
    if settings.highlight_selection_matches
        && let Some((s, e)) = selection
        && e - s <= 256
        && e - s >= 1
        && !(e - s == 1 && data[s] == 0)
    {
        let needle = &data[s..e];
        let n = needle.len();
        let ws = win_start.saturating_sub(n - 1);
        let we = (win_end + n - 1).min(len);
        if we > ws {
            for p in memchr::memmem::find_iter(&data[ws..we], needle) {
                let p = p + ws;
                if p == s {
                    continue;
                }
                for o in p.max(win_start)..(p + n).min(win_end) {
                    flags[o - win_start] |= SEL_MATCH;
                }
            }
        }
    }
    let bookmarks: Vec<(usize, usize, Color32)> = doc
        .bookmarks
        .iter()
        .filter(|b| b.offset < win_end.max(win_start + 1) && b.offset + b.len.max(1) > win_start)
        .map(|b| {
            let [r, g, bl] = b.color;
            (
                b.offset,
                b.offset + b.len.max(1),
                Color32::from_rgba_unmultiplied(r, g, bl, 70),
            )
        })
        .collect();

    let char_rect = |x: f32, y: f32, w: f32| Rect::from_min_size(pos2(x, y), vec2(w, m.row_h));

    for r in 0..rows_to_draw {
        let row = view.top_row + r;
        let row_start = row * m.bpr;
        if row_start > len {
            break;
        }
        let y = rows_rect.top() + r as f32 * m.row_h;
        let ty = y + m.text_dy;
        let cursor_row = view.cursor / m.bpr == row;
        if cursor_row {
            painter.rect_filled(
                Rect::from_min_max(pos2(area.left(), y), pos2(area.right(), y + m.row_h)),
                CornerRadius::ZERO,
                pal.row_highlight,
            );
        }
        let off_text = settings.offset_base.format(row_start, m.digits, upper);
        painter.text(
            pos2(ox + m.cw * 0.5, ty),
            Align2::LEFT_TOP,
            off_text,
            font.clone(),
            if cursor_row { pal.text } else { pal.weak },
        );

        for col in 0..m.bpr {
            let off = row_start + col;
            if off > len {
                break;
            }
            let hx = ox + m.col_hex_x(col);
            let tx = ox + m.col_text_x(col);
            let is_cursor = off == view.cursor;
            if off == len {
                if is_cursor {
                    draw_cursor(
                        &painter,
                        &m,
                        &pal,
                        hx,
                        tx,
                        y,
                        view,
                        args.insert_mode,
                        focused,
                    );
                }
                break;
            }
            let b = data[off];
            let cell = m.hex_cell_rect(ox, y, col);
            let tcell = char_rect(tx, y, m.cw);

            // Backgrounds, lowest priority first.
            let mut bgs: [Option<Color32>; 5] = [None; 5];
            if let Some(&(_, _, c)) = bookmarks
                .iter()
                .rev()
                .find(|(s, e, _)| off >= *s && off < *e)
            {
                bgs[0] = Some(c);
            }
            let f = flags[off - win_start];
            if f & SEL_MATCH != 0 {
                bgs[1] = Some(pal.selection_match_bg);
            }
            if f & (HIT | CURRENT_HIT) != 0 {
                bgs[2] = Some(if f & CURRENT_HIT != 0 {
                    pal.current_match_bg
                } else {
                    pal.match_bg
                });
            }
            if let Some(other) = args.compare
                && other.get(off) != Some(&b)
            {
                bgs[3] = Some(pal.diff_bg);
            }
            if selection.is_some_and(|(s, e)| off >= s && off < e) {
                bgs[4] = Some(pal.selection);
            }
            for c in bgs.into_iter().flatten() {
                painter.rect_filled(cell, CornerRadius::ZERO, c);
                if m.show_text {
                    painter.rect_filled(tcell, CornerRadius::ZERO, c);
                }
            }
            if is_cursor {
                draw_cursor(
                    &painter,
                    &m,
                    &pal,
                    hx,
                    tx,
                    y,
                    view,
                    args.insert_mode,
                    focused,
                );
            }

            let color = if settings.highlight_modified && doc.modified.contains(off) {
                pal.modified
            } else {
                pal.byte(b, settings.color_bytes)
            };
            painter.text(
                pos2(hx, ty),
                Align2::LEFT_TOP,
                hex_byte(b, upper),
                font.clone(),
                color,
            );
            if m.show_text {
                match settings.text_encoding.decode(b) {
                    Some(c) => {
                        let mut buf = [0u8; 4];
                        painter.text(
                            pos2(tx, ty),
                            Align2::LEFT_TOP,
                            &*c.encode_utf8(&mut buf),
                            font.clone(),
                            color,
                        );
                    }
                    None => {
                        painter.text(
                            pos2(tx, ty),
                            Align2::LEFT_TOP,
                            "·",
                            font.clone(),
                            pal.weak.gamma_multiply(0.7),
                        );
                    }
                }
            }
        }
    }

    // ---- horizontal scrollbar ----------------------------------------------
    if needs_hscroll {
        let track = Rect::from_min_max(pos2(area.left(), area.bottom() - hscroll_h), area.max);
        let hresp = ui.interact(track, id.with("hscroll"), Sense::click_and_drag());
        let thumb_w = (track.width() * area.width() / m.content_w).max(24.0);
        let free = (track.width() - thumb_w).max(1.0);
        if hresp.is_pointer_button_down_on() {
            if let Some(p) = pointer_pos {
                let thumb_x = track.left() + free * (view.h_offset / max_h_offset.max(1.0));
                let grab = *view.hscroll_grab.get_or_insert_with(|| {
                    if p.x >= thumb_x && p.x <= thumb_x + thumb_w {
                        p.x - thumb_x
                    } else {
                        thumb_w / 2.0
                    }
                });
                let frac = ((p.x - grab - track.left()) / free).clamp(0.0, 1.0);
                view.h_offset = frac * max_h_offset;
            }
        } else {
            view.hscroll_grab = None;
        }
        let thumb_x = track.left() + free * (view.h_offset / max_h_offset.max(1.0));
        let thumb = Rect::from_min_size(
            pos2(thumb_x, track.top() + 2.0),
            vec2(thumb_w, hscroll_h - 4.0),
        );
        painter.rect_filled(track, CornerRadius::ZERO, pal.header_bg);
        let thumb_color = if hresp.hovered() || hresp.is_pointer_button_down_on() {
            pal.text
        } else {
            pal.weak
        };
        painter.rect_filled(
            thumb,
            CornerRadius::same(3),
            thumb_color.gamma_multiply(0.6),
        );
    }

    // ---- minimap ------------------------------------------------------------
    let side_painter = ui.painter();
    if mm_w > 0.0 {
        let mm_rect = Rect::from_min_max(
            pos2(area.right(), full.top()),
            pos2(area.right() + mm_w, full.bottom()),
        );
        side_painter.rect_filled(mm_rect, CornerRadius::ZERO, pal.header_bg);
        let inner = mm_rect.shrink2(vec2(3.0, 2.0));
        let mresp = ui.interact(mm_rect, id.with("minimap"), Sense::click_and_drag());
        if mresp.is_pointer_button_down_on()
            && let Some(p) = pointer_pos
            && len > 0
        {
            let frac = ((p.y - inner.top()) / inner.height()).clamp(0.0, 1.0);
            let target_row = (frac * len as f32) as usize / m.bpr;
            view.top_row = target_row.saturating_sub(visible_rows / 2).min(max_top);
        }
        if let Some(mm) = args.minimap.filter(|mm| !mm.blocks.is_empty()) {
            let h = inner.height().max(1.0) as usize;
            let nb = mm.blocks.len();
            let mut py = 0;
            while py < h {
                let bi = (py * nb / h).min(nb - 1);
                let color = pal.block(&mm.blocks[bi], settings.minimap_mode);
                let mut run = 1;
                while py + run < h && (((py + run) * nb / h).min(nb - 1)) == bi {
                    run += 1;
                }
                side_painter.rect_filled(
                    Rect::from_min_size(
                        pos2(inner.left(), inner.top() + py as f32),
                        vec2(inner.width(), run as f32),
                    ),
                    CornerRadius::ZERO,
                    color,
                );
                py += run;
            }
        }
        if len > 0 {
            let y_of = |off: usize| inner.top() + inner.height() * (off as f32 / len as f32);
            // Search hits, sampled to one tick per pixel row.
            let mut last_y = f32::NEG_INFINITY;
            for &h in args.search_hits {
                let y = y_of(h).round();
                if y > last_y {
                    side_painter.hline(
                        inner.right() - 6.0..=inner.right(),
                        y,
                        Stroke::new(1.0, Color32::from_rgb(255, 210, 0)),
                    );
                    last_y = y;
                }
            }
            for b in &doc.bookmarks {
                let [r, g, bl] = b.color;
                side_painter.hline(
                    inner.left()..=inner.left() + 6.0,
                    y_of(b.offset).round(),
                    Stroke::new(2.0, Color32::from_rgb(r, g, bl)),
                );
            }
            let vy0 = y_of(win_start);
            let vy1 = y_of((view.top_row + visible_rows) * m.bpr).max(vy0 + 3.0);
            side_painter.rect_stroke(
                Rect::from_min_max(
                    pos2(mm_rect.left() + 1.0, vy0),
                    pos2(mm_rect.right() - 1.0, vy1.min(inner.bottom())),
                ),
                CornerRadius::same(2),
                Stroke::new(1.5, pal.text.gamma_multiply(0.8)),
                StrokeKind::Inside,
            );
            side_painter.hline(
                inner.x_range(),
                y_of(view.cursor).round(),
                Stroke::new(1.0, pal.cursor),
            );
        }
    }

    // ---- vertical scrollbar ---------------------------------------------------
    {
        let sb_rect = Rect::from_min_max(pos2(full.right() - sb_w, full.top()), full.max);
        let track = sb_rect.shrink2(vec2(3.0, 3.0));
        let sresp = ui.interact(sb_rect, id.with("vscroll"), Sense::click_and_drag());
        let thumb_h = (track.height() * visible_rows as f32 / rows.max(1) as f32)
            .clamp(track.height().min(24.0), track.height());
        let free = (track.height() - thumb_h).max(0.0);
        let thumb_y = |top: usize| {
            track.top()
                + if max_top > 0 {
                    free * top as f32 / max_top as f32
                } else {
                    0.0
                }
        };
        if sresp.is_pointer_button_down_on() {
            if let Some(p) = pointer_pos {
                let ty = thumb_y(view.top_row);
                let grab = *view.vscroll_grab.get_or_insert_with(|| {
                    if p.y >= ty && p.y <= ty + thumb_h {
                        p.y - ty
                    } else {
                        thumb_h / 2.0
                    }
                });
                if free > 0.0 {
                    let frac = ((p.y - grab - track.top()) / free).clamp(0.0, 1.0);
                    view.top_row = (frac * max_top as f32).round() as usize;
                }
            }
        } else {
            view.vscroll_grab = None;
        }
        side_painter.rect_filled(sb_rect, CornerRadius::ZERO, pal.header_bg);
        let thumb = Rect::from_min_size(
            pos2(track.left(), thumb_y(view.top_row)),
            vec2(track.width(), thumb_h),
        );
        let active = sresp.hovered() || sresp.is_pointer_button_down_on();
        let thumb_color = if active { pal.text } else { pal.weak };
        side_painter.rect_filled(
            thumb,
            CornerRadius::same(4),
            thumb_color.gamma_multiply(0.55),
        );
    }

    // ---- context menu ----------------------------------------------------------
    let has_sel = selection.is_some();
    let writable = !doc.read_only;
    resp.context_menu(|ui| {
        macro_rules! item {
            ($label:expr, $cmd:expr, $enabled:expr) => {
                if ui
                    .add_enabled($enabled, egui::Button::new($label))
                    .clicked()
                {
                    cmds.push($cmd);
                    ui.close();
                }
            };
        }
        item!("Cut", Command::Cut, has_sel && writable);
        item!("Copy", Command::Copy, true);
        ui.menu_button("Copy as", |ui| {
            for fmt in CopyFormat::ALL {
                if ui.button(fmt.name()).clicked() {
                    cmds.push(Command::CopyAs(fmt));
                    ui.close();
                }
            }
        });
        item!("Copy offset", Command::CopyOffset, true);
        item!("Paste", Command::Paste(PasteMode::Auto), writable);
        item!("Delete", Command::Delete, writable);
        ui.separator();
        item!("Select all", Command::SelectAll, true);
        item!("Search for selection", Command::SearchSelection, has_sel);
        item!("Toggle bookmark", Command::ToggleBookmark, true);
        ui.separator();
        item!("Fill selection…", Command::Fill, has_sel && writable);
        item!("Transform…", Command::Transform, writable);
        item!("Insert bytes…", Command::InsertBytes, writable);
        item!("Save selection as…", Command::SaveSelectionAs, has_sel);
    });

    cmds
}

#[allow(clippy::too_many_arguments)]
fn draw_cursor(
    painter: &egui::Painter,
    m: &Metrics,
    pal: &Palette,
    hx: f32,
    tx: f32,
    y: f32,
    view: &ViewState,
    insert_mode: bool,
    focused: bool,
) {
    let cw = m.cw;
    let fill = pal.cursor.gamma_multiply(if focused { 0.45 } else { 0.2 });
    let stroke = Stroke::new(1.0, pal.cursor);
    let hex_rect = Rect::from_min_size(pos2(hx - 1.0, y), vec2(2.0 * cw + 2.0, m.row_h));
    let text_rect = Rect::from_min_size(pos2(tx, y), vec2(cw, m.row_h));
    let (active, inactive) = match view.pane {
        Pane::Hex => (hex_rect, text_rect),
        Pane::Text => (text_rect, hex_rect),
    };
    if insert_mode {
        painter.vline(
            active.left(),
            active.y_range(),
            Stroke::new(2.0, pal.cursor),
        );
    } else if view.pane == Pane::Hex {
        let nib = Rect::from_min_size(pos2(hx + view.nibble as f32 * cw, y), vec2(cw, m.row_h));
        painter.rect_filled(hex_rect, CornerRadius::same(2), fill.gamma_multiply(0.4));
        painter.rect_filled(nib, CornerRadius::same(2), fill);
        painter.hline(
            nib.x_range(),
            nib.bottom() - 1.0,
            Stroke::new(2.0, pal.cursor),
        );
    } else {
        painter.rect_filled(active, CornerRadius::same(2), fill);
        painter.hline(
            active.x_range(),
            active.bottom() - 1.0,
            Stroke::new(2.0, pal.cursor),
        );
    }
    if m.show_text {
        painter.rect_stroke(inactive, CornerRadius::same(2), stroke, StrokeKind::Inside);
    }
}

fn handle_key(
    doc: &mut Document,
    view: &mut ViewState,
    key: Key,
    mods: Modifiers,
    insert_mode: bool,
    show_text: bool,
    cmds: &mut Vec<Command>,
) {
    let len = doc.len();
    let bpr = view.bytes_per_row.max(1);
    let page = view.visible_rows.max(1) * bpr;
    let shift = mods.shift;
    let cur = view.cursor;
    let moved = |view: &mut ViewState, pos: usize| {
        view.move_to(pos, shift, len);
    };
    match key {
        Key::ArrowLeft if mods.command => moved(view, cur.saturating_sub(bpr.clamp(1, 8))),
        Key::ArrowRight if mods.command => moved(view, (cur + bpr.clamp(1, 8)).min(len)),
        Key::ArrowLeft => {
            if view.pane == Pane::Hex && view.nibble == 1 && !shift {
                view.nibble = 0;
            } else {
                moved(view, cur.saturating_sub(1));
            }
        }
        Key::ArrowRight => moved(view, (cur + 1).min(len)),
        Key::ArrowUp => {
            if cur >= bpr {
                moved(view, cur - bpr);
            } else if shift {
                moved(view, 0);
            }
        }
        Key::ArrowDown => {
            if cur + bpr <= len {
                moved(view, cur + bpr);
            } else if shift {
                moved(view, len);
            }
        }
        Key::PageUp => {
            moved(view, cur.saturating_sub(page));
            view.top_row = view.top_row.saturating_sub(view.visible_rows);
        }
        Key::PageDown => {
            moved(view, (cur + page).min(len));
            view.top_row += view.visible_rows;
        }
        Key::Home if mods.command => moved(view, 0),
        Key::End if mods.command => moved(view, len),
        Key::Home => moved(view, cur - cur % bpr),
        Key::End => moved(view, (cur - cur % bpr + bpr - 1).min(len)),
        Key::Tab if !mods.command && show_text => {
            view.pane = match view.pane {
                Pane::Hex => Pane::Text,
                Pane::Text => Pane::Hex,
            };
            view.nibble = 0;
            doc.break_merge();
        }
        Key::Insert if !mods.any() => cmds.push(Command::ToggleInsertMode),
        Key::Delete if !mods.command => cmds.push(Command::Delete),
        Key::Backspace => {
            if doc.read_only {
                cmds.push(Command::ReadOnlyNotice);
            } else {
                backspace(doc, view, insert_mode);
            }
        }
        Key::Escape => {
            view.anchor = None;
            view.nibble = 0;
            doc.break_merge();
        }
        _ => {}
    }
    if matches!(
        key,
        Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::PageUp
            | Key::PageDown
            | Key::Home
            | Key::End
    ) {
        doc.break_merge();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(bytes: &[u8]) -> (Document, ViewState) {
        (
            Document::from_bytes(bytes.to_vec(), None, "t".into()),
            ViewState::default(),
        )
    }

    #[test]
    fn selection_is_inclusive() {
        let mut v = ViewState {
            anchor: Some(5),
            cursor: 2,
            ..Default::default()
        };
        assert_eq!(v.selection(10), Some((2, 6)));
        assert_eq!(v.selection(4), Some((2, 4)));
        v.anchor = None;
        assert_eq!(v.selection(10), None);
        assert_eq!(v.selection_or_cursor(10), Some((2, 3)));
    }

    #[test]
    fn typing_hex_overwrite() {
        let (mut d, mut v) = setup(&[0x00, 0x00]);
        type_nibble(&mut d, &mut v, 0xA, false);
        type_nibble(&mut d, &mut v, 0xB, false);
        type_nibble(&mut d, &mut v, 0xC, false);
        assert_eq!(d.bytes(), &[0xAB, 0xC0]);
        assert_eq!(v.cursor, 1);
        assert_eq!(v.nibble, 1);
    }

    #[test]
    fn typing_hex_insert_and_append() {
        let (mut d, mut v) = setup(&[0x11]);
        type_nibble(&mut d, &mut v, 0x2, true);
        type_nibble(&mut d, &mut v, 0x2, true);
        assert_eq!(d.bytes(), &[0x22, 0x11]);
        v.cursor = 2;
        type_byte(&mut d, &mut v, b'A', false);
        assert_eq!(d.bytes(), &[0x22, 0x11, b'A']);
    }

    #[test]
    fn paste_and_delete() {
        let (mut d, mut v) = setup(b"abcdef");
        v.select(1, 3, 6);
        write_bytes(&mut d, &mut v, b"XYZ".to_vec(), true);
        assert_eq!(d.bytes(), b"aXYZdef");
        assert_eq!(v.selection(d.len()), Some((1, 4)));
        delete_forward(&mut d, &mut v);
        assert_eq!(d.bytes(), b"adef");
        v.cursor = 2;
        backspace(&mut d, &mut v, true);
        assert_eq!(d.bytes(), b"aef");
        write_bytes(&mut d, &mut v, b"QQ".to_vec(), false);
        assert_eq!(d.bytes(), b"aQQ");
    }
}

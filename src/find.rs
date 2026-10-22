//! Find / replace bar and search execution.

use egui::text::CCursorRange;
use egui::{Key, RichText, TextEdit, TextStyle, Ui};

use crate::app::{HexApp, plural};
use crate::search::{Pattern, QuerySpec, SearchKind, TextSearchEncoding};
use crate::settings::SidePanel;
use crate::values::NumType;

/// Maximum number of matches collected by "find all".
pub const MAX_HITS: usize = 200_000;
/// Files up to this size get live match highlighting while typing.
const LIVE_SEARCH_LIMIT: usize = 16 << 20;

#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub replace: bool,
    pub query: String,
    pub replacement: String,
    pub focus: bool,
    pub error: Option<String>,
    /// (tab id, doc version, query, spec) of the last live search.
    live_key: Option<(u64, u64, String, QuerySpec)>,
}

/// Matches found in a document.
#[derive(Default, Debug)]
pub struct SearchResults {
    pub hits: Vec<usize>,
    pub len: usize,
    pub truncated: bool,
    /// Index into `hits` of the current match.
    pub current: Option<usize>,
    pub version: u64,
    pub query: String,
}

impl HexApp {
    fn build_pattern(&mut self) -> Option<Pattern> {
        match self.settings.search.build(&self.find.query) {
            Ok(p) => {
                self.find.error = None;
                Some(p)
            }
            Err(e) => {
                self.find.error = Some(e);
                None
            }
        }
    }

    pub(crate) fn find_step(&mut self, forward: bool) {
        let Some(pattern) = self.build_pattern() else {
            self.find.open = true;
            return;
        };
        let Some(tab) = self.tab_mut() else { return };
        let data = tab.doc.bytes();
        let len = data.len();
        let sel = tab.view.selection(len);
        let found = if forward {
            let from = match sel {
                Some((s, _)) if pattern.matches_at(data, s) => s + 1,
                Some((s, _)) => s,
                None => tab.view.cursor,
            };
            pattern
                .find_forward(data, from)
                .map(|p| (p, false))
                .or_else(|| pattern.find_forward(data, 0).map(|p| (p, true)))
        } else {
            let before = sel.map_or(tab.view.cursor, |(s, _)| s);
            pattern
                .find_backward(data, before)
                .map(|p| (p, false))
                .or_else(|| pattern.find_backward(data, len + 1).map(|p| (p, true)))
        };
        match found {
            Some((pos, wrapped)) => {
                let n = pattern.len();
                tab.view.select(pos, pos + n, len);
                tab.view.cursor = pos;
                tab.view.anchor = Some(pos + n - 1);
                tab.view.center_cursor = true;
                tab.search.current = tab.search.hits.binary_search(&pos).ok();
                if wrapped {
                    self.set_status("Search wrapped around");
                }
            }
            None => self.set_status_error("Pattern not found"),
        }
    }

    pub(crate) fn find_all(&mut self) {
        let Some(pattern) = self.build_pattern() else {
            self.find.open = true;
            return;
        };
        let query = self.find.query.clone();
        let Some(tab) = self.tab_mut() else { return };
        let (hits, truncated) = pattern.find_all(tab.doc.bytes(), MAX_HITS);
        let count = hits.len();
        tab.search = SearchResults {
            hits,
            len: pattern.len(),
            truncated,
            current: None,
            version: tab.doc.version,
            query,
        };
        self.settings.show_side_panel = true;
        self.settings.side_panel = SidePanel::SearchResults;
        if count == 0 {
            self.set_status_error("Pattern not found");
        } else {
            self.set_status(format!(
                "Found {}{}",
                plural(count, "match"),
                if truncated { " (limit reached)" } else { "" }
            ));
        }
    }

    fn replacement_bytes(&mut self) -> Option<Vec<u8>> {
        match self
            .settings
            .search
            .replacement_bytes(&self.find.replacement)
        {
            Ok(b) => Some(b),
            Err(e) => {
                self.find.error = Some(format!("Replacement: {e}"));
                None
            }
        }
    }

    fn replace_current(&mut self) {
        let Some(pattern) = self.build_pattern() else {
            return;
        };
        let Some(rep) = self.replacement_bytes() else {
            return;
        };
        let Some(tab) = self.tab_mut() else { return };
        if tab.doc.read_only {
            return self.set_status_error("Document is read-only");
        }
        let len = tab.doc.len();
        if let Some((s, e)) = tab.view.selection(len)
            && e - s == pattern.len()
            && pattern.matches_at(tab.doc.bytes(), s)
        {
            let n = rep.len();
            tab.doc.replace(s, e - s, rep, false);
            let len = tab.doc.len();
            tab.view.select(s + n, s + n, len);
        }
        self.find_step(true);
    }

    fn replace_all(&mut self) {
        let Some(pattern) = self.build_pattern() else {
            return;
        };
        let Some(rep) = self.replacement_bytes() else {
            return;
        };
        let Some(tab) = self.tab_mut() else { return };
        if tab.doc.read_only {
            return self.set_status_error("Document is read-only");
        }
        let data = tab.doc.bytes();
        let (hits, _) = pattern.find_all(data, usize::MAX);
        let (Some(&first), Some(&last)) = (hits.first(), hits.last()) else {
            return self.set_status_error("Pattern not found");
        };
        let n = pattern.len();
        let end = last + n;
        let mut span = Vec::with_capacity(end - first + hits.len() * rep.len().saturating_sub(n));
        let mut pos = first;
        for &h in &hits {
            span.extend_from_slice(&data[pos..h]);
            span.extend_from_slice(&rep);
            pos = h + n;
        }
        let new_len = span.len();
        tab.doc.replace(first, end - first, span, false);
        let len = tab.doc.len();
        tab.view.select(first, first + new_len, len);
        tab.search = SearchResults::default();
        self.find.live_key = None;
        self.set_status(format!("Replaced {}", plural(hits.len(), "occurrence")));
    }

    /// Re-runs the search as the query changes so matches are highlighted live.
    fn live_search(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if tab.doc.len() > LIVE_SEARCH_LIMIT {
            return;
        }
        let key = (
            tab.id,
            tab.doc.version,
            self.find.query.clone(),
            self.settings.search.clone(),
        );
        if self.find.live_key.as_ref() == Some(&key) {
            return;
        }
        self.find.live_key = Some(key);
        if self.find.query.trim().is_empty() {
            self.find.error = None;
            if let Some(tab) = self.tab_mut() {
                tab.search = Default::default();
            }
            return;
        }
        let pattern = self.settings.search.build(&self.find.query);
        let query = self.find.query.clone();
        let tab = &mut self.tabs[self.active];
        match pattern {
            Ok(p) => {
                self.find.error = None;
                let (hits, truncated) = p.find_all(tab.doc.bytes(), MAX_HITS);
                let current = tab
                    .view
                    .selection(tab.doc.len())
                    .and_then(|(s, _)| hits.binary_search(&s).ok());
                tab.search = SearchResults {
                    hits,
                    len: p.len(),
                    truncated,
                    current,
                    version: tab.doc.version,
                    query,
                };
            }
            Err(e) => {
                self.find.error = Some(e);
                tab.search = Default::default();
            }
        }
    }

    fn close_find_bar(&mut self) {
        self.find.open = false;
        self.find.live_key = None;
        for tab in &mut self.tabs {
            tab.search.current = None;
        }
        if let Some(tab) = self.tab_mut() {
            tab.search = Default::default();
        }
    }

    pub(crate) fn find_bar(&mut self, ui: &mut Ui) {
        let mut action: Option<fn(&mut HexApp)> = None;
        let mut close = false;
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label("Find");
            let spec = &mut self.settings.search;
            let hint = match spec.kind {
                SearchKind::Hex => "e.g. 4D 5A ?? 00",
                SearchKind::Text => "text",
                SearchKind::Value => "e.g. 1234, -5, 0x1F, 3.14",
            };
            let mut output = TextEdit::singleline(&mut self.find.query)
                .hint_text(hint)
                .font(TextStyle::Monospace)
                .desired_width(260.0)
                .show(ui);
            let resp = output.response.clone();
            if self.find.focus {
                // Select the (possibly pre-filled) query so typing replaces it.
                resp.request_focus();
                output
                    .state
                    .cursor
                    .set_char_range(Some(CCursorRange::select_all_str(&self.find.query)));
                output.state.store(ui.ctx(), resp.id);
                self.find.focus = false;
            }
            if resp.lost_focus() {
                let (enter, shift, esc) = ui.input(|i| {
                    (
                        i.key_pressed(Key::Enter),
                        i.modifiers.shift,
                        i.key_pressed(Key::Escape),
                    )
                });
                if enter {
                    action = Some(if shift {
                        |a| a.find_step(false)
                    } else {
                        |a| a.find_step(true)
                    });
                    resp.request_focus();
                } else if esc {
                    close = true;
                }
            }

            egui::ComboBox::from_id_salt("find_kind")
                .width(70.0)
                .selected_text(spec.kind.name())
                .show_ui(ui, |ui| {
                    for k in SearchKind::ALL {
                        ui.selectable_value(&mut spec.kind, k, k.name());
                    }
                });
            match spec.kind {
                SearchKind::Hex => {
                    ui.weak("? = wildcard nibble");
                }
                SearchKind::Text => {
                    egui::ComboBox::from_id_salt("find_text_enc")
                        .selected_text(spec.text_encoding.name())
                        .show_ui(ui, |ui| {
                            for e in TextSearchEncoding::ALL {
                                ui.selectable_value(&mut spec.text_encoding, e, e.name());
                            }
                        });
                    ui.checkbox(&mut spec.case_sensitive, "Match case");
                }
                SearchKind::Value => {
                    egui::ComboBox::from_id_salt("find_num_type")
                        .selected_text(spec.num_type.name())
                        .show_ui(ui, |ui| {
                            for t in NumType::ALL {
                                ui.selectable_value(&mut spec.num_type, t, t.name());
                            }
                        });
                    ui.selectable_value(&mut spec.big_endian, false, "LE");
                    ui.selectable_value(&mut spec.big_endian, true, "BE");
                }
            }
            ui.separator();
            if ui
                .button("⏶ Prev")
                .on_hover_text("Find previous (Shift+F3)")
                .clicked()
            {
                action = Some(|a| a.find_step(false));
            }
            if ui
                .button("⏷ Next")
                .on_hover_text("Find next (F3)")
                .clicked()
            {
                action = Some(|a| a.find_step(true));
            }
            if ui
                .button("Find all")
                .on_hover_text("List all matches (Ctrl+Shift+F)")
                .clicked()
            {
                action = Some(HexApp::find_all);
            }
            ui.toggle_value(&mut self.find.replace, "Replace…");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("🗙").on_hover_text("Close (Esc)").clicked() {
                    close = true;
                }
                if let Some(tab) = self.tabs.get(self.active) {
                    let s = &tab.search;
                    if !s.hits.is_empty() {
                        let pos = s.current.map_or(String::from("–"), |c| (c + 1).to_string());
                        ui.weak(format!(
                            "{pos} of {}{}",
                            s.hits.len(),
                            if s.truncated { "+" } else { "" }
                        ));
                    }
                }
            });
        });

        if self.find.replace {
            ui.horizontal(|ui| {
                ui.label("With");
                ui.add(
                    TextEdit::singleline(&mut self.find.replacement)
                        .hint_text("replacement (same format as the search)")
                        .font(TextStyle::Monospace)
                        .desired_width(260.0),
                );
                if ui.button("Replace").clicked() {
                    action = Some(HexApp::replace_current);
                }
                if ui.button("Replace all").clicked() {
                    action = Some(HexApp::replace_all);
                }
            });
        }

        if let Some(err) = &self.find.error {
            ui.label(RichText::new(err).color(ui.visuals().error_fg_color));
        }
        ui.add_space(2.0);

        if close {
            self.close_find_bar();
            return;
        }
        if let Some(f) = action {
            f(self);
        }
        if self.find.open {
            self.live_search();
        }
    }
}

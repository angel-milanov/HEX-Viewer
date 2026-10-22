//! Application shell: tabs, menus, toolbar, status bar and command dispatch.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, CentralPanel, Color32, FontId, Id, LayerId, Layout, Order, Panel, RichText,
    ScrollArea, Ui, ViewportCommand,
};

use crate::analysis::{
    HashResult, Job, JobPoll, MinimapData, Stats, StringsResult, compute_minimap, detect_file_type,
    next_difference, prev_difference,
};
use crate::commands::{Command, PasteMode, editor_shortcuts, global_shortcuts, shortcut_for};
use crate::dialogs::Dialogs;
use crate::document::{BOOKMARK_COLORS, Bookmark, Document, file_name_of};
use crate::editor::{self, HexViewArgs, Pane, ViewState, hex_view};
use crate::encoding::TextEncoding;
use crate::export::{CopyFormat, base64_decode, parse_hex, to_hex};
use crate::find::{FindState, SearchResults};
use crate::inspector::InspectorState;
use crate::settings::{MinimapMode, OffsetBase, Settings, SidePanel, ThemeChoice};

pub const APP_NAME: &str = "Hex Editor";
const MAX_CLIPBOARD_BYTES: usize = 64 << 20;
const SYNC_MINIMAP_LIMIT: usize = 2 << 20;

pub struct Tab {
    pub id: u64,
    pub doc: Document,
    pub view: ViewState,
    pub search: SearchResults,
    pub inspector: InspectorState,
    pub minimap: Option<MinimapData>,
    pub minimap_job: Option<Job<Option<MinimapData>>>,
    minimap_dirty_since: Option<Instant>,
    pub hashes: Option<HashResult>,
    pub hash_job: Option<Job<Option<HashResult>>>,
    pub stats: Option<Stats>,
    pub stats_job: Option<Job<Option<Stats>>>,
    pub strings: Option<StringsResult>,
    pub strings_job: Option<Job<Option<StringsResult>>>,
    pub strings_filter: String,
    pub compare_with: Option<u64>,
    pub diff_count: Option<(u64, u64, usize)>,
    disk_change_prompted: bool,
}

impl Tab {
    fn new(id: u64, doc: Document) -> Self {
        Self {
            id,
            doc,
            view: ViewState::default(),
            search: SearchResults::default(),
            inspector: InspectorState::default(),
            minimap: None,
            minimap_job: None,
            minimap_dirty_since: None,
            hashes: None,
            hash_job: None,
            stats: None,
            stats_job: None,
            strings: None,
            strings_job: None,
            strings_filter: String::new(),
            compare_with: None,
            diff_count: None,
            disk_change_prompted: false,
        }
    }

    pub fn jobs_running(&self) -> bool {
        self.minimap_job.is_some()
            || self.hash_job.is_some()
            || self.stats_job.is_some()
            || self.strings_job.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    CloseTab(u64),
    Reload(u64),
    DiskChanged(u64),
    Exit,
}

struct Status {
    text: String,
    error: bool,
    at: Instant,
}

pub struct HexApp {
    pub(crate) settings: Settings,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    next_tab_id: u64,
    untitled_counter: usize,
    pub(crate) insert_mode: bool,
    pub(crate) find: FindState,
    pub(crate) dialogs: Dialogs,
    pub(crate) pending: Vec<Command>,
    pub(crate) confirm: Option<Confirm>,
    pub(crate) error: Option<String>,
    status: Option<Status>,
    allow_close: bool,
    title: String,
    last_disk_check: Instant,
    pub(crate) analysis_use_selection: bool,
    pub(crate) highlight_diffs: bool,
    pub(crate) histogram_log: bool,
    applied_theme: Option<ThemeChoice>,
}

impl HexApp {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        settings.sanitize();
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let mut app = Self {
            settings,
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 1,
            untitled_counter: 0,
            insert_mode: false,
            find: FindState::default(),
            dialogs: Dialogs::default(),
            pending: Vec::new(),
            confirm: None,
            error: None,
            status: None,
            allow_close: false,
            title: String::new(),
            last_disk_check: Instant::now(),
            analysis_use_selection: false,
            highlight_diffs: true,
            histogram_log: false,
            applied_theme: None,
        };
        for f in files {
            app.open_path(&f);
        }
        app
    }

    // ---------------------------------------------------------------------
    // Tab helpers

    pub(crate) fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub(crate) fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn add_tab(&mut self, doc: Document) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(id, doc));
        self.active = self.tabs.len() - 1;
    }

    pub(crate) fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub(crate) fn set_status_error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    pub fn open_path(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.tabs.iter().position(|t| {
            t.doc.path.as_ref().is_some_and(|p| {
                std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical
            })
        }) {
            self.active = i;
            return;
        }
        match Document::open(path) {
            Ok(doc) => {
                let size = doc.len();
                let ro = doc.read_only;
                self.add_tab(doc);
                self.settings.push_recent(canonical);
                self.set_status(format!(
                    "Opened {} ({}){}",
                    file_name_of(path),
                    human_size(size),
                    if ro { " — read-only" } else { "" }
                ));
            }
            Err(e) => {
                self.settings.recent_files.retain(|p| p != &canonical);
                self.error = Some(format!("Could not open {}:\n{e}", path.display()));
            }
        }
    }

    fn new_file(&mut self) {
        self.untitled_counter += 1;
        self.add_tab(Document::new_empty(format!(
            "Untitled-{}",
            self.untitled_counter
        )));
    }

    /// Saves a tab, asking for a path if it has none. Returns success.
    pub(crate) fn save_tab(&mut self, idx: usize) -> bool {
        if self.tabs[idx].doc.path.is_none() {
            return self.save_tab_as(idx);
        }
        match self.tabs[idx].doc.save() {
            Ok(()) => {
                let msg = format!("Saved {}", self.tabs[idx].doc.display_path());
                self.set_status(msg);
                true
            }
            Err(e) => {
                self.error = Some(format!(
                    "Could not save {}:\n{e}",
                    self.tabs[idx].doc.display_path()
                ));
                false
            }
        }
    }

    fn save_tab_as(&mut self, idx: usize) -> bool {
        let doc = &self.tabs[idx].doc;
        let mut dialog = rfd::FileDialog::new().set_file_name(&doc.title);
        if let Some(dir) = doc.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return false;
        };
        match self.tabs[idx].doc.save_as(&path) {
            Ok(()) => {
                self.settings.push_recent(path.clone());
                self.set_status(format!("Saved {}", path.display()));
                true
            }
            Err(e) => {
                self.error = Some(format!("Could not save {}:\n{e}", path.display()));
                false
            }
        }
    }

    fn close_tab_now(&mut self, idx: usize) {
        let id = self.tabs[idx].id;
        self.tabs.remove(idx);
        for t in &mut self.tabs {
            if t.compare_with == Some(id) {
                t.compare_with = None;
            }
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if self.active > idx {
            self.active -= 1;
        }
    }

    pub(crate) fn request_close_tab(&mut self, idx: usize) {
        if self.tabs[idx].doc.is_modified() {
            self.active = idx;
            self.confirm = Some(Confirm::CloseTab(self.tabs[idx].id));
        } else {
            self.close_tab_now(idx);
        }
    }

    pub(crate) fn compare_data(&self) -> Option<Arc<Vec<u8>>> {
        let other = self.tab()?.compare_with?;
        self.tabs
            .iter()
            .find(|t| t.id == other)
            .map(|t| t.doc.snapshot())
    }

    // ---------------------------------------------------------------------
    // Commands

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (sc, cmd) in global_shortcuts() {
                if i.consume_shortcut(&sc) {
                    fired.push(cmd);
                }
            }
        });
        let text_focus = ctx.text_edit_focused();
        if !text_focus {
            ctx.input_mut(|i| {
                for (sc, cmd) in editor_shortcuts() {
                    if i.consume_shortcut(&sc) {
                        fired.push(cmd);
                    }
                }
            });
        }
        self.pending.extend(fired);
    }

    pub(crate) fn run(&mut self, ctx: &egui::Context, cmd: Command) {
        match cmd {
            Command::NewFile => self.new_file(),
            Command::OpenFile => {
                let mut dialog = rfd::FileDialog::new();
                if let Some(dir) = self
                    .tab()
                    .and_then(|t| t.doc.path.as_ref())
                    .and_then(|p| p.parent())
                {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(paths) = dialog.pick_files() {
                    for p in paths {
                        self.open_path(&p);
                    }
                }
            }
            Command::OpenPath(p) => self.open_path(&p),
            Command::Save => {
                if !self.tabs.is_empty() {
                    self.save_tab(self.active);
                }
            }
            Command::SaveAs => {
                if !self.tabs.is_empty() {
                    self.save_tab_as(self.active);
                }
            }
            Command::SaveAll => {
                for i in 0..self.tabs.len() {
                    if self.tabs[i].doc.is_modified() && !self.save_tab(i) {
                        break;
                    }
                }
            }
            Command::SaveSelectionAs => self.save_selection_as(),
            Command::InsertFile => self.insert_file(),
            Command::Reload => {
                if let Some(tab) = self.tab() {
                    if tab.doc.path.is_none() {
                        return;
                    }
                    if tab.doc.is_modified() {
                        self.confirm = Some(Confirm::Reload(tab.id));
                    } else {
                        self.reload_tab(self.active);
                    }
                }
            }
            Command::CloseTab => {
                if !self.tabs.is_empty() {
                    self.request_close_tab(self.active);
                }
            }
            Command::Exit => ctx.send_viewport_cmd(ViewportCommand::Close),
            Command::Undo | Command::Redo => {
                let Some(tab) = self.tab_mut() else { return };
                let range = if cmd == Command::Undo {
                    tab.doc.undo()
                } else {
                    tab.doc.redo()
                };
                match range {
                    Some((off, n)) => {
                        let len = tab.doc.len();
                        tab.view.select(off, off + n, len);
                    }
                    None if tab.doc.read_only => self.read_only_notice(),
                    None => {}
                }
            }
            Command::Copy => {
                self.copy(ctx, None);
            }
            Command::CopyAs(fmt) => {
                self.copy(ctx, Some(fmt));
            }
            Command::CopyOffset => {
                if let Some(tab) = self.tab() {
                    let text = format!("0x{:X}", tab.view.cursor);
                    ctx.copy_text(text.clone());
                    self.set_status(format!("Copied offset {text}"));
                }
            }
            Command::Cut => {
                let Some(tab) = self.tab() else { return };
                if tab.doc.read_only {
                    return self.read_only_notice();
                }
                if tab.view.selection(tab.doc.len()).is_none() {
                    return;
                }
                if self.copy(ctx, None)
                    && let Some(tab) = self.tab_mut()
                {
                    editor::delete_forward(&mut tab.doc, &mut tab.view);
                }
            }
            Command::Paste(mode) => {
                match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                    Ok(text) => self.paste_text(text, mode),
                    Err(e) => self.set_status_error(format!("Clipboard unavailable: {e}")),
                }
            }
            Command::PasteText(text) => self.paste_text(text, PasteMode::Auto),
            Command::Delete => {
                let Some(tab) = self.tab_mut() else { return };
                if tab.doc.read_only {
                    return self.read_only_notice();
                }
                editor::delete_forward(&mut tab.doc, &mut tab.view);
            }
            Command::SelectAll => {
                if let Some(tab) = self.tab_mut()
                    && !tab.doc.is_empty()
                {
                    let len = tab.doc.len();
                    tab.view.select(0, len, len);
                    tab.view.cursor = len.saturating_sub(1);
                    tab.view.scroll_to_cursor = false;
                }
            }
            Command::SelectNone => {
                if let Some(tab) = self.tab_mut() {
                    tab.view.anchor = None;
                }
            }
            Command::ToggleInsertMode => self.insert_mode = !self.insert_mode,
            Command::ToggleReadOnly => {
                if let Some(tab) = self.tab_mut() {
                    tab.doc.read_only = !tab.doc.read_only;
                    let ro = tab.doc.read_only;
                    self.set_status(if ro {
                        "Read-only mode on"
                    } else {
                        "Read-only mode off"
                    });
                }
            }
            Command::Find | Command::Replace => {
                self.find.open = true;
                self.find.replace = cmd == Command::Replace;
                self.find.focus = true;
                if let Some(tab) = self.tab()
                    && let Some((s, e)) = tab.view.selection(tab.doc.len())
                    && e - s <= 64
                    && self.settings.search.kind == crate::search::SearchKind::Hex
                    && (self.find.query.is_empty() || e - s > 1)
                {
                    self.find.query = to_hex_spaced(&tab.doc.bytes()[s..e]);
                }
            }
            Command::FindNext | Command::FindPrev => {
                if self.find.query.is_empty() {
                    self.find.open = true;
                    self.find.focus = true;
                } else {
                    self.find_step(cmd == Command::FindNext);
                }
            }
            Command::FindAll => {
                if self.find.query.is_empty() {
                    self.find.open = true;
                    self.find.focus = true;
                } else {
                    self.find_all();
                }
            }
            Command::SearchSelection => {
                let Some(tab) = self.tab() else { return };
                let Some((s, e)) = tab.view.selection(tab.doc.len()) else {
                    return;
                };
                let e = e.min(s + 256);
                self.find.query = to_hex_spaced(&tab.doc.bytes()[s..e]);
                self.settings.search.kind = crate::search::SearchKind::Hex;
                self.find.open = true;
                self.find_all();
            }
            Command::GoTo => self.dialogs.open_goto(),
            Command::SelectRange => {
                let tab = self.tab();
                let sel = tab.and_then(|t| t.view.selection(t.doc.len()));
                self.dialogs.open_select_range(sel);
            }
            Command::Fill => {
                let Some(tab) = self.tab() else { return };
                if tab.doc.read_only {
                    return self.read_only_notice();
                }
                if tab.view.selection(tab.doc.len()).is_none() {
                    self.set_status_error("Select the bytes to fill first");
                } else {
                    self.dialogs.open_fill();
                }
            }
            Command::InsertBytes => {
                if self.tab().is_some_and(|t| t.doc.read_only) {
                    return self.read_only_notice();
                }
                if self.tab().is_some() {
                    self.dialogs.open_insert();
                }
            }
            Command::Transform => {
                if self.tab().is_some_and(|t| t.doc.read_only) {
                    return self.read_only_notice();
                }
                if self.tab().is_some() {
                    self.dialogs.open_transform();
                }
            }
            Command::ToggleBookmark => self.toggle_bookmark(),
            Command::NextBookmark | Command::PrevBookmark => {
                self.jump_bookmark(cmd == Command::NextBookmark)
            }
            Command::NextTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                }
            }
            Command::PrevTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
                }
            }
            Command::ShowPanel(p) => {
                self.settings.show_side_panel = true;
                self.settings.side_panel = p;
            }
            Command::NextDifference | Command::PrevDifference => {
                self.jump_difference(cmd == Command::NextDifference)
            }
            Command::FontBigger => {
                self.settings.font_size = (self.settings.font_size + 1.0).min(40.0)
            }
            Command::FontSmaller => {
                self.settings.font_size = (self.settings.font_size - 1.0).max(8.0)
            }
            Command::FontReset => self.settings.font_size = Settings::default().font_size,
            Command::ReadOnlyNotice => self.read_only_notice(),
            Command::ShowShortcuts => self.dialogs.shortcuts = true,
            Command::About => self.dialogs.about = true,
        }
    }

    fn read_only_notice(&mut self) {
        self.set_status_error("Document is read-only (Ctrl+R to toggle)");
    }

    fn reload_tab(&mut self, idx: usize) {
        let tab = &mut self.tabs[idx];
        match tab.doc.reload() {
            Ok(()) => {
                let len = tab.doc.len();
                tab.view.clamp(len);
                tab.view.anchor = None;
                tab.disk_change_prompted = false;
                let name = tab.doc.title.clone();
                self.set_status(format!("Reloaded {name}"));
            }
            Err(e) => self.error = Some(format!("Could not reload:\n{e}")),
        }
    }

    /// Copies the selection; returns whether anything was copied.
    fn copy(&mut self, ctx: &egui::Context, fmt: Option<CopyFormat>) -> bool {
        let Some(tab) = self.tab() else { return false };
        let len = tab.doc.len();
        let Some((s, e)) = tab.view.selection_or_cursor(len) else {
            return false;
        };
        if e - s > MAX_CLIPBOARD_BYTES {
            self.set_status_error(format!(
                "Selection too large for the clipboard ({}); use Save selection as…",
                human_size(e - s)
            ));
            return false;
        }
        let fmt = fmt.unwrap_or(match tab.view.pane {
            Pane::Hex => CopyFormat::HexSpaced,
            Pane::Text => CopyFormat::Text,
        });
        let text = fmt.format(
            &tab.doc.bytes()[s..e],
            s,
            self.settings.uppercase_hex,
            self.settings.text_encoding,
        );
        ctx.copy_text(text);
        self.set_status(format!(
            "Copied {} as {}",
            plural(e - s, "byte"),
            fmt.name()
        ));
        true
    }

    fn paste_text(&mut self, text: String, mode: PasteMode) {
        let insert_mode = self.insert_mode;
        let encoding = self.settings.text_encoding;
        let Some(tab) = self.tab_mut() else { return };
        if tab.doc.read_only {
            return self.read_only_notice();
        }
        let bytes = match mode {
            PasteMode::Auto => match tab.view.pane {
                Pane::Hex => parse_hex(&text).unwrap_or_else(|_| text.into_bytes()),
                Pane::Text if encoding == TextEncoding::Ascii => text.into_bytes(),
                Pane::Text => encoding.encode_str(&text),
            },
            PasteMode::Hex => match parse_hex(&text) {
                Ok(b) => b,
                Err(e) => return self.set_status_error(format!("Clipboard is not valid hex: {e}")),
            },
            PasteMode::Text => text.into_bytes(),
            PasteMode::Base64 => match base64_decode(&text) {
                Ok(b) => b,
                Err(e) => {
                    return self.set_status_error(format!("Clipboard is not valid Base64: {e}"));
                }
            },
        };
        let n = bytes.len();
        if editor::write_bytes(&mut tab.doc, &mut tab.view, bytes, insert_mode) {
            self.set_status(format!("Pasted {}", plural(n, "byte")));
        }
    }

    fn save_selection_as(&mut self) {
        let Some(tab) = self.tab() else { return };
        let Some((s, e)) = tab.view.selection_or_cursor(tab.doc.len()) else {
            return;
        };
        let name = format!("{}_{:X}-{:X}.bin", tab.doc.title, s, e - 1);
        let Some(path) = rfd::FileDialog::new().set_file_name(&name).save_file() else {
            return;
        };
        let result = std::fs::write(&path, &self.tabs[self.active].doc.bytes()[s..e]);
        match result {
            Ok(()) => self.set_status(format!(
                "Saved {} to {}",
                plural(e - s, "byte"),
                path.display()
            )),
            Err(e) => self.error = Some(format!("Could not write {}:\n{e}", path.display())),
        }
    }

    fn insert_file(&mut self) {
        if self.tab().is_none_or(|t| t.doc.read_only) {
            return self.read_only_notice();
        }
        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let n = bytes.len();
                let tab = self.tab_mut().expect("checked above");
                let at = tab.view.cursor;
                if tab.doc.insert(at, bytes, false) {
                    let len = tab.doc.len();
                    tab.view.select(at, at + n, len);
                }
                self.set_status(format!(
                    "Inserted {} from {}",
                    plural(n, "byte"),
                    file_name_of(&path)
                ));
            }
            Err(e) => self.error = Some(format!("Could not read {}:\n{e}", path.display())),
        }
    }

    fn toggle_bookmark(&mut self) {
        let Some(tab) = self.tab_mut() else { return };
        let len = tab.doc.len();
        if let Some(i) = tab.doc.bookmark_at(tab.view.cursor) {
            tab.doc.bookmarks.remove(i);
            return self.set_status("Bookmark removed");
        }
        let Some((s, e)) = tab.view.selection_or_cursor(len) else {
            return;
        };
        let n = tab.doc.bookmarks.len();
        tab.doc.bookmarks.push(Bookmark {
            offset: s,
            len: e - s,
            name: format!("Bookmark {}", n + 1),
            color: BOOKMARK_COLORS[n % BOOKMARK_COLORS.len()],
        });
        tab.doc.bookmarks.sort_by_key(|b| b.offset);
        self.set_status(format!("Bookmark added at 0x{s:X}"));
    }

    fn jump_bookmark(&mut self, forward: bool) {
        let Some(tab) = self.tab_mut() else { return };
        if tab.doc.bookmarks.is_empty() {
            return self.set_status("No bookmarks");
        }
        let cur = tab.view.cursor;
        let mut offsets: Vec<(usize, usize)> = tab
            .doc
            .bookmarks
            .iter()
            .map(|b| (b.offset, b.len))
            .collect();
        offsets.sort_unstable();
        let target = if forward {
            offsets.iter().find(|(o, _)| *o > cur).or(offsets.first())
        } else {
            offsets
                .iter()
                .rev()
                .find(|(o, _)| *o < cur)
                .or(offsets.last())
        };
        if let Some(&(o, n)) = target {
            let len = tab.doc.len();
            tab.view.select(o, o + n, len);
            tab.view.cursor = o;
            tab.view.anchor = Some((o + n).saturating_sub(1).max(o));
            tab.view.center_cursor = true;
        }
    }

    fn jump_difference(&mut self, forward: bool) {
        let Some(other) = self.compare_data() else {
            self.settings.show_side_panel = true;
            self.settings.side_panel = SidePanel::Compare;
            return self.set_status("Choose a file to compare with first");
        };
        let tab = self.tab_mut().expect("compare implies a tab");
        let data = tab.doc.bytes();
        let cur = tab.view.cursor;
        let found = if forward {
            next_difference(data, &other, cur + 1)
        } else {
            prev_difference(data, &other, cur)
        };
        match found {
            Some(p) => {
                let len = tab.doc.len();
                tab.view.select(p, p, len);
                tab.view.center_cursor = true;
            }
            None => self.set_status("No more differences"),
        }
    }

    // ---------------------------------------------------------------------
    // Background work

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        fn poll<T: Send + 'static>(job: &mut Option<Job<Option<T>>>, out: &mut Option<T>) {
            if let Some(j) = job {
                match j.poll() {
                    JobPoll::Pending => {}
                    JobPoll::Done(v) => {
                        if v.is_some() {
                            *out = v;
                        }
                        *job = None;
                    }
                    JobPoll::Failed => *job = None,
                }
            }
        }
        let mut busy = false;
        for tab in &mut self.tabs {
            poll(&mut tab.minimap_job, &mut tab.minimap);
            poll(&mut tab.hash_job, &mut tab.hashes);
            poll(&mut tab.stats_job, &mut tab.stats);
            poll(&mut tab.strings_job, &mut tab.strings);
            busy |= tab.jobs_running();
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Keep the minimap of the active tab current.
        if !self.settings.show_minimap {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let current = tab
            .minimap
            .as_ref()
            .is_some_and(|m| m.version == tab.doc.version);
        if current || tab.minimap_job.is_some() {
            return;
        }
        let version = tab.doc.version;
        if tab.doc.len() <= SYNC_MINIMAP_LIMIT {
            tab.minimap = compute_minimap(tab.doc.bytes(), version, &AtomicBool::new(false));
            return;
        }
        let since = *tab.minimap_dirty_since.get_or_insert_with(Instant::now);
        if tab.minimap.is_some() && since.elapsed() < Duration::from_millis(400) {
            ctx.request_repaint_after(Duration::from_millis(400));
            return;
        }
        tab.minimap_dirty_since = None;
        let data = tab.doc.snapshot();
        tab.minimap_job = Some(Job::spawn(ctx, move |cancel| {
            compute_minimap(&data, version, cancel)
        }));
    }

    fn check_disk_changes(&mut self, ctx: &egui::Context) {
        if self.last_disk_check.elapsed() < Duration::from_secs(2) || self.confirm.is_some() {
            return;
        }
        self.last_disk_check = Instant::now();
        if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active)
            && !tab.disk_change_prompted
            && tab.doc.changed_on_disk()
        {
            tab.disk_change_prompted = true;
            self.confirm = Some(Confirm::DiskChanged(tab.id));
        }
    }

    // ---------------------------------------------------------------------
    // Window integration

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.tabs.iter().any(|t| t.doc.is_modified())
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Exit);
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let (dropped, hovering) = ctx.input(|i| {
            (
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect::<Vec<_>>(),
                !i.raw.hovered_files.is_empty(),
            )
        });
        for p in dropped {
            self.open_path(&p);
        }
        if hovering {
            let painter =
                ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("drop_overlay")));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 160));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Drop files to open",
                FontId::proportional(28.0),
                Color32::WHITE,
            );
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.tab() {
            Some(t) => format!(
                "{}{}{} — {APP_NAME}",
                t.doc.title,
                if t.doc.is_modified() { " •" } else { "" },
                if t.doc.read_only { " [read-only]" } else { "" }
            ),
            None => APP_NAME.to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme != Some(self.settings.theme) {
            ctx.set_theme(self.settings.theme.to_egui());
            self.applied_theme = Some(self.settings.theme);
        }
    }

    // ---------------------------------------------------------------------
    // UI

    pub(crate) fn menu_item(&mut self, ui: &mut Ui, label: &str, cmd: Command, enabled: bool) {
        let mut button = egui::Button::new(label);
        if let Some(sc) =
            shortcut_for(ui.ctx(), &cmd).or_else(|| builtin_shortcut(&cmd).map(String::from))
        {
            button = button.shortcut_text(sc);
        }
        if ui.add_enabled(enabled, button).clicked() {
            self.pending.push(cmd);
            ui.close();
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        let has_tab = self.tab().is_some();
        let (has_sel, writable, has_path, can_undo, can_redo) =
            self.tab().map_or((false, false, false, false, false), |t| {
                (
                    t.view.selection(t.doc.len()).is_some(),
                    !t.doc.read_only,
                    t.doc.path.is_some(),
                    t.doc.can_undo() && !t.doc.read_only,
                    t.doc.can_redo() && !t.doc.read_only,
                )
            });

        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                self.menu_item(ui, "New", Command::NewFile, true);
                self.menu_item(ui, "Open…", Command::OpenFile, true);
                ui.menu_button("Open recent", |ui| {
                    if self.settings.recent_files.is_empty() {
                        ui.weak("No recent files");
                    }
                    for p in self.settings.recent_files.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            self.pending.push(Command::OpenPath(p));
                            ui.close();
                        }
                    }
                    if !self.settings.recent_files.is_empty() {
                        ui.separator();
                        if ui.button("Clear recent files").clicked() {
                            self.settings.recent_files.clear();
                            ui.close();
                        }
                    }
                });
                ui.separator();
                self.menu_item(ui, "Save", Command::Save, has_tab);
                self.menu_item(ui, "Save as…", Command::SaveAs, has_tab);
                self.menu_item(ui, "Save all", Command::SaveAll, has_tab);
                self.menu_item(ui, "Save selection as…", Command::SaveSelectionAs, has_tab);
                ui.separator();
                self.menu_item(
                    ui,
                    "Insert file at cursor…",
                    Command::InsertFile,
                    has_tab && writable,
                );
                self.menu_item(ui, "Reload from disk", Command::Reload, has_path);
                ui.separator();
                self.menu_item(ui, "Close tab", Command::CloseTab, has_tab);
                self.menu_item(ui, "Exit", Command::Exit, true);
            });

            ui.menu_button("Edit", |ui| {
                self.menu_item(ui, "Undo", Command::Undo, can_undo);
                self.menu_item(ui, "Redo", Command::Redo, can_redo);
                ui.separator();
                self.menu_item(ui, "Cut", Command::Cut, has_sel && writable);
                self.menu_item(ui, "Copy", Command::Copy, has_tab);
                ui.menu_button("Copy as", |ui| {
                    for fmt in CopyFormat::ALL {
                        self.menu_item(ui, fmt.name(), Command::CopyAs(fmt), has_tab);
                    }
                });
                self.menu_item(ui, "Copy offset", Command::CopyOffset, has_tab);
                self.menu_item(ui, "Paste", Command::Paste(PasteMode::Auto), writable);
                ui.menu_button("Paste special", |ui| {
                    self.menu_item(
                        ui,
                        "Paste hex string",
                        Command::Paste(PasteMode::Hex),
                        writable,
                    );
                    self.menu_item(
                        ui,
                        "Paste text (UTF-8)",
                        Command::Paste(PasteMode::Text),
                        writable,
                    );
                    self.menu_item(
                        ui,
                        "Paste Base64",
                        Command::Paste(PasteMode::Base64),
                        writable,
                    );
                });
                self.menu_item(ui, "Delete", Command::Delete, writable);
                ui.separator();
                self.menu_item(ui, "Select all", Command::SelectAll, has_tab);
                self.menu_item(ui, "Select none", Command::SelectNone, has_sel);
                self.menu_item(ui, "Select range…", Command::SelectRange, has_tab);
                ui.separator();
                self.menu_item(ui, "Fill selection…", Command::Fill, has_sel && writable);
                self.menu_item(ui, "Insert bytes…", Command::InsertBytes, writable);
                self.menu_item(ui, "Transform…", Command::Transform, writable);
                ui.separator();
                let mut ins = self.insert_mode;
                if ui.checkbox(&mut ins, "Insert mode").clicked() {
                    self.insert_mode = ins;
                }
                if let Some(tab) = self.tab_mut() {
                    ui.checkbox(&mut tab.doc.read_only, "Read-only");
                }
            });

            ui.menu_button("Search", |ui| {
                self.menu_item(ui, "Find…", Command::Find, has_tab);
                self.menu_item(ui, "Replace…", Command::Replace, has_tab && writable);
                self.menu_item(ui, "Find next", Command::FindNext, has_tab);
                self.menu_item(ui, "Find previous", Command::FindPrev, has_tab);
                self.menu_item(ui, "Find all", Command::FindAll, has_tab);
                self.menu_item(
                    ui,
                    "Search for selection",
                    Command::SearchSelection,
                    has_sel,
                );
                ui.separator();
                self.menu_item(ui, "Go to offset…", Command::GoTo, has_tab);
                ui.separator();
                self.menu_item(ui, "Toggle bookmark", Command::ToggleBookmark, has_tab);
                self.menu_item(ui, "Next bookmark", Command::NextBookmark, has_tab);
                self.menu_item(ui, "Previous bookmark", Command::PrevBookmark, has_tab);
            });

            ui.menu_button("View", |ui| self.view_menu(ui));

            ui.menu_button("Tools", |ui| {
                for p in SidePanel::ALL {
                    let label = match p {
                        SidePanel::Inspector => "Data inspector",
                        SidePanel::Bookmarks => "Bookmarks",
                        SidePanel::SearchResults => "Search results",
                        SidePanel::Strings => "Extract strings",
                        SidePanel::Checksums => "Checksums & hashes",
                        SidePanel::Statistics => "Statistics & entropy",
                        SidePanel::Compare => "Compare files",
                    };
                    self.menu_item(ui, label, Command::ShowPanel(p), true);
                }
                ui.separator();
                self.menu_item(ui, "Next difference", Command::NextDifference, has_tab);
                self.menu_item(ui, "Previous difference", Command::PrevDifference, has_tab);
            });

            ui.menu_button("Help", |ui| {
                self.menu_item(ui, "Keyboard shortcuts", Command::ShowShortcuts, true);
                self.menu_item(ui, "About", Command::About, true);
            });
        });
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        ui.menu_button("Theme", |ui| {
            ui.radio_value(&mut s.theme, ThemeChoice::System, "Follow system");
            ui.radio_value(&mut s.theme, ThemeChoice::Dark, "Dark");
            ui.radio_value(&mut s.theme, ThemeChoice::Light, "Light");
        });
        ui.menu_button("Bytes per row", |ui| {
            ui.radio_value(&mut s.bytes_per_row, 0, "Fit to window");
            for n in [4, 8, 12, 16, 20, 24, 32, 48, 64] {
                ui.radio_value(&mut s.bytes_per_row, n, n.to_string());
            }
        });
        ui.menu_button("Group bytes", |ui| {
            for n in [1, 2, 4, 8, 16] {
                ui.radio_value(&mut s.group_size, n, format!("{n}"));
            }
        });
        ui.menu_button("Offset format", |ui| {
            ui.radio_value(&mut s.offset_base, OffsetBase::Hex, "Hexadecimal");
            ui.radio_value(&mut s.offset_base, OffsetBase::Decimal, "Decimal");
            ui.radio_value(&mut s.offset_base, OffsetBase::Octal, "Octal");
        });
        ui.menu_button("Text encoding", |ui| {
            for enc in TextEncoding::ALL {
                ui.radio_value(&mut s.text_encoding, enc, enc.name());
            }
        });
        ui.menu_button("Minimap", |ui| {
            ui.checkbox(&mut s.show_minimap, "Show minimap");
            ui.separator();
            ui.radio_value(&mut s.minimap_mode, MinimapMode::ByteClass, "Byte classes");
            ui.radio_value(&mut s.minimap_mode, MinimapMode::Entropy, "Entropy");
        });
        ui.menu_button("UI scale", |ui| {
            let current = ui.ctx().zoom_factor();
            for z in [0.8f32, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0] {
                if ui
                    .radio((current - z).abs() < 0.01, format!("{:.0}%", z * 100.0))
                    .clicked()
                {
                    ui.ctx().set_zoom_factor(z);
                    ui.close();
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut s.uppercase_hex, "Uppercase hex");
        ui.checkbox(&mut s.color_bytes, "Color bytes by class");
        ui.checkbox(&mut s.highlight_modified, "Highlight modified bytes");
        ui.checkbox(
            &mut s.highlight_selection_matches,
            "Highlight selection matches",
        );
        ui.checkbox(&mut s.show_text_column, "Text column");
        ui.checkbox(&mut s.show_header, "Column header");
        ui.separator();
        ui.checkbox(&mut s.show_toolbar, "Toolbar");
        ui.checkbox(&mut s.show_status_bar, "Status bar");
        ui.checkbox(&mut s.show_side_panel, "Side panel");
        ui.separator();
        self.menu_item(ui, "Larger font", Command::FontBigger, true);
        self.menu_item(ui, "Smaller font", Command::FontSmaller, true);
        self.menu_item(ui, "Reset font size", Command::FontReset, true);
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let (writable, has_tab, can_undo, can_redo) =
            self.tab().map_or((false, false, false, false), |t| {
                (!t.doc.read_only, true, t.doc.can_undo(), t.doc.can_redo())
            });
        ui.horizontal(|ui| {
            let mut tool = |ui: &mut Ui, icon: &str, tip: &str, cmd: Command, enabled: bool| {
                let sc = shortcut_for(ui.ctx(), &cmd)
                    .or_else(|| builtin_shortcut(&cmd).map(String::from));
                let tip = match sc {
                    Some(sc) => format!("{tip} ({sc})"),
                    None => tip.to_string(),
                };
                let b = egui::Button::new(RichText::new(icon).size(15.0)).frame(false);
                if ui.add_enabled(enabled, b).on_hover_text(tip).clicked() {
                    self.pending.push(cmd);
                }
            };
            tool(ui, "🗋", "New file", Command::NewFile, true);
            tool(ui, "🗁", "Open file", Command::OpenFile, true);
            tool(ui, "💾", "Save", Command::Save, has_tab);
            ui.separator();
            tool(ui, "⟲", "Undo", Command::Undo, can_undo && writable);
            tool(ui, "⟳", "Redo", Command::Redo, can_redo && writable);
            ui.separator();
            tool(ui, "✂", "Cut", Command::Cut, writable);
            tool(ui, "🗐", "Copy", Command::Copy, has_tab);
            tool(ui, "📋", "Paste", Command::Paste(PasteMode::Auto), writable);
            ui.separator();
            tool(ui, "🔍", "Find", Command::Find, has_tab);
            tool(ui, "⮩", "Go to offset", Command::GoTo, has_tab);
            tool(
                ui,
                "🔖",
                "Toggle bookmark",
                Command::ToggleBookmark,
                has_tab,
            );
            ui.separator();
            tool(
                ui,
                "ℹ",
                "Data inspector",
                Command::ShowPanel(SidePanel::Inspector),
                true,
            );
            tool(
                ui,
                "#",
                "Checksums",
                Command::ShowPanel(SidePanel::Checksums),
                true,
            );
            tool(
                ui,
                "📊",
                "Statistics",
                Command::ShowPanel(SidePanel::Statistics),
                true,
            );
            tool(
                ui,
                "🔤",
                "Strings",
                Command::ShowPanel(SidePanel::Strings),
                true,
            );
            tool(
                ui,
                "⚖",
                "Compare",
                Command::ShowPanel(SidePanel::Compare),
                true,
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.insert_mode {
                    "INSERT"
                } else {
                    "OVERWRITE"
                };
                if ui
                    .selectable_label(self.insert_mode, label)
                    .on_hover_text("Toggle insert / overwrite mode (Insert)")
                    .clicked()
                {
                    self.insert_mode = !self.insert_mode;
                }
            });
        });
    }

    fn tab_bar(&mut self, ui: &mut Ui) {
        let mut switch = None;
        let mut close = None;
        ScrollArea::horizontal()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let mut text = RichText::new(format!(
                            "{}{}",
                            if tab.doc.read_only { "🔒 " } else { "" },
                            tab.doc.title
                        ));
                        if tab.doc.is_modified() {
                            text = text.italics();
                        }
                        let resp = ui
                            .selectable_label(i == self.active, text)
                            .on_hover_text(tab.doc.display_path());
                        if resp.clicked() {
                            switch = Some(i);
                        }
                        if resp.middle_clicked() {
                            close = Some(i);
                        }
                        let close_label = if tab.doc.is_modified() { "⏺" } else { "×" };
                        if ui
                            .add(egui::Button::new(close_label).frame(false).small())
                            .on_hover_text("Close")
                            .clicked()
                        {
                            close = Some(i);
                        }
                        ui.add_space(6.0);
                    }
                });
            });
        if let Some(i) = switch {
            self.active = i;
        }
        if let Some(i) = close {
            self.request_close_tab(i);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        // Right-aligned controls are laid out first so the left-hand
        // information truncates instead of pushing them off-screen.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                let ro = tab.doc.read_only;
                if ui
                    .selectable_label(ro, if ro { "RO" } else { "RW" })
                    .on_hover_text("Toggle read-only (Ctrl+R)")
                    .clicked()
                {
                    tab.doc.read_only = !ro;
                }
            }
            if ui
                .selectable_label(
                    self.insert_mode,
                    if self.insert_mode { "INS" } else { "OVR" },
                )
                .on_hover_text("Toggle insert / overwrite mode (Insert)")
                .clicked()
            {
                self.insert_mode = !self.insert_mode;
            }
            ui.separator();
            egui::ComboBox::from_id_salt("status_encoding")
                .selected_text(self.settings.text_encoding.name())
                .show_ui(ui, |ui| {
                    for enc in TextEncoding::ALL {
                        ui.selectable_value(&mut self.settings.text_encoding, enc, enc.name());
                    }
                });
            if let Some(tab) = self.tab() {
                ui.separator();
                ui.weak(format!("{} per row", tab.view.bytes_per_row));
                if tab.jobs_running() {
                    ui.spinner();
                }
            }

            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if let Some(tab) = self.tab() {
                    let len = tab.doc.len();
                    let cur = tab.view.cursor;
                    ui.label(format!("Offset: 0x{cur:X} ({cur})"));
                    ui.separator();
                    if let Some(b) = tab.doc.get(cur) {
                        let ch = if (0x20..0x7f).contains(&b) {
                            format!(" '{}'", b as char)
                        } else {
                            String::new()
                        };
                        ui.label(format!("Value: 0x{b:02X} ({b}){ch}"));
                        ui.separator();
                    }
                    if let Some((s, e)) = tab.view.selection(len) {
                        ui.label(format!(
                            "Selection: 0x{s:X}–0x{:X} ({})",
                            e - 1,
                            plural(e - s, "byte")
                        ));
                        ui.separator();
                    }
                    ui.label(format!("Size: {} ({})", human_size(len), group_digits(len)))
                        .on_hover_text(tab.doc.display_path());
                    if let Some(kind) = detect_file_type(&tab.doc.bytes()[..len.min(4096)]) {
                        ui.separator();
                        ui.label(kind);
                    }
                } else {
                    ui.label("No file open");
                }

                let status = self
                    .status
                    .as_ref()
                    .filter(|s| s.at.elapsed() < Duration::from_secs(6));
                if let Some(s) = status {
                    ui.separator();
                    let color = if s.error {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().strong_text_color()
                    };
                    ui.add(egui::Label::new(RichText::new(&s.text).color(color)).truncate())
                        .on_hover_text(&s.text);
                    ui.ctx().request_repaint_after(
                        Duration::from_secs(6).saturating_sub(s.at.elapsed()),
                    );
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.2);
            ui.heading(RichText::new(APP_NAME).size(32.0).strong());
            ui.weak("Open a file, drop one onto the window, or start a new one.");
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let w = 260.0;
                ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                if ui.button(RichText::new("🗋  New file").size(16.0)).clicked() {
                    self.pending.push(Command::NewFile);
                }
                if ui
                    .button(RichText::new("🗁  Open file…").size(16.0))
                    .clicked()
                {
                    self.pending.push(Command::OpenFile);
                }
            });
            if !self.settings.recent_files.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("Recent files").strong());
                for p in self.settings.recent_files.clone() {
                    if ui.link(p.display().to_string()).clicked() {
                        self.pending.push(Command::OpenPath(p));
                    }
                }
            }
        });
    }

    fn central(&mut self, ui: &mut Ui) {
        if self.tabs.is_empty() {
            self.welcome(ui);
            return;
        }
        let compare = if self.highlight_diffs {
            self.compare_data()
        } else {
            None
        };
        let tab = &mut self.tabs[self.active];
        let current_hit = tab
            .search
            .current
            .and_then(|i| tab.search.hits.get(i).copied());
        let args = HexViewArgs {
            settings: &mut self.settings,
            insert_mode: self.insert_mode,
            search_hits: &tab.search.hits,
            search_len: tab.search.len,
            current_hit,
            compare: compare.as_deref().map(Vec::as_slice),
            minimap: tab.minimap.as_ref(),
        };
        let cmds = hex_view(ui, &mut tab.doc, &mut tab.view, args);
        self.pending.extend(cmds);
    }
}

impl eframe::App for HexApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.handle_close_request(&ctx);
        self.handle_dropped_files(&ctx);
        self.poll_jobs(&ctx);
        self.check_disk_changes(&ctx);
        self.handle_shortcuts(&ctx);

        Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        if self.settings.show_toolbar {
            Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        }
        if !self.tabs.is_empty() {
            Panel::top("tab_bar").show(ui, |ui| self.tab_bar(ui));
        }
        if self.settings.show_status_bar {
            Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        }
        if self.find.open && !self.tabs.is_empty() {
            Panel::bottom("find_bar").show(ui, |ui| self.find_bar(ui));
        }
        if self.settings.show_side_panel {
            Panel::right("side_panel")
                .resizable(true)
                .default_size(330.0)
                .size_range(240.0..=700.0)
                .show(ui, |ui| self.side_panel(ui));
        }
        CentralPanel::no_frame().show(ui, |ui| self.central(ui));

        self.dialogs_ui(&ctx);
        self.modals_ui(&ctx);

        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            for cmd in pending {
                self.run(&ctx, cmd);
            }
            ctx.request_repaint();
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let len = tab.doc.len();
            tab.view.clamp(len);
        }
        self.update_title(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

// ---------------------------------------------------------------------------
// Confirmation & error modals

impl HexApp {
    fn modals_ui(&mut self, ctx: &egui::Context) {
        if let Some(msg) = self.error.clone() {
            let resp = egui::Modal::new(Id::unique("error_modal")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading("Error");
                ui.add_space(6.0);
                ui.label(msg);
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.button("OK").clicked()
                })
                .inner
            });
            if resp.inner || resp.should_close() {
                self.error = None;
            }
            return;
        }

        let Some(confirm) = self.confirm else { return };
        let (title, body, buttons): (&str, String, &[&str]) = match confirm {
            Confirm::CloseTab(id) => (
                "Unsaved changes",
                format!(
                    "Save changes to “{}” before closing?",
                    self.tab_index(id)
                        .map_or(String::new(), |i| self.tabs[i].doc.title.clone())
                ),
                &["Save", "Don't save", "Cancel"],
            ),
            Confirm::Reload(id) => (
                "Reload file",
                format!(
                    "Discard unsaved changes to “{}” and reload it from disk?",
                    self.tab_index(id)
                        .map_or(String::new(), |i| self.tabs[i].doc.title.clone())
                ),
                &["Reload", "Cancel"],
            ),
            Confirm::DiskChanged(id) => (
                "File changed on disk",
                format!(
                    "“{}” was modified by another program.{}\nReload it?",
                    self.tab_index(id)
                        .map_or(String::new(), |i| self.tabs[i].doc.title.clone()),
                    if self
                        .tab_index(id)
                        .is_some_and(|i| self.tabs[i].doc.is_modified())
                    {
                        " Your unsaved changes will be lost."
                    } else {
                        ""
                    }
                ),
                &["Reload", "Keep my version"],
            ),
            Confirm::Exit => {
                let n = self.tabs.iter().filter(|t| t.doc.is_modified()).count();
                (
                    "Unsaved changes",
                    format!("{} unsaved changes. Save before exiting?", plural_files(n)),
                    &["Save all", "Discard", "Cancel"],
                )
            }
        };

        let mut clicked = None;
        let resp = egui::Modal::new(Id::unique("confirm_modal")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(title);
            ui.add_space(6.0);
            ui.label(body);
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for (i, label) in buttons.iter().enumerate().rev() {
                    if ui.button(*label).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
        let cancel_index = buttons.len() - 1;
        let choice = match (clicked, resp.should_close()) {
            (Some(i), _) => i,
            (None, true) => cancel_index,
            (None, false) => return,
        };
        self.confirm = None;

        match confirm {
            Confirm::CloseTab(id) => {
                let Some(idx) = self.tab_index(id) else {
                    return;
                };
                match choice {
                    0 if self.save_tab(idx) => self.close_tab_now(idx),
                    1 => self.close_tab_now(idx),
                    _ => {}
                }
            }
            Confirm::Reload(id) => {
                if choice == 0
                    && let Some(idx) = self.tab_index(id)
                {
                    self.reload_tab(idx);
                }
            }
            Confirm::DiskChanged(id) => {
                if let Some(idx) = self.tab_index(id) {
                    if choice == 0 {
                        self.reload_tab(idx);
                    } else {
                        self.tabs[idx].doc.acknowledge_disk_change();
                        self.tabs[idx].disk_change_prompted = false;
                    }
                }
            }
            Confirm::Exit => match choice {
                0 => {
                    let all_saved = (0..self.tabs.len())
                        .all(|i| !self.tabs[i].doc.is_modified() || self.save_tab(i));
                    if all_saved {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
                1 => {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                _ => {}
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers

fn builtin_shortcut(cmd: &Command) -> Option<&'static str> {
    Some(match cmd {
        Command::Cut => "Ctrl+X",
        Command::Copy => "Ctrl+C",
        Command::Paste(PasteMode::Auto) => "Ctrl+V",
        Command::Delete => "Del",
        Command::ToggleInsertMode => "Ins",
        _ => return None,
    })
}

pub fn human_size(n: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.2} {}", UNITS[unit])
}

pub fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    format!("{out} bytes")
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn plural_files(n: usize) -> String {
    if n == 1 {
        "1 file has".into()
    } else {
        format!("{n} files have")
    }
}

pub fn to_hex_spaced(bytes: &[u8]) -> String {
    let hex = to_hex(bytes, true);
    hex.as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_helpers() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.50 KiB");
        assert_eq!(group_digits(1234567), "1,234,567 bytes");
        assert_eq!(group_digits(12), "12 bytes");
        assert_eq!(plural(1, "byte"), "1 byte");
        assert_eq!(plural(3, "byte"), "3 bytes");
        assert_eq!(to_hex_spaced(&[0xde, 0xad]), "DE AD");
    }
}

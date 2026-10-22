//! Application commands and their keyboard shortcuts.

use std::path::PathBuf;

use egui::{Key, KeyboardShortcut, Modifiers};

use crate::export::CopyFormat;
use crate::settings::SidePanel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteMode {
    Auto,
    Hex,
    Text,
    Base64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    NewFile,
    OpenFile,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    SaveAll,
    SaveSelectionAs,
    InsertFile,
    Reload,
    CloseTab,
    Exit,
    Undo,
    Redo,
    Cut,
    Copy,
    CopyAs(CopyFormat),
    CopyOffset,
    Paste(PasteMode),
    PasteText(String),
    Delete,
    SelectAll,
    SelectNone,
    ToggleInsertMode,
    ToggleReadOnly,
    Find,
    Replace,
    FindNext,
    FindPrev,
    FindAll,
    SearchSelection,
    GoTo,
    SelectRange,
    Fill,
    InsertBytes,
    Transform,
    ToggleBookmark,
    NextBookmark,
    PrevBookmark,
    NextTab,
    PrevTab,
    ShowPanel(SidePanel),
    NextDifference,
    PrevDifference,
    FontBigger,
    FontSmaller,
    FontReset,
    ReadOnlyNotice,
    ShowShortcuts,
    About,
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers::SHIFT;

const fn sc(m: Modifiers, k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(m, k)
}

/// Shortcuts that work regardless of focus. More specific shortcuts come
/// first so that e.g. Ctrl+Shift+S isn't swallowed by Ctrl+S.
pub fn global_shortcuts() -> Vec<(KeyboardShortcut, Command)> {
    vec![
        (sc(CMD_SHIFT, Key::S), Command::SaveAs),
        (sc(CMD_SHIFT, Key::Tab), Command::PrevTab),
        (sc(CMD_SHIFT, Key::F), Command::FindAll),
        (sc(CMD_SHIFT, Key::G), Command::SelectRange),
        (sc(SHIFT, Key::F3), Command::FindPrev),
        (sc(SHIFT, Key::F2), Command::PrevBookmark),
        (sc(SHIFT, Key::F7), Command::PrevDifference),
        (sc(CMD, Key::N), Command::NewFile),
        (sc(CMD, Key::O), Command::OpenFile),
        (sc(CMD, Key::S), Command::Save),
        (sc(CMD, Key::W), Command::CloseTab),
        (sc(CMD, Key::Q), Command::Exit),
        (sc(CMD, Key::F), Command::Find),
        (sc(CMD, Key::H), Command::Replace),
        (sc(CMD, Key::G), Command::GoTo),
        (sc(CMD, Key::B), Command::ToggleBookmark),
        (sc(CMD, Key::Tab), Command::NextTab),
        (sc(CMD, Key::PageDown), Command::NextTab),
        (sc(CMD, Key::PageUp), Command::PrevTab),
        (sc(CMD, Key::Plus), Command::FontBigger),
        (sc(CMD, Key::Equals), Command::FontBigger),
        (sc(CMD, Key::Minus), Command::FontSmaller),
        (sc(CMD, Key::Num0), Command::FontReset),
        (sc(NONE, Key::F3), Command::FindNext),
        (sc(NONE, Key::F2), Command::NextBookmark),
        (sc(NONE, Key::F7), Command::NextDifference),
        (sc(NONE, Key::F1), Command::ShowShortcuts),
    ]
}

/// Shortcuts that conflict with text editing; only active when no text
/// field has keyboard focus.
pub fn editor_shortcuts() -> Vec<(KeyboardShortcut, Command)> {
    vec![
        (sc(CMD_SHIFT, Key::Z), Command::Redo),
        (sc(CMD_SHIFT, Key::A), Command::SelectNone),
        (sc(CMD, Key::Z), Command::Undo),
        (sc(CMD, Key::Y), Command::Redo),
        (sc(CMD, Key::A), Command::SelectAll),
        (sc(CMD, Key::E), Command::Fill),
        (sc(CMD, Key::I), Command::InsertBytes),
        (sc(CMD, Key::T), Command::Transform),
        (sc(CMD, Key::R), Command::ToggleReadOnly),
    ]
}

/// Human-readable shortcut for menus, e.g. "Ctrl+S".
pub fn shortcut_for(ctx: &egui::Context, cmd: &Command) -> Option<String> {
    global_shortcuts()
        .into_iter()
        .chain(editor_shortcuts())
        .rev()
        .find(|(_, c)| c == cmd)
        .map(|(s, _)| ctx.format_shortcut(&s))
}

/// Shortcut reference shown in the help window.
pub const SHORTCUT_HELP: &[(&str, &str)] = &[
    ("Ctrl+N / Ctrl+O", "New file / open file"),
    ("Ctrl+S / Ctrl+Shift+S", "Save / save as"),
    ("Ctrl+W / Ctrl+Q", "Close tab / quit"),
    ("Ctrl+Tab / Ctrl+Shift+Tab", "Next / previous tab"),
    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
    ("Ctrl+X / Ctrl+C / Ctrl+V", "Cut / copy / paste"),
    ("Ctrl+A / Ctrl+Shift+A", "Select all / select none"),
    ("Insert", "Toggle insert / overwrite mode"),
    ("Delete / Backspace", "Delete selection or byte"),
    ("Tab", "Switch between hex and text column"),
    ("Arrows, PgUp/PgDn", "Move cursor (Shift extends selection)"),
    ("Home / End", "Start / end of row"),
    ("Ctrl+Home / Ctrl+End", "Start / end of file"),
    ("Ctrl+F / Ctrl+H", "Find / replace"),
    ("F3 / Shift+F3", "Find next / previous"),
    ("Ctrl+Shift+F", "Find all"),
    ("Ctrl+G / Ctrl+Shift+G", "Go to offset / select range"),
    ("Ctrl+B", "Toggle bookmark"),
    ("F2 / Shift+F2", "Next / previous bookmark"),
    ("F7 / Shift+F7", "Next / previous difference (compare)"),
    (
        "Ctrl+E / Ctrl+I / Ctrl+T",
        "Fill / insert bytes / transform",
    ),
    ("Ctrl+R", "Toggle read-only"),
    ("Ctrl+Plus / Ctrl+Minus / Ctrl+0", "Hex font size"),
    ("Ctrl+Mouse wheel", "Hex font size"),
    ("F1", "Keyboard shortcuts"),
];

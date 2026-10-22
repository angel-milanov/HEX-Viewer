//! User preferences, persisted between sessions.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::encoding::TextEncoding;
use crate::search::QuerySpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub fn to_egui(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OffsetBase {
    #[default]
    Hex,
    Decimal,
    Octal,
}

impl OffsetBase {
    pub fn format(self, value: usize, width: usize, uppercase: bool) -> String {
        match (self, uppercase) {
            (Self::Hex, true) => format!("{value:0width$X}"),
            (Self::Hex, false) => format!("{value:0width$x}"),
            (Self::Decimal, _) => format!("{value:0width$}"),
            (Self::Octal, _) => format!("{value:0width$o}"),
        }
    }

    /// Number of digits needed to show offsets up to `max`.
    pub fn digits(self, max: usize) -> usize {
        let radix = match self {
            Self::Hex => 16,
            Self::Decimal => 10,
            Self::Octal => 8,
        };
        let mut n = 1;
        let mut v = max;
        while v >= radix {
            v /= radix;
            n += 1;
        }
        n.max(8)
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Hex => "h",
            Self::Decimal => "",
            Self::Octal => "o",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MinimapMode {
    #[default]
    ByteClass,
    Entropy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SidePanel {
    #[default]
    Inspector,
    Bookmarks,
    SearchResults,
    Strings,
    Checksums,
    Statistics,
    Compare,
}

impl SidePanel {
    pub const ALL: [Self; 7] = [
        Self::Inspector,
        Self::Bookmarks,
        Self::SearchResults,
        Self::Strings,
        Self::Checksums,
        Self::Statistics,
        Self::Compare,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Inspector => "Inspector",
            Self::Bookmarks => "Bookmarks",
            Self::SearchResults => "Search",
            Self::Strings => "Strings",
            Self::Checksums => "Checksums",
            Self::Statistics => "Statistics",
            Self::Compare => "Compare",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// 0 = fit to window width.
    pub bytes_per_row: usize,
    pub group_size: usize,
    pub uppercase_hex: bool,
    pub offset_base: OffsetBase,
    pub text_encoding: TextEncoding,
    pub font_size: f32,
    pub color_bytes: bool,
    pub highlight_modified: bool,
    pub highlight_selection_matches: bool,
    pub show_text_column: bool,
    pub show_header: bool,
    pub show_minimap: bool,
    pub minimap_mode: MinimapMode,
    pub show_side_panel: bool,
    pub side_panel: SidePanel,
    pub show_toolbar: bool,
    pub show_status_bar: bool,
    pub inspector_big_endian: bool,
    pub strings_min_len: usize,
    pub strings_utf16: bool,
    pub search: QuerySpec,
    pub recent_files: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            bytes_per_row: 16,
            group_size: 8,
            uppercase_hex: true,
            offset_base: OffsetBase::Hex,
            text_encoding: TextEncoding::Ascii,
            font_size: 14.0,
            color_bytes: true,
            highlight_modified: true,
            highlight_selection_matches: true,
            show_text_column: true,
            show_header: true,
            show_minimap: true,
            minimap_mode: MinimapMode::ByteClass,
            show_side_panel: true,
            side_panel: SidePanel::Inspector,
            show_toolbar: true,
            show_status_bar: true,
            inspector_big_endian: false,
            strings_min_len: 4,
            strings_utf16: true,
            search: QuerySpec::default(),
            recent_files: Vec::new(),
        }
    }
}

pub const MAX_RECENT: usize = 12;

impl Settings {
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(MAX_RECENT);
    }

    /// Clamp values loaded from storage into sane ranges.
    pub fn sanitize(&mut self) {
        self.font_size = self.font_size.clamp(8.0, 40.0);
        if self.bytes_per_row > 256 {
            self.bytes_per_row = 16;
        }
        if ![1, 2, 4, 8, 16].contains(&self.group_size) {
            self.group_size = 8;
        }
        self.strings_min_len = self.strings_min_len.clamp(1, 256);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_formatting() {
        assert_eq!(OffsetBase::Hex.format(255, 8, true), "000000FF");
        assert_eq!(OffsetBase::Decimal.format(255, 4, true), "0255");
        assert_eq!(OffsetBase::Octal.format(8, 3, true), "010");
        assert_eq!(OffsetBase::Hex.digits(0xFFFF_FFFF), 8);
        assert_eq!(OffsetBase::Hex.digits(0x1_0000_0000), 9);
    }

    #[test]
    fn recent_files_dedup() {
        let mut s = Settings::default();
        s.push_recent("a".into());
        s.push_recent("b".into());
        s.push_recent("a".into());
        assert_eq!(s.recent_files, vec![PathBuf::from("a"), PathBuf::from("b")]);
    }
}

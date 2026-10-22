//! In-memory binary document with undo/redo history, modification tracking
//! and bookmarks.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// A single reversible edit: the bytes `old` at `offset` were replaced by `new`.
#[derive(Clone, Debug)]
pub struct Splice {
    pub offset: usize,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    id: u64,
}

impl Splice {
    fn inverse(&self) -> Splice {
        Splice {
            offset: self.offset,
            old: self.new.clone(),
            new: self.old.clone(),
            id: self.id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub offset: usize,
    pub len: usize,
    pub name: String,
    pub color: [u8; 3],
}

impl Bookmark {
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.offset && offset < self.offset + self.len.max(1)
    }
}

pub const BOOKMARK_COLORS: [[u8; 3]; 6] = [
    [255, 170, 0],
    [80, 200, 120],
    [90, 160, 255],
    [230, 90, 200],
    [240, 80, 80],
    [0, 200, 200],
];

/// Sorted, non-overlapping set of half-open byte ranges.
#[derive(Clone, Debug, Default)]
pub struct RangeSet {
    ranges: Vec<(usize, usize)>,
}

impl RangeSet {
    pub fn clear(&mut self) {
        self.ranges.clear();
    }

    pub fn contains(&self, pos: usize) -> bool {
        let idx = self.ranges.partition_point(|&(_, end)| end <= pos);
        self.ranges.get(idx).is_some_and(|&(start, _)| start <= pos)
    }

    /// Total number of bytes covered.
    #[cfg(test)]
    pub fn covered(&self) -> usize {
        self.ranges.iter().map(|(s, e)| e - s).sum()
    }

    /// Updates the set for a splice that removed `removed` bytes at `offset`
    /// and inserted `inserted` bytes in their place, marking the inserted bytes.
    pub fn apply_splice(&mut self, offset: usize, removed: usize, inserted: usize) {
        let removed_end = offset + removed;
        let mut out = Vec::with_capacity(self.ranges.len() + 2);
        for &(start, end) in &self.ranges {
            if end <= offset {
                out.push((start, end));
            } else if start >= removed_end {
                out.push((start - removed + inserted, end - removed + inserted));
            } else {
                if start < offset {
                    out.push((start, offset));
                }
                if end > removed_end {
                    out.push((offset + inserted, end - removed + inserted));
                }
            }
        }
        if inserted > 0 {
            out.push((offset, offset + inserted));
        }
        out.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(out.len());
        for (start, end) in out {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        self.ranges = merged;
    }
}

#[derive(Debug)]
pub struct Document {
    data: Arc<Vec<u8>>,
    pub path: Option<PathBuf>,
    pub title: String,
    pub read_only: bool,
    undo_stack: Vec<Splice>,
    redo_stack: Vec<Splice>,
    /// Whether the top undo entry may absorb the next typed edit.
    merge_open: bool,
    next_id: u64,
    /// Id of the top undo entry at the time of the last save (0 = empty stack).
    saved_id: u64,
    /// Incremented on every content change; used to invalidate caches.
    pub version: u64,
    pub modified: RangeSet,
    pub bookmarks: Vec<Bookmark>,
    pub disk_mtime: Option<SystemTime>,
}

impl Document {
    pub fn new_empty(title: String) -> Self {
        Self::from_bytes(Vec::new(), None, title)
    }

    pub fn from_bytes(data: Vec<u8>, path: Option<PathBuf>, title: String) -> Self {
        Self {
            data: Arc::new(data),
            path,
            title,
            read_only: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            merge_open: false,
            next_id: 1,
            saved_id: 0,
            version: 0,
            modified: RangeSet::default(),
            bookmarks: Vec::new(),
            disk_mtime: None,
        }
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let data = fs::read(path)?;
        let meta = fs::metadata(path).ok();
        let title = file_name_of(path);
        let mut doc = Self::from_bytes(data, Some(path.to_path_buf()), title);
        doc.read_only = meta.as_ref().is_some_and(|m| m.permissions().readonly());
        doc.disk_mtime = meta.and_then(|m| m.modified().ok());
        Ok(doc)
    }

    /// Re-reads the file from disk, discarding history.
    pub fn reload(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "document has no file path",
            ));
        };
        let fresh = Self::open(&path)?;
        let bookmarks = std::mem::take(&mut self.bookmarks);
        let version = self.version + 1;
        *self = fresh;
        self.version = version;
        self.bookmarks = bookmarks
            .into_iter()
            .filter(|b| b.offset < self.len())
            .collect();
        Ok(())
    }

    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "document has no file path",
            ));
        };
        self.save_as(&path)
    }

    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        write_file_safely(path, &self.data)?;
        self.path = Some(path.to_path_buf());
        self.title = file_name_of(path);
        self.saved_id = self.top_id();
        self.merge_open = false;
        self.modified.clear();
        self.disk_mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    /// Whether the file on disk has a different modification time than when
    /// it was last loaded or saved.
    pub fn changed_on_disk(&self) -> bool {
        let (Some(path), Some(known)) = (&self.path, self.disk_mtime) else {
            return false;
        };
        fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t != known)
    }

    pub fn acknowledge_disk_change(&mut self) {
        if let Some(path) = &self.path {
            self.disk_mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
        }
    }

    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// Cheap shared snapshot of the content, for background work.
    pub fn snapshot(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.data)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[inline]
    pub fn get(&self, offset: usize) -> Option<u8> {
        self.data.get(offset).copied()
    }

    pub fn is_modified(&self) -> bool {
        self.top_id() != self.saved_id
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Stop merging typed edits into the previous undo step.
    pub fn break_merge(&mut self) {
        self.merge_open = false;
    }

    fn top_id(&self) -> u64 {
        self.undo_stack.last().map_or(0, |s| s.id)
    }

    /// Replaces `remove` bytes at `offset` with `new`. Returns `false` if the
    /// document is read-only or the edit is a no-op.
    ///
    /// When `merge` is set, the edit may be folded into the previous undo step
    /// (used for continuous typing).
    pub fn replace(&mut self, offset: usize, remove: usize, new: Vec<u8>, merge: bool) -> bool {
        if self.read_only {
            return false;
        }
        let offset = offset.min(self.len());
        let remove = remove.min(self.len() - offset);
        let old = self.data[offset..offset + remove].to_vec();
        if old == new {
            return false;
        }
        self.apply_raw(offset, remove, &new);
        self.redo_stack.clear();

        let merged = merge && self.merge_open && self.try_merge(offset, &old, &new);
        if !merged {
            let id = self.next_id;
            self.next_id += 1;
            self.undo_stack.push(Splice {
                offset,
                old,
                new,
                id,
            });
        }
        self.merge_open = merge;
        true
    }

    fn try_merge(&mut self, offset: usize, old: &[u8], new: &[u8]) -> bool {
        let saved_id = self.saved_id;
        let Some(prev) = self.undo_stack.last_mut() else {
            return false;
        };
        if prev.id == saved_id {
            return false;
        }
        let prev_end = prev.offset + prev.new.len();
        if old.len() == new.len() && offset >= prev.offset && offset + new.len() <= prev_end {
            // Overwrite inside the region the previous step produced.
            let rel = offset - prev.offset;
            prev.new[rel..rel + new.len()].copy_from_slice(new);
            true
        } else if offset == prev_end {
            prev.old.extend_from_slice(old);
            prev.new.extend_from_slice(new);
            true
        } else {
            false
        }
    }

    fn apply_raw(&mut self, offset: usize, remove: usize, new: &[u8]) {
        let data = Arc::make_mut(&mut self.data);
        if remove == new.len() {
            data[offset..offset + remove].copy_from_slice(new);
        } else {
            data.splice(offset..offset + remove, new.iter().copied());
        }
        self.modified.apply_splice(offset, remove, new.len());
        self.shift_bookmarks(offset, remove, new.len());
        self.version += 1;
    }

    fn shift_bookmarks(&mut self, offset: usize, removed: usize, inserted: usize) {
        if removed == inserted {
            return;
        }
        let removed_end = offset + removed;
        for bm in &mut self.bookmarks {
            if bm.offset >= removed_end {
                bm.offset = bm.offset - removed + inserted;
            } else if bm.offset >= offset {
                bm.offset = offset;
            }
        }
        let len = self.data.len();
        for bm in &mut self.bookmarks {
            bm.offset = bm.offset.min(len.saturating_sub(1));
            bm.len = bm.len.min(len - bm.offset.min(len)).max(1);
        }
    }

    /// Overwrites bytes starting at `offset`, growing the file if needed.
    pub fn overwrite(&mut self, offset: usize, bytes: Vec<u8>, merge: bool) -> bool {
        let remove = bytes.len().min(self.len().saturating_sub(offset));
        self.replace(offset, remove, bytes, merge)
    }

    pub fn insert(&mut self, offset: usize, bytes: Vec<u8>, merge: bool) -> bool {
        self.replace(offset, 0, bytes, merge)
    }

    pub fn delete(&mut self, offset: usize, len: usize) -> bool {
        self.replace(offset, len, Vec::new(), false)
    }

    /// Undoes the last step. Returns the affected range `(offset, len)`.
    pub fn undo(&mut self) -> Option<(usize, usize)> {
        if self.read_only {
            return None;
        }
        let splice = self.undo_stack.pop()?;
        self.apply_raw(splice.offset, splice.new.len(), &splice.old);
        let range = (splice.offset, splice.old.len());
        self.redo_stack.push(splice);
        self.merge_open = false;
        if !self.is_modified() {
            self.modified.clear();
        }
        Some(range)
    }

    /// Redoes the last undone step. Returns the affected range `(offset, len)`.
    pub fn redo(&mut self) -> Option<(usize, usize)> {
        if self.read_only {
            return None;
        }
        let splice = self.redo_stack.pop()?;
        let inv = splice.inverse();
        self.apply_raw(inv.offset, inv.new.len(), &inv.old);
        let range = (splice.offset, splice.new.len());
        self.undo_stack.push(splice);
        self.merge_open = false;
        if !self.is_modified() {
            self.modified.clear();
        }
        Some(range)
    }

    #[cfg(test)]
    pub fn undo_count(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn bookmark_at(&self, offset: usize) -> Option<usize> {
        self.bookmarks.iter().position(|b| b.contains(offset))
    }

    pub fn display_path(&self) -> String {
        self.path
            .as_ref()
            .map_or_else(|| self.title.clone(), |p| p.display().to_string())
    }
}

pub fn file_name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Writes to a temporary file next to `path` and atomically renames it over
/// the target, falling back to a direct write when that isn't possible.
fn write_file_safely(path: &Path, data: &[u8]) -> io::Result<()> {
    let is_symlink = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !is_symlink {
        let tmp = dir.join(format!(
            ".{}.{}.tmp",
            file_name_of(path),
            std::process::id()
        ));
        let attempt = (|| -> io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
            drop(f);
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(&tmp, perms);
            }
            fs::rename(&tmp, path)
        })();
        match attempt {
            Ok(()) => return Ok(()),
            Err(_) => {
                let _ = fs::remove_file(&tmp);
            }
        }
    }
    fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(bytes: &[u8]) -> Document {
        Document::from_bytes(bytes.to_vec(), None, "t".into())
    }

    #[test]
    fn overwrite_undo_redo() {
        let mut d = doc(b"hello");
        assert!(d.overwrite(1, b"EL".to_vec(), false));
        assert_eq!(d.bytes(), b"hELlo");
        assert!(d.is_modified());
        assert_eq!(d.undo(), Some((1, 2)));
        assert_eq!(d.bytes(), b"hello");
        assert!(!d.is_modified());
        assert_eq!(d.redo(), Some((1, 2)));
        assert_eq!(d.bytes(), b"hELlo");
    }

    #[test]
    fn insert_delete() {
        let mut d = doc(b"abcdef");
        d.insert(3, b"XYZ".to_vec(), false);
        assert_eq!(d.bytes(), b"abcXYZdef");
        d.delete(0, 2);
        assert_eq!(d.bytes(), b"cXYZdef");
        d.undo();
        d.undo();
        assert_eq!(d.bytes(), b"abcdef");
    }

    #[test]
    fn overwrite_past_end_grows() {
        let mut d = doc(b"ab");
        d.overwrite(1, b"XYZ".to_vec(), false);
        assert_eq!(d.bytes(), b"aXYZ");
        d.undo();
        assert_eq!(d.bytes(), b"ab");
    }

    #[test]
    fn typing_merges_into_one_step() {
        let mut d = doc(&[0; 4]);
        d.overwrite(0, vec![0x10], true);
        d.overwrite(0, vec![0x12], true);
        d.overwrite(1, vec![0x30], true);
        d.overwrite(1, vec![0x34], true);
        assert_eq!(d.bytes(), &[0x12, 0x34, 0, 0]);
        assert_eq!(d.undo_count(), 1);
        d.undo();
        assert_eq!(d.bytes(), &[0, 0, 0, 0]);
    }

    #[test]
    fn insert_typing_merges() {
        let mut d = doc(b"ab");
        d.insert(1, vec![0xA0], true);
        d.overwrite(1, vec![0xAB], true);
        d.insert(2, vec![0xC0], true);
        assert_eq!(d.bytes(), &[b'a', 0xAB, 0xC0, b'b']);
        assert_eq!(d.undo_count(), 1);
        d.undo();
        assert_eq!(d.bytes(), b"ab");
    }

    #[test]
    fn break_merge_starts_new_step() {
        let mut d = doc(&[0; 4]);
        d.overwrite(0, vec![1], true);
        d.break_merge();
        d.overwrite(1, vec![2], true);
        assert_eq!(d.undo_count(), 2);
    }

    #[test]
    fn read_only_blocks_edits() {
        let mut d = doc(b"abc");
        d.read_only = true;
        assert!(!d.overwrite(0, b"x".to_vec(), false));
        assert_eq!(d.bytes(), b"abc");
    }

    #[test]
    fn range_set_tracks_splices() {
        let mut r = RangeSet::default();
        r.apply_splice(10, 2, 2);
        assert!(r.contains(10) && r.contains(11) && !r.contains(12));
        // insert 3 bytes before: range shifts right
        r.apply_splice(0, 0, 3);
        assert!(r.contains(13) && r.contains(14) && !r.contains(12));
        assert!(r.contains(0) && r.contains(2));
        // delete the middle
        r.apply_splice(5, 4, 0);
        assert!(r.contains(9) && r.contains(10) && !r.contains(11));
        assert_eq!(r.covered(), 5);
    }

    #[test]
    fn bookmarks_shift_on_insert() {
        let mut d = doc(b"0123456789");
        d.bookmarks.push(Bookmark {
            offset: 5,
            len: 2,
            name: "b".into(),
            color: BOOKMARK_COLORS[0],
        });
        d.insert(2, b"xx".to_vec(), false);
        assert_eq!(d.bookmarks[0].offset, 7);
        d.delete(0, 8);
        assert_eq!(d.bookmarks[0].offset, 0);
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hexeditor-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.bin");
        fs::write(&path, b"original").unwrap();
        let mut d = Document::open(&path).unwrap();
        d.overwrite(0, b"O".to_vec(), false);
        d.save().unwrap();
        assert!(!d.is_modified());
        assert_eq!(fs::read(&path).unwrap(), b"Original");
        d.reload().unwrap();
        assert_eq!(d.bytes(), b"Original");
        fs::remove_dir_all(&dir).unwrap();
    }
}

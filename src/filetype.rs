// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

use std::path::Path;

// Each selectable find(1) -type maps to one bit in a u8 mask, so
// `ignore_filetype` reduces the per-entry type test to one AND. `--empty`
// is a separate extra constraint (a stat), not a type selector.
const FT_FILE: u8 = 1 << 0;
const FT_DIRECTORY: u8 = 1 << 1;
const FT_SYMLINK: u8 = 1 << 2;
const FT_BLOCK_DEVICE: u8 = 1 << 3;
const FT_CHAR_DEVICE: u8 = 1 << 4;
const FT_PIPE: u8 = 1 << 5;
const FT_SOCKET: u8 = 1 << 6;

/// The concrete type of a directory entry, as classified by the walker.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EntryType {
    File,
    Dir,
    Symlink,
    BlockDevice,
    CharDevice,
    Fifo,
    Socket,
}

#[derive(Default, Copy, Clone)]
pub struct FileType {
    selected: u8,
    empty: bool,
}

impl FileType {
    /// Builds the type mask from the parsed `--type` selectors.
    pub fn new(selectors: &[crate::args::FileType]) -> Self {
        use crate::args::FileType as A;
        let mut selected = 0u8;
        let mut empty = false;

        for v in selectors {
            match v {
                A::Empty => empty = true,
                A::BlockDevice => selected |= FT_BLOCK_DEVICE,
                A::CharDevice => selected |= FT_CHAR_DEVICE,
                A::Directory => selected |= FT_DIRECTORY,
                A::Pipe => selected |= FT_PIPE,
                A::File => selected |= FT_FILE,
                A::Symlink => selected |= FT_SYMLINK,
                A::Socket => selected |= FT_SOCKET,
            }
        }

        // `--empty` alone implies both files and directories
        if empty && selected & (FT_FILE | FT_DIRECTORY) == 0 {
            selected |= FT_FILE | FT_DIRECTORY;
        }

        Self { selected, empty }
    }

    /// Whether an entry of `ty` at `path` should be skipped.
    #[inline]
    pub fn ignore_filetype(self, ty: EntryType, path: &Path) -> bool {
        if Self::type_bit(ty) & self.selected == 0 {
            return true;
        }
        // emptiness check last: it costs an extra stat/`read_dir`
        self.empty && !Self::is_empty(path, ty == EntryType::Dir)
    }

    #[inline]
    fn type_bit(ty: EntryType) -> u8 {
        match ty {
            EntryType::File => FT_FILE,
            EntryType::Dir => FT_DIRECTORY,
            EntryType::Symlink => FT_SYMLINK,
            EntryType::BlockDevice => FT_BLOCK_DEVICE,
            EntryType::CharDevice => FT_CHAR_DEVICE,
            EntryType::Fifo => FT_PIPE,
            EntryType::Socket => FT_SOCKET,
        }
    }

    /// Whether the entry is empty: a directory with no children, else a
    /// zero-byte file.
    #[inline]
    pub fn is_empty(path: &Path, is_dir: bool) -> bool {
        if is_dir {
            path.read_dir().is_ok_and(|mut r| r.next().is_none())
        } else {
            std::fs::metadata(path).is_ok_and(|m| m.len() == 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;
    use std::fs;
    use tempfile::TempDir;

    fn ft(types: &[args::FileType]) -> FileType {
        FileType::new(types)
    }

    #[test]
    fn new_empty_slice_selects_nothing() {
        let f = ft(&[]);
        assert_eq!(f.selected, 0);
        assert!(!f.empty);
    }

    #[test]
    fn new_file_only() {
        assert_eq!(ft(&[args::FileType::File]).selected, FT_FILE);
    }

    #[test]
    fn new_all_types() {
        let f = ft(&[
            args::FileType::File,
            args::FileType::Directory,
            args::FileType::Symlink,
            args::FileType::BlockDevice,
            args::FileType::CharDevice,
            args::FileType::Pipe,
            args::FileType::Socket,
        ]);
        assert_eq!(
            f.selected,
            FT_FILE
                | FT_DIRECTORY
                | FT_SYMLINK
                | FT_BLOCK_DEVICE
                | FT_CHAR_DEVICE
                | FT_PIPE
                | FT_SOCKET
        );
    }

    #[test]
    fn empty_alone_auto_expands_to_file_and_dir() {
        let f = ft(&[args::FileType::Empty]);
        assert!(f.empty);
        assert_eq!(f.selected, FT_FILE | FT_DIRECTORY);
    }

    #[test]
    fn empty_with_dir_does_not_add_file() {
        let f = ft(&[args::FileType::Empty, args::FileType::Directory]);
        assert!(f.empty);
        assert_eq!(f.selected, FT_DIRECTORY);
    }

    #[test]
    fn ignore_filetype_rejects_unselected_type() {
        let f = ft(&[args::FileType::Directory]);
        assert!(f.ignore_filetype(EntryType::File, Path::new("/x")));
        assert!(!f.ignore_filetype(EntryType::Dir, Path::new("/x")));
    }

    #[test]
    fn is_empty_zero_byte_file_true_nonempty_false() {
        let tmp = TempDir::new().unwrap();
        let empty = tmp.path().join("empty.txt");
        let full = tmp.path().join("full.txt");
        fs::write(&empty, b"").unwrap();
        fs::write(&full, b"x").unwrap();
        assert!(FileType::is_empty(&empty, false));
        assert!(!FileType::is_empty(&full, false));
    }

    #[test]
    fn is_empty_dir_true_when_childless() {
        let tmp = TempDir::new().unwrap();
        let empty_dir = tmp.path().join("d");
        fs::create_dir(&empty_dir).unwrap();
        assert!(FileType::is_empty(&empty_dir, true));
        fs::write(empty_dir.join("c"), b"x").unwrap();
        assert!(!FileType::is_empty(&empty_dir, true));
    }

    #[test]
    fn empty_constraint_rejects_nonempty_file() {
        let tmp = TempDir::new().unwrap();
        let full = tmp.path().join("full.txt");
        fs::write(&full, b"x").unwrap();
        let f = ft(&[args::FileType::Empty]);
        assert!(f.ignore_filetype(EntryType::File, &full));
    }
}

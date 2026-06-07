use crate::args;
use ignore::DirEntry;
use std::fs;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

// Each selectable find(1) -type maps to one bit in a u8 mask. Packing the
// seven concrete types into a single mask lets `ignore_filetype` reduce the
// per-entry type test to one bitwise AND instead of a seven-term `||` chain.
// `--empty` is kept separate because it is an *extra* constraint (a stat),
// not a type selector. A hand-rolled mask is used over the `bitflags` crate
// to avoid a dependency for what is a private, internal representation.
const FT_FILE: u8 = 1 << 0;
const FT_DIRECTORY: u8 = 1 << 1;
const FT_SYMLINK: u8 = 1 << 2;
const FT_BLOCK_DEVICE: u8 = 1 << 3;
const FT_CHAR_DEVICE: u8 = 1 << 4;
const FT_PIPE: u8 = 1 << 5;
const FT_SOCKET: u8 = 1 << 6;

#[derive(Default, Copy, Clone)]
pub struct FileType {
    /// Bitmask of the concrete types to match (see `FT_*` constants).
    selected: u8,
    /// When set, an entry must additionally be empty (zero-byte file or
    /// childless directory) to match — costs one extra stat/`read_dir`.
    empty: bool,
}

impl FileType {
    /// Builds the type mask from the parsed `--type` selectors.
    pub fn new(clap_filetype: &[args::FileType]) -> Self {
        let mut selected = 0u8;
        let mut empty = false;

        for v in clap_filetype {
            match v {
                args::FileType::Empty => empty = true,
                args::FileType::BlockDevice => selected |= FT_BLOCK_DEVICE,
                args::FileType::CharDevice => selected |= FT_CHAR_DEVICE,
                args::FileType::Directory => selected |= FT_DIRECTORY,
                args::FileType::Pipe => selected |= FT_PIPE,
                args::FileType::File => selected |= FT_FILE,
                args::FileType::Symlink => selected |= FT_SYMLINK,
                args::FileType::Socket => selected |= FT_SOCKET,
            }
        }

        // `--empty` alone implies both files and directories
        if empty && selected & (FT_FILE | FT_DIRECTORY) == 0 {
            selected |= FT_FILE | FT_DIRECTORY;
        }

        Self { selected, empty }
    }

    /// Whether `dir_entry` should be skipped given the selected types.
    #[inline]
    pub fn ignore_filetype(self, dir_entry: &DirEntry) -> bool {
        let Some(entry_type) = dir_entry.file_type() else {
            return true;
        };

        if Self::type_bit(entry_type) & self.selected == 0 {
            return true;
        }

        // emptiness check last: it costs an extra stat/`read_dir`
        self.empty && !Self::is_empty(dir_entry, entry_type)
    }

    /// Maps a concrete `fs::FileType` to its single `FT_*` mask bit.
    ///
    /// File-system types are mutually exclusive, so exactly one bit is set;
    /// the chain short-circuits at the first match (regular files first, as
    /// they dominate most trees). Returns 0 for any type with no selector
    /// (unreachable in practice: Unix covers all seven `st_mode` types and
    /// non-Unix entries are always file/dir/symlink).
    #[inline]
    fn type_bit(entry_type: fs::FileType) -> u8 {
        if entry_type.is_file() {
            FT_FILE
        } else if entry_type.is_dir() {
            FT_DIRECTORY
        } else if entry_type.is_symlink() {
            FT_SYMLINK
        // remaining types need the Unix-only FileTypeExt helpers below
        } else if Self::is_block_device(entry_type) {
            FT_BLOCK_DEVICE
        } else if Self::is_char_device(entry_type) {
            FT_CHAR_DEVICE
        } else if Self::is_pipe(entry_type) {
            FT_PIPE
        } else if Self::is_socket(entry_type) {
            FT_SOCKET
        } else {
            0
        }
    }

    #[cfg(unix)]
    #[inline]
    pub fn is_block_device(entry_type: fs::FileType) -> bool {
        entry_type.is_block_device()
    }

    #[cfg(not(unix))]
    #[inline]
    pub fn is_block_device(_: fs::FileType) -> bool {
        false
    }

    #[cfg(unix)]
    #[inline]
    pub fn is_char_device(entry_type: fs::FileType) -> bool {
        entry_type.is_char_device()
    }

    #[cfg(not(unix))]
    #[inline]
    pub fn is_char_device(_: fs::FileType) -> bool {
        false
    }

    #[cfg(unix)]
    #[inline]
    pub fn is_pipe(entry_type: fs::FileType) -> bool {
        entry_type.is_fifo()
    }

    #[cfg(not(unix))]
    #[inline]
    pub fn is_pipe(_: fs::FileType) -> bool {
        false
    }

    #[cfg(unix)]
    #[inline]
    pub fn is_socket(entry_type: fs::FileType) -> bool {
        entry_type.is_socket()
    }

    #[cfg(not(unix))]
    #[inline]
    pub fn is_socket(_: fs::FileType) -> bool {
        false
    }

    /// Whether the entry is empty: a directory with no children, else a
    /// zero-byte file.
    #[inline]
    pub fn is_empty(dir_entry: &DirEntry, entry_type: fs::FileType) -> bool {
        if entry_type.is_dir() {
            dir_entry.path().read_dir().is_ok_and(|mut r| r.next().is_none())
        } else {
            dir_entry.metadata().is_ok_and(|m| m.len() == 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;
    use ignore::WalkBuilder;
    use std::fs;
    use tempfile::TempDir;

    // --- FileType::new() ---

    #[test]
    fn test_new_empty_slice() {
        let ft = FileType::new(&[]);
        assert_eq!(ft.selected, 0);
        assert!(!ft.empty);
    }

    #[test]
    fn test_new_file_only() {
        let ft = FileType::new(&[args::FileType::File]);
        assert_eq!(ft.selected, FT_FILE);
        assert!(!ft.empty);
    }

    #[test]
    fn test_new_directory_only() {
        let ft = FileType::new(&[args::FileType::Directory]);
        assert_eq!(ft.selected, FT_DIRECTORY);
    }

    #[test]
    fn test_new_symlink_only() {
        let ft = FileType::new(&[args::FileType::Symlink]);
        assert_eq!(ft.selected, FT_SYMLINK);
    }

    #[test]
    fn test_new_empty_alone_auto_expands() {
        // Empty alone must auto-set both file and directory
        let ft = FileType::new(&[args::FileType::Empty]);
        assert!(ft.empty);
        assert_eq!(ft.selected, FT_FILE | FT_DIRECTORY);
    }

    #[test]
    fn test_new_empty_with_directory_no_file_expansion() {
        let ft =
            FileType::new(&[args::FileType::Empty, args::FileType::Directory]);
        assert!(ft.empty);
        assert_eq!(ft.selected, FT_DIRECTORY);
    }

    #[test]
    fn test_new_empty_with_file_no_dir_expansion() {
        let ft = FileType::new(&[args::FileType::Empty, args::FileType::File]);
        assert!(ft.empty);
        assert_eq!(ft.selected, FT_FILE);
    }

    #[test]
    fn test_new_empty_with_both_no_expansion_needed() {
        let ft = FileType::new(&[
            args::FileType::Empty,
            args::FileType::File,
            args::FileType::Directory,
        ]);
        assert!(ft.empty);
        assert_eq!(ft.selected, FT_FILE | FT_DIRECTORY);
    }

    #[test]
    fn test_new_all_types() {
        let ft = FileType::new(&[
            args::FileType::File,
            args::FileType::Directory,
            args::FileType::Symlink,
            args::FileType::BlockDevice,
            args::FileType::CharDevice,
            args::FileType::Pipe,
            args::FileType::Socket,
        ]);
        assert_eq!(
            ft.selected,
            FT_FILE
                | FT_DIRECTORY
                | FT_SYMLINK
                | FT_BLOCK_DEVICE
                | FT_CHAR_DEVICE
                | FT_PIPE
                | FT_SOCKET
        );
    }

    // --- ignore_filetype() ---

    /// Build a temp directory with a known fixture layout:
    /// - file.txt  (non-empty regular file)
    /// - empty.txt (empty regular file)
    /// - subdir/   (empty subdirectory)
    fn setup_fixture() -> TempDir {
        let tmp = TempDir::new().expect("failed to create tempdir");
        fs::write(tmp.path().join("file.txt"), b"content")
            .expect("write file.txt");
        fs::write(tmp.path().join("empty.txt"), b"").expect("write empty.txt");
        fs::create_dir(tmp.path().join("subdir")).expect("create subdir");
        tmp
    }

    fn walk_all(root: &std::path::Path) -> Vec<ignore::DirEntry> {
        WalkBuilder::new(root)
            .hidden(false)
            .standard_filters(false)
            .build()
            .filter_map(|e| e.ok())
            .collect()
    }

    fn find_entry<'a>(
        entries: &'a [ignore::DirEntry],
        name: &str,
        root: &std::path::Path,
    ) -> &'a ignore::DirEntry {
        let target = root.join(name);
        entries
            .iter()
            .find(|e| e.path() == target)
            .unwrap_or_else(|| panic!("{name} not found in walk"))
    }

    #[test]
    fn test_ignore_filetype_file_accepted() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::File]);
        let e = find_entry(&entries, "file.txt", tmp.path());
        assert!(!ft.ignore_filetype(e));
    }

    #[test]
    fn test_ignore_filetype_file_rejected_when_dir_only() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::Directory]);
        let e = find_entry(&entries, "file.txt", tmp.path());
        assert!(ft.ignore_filetype(e));
    }

    #[test]
    fn test_ignore_filetype_dir_accepted() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::Directory]);
        let e = find_entry(&entries, "subdir", tmp.path());
        assert!(!ft.ignore_filetype(e));
    }

    #[test]
    fn test_ignore_filetype_dir_rejected_when_file_only() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::File]);
        let e = find_entry(&entries, "subdir", tmp.path());
        assert!(ft.ignore_filetype(e));
    }

    #[test]
    fn test_ignore_filetype_empty_file_accepted() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        // Empty alone auto-expands to file+dir; empty.txt qualifies
        let ft = FileType::new(&[args::FileType::Empty]);
        let e = find_entry(&entries, "empty.txt", tmp.path());
        assert!(!ft.ignore_filetype(e));
    }

    #[test]
    fn test_ignore_filetype_nonempty_file_rejected_with_empty() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::Empty]);
        let e = find_entry(&entries, "file.txt", tmp.path());
        // file.txt has content → not empty → rejected
        assert!(ft.ignore_filetype(e));
    }

    #[test]
    fn test_ignore_filetype_empty_dir_accepted() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::Empty]);
        let e = find_entry(&entries, "subdir", tmp.path());
        // subdir has no children → empty → accepted
        assert!(!ft.ignore_filetype(e));
    }

    #[cfg(unix)]
    #[test]
    fn test_ignore_filetype_symlink_accepted() {
        let tmp = setup_fixture();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(tmp.path().join("file.txt"), &link)
            .expect("create symlink");
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::Symlink]);
        let e = find_entry(&entries, "link.txt", tmp.path());
        assert!(!ft.ignore_filetype(e));
    }

    #[cfg(unix)]
    #[test]
    fn test_ignore_filetype_symlink_rejected_when_file_only() {
        let tmp = setup_fixture();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(tmp.path().join("file.txt"), &link)
            .expect("create symlink");
        let entries = walk_all(tmp.path());
        let ft = FileType::new(&[args::FileType::File]);
        let e = find_entry(&entries, "link.txt", tmp.path());
        assert!(ft.ignore_filetype(e));
    }

    // --- FileType::is_empty() ---

    // F6 — is_empty: zero-byte file must return true
    #[test]
    fn test_is_empty_zero_byte_file_returns_true() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let e = find_entry(&entries, "empty.txt", tmp.path());
        let ft = e.file_type().unwrap();
        assert!(
            FileType::is_empty(e, ft),
            "zero-byte file must be reported as empty"
        );
    }

    // F6 — is_empty: non-zero file must return false
    #[test]
    fn test_is_empty_nonempty_file_returns_false() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        let e = find_entry(&entries, "file.txt", tmp.path());
        let ft = e.file_type().unwrap();
        assert!(
            !FileType::is_empty(e, ft),
            "file with content must not be reported as empty"
        );
    }

    // F7 — is_empty: empty directory must return true
    #[test]
    fn test_is_empty_empty_dir_returns_true() {
        let tmp = setup_fixture();
        let entries = walk_all(tmp.path());
        // setup_fixture creates subdir/ with no children
        let e = find_entry(&entries, "subdir", tmp.path());
        let ft = e.file_type().unwrap();
        assert!(
            FileType::is_empty(e, ft),
            "empty directory must be reported as empty"
        );
    }

    // F7 — is_empty: directory with a child must return false
    #[test]
    fn test_is_empty_nonempty_dir_returns_false() {
        let tmp = TempDir::new().expect("tempdir");
        let sub = tmp.path().join("nonempty");
        fs::create_dir(&sub).expect("create subdir");
        fs::write(sub.join("child.txt"), b"x").expect("write child");
        let entries = walk_all(tmp.path());
        let e = find_entry(&entries, "nonempty", tmp.path());
        let ft = e.file_type().unwrap();
        assert!(
            !FileType::is_empty(e, ft),
            "directory with a child must not be reported as empty"
        );
    }

    // F8 — is_empty: correct branch selected for file AND directory
    #[test]
    fn test_is_empty_dispatches_correct_branch_for_both_types() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("zero.txt"), b"").expect("write zero.txt");
        fs::create_dir(tmp.path().join("emptydir")).expect("create emptydir");
        let entries = walk_all(tmp.path());

        let file_e = find_entry(&entries, "zero.txt", tmp.path());
        let file_ft = file_e.file_type().unwrap();
        assert!(
            FileType::is_empty(file_e, file_ft),
            "zero-byte file must be empty (tests file branch)"
        );

        let dir_e = find_entry(&entries, "emptydir", tmp.path());
        let dir_ft = dir_e.file_type().unwrap();
        assert!(
            FileType::is_empty(dir_e, dir_ft),
            "empty directory must be empty (tests directory branch)"
        );
    }
}

use crate::args;
use ignore::DirEntry;
use std::fs;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

#[allow(clippy::struct_excessive_bools)]
#[derive(Default, Copy, Clone)]
pub struct FileType {
    pub empty: bool,
    pub block_device: bool,
    pub char_device: bool,
    pub directory: bool,
    pub pipe: bool,
    pub file: bool,
    pub symlink: bool,
    pub socket: bool,
}

impl FileType {
    /// Creates a new instance of `FileType` based on the provided Vec of `args::FileType`.
    ///
    /// # Arguments
    ///
    /// * `clap_filetype` - A reference to a Vec of `args::FileType` enums.
    ///
    /// # Returns
    ///
    /// * `Self` - A new instance of `FileType` with flags set based on the input Vec.
    pub fn new(clap_filetype: &[args::FileType]) -> Self {
        let mut filetype = Self::default();

        for v in clap_filetype {
            match v {
                args::FileType::Empty => filetype.empty = true,
                args::FileType::BlockDevice => filetype.block_device = true,
                args::FileType::CharDevice => filetype.char_device = true,
                args::FileType::Directory => filetype.directory = true,
                args::FileType::Pipe => filetype.pipe = true,
                args::FileType::File => filetype.file = true,
                args::FileType::Symlink => filetype.symlink = true,
                args::FileType::Socket => filetype.socket = true,
            }
        }

        // helpful default of searching for both empty files and directories
        if filetype.empty && !filetype.directory && !filetype.file {
            filetype.directory = true;
            filetype.file = true;
        }

        filetype
    }
    /// Determines whether to ignore a file type based on the flags set in the `FileType` instance.
    ///
    /// # Arguments
    ///
    /// * `dir_entry` - A reference to the `DirEntry` representing the file type to check.
    ///
    /// # Returns
    ///
    /// * `bool` - `true` if the file type should be ignored, `false` otherwise.
    #[inline]
    pub fn ignore_filetype(self, dir_entry: &DirEntry) -> bool {
        if let Some(entry_type) = dir_entry.file_type() {
            // works everywhere
            (!self.file && entry_type.is_file())
                || (!self.directory && entry_type.is_dir())
                || (!self.symlink && entry_type.is_symlink())
                // requires Unix-only std::os::unix::fs::FileTypeExt trait
                || (!self.block_device && Self::is_block_device(entry_type))
                || (!self.char_device && Self::is_char_device(entry_type))
                || (!self.pipe && Self::is_pipe(entry_type))
                || (!self.socket && Self::is_socket(entry_type))
                // exclusive search; requires additional lookups
                || (self.empty && !Self::is_empty(dir_entry, entry_type))
        } else {
            true
        }
    }

    /// Checks if the given file type represents a block device.
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

    /// Checks if the given file type represents a character device.
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

    /// Checks if the given file type represents a named FIFO
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

    /// Checks if the given file type represents a socket
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

    /// Checks if a directory entry is empty based on the given file type.
    ///
    /// If the file type is a directory, it checks if the directory is empty.
    /// If the file type is not a directory, it checks if the file has a size of 0.
    ///
    /// # Arguments
    /// * `dir_entry` - A reference to the directory entry to check.
    /// * `entry_type` - The file type of the directory entry.
    ///
    /// # Returns
    /// A boolean value indicating whether the directory entry is empty.
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
        assert!(!ft.file);
        assert!(!ft.directory);
        assert!(!ft.symlink);
        assert!(!ft.block_device);
        assert!(!ft.char_device);
        assert!(!ft.pipe);
        assert!(!ft.socket);
        assert!(!ft.empty);
    }

    #[test]
    fn test_new_file_only() {
        let ft = FileType::new(&[args::FileType::File]);
        assert!(ft.file);
        assert!(!ft.directory);
        assert!(!ft.symlink);
        assert!(!ft.empty);
    }

    #[test]
    fn test_new_directory_only() {
        let ft = FileType::new(&[args::FileType::Directory]);
        assert!(!ft.file);
        assert!(ft.directory);
        assert!(!ft.symlink);
    }

    #[test]
    fn test_new_symlink_only() {
        let ft = FileType::new(&[args::FileType::Symlink]);
        assert!(!ft.file);
        assert!(!ft.directory);
        assert!(ft.symlink);
    }

    #[test]
    fn test_new_empty_alone_auto_expands() {
        // Empty alone must auto-set both file and directory
        let ft = FileType::new(&[args::FileType::Empty]);
        assert!(ft.empty);
        assert!(ft.file);
        assert!(ft.directory);
    }

    #[test]
    fn test_new_empty_with_directory_no_file_expansion() {
        let ft =
            FileType::new(&[args::FileType::Empty, args::FileType::Directory]);
        assert!(ft.empty);
        assert!(ft.directory);
        assert!(!ft.file);
    }

    #[test]
    fn test_new_empty_with_file_no_dir_expansion() {
        let ft = FileType::new(&[args::FileType::Empty, args::FileType::File]);
        assert!(ft.empty);
        assert!(ft.file);
        assert!(!ft.directory);
    }

    #[test]
    fn test_new_empty_with_both_no_expansion_needed() {
        let ft = FileType::new(&[
            args::FileType::Empty,
            args::FileType::File,
            args::FileType::Directory,
        ]);
        assert!(ft.empty);
        assert!(ft.file);
        assert!(ft.directory);
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
        assert!(ft.file);
        assert!(ft.directory);
        assert!(ft.symlink);
        assert!(ft.block_device);
        assert!(ft.char_device);
        assert!(ft.pipe);
        assert!(ft.socket);
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

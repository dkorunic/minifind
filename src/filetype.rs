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
    pub fn new(clap_filetype: &Vec<args::FileType>) -> Self {
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
    pub fn ignore_filetype(self, dir_entry: &DirEntry) -> bool {
        if let Some(ref entry_type) = dir_entry.file_type() {
            // works everywhere
            (!self.file && entry_type.is_file())
                || (!self.directory && entry_type.is_dir())
                || (!self.symlink && entry_type.is_symlink())
                // requires Unix-only std::os::unix::fs::FileTypeExt trait
                || (!self.block_device && Self::is_block_device(*entry_type))
                || (!self.char_device && Self::is_char_device(*entry_type))
                || (!self.pipe && Self::is_pipe(*entry_type))
                || (!self.socket && Self::is_socket(*entry_type))
                // exclusive search; requires additional lookups
                || (self.empty && !Self::is_empty(dir_entry, *entry_type))
        } else {
            true
        }
    }

    /// Checks if the given file type represents a block device.
    #[cfg(unix)]
    pub fn is_block_device(entry_type: fs::FileType) -> bool {
        entry_type.is_block_device()
    }

    #[cfg(not(unix))]
    pub fn is_block_device(_: fs::FileType) -> bool {
        false
    }

    /// Checks if the given file type represents a character device.
    #[cfg(unix)]
    pub fn is_char_device(entry_type: fs::FileType) -> bool {
        entry_type.is_char_device()
    }

    #[cfg(not(unix))]
    pub fn is_char_device(_: fs::FileType) -> bool {
        false
    }

    /// Checks if the given file type represents a named FIFO
    #[cfg(unix)]
    pub fn is_pipe(entry_type: fs::FileType) -> bool {
        entry_type.is_fifo()
    }

    #[cfg(not(unix))]
    pub fn is_pipe(_: fs::FileType) -> bool {
        false
    }

    /// Checks if the given file type represents a socket
    #[cfg(unix)]
    pub fn is_socket(entry_type: fs::FileType) -> bool {
        entry_type.is_socket()
    }

    #[cfg(not(unix))]
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
    pub fn is_empty(dir_entry: &DirEntry, entry_type: fs::FileType) -> bool {
        if entry_type.is_dir() {
            dir_entry.path().read_dir().is_ok_and(|mut r| r.next().is_none())
        } else {
            dir_entry.metadata().is_ok_and(|m| m.len() == 0)
        }
    }
}

use crate::args;
use ignore::DirEntry;

#[derive(Default)]
pub struct FileType {
    pub file: bool,
    pub directory: bool,
    pub symlink: bool,
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
                args::FileType::File => filetype.file = true,
                args::FileType::Directory => filetype.directory = true,
                args::FileType::Symlink => filetype.symlink = true,
            }
        }

        filetype
    }

    /// Determines whether to ignore a specific file type based on the `FileType` flags set.
    ///
    /// # Arguments
    ///
    /// * `dir_entry` - A reference to the `DirEntry` to check for file type.
    ///
    /// # Returns
    ///
    /// * `bool` - A boolean indicating whether to ignore the file type.
    pub fn ignore_filetype(&self, dir_entry: &DirEntry) -> bool {
        if let Some(ref entry_type) = dir_entry.file_type() {
            (!self.file && entry_type.is_file())
                || (!self.directory && entry_type.is_dir())
                || (!self.symlink && entry_type.is_symlink())
        } else {
            true
        }
    }
}

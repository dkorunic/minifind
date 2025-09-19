use anstyle::AnsiColor;
use anyhow::{Error, anyhow};
use clap::ValueHint;
use clap::builder::{ValueParser, styling::Styles};
use clap::{Parser, ValueEnum};
use normpath::PathExt;
use std::path::{Path, PathBuf};
use std::thread;

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default())
    .usage(AnsiColor::Green.on_default())
    .literal(AnsiColor::Green.on_default())
    .placeholder(AnsiColor::Green.on_default());

#[derive(Parser, Default, Debug, Clone)]
#[clap(author, version, about, long_about = None, styles=STYLES)]
pub struct Args {
    /// Follow symlinks
    #[clap(short = 'f', long, action = clap::ArgAction::Set, default_value_t = false, visible_short_alias = 'L')]
    pub follow_symlinks: bool,

    /// Do not cross mount points
    #[clap(short = 'o', long, action = clap::ArgAction::Set, default_value_t = true, visible_alias = "xdev")]
    pub one_filesystem: bool,

    /// Number of threads to use when calibrating and scanning
    #[clap(short = 'x', long, value_parser = ValueParser::new(parse_threads), default_value_t = thread::available_parallelism().map(| n | n.get()).unwrap_or(2))]
    pub threads: usize,

    /// Maximum depth to traverse
    #[clap(short = 'd', long, value_parser)]
    pub max_depth: Option<usize>,

    /// Base of the file name matching globbing pattern
    #[clap(short = 'n', long, value_parser, conflicts_with = "regex")]
    pub name: Option<Vec<String>>,

    /// File name (full path) matching regular expression pattern
    #[clap(short = 'r', long, value_parser, conflicts_with = "name")]
    pub regex: Option<Vec<String>>,

    /// Case-insensitive matching for globbing and regular expression patterns
    #[clap(short = 'i', long, action = clap::ArgAction::Set, default_value_t = false)]
    pub case_insensitive: bool,

    /// Filter matches by type. Also accepts 'b', 'c', 'd', 'p', 'f', 'l', 's' and 'e' aliases.
    #[clap(short = 't', long, value_enum, default_values_t = [FileType::Directory, FileType::File, FileType::Symlink])]
    pub file_type: Vec<FileType>,

    /// Paths to check for large directories
    #[clap(required = true, value_parser = ValueParser::new(parse_paths), value_hint = ValueHint::AnyPath)]
    pub path: Vec<PathBuf>,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
pub enum FileType {
    #[value(alias = "e")]
    Empty,
    #[value(alias = "b")]
    BlockDevice,
    #[value(alias = "c")]
    CharDevice,
    #[value(alias = "d")]
    Directory,
    #[value(alias = "p")]
    Pipe,
    #[value(alias = "f")]
    File,
    #[value(alias = "l")]
    Symlink,
    #[value(alias = "s")]
    Socket,
}

/// Parses a string into an unsigned integer representing the number of threads.
///
/// # Arguments
///
/// * `x` - A string slice to be parsed into an unsigned integer.
///
/// # Returns
///
/// * `Result<usize, Error>` - An `Ok` variant containing the parsed value if it falls within the range (2..=65535),
///    or an `Err` variant with an error message if the value is outside the range.
fn parse_threads(x: &str) -> Result<usize, Error> {
    match x.parse::<usize>() {
        Ok(v) => match v {
            v if !(2..=65535).contains(&v) => {
                Err(anyhow!("threads should be in (2..65536) range"))
            }
            v => Ok(v),
        },
        Err(e) => Err(Error::from(e)),
    }
}

/// Parses a string into a `PathBuf`, checking if the path is a directory and exists.
///
/// # Arguments
///
/// * `x` - A string slice to be parsed into a `PathBuf`.
///
/// # Returns
///
/// * `Result<PathBuf, Error>` - An `Ok` variant containing a normalized `PathBuf` if the path is an existing directory,
///    or an `Err` variant with an error message if the path does not exist or is not a directory.
fn parse_paths(x: &str) -> Result<PathBuf, Error> {
    let p = Path::new(x);

    if directory_exists(p) {
        Ok(p.normalize()?.into_path_buf())
    } else {
        Err(anyhow!("'{x}' is not an existing directory"))
    }
}

/// Checks if the given path is a directory and exists.
///
/// # Arguments
///
/// * `x` - A reference to the path to check.
///
/// # Returns
///
/// * `bool` - `true` if the path is an existing directory, `false` otherwise.
#[inline]
fn directory_exists(x: &Path) -> bool {
    x.is_dir() && x.normalize().is_ok()
}

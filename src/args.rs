// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

use anstyle::AnsiColor;
use anyhow::{anyhow, Error};
use clap::builder::{styling::Styles, ValueParser};
use clap::ValueHint;
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

    /// Number of worker threads (default: logical CPU count; exceeding it may
    /// reduce throughput)
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

/// Warns (the caller still honors the value) when `threads > available`:
/// throughput typically drops past the core count as the output thread and
/// channel become the bottleneck.
#[must_use]
pub fn oversubscription_warning(
    threads: usize,
    available: usize,
) -> Option<String> {
    (threads > available).then(|| {
        format!(
            "--threads {threads} exceeds available parallelism \
             ({available}); throughput may decrease"
        )
    })
}

/// Parses `--threads`, enforcing `2..=65535`; the floor is 2 because one
/// thread is always reserved for output.
fn parse_threads(x: &str) -> Result<usize, Error> {
    let v = x.parse::<usize>()?;

    if (2..=65535).contains(&v) {
        Ok(v)
    } else {
        Err(anyhow!("threads should be in [2..=65535] range"))
    }
}

/// Parses a path argument, requiring an existing directory and normalizing it.
fn parse_paths(x: &str) -> Result<PathBuf, Error> {
    let p = Path::new(x);

    if p.is_dir() {
        Ok(p.normalize()?.into_path_buf())
    } else {
        Err(anyhow!("'{x}' is not an existing directory"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_threads_min_valid() {
        assert_eq!(parse_threads("2").unwrap(), 2);
    }

    #[test]
    fn test_parse_threads_max_valid() {
        assert_eq!(parse_threads("65535").unwrap(), 65535);
    }

    #[test]
    fn test_parse_threads_mid_valid() {
        assert_eq!(parse_threads("100").unwrap(), 100);
    }

    #[test]
    fn test_parse_threads_zero_invalid() {
        assert!(parse_threads("0").is_err());
    }

    #[test]
    fn test_parse_threads_one_invalid() {
        assert!(parse_threads("1").is_err());
    }

    #[test]
    fn test_parse_threads_too_large() {
        assert!(parse_threads("65536").is_err());
    }

    #[test]
    fn test_parse_threads_non_numeric() {
        assert!(parse_threads("abc").is_err());
        assert!(parse_threads("").is_err());
    }

    #[test]
    fn test_parse_threads_negative() {
        // Negative strings fail usize parse
        assert!(parse_threads("-1").is_err());
    }

    #[test]
    fn test_parse_paths_valid_dir() {
        let tmp = std::env::temp_dir();
        let result = parse_paths(tmp.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_paths_normalizes() {
        let tmp = std::env::temp_dir();
        let result = parse_paths(tmp.to_str().unwrap()).unwrap();
        assert!(result.is_dir());
    }

    #[test]
    fn test_parse_paths_nonexistent() {
        assert!(parse_paths("/nonexistent/xyz/abc123").is_err());
    }

    #[test]
    fn test_parse_paths_file_not_dir() {
        // /etc/hosts exists on macOS and Linux but is not a directory
        assert!(parse_paths("/etc/hosts").is_err());
    }

    #[test]
    fn test_oversubscription_warning_above_cpu_count() {
        let w = oversubscription_warning(16, 8);
        assert!(w.is_some());
        let msg = w.unwrap();
        assert!(msg.contains("16") && msg.contains('8'));
    }

    #[test]
    fn test_oversubscription_warning_at_or_below_cpu_count() {
        assert!(oversubscription_warning(8, 8).is_none());
        assert!(oversubscription_warning(4, 8).is_none());
        assert!(oversubscription_warning(2, 8).is_none());
    }

    // A4 — parse_paths normalises away ".." components
    #[test]
    fn test_parse_paths_normalizes_dotdot() {
        use std::path::Component;
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().parent().unwrap();
        let name = tmp.path().file_name().unwrap();
        // Construct: <parent>/<name>/../<name> — contains a redundant ".."
        let with_dotdot = parent.join(name).join("..").join(name);
        let result = parse_paths(with_dotdot.to_str().unwrap()).unwrap();
        assert!(
            !result.components().any(|c| matches!(c, Component::ParentDir)),
            "normalized path must not contain .. components"
        );
    }
}

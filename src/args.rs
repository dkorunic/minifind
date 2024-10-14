use anstyle::AnsiColor;
use anyhow::{anyhow, Error};
use clap::builder::{styling::Styles, ValueParser};
use clap::Parser;
use clap::ValueHint;
use std::path::PathBuf;
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
    #[clap(short = 'f', long, action = clap::ArgAction::Set, default_value_t = false)]
    pub follow_symlinks: bool,

    /// Do not cross mount points
    #[clap(short = 'o', long, action = clap::ArgAction::Set, default_value_t = true)]
    pub one_filesystem: bool,

    /// Number of threads to use when calibrating and scanning
    #[clap(short = 'x', long, value_parser = ValueParser::new(parse_threads), default_value_t = thread::available_parallelism().map(| n | n.get()).unwrap_or(1))]
    pub threads: usize,

    /// Maximum depth to traverse
    #[clap(short = 'd', long, value_parser)]
    pub max_depth: Option<usize>,

    /// Paths to check for large directories
    #[clap(required = true, value_parser, value_hint = ValueHint::AnyPath)]
    pub path: Vec<PathBuf>,
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

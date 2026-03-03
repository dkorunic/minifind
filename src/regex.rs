use anyhow::{Context, Error};
use regex::bytes::{RegexSet, RegexSetBuilder};
use std::borrow::Cow;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

/// Builds a `RegexSet` from a list of regular expression patterns.
///
/// # Arguments
///
/// * `patterns` - An optional reference to a vector of regular expression patterns.
///
/// # Returns
///
/// A Result containing the constructed `RegexSet` or an Error if the construction fails.
pub fn build_regex_set(
    patterns: Option<&Vec<String>>,
    case_insensitive: bool,
) -> Result<RegexSet, Error> {
    RegexSetBuilder::new(patterns.into_iter().flatten())
        .case_insensitive(case_insensitive)
        .build()
        .context("Unable to parse and build regular expression set")
}

/// Converts the given path to a byte slice, lossily on non-Unix.
#[cfg(unix)]
#[inline]
pub fn path_to_bytes<P: AsRef<Path> + ?Sized>(path: &P) -> Cow<'_, [u8]> {
    Cow::Borrowed(path.as_ref().as_os_str().as_bytes())
}

#[cfg(not(unix))]
#[inline]
pub fn path_to_bytes<P: AsRef<Path> + ?Sized>(path: &P) -> Cow<'_, [u8]> {
    match path.as_ref().to_string_lossy() {
        Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
        Cow::Owned(s) => Cow::Owned(s.into_bytes()),
    }
}

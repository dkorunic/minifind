use anyhow::{Context, Error};
use regex::{RegexSet, RegexSetBuilder};
use std::path::Path;

/// Builds a `GlobSet` from a list of glob patterns.
///
/// # Arguments
///
/// * `patterns` - An optional reference to a vector of glob patterns.
///
/// # Returns
///
/// A Result containing the constructed `GlobSet` or an Error if the construction fails.
pub fn build_regex_set(
    patterns: Option<&Vec<String>>,
    case_insensitive: bool,
) -> Result<RegexSet, Error> {
    RegexSetBuilder::new(patterns.map_or(&Vec::new(), |v| v))
        .case_insensitive(case_insensitive)
        .build()
        .context("Unable to build regular expression set")
}

/// Converts the given path to a string slice, returning an empty string if conversion fails.
pub fn path_to_bytes<P: AsRef<Path>>(path: &P) -> &str {
    path.as_ref().as_os_str().to_str().unwrap_or("")
}

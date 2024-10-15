use anyhow::{Context, Error};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Builds a `GlobSet` from a list of glob patterns.
///
/// # Arguments
///
/// * `patterns` - An optional reference to a vector of glob patterns.
///
/// # Returns
///
/// A Result containing the constructed `GlobSet` or an Error if the construction fails.
pub fn build_glob_set(
    patterns: &Option<Vec<String>>,
    case_insensitive: bool,
) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();

    for p in patterns.as_ref().unwrap_or(&Vec::new()) {
        builder.add(
            GlobBuilder::new(p).case_insensitive(case_insensitive).build()?,
        );
    }

    builder.build().context("Unable to build globbing set")
}

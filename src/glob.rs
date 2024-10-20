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
    patterns: Option<&Vec<String>>,
    case_insensitive: bool,
) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();

    for p in patterns.map_or(&Vec::new(), |v| v) {
        builder.add(
            GlobBuilder::new(p)
                .case_insensitive(case_insensitive)
                .build()
                .context("Unable to parse and build glob pattern")?,
        );
    }

    builder.build().context("Unable to build globbing set")
}

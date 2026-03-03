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

    for p in patterns.into_iter().flatten() {
        builder.add(
            GlobBuilder::new(p)
                .case_insensitive(case_insensitive)
                .build()
                .context("Unable to parse and build glob pattern")?,
        );
    }

    builder.build().context("Unable to build globbing set")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_glob_set_none() {
        let gs = build_glob_set(None, false).unwrap();
        assert!(!gs.is_match("anything"));
    }

    #[test]
    fn test_build_glob_set_empty_vec() {
        let patterns: Vec<String> = vec![];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(!gs.is_match("anything"));
    }

    #[test]
    fn test_build_glob_set_single_pattern() {
        let patterns = vec!["*.rs".to_string()];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(gs.is_match("main.rs"));
        assert!(!gs.is_match("main.py"));
    }

    #[test]
    fn test_build_glob_set_multiple_patterns() {
        let patterns =
            vec!["*.rs".to_string(), "*.toml".to_string()];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(gs.is_match("main.rs"));
        assert!(gs.is_match("Cargo.toml"));
        assert!(!gs.is_match("main.py"));
    }

    #[test]
    fn test_build_glob_set_case_sensitive() {
        let patterns = vec!["*.RS".to_string()];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(!gs.is_match("main.rs"));
        assert!(gs.is_match("main.RS"));
    }

    #[test]
    fn test_build_glob_set_case_insensitive() {
        let patterns = vec!["*.RS".to_string()];
        let gs = build_glob_set(Some(&patterns), true).unwrap();
        assert!(gs.is_match("main.rs"));
        assert!(gs.is_match("main.RS"));
    }

    #[test]
    fn test_build_glob_set_invalid_pattern() {
        // Unclosed bracket is an invalid glob pattern
        let patterns = vec!["[invalid".to_string()];
        assert!(build_glob_set(Some(&patterns), false).is_err());
    }

    #[test]
    fn test_build_glob_set_wildcard() {
        let patterns = vec!["*".to_string()];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(gs.is_match("anything"));
        assert!(gs.is_match("file.txt"));
    }

    #[test]
    fn test_build_glob_set_question_mark() {
        let patterns = vec!["?.rs".to_string()];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(gs.is_match("a.rs"));
        assert!(!gs.is_match("ab.rs"));
    }

    #[test]
    fn test_build_glob_set_double_star() {
        let patterns = vec!["**/*.rs".to_string()];
        let gs = build_glob_set(Some(&patterns), false).unwrap();
        assert!(gs.is_match("src/main.rs"));
        assert!(gs.is_match("a/b/c.rs"));
        assert!(!gs.is_match("src/main.py"));
    }
}

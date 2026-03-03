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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_regex_set_none() {
        let rs = build_regex_set(None, false).unwrap();
        assert!(!rs.is_match(b"anything"));
    }

    #[test]
    fn test_build_regex_set_empty_vec() {
        let patterns: Vec<String> = vec![];
        let rs = build_regex_set(Some(&patterns), false).unwrap();
        assert!(!rs.is_match(b"anything"));
    }

    #[test]
    fn test_build_regex_set_single_pattern() {
        let patterns = vec![r"\.rs$".to_string()];
        let rs = build_regex_set(Some(&patterns), false).unwrap();
        assert!(rs.is_match(b"/src/main.rs"));
        assert!(!rs.is_match(b"/src/main.py"));
    }

    #[test]
    fn test_build_regex_set_multiple_patterns() {
        let patterns =
            vec![r"\.rs$".to_string(), r"\.toml$".to_string()];
        let rs = build_regex_set(Some(&patterns), false).unwrap();
        assert!(rs.is_match(b"main.rs"));
        assert!(rs.is_match(b"Cargo.toml"));
        assert!(!rs.is_match(b"main.py"));
    }

    #[test]
    fn test_build_regex_set_case_sensitive() {
        let patterns = vec![r"\.RS$".to_string()];
        let rs = build_regex_set(Some(&patterns), false).unwrap();
        assert!(!rs.is_match(b"/src/main.rs"));
        assert!(rs.is_match(b"/src/main.RS"));
    }

    #[test]
    fn test_build_regex_set_case_insensitive() {
        let patterns = vec![r"\.RS$".to_string()];
        let rs = build_regex_set(Some(&patterns), true).unwrap();
        assert!(rs.is_match(b"/src/main.rs"));
        assert!(rs.is_match(b"/src/main.RS"));
    }

    #[test]
    fn test_build_regex_set_invalid() {
        let patterns = vec!["[invalid".to_string()];
        assert!(build_regex_set(Some(&patterns), false).is_err());
    }

    #[test]
    fn test_build_regex_set_anchored() {
        let patterns = vec![r"^/src/".to_string()];
        let rs = build_regex_set(Some(&patterns), false).unwrap();
        assert!(rs.is_match(b"/src/main.rs"));
        assert!(!rs.is_match(b"/other/main.rs"));
    }

    #[test]
    fn test_path_to_bytes_simple() {
        let path = Path::new("/some/path/file.txt");
        let bytes = path_to_bytes(path);
        assert_eq!(bytes.as_ref(), b"/some/path/file.txt");
    }

    #[test]
    fn test_path_to_bytes_with_spaces() {
        let path = Path::new("/path/with spaces/file.txt");
        let bytes = path_to_bytes(path);
        assert_eq!(bytes.as_ref(), b"/path/with spaces/file.txt");
    }

    #[test]
    fn test_path_to_bytes_root() {
        let path = Path::new("/");
        let bytes = path_to_bytes(path);
        assert_eq!(bytes.as_ref(), b"/");
    }
}

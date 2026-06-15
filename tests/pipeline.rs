// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Integration tests that drive the real `minifind::run` pipeline end to end,
//! capturing its byte output through a shared in-memory sink instead of
//! re-implementing the walker/filter/output wiring.

use minifind::args::{Args, FileType};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

/// A cloneable, thread-safe `Write` sink: `run` takes one clone on its output
/// thread while the test keeps another to read the bytes afterwards.
#[derive(Clone)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn base_args(paths: Vec<PathBuf>, file_type: Vec<FileType>) -> Args {
    Args {
        threads: 2,
        path: paths,
        follow_symlinks: false,
        one_filesystem: true,
        max_depth: None,
        max_scan_rate: None,
        name: None,
        regex: None,
        case_insensitive: false,
        file_type,
    }
}

/// Runs the real pipeline and parses its newline-delimited output into paths.
fn run_capture(args: &Args) -> Vec<PathBuf> {
    let sink = SharedSink(Arc::new(Mutex::new(Vec::new())));
    let out = sink.clone();
    // run joins its output thread before returning, so all bytes are present.
    minifind::run(args, move || out).unwrap();
    let bytes = sink.0.lock().unwrap();
    parse_paths(&bytes)
}

#[cfg(unix)]
fn parse_paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(OsStr::from_bytes(line)))
        .collect()
}

#[cfg(not(unix))]
fn parse_paths(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[test]
fn no_filter_emits_every_entry() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), b"x").unwrap();
    let args = base_args(
        vec![tmp.path().to_path_buf()],
        vec![FileType::File, FileType::Directory],
    );
    let results = run_capture(&args);
    assert!(
        results.iter().any(|p| p.ends_with("hello.txt")),
        "hello.txt must appear when no name/regex filter is active"
    );
}

#[test]
fn name_filter_includes_matching_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("keep.rs"), b"x").unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.name = Some(vec!["*.rs".to_string()]);
    let results = run_capture(&args);
    assert!(
        results.iter().any(|p| p.ends_with("keep.rs")),
        "keep.rs must match the '*.rs' name glob"
    );
}

#[test]
fn name_filter_excludes_nonmatching_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("skip.txt"), b"x").unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.name = Some(vec!["*.rs".to_string()]);
    let results = run_capture(&args);
    assert!(
        !results.iter().any(|p| p.ends_with("skip.txt")),
        "skip.txt must not match the '*.rs' name glob"
    );
}

#[test]
fn regex_filter_matches_against_full_path() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("inner");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("file.log"), b"x").unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.regex = Some(vec![r"inner/.*\.log$".to_string()]);
    let results = run_capture(&args);
    assert!(
        results.iter().any(|p| p.ends_with("file.log")),
        "regex must match against the full path, including 'inner/'"
    );
}

#[test]
fn file_type_filter_still_descends_into_directories() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("inner.txt"), b"x").unwrap();
    // File-only: directory entries are rejected, but their children must still
    // be visited (the walker must Continue, not Skip, the subtree).
    let args = base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    let results = run_capture(&args);
    assert!(
        results.iter().any(|p| p.ends_with("inner.txt")),
        "inner.txt must appear with --type file; missing means the \
         directory subtree was skipped instead of descended"
    );
}

// Pins the exact output framing: every emitted path is followed by exactly
// one newline, with no extra bytes (guards the output thread against a missing
// newline, a doubled newline, or a doubled path copy).
#[cfg(unix)]
#[test]
fn output_is_each_path_followed_by_a_single_newline() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("d")).unwrap();
    std::fs::write(tmp.path().join("d/a.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("b.txt"), b"x").unwrap();
    let args = base_args(
        vec![tmp.path().to_path_buf()],
        vec![FileType::File, FileType::Directory],
    );

    let sink = SharedSink(Arc::new(Mutex::new(Vec::new())));
    let out = sink.clone();
    minifind::run(&args, move || out).unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    assert!(!bytes.is_empty(), "output must not be empty");
    assert_eq!(
        *bytes.last().unwrap(),
        b'\n',
        "output must end with a newline"
    );

    let segments: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    // The split's final segment is empty (trailing newline); no others may be.
    assert!(segments.last().unwrap().is_empty());
    for seg in &segments[..segments.len() - 1] {
        assert!(!seg.is_empty(), "no empty lines / no doubled newline");
    }

    // Total bytes must equal exactly sum(path_len + 1) — one newline per path,
    // no extra copies.
    let paths = parse_paths(&bytes);
    let expected: usize =
        paths.iter().map(|p| p.as_os_str().as_bytes().len() + 1).sum();
    assert_eq!(
        bytes.len(),
        expected,
        "each path must be followed by exactly one newline"
    );
}

// Exercises the batched channel path across the BATCH_SIZE (64) boundary:
// with many entries, full batches are flushed mid-walk and the trailing
// partial batch is flushed on Drop. Every entry must appear exactly once.
#[test]
fn emits_every_entry_across_batch_boundaries() {
    let tmp = TempDir::new().unwrap();
    const N: usize = 500; // well over BATCH_SIZE, spanning several batches
    for i in 0..N {
        std::fs::write(tmp.path().join(format!("f{i:04}.txt")), b"x").unwrap();
    }
    // File-only: the root directory is not emitted, so every result is one
    // of the N files.
    let args = base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    let results = run_capture(&args);
    let files = results
        .iter()
        .filter(|p| p.extension() == Some("txt".as_ref()))
        .count();
    assert_eq!(files, N, "all {N} files must be emitted exactly once");
}

#[test]
fn duplicate_paths_are_emitted_once() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("once.txt"), b"x").unwrap();
    let path = tmp.path().to_path_buf();
    let args = base_args(vec![path.clone(), path], vec![FileType::File]);
    let results = run_capture(&args);
    let count = results.iter().filter(|p| p.ends_with("once.txt")).count();
    assert_eq!(
        count, 1,
        "once.txt must appear exactly once when the same path is given twice"
    );
}

#[test]
fn output_unchanged_under_scan_rate_limit() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    std::fs::write(tmp.path().join("sub/a.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("b.txt"), b"x").unwrap();

    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    let baseline = run_capture(&args);

    args.max_scan_rate = Some(100_000);
    let limited = run_capture(&args);

    assert_eq!(baseline, limited);
}

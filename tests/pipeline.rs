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
        idle: false,
        path: paths,
        follow_symlinks: false,
        one_filesystem: true,
        max_depth: None,
        min_depth: None,
        max_scan_rate: None,
        max_results: None,
        name: None,
        regex: None,
        case_insensitive: false,
        file_type,
        meta: minifind::meta::Predicates::default(),
        path_glob: None,
        lname: None,
        access: 0,
        exclude: None,
        null: false,
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

// With --null, every path is terminated by a NUL byte and no newline appears,
// so paths that themselves contain newlines stay unambiguous (the point of
// -print0 / xargs -0).
#[cfg(unix)]
#[test]
fn null_separator_terminates_each_path_with_nul() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("d")).unwrap();
    std::fs::write(tmp.path().join("d/a.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("b.txt"), b"x").unwrap();
    let mut args = base_args(
        vec![tmp.path().to_path_buf()],
        vec![FileType::File, FileType::Directory],
    );
    args.null = true;

    let sink = SharedSink(Arc::new(Mutex::new(Vec::new())));
    let out = sink.clone();
    minifind::run(&args, move || out).unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    assert!(!bytes.is_empty(), "output must not be empty");
    assert_eq!(*bytes.last().unwrap(), 0, "each path ends with a NUL");
    assert!(!bytes.contains(&b'\n'), "no newline separators under --null");

    // Splitting on NUL yields one non-empty segment per path plus a trailing
    // empty (from the final terminator), and total bytes == sum(len + 1).
    let segments: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    assert!(segments.last().unwrap().is_empty());
    for seg in &segments[..segments.len() - 1] {
        assert!(!seg.is_empty(), "no doubled NUL / empty segment");
    }
    let paths: Vec<PathBuf> = segments[..segments.len() - 1]
        .iter()
        .map(|s| PathBuf::from(OsStr::from_bytes(s)))
        .collect();
    let expected: usize =
        paths.iter().map(|p| p.as_os_str().as_bytes().len() + 1).sum();
    assert_eq!(bytes.len(), expected, "exactly one NUL per path");
}

// Exercises the batched channel path across the BATCH_SIZE (256) boundary:
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
fn min_depth_suppresses_shallow_entries_but_still_descends() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("a")).unwrap();
    std::fs::write(tmp.path().join("a/deep.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("top.txt"), b"x").unwrap();
    // depths: root=0, a=1, top.txt=1, a/deep.txt=2
    let mut args = base_args(
        vec![tmp.path().to_path_buf()],
        vec![FileType::File, FileType::Directory],
    );

    // --min-depth 1: root (depth 0) is suppressed, its children remain.
    args.min_depth = Some(1);
    let r1 = run_capture(&args);
    assert!(!r1.iter().any(|p| p == tmp.path()), "root (depth 0) suppressed");
    assert!(r1.iter().any(|p| p.ends_with("a")));
    assert!(r1.iter().any(|p| p.ends_with("top.txt")));
    assert!(r1.iter().any(|p| p.ends_with("deep.txt")));

    // --min-depth 2: only the depth-2 file remains; that it appears at all
    // proves traversal still descended through the suppressed depth-1 dir.
    args.min_depth = Some(2);
    let r2 = run_capture(&args);
    assert!(
        r2.iter().any(|p| p.ends_with("deep.txt")),
        "depth-2 entry must still be reached through the ungated shallow dir"
    );
    assert!(!r2.iter().any(|p| p.ends_with("top.txt")), "depth 1 suppressed");
    assert!(!r2.iter().any(|p| p.ends_with("/a")));
}

#[test]
fn max_results_caps_output_at_exactly_n() {
    let tmp = TempDir::new().unwrap();
    for i in 0..50 {
        std::fs::write(tmp.path().join(format!("f{i:02}.txt")), b"x").unwrap();
    }
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.max_results = Some(5);
    let results = run_capture(&args);
    assert_eq!(results.len(), 5, "output is capped at exactly N matches");
}

#[test]
fn max_results_counts_post_filter_matches() {
    // The cap counts emitted *results* (after the --name glob), not walked
    // entries — proving it lives downstream of filtering.
    let tmp = TempDir::new().unwrap();
    for i in 0..30 {
        std::fs::write(tmp.path().join(format!("k{i:02}.rs")), b"x").unwrap();
        std::fs::write(tmp.path().join(format!("s{i:02}.txt")), b"x").unwrap();
    }
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.name = Some(vec!["*.rs".to_string()]);
    args.max_results = Some(4);
    let results = run_capture(&args);
    assert_eq!(results.len(), 4);
    assert!(
        results.iter().all(|p| p.extension() == Some("rs".as_ref())),
        "only --name matches count toward --max-results"
    );
}

#[test]
fn size_predicate_filters_by_bytes() {
    use minifind::meta::SizePred;
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("small.bin"), vec![0u8; 10]).unwrap();
    std::fs::write(tmp.path().join("big.bin"), vec![0u8; 5000]).unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    // > 1 KiB: only the 5000-byte file
    args.meta.size = Some(SizePred::parse("+1k").unwrap());
    let results = run_capture(&args);
    assert!(results.iter().any(|p| p.ends_with("big.bin")));
    assert!(!results.iter().any(|p| p.ends_with("small.bin")));
}

#[test]
fn mtime_predicate_is_relative_to_now() {
    use minifind::meta::{TimePred, DAY};
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("fresh.txt"), b"x").unwrap();

    // just-created file: 0 days elapsed → matches `-mtime -1` (less than a day)
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.times.push(TimePred::mtime("-1", DAY).unwrap());
    assert!(run_capture(&args).iter().any(|p| p.ends_with("fresh.txt")));

    // ...but not `-mtime +0` (which needs at least one full day elapsed)
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.times.push(TimePred::mtime("+0", DAY).unwrap());
    assert!(!run_capture(&args).iter().any(|p| p.ends_with("fresh.txt")));
}

#[cfg(unix)]
#[test]
fn perm_predicate_matches_mode_bits() {
    use minifind::meta::PermPred;
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("secret");
    std::fs::write(&f, b"x").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))
        .unwrap();

    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.perm = Some(PermPred::parse("600").unwrap());
    assert!(run_capture(&args).iter().any(|p| p.ends_with("secret")));

    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.perm = Some(PermPred::parse("644").unwrap());
    assert!(!run_capture(&args).iter().any(|p| p.ends_with("secret")));
}

#[cfg(unix)]
#[test]
fn uid_predicate_matches_owner() {
    use minifind::meta::IdPred;
    use std::os::unix::fs::MetadataExt;
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("owned");
    std::fs::write(&f, b"x").unwrap();
    let uid = std::fs::metadata(&f).unwrap().uid();

    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.uid = Some(IdPred::exact(uid));
    assert!(run_capture(&args).iter().any(|p| p.ends_with("owned")));

    // a uid that is definitely not the owner's
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.uid = Some(IdPred::exact(uid.wrapping_add(1)));
    assert!(!run_capture(&args).iter().any(|p| p.ends_with("owned")));
}

#[test]
fn path_glob_matches_full_path() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    std::fs::write(tmp.path().join("a/b/deep.txt"), b"x").unwrap();
    std::fs::write(tmp.path().join("top.txt"), b"x").unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    // `*` crosses `/` (find -path semantics), so this matches the nested file
    args.path_glob = Some(vec!["*/a/b/*".to_string()]);
    let results = run_capture(&args);
    assert!(results.iter().any(|p| p.ends_with("deep.txt")));
    assert!(!results.iter().any(|p| p.ends_with("top.txt")));
}

#[cfg(unix)]
#[test]
fn lname_matches_symlink_target() {
    let tmp = TempDir::new().unwrap();
    std::os::unix::fs::symlink("/usr/lib/libc.so", tmp.path().join("good"))
        .unwrap();
    std::os::unix::fs::symlink("/etc/hosts", tmp.path().join("other"))
        .unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::Symlink]);
    args.lname = Some(vec!["*.so".to_string()]);
    let results = run_capture(&args);
    assert!(results.iter().any(|p| p.ends_with("good")));
    assert!(!results.iter().any(|p| p.ends_with("other")));
}

#[cfg(unix)]
#[test]
fn links_predicate_matches_hardlink_count() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a");
    std::fs::write(&a, b"x").unwrap();
    std::fs::hard_link(&a, tmp.path().join("b")).unwrap(); // a,b now nlink 2
    std::fs::write(tmp.path().join("solo"), b"x").unwrap(); // nlink 1
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.links = Some(minifind::meta::IdPred::parse("2").unwrap());
    let results = run_capture(&args);
    assert!(!results.iter().any(|p| p.ends_with("solo")));
    assert_eq!(
        results
            .iter()
            .filter(|p| p.ends_with("a") || p.ends_with("b"))
            .count(),
        2
    );
}

#[cfg(unix)]
#[test]
fn inum_predicate_matches_inode() {
    use std::os::unix::fs::MetadataExt;
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("f");
    std::fs::write(&f, b"x").unwrap();
    let ino = std::fs::metadata(&f).unwrap().ino();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.inum =
        Some(minifind::meta::IdPred::parse(&ino.to_string()).unwrap());
    let results = run_capture(&args);
    assert!(results.iter().any(|p| p.ends_with("f")));
}

#[cfg(unix)]
#[test]
fn readable_predicate_keeps_a_readable_file() {
    // R_OK is unaffected by a noexec mount or by running as root, so a normal
    // 0644 file is reliably "readable" — this exercises the faccessat wiring.
    // (-executable/-writable negatives depend on the mount and uid, so the
    // find-parity check covers those end to end.)
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("r.txt"), b"x").unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.access = minifind::meta::access::READ;
    let results = run_capture(&args);
    assert!(results.iter().any(|p| p.ends_with("r.txt")));
}

#[cfg(unix)]
#[test]
fn nouser_excludes_owned_entries() {
    // Files we create are owned by a uid that resolves (the running user), so
    // -nouser must exclude them.
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("owned.txt"), b"x").unwrap();
    let mut args =
        base_args(vec![tmp.path().to_path_buf()], vec![FileType::File]);
    args.meta.nouser = true;
    assert!(run_capture(&args).is_empty());
}

#[test]
fn exclude_prunes_subtree_end_to_end() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
    std::fs::write(tmp.path().join("node_modules/pkg/index.js"), b"x")
        .unwrap();
    std::fs::write(tmp.path().join("app.js"), b"x").unwrap();
    let mut args = base_args(
        vec![tmp.path().to_path_buf()],
        vec![FileType::File, FileType::Directory],
    );
    args.exclude = Some(vec!["node_modules".to_string()]);
    let results = run_capture(&args);
    assert!(
        results.iter().any(|p| p.ends_with("app.js")),
        "non-excluded file must appear"
    );
    assert!(
        !results.iter().any(|p| p.to_string_lossy().contains("node_modules")),
        "excluded subtree must be entirely absent"
    );
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

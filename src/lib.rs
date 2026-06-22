// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! `minifind` is a minimal, parallel `find(1)` reimplementation.
//!
//! The crate is split into a thin binary (`main.rs`) and this library so the
//! traversal pipeline can be driven through a public API and exercised by
//! integration tests. [`run`] spawns a dedicated output thread, walks every
//! requested path in parallel via [`walk::walk_parallel`], filters entries by
//! file type / glob / regex, and writes matched paths to a caller-supplied sink.
//!
//! # Examples
//!
//! ```no_run
//! use minifind::args::Args;
//! use std::io;
//!
//! let args = Args::default();
//! minifind::run(&args, || io::stdout().lock()).unwrap();
//! ```

use anyhow::Error;
use crossbeam_channel::{bounded, Sender};
use itertools::Itertools;
use std::io::{BufWriter, Write};
use std::num::NonZeroU32;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

pub mod args;
pub mod filetype;
pub mod glob;
pub mod interrupt;
pub mod meta;
pub mod ratelimit;
pub mod regex;
pub mod walk;

use args::Args;
use ratelimit::Limiter;
use walk::{Entry, WalkState};

/// Entries per channel message; batching amortizes the per-send atomic
/// synchronization. Tuned against a warm Linux-kernel tree: gains flatten by
/// 256, and larger batches only add RSS and latency-to-first-line.
const BATCH_SIZE: usize = 256;

/// Channel capacity in batches is `CHAN_MULT * (threads - 1)`; throughput is
/// insensitive to it across ~2..16.
const CHAN_MULT: usize = 4;

/// Per-walker-thread accumulator; sends [`Entry`] items in batches and
/// flushes the partial tail on `Drop` (when the visitor closure ends).
struct BatchSender {
    buf: Vec<Entry>,
    tx: Sender<Vec<Entry>>,
    closed: bool,
}

impl BatchSender {
    fn new(tx: Sender<Vec<Entry>>) -> Self {
        Self { buf: Vec::with_capacity(BATCH_SIZE), tx, closed: false }
    }

    /// Queues `entry`, flushing when full. Returns `false` once the channel
    /// has closed, signalling the caller to stop walking.
    fn push(&mut self, entry: Entry) -> bool {
        self.buf.push(entry);
        if self.buf.len() >= BATCH_SIZE {
            self.flush()
        } else {
            true
        }
    }

    /// Sends the current batch (if any). Returns `false` if the channel closed.
    fn flush(&mut self) -> bool {
        if self.buf.is_empty() {
            return !self.closed;
        }
        let batch =
            std::mem::replace(&mut self.buf, Vec::with_capacity(BATCH_SIZE));
        if self.tx.send(batch).is_err() {
            self.closed = true;
            return false;
        }
        true
    }
}

impl Drop for BatchSender {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Runs the pipeline: parallel walk → filter by type/glob/regex → write
/// matched paths.
///
/// `make_out` builds the sink *on the output thread*, so callers can hand over
/// a non-`Send` handle like [`std::io::StdoutLock`] without crossing threads.
///
/// # Errors
///
/// Signal registration failure, an invalid glob/regex pattern, or no paths.
pub fn run<W, F>(args: &Args, make_out: F) -> Result<(), Error>
where
    W: Write,
    F: FnOnce() -> W + Send + 'static,
{
    let shutdown = Arc::new(AtomicBool::new(false));
    interrupt::setup_interrupt_handler(&shutdown)?;

    let glob_name =
        glob::build_glob_set(args.name.as_deref(), args.case_insensitive)?;
    let glob_enabled = args.name.is_some();

    let regex_name =
        regex::build_regex_set(args.regex.as_deref(), args.case_insensitive)?;
    let regex_enabled = args.regex.is_some();

    // -path/-wholename: glob over the full path. globset's default lets `*`
    // cross `/`, matching find's -path semantics (file_name() glob never sees
    // a separator, so --name is unaffected).
    let glob_path = glob::build_glob_set(
        args.path_glob.as_deref(),
        args.case_insensitive,
    )?;
    let path_glob_enabled = args.path_glob.is_some();

    // -lname: glob over a symlink's target (matched after a readlink).
    let glob_lname =
        glob::build_glob_set(args.lname.as_deref(), args.case_insensitive)?;
    let lname_enabled = args.lname.is_some();
    let access = args.access;

    // built here so a bad glob errors before the walk; applied in the walker
    // (where a matched dir can be pruned)
    let exclude_set =
        glob::build_glob_set(args.exclude.as_deref(), args.case_insensitive)?;
    let exclude = args.exclude.is_some().then_some(&exclude_set);

    let predicates = &args.meta;
    let meta_active = predicates.is_active();
    let meta_mask = predicates.mask();
    let now = meta::now_secs();

    let (tx, rx) = bounded::<Vec<Entry>>(CHAN_MULT * (args.threads - 1));

    // NUL for --null (xargs -0 / find -print0), else newline
    let separator: u8 = if args.null { b'\0' } else { b'\n' };

    // result cap (None/0 = unlimited); the sole writer enforces it
    let max_results = args.max_results.filter(|&n| n > 0);

    let print_thread = thread::spawn(move || {
        // the BufWriter coalesces, so write paths in directly (no scratch)
        let mut stdout = BufWriter::with_capacity(256 * 1024, make_out());
        let mut written: usize = 0;

        'outer: for batch in rx {
            for entry in batch {
                #[cfg(unix)]
                stdout
                    .write_all(entry.path.as_os_str().as_bytes())
                    .unwrap_or(());
                #[cfg(not(unix))]
                stdout
                    .write_all(entry.path.to_string_lossy().as_bytes())
                    .unwrap_or(());
                stdout.write_all(&[separator]).unwrap_or(());

                // dropping `rx` on the Nth result closes the channel; the
                // walkers observe that as WalkState::Quit
                written += 1;
                if max_results.is_some_and(|n| written >= n) {
                    break 'outer;
                }
            }
        }

        stdout.flush().unwrap_or(());
    });

    // dedup roots
    let unique_paths: Vec<&Path> =
        args.path.iter().map(PathBuf::as_path).unique().collect();
    let filetype_proto = filetype::FileType::new(&args.file_type);

    // root = depth 0; gated in the visitor so shallower levels still descend
    let min_depth = args.min_depth.unwrap_or(0);

    // None/0 → unlimited (no limiter built, hot path unchanged)
    let limiter =
        args.max_scan_rate.and_then(NonZeroU32::new).map(Limiter::new);

    // Every per-entry filter runs in the walker threads, cheapest first, so
    // statx is reached only after type/name/regex have kept the entry.
    walk::walk_parallel(
        args,
        &unique_paths,
        limiter.as_ref(),
        exclude,
        || {
            let filetype = filetype_proto;
            let shutdown = Arc::clone(&shutdown);
            // reborrow so the move-visitor captures `&GlobSet`, not copies
            let glob_name = &glob_name;
            let regex_name = &regex_name;
            let glob_path = &glob_path;
            let glob_lname = &glob_lname;
            // per-thread memo for -nouser/-nogroup reverse lookups
            #[cfg(unix)]
            let mut nss = meta::NssCache::default();
            let mut batch = BatchSender::new(tx.clone());
            move |entry: Entry, stat: &walk::StatAt| {
                if shutdown.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                // --min-depth: suppress shallow entries (descent continues).
                if entry.depth < min_depth {
                    return WalkState::Continue;
                }
                if filetype.ignore_filetype(entry.file_type, &entry.path) {
                    return WalkState::Continue;
                }
                if glob_enabled && !glob_name.is_match(entry.file_name()) {
                    return WalkState::Continue;
                }
                // regex matches the full path; glob only the file name
                if regex_enabled
                    && !regex_name.is_match(&regex::path_to_bytes(&entry.path))
                {
                    return WalkState::Continue;
                }
                // -path/-wholename
                if path_glob_enabled && !glob_path.is_match(&entry.path) {
                    return WalkState::Continue;
                }
                // -lname: a symlink's target (non-symlinks never match)
                if lname_enabled {
                    if entry.file_type != filetype::EntryType::Symlink {
                        return WalkState::Continue;
                    }
                    match stat.readlink() {
                        Some(t) if glob_lname.is_match(Path::new(&t)) => {}
                        _ => return WalkState::Continue,
                    }
                }
                // stat-based predicates (lazy); unstattable → skipped, like find
                if meta_active {
                    let Ok(m) = stat.fetch(meta_mask) else {
                        return WalkState::Continue;
                    };
                    if !predicates.matches(&m, now) {
                        return WalkState::Continue;
                    }
                    // -nouser/-nogroup: reject when the id *does* resolve
                    #[cfg(unix)]
                    {
                        if predicates.nouser && nss.user_exists(m.uid) {
                            return WalkState::Continue;
                        }
                        if predicates.nogroup && nss.group_exists(m.gid) {
                            return WalkState::Continue;
                        }
                    }
                }
                // -readable/-writable/-executable (faccessat, real uid/gid)
                if access != 0 && !stat.access(access) {
                    return WalkState::Continue;
                }
                // stop walking once the output channel closes
                if !batch.push(entry) {
                    return WalkState::Quit;
                }
                WalkState::Continue
            }
        },
    );

    drop(tx);
    print_thread.join().unwrap();

    Ok(())
}

/// Raises the soft `RLIMIT_NOFILE` to the hard limit, giving the walker
/// headroom for its pinned-parent-fd frontier (≈ O(workers × depth)), as
/// `find`/`fd` do. Best-effort; returns the resulting soft limit (`None` =
/// unlimited).
#[cfg(unix)]
pub fn raise_nofile_limit() -> Option<u64> {
    use rustix::process::{getrlimit, setrlimit, Resource};
    let mut lim = getrlimit(Resource::Nofile);
    if lim.current != lim.maximum {
        lim.current = lim.maximum;
        let _ = setrlimit(Resource::Nofile, lim);
    }
    getrlimit(Resource::Nofile).current
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn raise_nofile_limit_never_lowers_soft() {
        use rustix::process::{getrlimit, Resource};
        let before = getrlimit(Resource::Nofile).current;
        let after = super::raise_nofile_limit();
        // soft limit must never decrease; None means unlimited (either it was
        // already, or we raised it there) — both acceptable.
        if let (Some(b), Some(a)) = (before, after) {
            assert!(a >= b, "soft limit dropped: {a} < {b}");
        }
    }
}

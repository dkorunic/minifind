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

    let (tx, rx) = bounded::<Vec<Entry>>(CHAN_MULT * (args.threads - 1));

    let print_thread = thread::spawn(move || {
        // write straight into the BufWriter (it coalesces) — no scratch copy
        let mut stdout = BufWriter::with_capacity(256 * 1024, make_out());

        for batch in rx {
            for entry in batch {
                if glob_enabled && !glob_name.is_match(entry.file_name()) {
                    continue;
                }
                // regex matches the full path, glob only the file name
                if regex_enabled
                    && !regex_name.is_match(&regex::path_to_bytes(&entry.path))
                {
                    continue;
                }

                #[cfg(unix)]
                stdout
                    .write_all(entry.path.as_os_str().as_bytes())
                    .unwrap_or(());
                #[cfg(not(unix))]
                stdout
                    .write_all(entry.path.to_string_lossy().as_bytes())
                    .unwrap_or(());
                stdout.write_all(b"\n").unwrap_or(());
            }
        }

        stdout.flush().unwrap_or(());
    });

    // dedup roots (borrowed, no clone)
    let unique_paths: Vec<&Path> =
        args.path.iter().map(PathBuf::as_path).unique().collect();
    let filetype_proto = filetype::FileType::new(&args.file_type);

    // None or 0 IOPS => no limiter (unlimited); otherwise cap directory visits
    let limiter = args.max_iops.and_then(NonZeroU32::new).map(Limiter::new);

    walk::walk_parallel(args, &unique_paths, limiter.as_ref(), || {
        let filetype = filetype_proto;
        let shutdown = Arc::clone(&shutdown);
        let mut batch = BatchSender::new(tx.clone());
        move |entry: Entry| {
            if shutdown.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            if filetype.ignore_filetype(entry.file_type, &entry.path) {
                return WalkState::Continue;
            }
            // stop walking once the output channel closes
            if !batch.push(entry) {
                return WalkState::Quit;
            }
            WalkState::Continue
        }
    });

    drop(tx);
    print_thread.join().unwrap();

    Ok(())
}

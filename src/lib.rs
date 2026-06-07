//! `minifind` is a minimal, parallel `find(1)` reimplementation.
//!
//! The crate is split into a thin binary (`main.rs`) and this library so the
//! traversal pipeline can be driven through a public API and exercised by
//! integration tests. [`run`] spawns a dedicated output thread, walks every
//! requested path in parallel via [`ignore::WalkParallel`], filters entries by
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
use ignore::{DirEntry, WalkState};
use itertools::Itertools;
use std::io::{BufWriter, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

pub mod args;
pub mod filetype;
pub mod glob;
pub mod interrupt;
pub mod regex;
pub mod walk;

use args::Args;

/// Entries per channel message; batching amortizes the per-send atomic
/// synchronization. Tuned against a warm Linux-kernel tree: gains flatten by
/// 256, and larger batches only add RSS and latency-to-first-line.
const BATCH_SIZE: usize = 256;

/// Channel capacity in batches is `CHAN_MULT * (threads - 1)`; throughput is
/// insensitive to it across ~2..16.
const CHAN_MULT: usize = 4;

/// Per-walker-thread accumulator; sends [`DirEntry`] items in batches and
/// flushes the partial tail on `Drop` (when the visitor closure ends).
struct BatchSender {
    buf: Vec<DirEntry>,
    tx: Sender<Vec<DirEntry>>,
    cap: usize,
    closed: bool,
}

impl BatchSender {
    fn new(tx: Sender<Vec<DirEntry>>, cap: usize) -> Self {
        Self { buf: Vec::with_capacity(cap), tx, cap, closed: false }
    }

    /// Queues `entry`, flushing when full. Returns `false` once the channel
    /// has closed, signalling the caller to stop walking.
    fn push(&mut self, entry: DirEntry) -> bool {
        self.buf.push(entry);
        if self.buf.len() >= self.cap {
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
            std::mem::replace(&mut self.buf, Vec::with_capacity(self.cap));
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

    let (tx, rx) = bounded::<Vec<DirEntry>>(CHAN_MULT * (args.threads - 1));

    let print_thread = thread::spawn(move || {
        // write straight into the BufWriter (it coalesces) — no scratch copy
        let mut stdout = BufWriter::with_capacity(256 * 1024, make_out());

        for batch in rx {
            for dir_entry in batch {
                if glob_enabled && !glob_name.is_match(dir_entry.file_name()) {
                    continue;
                }

                // regex matches the full path, glob only the file name
                if regex_enabled
                    && !regex_name
                        .is_match(&regex::path_to_bytes(dir_entry.path()))
                {
                    continue;
                }

                #[cfg(unix)]
                stdout
                    .write_all(dir_entry.path().as_os_str().as_bytes())
                    .unwrap_or(());
                #[cfg(not(unix))]
                stdout
                    .write_all(dir_entry.path().to_string_lossy().as_bytes())
                    .unwrap_or(());
                stdout.write_all(b"\n").unwrap_or(());
            }
        }

        stdout.flush().unwrap_or(());
    });

    // dedup paths (borrowed, no clone)
    let unique_paths = args.path.iter().unique().collect::<Vec<&PathBuf>>();
    let walker = walk::build_walker(args, &unique_paths)?;
    let filetype_proto = filetype::FileType::new(&args.file_type);

    walker.run(|| {
        let filetype = filetype_proto;
        let shutdown = Arc::clone(&shutdown);
        let mut batch = BatchSender::new(tx.clone(), BATCH_SIZE);

        Box::new(move |dir_entry| {
            if let Ok(dir_entry) = dir_entry {
                if filetype.ignore_filetype(&dir_entry) {
                    return WalkState::Continue;
                }

                // stop walking once the output channel closes
                if !batch.push(dir_entry) {
                    return WalkState::Quit;
                }
            }

            if shutdown.load(Ordering::Relaxed) {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });

    drop(tx);
    print_thread.join().unwrap();

    Ok(())
}

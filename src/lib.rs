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
use crossbeam_channel::bounded;
use ignore::{DirEntry, WalkState};
use itertools::Itertools;
use std::io::{BufWriter, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub mod args;
pub mod filetype;
pub mod glob;
pub mod interrupt;
pub mod regex;
pub mod walk;

use args::Args;

/// Runs the minifind pipeline: spawns a dedicated output thread, walks every
/// requested path in parallel, filters entries by file type / glob / regex,
/// and writes matched paths to `make_out()`.
///
/// The writer is produced by `make_out` *inside* the output thread, which lets
/// the caller hand over a non-`Send` handle such as
/// [`std::io::StdoutLock`] without crossing a thread boundary.
///
/// # Arguments
///
/// * `args` - Parsed CLI arguments controlling traversal and filtering.
/// * `make_out` - Factory invoked once on the output thread to create the sink.
///
/// # Errors
///
/// Returns an error if signal handlers cannot be registered, if a glob/regex
/// pattern fails to compile, or if no traversal paths are available.
pub fn run<W, F>(args: &Args, make_out: F) -> Result<(), Error>
where
    W: Write,
    F: FnOnce() -> W + Send + 'static,
{
    let shutdown = Arc::new(AtomicBool::new(false));

    // interrupt handler
    interrupt::setup_interrupt_handler(&shutdown)?;

    // build name GlobSet
    let glob_name =
        glob::build_glob_set(args.name.as_deref(), args.case_insensitive)?;
    let glob_enabled = args.name.is_some();

    // build regex RegexSet
    let regex_name =
        regex::build_regex_set(args.regex.as_deref(), args.case_insensitive)?;
    let regex_enabled = args.regex.is_some();

    // output/print channel
    // "magic number" for size determined by performance testing
    let (tx, rx) = bounded::<DirEntry>(16 * (args.threads - 1));

    // output thread
    let print_thread = thread::spawn(move || {
        // use larger capacity buffer for less flush/write cycles
        let mut stdout = BufWriter::with_capacity(256 * 1024, make_out());
        // reusable scratch buffer: one write_all per entry, no per-entry alloc
        let mut line_buf: Vec<u8> = Vec::with_capacity(4096);

        for dir_entry in rx {
            // glob filename matching if --name option was provided
            if glob_enabled && !glob_name.is_match(dir_entry.file_name()) {
                continue;
            }

            // regex full path matching if --regex option was provided
            if regex_enabled
                && !regex_name
                    .is_match(&regex::path_to_bytes(dir_entry.path()))
            {
                continue;
            }

            // buffered output: path + newline in a single write_all
            line_buf.clear();
            #[cfg(unix)]
            line_buf
                .extend_from_slice(dir_entry.path().as_os_str().as_bytes());
            #[cfg(not(unix))]
            line_buf.extend_from_slice(
                dir_entry.path().to_string_lossy().as_bytes(),
            );
            line_buf.push(b'\n');
            stdout.write_all(&line_buf).unwrap_or(());
        }

        stdout.flush().unwrap_or(());
    });

    // deduplicate paths (borrow, no clone — build_walker is generic)
    let unique_paths = args.path.iter().unique().collect::<Vec<&PathBuf>>();

    // build ignore walkers for all paths specified
    let walker = walk::build_walker(args, &unique_paths)?;

    // walker threads
    let filetype_proto = filetype::FileType::new(&args.file_type);

    walker.run(|| {
        let tx = tx.clone();
        let filetype = filetype_proto;
        let shutdown = Arc::clone(&shutdown);

        Box::new(move |dir_entry| {
            if let Ok(dir_entry) = dir_entry {
                // check if filetype should be ignored
                if filetype.ignore_filetype(&dir_entry) {
                    return WalkState::Continue;
                }

                // on channel errors stop walking
                if tx.send(dir_entry).is_err() {
                    return WalkState::Quit;
                }
            }

            // on stop signals stop walking
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

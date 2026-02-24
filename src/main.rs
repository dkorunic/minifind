use anyhow::Error;
#[cfg(not(unix))]
use bstr::ByteVec;
use clap::Parser;
use crossbeam_channel::bounded;
use ignore::DirEntry;
use ignore::WalkState;
use itertools::Itertools;
use std::io;
use std::io::{BufWriter, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

mod args;
mod filetype;
mod glob;
mod interrupt;
mod regex;
mod walk;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Executes the main program logic, including setting up an interrupt handler, building a walker, and managing threads for output and walking directories.
///
/// # Returns
///
/// * `Result<(), Error>` - An `Ok` variant upon successful execution, or an `Err` variant if an error occurs during program execution.
fn main() -> Result<(), Error> {
    let args = args::Args::parse();
    let shutdown = &Arc::new(AtomicBool::new(false));

    // reset SIGPIPE signal handling
    interrupt::reset_sigpipe();

    // interrupt handler
    interrupt::setup_interrupt_handler(shutdown)?;

    // build name GlobSet
    let glob_name =
        glob::build_glob_set(args.name.as_ref(), args.case_insensitive)?;
    let glob_enabled = args.name.is_some();

    // build regex RegexSet
    let regex_name =
        regex::build_regex_set(args.regex.as_ref(), args.case_insensitive)?;
    let regex_enabled = args.regex.is_some();

    // output/print channel
    // "magic number" for size determined by performance testing
    let (tx, rx) = bounded::<DirEntry>(16 * (args.threads - 1));

    // output thread
    let print_thread = thread::spawn(move || {
        // use larger capacity buffer for less flush/write cycles
        let mut stdout =
            BufWriter::with_capacity(256 * 1024, io::stdout().lock());

        for dir_entry in rx {
            // glob filename matching if --name option was provided
            if glob_enabled && !glob_name.is_match(dir_entry.file_name()) {
                continue;
            }

            // regex full path matching if --regex option was provided
            if regex_enabled
                && !regex_name
                    .is_match(regex::path_to_bytes(&dir_entry.path()))
            {
                continue;
            }

            // buffered output
            #[cfg(unix)]
            {
                stdout
                    .write_all(dir_entry.path().as_os_str().as_bytes())
                    .unwrap_or(());
            }
            #[cfg(not(unix))]
            {
                stdout
                    .write_all(&Vec::from_path_lossy(dir_entry.path()))
                    .unwrap_or(());
            }
            stdout.write_all(b"\n").unwrap_or(());
        }

        stdout.flush().unwrap_or(());
    });

    // deduplicate paths
    let unique_paths =
        &args.path.iter().unique().cloned().collect::<Vec<PathBuf>>();

    // build ignore walkers for all paths specified
    let walker = walk::build_walker(&args, unique_paths);

    // walker threads
    let filetype_proto = filetype::FileType::new(&args.file_type);

    walker.run(|| {
        let tx = tx.clone();
        let filetype = filetype_proto;

        Box::new(move |dir_entry| {
            if let Ok(dir_entry) = dir_entry {
                // check if filetype should be ignored
                if filetype.ignore_filetype(&dir_entry) {
                    return WalkState::Continue;
                }

                // send to output/print channel
                match tx.send(dir_entry) {
                    Ok(()) => {}
                    Err(_) => {
                        // on channel errors stop walking
                        return WalkState::Quit;
                    }
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

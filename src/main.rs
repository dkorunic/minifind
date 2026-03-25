use anyhow::Error;
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
    let shutdown = Arc::new(AtomicBool::new(false));

    // reset SIGPIPE signal handling
    interrupt::reset_sigpipe();

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
        let mut stdout =
            BufWriter::with_capacity(256 * 1024, io::stdout().lock());
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

    // deduplicate paths
    let unique_paths =
        args.path.iter().unique().cloned().collect::<Vec<PathBuf>>();

    // build ignore walkers for all paths specified
    let walker = walk::build_walker(&args, &unique_paths)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn make_args(path: PathBuf, file_type: Vec<args::FileType>) -> args::Args {
        args::Args {
            threads: 2,
            path: vec![path],
            follow_symlinks: false,
            one_filesystem: true,
            max_depth: None,
            name: None,
            regex: None,
            case_insensitive: false,
            file_type,
        }
    }

    /// Runs the same pipeline as `main()` and collects output paths.
    fn run_pipeline(
        all_paths: Vec<PathBuf>,
        name: Option<Vec<String>>,
        regex_pats: Option<Vec<String>>,
        file_types: Vec<args::FileType>,
    ) -> Vec<PathBuf> {
        let ci = false;
        let glob_name = glob::build_glob_set(name.as_deref(), ci).unwrap();
        let glob_enabled = name.is_some();
        let regex_name =
            regex::build_regex_set(regex_pats.as_deref(), ci).unwrap();
        let regex_enabled = regex_pats.is_some();

        let mut a = make_args(all_paths[0].clone(), file_types);
        a.path = all_paths.clone();

        let (tx, rx) = crossbeam_channel::bounded::<DirEntry>(32);
        let results: Arc<Mutex<Vec<PathBuf>>> =
            Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&results);

        let print_thread = thread::spawn(move || {
            for e in rx {
                if glob_enabled && !glob_name.is_match(e.file_name()) {
                    continue;
                }
                if regex_enabled
                    && !regex_name.is_match(&regex::path_to_bytes(e.path()))
                {
                    continue;
                }
                sink.lock().unwrap().push(e.path().to_path_buf());
            }
        });

        // M4: deduplicate paths before walking
        let unique_paths =
            all_paths.iter().unique().cloned().collect::<Vec<_>>();
        let walker = walk::build_walker(&a, &unique_paths).unwrap();
        let ft = filetype::FileType::new(&a.file_type);

        walker.run(|| {
            let tx = tx.clone();
            let ft = ft;
            Box::new(move |entry| {
                if let Ok(e) = entry {
                    if ft.ignore_filetype(&e) {
                        // M3: must be Continue, not Skip
                        return WalkState::Continue;
                    }
                    if tx.send(e).is_err() {
                        return WalkState::Quit;
                    }
                }
                WalkState::Continue
            })
        });

        drop(tx);
        print_thread.join().unwrap();
        Arc::try_unwrap(results).unwrap().into_inner().unwrap()
    }

    // M1 — glob_enabled guard: without --name all entries must appear
    #[test]
    fn test_m1_no_name_flag_produces_output() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), b"x").unwrap();
        let results = run_pipeline(
            vec![tmp.path().to_path_buf()],
            None, // no --name
            None,
            vec![args::FileType::File, args::FileType::Directory],
        );
        assert!(
            results.iter().any(|p| p.ends_with("hello.txt")),
            "hello.txt must appear when no --name filter is active"
        );
    }

    // M2 — regex_enabled guard: without --regex all entries must appear
    #[test]
    fn test_m2_no_regex_flag_produces_output() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("hello.txt"), b"x").unwrap();
        let results = run_pipeline(
            vec![tmp.path().to_path_buf()],
            None,
            None, // no --regex
            vec![args::FileType::File, args::FileType::Directory],
        );
        assert!(
            results.iter().any(|p| p.ends_with("hello.txt")),
            "hello.txt must appear when no --regex filter is active"
        );
    }

    // M3 — filetype rejection uses Continue, not Skip:
    //      files inside a rejected directory must still appear
    #[test]
    fn test_m3_filetype_rejection_continues_into_subtree() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inner.txt"), b"x").unwrap();
        // File-only: directory entries are ignored but their children
        // must still be visited (Continue, not Skip).
        let results = run_pipeline(
            vec![tmp.path().to_path_buf()],
            None,
            None,
            vec![args::FileType::File],
        );
        assert!(
            results.iter().any(|p| p.ends_with("inner.txt")),
            "inner.txt must appear with --type file; \
             missing means directory was Skip-ped instead of Continue-d"
        );
    }

    // M4 — duplicate paths are deduplicated before walking
    #[test]
    fn test_m4_duplicate_paths_not_walked_twice() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("once.txt"), b"x").unwrap();
        let path = tmp.path().to_path_buf();
        // Same path supplied twice
        let results = run_pipeline(
            vec![path.clone(), path.clone()],
            None,
            None,
            vec![args::FileType::File],
        );
        let count = results.iter().filter(|p| p.ends_with("once.txt")).count();
        assert_eq!(
            count, 1,
            "once.txt must appear exactly once when same path is \
             given twice (deduplication must be active)"
        );
    }
}

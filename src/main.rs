use anyhow::Error;
use bstr::ByteVec;
use cfg_if::cfg_if;
use clap::Parser;
use crossbeam_channel::unbounded;
use ignore::DirEntry;
use ignore::WalkState;
use std::io;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

mod args;
mod interrupt;
mod walk;

cfg_if! {
    if #[cfg(all(target_os = "linux", target_arch = "x86_64"))] {
        use_jemalloc!();
    } else if #[cfg(all(target_os = "linux", target_arch = "aarch64"))] {
        use_jemalloc!();
    } else if #[cfg(target_os = "macos")] {
        use_jemalloc!();
    }
}

/// Executes the main program logic, including setting up an interrupt handler, building a walker, and managing threads for output and walking directories.
///
/// # Returns
///
/// * `Result<(), Error>` - An `Ok` variant upon successful execution, or an `Err` variant if an error occurs during program execution.
fn main() -> Result<(), Error> {
    let args = args::Args::parse();
    let shutdown = &Arc::new(AtomicBool::new(false));

    interrupt::reset_sigpipe();

    // interrupt handler
    interrupt::setup_interrupt_handler(shutdown)?;

    let walker = walk::build_walker(&args, &args.path);
    let (tx, rx) = unbounded::<DirEntry>();

    // output thread
    let print_thread = thread::spawn(move || {
        let mut stdout = BufWriter::new(io::stdout());

        for ent in rx {
            stdout.write_all(&Vec::from_path_lossy(ent.path())).unwrap_or(());
            stdout.write_all(b"\n").unwrap_or(());
        }

        stdout.flush().unwrap_or(());
    });

    // walker threads
    walker.run(|| {
        let tx = tx.clone();
        Box::new(move |dir_entry_result| {
            if let Ok(e) = dir_entry_result {
                match tx.send(e) {
                    Ok(()) => {}
                    Err(_) => {
                        return WalkState::Quit;
                    }
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

#[macro_export]
macro_rules! use_jemalloc {
    () => {
        use tikv_jemallocator::Jemalloc;

        #[global_allocator]
        static GLOBAL: Jemalloc = Jemalloc;
    };
}

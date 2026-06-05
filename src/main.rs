use anyhow::Error;
use clap::Parser;
use mimalloc::MiMalloc;
use std::io;

use minifind::args::Args;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Parses CLI arguments, restores default `SIGPIPE` handling, and runs the
/// minifind pipeline writing matched paths to locked stdout.
fn main() -> Result<(), Error> {
    let args = Args::parse();

    // reset SIGPIPE signal handling
    minifind::interrupt::reset_sigpipe();

    // lock stdout inside the output thread (StdoutLock is not Send)
    minifind::run(&args, || io::stdout().lock())
}

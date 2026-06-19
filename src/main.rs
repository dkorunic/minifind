// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

use anyhow::Error;
use mimalloc::MiMalloc;
use std::io;
use std::thread;

use minifind::args::Args;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Binary entry point: wires parsed CLI args into the library pipeline.
fn main() -> Result<(), Error> {
    let args = Args::parse();

    // honor the requested thread count, but warn past the core count
    let available =
        thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    if let Some(w) =
        minifind::args::oversubscription_warning(args.threads, available)
    {
        eprintln!("minifind: {w}");
    }

    minifind::interrupt::reset_sigpipe();

    // give the fd-anchored walker headroom for its pinned-parent-fd frontier
    #[cfg(unix)]
    let _ = minifind::raise_nofile_limit();

    // defer locking stdout to the output thread (StdoutLock is not Send)
    minifind::run(&args, || io::stdout().lock())
}

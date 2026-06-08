// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Error};
use signal_hook::consts::signal;
use signal_hook::flag::register;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(unix)]
const STOP_SIGNALS: &[i32] = &[
    signal::SIGINT,
    signal::SIGTERM,
    signal::SIGQUIT,
    signal::SIGHUP,
    signal::SIGUSR1,
    signal::SIGUSR2,
];

#[cfg(not(unix))]
const STOP_SIGNALS: &[i32] = &[signal::SIGTERM, signal::SIGINT];

/// Points every termination signal at `shutdown`, so the walker can stop
/// gracefully (it polls the flag) instead of being killed mid-traversal.
/// Errors if a handler cannot be registered.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::AtomicBool;
/// use minifind::interrupt::setup_interrupt_handler;
///
/// let shutdown = Arc::new(AtomicBool::new(false));
/// assert!(setup_interrupt_handler(&shutdown).is_ok());
/// ```
pub fn setup_interrupt_handler(
    shutdown: &Arc<AtomicBool>,
) -> Result<(), Error> {
    for sig in STOP_SIGNALS {
        let name =
            signal_hook::low_level::signal_name(*sig).unwrap_or_default();
        register(*sig, Arc::clone(shutdown)).with_context(|| {
            format!("Unable to register signal handler for {name}/{sig}")
        })?;
    }

    Ok(())
}

/// Restores default `SIGPIPE` disposition so a closed output pipe (e.g. piping
/// to `head`) terminates the process instead of surfacing as a write error.
#[cfg(unix)]
pub fn reset_sigpipe() {
    // SAFETY: async-signal-safe, and called once before any thread is spawned,
    // so there is no race on the process signal table.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub fn reset_sigpipe() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn test_setup_interrupt_handler_returns_ok() {
        let shutdown = Arc::new(AtomicBool::new(false));
        assert!(setup_interrupt_handler(&shutdown).is_ok());
    }

    #[test]
    fn test_setup_interrupt_handler_multiple_calls() {
        // signal_hook allows repeated registration; each call must succeed
        let s1 = Arc::new(AtomicBool::new(false));
        let s2 = Arc::new(AtomicBool::new(false));
        assert!(setup_interrupt_handler(&s1).is_ok());
        assert!(setup_interrupt_handler(&s2).is_ok());
    }

    #[test]
    fn test_reset_sigpipe_no_panic() {
        // Must not panic on any platform
        reset_sigpipe();
    }
}

use anyhow::{Context, Error};
use signal_hook::consts::signal;
use signal_hook::flag::register;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

/// Sets up interrupt handlers for termination signals and registers signal handlers for graceful shutdown.
///
/// # Arguments
///
/// * `shutdown` - A reference to an `AtomicBool` wrapped in an `Arc`, used to control the shutdown process.
///
/// # Returns
///
/// A `Result` indicating success or an `Error` if unable to register signal handlers.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, atomic::AtomicBool};
/// use anyhow::Error;
///
/// let shutdown = Arc::new(AtomicBool::new(false));
/// let result = setup_interrupt_handler(&shutdown);
/// assert!(result.is_ok());
/// ```
pub fn setup_interrupt_handler(
    shutdown: &Arc<AtomicBool>,
) -> Result<(), Error> {
    for sig in STOP_SIGNALS {
        let name =
            signal_hook::low_level::signal_name(*sig).unwrap_or_default();
        register(*sig, shutdown.clone()).with_context(|| {
            format!("Unable to register signal handler for {name}/{sig}")
        })?;
    }

    Ok(())
}

/// Resets the signal handling for SIGPIPE to the default behavior on Unix systems.
#[cfg(unix)]
pub fn reset_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub fn reset_sigpipe() {
    // no-op
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

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

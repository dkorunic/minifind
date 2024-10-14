use anyhow::{Context, Error};
use signal_hook::consts::signal;
use signal_hook::flag::register;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const STOP_SIGNALS: &[i32] = &[
    signal::SIGINT,
    signal::SIGTERM,
    signal::SIGQUIT,
    signal::SIGPIPE,
    signal::SIGHUP,
    signal::SIGUSR1,
    signal::SIGUSR2,
];

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

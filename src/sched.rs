// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! `--idle` scheduling (Linux): SCHED_IDLE + IOPRIO_CLASS_IDLE for the walker
//! pool (per worker) plus nice +19 process-wide, so a heavy walk yields the
//! CPU and disk to other work.

/// Moves the calling thread to SCHED_IDLE; called per walker so only the pool
/// is de-prioritized while the output thread keeps draining.
///
/// # Errors
///
/// The OS error from `sched_setscheduler` (de-escalation needs no privilege).
#[cfg(target_os = "linux")]
pub fn set_idle_cpu() -> std::io::Result<()> {
    // SAFETY: a zeroed sched_param is valid; SCHED_IDLE requires priority 0.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    param.sched_priority = 0;
    // SAFETY: pid 0 targets the calling thread.
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_IDLE, &param) };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Moves the calling thread to the IOPRIO_CLASS_IDLE I/O class (issues disk
/// I/O only when nothing else wants the device); called per walker beside
/// [`set_idle_cpu`].
///
/// # Errors
///
/// The OS error from `ioprio_set` (de-escalation needs no privilege).
#[cfg(target_os = "linux")]
pub fn set_idle_io() -> std::io::Result<()> {
    // ioprio_set has no libc wrapper, so use the raw syscall. The prio word
    // packs the class into the high bits: (class << 13) | level; IDLE ignores
    // the level. who = 0 targets the calling thread.
    const IOPRIO_WHO_PROCESS: libc::c_int = 1;
    const IOPRIO_CLASS_IDLE: libc::c_int = 3;
    const IOPRIO_CLASS_SHIFT: libc::c_int = 13;
    let prio: libc::c_int = IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT;
    // SAFETY: ioprio_set takes plain scalars; who = 0 targets this thread.
    let rc = unsafe {
        libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, prio)
    };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Lowers the whole process to nice +19; called once before workers spawn.
///
/// # Errors
///
/// The OS error from `setpriority` (raising niceness is always permitted).
#[cfg(target_os = "linux")]
pub fn lower_nice() -> std::io::Result<()> {
    // SAFETY: who=0 targets the calling process; the args are plain scalars.
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn set_idle_cpu_succeeds_and_restores() {
        let orig = unsafe { libc::sched_getscheduler(0) };
        assert!(set_idle_cpu().is_ok());
        // Restore so this libtest thread does not linger in SCHED_IDLE.
        let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
        param.sched_priority = 0;
        unsafe { libc::sched_setscheduler(0, orig.max(0), &param) };
    }

    #[test]
    fn set_idle_io_succeeds_and_restores() {
        const IOPRIO_WHO_PROCESS: libc::c_int = 1;
        let orig = unsafe {
            libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0)
        };
        assert!(set_idle_io().is_ok());
        // Restore so this libtest thread does not linger in the IDLE I/O class.
        if orig >= 0 {
            unsafe {
                libc::syscall(
                    libc::SYS_ioprio_set,
                    IOPRIO_WHO_PROCESS,
                    0,
                    orig as libc::c_int,
                );
            }
        }
    }
}

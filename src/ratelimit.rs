// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Global I/O rate limiter for the walker, wrapping `governor`.
//!
//! `governor`'s blocking `until_ready()` is async-only, so this exposes a
//! synchronous [`Limiter::acquire`] that loops `check()` and sleeps, staying
//! responsive to the walker's shutdown flag.

use governor::clock::{Clock, DefaultClock};
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Longest single sleep while throttled; bounds how soon a throttled worker
/// notices the shutdown flag.
const MAX_NAP: Duration = Duration::from_millis(100);

/// The concrete governor limiter we use: direct (un-keyed), in-memory, no
/// middleware. Aliased to keep the [`Limiter`] field readable and to avoid
/// clippy's `type_complexity` lint.
type DirectLimiter<C> = RateLimiter<
    NotKeyed,
    InMemoryState,
    C,
    NoOpMiddleware<<C as Clock>::Instant>,
>;

/// A direct (un-keyed) token-bucket limiter shared across walker threads.
pub struct Limiter<C: Clock = DefaultClock> {
    inner: DirectLimiter<C>,
    clock: C,
}

impl Limiter<DefaultClock> {
    /// Allows up to `rate` operations per second (burst capacity `rate`).
    #[must_use]
    pub fn new(rate: NonZeroU32) -> Self {
        Self::with_clock(rate, DefaultClock::default())
    }
}

impl<C: Clock + Clone> Limiter<C> {
    /// Like [`Limiter::new`] but with an explicit clock (tests inject
    /// `FakeRelativeClock`).
    pub fn with_clock(rate: NonZeroU32, clock: C) -> Self {
        let inner = RateLimiter::direct_with_clock(
            Quota::per_second(rate),
            clock.clone(),
        );
        Self { inner, clock }
    }

    /// Tries to spend one token without blocking; on refusal returns how long
    /// until one is available.
    pub fn try_acquire(&self) -> Result<(), Duration> {
        self.inner.check().map_err(|nu| nu.wait_time_from(self.clock.now()))
    }

    /// Spends one token, sleeping until one is available. Returns `false` if
    /// `quit` is set before a token is acquired.
    #[must_use]
    pub fn acquire(&self, quit: &AtomicBool) -> bool {
        loop {
            if quit.load(Ordering::Relaxed) {
                return false;
            }
            match self.try_acquire() {
                Ok(()) => return true,
                // wait may be zero (cell already due) → re-check; cap the nap
                // so shutdown stays responsive
                Err(wait) => thread::sleep(wait.min(MAX_NAP)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor::clock::FakeRelativeClock;

    fn lim(rate: u32, clock: FakeRelativeClock) -> Limiter<FakeRelativeClock> {
        Limiter::with_clock(NonZeroU32::new(rate).unwrap(), clock)
    }

    #[test]
    fn try_acquire_allows_burst_then_denies() {
        let clock = FakeRelativeClock::default();
        let l = lim(2, clock);
        assert!(l.try_acquire().is_ok());
        assert!(l.try_acquire().is_ok());
        assert!(l.try_acquire().is_err());
    }

    #[test]
    fn try_acquire_replenishes_after_time() {
        let clock = FakeRelativeClock::default();
        let l = lim(2, clock.clone());
        assert!(l.try_acquire().is_ok());
        assert!(l.try_acquire().is_ok());
        assert!(l.try_acquire().is_err());
        // at 2/sec, one cell replenishes after 500ms
        clock.advance(Duration::from_millis(500));
        assert!(l.try_acquire().is_ok());
        assert!(l.try_acquire().is_err());
    }

    #[test]
    fn acquire_returns_false_when_quit_already_set() {
        let clock = FakeRelativeClock::default();
        let l = lim(1, clock);
        let quit = AtomicBool::new(true);
        assert!(!l.acquire(&quit));
    }

    #[test]
    fn acquire_succeeds_immediately_when_token_available() {
        let clock = FakeRelativeClock::default();
        let l = lim(5, clock);
        let quit = AtomicBool::new(false);
        assert!(l.acquire(&quit));
    }
}

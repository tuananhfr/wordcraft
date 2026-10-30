//! When a refused edit may ask PI again: at most one sync a minute, doubling to an hour while the
//! refusal stays (the background schedule of lease.md §6).

use std::time::{Duration, Instant};

use erpcons_account::Error;

/// The schedule's bounds; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timing {
    pub min: Duration,
    pub max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { min: Duration::from_secs(60), max: Duration::from_secs(3600) }
    }
}

pub(super) struct Resync {
    pub timing: Timing,
    /// When the next sync for a refused edit may run.
    pub not_before: Instant,
    pub failures: u32,
}

impl Resync {
    pub fn new(timing: Timing, now: Instant) -> Self {
        Self { timing, not_before: now, failures: 0 }
    }

    /// After a sync: back off while it failed or editing stays locked for a reason a sync may lift.
    pub fn after(&mut self, now: Instant, result: &Result<(), Error>, still_locked: bool) {
        let delay = if still_locked || result.is_err() {
            let doubled = self.timing.min.saturating_mul(1 << self.failures.min(6)).min(self.timing.max);
            self.failures = self.failures.saturating_add(1);
            result.as_ref().err().and_then(Error::retry_after).map_or(doubled, |after| after.max(doubled))
        } else {
            self.failures = 0;
            self.timing.min
        };
        self.not_before = now.checked_add(delay).unwrap_or(now);
    }

    /// The user's Retry starts the schedule over.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use erpcons_account::Error;

    use super::{Resync, Timing};

    #[test]
    fn waits_a_minute_and_backs_off_to_an_hour_while_locked() {
        let now = Instant::now();
        let mut r = Resync::new(Timing::default(), now);
        let delays: Vec<u64> = (0..8)
            .map(|_| {
                r.after(now, &Ok(()), true);
                r.not_before.duration_since(now).as_secs()
            })
            .collect();
        assert_eq!(delays, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
        r.after(now, &Err(Error::Timeout), false);
        assert_eq!(r.not_before.duration_since(now), Duration::from_secs(3600));
        r.after(now, &Ok(()), false);
        assert_eq!(r.not_before.duration_since(now), Duration::from_secs(60), "a lifted lock resets the schedule");
        r.after(now, &Ok(()), true);
        r.reset();
        r.after(now, &Ok(()), true);
        assert_eq!(r.not_before.duration_since(now), Duration::from_secs(60), "Retry starts over");
    }
}

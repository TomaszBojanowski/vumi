//! Getting back online: when to try the server again after it could not be
//! reached, and one way for any part of the app to say that a request to the
//! server failed (the main view then asks the server once whether it is still
//! there).

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// How long to wait before attempt `attempt` after a failed one (0 is the
/// first): 5 s, 15 s, 30 s, 60 s, then every 2 minutes.
pub(crate) fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 => 5,
        1 => 15,
        2 => 30,
        3 => 60,
        _ => 120,
    })
}

/// Lets something happen at most once per `gap`.
#[derive(Default)]
pub struct Gate {
    last: Option<Instant>,
}

impl Gate {
    /// True, and remembers `now`, unless the last time was less than `gap` ago.
    pub(crate) fn allow(&mut self, now: Instant, gap: Duration) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < gap)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

thread_local! {
    static ON_FAILURE: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
}

/// Who hears about failed requests: the main view, which checks the connection.
pub(crate) fn set_failure_handler(handler: impl Fn() + 'static) {
    ON_FAILURE.with(|h| h.replace(Some(Box::new(handler))));
}

/// A request to the server failed (an album page, a stream): the connection
/// may be gone.
pub(crate) fn request_failed() {
    ON_FAILURE.with(|h| {
        if let Some(handler) = h.borrow().as_ref() {
            handler();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_come_after_5_15_30_and_60_seconds_then_every_two_minutes() {
        let delays: Vec<u64> = (0..7).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(delays, [5, 15, 30, 60, 120, 120, 120]);
        assert_eq!(retry_delay(u32::MAX).as_secs(), 120);
    }

    #[test]
    fn a_gate_lets_one_through_then_waits_for_the_gap() {
        let mut gate = Gate::default();
        let start = Instant::now();
        let gap = Duration::from_secs(10);
        assert!(gate.allow(start, gap));
        assert!(!gate.allow(start + Duration::from_secs(9), gap));
        assert!(gate.allow(start + Duration::from_secs(10), gap));
        // Counted from the last time it let one through.
        assert!(!gate.allow(start + Duration::from_secs(15), gap));
        assert!(gate.allow(start + Duration::from_secs(20), gap));
    }

    #[test]
    fn a_failed_request_reaches_the_handler() {
        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = count.clone();
        set_failure_handler(move || seen.set(seen.get() + 1));
        request_failed();
        request_failed();
        assert_eq!(count.get(), 2);
    }
}

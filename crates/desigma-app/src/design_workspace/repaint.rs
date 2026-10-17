//! Pure workspace scheduling, independently testable with `rustc --test`.
use std::time::{Duration, Instant};

pub(crate) const RENDER_ACK_TIMEOUT: Duration = Duration::from_secs(30);
const PROGRESS_POLL: Duration = Duration::from_millis(100);

fn acknowledgement_remaining(now: Instant, started: Option<Instant>) -> Option<Duration> {
    started.map(|at| RENDER_ACK_TIMEOUT.saturating_sub(now.saturating_duration_since(at)))
}

pub(crate) fn progress_delay(
    now: Instant,
    busy: bool,
    visible_pending: bool,
    render_started: Option<Instant>,
) -> Option<Duration> {
    // Helper replies and pending debounced work still need polling. WebView IPC
    // wakes egui independently, so its receipt needs only a deadline wakeup.
    let progress = (busy || visible_pending).then_some(PROGRESS_POLL);
    progress.into_iter().chain(acknowledgement_remaining(now, render_started)).min()
}

pub(crate) fn acknowledgement_expired(now: Instant, render_started: Option<Instant>) -> bool {
    acknowledgement_remaining(now, render_started) == Some(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_deadline_is_not_postponed_by_repeated_observations() {
        let started = Instant::now();
        for seconds in [0, 1, 15, 29, 30, 31] {
            let elapsed = Duration::from_secs(seconds);
            assert_eq!(progress_delay(started + elapsed, false, false, Some(started)),
                Some(RENDER_ACK_TIMEOUT.saturating_sub(elapsed)));
        }
    }

    #[test]
    fn busy_and_pending_work_use_the_earlier_progress_or_acknowledgement_delay() {
        let started = Instant::now();
        for (busy, pending) in [(true, false), (false, true), (true, true)] {
            assert_eq!(progress_delay(started, busy, pending, None), Some(PROGRESS_POLL));
            assert_eq!(progress_delay(started, busy, pending, Some(started)), Some(PROGRESS_POLL));
            assert_eq!(progress_delay(started + Duration::from_millis(29_950), busy, pending, Some(started)),
                Some(Duration::from_millis(50)));
            assert_eq!(progress_delay(started + RENDER_ACK_TIMEOUT, busy, pending, Some(started)),
                Some(Duration::ZERO));
        }
    }

    #[test]
    fn no_active_work_schedules_no_wakeup() {
        let now = Instant::now();
        assert_eq!(progress_delay(now, false, false, None), None);
        assert!(!acknowledgement_expired(now, None));
    }

    #[test]
    fn acknowledgement_expires_at_the_exact_original_deadline() {
        let started = Instant::now();
        assert!(!acknowledgement_expired(started + RENDER_ACK_TIMEOUT - Duration::from_nanos(1), Some(started)));
        assert!(acknowledgement_expired(started + RENDER_ACK_TIMEOUT, Some(started)));
        assert!(acknowledgement_expired(started + RENDER_ACK_TIMEOUT + Duration::from_secs(1), Some(started)));
    }

    #[test]
    fn observation_before_the_start_never_underflows_or_expires() {
        let observed = Instant::now();
        let started = observed + Duration::from_secs(1);
        assert_eq!(progress_delay(observed, false, false, Some(started)), Some(RENDER_ACK_TIMEOUT));
        assert!(!acknowledgement_expired(observed, Some(started)));
    }
}

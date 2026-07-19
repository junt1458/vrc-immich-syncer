//! OS toast notifications via `notify-rust`, plus a small rate-limiting
//! wrapper so repeated upload failures don't spam the user.
//!
//! Wiring these functions to real upload failure/recovery events is M5's
//! job (it needs the uploader's `SyncEvent` stream). For M4 the functions
//! are implemented and independently testable/demoable via the Settings
//! tab's "Send test notification" button.

use std::time::{Duration, Instant};

const APP_NAME: &str = "vrc-immich-syncer";

/// Shows a toast notifying the user that uploads have been failing.
pub fn notify_upload_failure(count: usize) {
    let body = if count == 1 {
        "1 upload failed. It will be retried automatically.".to_string()
    } else {
        format!("{count} uploads failed. They will be retried automatically.")
    };
    send_notification("Upload failed", &body);
}

/// Shows a toast notifying the user that previously failing uploads have
/// recovered (the failed queue has drained).
pub fn notify_recovered() {
    send_notification(APP_NAME, "Uploads have recovered and are syncing normally.");
}

/// Shows a plain test toast, used by the Settings tab's manual test button
/// so notification delivery can be exercised without waiting for M5's real
/// event wiring.
pub fn notify_test() {
    send_notification(
        APP_NAME,
        "This is a test notification from vrc-immich-syncer.",
    );
}

fn send_notification(summary: &str, body: &str) {
    let result = notify_rust::Notification::new()
        .summary(summary)
        .body(body)
        .appname(APP_NAME)
        .show();

    if let Err(err) = result {
        tracing::warn!("failed to show notification: {err}");
    }
}

/// Rate-limits a repeated notification (e.g. "upload failed") to at most one
/// per `min_interval`. Pure logic, injectable clock via [`RateLimiter::check`]
/// taking an explicit `Instant`, so it is unit-testable without sleeping.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    min_interval: Duration,
    last_sent: Option<Instant>,
}

impl RateLimiter {
    /// Creates a rate limiter that allows at most one notification per
    /// `min_interval`.
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_sent: None,
        }
    }

    /// Returns `true` (and records `now` as the last-sent time) if a
    /// notification should be sent now, i.e. either none has been sent yet
    /// or `min_interval` has elapsed since the last one.
    pub fn should_send(&mut self, now: Instant) -> bool {
        let allowed = match self.last_sent {
            None => true,
            Some(last) => now.duration_since(last) >= self.min_interval,
        };
        if allowed {
            self.last_sent = Some(now);
        }
        allowed
    }

    /// Resets the limiter so the next call to [`Self::should_send`] is
    /// always allowed (e.g. call this on recovery so the next failure batch
    /// is reported promptly).
    ///
    /// Returns `true` if a notification had actually been sent since the
    /// last reset (i.e. this reset corresponds to a real recovery, not a
    /// no-op), so callers can decide whether to show a "recovered" toast.
    pub fn reset(&mut self) -> bool {
        let had_sent = self.last_sent.is_some();
        self.last_sent = None;
        had_sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_always_allowed() {
        let mut limiter = RateLimiter::new(Duration::from_secs(60));
        assert!(limiter.should_send(Instant::now()));
    }

    #[test]
    fn second_call_within_window_is_blocked() {
        let mut limiter = RateLimiter::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.should_send(t0));
        assert!(!limiter.should_send(t0 + Duration::from_secs(30)));
    }

    #[test]
    fn call_after_window_is_allowed() {
        let mut limiter = RateLimiter::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.should_send(t0));
        assert!(limiter.should_send(t0 + Duration::from_secs(61)));
    }

    #[test]
    fn call_exactly_at_window_boundary_is_allowed() {
        let mut limiter = RateLimiter::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.should_send(t0));
        assert!(limiter.should_send(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn reset_allows_immediate_resend() {
        let mut limiter = RateLimiter::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.should_send(t0));
        assert!(
            limiter.reset(),
            "a notification had been sent, so this is a real recovery"
        );
        assert!(limiter.should_send(t0 + Duration::from_secs(1)));
    }

    #[test]
    fn reset_reports_whether_anything_had_been_sent() {
        let mut limiter = RateLimiter::new(Duration::from_secs(60));
        assert!(
            !limiter.reset(),
            "nothing was sent yet, so this is not a recovery"
        );
    }
}

use std::time::{Duration, Instant};

/// Returns an `Instant` `d` in the past, saturating at "now".
///
/// `Instant::now() - d` panics ("overflow when subtracting duration from
/// instant") when the process starts within `d` of system boot, because the
/// monotonic clock has not yet advanced past `d`. This is exactly the boot path
/// where the service worker launches seconds after startup, so the naive
/// subtraction crashes the worker in a relaunch loop. Saturate instead.
pub fn stale(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_sub(d).unwrap_or(now)
}

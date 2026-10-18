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

#[cfg(target_os = "linux")]
pub fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as libc::time_t);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&now, &mut tm);
    }
    (
        (tm.tm_year + 1900) as u16,
        (tm.tm_mon + 1) as u16,
        tm.tm_mday as u16,
        tm.tm_hour as u16,
        tm.tm_min as u16,
        tm.tm_sec as u16,
    )
}

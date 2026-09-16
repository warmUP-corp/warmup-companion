//! Motion language for the on-screen keyboard: one place for every duration,
//! curve and scale the renderer animates with, so the values are deliberate and
//! consistent instead of scattered magic numbers.
//!
//! Rules of thumb these tokens follow:
//! - Typing is a high-frequency interaction, so feedback is *instant* and only
//!   the release settles (never a delay before the key fires).
//! - Enters are ease-out and stay well under 300 ms; exits are shorter than
//!   enters or, when motion adds no information, instant.
//! - Nothing scales from 0; press feedback dips to 0.96, never below 0.95.
//!
//! Platform-neutral so the maths is unit-tested on every host.

/// Scale a key fills to at the instant it fires; it eases back to 1.0.
pub const PRESS_SCALE: f32 = 0.96;
/// Time for a pressed key to settle back to full size.
pub const PRESS_FEEDBACK_MS: f32 = 150.0;

/// Suggestion-strip entrance: opacity 0→1 with a small rise from below.
pub const STRIP_ENTER_MS: f32 = 120.0;
/// Rise distance (px) the strip travels while fading in.
pub const STRIP_ENTER_RISE_PX: f32 = 4.0;

/// Starts fast, lands soft. The default curve for anything entering or settling.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Normalised progress of an animation `elapsed_ms` into a `duration_ms` run.
fn progress(elapsed_ms: f32, duration_ms: f32) -> f32 {
    if duration_ms <= 0.0 {
        return 1.0;
    }
    (elapsed_ms / duration_ms).clamp(0.0, 1.0)
}

/// Uniform scale for a key `elapsed_ms` after it fired: dips to
/// [`PRESS_SCALE`] on the press frame and eases back to `1.0` by
/// [`PRESS_FEEDBACK_MS`]. Re-firing (auto-repeat) restarts the dip.
pub fn press_scale(elapsed_ms: f32) -> f32 {
    let t = ease_out_cubic(progress(elapsed_ms, PRESS_FEEDBACK_MS));
    PRESS_SCALE + (1.0 - PRESS_SCALE) * t
}

/// True while [`press_scale`] still differs from `1.0`, so callers can skip the
/// transform once the key has settled.
pub fn press_active(elapsed_ms: f32) -> bool {
    elapsed_ms < PRESS_FEEDBACK_MS
}

/// `(opacity, y_offset_px)` for the suggestion strip `elapsed_ms` after it
/// appeared: fades in while rising [`STRIP_ENTER_RISE_PX`] into place. Fully
/// settled returns `(1.0, 0.0)`.
pub fn strip_enter(elapsed_ms: f32) -> (f32, f32) {
    let t = ease_out_cubic(progress(elapsed_ms, STRIP_ENTER_MS));
    (t, STRIP_ENTER_RISE_PX * (1.0 - t))
}

/// Concentric nesting: a surface wrapping another rounded surface with `padding`
/// between them needs `inner_radius + padding` for its corners to run parallel.
pub fn concentric_radius(inner_radius: f32, padding: f32) -> f32 {
    inner_radius + padding.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_dips_on_the_press_frame_and_settles_to_full_size() {
        assert_eq!(press_scale(0.0), PRESS_SCALE);
        assert!(press_active(0.0));
        // Ease-out: most of the recovery happens early.
        let mid = press_scale(PRESS_FEEDBACK_MS * 0.5);
        assert!(
            mid > 0.99,
            "half-way should already be near full size: {mid}"
        );
        assert!(mid < 1.0);
        assert_eq!(press_scale(PRESS_FEEDBACK_MS), 1.0);
        assert_eq!(press_scale(PRESS_FEEDBACK_MS * 3.0), 1.0);
        assert!(!press_active(PRESS_FEEDBACK_MS));
    }

    #[test]
    fn press_scale_never_exaggerates() {
        // A dip below 0.95 reads as a squash, not a tap.
        assert!(PRESS_SCALE >= 0.95 && PRESS_SCALE < 1.0);
        for ms in [0.0, 10.0, 40.0, 90.0, 149.0] {
            let s = press_scale(ms);
            assert!((PRESS_SCALE..=1.0).contains(&s), "{ms}ms -> {s}");
        }
    }

    #[test]
    fn strip_enters_by_fading_up_and_stays_put_once_settled() {
        let (a0, dy0) = strip_enter(0.0);
        assert_eq!(a0, 0.0);
        assert_eq!(dy0, STRIP_ENTER_RISE_PX);
        let (a_mid, dy_mid) = strip_enter(STRIP_ENTER_MS * 0.5);
        assert!(a_mid > 0.5 && a_mid < 1.0);
        assert!(dy_mid > 0.0 && dy_mid < STRIP_ENTER_RISE_PX);
        assert_eq!(strip_enter(STRIP_ENTER_MS), (1.0, 0.0));
        assert_eq!(strip_enter(f32::MAX), (1.0, 0.0));
    }

    #[test]
    fn durations_stay_in_the_ui_band() {
        // Press feedback 100–160 ms; strip entrance ≤ 150 ms for a
        // per-word (high-frequency) surface.
        assert!((100.0..=160.0).contains(&PRESS_FEEDBACK_MS));
        assert!(STRIP_ENTER_MS <= 150.0);
    }

    #[test]
    fn ease_out_is_monotonic_and_clamped() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        let mut last = 0.0;
        for i in 0..=20 {
            let v = ease_out_cubic(i as f32 / 20.0);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn concentric_radius_adds_the_padding() {
        assert_eq!(concentric_radius(6.8, 18.0), 24.8);
        assert_eq!(concentric_radius(20.0, 0.0), 20.0);
        assert_eq!(concentric_radius(20.0, -5.0), 20.0);
    }
}

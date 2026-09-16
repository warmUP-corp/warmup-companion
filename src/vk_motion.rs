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

/// Dictation pill entrance: opacity 0→1 with a scale from [`VOICE_ENTER_SCALE`].
/// R3 starts dictation a handful of times a day, so this stays short.
pub const VOICE_ENTER_MS: f32 = 160.0;
pub const VOICE_ENTER_SCALE: f32 = 0.95;
/// Dictation pill exit: a plain fade, shorter than the entrance.
pub const VOICE_EXIT_MS: f32 = 120.0;
/// Phase label swap (Starting → Listening → Transcribing): new label fades in.
pub const VOICE_LABEL_FADE_MS: f32 = 150.0;

/// `(opacity, scale)` for the dictation pill `elapsed_ms` after it appeared.
pub fn voice_enter(elapsed_ms: f32) -> (f32, f32) {
    let t = ease_out_cubic(progress(elapsed_ms, VOICE_ENTER_MS));
    (t, VOICE_ENTER_SCALE + (1.0 - VOICE_ENTER_SCALE) * t)
}

/// Opacity for the dictation pill `elapsed_ms` into its exit fade.
pub fn voice_exit(elapsed_ms: f32) -> f32 {
    1.0 - ease_out_cubic(progress(elapsed_ms, VOICE_EXIT_MS))
}

/// Opacity of a freshly swapped phase label `elapsed_ms` after the swap.
pub fn voice_label_fade(elapsed_ms: f32) -> f32 {
    ease_out_cubic(progress(elapsed_ms, VOICE_LABEL_FADE_MS))
}

/// Mic-level envelope: react to speech almost at once, let go slowly, so the orb
/// jumps with the voice and breathes out instead of flickering with each 50 ms
/// level sample.
pub const LEVEL_ATTACK_MS: f32 = 40.0;
pub const LEVEL_RELEASE_MS: f32 = 180.0;

/// Move `current` toward `target` by one frame of `dt_ms`, with a fast attack
/// and slow release. Frame-rate independent.
pub fn smooth_level(current: f32, target: f32, dt_ms: f32) -> f32 {
    let target = target.clamp(0.0, 1.0);
    if dt_ms <= 0.0 {
        return current;
    }
    let tau = if target > current {
        LEVEL_ATTACK_MS
    } else {
        LEVEL_RELEASE_MS
    };
    let k = 1.0 - (-dt_ms / tau).exp();
    (current + (target - current) * k).clamp(0.0, 1.0)
}

/// Indeterminate "transcribing" pulse rate. Brisker than a lazy breath: the
/// same wait feels shorter when the indicator moves with purpose.
pub const TRANSCRIBE_PULSE_HZ: f32 = 1.6;

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
    fn voice_pill_enters_from_a_visible_size_and_exits_faster() {
        let (a0, s0) = voice_enter(0.0);
        assert_eq!(a0, 0.0);
        // Nothing appears from scale(0).
        assert_eq!(s0, VOICE_ENTER_SCALE);
        assert!(VOICE_ENTER_SCALE >= 0.9);
        assert_eq!(voice_enter(VOICE_ENTER_MS), (1.0, 1.0));
        assert_eq!(voice_exit(0.0), 1.0);
        assert_eq!(voice_exit(VOICE_EXIT_MS), 0.0);
        assert!(VOICE_EXIT_MS < VOICE_ENTER_MS);
        assert!(VOICE_ENTER_MS <= 300.0);
        assert_eq!(voice_label_fade(0.0), 0.0);
        assert_eq!(voice_label_fade(VOICE_LABEL_FADE_MS), 1.0);
    }

    #[test]
    fn level_envelope_attacks_fast_and_releases_slow() {
        // One 16 ms frame toward a loud target covers most of the distance...
        let up = smooth_level(0.0, 1.0, 16.0);
        assert!(up > 0.3, "attack too slow: {up}");
        // ...while the same frame toward silence only lets go a little.
        let down = smooth_level(1.0, 0.0, 16.0);
        assert!(down > 0.9, "release too fast: {down}");
        assert!(LEVEL_RELEASE_MS > LEVEL_ATTACK_MS * 3.0);
        // Converges and stays in range.
        let mut v = 0.0;
        for _ in 0..60 {
            v = smooth_level(v, 0.7, 16.0);
        }
        assert!((v - 0.7).abs() < 0.01);
        assert_eq!(smooth_level(0.5, 0.8, 0.0), 0.5);
        assert!(smooth_level(0.5, 7.0, 16.0) <= 1.0);
    }

    #[test]
    fn concentric_radius_adds_the_padding() {
        assert_eq!(concentric_radius(6.8, 18.0), 24.8);
        assert_eq!(concentric_radius(20.0, 0.0), 20.0);
        assert_eq!(concentric_radius(20.0, -5.0), 20.0);
    }
}

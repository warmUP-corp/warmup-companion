//! Lightbar animation state and ~30 Hz render loop (config / one-shot LED frames).

use crate::gamepad_backend::PadCommand;
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

/// Lightbar animation the companion drives. Mirrors the desktop `ledEffect` vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LedEffect {
    Solid,
    Breathing,
    Rainbow,
    Gradient,
    Off,
}

impl LedEffect {
    fn parse(s: &str) -> Self {
        match s {
            "off" => Self::Off,
            "breathing" => Self::Breathing,
            "rainbow" => Self::Rainbow,
            "gradient" => Self::Gradient,
            _ => Self::Solid,
        }
    }
}

/// Desired lightbar state, set from `config` frames and rendered by the LED engine thread.
#[derive(Clone, Copy)]
pub(crate) struct LedState {
    pub(crate) effect: LedEffect,
    /// Base colour (true RGB channels).
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
    /// Secondary colour for `gradient` effect.
    pub(crate) r2: u8,
    pub(crate) g2: u8,
    pub(crate) b2: u8,
    /// 0.0–1.0 brightness multiplier.
    pub(crate) brightness: f32,
}

impl Default for LedState {
    fn default() -> Self {
        // warmUP primary #b6a0ff.
        Self {
            effect: LedEffect::Solid,
            r: 0xb6,
            g: 0xa0,
            b: 0xff,
            r2: 0x4c,
            g2: 0x7b,
            b2: 0x99,
            brightness: 1.0,
        }
    }
}

static LED_STATE: OnceLock<Mutex<LedState>> = OnceLock::new();
static LED_ENGINE: Once = Once::new();

pub(crate) fn led_state() -> &'static Mutex<LedState> {
    LED_STATE.get_or_init(|| Mutex::new(LedState::default()))
}

fn scale_channel(c: u8, f: f32) -> u8 {
    (c as f32 * f).round().clamp(0.0, 255.0) as u8
}

pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        scale_channel(255, r + m),
        scale_channel(255, g + m),
        scale_channel(255, b + m),
    )
}

pub(crate) fn blend_channel(a: u8, b: u8, t: f32, brightness: f32) -> u8 {
    let c = a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0);
    scale_channel(c.round() as u8, brightness)
}

/// Effective lightbar colour for `state` at elapsed time `t` seconds.
pub(crate) fn led_color_at(state: &LedState, t: f32) -> (u8, u8, u8) {
    match state.effect {
        LedEffect::Off => (0, 0, 0),
        LedEffect::Solid => (
            scale_channel(state.r, state.brightness),
            scale_channel(state.g, state.brightness),
            scale_channel(state.b, state.brightness),
        ),
        LedEffect::Breathing => {
            // 0.15–1.0 sine envelope, ~3.6 s period.
            let env = 0.15 + 0.85 * (0.5 - 0.5 * (t * std::f32::consts::TAU / 3.6).cos());
            let f = state.brightness * env;
            (
                scale_channel(state.r, f),
                scale_channel(state.g, f),
                scale_channel(state.b, f),
            )
        }
        // ~6 s hue sweep; the base colour is replaced by the cycling hue.
        LedEffect::Rainbow => hsv_to_rgb((t / 6.0).fract(), 1.0, state.brightness),
        LedEffect::Gradient => {
            // Smoothly ping-pong between the two configured colours over ~5 seconds.
            let mix = 0.5 - 0.5 * (t * std::f32::consts::TAU / 5.0).cos();
            (
                blend_channel(state.r, state.r2, mix, state.brightness),
                blend_channel(state.g, state.g2, mix, state.brightness),
                blend_channel(state.b, state.b2, mix, state.brightness),
            )
        }
    }
}

/// Spawn the LED engine once. It re-renders the current [`LedState`] at ~30 Hz and pushes a
/// `Led` device command only when the colour changes, so static effects cost one command.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn ensure_led_engine() {
    LED_ENGINE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("warmup-led".into())
            .spawn(|| {
                let start = Instant::now();
                let mut last: Option<(u8, u8, u8)> = None;
                loop {
                    // Guard the body so a panic (push failure, future logic) can't
                    // silently kill the LED thread and freeze the lightbar until a
                    // service restart. Sleep stays outside, so a persistent panic
                    // paces at 33ms instead of busy-spinning.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let state = led_state().lock().map(|s| *s).unwrap_or_default();
                        let color = led_color_at(&state, start.elapsed().as_secs_f32());
                        if last != Some(color) {
                            crate::device_commands::push_device_command(PadCommand::Led {
                                r: color.0,
                                g: color.1,
                                b: color.2,
                            });
                            last = Some(color);
                        }
                    }));
                    std::thread::sleep(Duration::from_millis(33));
                }
            });
    });
}

/// Update the lightbar state from a `config` frame (colour / effect / brightness) and make
/// sure the engine is running. Absent fields keep the current value.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn apply_led_config(p: &crate::protocol::ConfigPayload) {
    if let Ok(mut st) = led_state().lock() {
        // `parse_theme_color` yields a Windows COLORREF (0x00BBGGRR); extract true RGB
        // channels (the earlier `>>16 = r` read swapped red and blue).
        if let Some(cref) = p
            .led_color
            .as_deref()
            .and_then(crate::config::parse_theme_color)
        {
            st.r = (cref & 0xff) as u8;
            st.g = ((cref >> 8) & 0xff) as u8;
            st.b = ((cref >> 16) & 0xff) as u8;
        }
        if let Some(cref) = p
            .led_secondary_color
            .as_deref()
            .and_then(crate::config::parse_theme_color)
        {
            st.r2 = (cref & 0xff) as u8;
            st.g2 = ((cref >> 8) & 0xff) as u8;
            st.b2 = ((cref >> 16) & 0xff) as u8;
        }
        if let Some(effect) = p.led_effect.as_deref() {
            st.effect = LedEffect::parse(effect);
        }
        if let Some(brightness) = p.led_brightness {
            st.brightness = brightness.clamp(0.0, 1.0);
        }
    }
    ensure_led_engine();
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn apply_led(p: &crate::protocol::LedPayload) {
    crate::install::log_line("pipe inbound led");
    let mut immediate = None;
    if let Ok(mut st) = led_state().lock() {
        match st.effect {
            LedEffect::Solid => {
                st.effect = LedEffect::Solid;
                st.r = p.r;
                st.g = p.g;
                st.b = p.b;
                immediate = Some(led_color_at(&st, 0.0));
            }
            LedEffect::Off => {
                immediate = Some((0, 0, 0));
            }
            LedEffect::Breathing | LedEffect::Gradient => {
                st.r = p.r;
                st.g = p.g;
                st.b = p.b;
            }
            LedEffect::Rainbow => {}
        }
    } else {
        immediate = Some((p.r, p.g, p.b));
    }
    if let Some(color) = immediate {
        crate::device_commands::push_device_command(PadCommand::Led {
            r: color.0,
            g: color.1,
            b: color.2,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad_backend::PadCommand;
    use crate::device_commands::drain_device_commands;
    use std::sync::{Mutex, OnceLock};

    static TEST_LED_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_led_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LED_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn test_config(
        led_color: Option<&str>,
        led_effect: Option<&str>,
        led_brightness: Option<f32>,
    ) -> crate::protocol::ConfigPayload {
        crate::protocol::ConfigPayload {
            deadzone: 0.15,
            sensitivity: 15.0,
            acceleration_exp: 2.0,
            scroll_sensitivity: 5.0,
            enabled: true,
            clicks_enabled: true,
            led_color: led_color.map(str::to_string),
            led_secondary_color: None,
            led_effect: led_effect.map(str::to_string),
            led_brightness,
            natural_scroll: false,
            cursor_smoothing: 0.0,
            keyboard_theme: None,
            vk_mode: None,
        }
    }

    #[test]
    fn led_config_updates_effect_color_and_brightness() {
        let _guard = test_led_lock();
        apply_led_config(&test_config(Some("#112233"), Some("breathing"), Some(0.5)));
        let st = *led_state().lock().unwrap();
        assert_eq!(st.effect, LedEffect::Breathing);
        assert_eq!((st.r, st.g, st.b), (0x11, 0x22, 0x33));
        assert_eq!(st.brightness, 0.5);
        assert_eq!(led_color_at(&st, 0.0), (1, 3, 4));
    }

    #[test]
    fn one_shot_led_does_not_flash_over_rainbow() {
        let _guard = test_led_lock();
        apply_led_config(&test_config(Some("#112233"), Some("rainbow"), Some(0.5)));
        drain_device_commands();
        let before = *led_state().lock().unwrap();
        apply_led(&crate::protocol::LedPayload {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        });
        let cmds = drain_device_commands();
        assert!(
            !cmds.iter().any(|cmd| matches!(
                cmd,
                PadCommand::Led {
                    r: 0x55,
                    g: 0x5e,
                    b: 0x66,
                }
            )),
            "one-shot LED command must not interleave steady color with animated engine output"
        );

        let after = *led_state().lock().unwrap();
        assert_eq!(after.effect, before.effect);
        assert_eq!((after.r, after.g, after.b), (before.r, before.g, before.b));
        assert_eq!(after.brightness, 0.5);
        assert_ne!(led_color_at(&after, 0.0), led_color_at(&after, 1.0));
    }

    #[test]
    fn one_shot_led_updates_breathing_base_without_direct_flash() {
        let _guard = test_led_lock();
        apply_led_config(&test_config(Some("#112233"), Some("breathing"), Some(0.5)));
        drain_device_commands();
        apply_led(&crate::protocol::LedPayload {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        });

        let cmds = drain_device_commands();
        assert!(
            !cmds.iter().any(|cmd| matches!(
                cmd,
                PadCommand::Led {
                    r: 0x55,
                    g: 0x5e,
                    b: 0x66,
                }
            )),
            "breathing engine should own animated LED output"
        );
        let st = *led_state().lock().unwrap();
        assert_eq!(st.effect, LedEffect::Breathing);
        assert_eq!((st.r, st.g, st.b), (0xaa, 0xbb, 0xcc));
        assert_eq!(st.brightness, 0.5);
    }

    #[test]
    fn one_shot_led_does_not_turn_off_effect_back_on() {
        let _guard = test_led_lock();
        apply_led_config(&test_config(Some("#112233"), Some("off"), Some(0.5)));
        drain_device_commands();
        apply_led(&crate::protocol::LedPayload {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        });

        let cmds = drain_device_commands();
        assert!(cmds
            .iter()
            .any(|cmd| matches!(cmd, PadCommand::Led { r: 0, g: 0, b: 0 })));
        let st = *led_state().lock().unwrap();
        assert_eq!(st.effect, LedEffect::Off);
        assert_eq!(led_color_at(&st, 0.0), (0, 0, 0));
    }

    #[test]
    fn gradient_cycles_between_primary_and_secondary_colors() {
        let _guard = test_led_lock();
        let mut p = test_config(Some("#000000"), Some("gradient"), Some(1.0));
        p.led_secondary_color = Some("#ffffff".into());
        apply_led_config(&p);

        let st = *led_state().lock().unwrap();
        assert_eq!(st.effect, LedEffect::Gradient);
        assert_eq!(led_color_at(&st, 0.0), (0x00, 0x00, 0x00));
        assert_eq!(led_color_at(&st, 2.5), (0xff, 0xff, 0xff));
        assert_eq!(led_color_at(&st, 5.0), (0x00, 0x00, 0x00));
    }
}

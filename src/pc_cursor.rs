//! OS mouse cursor from sticks (same math as warmUP `gamepad/cursor.rs`).
//!
//! Cursor model: relative-velocity
//! `MOUSEEVENTF_MOVE` from the left stick, and the action button drives a real
//! mouse-button HOLD (down on press-edge, up on release-edge,
//! not an instant click). Stick velocity is normalized to a 1080p reference
//! (`screenW/1920`, `screenH/1080`), so the feel
//! is resolution-independent.

use enigo::{Axis, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

const BASE_SPEED: f32 = 100.0;
const BASE_SCROLL_SPEED: f32 = 20.0;
const TOUCHPAD_PIXEL_SCALE: f32 = 1.5;

/// Reference resolution: sensitivity constants are tuned for
/// 1080p and scaled by `actual/reference` per axis (`FUN_00422dd0`).
const REF_WIDTH: f32 = 1920.0;
const REF_HEIGHT: f32 = 1080.0;

/// Per-axis velocity scale = actual screen size / 1080p reference.
/// 1.0 on a 1080p display; >1 on higher-res so the cursor crosses the screen in
/// the same physical stick-throw regardless of resolution. Falls back to 1.0
/// off Windows / if the metrics query fails.
fn screen_scale() -> (f32, f32) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 {
            return (w as f32 / REF_WIDTH, h as f32 / REF_HEIGHT);
        }
    }
    (1.0, 1.0)
}

pub struct PcCursor {
    enigo: Option<Enigo>,
    /// Service worker only: dedicated SendInput thread on the Default desktop.
    /// The worker main thread is attached to Winlogon, so injecting from it lands
    /// on the wrong desktop (invisible after login). The injector thread inherits
    /// the process startup desktop (`winsta0\default`), so its SendInput reaches
    /// the user's desktop with no SetThreadDesktop (which fails ERROR_BUSY on a
    /// thread that owns windows).
    injector: Option<Injector>,
    /// When set (service worker on the Winlogon/lock desktop), inject on the
    /// CALLING thread instead of the default-desktop injector. The gamepad loop
    /// thread is attached to "winlogon" there, so its `SendInput` lands on the
    /// secure desktop — where the native PIN keypad lives. The default-desktop
    /// injector would target `winsta0\default` (invisible on the lock screen).
    on_winlogon: bool,
    remainder_x: f32,
    remainder_y: f32,
    scroll_remainder_x: f32,
    scroll_remainder_y: f32,
    /// Resolution normalization: velocity * actual/1080p.
    scale_x: f32,
    scale_y: f32,
    /// Left mouse button currently held, edge-tracked.
    left_held: bool,
    /// Right mouse button currently held, edge-tracked.
    right_held: bool,
    /// EMA-smoothed cursor delta carried between frames (cursor_smoothing).
    smooth_dx: f32,
    smooth_dy: f32,
}

impl PcCursor {
    pub fn new() -> Result<Self, String> {
        let enigo =
            Enigo::new(&Settings::default()).map_err(|e| format!("enigo init failed: {e}"))?;
        Ok(Self::with_enigo(Some(enigo)))
    }

    /// Service worker (user session, not session 0). Holds BOTH an inline Enigo
    /// (used on the Winlogon desktop, where the calling loop thread is attached)
    /// and a dedicated Default-desktop injector thread (used post-login). The
    /// active path is chosen per-frame by [`set_on_winlogon`].
    pub fn new_service() -> Self {
        let (scale_x, scale_y) = screen_scale();
        Self {
            enigo: Enigo::new(&Settings::default()).ok(),
            injector: Injector::spawn(),
            on_winlogon: false,
            remainder_x: 0.0,
            remainder_y: 0.0,
            scroll_remainder_x: 0.0,
            scroll_remainder_y: 0.0,
            scale_x,
            scale_y,
            left_held: false,
            right_held: false,
            smooth_dx: 0.0,
            smooth_dy: 0.0,
        }
    }

    fn with_enigo(enigo: Option<Enigo>) -> Self {
        let (scale_x, scale_y) = screen_scale();
        Self {
            enigo,
            injector: None,
            on_winlogon: false,
            remainder_x: 0.0,
            remainder_y: 0.0,
            scroll_remainder_x: 0.0,
            scroll_remainder_y: 0.0,
            scale_x,
            scale_y,
            left_held: false,
            right_held: false,
            smooth_dx: 0.0,
            smooth_dy: 0.0,
        }
    }

    /// Published each poll by the service loop: true while the input desktop is
    /// Winlogon. Routes injection to the calling (winlogon-attached) thread.
    pub fn set_on_winlogon(&mut self, on_winlogon: bool) {
        self.on_winlogon = on_winlogon;
    }

    pub fn move_stick(&mut self, stick_x: f32, stick_y: f32, dt_secs: f32) {
        let settings = crate::config::gamepad_settings();
        let (dx, dy) = stick_delta(
            stick_x,
            stick_y,
            settings.cursor_deadzone,
            settings.cursor_speed,
            settings.cursor_accel,
            dt_secs,
        );
        // Normalize velocity to actual/1080p so the throw feels the same at any resolution.
        let (dx, dy) = (dx * self.scale_x, dy * self.scale_y);
        if dx == 0.0 && dy == 0.0 {
            self.remainder_x = 0.0;
            self.remainder_y = 0.0;
            // Snap smoothing state to rest so the cursor doesn't glide after release.
            self.smooth_dx = 0.0;
            self.smooth_dy = 0.0;
            return;
        }
        // EMA smoothing: higher factor = smoother but laggier. Clamped so the cursor
        // always keeps moving (a factor of 1.0 would freeze it).
        let s = settings.cursor_smoothing.clamp(0.0, 0.95);
        let (dx, dy) = if s > 0.0 {
            let sx = self.smooth_dx * s + dx * (1.0 - s);
            let sy = self.smooth_dy * s + dy * (1.0 - s);
            self.smooth_dx = sx;
            self.smooth_dy = sy;
            (sx, sy)
        } else {
            (dx, dy)
        };
        let total_x = dx + self.remainder_x;
        let total_y = dy + self.remainder_y;
        let int_x = total_x as i32;
        let int_y = total_y as i32;
        self.remainder_x = total_x - int_x as f32;
        self.remainder_y = total_y - int_y as f32;
        if int_x != 0 || int_y != 0 {
            self.dispatch(Cmd::Move(int_x, int_y));
            // Hint the warmUP desktop so its visual cursor tracks the OS cursor (#349).
            crate::pipe_server::publish_cursor_moved(int_x as f64, int_y as f64);
        }
    }

    pub fn scroll_stick(&mut self, stick_x: f32, stick_y: f32, dt_secs: f32) {
        let settings = crate::config::gamepad_settings();
        let (sx, sy) = scroll_delta(
            stick_x,
            stick_y,
            settings.scroll_deadzone,
            settings.scroll_speed,
            settings.scroll_accel,
            dt_secs,
        );
        if sx == 0.0 && sy == 0.0 {
            self.scroll_remainder_x = 0.0;
            self.scroll_remainder_y = 0.0;
            return;
        }
        let total_y = sy + self.scroll_remainder_y;
        let int_y = total_y as i32;
        self.scroll_remainder_y = total_y - int_y as f32;
        let total_x = sx + self.scroll_remainder_x;
        let int_x = total_x as i32;
        self.scroll_remainder_x = total_x - int_x as f32;
        // Default scrolls content opposite the stick; natural_scroll makes content
        // follow the stick (reverse direction).
        let v_sign = if settings.natural_scroll { 1 } else { -1 };
        if int_y != 0 {
            self.dispatch(Cmd::ScrollV(v_sign * int_y));
        }
        if int_x != 0 {
            self.dispatch(Cmd::ScrollH(int_x));
        }
    }

    pub fn move_touchpad(&mut self, delta: Option<(f32, f32)>) {
        let Some((dx, dy)) = delta else {
            return;
        };
        let dx = dx * REF_WIDTH * self.scale_x * TOUCHPAD_PIXEL_SCALE;
        let dy = dy * REF_HEIGHT * self.scale_y * TOUCHPAD_PIXEL_SCALE;
        let total_x = dx + self.remainder_x;
        let total_y = dy + self.remainder_y;
        let int_x = total_x as i32;
        let int_y = total_y as i32;
        self.remainder_x = total_x - int_x as f32;
        self.remainder_y = total_y - int_y as f32;
        if int_x != 0 || int_y != 0 {
            self.dispatch(Cmd::Move(int_x, int_y));
            crate::pipe_server::publish_cursor_moved(int_x as f64, int_y as f64);
        }
    }

    /// Drive the left mouse button as a real HOLD, edge-tracked
    /// (DOWN on press-edge, UP on release-edge) instead of an
    /// instant click. Giving XAML/LogonUI a genuine press duration is what makes
    /// the native PIN keypad register the tap reliably on the secure desktop.
    /// Idempotent: repeated same-state calls are dropped.
    pub fn set_left_button(&mut self, down: bool) {
        if down == self.left_held {
            return;
        }
        self.left_held = down;
        self.dispatch(if down { Cmd::ButtonDown } else { Cmd::ButtonUp });
    }

    /// Right mouse button as a real HOLD, edge-tracked (same shape as
    /// [`set_left_button`]). Idempotent.
    pub fn set_right_button(&mut self, down: bool) {
        if down == self.right_held {
            return;
        }
        self.right_held = down;
        self.dispatch(if down { Cmd::RButtonDown } else { Cmd::RButtonUp });
    }

    /// Tap Enter into the focused app, routed through the same desktop-correct
    /// dispatch the cursor uses (so it lands on the user's desktop post-login).
    pub fn tap_enter(&mut self) {
        self.dispatch(Cmd::EnterTap);
    }

    /// Route a command. On Winlogon: inline on the calling (winlogon-attached) loop
    /// thread so `SendInput` reaches the secure desktop. Otherwise: the Default-
    /// desktop injector thread (service post-login), else inline Enigo.
    fn dispatch(&mut self, cmd: Cmd) {
        if self.on_winlogon {
            if let Some(enigo) = self.enigo.as_mut() {
                apply_cmd(enigo, cmd);
            }
            return;
        }
        if let Some(injector) = &self.injector {
            injector.send(cmd);
            return;
        }
        if let Some(enigo) = self.enigo.as_mut() {
            apply_cmd(enigo, cmd);
        }
    }
}

/// Apply one cursor command through an Enigo instance (SendInput on the calling
/// thread's desktop). Shared by the inline path and the injector thread.
fn apply_cmd(enigo: &mut Enigo, cmd: Cmd) {
    match cmd {
        Cmd::Move(dx, dy) => {
            let _ = enigo.move_mouse(dx, dy, Coordinate::Rel);
        }
        Cmd::ScrollV(v) => {
            let _ = enigo.scroll(v, Axis::Vertical);
        }
        Cmd::ScrollH(h) => {
            let _ = enigo.scroll(h, Axis::Horizontal);
        }
        Cmd::ButtonDown => {
            let _ = enigo.button(enigo::Button::Left, Direction::Press);
        }
        Cmd::ButtonUp => {
            let _ = enigo.button(enigo::Button::Left, Direction::Release);
        }
        Cmd::RButtonDown => {
            let _ = enigo.button(enigo::Button::Right, Direction::Press);
        }
        Cmd::RButtonUp => {
            let _ = enigo.button(enigo::Button::Right, Direction::Release);
        }
        Cmd::EnterTap => {
            let _ = enigo.key(Key::Return, Direction::Click);
        }
    }
}

#[derive(Clone, Copy)]
enum Cmd {
    Move(i32, i32),
    ScrollV(i32),
    ScrollH(i32),
    ButtonDown,
    ButtonUp,
    RButtonDown,
    RButtonUp,
    EnterTap,
}

/// Dedicated cursor-injection thread. Owns its own Enigo and runs on whatever
/// desktop the thread was born on — for the service worker that is the process
/// startup desktop `winsta0\default`, i.e. the user's desktop after login.
struct Injector {
    tx: std::sync::mpsc::Sender<Cmd>,
}

impl Injector {
    fn spawn() -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("warmup-cursor-inject".into())
            .spawn(move || injector_main(rx))
            .ok()?;
        Some(Self { tx })
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn injector_main(rx: std::sync::mpsc::Receiver<Cmd>) {
    let mut enigo = Enigo::new(&Settings::default()).ok();
    while let Ok(cmd) = rx.recv() {
        let Some(enigo) = enigo.as_mut() else {
            continue;
        };
        apply_cmd(enigo, cmd);
    }
}

fn scroll_delta(
    stick_x: f32,
    stick_y: f32,
    deadzone: f32,
    sensitivity: f32,
    accel_exp: f32,
    dt_secs: f32,
) -> (f32, f32) {
    let magnitude = (stick_x * stick_x + stick_y * stick_y).sqrt();
    if magnitude < deadzone || magnitude == 0.0 {
        return (0.0, 0.0);
    }
    let denom = (1.0 - deadzone).max(1e-6);
    let effective = ((magnitude - deadzone) / denom).min(1.0);
    let accelerated = effective.powf(accel_exp);
    let norm_x = stick_x / magnitude;
    let norm_y = stick_y / magnitude;
    let speed = accelerated * sensitivity * BASE_SCROLL_SPEED * dt_secs;
    (norm_x * speed, norm_y * speed)
}

fn stick_delta(
    stick_x: f32,
    stick_y: f32,
    deadzone: f32,
    sensitivity: f32,
    accel_exp: f32,
    dt_secs: f32,
) -> (f32, f32) {
    let magnitude = (stick_x * stick_x + stick_y * stick_y).sqrt();
    if magnitude < deadzone || magnitude == 0.0 {
        return (0.0, 0.0);
    }
    let denom = (1.0 - deadzone).max(1e-6);
    let effective = ((magnitude - deadzone) / denom).min(1.0);
    let accelerated = effective.powf(accel_exp);
    let norm_x = stick_x / magnitude;
    let norm_y = stick_y / magnitude;
    let speed = accelerated * sensitivity * BASE_SPEED * dt_secs;
    (norm_x * speed, -norm_y * speed)
}

#[cfg(test)]
mod parity_tests {
    //! Golden-vector parity (#349): the companion `stick_delta` must reproduce the
    //! checked-in `(stick, dt, config) -> (dx, dy)` table that the desktop `cursor.rs`
    //! also reproduces. The fixture is byte-identical across both repos; if either math
    //! impl drifts, this test (and the desktop mirror) fails.
    use super::stick_delta;
    use crate::golden::GoldenFixture;
    use std::path::Path;

    /// Max per-axis deviation. Generous for f32 rounding, far tighter than any real
    /// constant/formula change (a 1% sensitivity drift moves the diagonal case ~0.16px).
    const TOL: f64 = 1e-3;

    #[test]
    fn pc_cursor_matches_golden_cursor_vectors() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cursor-scroll-golden.json");
        let fx = GoldenFixture::load_from_path(&path).expect("golden fixture loads");
        let mut report = String::new();
        let mut missing = false;
        for case in &fx.cases {
            let c = &case.input.config;
            let (dx, dy) = stick_delta(
                case.input.stick_x,
                case.input.stick_y,
                c.deadzone,
                c.sensitivity,
                c.acceleration_exp,
                case.input.dt,
            );
            report.push_str(&format!("{}: dx={dx} dy={dy}\n", case.name));
            match &case.expected {
                Some(e) => {
                    assert!(
                        (dx as f64 - e.dx).abs() < TOL && (dy as f64 - e.dy).abs() < TOL,
                        "case '{}': got ({dx}, {dy}), expected ({}, {})",
                        case.name,
                        e.dx,
                        e.dy
                    );
                }
                None => missing = true,
            }
        }
        assert!(
            !missing,
            "fixture has unpopulated expected values:\n{report}"
        );
    }
}

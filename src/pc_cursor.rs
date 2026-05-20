//! OS mouse cursor from sticks (same math as warmUP `gamepad/cursor.rs`).

use enigo::{Axis, Coordinate, Direction, Enigo, Mouse, Settings};

const BASE_SPEED: f32 = 100.0;
const BASE_SCROLL_SPEED: f32 = 20.0;
const DEADZONE: f32 = 0.15;
const SENSITIVITY: f32 = 15.0;
const ACCEL_EXP: f32 = 2.0;
const SCROLL_SENSITIVITY: f32 = 5.0;

pub struct PcCursor {
    enigo: Option<Enigo>,
    remainder_x: f32,
    remainder_y: f32,
    scroll_remainder_x: f32,
    scroll_remainder_y: f32,
}

fn sync_service_input_desktop() {
    #[cfg(windows)]
    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
        let _ = crate::win::sync_input_desktop();
    }
}

impl PcCursor {
    pub fn new() -> Result<Self, String> {
        let enigo =
            Enigo::new(&Settings::default()).map_err(|e| format!("enigo init failed: {e}"))?;
        Ok(Self::with_enigo(Some(enigo)))
    }

    /// Session-0 service: skip mouse (enigo often fails without interactive desktop).
    pub fn new_service() -> Self {
        let enigo = Enigo::new(&Settings::default()).ok();
        Self::with_enigo(enigo)
    }

    fn with_enigo(enigo: Option<Enigo>) -> Self {
        Self {
            enigo,
            remainder_x: 0.0,
            remainder_y: 0.0,
            scroll_remainder_x: 0.0,
            scroll_remainder_y: 0.0,
        }
    }

    pub fn move_stick(&mut self, stick_x: f32, stick_y: f32, dt_secs: f32) {
        sync_service_input_desktop();
        let (dx, dy) = stick_delta(stick_x, stick_y, SENSITIVITY, dt_secs);
        if dx == 0.0 && dy == 0.0 {
            self.remainder_x = 0.0;
            self.remainder_y = 0.0;
            return;
        }
        let total_x = dx + self.remainder_x;
        let total_y = dy + self.remainder_y;
        let int_x = total_x as i32;
        let int_y = total_y as i32;
        self.remainder_x = total_x - int_x as f32;
        self.remainder_y = total_y - int_y as f32;
        if int_x != 0 || int_y != 0 {
            if let Some(enigo) = self.enigo.as_mut() {
                let _ = enigo.move_mouse(int_x, int_y, Coordinate::Rel);
            }
        }
    }

    pub fn scroll_stick(&mut self, stick_x: f32, stick_y: f32, dt_secs: f32) {
        sync_service_input_desktop();
        let (sx, sy) = scroll_delta(stick_x, stick_y, dt_secs);
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
        if let Some(enigo) = self.enigo.as_mut() {
            if int_y != 0 {
                let _ = enigo.scroll(-int_y, Axis::Vertical);
            }
            if int_x != 0 {
                let _ = enigo.scroll(int_x, Axis::Horizontal);
            }
        }
    }

    pub fn left_click(&mut self) {
        sync_service_input_desktop();
        if let Some(enigo) = self.enigo.as_mut() {
            let _ = enigo.button(enigo::Button::Left, Direction::Click);
        }
    }
}

fn scroll_delta(stick_x: f32, stick_y: f32, dt_secs: f32) -> (f32, f32) {
    let magnitude = (stick_x * stick_x + stick_y * stick_y).sqrt();
    if magnitude < DEADZONE || magnitude == 0.0 {
        return (0.0, 0.0);
    }
    let denom = (1.0 - DEADZONE).max(1e-6);
    let effective = ((magnitude - DEADZONE) / denom).min(1.0);
    let accelerated = effective.powf(ACCEL_EXP);
    let norm_x = stick_x / magnitude;
    let norm_y = stick_y / magnitude;
    let speed = accelerated * SCROLL_SENSITIVITY * BASE_SCROLL_SPEED * dt_secs;
    (norm_x * speed, norm_y * speed)
}

fn stick_delta(stick_x: f32, stick_y: f32, sensitivity: f32, dt_secs: f32) -> (f32, f32) {
    let magnitude = (stick_x * stick_x + stick_y * stick_y).sqrt();
    if magnitude < DEADZONE || magnitude == 0.0 {
        return (0.0, 0.0);
    }
    let denom = (1.0 - DEADZONE).max(1e-6);
    let effective = ((magnitude - DEADZONE) / denom).min(1.0);
    let accelerated = effective.powf(ACCEL_EXP);
    let norm_x = stick_x / magnitude;
    let norm_y = stick_y / magnitude;
    let speed = accelerated * sensitivity * BASE_SPEED * dt_secs;
    (norm_x * speed, -norm_y * speed)
}

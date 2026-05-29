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
    /// Service worker only: dedicated SendInput thread on the Default desktop.
    /// The worker main thread is attached to Winlogon, so injecting from it lands
    /// on the wrong desktop (invisible after login). The injector thread inherits
    /// the process startup desktop (`winsta0\default`), so its SendInput reaches
    /// the user's desktop with no SetThreadDesktop (which fails ERROR_BUSY on a
    /// thread that owns windows).
    injector: Option<Injector>,
    remainder_x: f32,
    remainder_y: f32,
    scroll_remainder_x: f32,
    scroll_remainder_y: f32,
}

impl PcCursor {
    pub fn new() -> Result<Self, String> {
        let enigo =
            Enigo::new(&Settings::default()).map_err(|e| format!("enigo init failed: {e}"))?;
        Ok(Self::with_enigo(Some(enigo)))
    }

    /// Service worker (user session, not session 0). Cursor injection runs on a
    /// dedicated Default-desktop thread; falls back to inline Enigo if the thread
    /// can't spawn.
    pub fn new_service() -> Self {
        match Injector::spawn() {
            Some(injector) => Self {
                enigo: None,
                injector: Some(injector),
                remainder_x: 0.0,
                remainder_y: 0.0,
                scroll_remainder_x: 0.0,
                scroll_remainder_y: 0.0,
            },
            None => Self::with_enigo(Enigo::new(&Settings::default()).ok()),
        }
    }

    fn with_enigo(enigo: Option<Enigo>) -> Self {
        Self {
            enigo,
            injector: None,
            remainder_x: 0.0,
            remainder_y: 0.0,
            scroll_remainder_x: 0.0,
            scroll_remainder_y: 0.0,
        }
    }

    pub fn move_stick(&mut self, stick_x: f32, stick_y: f32, dt_secs: f32) {
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
            self.dispatch(Cmd::Move(int_x, int_y));
        }
    }

    pub fn scroll_stick(&mut self, stick_x: f32, stick_y: f32, dt_secs: f32) {
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
        if int_y != 0 {
            self.dispatch(Cmd::ScrollV(-int_y));
        }
        if int_x != 0 {
            self.dispatch(Cmd::ScrollH(int_x));
        }
    }

    pub fn left_click(&mut self) {
        self.dispatch(Cmd::Click);
    }

    /// Route a command to the injector thread (service) or inline Enigo.
    fn dispatch(&mut self, cmd: Cmd) {
        if let Some(injector) = &self.injector {
            injector.send(cmd);
            return;
        }
        let Some(enigo) = self.enigo.as_mut() else {
            return;
        };
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
            Cmd::Click => {
                let _ = enigo.button(enigo::Button::Left, Direction::Click);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Cmd {
    Move(i32, i32),
    ScrollV(i32),
    ScrollH(i32),
    Click,
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
            Cmd::Click => {
                let _ = enigo.button(enigo::Button::Left, Direction::Click);
            }
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

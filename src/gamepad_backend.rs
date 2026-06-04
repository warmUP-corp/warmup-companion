//! Logical gamepad input — SDL3 on userland / `--gamepad`; HID+XInput on Winlogon service path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub use warmup_gamepad::{Button, ButtonChange, GamepadInput, PollMode, TouchpadSample};

/// Owned, thread-portable snapshot of one touchpad poll. Unlike
/// [`GamepadInput::poll_touchpad`] (which borrows internal state), this can be
/// published across the SDL thread boundary.
#[derive(Clone, Debug, Default)]
pub struct TouchpadFrame {
    /// Primary-finger delta (normalized 0..1 units) since the previous sample,
    /// or `None` when no finger is down / first contact. Not carried by the
    /// `touchpad` IPC frame (which sends raw finger slots); kept for a future
    /// touchpad-as-cursor consumer.
    #[allow(dead_code)]
    pub delta: Option<(f32, f32)>,
    /// Every supported finger slot captured this poll.
    pub fingers: Vec<TouchpadSample>,
}

/// Owned battery snapshot. `percent` is 0–100 or −1 when unknown.
#[derive(Clone, Copy, Debug)]
pub struct BatteryFrame {
    pub percent: i32,
    pub charging: bool,
    pub wired: bool,
}

impl Default for BatteryFrame {
    fn default() -> Self {
        Self {
            percent: -1,
            charging: false,
            wired: false,
        }
    }
}

/// Device-feature command sent to a backend that owns the pad on another thread.
/// Writes (LED/rumble) can't touch the pad from the loop thread, so they are
/// queued and applied where the `GamepadInput` lives.
#[derive(Clone, Copy, Debug)]
pub enum PadCommand {
    Led { r: u8, g: u8, b: u8 },
    Rumble { strong: f32, weak: f32, ms: u32 },
    TriggerRumble { left: f32, right: f32, ms: u32 },
}

fn effective_userland_poll_mode() -> PollMode {
    if crate::pipe_server::game_active() {
        PollMode::Sleep
    } else {
        crate::config::userland_gamepad_poll_mode()
    }
}

/// Polls physical controller state and produces normalized axes + button edges.
///
/// Device features (touchpad/gyro/battery/LED/rumble) default to "unsupported":
/// reads return empty/`None`, writes no-op. Backends that own a feature-rich pad
/// (the SDL backends) override them; the Winlogon XInput path leaves the defaults.
pub trait GamepadBackend {
    fn poll(&mut self) -> Result<(), String>;
    fn button_changes(&mut self) -> Vec<ButtonChange>;
    fn axes(&self) -> (f32, f32, f32, f32);
    fn controller_label(&self) -> String;
    fn live_input_summary(&self) -> String;

    /// Latest touchpad sample from the most recent poll.
    fn touchpad(&self) -> TouchpadFrame {
        TouchpadFrame::default()
    }
    /// Latest gyro angular velocity `(pitch, yaw, roll)` in rad/s, if the pad has one.
    fn gyro(&self) -> Option<(f32, f32, f32)> {
        None
    }
    /// Latest battery snapshot.
    fn battery(&self) -> BatteryFrame {
        BatteryFrame::default()
    }
    /// Set the lightbar/LED colour. No-op if the pad has no LED.
    fn set_led(&mut self, _r: u8, _g: u8, _b: u8) {}
    /// Fire a main rumble effect (`strong`/`weak` in 0..=1).
    fn rumble(&mut self, _strong: f32, _weak: f32, _duration_ms: u32) {}
    /// Fire a trigger (adaptive) rumble effect (`left`/`right` in 0..=1).
    fn trigger_rumble(&mut self, _left: f32, _right: f32, _duration_ms: u32) {}
}

pub struct SdlBackend {
    input: GamepadInput,
    pending: Vec<ButtonChange>,
    touchpad: TouchpadFrame,
    gyro: Option<(f32, f32, f32)>,
    battery: BatteryFrame,
    /// Gyro must be enabled once after each (re)connect before `read_gyro` returns data.
    gyro_enabled: bool,
}

impl SdlBackend {
    pub fn open() -> Result<Self, String> {
        let db = mapping_db_path();
        let mut input = GamepadInput::new(&db)?;
        let gyro_enabled = input.enable_gyro();
        Ok(Self {
            input,
            pending: Vec::new(),
            touchpad: TouchpadFrame::default(),
            gyro: None,
            battery: BatteryFrame::default(),
            gyro_enabled,
        })
    }

    /// Capture the device-feature reads for this poll cycle. Only in `Full` mode;
    /// `Sleep` keeps the pad quiet (matches axes/summary gating).
    fn capture_features(&mut self, mode: PollMode, connected_change: bool) {
        if connected_change {
            // Re-arm gyro on the freshly opened pad (or clear if it went away).
            self.gyro_enabled = self.input.enable_gyro();
        }
        if mode != PollMode::Full {
            self.touchpad = TouchpadFrame::default();
            self.gyro = None;
            return;
        }
        let (delta, fingers) = self.input.poll_touchpad();
        self.touchpad = TouchpadFrame {
            delta,
            fingers: fingers.to_vec(),
        };
        self.gyro = if self.gyro_enabled {
            self.input.read_gyro()
        } else {
            None
        };
        let (percent, charging, wired) = self.input.battery();
        self.battery = BatteryFrame {
            percent,
            charging,
            wired,
        };
    }
}

impl GamepadBackend for SdlBackend {
    fn poll(&mut self) -> Result<(), String> {
        let mode = effective_userland_poll_mode();
        let connected_change = self.input.poll_events_with_mode(mode);
        self.pending = self.input.detect_button_changes();
        self.capture_features(mode, connected_change);
        Ok(())
    }

    fn touchpad(&self) -> TouchpadFrame {
        self.touchpad.clone()
    }

    fn gyro(&self) -> Option<(f32, f32, f32)> {
        self.gyro
    }

    fn battery(&self) -> BatteryFrame {
        self.battery
    }

    fn set_led(&mut self, r: u8, g: u8, b: u8) {
        self.input.set_led(r, g, b);
    }

    fn rumble(&mut self, strong: f32, weak: f32, duration_ms: u32) {
        self.input.rumble(strong, weak, duration_ms);
    }

    fn trigger_rumble(&mut self, left: f32, right: f32, duration_ms: u32) {
        self.input.trigger_rumble(left, right, duration_ms);
    }

    fn button_changes(&mut self) -> Vec<ButtonChange> {
        std::mem::take(&mut self.pending)
    }

    fn axes(&self) -> (f32, f32, f32, f32) {
        match effective_userland_poll_mode() {
            PollMode::Full => self.input.axes(),
            PollMode::Sleep => (0.0, 0.0, 0.0, 0.0),
        }
    }

    fn controller_label(&self) -> String {
        self.input
            .active_controller_name()
            .unwrap_or_else(|| "none".to_string())
    }

    fn live_input_summary(&self) -> String {
        match effective_userland_poll_mode() {
            PollMode::Full => self.input.live_input_summary(),
            PollMode::Sleep => "sleep (guide only)".to_string(),
        }
    }
}

/// State the SDL thread publishes and the loop thread snapshots. SDL3 creates
/// internal message windows on whatever thread inits it; in the service worker
/// the loop thread does `SendInput` + `SetThreadDesktop`, which fails with
/// `ERROR_BUSY` if that thread owns any window. So SDL must run on its own thread.
struct SdlShared {
    changes: Mutex<Vec<ButtonChange>>,
    axes: Mutex<(f32, f32, f32, f32)>,
    label: Mutex<String>,
    summary: Mutex<String>,
    touchpad: Mutex<TouchpadFrame>,
    gyro: Mutex<Option<(f32, f32, f32)>>,
    battery: Mutex<BatteryFrame>,
}

/// `GamepadBackend` proxy whose `GamepadInput` (and SDL's windows) live entirely
/// on a dedicated thread. The loop thread only reads the published snapshot, so
/// it never owns an SDL window and can freely migrate desktops.
pub struct SdlThreadBackend {
    shared: Arc<SdlShared>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Write commands (LED/rumble) for the SDL thread, which owns the pad.
    cmd_tx: mpsc::Sender<PadCommand>,
}

impl SdlThreadBackend {
    pub fn open() -> Result<Self, String> {
        let db = mapping_db_path();
        let shared = Arc::new(SdlShared {
            changes: Mutex::new(Vec::new()),
            axes: Mutex::new((0.0, 0.0, 0.0, 0.0)),
            label: Mutex::new("none".to_string()),
            summary: Mutex::new(String::new()),
            touchpad: Mutex::new(TouchpadFrame::default()),
            gyro: Mutex::new(None),
            battery: Mutex::new(BatteryFrame::default()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<PadCommand>();
        let handle = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("warmup-sdl".into())
                .spawn(move || sdl_thread_main(db, shared, stop, tx, cmd_rx))
                .map_err(|e| format!("spawn SDL thread: {e}"))?
        };
        // Block until the thread reports SDL init, so open() fails cleanly if the
        // controller subsystem can't start (mirrors SdlBackend::open semantics).
        match rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                stop,
                handle: Some(handle),
                cmd_tx,
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err("SDL thread exited during init".to_string()),
        }
    }

    /// Queue a write command; dropped silently if the SDL thread has exited.
    fn send_cmd(&self, cmd: PadCommand) {
        let _ = self.cmd_tx.send(cmd);
    }
}

/// Runs on the dedicated SDL thread. Owns `GamepadInput` for its whole life and
/// drops it here, so every SDL call (init/poll/teardown) stays on this thread.
fn sdl_thread_main(
    db: PathBuf,
    shared: Arc<SdlShared>,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<Result<(), String>>,
    cmd_rx: mpsc::Receiver<PadCommand>,
) {
    let mut input = match GamepadInput::new(&db) {
        Ok(i) => {
            let _ = tx.send(Ok(()));
            i
        }
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };
    // Arm gyro on whatever pad is already open; re-armed on each hotplug below.
    let mut gyro_enabled = input.enable_gyro();
    while !stop.load(Ordering::Relaxed) {
        // Apply queued writes (LED/rumble) first — they must run on this thread
        // because it owns the pad. Drain the whole backlog, newest wins for LED.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PadCommand::Led { r, g, b } => input.set_led(r, g, b),
                PadCommand::Rumble { strong, weak, ms } => input.rumble(strong, weak, ms),
                PadCommand::TriggerRumble { left, right, ms } => {
                    input.trigger_rumble(left, right, ms)
                }
            }
        }

        let mode = effective_userland_poll_mode();
        let connected_change = input.poll_events_with_mode(mode);
        if connected_change {
            // Pad connected/disconnected: re-arm gyro on the new device.
            gyro_enabled = input.enable_gyro();
        }
        let changes = input.detect_button_changes();
        if !changes.is_empty() {
            if let Ok(mut q) = shared.changes.lock() {
                q.extend(changes);
            }
        }
        if let Ok(mut a) = shared.axes.lock() {
            *a = match mode {
                PollMode::Full => input.axes(),
                PollMode::Sleep => (0.0, 0.0, 0.0, 0.0),
            };
        }
        if let Ok(mut l) = shared.label.lock() {
            *l = input
                .active_controller_name()
                .unwrap_or_else(|| "none".to_string());
        }
        if let Ok(mut s) = shared.summary.lock() {
            *s = match mode {
                PollMode::Full => input.live_input_summary(),
                PollMode::Sleep => "sleep (guide only)".to_string(),
            };
        }
        // Device-feature reads — only in Full mode (Sleep keeps the pad quiet).
        let (touchpad, gyro) = if mode == PollMode::Full {
            let (delta, fingers) = input.poll_touchpad();
            let tp = TouchpadFrame {
                delta,
                fingers: fingers.to_vec(),
            };
            let gy = if gyro_enabled { input.read_gyro() } else { None };
            (tp, gy)
        } else {
            (TouchpadFrame::default(), None)
        };
        if let Ok(mut t) = shared.touchpad.lock() {
            *t = touchpad;
        }
        if let Ok(mut g) = shared.gyro.lock() {
            *g = gyro;
        }
        if let Ok(mut bat) = shared.battery.lock() {
            let (percent, charging, wired) = input.battery();
            *bat = BatteryFrame {
                percent,
                charging,
                wired,
            };
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

impl GamepadBackend for SdlThreadBackend {
    fn poll(&mut self) -> Result<(), String> {
        // The dedicated thread polls SDL; nothing to do on the loop thread.
        Ok(())
    }

    fn button_changes(&mut self) -> Vec<ButtonChange> {
        self.shared
            .changes
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    fn axes(&self) -> (f32, f32, f32, f32) {
        self.shared
            .axes
            .lock()
            .map(|a| *a)
            .unwrap_or((0.0, 0.0, 0.0, 0.0))
    }

    fn controller_label(&self) -> String {
        self.shared
            .label
            .lock()
            .map(|l| l.clone())
            .unwrap_or_else(|_| "none".to_string())
    }

    fn live_input_summary(&self) -> String {
        self.shared
            .summary
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    fn touchpad(&self) -> TouchpadFrame {
        self.shared
            .touchpad
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }

    fn gyro(&self) -> Option<(f32, f32, f32)> {
        self.shared.gyro.lock().map(|g| *g).unwrap_or(None)
    }

    fn battery(&self) -> BatteryFrame {
        self.shared
            .battery
            .lock()
            .map(|b| *b)
            .unwrap_or_default()
    }

    fn set_led(&mut self, r: u8, g: u8, b: u8) {
        self.send_cmd(PadCommand::Led { r, g, b });
    }

    fn rumble(&mut self, strong: f32, weak: f32, duration_ms: u32) {
        self.send_cmd(PadCommand::Rumble {
            strong,
            weak,
            ms: duration_ms,
        });
    }

    fn trigger_rumble(&mut self, left: f32, right: f32, duration_ms: u32) {
        self.send_cmd(PadCommand::TriggerRumble {
            left,
            right,
            ms: duration_ms,
        });
    }
}

impl Drop for SdlThreadBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            // Join so SDL teardown completes on its own thread, never the loop thread.
            let _ = h.join();
        }
    }
}

pub fn mapping_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("WARMUP_GAMECONTROLLER_DB") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        let installed = PathBuf::from(r"C:\ProgramData\WarmupVk\gamecontrollerdb.txt");
        if installed.is_file() {
            return installed;
        }
    }
    let warmup_db = PathBuf::from(
        r"C:\Users\jonas\warmUp\apps\desktop\src-tauri\resources\gamecontrollerdb.txt",
    );
    if warmup_db.is_file() {
        return warmup_db;
    }
    PathBuf::from(
        r"C:\Users\Jonas.Voegel\Full-Screen-Console-PC-v2-Tauri\apps\desktop\src-tauri\resources\gamecontrollerdb.txt",
    )
}

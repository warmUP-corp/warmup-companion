//! Logical gamepad input — SDL3 on userland / `--gamepad`; HID+XInput on Winlogon service path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub use warmup_gamepad::{Button, ButtonChange, GamepadInput};

/// Polls physical controller state and produces normalized axes + button edges.
pub trait GamepadBackend {
    fn poll(&mut self) -> Result<(), String>;
    fn button_changes(&mut self) -> Vec<ButtonChange>;
    fn axes(&self) -> (f32, f32, f32, f32);
    fn controller_label(&self) -> String;
    fn live_input_summary(&self) -> String;
}

pub struct SdlBackend {
    input: GamepadInput,
    pending: Vec<ButtonChange>,
}

impl SdlBackend {
    pub fn open() -> Result<Self, String> {
        let db = mapping_db_path();
        let input = GamepadInput::new(&db)?;
        Ok(Self {
            input,
            pending: Vec::new(),
        })
    }
}

impl GamepadBackend for SdlBackend {
    fn poll(&mut self) -> Result<(), String> {
        self.input.poll_events();
        self.pending = self.input.detect_button_changes();
        Ok(())
    }

    fn button_changes(&mut self) -> Vec<ButtonChange> {
        std::mem::take(&mut self.pending)
    }

    fn axes(&self) -> (f32, f32, f32, f32) {
        self.input.axes()
    }

    fn controller_label(&self) -> String {
        self.input
            .active_controller_name()
            .unwrap_or_else(|| "none".to_string())
    }

    fn live_input_summary(&self) -> String {
        self.input.live_input_summary()
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
}

/// `GamepadBackend` proxy whose `GamepadInput` (and SDL's windows) live entirely
/// on a dedicated thread. The loop thread only reads the published snapshot, so
/// it never owns an SDL window and can freely migrate desktops.
pub struct SdlThreadBackend {
    shared: Arc<SdlShared>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SdlThreadBackend {
    pub fn open() -> Result<Self, String> {
        let db = mapping_db_path();
        let shared = Arc::new(SdlShared {
            changes: Mutex::new(Vec::new()),
            axes: Mutex::new((0.0, 0.0, 0.0, 0.0)),
            label: Mutex::new("none".to_string()),
            summary: Mutex::new(String::new()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let handle = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("warmup-sdl".into())
                .spawn(move || sdl_thread_main(db, shared, stop, tx))
                .map_err(|e| format!("spawn SDL thread: {e}"))?
        };
        // Block until the thread reports SDL init, so open() fails cleanly if the
        // controller subsystem can't start (mirrors SdlBackend::open semantics).
        match rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                stop,
                handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err("SDL thread exited during init".to_string()),
        }
    }
}

/// Runs on the dedicated SDL thread. Owns `GamepadInput` for its whole life and
/// drops it here, so every SDL call (init/poll/teardown) stays on this thread.
fn sdl_thread_main(
    db: PathBuf,
    shared: Arc<SdlShared>,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<Result<(), String>>,
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
    while !stop.load(Ordering::Relaxed) {
        input.poll_events();
        let changes = input.detect_button_changes();
        if !changes.is_empty() {
            if let Ok(mut q) = shared.changes.lock() {
                q.extend(changes);
            }
        }
        if let Ok(mut a) = shared.axes.lock() {
            *a = input.axes();
        }
        if let Ok(mut l) = shared.label.lock() {
            *l = input
                .active_controller_name()
                .unwrap_or_else(|| "none".to_string());
        }
        if let Ok(mut s) = shared.summary.lock() {
            *s = input.live_input_summary();
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

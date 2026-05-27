//! Logical gamepad input — SDL3 on userland / `--gamepad`; HID+XInput on Winlogon service path.

use std::path::PathBuf;

pub use warmup_gamepad::{ButtonChange, GamepadInput};

/// Polls physical controller state and produces normalized axes + button edges.
pub trait GamepadBackend {
    fn poll(&mut self) -> Result<(), String>;
    fn button_changes(&mut self) -> Vec<ButtonChange>;
    fn axes(&self) -> (f32, f32, f32, f32);
    fn controller_label(&self) -> String;
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

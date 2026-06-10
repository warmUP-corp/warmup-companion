use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct DebugSnapshot {
    pub connected: bool,
    /// Human-readable device name of the active controller (e.g. "DualSense
    /// Wireless Controller"), as reported by the backend. Empty when unknown.
    pub name: String,
    pub input: String,
    /// Low-level diagnostic line (e.g. raw XUSB report bytes on Winlogon), for
    /// confirming reverse-engineered byte offsets against live button presses.
    pub detail: String,
}

#[derive(Debug, Default)]
struct DebugState {
    connected: bool,
    name: String,
    input: String,
    detail: String,
}

static STATE: OnceLock<Mutex<DebugState>> = OnceLock::new();

fn state() -> &'static Mutex<DebugState> {
    STATE.get_or_init(|| Mutex::new(DebugState::default()))
}

/// Latest gamepad connection + device name + live buttons/sticks (updated each poll).
pub fn set_gamepad(connected: bool, name: impl Into<String>, input: impl Into<String>) {
    if let Ok(mut s) = state().lock() {
        s.connected = connected;
        s.name = name.into();
        s.input = input.into();
    }
}

/// Low-level diagnostic line (raw XUSB bytes on the secure desktop, etc.).
pub fn set_detail(detail: impl Into<String>) {
    if let Ok(mut s) = state().lock() {
        s.detail = detail.into();
    }
}

pub fn snapshot() -> DebugSnapshot {
    match state().lock() {
        Ok(s) => DebugSnapshot {
            connected: s.connected,
            name: s.name.clone(),
            input: s.input.clone(),
            detail: s.detail.clone(),
        },
        Err(_) => DebugSnapshot {
            connected: false,
            name: String::new(),
            input: "poisoned".into(),
            detail: String::new(),
        },
    }
}

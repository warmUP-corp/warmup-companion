use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct DebugSnapshot {
    pub connected: bool,
    pub input: String,
}

#[derive(Debug, Default)]
struct DebugState {
    connected: bool,
    input: String,
}

static STATE: OnceLock<Mutex<DebugState>> = OnceLock::new();

fn state() -> &'static Mutex<DebugState> {
    STATE.get_or_init(|| Mutex::new(DebugState::default()))
}

/// Latest gamepad connection + live buttons/sticks (updated each poll).
pub fn set_gamepad(connected: bool, input: impl Into<String>) {
    if let Ok(mut s) = state().lock() {
        s.connected = connected;
        s.input = input.into();
    }
}

pub fn snapshot() -> DebugSnapshot {
    match state().lock() {
        Ok(s) => DebugSnapshot {
            connected: s.connected,
            input: s.input.clone(),
        },
        Err(_) => DebugSnapshot {
            connected: false,
            input: "poisoned".into(),
        },
    }
}

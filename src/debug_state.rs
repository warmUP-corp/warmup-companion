use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct DebugSnapshot {
    pub xinput_loader: String,
    pub last_buttons: String,
    pub last_action: String,
}

#[derive(Debug)]
struct DebugState {
    xinput_loader: String,
    last_buttons: String,
    last_action: String,
    started: Instant,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            xinput_loader: "unknown".into(),
            last_buttons: "never".into(),
            last_action: "none".into(),
            started: Instant::now(),
        }
    }
}

static STATE: OnceLock<Mutex<DebugState>> = OnceLock::new();

fn state() -> &'static Mutex<DebugState> {
    STATE.get_or_init(|| Mutex::new(DebugState::default()))
}

pub fn set_xinput_loader(label: impl Into<String>) {
    if let Ok(mut s) = state().lock() {
        s.xinput_loader = label.into();
    }
}

pub fn record_xinput_buttons(mask: u16, names: &str) {
    if let Ok(mut s) = state().lock() {
        let elapsed = s.started.elapsed().as_millis();
        let names = if names.is_empty() { "none" } else { names };
        s.last_buttons = format!("t+{elapsed}ms mask=0x{mask:04x} [{names}]");
    }
}

pub fn record_action(label: impl Into<String>) {
    if let Ok(mut s) = state().lock() {
        let elapsed = s.started.elapsed().as_millis();
        s.last_action = format!("t+{elapsed}ms {}", label.into());
    }
}

pub fn snapshot() -> DebugSnapshot {
    match state().lock() {
        Ok(s) => DebugSnapshot {
            xinput_loader: s.xinput_loader.clone(),
            last_buttons: s.last_buttons.clone(),
            last_action: s.last_action.clone(),
        },
        Err(_) => DebugSnapshot {
            xinput_loader: "poisoned".into(),
            last_buttons: "poisoned".into(),
            last_action: "poisoned".into(),
        },
    }
}

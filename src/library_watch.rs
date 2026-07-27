//! Persisted library watchlist for offline playtime tracking while warmUP is closed.

#![cfg(windows)]

use std::sync::{Mutex, OnceLock};

use crate::protocol::{LibraryWatchGameEntry, LibraryWatchPayload};

const WATCH_FILE: &str = r"C:\ProgramData\WarmupVk\library-watch.json";

#[derive(Debug, Clone, Default)]
struct WatchState {
    watch: LibraryWatchPayload,
}

static STATE: OnceLock<Mutex<WatchState>> = OnceLock::new();

fn state() -> &'static Mutex<WatchState> {
    STATE.get_or_init(|| Mutex::new(WatchState::default()))
}

pub fn apply_watch(payload: &LibraryWatchPayload) {
    if let Ok(mut slot) = state().lock() {
        slot.watch = payload.clone();
    }
    persist_watch(payload);
}

pub fn load_persisted_watch() {
    let Ok(raw) = std::fs::read_to_string(WATCH_FILE) else {
        return;
    };
    if let Ok(payload) = serde_json::from_str::<LibraryWatchPayload>(&raw) {
        apply_watch(&payload);
    }
}

pub fn current_watch() -> LibraryWatchPayload {
    state()
        .lock()
        .map(|s| s.watch.clone())
        .unwrap_or_default()
}

pub fn current_games() -> Vec<LibraryWatchGameEntry> {
    let watch = current_watch();
    if watch.enabled {
        watch.games
    } else {
        Vec::new()
    }
}

fn persist_watch(payload: &LibraryWatchPayload) {
    if let Ok(json) = serde_json::to_string(payload) {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\WarmupVk");
        let _ = std::fs::write(WATCH_FILE, json);
    }
}

#[cfg(not(windows))]
pub fn apply_watch(_payload: &LibraryWatchPayload) {}
#[cfg(not(windows))]
pub fn load_persisted_watch() {}
#[cfg(not(windows))]
pub fn current_watch() -> LibraryWatchPayload {
    LibraryWatchPayload {
        enabled: false,
        games: Vec::new(),
    }
}
#[cfg(not(windows))]
pub fn current_games() -> Vec<LibraryWatchGameEntry> {
    Vec::new()
}

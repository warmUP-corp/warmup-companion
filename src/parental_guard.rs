//! Kid Mode system-wide game blocking for processes started outside warmUP.
//!
//! warmUP pushes a [`ParentalGuardPayload`] over IPC; this module holds the
//! in-memory policy and polls running processes while Kid Mode is active.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::process_guard;
use crate::protocol::{ParentalBlockedPayload, ParentalGuardPayload};
use crate::{parental_store, pipe_server};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const NOTIFY_COOLDOWN: Duration = Duration::from_secs(20);

static STATE: OnceLock<Mutex<GuardState>> = OnceLock::new();
static BLOCKED_NOTIFY: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
struct GuardState {
    guard: ParentalGuardPayload,
}

fn state() -> &'static Mutex<GuardState> {
    STATE.get_or_init(|| Mutex::new(GuardState::default()))
}

fn notify_cooldown() -> &'static Mutex<HashMap<String, Instant>> {
    BLOCKED_NOTIFY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn spawn_guardian_loop() {
    std::thread::Builder::new()
        .name("warmup-parental-guard".into())
        .spawn(guardian_loop)
        .ok();
}

pub fn apply_guard(payload: &ParentalGuardPayload) {
    if let Ok(mut slot) = state().lock() {
        slot.guard = payload.clone();
    }
    parental_store::save(payload);
}

pub fn load_persisted_guard() {
    if let Some(payload) = parental_store::load() {
        apply_guard(&payload);
    }
}

pub fn publish_blocked(payload: ParentalBlockedPayload) {
    pipe_server::publish_parental_blocked(payload);
}

/// Re-export process snapshots so existing callers keep compiling.
pub(crate) use process_guard::snapshot_processes_for_owner;

fn guardian_loop() {
    load_persisted_guard();
    process_guard::enable_debug_privilege();
    loop {
        tick_guardian();
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn tick_guardian() {
    let guard = state().lock().map(|s| s.guard.clone()).unwrap_or_default();
    if !guard.enabled {
        return;
    }
    let blocked_stems: HashSet<String> = guard.blocked_exe_stems.iter().cloned().collect();
    let blocked_dirs: Vec<String> = guard.blocked_install_dir_prefixes.clone();
    if blocked_stems.is_empty() && blocked_dirs.is_empty() {
        return;
    }

    let our_pid = std::process::id();
    for (pid, exe_name, image_path) in process_guard::snapshot_processes() {
        if pid == 0 || pid == our_pid || is_protected_process(&exe_name, image_path.as_deref()) {
            continue;
        }
        let stem = exe_stem_lower(&exe_name);
        let blocked_by_stem = !stem.is_empty() && blocked_stems.contains(&stem);
        let blocked_by_dir = image_path
            .as_deref()
            .map(|path| path_blocked_by_install_dir(path, &blocked_dirs))
            .unwrap_or(false);
        if !blocked_by_stem && !blocked_by_dir {
            continue;
        }
        if process_guard::terminate_blocked_process(pid) {
            maybe_notify_blocked(&stem, pid);
        }
    }
}

fn maybe_notify_blocked(stem: &str, pid: u32) {
    let now = Instant::now();
    let key = format!("{stem}:{pid}");
    if let Ok(mut map) = notify_cooldown().lock() {
        map.retain(|_, at| now.duration_since(*at) < NOTIFY_COOLDOWN);
        if map
            .get(&key)
            .is_some_and(|at| now.duration_since(*at) < NOTIFY_COOLDOWN)
        {
            return;
        }
        map.insert(key, now);
    }
    publish_blocked(ParentalBlockedPayload {
        exe_stem: stem.to_string(),
        pid,
    });
}

fn path_blocked_by_install_dir(path: &str, blocked_dirs: &[String]) -> bool {
    let lower = path.replace('/', "\\").to_ascii_lowercase();
    blocked_dirs.iter().any(|prefix| lower.starts_with(prefix))
}

fn exe_stem_lower(exe_name: &str) -> String {
    let lower = exe_name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .unwrap_or(lower.as_str())
        .to_string()
}

fn is_protected_process(exe_name: &str, image_path: Option<&str>) -> bool {
    let stem = exe_stem_lower(exe_name);
    if matches!(
        stem.as_str(),
        "warmup"
            | "warmup-companion"
            | "warmup-keyboard"
            | "explorer"
            | "steam"
            | "epicgameslauncher"
            | "galaxyclient"
            | "eadesktop"
            | "origin"
            | "ubisoftconnect"
            | "battle.net"
            | "xboxpcapp"
            | "csrss"
            | "winlogon"
            | "dwm"
            | "lsass"
            | "services"
            | "smss"
            | "sihost"
            | "fontdrvhost"
            | "taskhostw"
            | "wininit"
            | "userinit"
            | "searchhost"
            | "runtimebroker"
            | "svchost"
            | "dllhost"
            | "audiodg"
            | "system"
            | "idle"
    ) {
        return true;
    }
    if let Some(path) = image_path {
        let lower = path.replace('/', "\\").to_ascii_lowercase();
        if lower.contains("\\windows\\system32\\")
            || lower.contains("\\windows\\syswow64\\")
            || lower.contains("\\program files\\windowsapps\\")
        {
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
pub fn spawn_guardian_loop() {}
#[cfg(not(windows))]
pub fn apply_guard(_payload: &ParentalGuardPayload) {}
#[cfg(not(windows))]
pub fn load_persisted_guard() {}
#[cfg(not(windows))]
pub fn publish_blocked(_payload: ParentalBlockedPayload) {}

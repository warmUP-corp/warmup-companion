//! Offline playtime tracking while warmUP is disconnected from the companion pipe.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::parental_guard;
use crate::pipe_server;
use crate::protocol::{
    ExternalPlaySession, LibraryWatchGameEntry, PlaySessionsAckPayload, PlaySessionsPayload,
};

const PENDING_FILE: &str = r"C:\ProgramData\WarmupVk\pending-play-sessions.json";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EXIT_MISSES: u32 = 5;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
static STORE: OnceLock<Mutex<PendingStore>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PendingStore {
    #[serde(default)]
    open: Vec<OpenSession>,
    #[serde(default)]
    closed: Vec<ClosedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OpenSession {
    external_id: String,
    game_id: String,
    pid: u32,
    started_at: i64,
    #[serde(default)]
    miss_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ClosedSession {
    external_id: String,
    game_id: String,
    started_at: i64,
    ended_at: i64,
    duration_minutes: i64,
}

fn store() -> &'static Mutex<PendingStore> {
    STORE.get_or_init(|| Mutex::new(load_pending_from_disk()))
}

pub fn spawn_tracker_loop() {
    std::thread::Builder::new()
        .name("warmup-playtime-tracker".into())
        .spawn(tracker_loop)
        .ok();
}

pub fn on_desktop_connected() {
    let now = unix_now_secs();
    let mut closed = Vec::new();
    if let Ok(mut pending) = store().lock() {
        let open: Vec<OpenSession> = pending.open.drain(..).collect();
        for session in open {
            if let Some(closed_session) = finalize_open(session, now) {
                pending.closed.push(closed_session.clone());
                closed.push(closed_session);
            }
        }
        persist_pending(&pending);
    }
    if !closed.is_empty() {
        crate::install::log_line(&format!(
            "playtime tracker: finalized {} open session(s) on warmUP connect",
            closed.len()
        ));
    }
}

pub fn take_closed_sessions_for_flush() -> PlaySessionsPayload {
    PlaySessionsPayload {
        sessions: closed_sessions_for_wire(),
    }
}

pub fn apply_play_sessions_ack(payload: &PlaySessionsAckPayload) {
    if payload.external_ids.is_empty() {
        return;
    }
    let acked: HashSet<&str> = payload.external_ids.iter().map(String::as_str).collect();
    if let Ok(mut pending) = store().lock() {
        let before = pending.closed.len();
        pending
            .closed
            .retain(|session| !acked.contains(session.external_id.as_str()));
        if pending.closed.len() != before {
            persist_pending(&pending);
        }
    }
}

fn tracker_loop() {
    crate::library_watch::load_persisted_watch();
    load_pending_from_disk_into_memory();
    loop {
        tick_tracker();
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn tick_tracker() {
    if pipe_server::desktop_connected() {
        return;
    }
    let games = crate::library_watch::current_games();
    if games.is_empty() {
        return;
    }

    let now = unix_now_secs();
    let our_pid = std::process::id();
    let processes: Vec<(u32, String, Option<String>)> = parental_guard::snapshot_processes()
        .into_iter()
        .filter(|(pid, _, _)| *pid != 0 && *pid != our_pid)
        .collect();

    let mut active_by_game: HashMap<String, u32> = HashMap::new();
    for (pid, exe_name, image_path) in &processes {
        if let Some(game_id) = match_process_to_game(exe_name, image_path.as_deref(), &games) {
            active_by_game.entry(game_id).or_insert(*pid);
        }
    }

    let mut pending = match store().lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let mut changed = false;
    let open_snapshot: Vec<OpenSession> = pending.open.clone();
    for mut session in open_snapshot {
        let idx = pending
            .open
            .iter()
            .position(|s| s.external_id == session.external_id);
        let Some(idx) = idx else {
            continue;
        };

        if let Some(pid) = active_by_game.get(&session.game_id).copied() {
            if session.pid != pid {
                pending.open[idx].pid = pid;
                pending.open[idx].miss_count = 0;
                changed = true;
            } else if pending.open[idx].miss_count != 0 {
                pending.open[idx].miss_count = 0;
                changed = true;
            }
            active_by_game.remove(&session.game_id);
            continue;
        }

        session.miss_count = session.miss_count.saturating_add(1);
        pending.open[idx].miss_count = session.miss_count;
        changed = true;
        if session.miss_count >= EXIT_MISSES {
            let removed = pending.open.remove(idx);
            if let Some(closed) = finalize_open(removed, now) {
                pending.closed.push(closed);
            }
        }
    }

    for (game_id, pid) in active_by_game {
        if pending.open.iter().any(|s| s.game_id == game_id) {
            continue;
        }
        pending.open.push(OpenSession {
            external_id: new_external_id(&game_id, now),
            game_id,
            pid,
            started_at: now,
            miss_count: 0,
        });
        changed = true;
    }

    if changed {
        persist_pending(&pending);
    }
}

fn match_process_to_game(
    exe_name: &str,
    image_path: Option<&str>,
    games: &[LibraryWatchGameEntry],
) -> Option<String> {
    let stem = exe_stem_lower(exe_name);
    if stem.is_empty() {
        return None;
    }

    let path_lower = image_path.map(|path| path.replace('/', "\\").to_ascii_lowercase());

    let mut install_matches: Vec<&str> = Vec::new();
    if let Some(path) = path_lower.as_deref() {
        for game in games {
            if game
                .install_dir_prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix))
            {
                install_matches.push(game.game_id.as_str());
            }
        }
        install_matches.sort_unstable();
        install_matches.dedup();
        if install_matches.len() == 1 {
            return Some(install_matches[0].to_string());
        }
        if install_matches.len() > 1 {
            crate::install::log_line(&format!(
                "playtime tracker: install-dir collision for {path} -> using {}",
                install_matches[0]
            ));
            return Some(install_matches[0].to_string());
        }
    }

    let mut stem_matches: Vec<&str> = games
        .iter()
        .filter(|game| game.exe_stems.iter().any(|s| s == &stem))
        .map(|game| game.game_id.as_str())
        .collect();
    stem_matches.sort_unstable();
    stem_matches.dedup();
    if stem_matches.len() == 1 {
        return Some(stem_matches[0].to_string());
    }
    if stem_matches.len() > 1 {
        crate::install::log_line(&format!(
            "playtime tracker: exe-stem collision for {stem} -> using {}",
            stem_matches[0]
        ));
        return Some(stem_matches[0].to_string());
    }
    None
}

fn finalize_open(session: OpenSession, ended_at: i64) -> Option<ClosedSession> {
    let ended_at = ended_at.max(session.started_at);
    let duration_minutes = ((ended_at - session.started_at + 30) / 60).max(1);
    Some(ClosedSession {
        external_id: session.external_id,
        game_id: session.game_id,
        started_at: session.started_at,
        ended_at,
        duration_minutes,
    })
}

fn closed_sessions_for_wire() -> Vec<ExternalPlaySession> {
    store()
        .lock()
        .map(|pending| {
            pending
                .closed
                .iter()
                .map(|session| ExternalPlaySession {
                    external_id: session.external_id.clone(),
                    game_id: session.game_id.clone(),
                    started_at: session.started_at,
                    ended_at: session.ended_at,
                    duration_minutes: session.duration_minutes,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn load_pending_from_disk() -> PendingStore {
    let Ok(raw) = std::fs::read_to_string(PENDING_FILE) else {
        return PendingStore::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn load_pending_from_disk_into_memory() {
    let disk = load_pending_from_disk();
    if let Ok(mut pending) = store().lock() {
        *pending = disk;
    }
}

fn persist_pending(store: &PendingStore) {
    if let Ok(json) = serde_json::to_string(store) {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\WarmupVk");
        let _ = std::fs::write(PENDING_FILE, json);
    }
}

fn new_external_id(game_id: &str, started_at: i64) -> String {
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cmp-{game_id}-{started_at}-{n}")
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn exe_stem_lower(exe_name: &str) -> String {
    let lower = exe_name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .unwrap_or(lower.as_str())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_match_wins_over_stem_collision() {
        let games = vec![
            LibraryWatchGameEntry {
                game_id: "a".into(),
                exe_stems: vec!["game".into()],
                install_dir_prefixes: vec![r"c:\games\a\".into()],
            },
            LibraryWatchGameEntry {
                game_id: "b".into(),
                exe_stems: vec!["game".into()],
                install_dir_prefixes: vec![r"c:\games\b\".into()],
            },
        ];
        let matched = match_process_to_game(
            "game.exe",
            Some(r"C:\Games\B\bin\game.exe"),
            &games,
        );
        assert_eq!(matched.as_deref(), Some("b"));
    }

    #[test]
    fn duration_uses_warmup_minimum_one_minute() {
        let closed = finalize_open(
            OpenSession {
                external_id: "x".into(),
                game_id: "g".into(),
                pid: 1,
                started_at: 100,
                miss_count: 0,
            },
            110,
        )
        .expect("closed");
        assert_eq!(closed.duration_minutes, 1);
    }
}

#[cfg(not(windows))]
pub fn spawn_tracker_loop() {}
#[cfg(not(windows))]
pub fn on_desktop_connected() {}
#[cfg(not(windows))]
pub fn take_closed_sessions_for_flush() -> PlaySessionsPayload {
    PlaySessionsPayload {
        sessions: Vec::new(),
    }
}
#[cfg(not(windows))]
pub fn apply_play_sessions_ack(_payload: &PlaySessionsAckPayload) {}

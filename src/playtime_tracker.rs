//! Offline playtime tracking while warmUP is disconnected from the companion pipe.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::parental_guard;
use crate::pipe_server::{self, TrackingOwner};
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
    owner: TrackingOwner,
    external_id: String,
    game_id: String,
    pid: u32,
    started_at: i64,
    #[serde(default)]
    miss_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ClosedSession {
    owner: TrackingOwner,
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

pub fn on_desktop_connected(owner: &TrackingOwner) {
    let now = unix_now_secs();
    let closed = if let Ok(mut pending) = store().lock() {
        let closed = finalize_owner_sessions(&mut pending, owner, now);
        persist_pending(&pending);
        closed
    } else {
        Vec::new()
    };
    if !closed.is_empty() {
        crate::install::log_line(&format!(
            "playtime tracker: finalized {} open session(s) on warmUP connect",
            closed.len()
        ));
    }
}

pub fn take_closed_sessions_for_flush(owner: &TrackingOwner) -> PlaySessionsPayload {
    PlaySessionsPayload {
        sessions: closed_sessions_for_wire(owner),
    }
}

pub fn apply_play_sessions_ack(
    owner: &TrackingOwner,
    payload: &PlaySessionsAckPayload,
    issued: &HashSet<String>,
) -> usize {
    if payload.external_ids.is_empty() {
        return 0;
    }
    if let Ok(mut pending) = store().lock() {
        let removed = acknowledge_closed_sessions(&mut pending, owner, payload, issued);
        if removed != 0 {
            persist_pending(&pending);
        }
        return removed;
    }
    0
}

fn acknowledge_closed_sessions(
    pending: &mut PendingStore,
    owner: &TrackingOwner,
    payload: &PlaySessionsAckPayload,
    issued: &HashSet<String>,
) -> usize {
    let acked: HashSet<&str> = payload
        .external_ids
        .iter()
        .map(String::as_str)
        .filter(|external_id| issued.contains(*external_id))
        .collect();
    let before = pending.closed.len();
    pending
        .closed
        .retain(|session| session.owner != *owner || !acked.contains(session.external_id.as_str()));
    before - pending.closed.len()
}

fn tracker_loop() {
    crate::library_watch::load_persisted_watch();
    let _ = store();
    loop {
        tick_tracker();
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn tick_tracker() {
    if pipe_server::desktop_connected() {
        return;
    }
    let Some((owner, games)) = crate::library_watch::current_games() else {
        return;
    };

    let now = unix_now_secs();
    let our_pid = std::process::id();
    let processes: Vec<(u32, String, Option<String>)> =
        parental_guard::snapshot_processes_for_owner(&owner)
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
    let open_snapshot: Vec<OpenSession> = pending
        .open
        .iter()
        .filter(|session| session.owner == owner)
        .cloned()
        .collect();
    for mut session in open_snapshot {
        let idx = pending
            .open
            .iter()
            .position(|s| s.owner == owner && s.external_id == session.external_id);
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
        if pending
            .open
            .iter()
            .any(|session| session.owner == owner && session.game_id == game_id)
        {
            continue;
        }
        pending.open.push(OpenSession {
            owner: owner.clone(),
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
        owner: session.owner,
        external_id: session.external_id,
        game_id: session.game_id,
        started_at: session.started_at,
        ended_at,
        duration_minutes,
    })
}

fn finalize_owner_sessions(
    pending: &mut PendingStore,
    owner: &TrackingOwner,
    ended_at: i64,
) -> Vec<ClosedSession> {
    let (owned, remaining) = std::mem::take(&mut pending.open)
        .into_iter()
        .partition(|session| session.owner == *owner);
    pending.open = remaining;
    let closed: Vec<_> = owned
        .into_iter()
        .filter_map(|session| finalize_open(session, ended_at))
        .collect();
    pending.closed.extend(closed.iter().cloned());
    closed
}

fn closed_sessions_for_wire(owner: &TrackingOwner) -> Vec<ExternalPlaySession> {
    store()
        .lock()
        .map(|pending| {
            pending
                .closed
                .iter()
                .filter(|session| session.owner == *owner)
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
    match serde_json::from_str(&raw) {
        Ok(pending) => pending,
        Err(_) => {
            crate::install::log_line(
                "playtime tracker: rejected legacy or malformed pending state without an owner",
            );
            PendingStore::default()
        }
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
                owner: TrackingOwner {
                    user_sid: "S-1-5-21-test".into(),
                    session_id: 1,
                },
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

    #[test]
    fn acknowledgements_are_owner_and_connection_scoped() {
        let alice = TrackingOwner {
            user_sid: "S-1-5-21-alice".into(),
            session_id: 1,
        };
        let bob = TrackingOwner {
            user_sid: "S-1-5-21-bob".into(),
            session_id: 2,
        };
        let mut pending = PendingStore {
            open: Vec::new(),
            closed: vec![
                ClosedSession {
                    owner: alice.clone(),
                    external_id: "alice-issued".into(),
                    game_id: "a".into(),
                    started_at: 1,
                    ended_at: 2,
                    duration_minutes: 1,
                },
                ClosedSession {
                    owner: alice.clone(),
                    external_id: "alice-unissued".into(),
                    game_id: "a".into(),
                    started_at: 1,
                    ended_at: 2,
                    duration_minutes: 1,
                },
                ClosedSession {
                    owner: bob,
                    external_id: "bob-issued".into(),
                    game_id: "b".into(),
                    started_at: 1,
                    ended_at: 2,
                    duration_minutes: 1,
                },
            ],
        };
        let payload = PlaySessionsAckPayload {
            external_ids: vec![
                "alice-issued".into(),
                "alice-unissued".into(),
                "bob-issued".into(),
            ],
        };
        let issued = HashSet::from(["alice-issued".to_string(), "bob-issued".to_string()]);

        assert_eq!(
            acknowledge_closed_sessions(&mut pending, &alice, &payload, &issued),
            1
        );
        assert_eq!(
            pending
                .closed
                .iter()
                .map(|session| session.external_id.as_str())
                .collect::<Vec<_>>(),
            vec!["alice-unissued", "bob-issued"]
        );
    }

    #[test]
    fn connection_finalizes_only_the_authenticated_owner() {
        let alice = TrackingOwner {
            user_sid: "S-1-5-21-alice".into(),
            session_id: 1,
        };
        let bob = TrackingOwner {
            user_sid: "S-1-5-21-bob".into(),
            session_id: 2,
        };
        let session = |owner: TrackingOwner, id: &str| OpenSession {
            owner,
            external_id: id.into(),
            game_id: "g".into(),
            pid: 1,
            started_at: 100,
            miss_count: 0,
        };
        let mut pending = PendingStore {
            open: vec![session(alice.clone(), "alice"), session(bob.clone(), "bob")],
            closed: Vec::new(),
        };

        assert_eq!(finalize_owner_sessions(&mut pending, &alice, 110).len(), 1);
        assert_eq!(pending.open, vec![session(bob, "bob")]);
        assert_eq!(pending.closed[0].owner, alice);
    }

    #[test]
    fn legacy_pending_sessions_without_owner_are_rejected() {
        let legacy = r#"{"open":[],"closed":[{"external_id":"x","game_id":"g","started_at":1,"ended_at":2,"duration_minutes":1}]}"#;
        assert!(serde_json::from_str::<PendingStore>(legacy).is_err());
    }
}

//! Offline playtime tracking while warmUP is disconnected from the companion pipe.

#![cfg(any(windows, target_os = "linux"))]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use crate::parental_guard;
use crate::pipe_server::{self, TrackingOwner};
use crate::playtime_store::{
    acknowledge_closed_sessions, closed_sessions_for_wire, finalize_open, finalize_owner_sessions,
    new_external_id, persist_pending, store, unix_now_secs, OpenSession,
};
#[cfg(test)]
use crate::protocol::LibraryWatchGameEntry;
use crate::protocol::{
    PlaySessionsAckPayload, PlaySessionsPayload, RunningGame, RunningGamesPayload,
};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EXIT_MISSES: u32 = 5;
const RUNNING_GAMES_INTERVAL: Duration = Duration::from_secs(1);

static RUNNING_GAMES: Mutex<(Option<TrackingOwner>, Option<RunningGamesPayload>)> =
    Mutex::new((None, None));

pub fn spawn_tracker_loop() {
    std::thread::Builder::new()
        .name("warmup-playtime-tracker".into())
        .spawn(tracker_loop)
        .ok();
    std::thread::Builder::new()
        .name("warmup-running-games".into())
        .spawn(running_games_loop)
        .ok();
}

pub fn set_running_games_owner(owner: Option<&TrackingOwner>) {
    if let Ok(mut slot) = RUNNING_GAMES.lock() {
        *slot = (owner.cloned(), None);
    }
}

pub fn current_running_games() -> Option<RunningGamesPayload> {
    RUNNING_GAMES.lock().ok()?.1.clone()
}

fn running_games_loop() {
    loop {
        let owner = RUNNING_GAMES.lock().ok().and_then(|slot| slot.0.clone());
        if let Some(owner) = owner {
            let payload = compute_running_games(&owner);
            if let Ok(mut slot) = RUNNING_GAMES.lock() {
                if slot.0.as_ref() == Some(&owner) {
                    slot.1 = Some(payload);
                }
            }
        }
        std::thread::sleep(RUNNING_GAMES_INTERVAL);
    }
}

fn compute_running_games(owner: &TrackingOwner) -> RunningGamesPayload {
    let games = crate::library_watch::current_games()
        .filter(|(watch_owner, _)| watch_owner == owner)
        .map(|(_, games)| games)
        .unwrap_or_default();
    let our_pid = std::process::id();
    let processes: Vec<(u32, String, Option<String>)> =
        parental_guard::snapshot_processes_for_owner(owner)
            .into_iter()
            .filter(|(pid, _, _)| *pid != 0 && *pid != our_pid)
            .collect();
    let fullscreen_pid = foreground_fullscreen_pid();
    let mut by_game: BTreeMap<String, RunningGame> = BTreeMap::new();
    for matched in crate::game_list::match_processes(
        &processes,
        &games,
        crate::process_guard::steam_running_app_id,
    ) {
        let fullscreen = fullscreen_pid == Some(matched.pid);
        let key = match &matched.game_id {
            Some(game_id) => format!("game:{game_id}"),
            None => format!("image:{}", crate::game_list::normalize(&matched.image_path)),
        };
        let entry = RunningGame {
            game_id: matched.game_id,
            pid: matched.pid,
            exe_stem: matched.exe_stem,
            image_path: matched.image_path,
            steam_app_id: matched.steam_app_id,
            fullscreen,
        };
        match by_game.get_mut(&key) {
            Some(existing) => {
                let fullscreen = existing.fullscreen || fullscreen;
                if entry.pid < existing.pid {
                    *existing = entry;
                }
                existing.fullscreen = fullscreen;
            }
            None => {
                by_game.insert(key, entry);
            }
        }
    }
    RunningGamesPayload {
        games: by_game.into_values().collect(),
    }
}

#[cfg(windows)]
fn foreground_fullscreen_pid() -> Option<u32> {
    crate::win::game_detect::foreground_fullscreen_pid()
}

#[cfg(target_os = "linux")]
fn foreground_fullscreen_pid() -> Option<u32> {
    crate::linux_game_detect::foreground_fullscreen_pid()
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
    for matched in crate::game_list::match_processes(
        &processes,
        &games,
        crate::process_guard::steam_running_app_id,
    ) {
        if let Some(game_id) = matched.game_id {
            active_by_game.entry(game_id).or_insert(matched.pid);
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

#[cfg(test)]
fn match_process_to_game(
    exe_name: &str,
    image_path: Option<&str>,
    games: &[LibraryWatchGameEntry],
) -> Option<String> {
    let processes = [(0, exe_name.to_string(), image_path.map(str::to_string))];
    crate::game_list::match_processes(&processes, games, |_| None)
        .into_iter()
        .find_map(|matched| matched.game_id)
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
        let matched = match_process_to_game("game.exe", Some(r"C:\Games\B\bin\game.exe"), &games);
        assert_eq!(matched.as_deref(), Some("b"));
    }
}

//! Offline playtime tracking while warmUP is disconnected from the companion pipe.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::parental_guard;
use crate::pipe_server::{self, TrackingOwner};
use crate::playtime_store::{
    acknowledge_closed_sessions, closed_sessions_for_wire, finalize_open, finalize_owner_sessions,
    new_external_id, persist_pending, store, unix_now_secs, OpenSession,
};
use crate::protocol::{LibraryWatchGameEntry, PlaySessionsAckPayload, PlaySessionsPayload};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EXIT_MISSES: u32 = 5;

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
        let matched = match_process_to_game("game.exe", Some(r"C:\Games\B\bin\game.exe"), &games);
        assert_eq!(matched.as_deref(), Some("b"));
    }
}

//! Pending offline play-session store (disk + session finalize math).

#![cfg(windows)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::tracking_owner::TrackingOwner;
use crate::protocol::{ExternalPlaySession, PlaySessionsAckPayload};

const PENDING_FILE: &str = r"C:\ProgramData\WarmupVk\pending-play-sessions.json";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
static STORE: OnceLock<Mutex<PendingStore>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PendingStore {
    #[serde(default)]
    pub open: Vec<OpenSession>,
    #[serde(default)]
    pub closed: Vec<ClosedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OpenSession {
    pub owner: TrackingOwner,
    pub external_id: String,
    pub game_id: String,
    pub pid: u32,
    pub started_at: i64,
    #[serde(default)]
    pub miss_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ClosedSession {
    pub owner: TrackingOwner,
    pub external_id: String,
    pub game_id: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_minutes: i64,
}

pub(crate) fn store() -> &'static Mutex<PendingStore> {
    STORE.get_or_init(|| Mutex::new(load_pending_from_disk()))
}

pub(crate) fn acknowledge_closed_sessions(
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

pub(crate) fn finalize_open(session: OpenSession, ended_at: i64) -> Option<ClosedSession> {
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

pub(crate) fn finalize_owner_sessions(
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

pub(crate) fn closed_sessions_for_wire(owner: &TrackingOwner) -> Vec<ExternalPlaySession> {
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

pub(crate) fn persist_pending(store: &PendingStore) {
    if let Ok(json) = serde_json::to_string(store) {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\WarmupVk");
        let _ = std::fs::write(PENDING_FILE, json);
    }
}

pub(crate) fn new_external_id(game_id: &str, started_at: i64) -> String {
    let n = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cmp-{game_id}-{started_at}-{n}")
}

pub(crate) fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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

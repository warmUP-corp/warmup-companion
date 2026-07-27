//! Persisted library watchlist for offline playtime tracking while warmUP is closed.

#![cfg(windows)]

use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::pipe_server::TrackingOwner;
use crate::protocol::{LibraryWatchGameEntry, LibraryWatchPayload};

const WATCH_FILE: &str = r"C:\ProgramData\WarmupVk\library-watch.json";
const MAX_WATCH_GAMES: usize = 256;
const MAX_SELECTORS_PER_GAME: usize = 16;
const MAX_TOTAL_SELECTORS: usize = MAX_WATCH_GAMES * MAX_SELECTORS_PER_GAME;
const MAX_GAME_ID_BYTES: usize = 256;
const MAX_SELECTOR_BYTES: usize = 1024;
const MAX_WATCH_TEXT_BYTES: usize = 60 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedWatch {
    pub owner: TrackingOwner,
    pub watch: LibraryWatchPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchValidationError {
    TooManyGames,
    TooManySelectorsPerGame,
    TooManySelectors,
    EmptyField,
    FieldTooLong,
    PayloadTooLarge,
    StateUnavailable,
}

#[derive(Debug, Clone, Default)]
struct WatchState {
    watch: Option<OwnedWatch>,
}

static STATE: OnceLock<Mutex<WatchState>> = OnceLock::new();

fn state() -> &'static Mutex<WatchState> {
    STATE.get_or_init(|| Mutex::new(WatchState::default()))
}

pub fn validate_watch(payload: &LibraryWatchPayload) -> Result<(), WatchValidationError> {
    if payload.games.len() > MAX_WATCH_GAMES {
        return Err(WatchValidationError::TooManyGames);
    }

    let mut total_selectors = 0usize;
    let mut total_text_bytes = 0usize;
    for game in &payload.games {
        if game.game_id.trim().is_empty() {
            return Err(WatchValidationError::EmptyField);
        }
        if game.game_id.len() > MAX_GAME_ID_BYTES {
            return Err(WatchValidationError::FieldTooLong);
        }

        let selectors = game.exe_stems.len() + game.install_dir_prefixes.len();
        if selectors > MAX_SELECTORS_PER_GAME {
            return Err(WatchValidationError::TooManySelectorsPerGame);
        }
        total_selectors = total_selectors
            .checked_add(selectors)
            .ok_or(WatchValidationError::TooManySelectors)?;
        if total_selectors > MAX_TOTAL_SELECTORS {
            return Err(WatchValidationError::TooManySelectors);
        }

        total_text_bytes = total_text_bytes
            .checked_add(game.game_id.len())
            .ok_or(WatchValidationError::PayloadTooLarge)?;
        for selector in game.exe_stems.iter().chain(&game.install_dir_prefixes) {
            if selector.trim().is_empty() {
                return Err(WatchValidationError::EmptyField);
            }
            if selector.len() > MAX_SELECTOR_BYTES {
                return Err(WatchValidationError::FieldTooLong);
            }
            total_text_bytes = total_text_bytes
                .checked_add(selector.len())
                .ok_or(WatchValidationError::PayloadTooLarge)?;
        }
        if total_text_bytes > MAX_WATCH_TEXT_BYTES {
            return Err(WatchValidationError::PayloadTooLarge);
        }
    }
    Ok(())
}

pub fn apply_watch(
    owner: &TrackingOwner,
    payload: &LibraryWatchPayload,
) -> Result<(), WatchValidationError> {
    validate_watch(payload)?;
    let owned = OwnedWatch {
        owner: owner.clone(),
        watch: payload.clone(),
    };
    state()
        .lock()
        .map_err(|_| WatchValidationError::StateUnavailable)?
        .watch = Some(owned.clone());
    persist_watch(&owned);
    Ok(())
}

pub fn load_persisted_watch() {
    let Ok(raw) = std::fs::read_to_string(WATCH_FILE) else {
        return;
    };
    let Ok(owned) = serde_json::from_str::<OwnedWatch>(&raw) else {
        crate::install::log_line(
            "library watch: rejected legacy or malformed persisted state without an owner",
        );
        return;
    };
    if let Err(error) = validate_watch(&owned.watch) {
        crate::install::log_line(&format!(
            "library watch: rejected persisted state outside security limits: {error:?}"
        ));
        return;
    }
    if let Ok(mut slot) = state().lock() {
        slot.watch = Some(owned);
    }
}

pub fn current_watch() -> Option<OwnedWatch> {
    state().lock().ok()?.watch.clone()
}

pub fn current_games() -> Option<(TrackingOwner, Vec<LibraryWatchGameEntry>)> {
    let owned = current_watch()?;
    owned
        .watch
        .enabled
        .then_some((owned.owner, owned.watch.games))
}

fn persist_watch(owned: &OwnedWatch) {
    if let Ok(json) = serde_json::to_string(owned) {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\WarmupVk");
        let _ = std::fs::write(WATCH_FILE, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: usize) -> LibraryWatchGameEntry {
        LibraryWatchGameEntry {
            game_id: format!("game-{id}"),
            exe_stems: vec![format!("game-{id}")],
            install_dir_prefixes: vec![format!(r"c:\games\game-{id}\")],
        }
    }

    #[test]
    fn watch_limits_accept_a_normal_library() {
        let payload = LibraryWatchPayload {
            enabled: true,
            games: (0..32).map(game).collect(),
        };
        assert_eq!(validate_watch(&payload), Ok(()));
    }

    #[test]
    fn watch_limits_reject_oversized_game_and_selector_sets() {
        let too_many_games = LibraryWatchPayload {
            enabled: true,
            games: (0..=MAX_WATCH_GAMES).map(game).collect(),
        };
        assert_eq!(
            validate_watch(&too_many_games),
            Err(WatchValidationError::TooManyGames)
        );

        let mut selector_heavy = game(0);
        selector_heavy.exe_stems = (0..=MAX_SELECTORS_PER_GAME)
            .map(|index| format!("selector-{index}"))
            .collect();
        assert_eq!(
            validate_watch(&LibraryWatchPayload {
                enabled: true,
                games: vec![selector_heavy],
            }),
            Err(WatchValidationError::TooManySelectorsPerGame)
        );
    }

    #[test]
    fn legacy_watch_without_owner_is_rejected() {
        let legacy = r#"{"enabled":true,"games":[]}"#;
        assert!(serde_json::from_str::<OwnedWatch>(legacy).is_err());
    }
}

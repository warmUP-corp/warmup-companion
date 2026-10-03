//! Persist Kid Mode guard config under ProgramData.

#![cfg(windows)]

use crate::protocol::ParentalGuardPayload;

fn guard_file() -> std::path::PathBuf {
    crate::paths::parental_guard_file()
}

pub(crate) fn load() -> Option<ParentalGuardPayload> {
    let raw = std::fs::read_to_string(guard_file()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) fn save(payload: &ParentalGuardPayload) {
    if let Ok(json) = serde_json::to_string(payload) {
        let path = guard_file();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }
}

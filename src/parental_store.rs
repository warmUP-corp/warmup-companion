//! Persist Kid Mode guard config under ProgramData.

#![cfg(windows)]

use crate::protocol::ParentalGuardPayload;

const GUARD_FILE: &str = r"C:\ProgramData\WarmupVk\parental-guard.json";

pub(crate) fn load() -> Option<ParentalGuardPayload> {
    let raw = std::fs::read_to_string(GUARD_FILE).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) fn save(payload: &ParentalGuardPayload) {
    if let Ok(json) = serde_json::to_string(payload) {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\WarmupVk");
        let _ = std::fs::write(GUARD_FILE, json);
    }
}

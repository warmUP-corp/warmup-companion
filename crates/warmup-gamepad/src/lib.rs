//! Single source of truth: warmUP Tauri `gamepad/input.rs` (SDL3).
//!
//! This crate exists only so `warmup-vk-prototype` can depend on `warmup_gamepad` without
//! duplicating the SDL implementation. The real code lives in the desktop app tree.

#[path = "../../../../Full-Screen-Console-PC-v2-Tauri/apps/desktop/src-tauri/src/gamepad/input.rs"]
mod tauri_desktop_gamepad_input;

pub use tauri_desktop_gamepad_input::*;

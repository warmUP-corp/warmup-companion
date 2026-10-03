//! Named-pipe server (#347): the companion hosts `\\.\pipe\warmup-input` and streams
//! `connection` frames to the warmUP desktop client. The companion is always running,
//! so it is the server; the desktop is a reconnecting client. See
//! `docs/companion-ipc-protocol.md`.
//!
//! The gamepad loop calls [`publish_from_label`] every frame with the active backend's
//! controller label; the server thread streams the latest connection snapshot to the
//! connected client. The pipe is ACL'd to the interactive user.

use crate::gamepad_backend::PadCommand;
use crate::protocol::{
    AxisPayload, BatteryPayload, ButtonPayload, CompanionSettingsPayload, ConnectionPayload,
    ModeSnapshot, ParentalBlockedPayload, ParentalGuardPayload, RumblePayload, TouchpadPayload,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

#[path = "ipc_io.rs"]
mod ipc_io;

/// Cursor mode (A → OS left-click). `false` = focus/D-pad mode (buttons only). Default true;
/// the connected desktop pushes the real value via `config` frames (#349).
static CLICKS_ENABLED: AtomicBool = AtomicBool::new(true);
/// WarmUp webview text entry is active. While true, the companion must not open
/// or drive the native VK; button edges keep flowing to the desktop web VK.
static LAUNCHER_OWNS_TEXT_INPUT: AtomicBool = AtomicBool::new(false);
/// A game session is active (a real game process owns the foreground).
static GAME_ACTIVE: AtomicBool = AtomicBool::new(false);
/// The warmUP launcher window is the foreground surface. The companion sleeps while this is true.
static LAUNCHER_FOREGROUND_NAV: AtomicBool = AtomicBool::new(false);
/// The standalone warmUP browser/overlay owns the foreground experience. Browser mode keeps
/// companion L3/R3 actions local (native VK / voice) instead of forwarding them to the launcher.
static BROWSER_ACTIVE: AtomicBool = AtomicBool::new(false);
/// True while a warmUP desktop client has completed the pipe handshake.
static DESKTOP_CONNECTED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
pub(crate) use crate::tracking_owner::tracking_owner_from_process;
pub(crate) use crate::tracking_owner::TrackingOwner;
pub(crate) use crate::led_engine::apply_led;
pub use crate::device_commands::drain_device_commands;

/// Coalesced visual-cursor hint accumulated since the last send: `(dx, dy, dirty)`.
static CURSOR_ACC: OnceLock<Mutex<(f64, f64, bool)>> = OnceLock::new();

fn cursor_acc() -> &'static Mutex<(f64, f64, bool)> {
    CURSOR_ACC.get_or_init(|| Mutex::new((0.0, 0.0, false)))
}

/// Whether A should inject an OS left-click (cursor mode). Read by the gamepad loop.
pub fn clicks_enabled() -> bool {
    CLICKS_ENABLED.load(Ordering::Relaxed)
}

/// Whether WarmUp's webview VK currently owns controller text entry.
pub fn launcher_owns_text_input() -> bool {
    LAUNCHER_OWNS_TEXT_INPUT.load(Ordering::Relaxed)
}

/// Whether the companion native VK should be suppressed because warmUP owns text
/// entry or an active game handoff is running.
pub fn native_vk_suppressed() -> bool {
    LAUNCHER_OWNS_TEXT_INPUT.load(Ordering::Relaxed)
        || GAME_ACTIVE.load(Ordering::Relaxed)
        || LAUNCHER_FOREGROUND_NAV.load(Ordering::Relaxed)
}

/// Whether a real game session owns the foreground (raw flag from the desktop).
pub fn game_active() -> bool {
    GAME_ACTIVE.load(Ordering::Relaxed)
}

/// Whether the warmUP launcher is the foreground surface over a running game. When true, the
/// companion stays in the full poll mode even though [`game_active`] is set.
pub fn launcher_foreground_nav() -> bool {
    LAUNCHER_FOREGROUND_NAV.load(Ordering::Relaxed)
}

/// Whether the standalone desktop launch chord may open warmUP.
pub fn warmup_launch_allowed() -> bool {
    !game_active() && !launcher_foreground_nav()
}

pub fn browser_active() -> bool {
    BROWSER_ACTIVE.load(Ordering::Relaxed)
}

/// Whether warmUP is connected and should be the source of truth for mode state.
pub fn desktop_connected() -> bool {
    DESKTOP_CONNECTED.load(Ordering::Relaxed)
}

/// Pending `native_vk` request from the desktop: 0 = none, 1 = open, 2 = close.
/// Set by the inbound frame reader, drained by the gamepad loop (which owns VK state).
static NATIVE_VK_REQUEST: AtomicU8 = AtomicU8::new(0);

#[cfg_attr(not(windows), allow(dead_code))]
fn set_native_vk_request(p: &crate::protocol::NativeVkPayload) {
    let v = match p.action.as_str() {
        "open" => 1,
        "close" => 2,
        other => {
            crate::install::log_line(&format!("pipe inbound native_vk unknown action {other:?}"));
            return;
        }
    };
    crate::install::log_line("pipe inbound native_vk action");
    NATIVE_VK_REQUEST.store(v, Ordering::Relaxed);
}

/// Take the pending desktop VK request, if any. `Some(true)` = open, `Some(false)` = close.
pub fn take_native_vk_request() -> Option<bool> {
    match NATIVE_VK_REQUEST.swap(0, Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Accumulate a visual-cursor hint. The companion has already injected the OS move; this
/// only keeps the webview's visual cursor in sync. Coalesced; the server sends it throttled.
pub fn publish_cursor_moved(dx: f64, dy: f64) {
    if let Ok(mut a) = cursor_acc().lock() {
        a.0 += dx;
        a.1 += dy;
        a.2 = true;
    }
}

/// Take the accumulated cursor hint, if any, resetting the accumulator.
#[cfg_attr(not(windows), allow(dead_code))]
fn take_cursor_moved() -> Option<crate::protocol::CursorMovedPayload> {
    let mut a = cursor_acc().lock().ok()?;
    if !a.2 {
        return None;
    }
    let payload = crate::protocol::CursorMovedPayload { dx: a.0, dy: a.1 };
    *a = (0.0, 0.0, false);
    Some(payload)
}

/// Latest battery snapshot from the gamepad loop. The server sends it on change.
static BATTERY: OnceLock<Mutex<Option<BatteryPayload>>> = OnceLock::new();
/// Latest raw stick snapshot from the gamepad loop. The server sends it throttled.
static AXIS: OnceLock<Mutex<Option<AxisPayload>>> = OnceLock::new();
/// Latest touchpad sample + a dirty flag, coalesced like `cursor_moved` and sent throttled.
static TOUCHPAD: OnceLock<Mutex<(Option<TouchpadPayload>, bool)>> = OnceLock::new();
/// Device write commands pushed by inbound `config`/`rumble` frames, drained by the
/// gamepad loop (which owns the backend) and applied to the pad.

fn battery_slot() -> &'static Mutex<Option<BatteryPayload>> {
    BATTERY.get_or_init(|| Mutex::new(None))
}

fn axis_slot() -> &'static Mutex<Option<AxisPayload>> {
    AXIS.get_or_init(|| Mutex::new(None))
}

fn touchpad_slot() -> &'static Mutex<(Option<TouchpadPayload>, bool)> {
    TOUCHPAD.get_or_init(|| Mutex::new((None, false)))
}

/// Publish the latest battery snapshot (read by the server, sent on change).
pub fn publish_battery(percent: i32, charging: bool, wired: bool) {
    if let Ok(mut b) = battery_slot().lock() {
        *b = Some(BatteryPayload {
            percent,
            charging,
            wired,
        });
    }
}

/// Publish the latest raw stick snapshot (read by the server, sent throttled).
pub fn publish_axis(left_x: f32, left_y: f32, right_x: f32, right_y: f32) {
    if let Ok(mut a) = axis_slot().lock() {
        *a = Some(AxisPayload {
            left_x,
            left_y,
            right_x,
            right_y,
        });
    }
}

/// Publish the latest touchpad sample (coalesced; the server sends it throttled).
pub fn publish_touchpad(payload: TouchpadPayload) {
    if let Ok(mut t) = touchpad_slot().lock() {
        *t = (Some(payload), true);
    }
}

/// Current battery snapshot, if any has been published.
#[cfg_attr(not(windows), allow(dead_code))]
fn current_battery() -> Option<BatteryPayload> {
    battery_slot().lock().ok().and_then(|b| *b)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn current_axis() -> Option<AxisPayload> {
    axis_slot().lock().ok().and_then(|a| *a)
}

/// Take the latest touchpad sample if it changed since the last send.
#[cfg_attr(not(windows), allow(dead_code))]
fn take_touchpad() -> Option<TouchpadPayload> {
    let mut t = touchpad_slot().lock().ok()?;
    if !t.1 {
        return None;
    }
    t.1 = false;
    t.0.clone()
}

/// Apply a pushed `config`: write through to the companion's cursor settings (read live by
/// `pc_cursor`) and set the clicks-enabled mode. Maps desktop fields → companion params.
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_config(p: &crate::protocol::ConfigPayload) {
    CLICKS_ENABLED.store(p.clicks_enabled, Ordering::Relaxed);
    // "Enable gamepad cursor" master switch. Persisted like the feel settings so the
    // choice survives a companion restart and still applies while warmUP is not running.
    let _ = crate::config::set_gamepad_setting(
        "cursor_enabled",
        if p.enabled { "true" } else { "false" },
    );
    let _ = crate::config::set_gamepad_setting("cursor_deadzone", &p.deadzone.to_string());
    let _ = crate::config::set_gamepad_setting("cursor_speed", &p.sensitivity.to_string());
    let _ = crate::config::set_gamepad_setting("cursor_accel", &p.acceleration_exp.to_string());
    let _ = crate::config::set_gamepad_setting("scroll_speed", &p.scroll_sensitivity.to_string());
    let _ = crate::config::set_gamepad_setting(
        "natural_scroll",
        if p.natural_scroll { "true" } else { "false" },
    );
    // Clamp below the 0.95 setting ceiling; pc_cursor clamps again at apply time.
    let _ = crate::config::set_gamepad_setting(
        "cursor_smoothing",
        &p.cursor_smoothing.clamp(0.0, 0.9).to_string(),
    );
    if let Some(theme) = &p.keyboard_theme {
        let _ = crate::config::set_keyboard_theme(&keyboard_theme_from_payload(theme));
    }
    if let Some(mode) = &p.vk_mode {
        let _ = crate::config::set_gamepad_setting("vk_mode", mode);
    }
    crate::led_engine::apply_led_config(p);
}

/// Queue a one-shot rumble command from an inbound `rumble` frame.
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_rumble(p: &RumblePayload) {
    let cmd = match *p {
        RumblePayload::Full {
            strong,
            weak,
            duration_ms,
        } => {
            crate::install::log_line("pipe inbound rumble full");
            PadCommand::Rumble {
                strong,
                weak,
                ms: duration_ms,
            }
        }
        RumblePayload::Triggers {
            left,
            right,
            duration_ms,
        } => {
            crate::install::log_line("pipe inbound rumble triggers");
            PadCommand::TriggerRumble {
                left,
                right,
                ms: duration_ms,
            }
        }
    };
    crate::device_commands::push_device_command(cmd);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn keyboard_theme_from_payload(
    p: &crate::protocol::KeyboardThemePayload,
) -> crate::config::KeyboardTheme {
    crate::config::KeyboardTheme {
        bg: p
            .background
            .as_deref()
            .and_then(crate::config::parse_theme_color),
        key: p.key.as_deref().and_then(crate::config::parse_theme_color),
        accent: p
            .accent
            .as_deref()
            .and_then(crate::config::parse_theme_color),
        text: p.text.as_deref().and_then(crate::config::parse_theme_color),
        sel_text: p
            .selected_text
            .as_deref()
            .and_then(crate::config::parse_theme_color),
        border: p
            .border
            .as_deref()
            .and_then(crate::config::parse_theme_color),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn apply_mode(p: &ModeSnapshot) {
    crate::install::log_line(&format!(
        "pipe mode: game_active={} launcher_nav={} clicks={} launcher_text={} browser={}",
        p.game_active,
        p.launcher_foreground_nav,
        p.clicks_enabled,
        p.launcher_owns_text_input,
        p.browser_active
    ));
    CLICKS_ENABLED.store(p.clicks_enabled, Ordering::Relaxed);
    LAUNCHER_OWNS_TEXT_INPUT.store(p.launcher_owns_text_input, Ordering::Relaxed);
    GAME_ACTIVE.store(p.game_active, Ordering::Relaxed);
    LAUNCHER_FOREGROUND_NAV.store(p.launcher_foreground_nav, Ordering::Relaxed);
    BROWSER_ACTIVE.store(p.browser_active, Ordering::Relaxed);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn clear_desktop_mode() {
    LAUNCHER_OWNS_TEXT_INPUT.store(false, Ordering::Relaxed);
    GAME_ACTIVE.store(false, Ordering::Relaxed);
    LAUNCHER_FOREGROUND_NAV.store(false, Ordering::Relaxed);
    BROWSER_ACTIVE.store(false, Ordering::Relaxed);
}

/// Apply companion-local settings pushed by warmUP (protocol v4 `companion_settings` frame).
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_companion_settings(p: &CompanionSettingsPayload) {
    if let Some(v) = p.sleep_on_game {
        let _ =
            crate::config::set_gamepad_setting("sleep_on_game", if v { "true" } else { "false" });
    }
    if let Some(v) = p.auto_stop_on_game {
        let _ = crate::config::set_gamepad_setting(
            "auto_stop_on_game",
            if v { "true" } else { "false" },
        );
    }
    if let Some(v) = p.prompt_userland_debug {
        let _ = crate::config::set_prompt_userland_debug(v);
    }
    if let Some(v) = p.userland_poll_paused {
        crate::gamepad_backend::set_userland_poll_paused(v);
        let _ = crate::config::write_userland_poll_paused(v);
    }
}

/// Latest connection snapshot, published by the gamepad loop and read by the server.
static STATE: OnceLock<Mutex<ConnectionPayload>> = OnceLock::new();

/// Outbound button-edge queue (drained by the server). Bounded so a slow/absent client
/// cannot grow it without bound; oldest edges are dropped first.
static BUTTONS: OnceLock<Mutex<VecDeque<ButtonPayload>>> = OnceLock::new();
const BUTTON_QUEUE_CAP: usize = 256;

static PARENTAL_BLOCKED: OnceLock<Mutex<VecDeque<ParentalBlockedPayload>>> = OnceLock::new();
const PARENTAL_BLOCKED_QUEUE_CAP: usize = 32;

/// Last published `GUIDE` edge state, for consecutive-Guide-edge dedupe (SDL3 can emit
/// duplicate Guide edges on some firmware) — preserves the desktop's old behaviour.
static LAST_GUIDE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn state() -> &'static Mutex<ConnectionPayload> {
    STATE.get_or_init(|| Mutex::new(disconnected()))
}

fn buttons() -> &'static Mutex<VecDeque<ButtonPayload>> {
    BUTTONS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn parental_blocked_queue() -> &'static Mutex<VecDeque<ParentalBlockedPayload>> {
    PARENTAL_BLOCKED.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Queue a Kid Mode block notification for the connected warmUP desktop client.
#[cfg(windows)]
pub fn publish_parental_blocked(payload: ParentalBlockedPayload) {
    if let Ok(mut q) = parental_blocked_queue().lock() {
        while q.len() >= PARENTAL_BLOCKED_QUEUE_CAP {
            q.pop_front();
        }
        q.push_back(payload);
    }
}

#[cfg(not(windows))]
pub fn publish_parental_blocked(_payload: ParentalBlockedPayload) {}

fn last_guide() -> &'static Mutex<Option<bool>> {
    LAST_GUIDE.get_or_init(|| Mutex::new(None))
}

fn disconnected() -> ConnectionPayload {
    ConnectionPayload {
        connected: false,
        controller_type: "generic".into(),
        controller_name: String::new(),
    }
}

/// Map the active backend's controller label to a connection snapshot and store it.
/// `"none"` (the `GamepadPoll` sentinel) or an empty label means no controller.
pub fn connection_name() -> String {
    state()
        .lock()
        .map(|snap| {
            if snap.connected {
                snap.controller_name.clone()
            } else {
                "none".to_string()
            }
        })
        .unwrap_or_else(|_| "none".to_string())
}

pub fn publish_from_label(label: &str) {
    let next = label_to_payload(label);
    if let Ok(mut g) = state().lock() {
        *g = next;
    }
}

fn label_to_payload(label: &str) -> ConnectionPayload {
    if label == "none" || label.is_empty() {
        disconnected()
    } else {
        ConnectionPayload {
            connected: true,
            controller_type: controller_type_for(label),
            controller_name: label.to_string(),
        }
    }
}

/// Best-effort controller family from the human-readable label, using the desktop's
/// existing vocabulary (`xbox` / `ps5` / `ps4` / `switch` / `generic`).
fn controller_type_for(label: &str) -> String {
    let l = label.to_ascii_lowercase();
    if l.contains("xbox") {
        "xbox".into()
    } else if l.contains("dualsense")
        || l.contains("ps5")
        || l.contains("dualshock")
        || l.contains("ps4")
    {
        "playstation".into()
    } else if l.contains("nintendo") || l.contains("switch") || l.contains("pro controller") {
        "switch".into()
    } else {
        "generic".into()
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn current() -> ConnectionPayload {
    state()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| disconnected())
}

/// Queue one button press/release edge for the connected desktop client. `button` is the
/// canonical name (`A`/`GUIDE`/`LT`/…). Consecutive identical `GUIDE` edges are dropped.
/// The `controller_type` rides along from the current connection snapshot.
pub fn publish_button(button: &str, pressed: bool) {
    if button == "GUIDE" {
        if let Ok(mut last) = last_guide().lock() {
            if *last == Some(pressed) {
                return; // consecutive identical Guide edge — drop
            }
            *last = Some(pressed);
        }
    }
    let payload = ButtonPayload {
        button: button.to_string(),
        pressed,
        controller_type: current().controller_type,
    };
    if let Ok(mut q) = buttons().lock() {
        if q.len() >= BUTTON_QUEUE_CAP {
            q.pop_front();
        }
        q.push_back(payload);
    }
}

/// Drain all queued button edges in order (oldest first).
#[cfg_attr(not(windows), allow(dead_code))]
fn drain_buttons() -> Vec<ButtonPayload> {
    buttons()
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn drain_parental_blocked() -> Vec<ParentalBlockedPayload> {
    parental_blocked_queue()
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

#[cfg(all(windows, feature = "gamepad"))]
fn apply_library_watch(owner: &TrackingOwner, payload: &crate::protocol::LibraryWatchPayload) {
    if let Err(error) = crate::library_watch::apply_watch(owner, payload) {
        crate::install::log_line(&format!(
            "library watch rejected without changing state: {error:?}"
        ));
    }
}

#[cfg(not(all(windows, feature = "gamepad")))]
fn apply_library_watch(_owner: &TrackingOwner, _payload: &crate::protocol::LibraryWatchPayload) {}

#[cfg(all(windows, feature = "gamepad"))]
fn apply_parental_guard(payload: &ParentalGuardPayload) {
    crate::parental_guard::apply_guard(payload);
}

#[cfg(not(all(windows, feature = "gamepad")))]
fn apply_parental_guard(_payload: &ParentalGuardPayload) {}

#[cfg(all(windows, feature = "gamepad"))]
fn apply_play_sessions_ack(
    owner: &TrackingOwner,
    payload: &crate::protocol::PlaySessionsAckPayload,
    issued: &std::collections::HashSet<String>,
) -> usize {
    crate::playtime_tracker::apply_play_sessions_ack(owner, payload, issued)
}

#[cfg(not(all(windows, feature = "gamepad")))]
fn apply_play_sessions_ack(
    _owner: &TrackingOwner,
    _payload: &crate::protocol::PlaySessionsAckPayload,
    _issued: &std::collections::HashSet<String>,
) -> usize {
    0
}

/// Drop any edges queued before a client connected (they are stale to the new client),
/// and reset Guide-dedupe state so the first post-connect Guide edge always sends.
#[cfg_attr(not(windows), allow(dead_code))]
fn reset_button_stream() {
    if let Ok(mut q) = buttons().lock() {
        q.clear();
    }
    if let Ok(mut last) = last_guide().lock() {
        *last = None;
    }
}

fn inbound_frame_ready(peeked: &[u8], available: u32) -> bool {
    peeked.contains(&b'\n') || available as usize > peeked.len()
}

#[cfg(any(windows, target_os = "linux"))]
pub fn spawn() {
    std::thread::Builder::new()
        .name("warmup-input-server".into())
        .spawn(|| {
            #[cfg(windows)]
            server::serve_forever();
            #[cfg(target_os = "linux")]
            ipc_io::serve_unix();
        })
        .ok();
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn spawn() {}

#[cfg(windows)]
mod server {
    use super::{
        clear_desktop_mode, reset_button_stream, tracking_owner_from_process, DESKTOP_CONNECTED,
    };
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{
        ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE,
        PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
        PeekNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    const PIPE_NAME: &str = r"\\.\pipe\warmup-input";
    /// SYSTEM full control; the interactive user (the desktop's account) gets read+write.
    const SDDL: &str = "D:(A;;GA;;;SY)(A;;GRGW;;;IU)";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn io_err(msg: &str) -> std::io::Error {
        std::io::Error::other(msg)
    }

    fn to_io(e: windows::core::Error) -> std::io::Error {
        io_err(&e.message())
    }

    pub fn serve_forever() {
        let name = wide(PIPE_NAME);
        loop {
            let Some((sa, sd)) = build_security_attributes() else {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            let handle = unsafe {
                CreateNamedPipeW(
                    PCWSTR(name.as_ptr()),
                    FILE_FLAGS_AND_ATTRIBUTES(
                        PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0,
                    ),
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    0,
                    Some(&sa as *const SECURITY_ATTRIBUTES),
                )
            };
            // The kernel copies the descriptor into the pipe object; free our copy now.
            if !sd.0.is_null() {
                unsafe {
                    let _ = LocalFree(HLOCAL(sd.0));
                }
            }
            if handle == INVALID_HANDLE_VALUE {
                crate::install::log_line(&format!(
                    "pipe bind failed for {PIPE_NAME}: {}",
                    unsafe { GetLastError().0 }
                ));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            serve_one(handle);
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }

    fn build_security_attributes() -> Option<(SECURITY_ATTRIBUTES, PSECURITY_DESCRIPTOR)> {
        let sddl = wide(SDDL);
        let mut psd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                1, // SDDL_REVISION_1
                &mut psd,
                None,
            )
        }
        .ok()?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: psd.0,
            bInheritHandle: false.into(),
        };
        Some((sa, psd))
    }

    fn serve_one(pipe: HANDLE) {
        let _ = unsafe { ConnectNamedPipe(pipe, None) };
        let Some(identity) = client_identity(pipe) else {
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
            }
            return;
        };
        DESKTOP_CONNECTED.store(true, Ordering::Relaxed);
        let mut session = super::ipc_io::Session::new(PipeRead { pipe }, PipeWrite { pipe });
        if let Ok(issued_session_ids) = super::ipc_io::handshake(&mut session, &identity) {
            reset_button_stream();
            super::ipc_io::stream(&mut session, &identity.owner, issued_session_ids);
        }
        DESKTOP_CONNECTED.store(false, Ordering::Relaxed);
        clear_desktop_mode();
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
        }
    }

    fn client_identity(pipe: HANDLE) -> Option<super::ipc_io::ClientIdentity> {
        let mut pid = 0u32;
        if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }.is_err() {
            return None;
        }
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return None;
        };
        let owner = tracking_owner_from_process(process, pid);
        unsafe {
            let _ = CloseHandle(process);
        }
        if owner.is_none() {
            crate::install::log_line(
                "pipe client rejected: process has no interactive non-System owner",
            );
        }
        owner.map(|owner| super::ipc_io::ClientIdentity { owner })
    }

    struct PipeRead {
        pipe: HANDLE,
    }

    impl std::io::Read for PipeRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut avail = 0u32;
            unsafe { PeekNamedPipe(self.pipe, None, 0, None, Some(&mut avail), None) }
                .map_err(to_io)?;
            if avail == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "pipe empty",
                ));
            }
            let mut read = 0u32;
            let take = buf.len().min(avail as usize);
            unsafe { ReadFile(self.pipe, Some(&mut buf[..take]), Some(&mut read), None) }
                .map_err(to_io)?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "pipe closed",
                ));
            }
            Ok(read as usize)
        }
    }

    struct PipeWrite {
        pipe: HANDLE,
    }

    impl std::io::Write for PipeWrite {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut written = 0u32;
            unsafe { WriteFile(self.pipe, Some(buf), Some(&mut written), None) }.map_err(to_io)?;
            if written == 0 {
                return Err(io_err("pipe write returned 0 bytes"));
            }
            Ok(written as usize)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_commands::push_device_command;

    #[test]
    fn none_or_empty_label_is_disconnected() {
        assert!(!label_to_payload("none").connected);
        assert!(!label_to_payload("").connected);
    }

    #[test]
    fn xbox_label_maps_to_xbox_type_and_keeps_name() {
        let p = label_to_payload("Xbox Wireless Controller");
        assert!(p.connected);
        assert_eq!(p.controller_type, "xbox");
        assert_eq!(p.controller_name, "Xbox Wireless Controller");
    }

    #[test]
    fn dualsense_maps_to_playstation() {
        // warmUp's `ControllerType` vocabulary is 'xbox' | 'playstation' | 'switch' | 'generic'.
        assert_eq!(
            label_to_payload("DualSense Wireless Controller").controller_type,
            "playstation"
        );
    }

    #[test]
    fn unknown_pad_is_generic_but_connected() {
        let p = label_to_payload("Acme Arcade Stick");
        assert!(p.connected);
        assert_eq!(p.controller_type, "generic");
    }

    #[test]
    fn publish_updates_current_snapshot() {
        publish_from_label("Xbox 360 Controller");
        assert!(current().connected);
        publish_from_label("none");
        assert!(!current().connected);
    }

    #[test]
    fn button_stream_dedupes_guide_and_preserves_order() {
        reset_button_stream();
        publish_button("A", true);
        publish_button("GUIDE", true);
        publish_button("GUIDE", true); // consecutive identical Guide → dropped
        publish_button("GUIDE", false);
        publish_button("A", true); // non-Guide repeats are kept
        let edges: Vec<(String, bool)> = drain_buttons()
            .into_iter()
            .map(|e| (e.button, e.pressed))
            .collect();
        assert_eq!(
            edges,
            vec![
                ("A".into(), true),
                ("GUIDE".into(), true),
                ("GUIDE".into(), false),
                ("A".into(), true),
            ]
        );
        // A fresh client connect clears any queued edges.
        publish_button("B", true);
        reset_button_stream();
        assert!(drain_buttons().is_empty());
    }

    #[test]
    fn inbound_frames_larger_than_the_peek_buffer_are_drained() {
        assert!(inbound_frame_ready(&[b'x'; 4096], 4097));
        assert!(inbound_frame_ready(b"{}\n", 3));
        assert!(!inbound_frame_ready(b"{", 1));
    }

    #[test]
    fn device_command_drain_coalesces_led_bursts() {
        push_device_command(PadCommand::Led { r: 1, g: 2, b: 3 });
        push_device_command(PadCommand::Led { r: 4, g: 5, b: 6 });
        push_device_command(PadCommand::Rumble {
            strong: 0.1,
            weak: 0.2,
            ms: 30,
        });
        push_device_command(PadCommand::Led { r: 7, g: 8, b: 9 });

        let cmds = drain_device_commands();
        assert_eq!(cmds.len(), 2);
        assert!(matches!(
            cmds[0],
            PadCommand::Rumble {
                strong: 0.1,
                weak: 0.2,
                ms: 30,
            }
        ));
        assert!(matches!(cmds[1], PadCommand::Led { r: 7, g: 8, b: 9 }));
    }

    #[test]
    fn mode_tracks_launcher_text_input_owner() {
        apply_mode(&ModeSnapshot {
            game_active: false,
            launcher_foreground_nav: false,
            clicks_enabled: false,
            launcher_owns_text_input: true,
            browser_active: false,
        });
        assert!(!clicks_enabled());
        assert!(launcher_owns_text_input());

        apply_mode(&ModeSnapshot {
            game_active: false,
            launcher_foreground_nav: false,
            clicks_enabled: true,
            launcher_owns_text_input: false,
            browser_active: false,
        });
        assert!(clicks_enabled());
        assert!(!launcher_owns_text_input());

        // Surface flags drive the poll mode (see `effective_userland_poll_mode`):
        // in-game = a game owns the foreground and the launcher is not over it.
        apply_mode(&ModeSnapshot {
            game_active: true,
            launcher_foreground_nav: false,
            clicks_enabled: false,
            launcher_owns_text_input: false,
            browser_active: false,
        });
        assert!(
            game_active() && !launcher_foreground_nav(),
            "in-game → sleep"
        );

        assert!(!warmup_launch_allowed());
        assert!(native_vk_suppressed());

        // warmUP owns its controller input, including when shown over a game.
        apply_mode(&ModeSnapshot {
            game_active: true,
            launcher_foreground_nav: true,
            clicks_enabled: true,
            launcher_owns_text_input: true,
            browser_active: false,
        });
        assert!(
            game_active() && launcher_foreground_nav(),
            "launcher over game → full"
        );

        assert!(!warmup_launch_allowed());
        assert!(native_vk_suppressed());

        // Disconnect resets every surface flag.
        clear_desktop_mode();
        assert!(!game_active());
        assert!(!launcher_foreground_nav());
        assert!(warmup_launch_allowed());

        apply_mode(&ModeSnapshot {
            game_active: false,
            launcher_foreground_nav: true,
            clicks_enabled: true,
            launcher_owns_text_input: false,
            browser_active: false,
        });
        assert!(!warmup_launch_allowed());
        assert!(native_vk_suppressed());
    }
}

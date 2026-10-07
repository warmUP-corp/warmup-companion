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
    AxisPayload, BatteryPayload, ButtonPayload, CompanionSettingsPayload, ConfigPayload,
    ConnectionPayload, KeyboardThemePayload, ModeSnapshot, ParentalBlockedPayload,
    ParentalGuardPayload, RumblePayload, TouchpadPayload,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

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

pub use crate::device_commands::drain_device_commands;
pub(crate) use crate::led_engine::apply_led;
pub(crate) use crate::tracking_owner::{tracking_owner_from_process, TrackingOwner};

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

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn request_native_vk(open: bool) {
    NATIVE_VK_REQUEST.store(if open { 1 } else { 2 }, Ordering::Relaxed);
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

static PAD_USB: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn publish_usb(usb: Option<bool>) {
    PAD_USB.store(
        match usb {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        },
        Ordering::Relaxed,
    );
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn current_usb() -> Option<bool> {
    match PAD_USB.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
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
pub(crate) fn current_battery() -> Option<BatteryPayload> {
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

#[derive(Debug, Clone, PartialEq)]
struct ConfigChanges {
    settings: Vec<(&'static str, String)>,
    keyboard_theme: Option<KeyboardThemePayload>,
    led: ConfigPayload,
}

#[derive(Debug, Clone, PartialEq)]
struct SettingsSync {
    config_baseline: Option<ConfigPayload>,
    companion_baseline: CompanionSettingsPayload,
}

fn bool_setting(v: bool) -> String {
    if v { "true" } else { "false" }.to_string()
}

fn take_changed_flag(next: Option<bool>, base: &mut Option<bool>) -> Option<bool> {
    let v = next?;
    if *base == Some(v) {
        return None;
    }
    *base = Some(v);
    Some(v)
}

#[cfg_attr(not(windows), allow(dead_code))]
impl SettingsSync {
    fn new(companion_baseline: CompanionSettingsPayload) -> Self {
        Self {
            config_baseline: None,
            companion_baseline,
        }
    }

    fn config(&mut self, p: &ConfigPayload) -> ConfigChanges {
        let mut changes = ConfigChanges {
            settings: Vec::new(),
            keyboard_theme: None,
            led: p.clone(),
        };
        let Some(prev) = self.config_baseline.replace(p.clone()) else {
            return changes;
        };
        let s = &mut changes.settings;
        if prev.enabled != p.enabled {
            s.push(("cursor_enabled", bool_setting(p.enabled)));
        }
        if prev.deadzone != p.deadzone {
            s.push(("cursor_deadzone", p.deadzone.to_string()));
        }
        if prev.sensitivity != p.sensitivity {
            s.push(("cursor_speed", p.sensitivity.to_string()));
        }
        if prev.acceleration_exp != p.acceleration_exp {
            s.push(("cursor_accel", p.acceleration_exp.to_string()));
        }
        if prev.scroll_sensitivity != p.scroll_sensitivity {
            s.push(("scroll_speed", p.scroll_sensitivity.to_string()));
        }
        if prev.natural_scroll != p.natural_scroll {
            s.push(("natural_scroll", bool_setting(p.natural_scroll)));
        }
        if prev.cursor_smoothing != p.cursor_smoothing {
            s.push((
                "cursor_smoothing",
                p.cursor_smoothing.clamp(0.0, 0.9).to_string(),
            ));
        }
        if let Some(mode) = &p.vk_mode {
            if prev.vk_mode.as_ref() != Some(mode) {
                s.push(("vk_mode", mode.clone()));
            }
        }
        if p.keyboard_theme.is_some() && p.keyboard_theme != prev.keyboard_theme {
            changes.keyboard_theme = p.keyboard_theme.clone();
        }
        if prev.led_color == p.led_color {
            changes.led.led_color = None;
        }
        if prev.led_secondary_color == p.led_secondary_color {
            changes.led.led_secondary_color = None;
        }
        if prev.led_effect == p.led_effect {
            changes.led.led_effect = None;
        }
        if prev.led_brightness == p.led_brightness {
            changes.led.led_brightness = None;
        }
        changes
    }

    fn companion_settings(&mut self, p: &CompanionSettingsPayload) -> CompanionSettingsPayload {
        let base = &mut self.companion_baseline;
        CompanionSettingsPayload {
            sleep_on_game: take_changed_flag(p.sleep_on_game, &mut base.sleep_on_game),
            auto_stop_on_game: take_changed_flag(p.auto_stop_on_game, &mut base.auto_stop_on_game),
            userland_poll_paused: take_changed_flag(
                p.userland_poll_paused,
                &mut base.userland_poll_paused,
            ),
            prompt_userland_debug: take_changed_flag(
                p.prompt_userland_debug,
                &mut base.prompt_userland_debug,
            ),
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn companion_snapshot() -> (ConfigPayload, CompanionSettingsPayload) {
    let g = crate::config::gamepad_settings();
    let vk_mode = match crate::config::vk_layout_mode() {
        crate::config::VkLayoutMode::Docked => "docked",
        crate::config::VkLayoutMode::Floating => "floating",
    };
    let config = ConfigPayload {
        deadzone: g.cursor_deadzone,
        sensitivity: g.cursor_speed,
        acceleration_exp: g.cursor_accel,
        scroll_sensitivity: g.scroll_speed,
        enabled: g.cursor_enabled,
        clicks_enabled: clicks_enabled(),
        led_color: None,
        led_secondary_color: None,
        led_effect: None,
        led_brightness: None,
        natural_scroll: g.natural_scroll,
        cursor_smoothing: g.cursor_smoothing,
        keyboard_theme: None,
        vk_mode: Some(vk_mode.to_string()),
    };
    let settings = CompanionSettingsPayload {
        sleep_on_game: Some(g.sleep_on_game),
        auto_stop_on_game: Some(g.auto_stop_on_game),
        userland_poll_paused: Some(crate::config::read_userland_poll_paused()),
        prompt_userland_debug: Some(crate::config::prompt_userland_debug()),
    };
    (config, settings)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn apply_config(sync: &mut SettingsSync, p: &ConfigPayload) {
    CLICKS_ENABLED.store(p.clicks_enabled, Ordering::Relaxed);
    let first = sync.config_baseline.is_none();
    let changes = sync.config(p);
    if first {
        crate::install::log_line("pipe config: first frame seeds baseline, settings not applied");
    } else if !changes.settings.is_empty() || changes.keyboard_theme.is_some() {
        let mut keys: Vec<&str> = changes.settings.iter().map(|(k, _)| *k).collect();
        if changes.keyboard_theme.is_some() {
            keys.push("keyboard_theme");
        }
        crate::install::log_line(&format!("pipe config applied: {}", keys.join(",")));
    }
    for (key, value) in &changes.settings {
        let _ = crate::config::set_gamepad_setting(key, value);
    }
    if let Some(theme) = &changes.keyboard_theme {
        let _ = crate::config::set_keyboard_theme(&keyboard_theme_from_payload(theme));
    }
    crate::led_engine::apply_led_config(&changes.led);
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
fn apply_companion_settings(sync: &mut SettingsSync, p: &CompanionSettingsPayload) {
    let p = sync.companion_settings(p);
    if p == CompanionSettingsPayload::default() {
        return;
    }
    let keys: Vec<&str> = [
        ("sleep_on_game", p.sleep_on_game),
        ("auto_stop_on_game", p.auto_stop_on_game),
        ("prompt_userland_debug", p.prompt_userland_debug),
        ("userland_poll_paused", p.userland_poll_paused),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|_| k))
    .collect();
    crate::install::log_line(&format!(
        "pipe companion_settings applied: {}",
        keys.join(",")
    ));
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

#[cfg(all(windows, not(feature = "gamepad")))]
fn apply_library_watch(_owner: &TrackingOwner, _payload: &crate::protocol::LibraryWatchPayload) {}

#[cfg(all(windows, feature = "gamepad"))]
fn apply_parental_guard(payload: &ParentalGuardPayload) {
    crate::parental_guard::apply_guard(payload);
}

#[cfg(all(windows, not(feature = "gamepad")))]
fn apply_parental_guard(_payload: &ParentalGuardPayload) {}

#[cfg(all(windows, feature = "gamepad"))]
fn apply_play_sessions_ack(
    owner: &TrackingOwner,
    payload: &crate::protocol::PlaySessionsAckPayload,
    issued: &std::collections::HashSet<String>,
) -> usize {
    crate::playtime_tracker::apply_play_sessions_ack(owner, payload, issued)
}

#[cfg(all(windows, not(feature = "gamepad")))]
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

#[cfg(test)]
fn pipe_shutdown_refused(kid_mode_blocking: bool) -> bool {
    kid_mode_blocking
}

#[cfg(test)]
fn parental_security_log_writable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Start the pipe server on its own thread. No-op on non-Windows (there the desktop
/// owns input in-process, so there is no companion to serve).
#[cfg(windows)]
pub fn spawn() {
    std::thread::Builder::new()
        .name("warmup-pipe-server".into())
        .spawn(server::serve_forever)
        .ok();
}

#[cfg(not(windows))]
pub fn spawn() {}

#[cfg(windows)]
mod server {
    use super::{
        apply_companion_settings, apply_config, apply_led, apply_library_watch, apply_mode,
        apply_parental_guard, apply_play_sessions_ack, apply_rumble, clear_desktop_mode,
        companion_snapshot, current, current_axis, current_battery, drain_buttons,
        drain_parental_blocked, inbound_frame_ready, reset_button_stream, set_native_vk_request,
        take_cursor_moved, take_touchpad, tracking_owner_from_process, SettingsSync, TrackingOwner,
        DESKTOP_CONNECTED,
    };
    use crate::protocol::{
        is_supported_protocol_version, AxisPayload, BatteryPayload, ConnectionPayload, DownFrame,
        Hello, UpFrame, PROTOCOL_VERSION,
    };
    use std::collections::HashSet;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
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
    /// Re-send the current snapshot at least this often so a dropped idle client is noticed
    /// (the write fails) and the server loops back to accept a new one.
    const KEEPALIVE: Duration = Duration::from_secs(1);
    /// Throttle for outbound `cursor_moved` hints (the OS cursor already moved; this just
    /// keeps the webview's visual cursor in sync).
    const CURSOR_HINT_INTERVAL: Duration = Duration::from_millis(100);

    struct ClientIdentity {
        owner: TrackingOwner,
    }

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

    /// Handle one client connection start-to-finish, then disconnect the pipe instance.
    fn serve_one(pipe: HANDLE) {
        // Block until a client connects. ERROR_PIPE_CONNECTED (client beat us to it) is fine.
        let _ = unsafe { ConnectNamedPipe(pipe, None) };
        let Some(identity) = client_identity(pipe) else {
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
            }
            return;
        };
        DESKTOP_CONNECTED.store(true, Ordering::Relaxed);
        if let Ok((issued_session_ids, sync)) = handshake(pipe, &identity) {
            // Drop edges queued before this client connected (stale to it).
            reset_button_stream();
            stream(pipe, &identity.owner, issued_session_ids, sync);
        }
        DESKTOP_CONNECTED.store(false, Ordering::Relaxed);
        clear_desktop_mode();
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
        }
    }

    fn client_identity(pipe: HANDLE) -> Option<ClientIdentity> {
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
        owner.map(|owner| ClientIdentity { owner })
    }

    /// Read the client `hello`, reject unsupported versions, reply with the negotiated version.
    fn handshake(
        pipe: HANDLE,
        identity: &ClientIdentity,
    ) -> std::io::Result<(HashSet<String>, SettingsSync)> {
        let line = read_line(pipe)?;
        let protocol_version;
        match DownFrame::parse_line(line.trim_end()) {
            Ok(DownFrame::Hello(h)) if is_supported_protocol_version(h.protocol_version) => {
                protocol_version = h.protocol_version;
                if let Some(mode) = h.mode {
                    apply_mode(&mode);
                }
                if let Some(guard) = h.parental_guard {
                    apply_parental_guard(&guard);
                }
                if let Some(watch) = h.library_watch {
                    apply_library_watch(&identity.owner, &watch);
                }
            }
            Ok(DownFrame::Hello(h)) => {
                return Err(io_err(&format!(
                    "hello rejected: client protocol_version={}, companion expects {PROTOCOL_VERSION}",
                    h.protocol_version
                )));
            }
            _ => return Err(io_err("hello rejected: missing or invalid hello frame")),
        }
        let (config_snapshot, settings_snapshot) = companion_snapshot();
        let sync = SettingsSync::new(settings_snapshot.clone());
        let reply = UpFrame::Hello(Hello {
            protocol_version,
            config: serde_json::to_value(config_snapshot).ok(),
            mode: None,
            companion_settings: Some(settings_snapshot),
            parental_guard: None,
            library_watch: None,
        });
        write_all(pipe, reply.to_ndjson_line().as_bytes())?;
        #[cfg(feature = "gamepad")]
        {
            crate::playtime_tracker::on_desktop_connected(&identity.owner);
            if protocol_version == PROTOCOL_VERSION {
                return flush_play_sessions(pipe, &identity.owner).map(|issued| (issued, sync));
            }
        }
        Ok((HashSet::new(), sync))
    }

    /// Push any closed offline sessions to warmUP immediately after handshake.
    fn flush_play_sessions(
        pipe: HANDLE,
        owner: &TrackingOwner,
    ) -> std::io::Result<HashSet<String>> {
        let payload = crate::playtime_tracker::take_closed_sessions_for_flush(owner);
        if payload.sessions.is_empty() {
            return Ok(HashSet::new());
        }
        let issued = payload
            .sessions
            .iter()
            .map(|session| session.external_id.clone())
            .collect();
        write_all(
            pipe,
            UpFrame::PlaySessions(payload).to_ndjson_line().as_bytes(),
        )?;
        Ok(issued)
    }

    /// Full-duplex session: drain inbound `config` (non-blocking), and write button edges
    /// (low latency), `cursor_moved` hints (throttled), and connection snapshots (on change
    /// plus a keepalive so a dropped idle client is noticed via the write error).
    fn stream(
        pipe: HANDLE,
        owner: &TrackingOwner,
        mut issued_session_ids: HashSet<String>,
        mut sync: SettingsSync,
    ) {
        let mut last: Option<ConnectionPayload> = None;
        let mut last_battery: Option<BatteryPayload> = None;
        let mut last_axis: Option<AxisPayload> = None;
        let mut last_conn_write = Instant::now()
            .checked_sub(KEEPALIVE)
            .unwrap_or_else(Instant::now);
        let mut last_cursor_write = Instant::now();
        loop {
            // Inbound config — never block the writer; only read a line that is fully buffered.
            if drain_inbound_config(pipe, owner, &mut issued_session_ids, &mut sync).is_err() {
                return;
            }
            for edge in drain_buttons() {
                if write_all(pipe, UpFrame::Button(edge).to_ndjson_line().as_bytes()).is_err() {
                    return; // client gone — return to accept a new one
                }
            }
            for blocked in drain_parental_blocked() {
                if write_all(
                    pipe,
                    UpFrame::ParentalBlocked(blocked)
                        .to_ndjson_line()
                        .as_bytes(),
                )
                .is_err()
                {
                    return;
                }
            }
            // Battery — send only when it changes (low-rate; no keepalive needed).
            let battery = current_battery();
            if battery.is_some() && battery != last_battery {
                if let Some(b) = battery {
                    if write_all(pipe, UpFrame::Battery(b).to_ndjson_line().as_bytes()).is_err() {
                        return;
                    }
                    last_battery = Some(b);
                }
            }
            if last_cursor_write.elapsed() >= CURSOR_HINT_INTERVAL {
                if let Some(hint) = take_cursor_moved() {
                    if write_all(pipe, UpFrame::CursorMoved(hint).to_ndjson_line().as_bytes())
                        .is_err()
                    {
                        return;
                    }
                }
                // Touchpad shares the cursor throttle (both are ≈100 ms visual hints).
                let axis = current_axis();
                if axis.is_some() && axis != last_axis {
                    if let Some(a) = axis {
                        if write_all(pipe, UpFrame::Axis(a).to_ndjson_line().as_bytes()).is_err() {
                            return;
                        }
                        last_axis = Some(a);
                    }
                }
                if let Some(tp) = take_touchpad() {
                    if write_all(pipe, UpFrame::Touchpad(tp).to_ndjson_line().as_bytes()).is_err() {
                        return;
                    }
                }
                last_cursor_write = Instant::now();
            }
            let cur = current();
            if last.as_ref() != Some(&cur) || last_conn_write.elapsed() >= KEEPALIVE {
                if write_all(
                    pipe,
                    UpFrame::Connection(cur.clone()).to_ndjson_line().as_bytes(),
                )
                .is_err()
                {
                    return;
                }
                last = Some(cur);
                last_conn_write = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Read and apply any fully-buffered `config` lines without blocking the write side.
    fn drain_inbound_config(
        pipe: HANDLE,
        owner: &TrackingOwner,
        issued_session_ids: &mut HashSet<String>,
        sync: &mut SettingsSync,
    ) -> std::io::Result<()> {
        while peek_has_newline(pipe)? {
            let line = read_line(pipe)?;
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            match DownFrame::parse_line(trimmed) {
                Ok(DownFrame::Config(p)) => apply_config(sync, &p),
                Ok(DownFrame::Mode(p)) => apply_mode(&p),
                Ok(DownFrame::Rumble(p)) => apply_rumble(&p),
                Ok(DownFrame::Led(p)) => apply_led(&p),
                Ok(DownFrame::CompanionSettings(p)) => apply_companion_settings(sync, &p),
                Ok(DownFrame::NativeVk(p)) => set_native_vk_request(&p),
                Ok(DownFrame::ParentalGuard(p)) => apply_parental_guard(&p),
                Ok(DownFrame::LibraryWatch(p)) => apply_library_watch(owner, &p),
                Ok(DownFrame::PlaySessionsAck(p)) => {
                    let removed = apply_play_sessions_ack(owner, &p, issued_session_ids);
                    if removed != 0 {
                        issued_session_ids
                            .retain(|external_id| !p.external_ids.contains(external_id));
                    }
                }
                Ok(DownFrame::ShutDown(_)) => handle_shut_down(),
                // A malformed known-type frame is a contract break (e.g. the rumble
                // durationMs mismatch) — log it instead of dropping silently.
                Err(e) => {
                    crate::install::log_line(&format!("pipe inbound frame parse error: {e}"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Read once a newline is visible or the frame exceeds the peek buffer; the latter must
    /// be drained so a large frame cannot permanently hide its newline.
    fn peek_has_newline(pipe: HANDLE) -> std::io::Result<bool> {
        let mut buf = [0u8; 4096];
        let mut read = 0u32;
        let mut avail = 0u32;
        unsafe {
            PeekNamedPipe(
                pipe,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                buf.len() as u32,
                Some(&mut read),
                Some(&mut avail),
                None,
            )
        }
        .map_err(to_io)?;
        Ok(inbound_frame_ready(&buf[..read as usize], avail))
    }

    fn read_line(pipe: HANDLE) -> std::io::Result<String> {
        let mut out: Vec<u8> = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        loop {
            let mut read = 0u32;
            unsafe { ReadFile(pipe, Some(&mut byte), Some(&mut read), None) }.map_err(to_io)?;
            if read == 0 {
                break; // EOF
            }
            if byte[0] == b'\n' {
                break;
            }
            out.push(byte[0]);
            if out.len() > 64 * 1024 {
                return Err(io_err("hello line too long"));
            }
        }
        String::from_utf8(out).map_err(|_| io_err("hello not valid UTF-8"))
    }

    fn handle_shut_down() {
        #[cfg(feature = "gamepad")]
        {
            if crate::parental_guard::is_blocking_active() {
                crate::install::log_line("pipe shut_down refused: Kid Mode blocking is active");
                crate::parental_guard::log_refused_shutdown();
                return;
            }
        }
        crate::install::request_service_stop();
        #[cfg(feature = "gamepad")]
        crate::gamepad::request_stop();
    }

    fn write_all(pipe: HANDLE, mut buf: &[u8]) -> std::io::Result<()> {
        while !buf.is_empty() {
            let mut written = 0u32;
            unsafe { WriteFile(pipe, Some(buf), Some(&mut written), None) }.map_err(to_io)?;
            if written == 0 {
                return Err(io_err("pipe write returned 0 bytes"));
            }
            buf = &buf[written as usize..];
        }
        Ok(())
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
    fn pipe_shutdown_is_refused_only_when_kid_mode_blocks() {
        assert!(!pipe_shutdown_refused(false));
        assert!(pipe_shutdown_refused(true));
    }

    #[test]
    fn parental_security_log_is_used_only_when_the_file_exists() {
        let missing = std::path::Path::new("no-such-parental-security.log");
        assert!(!parental_security_log_writable(missing));
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "warmup-parental-security-{}.log",
            std::process::id()
        ));
        std::fs::write(&path, b"").unwrap();
        assert!(parental_security_log_writable(&path));
        let _ = std::fs::remove_file(&path);
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

    fn sample_config() -> ConfigPayload {
        ConfigPayload {
            deadzone: 0.1,
            sensitivity: 1.0,
            acceleration_exp: 2.0,
            scroll_sensitivity: 1.0,
            enabled: true,
            clicks_enabled: true,
            led_color: Some("#ff0000".into()),
            led_secondary_color: Some("#0000ff".into()),
            led_effect: Some("solid".into()),
            led_brightness: Some(0.5),
            natural_scroll: false,
            cursor_smoothing: 0.3,
            keyboard_theme: None,
            vk_mode: Some("docked".into()),
        }
    }

    fn sample_settings() -> CompanionSettingsPayload {
        CompanionSettingsPayload {
            sleep_on_game: Some(true),
            auto_stop_on_game: Some(false),
            userland_poll_paused: Some(false),
            prompt_userland_debug: Some(false),
        }
    }

    #[test]
    fn first_config_frame_seeds_baseline_and_passes_led() {
        let mut sync = SettingsSync::new(sample_settings());
        let p = sample_config();
        let changes = sync.config(&p);
        assert!(changes.settings.is_empty());
        assert!(changes.keyboard_theme.is_none());
        assert_eq!(changes.led, p);
        assert_eq!(sync.config_baseline, Some(p));
    }

    #[test]
    fn later_config_frame_writes_only_changed_fields() {
        let mut sync = SettingsSync::new(sample_settings());
        let p = sample_config();
        sync.config(&p);
        let mut next = p.clone();
        next.sensitivity = 1.5;
        let changes = sync.config(&next);
        assert_eq!(changes.settings, vec![("cursor_speed", "1.5".to_string())]);
        assert!(changes.keyboard_theme.is_none());
        assert_eq!(changes.led.led_color, None);
        assert_eq!(changes.led.led_secondary_color, None);
        assert_eq!(changes.led.led_effect, None);
        assert_eq!(changes.led.led_brightness, None);
        assert!(sync.config(&next).settings.is_empty());
    }

    #[test]
    fn changed_led_fields_pass_through_and_others_clear() {
        let mut sync = SettingsSync::new(sample_settings());
        sync.config(&sample_config());
        let mut next = sample_config();
        next.led_effect = Some("rainbow".into());
        let changes = sync.config(&next);
        assert!(changes.settings.is_empty());
        assert_eq!(changes.led.led_effect.as_deref(), Some("rainbow"));
        assert_eq!(changes.led.led_color, None);
        assert_eq!(changes.led.led_brightness, None);
    }

    #[test]
    fn changed_theme_and_vk_mode_are_reported() {
        let mut sync = SettingsSync::new(sample_settings());
        sync.config(&sample_config());
        let theme = KeyboardThemePayload {
            background: Some("#101010".into()),
            key: None,
            accent: None,
            text: None,
            selected_text: None,
            border: None,
        };
        let mut next = sample_config();
        next.vk_mode = Some("floating".into());
        next.keyboard_theme = Some(theme.clone());
        next.cursor_smoothing = 0.95;
        let changes = sync.config(&next);
        assert_eq!(
            changes.settings,
            vec![
                ("cursor_smoothing", 0.9f32.to_string()),
                ("vk_mode", "floating".to_string()),
            ]
        );
        assert_eq!(changes.keyboard_theme, Some(theme));
        let again = sync.config(&next);
        assert!(again.settings.is_empty());
        assert!(again.keyboard_theme.is_none());
        let mut absent = next.clone();
        absent.vk_mode = None;
        assert!(sync.config(&absent).settings.is_empty());
    }

    #[test]
    fn companion_settings_apply_only_differences_from_baseline() {
        let mut sync = SettingsSync::new(sample_settings());
        assert_eq!(
            sync.companion_settings(&sample_settings()),
            CompanionSettingsPayload::default()
        );
        let mut next = sample_settings();
        next.auto_stop_on_game = Some(true);
        next.sleep_on_game = None;
        assert_eq!(
            sync.companion_settings(&next),
            CompanionSettingsPayload {
                auto_stop_on_game: Some(true),
                ..Default::default()
            }
        );
        assert_eq!(
            sync.companion_settings(&next),
            CompanionSettingsPayload::default()
        );
        assert_eq!(sync.companion_baseline.sleep_on_game, Some(true));
    }
}

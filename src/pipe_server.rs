//! Named-pipe server (#347): the companion hosts `\\.\pipe\warmup-input` and streams
//! `connection` frames to the warmUP desktop client. The companion is always running,
//! so it is the server; the desktop is a reconnecting client (ADR 0002 /
//! `docs/companion-ipc-protocol.md`).
//!
//! The gamepad loop calls [`publish_from_label`] every frame with the active backend's
//! controller label; the server thread streams the latest connection snapshot to the
//! connected client. The pipe is ACL'd to the interactive user.

use crate::gamepad_backend::PadCommand;
use crate::protocol::{
    AxisPayload, BatteryPayload, ButtonPayload, CompanionSettingsPayload, ConnectionPayload,
    ModeSnapshot, RumblePayload, TouchpadPayload,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

/// Cursor mode (A → OS left-click). `false` = focus/D-pad mode (buttons only). Default true;
/// the connected desktop pushes the real value via `config` frames (#349).
static CLICKS_ENABLED: AtomicBool = AtomicBool::new(true);
/// WarmUp webview text entry is active. While true, the companion must not open
/// or drive the native VK; button edges keep flowing to the desktop web VK.
static LAUNCHER_OWNS_TEXT_INPUT: AtomicBool = AtomicBool::new(false);
/// A game session is active (a real game process owns the foreground). Combined with
/// [`LAUNCHER_FOREGROUND_NAV`] this selects the poll mode: see [`launcher_foreground_nav`].
static GAME_ACTIVE: AtomicBool = AtomicBool::new(false);
/// The warmUP launcher window is the foreground surface. When a game session is active but the
/// user has brought the launcher forward (Guide-wake over a running game), this stays the full
/// poll mode instead of sleeping, so the controller can navigate the launcher.
static LAUNCHER_FOREGROUND_NAV: AtomicBool = AtomicBool::new(false);
/// True while a warmUP desktop client has completed the pipe handshake.
static DESKTOP_CONNECTED: AtomicBool = AtomicBool::new(false);

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
    LAUNCHER_OWNS_TEXT_INPUT.load(Ordering::Relaxed) || GAME_ACTIVE.load(Ordering::Relaxed)
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

/// Whether warmUP is connected and should be the source of truth for mode state.
pub fn desktop_connected() -> bool {
    DESKTOP_CONNECTED.load(Ordering::Relaxed)
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
static DEVICE_CMDS: OnceLock<Mutex<VecDeque<PadCommand>>> = OnceLock::new();
const DEVICE_CMD_CAP: usize = 64;

fn battery_slot() -> &'static Mutex<Option<BatteryPayload>> {
    BATTERY.get_or_init(|| Mutex::new(None))
}

fn axis_slot() -> &'static Mutex<Option<AxisPayload>> {
    AXIS.get_or_init(|| Mutex::new(None))
}

fn touchpad_slot() -> &'static Mutex<(Option<TouchpadPayload>, bool)> {
    TOUCHPAD.get_or_init(|| Mutex::new((None, false)))
}

fn device_cmds() -> &'static Mutex<VecDeque<PadCommand>> {
    DEVICE_CMDS.get_or_init(|| Mutex::new(VecDeque::new()))
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

/// Queue a device write command (bounded; oldest dropped first).
#[cfg_attr(not(windows), allow(dead_code))]
fn push_device_command(cmd: PadCommand) {
    if let Ok(mut q) = device_cmds().lock() {
        if q.len() >= DEVICE_CMD_CAP {
            q.pop_front();
        }
        q.push_back(cmd);
    }
}

/// Drain queued device write commands (LED/rumble) for the gamepad loop to apply.
pub fn drain_device_commands() -> Vec<PadCommand> {
    device_cmds()
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

/// Lightbar animation the companion drives. Mirrors the desktop `ledEffect` vocabulary.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LedEffect {
    Solid,
    Breathing,
    Rainbow,
    Off,
}

impl LedEffect {
    fn parse(s: &str) -> Self {
        match s {
            "off" => Self::Off,
            "breathing" => Self::Breathing,
            "rainbow" => Self::Rainbow,
            _ => Self::Solid,
        }
    }
}

/// Desired lightbar state, set from `config` frames and rendered by the LED engine thread.
#[derive(Clone, Copy)]
struct LedState {
    effect: LedEffect,
    /// Base colour (true RGB channels).
    r: u8,
    g: u8,
    b: u8,
    /// 0.0–1.0 brightness multiplier.
    brightness: f32,
}

impl Default for LedState {
    fn default() -> Self {
        // warmUP primary #b6a0ff.
        Self {
            effect: LedEffect::Solid,
            r: 0xb6,
            g: 0xa0,
            b: 0xff,
            brightness: 1.0,
        }
    }
}

static LED_STATE: OnceLock<Mutex<LedState>> = OnceLock::new();
static LED_ENGINE: Once = Once::new();

fn led_state() -> &'static Mutex<LedState> {
    LED_STATE.get_or_init(|| Mutex::new(LedState::default()))
}

fn scale_channel(c: u8, f: f32) -> u8 {
    (c as f32 * f).round().clamp(0.0, 255.0) as u8
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        scale_channel(255, r + m),
        scale_channel(255, g + m),
        scale_channel(255, b + m),
    )
}

/// Effective lightbar colour for `state` at elapsed time `t` seconds.
fn led_color_at(state: &LedState, t: f32) -> (u8, u8, u8) {
    match state.effect {
        LedEffect::Off => (0, 0, 0),
        LedEffect::Solid => (
            scale_channel(state.r, state.brightness),
            scale_channel(state.g, state.brightness),
            scale_channel(state.b, state.brightness),
        ),
        LedEffect::Breathing => {
            // 0.15–1.0 sine envelope, ~3.6 s period.
            let env = 0.15 + 0.85 * (0.5 - 0.5 * (t * std::f32::consts::TAU / 3.6).cos());
            let f = state.brightness * env;
            (
                scale_channel(state.r, f),
                scale_channel(state.g, f),
                scale_channel(state.b, f),
            )
        }
        // ~6 s hue sweep; the base colour is replaced by the cycling hue.
        LedEffect::Rainbow => hsv_to_rgb((t / 6.0).fract(), 1.0, state.brightness),
    }
}

/// Spawn the LED engine once. It re-renders the current [`LedState`] at ~30 Hz and pushes a
/// `Led` device command only when the colour changes, so static effects cost one command.
#[cfg_attr(not(windows), allow(dead_code))]
fn ensure_led_engine() {
    LED_ENGINE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("warmup-led".into())
            .spawn(|| {
                let start = Instant::now();
                let mut last: Option<(u8, u8, u8)> = None;
                loop {
                    let state = led_state().lock().map(|s| *s).unwrap_or_default();
                    let color = led_color_at(&state, start.elapsed().as_secs_f32());
                    if last != Some(color) {
                        push_device_command(PadCommand::Led {
                            r: color.0,
                            g: color.1,
                            b: color.2,
                        });
                        last = Some(color);
                    }
                    std::thread::sleep(Duration::from_millis(33));
                }
            });
    });
}

/// Update the lightbar state from a `config` frame (colour / effect / brightness) and make
/// sure the engine is running. Absent fields keep the current value.
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_led_config(p: &crate::protocol::ConfigPayload) {
    if let Ok(mut st) = led_state().lock() {
        // `parse_theme_color` yields a Windows COLORREF (0x00BBGGRR); extract true RGB
        // channels (the earlier `>>16 = r` read swapped red and blue).
        if let Some(cref) = p
            .led_color
            .as_deref()
            .and_then(crate::config::parse_theme_color)
        {
            st.r = (cref & 0xff) as u8;
            st.g = ((cref >> 8) & 0xff) as u8;
            st.b = ((cref >> 16) & 0xff) as u8;
        }
        if let Some(effect) = p.led_effect.as_deref() {
            st.effect = LedEffect::parse(effect);
        }
        if let Some(brightness) = p.led_brightness {
            st.brightness = brightness.clamp(0.0, 1.0);
        }
    }
    ensure_led_engine();
}

/// Apply a pushed `config`: write through to the companion's cursor settings (read live by
/// `pc_cursor`) and set the clicks-enabled mode. Maps desktop fields → companion params.
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_config(p: &crate::protocol::ConfigPayload) {
    CLICKS_ENABLED.store(p.clicks_enabled, Ordering::Relaxed);
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
    apply_led_config(p);
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
            crate::install::log_line(&format!(
                "pipe inbound rumble full strong={strong} weak={weak} ms={duration_ms}"
            ));
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
            crate::install::log_line(&format!(
                "pipe inbound rumble triggers left={left} right={right} ms={duration_ms}"
            ));
            PadCommand::TriggerRumble {
                left,
                right,
                ms: duration_ms,
            }
        }
    };
    push_device_command(cmd);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn apply_led(p: &crate::protocol::LedPayload) {
    crate::install::log_line(&format!("pipe inbound led r={} g={} b={}", p.r, p.g, p.b));
    push_device_command(PadCommand::Led {
        r: p.r,
        g: p.g,
        b: p.b,
    });
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
    CLICKS_ENABLED.store(p.clicks_enabled, Ordering::Relaxed);
    LAUNCHER_OWNS_TEXT_INPUT.store(p.launcher_owns_text_input, Ordering::Relaxed);
    GAME_ACTIVE.store(p.game_active, Ordering::Relaxed);
    LAUNCHER_FOREGROUND_NAV.store(p.launcher_foreground_nav, Ordering::Relaxed);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn clear_desktop_mode() {
    LAUNCHER_OWNS_TEXT_INPUT.store(false, Ordering::Relaxed);
    GAME_ACTIVE.store(false, Ordering::Relaxed);
    LAUNCHER_FOREGROUND_NAV.store(false, Ordering::Relaxed);
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

/// Last published `GUIDE` edge state, for consecutive-Guide-edge dedupe (SDL3 can emit
/// duplicate Guide edges on some firmware) — preserves the desktop's old behaviour.
static LAST_GUIDE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn state() -> &'static Mutex<ConnectionPayload> {
    STATE.get_or_init(|| Mutex::new(disconnected()))
}

fn buttons() -> &'static Mutex<VecDeque<ButtonPayload>> {
    BUTTONS.get_or_init(|| Mutex::new(VecDeque::new()))
}

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
    } else if l.contains("dualsense") || l.contains("ps5") {
        "playstation".into()
    } else if l.contains("dualshock") || l.contains("ps4") {
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

/// Start the pipe server on its own thread. No-op on non-Windows (there the desktop
/// owns input in-process, so there is no companion to serve).
#[cfg(windows)]
pub fn spawn() {
    std::thread::Builder::new()
        .name("warmup-pipe-server".into())
        .spawn(|| server::serve_forever())
        .ok();
}

#[cfg(not(windows))]
pub fn spawn() {}

#[cfg(windows)]
mod server {
    use super::{
        apply_companion_settings, apply_config, apply_led, apply_mode, apply_rumble,
        clear_desktop_mode, current, current_axis, current_battery, drain_buttons,
        reset_button_stream, take_cursor_moved, take_touchpad, DESKTOP_CONNECTED,
    };
    use crate::protocol::{
        AxisPayload, BatteryPayload, ConnectionPayload, DownFrame, Hello, UpFrame, PROTOCOL_VERSION,
    };
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    const PIPE_NAME: &str = r"\\.\pipe\warmup-input";
    /// SYSTEM full control; the interactive user (the desktop's account) gets read+write.
    const SDDL: &str = "D:(A;;GA;;;SY)(A;;GRGW;;;IU)";
    /// Re-send the current snapshot at least this often so a dropped idle client is noticed
    /// (the write fails) and the server loops back to accept a new one.
    const KEEPALIVE: Duration = Duration::from_secs(1);
    /// Throttle for outbound `cursor_moved` hints (the OS cursor already moved; this just
    /// keeps the webview's visual cursor in sync).
    const CURSOR_HINT_INTERVAL: Duration = Duration::from_millis(100);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn io_err(msg: &str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Other, msg)
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
                    PIPE_ACCESS_DUPLEX,
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
        if handshake(pipe).is_ok() {
            // Drop edges queued before this client connected (stale to it).
            reset_button_stream();
            DESKTOP_CONNECTED.store(true, Ordering::Relaxed);
            stream(pipe);
            DESKTOP_CONNECTED.store(false, Ordering::Relaxed);
            clear_desktop_mode();
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
        }
    }

    /// Read the client `hello`, reject on version mismatch, reply with our `hello`.
    fn handshake(pipe: HANDLE) -> std::io::Result<()> {
        let line = read_line(pipe)?;
        match DownFrame::parse_line(line.trim_end()) {
            Ok(DownFrame::Hello(h)) if h.protocol_version == PROTOCOL_VERSION => {
                if let Some(config) = h.config {
                    if let Ok(p) = serde_json::from_value(config) {
                        apply_config(&p);
                    }
                }
                if let Some(mode) = h.mode {
                    apply_mode(&mode);
                }
                if let Some(settings) = h.companion_settings {
                    apply_companion_settings(&settings);
                }
            }
            _ => return Err(io_err("hello rejected (missing or version mismatch)")),
        }
        let reply = UpFrame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            config: None,
            mode: None,
            companion_settings: None,
        });
        write_all(pipe, reply.to_ndjson_line().as_bytes())
    }

    /// Full-duplex session: drain inbound `config` (non-blocking), and write button edges
    /// (low latency), `cursor_moved` hints (throttled), and connection snapshots (on change
    /// plus a keepalive so a dropped idle client is noticed via the write error).
    fn stream(pipe: HANDLE) {
        let mut last: Option<ConnectionPayload> = None;
        let mut last_battery: Option<BatteryPayload> = None;
        let mut last_axis: Option<AxisPayload> = None;
        let mut last_conn_write = Instant::now()
            .checked_sub(KEEPALIVE)
            .unwrap_or_else(Instant::now);
        let mut last_cursor_write = Instant::now();
        loop {
            // Inbound config — never block the writer; only read a line that is fully buffered.
            if drain_inbound_config(pipe).is_err() {
                return;
            }
            for edge in drain_buttons() {
                if write_all(pipe, UpFrame::Button(edge).to_ndjson_line().as_bytes()).is_err() {
                    return; // client gone — return to accept a new one
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
    fn drain_inbound_config(pipe: HANDLE) -> std::io::Result<()> {
        while peek_has_newline(pipe)? {
            let line = read_line(pipe)?;
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            match DownFrame::parse_line(trimmed) {
                Ok(DownFrame::Config(p)) => apply_config(&p),
                Ok(DownFrame::Mode(p)) => apply_mode(&p),
                Ok(DownFrame::Rumble(p)) => apply_rumble(&p),
                Ok(DownFrame::Led(p)) => apply_led(&p),
                Ok(DownFrame::CompanionSettings(p)) => apply_companion_settings(&p),
                _ => {}
            }
        }
        Ok(())
    }

    /// True when the inbound buffer already contains a complete line (so [`read_line`] will
    /// not block). Uses `PeekNamedPipe` to inspect without consuming.
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
        Ok(buf[..read as usize].contains(&b'\n'))
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
    fn dualsense_maps_to_ps5() {
        assert_eq!(
            label_to_payload("DualSense Wireless Controller").controller_type,
            "ps5"
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
    fn mode_tracks_launcher_text_input_owner() {
        apply_mode(&ModeSnapshot {
            game_active: false,
            launcher_foreground_nav: false,
            clicks_enabled: false,
            launcher_owns_text_input: true,
        });
        assert!(!clicks_enabled());
        assert!(launcher_owns_text_input());

        apply_mode(&ModeSnapshot {
            game_active: false,
            launcher_foreground_nav: false,
            clicks_enabled: true,
            launcher_owns_text_input: false,
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
        });
        assert!(
            game_active() && !launcher_foreground_nav(),
            "in-game → sleep"
        );

        // Launcher woken over the running game: nav flips true so the pad keeps driving the launcher.
        apply_mode(&ModeSnapshot {
            game_active: true,
            launcher_foreground_nav: true,
            clicks_enabled: true,
            launcher_owns_text_input: true,
        });
        assert!(
            game_active() && launcher_foreground_nav(),
            "launcher over game → full"
        );

        // Disconnect resets every surface flag.
        clear_desktop_mode();
        assert!(!game_active());
        assert!(!launcher_foreground_nav());
    }
}

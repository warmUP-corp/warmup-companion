//! Winlogon gamepad polling: vendor-agnostic HID (primary) + XInput (Xbox fast path).
//!
//! XInputGetState can return neutral (zeroed) state to processes
//! that have no foreground-eligible window on the input desktop. Mitigation: the
//! secure poll thread runs a real Win32 UI message pump and owns a tiny anchor
//! window on the Winlogon desktop. PlayStation and Xbox pads are read via raw
//! HID + `hid_gamepad` (SDL `gamecontrollerdb.txt` for VID:PID hints); XInput
//! supplements when the driver exposes a real packet.

mod identity;
mod raw_hid;
mod secure_poll;
mod types;
mod xinput_poll;

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::XboxController::{XINPUT_GAMEPAD, XINPUT_STATE};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, SetForegroundWindow};

use crate::gamepad_backend::{Button, ButtonChange, GamepadBackend};
use crate::pad_decode::{
    button_edges, norm_thumb, trigger_edges, BUTTON_MASKS, LEFT_DEADZONE, RIGHT_DEADZONE,
};

use secure_poll::SecurePollThread;
use types::{
    ERROR_DEVICE_NOT_CONNECTED, ERROR_EMPTY, ERROR_SUCCESS, SLOTS, SecureMsg, XInputGetKeystrokeFn,
    XInputGetStateFn, XInputKeystroke, XINPUT_KEYSTROKE_KEYDOWN, XINPUT_KEYSTROKE_KEYUP,
};
use xinput_poll::{key_to_mask, load_xinput_api};

pub(crate) static LOGON_FG_HWND: AtomicIsize = AtomicIsize::new(0);
/// While > 0 the poll tick skips reclaiming foreground for the anchor, so an
/// inject burst (focus + SendInput on the loop thread) keeps LogonUI foreground
/// long enough for its keys to land. Decremented once per ~8ms poll tick.
pub(crate) static INJECT_HOLD_TICKS: AtomicU32 = AtomicU32::new(0);
pub(crate) static INJECT_HOLD_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Poll ticks to suppress the reclaim per committed key (~8ms each ⇒ ~48ms).
const INJECT_HOLD_WINDOW: u32 = 6;

/// The credential window (LogonUI / UAC) the secure poll last saw in foreground,
/// or None if not seen yet. The inject path foregrounds this so SendInput reaches
/// the PIN field.
pub fn logon_credential_window() -> Option<HWND> {
    let h = LOGON_FG_HWND.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

/// Suppress the anchor's foreground reclaim for one inject burst so the loop
/// thread can foreground LogonUI and SendInput uninterrupted. Called from the
/// inject path; the poll tick reclaims foreground once the window elapses.
pub fn begin_inject_hold() {
    INJECT_HOLD_ACTIVE.store(true, Ordering::Relaxed);
    INJECT_HOLD_TICKS.store(INJECT_HOLD_WINDOW, Ordering::Relaxed);
}

/// Reliably take foreground for `hwnd` even against a window that keeps grabbing
/// it back (LogonUI). A bare `SetForegroundWindow` is throttled by Windows'
/// foreground-lock and loses the tug-of-war; attaching our input queue to the
/// current foreground thread lifts that restriction for the duration of the call.
/// Self-limiting: once we win, the next tick sees `cur == hwnd` and skips this.
///
/// Retained (dead) after the no-foreground switch: the inject path
/// may still want a one-shot foreground hand-off to LogonUI for keystroke sends.
#[allow(dead_code)]
unsafe fn force_foreground(hwnd: HWND, current_fg: HWND) {
    let our_tid = GetCurrentThreadId();
    let fg_tid = GetWindowThreadProcessId(current_fg, None);
    if fg_tid != 0 && fg_tid != our_tid {
        let attached = AttachThreadInput(fg_tid, our_tid, true).as_bool();
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(fg_tid, our_tid, false);
        }
    } else {
        let _ = SetForegroundWindow(hwnd);
    }
}
pub struct XInputBackend {
    _module: Option<HMODULE>,
    get_state: Option<XInputGetStateFn>,
    get_keystroke: Option<XInputGetKeystrokeFn>,
    secure: Option<SecurePollThread>,
    active_slot: Option<u32>,
    active_secure_hid: bool,
    prev_buttons: [u16; 4],
    slot_connected: [bool; 4],
    pending: Vec<ButtonChange>,
    prev_trigger_left: bool,
    prev_trigger_right: bool,
    axes: (f32, f32, f32, f32),
    last_status_log: Instant,
    last_no_pad_log: Instant,
    last_raw_log: Instant,
    last_secure_check: Instant,
    /// Throttle for the non-fatal `XInputGetState error` log so a persistently
    /// failing slot can't spam the service log every poll (~125 lines/s).
    last_slot_err_log: Instant,
    /// Consecutive input-desktop probes that were not Winlogon while helper runs.
    secure_leave_winlogon_streak: u8,
}
impl XInputBackend {
    pub fn new() -> Self {
        let (module, get_state, get_keystroke) = load_xinput_api();
        Self {
            _module: module,
            get_state,
            get_keystroke,
            secure: None,
            active_slot: None,
            active_secure_hid: false,
            prev_buttons: [0; 4],
            slot_connected: [false; 4],
            pending: Vec::new(),
            prev_trigger_left: false,
            prev_trigger_right: false,
            axes: (0.0, 0.0, 0.0, 0.0),
            last_status_log: crate::time_util::stale(Duration::from_secs(60)),
            last_no_pad_log: crate::time_util::stale(Duration::from_secs(60)),
            last_raw_log: crate::time_util::stale(Duration::from_secs(60)),
            last_secure_check: crate::time_util::stale(Duration::from_secs(60)),
            last_slot_err_log: crate::time_util::stale(Duration::from_secs(60)),
            secure_leave_winlogon_streak: 0,
        }
    }

    fn input_is_winlogon(&mut self) -> bool {
        let winlogon = crate::win::surface::input().is_some_and(|s| s.is_winlogon());
        if winlogon {
            self.secure_leave_winlogon_streak = 0;
            return true;
        }
        self.secure_leave_winlogon_streak = self.secure_leave_winlogon_streak.saturating_add(1);
        self.secure.is_some() && self.secure_leave_winlogon_streak < 12
    }

    fn get_state(&self, slot: u32, state: &mut XINPUT_STATE) -> u32 {
        match self.get_state {
            Some(f) => unsafe { f(slot, state) },
            None => ERROR_DEVICE_NOT_CONNECTED,
        }
    }

    fn get_keystroke(&self, slot: u32, key: &mut XInputKeystroke) -> u32 {
        match self.get_keystroke {
            Some(f) => unsafe { f(slot, 0, key) },
            None => ERROR_EMPTY,
        }
    }

    fn log_slots_if_changed(&mut self, connected: [bool; 4]) {
        let changed = connected
            .iter()
            .zip(self.slot_connected.iter())
            .any(|(a, b)| a != b);
        if !changed && self.last_status_log.elapsed() < Duration::from_secs(30) {
            return;
        }
        if changed || self.last_status_log.elapsed() >= Duration::from_secs(30) {
            self.last_status_log = Instant::now();
            let summary: Vec<String> = (0..SLOTS)
                .map(|i| {
                    if connected[i as usize] {
                        format!("{i}:connected")
                    } else {
                        format!("{i}:empty")
                    }
                })
                .collect();
            service_log(&format!("XInput slots [{}]", summary.join(", ")));
        }
        self.slot_connected = connected;
    }

    fn log_no_controller(&mut self) {
        if self.last_no_pad_log.elapsed() >= Duration::from_secs(15) {
            self.last_no_pad_log = Instant::now();
            service_log("XInput: no controller connected (retrying)");
        }
    }

    fn pick_active_slot(&mut self, connected: &[bool; 4]) -> Option<u32> {
        if let Some(slot) = self.active_slot {
            if connected[slot as usize] {
                return Some(slot);
            }
            self.active_slot = None;
        }
        for i in 0..SLOTS {
            if connected[i as usize] {
                self.active_slot = Some(i);
                service_log(&format!("XInput: using slot {i}"));
                return Some(i);
            }
        }
        None
    }

    fn log_button_change(&mut self, slot: u32, prev: u16, cur: u16) {
        if prev == cur {
            return;
        }
        let names: Vec<&str> = BUTTON_MASKS
            .iter()
            .filter(|&(_b, mask)| cur & *mask != 0)
            .map(|(b, _mask)| b.as_str())
            .collect();
        service_log(&format!(
            "XInput buttons slot {slot}: 0x{prev:04x} -> 0x{cur:04x} [{}]",
            names.join("+")
        ));
        self.last_raw_log = Instant::now();
    }

    fn poll_keystrokes(&mut self, slot: u32) {
        for _ in 0..16 {
            let mut key = XInputKeystroke::default();
            let err = self.get_keystroke(slot, &mut key);
            if err == ERROR_EMPTY || err == ERROR_DEVICE_NOT_CONNECTED {
                break;
            }
            if err != ERROR_SUCCESS {
                service_log(&format!("XInputGetKeystroke({slot}) error {err}"));
                break;
            }
            service_log(&format!(
                "XInput keystroke slot {slot}: vk=0x{:04x} flags=0x{:04x} user={} hid=0x{:02x}",
                key.virtual_key, key.flags, key.user_index, key.hid_code
            ));
            let Some(mask) = key_to_mask(key.virtual_key) else {
                continue;
            };
            let idx = slot as usize;
            let prev = self.prev_buttons[idx];
            let mut cur = prev;
            if key.flags & XINPUT_KEYSTROKE_KEYDOWN != 0 {
                cur |= mask;
            }
            if key.flags & XINPUT_KEYSTROKE_KEYUP != 0 {
                cur &= !mask;
            }
            if prev != cur {
                self.prev_buttons[idx] = cur;
                self.log_button_change(slot, prev, cur);
                self.pending.extend(button_edges(prev, cur));
            }
        }
    }

    fn sync_secure_helper(&mut self) -> bool {
        if self.last_secure_check.elapsed() >= Duration::from_millis(250) {
            self.last_secure_check = Instant::now();
            let on_winlogon = self.input_is_winlogon();
            match (on_winlogon, self.secure.is_some()) {
                (true, false) => match SecurePollThread::spawn() {
                    Ok(thread) => {
                        service_log("XInput secure helper: starting on input desktop");
                        self.secure = Some(thread);
                    }
                    Err(e) => service_log(&format!("XInput secure helper: spawn failed: {e}")),
                },
                (false, true) => {
                    service_log("XInput secure helper: stopping");
                    self.secure.take();
                    self.clear_secure_state();
                    self.secure_leave_winlogon_streak = 0;
                }
                _ => {}
            }
        }

        let Some(secure) = self.secure.as_ref() else {
            return false;
        };

        let mut messages = Vec::new();
        let mut thread_dead = false;
        loop {
            match secure.rx.try_recv() {
                Ok(msg) => messages.push(msg),
                Err(mpsc::TryRecvError::Empty) => break,
                // Sender dropped => the secure thread exited (panic, or pump end).
                // It is never restarted otherwise, so flag it for respawn below.
                Err(mpsc::TryRecvError::Disconnected) => {
                    thread_dead = true;
                    break;
                }
            }
        }
        let got_msg = !messages.is_empty();
        for msg in messages {
            match msg {
                SecureMsg::Ready(desktop) => {
                    service_log(&format!("XInput secure helper: thread on {desktop}"));
                }
                SecureMsg::Slots(connected) => {
                    self.log_slots_if_changed(connected);
                    // Reflect the helper's connection state in active_slot so
                    // controller_label()/is_connected() and the debug overlay see the
                    // pad the helper is reading on Winlogon.
                    let _ = self.pick_active_slot(&connected);
                    if self.active_slot != Some(0) {
                        self.active_secure_hid = false;
                    }
                }
                SecureMsg::Buttons { slot, prev, cur } => {
                    self.log_button_change(slot, prev, cur);
                    // The helper owns button state on Winlogon; record it so the live
                    // input summary (and controller_label) reflect the active pad.
                    if (slot as usize) < self.prev_buttons.len() {
                        self.active_slot = Some(slot);
                        self.prev_buttons[slot as usize] = cur;
                    }
                    self.pending.extend(button_edges(prev, cur));
                }
                SecureMsg::Trigger(edge) => {
                    match edge.button {
                        Button::Lt => self.prev_trigger_left = edge.pressed,
                        Button::Rt => self.prev_trigger_right = edge.pressed,
                        _ => {}
                    }
                    self.pending.push(edge);
                }
                SecureMsg::HidActive(active) => {
                    self.active_secure_hid = active;
                    if active {
                        self.active_slot = Some(0);
                    }
                }
                SecureMsg::Axes(axes) => self.axes = axes,
                SecureMsg::NoController => {
                    self.active_slot = None;
                    self.clear_secure_state();
                    self.axes = (0.0, 0.0, 0.0, 0.0);
                    self.log_no_controller();
                }
                SecureMsg::Error(e) => service_log(&format!("XInput secure helper: {e}")),
            }
        }
        if thread_dead {
            // Drop the dead handle; the (true, false) arm above respawns it on the
            // next tick (<=250ms) if we're still on the input desktop.
            service_log("XInput secure helper: thread exited; clearing for respawn");
            self.secure = None;
            self.clear_secure_state();
        }
        got_msg || self.secure.is_some()
    }

    fn clear_secure_state(&mut self) {
        self.active_secure_hid = false;
        self.prev_trigger_left = false;
        self.prev_trigger_right = false;
    }
}
impl GamepadBackend for XInputBackend {
    fn poll(&mut self) -> Result<(), String> {
        self.pending.clear();
        // Keep the Winlogon-attached helper alive for diagnostics/fallback, but
        // still poll XInput from the service worker's Default desktop. The
        // controller loop runs from its normal UI/render thread; attaching the
        // polling thread to Winlogon can produce packet changes with neutral
        // buttons on some systems.
        let _ = self.sync_secure_helper();

        // Winlogon helper owns real button state; primary GetState on Default returns
        // neutral masks and would clobber secure edges if we merged them here.
        if self.secure.is_some() {
            return Ok(());
        }

        let mut connected = [false; 4];
        let mut states: [Option<XINPUT_GAMEPAD>; 4] = [None; 4];

        for slot in 0..SLOTS {
            let mut state = XINPUT_STATE::default();
            let err = self.get_state(slot, &mut state);
            if err == ERROR_SUCCESS {
                connected[slot as usize] = true;
                states[slot as usize] = Some(state.Gamepad);
            } else if err != ERROR_DEVICE_NOT_CONNECTED
                && self.last_slot_err_log.elapsed() >= Duration::from_secs(1)
            {
                self.last_slot_err_log = Instant::now();
                service_log(&format!("XInputGetState({slot}) error {err}"));
            }
        }

        self.log_slots_if_changed(connected);

        let Some(slot) = self.pick_active_slot(&connected) else {
            self.axes = (0.0, 0.0, 0.0, 0.0);
            self.log_no_controller();
            return Ok(());
        };

        let pad = states[slot as usize].expect("connected slot has state");
        let idx = slot as usize;
        let prev = self.prev_buttons[idx];
        let cur = pad.wButtons.0;
        self.prev_buttons[idx] = cur;
        self.log_button_change(slot, prev, cur);
        self.pending.extend(button_edges(prev, cur));
        trigger_edges(
            &mut self.prev_trigger_left,
            &mut self.prev_trigger_right,
            pad.bLeftTrigger,
            pad.bRightTrigger,
            &mut self.pending,
        );
        self.poll_keystrokes(slot);
        self.axes = (
            norm_thumb(pad.sThumbLX, LEFT_DEADZONE),
            norm_thumb(pad.sThumbLY, LEFT_DEADZONE),
            norm_thumb(pad.sThumbRX, RIGHT_DEADZONE),
            norm_thumb(pad.sThumbRY, RIGHT_DEADZONE),
        );
        Ok(())
    }

    fn button_changes(&mut self) -> Vec<ButtonChange> {
        std::mem::take(&mut self.pending)
    }

    fn axes(&self) -> (f32, f32, f32, f32) {
        self.axes
    }

    fn controller_label(&self) -> String {
        if self.active_secure_hid {
            return "HID slot 0".to_string();
        }
        match self.active_slot {
            Some(i) => format!("XInput slot {i}"),
            None => "none".to_string(),
        }
    }

    fn live_input_summary(&self) -> String {
        let Some(slot) = self.active_slot else {
            return String::new();
        };
        let mask = self.prev_buttons[slot as usize];
        let mut pressed: Vec<&str> = BUTTON_MASKS
            .iter()
            .filter_map(|(b, m)| (mask & *m != 0).then_some(b.as_str()))
            .collect();
        if self.prev_trigger_left {
            pressed.push("LT");
        }
        if self.prev_trigger_right {
            pressed.push("RT");
        }
        warmup_gamepad::live_input_format(&pressed, self.axes)
    }
}

fn service_log(msg: &str) {
    if crate::config::service_mode() {
        crate::install::log_line(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_label_identifies_secure_hid_source() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.active_secure_hid = true;

        assert_eq!(backend.controller_label(), "HID slot 0");
    }

    #[test]
    fn clear_secure_state_removes_hid_label() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.active_secure_hid = true;

        backend.clear_secure_state();

        assert_eq!(backend.controller_label(), "XInput slot 0");
    }

    #[test]
    fn live_input_summary_includes_held_triggers() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.prev_trigger_left = true;
        backend.prev_trigger_right = true;

        let summary = backend.live_input_summary();
        let parts: Vec<&str> = summary.split_whitespace().collect();
        assert!(parts.contains(&"LT"));
        assert!(parts.contains(&"RT"));
    }

    #[test]
    fn clear_secure_state_removes_trigger_summary_state() {
        let mut backend = XInputBackend::new();
        backend.active_slot = Some(0);
        backend.prev_trigger_left = true;
        backend.prev_trigger_right = true;

        backend.clear_secure_state();

        assert!(!backend.live_input_summary().contains("LT"));
        assert!(!backend.live_input_summary().contains("RT"));
    }

    #[test]
    fn xusb_presence_keeps_hid_from_shadowing_slot_zero() {
        use xinput_poll::hid_is_authoritative;
        assert!(hid_is_authoritative(1, 0));
        assert!(!hid_is_authoritative(1, 1));
        assert!(!hid_is_authoritative(0, 1));
    }
}

use sdl3::GamepadSubsystem;
use sdl3::gamepad::{Axis, Button as SdlButton, Gamepad, GamepadType};
use sdl3::sys::gamepad::{SDL_GetGamepadFromID, SDL_GetGamepadTouchpadFinger, SDL_SetGamepadLED};
use std::collections::HashMap;
use std::ffi::c_char;
use std::path::Path;

/// SDL3 trigger press threshold: SDL3 range is 0..=32767; 50% ≈ 16383.
const TRIGGER_THRESHOLD: i16 = 16_383;

fn set_sdl_hint(name: &'static [u8], value: &'static [u8]) {
    unsafe extern "C" {
        fn SDL_SetHint(name: *const c_char, value: *const c_char) -> bool;
    }
    unsafe {
        SDL_SetHint(name.as_ptr().cast(), value.as_ptr().cast());
    }
}

fn set_controller_feature_hints() {
    // Optional DualSense features require SDL's HIDAPI driver; set before SDL init.
    set_sdl_hint(b"SDL_JOYSTICK_HIDAPI\0", b"1\0");
    set_sdl_hint(b"SDL_JOYSTICK_HIDAPI_PS5\0", b"1\0");
    set_sdl_hint(b"SDL_JOYSTICK_ENHANCED_REPORTS\0", b"1\0");
}

/// When several pads are plugged in, prefer PlayStation / Xbox over generic HID.
fn open_preferred_gamepad(gc_sub: &GamepadSubsystem) -> Option<Gamepad> {
    let ids: Vec<_> = gc_sub.gamepads().unwrap_or_default();
    let mut best: Option<(i32, Gamepad)> = None;
    for id in ids {
        let Ok(gp) = gc_sub.open(id) else {
            continue;
        };
        let score = score_gamepad(&gp);
        let replace = best.as_ref().map(|(s, _)| score > *s).unwrap_or(true);
        if replace {
            best = Some((score, gp));
        }
    }
    best.map(|(_, gp)| gp)
}

fn score_gamepad(gp: &Gamepad) -> i32 {
    match gp.r#type() {
        GamepadType::PS5 => 100,
        GamepadType::PS4 => 90,
        GamepadType::XboxOne => 80,
        GamepadType::Xbox360 => 70,
        _ => {
            let name = gp.name().unwrap_or_default().to_ascii_lowercase();
            if name.contains("dualsense") || name.contains("ps5") {
                95
            } else if name.contains("dualshock") || name.contains("ps4") || name.contains("playstation")
            {
                85
            } else if name.contains("xbox") {
                75
            } else {
                0
            }
        }
    }
}

/// Maps SDL3 `GamepadType` to our canonical controller type string.
/// SDL3 identifies most controllers natively, eliminating the need for name heuristics.
pub fn sdl3_type_to_str(t: GamepadType) -> &'static str {
    match t {
        GamepadType::Xbox360 | GamepadType::XboxOne => "xbox",
        GamepadType::PS3 | GamepadType::PS4 | GamepadType::PS5 => "playstation",
        GamepadType::NintendoSwitchPro
        | GamepadType::NintendoSwitchJoyconLeft
        | GamepadType::NintendoSwitchJoyconRight
        | GamepadType::NintendoSwitchJoyconPair => "switch",
        _ => "generic",
    }
}

/// The canonical gamepad button — one identity shared by every backend
/// (SDL3, XInput, HID) and every consumer (VK nav, the toggle gate). Backends
/// translate their native codes into this enum; callers match on it instead of
/// comparing `&str` names, so a typo or a missing button is a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    A,
    B,
    X,
    Y,
    Lb,
    Rb,
    Lt,
    Rt,
    Select,
    Start,
    L3,
    R3,
    Up,
    Down,
    Left,
    Right,
    Guide,
    Touchpad,
}

impl Button {
    /// Canonical display name (used in logs and the debug overlay).
    pub fn as_str(self) -> &'static str {
        match self {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::Lb => "LB",
            Button::Rb => "RB",
            Button::Lt => "LT",
            Button::Rt => "RT",
            Button::Select => "SELECT",
            Button::Start => "START",
            Button::L3 => "L3",
            Button::R3 => "R3",
            Button::Up => "UP",
            Button::Down => "DOWN",
            Button::Left => "LEFT",
            Button::Right => "RIGHT",
            Button::Guide => "GUIDE",
            Button::Touchpad => "TOUCHPAD",
        }
    }
}

/// SDL3 buttons tracked for press/release changes.
/// Triggers (LT/RT) are axes in SDL3; they are handled separately and synthesised into ButtonChange.
const TRACKED_BUTTONS: &[SdlButton] = &[
    SdlButton::South,
    SdlButton::East,
    SdlButton::West,
    SdlButton::North,
    SdlButton::LeftShoulder,
    SdlButton::RightShoulder,
    SdlButton::Back,
    SdlButton::Start,
    SdlButton::LeftStick,
    SdlButton::RightStick,
    SdlButton::DPadUp,
    SdlButton::DPadDown,
    SdlButton::DPadLeft,
    SdlButton::DPadRight,
    SdlButton::Guide,
    SdlButton::Touchpad,
];

/// Maps an SDL3 button to our canonical [`Button`]. Returns `None` for buttons
/// we do not track.
fn sdl_to_button(btn: SdlButton) -> Option<Button> {
    Some(match btn {
        SdlButton::South => Button::A,
        SdlButton::East => Button::B,
        SdlButton::West => Button::X,
        SdlButton::North => Button::Y,
        SdlButton::LeftShoulder => Button::Lb,
        SdlButton::RightShoulder => Button::Rb,
        SdlButton::Back => Button::Select,
        SdlButton::Start => Button::Start,
        SdlButton::LeftStick => Button::L3,
        SdlButton::RightStick => Button::R3,
        SdlButton::DPadUp => Button::Up,
        SdlButton::DPadDown => Button::Down,
        SdlButton::DPadLeft => Button::Left,
        SdlButton::DPadRight => Button::Right,
        SdlButton::Guide => Button::Guide,
        SdlButton::Touchpad => Button::Touchpad,
        _ => return None,
    })
}

const STICK_DEADZONE: f32 = 0.12;

/// Format held buttons plus stick deflection for debug UIs.
pub fn live_input_format(pressed: &[&str], axes: (f32, f32, f32, f32)) -> String {
    let mut parts: Vec<String> = pressed.iter().map(|s| (*s).to_string()).collect();
    let (lx, ly, rx, ry) = axes;
    if lx.abs() >= STICK_DEADZONE || ly.abs() >= STICK_DEADZONE {
        parts.push(format!("L({lx:.2},{ly:.2})"));
    }
    if rx.abs() >= STICK_DEADZONE || ry.abs() >= STICK_DEADZONE {
        parts.push(format!("R({rx:.2},{ry:.2})"));
    }
    if parts.is_empty() {
        "(idle)".into()
    } else {
        parts.join(" ")
    }
}

/// A detected button state change (including synthesised LT/RT from trigger axes).
#[derive(Clone, Copy)]
pub struct ButtonChange {
    pub button: Button,
    pub pressed: bool,
}

/// Userland SDL polling level. `Sleep` keeps only the guide button hot so the
/// shell can wake Warmup without letting normal controls affect cursor/VK state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollMode {
    Full,
    Sleep,
}

/// Raw touchpad finger data read from SDL3 in a single poll cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchpadSample {
    pub index: u8,
    pub down: bool,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// Tracks gamepad state using SDL3 and provides button change detection.
///
/// Holds the SDL context and gamepad subsystem to keep them alive for the duration of the
/// polling thread. All SDL work happens on the same background thread; no Send/Sync transfer.
pub struct GamepadInput {
    _sdl: sdl3::Sdl,
    gc_sub: GamepadSubsystem,
    active_gamepad: Option<Gamepad>,
    prev_buttons: HashMap<SdlButton, bool>,
    prev_trigger_left: bool,
    prev_trigger_right: bool,
    pending_button_changes: Vec<ButtonChange>,
    touchpad_samples: Vec<TouchpadSample>,
    poll_mode: PollMode,
    /// Last known position of touchpad 0 finger 0 while it was down; used for delta computation.
    prev_touchpad_pos: Option<(f32, f32)>,
}

impl GamepadInput {
    /// Initialise SDL3, load the community game controller DB, and open the first available pad.
    pub fn new(db_path: &Path) -> Result<Self, String> {
        set_controller_feature_hints();
        let sdl = sdl3::init().map_err(|e| format!("Failed to init SDL3: {e}"))?;
        let gc_sub = sdl
            .gamepad()
            .map_err(|e| format!("SDL3 gamepad subsystem: {e}"))?;

        if db_path.exists() {
            match gc_sub.load_mappings(db_path) {
                Ok(n) => eprintln!("[gamepad] SDL3: loaded {n} controller mappings from DB"),
                Err(e) => eprintln!("[gamepad] SDL3: failed to load controller DB: {e}"),
            }
        }

        let active_gamepad = open_preferred_gamepad(&gc_sub);
        if let Some(ref gp) = active_gamepad {
            eprintln!(
                "[gamepad] SDL3: active pad {} ({})",
                gp.name().unwrap_or_else(|| "?".to_string()),
                sdl3_type_to_str(gp.r#type())
            );
        }

        let mut input = Self {
            _sdl: sdl,
            gc_sub,
            active_gamepad,
            prev_buttons: HashMap::with_capacity(TRACKED_BUTTONS.len()),
            prev_trigger_left: false,
            prev_trigger_right: false,
            pending_button_changes: Vec::with_capacity(TRACKED_BUTTONS.len() + 2),
            touchpad_samples: Vec::new(),
            poll_mode: PollMode::Full,
            prev_touchpad_pos: None,
        };
        // Baseline all buttons to suppress press storms on startup (Xbox triggers at rest = pressed).
        input.sync_prev_buttons_to_current();
        Ok(input)
    }

    /// Updates SDL3 controller state, detects hotplug, and captures button/trigger changes.
    /// Returns `true` if a connect or disconnect occurred.
    pub fn poll_events(&mut self) -> bool {
        self.poll_events_with_mode(PollMode::Full)
    }

    /// Updates SDL3 controller state according to a userland polling mode.
    pub fn poll_events_with_mode(&mut self, mode: PollMode) -> bool {
        self.gc_sub.update();
        let mut any_change = false;

        if self.poll_mode != mode {
            self.poll_mode = mode;
            self.sync_prev_buttons_to_current();
        }

        // Detect disconnection.
        if let Some(ref gp) = self.active_gamepad {
            if !gp.connected() {
                self.active_gamepad = None;
                self.pending_button_changes.clear();
                self.prev_buttons.clear();
                self.prev_trigger_left = false;
                self.prev_trigger_right = false;
                self.prev_touchpad_pos = None;
                any_change = true;
            }
        }

        // Detect new connection when no pad is active.
        if self.active_gamepad.is_none() {
            if let Some(gp) = open_preferred_gamepad(&self.gc_sub) {
                eprintln!(
                    "[gamepad] SDL3: connected {} ({})",
                    gp.name().unwrap_or_else(|| "?".to_string()),
                    sdl3_type_to_str(gp.r#type())
                );
                self.active_gamepad = Some(gp);
                any_change = true;
            }
        }

        if any_change {
            // Reset baseline to prevent press-storm on reconnect.
            self.sync_prev_buttons_to_current();
            return true;
        }

        // Poll button state changes on the active pad.
        if let Some(ref gp) = self.active_gamepad {
            let tracked = match mode {
                PollMode::Full => TRACKED_BUTTONS,
                PollMode::Sleep => &[SdlButton::Guide][..],
            };
            for &btn in tracked {
                let pressed = gp.button(btn);
                let was = self.prev_buttons.get(&btn).copied().unwrap_or(false);
                if pressed != was {
                    self.prev_buttons.insert(btn, pressed);
                    if let Some(button) = sdl_to_button(btn) {
                        self.pending_button_changes.push(ButtonChange { button, pressed });
                    }
                }
            }
            if mode == PollMode::Sleep {
                return false;
            }
            // Synthesise LT/RT from trigger axes.
            let lt_pressed = gp.axis(Axis::TriggerLeft) > TRIGGER_THRESHOLD;
            let rt_pressed = gp.axis(Axis::TriggerRight) > TRIGGER_THRESHOLD;
            if lt_pressed != self.prev_trigger_left {
                self.prev_trigger_left = lt_pressed;
                self.pending_button_changes.push(ButtonChange {
                    button: Button::Lt,
                    pressed: lt_pressed,
                });
            }
            if rt_pressed != self.prev_trigger_right {
                self.prev_trigger_right = rt_pressed;
                self.pending_button_changes.push(ButtonChange {
                    button: Button::Rt,
                    pressed: rt_pressed,
                });
            }
        }

        false
    }

    /// Baseline button state from current SDL3 values; clears pending changes.
    fn sync_prev_buttons_to_current(&mut self) {
        self.pending_button_changes.clear();
        self.prev_buttons.clear();
        self.prev_trigger_left = false;
        self.prev_trigger_right = false;
        self.prev_touchpad_pos = None;
        if let Some(ref gp) = self.active_gamepad {
            for &btn in TRACKED_BUTTONS {
                self.prev_buttons.insert(btn, gp.button(btn));
            }
            self.prev_trigger_left = gp.axis(Axis::TriggerLeft) > TRIGGER_THRESHOLD;
            self.prev_trigger_right = gp.axis(Axis::TriggerRight) > TRIGGER_THRESHOLD;
        }
    }

    /// Returns `(left_x, left_y, right_x, right_y)` normalized to `-1.0..=1.0`.
    pub fn axes(&self) -> (f32, f32, f32, f32) {
        let Some(ref gp) = self.active_gamepad else {
            return (0.0, 0.0, 0.0, 0.0);
        };
        const NORM: f32 = 32767.0;
        // SDL3 Y axes: up = negative. Negate so callers (cursor, scroll) receive up = positive.
        (
            gp.axis(Axis::LeftX) as f32 / NORM,
            -(gp.axis(Axis::LeftY) as f32 / NORM),
            gp.axis(Axis::RightX) as f32 / NORM,
            -(gp.axis(Axis::RightY) as f32 / NORM),
        )
    }

    /// Pressed buttons and non-idle sticks for debug overlays.
    pub fn live_input_summary(&self) -> String {
        let Some(ref gp) = self.active_gamepad else {
            return String::new();
        };
        let mut pressed = Vec::new();
        for &btn in TRACKED_BUTTONS {
            if gp.button(btn) {
                if let Some(b) = sdl_to_button(btn) {
                    pressed.push(b.as_str());
                }
            }
        }
        if self.prev_trigger_left {
            pressed.push(Button::Lt.as_str());
        }
        if self.prev_trigger_right {
            pressed.push(Button::Rt.as_str());
        }
        live_input_format(&pressed, self.axes())
    }

    /// Returns the name of the active controller as reported by SDL3, if any.
    pub fn active_controller_name(&self) -> Option<String> {
        self.active_gamepad.as_ref()?.name()
    }

    /// Returns the SDL3-detected controller type as a canonical string.
    /// Uses `sdl3_type_to_str(gamepad.type())` — no name heuristic needed.
    pub fn active_controller_type(&self) -> &'static str {
        self.active_gamepad
            .as_ref()
            .map(|gp| sdl3_type_to_str(gp.r#type()))
            .unwrap_or("generic")
    }

    /// Sets the controller lightbar / LED color. No-ops if the controller has no LED.
    ///
    /// Clears SDL3's automatic player-index lightbar color first so the custom color is not
    /// overwritten by SDL3's internal player-assignment logic (relevant for DualSense / DS4).
    pub fn set_led(&mut self, r: u8, g: u8, b: u8) {
        let Some(ref gp) = self.active_gamepad else {
            return;
        };
        let Ok(id) = gp.id() else { return };
        let raw = unsafe { SDL_GetGamepadFromID(id) };
        if raw.is_null() {
            return;
        }
        unsafe extern "C" {
            // Setting player index to -1 removes SDL3's automatic player-indicator color so
            // SDL_SetGamepadLED can take full control of the DualSense / DS4 lightbar.
            fn SDL_SetGamepadPlayerIndex(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                player_index: ::std::ffi::c_int,
            ) -> bool;
        }
        unsafe {
            SDL_SetGamepadPlayerIndex(raw, -1);
            SDL_SetGamepadLED(raw, r, g, b);
        }
    }

    /// Reads the controller battery level via `SDL_GetGamepadPowerInfo`.
    ///
    /// Returns `(percent, charging, wired)` where `percent` is 0–100 or −1 when unknown.
    /// `charging` is true while plugged in and not yet full; `wired` means no internal battery.
    pub fn battery(&self) -> (i32, bool, bool) {
        let Some(ref gp) = self.active_gamepad else {
            return (-1, false, false);
        };
        let Ok(id) = gp.id() else {
            return (-1, false, false);
        };
        let raw = unsafe { SDL_GetGamepadFromID(id) };
        if raw.is_null() {
            return (-1, false, false);
        }
        // Declare SDL_GetGamepadPowerInfo inline to avoid importing SDL_PowerState across sdl3_sys versions.
        // SDL_PowerState maps to c_int; values: ERROR=-1, UNKNOWN=0, ON_BATTERY=1, NO_BATTERY=2,
        // CHARGING=3, CHARGED=4.
        unsafe extern "C" {
            fn SDL_GetGamepadPowerInfo(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                percent: *mut ::std::ffi::c_int,
            ) -> ::std::ffi::c_int;
        }
        let mut pct: std::ffi::c_int = -1;
        let state = unsafe { SDL_GetGamepadPowerInfo(raw, &mut pct) };
        let charging = state == 3 || state == 4; // CHARGING or CHARGED
        let wired = state == 2; // NO_BATTERY
        (pct, charging, wired)
    }

    /// Reads finger data from touchpad 0 and computes the relative movement delta of finger 0.
    ///
    /// Returns `(delta, fingers)` where:
    /// - `delta` is `Some((dx, dy))` in normalised [0, 1] coordinates when finger 0 is down and
    ///   a previous position exists (i.e. not the first contact). Scale to pixels in the caller.
    /// - `fingers` contains every supported finger slot on touchpad 0.
    ///
    /// Returns `(None, [])` when no gamepad is connected, the controller has no touchpad
    /// (e.g. Xbox/Switch), or no finger is currently touching.
    pub fn poll_touchpad(&mut self) -> (Option<(f32, f32)>, &[TouchpadSample]) {
        self.touchpad_samples.clear();

        // Collect the data we need while holding the immutable borrow on active_gamepad.
        let (id, num_fingers) = {
            let Some(ref gp) = self.active_gamepad else {
                self.prev_touchpad_pos = None;
                return (None, &self.touchpad_samples);
            };
            if gp.touchpads_count() == 0 {
                self.prev_touchpad_pos = None;
                return (None, &self.touchpad_samples);
            }
            let nf = gp.supported_touchpad_fingers(0);
            let id = match gp.id() {
                Ok(id) => id,
                Err(_) => {
                    self.prev_touchpad_pos = None;
                    return (None, &self.touchpad_samples);
                }
            };
            (id, nf)
        };

        if num_fingers == 0 {
            self.prev_touchpad_pos = None;
            return (None, &self.touchpad_samples);
        }

        // Obtain a raw SDL3 pointer via the instance ID so we can call APIs the high-level
        // wrapper does not expose.  Safety: `active_gamepad` keeps the device open; the pointer
        // returned by SDL_GetGamepadFromID is valid for as long as the gamepad is open.
        let raw = unsafe { SDL_GetGamepadFromID(id) };
        if raw.is_null() {
            self.prev_touchpad_pos = None;
            return (None, &self.touchpad_samples);
        }

        self.touchpad_samples.reserve(num_fingers as usize);
        let mut primary_pos: Option<(f32, f32)> = None;
        let mut primary_down = false;

        for i in 0..num_fingers as i32 {
            let mut down = false;
            let mut x = 0.0_f32;
            let mut y = 0.0_f32;
            let mut pressure = 0.0_f32;
            let ok = unsafe {
                SDL_GetGamepadTouchpadFinger(raw, 0, i, &mut down, &mut x, &mut y, &mut pressure)
            };
            if ok {
                if i == 0 {
                    primary_down = down;
                    if down {
                        primary_pos = Some((x, y));
                    }
                }
                self.touchpad_samples.push(TouchpadSample {
                    index: i as u8,
                    down,
                    x,
                    y,
                    pressure,
                });
            }
        }

        let delta = if primary_down {
            if let (Some(cur), Some(prev)) = (primary_pos, self.prev_touchpad_pos) {
                Some((cur.0 - prev.0, cur.1 - prev.1))
            } else {
                None
            }
        } else {
            None
        };

        self.prev_touchpad_pos = primary_pos;

        (delta, &self.touchpad_samples)
    }

    /// Returns pending button changes from the last `poll_events()` call and clears them.
    pub fn detect_button_changes(&mut self) -> Vec<ButtonChange> {
        std::mem::take(&mut self.pending_button_changes)
    }

    /// Fires a rumble effect on the active gamepad. Returns false if unsupported or no pad.
    pub fn rumble(&mut self, strong: f32, weak: f32, duration_ms: u32) -> bool {
        let Some(ref mut gp) = self.active_gamepad else {
            return false;
        };
        let strong_u16 = (strong.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        let weak_u16 = (weak.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        // Cap duration: passing u32::MAX overflows SDL3 internally and ends immediately.
        gp.set_rumble(strong_u16, weak_u16, duration_ms.min(30_000))
            .is_ok()
    }

    /// Clears main and trigger rumble on the active gamepad.
    ///
    /// SDL uses **duration 0** to stop ongoing effects; call this on app shutdown so motors
    /// do not stay on after the process exits.
    pub fn stop_rumble_effects(&mut self) {
        self.rumble(0.0, 0.0, 0);
        self.trigger_rumble(0.0, 0.0, 0);
    }

    /// Fires a rumble effect on the trigger (adaptive) motors.
    /// Only supported on controllers with per-trigger haptics (e.g. DualSense, Xbox Series).
    pub fn trigger_rumble(&mut self, left: f32, right: f32, duration_ms: u32) -> bool {
        let Some(ref gp) = self.active_gamepad else {
            return false;
        };
        let Ok(id) = gp.id() else { return false };
        let raw = unsafe { SDL_GetGamepadFromID(id) };
        if raw.is_null() {
            return false;
        }
        // SDL3: SDL_RumbleGamepadTriggers(gamepad, left_rumble_u16, right_rumble_u16, duration_ms)
        unsafe extern "C" {
            fn SDL_RumbleGamepadTriggers(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                left_rumble: u16,
                right_rumble: u16,
                duration_ms: u32,
            ) -> bool;
        }
        let left_u16 = (left.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        let right_u16 = (right.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        unsafe { SDL_RumbleGamepadTriggers(raw, left_u16, right_u16, duration_ms.min(30_000)) }
    }

    /// Enables the gyroscope sensor on the active gamepad.
    /// Must be called once after connect before `read_gyro` returns data.
    /// Returns `true` if the controller supports gyro and it was enabled.
    pub fn enable_gyro(&self) -> bool {
        let Some(ref gp) = self.active_gamepad else {
            return false;
        };
        let Ok(id) = gp.id() else { return false };
        let raw = unsafe { SDL_GetGamepadFromID(id) };
        if raw.is_null() {
            return false;
        }
        // SDL_SensorType: SDL_SENSOR_GYRO = 2
        unsafe extern "C" {
            fn SDL_SetGamepadSensorEnabled(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                sensor_type: ::std::ffi::c_int,
                enabled: bool,
            ) -> bool;
        }
        unsafe { SDL_SetGamepadSensorEnabled(raw, 2, true) }
    }

    /// Reads the current gyroscope angular velocity (rad/s).
    /// Returns `Some((pitch, yaw, roll))` where pitch is rotation around the X axis,
    /// yaw around Y, and roll around Z.
    /// Returns `None` when no gamepad is connected or the sensor is unavailable.
    pub fn read_gyro(&self) -> Option<(f32, f32, f32)> {
        let gp = self.active_gamepad.as_ref()?;
        let id = gp.id().ok()?;
        let raw = unsafe { SDL_GetGamepadFromID(id) };
        if raw.is_null() {
            return None;
        }
        unsafe extern "C" {
            fn SDL_GetGamepadSensorData(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                sensor_type: ::std::ffi::c_int,
                data: *mut f32,
                num_values: ::std::ffi::c_int,
            ) -> bool;
        }
        let mut data = [0.0_f32; 3];
        let ok = unsafe { SDL_GetGamepadSensorData(raw, 2, data.as_mut_ptr(), 3) };
        if ok {
            Some((data[0], data[1], data[2]))
        } else {
            None
        }
    }
}

impl Drop for GamepadInput {
    fn drop(&mut self) {
        self.stop_rumble_effects();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_button_maps_to_guide() {
        assert_eq!(sdl_to_button(SdlButton::Guide), Some(Button::Guide));
        assert_eq!(Button::Guide.as_str(), "GUIDE");
    }

    #[test]
    fn sdl3_xbox_types_map_to_xbox() {
        assert_eq!(sdl3_type_to_str(GamepadType::XboxOne), "xbox");
        assert_eq!(sdl3_type_to_str(GamepadType::Xbox360), "xbox");
    }

    #[test]
    fn sdl3_playstation_types_map_correctly() {
        assert_eq!(sdl3_type_to_str(GamepadType::PS5), "playstation");
        assert_eq!(sdl3_type_to_str(GamepadType::PS4), "playstation");
    }

    #[test]
    fn sdl3_switch_type_maps_correctly() {
        assert_eq!(sdl3_type_to_str(GamepadType::NintendoSwitchPro), "switch");
    }

    #[test]
    fn sdl3_unknown_type_maps_to_generic() {
        assert_eq!(sdl3_type_to_str(GamepadType::Unknown), "generic");
    }
}

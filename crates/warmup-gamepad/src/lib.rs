use sdl3::gamepad::{Axis, Button as SdlButton, Gamepad, GamepadType};
use sdl3::sys::gamepad::{
    SDL_GetGamepadFromID, SDL_GetGamepadPath, SDL_GetGamepadProduct, SDL_GetGamepadTouchpadFinger,
    SDL_GetGamepadVendor, SDL_SetGamepadLED,
};
use sdl3::GamepadSubsystem;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::path::Path;
use std::time::{Duration, Instant};

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
            } else if name.contains("dualshock")
                || name.contains("ps4")
                || name.contains("playstation")
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
    Qam,
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
            Button::Qam => "QAM",
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
    SdlButton::Misc1,
];

const SLEEP_TRACKED_BUTTONS: &[SdlButton] = &[
    SdlButton::Guide,
    SdlButton::Back,
    SdlButton::LeftShoulder,
    SdlButton::RightShoulder,
    SdlButton::Misc1,
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
        SdlButton::Misc1 => Button::Qam,
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

const STICK_ACTIVITY: i32 = 16_000;

unsafe extern "C" {
    fn SDL_RumbleGamepadTriggers(
        gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
        left_rumble: u16,
        right_rumble: u16,
        duration_ms: u32,
    ) -> bool;
}

struct Pad {
    gp: Gamepad,
    id: u32,
    activity: u32,
}

fn pad_raw(gp: &Gamepad) -> Option<*mut sdl3::sys::gamepad::SDL_Gamepad> {
    let raw = unsafe { SDL_GetGamepadFromID(gp.id().ok()?) };
    (!raw.is_null()).then_some(raw)
}

fn activity_mask(gp: &Gamepad) -> u32 {
    let mut mask = 0u32;
    for (i, &btn) in TRACKED_BUTTONS.iter().enumerate() {
        if gp.button(btn) {
            mask |= 1 << i;
        }
    }
    let base = TRACKED_BUTTONS.len();
    let axes = [
        gp.axis(Axis::TriggerLeft) > TRIGGER_THRESHOLD,
        gp.axis(Axis::TriggerRight) > TRIGGER_THRESHOLD,
        stick_magnitude(gp.axis(Axis::LeftX), gp.axis(Axis::LeftY)) > STICK_ACTIVITY,
        stick_magnitude(gp.axis(Axis::RightX), gp.axis(Axis::RightY)) > STICK_ACTIVITY,
    ];
    for (i, on) in axes.into_iter().enumerate() {
        if on {
            mask |= 1 << (base + i);
        }
    }
    mask
}

fn stick_magnitude(x: i16, y: i16) -> i32 {
    let (x, y) = (x as f32, y as f32);
    (x * x + y * y).sqrt() as i32
}

/// Tracks gamepad state using SDL3 and provides button change detection.
///
/// Holds the SDL context and gamepad subsystem to keep them alive for the duration of the
/// polling thread. All SDL work happens on the same background thread; no Send/Sync transfer.
pub struct GamepadInput {
    _sdl: sdl3::Sdl,
    gc_sub: GamepadSubsystem,
    pads: Vec<Pad>,
    primary: Option<u32>,
    merge: bool,
    prev_buttons: HashMap<SdlButton, bool>,
    prev_trigger_left: bool,
    prev_trigger_right: bool,
    pending_button_changes: Vec<ButtonChange>,
    touchpad_samples: Vec<TouchpadSample>,
    poll_mode: PollMode,
    /// Last known position of touchpad 0 finger 0 while it was down; used for delta computation.
    prev_touchpad_pos: Option<(f32, f32)>,
    last_hotplug_scan: Instant,
    bt_battery: Cell<Option<(u32, Instant, Option<i32>)>>,
}

impl GamepadInput {
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

        let mut input = Self {
            _sdl: sdl,
            gc_sub,
            pads: Vec::new(),
            primary: None,
            merge: true,
            prev_buttons: HashMap::with_capacity(TRACKED_BUTTONS.len()),
            prev_trigger_left: false,
            prev_trigger_right: false,
            pending_button_changes: Vec::with_capacity(TRACKED_BUTTONS.len() + 2),
            touchpad_samples: Vec::new(),
            poll_mode: PollMode::Full,
            prev_touchpad_pos: None,
            last_hotplug_scan: Instant::now(),
            bt_battery: Cell::new(None),
        };
        input.open_new_pads();
        // Baseline all buttons to suppress press storms on startup (Xbox triggers at rest = pressed).
        input.sync_prev_buttons_to_current();
        Ok(input)
    }

    pub fn set_merge(&mut self, merge: bool) {
        if self.merge == merge {
            return;
        }
        self.merge = merge;
        if !merge {
            self.primary = self.best_scored_pad();
        }
        self.sync_prev_buttons_to_current();
    }

    fn active(&self) -> Option<&Gamepad> {
        let id = self.primary?;
        self.pads.iter().find(|p| p.id == id).map(|p| &p.gp)
    }

    fn inputs(&self) -> impl Iterator<Item = &Gamepad> {
        let primary = self.primary;
        let merge = self.merge;
        self.pads
            .iter()
            .filter(move |p| merge || Some(p.id) == primary)
            .map(|p| &p.gp)
    }

    fn merged_button(&self, btn: SdlButton) -> bool {
        self.inputs().any(|gp| gp.button(btn))
    }

    fn merged_trigger(&self, axis: Axis) -> bool {
        self.inputs()
            .map(|gp| gp.axis(axis))
            .max()
            .is_some_and(|v| v > TRIGGER_THRESHOLD)
    }

    fn merged_stick(&self, x: Axis, y: Axis) -> (i16, i16) {
        self.inputs()
            .map(|gp| (gp.axis(x), gp.axis(y)))
            .max_by_key(|&(x, y)| stick_magnitude(x, y))
            .unwrap_or((0, 0))
    }

    fn best_scored_pad(&self) -> Option<u32> {
        self.pads
            .iter()
            .max_by_key(|p| score_gamepad(&p.gp))
            .map(|p| p.id)
    }

    fn open_new_pads(&mut self) -> bool {
        let mut added = false;
        for id in self.gc_sub.gamepads().unwrap_or_default() {
            let raw: u32 = id.into();
            if self.pads.iter().any(|p| p.id == raw) {
                continue;
            }
            let Ok(gp) = self.gc_sub.open(id) else {
                continue;
            };
            eprintln!(
                "[gamepad] SDL3: connected {} ({})",
                gp.name().unwrap_or_else(|| "?".to_string()),
                sdl3_type_to_str(gp.r#type())
            );
            let activity = activity_mask(&gp);
            self.pads.push(Pad {
                gp,
                id: raw,
                activity,
            });
            added = true;
        }
        if added && self.active().is_none() {
            self.primary = self.best_scored_pad();
        }
        added
    }

    fn drop_disconnected_pads(&mut self) -> bool {
        let before = self.pads.len();
        self.pads.retain(|p| p.gp.connected());
        if self.pads.len() == before {
            return false;
        }
        if self.active().is_none() {
            self.primary = self.best_scored_pad();
            self.prev_touchpad_pos = None;
        }
        true
    }

    fn update_primary(&mut self) {
        let mut newest = None;
        for pad in self.pads.iter_mut() {
            let mask = activity_mask(&pad.gp);
            if mask & !pad.activity != 0 {
                newest = Some(pad.id);
            }
            pad.activity = mask;
        }
        if !self.merge {
            return;
        }
        if let Some(id) = newest {
            if self.primary != Some(id) {
                self.primary = Some(id);
                self.prev_touchpad_pos = None;
            }
        }
    }

    /// Updates SDL3 controller state, detects hotplug, and captures button/trigger changes.
    /// Returns `true` if a connect or disconnect occurred.
    pub fn poll_events(&mut self) -> bool {
        self.poll_events_with_mode(PollMode::Full)
    }

    /// Updates SDL3 controller state according to a userland polling mode.
    pub fn poll_events_with_mode(&mut self, mode: PollMode) -> bool {
        self.gc_sub.update();

        if self.poll_mode != mode {
            self.poll_mode = mode;
            self.sync_prev_buttons_to_current();
        }

        let had_pads = !self.pads.is_empty();
        let mut any_change = self.drop_disconnected_pads();
        if self.last_hotplug_scan.elapsed() >= Duration::from_millis(1000) {
            self.last_hotplug_scan = Instant::now();
            any_change |= self.open_new_pads();
        }

        if any_change && (!had_pads || self.pads.is_empty()) {
            // Reset baseline to prevent press-storm on reconnect.
            self.sync_prev_buttons_to_current();
            return true;
        }

        if self.pads.is_empty() {
            return any_change;
        }
        self.update_primary();

        let tracked = match mode {
            PollMode::Full => TRACKED_BUTTONS,
            PollMode::Sleep => SLEEP_TRACKED_BUTTONS,
        };
        for &btn in tracked {
            let pressed = self.merged_button(btn);
            let was = self.prev_buttons.get(&btn).copied().unwrap_or(false);
            if pressed != was {
                self.prev_buttons.insert(btn, pressed);
                if let Some(button) = sdl_to_button(btn) {
                    self.pending_button_changes
                        .push(ButtonChange { button, pressed });
                }
            }
        }
        if mode == PollMode::Sleep {
            return any_change;
        }
        // Synthesise LT/RT from trigger axes.
        let lt_pressed = self.merged_trigger(Axis::TriggerLeft);
        let rt_pressed = self.merged_trigger(Axis::TriggerRight);
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

        any_change
    }

    /// Baseline button state from current SDL3 values; clears pending changes.
    fn sync_prev_buttons_to_current(&mut self) {
        self.pending_button_changes.clear();
        self.prev_buttons.clear();
        self.prev_trigger_left = false;
        self.prev_trigger_right = false;
        self.prev_touchpad_pos = None;
        if self.pads.is_empty() {
            return;
        }
        for &btn in TRACKED_BUTTONS {
            let pressed = self.merged_button(btn);
            self.prev_buttons.insert(btn, pressed);
        }
        self.prev_trigger_left = self.merged_trigger(Axis::TriggerLeft);
        self.prev_trigger_right = self.merged_trigger(Axis::TriggerRight);
    }

    /// Returns `(left_x, left_y, right_x, right_y)` normalized to `-1.0..=1.0`.
    pub fn axes(&self) -> (f32, f32, f32, f32) {
        const NORM: f32 = 32767.0;
        let (lx, ly) = self.merged_stick(Axis::LeftX, Axis::LeftY);
        let (rx, ry) = self.merged_stick(Axis::RightX, Axis::RightY);
        // SDL3 Y axes: up = negative. Negate so callers (cursor, scroll) receive up = positive.
        (
            lx as f32 / NORM,
            -(ly as f32 / NORM),
            rx as f32 / NORM,
            -(ry as f32 / NORM),
        )
    }

    /// Pressed buttons and non-idle sticks for debug overlays.
    pub fn live_input_summary(&self) -> String {
        if self.pads.is_empty() {
            return String::new();
        }
        let mut pressed = Vec::new();
        for &btn in TRACKED_BUTTONS {
            if self.merged_button(btn) {
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
        self.active()?.name()
    }

    /// Returns the SDL3-detected controller type as a canonical string.
    /// Uses `sdl3_type_to_str(gamepad.type())` — no name heuristic needed.
    /// Puts the DualSense / DS4 back on its PlayStation player lighting: the player colour on
    /// the lightbar and the white player lights under the touchpad.
    ///
    /// SDL3 keeps the last `SDL_SetGamepadLED` colour for as long as the pad stays open (its
    /// HIDAPI driver never clears `color_set`), so re-assigning the player index alone leaves a
    /// custom colour on the lightbar. The player colour is therefore written explicitly.
    pub fn release_led(&mut self) {
        unsafe extern "C" {
            fn SDL_SetGamepadPlayerIndex(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                player_index: ::std::ffi::c_int,
            ) -> bool;
        }
        for (index, pad) in self.pads.iter().enumerate() {
            let Some(raw) = pad_raw(&pad.gp) else {
                continue;
            };
            let (r, g, b) = player_led_color(index);
            // Clear first so SDL re-sends the player lights even when the index is unchanged.
            unsafe {
                SDL_SetGamepadPlayerIndex(raw, -1);
                SDL_SetGamepadPlayerIndex(raw, index as ::std::ffi::c_int);
                SDL_SetGamepadLED(raw, r, g, b);
            }
        }
    }

    pub fn active_controller_type(&self) -> &'static str {
        self.active()
            .map(|gp| sdl3_type_to_str(gp.r#type()))
            .unwrap_or("generic")
    }

    /// Sets the controller lightbar / LED color. No-ops if the controller has no LED.
    ///
    /// The player index is left alone: once a colour is set SDL3 always sends it instead of the
    /// player colour, and clearing the index would also switch off the white player lights.
    pub fn set_led(&mut self, r: u8, g: u8, b: u8) {
        for pad in &self.pads {
            let Some(raw) = pad_raw(&pad.gp) else {
                continue;
            };
            unsafe {
                SDL_SetGamepadLED(raw, r, g, b);
            }
        }
    }

    /// Reads the controller battery level via `SDL_GetGamepadPowerInfo`.
    ///
    /// Returns `(percent, charging, wired)` where `percent` is 0–100 or −1 when unknown.
    /// `charging` is true while plugged in and not yet full; `wired` means no internal battery.
    pub fn battery(&self) -> (i32, bool, bool) {
        let Some(raw) = self.active().and_then(pad_raw) else {
            return (-1, false, false);
        };
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
        let xbox_wired =
            self.active_controller_type() == "xbox" && self.connection_wired() == Some(true);
        let (pct, charging, wired) = map_power_info(state, pct, xbox_wired);
        if wired || self.active_controller_type() == "playstation" {
            return (pct, charging, wired);
        }
        match self.bluetooth_battery() {
            Some(bt) => (bt, charging, false),
            None => (pct, charging, wired),
        }
    }

    fn bluetooth_battery(&self) -> Option<i32> {
        let id = self.primary?;
        if let Some((cached_id, at, value)) = self.bt_battery.get() {
            if cached_id == id && at.elapsed() < BT_BATTERY_TTL {
                return value;
            }
        }
        let value = self
            .active()
            .and_then(pad_raw)
            .and_then(|raw| {
                let path = unsafe { SDL_GetGamepadPath(raw) };
                (!path.is_null())
                    .then(|| {
                        unsafe { CStr::from_ptr(path) }
                            .to_string_lossy()
                            .into_owned()
                    })
                    .and_then(|path| bluetooth_address(&path))
                    .or_else(|| {
                        let (vid, pid) =
                            unsafe { (SDL_GetGamepadVendor(raw), SDL_GetGamepadProduct(raw)) };
                        hid_bluetooth_address(vid, pid)
                    })
            })
            .and_then(|addr| pnp_battery_percent(&addr));
        self.bt_battery.set(Some((id, Instant::now(), value)));
        value
    }

    pub fn connection_wired(&self) -> Option<bool> {
        let raw = self.active().and_then(pad_raw)?;
        unsafe extern "C" {
            fn SDL_GetGamepadConnectionState(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
            ) -> ::std::ffi::c_int;
        }
        wired_from_connection_state(unsafe { SDL_GetGamepadConnectionState(raw) })
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
            let Some(gp) = self.active() else {
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
        let Some(id) = self.primary else {
            return false;
        };
        let Some(gp) = self.pads.iter_mut().find(|p| p.id == id).map(|p| &mut p.gp) else {
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
        for pad in self.pads.iter_mut() {
            let _ = pad.gp.set_rumble(0, 0, 0);
            if let Some(raw) = pad_raw(&pad.gp) {
                unsafe { SDL_RumbleGamepadTriggers(raw, 0, 0, 0) };
            }
        }
    }

    /// Fires a rumble effect on the trigger (adaptive) motors.
    /// Only supported on controllers with per-trigger haptics (e.g. DualSense, Xbox Series).
    pub fn trigger_rumble(&mut self, left: f32, right: f32, duration_ms: u32) -> bool {
        let Some(raw) = self.active().and_then(pad_raw) else {
            return false;
        };
        // SDL3: SDL_RumbleGamepadTriggers(gamepad, left_rumble_u16, right_rumble_u16, duration_ms)
        let left_u16 = (left.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        let right_u16 = (right.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        unsafe { SDL_RumbleGamepadTriggers(raw, left_u16, right_u16, duration_ms.min(30_000)) }
    }

    /// Enables the gyroscope sensor on the active gamepad.
    /// Must be called once after connect before `read_gyro` returns data.
    /// Returns `true` if the controller supports gyro and it was enabled.
    pub fn enable_gyro(&self) -> bool {
        // SDL_SensorType: SDL_SENSOR_GYRO = 2
        unsafe extern "C" {
            fn SDL_SetGamepadSensorEnabled(
                gamepad: *mut sdl3::sys::gamepad::SDL_Gamepad,
                sensor_type: ::std::ffi::c_int,
                enabled: bool,
            ) -> bool;
        }
        let mut any = false;
        for raw in self.pads.iter().filter_map(|p| pad_raw(&p.gp)) {
            any |= unsafe { SDL_SetGamepadSensorEnabled(raw, 2, true) };
        }
        any
    }

    /// Reads the current gyroscope angular velocity (rad/s).
    /// Returns `Some((pitch, yaw, roll))` where pitch is rotation around the X axis,
    /// yaw around Y, and roll around Z.
    /// Returns `None` when no gamepad is connected or the sensor is unavailable.
    pub fn read_gyro(&self) -> Option<(f32, f32, f32)> {
        let raw = self.active().and_then(pad_raw)?;
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

pub fn map_power_info(state: i32, pct: i32, xbox_wired: bool) -> (i32, bool, bool) {
    let charging = state == 3 || state == 4;
    let pct = if (0..=100).contains(&pct) { pct } else { -1 };
    match state {
        2 => (-1, false, true),
        1 | 3 | 4 if xbox_wired => (-1, charging, true),
        1 | 3 | 4 => (pct, charging, false),
        _ => (pct, false, false),
    }
}

const BT_BATTERY_TTL: Duration = Duration::from_secs(30);

/// SDL3's own DualSense / DS4 player colours (`SetLedsForPlayerIndex` in its HIDAPI drivers).
fn player_led_color(index: usize) -> (u8, u8, u8) {
    const COLORS: [(u8, u8, u8); 7] = [
        (0x00, 0x00, 0x40),
        (0x40, 0x00, 0x00),
        (0x00, 0x40, 0x00),
        (0x20, 0x00, 0x20),
        (0x20, 0x10, 0x00),
        (0x00, 0x10, 0x10),
        (0x10, 0x10, 0x10),
    ];
    COLORS[index % COLORS.len()]
}

fn bluetooth_address(path: &str) -> Option<String> {
    path.split(['&', '#', '\\', '_'])
        .find(|t| t.len() == 12 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|t| t.to_ascii_uppercase())
}

#[cfg(windows)]
fn present_device_ids(enumerator: &str) -> Vec<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_GETIDLIST_FILTER_ENUMERATOR,
        CM_GETIDLIST_FILTER_PRESENT, CR_SUCCESS,
    };
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    let filter: Vec<u16> = enumerator.encode_utf16().chain([0]).collect();
    let mut len = 0u32;
    if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR(filter.as_ptr()), flags) }
        != CR_SUCCESS
    {
        return Vec::new();
    }
    let mut list = vec![0u16; len as usize];
    if unsafe { CM_Get_Device_ID_ListW(PCWSTR(filter.as_ptr()), &mut list, flags) } != CR_SUCCESS {
        return Vec::new();
    }
    list.split(|&c| c == 0)
        .filter(|id| !id.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(windows)]
fn hid_bluetooth_address(vid: u16, pid: u16) -> Option<String> {
    let needle = format!("VID_{vid:04X}&PID_{pid:04X}");
    let mut addrs: Vec<String> = present_device_ids("HID")
        .into_iter()
        .map(|id| id.to_ascii_uppercase())
        .filter(|id| id.contains("00805F9B34FB") && id.contains(&needle))
        .filter_map(|id| bluetooth_address(&id))
        .collect();
    addrs.dedup();
    match addrs.as_slice() {
        [addr] => Some(addr.clone()),
        _ => None,
    }
}

#[cfg(windows)]
fn pnp_battery_percent(addr: &str) -> Option<i32> {
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
    };
    use windows::Win32::Devices::Properties::{DEVPROPKEY, DEVPROPTYPE, DEVPROP_TYPE_BYTE};
    const KEY: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
        pid: 2,
    };
    let ids = present_device_ids("BTHLE")
        .into_iter()
        .chain(present_device_ids("BTHENUM"));
    for id in ids.filter(|id| id.to_ascii_uppercase().contains(addr)) {
        let id: Vec<u16> = id.encode_utf16().chain([0]).collect();
        let mut node = 0u32;
        if unsafe { CM_Locate_DevNodeW(&mut node, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) }
            != CR_SUCCESS
        {
            continue;
        }
        let mut ty = DEVPROPTYPE(0);
        let mut value = 0u8;
        let mut size = 1u32;
        let ok = unsafe {
            CM_Get_DevNode_PropertyW(node, &KEY, &mut ty, Some(&mut value), &mut size, 0)
        } == CR_SUCCESS;
        if ok && ty == DEVPROP_TYPE_BYTE && size == 1 && value <= 100 {
            return Some(value as i32);
        }
    }
    None
}

#[cfg(not(windows))]
fn hid_bluetooth_address(_vid: u16, _pid: u16) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn pnp_battery_percent(_addr: &str) -> Option<i32> {
    None
}

pub fn wired_from_connection_state(state: i32) -> Option<bool> {
    match state {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_state_maps_to_wired_or_wireless() {
        assert_eq!(wired_from_connection_state(1), Some(true));
        assert_eq!(wired_from_connection_state(2), Some(false));
        assert_eq!(wired_from_connection_state(0), None);
        assert_eq!(wired_from_connection_state(-1), None);
    }

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

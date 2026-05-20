//! XInput polling for Session-0 service / secure desktop (sign-in, UAC).

use std::time::{Duration, Instant};

use windows::Win32::UI::Input::XboxController::{
    XInputGetState, XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_DPAD_DOWN,
    XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP,
    XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_X,
    XINPUT_GAMEPAD_Y, XINPUT_STATE,
};

use crate::gamepad_backend::ButtonChange;
use crate::gamepad_backend::GamepadBackend;

const SLOTS: u32 = 4;
const ERROR_SUCCESS: u32 = 0;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

const LEFT_DEADZONE: i16 = 7849;
const RIGHT_DEADZONE: i16 = 8689;

fn button_masks() -> [(&'static str, u16); 10] {
    [
        ("UP", XINPUT_GAMEPAD_DPAD_UP.0),
        ("DOWN", XINPUT_GAMEPAD_DPAD_DOWN.0),
        ("LEFT", XINPUT_GAMEPAD_DPAD_LEFT.0),
        ("RIGHT", XINPUT_GAMEPAD_DPAD_RIGHT.0),
        ("A", XINPUT_GAMEPAD_A.0),
        ("B", XINPUT_GAMEPAD_B.0),
        ("X", XINPUT_GAMEPAD_X.0),
        ("Y", XINPUT_GAMEPAD_Y.0),
        ("LB", XINPUT_GAMEPAD_LEFT_SHOULDER.0),
        ("RB", XINPUT_GAMEPAD_RIGHT_SHOULDER.0),
    ]
}

pub struct XInputBackend {
    active_slot: Option<u32>,
    prev_buttons: [u16; 4],
    slot_connected: [bool; 4],
    pending: Vec<ButtonChange>,
    axes: (f32, f32, f32, f32),
    last_status_log: Instant,
    last_no_pad_log: Instant,
}

impl XInputBackend {
    pub fn new() -> Self {
        Self {
            active_slot: None,
            prev_buttons: [0; 4],
            slot_connected: [false; 4],
            pending: Vec::new(),
            axes: (0.0, 0.0, 0.0, 0.0),
            last_status_log: Instant::now() - Duration::from_secs(60),
            last_no_pad_log: Instant::now() - Duration::from_secs(60),
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

    fn norm_thumb(value: i16, deadzone: i16) -> f32 {
        let v = value as f32;
        if v.abs() < deadzone as f32 {
            return 0.0;
        }
        (v / 32767.0).clamp(-1.0, 1.0)
    }

    fn edges(prev: u16, cur: u16) -> Vec<ButtonChange> {
        let mut out = Vec::new();
        for (name, mask) in button_masks() {
            let was = prev & mask != 0;
            let now = cur & mask != 0;
            if was != now {
                out.push(ButtonChange {
                    button_name: name,
                    pressed: now,
                });
            }
        }
        out
    }
}

impl GamepadBackend for XInputBackend {
    fn poll(&mut self) -> Result<(), String> {
        self.pending.clear();
        let mut connected = [false; 4];
        let mut states: [Option<XINPUT_GAMEPAD>; 4] = [None; 4];

        for slot in 0..SLOTS {
            let mut state = XINPUT_STATE::default();
            let err = unsafe { XInputGetState(slot, &mut state) };
            if err == ERROR_SUCCESS {
                connected[slot as usize] = true;
                states[slot as usize] = Some(state.Gamepad);
            } else if err != ERROR_DEVICE_NOT_CONNECTED {
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
        self.pending = Self::edges(prev, cur);
        self.axes = (
            Self::norm_thumb(pad.sThumbLX, LEFT_DEADZONE),
            Self::norm_thumb(pad.sThumbLY, LEFT_DEADZONE),
            Self::norm_thumb(pad.sThumbRX, RIGHT_DEADZONE),
            Self::norm_thumb(pad.sThumbRY, RIGHT_DEADZONE),
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
        match self.active_slot {
            Some(i) => format!("XInput slot {i}"),
            None => "none".to_string(),
        }
    }
}

fn service_log(msg: &str) {
    if std::env::var_os("WARMUP_VK_SERVICE").is_some_and(|v| v != "0") {
        crate::install::log_line(msg);
    }
}

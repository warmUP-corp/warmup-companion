//! Device-decode seam: the single place raw pad state becomes buttons + edges.
//!
//! Every transport — XInput slot, XUSB IOCTL, vendor HID — normalizes into a
//! [`PadSample`]. The one [`BUTTON_MASKS`] table maps bits to [`Button`]s;
//! nothing else hard-codes a button bit. [`button_edges`]/[`trigger_edges`] are
//! the pure prev→cur edge core shared by every path. [`DevicePoller`] is the
//! seam each transport satisfies (`HidReader`, `XusbDevice`) so pads can be
//! polled — and tested — uniformly through one currency type.

use crate::gamepad_backend::{Button, ButtonChange};
use crate::hid_gamepad::PadSample;
use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK, XINPUT_GAMEPAD_DPAD_DOWN,
    XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP,
    XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_LEFT_THUMB, XINPUT_GAMEPAD_RIGHT_SHOULDER,
    XINPUT_GAMEPAD_RIGHT_THUMB, XINPUT_GAMEPAD_START, XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y,
};

/// XInput "Guide" (center) button bit — not exposed as a named constant by the
/// windows crate.
pub const GUIDE_BUTTON_MASK: u16 = 0x0400;

/// Left/right thumbstick inner deadzones (XInput reference values).
pub const LEFT_DEADZONE: i16 = 7849;
pub const RIGHT_DEADZONE: i16 = 8689;

/// Analog trigger pressed threshold (0–255), matching SDL `TRIGGER_THRESHOLD` feel.
pub const TRIGGER_PRESS_THRESH: u8 = 30;

/// The one [`Button`] ↔ XInput mask table. XInput, XUSB, and HID all map through
/// this; nothing else hard-codes the bit for a button.
pub const BUTTON_MASKS: &[(Button, u16)] = &[
    (Button::Up, XINPUT_GAMEPAD_DPAD_UP.0),
    (Button::Down, XINPUT_GAMEPAD_DPAD_DOWN.0),
    (Button::Left, XINPUT_GAMEPAD_DPAD_LEFT.0),
    (Button::Right, XINPUT_GAMEPAD_DPAD_RIGHT.0),
    (Button::A, XINPUT_GAMEPAD_A.0),
    (Button::B, XINPUT_GAMEPAD_B.0),
    (Button::X, XINPUT_GAMEPAD_X.0),
    (Button::Y, XINPUT_GAMEPAD_Y.0),
    (Button::Lb, XINPUT_GAMEPAD_LEFT_SHOULDER.0),
    (Button::Rb, XINPUT_GAMEPAD_RIGHT_SHOULDER.0),
    (Button::Select, XINPUT_GAMEPAD_BACK.0),
    (Button::Start, XINPUT_GAMEPAD_START.0),
    (Button::L3, XINPUT_GAMEPAD_LEFT_THUMB.0),
    (Button::R3, XINPUT_GAMEPAD_RIGHT_THUMB.0),
    (Button::Guide, GUIDE_BUTTON_MASK),
];

/// XInput mask bit for a button, if it has one.
#[allow(dead_code)] // seam helper exercised by the decode tests; no live caller yet
pub fn button_mask(button: Button) -> Option<u16> {
    BUTTON_MASKS
        .iter()
        .find_map(|&(b, mask)| (b == button).then_some(mask))
}

/// Normalize a raw thumbstick axis (i16) to [-1, 1], zeroing inside `deadzone`.
pub fn norm_thumb(value: i16, deadzone: i16) -> f32 {
    let v = value as f32;
    if v.abs() < deadzone as f32 {
        return 0.0;
    }
    (v / 32767.0).clamp(-1.0, 1.0)
}

/// Press/release edges between two button masks, through [`BUTTON_MASKS`].
pub fn button_edges(prev: u16, cur: u16) -> Vec<ButtonChange> {
    let mut out = Vec::new();
    for &(button, mask) in BUTTON_MASKS {
        let was = prev & mask != 0;
        let now = cur & mask != 0;
        if was != now {
            out.push(ButtonChange {
                button,
                pressed: now,
            });
        }
    }
    out
}

/// Analog-trigger press/release edges past [`TRIGGER_PRESS_THRESH`]. `prev_*`
/// hold the last pressed state across calls (the caller owns the storage).
pub fn trigger_edges(
    prev_lt: &mut bool,
    prev_rt: &mut bool,
    left: u8,
    right: u8,
    out: &mut Vec<ButtonChange>,
) {
    let lt = left > TRIGGER_PRESS_THRESH;
    let rt = right > TRIGGER_PRESS_THRESH;
    if lt != *prev_lt {
        *prev_lt = lt;
        out.push(ButtonChange {
            button: Button::Lt,
            pressed: lt,
        });
    }
    if rt != *prev_rt {
        *prev_rt = rt;
        out.push(ButtonChange {
            button: Button::Rt,
            pressed: rt,
        });
    }
}

/// Stateful edge core: feed a stream of [`PadSample`]s, get clean button +
/// trigger [`ButtonChange`] edges. One owned prev-state instead of threading
/// `prev`/`prev_lt`/`prev_rt` by hand.
///
/// `allow(dead_code)`: the stateful prod consumer is the secure poll thread,
/// which still tracks prev by hand; it adopts `EdgeTracker` with the secure
/// split. Exercised now by the tests below and ready for that slice.
#[allow(dead_code)]
#[derive(Default)]
pub struct EdgeTracker {
    prev_buttons: u16,
    prev_lt: bool,
    prev_rt: bool,
}

#[allow(dead_code)]
impl EdgeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one decoded sample; returns the button + trigger edges since the
    /// previous sample.
    pub fn feed(&mut self, sample: &PadSample) -> Vec<ButtonChange> {
        let mut edges = button_edges(self.prev_buttons, sample.buttons);
        self.prev_buttons = sample.buttons;
        trigger_edges(
            &mut self.prev_lt,
            &mut self.prev_rt,
            sample.lt,
            sample.rt,
            &mut edges,
        );
        edges
    }
}

/// The device seam: any pad transport that can be polled for the next decoded
/// sample. Implemented by `HidReader` (vendor HID) and `XusbDevice` (Xbox IOCTL);
/// XInput slots normalize through [`norm_thumb`]/[`button_edges`] directly.
///
/// `allow(dead_code)`: the polymorphic consumer is the secure poll thread, which
/// still calls each transport's inherent `poll` by hand; it adopts the seam with
/// the secure split. The adapters compile and are ready for that slice.
#[allow(dead_code)]
pub trait DevicePoller {
    fn poll(&mut self) -> Option<PadSample>;
    fn label(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(buttons: u16, lt: u8, rt: u8) -> PadSample {
        PadSample {
            buttons,
            lt,
            rt,
            lx: 0.0,
            ly: 0.0,
            rx: 0.0,
            ry: 0.0,
        }
    }

    // ButtonChange has no PartialEq; compare on (button, pressed) pairs.
    fn pairs(edges: Vec<ButtonChange>) -> Vec<(Button, bool)> {
        edges.into_iter().map(|e| (e.button, e.pressed)).collect()
    }

    #[test]
    fn button_edges_report_press_then_release() {
        assert_eq!(
            pairs(button_edges(0, XINPUT_GAMEPAD_A.0)),
            vec![(Button::A, true)]
        );
        assert_eq!(
            pairs(button_edges(XINPUT_GAMEPAD_A.0, 0)),
            vec![(Button::A, false)]
        );
    }

    #[test]
    fn button_mask_round_trips_through_the_one_table() {
        assert_eq!(button_mask(Button::Guide), Some(GUIDE_BUTTON_MASK));
        assert_eq!(button_mask(Button::Select), Some(XINPUT_GAMEPAD_BACK.0));
    }

    #[test]
    fn system_and_stick_buttons_map_through_the_table() {
        let cur = XINPUT_GAMEPAD_BACK.0
            | XINPUT_GAMEPAD_START.0
            | XINPUT_GAMEPAD_LEFT_THUMB.0
            | XINPUT_GAMEPAD_RIGHT_THUMB.0
            | GUIDE_BUTTON_MASK;
        let pressed: Vec<Button> = button_edges(0, cur)
            .into_iter()
            .filter_map(|e| e.pressed.then_some(e.button))
            .collect();
        assert_eq!(
            pressed,
            vec![
                Button::Select,
                Button::Start,
                Button::L3,
                Button::R3,
                Button::Guide,
            ]
        );
    }

    #[test]
    fn norm_thumb_zeroes_inside_deadzone_and_clamps() {
        assert_eq!(norm_thumb(1000, LEFT_DEADZONE), 0.0);
        assert_eq!(norm_thumb(32767, LEFT_DEADZONE), 1.0);
        assert_eq!(norm_thumb(-32768, LEFT_DEADZONE), -1.0);
    }

    #[test]
    fn trigger_edges_fire_once_per_threshold_crossing() {
        let (mut lt, mut rt) = (false, false);
        let mut out = Vec::new();
        trigger_edges(&mut lt, &mut rt, TRIGGER_PRESS_THRESH + 1, 0, &mut out);
        assert_eq!(pairs(out.clone()), vec![(Button::Lt, true)]);
        out.clear();
        // still held — no new edge
        trigger_edges(&mut lt, &mut rt, TRIGGER_PRESS_THRESH + 5, 0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn edge_tracker_feeds_buttons_and_triggers_from_samples() {
        let mut t = EdgeTracker::new();
        let edges = pairs(t.feed(&sample(XINPUT_GAMEPAD_B.0, TRIGGER_PRESS_THRESH + 1, 0)));
        assert!(edges.contains(&(Button::B, true)));
        assert!(edges.contains(&(Button::Lt, true)));
        // unchanged sample → no edges
        assert!(t
            .feed(&sample(XINPUT_GAMEPAD_B.0, TRIGGER_PRESS_THRESH + 1, 0))
            .is_empty());
    }
}

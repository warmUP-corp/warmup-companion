//! Companion IPC wire frames (#347). Serde types for the NDJSON contract in
//! `docs/companion-ipc-protocol.md`. This slice carries only the frames #347
//! needs — `hello` (handshake) and `connection` (up). Later slices add `button`,
//! `battery`, `cursor_moved`, `touchpad`, `config`, `mode`, `rumble`.
//!
//! Mirror of `warmUp/apps/desktop/src-tauri/src/gamepad/protocol.rs`.

use serde::{Deserialize, Serialize};

/// Wire protocol version. Bumped on any breaking frame/framing change. A `hello`
/// mismatch closes the connection (see ADR 0002).
///
/// v2: additive customisation fields on the `config` frame — `ledEffect`,
/// `ledBrightness`, `naturalScroll`, `cursorSmoothing` (consumed by the desktop /
/// future companion device control), alongside the existing `keyboardTheme`.
pub const PROTOCOL_VERSION: u32 = 2;

/// Desktop mode snapshot carried in `hello` and the `mode` down-frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeSnapshot {
    pub game_active: bool,
    pub launcher_foreground_nav: bool,
    #[serde(default)]
    pub clicks_enabled: bool,
    #[serde(default)]
    pub launcher_owns_text_input: bool,
}

/// `hello` handshake payload. The client (desktop) includes its config/mode
/// snapshot; the server (companion) replies with version only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol_version: u32,
    /// Desktop `GamepadConfig` snapshot, opaque to the companion until applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeSnapshot>,
}

/// `connection` frame payload — authoritative controller connection snapshot.
/// Shape matches the desktop `gamepad:connection` webview event 1:1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionPayload {
    pub connected: bool,
    pub controller_type: String,
    pub controller_name: String,
}

/// `button` frame payload — one press/release edge (incl. synthesised LT/RT).
/// Shape matches the desktop `gamepad:button` webview event 1:1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonPayload {
    pub button: String,
    pub pressed: bool,
    pub controller_type: String,
}

/// `cursor_moved` frame payload — visual-cursor hint (pixels this frame). Shape matches
/// the desktop `gamepad:cursor_moved` webview event 1:1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorMovedPayload {
    pub dx: f64,
    pub dy: f64,
}

/// `battery` frame payload. `percent` is 0–100 or −1 when unknown; `wired` = no
/// internal battery. Shape matches the desktop `gamepad:battery` webview event 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryPayload {
    pub percent: i32,
    pub charging: bool,
    pub wired: bool,
}

/// One touchpad finger slot. `x`/`y` are normalized 0..1; `pressure` 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchpadFingerPayload {
    pub index: u8,
    pub down: bool,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// `touchpad` frame payload — every supported finger slot this poll. Shape matches
/// the desktop `gamepad:touchpad` webview event 1:1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchpadPayload {
    pub fingers: Vec<TouchpadFingerPayload>,
}

/// `rumble` down-frame payload — one-shot force feedback (#352). Internally tagged by
/// `kind`: `"full"` drives the main motors, `"triggers"` the adaptive-trigger motors.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RumblePayload {
    Full {
        strong: f32,
        weak: f32,
        #[serde(rename = "durationMs")]
        duration_ms: u32,
    },
    Triggers {
        left: f32,
        right: f32,
        #[serde(rename = "durationMs")]
        duration_ms: u32,
    },
}

/// Optional native keyboard theme colors. Each field is `#RRGGBB`; absent fields keep
/// the companion's current dark/light default for that slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardThemePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_text: Option<String>,
}

/// `config` down-frame payload — cursor-relevant tuning the companion applies, plus the
/// `clicksEnabled` mode (cursor vs focus/D-pad). Device features (LED/rumble/gyro) ride
/// later frames (#352).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPayload {
    pub deadzone: f32,
    pub sensitivity: f32,
    pub acceleration_exp: f32,
    pub scroll_sensitivity: f32,
    pub enabled: bool,
    pub clicks_enabled: bool,
    /// Lightbar/LED colour `#RRGGBB` for pads with an LED (DualSense / DS4).
    /// Absent leaves the current colour untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_theme: Option<KeyboardThemePayload>,
}

/// Up frames (companion → desktop). This slice (#347) knows `hello` + `connection`;
/// any other frame type deserializes to [`UpFrame::Unknown`] so the client tolerates
/// frames added by later slices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum UpFrame {
    Hello(Hello),
    Connection(ConnectionPayload),
    Button(ButtonPayload),
    CursorMoved(CursorMovedPayload),
    Battery(BatteryPayload),
    Touchpad(TouchpadPayload),
    /// A frame whose `type` this slice does not know (added by a later slice).
    /// Never serialized; produced only by [`UpFrame::parse_line`].
    #[serde(skip)]
    Unknown,
}

/// Down frames (desktop → companion): `hello` handshake + live `config` push.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum DownFrame {
    Hello(Hello),
    Config(ConfigPayload),
    Mode(ModeSnapshot),
    Rumble(RumblePayload),
    #[serde(skip)]
    Unknown,
}

/// Just the tag, for tolerating unknown frame types without discarding malformed JSON.
#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    ty: String,
}

/// One NDJSON line: the frame's JSON object followed by `\n`.
fn to_ndjson_line<T: Serialize>(frame: &T) -> String {
    let mut s = serde_json::to_string(frame).expect("frame serializes");
    s.push('\n');
    s
}

impl UpFrame {
    pub fn to_ndjson_line(&self) -> String {
        to_ndjson_line(self)
    }
    /// Parse a single NDJSON line (without the trailing newline). Malformed JSON is an
    /// error; a well-formed frame with an unrecognised `type` yields [`UpFrame::Unknown`].
    pub fn parse_line(line: &str) -> Result<Self, serde_json::Error> {
        let env: Envelope = serde_json::from_str(line)?;
        match env.ty.as_str() {
            "hello" | "connection" | "button" | "cursor_moved" | "battery" | "touchpad" => {
                serde_json::from_str(line)
            }
            _ => Ok(Self::Unknown),
        }
    }
}

impl DownFrame {
    pub fn to_ndjson_line(&self) -> String {
        to_ndjson_line(self)
    }
    pub fn parse_line(line: &str) -> Result<Self, serde_json::Error> {
        let env: Envelope = serde_json::from_str(line)?;
        match env.ty.as_str() {
            "hello" | "config" | "mode" | "rumble" => serde_json::from_str(line),
            _ => Ok(Self::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_frame_serializes_to_adjacently_tagged_ndjson() {
        let frame = UpFrame::Connection(ConnectionPayload {
            connected: true,
            controller_type: "xbox".into(),
            controller_name: "Xbox Wireless Controller".into(),
        });
        let line = frame.to_ndjson_line();
        assert!(line.ends_with('\n'), "NDJSON line must end with newline");
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "connection");
        assert_eq!(json["payload"]["connected"], true);
        assert_eq!(json["payload"]["controllerType"], "xbox");
        assert_eq!(
            json["payload"]["controllerName"],
            "Xbox Wireless Controller"
        );
    }

    #[test]
    fn connection_frame_round_trips() {
        let frame = UpFrame::Connection(ConnectionPayload {
            connected: false,
            controller_type: "generic".into(),
            controller_name: String::new(),
        });
        let line = frame.to_ndjson_line();
        let parsed = UpFrame::parse_line(line.trim_end()).unwrap();
        assert_eq!(parsed, frame);
    }

    #[test]
    fn button_frame_round_trips_and_is_adjacently_tagged() {
        let frame = UpFrame::Button(ButtonPayload {
            button: "GUIDE".into(),
            pressed: true,
            controller_type: "xbox".into(),
        });
        let line = frame.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "button");
        assert_eq!(json["payload"]["button"], "GUIDE");
        assert_eq!(json["payload"]["pressed"], true);
        assert_eq!(json["payload"]["controllerType"], "xbox");
        assert_eq!(UpFrame::parse_line(line.trim_end()).unwrap(), frame);
    }

    #[test]
    fn config_down_frame_round_trips() {
        let frame = DownFrame::Config(ConfigPayload {
            deadzone: 0.15,
            sensitivity: 15.0,
            acceleration_exp: 2.0,
            scroll_sensitivity: 5.0,
            enabled: true,
            clicks_enabled: false,
            led_color: Some("#b6a0ff".into()),
            keyboard_theme: Some(KeyboardThemePayload {
                background: Some("#101010".into()),
                key: Some("#202020".into()),
                accent: Some("#4C7B99".into()),
                text: Some("#FFFFFF".into()),
                selected_text: Some("#FFFFFF".into()),
            }),
        });
        let line = frame.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "config");
        assert_eq!(json["payload"]["clicksEnabled"], false);
        assert_eq!(json["payload"]["ledColor"], "#b6a0ff");
        assert_eq!(json["payload"]["accelerationExp"], 2.0);
        assert_eq!(json["payload"]["keyboardTheme"]["background"], "#101010");
        assert_eq!(json["payload"]["keyboardTheme"]["selectedText"], "#FFFFFF");
        assert_eq!(DownFrame::parse_line(line.trim_end()).unwrap(), frame);
    }

    #[test]
    fn mode_down_frame_round_trips() {
        let frame = DownFrame::Mode(ModeSnapshot {
            game_active: false,
            launcher_foreground_nav: true,
            clicks_enabled: false,
            launcher_owns_text_input: true,
        });
        let line = frame.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "mode");
        assert_eq!(json["payload"]["launcherOwnsTextInput"], true);
        assert_eq!(DownFrame::parse_line(line.trim_end()).unwrap(), frame);
    }

    #[test]
    fn cursor_moved_up_frame_round_trips() {
        let frame = UpFrame::CursorMoved(CursorMovedPayload { dx: 1.5, dy: -2.0 });
        let line = frame.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "cursor_moved");
        assert_eq!(json["payload"]["dx"], 1.5);
        assert_eq!(UpFrame::parse_line(line.trim_end()).unwrap(), frame);
    }

    #[test]
    fn unknown_up_frame_type_is_tolerated() {
        // A future frame (one this version doesn't know) must not break the client.
        let parsed =
            UpFrame::parse_line(r#"{"type":"gyro","payload":{"pitch":0.1}}"#).unwrap();
        assert_eq!(parsed, UpFrame::Unknown);
    }

    #[test]
    fn battery_up_frame_round_trips() {
        let frame = UpFrame::Battery(BatteryPayload {
            percent: 80,
            charging: true,
            wired: false,
        });
        let line = frame.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "battery");
        assert_eq!(json["payload"]["percent"], 80);
        assert_eq!(json["payload"]["charging"], true);
        assert_eq!(UpFrame::parse_line(line.trim_end()).unwrap(), frame);
    }

    #[test]
    fn touchpad_up_frame_round_trips() {
        let frame = UpFrame::Touchpad(TouchpadPayload {
            fingers: vec![TouchpadFingerPayload {
                index: 0,
                down: true,
                x: 0.5,
                y: 0.25,
                pressure: 1.0,
            }],
        });
        let line = frame.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "touchpad");
        assert_eq!(json["payload"]["fingers"][0]["down"], true);
        assert_eq!(json["payload"]["fingers"][0]["x"], 0.5);
        assert_eq!(UpFrame::parse_line(line.trim_end()).unwrap(), frame);
    }

    #[test]
    fn rumble_down_frame_round_trips_both_kinds() {
        let full = DownFrame::Rumble(RumblePayload::Full {
            strong: 0.8,
            weak: 0.4,
            duration_ms: 200,
        });
        let line = full.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "rumble");
        assert_eq!(json["payload"]["kind"], "full");
        assert_eq!(json["payload"]["durationMs"], 200);
        assert_eq!(DownFrame::parse_line(line.trim_end()).unwrap(), full);

        let trig = DownFrame::Rumble(RumblePayload::Triggers {
            left: 0.3,
            right: 0.6,
            duration_ms: 150,
        });
        let line = trig.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["payload"]["kind"], "triggers");
        assert_eq!(json["payload"]["left"], 0.3);
        assert_eq!(DownFrame::parse_line(line.trim_end()).unwrap(), trig);
    }

    #[test]
    fn hello_carries_protocol_version() {
        let hello = DownFrame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            config: None,
            mode: Some(ModeSnapshot {
                game_active: false,
                launcher_foreground_nav: false,
                clicks_enabled: true,
                launcher_owns_text_input: false,
            }),
        });
        let line = hello.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "hello");
        assert_eq!(json["payload"]["protocolVersion"], 2);
        assert_eq!(json["payload"]["mode"]["gameActive"], false);
    }
}

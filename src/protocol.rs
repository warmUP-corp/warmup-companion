//! Companion IPC wire frames (#347). Serde types for the NDJSON contract in
//! `docs/companion-ipc-protocol.md`. This slice carries only the frames #347
//! needs — `hello` (handshake) and `connection` (up). Later slices add `button`,
//! `battery`, `cursor_moved`, `touchpad`, `config`, `mode`, `rumble`.
//!
//! Mirror of `warmUp/apps/desktop/src-tauri/src/gamepad/protocol.rs`.

use serde::{Deserialize, Serialize};

/// Wire protocol version. Bumped on any breaking frame/framing change. A `hello`
/// mismatch closes the connection (see ADR 0002).
pub const PROTOCOL_VERSION: u32 = 1;

/// Desktop mode snapshot carried in `hello` and the `mode` down-frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeSnapshot {
    pub game_active: bool,
    pub launcher_foreground_nav: bool,
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

/// Up frames (companion → desktop). This slice (#347) knows `hello` + `connection`;
/// any other frame type deserializes to [`UpFrame::Unknown`] so the client tolerates
/// frames added by later slices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum UpFrame {
    Hello(Hello),
    Connection(ConnectionPayload),
    /// A frame whose `type` this slice does not know (added by a later slice).
    /// Never serialized; produced only by [`UpFrame::parse_line`].
    #[serde(skip)]
    Unknown,
}

/// Down frames (desktop → companion). This slice sends only `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum DownFrame {
    Hello(Hello),
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
            "hello" | "connection" => serde_json::from_str(line),
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
            "hello" => serde_json::from_str(line),
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
        assert_eq!(json["payload"]["controllerName"], "Xbox Wireless Controller");
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
    fn unknown_up_frame_type_is_tolerated() {
        // A future frame (e.g. battery) must not break a client that only knows connection.
        let parsed = UpFrame::parse_line(r#"{"type":"battery","payload":{"percent":50}}"#).unwrap();
        assert_eq!(parsed, UpFrame::Unknown);
    }

    #[test]
    fn hello_carries_protocol_version() {
        let hello = DownFrame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            config: None,
            mode: Some(ModeSnapshot {
                game_active: false,
                launcher_foreground_nav: false,
            }),
        });
        let line = hello.to_ndjson_line();
        let json: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(json["type"], "hello");
        assert_eq!(json["payload"]["protocolVersion"], 1);
        assert_eq!(json["payload"]["mode"]["gameActive"], false);
    }
}

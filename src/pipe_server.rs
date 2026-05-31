//! Named-pipe server (#347): the companion hosts `\\.\pipe\warmup-input` and streams
//! `connection` frames to the warmUP desktop client. The companion is always running,
//! so it is the server; the desktop is a reconnecting client (ADR 0002 /
//! `docs/companion-ipc-protocol.md`).
//!
//! The gamepad loop calls [`publish_from_label`] every frame with the active backend's
//! controller label; the server thread streams the latest connection snapshot to the
//! connected client. The pipe is ACL'd to the interactive user.

use crate::protocol::{ButtonPayload, ConnectionPayload};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Cursor mode (A → OS left-click). `false` = focus/D-pad mode (buttons only). Default true;
/// the connected desktop pushes the real value via `config` frames (#349).
static CLICKS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Coalesced visual-cursor hint accumulated since the last send: `(dx, dy, dirty)`.
static CURSOR_ACC: OnceLock<Mutex<(f64, f64, bool)>> = OnceLock::new();

fn cursor_acc() -> &'static Mutex<(f64, f64, bool)> {
    CURSOR_ACC.get_or_init(|| Mutex::new((0.0, 0.0, false)))
}

/// Whether A should inject an OS left-click (cursor mode). Read by the gamepad loop.
pub fn clicks_enabled() -> bool {
    CLICKS_ENABLED.load(Ordering::Relaxed)
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

/// Apply a pushed `config`: write through to the companion's cursor settings (read live by
/// `pc_cursor`) and set the clicks-enabled mode. Maps desktop fields → companion params.
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_config(p: &crate::protocol::ConfigPayload) {
    CLICKS_ENABLED.store(p.clicks_enabled, Ordering::Relaxed);
    let _ = crate::config::set_gamepad_setting("cursor_deadzone", &p.deadzone.to_string());
    let _ = crate::config::set_gamepad_setting("cursor_speed", &p.sensitivity.to_string());
    let _ = crate::config::set_gamepad_setting("cursor_accel", &p.acceleration_exp.to_string());
    let _ = crate::config::set_gamepad_setting("scroll_speed", &p.scroll_sensitivity.to_string());
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
        "ps5".into()
    } else if l.contains("dualshock") || l.contains("ps4") {
        "ps4".into()
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
    use super::{apply_config, current, drain_buttons, reset_button_stream, take_cursor_moved};
    use crate::protocol::{ConnectionPayload, DownFrame, Hello, UpFrame, PROTOCOL_VERSION};
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
            stream(pipe);
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
        }
    }

    /// Read the client `hello`, reject on version mismatch, reply with our `hello`.
    fn handshake(pipe: HANDLE) -> std::io::Result<()> {
        let line = read_line(pipe)?;
        match DownFrame::parse_line(line.trim_end()) {
            Ok(DownFrame::Hello(h)) if h.protocol_version == PROTOCOL_VERSION => {}
            _ => return Err(io_err("hello rejected (missing or version mismatch)")),
        }
        let reply = UpFrame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            config: None,
            mode: None,
        });
        write_all(pipe, reply.to_ndjson_line().as_bytes())
    }

    /// Full-duplex session: drain inbound `config` (non-blocking), and write button edges
    /// (low latency), `cursor_moved` hints (throttled), and connection snapshots (on change
    /// plus a keepalive so a dropped idle client is noticed via the write error).
    fn stream(pipe: HANDLE) {
        let mut last: Option<ConnectionPayload> = None;
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
            if last_cursor_write.elapsed() >= CURSOR_HINT_INTERVAL {
                if let Some(hint) = take_cursor_moved() {
                    if write_all(pipe, UpFrame::CursorMoved(hint).to_ndjson_line().as_bytes())
                        .is_err()
                    {
                        return;
                    }
                }
                last_cursor_write = Instant::now();
            }
            let cur = current();
            if last.as_ref() != Some(&cur) || last_conn_write.elapsed() >= KEEPALIVE {
                if write_all(pipe, UpFrame::Connection(cur.clone()).to_ndjson_line().as_bytes())
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
            if let Ok(DownFrame::Config(p)) = DownFrame::parse_line(trimmed) {
                apply_config(&p);
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
}

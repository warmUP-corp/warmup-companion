use std::collections::HashSet;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use std::sync::atomic::Ordering;

use crate::protocol::{
    is_supported_protocol_version, AxisPayload, BatteryPayload, ConnectionPayload, DownFrame,
    Hello, PadBatteryPayload, PadsPayload, UpFrame, PROTOCOL_VERSION,
};
use crate::tracking_owner::TrackingOwner;

use super::{
    apply_companion_settings, apply_config, apply_led, apply_library_watch, apply_mode,
    apply_parental_guard, apply_play_sessions_ack, apply_rumble, companion_snapshot, current,
    current_axis, current_battery, current_pads, drain_buttons, drain_parental_blocked,
    ensure_settings_watcher, set_native_vk_request, take_cursor_moved, take_touchpad,
    SettingsSync, SETTINGS_GEN,
};

const KEEPALIVE: Duration = Duration::from_secs(1);
const CURSOR_HINT_INTERVAL: Duration = Duration::from_millis(100);
const FRAME_LIMIT: usize = 64 * 1024;

pub(super) struct ClientIdentity {
    pub(super) owner: TrackingOwner,
}

pub(super) struct Session<R, W> {
    reader: R,
    writer: W,
    pending: Vec<u8>,
    eof: bool,
}

impl<R: Read, W: Write> Session<R, W> {
    pub(super) fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            pending: Vec::new(),
            eof: false,
        }
    }

    pub(super) fn read_line(&mut self) -> std::io::Result<String> {
        loop {
            if let Some(line) = self.pop_line()? {
                return Ok(line);
            }
            if self.eof {
                return Err(io_err("connection closed"));
            }
            self.pull_some(true)?;
        }
    }

    pub(super) fn write_frame(&mut self, mut buf: &[u8]) -> std::io::Result<()> {
        while !buf.is_empty() {
            match self.writer.write(buf) {
                Ok(0) => return Err(io_err("write returned 0 bytes")),
                Ok(n) => buf = &buf[n..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(error),
            }
        }
        self.writer.flush()?;
        Ok(())
    }

    fn fill_available(&mut self) -> std::io::Result<()> {
        if self.eof {
            return Ok(());
        }
        let mut buf = [0u8; 4096];
        loop {
            match self.reader.read(&mut buf) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.pending.extend_from_slice(&buf[..n]);
                    if self.pending.len() > FRAME_LIMIT && !self.pending.contains(&b'\n') {
                        return Err(io_err("frame too long"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn pull_some(&mut self, block: bool) -> std::io::Result<()> {
        let mut buf = [0u8; 1024];
        loop {
            match self.reader.read(&mut buf) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.pending.extend_from_slice(&buf[..n]);
                    if self.pending.len() > FRAME_LIMIT && !self.pending.contains(&b'\n') {
                        return Err(io_err("frame too long"));
                    }
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if block {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn pop_line(&mut self) -> std::io::Result<Option<String>> {
        let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') else {
            if self.pending.len() > FRAME_LIMIT {
                return Err(io_err("frame too long"));
            }
            return Ok(None);
        };
        let mut line: Vec<u8> = self.pending.drain(..=index).collect();
        line.pop();
        String::from_utf8(line)
            .map(Some)
            .map_err(|_| io_err("frame not valid UTF-8"))
    }
}

pub(super) fn handshake<R: Read, W: Write>(
    session: &mut Session<R, W>,
    identity: &ClientIdentity,
) -> std::io::Result<(HashSet<String>, SettingsSync)> {
    let line = session.read_line()?;
    let protocol_version = match DownFrame::parse_line(line.trim_end()) {
        Ok(DownFrame::Hello(hello)) if is_supported_protocol_version(hello.protocol_version) => {
            if let Some(mode) = hello.mode {
                apply_mode(&mode);
            }
            if let Some(guard) = hello.parental_guard {
                apply_parental_guard(&guard);
            }
            if let Some(watch) = hello.library_watch {
                apply_library_watch(&identity.owner, &watch);
            }
            hello.protocol_version
        }
        Ok(DownFrame::Hello(hello)) => {
            return Err(io_err(&format!(
                "hello rejected: client protocol_version={}, companion expects {PROTOCOL_VERSION}",
                hello.protocol_version
            )));
        }
        _ => return Err(io_err("hello rejected: missing or invalid hello frame")),
    };
    let (config_snapshot, settings_snapshot) = companion_snapshot();
    let mut sync = SettingsSync::new(settings_snapshot.clone());
    sync.config_reference = Some(config_snapshot.clone());
    let reply = UpFrame::Hello(Hello {
        protocol_version,
        config: serde_json::to_value(config_snapshot).ok(),
        mode: None,
        companion_settings: Some(settings_snapshot),
        parental_guard: None,
        library_watch: None,
    });
    session.write_frame(reply.to_ndjson_line().as_bytes())?;
    #[cfg(all(any(windows, target_os = "linux"), feature = "gamepad"))]
    {
        crate::playtime_tracker::on_desktop_connected(&identity.owner);
        if protocol_version == PROTOCOL_VERSION {
            return flush_play_sessions(session, &identity.owner).map(|issued| (issued, sync));
        }
    }
    Ok((HashSet::new(), sync))
}

#[cfg(all(any(windows, target_os = "linux"), feature = "gamepad"))]
fn flush_play_sessions<R: Read, W: Write>(
    session: &mut Session<R, W>,
    owner: &TrackingOwner,
) -> std::io::Result<HashSet<String>> {
    let payload = crate::playtime_tracker::take_closed_sessions_for_flush(owner);
    if payload.sessions.is_empty() {
        return Ok(HashSet::new());
    }
    let issued = payload
        .sessions
        .iter()
        .map(|play| play.external_id.clone())
        .collect();
    session.write_frame(UpFrame::PlaySessions(payload).to_ndjson_line().as_bytes())?;
    Ok(issued)
}

pub(super) fn stream<R: Read, W: Write>(
    session: &mut Session<R, W>,
    owner: &TrackingOwner,
    mut issued_session_ids: HashSet<String>,
    mut sync: SettingsSync,
) {
    let mut last: Option<ConnectionPayload> = None;
    let mut last_battery: Option<BatteryPayload> = None;
    let mut last_pads: Option<Vec<PadBatteryPayload>> = None;
    let mut last_axis: Option<AxisPayload> = None;
    let mut last_conn_write = Instant::now()
        .checked_sub(KEEPALIVE)
        .unwrap_or_else(Instant::now);
    let mut last_cursor_write = Instant::now();
    ensure_settings_watcher();
    let mut seen = SETTINGS_GEN.load(Ordering::Acquire);
    let mut last_input = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .unwrap_or_else(Instant::now);
    loop {
        let gen = SETTINGS_GEN.load(Ordering::Acquire);
        if gen != seen {
            seen = gen;
            crate::config::invalidate_gamepad_settings_cache();
            let (config, settings) = companion_snapshot();
            if let Some(changed) = sync.absorb_snapshot(&config, &settings) {
                crate::install::log_line("pipe settings_changed sent");
                if session
                    .write_frame(UpFrame::SettingsChanged(changed).to_ndjson_line().as_bytes())
                    .is_err()
                {
                    return;
                }
            }
        }
        if drain_inbound(session, owner, &mut issued_session_ids, &mut sync).is_err() {
            return;
        }
        if session.eof && session.pending.is_empty() {
            return;
        }
        for edge in drain_buttons() {
            last_input = Instant::now();
            if session
                .write_frame(UpFrame::Button(edge).to_ndjson_line().as_bytes())
                .is_err()
            {
                return;
            }
        }
        for blocked in drain_parental_blocked() {
            if session
                .write_frame(UpFrame::ParentalBlocked(blocked).to_ndjson_line().as_bytes())
                .is_err()
            {
                return;
            }
        }
        let battery = current_battery();
        if battery.is_some() && battery != last_battery {
            if let Some(snapshot) = battery {
                if session
                    .write_frame(UpFrame::Battery(snapshot).to_ndjson_line().as_bytes())
                    .is_err()
                {
                    return;
                }
                last_battery = Some(snapshot);
            }
        }
        let pads = current_pads();
        if last_pads.as_ref() != Some(&pads) {
            let frame = UpFrame::Pads(PadsPayload { pads: pads.clone() });
            if session.write_frame(frame.to_ndjson_line().as_bytes()).is_err() {
                return;
            }
            last_pads = Some(pads);
        }
        if last_cursor_write.elapsed() >= CURSOR_HINT_INTERVAL {
            if let Some(hint) = take_cursor_moved() {
                last_input = Instant::now();
                if session
                    .write_frame(UpFrame::CursorMoved(hint).to_ndjson_line().as_bytes())
                    .is_err()
                {
                    return;
                }
            }
            let axis = current_axis();
            if axis.is_some() && axis != last_axis {
                if let Some(snapshot) = axis {
                    last_input = Instant::now();
                    if session
                        .write_frame(UpFrame::Axis(snapshot).to_ndjson_line().as_bytes())
                        .is_err()
                    {
                        return;
                    }
                    last_axis = Some(snapshot);
                }
            }
            if let Some(touchpad) = take_touchpad() {
                last_input = Instant::now();
                if session
                    .write_frame(UpFrame::Touchpad(touchpad).to_ndjson_line().as_bytes())
                    .is_err()
                {
                    return;
                }
            }
            last_cursor_write = Instant::now();
        }
        let connection = current();
        if last.as_ref() != Some(&connection) {
            last_battery = None;
            last_pads = None;
        }
        if last.as_ref() != Some(&connection) || last_conn_write.elapsed() >= KEEPALIVE {
            if session
                .write_frame(
                    UpFrame::Connection(connection.clone())
                        .to_ndjson_line()
                        .as_bytes(),
                )
                .is_err()
            {
                return;
            }
            last = Some(connection);
            last_conn_write = Instant::now();
        }
        let idle = if last_input.elapsed() < Duration::from_secs(1) {
            Duration::from_millis(5)
        } else {
            Duration::from_millis(50)
        };
        std::thread::sleep(idle);
    }
}

fn drain_inbound<R: Read, W: Write>(
    session: &mut Session<R, W>,
    owner: &TrackingOwner,
    issued_session_ids: &mut HashSet<String>,
    sync: &mut SettingsSync,
) -> std::io::Result<()> {
    session.fill_available()?;
    while let Some(line) = session.pop_line()? {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        match DownFrame::parse_line(trimmed) {
            Ok(DownFrame::Config(payload)) => apply_config(sync, &payload),
            Ok(DownFrame::Mode(payload)) => apply_mode(&payload),
            Ok(DownFrame::Rumble(payload)) => apply_rumble(&payload),
            Ok(DownFrame::Led(payload)) => apply_led(&payload),
            Ok(DownFrame::CompanionSettings(payload)) => apply_companion_settings(sync, &payload),
            Ok(DownFrame::NativeVk(payload)) => set_native_vk_request(&payload),
            Ok(DownFrame::ParentalGuard(payload)) => apply_parental_guard(&payload),
            Ok(DownFrame::LibraryWatch(payload)) => apply_library_watch(owner, &payload),
            Ok(DownFrame::PlaySessionsAck(payload)) => {
                let removed = apply_play_sessions_ack(owner, &payload, issued_session_ids);
                if removed != 0 {
                    issued_session_ids
                        .retain(|external_id| !payload.external_ids.contains(external_id));
                }
            }
            Ok(DownFrame::ShutDown(_)) => handle_shut_down(),
            Err(error) => {
                crate::install::log_line(&format!("pipe inbound frame parse error: {error}"));
            }
            _ => {}
        }
    }
    if session.eof && session.pending.is_empty() {
        return Err(io_err("connection closed"));
    }
    Ok(())
}

fn handle_shut_down() {
    #[cfg(all(any(windows, target_os = "linux"), feature = "gamepad"))]
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

fn io_err(message: &str) -> std::io::Error {
    std::io::Error::other(message)
}

pub(super) fn uid_allowed(peer_uid: u32, euid: u32) -> bool {
    peer_uid == euid
}

#[cfg(target_os = "linux")]
pub(super) fn serve_unix() {
    loop {
        let lock = match acquire_lock(&crate::paths::socket_lock_file()) {
            Ok(lock) => lock,
            Err(error) => {
                crate::install::log_line(&format!(
                    "input socket: another companion holds the lock ({error}); retrying"
                ));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let listener = match bind_listener() {
            Ok(listener) => listener,
            Err(error) => {
                crate::install::log_line(&format!("input socket bind failed: {error}"));
                drop(lock);
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        crate::install::log_line(&format!(
            "input socket listening on {}",
            crate::paths::socket_file().display()
        ));
        loop {
            match listener.accept() {
                Ok((stream, _)) => serve_client(stream),
                Err(error) => {
                    crate::install::log_line(&format!("input socket accept failed: {error}"));
                    break;
                }
            }
        }
        drop(lock);
    }
}

#[cfg(target_os = "linux")]
fn acquire_lock(path: &std::path::Path) -> Result<nix::fcntl::Flock<std::fs::File>, String> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("chmod {}: {error}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(lock),
        Err((_, error)) => Err(error.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn bind_listener() -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::paths::runtime_dir();
    let sock = crate::paths::socket_file();
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let _ = std::fs::remove_file(&sock);
    let listener = std::os::unix::net::UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(target_os = "linux")]
fn serve_client(client: std::os::unix::net::UnixStream) {
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::Ordering;
    if !peer_allowed(&client) {
        crate::install::log_line(
            "input socket client rejected: peer uid does not match this user",
        );
        return;
    }
    let uid = nix::unistd::Uid::effective().as_raw();
    if let Err(error) = client.set_nonblocking(true) {
        crate::install::log_line(&format!("input socket set_nonblocking failed: {error}"));
        return;
    }
    let writer: UnixStream = match client.try_clone() {
        Ok(writer) => writer,
        Err(error) => {
            crate::install::log_line(&format!("input socket try_clone failed: {error}"));
            return;
        }
    };
    let identity = ClientIdentity {
        owner: TrackingOwner {
            user_sid: format!("uid:{uid}"),
            session_id: 1,
        },
    };
    super::DESKTOP_CONNECTED.store(true, Ordering::Relaxed);
    let mut session = Session::new(client, writer);
    if let Ok((issued, sync)) = handshake(&mut session, &identity) {
        super::reset_button_stream();
        stream(&mut session, &identity.owner, issued, sync);
    }
    super::DESKTOP_CONNECTED.store(false, Ordering::Relaxed);
    super::clear_desktop_mode();
}

#[cfg(target_os = "linux")]
fn peer_allowed(stream: &std::os::unix::net::UnixStream) -> bool {
    match nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials) {
        Ok(creds) => uid_allowed(creds.uid(), nix::unistd::Uid::effective().as_raw()),
        Err(error) => {
            crate::install::log_line(&format!("input socket peer credentials failed: {error}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::uid_allowed;

    #[test]
    fn peer_uid_must_equal_euid() {
        assert!(uid_allowed(1000, 1000));
        assert!(!uid_allowed(1000, 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unix_pair_handshake_replies_with_version_six() {
        use super::{handshake, ClientIdentity, Session};
        use crate::tracking_owner::TrackingOwner;
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        let (mut client, server) = UnixStream::pair().expect("socket pair");
        server.set_nonblocking(true).expect("nonblocking");
        client.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let writer = server.try_clone().expect("clone");
        writeln!(
            client,
            r#"{{"type":"hello","payload":{{"protocolVersion":6}}}}"#
        )
        .expect("write hello");
        let identity = ClientIdentity {
            owner: TrackingOwner {
                user_sid: "uid:1".into(),
                session_id: 1,
            },
        };
        let mut session = Session::new(server, writer);
        handshake(&mut session, &identity).expect("handshake");
        let mut buf = [0u8; 512];
        let n = client.read(&mut buf).expect("read reply");
        let reply = std::str::from_utf8(&buf[..n]).expect("utf-8");
        assert!(reply.contains("\"protocolVersion\":6"), "{reply}");
    }
}

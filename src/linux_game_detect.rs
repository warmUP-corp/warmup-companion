use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const FULLSCREEN_PERCENT_BASE: i64 = 100;
const EXTERNAL_FULLSCREEN_AXIS_PERCENT: i64 = 85;
const EXTERNAL_FULLSCREEN_OVERLAP_PERCENT: i64 = 72;
const DETECT_INTERVAL: Duration = Duration::from_millis(500);

const EXTERNAL_GAME_EXE_DENYLIST: &[&str] = &[
    "applicationframehost.exe",
    "arc.exe",
    "brave.exe",
    "chrome.exe",
    "chromium.exe",
    "ctfmon.exe",
    "duckduckgo.exe",
    "dllhost.exe",
    "dwm.exe",
    "explorer.exe",
    "firefox.exe",
    "lockapp.exe",
    "msedge.exe",
    "opera.exe",
    "perplexity.exe",
    "runtimebroker.exe",
    "searchhost.exe",
    "searchapp.exe",
    "shellhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "steamwebhelper.exe",
    "systemsettings.exe",
    "textinputhost.exe",
    "webviewhost.exe",
    "widgets.exe",
    "windowsterminal.exe",
    "wt.exe",
    "steam.exe",
    "epicgameslauncher.exe",
    "galaxyclient.exe",
    "gog galaxy.exe",
    "goggalaxy.exe",
    "eadesktop.exe",
    "origin.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "uplay.exe",
    "battle.net.exe",
    "amazon games.exe",
    "nile.exe",
    "xboxpcapp.exe",
    "gamingservices.exe",
    "warmup.exe",
];

#[derive(Clone, Copy)]
struct DetectionCache {
    checked_at: Instant,
    active: bool,
}

#[derive(Clone, Debug)]
struct Surface {
    exe: String,
    class_name: String,
    title: String,
    pid: u32,
    our_pid: u32,
    fullscreen: bool,
    floating: bool,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    mon_x: i64,
    mon_y: i64,
    mon_w: i64,
    mon_h: i64,
}

static CACHE: OnceLock<Mutex<Option<DetectionCache>>> = OnceLock::new();
static LATEST: OnceLock<Mutex<Option<Surface>>> = OnceLock::new();
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FOREGROUND_PRIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn start() {
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if hypr_runtime().is_some() {
        crate::install::log_line("game detection: Hyprland IPC");
        let _ = std::thread::Builder::new()
            .name("warmup-game-detect".into())
            .spawn(hypr_loop);
        return;
    }
    match wlr::spawn() {
        Ok(()) => crate::install::log_line(
            "game detection: zwlr_foreign_toplevel_management_v1",
        ),
        Err(error) => {
            crate::install::log_line(&format!(
                "game detection: this compositor does not expose zwlr_foreign_toplevel_management_v1 ({error}). KWin does not implement that protocol, so Plasma has no fullscreen-game signal or per-app profiles from it."
            ));
        }
    }
}

pub fn foreground_identity() -> Option<(String, String)> {
    foreground_window().map(|(exe, class_name, _)| (exe, class_name))
}

pub fn foreground_window() -> Option<(String, String, String)> {
    start();
    if latest_surface().is_none()
        && !FOREGROUND_PRIMED.swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        if let Some((command, _)) = hypr_runtime() {
            if let Ok(surface) = hypr_surface(&command) {
                publish(Some(surface));
            }
        }
    }
    let surface = latest_surface()?;
    if surface.exe.is_empty() && surface.class_name.is_empty() {
        None
    } else {
        Some((surface.exe, surface.class_name, surface.title))
    }
}

pub fn standalone_game_active_cached() -> bool {
    let now = Instant::now();
    let mut slot = match cache().lock() {
        Ok(slot) => slot,
        Err(_) => return surface_active(),
    };
    if let Some(cached) = *slot {
        if now.duration_since(cached.checked_at) < DETECT_INTERVAL {
            return cached.active;
        }
    }
    let active = surface_active();
    *slot = Some(DetectionCache {
        checked_at: now,
        active,
    });
    active
}

fn cache() -> &'static Mutex<Option<DetectionCache>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn latest() -> &'static Mutex<Option<Surface>> {
    LATEST.get_or_init(|| Mutex::new(None))
}

fn latest_surface() -> Option<Surface> {
    latest().lock().ok().and_then(|slot| slot.clone())
}

fn surface_active() -> bool {
    latest()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .is_some_and(|surface| surface_is_standalone_game(&surface))
}

fn publish(surface: Option<Surface>) {
    if let Ok(mut slot) = latest().lock() {
        *slot = surface;
    }
}

fn surface_is_standalone_game(surface: &Surface) -> bool {
    if surface.pid != 0 && surface.pid == surface.our_pid {
        return false;
    }
    if is_denied(&surface.exe) || is_denied(&surface.class_name) {
        return false;
    }
    if surface.fullscreen {
        return true;
    }
    if !surface.floating {
        return false;
    }
    rect_is_external_fullscreen(
        surface.x,
        surface.y,
        surface.w,
        surface.h,
        surface.mon_x,
        surface.mon_y,
        surface.mon_w,
        surface.mon_h,
    )
}

fn is_denied(name: &str) -> bool {
    let base = basename(name);
    if base.is_empty() {
        return false;
    }
    EXTERNAL_GAME_EXE_DENYLIST.iter().any(|entry| {
        let stem = deny_stem(entry);
        base == stem
            || base
                .split(|ch: char| ch == '.' || ch == ' ')
                .any(|part| part == stem)
    })
}

fn basename(name: &str) -> String {
    let trimmed = name.trim();
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).trim().to_string()
}

fn deny_stem(entry: &str) -> String {
    basename(entry)
}

fn rect_is_external_fullscreen(
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    mon_x: i64,
    mon_y: i64,
    mon_w: i64,
    mon_h: i64,
) -> bool {
    if w <= 0 || h <= 0 || mon_w <= 0 || mon_h <= 0 {
        return false;
    }
    if w * FULLSCREEN_PERCENT_BASE >= mon_w * EXTERNAL_FULLSCREEN_AXIS_PERCENT
        && h * FULLSCREEN_PERCENT_BASE >= mon_h * EXTERNAL_FULLSCREEN_AXIS_PERCENT
    {
        return true;
    }
    let left = x.max(mon_x);
    let top = y.max(mon_y);
    let right = (x + w).min(mon_x + mon_w);
    let bottom = (y + h).min(mon_y + mon_h);
    if right <= left || bottom <= top {
        return false;
    }
    let overlap = (right - left) * (bottom - top);
    let mon_area = mon_w * mon_h;
    overlap * FULLSCREEN_PERCENT_BASE >= mon_area * EXTERNAL_FULLSCREEN_OVERLAP_PERCENT
}

fn hypr_runtime() -> Option<(PathBuf, PathBuf)> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    if sig.is_empty() || runtime.is_empty() {
        return None;
    }
    let dir = PathBuf::from(runtime).join("hypr").join(sig);
    let command = dir.join(".socket.sock");
    let events = dir.join(".socket2.sock");
    if command.exists() && events.exists() {
        Some((command, events))
    } else {
        None
    }
}

fn hypr_loop() {
    loop {
        let Some((command, events)) = hypr_runtime() else {
            publish(None);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        match hypr_surface(&command) {
            Ok(surface) => publish(Some(surface)),
            Err(_) => publish(None),
        }
        match UnixStream::connect(&events) {
            Ok(stream) => read_hypr_events(stream, &command),
            Err(error) => {
                crate::install::log_line(&format!("game detection: hyprland events: {error}"));
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

fn read_hypr_events(mut stream: UnixStream, command: &std::path::Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut pending = String::new();
    let mut buf = [0u8; 2048];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(index) = pending.find('\n') {
                    let line: String = pending.drain(..=index).collect();
                    if line.starts_with("fullscreen>>") || line.starts_with("activewindow>>") {
                        match hypr_surface(command) {
                            Ok(surface) => publish(Some(surface)),
                            Err(error) => {
                                publish(None);
                                crate::install::log_line(&format!(
                                    "game detection: hyprland activewindow: {error}"
                                ));
                            }
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return,
        }
    }
}

fn hypr_surface(command: &std::path::Path) -> Result<Surface, String> {
    let window = hypr_json(command, "j/activewindow")?;
    let monitors = hypr_json(command, "j/monitors")?;
    let monitor_id = window
        .get("monitor")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let monitor = monitors
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("id").and_then(|id| id.as_i64()) == Some(monitor_id))
        })
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let pid = window.get("pid").and_then(|value| value.as_u64()).unwrap_or(0) as u32;
    let class_name = json_string(&window, "class");
    let title = json_string(&window, "title");
    let exe = exe_for_pid(pid).unwrap_or_else(|| class_name.clone());
    let (x, y) = json_pair(&window, "at");
    let (w, h) = json_pair(&window, "size");
    Ok(Surface {
        exe,
        class_name,
        title,
        pid,
        our_pid: std::process::id(),
        fullscreen: window
            .get("fullscreen")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            & 2
            != 0,
        floating: window
            .get("floating")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        x,
        y,
        w,
        h,
        mon_x: monitor.get("x").and_then(|value| value.as_i64()).unwrap_or(0),
        mon_y: monitor.get("y").and_then(|value| value.as_i64()).unwrap_or(0),
        mon_w: monitor
            .get("width")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        mon_h: monitor
            .get("height")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
    })
}

pub fn active_window_geometry() -> Option<(i64, i64, i64, i64)> {
    let (command, _) = hypr_runtime()?;
    let window = hypr_json(&command, "j/activewindow").ok()?;
    let (x, y) = json_pair(&window, "at");
    let (w, h) = json_pair(&window, "size");
    (w > 0 && h > 0).then_some((x, y, w, h))
}

pub fn active_workspace_windows() -> Option<i64> {
    let (command, _) = hypr_runtime()?;
    let workspace = hypr_json(&command, "j/activeworkspace").ok()?;
    workspace.get("windows").and_then(|value| value.as_i64())
}

pub fn focused_output_logical_size() -> Option<(f32, f32)> {
    let (command, _) = hypr_runtime()?;
    let monitors = hypr_json(&command, "j/monitors").ok()?;
    let rows = monitors.as_array()?;
    let monitor = rows
        .iter()
        .find(|row| row.get("focused").and_then(|value| value.as_bool()) == Some(true))
        .or_else(|| rows.first())?;
    let width = monitor.get("width")?.as_f64()?;
    let height = monitor.get("height")?.as_f64()?;
    let scale = monitor
        .get("scale")
        .and_then(|value| value.as_f64())
        .filter(|scale| *scale > 0.0)
        .unwrap_or(1.0);
    let rotated = monitor
        .get("transform")
        .and_then(|value| value.as_i64())
        .is_some_and(|transform| transform % 2 == 1);
    let (width, height) = if rotated { (height, width) } else { (width, height) };
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(((width / scale) as f32, (height / scale) as f32))
}

fn hypr_json(sock: &std::path::Path, cmd: &str) -> Result<serde_json::Value, String> {
    let mut stream = UnixStream::connect(sock)
        .map_err(|error| format!("connect {}: {error}", sock.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(400)))
        .ok();
    stream
        .write_all(cmd.as_bytes())
        .map_err(|error| format!("write {cmd}: {error}"))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                break;
            }
            Err(error) => return Err(format!("read {cmd}: {error}")),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let text = text.trim();
    serde_json::from_str(text).map_err(|error| format!("parse {cmd}: {error}"))
}

pub fn hypr_dispatch(lua: &str) -> bool {
    hypr_dispatch_reply(lua).is_some_and(|reply| hypr_reply_ok(&reply))
}

fn hypr_dispatch_reply(command_text: &str) -> Option<String> {
    let (command, _) = hypr_runtime()?;
    let mut stream = UnixStream::connect(&command).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let cmd = format!("dispatch {command_text}");
    stream.write_all(cmd.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                break;
            }
            Err(_) => return None,
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

static HYPR_LUA: Mutex<Option<(std::ffi::OsString, bool)>> = Mutex::new(None);

pub fn hypr_run(lua: &str, legacy: &str) -> bool {
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default();
    let cached = HYPR_LUA
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().filter(|(key, _)| *key == instance).map(|(_, lua)| *lua));
    let remember = |uses_lua: bool| {
        if let Ok(mut slot) = HYPR_LUA.lock() {
            *slot = Some((instance.clone(), uses_lua));
        }
    };
    let (ok, mode) = if cached == Some(false) {
        (hypr_dispatch(legacy), "classic")
    } else {
        match hypr_dispatch_reply(lua) {
            None => (false, "lua"),
            Some(reply) if reply.trim().is_empty() => (true, "lua, no reply"),
            Some(reply) if hypr_reply_ok(&reply) => {
                remember(true);
                (true, "lua")
            }
            Some(_) if cached.is_none() => {
                let ok = hypr_dispatch(legacy);
                if ok {
                    remember(false);
                }
                (ok, "classic after lua was rejected")
            }
            Some(_) => (false, "lua"),
        }
    };
    if std::env::var_os("WARMUP_DEBUG_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        crate::install::log_line(&format!("hyprland profile dispatch: {legacy} ({mode}) ok={ok}"));
    }
    ok
}

fn hypr_reply_ok(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("expected") {
        return false;
    }
    text.lines().any(|line| line.trim() == "ok")
}

fn json_string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string()
}

fn json_pair(value: &serde_json::Value, key: &str) -> (i64, i64) {
    let Some(pair) = value.get(key).and_then(|item| item.as_array()) else {
        return (0, 0);
    };
    (
        pair.first().and_then(|item| item.as_i64()).unwrap_or(0),
        pair.get(1).and_then(|item| item.as_i64()).unwrap_or(0),
    )
}

fn exe_for_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let link = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(link.to_string_lossy().into_owned())
}

mod wlr {
    use std::collections::HashMap;

    use wayland_client::protocol::wl_output;
    use wayland_client::{
        Connection, Dispatch, Proxy, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
    };
    use wayland_protocols_wlr::foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
    };

    use super::{Surface, publish};

    struct Toplevel {
        app_id: String,
        title: String,
        activated: bool,
        fullscreen: bool,
    }

    struct State {
        tops: HashMap<u32, Toplevel>,
        outputs: HashMap<u32, (i32, i32)>,
    }

    pub fn spawn() -> Result<(), String> {
        let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|error| error.to_string())?;
        let qh = queue.handle();
        let manager = globals
            .bind::<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, _, _>(
                &qh,
                1..=3,
                (),
            )
            .map_err(|_| {
                "global zwlr_foreign_toplevel_manager_v1 is not advertised".to_string()
            })?;
        std::thread::Builder::new()
            .name("warmup-wlr-toplevel".into())
            .spawn(move || {
                let mut data = State {
                    tops: HashMap::new(),
                    outputs: HashMap::new(),
                };
                let _keep = manager;
                loop {
                    if queue.blocking_dispatch(&mut data).is_err() {
                        break;
                    }
                    publish_focused(&data);
                }
            })
            .map_err(|error| format!("spawn: {error}"))?;
        Ok(())
    }

    fn publish_focused(state: &State) {
        let Some(top) = state.tops.values().find(|top| top.activated) else {
            publish(None);
            return;
        };
        publish(Some(Surface {
            exe: top.app_id.clone(),
            class_name: top.app_id.clone(),
            title: top.title.clone(),
            pid: 0,
            our_pid: std::process::id(),
            fullscreen: top.fullscreen,
            floating: false,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            mon_x: 0,
            mon_y: 0,
            mon_w: 0,
            mon_h: 0,
        }));
    }

    impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for State {
        fn event(
            state: &mut Self,
            _proxy: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
            event: zwlr_foreign_toplevel_manager_v1::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
                state.tops.insert(
                    toplevel.id().protocol_id(),
                    Toplevel {
                        app_id: String::new(),
                        title: String::new(),
                        activated: false,
                        fullscreen: false,
                    },
                );
            }
        }

        wayland_client::event_created_child!(State, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, [
            zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()),
        ]);
    }

    impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for State {
        fn event(
            state: &mut Self,
            proxy: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
            event: zwlr_foreign_toplevel_handle_v1::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            let id = proxy.id().protocol_id();
            let Some(top) = state.tops.get_mut(&id) else {
                return;
            };
            match event {
                zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                    top.app_id = app_id;
                }
                zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                    top.title = title;
                }
                zwlr_foreign_toplevel_handle_v1::Event::State { state: bits } => {
                    let fullscreen = bits.chunks_exact(4).any(|chunk| {
                        u32::from_ne_bytes(chunk.try_into().unwrap_or([0; 4])) == 3
                    });
                    let activated = bits.chunks_exact(4).any(|chunk| {
                        u32::from_ne_bytes(chunk.try_into().unwrap_or([0; 4])) == 2
                    });
                    top.fullscreen = fullscreen;
                    top.activated = activated;
                }
                zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                    state.tops.remove(&id);
                }
                zwlr_foreign_toplevel_handle_v1::Event::Done => {
                    publish_focused(state);
                }
                _ => {}
            }
        }
    }

    impl Dispatch<wl_output::WlOutput, ()> for State {
        fn event(
            state: &mut Self,
            proxy: &wl_output::WlOutput,
            event: wl_output::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let wl_output::Event::Mode { width, height, .. } = event {
                state
                    .outputs
                    .insert(proxy.id().protocol_id(), (width, height));
            }
        }
    }

    impl Dispatch<wayland_client::protocol::wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(
            _state: &mut Self,
            _proxy: &wayland_client::protocol::wl_registry::WlRegistry,
            _event: wayland_client::protocol::wl_registry::Event,
            _data: &GlobalListContents,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(fullscreen: bool, floating: bool, w: i64, h: i64) -> Surface {
        Surface {
            exe: "game".into(),
            class_name: "game".into(),
            title: String::new(),
            pid: 50,
            our_pid: 1,
            fullscreen,
            floating,
            x: 0,
            y: 0,
            w,
            h,
            mon_x: 0,
            mon_y: 0,
            mon_w: 1920,
            mon_h: 1080,
        }
    }

    #[test]
    fn fullscreen_flag_is_a_game_unless_denied() {
        assert!(surface_is_standalone_game(&surface(true, false, 100, 100)));
        let mut denied = surface(true, false, 1920, 1080);
        denied.exe = "/usr/bin/steam".into();
        assert!(!surface_is_standalone_game(&denied));
    }

    #[test]
    fn tiled_window_is_not_a_game_even_when_large() {
        assert!(!surface_is_standalone_game(&surface(false, false, 1920, 1080)));
    }

    #[test]
    fn floating_cover_uses_the_rect_heuristic() {
        assert!(surface_is_standalone_game(&surface(false, true, 1920, 1080)));
        assert!(surface_is_standalone_game(&surface(false, true, 1920, 1000)));
        assert!(!surface_is_standalone_game(&surface(false, true, 800, 600)));
    }

    #[test]
    fn denylist_matches_linux_basenames() {
        assert!(is_denied("chrome"));
        assert!(is_denied("/usr/bin/steam"));
        assert!(is_denied("com.google.Chrome"));
        assert!(is_denied("warmup"));
        assert!(!is_denied("warmup-companion"));
        assert!(!is_denied("Celeste"));
    }
}

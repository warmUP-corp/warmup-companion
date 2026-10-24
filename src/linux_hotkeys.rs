use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REGISTRY: &str = "org.freedesktop.host.portal.Registry";
const APP_ID: &str = "warmup-companion";
const UNIT_NAME: &str = "warmup-companion.service";
const SYNC_EVERY: Duration = Duration::from_secs(1);
const BIND_CHECK_TICKS: u32 = 3;
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const SIGNAL_POLL: Duration = Duration::from_millis(100);
const LABEL_PREFIX: &str = "warmUP Companion: ";

#[derive(Clone, Copy, PartialEq)]
enum Hotkey {
    Center,
    Voice,
}

impl Hotkey {
    const ALL: [Hotkey; 2] = [Hotkey::Center, Hotkey::Voice];

    fn id(self) -> &'static str {
        match self {
            Hotkey::Center => "control-center",
            Hotkey::Voice => "voice-typing",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Hotkey::Center => "Open Control Center",
            Hotkey::Voice => "Start or stop voice typing",
        }
    }

    fn portal_trigger(self) -> &'static str {
        match self {
            Hotkey::Center => "CTRL+ALT+c",
            Hotkey::Voice => "CTRL+ALT+v",
        }
    }

    fn hypr_key(self) -> &'static str {
        match self {
            Hotkey::Center => "C",
            Hotkey::Voice => "V",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Hotkey::Center => "Ctrl+Alt+C",
            Hotkey::Voice => "Ctrl+Alt+V",
        }
    }

    fn signal(self) -> &'static str {
        match self {
            Hotkey::Center => "SIGUSR1",
            Hotkey::Voice => "SIGUSR2",
        }
    }

    fn from_id(id: &str) -> Option<Hotkey> {
        Hotkey::ALL.into_iter().find(|h| h.id() == id)
    }

    fn run(self) {
        match self {
            Hotkey::Center => {
                crate::win::controller_center::toggle();
            }
            Hotkey::Voice => {
                if crate::config::voice_enabled() {
                    crate::vk_nav::start_voice_input();
                }
            }
        }
    }
}

#[derive(Clone)]
enum Target {
    Global(String),
    Signal(&'static str),
}

static STARTED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static CENTER_ACCEL: Mutex<Option<String>> = Mutex::new(None);
static CENTER_SIGNAL: AtomicBool = AtomicBool::new(false);
static VOICE_SIGNAL: AtomicBool = AtomicBool::new(false);

pub(crate) fn center_accel() -> Option<String> {
    CENTER_ACCEL.lock().ok().and_then(|a| a.clone())
}

fn set_center_accel(label: Option<String>) {
    if let Ok(mut a) = CENTER_ACCEL.lock() {
        *a = label;
    }
    crate::linux_tray::refresh();
}

pub(crate) fn spawn() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !service_main() && service_active() {
        log("the service holds the global shortcuts");
        STARTED.store(false, Ordering::SeqCst);
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-hotkeys".into())
        .spawn(run);
}

pub(crate) fn release_on_exit_signals() {
    let mut mask = nix::sys::signal::SigSet::empty();
    mask.add(nix::sys::signal::Signal::SIGTERM);
    mask.add(nix::sys::signal::Signal::SIGINT);
    if let Err(e) = mask.thread_block() {
        log(&format!("could not block stop signals: {e}"));
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-exit-signals".into())
        .spawn(move || {
            if mask.wait().is_ok() {
                release();
                std::process::exit(0);
            }
        });
}

pub(crate) fn release() {
    if !STARTED.load(Ordering::SeqCst) || !hyprland() {
        return;
    }
    for hotkey in Hotkey::ALL {
        hypr_unbind(hotkey);
    }
}

fn run() {
    let signals = hyprland() && service_main();
    if signals {
        install_signal_handlers();
        let _ = std::thread::Builder::new()
            .name("warmup-hotkey-signals".into())
            .spawn(poll_signals);
    }
    let mut synced = false;
    let mut first = true;
    let mut backoff = RECONNECT_MIN;
    loop {
        match Portal::open() {
            Ok(portal) => {
                first = false;
                backoff = RECONNECT_MIN;
                let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(trigger) = portal.triggers.get(Hotkey::Center.id()) {
                    set_center_accel(Some(trigger.clone()));
                }
                if hyprland() && !synced {
                    let names = global_names();
                    for hotkey in Hotkey::ALL {
                        if portal.triggers.contains_key(hotkey.id()) {
                            continue;
                        }
                        if let Some(name) = names.get(hotkey.id()) {
                            spawn_hypr_sync(hotkey, Target::Global(name.clone()));
                            synced = true;
                        } else if signals {
                            spawn_hypr_sync(hotkey, Target::Signal(hotkey.signal()));
                            synced = true;
                        }
                    }
                }
                let portal = Arc::new(portal);
                for (name, watch) in [
                    ("warmup-hotkeys-activated", listen as fn(&Portal, u64)),
                    ("warmup-hotkeys-changed", follow_changes as fn(&Portal, u64)),
                ] {
                    let portal = portal.clone();
                    let _ = std::thread::Builder::new()
                        .name(name.into())
                        .spawn(move || watch(&portal, generation));
                }
                wait_for_portal_restart(&portal.conn);
                log("the global shortcuts portal went away, reconnecting");
                std::thread::sleep(backoff);
            }
            Err(e) if first => {
                log(&format!("global shortcuts portal unavailable: {e}"));
                if signals {
                    for hotkey in Hotkey::ALL {
                        spawn_hypr_sync(hotkey, Target::Signal(hotkey.signal()));
                    }
                }
                return;
            }
            Err(e) => {
                log(&format!("global shortcuts portal: {e}; retrying in {}s", backoff.as_secs()));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(RECONNECT_MAX);
            }
        }
    }
}

fn wait_for_portal_restart(conn: &Connection) {
    let changes = zbus::blocking::fdo::DBusProxy::new(conn)
        .and_then(|dbus| dbus.receive_name_owner_changed());
    let Ok(changes) = changes else {
        loop {
            std::thread::park();
        }
    };
    for change in changes {
        if change.args().is_ok_and(|args| args.name().as_str() == DEST) {
            return;
        }
    }
}

fn current(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}

fn listen(portal: &Portal, generation: u64) {
    let Ok(proxy) = Proxy::new(&portal.conn, DEST, PORTAL_PATH, IFACE) else {
        return;
    };
    let Ok(signals) = proxy.receive_signal("Activated") else {
        return;
    };
    for message in signals {
        if !current(generation) {
            return;
        }
        let Ok((session, id, _, _)) = message
            .body()
            .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
        else {
            continue;
        };
        if session != portal.session {
            continue;
        }
        if let Some(hotkey) = Hotkey::from_id(&id) {
            hotkey.run();
        }
    }
}

fn follow_changes(portal: &Portal, generation: u64) {
    let Ok(proxy) = Proxy::new(&portal.conn, DEST, PORTAL_PATH, IFACE) else {
        return;
    };
    let Ok(signals) = proxy.receive_signal("ShortcutsChanged") else {
        return;
    };
    for message in signals {
        if !current(generation) {
            return;
        }
        let Ok((session, shortcuts)) = message.body().deserialize::<(
            OwnedObjectPath,
            Vec<(String, HashMap<String, OwnedValue>)>,
        )>() else {
            continue;
        };
        if session != portal.session || hyprland() {
            continue;
        }
        let trigger = shortcuts
            .iter()
            .find(|(id, _)| id == Hotkey::Center.id())
            .and_then(|(_, props)| props.get("trigger_description"))
            .and_then(|v| String::try_from(v.clone()).ok())
            .filter(|t| !t.is_empty());
        log(&format!("Control Center shortcut is now {trigger:?}"));
        set_center_accel(trigger);
    }
}

struct Portal {
    conn: Connection,
    session: OwnedObjectPath,
    triggers: HashMap<String, String>,
}

impl Portal {
    fn open() -> Result<Portal, String> {
        ensure_desktop_entry();
        let conn = Connection::session().map_err(err)?;
        let registry = Proxy::new(&conn, DEST, PORTAL_PATH, REGISTRY).map_err(err)?;
        let none: HashMap<&str, Value> = HashMap::new();
        if let Err(e) = registry.call::<_, _, ()>("Register", &(APP_ID, none)) {
            log(&format!("portal registry: {e}"));
        }
        let proxy = Proxy::new(&conn, DEST, PORTAL_PATH, IFACE).map_err(err)?;
        let token = format!("warmup_session_{}", std::process::id());
        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("handle_token", Value::from(token.as_str()));
        options.insert("session_handle_token", Value::from(token.as_str()));
        let results = request(&conn, &token, || {
            proxy
                .call::<_, _, OwnedObjectPath>("CreateSession", &(options,))
                .map(|_| ())
        })?;
        let session = results
            .get("session_handle")
            .and_then(|v| String::try_from(v.clone()).ok())
            .ok_or("portal returned no session")?;
        let session = OwnedObjectPath::try_from(session).map_err(err)?;

        let shortcuts: Vec<(&str, HashMap<&str, Value>)> = Hotkey::ALL
            .iter()
            .map(|h| {
                let mut props: HashMap<&str, Value> = HashMap::new();
                props.insert("description", Value::from(h.description()));
                props.insert("preferred_trigger", Value::from(h.portal_trigger()));
                (h.id(), props)
            })
            .collect();
        let token = format!("warmup_bind_{}", std::process::id());
        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("handle_token", Value::from(token.as_str()));
        let session_path: ObjectPath = session.as_ref();
        let results = request(&conn, &token, || {
            proxy
                .call::<_, _, OwnedObjectPath>(
                    "BindShortcuts",
                    &(session_path.clone(), shortcuts, "", options),
                )
                .map(|_| ())
        })?;
        let mut triggers = HashMap::new();
        if let Some(bound) = results.get("shortcuts") {
            let bound: Vec<(String, HashMap<String, OwnedValue>)> =
                bound.try_clone().map_err(err)?.try_into().map_err(err)?;
            for (id, props) in bound {
                let trigger = props
                    .get("trigger_description")
                    .and_then(|v| String::try_from(v.clone()).ok())
                    .unwrap_or_default();
                if !trigger.is_empty() {
                    triggers.insert(id, trigger);
                }
            }
        }
        log(&format!("global shortcuts bound: {triggers:?}"));
        Ok(Portal {
            conn,
            session,
            triggers,
        })
    }
}

fn request(
    conn: &Connection,
    token: &str,
    call: impl FnOnce() -> zbus::Result<()>,
) -> Result<HashMap<String, OwnedValue>, String> {
    let sender = conn
        .unique_name()
        .ok_or("no bus name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let request = Proxy::new(conn, DEST, path.as_str(), "org.freedesktop.portal.Request")
        .map_err(err)?;
    let mut responses = request.receive_signal("Response").map_err(err)?;
    call().map_err(err)?;
    let message = responses.next().ok_or("portal closed the request")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) =
        message.body().deserialize().map_err(err)?;
    if code != 0 {
        return Err(format!("portal answered {code}"));
    }
    Ok(results)
}

fn ensure_desktop_entry() {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("applications");
    let path = dir.join(format!("{APP_ID}.desktop"));
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| APP_ID.to_string());
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=warmUP Companion\nExec={exe}\nIcon=input-gaming\nNoDisplay=true\n"
    );
    if std::fs::read_to_string(&path).is_ok_and(|current| current == entry) {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, entry)) {
        log(&format!("{}: {e}", path.display()));
        return;
    }
    std::thread::sleep(Duration::from_millis(500));
}

fn service_main() -> bool {
    Command::new("systemctl")
        .args(["--user", "show", "-p", "MainPID", "--value", UNIT_NAME])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
        == Some(std::process::id())
}

fn service_active() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", UNIT_NAME])
        .status()
        .is_ok_and(|s| s.success())
}

fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

fn hyprctl(args: &[&str]) -> Option<String> {
    let output = Command::new("hyprctl").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn global_names() -> HashMap<&'static str, String> {
    let mut names = HashMap::new();
    let listing = hyprctl(&["globalshortcuts"]).unwrap_or_default();
    for line in listing.lines() {
        let Some((name, description)) = line.split_once(" -> ") else {
            continue;
        };
        for hotkey in Hotkey::ALL {
            if name.ends_with(&format!(":{}", hotkey.id()))
                && description.trim() == hotkey.description()
            {
                names.insert(hotkey.id(), name.trim().to_string());
            }
        }
    }
    names
}

struct ComboBinds {
    ours: bool,
    others: Vec<serde_json::Value>,
}

fn combo_binds(hotkey: Hotkey) -> Option<ComboBinds> {
    let json = hyprctl(&["binds", "-j"])?;
    let serde_json::Value::Array(binds) = serde_json::from_str::<serde_json::Value>(&json).ok()?
    else {
        return None;
    };
    let mut combo = ComboBinds {
        ours: false,
        others: Vec::new(),
    };
    for bind in binds {
        let on_combo = bind["modmask"].as_u64() == Some(12)
            && bind["submap"].as_str().unwrap_or_default().is_empty()
            && bind["key"]
                .as_str()
                .is_some_and(|k| k.eq_ignore_ascii_case(hotkey.hypr_key()));
        if !on_combo {
            continue;
        }
        if bind["description"]
            .as_str()
            .unwrap_or_default()
            .starts_with(LABEL_PREFIX)
        {
            combo.ours = true;
        } else {
            combo.others.push(bind);
        }
    }
    Some(combo)
}

fn bound_by_other(hotkey: Hotkey) -> bool {
    combo_binds(hotkey).is_some_and(|c| !c.others.is_empty())
}

fn legacy_bind_line(hotkey: Hotkey, bind: &serde_json::Value) -> Option<(String, String)> {
    let dispatcher = bind["dispatcher"].as_str()?;
    if dispatcher == "__lua" {
        return None;
    }
    let flag = |key: &str, letter: char| bind[key].as_bool().unwrap_or(false).then_some(letter);
    let mut flags: String = [
        flag("locked", 'l'),
        flag("release", 'r'),
        flag("repeat", 'e'),
        flag("non_consuming", 'n'),
        flag("mouse", 'm'),
    ]
    .into_iter()
    .flatten()
    .collect();
    let arg = bind["arg"].as_str().unwrap_or_default();
    let line = match bind["description"].as_str().filter(|d| !d.is_empty()) {
        Some(description) => {
            flags.push('d');
            format!("CTRL ALT, {}, {description}, {dispatcher}, {arg}", hotkey.hypr_key())
        }
        None => format!("CTRL ALT, {}, {dispatcher}, {arg}", hotkey.hypr_key()),
    };
    Some((format!("bind{flags}"), line))
}

fn hypr_bind(hotkey: Hotkey, target: &Target) -> bool {
    hypr_unbind(hotkey);
    if bound_by_other(hotkey) {
        log(&format!("{} is already bound in Hyprland", hotkey.label()));
        return false;
    }
    let description = format!("{LABEL_PREFIX}{}", hotkey.description());
    let (dispatcher, arg) = match target {
        Target::Global(name) => ("global", name.clone()),
        Target::Signal(sig) => (
            "exec",
            format!("systemctl --user kill --kill-whom=main -s {sig} {UNIT_NAME}"),
        ),
    };
    let legacy = format!(
        "CTRL ALT, {}, {description}, {dispatcher}, {arg}",
        hotkey.hypr_key()
    );
    match hyprctl(&["keyword", "bindd", &legacy]) {
        Some(out) if out.contains("non-legacy") => {
            let lua_dispatcher = match dispatcher {
                "global" => format!("hl.dsp.global({})", lua_string(&arg)),
                _ => format!("hl.dsp.exec_cmd({})", lua_string(&arg)),
            };
            let lua = format!(
                "hl.bind(\"CTRL + ALT + {}\", {lua_dispatcher}, {{ description = {} }})",
                hotkey.hypr_key(),
                lua_string(&description)
            );
            hyprctl(&["eval", &lua]).is_some()
        }
        Some(out) => out.is_empty() || out == "ok",
        None => false,
    }
}

fn hypr_unbind(hotkey: Hotkey) {
    let Some(combo) = combo_binds(hotkey) else {
        return;
    };
    if !combo.ours {
        return;
    }
    let restore: Option<Vec<(String, String)>> = combo
        .others
        .iter()
        .map(|bind| legacy_bind_line(hotkey, bind))
        .collect();
    let Some(restore) = restore else {
        log(&format!(
            "left {} bound: another bind shares the combo and cannot be restored",
            hotkey.label()
        ));
        return;
    };
    let legacy = format!("CTRL ALT, {}", hotkey.hypr_key());
    match hyprctl(&["keyword", "unbind", &legacy]) {
        Some(out) if out.contains("non-legacy") => {
            let lua = format!("hl.unbind(\"CTRL + ALT + {}\")", hotkey.hypr_key());
            let _ = hyprctl(&["eval", &lua]);
        }
        Some(_) => {
            for (keyword, line) in restore {
                let _ = hyprctl(&["keyword", &keyword, &line]);
            }
        }
        None => {}
    }
}

fn lua_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn wanted(hotkey: Hotkey) -> bool {
    match hotkey {
        Hotkey::Center => true,
        Hotkey::Voice => crate::config::voice_enabled(),
    }
}

fn spawn_hypr_sync(hotkey: Hotkey, target: Target) {
    let _ = std::thread::Builder::new()
        .name(format!("warmup-hotkey-{}", hotkey.id()))
        .spawn(move || {
            let mut bound: Option<bool> = None;
            let mut ok = false;
            let mut tick: u32 = 0;
            loop {
                let want = wanted(hotkey);
                let lost = want
                    && ok
                    && tick % BIND_CHECK_TICKS == 0
                    && combo_binds(hotkey).is_some_and(|c| !c.ours);
                if lost {
                    log(&format!("{} bind is gone, binding it again", hotkey.label()));
                }
                tick = tick.wrapping_add(1);
                if bound != Some(want) || lost {
                    ok = if want {
                        hypr_bind(hotkey, &target)
                    } else {
                        hypr_unbind(hotkey);
                        false
                    };
                    if hotkey == Hotkey::Center {
                        set_center_accel(ok.then(|| hotkey.label().to_string()));
                    }
                    bound = Some(want);
                }
                std::thread::sleep(SYNC_EVERY);
            }
        });
}

extern "C" fn on_signal(sig: libc::c_int) {
    if sig == libc::SIGUSR1 {
        CENTER_SIGNAL.store(true, Ordering::SeqCst);
    } else if sig == libc::SIGUSR2 {
        VOICE_SIGNAL.store(true, Ordering::SeqCst);
    }
}

fn install_signal_handlers() {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGUSR1, handler);
        libc::signal(libc::SIGUSR2, handler);
    }
}

fn poll_signals() {
    loop {
        if CENTER_SIGNAL.swap(false, Ordering::SeqCst) {
            Hotkey::Center.run();
        }
        if VOICE_SIGNAL.swap(false, Ordering::SeqCst) {
            Hotkey::Voice.run();
        }
        std::thread::sleep(SIGNAL_POLL);
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn log(msg: &str) {
    crate::install::log_line(&format!("hotkeys: {msg}"));
}

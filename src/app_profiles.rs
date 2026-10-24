use enigo::Key;

pub const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
];

pub const BUTTONS: [&str; 14] = [
    "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "SELECT", "START", "LEFT", "RIGHT", "UP", "DOWN",
];

pub struct Builtin {
    pub name: &'static str,
    pub exes: &'static [&'static str],
    pub map: &'static [(&'static str, &'static str)],
    pub presets: &'static [(&'static str, &'static str)],
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "Web browsers",
        exes: BROWSERS,
        map: &[
            ("B", "ctrl+w"),
            ("Y", "ctrl+t"),
            ("RT", "ctrl+tab"),
            ("LT", "ctrl+shift+tab"),
        ],
        presets: &[
            ("Close tab", "ctrl+w"),
            ("New tab", "ctrl+t"),
            ("Reopen closed tab", "ctrl+shift+t"),
            ("Next tab", "ctrl+tab"),
            ("Previous tab", "ctrl+shift+tab"),
            ("Back", "alt+left"),
            ("Forward", "alt+right"),
            ("Reload", "f5"),
            ("Address bar", "ctrl+l"),
            ("Full screen", "f11"),
        ],
    },
    Builtin {
        name: "VLC",
        exes: &["vlc.exe"],
        map: &[
            ("X", "space"),
            ("Y", "f"),
            ("LEFT", "alt+left"),
            ("RIGHT", "alt+right"),
            ("UP", "ctrl+up"),
            ("DOWN", "ctrl+down"),
            ("LT", "p"),
            ("RT", "n"),
        ],
        presets: &[
            ("Play / pause", "space"),
            ("Full screen", "f"),
            ("Back 10 s", "alt+left"),
            ("Forward 10 s", "alt+right"),
            ("Volume up", "ctrl+up"),
            ("Volume down", "ctrl+down"),
            ("Previous", "p"),
            ("Next", "n"),
            ("Mute", "m"),
            ("Subtitles", "v"),
        ],
    },
    Builtin {
        name: "Spotify",
        exes: &["spotify.exe"],
        map: &[
            ("X", "space"),
            ("LEFT", "ctrl+left"),
            ("RIGHT", "ctrl+right"),
            ("UP", "ctrl+up"),
            ("DOWN", "ctrl+down"),
        ],
        presets: &[
            ("Play / pause", "space"),
            ("Previous track", "ctrl+left"),
            ("Next track", "ctrl+right"),
            ("Volume up", "ctrl+up"),
            ("Volume down", "ctrl+down"),
        ],
    },
    Builtin {
        name: "File Explorer",
        exes: &["explorer.exe"],
        map: &[
            ("LEFT", "alt+left"),
            ("RIGHT", "alt+right"),
            ("UP", "alt+up"),
            ("RT", "ctrl+tab"),
            ("LT", "ctrl+shift+tab"),
        ],
        presets: &[
            ("Back", "alt+left"),
            ("Forward", "alt+right"),
            ("Parent folder", "alt+up"),
            ("Next tab", "ctrl+tab"),
            ("Previous tab", "ctrl+shift+tab"),
            ("New tab", "ctrl+t"),
            ("Close tab", "ctrl+w"),
        ],
    },
    Builtin {
        name: "Discord",
        exes: &["discord.exe"],
        map: &[
            ("UP", "alt+up"),
            ("DOWN", "alt+down"),
            ("Y", "ctrl+k"),
            ("X", "ctrl+shift+m"),
        ],
        presets: &[
            ("Previous channel", "alt+up"),
            ("Next channel", "alt+down"),
            ("Quick switcher", "ctrl+k"),
            ("Mute mic", "ctrl+shift+m"),
            ("Deafen", "ctrl+shift+d"),
        ],
    },
];

pub const APP_PRESETS: &[(&str, &str)] = &[
    ("Copy", "ctrl+c"),
    ("Paste", "ctrl+v"),
    ("Undo", "ctrl+z"),
    ("Save", "ctrl+s"),
    ("Escape", "esc"),
    ("Enter", "enter"),
];

pub const MODIFIERS: [&str; 4] = ["ctrl", "shift", "alt", "win"];

pub const KEYS: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "tab",
    "enter",
    "esc",
    "space",
    "backspace",
    "left",
    "right",
    "up",
    "down",
    "pageup",
    "pagedown",
    "home",
    "end",
    "f5",
    "f11",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub exes: Vec<String>,
    pub browsers: bool,
    pub enabled: bool,
    pub map: [Option<String>; 14],
}

impl Profile {
    pub fn name(&self) -> String {
        if let Some(b) = builtin_for(&self.exes[0]) {
            return b.name.into();
        }
        let exe = &self.exes[0];
        let stem = exe
            .strip_suffix(".exe")
            .or_else(|| exe.strip_suffix(".EXE"))
            .unwrap_or(exe);
        let mut chars = stem.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default()
    }

    pub fn builtin_map(&self) -> [Option<String>; 14] {
        BUTTONS.map(|b| builtin(&self.exes[0], b).map(str::to_string))
    }

    pub fn mapped(&self) -> usize {
        self.map.iter().filter(|m| m.is_some()).count()
    }
}

pub fn builtin_profiles() -> Vec<Profile> {
    BUILTINS
        .iter()
        .map(|b| {
            let mut p = Profile {
                exes: b.exes.iter().map(|s| s.to_string()).collect(),
                browsers: b.exes == BROWSERS,
                enabled: true,
                map: Default::default(),
            };
            p.map = p.builtin_map();
            p
        })
        .collect()
}

pub fn new_profile(exe: &str) -> Profile {
    Profile {
        exes: vec![exe.to_string()],
        browsers: false,
        enabled: true,
        map: Default::default(),
    }
}

fn builtin_for(exe: &str) -> Option<&'static Builtin> {
    BUILTINS
        .iter()
        .find(|b| b.exes.iter().any(|e| e.eq_ignore_ascii_case(exe)))
}

pub fn has_builtin(exe: &str) -> bool {
    builtin_for(exe).is_some()
}

pub fn presets_for(exe: &str) -> &'static [(&'static str, &'static str)] {
    builtin_for(exe).map_or(APP_PRESETS, |b| b.presets)
}

fn builtin(exe: &str, button: &str) -> Option<&'static str> {
    builtin_for(exe)?
        .map
        .iter()
        .find(|(b, _)| b.eq_ignore_ascii_case(button))
        .map(|(_, c)| *c)
}

fn setting<'a>(settings: &'a str, key: &str) -> Option<&'a str> {
    settings.lines().rev().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (!line.trim_start().starts_with('#') && k.trim().eq_ignore_ascii_case(key))
            .then(|| v.trim())
    })
}

fn is_off(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "off" | "false" | "0" | "no"
    )
}

#[cfg(any(windows, test))]
pub fn chord_for(settings: &str, exe: &str, button: &str) -> Option<Vec<Key>> {
    if !BUTTONS.iter().any(|b| b.eq_ignore_ascii_case(button))
        || setting(settings, "app_profiles").is_some_and(is_off)
        || setting(settings, &format!("profile.{exe}")).is_some_and(is_off)
    {
        return None;
    }
    let spec = setting(settings, &format!("profile.{exe}.{button}")).or(builtin(exe, button))?;
    parse_chord(spec)
}

#[cfg(windows)]
pub fn chord_for_foreground(button: &str) -> Option<Vec<Key>> {
    let settings = crate::config::settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    chord_for(&settings, &crate::vk_nav::foreground_exe(), button)
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
pub fn profile_candidates(exe: &str, class_name: &str) -> Vec<String> {
    let mut ids = Vec::new();
    push_profile_id(&mut ids, exe);
    push_profile_id(&mut ids, class_name);
    let aliases: Vec<&str> = ids.iter().filter_map(|id| linux_alias(id)).collect();
    for alias in aliases {
        push_profile_id(&mut ids, alias);
    }
    ids
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
fn push_profile_id(ids: &mut Vec<String>, raw: &str) {
    let trimmed = raw.trim();
    let base = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if base.is_empty() || ids.iter().any(|id| id.eq_ignore_ascii_case(base)) {
        return;
    }
    ids.push(base.to_string());
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
fn linux_alias(name: &str) -> Option<&'static str> {
    let trimmed = name.trim();
    let base = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    let stem = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    Some(match stem.to_ascii_lowercase().as_str() {
        "firefox" | "firefox-bin" | "firefox-esr" => "firefox.exe",
        "google-chrome"
        | "google-chrome-stable"
        | "google-chrome-beta"
        | "google-chrome-unstable"
        | "chromium"
        | "chromium-browser"
        | "chrome" => "chrome.exe",
        "microsoft-edge"
        | "microsoft-edge-stable"
        | "microsoft-edge-beta"
        | "microsoft-edge-dev"
        | "msedge" => "msedge.exe",
        "brave" | "brave-browser" => "brave.exe",
        "opera" | "opera-stable" => "opera.exe",
        "vivaldi" | "vivaldi-bin" | "vivaldi-stable" => "vivaldi.exe",
        "vlc" | "org.videolan.vlc" => "vlc.exe",
        "spotify" | "com.spotify.client" => "spotify.exe",
        "dolphin" | "org.kde.dolphin" | "thunar" | "nautilus" | "org.gnome.nautilus" | "nemo" => {
            "explorer.exe"
        }
        "discord" | "com.discordapp.discord" | "vesktop" | "dev.vencord.vesktop" | "legcord" => {
            "discord.exe"
        }
        _ => return None,
    })
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileAction {
    Keys(Vec<Key>),
    Hypr(HyprCommand),
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyprCommand {
    pub lua: String,
    pub legacy: String,
}

#[cfg(target_os = "linux")]
pub fn hypr_command(spec: &str) -> Option<HyprCommand> {
    let rest = spec.trim().strip_prefix("hypr:")?.trim();
    let (dispatcher, arg) = match rest.split_once(char::is_whitespace) {
        Some((d, a)) => (d, a.trim()),
        None => (rest, ""),
    };
    if !arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '+' | '-' | '_'))
    {
        return None;
    }
    let lua = match (dispatcher.to_ascii_lowercase().as_str(), arg) {
        ("workspace", a) if !a.is_empty() => format!(r#"hl.dsp.focus({{ workspace = "{a}" }})"#),
        ("movetoworkspace", a) if !a.is_empty() => {
            format!(r#"hl.dsp.window.move({{ workspace = "{a}", follow = true }})"#)
        }
        ("togglespecialworkspace", "") => "hl.dsp.workspace.toggle_special()".to_string(),
        ("togglespecialworkspace", a) => format!(r#"hl.dsp.workspace.toggle_special("{a}")"#),
        ("fullscreen", "" | "0") => {
            r#"hl.dsp.window.fullscreen({ mode = "fullscreen", action = "toggle" })"#.to_string()
        }
        ("fullscreen", "1") => {
            r#"hl.dsp.window.fullscreen({ mode = "maximized", action = "toggle" })"#.to_string()
        }
        ("togglefloating", "") => r#"hl.dsp.window.float({ action = "toggle" })"#.to_string(),
        ("killactive", "") => "hl.dsp.window.close()".to_string(),
        _ => return None,
    };
    let legacy = if arg.is_empty() {
        dispatcher.to_ascii_lowercase()
    } else {
        format!("{} {arg}", dispatcher.to_ascii_lowercase())
    };
    Some(HyprCommand { lua, legacy })
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
fn action_for(settings: &str, exe: &str, button: &str) -> Option<ProfileAction> {
    if !BUTTONS.iter().any(|b| b.eq_ignore_ascii_case(button))
        || setting(settings, "app_profiles").is_some_and(is_off)
        || setting(settings, &format!("profile.{exe}")).is_some_and(is_off)
    {
        return None;
    }
    let spec = setting(settings, &format!("profile.{exe}.{button}")).or(builtin(exe, button))?;
    match hypr_command(spec) {
        Some(command) => Some(ProfileAction::Hypr(command)),
        None => parse_chord(spec).map(ProfileAction::Keys),
    }
}

#[cfg(all(target_os = "linux", feature = "gamepad"))]
pub fn chord_for_foreground(button: &str) -> Option<ProfileAction> {
    let settings = crate::config::settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let (exe, class_name) = crate::linux_game_detect::foreground_identity()?;
    for id in profile_candidates(&exe, &class_name) {
        if setting(&settings, &format!("profile.{id}.{button}"))
            .is_some_and(|value| value.eq_ignore_ascii_case("none"))
        {
            return None;
        }
        if setting(&settings, &format!("profile.{id}")).is_some_and(is_off) {
            return None;
        }
        if let Some(action) = action_for(&settings, &id, button) {
            return Some(action);
        }
    }
    None
}

fn split_key(key: &str) -> Option<(&str, Option<&str>)> {
    let rest = key.trim().strip_prefix("profile.")?;
    match rest.rsplit_once('.') {
        Some((exe, button)) if BUTTONS.iter().any(|b| b.eq_ignore_ascii_case(button)) => {
            Some((exe, Some(button)))
        }
        _ => Some((rest, None)),
    }
}

pub fn load(settings: &str) -> (bool, Vec<Profile>) {
    let on = !setting(settings, "app_profiles").is_some_and(is_off);
    let mut exes: Vec<&str> = Vec::new();
    for line in settings
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
    {
        let Some((exe, _)) = line.split_once('=').and_then(|(k, _)| split_key(k)) else {
            continue;
        };
        if !exe.is_empty() && !has_builtin(exe) && !exes.iter().any(|e| e.eq_ignore_ascii_case(exe))
        {
            exes.push(exe);
        }
    }
    let read = |p: &mut Profile| {
        let exe = p.exes[0].clone();
        p.enabled = !setting(settings, &format!("profile.{exe}")).is_some_and(is_off);
        for (i, b) in BUTTONS.iter().enumerate() {
            if let Some(v) = setting(settings, &format!("profile.{exe}.{b}")) {
                p.map[i] = (!v.eq_ignore_ascii_case("none") && parse_chord(v).is_some())
                    .then(|| v.to_ascii_lowercase());
            }
        }
    };
    let mut profiles = builtin_profiles();
    profiles.iter_mut().for_each(read);
    for exe in exes {
        let mut p = new_profile(exe);
        read(&mut p);
        profiles.push(p);
    }
    (on, profiles)
}

pub fn settings_lines(on: bool, profiles: &[Profile]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !on {
        out.push(("app_profiles".to_string(), "off".to_string()));
    }
    for p in profiles {
        for exe in &p.exes {
            if !has_builtin(exe) || !p.enabled {
                out.push((
                    format!("profile.{exe}"),
                    if p.enabled { "on" } else { "off" }.to_string(),
                ));
            }
            for (i, b) in BUTTONS.iter().enumerate() {
                let key = format!("profile.{exe}.{}", b.to_ascii_lowercase());
                match (&p.map[i], builtin(exe, b)) {
                    (Some(c), Some(d)) if c.eq_ignore_ascii_case(d) => {}
                    (None, None) => {}
                    (Some(c), _) => out.push((key, c.clone())),
                    (None, Some(_)) => out.push((key, "none".to_string())),
                }
            }
        }
    }
    out
}

pub fn valid_setting(key: &str, value: &str) -> bool {
    let Some((exe, button)) = split_key(key) else {
        return false;
    };
    let value = value.trim();
    match button {
        _ if exe.is_empty() => false,
        None => matches!(
            value.to_ascii_lowercase().as_str(),
            "on" | "off" | "true" | "false" | "1" | "0" | "yes" | "no"
        ),
        Some(_) => value.eq_ignore_ascii_case("none") || parse_chord(value).is_some(),
    }
}

pub fn key_label(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => "Ctrl".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "backspace" => "Bksp".into(),
        n if n.len() == 1 => n.to_ascii_uppercase(),
        n => {
            let mut c = n.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

pub fn keycaps(spec: &str) -> Vec<String> {
    spec.split('+').map(key_label).collect()
}

fn parse_chord(spec: &str) -> Option<Vec<Key>> {
    if spec.trim().get(..5).is_some_and(|p| p.eq_ignore_ascii_case("hypr:")) {
        return None;
    }
    spec.split('+').map(parse_key).collect()
}

fn parse_key(name: &str) -> Option<Key> {
    let name = name.trim().to_ascii_lowercase();
    Some(match name.as_str() {
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" => Key::Alt,
        "win" => Key::Meta,
        "tab" => Key::Tab,
        "enter" => Key::Return,
        "esc" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "f5" => Key::F5,
        "f11" => Key::F11,
        _ => match name.as_bytes() {
            [c] if c.is_ascii_alphanumeric() => Key::Other(c.to_ascii_uppercase() as u32),
            _ => return None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_uses_builtin_profile() {
        assert_eq!(
            chord_for("", "Chrome.exe", "B"),
            Some(vec![Key::Control, Key::Other(b'W' as u32)])
        );
        assert_eq!(chord_for("", "chrome.exe", "A"), None);
    }

    #[test]
    fn unknown_app_falls_back_to_default() {
        assert_eq!(chord_for("", "notepad.exe", "B"), None);
    }

    #[test]
    fn user_config_overrides_and_adds_profiles() {
        let settings = "profile.chrome.exe.b = none\nprofile.notepad.exe.y = ctrl+s\n";
        assert_eq!(chord_for(settings, "chrome.exe", "B"), None);
        assert_eq!(
            chord_for(settings, "notepad.exe", "Y"),
            Some(vec![Key::Control, Key::Other(b'S' as u32)])
        );
    }
}

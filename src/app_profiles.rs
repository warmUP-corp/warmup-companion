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

const BROWSER_PROFILE: &[(&str, &str)] = &[
    ("B", "ctrl+w"),
    ("Y", "ctrl+t"),
    ("RT", "ctrl+tab"),
    ("LT", "ctrl+shift+tab"),
];

pub const BROWSER_PRESETS: &[(&str, &str)] = &[
    ("Close tab", "ctrl+w"),
    ("New tab", "ctrl+t"),
    ("Reopen closed tab", "ctrl+shift+t"),
    ("Next tab", "ctrl+tab"),
    ("Previous tab", "ctrl+shift+tab"),
    ("Back", "alt+left"),
    ("Forward", "alt+right"),
    ("Reload", "f5"),
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
        if self.browsers {
            return "Web browsers".into();
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

pub fn browser_profile() -> Profile {
    let mut p = Profile {
        exes: BROWSERS.iter().map(|s| s.to_string()).collect(),
        browsers: true,
        enabled: true,
        map: Default::default(),
    };
    p.map = p.builtin_map();
    p
}

pub fn new_profile(exe: &str) -> Profile {
    Profile {
        exes: vec![exe.to_string()],
        browsers: false,
        enabled: true,
        map: Default::default(),
    }
}

fn is_browser(exe: &str) -> bool {
    BROWSERS.iter().any(|b| b.eq_ignore_ascii_case(exe))
}

fn builtin(exe: &str, button: &str) -> Option<&'static str> {
    if !is_browser(exe) {
        return None;
    }
    BROWSER_PROFILE
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
        if !exe.is_empty() && !is_browser(exe) && !exes.iter().any(|e| e.eq_ignore_ascii_case(exe))
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
    let mut profiles = vec![browser_profile()];
    read(&mut profiles[0]);
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
            if !p.browsers || !p.enabled {
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

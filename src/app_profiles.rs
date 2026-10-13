use enigo::Key;

const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
];

const BROWSER_PROFILE: &[(&str, &str)] = &[
    ("B", "ctrl+w"),
    ("Y", "ctrl+t"),
    ("RT", "ctrl+tab"),
    ("LT", "ctrl+shift+tab"),
];

pub fn chord_for(settings: &str, exe: &str, button: &str) -> Option<Vec<Key>> {
    let key = format!("profile.{exe}.{button}");
    let user = settings.lines().rev().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (!line.trim_start().starts_with('#') && k.trim().eq_ignore_ascii_case(&key))
            .then(|| v.trim())
    });
    let spec = match user {
        Some(spec) => spec,
        None if BROWSERS.iter().any(|b| b.eq_ignore_ascii_case(exe)) => {
            BROWSER_PROFILE
                .iter()
                .find(|(b, _)| b.eq_ignore_ascii_case(button))?
                .1
        }
        None => return None,
    };
    parse_chord(spec)
}

#[cfg(windows)]
pub fn chord_for_foreground(button: &str) -> Option<Vec<Key>> {
    let settings = crate::config::settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    chord_for(&settings, &crate::vk_nav::foreground_exe(), button)
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

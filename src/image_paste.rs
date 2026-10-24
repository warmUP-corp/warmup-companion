use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LAST_IMAGE: Mutex<Option<(PathBuf, u32)>> = Mutex::new(None);

const TERMINAL_EXES: &[&str] = &[
    "wezterm-gui.exe",
    "windowsterminal.exe",
    "openconsole.exe",
    "alacritty.exe",
    "mintty.exe",
    "kitty.exe",
    "hyper.exe",
    "tabby.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
];

const TERMINAL_CLASSES: &[&str] = &[
    "org.wezfurlong.wezterm",
    "cascadia_hosting_window_class",
    "consolewindowclass",
];

#[cfg(target_os = "linux")]
const LINUX_TERMINALS: &[&str] = &[
    "alacritty",
    "kitty",
    "foot",
    "footclient",
    "org.wezfurlong.wezterm",
    "wezterm-gui",
    "com.mitchellh.ghostty",
    "ghostty",
    "konsole",
    "org.kde.konsole",
    "gnome-terminal",
    "gnome-terminal-server",
    "org.gnome.terminal",
    "xterm",
];

#[cfg(windows)]
pub fn is_terminal(exe: &str, class: &str) -> bool {
    TERMINAL_EXES.iter().any(|t| exe.eq_ignore_ascii_case(t))
        || TERMINAL_CLASSES.iter().any(|t| class.eq_ignore_ascii_case(t))
}

#[cfg(target_os = "linux")]
pub fn is_terminal(exe: &str, class: &str) -> bool {
    TERMINAL_EXES.iter().any(|t| exe.eq_ignore_ascii_case(t))
        || TERMINAL_CLASSES.iter().any(|t| class.eq_ignore_ascii_case(t))
        || LINUX_TERMINALS
            .iter()
            .any(|t| exe.eq_ignore_ascii_case(t) || class.eq_ignore_ascii_case(t))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClipState {
    pub text: bool,
    pub image: bool,
    pub files: bool,
    pub seq: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PasteAction {
    CtrlV,
    TypePath(PathBuf),
    SaveAndTypePath,
    TypeFiles,
}

pub fn decide(terminal: bool, clip: &ClipState, last: Option<&(PathBuf, u32)>) -> PasteAction {
    if !terminal || clip.text {
        return PasteAction::CtrlV;
    }
    if clip.files {
        return PasteAction::TypeFiles;
    }
    if !clip.image {
        return PasteAction::CtrlV;
    }
    match last {
        Some((path, seq)) if *seq == clip.seq => PasteAction::TypePath(path.clone()),
        _ => PasteAction::SaveAndTypePath,
    }
}

pub fn quote_paths<P: AsRef<Path>>(paths: &[P]) -> String {
    let mut out = String::new();
    for p in paths {
        out.push('"');
        out.push_str(&p.as_ref().to_string_lossy());
        out.push_str("\" ");
    }
    out
}

pub fn remember_image(path: PathBuf, seq: u32) {
    if let Ok(mut last) = LAST_IMAGE.lock() {
        *last = Some((path, seq));
    }
}

pub fn remember_screenshot(path: PathBuf, seq: u32) {
    remember_image(path, seq);
}

#[cfg(windows)]
pub fn terminal_paste_text(exe: &str) -> Option<String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    };
    const CF_BITMAP: u32 = 2;
    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const CF_DIBV5: u32 = 17;

    let avail = |f: u32| unsafe { IsClipboardFormatAvailable(f) }.is_ok();
    let clip = ClipState {
        text: avail(CF_UNICODETEXT),
        image: avail(CF_DIB) || avail(CF_DIBV5) || avail(CF_BITMAP),
        files: avail(CF_HDROP),
        seq: unsafe { GetClipboardSequenceNumber() },
    };
    let last = LAST_IMAGE.lock().ok().and_then(|l| l.clone());
    let paths = match decide(true, &clip, last.as_ref()) {
        PasteAction::CtrlV => return None,
        PasteAction::TypePath(path) => vec![path],
        PasteAction::TypeFiles => {
            if !open_clipboard() {
                crate::install::log_line("paste: OpenClipboard failed, falling back to Ctrl+V");
                return None;
            }
            let files = clipboard_files();
            unsafe {
                let _ = CloseClipboard();
            }
            if files.is_empty() {
                crate::install::log_line("paste: CF_HDROP had no paths, falling back to Ctrl+V");
                return None;
            }
            files
        }
        PasteAction::SaveAndTypePath => {
            if !open_clipboard() {
                crate::install::log_line("paste: OpenClipboard failed, falling back to Ctrl+V");
                return None;
            }
            let image = clipboard_image_rgba();
            unsafe {
                let _ = CloseClipboard();
            }
            let Some((w, h, rgba)) = image else {
                crate::install::log_line("paste: could not read clipboard image, falling back to Ctrl+V");
                return None;
            };
            let Some(path) = save_png(&timestamp_stem("Clipboard", ""), w, h, &rgba, "clipboard image") else {
                crate::install::log_line("paste: saving clipboard image failed, falling back to Ctrl+V");
                return None;
            };
            remember_image(path.clone(), clip.seq);
            vec![path]
        }
    };
    let text = quote_paths(&paths);
    crate::install::log_line(&format!("paste: image path into {exe} -> {}", text.trim_end()));
    Some(text)
}

#[cfg(windows)]
fn open_clipboard() -> bool {
    use windows::Win32::System::DataExchange::OpenClipboard;
    for _ in 0..10 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    false
}

#[cfg(windows)]
fn clipboard_files() -> Vec<PathBuf> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    const CF_HDROP: u32 = 15;
    let mut out = Vec::new();
    unsafe {
        let Ok(handle) = GetClipboardData(CF_HDROP) else {
            return out;
        };
        let hdrop = HDROP(handle.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; len + 1];
            let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            out.push(PathBuf::from(String::from_utf16_lossy(&buf[..n.min(len)])));
        }
    }
    out
}

#[cfg(windows)]
fn clipboard_image_rgba() -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::Graphics::Gdi::{GetDC, GetObjectW, ReleaseDC, BITMAP, HBITMAP};
    use windows::Win32::System::DataExchange::GetClipboardData;
    const CF_BITMAP: u32 = 2;
    unsafe {
        let handle = GetClipboardData(CF_BITMAP).ok()?;
        let bitmap = HBITMAP(handle.0);
        let mut info = BITMAP::default();
        let got = GetObjectW(
            bitmap,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut _ as *mut _),
        );
        if got == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
            return None;
        }
        let width = info.bmWidth;
        let height = info.bmHeight.abs();
        let dc = GetDC(None);
        if dc.is_invalid() {
            return None;
        }
        let rgba = bitmap_rgba(dc, bitmap, width, height);
        ReleaseDC(None, dc);
        rgba.map(|px| (width as u32, height as u32, px))
    }
}

#[cfg(windows)]
pub(crate) fn bitmap_rgba(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    width: i32,
    height: i32,
) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{
        GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0 as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bgra = vec![0u8; width as usize * height as usize * 4];
    let lines = unsafe {
        GetDIBits(
            hdc,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    if lines != height {
        crate::install::log_line("image save: GetDIBits failed");
        return None;
    }

    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Some(bgra)
}

#[cfg(windows)]
pub(crate) fn timestamp_stem(prefix: &str, suffix: &str) -> String {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{prefix} {:04}-{:02}-{:02} {:02}{:02}{:02}{suffix}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
    )
}

#[cfg(windows)]
pub(crate) fn save_png(stem: &str, width: u32, height: u32, rgba: &[u8], tag: &str) -> Option<PathBuf> {
    let (file, path) = create_unique(stem, "png", tag)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = match encoder.write_header() {
        Ok(w) => w,
        Err(e) => {
            crate::install::log_line(&format!("{tag} save: png header failed: {e}"));
            let _ = std::fs::remove_file(&path);
            return None;
        }
    };
    if let Err(e) = writer.write_image_data(rgba) {
        crate::install::log_line(&format!("{tag} save: png write failed: {e}"));
        drop(writer);
        let _ = std::fs::remove_file(&path);
        return None;
    }
    if let Err(e) = writer.finish() {
        crate::install::log_line(&format!("{tag} save: png finish failed: {e}"));
        let _ = std::fs::remove_file(&path);
        return None;
    }
    crate::install::log_line(&format!("{tag} saved: {}", path.display()));
    Some(path)
}

fn create_unique(stem: &str, ext: &str, tag: &str) -> Option<(std::fs::File, PathBuf)> {
    let Some(dir) = screenshots_dir() else {
        crate::install::log_line(&format!("{tag} save: could not resolve Screenshots folder"));
        return None;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::install::log_line(&format!("{tag} save: create_dir_all failed: {e}"));
        return None;
    }

    let mut opened = None;
    for n in 1..100 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => {
                opened = Some((f, path));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                crate::install::log_line(&format!("{tag} save: create file failed: {e}"));
                return None;
            }
        }
    }
    if opened.is_none() {
        crate::install::log_line(&format!("{tag} save: no free file name"));
    }
    opened
}

#[cfg(windows)]
fn screenshots_dir() -> Option<PathBuf> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::UI::Shell::{FOLDERID_Screenshots, SHGetKnownFolderPath, KF_FLAG_CREATE};

    unsafe {
        let mut token = HANDLE::default();
        let session_id = WTSGetActiveConsoleSessionId();
        let have_token = WTSQueryUserToken(session_id, &mut token).is_ok();

        let result = if have_token {
            SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, token)
        } else {
            SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None)
        };

        if have_token {
            let _ = CloseHandle(token);
        }

        match result {
            Ok(pwstr) => {
                let path = pwstr.to_string().ok().map(PathBuf::from);
                CoTaskMemFree(Some(pwstr.0.cast()));
                path
            }
            Err(e) => {
                crate::install::log_line(&format!("image save: SHGetKnownFolderPath failed: {e}"));
                None
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn timestamp_stem(prefix: &str, suffix: &str) -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{prefix} {:04}-{:02}-{:02} {:02}{:02}{:02}{suffix}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn save_bytes(stem: &str, ext: &str, bytes: &[u8], tag: &str) -> Option<PathBuf> {
    use std::io::Write;
    let (mut file, path) = create_unique(stem, ext, tag)?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.flush()) {
        crate::install::log_line(&format!("{tag} save: write failed: {e}"));
        drop(file);
        let _ = std::fs::remove_file(&path);
        return None;
    }
    crate::install::log_line(&format!("{tag} saved: {}", path.display()));
    Some(path)
}

#[cfg(target_os = "linux")]
fn screenshots_dir() -> Option<PathBuf> {
    pictures_dir().map(|dir| dir.join("Screenshots"))
}

#[cfg(target_os = "linux")]
fn pictures_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_PICTURES_DIR").map(PathBuf::from) {
        if dir.is_absolute() {
            return Some(dir);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    if let Ok(text) = std::fs::read_to_string(config.join("user-dirs.dirs")) {
        for line in text.lines() {
            let Some(value) = line.trim().strip_prefix("XDG_PICTURES_DIR=") else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            let dir = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(value),
            };
            if dir.is_absolute() && dir != home {
                return Some(dir);
            }
        }
    }
    Some(home.join("Pictures"))
}

#[cfg(target_os = "linux")]
pub(crate) fn clip_seq(bytes: &[u8]) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish() as u32
}

#[cfg(target_os = "linux")]
pub(crate) fn copy_to_clipboard(mime: &str, bytes: &[u8]) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let child = Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            crate::install::log_line(&format!("clipboard: wl-copy failed to start: {e}"));
            return false;
        }
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(bytes).is_ok());
    let ok = child.wait().is_ok_and(|status| status.success());
    if !(wrote && ok) {
        crate::install::log_line("clipboard: wl-copy failed");
    }
    wrote && ok
}

#[cfg(target_os = "linux")]
fn clipboard_types() -> Option<Vec<String>> {
    let out = std::process::Command::new("wl-paste")
        .arg("--list-types")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(Vec::new());
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect(),
    )
}

#[cfg(target_os = "linux")]
fn clipboard_bytes(mime: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("wl-paste")
        .args(["--no-newline", "--type", mime])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

#[cfg(target_os = "linux")]
fn uri_list_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .map(|rest| rest.find('/').map_or(rest, |i| &rest[i..]))
        .map(|path| PathBuf::from(percent_decode(path)))
        .collect()
}

#[cfg(target_os = "linux")]
pub fn terminal_paste_text(exe: &str) -> Option<String> {
    let Some(types) = clipboard_types() else {
        crate::install::log_line("paste: wl-paste is not installed, falling back to the paste shortcut");
        return None;
    };
    let has = |wanted: &str| types.iter().any(|t| t.eq_ignore_ascii_case(wanted));
    let files = has("text/uri-list");
    let text = !files
        && types.iter().any(|t| {
            t.starts_with("text/plain") || t == "UTF8_STRING" || t == "STRING" || t == "TEXT"
        });
    let image_type = types
        .iter()
        .find(|t| t.eq_ignore_ascii_case("image/png"))
        .or_else(|| types.iter().find(|t| t.starts_with("image/")))
        .cloned();
    let image_bytes = if !text && !files {
        image_type.as_deref().and_then(clipboard_bytes)
    } else {
        None
    };
    let clip = ClipState {
        text,
        image: image_bytes.is_some(),
        files,
        seq: image_bytes.as_deref().map(clip_seq).unwrap_or(0),
    };
    let last = LAST_IMAGE.lock().ok().and_then(|l| l.clone());
    let paths = match decide(true, &clip, last.as_ref()) {
        PasteAction::CtrlV => return None,
        PasteAction::TypePath(path) => vec![path],
        PasteAction::TypeFiles => {
            let files = clipboard_bytes("text/uri-list")
                .map(|bytes| uri_list_paths(&String::from_utf8_lossy(&bytes)))
                .unwrap_or_default();
            if files.is_empty() {
                crate::install::log_line("paste: text/uri-list had no paths, falling back to the paste shortcut");
                return None;
            }
            files
        }
        PasteAction::SaveAndTypePath => {
            let bytes = image_bytes?;
            let ext = image_type
                .as_deref()
                .and_then(|t| t.strip_prefix("image/"))
                .map(|ext| if ext.eq_ignore_ascii_case("jpeg") { "jpg" } else { ext })
                .unwrap_or("png")
                .to_ascii_lowercase();
            let Some(path) = save_bytes(&timestamp_stem("Clipboard", ""), &ext, &bytes, "clipboard image") else {
                crate::install::log_line("paste: saving clipboard image failed, falling back to the paste shortcut");
                return None;
            };
            remember_image(path.clone(), clip.seq);
            vec![path]
        }
    };
    let text = quote_paths(&paths);
    crate::install::log_line(&format!("paste: image path into {exe} -> {}", text.trim_end()));
    Some(text)
}

#[cfg(target_os = "linux")]
pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let value = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            if let Some(value) = value {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_are_recognised_case_insensitively() {
        assert!(is_terminal("wezterm-gui.exe", ""));
        assert!(is_terminal("WindowsTerminal.exe", ""));
        assert!(is_terminal("WINDOWSTERMINAL.EXE", ""));
        assert!(is_terminal("OpenConsole.exe", ""));
        assert!(is_terminal("alacritty.exe", ""));
        assert!(is_terminal("mintty.exe", ""));
        assert!(is_terminal("kitty.exe", ""));
        assert!(is_terminal("Hyper.exe", ""));
        assert!(is_terminal("Tabby.exe", ""));
        assert!(is_terminal("conhost.exe", ""));
        assert!(is_terminal("cmd.exe", ""));
        assert!(is_terminal("powershell.exe", ""));
        assert!(is_terminal("pwsh.exe", ""));
        assert!(is_terminal("", "ConsoleWindowClass"));
        assert!(is_terminal("", "org.wezfurlong.wezterm"));
        assert!(is_terminal("", "CASCADIA_HOSTING_WINDOW_CLASS"));
    }

    #[test]
    fn editors_and_other_apps_are_not_terminals() {
        assert!(!is_terminal("Code.exe", "Chrome_WidgetWin_1"));
        assert!(!is_terminal("Cursor.exe", "Chrome_WidgetWin_1"));
        assert!(!is_terminal("notepad.exe", "Notepad"));
        assert!(!is_terminal("explorer.exe", "CabinetWClass"));
        assert!(!is_terminal("", ""));
    }

    fn clip(text: bool, image: bool, files: bool, seq: u32) -> ClipState {
        ClipState { text, image, files, seq }
    }

    #[test]
    fn decide_picks_the_right_paste() {
        let shot = (PathBuf::from(r"C:\Users\me\Pictures\Screenshots\Screenshot 1.png"), 7);
        assert_eq!(decide(false, &clip(false, true, false, 7), Some(&shot)), PasteAction::CtrlV);
        assert_eq!(decide(true, &clip(true, true, true, 7), Some(&shot)), PasteAction::CtrlV);
        assert_eq!(decide(true, &clip(false, false, false, 7), Some(&shot)), PasteAction::CtrlV);
        assert_eq!(decide(true, &clip(false, true, true, 7), Some(&shot)), PasteAction::TypeFiles);
        assert_eq!(decide(true, &clip(false, false, true, 9), None), PasteAction::TypeFiles);
        assert_eq!(
            decide(true, &clip(false, true, false, 7), Some(&shot)),
            PasteAction::TypePath(shot.0.clone())
        );
        assert_eq!(
            decide(true, &clip(false, true, false, 8), Some(&shot)),
            PasteAction::SaveAndTypePath
        );
        assert_eq!(decide(true, &clip(false, true, false, 8), None), PasteAction::SaveAndTypePath);
    }

    #[test]
    fn paths_are_quoted_and_space_terminated() {
        assert_eq!(
            quote_paths(&[r"C:\Users\me\Pictures\Screenshots\Screenshot 2026-09-27 101010.png"]),
            "\"C:\\Users\\me\\Pictures\\Screenshots\\Screenshot 2026-09-27 101010.png\" "
        );
        assert_eq!(quote_paths(&[r"C:\a.txt", r"D:\b c.png"]), "\"C:\\a.txt\" \"D:\\b c.png\" ");
        assert_eq!(quote_paths::<&str>(&[]), "");
    }
}

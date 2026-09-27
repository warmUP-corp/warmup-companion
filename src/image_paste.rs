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

pub fn is_terminal(exe: &str, class: &str) -> bool {
    TERMINAL_EXES.iter().any(|t| exe.eq_ignore_ascii_case(t))
        || TERMINAL_CLASSES.iter().any(|t| class.eq_ignore_ascii_case(t))
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

pub fn remember_screenshot(path: PathBuf) {
    let seq = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
    remember_image(path, seq);
}

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

pub(crate) fn timestamp_stem(prefix: &str, suffix: &str) -> String {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{prefix} {:04}-{:02}-{:02} {:02}{:02}{:02}{suffix}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
    )
}

pub(crate) fn save_png(stem: &str, width: u32, height: u32, rgba: &[u8], tag: &str) -> Option<PathBuf> {
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
            format!("{stem}.png")
        } else {
            format!("{stem} ({n}).png")
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
    let Some((file, path)) = opened else {
        crate::install::log_line(&format!("{tag} save: no free file name"));
        return None;
    };
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
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

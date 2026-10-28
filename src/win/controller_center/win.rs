use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics,
    GetWindowRect, KillTimer, MessageBoxW, SendMessageW, SetClassLongPtrW, SetForegroundWindow,
    SetTimer, SetWindowPos, ShowWindow, GCLP_HICON, GCLP_HICONSM, HICON, HMENU, HTCAPTION,
    HTCLIENT, ICON_BIG, ICON_SMALL, IDOK, LR_DEFAULTCOLOR, MB_DEFBUTTON2, MB_ICONWARNING,
    MB_OKCANCEL, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WM_CLOSE,
    WM_CHAR, WM_DESTROY, WM_DPICHANGED, WM_KEYDOWN, WM_MOUSEWHEEL, WM_SYSKEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCHITTEST, WM_SETICON, WM_SETTINGCHANGE, WM_TIMER, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_POPUP,
};

use super::*;
use crate::win::desktop_window::{self, DesktopApp, DesktopWindowThread};

const PAD_X: f32 = 72.0;
const PAD_TOP: f32 = 48.0;
const PAD_BOTTOM: f32 = 96.0;
const SHADOW_DY: f32 = 24.0;
const SHADOW_SIGMA: f32 = 30.0;
const HERO_PNG: &[u8] = include_bytes!("../../../assets/controller-center/launcher-hero.png");

pub(super) const TYPE_PLACEHOLDER: &str = "Program name, for example notepad.exe";
pub(super) const TYPE_CAPTION: &str = "Opens the keyboard \u{00B7} for example notepad.exe";

const WINDOW_CLASS: PCWSTR = w!("WarmupControllerCenter");
const TIMER_ID: usize = 31;
const TIMER_MS: u32 = 33;
const EXE_ICON_PX: u32 = 48;

type Pixels = (u32, u32, Vec<u8>);

static APP_TARGETS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static EXE_ICONS: OnceLock<HashMap<String, Pixels>> = OnceLock::new();

thread_local! {
    static APP_ICONS: RefCell<HashMap<i32, HICON>> = RefCell::new(HashMap::new());
}

fn app_icon(px: i32) -> Option<HICON> {
    if let Some(icon) = APP_ICONS.with(|m| m.borrow().get(&px).copied()) {
        return Some(icon);
    }
    let (off, len) = ico_entry(APP_ICO, px as u32)?;
    let icon = unsafe {
        CreateIconFromResourceEx(
            &APP_ICO[off..off + len],
            true,
            0x0003_0000,
            px,
            px,
            LR_DEFAULTCOLOR,
        )
    }
    .ok()?;
    APP_ICONS.with(|m| m.borrow_mut().insert(px, icon));
    Some(icon)
}

unsafe fn set_window_icons(hwnd: HWND, scale: f32) {
    for (which, index, base) in [
        (ICON_BIG, GCLP_HICON, 32.0),
        (ICON_SMALL, GCLP_HICONSM, 16.0),
    ] {
        if let Some(icon) = app_icon((base * scale).round() as i32) {
            SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(which as usize),
                LPARAM(icon.0 as isize),
            );
            SetClassLongPtrW(hwnd, index, icon.0 as isize);
        }
    }
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(super) fn run_voice_download(id: &'static str) -> bool {
    use std::io::BufRead;
    use std::os::windows::process::CommandExt;
    let (script, args): (&str, &[&str]) = match id {
        "parakeet-redux" => ("Get-WarmupParakeet.ps1", &["-Variant", "redux"]),
        "parakeet" => ("Get-WarmupParakeet.ps1", &[]),
        _ => ("Get-WarmupSpeech.ps1", &["-Model", id]),
    };
    let dirs = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf())),
        std::env::var_os("ProgramW6432")
            .or_else(|| std::env::var_os("ProgramFiles"))
            .map(|p| std::path::PathBuf::from(p).join("WarmupCompanion")),
    ];
    let Some(script) = dirs
        .iter()
        .flatten()
        .map(|d| d.join(script))
        .find(|p| p.is_file())
        .or_else(|| dirs[0].as_ref().map(|d| d.join(script)))
    else {
        return false;
    };
    if !script.is_file() {
        crate::install::log_line(&format!(
            "control center: voice download: {} missing",
            script.display()
        ));
        return false;
    }
    let spawned = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            crate::install::log_line(&format!("control center: voice download: {e}"));
            return false;
        }
    };
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(pct) = line
                .split_once(": ")
                .and_then(|(_, rest)| rest.split_once('%'))
                .and_then(|(p, _)| p.trim().parse::<u8>().ok())
            {
                set_voice_dl(Some(VoiceDl::Running(id, pct.min(100))));
            }
        }
    }
    let ok = child.wait().is_ok_and(|st| st.success());
    if !ok {
        crate::install::log_line(&format!("control center: voice download {id} failed"));
    }
    ok && voice_installed(id)
}

fn local_stamp() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

fn documents_dir() -> Option<String> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Documents)
}

fn known_folder(id: &windows::core::GUID) -> Option<String> {
    folder_of(id, true)
}

fn folder_of(id: &windows::core::GUID, signed_in_user: bool) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let mut token = HANDLE::default();
        let have_token = signed_in_user
            && WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token).is_ok();
        let result = if have_token {
            SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, token)
        } else {
            SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)
        };
        if have_token {
            let _ = CloseHandle(token);
        }
        let pwstr = result.ok()?;
        let path = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0.cast()));
        path
    }
}

pub(super) unsafe fn pick_file(
    owner: HWND,
    save: bool,
    dirty: bool,
) -> Result<Option<std::path::PathBuf>, String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FileSaveDialog, IFileDialog, IShellItem, SHCreateItemFromParsingName,
        FOS_FILEMUSTEXIST, FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH,
    };
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let clsid = if save {
        &FileSaveDialog
    } else {
        &FileOpenDialog
    };
    let dialog: IFileDialog = CoCreateInstance(clsid, None, CLSCTX_INPROC_SERVER)
        .map_err(|e| format!("file dialog: {e}"))?;
    let filter = [COMDLG_FILTERSPEC {
        pszName: w!("Settings (*.ini)"),
        pszSpec: w!("*.ini"),
    }];
    let _ = dialog.SetFileTypes(&filter);
    let _ = dialog.SetDefaultExtension(w!("ini"));
    let title = match (save, dirty) {
        (true, true) => w!("Export settings (unsaved changes are not included)"),
        (true, false) => w!("Export settings"),
        _ => w!("Import settings"),
    };
    let _ = dialog.SetTitle(title);
    if save {
        let t = local_stamp();
        let name = HSTRING::from(export_file_name(t.wYear, t.wMonth, t.wDay));
        let _ = dialog.SetFileName(&name);
    }
    if let Ok(opts) = dialog.GetOptions() {
        let extra = if save {
            FOS_OVERWRITEPROMPT
        } else {
            FOS_FILEMUSTEXIST
        };
        let _ = dialog.SetOptions(opts | extra);
    }
    if let Some(dir) = documents_dir() {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(dir), None)
        {
            let _ = dialog.SetFolder(&item);
        }
    }
    OWNS_PAD.store(false, Ordering::SeqCst);
    let shown = dialog.Show(owner);
    OWNS_PAD.store(true, Ordering::SeqCst);
    if shown.is_err() {
        return Ok(None);
    }
    let item = dialog.GetResult().map_err(|e| format!("GetResult: {e}"))?;
    let path = item
        .GetDisplayName(SIGDN_FILESYSPATH)
        .map_err(|e| format!("GetDisplayName: {e}"))?;
    let out = path.to_string().ok().map(std::path::PathBuf::from);
    CoTaskMemFree(Some(path.0.cast()));
    Ok(out)
}

static THREAD: OnceLock<Mutex<Option<DesktopWindowThread>>> = OnceLock::new();

pub(super) fn hide() {
    OWNS_PAD.store(false, Ordering::SeqCst);
    if let Some(t) = THREAD
        .get()
        .and_then(|s| s.lock().ok())
        .as_ref()
        .and_then(|g| g.as_ref())
    {
        let _ = t.hide();
    }
}

struct CenterApp;

impl DesktopApp for CenterApp {
    const THREAD_NAME: &'static str = "warmup-controller-center";
    const CLASS_NAME: PCWSTR = WINDOW_CLASS;
    const BG_COLOR: u32 = 0x001E1C1C;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = wndproc;

    fn on_show(&mut self, _lparam: LPARAM) {
        ui_show();
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

pub fn show() {
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    apps::prewarm();
    if guard.is_none() {
        match desktop_window::spawn(CenterApp) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("control center: spawn failed: {e}"));
                return;
            }
        }
    }
    if let Some(t) = guard.as_ref() {
        let _ = take_pad_edges();
        OWNS_PAD.store(true, Ordering::SeqCst);
        if t.show(LPARAM(0)).is_err() {
            OWNS_PAD.store(false, Ordering::SeqCst);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn ui_show() {
    OWNS_PAD.store(true, Ordering::SeqCst);
    if let Some(hwnd) = UI.with(|u| u.borrow().as_ref().map(|u| u.hwnd)) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            OWNS_PAD.store(false, Ordering::SeqCst);
            return;
        };
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_APPWINDOW,
            WINDOW_CLASS,
            w!("Control Center"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            HMENU::default(),
            windows::Win32::Foundation::HINSTANCE(instance.0),
            None,
        ) else {
            crate::install::log_line("control center: CreateWindowExW failed");
            OWNS_PAD.store(false, Ordering::SeqCst);
            return;
        };
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let (w, h) = surface_px(scale);
        let card_w = (WIN_W * scale) as i32;
        let card_h = (WIN_H * scale) as i32;
        let x = (GetSystemMetrics(SM_CXSCREEN) - card_w) / 2 - (PAD_X * scale) as i32;
        let y = (GetSystemMetrics(SM_CYSCREEN) - card_h) / 2 - (PAD_TOP * scale) as i32;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        set_window_icons(hwnd, scale);
        let ui = new_ui(hwnd, scale);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        with_ui(|ui| {
            ui.led_base = led_values(ui);
            ui.led_previewed = ui.led_base;
            paint(ui);
        });
        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

pub(super) fn promo_art(scale: f32) -> Option<(i32, i32, Vec<u8>)> {
    if crate::tray::launcher_installed() {
        return None;
    }
    let (w, h) = (
        (TAB_W * scale).round() as u32,
        (PROMO_ART_H * scale).round() as u32,
    );
    cover_bgra(HERO_PNG, w, h, 12.0 * scale).map(|px| (w as i32, h as i32, px))
}

pub(super) unsafe fn confirm_signin_only(owner: HWND) -> bool {
    let title = wide("Sign-in screen only");
    let body = wide(
        "After you next sign in or unlock, the companion sleeps on the desktop: no controller input, \
         no tray icon and no Control Center.\r\n\r\n\
         It only wakes on the lock and sign-in screen. To change this setting later, use \
         warmUP > Settings > Controller > Companion (or Start menu > Warmup Companion).\r\n\r\n\
         Turn it on?",
    );
    MessageBoxW(
        owner,
        PCWSTR(body.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
    ) == IDOK
}

fn design_point(ui: &Ui, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x / ui.scale - PAD_X, y / ui.scale - PAD_TOP)
}

fn paint(ui: &mut Ui) {
    sync_led_preview(ui);
    let mut gfx = match ui.gfx.take() {
        Some(g) => g,
        None => match unsafe { Gfx::new() } {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("control center: gfx: {e}"));
                return;
            }
        },
    };
    if let Err(e) = unsafe { render(ui, &mut gfx) } {
        crate::install::log_line(&format!("control center: render: {e}"));
        ui.gfx = None;
        return;
    }
    ui.gfx = Some(gfx);
}

unsafe fn render(ui: &Ui, g: &mut Gfx) -> Result<(), String> {
    let s = ui.scale;
    g.begin(surface_px(s))?;
    g.draw_shadow(
        &[rect(
            PAD_X * s,
            (PAD_TOP + SHADOW_DY) * s,
            WIN_W * s,
            WIN_H * s,
        )],
        WIN_R * s,
        SHADOW_SIGMA * s,
    );
    g.transform(s, PAD_X * s, PAD_TOP * s);
    draw_window(ui, g);
    g.present(ui.hwnd, None)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let sx = (lparam.0 & 0xffff) as i16 as i32 - wr.left;
            let sy = ((lparam.0 >> 16) & 0xffff) as i16 as i32 - wr.top;
            let mut caption = false;
            with_ui(|ui| {
                let (x, y) =
                    design_point(ui, LPARAM(((sy as isize) << 16) | (sx as isize & 0xffff)));
                caption = hit_caption(x, y);
            });
            LRESULT(if caption { HTCAPTION } else { HTCLIENT } as isize)
        }
        WM_LBUTTONDOWN => {
            let mut after = After::Stay;
            with_ui(|ui| {
                let (x, y) = design_point(ui, lparam);
                after = click(ui, x, y);
                paint(ui);
            });
            finish(after);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
            with_ui(|ui| {
                if apps::wheel(ui, delta) {
                    paint(ui);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with_ui(|ui| {
                if let Some(i) = ui.drag {
                    let (x, _) = design_point(ui, lparam);
                    if let Some((_, r)) = rows(ui.tab, &ui.items, ui.led_open)
                        .into_iter()
                        .find(|(t, _)| *t == Target::Item(i))
                    {
                        if let Kind::Slider { min, max, .. } = ui.items[i].kind {
                            let v = slider_pos_at(r, min, max, x);
                            if v != ui.items[i].value {
                                ui.items[i].value = v;
                                paint(ui);
                            }
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_ui(|ui| {
                if ui.drag.take().is_some() {
                    let _ = ReleaseCapture();
                }
            });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let mut after = After::Stay;
            with_ui(|ui| {
                after = key_down(ui, wparam.0 as u16);
                paint(ui);
            });
            finish(after);
            LRESULT(0)
        }
        WM_SYSKEYDOWN => {
            let mut handled = false;
            with_ui(|ui| {
                if apps::records_keys(ui) {
                    let _ = key_down(ui, wparam.0 as u16);
                    paint(ui);
                    handled = true;
                }
            });
            if handled {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CHAR => {
            with_ui(|ui| {
                if apps::add_open(ui) {
                    if let Some(ch) = char::from_u32(wparam.0 as u32) {
                        apps::char_input(ui, ch);
                        paint(ui);
                    }
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            let mut after = After::Stay;
            with_ui(|ui| {
                let (dirty, a) = tick(ui);
                after = a;
                if dirty {
                    paint(ui);
                }
            });
            finish(after);
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            with_ui(|ui| {
                ui.theme = current_theme();
                paint(ui);
            });
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DPICHANGED => {
            let suggested = &*(lparam.0 as *const RECT);
            with_ui(|ui| {
                ui.scale = ((wparam.0 >> 16) & 0xffff).max(96) as f32 / 96.0;
                set_window_icons(hwnd, ui.scale);
                let (w, h) = surface_px(ui.scale);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Some(g) = ui.gfx.as_mut() {
                    g.icons.clear();
                }
                ui.promo = promo_art(ui.scale);
                paint(ui);
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            ui_hide();
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_ID);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub(super) fn center_items() -> Vec<Item> {
    let mut items = items();
    items.extend(voice_items());
    items
}

pub(super) fn close_window(ui: &Ui) {
    unsafe {
        let _ = KillTimer(ui.hwnd, TIMER_ID);
        let _ = DestroyWindow(ui.hwnd);
    }
}

pub(super) fn set_capture(hwnd: HWND) {
    unsafe {
        SetCapture(hwnd);
    }
}

pub(super) fn get_launcher() {
    crate::updater::download_launcher()
}

pub(super) unsafe fn draw_promo_art(g: &mut Gfx, w: i32, h: i32, px: &[u8], art: Rect) {
    if let Ok(bmp) = g.bitmap(w, h, px) {
        g.draw_bitmap(&bmp, art, 1.0, false);
    }
}

pub(super) fn program_name(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_string()
    } else {
        format!("{name}.exe")
    }
}

pub(super) fn installed_apps() -> Vec<(String, String)> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_CommonPrograms, FOLDERID_Profile, FOLDERID_Programs, IShellLinkW, ShellLink,
    };
    let own_profile = folder_of(&FOLDERID_Profile, false);
    let user_profile = known_folder(&FOLDERID_Profile);
    let mut links = Vec::new();
    let mut dirs: Vec<std::path::PathBuf> = [&FOLDERID_Programs, &FOLDERID_CommonPrograms]
        .into_iter()
        .filter_map(|id| known_folder(id).map(std::path::PathBuf::from))
        .collect();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
            {
                links.push(path);
            }
        }
    }
    let mut out: Vec<(String, String)> = Vec::new();
    let mut targets: Vec<(String, String)> = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for link in links {
            let Some(name) = link.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if ["uninstall", "readme", "help", "documentation", "website"]
                .iter()
                .any(|w| lower.contains(w))
            {
                continue;
            }
            let shell: IShellLinkW = match CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            {
                Ok(s) => s,
                Err(_) => continue,
            };
            let Ok(file) = shell.cast::<IPersistFile>() else {
                continue;
            };
            if file
                .Load(&windows::core::HSTRING::from(link.as_path()), STGM_READ)
                .is_err()
            {
                continue;
            }
            let mut buf = [0u16; 1024];
            if shell.GetPath(&mut buf, std::ptr::null_mut(), 0).is_err() {
                continue;
            }
            let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            let mut target = String::from_utf16_lossy(&buf[..len]);
            if let (Some(own), Some(user)) = (&own_profile, &user_profile) {
                if !own.eq_ignore_ascii_case(user)
                    && target.len() > own.len()
                    && target.is_char_boundary(own.len())
                    && target[..own.len()].eq_ignore_ascii_case(own)
                {
                    target = format!("{user}{}", &target[own.len()..]);
                }
            }
            if !std::path::Path::new(&target).is_file() {
                continue;
            }
            let Some(exe) = std::path::Path::new(&target)
                .file_name()
                .and_then(|s| s.to_str())
                .filter(|e| e.to_ascii_lowercase().ends_with(".exe"))
            else {
                continue;
            };
            if !out.iter().any(|(_, e)| e.eq_ignore_ascii_case(exe)) {
                out.push((name.to_string(), exe.to_string()));
                targets.push((exe.to_ascii_lowercase(), target.clone()));
            }
        }
    }
    if let Ok(mut t) = APP_TARGETS.lock() {
        *t = targets;
    }
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

pub(super) fn load_app_icons() {
    let targets = APP_TARGETS
        .lock()
        .map(|mut t| std::mem::take(&mut *t))
        .unwrap_or_default();
    let icons = targets
        .into_iter()
        .filter_map(|(exe, target)| unsafe { exe_icon(&target) }.map(|px| (exe, px)))
        .collect();
    let _ = EXE_ICONS.set(icons);
}

unsafe fn exe_icon(path: &str) -> Option<Pixels> {
    use windows::Win32::Foundation::S_OK;
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
    let mut icon = HICON::default();
    let hr = SHDefExtractIconW(
        &HSTRING::from(path),
        0,
        0,
        Some(&mut icon),
        None,
        EXE_ICON_PX,
    );
    if hr != S_OK || icon.is_invalid() {
        return None;
    }
    let px = icon_pixels(icon);
    let _ = DestroyIcon(icon);
    px
}

unsafe fn icon_pixels(icon: HICON) -> Option<Pixels> {
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};
    let mut info = ICONINFO::default();
    GetIconInfo(icon, &mut info).ok()?;
    let color = bitmap_bgra(info.hbmColor);
    let mask = match &color {
        Some((_, _, px)) if px.chunks_exact(4).all(|p| p[3] == 0) => bitmap_bgra(info.hbmMask),
        _ => None,
    };
    if !info.hbmColor.is_invalid() {
        let _ = DeleteObject(info.hbmColor);
    }
    if !info.hbmMask.is_invalid() {
        let _ = DeleteObject(info.hbmMask);
    }
    let (w, h, mut px) = color?;
    if let Some((mw, mh, m)) = mask {
        if mw != w || mh < h {
            return None;
        }
        for (p, k) in px.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
            p[3] = if k[..3] == [0, 0, 0] { 255 } else { 0 };
        }
    }
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        let (b, g, r) = (p[0] as u32, p[1] as u32, p[2] as u32);
        p[0] = (r * a / 255) as u8;
        p[1] = (g * a / 255) as u8;
        p[2] = (b * a / 255) as u8;
    }
    Some((w, h, px))
}

unsafe fn bitmap_bgra(bmp: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<Pixels> {
    use windows::Win32::Graphics::Gdi::{
        GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    if bmp.is_invalid() {
        return None;
    }
    let mut bm = BITMAP::default();
    let got = GetObjectW(
        bmp,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut BITMAP as *mut _),
    );
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if got == 0 || w <= 0 || h <= 0 {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut px = vec![0u8; w as usize * h as usize * 4];
    let dc = GetDC(None);
    if dc.is_invalid() {
        return None;
    }
    let lines = GetDIBits(
        dc,
        bmp,
        0,
        h as u32,
        Some(px.as_mut_ptr().cast()),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    ReleaseDC(None, dc);
    (lines == h).then_some((w as u32, h as u32, px))
}

pub(super) unsafe fn draw_app_icon(g: &mut Gfx, exe: &str, dst: Rect, alpha: f32) -> bool {
    let exe = exe.to_ascii_lowercase();
    let Some((w, h, px)) = EXE_ICONS.get().and_then(|m| m.get(&exe)) else {
        return false;
    };
    let key = format!("exe-icon:{exe}");
    if !g.images.contains_key(&key) {
        g.set_image(&key, *w, *h, px);
        if !g.images.contains_key(&key) {
            return false;
        }
    }
    g.draw_image(&key, dst, alpha, true);
    true
}

fn surface_px(scale: f32) -> (i32, i32) {
    (
        ((WIN_W + 2.0 * PAD_X) * scale).ceil() as i32,
        ((WIN_H + PAD_TOP + PAD_BOTTOM) * scale).ceil() as i32,
    )
}

fn finish(after: After) {
    if let After::Close = after {
        ui_hide();
    }
}

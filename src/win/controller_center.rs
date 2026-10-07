use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F;
use windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics,
    GetWindowRect, KillTimer, MessageBoxW, SendMessageW, SetClassLongPtrW, SetForegroundWindow,
    SetTimer, SetWindowPos, ShowWindow, GCLP_HICON, GCLP_HICONSM, HICON, HMENU, HTCAPTION,
    HTCLIENT, ICON_BIG, ICON_SMALL, IDOK, LR_DEFAULTCOLOR, MB_DEFBUTTON2, MB_ICONWARNING,
    MB_OKCANCEL, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WM_CLOSE,
    WM_DESTROY, WM_DPICHANGED, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCHITTEST, WM_SETICON, WM_SETTINGCHANGE, WM_TIMER, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_POPUP,
};

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::mono_ui::*;
use super::vk_renderer::{self, VkPalette};

const WINDOW_CLASS: PCWSTR = w!("WarmupControllerCenter");
const TIMER_ID: usize = 31;
const TIMER_MS: u32 = 33;

const WIN_W: f32 = 744.0;
const WIN_H: f32 = 720.0;
const WIN_R: f32 = 24.0;
const PAD_X: f32 = 72.0;
const PAD_TOP: f32 = 48.0;
const PAD_BOTTOM: f32 = 96.0;
const SHADOW_DY: f32 = 24.0;
const SHADOW_SIGMA: f32 = 30.0;

const TITLE_H: f32 = 64.0;
const TAB_X: f32 = 24.0;
const TAB_Y: f32 = 68.0;
const TAB_W: f32 = 180.0;
const TAB_H: f32 = 52.0;
const TAB_STEP: f32 = 60.0;
const CONTENT_X: f32 = 224.0;
const CONTENT_Y: f32 = 68.0;
const CONTENT_W: f32 = 496.0;
const ROW_GAP: f32 = 8.0;
const ROW_PAD: f32 = 16.0;
const HINT_Y: f32 = 636.0;
const TRACK_W: f32 = CONTENT_W - 2.0 * ROW_PAD;
const THUMB: f32 = 18.0;
const CLOSE: Rect = rect(702.0, 25.0, 18.0, 18.0);
const CANCEL: Rect = rect(512.0, 652.0, 103.0, 44.0);
const APPLY: Rect = rect(625.0, 652.0, 95.0, 44.0);
const PILL_RIGHT: f32 = 688.0;
const GREEN: u32 = 0x0058D130;
const RED: u32 = 0x003A45FF;
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_EVERY: Duration = Duration::from_millis(90);

const TABS: [(&str, Icon); 4] = [
    ("General", Icon::Settings),
    ("Mouse", Icon::Mouse),
    ("Keyboard", Icon::Keyboard),
    ("Controller", Icon::Gamepad),
];

const PAD_BUTTONS: [&str; 10] = ["A", "B", "X", "Y", "LB", "RB", "LT", "RT", "L3", "R3"];

const APP_ICO: &[u8] = include_bytes!("../../assets/icon.ico");

fn ico_entry(bytes: &[u8], want: u32) -> Option<(usize, usize)> {
    let count = u16::from_le_bytes([*bytes.get(4)?, *bytes.get(5)?]) as usize;
    let mut best: Option<(u32, usize, usize)> = None;
    for i in 0..count {
        let e = bytes.get(6 + 16 * i..22 + 16 * i)?;
        let size = if e[0] == 0 { 256 } else { e[0] as u32 };
        let len = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        if off + len > bytes.len() {
            continue;
        }
        let better = match best {
            None => true,
            Some((b, _, _)) if size >= want => b < want || size < b,
            Some((b, _, _)) => b < want && size > b,
        };
        if better {
            best = Some((size, off, len));
        }
    }
    best.map(|(_, off, len)| (off, len))
}

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

enum Kind {
    Check {
        key: &'static str,
        on_val: &'static str,
        off_val: &'static str,
    },
    PausePoll,
    Slider {
        key: &'static str,
        min: i32,
        max: i32,
        div: f32,
        decimals: usize,
    },
    Vocabulary,
    Choice {
        key: &'static str,
        options: &'static [(&'static str, &'static str)],
    },
    Swatch {
        options: &'static [(&'static str, Option<&'static str>)],
    },
}

struct Item {
    tab: usize,
    label: &'static str,
    caption: Option<&'static str>,
    kind: Kind,
    value: i32,
}

fn item(tab: usize, label: &'static str, kind: Kind) -> Item {
    Item {
        tab,
        label,
        caption: None,
        kind,
        value: 0,
    }
}

fn captioned(mut it: Item, caption: &'static str) -> Item {
    it.caption = Some(caption);
    it
}

fn check(tab: usize, label: &'static str, key: &'static str) -> Item {
    item(
        tab,
        label,
        Kind::Check {
            key,
            on_val: "true",
            off_val: "false",
        },
    )
}

fn slider(
    tab: usize,
    label: &'static str,
    key: &'static str,
    min: i32,
    max: i32,
    div: f32,
    decimals: usize,
) -> Item {
    item(
        tab,
        label,
        Kind::Slider {
            key,
            min,
            max,
            div,
            decimals,
        },
    )
}

const VK_STYLE_OPTIONS: &[(&str, &str)] =
    &[("Normal", "normal"), ("Mono", "mono"), ("Modern", "modern")];
const VK_DISPLAY_OPTIONS: &[(&str, &str)] = &[("Auto", "auto"), ("TV", "tv"), ("Desk", "desk")];
const LED_SWATCHES: &[(&str, Option<&str>)] = &[
    ("Lavender", Some("#B6A0FF")),
    ("White", Some("#FFFFFF")),
    ("Red", Some("#FF3B30")),
    ("Orange", Some("#FF9500")),
    ("Green", Some("#34C759")),
    ("Cyan", Some("#32D2F0")),
    ("Blue", Some("#0A84FF")),
    ("Pink", Some("#FF2D92")),
    ("Off", None),
];
const LED_EFFECT_OPTIONS: &[(&str, &str)] = &[
    ("Solid", "solid"),
    ("Breathing", "breathing"),
    ("Rainbow", "rainbow"),
];

fn led_swatch_index(off: bool, rgb: (u8, u8, u8)) -> i32 {
    let last = LED_SWATCHES.len() as i32 - 1;
    if off {
        return last;
    }
    let hex = format!("#{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2);
    LED_SWATCHES
        .iter()
        .position(|(_, c)| *c == Some(hex.as_str()))
        .map_or(0, |i| i as i32)
}

fn led_effect_name(effect: crate::led_engine::LedEffect) -> &'static str {
    match effect {
        crate::led_engine::LedEffect::Breathing => "breathing",
        crate::led_engine::LedEffect::Rainbow => "rainbow",
        _ => "solid",
    }
}

fn led_choice(swatch: i32, effect: i32) -> (Option<&'static str>, &'static str) {
    let color = LED_SWATCHES
        .get(swatch.max(0) as usize)
        .and_then(|(_, c)| *c);
    match color {
        None => (None, "off"),
        Some(c) => (
            Some(c),
            LED_EFFECT_OPTIONS
                .get(effect.max(0) as usize)
                .map_or("solid", |(_, v)| v),
        ),
    }
}

fn is_led(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Swatch { .. }
            | Kind::Choice {
                key: "led_effect",
                ..
            }
    )
}

fn swatch_rect(row: Rect, count: usize, i: usize) -> Rect {
    let total = count as f32 * 26.0 + (count as f32 - 1.0) * 10.0;
    let x0 = row.x + row.w - ROW_PAD - total;
    rect(x0 + i as f32 * 36.0, row.y + 13.0, 26.0, 26.0)
}

fn items() -> Vec<Item> {
    vec![
        captioned(
            check(0, "Gamepad cursor", "cursor_enabled"),
            "Sticks move the mouse, A and B click",
        ),
        captioned(
            check(0, "Sleep during games", "sleep_on_game"),
            "Only the Guide button is watched",
        ),
        check(0, "Guide-only in games (legacy)", "auto_stop_on_game"),
        check(0, "Controller hints on the sign-in screen", "signin_hints"),
        check(0, "Guide / PS opens warmUP when closed", "guide_launch"),
        check(0, "Voice typing", "voice_enabled"),
        item(0, "Pause gamepad input", Kind::PausePoll),
        captioned(
            item(
                0,
                "Sign-in and lock screen only",
                Kind::Check {
                    key: "run_mode",
                    on_val: "signin",
                    off_val: "always",
                },
            ),
            "Hides the tray icon and this window after you sign in",
        ),
        slider(1, "Cursor speed", "cursor_speed", 1, 40, 1.0, 0),
        slider(1, "Cursor acceleration", "cursor_accel", 10, 50, 10.0, 1),
        slider(1, "Cursor dead zone", "cursor_deadzone", 0, 90, 100.0, 2),
        slider(1, "Cursor smoothing", "cursor_smoothing", 0, 90, 100.0, 2),
        slider(1, "Scroll speed", "scroll_speed", 1, 20, 1.0, 0),
        slider(1, "Scroll acceleration", "scroll_accel", 10, 50, 10.0, 1),
        captioned(
            check(1, "Natural scrolling", "natural_scroll"),
            "Invert the scroll direction",
        ),
        item(
            2,
            "Keyboard style",
            Kind::Choice {
                key: "vk_style",
                options: VK_STYLE_OPTIONS,
            },
        ),
        captioned(
            item(
                2,
                "Display",
                Kind::Choice {
                    key: "vk_display",
                    options: VK_DISPLAY_OPTIONS,
                },
            ),
            "TV adds a number row and larger keys. Modern always uses it.",
        ),
        captioned(
            item(
                2,
                "Floating layout",
                Kind::Check {
                    key: "vk_mode",
                    on_val: "floating",
                    off_val: "docked",
                },
            ),
            "A card in the middle instead of a docked bar",
        ),
        captioned(
            item(
                2,
                "Compact size",
                Kind::Check {
                    key: "vk_bar_scale",
                    on_val: "0.8",
                    off_val: "1.0",
                },
            ),
            "Smaller docked bar",
        ),
        captioned(
            item(2, "Coding vocabulary", Kind::Vocabulary),
            "Terminal and agent terms for voice typing",
        ),
        item(
            3,
            "Color",
            Kind::Swatch {
                options: LED_SWATCHES,
            },
        ),
        item(
            3,
            "Lightbar effect",
            Kind::Choice {
                key: "led_effect",
                options: LED_EFFECT_OPTIONS,
            },
        ),
    ]
}

fn gamepad_check(key: &str, s: &crate::config::GamepadSettings) -> bool {
    match key {
        "cursor_enabled" => s.cursor_enabled,
        "sleep_on_game" => s.sleep_on_game,
        "auto_stop_on_game" => s.auto_stop_on_game,
        "signin_hints" => s.signin_hints,
        "guide_launch" => s.guide_launch,
        "voice_enabled" => s.voice_enabled,
        "natural_scroll" => s.natural_scroll,
        _ => false,
    }
}

fn gamepad_slider(key: &str, s: &crate::config::GamepadSettings) -> f32 {
    match key {
        "cursor_speed" => s.cursor_speed,
        "cursor_accel" => s.cursor_accel,
        "cursor_deadzone" => s.cursor_deadzone,
        "cursor_smoothing" => s.cursor_smoothing,
        "scroll_speed" => s.scroll_speed,
        "scroll_accel" => s.scroll_accel,
        _ => 0.0,
    }
}

fn vk_style_name(style: crate::config::VkStyle) -> &'static str {
    match style {
        crate::config::VkStyle::Normal => "normal",
        crate::config::VkStyle::Mono => "mono",
        crate::config::VkStyle::Modern => "modern",
    }
}

fn vk_display_name(display: crate::config::VkDisplay) -> &'static str {
    match display {
        crate::config::VkDisplay::Auto => "auto",
        crate::config::VkDisplay::Tv => "tv",
        crate::config::VkDisplay::Desk => "desk",
    }
}

fn led_swatch_of(st: &crate::led_engine::LedState) -> i32 {
    led_swatch_index(
        st.effect == crate::led_engine::LedEffect::Off,
        (st.r, st.g, st.b),
    )
}

fn current(item: &Item) -> (bool, i32) {
    let s = crate::config::gamepad_settings();
    match &item.kind {
        Kind::Check { key, .. } => (
            match *key {
                "vk_mode" => {
                    crate::config::vk_layout_mode() == crate::config::VkLayoutMode::Floating
                }
                "vk_bar_scale" => crate::config::vk_bar_scale() < 1.0,
                "run_mode" => crate::config::run_mode() == crate::config::RunMode::SignIn,
                k => gamepad_check(k, &s),
            },
            0,
        ),
        Kind::PausePoll => (crate::gamepad_backend::userland_poll_paused(), 0),
        Kind::Vocabulary => (crate::config::vocabulary().coding, 0),
        Kind::Choice { key, .. } => {
            let value = match *key {
                "vk_style" => vk_style_name(crate::config::vk_style()),
                "vk_display" => vk_display_name(crate::config::vk_display()),
                "led_effect" => led_effect_name(crate::led_engine::led_snapshot().effect),
                _ => "",
            };
            (false, choice_index(&item.kind, value))
        }
        Kind::Swatch { .. } => (false, led_swatch_of(&crate::led_engine::led_snapshot())),
        Kind::Slider { key, div, .. } => (false, (gamepad_slider(key, &s) * div).round() as i32),
    }
}

fn default_value(item: &Item) -> i32 {
    let s = crate::config::GamepadSettings::default();
    let led = crate::led_engine::LedState::default();
    match &item.kind {
        Kind::Check { key, .. } => {
            (match *key {
                "vk_mode" => {
                    crate::config::parse_vk_layout_mode(None)
                        == crate::config::VkLayoutMode::Floating
                }
                "vk_bar_scale" => false,
                "run_mode" => crate::config::parse_run_mode(None) == crate::config::RunMode::SignIn,
                k => gamepad_check(k, &s),
            }) as i32
        }
        Kind::PausePoll => 0,
        Kind::Vocabulary => crate::config::parse_vocabulary(None).coding as i32,
        Kind::Choice { key, .. } => choice_index(
            &item.kind,
            match *key {
                "vk_style" => vk_style_name(crate::config::parse_vk_style(None)),
                "vk_display" => vk_display_name(crate::config::parse_vk_display(None)),
                "led_effect" => led_effect_name(led.effect),
                _ => "",
            },
        ),
        Kind::Swatch { .. } => led_swatch_of(&led),
        Kind::Slider {
            key, div, min, max, ..
        } => ((gamepad_slider(key, &s) * div).round() as i32).clamp(*min, *max),
    }
}

const CONFIRM_CARD: Rect = rect(162.0, 262.0, 420.0, 196.0);
const CONFIRM_CANCEL: Rect = rect(328.0, 390.0, 110.0, 44.0);
const CONFIRM_RESET: Rect = rect(448.0, 390.0, 110.0, 44.0);
const TOAST_FOR: Duration = Duration::from_secs(3);

fn local_stamp() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

fn export_file_name(y: u16, mo: u16, d: u16) -> String {
    format!("warmup-companion-settings-{y:04}{mo:02}{d:02}.ini")
}

fn reload_all(ui: &mut Ui) {
    for it in &mut ui.items {
        it.value = stored_value(it);
    }
    ui.words = crate::config::vocabulary().words.join(", ");
    ui.theme = current_theme();
    ui.led_orig = crate::led_engine::led_snapshot();
    ui.led_base = led_values(ui);
    ui.led_previewed = ui.led_base;
}

#[derive(Clone, Debug, PartialEq)]
enum Pending {
    ResetAll,
    Import(crate::config::ImportPlan),
}

fn backup_label(backup: &std::path::Path) -> &str {
    backup.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

fn confirm_text(pending: &Pending) -> (&'static str, Vec<String>, &'static str) {
    match pending {
        Pending::ResetAll => (
            "Reset everything?",
            vec![
                "All Controller Center settings go back to how they shipped.".to_string(),
                "Your current settings are backed up first.".to_string(),
            ],
            "Reset",
        ),
        Pending::Import(plan) => {
            let n = plan.accepted.len();
            let mut lines = vec![
                format!(
                    "{n} setting{} will be replaced.",
                    if n == 1 { "" } else { "s" }
                ),
                "Your current settings are backed up first.".to_string(),
            ];
            let m = plan.rejected.len();
            if m > 0 {
                lines.push(format!(
                    "{m} line{} skipped.",
                    if m == 1 { " was" } else { "s were" }
                ));
            }
            ("Import settings?", lines, "Import")
        }
    }
}

fn import_settings(ui: &mut Ui, plan: &crate::config::ImportPlan) {
    let result = crate::config::backup_settings();
    let backup = match result {
        Ok(b) => b,
        Err(e) => {
            crate::install::log_line(&format!("controller center: import backup: {e}"));
            ui.toast = Some((
                "Import failed. Your settings were not changed.".to_string(),
                Instant::now(),
            ));
            return;
        }
    };
    let mut failed = 0;
    for (key, value) in &plan.accepted {
        if crate::config::set_gamepad_setting(key, value).is_err() {
            failed += 1;
        }
    }
    let led = |k: &str| {
        plan.accepted
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    if led("led_color").is_some() || led("led_effect").is_some() {
        crate::led_engine::apply_led_choice(led("led_color"), led("led_effect"));
    }
    reload_all(ui);
    let done = plan.accepted.len() - failed;
    ui.toast = Some((
        format!(
            "Imported {done} setting{}. Backup: {}",
            if done == 1 { "" } else { "s" },
            backup_label(&backup)
        ),
        Instant::now(),
    ));
}

fn start_import(ui: &mut Ui) {
    let picked = unsafe { pick_file(ui.hwnd, false, false) };
    let path = match picked {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            crate::install::log_line(&format!("controller center: import dialog: {e}"));
            ui.toast = Some(("Import failed".to_string(), Instant::now()));
            return;
        }
    };
    let text = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            crate::install::log_line(&format!("controller center: import read: {e}"));
            ui.toast = Some(("Could not read that file".to_string(), Instant::now()));
            return;
        }
    };
    let plan = crate::config::parse_settings_import(&text);
    if plan.accepted.is_empty() {
        ui.toast = Some((
            "No valid settings in that file. Nothing was changed.".to_string(),
            Instant::now(),
        ));
        return;
    }
    ui.confirm = Some((Pending::Import(plan), false));
}

fn reset_everything(ui: &mut Ui) {
    let result = crate::config::reset_settings_to_shipped();
    crate::gamepad_backend::set_userland_poll_paused(false);
    let _ = crate::config::write_userland_poll_paused(false);
    let _ = crate::config::set_prompt_userland_debug(false);
    crate::led_engine::restore_led(crate::led_engine::LedState::default());
    reload_all(ui);
    ui.toast = Some((
        match result {
            Ok(backup) => format!("Defaults restored. Backup: {}", backup_label(&backup)),
            Err(e) => {
                crate::install::log_line(&format!("controller center: reset all: {e}"));
                "Reset failed. Your settings were not changed.".to_string()
            }
        },
        Instant::now(),
    ));
}

fn staged_changes(ui: &Ui) -> bool {
    ui.items.iter().any(|it| it.value != stored_value(it)) || ui.led_previewed != ui.led_base
}

fn documents_dir() -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let mut token = HANDLE::default();
        let have_token = WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token).is_ok();
        let result = if have_token {
            SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, token)
        } else {
            SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None)
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

unsafe fn pick_file(
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

fn export_settings(ui: &mut Ui) {
    let dirty = staged_changes(ui);
    let result = unsafe { pick_file(ui.hwnd, true, dirty) }.and_then(|dest| {
        let Some(dest) = dest else {
            return Ok(None);
        };
        crate::config::export_settings_to(&dest)?;
        Ok(Some(dest))
    });
    let line = match result {
        Ok(None) => return,
        Ok(Some(dest)) => format!(
            "Settings exported to {}",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("")
        ),
        Err(e) => {
            crate::install::log_line(&format!("controller center: export: {e}"));
            "Export failed".to_string()
        }
    };
    ui.toast = Some((line, Instant::now()));
}

fn confirm_input(ui: &mut Ui, action: ConfirmInput) {
    let Some((pending, on_ok)) = ui.confirm.take() else {
        return;
    };
    match action {
        ConfirmInput::Toggle(ok) => ui.confirm = Some((pending, ok)),
        ConfirmInput::Cancel => {}
        ConfirmInput::Activate if !on_ok => {}
        ConfirmInput::Activate => match pending {
            Pending::ResetAll => reset_everything(ui),
            Pending::Import(plan) => import_settings(ui, &plan),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfirmInput {
    Toggle(bool),
    Cancel,
    Activate,
}

fn reset_tab(ui: &mut Ui) {
    let tab = ui.tab;
    for it in ui.items.iter_mut().filter(|it| it.tab == tab) {
        it.value = default_value(it);
    }
}

fn is_toggle(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Check { .. } | Kind::PausePoll | Kind::Vocabulary
    )
}

fn stored_value(item: &Item) -> i32 {
    let (on, pos) = current(item);
    if is_toggle(&item.kind) {
        on as i32
    } else {
        pos
    }
}

fn value_range(kind: &Kind) -> (i32, i32) {
    match kind {
        Kind::Slider { min, max, .. } => (*min, *max),
        Kind::Choice { options, .. } => (0, options.len() as i32 - 1),
        Kind::Swatch { options } => (0, options.len() as i32 - 1),
        _ => (0, 1),
    }
}

fn choice_index(kind: &Kind, value: &str) -> i32 {
    match kind {
        Kind::Choice { options, .. } => {
            options.iter().position(|(_, v)| *v == value).unwrap_or(0) as i32
        }
        _ => 0,
    }
}

fn slider_text(kind: &Kind, pos: i32) -> String {
    match kind {
        Kind::Slider { div, decimals, .. } => format!("{:.*}", decimals, pos as f32 / div),
        _ => String::new(),
    }
}

fn slider_fraction(min: i32, max: i32, pos: i32) -> f32 {
    if max <= min {
        return 0.0;
    }
    ((pos - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
}

fn slider_thumb_x(row: Rect, min: i32, max: i32, pos: i32) -> f32 {
    row.x + ROW_PAD + slider_fraction(min, max, pos) * (TRACK_W - THUMB)
}

fn slider_pos_at(row: Rect, min: i32, max: i32, x: f32) -> i32 {
    let f = ((x - row.x - ROW_PAD - THUMB / 2.0) / (TRACK_W - THUMB)).clamp(0.0, 1.0);
    min + (f * (max - min) as f32).round() as i32
}

fn segment_rect(row: Rect, count: usize, i: usize) -> Rect {
    let well = segmented_rect(row);
    let w = (well.w - 6.0 - 3.0 * (count as f32 - 1.0)) / count as f32;
    rect(well.x + 3.0 + i as f32 * (w + 3.0), well.y + 3.0, w, 32.0)
}

fn segmented_rect(row: Rect) -> Rect {
    rect(row.x + ROW_PAD, row.y + 39.0, TRACK_W, 38.0)
}

fn toggle_rect(row: Rect) -> Rect {
    rect(
        row.x + row.w - ROW_PAD - 46.0,
        row.y + ((row.h - 28.0) / 2.0).round(),
        46.0,
        28.0,
    )
}

fn is_card_picker(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Choice {
            key: "vk_style" | "vk_display",
            ..
        }
    )
}

const CARD_X: [f32; 3] = [0.0, 157.0, 315.0];
const CARD_W: f32 = 149.0;
const CARD_H: f32 = 113.0;

fn card_rect(row: Rect, caption: bool, i: usize) -> Rect {
    let top = if caption { 64.0 } else { 39.0 };
    rect(
        row.x + ROW_PAD + CARD_X[i.min(2)],
        row.y + top,
        CARD_W,
        CARD_H,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MiniLayout {
    rows: &'static [&'static str],
    key_w: f32,
    key_h: f32,
    pitch_x: f32,
    pitch_y: f32,
    inset: f32,
    radius: f32,
    label_px: f32,
    shadow: bool,
}

const MINI_DESK: MiniLayout = MiniLayout {
    rows: &["QWER", "ASDF"],
    key_w: 29.0,
    key_h: 19.0,
    pitch_x: 31.3,
    pitch_y: 21.0,
    inset: 5.0,
    radius: 3.0,
    label_px: 9.0,
    shadow: true,
};

const MINI_TV: MiniLayout = MiniLayout {
    rows: &["1234", "QWER", "ASDF"],
    key_w: 27.0,
    key_h: 17.0,
    pitch_x: 31.0,
    pitch_y: 21.0,
    inset: 7.0,
    radius: 3.0,
    label_px: 10.0,
    shadow: true,
};

const MINI_MODERN: MiniLayout = MiniLayout {
    pitch_x: 30.7,
    radius: 2.5,
    shadow: false,
    ..MINI_TV
};

fn mini_height(l: &MiniLayout) -> f32 {
    2.0 * l.inset + (l.rows.len() as f32 - 1.0) * l.pitch_y + l.key_h
}

fn mini_key_rect(art: Rect, l: &MiniLayout, row: usize, col: usize) -> Rect {
    rect(
        art.x + l.inset + col as f32 * l.pitch_x,
        art.y + l.inset + row as f32 * l.pitch_y,
        l.key_w,
        l.key_h,
    )
}

fn style_card_art(i: usize) -> (crate::config::VkStyle, &'static MiniLayout) {
    match i {
        0 => (crate::config::VkStyle::Normal, &MINI_DESK),
        1 => (crate::config::VkStyle::Mono, &MINI_DESK),
        _ => (crate::config::VkStyle::Modern, &MINI_MODERN),
    }
}

fn set(key: &str, value: &str) {
    if let Err(e) = crate::config::set_gamepad_setting(key, value) {
        crate::install::log_line(&format!("controller center: set {key}={value}: {e}"));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Item(usize),
    Device,
    Live,
    Rumble,
    Lightbar,
    Export,
    Import,
    ResetAll,
    Cancel,
    Apply,
}

impl Target {
    fn focusable(self) -> bool {
        !matches!(self, Target::Live)
    }
}

fn item_height(it: &Item) -> f32 {
    match it.kind {
        Kind::Slider { .. } => 63.0,
        Kind::Choice { .. } if is_card_picker(&it.kind) && it.caption.is_some() => 189.0,
        Kind::Choice { .. } if is_card_picker(&it.kind) => 164.0,
        Kind::Choice { .. } => 89.0,
        Kind::Swatch { .. } => 52.0,
        _ if it.caption.is_some() => 59.0,
        _ => 52.0,
    }
}

fn rows(tab: usize, items: &[Item], led_open: bool) -> Vec<(Target, Rect)> {
    let mut stack: Vec<(Target, f32)> = Vec::new();
    match tab {
        3 => stack.extend([
            (Target::Device, 84.0),
            (Target::Live, 238.0),
            (Target::Rumble, 48.0),
        ]),
        _ => {}
    }
    for (i, it) in items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.tab == tab && (led_open || !is_led(&it.kind)))
    {
        stack.push((Target::Item(i), item_height(it)));
    }
    if tab == 0 {
        stack.push((Target::Export, 48.0));
    }
    let mut out = Vec::new();
    let mut y = CONTENT_Y;
    for (t, h) in stack {
        if let Some(group) = group_of(t) {
            let n = group.len() as f32;
            let w = (CONTENT_W - ROW_GAP * (n - 1.0)) / n;
            for (gi, member) in group.iter().enumerate() {
                out.push((
                    *member,
                    rect(CONTENT_X + gi as f32 * (w + ROW_GAP), y, w, h),
                ));
            }
        } else {
            out.push((t, rect(CONTENT_X, y, CONTENT_W, h)));
        }
        y += h + ROW_GAP;
    }
    out
}

fn tab_rect(i: usize) -> Rect {
    rect(TAB_X, TAB_Y + i as f32 * TAB_STEP, TAB_W, TAB_H)
}

fn focus_order(tab: usize, items: &[Item], led_open: bool) -> Vec<Target> {
    rows(tab, items, led_open)
        .into_iter()
        .map(|(t, _)| t)
        .filter(|t| t.focusable())
        .chain([Target::Cancel, Target::Apply])
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav {
    Up,
    Down,
    Left,
    Right,
}

const ACTION_GROUPS: [&[Target]; 3] = [
    &[Target::Rumble, Target::Lightbar],
    &[Target::Export, Target::Import, Target::ResetAll],
    &[Target::Cancel, Target::Apply],
];

fn group_of(t: Target) -> Option<&'static [Target]> {
    ACTION_GROUPS.iter().copied().find(|g| g.contains(&t))
}

fn same_group(a: Target, b: Target) -> bool {
    group_of(a).is_some_and(|g| g.contains(&b))
}

fn step_focus(order: &[Target], cur: Target, nav: Nav) -> Target {
    let Some(idx) = order.iter().position(|t| *t == cur) else {
        return order.first().copied().unwrap_or(cur);
    };
    match nav {
        Nav::Up => order[..idx]
            .iter()
            .rev()
            .find(|t| !same_group(**t, cur))
            .copied()
            .unwrap_or(cur),
        Nav::Down => order[idx + 1..]
            .iter()
            .find(|t| !same_group(**t, cur))
            .copied()
            .unwrap_or(cur),
        Nav::Left | Nav::Right => {
            let Some(g) = group_of(cur) else {
                return cur;
            };
            let at = g.iter().position(|t| *t == cur).unwrap_or(0);
            let next = if nav == Nav::Left {
                at.saturating_sub(1)
            } else {
                (at + 1).min(g.len() - 1)
            };
            g[next]
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct PadInput {
    pressed: Vec<String>,
    ls: (f32, f32),
    rs: (f32, f32),
}

fn parse_stick(tok: &str, prefix: &str) -> Option<(f32, f32)> {
    let inner = tok.strip_prefix(prefix)?.strip_suffix(')')?;
    let (x, y) = inner.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn parse_input(s: &str) -> PadInput {
    let mut out = PadInput::default();
    for tok in s.split_whitespace() {
        if let Some(v) = parse_stick(tok, "L(") {
            out.ls = v;
        } else if let Some(v) = parse_stick(tok, "R(") {
            out.rs = v;
        } else if !tok.starts_with('(') {
            out.pressed.push(tok.to_string());
        }
    }
    out
}

fn short_pad_name(label: &str) -> &'static str {
    let l = label.to_ascii_lowercase();
    if l.contains("dualsense") {
        "DualSense"
    } else if l.contains("dualshock") || l.contains("ps4") {
        "DualShock 4"
    } else if l.contains("xbox") || l.contains("xinput") {
        "Xbox"
    } else if vk_renderer::is_playstation_label(label) {
        "PlayStation"
    } else {
        "Controller"
    }
}

fn device_name(label: &str) -> String {
    let l = label.to_ascii_lowercase();
    if l.starts_with("xinput slot") {
        "Xbox Controller".into()
    } else if l.starts_with("hid slot") {
        "PlayStation Controller".into()
    } else {
        label.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Battery {
    percent: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Live {
    connected: bool,
    label: String,
    input: PadInput,
    battery: Option<Battery>,
    usb: Option<bool>,
}

impl Live {
    fn read() -> Self {
        let s = crate::debug_state::snapshot();
        let connected = s.connected && !s.name.is_empty() && s.name != "none";
        let battery = crate::pipe_server::current_battery()
            .filter(|b| connected && b.percent >= 0)
            .map(|b| Battery {
                percent: b.percent.min(100),
            });
        Live {
            connected,
            label: s.name,
            input: if connected {
                parse_input(&s.input)
            } else {
                PadInput::default()
            },
            battery,
            usb: crate::pipe_server::current_usb().filter(|_| connected),
        }
    }

    fn playstation(&self) -> bool {
        !self.connected || vk_renderer::is_playstation_label(&self.label)
    }

    fn pill(&self) -> String {
        if !self.connected {
            return "No controller".into();
        }
        let name = short_pad_name(&self.label);
        match self.battery {
            Some(b) => format!("{name} \u{00B7} {}%", b.percent),
            None => name.into(),
        }
    }

    fn caption(&self) -> String {
        if !self.connected {
            return "Plug in or pair a controller".into();
        }
        let l = self.label.to_ascii_lowercase();
        let mut parts = vec!["Connected"];
        if l.contains("hid slot") {
            parts.push("HID");
        } else if l.contains("xinput") {
            parts.push("XInput");
        }
        if let Some(t) = transport_label(self.usb) {
            parts.push(t);
        }
        parts.join(" \u{00B7} ")
    }
}

fn transport_label(usb: Option<bool>) -> Option<&'static str> {
    usb.map(|wired| if wired { "USB" } else { "Bluetooth" })
}

fn surface_px(scale: f32) -> (i32, i32) {
    (
        ((WIN_W + 2.0 * PAD_X) * scale).ceil() as i32,
        ((WIN_H + PAD_TOP + PAD_BOTTOM) * scale).ceil() as i32,
    )
}

#[derive(Default)]
struct PadNav {
    held: Option<(Nav, Instant, Instant)>,
}

struct Ui {
    hwnd: HWND,
    scale: f32,
    tab: usize,
    focus: Target,
    items: Vec<Item>,
    words: String,
    theme: Theme,
    live: Live,
    gfx: Option<Gfx>,
    drag: Option<usize>,
    pad: PadNav,
    led_open: bool,
    led_orig: crate::led_engine::LedState,
    led_base: (i32, i32),
    led_previewed: (i32, i32),
    reset_hint: std::cell::Cell<Rect>,
    confirm: Option<(Pending, bool)>,
    toast: Option<(String, Instant)>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static THREAD: OnceLock<Mutex<Option<DesktopWindowThread>>> = OnceLock::new();
static OWNS_PAD: AtomicBool = AtomicBool::new(false);
static PAD_EDGES: Mutex<Vec<(&'static str, bool)>> = Mutex::new(Vec::new());
const PAD_EDGE_CAP: usize = 64;

static MENU_OWNS: AtomicBool = AtomicBool::new(false);

pub(crate) fn owns_pad() -> bool {
    OWNS_PAD.load(Ordering::SeqCst) || MENU_OWNS.load(Ordering::SeqCst)
}

pub(crate) fn set_menu_owns_pad(on: bool) {
    let _ = take_pad_edges();
    MENU_OWNS.store(on, Ordering::SeqCst);
}

pub(crate) fn take_menu_pad_edges() -> Vec<(&'static str, bool)> {
    take_pad_edges()
}

pub(crate) fn pad_status() -> (bool, String) {
    let live = Live::read();
    (live.connected, live.pill())
}

pub(crate) fn push_pad_edge(button: &'static str, pressed: bool) {
    if let Ok(mut q) = PAD_EDGES.lock() {
        if q.len() >= PAD_EDGE_CAP {
            q.remove(0);
        }
        q.push((button, pressed));
    }
}

fn take_pad_edges() -> Vec<(&'static str, bool)> {
    PAD_EDGES
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

pub(crate) fn toggle() -> bool {
    if OWNS_PAD.load(Ordering::SeqCst) {
        hide();
        false
    } else {
        show();
        OWNS_PAD.load(Ordering::SeqCst)
    }
}

fn hide() {
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
    if guard.is_none() {
        match desktop_window::spawn(CenterApp) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("controller center: spawn failed: {e}"));
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
            w!("Controller Center"),
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
            crate::install::log_line("controller center: CreateWindowExW failed");
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
        let mut items = items();
        for it in &mut items {
            it.value = stored_value(it);
        }
        let focus = focus_order(0, &items, false)[0];
        let ui = Ui {
            hwnd,
            scale,
            tab: 0,
            focus,
            items,
            words: crate::config::vocabulary().words.join(", "),
            theme: current_theme(),
            live: Live::read(),
            gfx: None,
            drag: None,
            pad: PadNav::default(),
            led_open: false,
            led_orig: crate::led_engine::led_snapshot(),
            led_base: (0, 0),
            led_previewed: (0, 0),
            reset_hint: std::cell::Cell::new(rect(0.0, 0.0, 0.0, 0.0)),
            confirm: None,
            toast: None,
        };
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

fn ui_hide() {
    OWNS_PAD.store(false, Ordering::SeqCst);
    let _ = take_pad_edges();
    if let Some(ui) = UI.with(|u| u.borrow_mut().take()) {
        if ui.led_previewed != ui.led_base {
            crate::led_engine::restore_led(ui.led_orig);
        }
        unsafe {
            let _ = KillTimer(ui.hwnd, TIMER_ID);
            let _ = DestroyWindow(ui.hwnd);
        }
    }
}

fn with_ui(f: impl FnOnce(&mut Ui)) {
    UI.with(|u| {
        if let Ok(mut b) = u.try_borrow_mut() {
            if let Some(ui) = b.as_mut() {
                f(ui);
            }
        }
    });
}

fn led_values(ui: &Ui) -> (i32, i32) {
    let pick = |f: fn(&Kind) -> bool| {
        ui.items
            .iter()
            .find(|it| f(&it.kind))
            .map_or(0, |it| it.value)
    };
    (
        pick(|k| matches!(k, Kind::Swatch { .. })),
        pick(|k| {
            matches!(
                k,
                Kind::Choice {
                    key: "led_effect",
                    ..
                }
            )
        }),
    )
}

fn sync_led_preview(ui: &mut Ui) {
    let now = led_values(ui);
    if now != ui.led_previewed {
        ui.led_previewed = now;
        let (color, effect) = led_choice(now.0, now.1);
        crate::led_engine::apply_led_choice(color, Some(effect));
    }
}

fn apply_led(ui: &mut Ui) {
    let now = led_values(ui);
    if now == ui.led_base {
        return;
    }
    let (color, effect) = led_choice(now.0, now.1);
    if let Some(c) = color {
        set("led_color", c);
    }
    set("led_effect", effect);
    crate::led_engine::apply_led_choice(color, Some(effect));
    ui.led_base = now;
    ui.led_previewed = now;
    ui.led_orig = crate::led_engine::led_snapshot();
}

fn apply(ui: &mut Ui) -> bool {
    apply_led(ui);
    let mut declined = false;
    let hwnd = ui.hwnd;
    let words = ui.words.clone();
    for it in &mut ui.items {
        if is_led(&it.kind) {
            continue;
        }
        let (on, pos) = current(it);
        let staged_on = it.value != 0;
        match &it.kind {
            Kind::Check {
                key,
                on_val,
                off_val,
            } => {
                if staged_on != on {
                    if *key == "run_mode" && staged_on && !unsafe { confirm_signin_only(hwnd) } {
                        it.value = 0;
                        declined = true;
                        continue;
                    }
                    set(key, if staged_on { on_val } else { off_val });
                }
            }
            Kind::PausePoll => {
                if staged_on != on {
                    crate::gamepad_backend::set_userland_poll_paused(staged_on);
                    let _ = crate::config::write_userland_poll_paused(staged_on);
                }
            }
            Kind::Vocabulary => {
                let staged = staged_vocabulary(staged_on, &words);
                if staged != crate::config::vocabulary() {
                    set("vocabulary", &crate::config::format_vocabulary(&staged));
                }
            }
            Kind::Choice { key, options } => {
                if it.value != pos {
                    if let Some((_, value)) = options.get(it.value as usize) {
                        set(key, value);
                    }
                }
            }
            Kind::Slider { key, .. } => {
                if it.value != pos {
                    set(key, &slider_text(&it.kind, it.value));
                }
            }
            Kind::Swatch { .. } => {}
        }
    }
    for it in &mut ui.items {
        it.value = stored_value(it);
    }
    ui.words = crate::config::vocabulary().words.join(", ");
    ui.theme = current_theme();
    !declined
}

unsafe fn confirm_signin_only(owner: HWND) -> bool {
    let title = wide("Sign-in screen only");
    let body = wide(
        "After you next sign in or unlock, the companion sleeps on the desktop: no controller input, \
         no tray icon and no Controller Center.\r\n\r\n\
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

fn staged_vocabulary(coding: bool, words: &str) -> crate::config::Vocabulary {
    let cleaned: String = words
        .chars()
        .map(|c| if c == '=' || c.is_control() { ' ' } else { c })
        .collect();
    let mut vocab = crate::config::parse_vocabulary_list(&cleaned);
    vocab.coding = coding;
    vocab
}

fn switch_tab(ui: &mut Ui, tab: usize) {
    ui.tab = tab % TABS.len();
    ui.focus = focus_order(ui.tab, &ui.items, ui.led_open)[0];
    ui.drag = None;
}

fn adjust(ui: &mut Ui, delta: i32) -> bool {
    let Target::Item(i) = ui.focus else {
        return false;
    };
    let it = &mut ui.items[i];
    if !matches!(
        it.kind,
        Kind::Slider { .. } | Kind::Choice { .. } | Kind::Swatch { .. }
    ) {
        return false;
    }
    let (lo, hi) = value_range(&it.kind);
    it.value = (it.value + delta).clamp(lo, hi);
    true
}

fn rumble() {
    crate::device_commands::push_device_command(crate::gamepad_backend::PadCommand::Rumble {
        strong: 0.6,
        weak: 0.6,
        ms: 300,
    });
}

enum After {
    Stay,
    Close,
}

fn activate(ui: &mut Ui, target: Target) -> After {
    match target {
        Target::Item(i) => {
            let it = &mut ui.items[i];
            if is_toggle(&it.kind) {
                it.value = (it.value == 0) as i32;
            } else if let Kind::Choice { options, .. } = it.kind {
                it.value = (it.value + 1) % options.len() as i32;
            } else if let Kind::Swatch { options } = it.kind {
                it.value = (it.value + 1) % options.len() as i32;
            }
        }
        Target::Rumble => rumble(),
        Target::Export => export_settings(ui),
        Target::Import => start_import(ui),
        Target::ResetAll => ui.confirm = Some((Pending::ResetAll, false)),
        Target::Lightbar => {
            ui.led_open = !ui.led_open;
            if ui.led_open {
                if let Some(i) = ui
                    .items
                    .iter()
                    .position(|it| matches!(it.kind, Kind::Swatch { .. }))
                {
                    ui.focus = Target::Item(i);
                }
            }
        }
        Target::Apply => {
            if apply(ui) {
                return After::Close;
            }
        }
        Target::Cancel => return After::Close,
        _ => {}
    }
    After::Stay
}

fn navigate(ui: &mut Ui, nav: Nav) {
    let delta = match nav {
        Nav::Left => -1,
        Nav::Right => 1,
        _ => 0,
    };
    if delta != 0 && adjust(ui, delta) {
        return;
    }
    let order = focus_order(ui.tab, &ui.items, ui.led_open);
    ui.focus = step_focus(&order, ui.focus, nav);
}

fn finish(after: After) {
    if let After::Close = after {
        ui_hide();
    }
}

fn design_point(ui: &Ui, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x / ui.scale - PAD_X, y / ui.scale - PAD_TOP)
}

fn click(ui: &mut Ui, x: f32, y: f32) -> After {
    if ui.confirm.is_some() {
        if CONFIRM_RESET.contains(x, y) {
            confirm_input(ui, ConfirmInput::Toggle(true));
            confirm_input(ui, ConfirmInput::Activate);
        } else if CONFIRM_CANCEL.contains(x, y) {
            confirm_input(ui, ConfirmInput::Cancel);
        }
        return After::Stay;
    }
    if CLOSE.inset(-7.0).contains(x, y) {
        return After::Close;
    }
    for i in 0..TABS.len() {
        if tab_rect(i).contains(x, y) {
            switch_tab(ui, i);
            return After::Stay;
        }
    }
    if ui.reset_hint.get().contains(x, y) {
        reset_tab(ui);
        return After::Stay;
    }
    for t in [Target::Cancel, Target::Apply] {
        if (if t == Target::Cancel { CANCEL } else { APPLY }).contains(x, y) {
            ui.focus = t;
            return activate(ui, t);
        }
    }
    let hit = rows(ui.tab, &ui.items, ui.led_open)
        .into_iter()
        .find(|(_, r)| r.contains(x, y));
    let Some((t, r)) = hit else {
        return After::Stay;
    };
    if t.focusable() {
        ui.focus = t;
    }
    match t {
        Target::Item(i) => match ui.items[i].kind {
            Kind::Slider { min, max, .. } => {
                ui.items[i].value = slider_pos_at(r, min, max, x);
                ui.drag = Some(i);
                unsafe {
                    SetCapture(ui.hwnd);
                }
            }
            Kind::Choice { options, .. } if is_card_picker(&ui.items[i].kind) => {
                let caption = ui.items[i].caption.is_some();
                if let Some(s) =
                    (0..options.len()).find(|s| card_rect(r, caption, *s).contains(x, y))
                {
                    ui.items[i].value = s as i32;
                }
            }
            Kind::Choice { options, .. } => {
                if let Some(s) =
                    (0..options.len()).find(|s| segment_rect(r, options.len(), *s).contains(x, y))
                {
                    ui.items[i].value = s as i32;
                }
            }
            Kind::Swatch { options } => {
                if let Some(s) = (0..options.len())
                    .find(|s| swatch_rect(r, options.len(), *s).inset(-4.0).contains(x, y))
                {
                    ui.items[i].value = s as i32;
                }
            }
            _ => return activate(ui, t),
        },
        Target::Rumble => rumble(),
        Target::Lightbar | Target::Export | Target::Import | Target::ResetAll => {
            return activate(ui, t)
        }
        _ => {}
    }
    After::Stay
}

fn key_down(ui: &mut Ui, vk: u16) -> After {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    if let Some(input) = confirm_key(vk) {
        if ui.confirm.is_some() {
            confirm_input(ui, input);
            return After::Stay;
        }
    }
    if ui.confirm.is_some() {
        return After::Stay;
    }
    match vk {
        v if v == VK_ESCAPE.0 => return After::Close,
        0x52 if ctrl => reset_tab(ui),
        v if v == VK_TAB.0 && ctrl => {
            let n = TABS.len();
            switch_tab(ui, if shift { ui.tab + n - 1 } else { ui.tab + 1 });
        }
        v if v == VK_TAB.0 => navigate(ui, if shift { Nav::Up } else { Nav::Down }),
        v if v == VK_UP.0 => navigate(ui, Nav::Up),
        v if v == VK_DOWN.0 => navigate(ui, Nav::Down),
        v if v == VK_LEFT.0 => navigate(ui, Nav::Left),
        v if v == VK_RIGHT.0 => navigate(ui, Nav::Right),
        v if v == VK_RETURN.0 => return activate(ui, ui.focus),
        v if v == VK_SPACE.0 => return activate(ui, ui.focus),
        _ => {}
    }
    After::Stay
}

fn pad_nav(button: &str) -> Option<Nav> {
    match button {
        "UP" => Some(Nav::Up),
        "DOWN" => Some(Nav::Down),
        "LEFT" => Some(Nav::Left),
        "RIGHT" => Some(Nav::Right),
        _ => None,
    }
}

fn confirm_key(vk: u16) -> Option<ConfirmInput> {
    match vk {
        v if v == VK_ESCAPE.0 => Some(ConfirmInput::Cancel),
        v if v == VK_RETURN.0 || v == VK_SPACE.0 => Some(ConfirmInput::Activate),
        v if v == VK_LEFT.0 => Some(ConfirmInput::Toggle(false)),
        v if v == VK_RIGHT.0 => Some(ConfirmInput::Toggle(true)),
        _ => None,
    }
}

fn confirm_pad(button: &str) -> Option<ConfirmInput> {
    match button {
        "A" => Some(ConfirmInput::Activate),
        "B" => Some(ConfirmInput::Cancel),
        "LEFT" => Some(ConfirmInput::Toggle(false)),
        "RIGHT" => Some(ConfirmInput::Toggle(true)),
        _ => None,
    }
}

fn tick(ui: &mut Ui) -> (bool, After) {
    let live = Live::read();
    let mut dirty = live != ui.live;
    ui.live = live;
    let now = Instant::now();
    if ui
        .toast
        .as_ref()
        .is_some_and(|(_, at)| at.elapsed() >= TOAST_FOR)
    {
        ui.toast = None;
        dirty = true;
    }
    let edges = if MENU_OWNS.load(Ordering::SeqCst) {
        Vec::new()
    } else {
        take_pad_edges()
    };
    for (button, pressed) in edges {
        dirty = true;
        if ui.confirm.is_some() {
            if pressed {
                if let Some(input) = confirm_pad(button) {
                    confirm_input(ui, input);
                }
            }
            continue;
        }
        if !pressed {
            if matches!(ui.pad.held, Some((nav, _, _)) if pad_nav(button) == Some(nav)) {
                ui.pad.held = None;
            }
            continue;
        }
        match button {
            "A" => {
                if let After::Close = activate(ui, ui.focus) {
                    return (dirty, After::Close);
                }
            }
            "B" => return (dirty, After::Close),
            "Y" => reset_tab(ui),
            "LB" => switch_tab(ui, ui.tab + TABS.len() - 1),
            "RB" => switch_tab(ui, ui.tab + 1),
            other => {
                if let Some(nav) = pad_nav(other) {
                    navigate(ui, nav);
                    ui.pad.held = Some((nav, now, now));
                }
            }
        }
    }
    if let Some((nav, since, last)) = ui.pad.held {
        if now - since >= REPEAT_DELAY && now - last >= REPEAT_EVERY {
            navigate(ui, nav);
            ui.pad.held = Some((nav, since, now));
            dirty = true;
        }
    }
    (dirty, After::Stay)
}

fn paint(ui: &mut Ui) {
    sync_led_preview(ui);
    let mut gfx = match ui.gfx.take() {
        Some(g) => g,
        None => match unsafe { Gfx::new() } {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("controller center: gfx: {e}"));
                return;
            }
        },
    };
    if let Err(e) = unsafe { render(ui, &mut gfx) } {
        crate::install::log_line(&format!("controller center: render: {e}"));
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

unsafe fn draw_window(ui: &Ui, g: &mut Gfx) {
    let t = ui.theme;
    let s = ui.scale;
    let card = rect(0.0, 0.0, WIN_W, WIN_H);
    g.fill(card, WIN_R, t.bg, 1.0);
    g.ring(card, WIN_R, 1.0, t.line, t.line_alpha);

    g.icon(Icon::Logo, rect(24.0, 25.0, 18.0, 18.0), t.text, 1.0, s);
    g.text(
        "Controller Center",
        rect(52.0, 25.0, 300.0, 18.0),
        15.0,
        true,
        t.text,
        1.0,
        Align::Left,
    );
    let pill = ui.live.pill();
    let tw = g.measure(&pill, 12.0, false);
    let pw = 12.0 + 8.0 + 8.0 + tw + 12.0;
    let px = PILL_RIGHT - pw;
    g.fill(rect(px, 20.0, pw, 28.0), 14.0, t.tile, 1.0);
    g.circle(
        px + 16.0,
        34.0,
        4.0,
        if ui.live.connected { GREEN } else { t.idle_dot },
    );
    g.text(
        &pill,
        rect(px + 28.0, 20.0, tw + 4.0, 28.0),
        12.0,
        false,
        t.dim,
        DIM_ALPHA,
        Align::Left,
    );
    g.icon(Icon::Close, CLOSE, t.dim, DIM_ALPHA, s);

    for (i, (name, icon)) in TABS.iter().enumerate() {
        let r = tab_rect(i);
        let sel = i == ui.tab;
        g.fill(r, 12.0, if sel { t.hi } else { t.tile }, 1.0);
        if sel {
            g.ring(r, 12.0, 2.0, t.ring, 1.0);
        }
        let (c, a) = if sel {
            (t.text, 1.0)
        } else {
            (t.dim, DIM_ALPHA)
        };
        g.icon(*icon, rect(40.0, r.y + 17.0, 18.0, 18.0), c, a, s);
        g.text(
            name,
            rect(70.0, r.y, r.w - 46.0, r.h),
            15.0,
            sel,
            c,
            a,
            Align::Left,
        );
    }

    for (target, r) in rows(ui.tab, &ui.items, ui.led_open) {
        draw_row(ui, g, target, r);
    }

    g.brush.SetColor(&color(t.line, t.line_alpha));
    g.rt.FillRectangle(&rect(0.0, HINT_Y, WIN_W, 1.0).d2d(), &g.brush);
    let ps = ui.live.playstation();
    let mut x = 24.0;
    let hints: &[(&[usize], &str)] = if ui.toast.is_some() {
        &[]
    } else {
        &[
            (&[0], "Select"),
            (&[1], "Back"),
            (&[4, 5], "Tabs"),
            (&[3], "Reset"),
        ]
    };
    if let Some((line, _)) = &ui.toast {
        ui.reset_hint.set(rect(0.0, 0.0, 0.0, 0.0));
        g.text(
            line,
            rect(24.0, 663.0, CANCEL.x - 40.0, 22.0),
            13.0,
            false,
            t.text,
            1.0,
            Align::Left,
        );
    }
    for (glyphs, label) in hints.iter().copied() {
        let start = x;
        for gi in glyphs {
            g.icon(
                Icon::Pad(ps, *gi),
                rect(x, 663.0, 22.0, 22.0),
                t.text,
                1.0,
                s,
            );
            x += 28.0;
        }
        let lw = g.measure(label, 13.0, false);
        g.text(
            label,
            rect(x, 663.0, lw + 4.0, 22.0),
            13.0,
            false,
            t.dim,
            DIM_ALPHA,
            Align::Left,
        );
        if label == "Reset" {
            ui.reset_hint.set(rect(start, 652.0, x + lw - start, 44.0));
        }
        x += lw + 16.0;
    }
    for (target, r, label, fill, ink) in [
        (Target::Cancel, CANCEL, "Cancel", t.tile, t.text),
        (Target::Apply, APPLY, "Apply", t.accent, t.on_accent),
    ] {
        g.fill(r, 12.0, fill, 1.0);
        if ui.focus == target {
            g.ring(r, 12.0, 2.0, t.ring, 1.0);
        }
        g.text(label, r, 15.0, true, ink, 1.0, Align::Center);
    }
    if let Some((pending, on_ok)) = &ui.confirm {
        draw_confirm(ui, g, pending, *on_ok);
    }
}

unsafe fn draw_confirm(ui: &Ui, g: &mut Gfx, pending: &Pending, on_reset: bool) {
    let (title, lines, ok_label) = confirm_text(pending);
    let t = ui.theme;
    g.fill(rect(0.0, 0.0, WIN_W, WIN_H), WIN_R, 0, 0.55);
    let c = CONFIRM_CARD;
    g.fill(c, 16.0, t.tile, 1.0);
    g.ring(c, 16.0, 1.0, t.line, t.line_alpha);
    g.text(
        title,
        rect(c.x + 24.0, c.y + 24.0, c.w - 48.0, 22.0),
        17.0,
        true,
        t.text,
        1.0,
        Align::Left,
    );
    for (i, line) in lines.iter().enumerate() {
        g.text(
            line,
            rect(c.x + 24.0, c.y + 58.0 + i as f32 * 20.0, c.w - 48.0, 18.0),
            13.0,
            false,
            t.dim,
            DIM_ALPHA,
            Align::Left,
        );
    }
    for (r, label, fill, ink, focused) in [
        (CONFIRM_CANCEL, "Cancel", t.hi, t.text, !on_reset),
        (CONFIRM_RESET, ok_label, t.accent, t.on_accent, on_reset),
    ] {
        g.fill(r, 12.0, fill, 1.0);
        if focused {
            g.ring(r, 12.0, 2.0, t.ring, 1.0);
        }
        g.text(label, r, 15.0, true, ink, 1.0, Align::Center);
    }
}

unsafe fn draw_card(ui: &Ui, g: &mut Gfx, card: Rect, label: &str, sel: bool, key: &str, i: usize) {
    let t = ui.theme;
    if sel {
        g.fill(card, 10.0, t.bg, 1.0);
        g.ring(card, 10.0, 1.5, t.text, 1.0);
    } else {
        g.ring(card, 10.0, 1.0, t.line, t.line_alpha);
    }
    let art_x = card.x + 8.0;
    let art_y = card.y + 8.0;
    let dark = super::vk_ui::is_dark_theme();
    let user = crate::config::keyboard_theme();
    let current = preview_palette(staged_style(ui), dark, user);
    match (key, i) {
        ("vk_style", _) => {
            let (style, layout) = style_card_art(i);
            draw_mini(g, art_x, art_y, &preview_palette(style, dark, user), layout);
        }
        (_, 0) => {
            let art = rect(art_x, art_y, CARD_W - 16.0, mini_height(&MINI_TV));
            g.fill(art, 6.0, t.tile, 1.0);
            g.icon(
                Icon::Monitor,
                rect(art.x + 56.0, art.y + 16.0, 22.0, 22.0),
                t.dim,
                DIM_ALPHA,
                ui.scale,
            );
            g.text(
                "Follows the screen",
                rect(art.x, art.y + 44.0, art.w, 14.0),
                11.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Center,
            );
        }
        (_, 1) => draw_mini(g, art_x, art_y, &current, &MINI_TV),
        _ => draw_mini(g, art_x, art_y, &current, &MINI_DESK),
    }
    let (c, a) = if sel {
        (t.text, 1.0)
    } else {
        (t.dim, DIM_ALPHA)
    };
    g.text(
        label,
        rect(card.x, card.y + 89.0, card.w, 16.0),
        13.0,
        sel,
        c,
        a,
        Align::Center,
    );
}

fn preview_palette(
    style: crate::config::VkStyle,
    dark: bool,
    user: crate::config::KeyboardTheme,
) -> VkPalette {
    super::vk_ui::themed_palette(vk_renderer::style_palette(style, dark), style, user)
}

fn staged_style(ui: &Ui) -> crate::config::VkStyle {
    let value = ui.items.iter().find_map(|it| match it.kind {
        Kind::Choice {
            key: "vk_style",
            options,
        } => options.get(it.value.max(0) as usize).map(|(_, v)| *v),
        _ => None,
    });
    crate::config::parse_vk_style(value)
}

unsafe fn draw_mini(g: &mut Gfx, x: f32, y: f32, pal: &VkPalette, l: &MiniLayout) {
    let art = rect(x, y, CARD_W - 16.0, mini_height(l));
    g.fill(art, 6.0, pal.bg, 1.0);
    g.ring(art, 6.0, 1.0, pal.panel_stroke, pal.panel_stroke_alpha);
    for (ri, row) in l.rows.iter().enumerate() {
        for (ki, ch) in row.chars().enumerate() {
            let k = mini_key_rect(art, l, ri, ki);
            let sel = ch == 'S';
            if sel {
                g.fill(
                    rect(k.x - 1.0, k.y + 1.0, k.w + 2.0, k.h + 2.0),
                    l.radius + 1.0,
                    0,
                    0.25,
                );
            } else if l.shadow {
                g.fill(rect(k.x, k.y + 1.0, k.w, k.h), l.radius, 0, DIM_ALPHA);
            }
            g.fill(k, l.radius, if sel { pal.accent } else { pal.key }, 1.0);
            if sel {
                g.ring(k, l.radius, 1.0, pal.sel_ring, 1.0);
            }
            g.text(
                &ch.to_string(),
                k,
                l.label_px,
                false,
                if sel { pal.sel_text } else { pal.text },
                1.0,
                Align::Center,
            );
        }
    }
}

unsafe fn draw_toggle(g: &Gfx, t: &Theme, r: Rect, on: bool) {
    g.fill(r, 14.0, if on { t.ring } else { t.toggle_off }, 1.0);
    let kx = if on { r.x + 21.0 } else { r.x + 3.0 };
    g.circle(kx + 11.0, r.y + 14.0, 11.0, 0x00FF_FFFF);
}

unsafe fn draw_row(ui: &Ui, g: &mut Gfx, target: Target, r: Rect) {
    let t = ui.theme;
    let s = ui.scale;
    let focused = ui.focus == target;
    let tile = |g: &Gfx, fill: u32| {
        g.fill(r, 12.0, if focused { t.hi } else { fill }, 1.0);
        if focused {
            g.ring(r, 12.0, 2.0, t.ring, 1.0);
        }
    };
    match target {
        Target::Item(i) => {
            let it = &ui.items[i];
            tile(g, t.tile);
            match it.kind {
                Kind::Slider { min, max, .. } => {
                    let text_w = r.w - 2.0 * ROW_PAD;
                    g.text(
                        it.label,
                        rect(r.x + ROW_PAD, r.y + 10.0, text_w, 17.0),
                        14.0,
                        false,
                        t.text,
                        1.0,
                        Align::Left,
                    );
                    g.text(
                        &slider_text(&it.kind, it.value),
                        rect(r.x + ROW_PAD, r.y + 10.0, text_w, 17.0),
                        13.0,
                        true,
                        t.accent,
                        1.0,
                        Align::Right,
                    );
                    let thumb = slider_thumb_x(r, min, max, it.value);
                    let ty = r.y + 41.0;
                    let track_end = r.x + ROW_PAD + TRACK_W;
                    g.fill(rect(thumb, ty, track_end - thumb, 6.0), 3.0, t.hi, 1.0);
                    if thumb > r.x + ROW_PAD {
                        g.fill(
                            rect(r.x + ROW_PAD, ty, thumb - r.x - ROW_PAD + THUMB / 2.0, 6.0),
                            3.0,
                            t.accent,
                            1.0,
                        );
                    }
                    g.circle(thumb + THUMB / 2.0, r.y + 44.0, THUMB / 2.0, 0x00FF_FFFF);
                }
                Kind::Choice { options, key } if is_card_picker(&it.kind) => {
                    g.text(
                        it.label,
                        rect(r.x + ROW_PAD, r.y + 12.0, r.w - 2.0 * ROW_PAD, 17.0),
                        14.0,
                        false,
                        t.text,
                        1.0,
                        Align::Left,
                    );
                    if let Some(caption) = it.caption {
                        g.text(
                            caption,
                            rect(r.x + ROW_PAD, r.y + 39.0, r.w - 2.0 * ROW_PAD, 15.0),
                            12.0,
                            false,
                            t.dim,
                            DIM_ALPHA,
                            Align::Left,
                        );
                    }
                    for (ci, (label, _)) in options.iter().enumerate() {
                        let card = card_rect(r, it.caption.is_some(), ci);
                        draw_card(ui, g, card, label, ci as i32 == it.value, key, ci);
                    }
                }
                Kind::Choice { options, .. } => {
                    g.text(
                        it.label,
                        rect(r.x + ROW_PAD, r.y + 12.0, r.w - 2.0 * ROW_PAD, 17.0),
                        14.0,
                        false,
                        t.text,
                        1.0,
                        Align::Left,
                    );
                    g.fill(segmented_rect(r), 10.0, t.bg, 1.0);
                    for (si, (label, _)) in options.iter().enumerate() {
                        let seg = segment_rect(r, options.len(), si);
                        let sel = si as i32 == it.value;
                        if sel {
                            g.fill(seg, 8.0, t.hi, 1.0);
                        }
                        let (c, a) = if sel {
                            (t.text, 1.0)
                        } else {
                            (t.dim, DIM_ALPHA)
                        };
                        g.text(label, seg, 13.0, sel, c, a, Align::Center);
                    }
                }
                Kind::Swatch { options } => {
                    g.text(
                        it.label,
                        rect(r.x + ROW_PAD, r.y, 120.0, r.h),
                        14.0,
                        false,
                        t.text,
                        1.0,
                        Align::Left,
                    );
                    for (si, (_, hex)) in options.iter().enumerate() {
                        let sw = swatch_rect(r, options.len(), si);
                        let (cx, cy) = (sw.x + 13.0, sw.y + 13.0);
                        if si as i32 == it.value {
                            g.brush.SetColor(&color(t.ring, 1.0));
                            g.rt.DrawEllipse(
                                &D2D1_ELLIPSE {
                                    point: D2D_POINT_2F { x: cx, y: cy },
                                    radiusX: 16.0,
                                    radiusY: 16.0,
                                },
                                &g.brush,
                                2.0,
                                None,
                            );
                        }
                        match hex.and_then(crate::config::parse_theme_color) {
                            Some(c) => g.circle(cx, cy, 13.0, c),
                            None => {
                                g.circle(cx, cy, 13.0, t.bg);
                                g.brush.SetColor(&color(t.dim, DIM_ALPHA));
                                g.rt.DrawLine(
                                    D2D_POINT_2F {
                                        x: cx - 7.0,
                                        y: cy + 7.0,
                                    },
                                    D2D_POINT_2F {
                                        x: cx + 7.0,
                                        y: cy - 7.0,
                                    },
                                    &g.brush,
                                    1.5,
                                    None,
                                );
                            }
                        }
                    }
                }
                _ => {
                    let tw = r.w - 2.0 * ROW_PAD - 46.0 - 16.0;
                    match it.caption {
                        Some(caption) => {
                            g.text(
                                it.label,
                                rect(r.x + ROW_PAD, r.y + 12.0, tw, 17.0),
                                14.0,
                                false,
                                t.text,
                                1.0,
                                Align::Left,
                            );
                            g.text(
                                caption,
                                rect(r.x + ROW_PAD, r.y + 32.0, tw, 15.0),
                                12.0,
                                false,
                                t.dim,
                                DIM_ALPHA,
                                Align::Left,
                            );
                        }
                        None => g.text(
                            it.label,
                            rect(r.x + ROW_PAD, r.y, tw, r.h),
                            14.0,
                            false,
                            t.text,
                            1.0,
                            Align::Left,
                        ),
                    }
                    draw_toggle(g, &t, toggle_rect(r), it.value != 0);
                }
            }
        }
        Target::Device => {
            tile(g, t.tile);
            g.circle(r.x + 42.0, r.y + 42.0, 26.0, t.bg);
            g.icon(
                Icon::Gamepad,
                rect(r.x + 29.0, r.y + 29.0, 26.0, 26.0),
                t.accent,
                1.0,
                s,
            );
            let text_w = r.w - 82.0 - 120.0;
            let name = if ui.live.connected {
                device_name(&ui.live.label)
            } else {
                "No controller".into()
            };
            g.text(
                &name,
                rect(r.x + 82.0, r.y + 23.5, text_w, 18.0),
                15.0,
                true,
                t.text,
                1.0,
                Align::Left,
            );
            g.text(
                &ui.live.caption(),
                rect(r.x + 82.0, r.y + 45.5, text_w, 15.0),
                12.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Left,
            );
            if let Some(b) = ui.live.battery {
                let label = format!("{}%", b.percent);
                let lw = g.measure(&label, 13.0, true);
                let cw = 10.0 + 18.0 + 6.0 + lw + 10.0;
                let chip = rect(r.x + r.w - ROW_PAD - cw, r.y + 27.0, cw, 30.0);
                g.fill(chip, 10.0, t.bg, 1.0);
                let tint = if b.percent <= 20 { RED } else { GREEN };
                g.icon(
                    Icon::Battery,
                    rect(chip.x + 10.0, chip.y + 6.0, 18.0, 18.0),
                    tint,
                    1.0,
                    s,
                );
                g.text(
                    &label,
                    rect(chip.x + 34.0, chip.y, lw + 4.0, chip.h),
                    13.0,
                    true,
                    t.text,
                    1.0,
                    Align::Left,
                );
            }
        }
        Target::Live => {
            g.fill(r, 12.0, t.tile, 1.0);
            let inner_w = r.w - 32.0;
            g.text(
                "Live input",
                rect(r.x + 16.0, r.y + 16.0, inner_w, 17.0),
                14.0,
                false,
                t.text,
                1.0,
                Align::Left,
            );
            g.text(
                "Press any button",
                rect(r.x + 16.0, r.y + 16.0, inner_w, 17.0),
                12.0,
                false,
                t.dim,
                DIM_ALPHA,
                Align::Right,
            );
            let col_w = (inner_w - 16.0) / 2.0;
            for (ci, (name, (sx, sy))) in [("LS", ui.live.input.ls), ("RS", ui.live.input.rs)]
                .into_iter()
                .enumerate()
            {
                let cx = r.x + 16.0 + ci as f32 * (col_w + 16.0) + col_w / 2.0;
                let cy = r.y + 47.0 + 52.0;
                g.circle(cx, cy, 52.0, t.bg);
                g.brush.SetColor(&color(t.hi, 1.0));
                g.rt.DrawEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x: cx, y: cy },
                        radiusX: 51.0,
                        radiusY: 51.0,
                    },
                    &g.brush,
                    2.0,
                    None,
                );
                g.rt.FillRectangle(&rect(cx - 40.0, cy - 1.0, 80.0, 2.0).d2d(), &g.brush);
                g.rt.FillRectangle(&rect(cx - 1.0, cy - 40.0, 2.0, 80.0).d2d(), &g.brush);
                let moved = sx.abs() > 0.0 || sy.abs() > 0.0;
                g.circle(
                    cx + sx.clamp(-1.0, 1.0) * 40.0,
                    cy - sy.clamp(-1.0, 1.0) * 40.0,
                    10.0,
                    if moved { t.accent } else { t.idle_dot },
                );
                g.text(
                    &format!("{name}  {sx:.2}, {sy:.2}"),
                    rect(cx - col_w / 2.0, r.y + 159.0, col_w, 15.0),
                    12.0,
                    false,
                    t.dim,
                    DIM_ALPHA,
                    Align::Center,
                );
            }
            let ps = ui.live.playstation();
            let bw = (inner_w - 9.0 * 6.0) / 10.0;
            for (bi, name) in PAD_BUTTONS.iter().enumerate() {
                let chip = rect(r.x + 16.0 + bi as f32 * (bw + 6.0), r.y + 188.0, bw, 34.0);
                let down = ui.live.input.pressed.iter().any(|p| p == name);
                g.fill(chip, 8.0, if down { t.accent } else { t.bg }, 1.0);
                g.icon(
                    Icon::Pad(ps, bi),
                    rect(chip.x + (chip.w - 22.0) / 2.0, chip.y + 6.0, 22.0, 22.0),
                    t.text,
                    if down || bi < 2 { 1.0 } else { DIM_ALPHA },
                    s,
                );
            }
        }
        Target::Rumble | Target::Lightbar | Target::Export | Target::Import | Target::ResetAll => {
            tile(g, t.tile);
            let (icon, label) = match target {
                Target::Rumble => (Icon::Vibrate, "Test rumble"),
                Target::Lightbar => (Icon::Lightbulb, "Lightbar"),
                Target::Export => (Icon::Download, "Export"),
                Target::Import => (Icon::Upload, "Import"),
                _ => (Icon::RotateCcw, "Reset all"),
            };
            let (icon_c, icon_a) = if target == Target::Lightbar && ui.led_open {
                (t.accent, 1.0)
            } else {
                (t.dim, DIM_ALPHA)
            };
            let lw = g.measure(label, 14.0, false);
            let x0 = r.x + (r.w - (18.0 + 10.0 + lw)) / 2.0;
            g.icon(icon, rect(x0, r.y + 15.0, 18.0, 18.0), icon_c, icon_a, s);
            g.text(
                label,
                rect(x0 + 28.0, r.y, lw + 4.0, r.h),
                14.0,
                false,
                t.text,
                1.0,
                Align::Left,
            );
        }
        Target::Cancel | Target::Apply => {}
    }
}

fn hit_caption(x: f32, y: f32) -> bool {
    (0.0..WIN_W).contains(&x) && (0.0..TITLE_H).contains(&y) && !CLOSE.inset(-7.0).contains(x, y)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_positions_round_trip_through_settings_scale() {
        let dz = slider(1, "", "cursor_deadzone", 0, 90, 100.0, 2);
        let Kind::Slider { div, decimals, .. } = dz.kind else {
            unreachable!()
        };
        let pos = (0.15f32 * div).round() as i32;
        assert_eq!(pos, 15);
        assert_eq!(format!("{:.*}", decimals, pos as f32 / div), "0.15");
        let ac = slider(1, "", "cursor_accel", 10, 50, 10.0, 1);
        let Kind::Slider { div, decimals, .. } = ac.kind else {
            unreachable!()
        };
        assert_eq!(format!("{:.*}", decimals, 20 as f32 / div), "2.0");
        assert_eq!(slider_text(&ac.kind, 20), "2.0");
    }

    #[test]
    fn vocabulary_controls_map_to_one_setting() {
        let fmt = |c: bool, w: &str| crate::config::format_vocabulary(&staged_vocabulary(c, w));
        assert_eq!(fmt(true, "herdr, MyProject"), "coding, herdr, MyProject");
        assert_eq!(fmt(true, " "), "coding");
        assert_eq!(fmt(false, "herdr,, MyProject "), "herdr, MyProject");
        assert_eq!(fmt(false, ""), "off");
        assert_eq!(fmt(false, "coding, herdr"), "herdr");
        assert_eq!(fmt(false, "a=b"), "a b");
        for raw in ["coding", "off", "herdr, MyProject", "coding, herdr, Jonas"] {
            let v = crate::config::parse_vocabulary(Some(&format!("vocabulary = {raw}")));
            assert_eq!(fmt(v.coding, &v.words.join(", ")), raw);
        }
    }

    #[test]
    fn choice_options_round_trip_through_config_parsers() {
        use crate::config::{parse_vk_display, parse_vk_style, VkDisplay, VkStyle};
        let styles = [VkStyle::Normal, VkStyle::Mono, VkStyle::Modern];
        for (i, (_, v)) in VK_STYLE_OPTIONS.iter().enumerate() {
            assert_eq!(parse_vk_style(Some(v)), styles[i]);
        }
        let displays = [VkDisplay::Auto, VkDisplay::Tv, VkDisplay::Desk];
        for (i, (_, v)) in VK_DISPLAY_OPTIONS.iter().enumerate() {
            assert_eq!(parse_vk_display(Some(v)), displays[i]);
        }
        let kind = Kind::Choice {
            key: "vk_display",
            options: VK_DISPLAY_OPTIONS,
        };
        assert_eq!(choice_index(&kind, "desk"), 2);
        assert_eq!(choice_index(&kind, "bogus"), 0);
    }

    #[test]
    fn every_item_sits_on_a_rail_tab() {
        assert!(items().iter().all(|i| i.tab < TABS.len()));
    }

    fn ys(tab: usize) -> Vec<(f32, f32)> {
        rows(tab, &items(), false)
            .into_iter()
            .map(|(_, r)| (r.y, r.h))
            .collect()
    }

    #[test]
    fn layout_matches_the_design_coordinates() {
        assert_eq!(tab_rect(0), rect(24.0, 68.0, 180.0, 52.0));
        assert_eq!(tab_rect(3), rect(24.0, 248.0, 180.0, 52.0));
        assert_eq!(
            ys(0),
            vec![
                (68.0, 59.0),
                (135.0, 59.0),
                (202.0, 52.0),
                (262.0, 52.0),
                (322.0, 52.0),
                (382.0, 52.0),
                (442.0, 52.0),
                (502.0, 59.0),
                (569.0, 48.0),
                (569.0, 48.0),
                (569.0, 48.0),
            ]
        );
        let general = rows(0, &items(), false);
        assert_eq!(
            general[8],
            (Target::Export, rect(224.0, 569.0, 160.0, 48.0))
        );
        assert_eq!(
            general[9],
            (Target::Import, rect(392.0, 569.0, 160.0, 48.0))
        );
        assert_eq!(
            general[10],
            (Target::ResetAll, rect(560.0, 569.0, 160.0, 48.0))
        );
        assert!(569.0 + 48.0 <= HINT_Y);
        assert_eq!(
            ys(1),
            vec![
                (68.0, 63.0),
                (139.0, 63.0),
                (210.0, 63.0),
                (281.0, 63.0),
                (352.0, 63.0),
                (423.0, 63.0),
                (494.0, 59.0),
            ]
        );
        assert_eq!(
            ys(2),
            vec![
                (68.0, 164.0),
                (240.0, 189.0),
                (437.0, 59.0),
                (504.0, 59.0),
                (571.0, 59.0),
            ]
        );
        assert!(571.0 + 59.0 <= HINT_Y);
        let style = rect(224.0, 68.0, 496.0, 164.0);
        assert_eq!(card_rect(style, false, 0), rect(240.0, 107.0, 149.0, 113.0));
        assert_eq!(card_rect(style, false, 1).x, 397.0);
        assert_eq!(card_rect(style, false, 2).x, 555.0);
        let display = rect(224.0, 240.0, 496.0, 189.0);
        assert_eq!(
            card_rect(display, true, 0),
            rect(240.0, 304.0, 149.0, 113.0)
        );
        assert_eq!(toggle_rect(rect(224.0, 437.0, 496.0, 59.0)).y, 453.0);
        assert_eq!(mini_height(&MINI_DESK), 50.0);
        assert_eq!(mini_height(&MINI_TV), 73.0);
        assert_eq!(mini_height(&MINI_MODERN), 73.0);
        let art = rect(248.0, 115.0, 133.0, 50.0);
        let s_key = mini_key_rect(art, &MINI_DESK, 1, 1);
        assert!((s_key.x - 284.3).abs() < 0.01 && s_key.y == 141.0 && s_key.w == 29.0);
        let last = mini_key_rect(art, &MINI_MODERN, 2, 3);
        assert!(last.x + last.w <= art.x + art.w);
        assert_eq!(style_card_art(2).0, crate::config::VkStyle::Modern);
        let ctl = rows(3, &items(), false);
        assert_eq!(ctl[0], (Target::Device, rect(224.0, 68.0, 496.0, 84.0)));
        assert_eq!(ctl[1], (Target::Live, rect(224.0, 160.0, 496.0, 238.0)));
        assert_eq!(ctl[2], (Target::Rumble, rect(224.0, 406.0, 244.0, 48.0)));
        assert_eq!(ctl[3], (Target::Lightbar, rect(476.0, 406.0, 244.0, 48.0)));
        assert!(rows(0, &items(), false)
            .iter()
            .filter(|(t, _)| matches!(t, Target::Item(_)))
            .all(|(_, r)| r.x == CONTENT_X && r.w == CONTENT_W));
        let row = rect(224.0, 68.0, 496.0, 52.0);
        assert_eq!(toggle_rect(row), rect(658.0, 80.0, 46.0, 28.0));
        let choice = rect(224.0, 186.0, 496.0, 89.0);
        assert_eq!(segmented_rect(choice), rect(240.0, 225.0, 464.0, 38.0));
        assert_eq!(segment_rect(choice, 3, 0).h, 32.0);
        assert_eq!(CANCEL.x + CANCEL.w + 10.0, APPLY.x);
        assert_eq!(APPLY.x + APPLY.w, 720.0);
    }

    #[test]
    fn slider_value_maps_to_thumb_position_and_back() {
        let row = rect(224.0, 68.0, 496.0, 63.0);
        assert_eq!(slider_thumb_x(row, 1, 40, 1), 240.0);
        assert_eq!(slider_thumb_x(row, 1, 40, 40), 240.0 + TRACK_W - THUMB);
        for pos in 1..=40 {
            let x = slider_thumb_x(row, 1, 40, pos) + THUMB / 2.0;
            assert_eq!(slider_pos_at(row, 1, 40, x), pos);
        }
        assert_eq!(slider_pos_at(row, 0, 90, 0.0), 0);
        assert_eq!(slider_pos_at(row, 0, 90, 10_000.0), 90);
        assert_eq!(slider_fraction(5, 5, 5), 0.0);
    }

    #[test]
    fn focus_moves_through_rows_then_buttons() {
        let items = items();
        let order = focus_order(0, &items, false);
        assert_eq!(order.len(), 13);
        assert_eq!(order[0], Target::Item(0));
        assert_eq!(order[8], Target::Export);
        assert_eq!(order[11], Target::Cancel);
        assert_eq!(
            step_focus(&order, Target::Item(0), Nav::Up),
            Target::Item(0)
        );
        assert_eq!(
            step_focus(&order, Target::Item(0), Nav::Down),
            Target::Item(1)
        );
        assert_eq!(
            step_focus(&order, Target::Item(7), Nav::Down),
            Target::Export
        );
        assert_eq!(
            step_focus(&order, Target::Export, Nav::Right),
            Target::Import
        );
        assert_eq!(
            step_focus(&order, Target::Import, Nav::Right),
            Target::ResetAll
        );
        assert_eq!(
            step_focus(&order, Target::ResetAll, Nav::Right),
            Target::ResetAll
        );
        assert_eq!(
            step_focus(&order, Target::Import, Nav::Down),
            Target::Cancel
        );
        assert_eq!(step_focus(&order, Target::Import, Nav::Up), Target::Item(7));
        assert_eq!(
            step_focus(&order, Target::Cancel, Nav::Right),
            Target::Apply
        );
        assert_eq!(step_focus(&order, Target::Apply, Nav::Left), Target::Cancel);
        assert_eq!(step_focus(&order, Target::Apply, Nav::Up), Target::ResetAll);
        assert_eq!(
            step_focus(&order, Target::Cancel, Nav::Down),
            Target::Cancel
        );

        let kb = focus_order(2, &items, false);
        assert_eq!(kb.len(), 7);
        assert!(matches!(kb[0], Target::Item(i) if items[i].label == "Keyboard style"));
        assert!(matches!(kb[1], Target::Item(i) if items[i].label == "Display"));
        assert!(matches!(kb[4], Target::Item(i) if matches!(items[i].kind, Kind::Vocabulary)));

        let ctl = focus_order(3, &items, false);
        assert_eq!(
            ctl,
            vec![
                Target::Device,
                Target::Rumble,
                Target::Lightbar,
                Target::Cancel,
                Target::Apply
            ]
        );
        assert_eq!(step_focus(&ctl, Target::Device, Nav::Down), Target::Rumble);
        assert_eq!(
            step_focus(&ctl, Target::Rumble, Nav::Right),
            Target::Lightbar
        );
        assert_eq!(step_focus(&ctl, Target::Rumble, Nav::Down), Target::Cancel);
        assert_eq!(step_focus(&ctl, Target::Lightbar, Nav::Up), Target::Device);
    }

    #[test]
    fn reset_defaults_match_the_config_defaults() {
        let expect: &[(&str, i32)] = &[
            ("Gamepad cursor", 1),
            ("Sleep during games", 1),
            ("Guide-only in games (legacy)", 0),
            ("Controller hints on the sign-in screen", 1),
            ("Guide / PS opens warmUP when closed", 1),
            ("Voice typing", 1),
            ("Pause gamepad input", 0),
            ("Sign-in and lock screen only", 0),
            ("Cursor speed", 15),
            ("Cursor acceleration", 20),
            ("Cursor dead zone", 15),
            ("Cursor smoothing", 0),
            ("Scroll speed", 5),
            ("Scroll acceleration", 20),
            ("Natural scrolling", 0),
            ("Keyboard style", 0),
            ("Display", 0),
            ("Floating layout", 1),
            ("Compact size", 0),
            ("Coding vocabulary", 1),
            ("Color", 0),
            ("Lightbar effect", 0),
        ];
        let items = items();
        assert_eq!(items.len(), expect.len());
        for (it, (label, value)) in items.iter().zip(expect) {
            assert_eq!(it.label, *label);
            assert_eq!(default_value(it), *value, "{label}");
            let (lo, hi) = value_range(&it.kind);
            assert!((lo..=hi).contains(&default_value(it)), "{label}");
        }
        let g = crate::config::GamepadSettings::default();
        assert_eq!(
            slider_text(&items[8].kind, default_value(&items[8])),
            format!("{:.0}", g.cursor_speed)
        );
        assert_eq!(
            slider_text(&items[10].kind, default_value(&items[10])),
            format!("{:.2}", g.cursor_deadzone)
        );
        assert_eq!(
            led_choice(default_value(&items[20]), default_value(&items[21])),
            (Some("#B6A0FF"), "solid")
        );
    }

    #[test]
    fn previews_follow_the_user_keyboard_theme() {
        use crate::config::{KeyboardTheme, VkStyle};
        for style in [VkStyle::Normal, VkStyle::Mono, VkStyle::Modern] {
            for dark in [true, false] {
                let plain = preview_palette(style, dark, KeyboardTheme::default());
                assert_eq!(plain, vk_renderer::style_palette(style, dark));
                let themed = preview_palette(
                    style,
                    dark,
                    KeyboardTheme {
                        accent: Some(0x00_30_3B_FF),
                        ..KeyboardTheme::default()
                    },
                );
                assert_ne!(themed.accent, plain.accent, "{style:?} {dark}");
                assert_eq!(themed.sel_ring == plain.sel_ring, false, "{style:?} {dark}");
            }
        }
        let keyed = preview_palette(
            VkStyle::Normal,
            true,
            KeyboardTheme {
                key: Some(0x00_11_22_33),
                bg: Some(0x00_01_02_03),
                ..KeyboardTheme::default()
            },
        );
        assert_eq!((keyed.key, keyed.bg), (0x00_11_22_33, 0x00_01_02_03));
    }

    #[test]
    fn confirm_cards_and_file_names() {
        let (title, lines, ok) = confirm_text(&Pending::ResetAll);
        assert_eq!((title, ok), ("Reset everything?", "Reset"));
        assert_eq!(lines.len(), 2);
        let plan = crate::config::ImportPlan {
            accepted: vec![("vk_style".into(), "mono".into()); 3],
            rejected: vec!["x".into()],
        };
        let (title, lines, ok) = confirm_text(&Pending::Import(plan));
        assert_eq!((title, ok), ("Import settings?", "Import"));
        assert_eq!(lines[0], "3 settings will be replaced.");
        assert_eq!(lines[2], "1 line was skipped.");
        assert_eq!(
            export_file_name(2026, 10, 7),
            "warmup-companion-settings-20261007.ini"
        );
        assert!(CONFIRM_CANCEL.y + CONFIRM_CANCEL.h <= CONFIRM_CARD.y + CONFIRM_CARD.h);
        assert_eq!(
            CONFIRM_RESET.x + CONFIRM_RESET.w,
            CONFIRM_CARD.x + CONFIRM_CARD.w - 24.0
        );
    }

    #[test]
    fn transport_label_uses_the_connection_not_the_battery() {
        assert_eq!(transport_label(Some(true)), Some("USB"));
        assert_eq!(transport_label(Some(false)), Some("Bluetooth"));
        assert_eq!(transport_label(None), None);
    }

    #[test]
    fn lightbar_choice_maps_to_led_settings() {
        assert_eq!(led_choice(0, 0), (Some("#B6A0FF"), "solid"));
        assert_eq!(led_choice(2, 1), (Some("#FF3B30"), "breathing"));
        assert_eq!(led_choice(7, 2), (Some("#FF2D92"), "rainbow"));
        assert_eq!(led_choice(8, 2), (None, "off"));
        assert_eq!(led_swatch_index(false, (0xB6, 0xA0, 0xFF)), 0);
        assert_eq!(led_swatch_index(false, (0x0A, 0x84, 0xFF)), 6);
        assert_eq!(led_swatch_index(false, (1, 2, 3)), 0);
        assert_eq!(led_swatch_index(true, (0xFF, 0xFF, 0xFF)), 8);
        for (_, hex) in LED_SWATCHES {
            if let Some(h) = hex {
                assert!(crate::config::parse_theme_color(h).is_some(), "{h}");
            }
        }
        use crate::led_engine::LedEffect;
        let names: Vec<&str> = [LedEffect::Solid, LedEffect::Breathing, LedEffect::Rainbow]
            .into_iter()
            .map(led_effect_name)
            .collect();
        let values: Vec<&str> = LED_EFFECT_OPTIONS.iter().map(|(_, v)| *v).collect();
        assert_eq!(names, values);
        assert_eq!(led_effect_name(LedEffect::Off), "solid");
    }

    #[test]
    fn lightbar_rows_open_below_the_action_tiles() {
        let items = items();
        let closed = rows(3, &items, false);
        assert_eq!(closed.len(), 4);
        let open = rows(3, &items, true);
        assert_eq!(open.len(), 6);
        assert_eq!(open[4].1, rect(224.0, 462.0, 496.0, 52.0));
        assert_eq!(open[5].1, rect(224.0, 522.0, 496.0, 89.0));
        assert!(open[5].1.y + open[5].1.h <= HINT_Y);
        let order = focus_order(3, &items, true);
        assert_eq!(order[3], open[4].0);
        let sw = swatch_rect(open[4].1, LED_SWATCHES.len(), LED_SWATCHES.len() - 1);
        assert_eq!(sw.x + sw.w, 720.0 - ROW_PAD);
    }

    #[test]
    fn app_icon_picks_the_closest_ico_entry() {
        let size = |want| {
            let (off, _) = ico_entry(APP_ICO, want).unwrap();
            (0..u16::from_le_bytes([APP_ICO[4], APP_ICO[5]]) as usize)
                .map(|i| &APP_ICO[6 + 16 * i..22 + 16 * i])
                .find(|e| u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize == off)
                .map(|e| if e[0] == 0 { 256 } else { e[0] as u32 })
                .unwrap()
        };
        assert_eq!(size(16), 16);
        assert_eq!(size(32), 32);
        assert_eq!(size(20), 24);
        assert_eq!(size(48), 48);
        assert_eq!(size(300), 256);
        assert_eq!(ico_entry(&[0, 0], 32), None);
    }

    #[test]
    fn live_input_summary_parses_buttons_and_sticks() {
        let p = parse_input("A LB L(0.42,-0.10) R(0.00,0.50)");
        assert_eq!(p.pressed, vec!["A".to_string(), "LB".to_string()]);
        assert_eq!(p.ls, (0.42, -0.10));
        assert_eq!(p.rs, (0.0, 0.5));
        assert_eq!(parse_input("(idle)"), PadInput::default());
    }

    #[test]
    fn shadow_is_dark_under_the_card_and_clear_at_the_edge() {
        let a = shadow_alpha(200, 200, &[rect(50.0, 50.0, 100.0, 100.0)], 10.0, 8.0);
        assert!(a[100 * 200 + 100] > 120);
        assert_eq!(a[0], 0);
    }
}

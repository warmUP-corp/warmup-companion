//! Controller Center: a JoyXoff-style native Settings dialog on the logged-in
//! desktop. Plain light window, big right-hand tab rail with Segoe MDL2 icons,
//! standard checkboxes / trackbars / buttons on the left, Ok · Cancel · Apply
//! bottom-right. All controls are Win32 common controls (v6, see app.manifest),
//! so the look is exactly Windows' own — no custom drawing.
//!
//! Edits are staged in the controls and written to settings.ini on Ok/Apply via
//! `config::set_gamepad_setting`; the gamepad loop re-reads that file live.
//! Runs on the shared [`super::desktop_window`] band (spawned lazily on first
//! open, kept alive after close).
//!
// ponytail: no dirty tracking — Apply is always enabled and rewrites only the
// keys whose control differs from the file. Add change flags if that matters.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetStockObject, InvalidateRect, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, DEFAULT_CHARSET, DEFAULT_PITCH, FW_NORMAL, HDC, HFONT, TRANSPARENT,
    WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, BST_CHECKED, ICC_BAR_CLASSES, INITCOMMONCONTROLSEX, TBM_SETPOS,
    TBM_SETRANGE, TBM_SETTICFREQ, TBS_AUTOTICKS, TBS_BOTTOM,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, KillTimer, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, BM_GETCHECK,
    BM_SETCHECK, BS_AUTOCHECKBOX, HMENU, HWND_TOP, SM_CXSCREEN, SM_CYSCREEN, SWP_NOZORDER, SW_HIDE,
    SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORBTN,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_HSCROLL, WM_SETFONT, WM_TIMER, WS_CAPTION, WS_CHILD,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};

// Missing from windows 0.58's bindings.
const TBM_GETPOS: u32 = 0x0400;
const SS_RIGHT: u32 = 0x0002;
const SS_NOTIFY: u32 = 0x0100;

const WINDOW_CLASS: PCWSTR = w!("WarmupControllerCenter");
/// Client size in logical (96-dpi) px, matching the reference dialog.
const CLIENT_W: i32 = 744;
const CLIENT_H: i32 = 660;
const CONTENT_X: i32 = 38;
const CONTENT_W: i32 = 470;
const RAIL_LABEL_X: i32 = 540;
const RAIL_LABEL_W: i32 = 148;
const RAIL_ICON_X: i32 = 694;
const RAIL_Y: i32 = 62;
const RAIL_STEP: i32 = 60;
const BTN_X: i32 = 562;
const BTN_W: i32 = 166;
const BTN_H: i32 = 26;
const TIMER_ID: usize = 31;
const TIMER_MS: u32 = 250;

const TEXT_DARK: u32 = 0x00333333;
const TEXT_GREY: u32 = 0x00A6A6A6;

const ID_OK: usize = 1;
const ID_CANCEL: usize = 2;
const ID_APPLY: usize = 3;
const ID_EDIT_INI: usize = 4;
const ID_TAB_BASE: usize = 100;
const ID_ITEM_BASE: usize = 200;

/// (label, Segoe MDL2 Assets glyph)
const TABS: [(&str, &str); 4] = [
    ("General", "\u{E713}"),
    ("Mouse", "\u{E962}"),
    ("Keyboard", "\u{E765}"),
    ("Controller", "\u{E7FC}"),
];

/// One staged setting. Checkboxes write `on_val`/`off_val`; trackbars write
/// `pos / div` with `decimals` places.
enum Kind {
    Check {
        key: &'static str,
        on_val: &'static str,
        off_val: &'static str,
    },
    /// Pause/resume the userland poll (runtime flag + persisted marker, like the tray).
    PausePoll,
    Slider {
        key: &'static str,
        min: i32,
        max: i32,
        div: f32,
        decimals: usize,
    },
}

struct Item {
    tab: usize,
    label: &'static str,
    kind: Kind,
    /// Main control (checkbox or trackbar).
    hwnd: HWND,
    /// Trackbar title + value readout (hidden with the item).
    extra: Vec<HWND>,
}

fn item(tab: usize, label: &'static str, kind: Kind) -> Item {
    Item {
        tab,
        label,
        kind,
        hwnd: HWND::default(),
        extra: Vec::new(),
    }
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

fn items() -> Vec<Item> {
    vec![
        // General
        check(
            0,
            "Enable gamepad cursor (sticks move the mouse, A/B click)",
            "cursor_enabled",
        ),
        check(
            0,
            "Sleep during games (only the Guide button is watched)",
            "sleep_on_game",
        ),
        check(0, "Guide-only in games (legacy)", "auto_stop_on_game"),
        item(0, "Pause gamepad input", Kind::PausePoll),
        // Mouse
        slider(1, "Cursor speed", "cursor_speed", 1, 40, 1.0, 0),
        slider(1, "Cursor acceleration", "cursor_accel", 10, 50, 10.0, 1),
        slider(1, "Cursor dead zone", "cursor_deadzone", 0, 90, 100.0, 2),
        slider(1, "Cursor smoothing", "cursor_smoothing", 0, 90, 100.0, 2),
        slider(1, "Scroll speed", "scroll_speed", 1, 20, 1.0, 0),
        slider(1, "Scroll acceleration", "scroll_accel", 10, 50, 10.0, 1),
        check(
            1,
            "Natural scrolling (invert scroll direction)",
            "natural_scroll",
        ),
        // Keyboard
        item(
            2,
            "Floating layout (card in the middle instead of a docked bar)",
            Kind::Check {
                key: "vk_mode",
                on_val: "floating",
                off_val: "docked",
            },
        ),
        item(
            2,
            "Compact size (smaller docked bar)",
            Kind::Check {
                key: "vk_bar_scale",
                on_val: "0.8",
                off_val: "1.0",
            },
        ),
    ]
}

/// Current value from settings: `(checked, trackbar position)`.
fn current(item: &Item) -> (bool, i32) {
    let s = crate::config::gamepad_settings();
    match &item.kind {
        Kind::Check { key, .. } => (
            match *key {
                "cursor_enabled" => s.cursor_enabled,
                "sleep_on_game" => s.sleep_on_game,
                "auto_stop_on_game" => s.auto_stop_on_game,
                "natural_scroll" => s.natural_scroll,
                "vk_mode" => {
                    crate::config::vk_layout_mode() == crate::config::VkLayoutMode::Floating
                }
                "vk_bar_scale" => crate::config::vk_bar_scale() < 1.0,
                _ => false,
            },
            0,
        ),
        Kind::PausePoll => (crate::gamepad_backend::userland_poll_paused(), 0),
        Kind::Slider { key, div, .. } => {
            let v = match *key {
                "cursor_speed" => s.cursor_speed,
                "cursor_accel" => s.cursor_accel,
                "cursor_deadzone" => s.cursor_deadzone,
                "cursor_smoothing" => s.cursor_smoothing,
                "scroll_speed" => s.scroll_speed,
                "scroll_accel" => s.scroll_accel,
                _ => 0.0,
            };
            (false, (v * div).round() as i32)
        }
    }
}

fn set(key: &str, value: &str) {
    if let Err(e) = crate::config::set_gamepad_setting(key, value) {
        crate::install::log_line(&format!("controller center: set {key}={value}: {e}"));
    }
}

// ---- window plumbing -------------------------------------------------------

struct Ui {
    hwnd: HWND,
    tab: usize,
    items: Vec<Item>,
    /// "Edit settings file" button (General tab only).
    edit: HWND,
    /// Live controller readout (Controller tab only).
    live: HWND,
    fonts: Vec<HFONT>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// Rail statics `(label, icon)` per tab + selected tab. Kept outside `UI`
    /// because `WM_CTLCOLORSTATIC` arrives synchronously from inside
    /// SetWindowText/ShowWindow while `UI` is mutably borrowed.
    static RAIL: RefCell<(usize, Vec<(HWND, HWND)>)> = const { RefCell::new((0, Vec::new())) };
    static COMCTL_READY: Cell<bool> = const { Cell::new(false) };
}

static THREAD: OnceLock<Mutex<Option<DesktopWindowThread>>> = OnceLock::new();

struct CenterApp;

impl DesktopApp for CenterApp {
    const THREAD_NAME: &'static str = "warmup-controller-center";
    const CLASS_NAME: PCWSTR = WINDOW_CLASS;
    const BG_COLOR: u32 = 0x00FFFFFF;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = wndproc;

    fn on_show(&mut self, _lparam: LPARAM) {
        ui_show();
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

/// Open (or raise) the Controller Center. Safe to call from any thread.
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
        let _ = t.show(LPARAM(0));
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create a child control at logical coordinates scaled by `scale`.
#[allow(clippy::too_many_arguments)]
unsafe fn child(
    parent: HWND,
    class: PCWSTR,
    text: &str,
    style: u32,
    id: usize,
    (x, y, w, h): (i32, i32, i32, i32),
    scale: f32,
    font: HFONT,
) -> HWND {
    let t = wide(text);
    let px = |v: i32| (v as f32 * scale).round() as i32;
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR(t.as_ptr()),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
        px(x),
        px(y),
        px(w),
        px(h),
        parent,
        HMENU(id as *mut _),
        None,
        None,
    )
    .unwrap_or_default();
    SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    hwnd
}

unsafe fn font(pt: f32, dpi: u32, face: PCWSTR) -> HFONT {
    let height = -((pt * dpi as f32 / 72.0).round() as i32);
    CreateFontW(
        height,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        0,
        0,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        face,
    )
}

fn ui_show() {
    if let Some(hwnd) = UI.with(|u| u.borrow().as_ref().map(|u| u.hwnd)) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    unsafe {
        if !COMCTL_READY.replace(true) {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            };
            let _ = InitCommonControlsEx(&icc);
        }
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("Controller Center"),
            style,
            0,
            0,
            CLIENT_W,
            CLIENT_H,
            None,
            HMENU::default(),
            windows::Win32::Foundation::HINSTANCE(instance.0),
            None,
        ) else {
            crate::install::log_line("controller center: CreateWindowExW failed");
            return;
        };
        // Size the client to CLIENT_W x CLIENT_H at this monitor's DPI, centred.
        let dpi = GetDpiForWindow(hwnd);
        let scale = dpi as f32 / 96.0;
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: (CLIENT_W as f32 * scale).round() as i32,
            bottom: (CLIENT_H as f32 * scale).round() as i32,
        };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOP,
            (GetSystemMetrics(SM_CXSCREEN) - ww) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - wh) / 2,
            ww,
            wh,
            SWP_NOZORDER,
        );

        let body = font(9.0, dpi, w!("Segoe UI"));
        let tab_font = font(20.0, dpi, w!("Segoe UI"));
        let icon_font = font(22.0, dpi, w!("Segoe MDL2 Assets"));

        // Right rail: big right-aligned names, icon beside each.
        let mut tabs = Vec::new();
        for (i, (name, glyph)) in TABS.iter().enumerate() {
            let y = RAIL_Y + i as i32 * RAIL_STEP;
            let id = ID_TAB_BASE + i;
            let label = child(
                hwnd,
                w!("STATIC"),
                name,
                SS_RIGHT | SS_NOTIFY,
                id,
                (RAIL_LABEL_X, y, RAIL_LABEL_W, 36),
                scale,
                tab_font,
            );
            let icon = child(
                hwnd,
                w!("STATIC"),
                glyph,
                SS_NOTIFY,
                id,
                (RAIL_ICON_X, y + 2, 40, 36),
                scale,
                icon_font,
            );
            tabs.push((label, icon));
        }

        // Content items, laid out top-down per tab.
        let mut items = items();
        let mut next_y = [50i32; 4];
        for (i, it) in items.iter_mut().enumerate() {
            let y = &mut next_y[it.tab];
            let id = ID_ITEM_BASE + i;
            match it.kind {
                Kind::Check { .. } | Kind::PausePoll => {
                    it.hwnd = child(
                        hwnd,
                        w!("BUTTON"),
                        it.label,
                        BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
                        id,
                        (CONTENT_X, *y, CONTENT_W, 24),
                        scale,
                        body,
                    );
                    *y += 40;
                }
                Kind::Slider { min, max, .. } => {
                    let title = child(
                        hwnd,
                        w!("STATIC"),
                        it.label,
                        0,
                        0,
                        (CONTENT_X, *y, CONTENT_W, 20),
                        scale,
                        body,
                    );
                    it.hwnd = child(
                        hwnd,
                        w!("msctls_trackbar32"),
                        "",
                        TBS_AUTOTICKS | TBS_BOTTOM | WS_TABSTOP.0,
                        id,
                        (CONTENT_X, *y + 22, CONTENT_W - 70, 30),
                        scale,
                        body,
                    );
                    let value = child(
                        hwnd,
                        w!("STATIC"),
                        "",
                        0,
                        0,
                        (CONTENT_X + CONTENT_W - 60, *y + 27, 60, 20),
                        scale,
                        body,
                    );
                    SendMessageW(
                        it.hwnd,
                        TBM_SETRANGE,
                        WPARAM(1),
                        LPARAM(((min as u32) | ((max as u32) << 16)) as isize),
                    );
                    SendMessageW(
                        it.hwnd,
                        TBM_SETTICFREQ,
                        WPARAM(((max - min) / 10).max(1) as usize),
                        LPARAM(0),
                    );
                    it.extra = vec![title, value];
                    *y += 66;
                }
            }
        }

        // General tab: shortcut to the ini for everything not surfaced here.
        let edit = child(
            hwnd,
            w!("BUTTON"),
            "Edit settings file",
            WS_TABSTOP.0,
            ID_EDIT_INI,
            (CONTENT_X, next_y[0] + 10, 146, BTN_H),
            scale,
            body,
        );
        // Controller tab: live state, refreshed by timer.
        let live = child(
            hwnd,
            w!("STATIC"),
            "",
            0,
            0,
            (CONTENT_X, 50, CONTENT_W, 140),
            scale,
            body,
        );
        for (id, label, y) in [
            (ID_OK, "Ok", 540),
            (ID_CANCEL, "Cancel", 576),
            (ID_APPLY, "Apply", 619),
        ] {
            child(
                hwnd,
                w!("BUTTON"),
                label,
                WS_TABSTOP.0,
                id,
                (BTN_X, y, BTN_W, BTN_H),
                scale,
                body,
            );
        }

        RAIL.with(|r| *r.borrow_mut() = (0, tabs));
        let ui = Ui {
            hwnd,
            tab: 0,
            items,
            edit,
            live,
            fonts: vec![body, tab_font, icon_font],
        };
        sync_from_config(&ui);
        apply_tab(&ui);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn ui_hide() {
    if let Some(ui) = UI.with(|u| u.borrow_mut().take()) {
        unsafe {
            let _ = KillTimer(ui.hwnd, TIMER_ID);
            let _ = DestroyWindow(ui.hwnd);
            for f in ui.fonts {
                let _ = DeleteObject(f);
            }
        }
    }
}

unsafe fn checked(hwnd: HWND) -> bool {
    SendMessageW(hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 as u32 == BST_CHECKED.0
}

/// Push settings.ini values into the controls.
fn sync_from_config(ui: &Ui) {
    for it in &ui.items {
        let (on, pos) = current(it);
        unsafe {
            match &it.kind {
                Kind::Check { .. } | Kind::PausePoll => {
                    SendMessageW(
                        it.hwnd,
                        BM_SETCHECK,
                        WPARAM(if on { BST_CHECKED.0 as usize } else { 0 }),
                        LPARAM(0),
                    );
                }
                Kind::Slider { .. } => {
                    SendMessageW(it.hwnd, TBM_SETPOS, WPARAM(1), LPARAM(pos as isize));
                    update_value_label(it);
                }
            }
        }
    }
}

unsafe fn update_value_label(it: &Item) {
    if let (Kind::Slider { div, decimals, .. }, Some(value)) = (&it.kind, it.extra.get(1)) {
        let pos = SendMessageW(it.hwnd, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as f32;
        let text = wide(&format!("{:.*}", decimals, pos / div));
        let _ = SetWindowTextW(*value, PCWSTR(text.as_ptr()));
    }
}

/// Write every control whose value differs from settings.ini.
fn apply(ui: &Ui) {
    for it in &ui.items {
        let (on, pos) = current(it);
        unsafe {
            match &it.kind {
                Kind::Check {
                    key,
                    on_val,
                    off_val,
                } => {
                    let c = checked(it.hwnd);
                    if c != on {
                        set(key, if c { on_val } else { off_val });
                    }
                }
                Kind::PausePoll => {
                    let c = checked(it.hwnd);
                    if c != on {
                        crate::gamepad_backend::set_userland_poll_paused(c);
                        let _ = crate::config::write_userland_poll_paused(c);
                    }
                }
                Kind::Slider {
                    key, div, decimals, ..
                } => {
                    let p = SendMessageW(it.hwnd, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32;
                    if p != pos {
                        set(key, &format!("{:.*}", decimals, p as f32 / div));
                    }
                }
            }
        }
    }
}

fn apply_tab(ui: &Ui) {
    let vis = |on: bool| if on { SW_SHOW } else { SW_HIDE };
    unsafe {
        for it in &ui.items {
            let _ = ShowWindow(it.hwnd, vis(it.tab == ui.tab));
            for h in &it.extra {
                let _ = ShowWindow(*h, vis(it.tab == ui.tab));
            }
        }
        let _ = ShowWindow(ui.edit, vis(ui.tab == 0));
        let _ = ShowWindow(ui.live, vis(ui.tab == 3));
        let rail = RAIL.with(|r| {
            r.borrow_mut().0 = ui.tab;
            r.borrow().1.clone()
        });
        for (label, icon) in rail {
            let _ = InvalidateRect(label, None, true);
            let _ = InvalidateRect(icon, None, true);
        }
    }
}

unsafe fn refresh_live(ui: &Ui) {
    if ui.tab != 3 {
        return;
    }
    let s = crate::debug_state::snapshot();
    let text = if s.connected {
        let name = if s.name.is_empty() {
            "controller"
        } else {
            s.name.as_str()
        };
        let input = if s.input.is_empty() {
            "(idle)"
        } else {
            s.input.as_str()
        };
        format!(
            "Connected: {name}\r\n\r\nLive input: {input}\r\n\r\n{}",
            s.detail
        )
    } else {
        "No controller connected.\r\n\r\nPlug in or pair a controller and its live button and stick state shows up here.".to_string()
    };
    let _ = SetWindowTextW(ui.live, PCWSTR(wide(&text).as_ptr()));
}

fn with_ui(f: impl FnOnce(&mut Ui)) {
    UI.with(|u| {
        if let Some(ui) = u.borrow_mut().as_mut() {
            f(ui);
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            // White dialog ground for every label/checkbox; rail names are dark
            // when selected, grey otherwise (the reference look).
            let hdc = HDC(wparam.0 as *mut _);
            let ctl = HWND(lparam.0 as *mut _);
            let color = RAIL.with(|r| {
                let (tab, ctls) = &*r.borrow();
                match ctls.iter().position(|(l, ic)| *l == ctl || *ic == ctl) {
                    Some(i) if i == *tab => TEXT_DARK,
                    Some(_) => TEXT_GREY,
                    None => 0x00000000,
                }
            });
            SetTextColor(hdc, COLORREF(color));
            SetBkMode(hdc, TRANSPARENT);
            LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            match id {
                ID_OK => {
                    with_ui(|ui| apply(ui));
                    ui_hide();
                }
                ID_CANCEL => ui_hide(),
                ID_APPLY => with_ui(|ui| {
                    apply(ui);
                    sync_from_config(ui);
                }),
                ID_EDIT_INI => crate::tray::edit_settings(),
                t if (ID_TAB_BASE..ID_TAB_BASE + TABS.len()).contains(&t) => with_ui(|ui| {
                    ui.tab = t - ID_TAB_BASE;
                    apply_tab(ui);
                    refresh_live(ui);
                }),
                _ => {}
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            let bar = HWND(lparam.0 as *mut _);
            with_ui(|ui| {
                if let Some(it) = ui.items.iter().find(|i| i.hwnd == bar) {
                    update_value_label(it);
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            with_ui(|ui| refresh_live(ui));
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
        // 0.15 dead zone at div=100 -> pos 15 -> "0.15"; 2.0 accel at div=10 -> pos 20 -> "2.0".
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
    }

    #[test]
    fn every_item_sits_on_a_rail_tab() {
        assert!(items().iter().all(|i| i.tab < TABS.len()));
    }
}

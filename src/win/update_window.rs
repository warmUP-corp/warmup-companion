use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, KillTimer,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, HMENU, HTCAPTION, HTCLIENT,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WM_CLOSE, WM_DESTROY,
    WM_DPICHANGED, WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCHITTEST, WM_SETTINGCHANGE, WM_TIMER,
    WS_EX_APPWINDOW, WS_EX_LAYERED, WS_POPUP,
};

use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE;

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::mono_ui::*;
use super::tray_menu::lucide;
use crate::updater::{self, Phase, Snapshot};

const CLASS: PCWSTR = w!("WarmupUpdateWindow");
const TIMER_ID: usize = 41;
const TIMER_MS: u32 = 33;
const WIN_W: f32 = 520.0;
const WIN_R: f32 = 24.0;
const PAD_X: f32 = 72.0;
const PAD_TOP: f32 = 48.0;
const PAD_BOTTOM: f32 = 96.0;
const SHADOW_DY: f32 = 24.0;
const SHADOW_SIGMA: f32 = 30.0;
const TITLE_H: f32 = 64.0;
const SIDE: f32 = 24.0;
const INNER: f32 = WIN_W - 2.0 * SIDE;
const CONTENT_Y: f32 = 142.0;
const FOOTER_H: f32 = 84.0;
const CLOSE: Rect = rect(478.0, 25.0, 18.0, 18.0);
const GREEN: u32 = 0x0058D130;
const RED: u32 = 0x006169FF;
const RED_TEXT: u32 = 0x00AEB4FF;
const STEP_H: f32 = 40.0;
const TOGGLE_ROW_H: f32 = 64.0;
const LINE_H: f32 = 18.0;
const EASE: f32 = 0.18;
const SPIN_DEG: f32 = 14.0;
const SHIMMER_W: f32 = 90.0;
const SHIMMER_SPEED: f32 = 7.0;

const CIRCLE_CHECK: &str =
    lucide!(r#"<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>"#);
const LOADER: &str = lucide!(r#"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"#);
const CIRCLE: &str = lucide!(r#"<circle cx="12" cy="12" r="10"/>"#);
const SHIELD: &str = lucide!(
    r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#
);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Install,
    Later,
    Cancel,
    Close,
    Retry,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Toggle,
    Button(usize),
}

enum Step {
    Done,
    Active,
    Todo,
}

struct Ui {
    hwnd: HWND,
    scale: f32,
    snap: Snapshot,
    auto_check: bool,
    focus: Target,
    theme: Theme,
    gfx: Option<Gfx>,
    playstation: bool,
    spin: u32,
    shown: f32,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static THREAD: OnceLock<Mutex<Option<DesktopWindowThread>>> = OnceLock::new();

struct UpdateApp;

impl DesktopApp for UpdateApp {
    const THREAD_NAME: &'static str = "warmup-update-window";
    const CLASS_NAME: PCWSTR = CLASS;
    const BG_COLOR: u32 = 0x001E1C1C;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = wndproc;

    fn on_show(&mut self, _lparam: LPARAM) {
        ui_show();
    }

    fn on_hide(&mut self) {
        ui_hide();
    }
}

pub(crate) fn show() {
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if guard.is_none() {
        match desktop_window::spawn(UpdateApp) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("update window: spawn failed: {e}"));
                return;
            }
        }
    }
    if let Some(t) = guard.as_ref() {
        let _ = t.show(LPARAM(0));
    }
}

fn pad_is_playstation() -> bool {
    let s = crate::debug_state::snapshot();
    !s.connected || s.name.is_empty() || super::vk_renderer::is_playstation_label(&s.name)
}

fn buttons(snap: &Snapshot) -> Vec<(&'static str, Act, bool)> {
    match snap.phase {
        Phase::Available => vec![
            ("Later", Act::Later, false),
            ("Install update", Act::Install, true),
        ],
        Phase::Downloading { .. } => vec![("Cancel", Act::Cancel, false)],
        Phase::Verifying | Phase::Installing => Vec::new(),
        Phase::Failed(_) => vec![("Close", Act::Close, false), ("Try again", Act::Retry, true)],
        Phase::Idle | Phase::Checking | Phase::UpToDate => vec![("Close", Act::Close, true)],
    }
}

fn hints(snap: &Snapshot) -> Vec<(usize, &'static str)> {
    match snap.phase {
        Phase::Available => vec![(0, "Install"), (1, "Later")],
        Phase::Downloading { .. } => vec![(0, "Select"), (1, "Hide")],
        Phase::Verifying | Phase::Installing => Vec::new(),
        Phase::Failed(_) => vec![(0, "Select"), (1, "Close")],
        Phase::UpToDate => vec![(0, "Select"), (1, "Close")],
        Phase::Idle | Phase::Checking => vec![(1, "Close")],
    }
}

fn focus_order(snap: &Snapshot) -> Vec<Target> {
    let mut order = Vec::new();
    if snap.phase == Phase::UpToDate {
        order.push(Target::Toggle);
    }
    order.extend((0..buttons(snap).len()).map(Target::Button));
    order
}

fn default_focus(snap: &Snapshot) -> Target {
    buttons(snap)
        .iter()
        .position(|b| b.2)
        .map(Target::Button)
        .unwrap_or(Target::Toggle)
}

fn notes_h(n: usize) -> f32 {
    if n == 0 {
        0.0
    } else {
        16.0 + 14.0 + 10.0 + n as f32 * 20.0 + (n - 1) as f32 * 10.0 + 16.0
    }
}

fn error_lines(snap: &Snapshot) -> Vec<String> {
    match &snap.phase {
        Phase::Failed(e) => e.lines().take(4).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn body_end(snap: &Snapshot) -> f32 {
    let content = match &snap.phase {
        Phase::Available => {
            let n = snap.release.as_ref().map_or(0, |r| r.notes.len());
            let card = notes_h(n);
            card + if card > 0.0 { 20.0 } else { 0.0 } + 16.0
        }
        Phase::Downloading { .. } | Phase::Verifying | Phase::Installing => {
            32.0 + 20.0 + 12.0 + 3.0 * STEP_H
        }
        Phase::UpToDate => TOGGLE_ROW_H,
        Phase::Failed(_) => 16.0 + 14.0 + 6.0 + error_lines(snap).len() as f32 * LINE_H + 16.0,
        Phase::Idle | Phase::Checking => -20.0,
    };
    CONTENT_Y + content + 24.0
}

fn win_h(snap: &Snapshot) -> f32 {
    body_end(snap) + FOOTER_H
}

fn surface_px(scale: f32, h: f32) -> (i32, i32) {
    (
        ((WIN_W + 2.0 * PAD_X) * scale).ceil() as i32,
        ((h + PAD_TOP + PAD_BOTTOM) * scale).ceil() as i32,
    )
}

fn button_rects(g: &mut Gfx, snap: &Snapshot) -> Vec<Rect> {
    let y = body_end(snap) + 16.0;
    let mut x = WIN_W - SIDE;
    let mut out: Vec<Rect> = buttons(snap)
        .iter()
        .rev()
        .map(|(label, _, _)| {
            let w = unsafe { g.measure(label, 15.0, true) }.ceil() + 44.0;
            x -= w;
            let r = rect(x, y, w, 44.0);
            x -= 10.0;
            r
        })
        .collect();
    out.reverse();
    out
}

fn toggle_row() -> Rect {
    rect(SIDE, CONTENT_Y, INNER, TOGGLE_ROW_H)
}

fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_048_576.0)
}

fn version(snap: &Snapshot) -> String {
    snap.release
        .as_ref()
        .map(|r| r.version.clone())
        .unwrap_or_default()
}

fn hero(snap: &Snapshot) -> (String, String) {
    let v = version(snap);
    let cur = updater::CURRENT;
    match &snap.phase {
        Phase::Available => {
            let size = snap.release.as_ref().map_or(0, |r| r.size);
            let sub = if size > 0 {
                format!(
                    "You have v{cur}. The download is {} MB and installs in the background.",
                    mb(size)
                )
            } else {
                format!("You have v{cur}.")
            };
            (format!("v{v} is available"), sub)
        }
        Phase::Downloading { .. } => (
            format!("Downloading v{v}"),
            "Keep playing — nothing changes until the download is verified.".into(),
        ),
        Phase::Verifying => (
            format!("Verifying v{v}"),
            "Making sure the download matches the release checksum.".into(),
        ),
        Phase::Installing => (
            format!("Installing v{v}"),
            "The controller keyboard restarts for a few seconds. Your mouse keeps working.".into(),
        ),
        Phase::UpToDate => (
            "You're up to date".into(),
            format!(
                "v{cur} is the latest release. Checked {}.",
                snap.last_checked.as_deref().unwrap_or("just now")
            ),
        ),
        Phase::Failed(_) if snap.release.is_some() => (
            "The update couldn't be installed".into(),
            format!("Nothing was changed. You're still on v{cur}."),
        ),
        Phase::Failed(_) => (
            "Couldn't check for updates".into(),
            "Check your connection and try again.".into(),
        ),
        Phase::Idle | Phase::Checking => (
            "Checking for updates".into(),
            "Asking GitHub for the latest release.".into(),
        ),
    }
}

fn pill(snap: &Snapshot) -> String {
    let v = version(snap);
    match snap.phase {
        Phase::Available if !v.is_empty() => format!("v{} \u{2192} v{v}", updater::CURRENT),
        Phase::Downloading { .. } | Phase::Verifying | Phase::Installing => format!("v{v}"),
        _ => format!("v{}", updater::CURRENT),
    }
}

fn steps(snap: &Snapshot) -> [(&'static str, Step, String); 3] {
    match &snap.phase {
        Phase::Downloading { got, .. } => [
            ("Download", Step::Active, format!("{} MB", mb(*got))),
            ("Verify SHA-256", Step::Todo, String::new()),
            ("Install and restart", Step::Todo, String::new()),
        ],
        Phase::Verifying => [
            (
                "Download",
                Step::Done,
                format!("{} MB", mb(snap.release.as_ref().map_or(0, |r| r.size))),
            ),
            ("Verify SHA-256", Step::Active, String::new()),
            ("Install and restart", Step::Todo, String::new()),
        ],
        _ => [
            (
                "Download",
                Step::Done,
                format!("{} MB", mb(snap.release.as_ref().map_or(0, |r| r.size))),
            ),
            ("Verify SHA-256", Step::Done, "match".into()),
            ("Install and restart", Step::Active, String::new()),
        ],
    }
}

fn ui_show() {
    super::controller_center::set_menu_owns_pad(true);
    if let Some(hwnd) = UI.with(|u| u.borrow().as_ref().map(|u| u.hwnd)) {
        with_ui(|ui| {
            ui.playstation = pad_is_playstation();
            refresh(ui, true);
        });
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(instance) = GetModuleHandleW(None) else {
            super::controller_center::set_menu_owns_pad(false);
            return;
        };
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_APPWINDOW,
            CLASS,
            w!("warmUP Companion update"),
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
            crate::install::log_line("update window: CreateWindowExW failed");
            super::controller_center::set_menu_owns_pad(false);
            return;
        };
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let snap = updater::snapshot();
        let ui = Ui {
            hwnd,
            scale,
            focus: default_focus(&snap),
            snap,
            auto_check: crate::config::gamepad_settings().update_check,
            theme: current_theme(),
            gfx: None,
            playstation: pad_is_playstation(),
            spin: 0,
            shown: 0.0,
        };
        UI.with(|u| *u.borrow_mut() = Some(ui));
        with_ui(|ui| {
            place(ui);
            paint(ui);
        });
        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn ui_hide() {
    super::controller_center::set_menu_owns_pad(false);
    if let Some(ui) = UI.with(|u| u.borrow_mut().take()) {
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

fn place(ui: &Ui) {
    let s = ui.scale;
    let h = win_h(&ui.snap);
    let (w, sh) = surface_px(s, h);
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(MonitorFromWindow(ui.hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
        let work = info.rcWork;
        let x = (work.left + work.right - (WIN_W * s) as i32) / 2 - (PAD_X * s) as i32;
        let y = (work.top + work.bottom - (h * s) as i32) / 2 - (PAD_TOP * s) as i32;
        let _ = SetWindowPos(ui.hwnd, None, x, y, w, sh, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn refresh(ui: &mut Ui, force: bool) -> bool {
    let snap = updater::snapshot();
    if !force && snap == ui.snap {
        return false;
    }
    let resized = win_h(&snap) != win_h(&ui.snap);
    let phase_changed = std::mem::discriminant(&snap.phase) != std::mem::discriminant(&ui.snap.phase);
    ui.snap = snap;
    if phase_changed || !focus_order(&ui.snap).contains(&ui.focus) {
        ui.focus = default_focus(&ui.snap);
    }
    if resized || force {
        place(ui);
    }
    true
}

fn activate(ui: &mut Ui, act: Act) -> bool {
    match act {
        Act::Install => updater::start_install(),
        Act::Cancel => updater::cancel(),
        Act::Retry => {
            if ui.snap.release.is_some() {
                updater::start_install()
            } else {
                updater::start_check()
            }
        }
        Act::Later | Act::Close => return true,
    }
    refresh(ui, false);
    false
}

fn press(ui: &mut Ui, target: Target) -> bool {
    match target {
        Target::Toggle => {
            ui.auto_check = !ui.auto_check;
            let _ = crate::config::set_gamepad_setting(
                "update_check",
                if ui.auto_check { "true" } else { "false" },
            );
            false
        }
        Target::Button(i) => match buttons(&ui.snap).get(i) {
            Some((_, act, _)) => activate(ui, *act),
            None => false,
        },
    }
}

fn back(ui: &mut Ui) -> bool {
    match ui.snap.phase {
        Phase::Downloading { .. } | Phase::Verifying | Phase::Installing => true,
        _ => buttons(&ui.snap)
            .iter()
            .find(|b| matches!(b.1, Act::Later | Act::Close))
            .is_some_and(|b| activate(ui, b.1)),
    }
}

fn step_focus(ui: &mut Ui, forward: bool) {
    let order = focus_order(&ui.snap);
    if order.is_empty() {
        return;
    }
    let i = order.iter().position(|t| *t == ui.focus).unwrap_or(0);
    let n = order.len();
    ui.focus = order[if forward { (i + 1) % n } else { (i + n - 1) % n }];
}

fn tick(ui: &mut Ui) -> (bool, bool) {
    if !super::controller_center::owns_pad() {
        super::controller_center::set_menu_owns_pad(true);
    }
    let mut dirty = refresh(ui, false);
    let target = progress_target(&ui.snap);
    if (target - ui.shown).abs() > 0.001 {
        ui.shown += (target - ui.shown) * EASE;
        if (target - ui.shown).abs() < 0.002 {
            ui.shown = target;
        }
        dirty = true;
    }
    if matches!(
        ui.snap.phase,
        Phase::Checking | Phase::Downloading { .. } | Phase::Verifying | Phase::Installing
    ) {
        ui.spin = ui.spin.wrapping_add(1);
        dirty = true;
    }
    for (button, pressed) in super::controller_center::take_menu_pad_edges() {
        if !pressed {
            continue;
        }
        dirty = true;
        let close = match button {
            "A" => press(ui, ui.focus),
            "B" => back(ui),
            "LEFT" | "UP" => {
                step_focus(ui, false);
                false
            }
            "RIGHT" | "DOWN" => {
                step_focus(ui, true);
                false
            }
            _ => false,
        };
        if close {
            return (dirty, true);
        }
    }
    (dirty, false)
}

fn key_down(ui: &mut Ui, vk: u16) -> bool {
    match vk {
        v if v == VK_RETURN.0 || v == VK_SPACE.0 => press(ui, ui.focus),
        v if v == VK_ESCAPE.0 => back(ui),
        v if v == VK_LEFT.0 || v == VK_UP.0 => {
            step_focus(ui, false);
            false
        }
        v if v == VK_RIGHT.0 || v == VK_DOWN.0 || v == VK_TAB.0 => {
            step_focus(ui, true);
            false
        }
        _ => false,
    }
}

fn click(ui: &mut Ui, x: f32, y: f32) -> bool {
    if CLOSE.inset(-7.0).contains(x, y) {
        return true;
    }
    if ui.snap.phase == Phase::UpToDate && toggle_row().contains(x, y) {
        ui.focus = Target::Toggle;
        return press(ui, Target::Toggle);
    }
    let rects = match ui.gfx.as_mut() {
        Some(g) => button_rects(g, &ui.snap),
        None => Vec::new(),
    };
    match rects.iter().position(|r| r.contains(x, y)) {
        Some(i) => {
            ui.focus = Target::Button(i);
            press(ui, Target::Button(i))
        }
        None => false,
    }
}

fn design_point(ui: &Ui, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x / ui.scale - PAD_X, y / ui.scale - PAD_TOP)
}

fn paint(ui: &mut Ui) {
    let mut gfx = match ui.gfx.take() {
        Some(g) => g,
        None => match unsafe { Gfx::new() } {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("update window: gfx: {e}"));
                return;
            }
        },
    };
    if let Err(e) = unsafe { render(ui, &mut gfx) } {
        crate::install::log_line(&format!("update window: render: {e}"));
        return;
    }
    ui.gfx = Some(gfx);
}

unsafe fn render(ui: &Ui, g: &mut Gfx) -> Result<(), String> {
    let s = ui.scale;
    let h = win_h(&ui.snap);
    g.begin(surface_px(s, h))?;
    g.draw_shadow(
        &[rect(PAD_X * s, (PAD_TOP + SHADOW_DY) * s, WIN_W * s, h * s)],
        WIN_R * s,
        SHADOW_SIGMA * s,
    );
    g.transform(s, PAD_X * s, PAD_TOP * s);
    draw(ui, g, h);
    g.present(ui.hwnd, None)
}

unsafe fn fit(g: &mut Gfx, text: &str, w: f32, px: f32, bold: bool) -> String {
    if g.measure(text, px, bold) <= w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let cut: String = chars.iter().collect::<String>() + "\u{2026}";
        if g.measure(&cut, px, bold) <= w {
            return cut;
        }
    }
    String::new()
}

unsafe fn draw(ui: &Ui, g: &mut Gfx, h: f32) {
    let t = ui.theme;
    let s = ui.scale;
    let snap = &ui.snap;
    let card = rect(0.0, 0.0, WIN_W, h);
    g.fill(card, WIN_R, t.bg, 1.0);
    g.ring(card, WIN_R, 1.0, t.line, t.line_alpha);

    g.icon(Icon::Logo, rect(SIDE, 25.0, 18.0, 18.0), t.text, 1.0, s);
    g.text("Update", rect(52.0, 25.0, 200.0, 18.0), 15.0, true, t.text, 1.0, Align::Left);
    let pill_text = pill(snap);
    let tw = g.measure(&pill_text, 12.0, false);
    let pw = tw + 24.0;
    let pr = rect(CLOSE.x - 14.0 - pw, 20.0, pw, 28.0);
    g.fill(pr, 14.0, t.tile, 1.0);
    g.text(&pill_text, pr, 12.0, false, t.dim, DIM_ALPHA, Align::Center);
    g.icon(Icon::Close, CLOSE, t.dim, DIM_ALPHA, s);

    let (title, sub) = hero(snap);
    let title = fit(g, &title, INNER, 22.0, true);
    g.text(&title, rect(SIDE, 72.0, INNER, 28.0), 22.0, true, t.text, 1.0, Align::Left);
    let sub = fit(g, &sub, INNER, 13.0, false);
    g.text(&sub, rect(SIDE, 103.0, INNER, 19.0), 13.0, false, t.dim, DIM_ALPHA, Align::Left);

    match &snap.phase {
        Phase::Available => draw_available(ui, g),
        Phase::Downloading { got, total } => {
            draw_progress(
                ui,
                g,
                &format!("{} of {} MB", mb(*got), mb(*total)),
                &format!("{} %", (ui.shown * 100.0).round() as i32),
            );
            draw_steps(ui, g);
        }
        Phase::Verifying => {
            draw_progress(ui, g, "Checking SHA-256\u{2026}", "100 %");
            draw_steps(ui, g);
        }
        Phase::Installing => {
            let sha = snap
                .release
                .as_ref()
                .map(|r| r.sha.chars().take(8).collect::<String>())
                .unwrap_or_default();
            draw_progress(ui, g, &format!("Verified {sha}\u{2026}"), "Done");
            draw_steps(ui, g);
        }
        Phase::UpToDate => draw_toggle_row(ui, g),
        Phase::Failed(_) => draw_error(ui, g),
        Phase::Idle | Phase::Checking => {}
    }

    let fy = body_end(snap);
    g.fill(rect(0.0, fy, WIN_W, 1.0), 0.0, t.line, t.line_alpha);
    let mut x = SIDE;
    let hy = fy + 27.0;
    if snap.phase == Phase::Installing {
        g.text("About 10 seconds", rect(x, hy, 200.0, 22.0), 13.0, false, t.dim, DIM_ALPHA, Align::Left);
    }
    for (glyph, label) in hints(snap) {
        g.icon(Icon::Pad(ui.playstation, glyph), rect(x, hy, 22.0, 22.0), t.text, 1.0, s);
        x += 28.0;
        let lw = g.measure(label, 13.0, false);
        g.text(label, rect(x, hy, lw + 4.0, 22.0), 13.0, false, t.dim, DIM_ALPHA, Align::Left);
        x += lw + 16.0;
    }
    let rects = button_rects(g, snap);
    for (i, ((label, _, primary), r)) in buttons(snap).iter().zip(rects).enumerate() {
        g.fill(r, 12.0, if *primary { t.accent } else { t.tile }, 1.0);
        if ui.focus == Target::Button(i) {
            g.ring(r, 12.0, 2.0, t.ring, 1.0);
        }
        g.text(
            label,
            r,
            15.0,
            true,
            if *primary { t.on_accent } else { t.text },
            1.0,
            Align::Center,
        );
    }
}

unsafe fn draw_available(ui: &Ui, g: &mut Gfx) {
    let t = ui.theme;
    let s = ui.scale;
    let notes = ui.snap.release.as_ref().map(|r| r.notes.clone()).unwrap_or_default();
    let mut y = CONTENT_Y;
    if !notes.is_empty() {
        let card = rect(SIDE, y, INNER, notes_h(notes.len()));
        g.fill(card, 12.0, t.tile, 1.0);
        g.text("WHAT'S NEW", rect(SIDE + 16.0, y + 16.0, 200.0, 14.0), 11.0, true, t.dim, DIM_ALPHA, Align::Left);
        let mut by = y + 40.0;
        for note in &notes {
            g.circle(SIDE + 19.0, by + 10.0, 3.0, t.accent);
            let w = INNER - 48.0;
            let line = fit(g, note, w, 14.0, false);
            g.text(&line, rect(SIDE + 32.0, by, w, 20.0), 14.0, false, t.text, 1.0, Align::Left);
            by += 30.0;
        }
        y += card.h + 20.0;
    }
    g.icon(Icon::Svg(SHIELD), rect(SIDE, y + 1.0, 14.0, 14.0), t.dim, DIM_ALPHA, s);
    g.text(
        "From github.com/warmUP-corp \u{00B7} SHA-256 verified before install",
        rect(SIDE + 22.0, y, INNER - 22.0, 16.0),
        12.0,
        false,
        t.dim,
        DIM_ALPHA,
        Align::Left,
    );
}

fn progress_target(snap: &Snapshot) -> f32 {
    match snap.phase {
        Phase::Downloading { got, total } if total > 0 => (got as f32 / total as f32).min(1.0),
        Phase::Verifying | Phase::Installing => 1.0,
        _ => 0.0,
    }
}

fn base_transform(ui: &Ui) -> Matrix3x2 {
    let s = ui.scale;
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: PAD_X * s,
        M32: PAD_TOP * s,
    }
}

unsafe fn draw_spinner(ui: &Ui, g: &mut Gfx, r: Rect) {
    let angle = (ui.spin as f32 * SPIN_DEG) % 360.0;
    let base = base_transform(ui);
    g.rt.SetTransform(&(Matrix3x2::rotation(angle, r.x + r.w / 2.0, r.y + r.h / 2.0) * base));
    g.icon(Icon::Svg(LOADER), r, ui.theme.accent, 1.0, ui.scale);
    g.rt.SetTransform(&base);
}

unsafe fn draw_progress(ui: &Ui, g: &mut Gfx, left: &str, right: &str) {
    let t = ui.theme;
    let track = rect(SIDE, CONTENT_Y, INNER, 8.0);
    g.fill(track, 4.0, t.hi, 1.0);
    if ui.shown > 0.0 {
        let bar = rect(SIDE, CONTENT_Y, (INNER * ui.shown).max(8.0), 8.0);
        g.fill(bar, 4.0, t.accent, 1.0);
        g.rt.PushAxisAlignedClip(&bar.d2d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let travel = bar.w + SHIMMER_W;
        let x = bar.x - SHIMMER_W + (ui.spin as f32 * SHIMMER_SPEED) % travel;
        let w = SHIMMER_W / 5.0;
        for (i, a) in [0.08f32, 0.16, 0.24, 0.16, 0.08].iter().enumerate() {
            g.fill(rect(x + i as f32 * w, bar.y, w, bar.h), 0.0, 0x00FFFFFF, *a);
        }
        g.rt.PopAxisAlignedClip();
    }
    let ly = CONTENT_Y + 16.0;
    g.text(left, rect(SIDE, ly, INNER / 2.0, 16.0), 12.0, false, t.dim, DIM_ALPHA, Align::Left);
    g.text(right, rect(SIDE + INNER / 2.0, ly, INNER / 2.0, 16.0), 12.0, false, t.dim, DIM_ALPHA, Align::Right);
}

unsafe fn draw_steps(ui: &Ui, g: &mut Gfx) {
    let t = ui.theme;
    let s = ui.scale;
    let top = CONTENT_Y + 32.0 + 20.0;
    let card = rect(SIDE, top, INNER, 12.0 + 3.0 * STEP_H);
    g.fill(card, 12.0, t.tile, 1.0);
    for (i, (label, step, detail)) in steps(&ui.snap).iter().enumerate() {
        let y = top + 6.0 + i as f32 * STEP_H;
        let ir = rect(SIDE + 16.0, y + 11.0, 18.0, 18.0);
        match step {
            Step::Done => g.icon(Icon::Svg(CIRCLE_CHECK), ir, GREEN, 1.0, s),
            Step::Active => draw_spinner(ui, g, ir),
            Step::Todo => g.icon(Icon::Svg(CIRCLE), ir, t.dim, 0.3, s),
        }
        let (c, a) = match step {
            Step::Todo => (t.dim, DIM_ALPHA),
            _ => (t.text, 1.0),
        };
        g.text(label, rect(SIDE + 46.0, y, 260.0, STEP_H), 14.0, false, c, a, Align::Left);
        if !detail.is_empty() {
            g.text(detail, rect(SIDE + INNER - 176.0, y, 160.0, STEP_H), 12.0, false, t.dim, DIM_ALPHA, Align::Right);
        }
    }
}

unsafe fn draw_toggle_row(ui: &Ui, g: &mut Gfx) {
    let t = ui.theme;
    let r = toggle_row();
    let focused = ui.focus == Target::Toggle;
    g.fill(r, 12.0, if focused { t.hi } else { t.tile }, 1.0);
    if focused {
        g.ring(r, 12.0, 2.0, t.ring, 1.0);
    }
    g.text("Check automatically", rect(r.x + 16.0, r.y + 12.0, 320.0, 20.0), 14.0, false, t.text, 1.0, Align::Left);
    let cap = fit(g, "Once a day. Only the version number is fetched from GitHub.", r.w - 110.0, 12.0, false);
    g.text(&cap, rect(r.x + 16.0, r.y + 34.0, r.w - 110.0, 16.0), 12.0, false, t.dim, DIM_ALPHA, Align::Left);
    let tr = rect(r.x + r.w - 16.0 - 46.0, r.y + (r.h - 28.0) / 2.0, 46.0, 28.0);
    g.fill(tr, 14.0, if ui.auto_check { t.accent } else { t.toggle_off }, 1.0);
    let kx = if ui.auto_check { tr.x + 35.0 } else { tr.x + 14.0 };
    g.circle(kx - 3.0, tr.y + 14.0, 11.0, 0x00FFFFFF);
}

unsafe fn draw_error(ui: &Ui, g: &mut Gfx) {
    let t = ui.theme;
    let lines = error_lines(&ui.snap);
    let card = rect(SIDE, CONTENT_Y, INNER, 16.0 + 14.0 + 6.0 + lines.len() as f32 * LINE_H + 16.0);
    g.fill(card, 12.0, t.tile, 1.0);
    g.ring(card, 12.0, 1.0, RED, 0.25);
    g.text("DETAILS", rect(SIDE + 16.0, CONTENT_Y + 16.0, 200.0, 14.0), 11.0, true, t.dim, DIM_ALPHA, Align::Left);
    let mut y = CONTENT_Y + 36.0;
    for line in &lines {
        let shown = fit(g, line, INNER - 32.0, 12.0, false);
        g.text(&shown, rect(SIDE + 16.0, y, INNER - 32.0, LINE_H), 12.0, false, RED_TEXT, 1.0, Align::Left);
        y += LINE_H;
    }
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
                caption = (0.0..WIN_W).contains(&x)
                    && (0.0..TITLE_H).contains(&y)
                    && !CLOSE.inset(-7.0).contains(x, y);
            });
            LRESULT(if caption { HTCAPTION } else { HTCLIENT } as isize)
        }
        WM_LBUTTONDOWN => {
            let mut close = false;
            with_ui(|ui| {
                let (x, y) = design_point(ui, lparam);
                close = click(ui, x, y);
                paint(ui);
            });
            if close {
                ui_hide();
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let mut close = false;
            with_ui(|ui| {
                close = key_down(ui, wparam.0 as u16);
                paint(ui);
            });
            if close {
                ui_hide();
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            let mut close = false;
            with_ui(|ui| {
                let (dirty, c) = tick(ui);
                close = c;
                if dirty {
                    paint(ui);
                }
            });
            if close {
                ui_hide();
            }
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
            with_ui(|ui| {
                ui.scale = ((wparam.0 >> 16) & 0xffff).max(96) as f32 / 96.0;
                if let Some(g) = ui.gfx.as_mut() {
                    g.icons.clear();
                }
                place(ui);
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

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, LoadCursorW, PostMessageW,
    RegisterClassW, SetForegroundWindow, SetTimer, ShowWindow, HMENU, IDC_ARROW, SW_SHOW,
    WA_INACTIVE, WM_ACTIVATE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::*;

const CLASS: windows::core::PCWSTR = w!("WarmupTrayMenu");
const TIMER_ID: usize = 7;

thread_local! {
    static CLASS_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn monitor_at(pt: POINT) -> (f32, Rect) {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let r = info.rcWork;
        (
            dx.max(96) as f32 / 96.0,
            rect(
                r.left as f32,
                r.top as f32,
                (r.right - r.left) as f32,
                (r.bottom - r.top) as f32,
            ),
        )
    }
}

pub(crate) fn tray_spec(owner: HWND, at: POINT, root: Vec<Entry>, build: &str) -> Spec {
    Spec {
        root,
        root_w: ROOT_W,
        sub_w: SUB_W,
        title: "warmUP Companion".to_string(),
        subtitle: build.to_string(),
        zoom: 1.0,
        placement: Placement::Anchor(at),
        handler: Box::new(move |action| match action {
            Action::Run(cmd) => Reply::Close(Some(Box::new(move || unsafe {
                let _ = PostMessageW(owner, WM_COMMAND, WPARAM(cmd), LPARAM(0));
            }))),
            _ => Reply::Stay,
        }),
        refresh: None,
        on_close: None,
    }
}

pub(crate) fn show(owner: HWND, at: POINT, root: Vec<Entry>, build: &str) {
    open(tray_spec(owner, at, root, build));
}

pub(crate) fn is_open() -> bool {
    MENU.with(|m| m.try_borrow().map(|m| m.is_some()).unwrap_or(true))
}

pub(crate) fn open(mut spec: Spec) {
    close();
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        if let Placement::Anchor(_) = spec.placement {
            let mut pt = POINT::default();
            if windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt).is_ok() {
                spec.placement = Placement::Anchor(pt);
            }
        }
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        if !CLASS_READY.with(|c| c.replace(true)) {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: CLASS,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("warmUP Companion"),
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
            return;
        };
        let at = match spec.placement {
            Placement::Anchor(p) | Placement::Center(p) => p,
        };
        let (dpi, work) = monitor_at(at);
        let mut menu = Menu {
            hwnd,
            scale: dpi * spec.zoom,
            work,
            spec,
            path: Vec::new(),
            focus: vec![None],
            panels: Vec::new(),
            origin: (0.0, 0.0),
            theme: current_theme(),
            status: crate::win::controller_center::pad_status(),
            update: crate::updater::available_version().map(|v| format!("v{v}")),
            gfx: Gfx::new().ok(),
            widths: Vec::new(),
            ticks: 0,
        };
        if matches!(menu.spec.placement, Placement::Center(_)) {
            menu.focus[0] = step_selectable(&menu.spec.root, None, true);
        }
        layout(&mut menu);
        MENU.with(|m| *m.borrow_mut() = Some(menu));
        crate::win::controller_center::set_menu_owns_pad(true);
        with_menu(paint);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
    }
}

pub(crate) fn close() {
    let menu = MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.take()));
    if let Some(mut menu) = menu {
        crate::win::controller_center::set_menu_owns_pad(false);
        unsafe {
            let _ = KillTimer(menu.hwnd, TIMER_ID);
            let _ = DestroyWindow(menu.hwnd);
        }
        if let Some(f) = menu.spec.on_close.take() {
            f();
        }
    }
}

fn paint(m: &mut Menu) {
    let mut gfx = match m.gfx.take() {
        Some(g) => g,
        None => match unsafe { Gfx::new() } {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("menu: gfx: {e}"));
                return;
            }
        },
    };
    if let Err(e) = unsafe { render(m, &mut gfx) } {
        crate::install::log_line(&format!("menu: render: {e}"));
        return;
    }
    m.gfx = Some(gfx);
}

fn finish(act: Act) {
    match act {
        Act::Stay => with_menu(paint),
        Act::Close(after) => {
            close();
            if let Some(f) = after {
                f();
            }
        }
    }
}

fn client_to_screen(m: &Menu, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x + m.origin.0, y + m.origin.1)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut changed = false;
            with_menu(|m| {
                let (x, y) = client_to_screen(m, lparam);
                if let (Hit::Row(level, i, _), _, _) = hit(m, x, y) {
                    if m.focus.get(level).copied().flatten() != Some(i) {
                        hover(m, level, i);
                        changed = true;
                    }
                }
            });
            if changed {
                finish(Act::Stay);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut act = Act::Stay;
            with_menu(|m| {
                let (sx, sy) = client_to_screen(m, lparam);
                act = match hit(m, sx, sy) {
                    (Hit::Row(level, i, row), x, _) => {
                        hover(m, level, i);
                        click(m, level, i, row, x)
                    }
                    (Hit::Panel, _, _) => Act::Stay,
                    (Hit::Outside, _, _) => Act::Close(None),
                };
            });
            finish(act);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = match wparam.0 as u16 {
                v if v == VK_UP.0 => Some(NavKey::Up),
                v if v == VK_DOWN.0 => Some(NavKey::Down),
                v if v == VK_LEFT.0 => Some(NavKey::Left),
                v if v == VK_RIGHT.0 => Some(NavKey::Right),
                v if v == VK_RETURN.0 || v == VK_SPACE.0 => Some(NavKey::Enter),
                v if v == VK_ESCAPE.0 => Some(NavKey::Back),
                _ => None,
            };
            if let Some(key) = key {
                let mut act = Act::Stay;
                with_menu(|m| act = nav(m, key));
                finish(act);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            let mut repaint = false;
            with_menu(|m| repaint = tick(m));
            if repaint {
                finish(Act::Stay);
            }
            for (button, pressed) in crate::win::controller_center::take_menu_pad_edges() {
                if !pressed {
                    continue;
                }
                if let Some(key) = pad_key(button) {
                    let mut act = Act::Stay;
                    with_menu(|m| act = nav(m, key));
                    let closing = matches!(act, Act::Close(_));
                    finish(act);
                    if closing {
                        break;
                    }
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE if (wparam.0 & 0xffff) as u32 == WA_INACTIVE => {
            if MENU.with(|m| m.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.hwnd)))
                == Some(hwnd)
            {
                close();
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn render(m: &Menu, g: &mut Gfx) -> Result<(), String> {
    g.begin(surface(m))?;
    draw_menu(m, g);
    g.present(
        m.hwnd,
        Some(POINT {
            x: m.origin.0 as i32,
            y: m.origin.1 as i32,
        }),
    )
}

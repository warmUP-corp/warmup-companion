use windows::Win32::Foundation::{BOOL, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect, MONITORINFOF_PRIMARY,
    SM_CXSCREEN, SM_CYSCREEN,
};

const FIT_SLOP: i32 = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

pub fn foreground_fits_monitor(window: ScreenRect, monitor: ScreenRect) -> bool {
    let ww = window.right - window.left;
    let wh = window.bottom - window.top;
    let mw = monitor.right - monitor.left;
    let mh = monitor.bottom - monitor.top;
    if ww <= 0 || wh <= 0 || mw <= 0 || mh <= 0 {
        return false;
    }
    if ww > mw + FIT_SLOP || wh > mh + FIT_SLOP {
        return false;
    }
    let win_area = ww as i64 * wh as i64;
    intersection_area(window, monitor) * 2 >= win_area
}

pub fn choose_monitor(
    foreground_window: Option<ScreenRect>,
    foreground_monitor: Option<ScreenRect>,
    cursor_monitor: Option<ScreenRect>,
    primary: ScreenRect,
) -> ScreenRect {
    if let (Some(window), Some(monitor)) = (foreground_window, foreground_monitor) {
        if foreground_fits_monitor(window, monitor) {
            return monitor;
        }
    }
    cursor_monitor.unwrap_or(primary)
}

pub unsafe fn active_monitor_rect() -> RECT {
    let (fg_win, fg_mon) = foreground_pair();
    let foreground_ok = matches!(
        (fg_win, fg_mon),
        (Some(window), Some(monitor)) if foreground_fits_monitor(window, monitor)
    );
    let cursor = if foreground_ok {
        None
    } else {
        cursor_monitor_rect()
    };
    let primary = if foreground_ok || cursor.is_some() {
        ScreenRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }
    } else {
        primary_screen_rect()
    };
    let chosen = choose_monitor(fg_win, fg_mon, cursor, primary);
    RECT {
        left: chosen.left,
        top: chosen.top,
        right: chosen.right,
        bottom: chosen.bottom,
    }
}

fn intersection_area(a: ScreenRect, b: ScreenRect) -> i64 {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a.right.min(b.right);
    let bottom = a.bottom.min(b.bottom);
    let w = (right - left).max(0) as i64;
    let h = (bottom - top).max(0) as i64;
    w * h
}

fn from_rect(r: RECT) -> ScreenRect {
    ScreenRect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

unsafe fn monitor_rect(mon: HMONITOR) -> Option<ScreenRect> {
    if mon.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(mon, &mut info).as_bool() {
        return None;
    }
    Some(from_rect(info.rcMonitor))
}

unsafe fn cursor_monitor_rect() -> Option<ScreenRect> {
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        return None;
    }
    monitor_rect(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST))
}

unsafe fn foreground_pair() -> (Option<ScreenRect>, Option<ScreenRect>) {
    let hwnd = GetForegroundWindow();
    if hwnd.is_invalid() {
        return (None, None);
    }
    let mut wr = RECT::default();
    if GetWindowRect(hwnd, &mut wr).is_err() {
        return (None, None);
    }
    let mon = monitor_rect(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
    (Some(from_rect(wr)), mon)
}

unsafe fn primary_screen_rect() -> ScreenRect {
    let mut found: Option<RECT> = None;
    let _ = EnumDisplayMonitors(
        HDC(std::ptr::null_mut()),
        None,
        Some(enum_primary),
        LPARAM(&mut found as *mut Option<RECT> as isize),
    );
    if let Some(r) = found {
        return from_rect(r);
    }
    let w = GetSystemMetrics(SM_CXSCREEN).max(1);
    let h = GetSystemMetrics(SM_CYSCREEN).max(1);
    ScreenRect {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    }
}

unsafe extern "system" fn enum_primary(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let slot = &mut *(data.0 as *mut Option<RECT>);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() && info.dwFlags & MONITORINFOF_PRIMARY != 0 {
        *slot = Some(info.rcMonitor);
        return BOOL(0);
    }
    BOOL(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right: left + w,
            bottom: top + h,
        }
    }

    #[test]
    fn foreground_on_secondary_beats_the_primary() {
        let primary = r(0, 0, 1920, 1080);
        let secondary = r(1920, 0, 1920, 1080);
        let window = r(2000, 100, 800, 600);
        let chosen = choose_monitor(Some(window), Some(secondary), Some(primary), primary);
        assert_eq!(chosen, secondary);
    }

    #[test]
    fn foreground_on_a_monitor_left_of_the_origin() {
        let primary = r(0, 0, 1920, 1080);
        let secondary = r(-1920, 0, 1920, 1080);
        let window = r(-1800, 80, 900, 700);
        let chosen = choose_monitor(Some(window), Some(secondary), None, primary);
        assert_eq!(chosen, secondary);
    }

    #[test]
    fn spanning_shell_uses_the_cursor_monitor() {
        let primary = r(0, 0, 1920, 1080);
        let secondary = r(1920, 0, 1920, 1080);
        let shell = r(0, 0, 3840, 1080);
        let chosen = choose_monitor(Some(shell), Some(primary), Some(secondary), primary);
        assert_eq!(chosen, secondary);
    }

    #[test]
    fn spanning_shell_without_cursor_stays_on_primary() {
        let primary = r(0, 0, 1920, 1080);
        let shell = r(0, 0, 3840, 1080);
        let chosen = choose_monitor(Some(shell), Some(primary), None, primary);
        assert_eq!(chosen, primary);
    }

    #[test]
    fn nothing_known_uses_primary() {
        let primary = r(0, 0, 1920, 1080);
        assert_eq!(choose_monitor(None, None, None, primary), primary);
    }

    #[test]
    fn maximized_overhang_still_fits_its_monitor() {
        let mon = r(1920, 0, 1920, 1080);
        let window = ScreenRect {
            left: 1912,
            top: -8,
            right: 3848,
            bottom: 1088,
        };
        assert!(foreground_fits_monitor(window, mon));
    }
}

use windows::Win32::Foundation::{BOOL, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
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
    active_monitor().1
}

/// Handle + rect for the same monitor [`active_monitor_rect`] would pick.
pub unsafe fn active_monitor() -> (HMONITOR, RECT) {
    let (fg_win, fg_mon, fg_hmon) = foreground_triple();
    let foreground_ok = matches!(
        (fg_win, fg_mon),
        (Some(window), Some(monitor)) if foreground_fits_monitor(window, monitor)
    );
    let (cursor, cursor_hmon) = if foreground_ok {
        (None, None)
    } else {
        cursor_monitor_pair()
    };
    let (primary, primary_hmon) = if foreground_ok || cursor.is_some() {
        (
            ScreenRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            None,
        )
    } else {
        primary_screen_pair()
    };
    let chosen = choose_monitor(fg_win, fg_mon, cursor, primary);
    let hmon = if foreground_ok {
        fg_hmon
    } else if cursor.is_some() {
        cursor_hmon
    } else {
        primary_hmon
    }
    .unwrap_or_else(|| MonitorFromPoint(POINT { x: chosen.left, y: chosen.top }, MONITOR_DEFAULTTONEAREST));
    (
        hmon,
        RECT {
            left: chosen.left,
            top: chosen.top,
            right: chosen.right,
            bottom: chosen.bottom,
        },
    )
}

pub unsafe fn dpi_scale(hmonitor: HMONITOR) -> f32 {
    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    if GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() || dpi_x == 0
    {
        return 1.0;
    }
    dpi_x as f32 / 96.0
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

unsafe fn monitor_pair(mon: HMONITOR) -> Option<(HMONITOR, ScreenRect)> {
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
    Some((mon, from_rect(info.rcMonitor)))
}

unsafe fn cursor_monitor_pair() -> (Option<ScreenRect>, Option<HMONITOR>) {
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        return (None, None);
    }
    match monitor_pair(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)) {
        Some((h, r)) => (Some(r), Some(h)),
        None => (None, None),
    }
}

unsafe fn foreground_triple() -> (Option<ScreenRect>, Option<ScreenRect>, Option<HMONITOR>) {
    let hwnd = GetForegroundWindow();
    if hwnd.is_invalid() {
        return (None, None, None);
    }
    let mut wr = RECT::default();
    if GetWindowRect(hwnd, &mut wr).is_err() {
        return (None, None, None);
    }
    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    match monitor_pair(mon) {
        Some((h, r)) => (Some(from_rect(wr)), Some(r), Some(h)),
        None => (Some(from_rect(wr)), None, None),
    }
}

unsafe fn primary_screen_pair() -> (ScreenRect, Option<HMONITOR>) {
    let mut found: Option<(HMONITOR, RECT)> = None;
    let _ = EnumDisplayMonitors(
        HDC(std::ptr::null_mut()),
        None,
        Some(enum_primary),
        LPARAM(&mut found as *mut Option<(HMONITOR, RECT)> as isize),
    );
    if let Some((h, r)) = found {
        return (from_rect(r), Some(h));
    }
    let w = GetSystemMetrics(SM_CXSCREEN).max(1);
    let h = GetSystemMetrics(SM_CYSCREEN).max(1);
    (
        ScreenRect {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        },
        None,
    )
}

unsafe extern "system" fn enum_primary(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let slot = &mut *(data.0 as *mut Option<(HMONITOR, RECT)>);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() && info.dwFlags & MONITORINFOF_PRIMARY != 0 {
        *slot = Some((monitor, info.rcMonitor));
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

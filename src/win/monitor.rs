use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, EnumDisplayMonitors, GetDeviceCaps, GetMonitorInfoW, MonitorFromPoint,
    MonitorFromWindow, HDC, HMONITOR, HORZRES, HORZSIZE, LOGPIXELSX, LOGPIXELSY, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, VERTRES, VERTSIZE,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect, MONITORINFOF_PRIMARY,
    SM_CXSCREEN, SM_CYSCREEN,
};

const FIT_SLOP: i32 = 96;
const TV_DIAGONAL_INCH_MIN: f32 = 40.0;
const TV_ASPECT_MAX: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayKind {
    Tv,
    Auto,
}

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

pub unsafe fn active_work_rect() -> RECT {
    let (hmon, rect) = active_monitor();
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut info).as_bool() {
        info.rcWork
    } else {
        rect
    }
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

fn diagonal_inches_from_mm(width_mm: i32, height_mm: i32) -> f32 {
    if width_mm <= 0 || height_mm <= 0 {
        return 0.0;
    }
    let w = width_mm as f32;
    let h = height_mm as f32;
    (w * w + h * h).sqrt() / 25.4
}

fn gdi_synthesised_mm(pixels: i32, log_pixels: i32) -> Option<i32> {
    if pixels <= 0 || log_pixels <= 0 {
        return None;
    }
    Some(((pixels as f64) * 25.4 / (log_pixels as f64)).round() as i32)
}

fn physical_size_mm(
    width_mm: i32,
    height_mm: i32,
    horz_res: i32,
    vert_res: i32,
    log_pixels_x: i32,
    log_pixels_y: i32,
) -> Option<(i32, i32)> {
    if width_mm <= 0 || height_mm <= 0 {
        return None;
    }
    if let (Some(syn_w), Some(syn_h)) = (
        gdi_synthesised_mm(horz_res, log_pixels_x),
        gdi_synthesised_mm(vert_res, log_pixels_y),
    ) {
        if (width_mm - syn_w).abs() <= 1 && (height_mm - syn_h).abs() <= 1 {
            return None;
        }
    }
    Some((width_mm, height_mm))
}

fn physical_diagonal_inches(
    width_mm: i32,
    height_mm: i32,
    horz_res: i32,
    vert_res: i32,
    log_pixels_x: i32,
    log_pixels_y: i32,
) -> Option<f32> {
    let (w, h) = physical_size_mm(
        width_mm,
        height_mm,
        horz_res,
        vert_res,
        log_pixels_x,
        log_pixels_y,
    )?;
    let d = diagonal_inches_from_mm(w, h);
    (d > 0.0).then_some(d)
}

fn is_tv_diagonal(diagonal_inches: f32) -> bool {
    diagonal_inches >= TV_DIAGONAL_INCH_MIN
}

fn is_tv(width_mm: i32, height_mm: i32) -> bool {
    if width_mm <= 0 || height_mm <= 0 {
        return false;
    }
    let aspect = width_mm as f32 / height_mm as f32;
    is_tv_diagonal(diagonal_inches_from_mm(width_mm, height_mm)) && aspect <= TV_ASPECT_MAX
}

fn display_kind_for_biggest(sizes: &[(i32, i32)]) -> DisplayKind {
    let biggest = sizes.iter().copied().max_by(|a, b| {
        diagonal_inches_from_mm(a.0, a.1)
            .partial_cmp(&diagonal_inches_from_mm(b.0, b.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    match biggest {
        Some((w, h)) if is_tv(w, h) => DisplayKind::Tv,
        _ => DisplayKind::Auto,
    }
}

unsafe extern "system" fn collect_size(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let out = &mut *(data.0 as *mut Vec<(i32, i32)>);
    if let Some(size) = size_from_device_caps(hmonitor) {
        out.push(size);
    }
    BOOL(1)
}

pub unsafe fn detect_display_once() {
    if crate::config::raw_setting("vk_display").is_some() {
        return;
    }
    let mut sizes: Vec<(i32, i32)> = Vec::new();
    let _ = EnumDisplayMonitors(
        HDC::default(),
        None,
        Some(collect_size),
        LPARAM(&mut sizes as *mut Vec<(i32, i32)> as isize),
    );
    let kind = display_kind_for_biggest(&sizes);
    let writes = first_start_settings(kind, |key| crate::config::raw_setting(key).is_some());
    for (key, value) in writes {
        if let Err(e) = crate::config::set_gamepad_setting(key, value) {
            crate::install::log_line(&format!("display detect: persist {key} failed: {e}"));
        }
    }
}

fn first_start_settings(
    kind: DisplayKind,
    present: impl Fn(&str) -> bool,
) -> Vec<(&'static str, &'static str)> {
    let mut writes: Vec<(&'static str, &'static str)> = match kind {
        DisplayKind::Tv => vec![
            ("vk_mode", "floating"),
            ("vk_bar_scale", "1.0"),
            ("vk_display", "tv"),
        ],
        DisplayKind::Auto => vec![("vk_display", "auto")],
    };
    writes.retain(|(key, _)| !present(key));
    writes
}

unsafe fn is_tv_monitor(hmonitor: HMONITOR) -> bool {
    size_from_device_caps(hmonitor).is_some_and(|(w, h)| is_tv(w, h))
}

pub unsafe fn is_active_monitor_tv() -> bool {
    match crate::config::vk_display() {
        crate::config::VkDisplay::Tv => true,
        crate::config::VkDisplay::Desk => false,
        crate::config::VkDisplay::Auto => is_tv_monitor(active_monitor().0),
    }
}

unsafe fn size_from_device_caps(hmonitor: HMONITOR) -> Option<(i32, i32)> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(
        hmonitor,
        &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
    )
    .as_bool()
    {
        return None;
    }
    let hdc = CreateDCW(
        PCWSTR::null(),
        PCWSTR::from_raw(info.szDevice.as_ptr()),
        PCWSTR::null(),
        None,
    );
    if hdc.is_invalid() {
        return None;
    }
    let w = GetDeviceCaps(hdc, HORZSIZE);
    let h = GetDeviceCaps(hdc, VERTSIZE);
    let horz_res = GetDeviceCaps(hdc, HORZRES);
    let vert_res = GetDeviceCaps(hdc, VERTRES);
    let log_pixels_x = GetDeviceCaps(hdc, LOGPIXELSX);
    let log_pixels_y = GetDeviceCaps(hdc, LOGPIXELSY);
    let _ = DeleteDC(hdc);
    physical_size_mm(w, h, horz_res, vert_res, log_pixels_x, log_pixels_y)
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

    #[test]
    fn tv_threshold_is_forty_inch_diagonal() {
        assert!(!is_tv_diagonal(39.9));
        assert!(is_tv_diagonal(40.0));
        assert!(is_tv_diagonal(65.0));
        assert!(!is_tv_diagonal(27.0));
        assert!(is_tv(914, 514));
        assert!(!is_tv(510, 287));
        assert_eq!(diagonal_inches_from_mm(0, 500), 0.0);
        assert_eq!(display_kind_for_biggest(&[]), DisplayKind::Auto);
        assert_eq!(
            display_kind_for_biggest(&[(510, 287), (598, 336)]),
            DisplayKind::Auto
        );
        assert_eq!(
            display_kind_for_biggest(&[(510, 287), (1218, 685)]),
            DisplayKind::Tv
        );
    }

    #[test]
    fn ultrawide_desk_is_not_a_tv() {
        assert!(is_tv_diagonal(diagonal_inches_from_mm(1200, 340)));
        assert!(!is_tv(1200, 340));
        assert_eq!(display_kind_for_biggest(&[(1200, 340)]), DisplayKind::Auto);
        assert!(is_tv(1218, 685));
    }

    #[test]
    fn synthesised_gdi_size_is_unknown() {
        assert_eq!(
            physical_diagonal_inches(1016, 572, 3840, 2160, 96, 96),
            None
        );
        let real = physical_diagonal_inches(1218, 685, 3840, 2160, 96, 96);
        assert!(real.is_some_and(|d| (d - 55.0).abs() < 0.5));
        assert_eq!(physical_diagonal_inches(0, 0, 3840, 2160, 96, 96), None);
        assert_eq!(physical_diagonal_inches(0, 500, 1920, 1080, 96, 96), None);
    }

    #[test]
    fn tv_first_start_defaults_to_floating_full_size_without_overwriting() {
        assert_eq!(
            first_start_settings(DisplayKind::Tv, |_| false),
            [
                ("vk_mode", "floating"),
                ("vk_bar_scale", "1.0"),
                ("vk_display", "tv")
            ]
        );
        assert_eq!(
            first_start_settings(DisplayKind::Tv, |key| key == "vk_mode"),
            [("vk_bar_scale", "1.0"), ("vk_display", "tv")]
        );
        assert_eq!(first_start_settings(DisplayKind::Tv, |_| true), []);
        assert_eq!(
            first_start_settings(DisplayKind::Auto, |_| false),
            [("vk_display", "auto")]
        );
        assert_eq!(first_start_settings(DisplayKind::Auto, |_| true), []);
    }
}

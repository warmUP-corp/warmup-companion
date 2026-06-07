//! Standalone game detection for the companion userland poll loop.
//!
//! Mirrors warmUP's external fullscreen-game heuristic: prefer the foreground
//! top-level window, require it to look fullscreen enough, and ignore shell /
//! browser / launcher processes that commonly create large windows.

#![cfg(windows)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    GA_ROOT,
};

const FULLSCREEN_PERCENT_BASE: i64 = 100;
const EXTERNAL_FULLSCREEN_AXIS_PERCENT: i64 = 85;
const EXTERNAL_FULLSCREEN_OVERLAP_PERCENT: i64 = 72;
const DETECT_INTERVAL: Duration = Duration::from_millis(500);

const EXTERNAL_GAME_EXE_DENYLIST: &[&str] = &[
    "applicationframehost.exe",
    "arc.exe",
    "brave.exe",
    "chrome.exe",
    "chromium.exe",
    "ctfmon.exe",
    "duckduckgo.exe",
    "dllhost.exe",
    "dwm.exe",
    "explorer.exe",
    "firefox.exe",
    "lockapp.exe",
    "msedge.exe",
    "opera.exe",
    "perplexity.exe",
    "runtimebroker.exe",
    "searchhost.exe",
    "searchapp.exe",
    "shellhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "steamwebhelper.exe",
    "systemsettings.exe",
    "textinputhost.exe",
    "webviewhost.exe",
    "widgets.exe",
    "windowsterminal.exe",
    "wt.exe",
    "steam.exe",
    "epicgameslauncher.exe",
    "galaxyclient.exe",
    "gog galaxy.exe",
    "goggalaxy.exe",
    "eadesktop.exe",
    "origin.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "uplay.exe",
    "battle.net.exe",
    "amazon games.exe",
    "nile.exe",
    "xboxpcapp.exe",
    "gamingservices.exe",
];

#[derive(Clone, Copy)]
struct DetectionCache {
    checked_at: Instant,
    active: bool,
}

static CACHE: OnceLock<Mutex<Option<DetectionCache>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<DetectionCache>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn standalone_game_active_cached() -> bool {
    let now = Instant::now();
    if let Ok(mut slot) = cache().lock() {
        if let Some(cached) = *slot {
            if now.duration_since(cached.checked_at) < DETECT_INTERVAL {
                return cached.active;
            }
        }
        let active = standalone_game_active();
        *slot = Some(DetectionCache {
            checked_at: now,
            active,
        });
        active
    } else {
        standalone_game_active()
    }
}

fn standalone_game_active() -> bool {
    let our_pid = std::process::id();
    let exe_by_pid = pid_to_exe_map();
    foreground_external_game_pid(our_pid, &exe_by_pid).is_some()
}

fn pid_to_exe_map() -> HashMap<u32, String> {
    let mut map = HashMap::new();
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return map,
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let nul = pe
                    .szExeFile
                    .iter()
                    .position(|&x| x == 0)
                    .unwrap_or(pe.szExeFile.len());
                let name = String::from_utf16_lossy(&pe.szExeFile[..nul]);
                map.insert(pe.th32ProcessID, name);
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    map
}

fn exe_base_name_lower(full: &str) -> String {
    let lower = full.to_ascii_lowercase();
    lower
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&lower)
        .to_string()
}

fn is_pid_exe_denylisted(pid: u32, exe_by_pid: &HashMap<u32, String>) -> bool {
    let Some(full) = exe_by_pid.get(&pid) else {
        return false;
    };
    let base = exe_base_name_lower(full);
    EXTERNAL_GAME_EXE_DENYLIST.contains(&base.as_str())
}

fn foreground_external_game_pid(our_pid: u32, exe_by_pid: &HashMap<u32, String>) -> Option<u32> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        let tid = GetWindowThreadProcessId(fg, Some(&mut pid));
        if tid == 0 || pid == 0 || pid == our_pid {
            return None;
        }
        let root = GetAncestor(fg, GA_ROOT);
        let hwnd = if root.is_invalid() { fg } else { root };
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let mut root_pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut root_pid));
        let pid = if root_pid != 0 { root_pid } else { pid };
        if pid == 0 || pid == our_pid {
            return None;
        }
        if !is_hwnd_external_fullscreen_candidate(hwnd) {
            return None;
        }
        if is_pid_exe_denylisted(pid, exe_by_pid) {
            return None;
        }
        Some(pid)
    }
}

fn is_hwnd_external_fullscreen_candidate(hwnd: HWND) -> bool {
    let Some((win_rect, mon_rect)) = hwnd_window_and_monitor_rect(hwnd) else {
        return false;
    };
    if rect_covers_monitor_axis_percent(win_rect, mon_rect, EXTERNAL_FULLSCREEN_AXIS_PERCENT) {
        return true;
    }
    let mon_area = rect_area(mon_rect);
    if mon_area <= 0 {
        return false;
    }
    let Some(ix) = rect_intersect(win_rect, mon_rect) else {
        return false;
    };
    rect_area(ix) * FULLSCREEN_PERCENT_BASE >= mon_area * EXTERNAL_FULLSCREEN_OVERLAP_PERCENT
}

fn hwnd_window_and_monitor_rect(hwnd: HWND) -> Option<(RECT, RECT)> {
    unsafe {
        let mut win_rect = RECT::default();
        if GetWindowRect(hwnd, &mut win_rect).is_err() {
            return None;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
            return None;
        }
        Some((win_rect, mi.rcMonitor))
    }
}

fn rect_covers_monitor_axis_percent(win_rect: RECT, mon_rect: RECT, axis_percent: i64) -> bool {
    let win_w = (win_rect.right - win_rect.left) as i64;
    let win_h = (win_rect.bottom - win_rect.top) as i64;
    let mon_w = (mon_rect.right - mon_rect.left) as i64;
    let mon_h = (mon_rect.bottom - mon_rect.top) as i64;
    if mon_w <= 0 || mon_h <= 0 {
        return false;
    }
    win_w * FULLSCREEN_PERCENT_BASE >= mon_w * axis_percent
        && win_h * FULLSCREEN_PERCENT_BASE >= mon_h * axis_percent
}

fn rect_area(r: RECT) -> i64 {
    let w = (r.right - r.left) as i64;
    let h = (r.bottom - r.top) as i64;
    w * h
}

fn rect_intersect(a: RECT, b: RECT) -> Option<RECT> {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a.right.min(b.right);
    let bottom = a.bottom.min(b.bottom);
    if right <= left || bottom <= top {
        return None;
    }
    Some(RECT {
        left,
        top,
        right,
        bottom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn axis_coverage_accepts_borderless_fullscreen() {
        assert!(rect_covers_monitor_axis_percent(
            rect(0, 0, 1920, 1080),
            rect(0, 0, 1920, 1080),
            EXTERNAL_FULLSCREEN_AXIS_PERCENT,
        ));
    }

    #[test]
    fn overlap_catches_letterboxed_candidate() {
        assert!(is_external_candidate_for_rects(
            rect(0, 40, 1920, 1040),
            rect(0, 0, 1920, 1080),
        ));
    }

    #[test]
    fn denylist_rejects_launcher_clients() {
        for exe in ["steam.exe", "epicgameslauncher.exe", "chrome.exe"] {
            assert!(EXTERNAL_GAME_EXE_DENYLIST.contains(&exe));
        }
    }

    fn is_external_candidate_for_rects(win_rect: RECT, mon_rect: RECT) -> bool {
        if rect_covers_monitor_axis_percent(win_rect, mon_rect, EXTERNAL_FULLSCREEN_AXIS_PERCENT) {
            return true;
        }
        let mon_area = rect_area(mon_rect);
        let Some(ix) = rect_intersect(win_rect, mon_rect) else {
            return false;
        };
        rect_area(ix) * FULLSCREEN_PERCENT_BASE >= mon_area * EXTERNAL_FULLSCREEN_OVERLAP_PERCENT
    }
}

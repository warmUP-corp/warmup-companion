//! Kid Mode system-wide game blocking for processes started outside warmUP.
//!
//! warmUP pushes a [`ParentalGuardPayload`] over IPC; this module persists it and
//! polls running processes while Kid Mode is active.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::pipe_server::TrackingOwner;
use crate::protocol::{ParentalBlockedPayload, ParentalGuardPayload};

const GUARD_FILE: &str = r"C:\ProgramData\WarmupVk\parental-guard.json";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const NOTIFY_COOLDOWN: Duration = Duration::from_secs(20);

static STATE: OnceLock<Mutex<GuardState>> = OnceLock::new();
static BLOCKED_NOTIFY: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
struct GuardState {
    guard: ParentalGuardPayload,
}

fn state() -> &'static Mutex<GuardState> {
    STATE.get_or_init(|| Mutex::new(GuardState::default()))
}

fn notify_cooldown() -> &'static Mutex<HashMap<String, Instant>> {
    BLOCKED_NOTIFY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn spawn_guardian_loop() {
    std::thread::Builder::new()
        .name("warmup-parental-guard".into())
        .spawn(guardian_loop)
        .ok();
}

pub fn apply_guard(payload: &ParentalGuardPayload) {
    if let Ok(mut slot) = state().lock() {
        slot.guard = payload.clone();
    }
    persist_guard(payload);
}

pub fn load_persisted_guard() {
    let Ok(raw) = std::fs::read_to_string(GUARD_FILE) else {
        return;
    };
    if let Ok(payload) = serde_json::from_str::<ParentalGuardPayload>(&raw) {
        apply_guard(&payload);
    }
}

fn persist_guard(payload: &ParentalGuardPayload) {
    if let Ok(json) = serde_json::to_string(payload) {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\WarmupVk");
        let _ = std::fs::write(GUARD_FILE, json);
    }
}

pub fn publish_blocked(payload: ParentalBlockedPayload) {
    crate::pipe_server::publish_parental_blocked(payload);
}

fn guardian_loop() {
    load_persisted_guard();
    enable_debug_privilege();
    loop {
        tick_guardian();
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn tick_guardian() {
    let guard = state().lock().map(|s| s.guard.clone()).unwrap_or_default();
    if !guard.enabled {
        return;
    }
    let blocked_stems: HashSet<String> = guard.blocked_exe_stems.iter().cloned().collect();
    let blocked_dirs: Vec<String> = guard.blocked_install_dir_prefixes.clone();
    if blocked_stems.is_empty() && blocked_dirs.is_empty() {
        return;
    }

    let our_pid = std::process::id();
    for (pid, exe_name, image_path) in snapshot_processes() {
        if pid == 0 || pid == our_pid || is_protected_process(&exe_name, image_path.as_deref()) {
            continue;
        }
        let stem = exe_stem_lower(&exe_name);
        let blocked_by_stem = !stem.is_empty() && blocked_stems.contains(&stem);
        let blocked_by_dir = image_path
            .as_deref()
            .map(|path| path_blocked_by_install_dir(path, &blocked_dirs))
            .unwrap_or(false);
        if !blocked_by_stem && !blocked_by_dir {
            continue;
        }
        if terminate_blocked_process(pid) {
            maybe_notify_blocked(&stem, pid);
        }
    }
}

fn maybe_notify_blocked(stem: &str, pid: u32) {
    let now = Instant::now();
    let key = format!("{stem}:{pid}");
    if let Ok(mut map) = notify_cooldown().lock() {
        map.retain(|_, at| now.duration_since(*at) < NOTIFY_COOLDOWN);
        if map
            .get(&key)
            .is_some_and(|at| now.duration_since(*at) < NOTIFY_COOLDOWN)
        {
            return;
        }
        map.insert(key, now);
    }
    publish_blocked(ParentalBlockedPayload {
        exe_stem: stem.to_string(),
        pid,
    });
}

fn path_blocked_by_install_dir(path: &str, blocked_dirs: &[String]) -> bool {
    let lower = path.replace('/', "\\").to_ascii_lowercase();
    blocked_dirs.iter().any(|prefix| lower.starts_with(prefix))
}

fn exe_stem_lower(exe_name: &str) -> String {
    let lower = exe_name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .unwrap_or(lower.as_str())
        .to_string()
}

fn is_protected_process(exe_name: &str, image_path: Option<&str>) -> bool {
    let stem = exe_stem_lower(exe_name);
    if matches!(
        stem.as_str(),
        "warmup"
            | "warmup-companion"
            | "warmup-keyboard"
            | "explorer"
            | "steam"
            | "epicgameslauncher"
            | "galaxyclient"
            | "eadesktop"
            | "origin"
            | "ubisoftconnect"
            | "battle.net"
            | "xboxpcapp"
            | "csrss"
            | "winlogon"
            | "dwm"
            | "lsass"
            | "services"
            | "smss"
            | "sihost"
            | "fontdrvhost"
            | "taskhostw"
            | "wininit"
            | "userinit"
            | "searchhost"
            | "runtimebroker"
            | "svchost"
            | "dllhost"
            | "audiodg"
            | "system"
            | "idle"
    ) {
        return true;
    }
    if let Some(path) = image_path {
        let lower = path.replace('/', "\\").to_ascii_lowercase();
        if lower.contains("\\windows\\system32\\")
            || lower.contains("\\windows\\syswow64\\")
            || lower.contains("\\program files\\windowsapps\\")
        {
            return true;
        }
    }
    false
}

pub(crate) fn snapshot_processes() -> Vec<(u32, String, Option<String>)> {
    snapshot_processes_filtered(None)
}

pub(crate) fn snapshot_processes_for_owner(
    owner: &TrackingOwner,
) -> Vec<(u32, String, Option<String>)> {
    snapshot_processes_filtered(Some(owner))
}

fn snapshot_processes_filtered(
    owner_filter: Option<&TrackingOwner>,
) -> Vec<(u32, String, Option<String>)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                if let Some(expected_owner) = owner_filter {
                    if !process_is_in_session(pe.th32ProcessID, expected_owner.session_id) {
                        if Process32NextW(snap, &mut pe).is_err() {
                            break;
                        }
                        continue;
                    }
                }
                let nul = pe
                    .szExeFile
                    .iter()
                    .position(|&x| x == 0)
                    .unwrap_or(pe.szExeFile.len());
                let name = String::from_utf16_lossy(&pe.szExeFile[..nul]);
                let image_path = if let Some(expected_owner) = owner_filter {
                    let Some(path) = owned_process_image_path(pe.th32ProcessID, expected_owner)
                    else {
                        if Process32NextW(snap, &mut pe).is_err() {
                            break;
                        }
                        continue;
                    };
                    path
                } else {
                    full_process_image_path(pe.th32ProcessID)
                };
                out.push((pe.th32ProcessID, name, image_path));
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn process_is_in_session(pid: u32, expected_session: u32) -> bool {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;

    let mut actual_session = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut actual_session) }.is_ok()
        && actual_session == expected_session
}

fn full_process_image_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let path = process_image_path(handle);
        let _ = CloseHandle(handle);
        path
    }
}

fn owned_process_image_path(pid: u32, expected_owner: &TrackingOwner) -> Option<Option<String>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        if crate::pipe_server::tracking_owner_from_process(handle, pid).as_ref()
            != Some(expected_owner)
        {
            let _ = CloseHandle(handle);
            return None;
        }
        let path = process_image_path(handle);
        let _ = CloseHandle(handle);
        Some(path)
    }
}

fn process_image_path(handle: windows::Win32::Foundation::HANDLE) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    let mut buf = vec![0u16; 1024];
    let mut size = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buf[..size as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_filter_requires_the_authenticated_sid_and_session() {
        use windows::Win32::System::Threading::GetCurrentProcess;

        let pid = std::process::id();
        let owner =
            crate::pipe_server::tracking_owner_from_process(unsafe { GetCurrentProcess() }, pid)
                .unwrap();

        assert!(process_is_in_session(pid, owner.session_id));
        assert!(!process_is_in_session(pid, u32::MAX));
        assert!(owned_process_image_path(pid, &owner).is_some());

        let mut other_user = owner;
        other_user.user_sid = "S-1-5-21-other".into();
        assert!(owned_process_image_path(pid, &other_user).is_none());
    }
}

fn terminate_blocked_process(pid: u32) -> bool {
    request_window_close(pid);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        let _ = terminate_pid_with_privilege(pid);
    });
    true
}

fn terminate_pid_with_privilege(pid: u32) -> bool {
    enable_debug_privilege();
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let ok = TerminateProcess(handle, 1).is_ok();
            let _ = CloseHandle(handle);
            if ok {
                crate::install::log_line(&format!("parental guard terminated pid {pid}"));
                return true;
            }
        }
    }
    false
}

fn enable_debug_privilege() {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .is_err()
        {
            return;
        }
        let mut luid = LUID::default();
        let name = windows::core::w!("SeDebugPrivilege");
        if LookupPrivilegeValueW(None, PCWSTR(name.as_ptr()), &mut luid).is_err() {
            let _ = CloseHandle(token);
            return;
        }
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let _ = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        let _ = CloseHandle(token);
    }
}

fn request_window_close(pid: u32) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    if let Some(hwnd) = largest_visible_hwnd(pid) {
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn largest_visible_hwnd(pid: u32) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Ctx {
        pid: u32,
        best: Option<HWND>,
        best_area: i32,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        let mut wpid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut wpid));
        if wpid != ctx.pid || !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return BOOL(1);
        }
        let area = (rect.right - rect.left) * (rect.bottom - rect.top);
        if area > ctx.best_area {
            ctx.best_area = area;
            ctx.best = Some(hwnd);
        }
        BOOL(1)
    }

    let mut ctx = Ctx {
        pid,
        best: None,
        best_area: 0,
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as _));
    }
    ctx.best
}

#[cfg(not(windows))]
pub fn spawn_guardian_loop() {}
#[cfg(not(windows))]
pub fn apply_guard(_payload: &ParentalGuardPayload) {}
#[cfg(not(windows))]
pub fn load_persisted_guard() {}
#[cfg(not(windows))]
pub fn publish_blocked(_payload: ParentalBlockedPayload) {}

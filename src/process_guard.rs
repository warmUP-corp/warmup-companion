//! Win32 process snapshot, image-path, and terminate helpers for Kid Mode / playtime.

#![cfg(any(windows, target_os = "linux"))]

#[cfg(windows)]
use std::time::Duration;

use crate::tracking_owner::TrackingOwner;

pub(crate) fn snapshot_processes() -> Vec<(u32, String, Option<String>)> {
    snapshot_processes_filtered(None)
}

pub(crate) fn snapshot_processes_for_owner(
    owner: &TrackingOwner,
) -> Vec<(u32, String, Option<String>)> {
    snapshot_processes_filtered(Some(owner))
}

#[cfg(windows)]
pub(crate) fn image_path(pid: u32) -> Option<String> {
    full_process_image_path(pid)
}

#[cfg(windows)]
pub(crate) fn steam_running_app_id(pid: u32) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_USERS, RRF_RT_REG_DWORD,
    };

    fn read(root: HKEY, subkey: &str) -> Option<u32> {
        let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let name: Vec<u16> = "RunningAppID"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let res = unsafe {
            RegGetValueW(
                root,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut core::ffi::c_void),
                Some(&mut size),
            )
        };
        res.is_ok().then_some(value)
    }

    let owner_sid = process_owner_sid(pid);
    let value = owner_sid
        .and_then(|sid| read(HKEY_USERS, &format!(r"{sid}\Software\Valve\Steam")))
        .or_else(|| read(HKEY_CURRENT_USER, r"Software\Valve\Steam"))?;
    (value != 0).then(|| value.to_string())
}

#[cfg(windows)]
fn process_owner_sid(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let owner = crate::tracking_owner::tracking_owner_from_process(handle, pid);
        let _ = CloseHandle(handle);
        owner.map(|owner| owner.user_sid)
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
fn process_is_in_session(pid: u32, expected_session: u32) -> bool {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;

    let mut actual_session = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut actual_session) }.is_ok()
        && actual_session == expected_session
}

#[cfg(windows)]
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

#[cfg(windows)]
fn owned_process_image_path(pid: u32, expected_owner: &TrackingOwner) -> Option<Option<String>> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        if crate::tracking_owner::tracking_owner_from_process(handle, pid).as_ref()
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

#[cfg(windows)]
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

#[cfg(windows)]
pub(crate) fn terminate_blocked_process(pid: u32) -> bool {
    request_window_close(pid);
    let _ = std::thread::Builder::new()
        .name("warmup-terminate-pid".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            let _ = terminate_pid_with_privilege(pid);
        });
    true
}

#[cfg(windows)]
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

#[cfg(windows)]
pub(crate) fn enable_debug_privilege() {
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

#[cfg(windows)]
fn request_window_close(pid: u32) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    if let Some(hwnd) = largest_visible_hwnd(pid) {
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(windows)]
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

#[cfg(target_os = "linux")]
pub(crate) fn image_path(pid: u32) -> Option<String> {
    if !proc_is_live(pid) {
        return None;
    }
    proc_identity(pid).and_then(|(_, path)| path)
}

#[cfg(target_os = "linux")]
pub(crate) fn steam_running_app_id(pid: u32) -> Option<String> {
    ["SteamAppId", "SteamGameId"].into_iter().find_map(|key| {
        proc_env(pid, key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty() && value != "0")
    })
}

#[cfg(target_os = "linux")]
fn snapshot_processes_filtered(
    owner_filter: Option<&TrackingOwner>,
) -> Vec<(u32, String, Option<String>)> {
    let owner_uid = match owner_filter {
        Some(owner) => match owner_uid(owner) {
            Some(uid) => Some(uid),
            None => return Vec::new(),
        },
        None => None,
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(uid) = owner_uid {
            if proc_real_uid(pid) != Some(uid) {
                continue;
            }
        }
        if !proc_is_live(pid) {
            continue;
        }
        if let Some((name, image_path)) = proc_identity(pid) {
            out.push((pid, name, image_path));
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn owner_uid(owner: &TrackingOwner) -> Option<u32> {
    owner.user_sid.strip_prefix("uid:")?.parse().ok()
}

#[cfg(target_os = "linux")]
fn proc_real_uid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn proc_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn proc_is_live(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(close) = stat.rfind(')') else {
        return false;
    };
    !matches!(
        stat[close + 1..].split_whitespace().next(),
        None | Some("Z") | Some("X") | Some("x")
    )
}

#[cfg(target_os = "linux")]
fn proc_identity(pid: u32) -> Option<(String, Option<String>)> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|link| {
            let text = link.to_string_lossy().into_owned();
            text.strip_suffix(" (deleted)")
                .map(str::to_string)
                .unwrap_or(text)
        });
    let Some(exe) = exe else {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let comm = comm.trim_end_matches('\n');
        return (!comm.is_empty()).then(|| (comm.to_string(), None));
    };
    let base = path_basename(&exe);
    if is_wine_loader(base) {
        if let Some(windows_path) = proc_argv0(pid).filter(|arg| is_windows_drive_path(arg)) {
            let name = path_basename(&windows_path).to_string();
            let image_path =
                resolve_wine_path(pid, &windows_path).unwrap_or(windows_path.clone());
            return Some((name, Some(image_path)));
        }
    }
    if base.starts_with("ld-linux") {
        if let Some(program) = loader_program(&proc_argv(pid)) {
            return Some((path_basename(&program).to_string(), Some(program)));
        }
    }
    Some((base.to_string(), Some(exe)))
}

#[cfg(target_os = "linux")]
fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(target_os = "linux")]
fn is_wine_loader(base: &str) -> bool {
    matches!(
        base,
        "wine" | "wine64" | "wine-preloader" | "wine64-preloader"
    )
}

#[cfg(target_os = "linux")]
fn is_windows_drive_path(arg: &str) -> bool {
    let bytes = arg.as_bytes();
    bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

#[cfg(target_os = "linux")]
fn proc_argv(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| {
            raw.split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn proc_argv0(pid: u32) -> Option<String> {
    proc_argv(pid).into_iter().next()
}

#[cfg(target_os = "linux")]
fn loader_program(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if matches!(
            arg.as_str(),
            "--library-path"
                | "--preload"
                | "--audit"
                | "--argv0"
                | "--glibc-hwcaps-prepend"
                | "--glibc-hwcaps-mask"
        ) {
            args.next();
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        return arg.starts_with('/').then(|| arg.clone());
    }
    None
}

#[cfg(target_os = "linux")]
fn proc_env(pid: u32, key: &str) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    raw.split(|byte| *byte == 0).find_map(|entry| {
        let entry = String::from_utf8_lossy(entry);
        let (name, value) = entry.split_once('=')?;
        (name == key && !value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(target_os = "linux")]
fn resolve_wine_path(pid: u32, windows_path: &str) -> Option<String> {
    let prefix = proc_env(pid, "WINEPREFIX")
        .map(std::path::PathBuf::from)
        .or_else(|| proc_env(pid, "HOME").map(|home| std::path::Path::new(&home).join(".wine")))?;
    let drive = windows_path[..1].to_ascii_lowercase();
    let root = std::fs::canonicalize(prefix.join("dosdevices").join(format!("{drive}:"))).ok()?;
    let mut path = root;
    for part in windows_path[3..].split(['\\', '/']).filter(|part| !part.is_empty()) {
        path.push(part);
    }
    Some(path.to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
pub(crate) fn terminate_blocked_process(pid: u32) -> bool {
    if !proc_is_live(pid) {
        return false;
    }
    let Some(started) = proc_start_ticks(pid) else {
        return false;
    };
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    }
    let _ = std::thread::Builder::new()
        .name("warmup-terminate-pid".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            if proc_start_ticks(pid) == Some(started)
                && unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0
            {
                crate::install::log_line(&format!("parental guard terminated pid {pid}"));
            }
        });
    true
}

#[cfg(target_os = "linux")]
pub(crate) fn enable_debug_privilege() {}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn process_filter_requires_the_authenticated_sid_and_session() {
        use windows::Win32::System::Threading::GetCurrentProcess;

        let pid = std::process::id();
        let owner =
            crate::tracking_owner::tracking_owner_from_process(unsafe { GetCurrentProcess() }, pid)
                .unwrap();

        assert!(process_is_in_session(pid, owner.session_id));
        assert!(!process_is_in_session(pid, u32::MAX));
        assert!(owned_process_image_path(pid, &owner).is_some());

        let mut other_user = owner;
        other_user.user_sid = "S-1-5-21-other".into();
        assert!(owned_process_image_path(pid, &other_user).is_none());
    }
}

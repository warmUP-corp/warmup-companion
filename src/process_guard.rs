//! Win32 process snapshot, image-path, and terminate helpers for Kid Mode / playtime.

#![cfg(windows)]

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

pub(crate) fn terminate_blocked_process(pid: u32) -> bool {
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

#[cfg(test)]
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

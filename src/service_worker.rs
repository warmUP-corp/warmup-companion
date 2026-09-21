//! Launch the sign-in VK worker into the active console session (token dup + CreateProcessAsUser).

#![cfg(all(windows, feature = "service"))]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    SetEntriesInAclW, EXPLICIT_ACCESS_W, SET_ACCESS, TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, AllocateAndInitializeSid, DuplicateTokenEx, FreeSid,
    InitializeSecurityDescriptor, LookupPrivilegeValueW, SecurityIdentification,
    SetSecurityDescriptorDacl, SetTokenInformation, TokenPrimary, TokenSessionId, ACL,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SECURITY_NT_AUTHORITY,
    SE_PRIVILEGE_ENABLED, SE_TAKE_OWNERSHIP_NAME, TOKEN_ACCESS_MASK, TOKEN_PRIVILEGES,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetExitCodeProcess, OpenProcess, OpenProcessToken,
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};

use crate::install;

const WORKER_ARGS: &[&str] = &["--service-worker", "--boot", "--cfg-winlogon"];
/// `WTSGetActiveConsoleSessionId` when no interactive session exists yet (pre-logon / boot).
const INVALID_CONSOLE_SESSION: u32 = 0xFFFF_FFFF;

pub struct WorkerProcess {
    pub handle: HANDLE,
    pub pid: u32,
}

pub fn launch_worker_in_active_session() -> Result<WorkerProcess, String> {
    unsafe {
        let session_id = WTSGetActiveConsoleSessionId();
        if session_id == INVALID_CONSOLE_SESSION {
            return Err("no active console session yet (wait for logon UI)".into());
        }
        install::log_line(&format!("active console session={session_id}"));
        let winlogon_pid = find_process_in_session("winlogon.exe", session_id)?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, winlogon_pid)
            .map_err(|e| format!("OpenProcess(winlogon pid={winlogon_pid}): {e}"))?;
        let token = duplicate_primary_token_for_session(process, session_id)?;
        let result = create_worker_process(token);
        let _ = CloseHandle(token);
        let _ = CloseHandle(process);
        result
    }
}

pub fn worker_exit_code(handle: HANDLE) -> u32 {
    let mut code = 0u32;
    unsafe {
        let _ = GetExitCodeProcess(handle, &mut code);
    }
    code
}

/// Token logon-session LUID (`TOKEN_STATISTICS.AuthenticationId`) as a u64.
/// 0 on failure. Used only for the secure-desktop diagnostic.
unsafe fn token_auth_id(token: HANDLE) -> u64 {
    use windows::Win32::Security::{GetTokenInformation, TokenStatistics, TOKEN_STATISTICS};
    let mut len = 0u32;
    let _ = GetTokenInformation(token, TokenStatistics, None, 0, &mut len);
    if len == 0 {
        return 0;
    }
    let mut buf = vec![0u8; len as usize];
    if GetTokenInformation(
        token,
        TokenStatistics,
        Some(buf.as_mut_ptr().cast()),
        len,
        &mut len,
    )
    .is_err()
    {
        return 0;
    }
    let st = &*(buf.as_ptr() as *const TOKEN_STATISTICS);
    ((st.AuthenticationId.HighPart as u64) << 32) | st.AuthenticationId.LowPart as u64
}

unsafe fn duplicate_primary_token_for_session(
    process: HANDLE,
    session_id: u32,
) -> Result<HANDLE, String> {
    let mut token = HANDLE::default();
    OpenProcessToken(process, TOKEN_ACCESS_MASK(0x201eb), &mut token)
        .map_err(|e| format!("OpenProcessToken(winlogon): {e}"))?;
    // Reference value for the diagnostic: winlogon's own token logon-session LUID.
    // The worker's SELF probe logs its token's `authid`; if it matches this, the
    // dup carried winlogon's context (else CreateProcessAsUserW lost it).
    install::log_line(&format!(
        "winlogon token authid=0x{:x}",
        token_auth_id(token)
    ));

    let mut admin_sid = PSID::default();
    AllocateAndInitializeSid(
        &SECURITY_NT_AUTHORITY,
        2,
        32,
        544,
        0,
        0,
        0,
        0,
        0,
        0,
        &mut admin_sid,
    )
    .map_err(|e| {
        let _ = CloseHandle(token);
        format!("AllocateAndInitializeSid(Admins): {e}")
    })?;

    let mut access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: 0xf003f,
        grfAccessMode: SET_ACCESS,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_GROUP,
            ptstrName: PWSTR(admin_sid.0.cast()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut acl: *mut ACL = std::ptr::null_mut();
    let acl_err = SetEntriesInAclW(Some(std::slice::from_mut(&mut access)), None, &mut acl);
    if acl_err.0 != 0 {
        let _ = FreeSid(admin_sid);
        let _ = CloseHandle(token);
        return Err(format!("SetEntriesInAclW: {}", acl_err.0));
    }

    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_ptr = PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
    if let Err(e) = InitializeSecurityDescriptor(descriptor_ptr, 1) {
        let _ = LocalFree(HLOCAL(acl.cast()));
        let _ = FreeSid(admin_sid);
        let _ = CloseHandle(token);
        return Err(format!("InitializeSecurityDescriptor: {e}"));
    }
    if let Err(e) = SetSecurityDescriptorDacl(descriptor_ptr, true, Some(acl), false) {
        let _ = LocalFree(HLOCAL(acl.cast()));
        let _ = FreeSid(admin_sid);
        let _ = CloseHandle(token);
        return Err(format!("SetSecurityDescriptorDacl: {e}"));
    }

    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
        bInheritHandle: windows::Win32::Foundation::BOOL(1),
    };

    let mut primary = HANDLE::default();
    let dup = DuplicateTokenEx(
        token,
        TOKEN_ACCESS_MASK(0x2000000),
        Some(&attrs),
        SecurityIdentification,
        TokenPrimary,
        &mut primary,
    );
    let _ = CloseHandle(token);
    let _ = LocalFree(HLOCAL(acl.cast()));
    let _ = FreeSid(admin_sid);
    dup.map_err(|e| format!("DuplicateTokenEx: {e}"))?;

    SetTokenInformation(
        primary,
        TokenSessionId,
        (&session_id as *const u32).cast(),
        std::mem::size_of::<u32>() as u32,
    )
    .map_err(|e| {
        let _ = CloseHandle(primary);
        format!("SetTokenInformation(TokenSessionId): {e}")
    })?;
    if let Err(e) = enable_take_ownership(primary) {
        let _ = CloseHandle(primary);
        return Err(e);
    }
    Ok(primary)
}

unsafe fn enable_take_ownership(token: HANDLE) -> Result<(), String> {
    let mut luid = Default::default();
    LookupPrivilegeValueW(None, SE_TAKE_OWNERSHIP_NAME, &mut luid)
        .map_err(|e| format!("LookupPrivilegeValueW(SeTakeOwnershipPrivilege): {e}"))?;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    AdjustTokenPrivileges(
        token,
        false,
        Some(&privileges),
        std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
        None,
        None,
    )
    .map_err(|e| format!("AdjustTokenPrivileges(SeTakeOwnershipPrivilege): {e}"))
}

unsafe fn create_worker_process(token: HANDLE) -> Result<WorkerProcess, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let exe_w = wide_os(exe.as_os_str());
    let mut cmd_w = wide(&command_line(&exe, WORKER_ARGS));
    // Default desktop in session; app attaches via OpenInputDesktop (lock/logon/UAC).
    let mut desktop = wide("winsta0\\default");
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    let mut env = std::ptr::null_mut();
    let env_created = CreateEnvironmentBlock(&mut env, token, false).is_ok();
    if !env_created {
        install::log_line("CreateEnvironmentBlock failed; launching without custom env");
    }
    let mut flags = if env_created {
        CREATE_UNICODE_ENVIRONMENT
    } else {
        PROCESS_CREATION_FLAGS(0)
    };
    if !crate::config::debug_ui_enabled() {
        flags |= CREATE_NO_WINDOW;
    }

    let created = CreateProcessAsUserW(
        token,
        PCWSTR(exe_w.as_ptr()),
        PWSTR(cmd_w.as_mut_ptr()),
        None,
        None,
        false,
        flags,
        if env_created { Some(env.cast()) } else { None },
        PCWSTR::null(),
        &startup,
        &mut info,
    );
    if env_created {
        let _ = DestroyEnvironmentBlock(env);
    }
    created.map_err(|e| format!("CreateProcessAsUserW({}): {e}", exe.display()))?;
    let _ = CloseHandle(info.hThread);
    Ok(WorkerProcess {
        handle: info.hProcess,
        pid: info.dwProcessId,
    })
}

unsafe fn find_process_in_session(name: &str, session_id: u32) -> Result<u32, String> {
    let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
        .map_err(|e| format!("CreateToolhelp32Snapshot: {e}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    let mut found = None;
    let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
    while ok {
        let exe = nul_terminated_utf16(&entry.szExeFile);
        if exe.eq_ignore_ascii_case(name) {
            let mut proc_session = 0u32;
            if ProcessIdToSessionId(entry.th32ProcessID, &mut proc_session).is_ok()
                && proc_session == session_id
            {
                found = Some(entry.th32ProcessID);
                break;
            }
        }
        ok = Process32NextW(snapshot, &mut entry).is_ok();
    }
    let _ = CloseHandle(snapshot);
    found.ok_or_else(|| format!("{name} not found in session {session_id}"))
}

fn command_line(exe: &PathBuf, args: &[&str]) -> String {
    let mut cmd = format!("\"{}\"", exe.display());
    for arg in args {
        cmd.push(' ');
        cmd.push_str(arg);
    }
    cmd
}

fn nul_terminated_utf16(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_os(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

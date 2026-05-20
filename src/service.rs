//! Windows service entry: SCM launcher plus worker process for sign-in VK.

#![cfg(all(windows, feature = "service"))]

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::{
    DuplicateTokenEx, SetTokenInformation, SecurityIdentification, TokenPrimary, TokenSessionId,
    TOKEN_ACCESS_MASK,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Environment::{
    CreateEnvironmentBlock, DestroyEnvironmentBlock,
};
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTSGetActiveConsoleSessionId,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetExitCodeProcess, OpenProcess, OpenProcessToken, TerminateProcess,
    WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType, SessionChangeReason,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::install::{self, SERVICE_NAME};
use crate::{run_boot_gamepad_loop, App};

const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
const WORKER_ARGS: &[&str] = &["--service-worker", "--boot", "--cfg-winlogon"];
const WAIT_SLICE_MS: u32 = 1000;
/// `WTSGetActiveConsoleSessionId` when no interactive session exists yet (pre-logon / boot).
const INVALID_CONSOLE_SESSION: u32 = 0xFFFF_FFFF;
const LAUNCH_RETRY_SECS: u64 = 2;

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static CHILD_PROCESS: AtomicIsize = AtomicIsize::new(0);

define_windows_service!(ffi_service_main, service_main);

/// Ok(()) when this process was started by SCM and the service ran to completion.
pub fn run_dispatcher() -> Result<(), String> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| format!("service dispatcher: {e}"))
}

fn service_main(_arguments: Vec<OsString>) {
    std::panic::set_hook(Box::new(|info| {
        install::log_line(&format!("PANIC: {info}"));
    }));
    match run_service_core() {
        Ok(()) => install::log_line("service main finished OK"),
        Err(e) => install::log_line(&format!("service exited with error: {e}")),
    }
}

fn run_service_core() -> Result<(), String> {
    std::env::set_var("WARMUP_VK_SERVICE", "1");
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    RESTART_REQUESTED.store(false, Ordering::SeqCst);
    install::log_line("WarmupVkSvc starting (launcher branch)");

    let status_handle = service_control_handler::register(SERVICE_NAME, move |event| {
        match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                install::log_line("service stop requested");
                crate::gamepad::request_stop();
                STOP_REQUESTED.store(true, Ordering::SeqCst);
                terminate_child();
                ServiceControlHandlerResult::NoError
            }
            // Win+L lock/unlock must not kill the worker — LockApp uses the input desktop (often Default).
            ServiceControl::SessionChange(change)
                if matches!(
                    change.reason,
                    SessionChangeReason::ConsoleConnect
                        | SessionChangeReason::SessionLogon
                        | SessionChangeReason::SessionLogoff
                ) =>
            {
                install::log_line(&format!("session change {:?}; relaunch requested", change.reason));
                RESTART_REQUESTED.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })
    .map_err(|e| format!("register service handler: {e}"))?;

    report_status(
        &status_handle,
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
    )?;
    report_status(
        &status_handle,
        ServiceState::Running,
        ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::SESSION_CHANGE,
    )?;

    let service_result = launcher_loop();
    let exit_code = if service_result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::ServiceSpecific(1)
    };
    report_status_stopped(&status_handle, exit_code)?;

    service_result
}

/// Worker child launched into active console session. Owns XInput polling and VK UI.
pub fn run_worker() -> Result<(), String> {
    std::env::set_var("WARMUP_VK_SERVICE", "1");
    install::log_line("service worker starting (XInput + VK UI)");

    let mut app = App::default();
    app.use_real_win32 = true;
    app.configure_boot_service();
    install::log_line("boot path active; tap Y on sign-in / UAC to open VK");

    let vk_open = std::cell::Cell::new(false);
    let gamepad_result = run_boot_gamepad_loop(&mut app, &vk_open, true);

    if let Some(session) = app.vk_session.take() {
        session.close();
    }

    match &gamepad_result {
        Ok(()) => install::log_line("service worker gamepad loop returned OK"),
        Err(e) => install::log_line(&format!("service worker gamepad loop failed: {e}")),
    }
    gamepad_result
}

fn launcher_loop() -> Result<(), String> {
    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        let child = match launch_worker_in_active_session() {
            Ok(c) => c,
            Err(e) => {
                install::log_line(&format!(
                    "worker launch waiting (sign-in may not be ready): {e}"
                ));
                for _ in 0..15 {
                    if STOP_REQUESTED.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                continue;
            }
        };
        let raw = child.handle.0 as isize;
        CHILD_PROCESS.store(raw, Ordering::SeqCst);
        install::log_line(&format!("service worker launched pid={}", child.pid));

        loop {
            if STOP_REQUESTED.load(Ordering::SeqCst) {
                terminate_child();
                return Ok(());
            }
            if RESTART_REQUESTED.swap(false, Ordering::SeqCst) {
                install::log_line("restarting service worker after session change");
                terminate_child();
                break;
            }
            let wait = unsafe { WaitForSingleObject(child.handle, WAIT_SLICE_MS) };
            if wait == WAIT_TIMEOUT {
                continue;
            }
            CHILD_PROCESS.compare_exchange(raw, 0, Ordering::SeqCst, Ordering::SeqCst).ok();
            unsafe {
                let _ = CloseHandle(child.handle);
            }
            if wait == WAIT_OBJECT_0 {
                let code = worker_exit_code(child.handle);
                install::log_line(&format!("service worker exited code={code}; relaunching"));
            } else {
                install::log_line(&format!("service worker wait returned {}", wait.0));
            }
            std::thread::sleep(Duration::from_secs(LAUNCH_RETRY_SECS));
            break;
        }
    }
    Ok(())
}

struct WorkerProcess {
    handle: HANDLE,
    pid: u32,
}

fn terminate_child() {
    let raw = CHILD_PROCESS.swap(0, Ordering::SeqCst);
    if raw == 0 {
        return;
    }
    let handle = HANDLE(raw as *mut _);
    unsafe {
        let _ = TerminateProcess(handle, 0);
        let _ = CloseHandle(handle);
    }
}

fn launch_worker_in_active_session() -> Result<WorkerProcess, String> {
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

unsafe fn duplicate_primary_token_for_session(
    process: HANDLE,
    session_id: u32,
) -> Result<HANDLE, String> {
    let mut token = HANDLE::default();
    OpenProcessToken(process, TOKEN_ACCESS_MASK(0x201eb), &mut token)
        .map_err(|e| format!("OpenProcessToken(winlogon): {e}"))?;

    let mut primary = HANDLE::default();
    let dup = DuplicateTokenEx(
        token,
        TOKEN_ACCESS_MASK(0x2000000),
        None,
        SecurityIdentification,
        TokenPrimary,
        &mut primary,
    );
    let _ = CloseHandle(token);
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
    Ok(primary)
}

unsafe fn create_worker_process(token: HANDLE) -> Result<WorkerProcess, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let exe_w = wide_os(exe.as_os_str());
    let mut cmd_w = wide(&command_line(&exe, WORKER_ARGS));
    // Joyxoff-style: Default desktop in session; app attaches via OpenInputDesktop (lock/logon/UAC).
    let mut desktop = wide("winsta0\\default");
    let mut startup = STARTUPINFOW {
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
    let flags = if env_created {
        CREATE_UNICODE_ENVIRONMENT
    } else {
        PROCESS_CREATION_FLAGS(0)
    };

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
        &mut startup,
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
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn wide_os(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn worker_exit_code(handle: HANDLE) -> u32 {
    let mut code = 0u32;
    unsafe {
        let _ = GetExitCodeProcess(handle, &mut code);
    }
    code
}

fn report_status_stopped(
    handle: &service_control_handler::ServiceStatusHandle,
    exit_code: ServiceExitCode,
) -> Result<(), String> {
    handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .map_err(|e| format!("SetServiceStatus(Stopped): {e}"))
}

fn report_status(
    handle: &service_control_handler::ServiceStatusHandle,
    state: ServiceState,
    controls: ServiceControlAccept,
) -> Result<(), String> {
    handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted: controls,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .map_err(|e| format!("SetServiceStatus: {e}"))
}

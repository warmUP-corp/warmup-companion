//! Windows service entry: SCM launcher plus worker process for sign-in VK.

#![cfg(all(windows, feature = "service"))]

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType, SessionChangeReason,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::install::{self, SERVICE_NAME};
use crate::service_worker;
use crate::warmup_launch::run_boot_gamepad_loop;
use crate::App;

const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
const WAIT_SLICE_MS: u32 = 1000;
const GRACEFUL_STOP_MS: u32 = 5000;
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
        crate::sentry_telemetry::capture_panic(info, "service-launcher");
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

    let status_handle = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            install::log_line("service stop requested");
            crate::gamepad::request_stop();
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            terminate_child();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(change) => {
            // User reached or reconnected to the desktop: keep the worker alive and
            // let the gamepad loop switch backend. Hard-restart tears down VK and
            // loses controller state mid-transition.
            let restart = matches!(change.reason, SessionChangeReason::SessionLogoff);
            install::log_line(&format!(
                "session change {:?}{}",
                change.reason,
                if restart {
                    "; worker restart requested"
                } else {
                    " (no worker restart)"
                }
            ));
            if restart {
                RESTART_REQUESTED.store(true, Ordering::SeqCst);
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
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
    std::panic::set_hook(Box::new(|info| {
        crate::sentry_telemetry::capture_panic(info, "service-worker");
        install::log_line(&format!("WORKER PANIC: {info}"));
    }));
    std::env::set_var("WARMUP_VK_SERVICE", "1");
    // Catch native access violations (0xC0000005) the panic hook can't see: logs
    // module+rva and writes a minidump before the worker dies. See src/crash.rs.
    crate::crash::install();
    install::log_line("service worker starting (XInput + VK UI)");
    install::log_line(r"service log file: C:\ProgramData\WarmupVk\service.log");

    let mut app = App::default();
    app.use_real_win32 = true;
    app.configure_boot_service();
    install::log_line("boot path active; tap Y on sign-in / UAC to open VK");

    // NOTE: do NOT create an anchor window on this (loop) thread. The loop thread
    // calls SetThreadDesktop via sync_service_backend on every desktop transition;
    // owning a window pins it to Winlogon (SetThreadDesktop -> ERROR_BUSY 0x800700AA),
    // so SendInput / desktop migration breaks for the worker's lifetime. The window
    // required for HID/XInput delivery on Winlogon lives on the XInput backend's own
    // dedicated poll thread (WarmupXInputAnchorWindow, xinput_backend.rs), which never
    // migrates desktops.

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
        let child = match service_worker::launch_worker_in_active_session() {
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
                install::log_line("stopping service worker after session change (graceful)");
                stop_child_gracefully(child.handle);
                break;
            }
            let wait = unsafe { WaitForSingleObject(child.handle, WAIT_SLICE_MS) };
            if wait == WAIT_TIMEOUT {
                continue;
            }
            CHILD_PROCESS
                .compare_exchange(raw, 0, Ordering::SeqCst, Ordering::SeqCst)
                .ok();
            if wait == WAIT_OBJECT_0 {
                let code = service_worker::worker_exit_code(child.handle);
                install::log_line(&format!("service worker exited code={code}; relaunching"));
            } else {
                install::log_line(&format!("service worker wait returned {}", wait.0));
            }
            unsafe {
                let _ = CloseHandle(child.handle);
            }
            std::thread::sleep(Duration::from_secs(LAUNCH_RETRY_SECS));
            break;
        }
    }
    Ok(())
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

/// Wait for the worker to exit after session change; avoid `TerminateProcess` when possible.
fn stop_child_gracefully(handle: HANDLE) {
    unsafe {
        let wait = WaitForSingleObject(handle, GRACEFUL_STOP_MS);
        if wait == WAIT_OBJECT_0 {
            let code = service_worker::worker_exit_code(handle);
            install::log_line(&format!("service worker exited gracefully (code={code})"));
        } else {
            install::log_line("service worker did not exit in time; terminating");
            let _ = TerminateProcess(handle, 0);
        }
        let _ = CloseHandle(handle);
    }
    CHILD_PROCESS.store(0, Ordering::SeqCst);
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

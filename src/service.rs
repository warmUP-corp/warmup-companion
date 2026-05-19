//! Windows service entry (`--service`): boot + winlogon attach, gamepad VK at sign-in.

#![cfg(all(windows, feature = "service"))]

use std::ffi::OsString;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::install::{self, SERVICE_NAME};
use crate::{run_boot_gamepad_loop, App};

const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

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
    install::log_line("WarmupVkSvc starting (features: service+gamepad, SDL3/enigo)");

    let status_handle = service_control_handler::register(SERVICE_NAME, move |event| {
        match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                install::log_line("service stop requested");
                crate::gamepad::request_stop();
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
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    )?;

    let mut app = App::default();
    app.use_real_win32 = true;
    app.configure_boot_service();
    install::log_line("boot path active; tap Y on sign-in / UAC to open VK");

    let vk_open = std::cell::Cell::new(false);
    let gamepad_result = run_boot_gamepad_loop(&mut app, &vk_open, true);

    if let Some(session) = app.vk_session.take() {
        session.close();
    }

    let exit_code = if gamepad_result.is_ok() {
        install::log_line("gamepad loop returned OK");
        ServiceExitCode::Win32(0)
    } else {
        let msg = gamepad_result.as_ref().err().map(String::as_str).unwrap_or("?");
        install::log_line(&format!("gamepad loop failed: {msg}"));
        ServiceExitCode::ServiceSpecific(1)
    };

    report_status_stopped(&status_handle, exit_code)?;

    gamepad_result
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

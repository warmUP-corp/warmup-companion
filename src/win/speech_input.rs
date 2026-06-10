//! VK mic key speech input.
//!
//! The companion worker runs as SYSTEM (winlogon's duplicated token) inside the
//! active console session — see `service::launch_worker_in_active_session`. That
//! token has no per-user microphone consent, so WinRT `SpeechRecognizer` returns
//! `E_ACCESSDENIED` (0x80070005) when constructed in the worker.
//!
//! So the worker does NOT recognize speech itself. On the mic toggle it spawns a
//! short-lived helper process as the *real logged-in user* (`--speech-helper`,
//! launched via `WTSQueryUserToken` + `CreateProcessAsUserW`, mirroring
//! `main::spawn_warmup_as_active_user`). The helper has the user's normal mic
//! consent, recognizes dictation, and injects the text with `SendInput` on
//! `winsta0\default` — it *is* the user, on the user's desktop. Toggling the mic
//! again kills the helper.
//!
//! Recognition is blocked on the Winlogon/secure desktop anyway (the caller in
//! `vk_nav::start_voice_input` bails on `logon_focus::is_active`), so the helper
//! only ever runs on the default desktop where the user has audio + consent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs, SpeechContinuousRecognitionMode,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint,
    SpeechRecognizer,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetExitCodeProcess, TerminateProcess, CREATE_NEW_PROCESS_GROUP,
    CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    STARTUPINFOW,
};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

const AUTO_STOP_SILENCE_TIMEOUT: TimeSpan = TimeSpan {
    // Windows TimeSpan is measured in 100ns ticks. Keep dictation open long
    // enough for gamepad use instead of dropping the mic after a short pause.
    Duration: 10 * 60 * 10_000_000,
};

/// `WTSGetActiveConsoleSessionId` sentinel for "no interactive session yet".
const INVALID_CONSOLE_SESSION: u32 = 0xFFFF_FFFF;
/// `GetExitCodeProcess` returns this while the process is still running.
const STILL_ACTIVE: u32 = 259;

// ---------------------------------------------------------------------------
// Worker side: helper process lifecycle.
// ---------------------------------------------------------------------------

struct Helper {
    process: HANDLE,
}

// HANDLE is a raw pointer wrapper; we only ever touch it under the mutex.
unsafe impl Send for Helper {}

static HELPER: Mutex<Option<Helper>> = Mutex::new(None);

fn process_alive(process: HANDLE) -> bool {
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(process, &mut code).is_ok() && code == STILL_ACTIVE }
}

/// Spawn the speech helper as the real logged-in user, if one is not already
/// running. Returns Ok if a helper is now (or was already) running.
pub fn start_helper() -> Result<(), String> {
    let mut guard = HELPER
        .lock()
        .map_err(|_| "speech helper lock poisoned".to_string())?;
    if let Some(h) = guard.as_ref() {
        if process_alive(h.process) {
            return Ok(());
        }
        // Previous helper exited on its own (silence auto-stop / crash) — reap it.
        unsafe {
            let _ = CloseHandle(h.process);
        }
        *guard = None;
    }

    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let process = spawn_helper_as_user(&exe)?;
    *guard = Some(Helper { process });
    Ok(())
}

/// Kill the running helper, if any. Safe to call when none is running.
pub fn stop_helper() {
    let taken = HELPER.lock().ok().and_then(|mut g| g.take());
    if let Some(h) = taken {
        unsafe {
            let _ = TerminateProcess(h.process, 0);
            let _ = CloseHandle(h.process);
        }
    }
}

/// Launch `<exe> --speech-helper` as the active console user on `winsta0\default`.
/// Mirrors `main::spawn_warmup_as_active_user`; returns the child process handle.
fn spawn_helper_as_user(exe: &std::path::Path) -> Result<HANDLE, String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    fn wide_os(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    unsafe {
        let session_id = WTSGetActiveConsoleSessionId();
        if session_id == INVALID_CONSOLE_SESSION {
            return Err("no active console session (cannot run mic as user)".into());
        }
        let mut token = HANDLE::default();
        WTSQueryUserToken(session_id, &mut token)
            .map_err(|e| format!("WTSQueryUserToken(session={session_id}): {e}"))?;

        let exe_w = wide_os(exe.as_os_str());
        let mut cmd_w = wide(&format!("\"{}\" --speech-helper", exe.display()));
        let cwd_w = exe.parent().map(|parent| wide_os(parent.as_os_str()));
        let mut desktop = wide("winsta0\\default");
        let mut startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        let mut env = std::ptr::null_mut();
        let env_created = CreateEnvironmentBlock(&mut env, token, false).is_ok();
        let env_arg = if env_created {
            Some(env.cast_const().cast())
        } else {
            None
        };
        let cwd_arg = cwd_w
            .as_ref()
            .map(|cwd| PCWSTR(cwd.as_ptr()))
            .unwrap_or_else(PCWSTR::null);
        let flags = CREATE_UNICODE_ENVIRONMENT
            | PROCESS_CREATION_FLAGS(DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0);

        let created = CreateProcessAsUserW(
            token,
            PCWSTR(exe_w.as_ptr()),
            PWSTR(cmd_w.as_mut_ptr()),
            None,
            None,
            false,
            flags,
            env_arg,
            cwd_arg,
            &mut startup,
            &mut info,
        );
        if env_created {
            let _ = DestroyEnvironmentBlock(env);
        }
        let _ = CloseHandle(token);
        created.map_err(|e| format!("CreateProcessAsUserW({}): {e}", exe.display()))?;
        let _ = CloseHandle(info.hThread);
        Ok(info.hProcess)
    }
}

// ---------------------------------------------------------------------------
// Helper side: actual recognition (`--speech-helper`, runs as the real user).
// ---------------------------------------------------------------------------

/// Set by the `Completed` handler so [`run_blocking`] can park until the session
/// ends. Process-global is fine: the helper runs exactly one recognition session
/// per process, then exits.
static DONE: AtomicBool = AtomicBool::new(false);

/// Run one continuous-dictation session to completion, injecting recognized text
/// via `SendInput`. Blocks until the session auto-stops (silence) or the worker
/// kills this process. Runs as the logged-in user, so mic consent applies here.
pub fn run_blocking() -> Result<(), String> {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    let recognizer = SpeechRecognizer::new().map_err(|e| format!("SpeechRecognizer: {e}"))?;
    let constraint = SpeechRecognitionTopicConstraint::Create(
        SpeechRecognitionScenario::Dictation,
        &HSTRING::new(),
    )
    .map_err(|e| format!("SpeechRecognitionTopicConstraint: {e}"))?;
    recognizer
        .Constraints()
        .map_err(|e| format!("SpeechRecognizer.Constraints: {e}"))?
        .Append(&constraint)
        .map_err(|e| format!("SpeechRecognizer constraints append: {e}"))?;
    let compile = recognizer
        .CompileConstraintsAsync()
        .map_err(|e| format!("CompileConstraintsAsync: {e}"))?
        .get()
        .map_err(|e| format!("CompileConstraintsAsync.get: {e}"))?;
    if compile
        .Status()
        .map_err(|e| format!("compile status: {e}"))?
        != SpeechRecognitionResultStatus::Success
    {
        return Err(format!(
            "speech constraint compile failed: {:?}",
            compile.Status().unwrap_or_default()
        ));
    }

    let session = recognizer
        .ContinuousRecognitionSession()
        .map_err(|e| format!("ContinuousRecognitionSession: {e}"))?;
    let _ = session.SetAutoStopSilenceTimeout(AUTO_STOP_SILENCE_TIMEOUT);

    let result_token = session
        .ResultGenerated(&TypedEventHandler::new(
            move |_sender: &Option<SpeechContinuousRecognitionSession>,
                  args: &Option<SpeechContinuousRecognitionResultGeneratedEventArgs>| {
                if let Some(args) = args {
                    if let Ok(result) = args.Result() {
                        if result.Status()? == SpeechRecognitionResultStatus::Success {
                            let text = result.Text()?.to_string();
                            if !text.trim().is_empty() {
                                crate::vk_nav::send_text_direct(&format!("{} ", text.trim()));
                            }
                        }
                    }
                }
                Ok(())
            },
        ))
        .map_err(|e| format!("ResultGenerated: {e}"))?;

    let completed_token = session
        .Completed(&TypedEventHandler::new(
            move |_sender: &Option<SpeechContinuousRecognitionSession>,
                  args: &Option<SpeechContinuousRecognitionCompletedEventArgs>| {
                if let Some(args) = args {
                    match args.Status() {
                        Ok(status) => crate::install::log_line(&format!(
                            "speech helper completed: {status:?}"
                        )),
                        Err(e) => crate::install::log_line(&format!(
                            "speech helper completed: status unavailable ({e})"
                        )),
                    }
                }
                DONE.store(true, Ordering::SeqCst);
                Ok(())
            },
        ))
        .map_err(|e| format!("Completed: {e}"))?;

    session
        .StartWithModeAsync(SpeechContinuousRecognitionMode::Default)
        .map_err(|e| format!("StartWithModeAsync: {e}"))?
        .get()
        .map_err(|e| format!("StartWithModeAsync.get: {e}"))?;

    // Park until the session ends. The worker kills this process for a manual
    // toggle-off; the `Completed` handler flips `DONE` for the silence auto-stop.
    while !DONE.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let _ = session.RemoveResultGenerated(result_token);
    let _ = session.RemoveCompleted(completed_token);
    let _ = session.StopAsync().and_then(|a| a.get());
    let _ = recognizer.Close();
    Ok(())
}

//! Gamepad boot loop and warmup.exe discovery / spawn helpers.

use std::env;

use crate::symbols::G_BOOT_SERVICE_MODE;
use crate::App;

#[cfg(feature = "gamepad")]
pub fn run_gamepad_mode() {
    let use_real = env::args().any(|a| a == "--real")
        || env::var_os("WARMUP_REAL_VK").is_some_and(|v| v != "0")
        || cfg!(windows);
    let args: Vec<String> = env::args().collect();
    let mut app = App::default();
    app.use_real_win32 = use_real;
    if args.iter().any(|a| a == "--boot") {
        app.start_boot();
        println!("> --boot: service path + {G_BOOT_SERVICE_MODE}");
    }
    if args
        .iter()
        .any(|a| a == "--cfg-winlogon" || a == "--winlogon")
    {
        app.config_winlogon_0xd9 = true;
        println!("> --cfg-winlogon: config +0xd9 set");
        if app.boot_mode {
            app.attach_named(crate::Desktop::Winlogon);
        }
    }
    println!("Warmup Companion gamepad mode — sticks move mouse; L3 toggles VK");
    if use_real {
        println!("real Win32 VK enabled (WarmupXboxVkWindow)");
    }
    println!("Sign-in service: build default release, then `install` as Admin");
    crate::repl_scroll::paint_state_panel(&app);
    let vk_open = std::cell::Cell::new(false);
    let result = run_boot_gamepad_loop(&mut app, &vk_open, false);
    if let Some(session) = app.vk_session.take() {
        session.close();
    }
    match result {
        Ok(()) => println!("> exited"),
        Err(e) => {
            eprintln!("gamepad: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "gamepad")]
pub(crate) fn run_boot_gamepad_loop(
    app: &mut App,
    vk_open: &std::cell::Cell<bool>,
    service_mode: bool,
) -> Result<(), String> {
    // The companion owns the device; host the pipe so the warmUP desktop can read
    // connection state over IPC (#347). No-op on non-Windows.
    crate::pipe_server::spawn();
    #[cfg(windows)]
    crate::parental_guard::spawn_guardian_loop();
    #[cfg(windows)]
    crate::playtime_tracker::spawn_tracker_loop();
    let on_action = |action: crate::gamepad::VkLoopAction| match action {
        crate::gamepad::VkLoopAction::Toggle => {
            app.toggle_virtual_keyboard_combo();
            vk_open.set(app.vk_session.is_some());
            if !service_mode {
                crate::repl_scroll::paint_state_panel(&*app);
            } else {
                #[cfg(windows)]
                {
                    if app.vk_session.is_some() {
                        let vis = crate::win::is_vk_visible();
                        crate::install::log_line(&format!("VK opened (window visible={vis})"));
                    } else {
                        crate::install::log_line("VK closed");
                    }
                }
            }
        }
        crate::gamepad::VkLoopAction::Close => {
            app.close_vk();
            vk_open.set(false);
            if !service_mode {
                crate::repl_scroll::paint_state_panel(&*app);
            } else {
                #[cfg(windows)]
                crate::install::log_line("VK closed");
            }
        }
        crate::gamepad::VkLoopAction::Reopen => {
            app.close_vk();
            let attach = crate::vk_gate::attach_for(app.gate_input());
            app.open_xbox_vk(attach);
            vk_open.set(app.vk_session.is_some());
            if !service_mode {
                crate::repl_scroll::paint_state_panel(&*app);
            } else {
                #[cfg(windows)]
                {
                    if app.vk_session.is_some() {
                        let vis = crate::win::is_vk_visible();
                        crate::install::log_line(&format!("VK reopened (window visible={vis})"));
                    } else {
                        crate::install::log_line("VK reopen failed");
                    }
                }
            }
        }
        crate::gamepad::VkLoopAction::LaunchWarmup => {
            if let Err(e) = launch_warmup_exe() {
                eprintln!("launch warmup.exe: {e}");
                #[cfg(windows)]
                if service_mode {
                    crate::install::log_line(&format!("launch warmup.exe failed: {e}"));
                }
            } else {
                #[cfg(windows)]
                if service_mode {
                    crate::install::log_line("launched warmup.exe from controller hotkey");
                }
            }
        }
    };
    if service_mode {
        crate::gamepad::run_watch_loop_service(|| vk_open.get(), on_action)
    } else {
        crate::gamepad::run_watch_loop(|| vk_open.get(), on_action)
    }
}

#[cfg(feature = "gamepad")]
fn launch_warmup_exe() -> Result<(), String> {
    let exe = warmup_exe_path()?;
    spawn_warmup(&exe).map_err(|e| format!("{}: {e}", exe.display()))
}

/// True if a `warmup.exe` can be located (same resolution as launch). Used by the
/// launch hotkey so it gives honest feedback instead of buzzing "success" when
/// there's nothing to open.
#[cfg(feature = "gamepad")]
pub(crate) fn warmup_installed() -> bool {
    warmup_exe_path().is_ok()
}

#[cfg(any(feature = "gamepad", windows))]
pub(crate) fn warmup_exe_path() -> Result<std::path::PathBuf, String> {
    if let Some(path) = std::env::var_os("WARMUP_EXE") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("WARMUP_EXE does not exist: {}", path.display()));
    }

    if let Ok(raw) = std::fs::read_to_string(crate::install::DEV_EXE_PATH) {
        let path = std::path::PathBuf::from(raw.trim().trim_matches('"'));
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "{} points to missing exe: {}",
            crate::install::DEV_EXE_PATH,
            path.display()
        ));
    }

    let current = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let dir = current
        .parent()
        .ok_or_else(|| format!("current exe has no parent: {}", current.display()))?;
    let mut candidates = Vec::new();
    candidates.push(dir.join("warmup.exe"));
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(std::path::PathBuf::from(program_files).join(r"warmUP\warmup.exe"));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(std::path::PathBuf::from(program_files_x86).join(r"warmUP\warmup.exe"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let local_app_data = std::path::PathBuf::from(local_app_data);
        candidates.push(local_app_data.join(r"dev.warmup.console\warmup.exe"));
        candidates.push(local_app_data.join(r"warmUP\warmup.exe"));
        candidates.push(local_app_data.join(r"Programs\warmUP\warmup.exe"));
    }
    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        candidates.push(
            std::path::PathBuf::from(user_profile)
                .join(r"warmUp\apps\desktop\src-tauri\target\debug\warmup.exe"),
        );
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "warmup.exe not found; set WARMUP_EXE or write the full path to {}",
                crate::install::DEV_EXE_PATH
            )
        })
}

#[cfg(all(feature = "gamepad", windows))]
fn spawn_warmup(exe: &std::path::Path) -> std::io::Result<()> {
    if crate::config::service_mode() {
        return spawn_warmup_as_active_user(exe);
    }

    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
}

#[cfg(all(feature = "gamepad", windows))]
fn spawn_warmup_as_active_user(exe: &std::path::Path) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
        DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
    };

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
        let mut token = Default::default();
        WTSQueryUserToken(session_id, &mut token)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let exe_w = wide_os(exe.as_os_str());
        let mut cmd_w = wide(&format!("\"{}\"", exe.display()));
        let cwd_w = exe.parent().map(|parent| wide_os(parent.as_os_str()));
        let mut desktop = wide("winsta0\\default");
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        let mut env = std::ptr::null_mut();
        if let Err(error) = CreateEnvironmentBlock(&mut env, token, false) {
            let _ = CloseHandle(token);
            return Err(std::io::Error::other(error.to_string()));
        }
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
            Some(env.cast_const().cast()),
            cwd_arg,
            &startup,
            &mut info,
        );
        let _ = DestroyEnvironmentBlock(env);
        let _ = CloseHandle(token);
        created.map_err(|e| std::io::Error::other(e.to_string()))?;
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(())
}

#[cfg(all(feature = "gamepad", not(windows)))]
fn spawn_warmup(exe: &std::path::Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    cmd.spawn().map(|_| ())
}

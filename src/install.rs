//! Install / uninstall Warmup Companion Windows service (`WarmupVkSvc`).

#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SERVICE_NAME: &str = "WarmupVkSvc";
const DISPLAY_NAME: &str = "Warmup Companion";
const DESCRIPTION: &str =
    "Companion service for Warmup gamepad input, sign-in keyboard, and UAC support.";

/// No spaces — `sc.exe` breaks on quoted paths under Program Files.
pub const INSTALL_DIR: &str = r"C:\ProgramData\WarmupVk\bin";
pub const DATA_DIR: &str = r"C:\ProgramData\WarmupVk";
const EXE_NAME: &str = "warmup-companion.exe";
const LEGACY_EXE_NAME: &str = "warmup-vk-prototype.exe";
const LOG_NAME: &str = "service.log";
const DEBUG_UI_FLAG: &str = r"C:\ProgramData\WarmupVk\debug-ui.enabled";

/// Leftover names from manual `sc create` debugging — removed on install/uninstall.
const TEST_SERVICE_NAMES: &[&str] = &[
    "WarmupVkTest",
    "WarmupVkTest2",
    "WarmupVkTest3",
    "WarmupVkTest4",
    "WarmupVkTest5",
    "WarmupVkTest6",
];

pub fn run_install(debug_ui: bool) {
    if let Err(e) = install_inner(debug_ui) {
        eprintln!("install failed: {e}");
        std::process::exit(1);
    }
    let bin = Path::new(INSTALL_DIR).join(EXE_NAME);
    println!("Installed Warmup Companion. Service {SERVICE_NAME} started.");
    println!("Binary (SCM uses this): {}", bin.display());
    println!("NOT C:\\Program Files\\WarmupVk\\ — that path is legacy only.");
    println!("Reboot or Win+L, then tap Y on the controller at the password screen.");
    println!("Log: {DATA_DIR}\\{LOG_NAME}");
    println!(
        "Debug UI: {}",
        if debug_ui { "enabled" } else { "disabled" }
    );
}

pub fn run_uninstall() {
    if let Err(e) = uninstall_inner() {
        eprintln!("uninstall failed: {e}");
        std::process::exit(1);
    }
    println!("Uninstalled {SERVICE_NAME}.");
}

/// Stop the service without deleting it. Use before manual `cargo run -- install`
/// so the installer can overwrite the locked exe.
pub fn run_stop() {
    if let Err(e) = require_admin() {
        eprintln!("stop failed: {e}");
        std::process::exit(1);
    }
    match stop_service_blocking() {
        Ok(StopOutcome::Stopped) => println!("Service {SERVICE_NAME} stopped."),
        Ok(StopOutcome::NotInstalled) => println!("Service {SERVICE_NAME} not installed."),
        Ok(StopOutcome::AlreadyStopped) => println!("Service {SERVICE_NAME} already stopped."),
        Err(e) => {
            eprintln!("stop failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Escape hatch: ask the SCM to stop the service from inside the worker process
/// (the debug overlay's F8). The worker runs under the duplicated Winlogon
/// (LocalSystem) token, which has `SERVICE_STOP` rights, so a detached `sc stop`
/// reaches the launcher's control handler — which tears the worker down. Spawned
/// with `CREATE_NO_WINDOW` so no console flashes on the secure desktop.
pub fn request_service_stop() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    log_line("debug ui: stop service requested (sc stop)");
    if let Err(e) = Command::new("sc")
        .args(["stop", SERVICE_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        log_line(&format!("debug ui: sc stop spawn failed: {e}"));
    }
}

fn install_inner(debug_ui: bool) -> Result<(), String> {
    require_admin()?;
    remove_legacy_install_artifacts();
    let src = std::env::current_exe().map_err(|e| e.to_string())?;
    fs::create_dir_all(INSTALL_DIR).map_err(|e| e.to_string())?;
    fs::create_dir_all(DATA_DIR).map_err(|e| e.to_string())?;
    set_debug_ui_flag(debug_ui)?;

    // Stop + delete BEFORE copying — old exe is locked by the running service.
    remove_test_services();
    uninstall_service_quiet();

    let dest = Path::new(INSTALL_DIR).join(EXE_NAME);
    let legacy_dest = Path::new(INSTALL_DIR).join(LEGACY_EXE_NAME);
    if legacy_dest.exists() {
        let _ = fs::remove_file(&legacy_dest);
    }
    fs::copy(&src, &dest).map_err(|e| format!("copy exe to {INSTALL_DIR}: {e}"))?;
    if !dest.is_file() {
        return Err(format!(
            "install copy missing: {} (service will not start)",
            dest.display()
        ));
    }
    copy_gamecontroller_db()?;
    // sc.exe: `binPath=` and path are separate argv tokens; no quotes (path has no spaces).
    // SCM starts the exe directly; main() dispatches to service_dispatcher when argc == 1.
    let exe = dest.display().to_string();
    sc(&["create", SERVICE_NAME, "binPath=", &exe])?;
    // sc.exe wants `start=` and `auto` as separate argv tokens (not one `start= auto` string).
    sc(&["config", SERVICE_NAME, "start=", "auto"])?;
    sc(&["config", SERVICE_NAME, "DisplayName=", DISPLAY_NAME])?;
    sc(&["description", SERVICE_NAME, "Description=", DESCRIPTION])?;
    // LocalSystem (default) — required for winlogon / sign-in desktop.
    sc(&["start", SERVICE_NAME])?;
    verify_service_running()?;
    log_line(&format!(
        "installed from {} -> {} (debug_ui={debug_ui})",
        src.display(),
        dest.display()
    ));
    Ok(())
}

fn uninstall_inner() -> Result<(), String> {
    require_admin()?;
    remove_test_services();
    uninstall_service_quiet();
    let exe = Path::new(INSTALL_DIR).join(EXE_NAME);
    if exe.exists() {
        fs::remove_file(&exe).ok();
    }
    let old_exe = Path::new(INSTALL_DIR).join(LEGACY_EXE_NAME);
    if old_exe.exists() {
        fs::remove_file(&old_exe).ok();
    }
    // Legacy path from earlier installer attempts.
    let legacy = Path::new(r"C:\Program Files\WarmupVk").join(LEGACY_EXE_NAME);
    if legacy.exists() {
        fs::remove_file(&legacy).ok();
    }
    let _ = fs::remove_file(DEBUG_UI_FLAG);
    log_line("uninstalled");
    Ok(())
}

fn set_debug_ui_flag(enabled: bool) -> Result<(), String> {
    let flag = Path::new(DEBUG_UI_FLAG);
    if enabled {
        fs::write(flag, b"1\n").map_err(|e| format!("write debug UI flag: {e}"))?;
        log_line("debug ui enabled by installer flag");
    } else {
        let _ = fs::remove_file(flag);
        log_line("debug ui disabled (no installer flag)");
    }
    Ok(())
}

fn uninstall_service_quiet() {
    let _ = stop_service_blocking();
    let _ = sc(&["delete", SERVICE_NAME]);
}

enum StopOutcome {
    Stopped,
    AlreadyStopped,
    NotInstalled,
}

/// Issue `sc stop`, then poll `sc query` until STOPPED or timeout.
/// Avoids the race where `sc delete` runs before the worker child has fully exited
/// and released its handle on the install dir exe.
fn stop_service_blocking() -> Result<StopOutcome, String> {
    match query_service_state()? {
        Some(state) if state == "STOPPED" => return Ok(StopOutcome::AlreadyStopped),
        None => return Ok(StopOutcome::NotInstalled),
        _ => {}
    }
    let _ = sc(&["stop", SERVICE_NAME]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(500));
        match query_service_state()? {
            Some(state) if state == "STOPPED" => return Ok(StopOutcome::Stopped),
            None => return Ok(StopOutcome::NotInstalled),
            _ => {}
        }
    }
    Err(format!("{SERVICE_NAME} did not reach STOPPED within 15s"))
}

/// Returns `Ok(None)` if the service is not installed, or the current state token
/// (e.g. "RUNNING", "STOPPED", "STOP_PENDING") parsed out of `sc query`.
fn query_service_state() -> Result<Option<String>, String> {
    let out = Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .output()
        .map_err(|e| format!("sc.exe query: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // 1060 = ERROR_SERVICE_DOES_NOT_EXIST
    if text.contains("1060") || stderr.contains("1060") {
        return Ok(None);
    }
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("STATE") {
            // "STATE              : 4  RUNNING"
            return Ok(rest.split_whitespace().last().map(|tok| tok.to_string()));
        }
    }
    Ok(None)
}

/// Old manual installs and terminal runs used `C:\Program Files\WarmupVk\`.
fn remove_legacy_install_artifacts() {
    let legacy_dir = Path::new(r"C:\Program Files\WarmupVk");
    let legacy_exe = legacy_dir.join(EXE_NAME);
    if legacy_exe.is_file() {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", EXE_NAME])
            .output();
        std::thread::sleep(std::time::Duration::from_millis(500));
        if fs::remove_file(&legacy_exe).is_ok() {
            log_line(&format!("removed legacy {}", legacy_exe.display()));
        }
        let _ = fs::remove_dir(legacy_dir);
    }
}

fn remove_test_services() {
    for name in TEST_SERVICE_NAMES {
        let _ = sc(&["stop", name]);
        match sc(&["delete", name]) {
            Ok(()) => log_line(&format!("removed test service {name}")),
            Err(_) => {}
        }
    }
}

fn copy_gamecontroller_db() -> Result<(), String> {
    let candidates = [std::env::var_os("WARMUP_GAMECONTROLLER_DB").map(PathBuf::from)];
    let dest = Path::new(DATA_DIR).join("gamecontrollerdb.txt");
    for c in candidates.into_iter().flatten() {
        if c.is_file() {
            fs::copy(&c, &dest).map_err(|e| format!("copy controller DB: {e}"))?;
            log_line(&format!("controller DB -> {}", dest.display()));
            return Ok(());
        }
    }
    log_line("warning: gamecontrollerdb.txt not found; set WARMUP_GAMECONTROLLER_DB");
    Ok(())
}

fn sc(args: &[&str]) -> Result<(), String> {
    let out = Command::new("sc.exe")
        .args(args)
        .output()
        .map_err(|e| format!("sc.exe: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "sc {} failed ({}): {stdout}{stderr}",
            args.join(" "),
            out.status
        ));
    }
    Ok(())
}

fn verify_service_running() -> Result<(), String> {
    let out = Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .output()
        .map_err(|e| format!("sc.exe query: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.contains("RUNNING") {
        return Ok(());
    }
    let hint = if !Path::new(INSTALL_DIR).join(EXE_NAME).is_file() {
        format!(
            " (missing {})",
            Path::new(INSTALL_DIR).join(EXE_NAME).display()
        )
    } else {
        String::new()
    };
    Err(format!(
        "{SERVICE_NAME} is not RUNNING after sc start{hint}. sc query output:\n{text}"
    ))
}

fn require_admin() -> Result<(), String> {
    let ok = Command::new("net.exe")
        .args(["session"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("Administrator required. Run PowerShell as Administrator.".into())
    }
}

pub fn log_line(msg: &str) {
    let _ = log_line_inner(msg);
    eprintln!("> {msg}");
}

fn log_line_inner(msg: &str) -> std::io::Result<()> {
    let path = Path::new(DATA_DIR).join(LOG_NAME);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let ts = chrono_lite_timestamp();
    writeln!(f, "[{ts}] {msg}")?;
    Ok(())
}

fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

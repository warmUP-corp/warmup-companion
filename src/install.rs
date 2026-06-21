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
const MAX_LOG_BYTES: u64 = 1024 * 1024;
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
    println!("Check it worked anytime: warmup-companion.exe verify");
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

/// `warmup-companion.exe verify` — read-only self-check so a new user can confirm
/// the install worked and see whether their controller is detected, instead of
/// testing blind at the lock screen. Controller status is read from the service's
/// own log (the service is what actually reads the pad), not by re-initialising
/// hardware in this throwaway process. Exits non-zero if anything is broken.
pub fn run_verify() {
    let mut healthy = true;
    let bin = Path::new(INSTALL_DIR).join(EXE_NAME);
    if bin.is_file() {
        println!("[ ok ] binary present: {}", bin.display());
    } else {
        healthy = false;
        println!("[FAIL] binary missing: {} — run: warmup-companion.exe install", bin.display());
    }

    match service_state() {
        Some(state) if state == "RUNNING" => println!("[ ok ] service {SERVICE_NAME}: RUNNING"),
        Some(state) => {
            healthy = false;
            println!("[FAIL] service {SERVICE_NAME}: {state} (expected RUNNING) — try: sc start {SERVICE_NAME}");
        }
        None => {
            healthy = false;
            println!("[FAIL] service {SERVICE_NAME} not installed — run: warmup-companion.exe install");
        }
    }

    let log = Path::new(DATA_DIR).join(LOG_NAME);
    match log_age_secs(&log) {
        Some(secs) if secs <= 120 => println!("[ ok ] service log active ({secs}s ago)"),
        Some(secs) => println!("[warn] service log last written {secs}s ago (service may be idle)"),
        None => println!("[warn] no service log yet: {}", log.display()),
    }

    match last_controller_line(&log) {
        Some(line) if !line.to_lowercase().contains("none") => {
            println!("[ ok ] controller seen by service: {line}");
        }
        _ => println!("[ !  ] no controller detected yet — connect a pad, press any button, re-run verify"),
    }

    println!();
    if healthy {
        println!("Installation looks healthy.");
    } else {
        println!("Problems found above. Log: {}", log.display());
        std::process::exit(1);
    }
}

/// Current SCM state word (e.g. "RUNNING", "STOPPED") for the service, or `None`
/// if it isn't installed.
fn service_state() -> Option<String> {
    let out = Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .output()
        .ok()?;
    parse_service_state(&String::from_utf8_lossy(&out.stdout))
}

/// Extract the SCM state word (e.g. "RUNNING") from `sc query` stdout, or `None`
/// if there's no STATE line (service not installed).
fn parse_service_state(sc_query_stdout: &str) -> Option<String> {
    sc_query_stdout
        .lines()
        .find(|l| l.trim_start().starts_with("STATE"))
        .and_then(|l| l.split_whitespace().last())
        .map(str::to_string)
}

/// Seconds since the service log was last written, or `None` if it's missing.
fn log_age_secs(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.elapsed().map(|d| d.as_secs()).unwrap_or(0))
}

/// The most recent log line that names the gamepad backend / controller, so
/// verify can echo what the service currently sees.
fn last_controller_line(path: &Path) -> Option<String> {
    find_controller_line(&fs::read_to_string(path).ok()?)
}

/// Most recent log line naming the gamepad backend / controller.
fn find_controller_line(log: &str) -> Option<String> {
    log.lines()
        .rev()
        .find(|l| l.contains("gamepad backend") || l.contains("gamepad loop running"))
        .map(|l| l.trim().to_string())
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
    restrict_data_dir_acl()?;
    // Re-open the bin subdir to Users (read+execute) so the non-elevated warmUP
    // desktop app can detect the install (else its settings show "Missing"). Done
    // after the lockdown so it adds to, not replaces, the SYSTEM+Admins grant. The
    // exe is copied below and inherits this read access.
    allow_bin_read_acl()?;
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
    // Version marker the warmUP desktop app reads (its `warmup-companion.version`
    // convention) to show the installed version and judge updates. Best-effort —
    // a marker failure must not fail the service install. Inherits Users:RX from bin.
    let _ = fs::write(
        Path::new(INSTALL_DIR).join("warmup-companion.version"),
        env!("CARGO_PKG_VERSION"),
    );
    // sc.exe: `binPath=` and path are separate argv tokens; no quotes (path has no spaces).
    // SCM starts the exe directly; main() dispatches to service_dispatcher when argc == 1.
    let exe = dest.display().to_string();
    sc(&["create", SERVICE_NAME, "binPath=", &exe])?;
    // sc.exe wants `start=` and `auto` as separate argv tokens (not one `start= auto` string).
    sc(&["config", SERVICE_NAME, "start=", "auto"])?;
    sc(&["config", SERVICE_NAME, "DisplayName=", DISPLAY_NAME])?;
    sc(&["description", SERVICE_NAME, "Description=", DESCRIPTION])?;
    // Auto-restart on crash. Without this, a single panic/abort leaves the service
    // — and therefore all controller input — dead until manual restart or reboot.
    // restart 60s after the 1st and 2nd failure; the last action repeats for any
    // further failure, so every crash self-heals. reset= (1 day) just bounds the
    // failure counter. Non-fatal if it fails on older SCMs — log and continue.
    if let Err(e) = sc(&[
        "failure",
        SERVICE_NAME,
        "reset=",
        "86400",
        "actions=",
        "restart/60000/restart/60000",
    ]) {
        log_line(&format!("sc failure (auto-restart) config failed: {e}"));
    }
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
        if let Ok(()) = sc(&["delete", name]) { log_line(&format!("removed test service {name}")) }
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

fn restrict_data_dir_acl() -> Result<(), String> {
    // Grant by well-known SID, not name: "Administrators"/"SYSTEM" don't resolve
    // on non-English Windows (e.g. German "Administratoren"). *S-1-5-18 = SYSTEM,
    // *S-1-5-32-544 = BUILTIN\Administrators.
    let out = Command::new("icacls.exe")
        .args([
            DATA_DIR,
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-544:(OI)(CI)F",
        ])
        .output()
        .map_err(|e| format!("icacls.exe: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        Err(format!("lock down {DATA_DIR} ACL failed: {stdout}{stderr}"))
    }
}

fn allow_bin_read_acl() -> Result<(), String> {
    // Grant BUILTIN\Users (S-1-5-32-545) read+execute on the bin dir only. The data
    // dir stays locked to SYSTEM+Admins (logs / VK context), but the binaries are
    // public release artifacts and the non-elevated desktop app must read the exe to
    // see the companion as installed. Read can't tamper with the SYSTEM-run service.
    // (OI)(CI) so the copied exe + version marker inherit it. Bypass-traverse-checking
    // (default for all users) lets the app reach bin through the locked data dir.
    let out = Command::new("icacls.exe")
        .args([INSTALL_DIR, "/grant:r", "*S-1-5-32-545:(OI)(CI)RX"])
        .output()
        .map_err(|e| format!("icacls.exe: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        Err(format!("grant {INSTALL_DIR} read ACL failed: {stdout}{stderr}"))
    }
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
    rotate_log_if_needed(&path)?;
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let ts = chrono_lite_timestamp();
    writeln!(f, "[{ts}] {msg}")?;
    Ok(())
}

fn rotate_log_if_needed(path: &Path) -> std::io::Result<()> {
    if fs::metadata(path).map(|m| m.len()).unwrap_or(0) < MAX_LOG_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    let _ = fs::remove_file(&rotated);
    fs::rename(path, rotated)?;
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

#[cfg(test)]
mod tests {
    use super::{find_controller_line, parse_service_state};

    #[test]
    fn parse_service_state_reads_state_word() {
        let out = "SERVICE_NAME: WarmupVkSvc\n\
                   TYPE               : 10  WIN32_OWN_PROCESS\n\
                   STATE              : 4  RUNNING\n";
        assert_eq!(parse_service_state(out).as_deref(), Some("RUNNING"));
        assert_eq!(parse_service_state("no state here"), None);
    }

    #[test]
    fn find_controller_line_picks_last_match() {
        let log = "boot\n\
                   gamepad backend: SDL3 (userland) - none\n\
                   tick\n\
                   gamepad backend: XInput - Xbox Controller\n\
                   tick\n";
        assert_eq!(
            find_controller_line(log).as_deref(),
            Some("gamepad backend: XInput - Xbox Controller")
        );
        assert_eq!(find_controller_line("nothing relevant"), None);
    }
}

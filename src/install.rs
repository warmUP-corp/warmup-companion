//! Install / uninstall WarmupVk Windows service (`WarmupVkSvc`).

#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SERVICE_NAME: &str = "WarmupVkSvc";
const DISPLAY_NAME: &str = "Warmup Xbox VK sign-in";
const DESCRIPTION: &str =
    "Gamepad on-screen keyboard at Windows sign-in, lock screen, and UAC (Warmup prototype).";

/// No spaces — `sc.exe` breaks on quoted paths under Program Files.
pub const INSTALL_DIR: &str = r"C:\ProgramData\WarmupVk\bin";
pub const DATA_DIR: &str = r"C:\ProgramData\WarmupVk";
const EXE_NAME: &str = "warmup-vk-prototype.exe";
const LOG_NAME: &str = "service.log";

/// Leftover names from manual `sc create` debugging — removed on install/uninstall.
const TEST_SERVICE_NAMES: &[&str] = &[
    "WarmupVkTest",
    "WarmupVkTest2",
    "WarmupVkTest3",
    "WarmupVkTest4",
    "WarmupVkTest5",
    "WarmupVkTest6",
];

pub fn run_install() {
    if let Err(e) = install_inner() {
        eprintln!("install failed: {e}");
        std::process::exit(1);
    }
    let bin = Path::new(INSTALL_DIR).join(EXE_NAME);
    println!("Installed. Service {SERVICE_NAME} started.");
    println!("Binary (SCM uses this): {}", bin.display());
    println!("NOT C:\\Program Files\\WarmupVk\\ — that path is legacy only.");
    println!("Reboot or Win+L, then tap Y on the controller at the password screen.");
    println!("Log: {DATA_DIR}\\{LOG_NAME}");
}

pub fn run_uninstall() {
    if let Err(e) = uninstall_inner() {
        eprintln!("uninstall failed: {e}");
        std::process::exit(1);
    }
    println!("Uninstalled {SERVICE_NAME}.");
}

fn install_inner() -> Result<(), String> {
    require_admin()?;
    remove_legacy_install_artifacts();
    let src = std::env::current_exe().map_err(|e| e.to_string())?;
    fs::create_dir_all(INSTALL_DIR).map_err(|e| e.to_string())?;
    fs::create_dir_all(DATA_DIR).map_err(|e| e.to_string())?;

    let dest = Path::new(INSTALL_DIR).join(EXE_NAME);
    fs::copy(&src, &dest).map_err(|e| format!("copy exe to {INSTALL_DIR}: {e}"))?;
    if !dest.is_file() {
        return Err(format!(
            "install copy missing: {} (service will not start)",
            dest.display()
        ));
    }
    copy_gamecontroller_db()?;

    remove_test_services();
    uninstall_service_quiet();
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
    log_line(&format!("installed from {} -> {}", src.display(), dest.display()));
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
    // Legacy path from earlier installer attempts.
    let legacy = Path::new(r"C:\Program Files\WarmupVk").join(EXE_NAME);
    if legacy.exists() {
        fs::remove_file(&legacy).ok();
    }
    log_line("uninstalled");
    Ok(())
}

fn uninstall_service_quiet() {
    let _ = sc(&["stop", SERVICE_NAME]);
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = sc(&["delete", SERVICE_NAME]);
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
    let candidates = [
        std::env::var_os("WARMUP_GAMECONTROLLER_DB").map(PathBuf::from),
        Some(PathBuf::from(
            r"C:\Users\jonas\warmUp\apps\desktop\src-tauri\resources\gamecontrollerdb.txt",
        )),
    ];
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
        format!(" (missing {})", Path::new(INSTALL_DIR).join(EXE_NAME).display())
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

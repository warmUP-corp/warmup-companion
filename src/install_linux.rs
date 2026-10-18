use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const UNIT_NAME: &str = "warmup-companion.service";
const BIN_NAME: &str = "warmup-companion";

pub fn run_install() {
    if let Err(error) = install_inner() {
        eprintln!("install failed: {error}");
        std::process::exit(1);
    }
}

pub fn run_uninstall() {
    if let Err(error) = uninstall_inner() {
        eprintln!("uninstall failed: {error}");
        std::process::exit(1);
    }
    println!("Uninstalled {UNIT_NAME}.");
}

pub fn run_verify() {
    let mut healthy = true;
    let bin = installed_bin();
    if bin.is_file() {
        println!("[ ok ] binary present: {}", bin.display());
    } else {
        healthy = false;
        println!(
            "[FAIL] binary missing: {} — run: warmup-companion install",
            bin.display()
        );
    }

    let unit = crate::paths::systemd_user_unit_file();
    if unit.is_file() {
        println!("[ ok ] user unit: {}", unit.display());
    } else {
        healthy = false;
        println!("[FAIL] user unit missing: {}", unit.display());
    }

    match systemctl(&["--user", "is-active", UNIT_NAME]) {
        Ok(state) if state.trim() == "active" => {
            println!("[ ok ] {UNIT_NAME} is active");
        }
        Ok(state) => {
            healthy = false;
            println!(
                "[FAIL] {UNIT_NAME} is {} — try: systemctl --user start {UNIT_NAME}",
                state.trim()
            );
        }
        Err(error) => {
            healthy = false;
            println!("[FAIL] systemctl is-active: {error}");
        }
    }

    let socket = crate::paths::socket_file();
    if socket.exists() {
        println!("[ ok ] input socket: {}", socket.display());
    } else {
        println!(
            "[warn] input socket not present yet: {} (the service creates it after start)",
            socket.display()
        );
    }

    if fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok() {
        println!("[ ok ] /dev/uinput is writable");
    } else {
        healthy = false;
        println!(
            "[FAIL] /dev/uinput is not writable. Gaming distros already grant this via Steam's udev rule. Other distros: see docs/linux-install.md"
        );
    }

    match systemctl(&["--user", "show-environment"]) {
        Ok(env) if env.lines().any(|line| {
            line.strip_prefix("WAYLAND_DISPLAY=")
                .is_some_and(|value| !value.is_empty())
        }) =>
        {
            println!("[ ok ] user manager has WAYLAND_DISPLAY");
        }
        Ok(_) => {
            healthy = false;
            println!(
                "[FAIL] the systemd user manager has no WAYLAND_DISPLAY. On Hyprland run: dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_RUNTIME_DIR"
            );
        }
        Err(error) => {
            healthy = false;
            println!("[FAIL] systemctl show-environment: {error}");
        }
    }

    let log = crate::paths::log_file();
    println!("Log: {}", log.display());
    if !healthy {
        std::process::exit(1);
    }
    println!("Installation looks healthy.");
}

pub fn run_stop() {
    if let Err(error) = systemctl_ok(&["--user", "stop", UNIT_NAME]) {
        eprintln!("stop failed: {error}");
        std::process::exit(1);
    }
    println!("Stopped {UNIT_NAME}.");
}

fn install_inner() -> Result<(), String> {
    let src = std::env::current_exe().map_err(|error| format!("current exe: {error}"))?;
    let dest = installed_bin();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("create install dir: {error}"))?;
    }
    copy_binary(&src, &dest)?;
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("chmod install binary: {error}"))?;

    let unit_path = crate::paths::systemd_user_unit_file();
    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("create systemd user dir: {error}"))?;
    }
    let unit = unit_text(&dest);
    fs::write(&unit_path, unit).map_err(|error| format!("write unit: {error}"))?;

    systemctl_ok(&["--user", "daemon-reload"])?;
    systemctl_ok(&["--user", "enable", "--now", UNIT_NAME])?;

    println!("Installed warmUP companion for this user.");
    println!("Binary: {}", dest.display());
    println!("Unit: {}", unit_path.display());
    println!("Socket: {}", crate::paths::socket_file().display());
    println!("Log: {}", crate::paths::log_file().display());
    println!(
        "Gaming distros already allow /dev/uinput. On any other distribution, install docs/linux-install.md before the pointer will move."
    );
    Ok(())
}

fn uninstall_inner() -> Result<(), String> {
    let _ = systemctl_ok(&["--user", "disable", "--now", UNIT_NAME]);
    let unit_path = crate::paths::systemd_user_unit_file();
    if unit_path.exists() {
        fs::remove_file(&unit_path).map_err(|error| format!("remove unit: {error}"))?;
    }
    systemctl_ok(&["--user", "daemon-reload"])?;
    let bin = installed_bin();
    if bin.exists() {
        fs::remove_file(&bin).map_err(|error| format!("remove binary: {error}"))?;
    }
    crate::install::log_line("uninstalled linux user service");
    Ok(())
}

fn installed_bin() -> PathBuf {
    crate::paths::install_dir().join(BIN_NAME)
}

fn copy_binary(src: &Path, dest: &Path) -> Result<(), String> {
    if same_file(src, dest) {
        return Ok(());
    }
    fs::copy(src, dest).map_err(|error| {
        format!(
            "copy {} to {}: {error}",
            src.display(),
            dest.display()
        )
    })?;
    Ok(())
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn unit_text(bin: &Path) -> String {
    format!(
        "[Unit]\n\
Description=warmUP companion\n\
PartOf=graphical-session.target\n\
After=graphical-session.target\n\
\n\
[Service]\n\
Type=simple\n\
Environment=__NV_DISABLE_EXPLICIT_SYNC=1\n\
ExecStart={exec} --service\n\
Restart=on-failure\n\
RestartSec=2\n\
\n\
[Install]\n\
WantedBy=graphical-session.target\n",
        exec = bin.display()
    )
}

fn systemctl_ok(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|error| format!("systemctl {}: {error}", args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "systemctl {} failed ({}): {stdout}{stderr}",
        args.join(" "),
        output.status
    ))
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|error| format!("systemctl {}: {error}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() || !stdout.trim().is_empty() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("{stdout}{stderr}"))
}

pub fn request_service_stop() {
    crate::install::log_line("service stop requested");
}

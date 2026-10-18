use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::release_feed::{
    asset, download_url, is_newer, page_url, release_notes, sidecar_sha, tag_version, CURRENT,
    LATEST_URL,
};

const BIN_NAME: &str = "warmup-companion";
const NEW_NAME: &str = "warmup-companion.new";
const OLD_NAME: &str = "warmup-companion.old";
const UNIT_NAME: &str = "warmup-companion.service";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const SETTLE: Duration = Duration::from_secs(3);

static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub(crate) struct Release {
    pub(crate) version: String,
    pub(crate) notes: Vec<String>,
    pub(crate) page: String,
    pub(crate) size: u64,
    bin_url: String,
    sha_url: String,
}

#[derive(Clone, Debug)]
pub(crate) enum Check {
    UpToDate(String),
    NoLinuxBuild(String),
    Available(Release),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Restart {
    Skip,
    Wait,
    Queue,
}

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub(crate) dir: PathBuf,
    pub(crate) dry_run: bool,
    pub(crate) restart: Restart,
}

impl Options {
    pub(crate) fn installed() -> Self {
        Self {
            dir: crate::paths::install_dir(),
            dry_run: false,
            restart: Restart::Queue,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Outcome {
    UpToDate(String),
    NoLinuxBuild(String),
    Verified(Release),
    Installed { release: Release, restarted: bool },
}

pub(crate) fn asset_name() -> String {
    format!("warmup-companion-linux-{}", std::env::consts::ARCH)
}

pub(crate) fn check() -> Result<Check, String> {
    let json: serde_json::Value =
        serde_json::from_str(&get_string(LATEST_URL)?).map_err(|e| format!("bad reply: {e}"))?;
    let version = tag_version(&json)?;
    if !is_newer(&version, CURRENT) {
        return Ok(Check::UpToDate(version));
    }
    let name = asset_name();
    let Some(bin) = asset(&json, &name) else {
        return Ok(Check::NoLinuxBuild(version));
    };
    let sha = asset(&json, &format!("{name}.sha256"))
        .ok_or("release has no checksum for the Linux build")?;
    Ok(Check::Available(Release {
        notes: release_notes(json["body"].as_str().unwrap_or_default()),
        page: page_url(&json),
        size: bin["size"].as_u64().unwrap_or(0),
        bin_url: download_url(&bin).ok_or("Linux build URL is not on GitHub")?,
        sha_url: download_url(&sha).ok_or("checksum URL is not on GitHub")?,
        version,
    }))
}

pub(crate) fn update(options: &Options) -> Result<Outcome, String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("an update is already running".into());
    }
    let result = match check() {
        Ok(Check::UpToDate(version)) => Ok(Outcome::UpToDate(version)),
        Ok(Check::NoLinuxBuild(version)) => Ok(Outcome::NoLinuxBuild(version)),
        Ok(Check::Available(release)) => install(release, options),
        Err(e) => Err(e),
    };
    BUSY.store(false, Ordering::SeqCst);
    result
}

#[allow(dead_code)]
pub(crate) fn start_update() {
    spawn("update-install", || match update(&Options::installed()) {
        Ok(Outcome::NoLinuxBuild(version)) => {
            notify(
                "No Linux build",
                &format!("v{version} has no Linux build yet."),
            );
        }
        Ok(Outcome::Installed {
            release,
            restarted: false,
        }) => {
            notify(
                "Update installed",
                &format!(
                    "v{} starts the next time the companion starts.",
                    release.version
                ),
            );
        }
        Ok(Outcome::UpToDate(_)) => {
            notify(
                "Up to date",
                &format!("warmUP companion v{CURRENT} is the latest version."),
            );
        }
        Ok(_) => {}
        Err(e) => {
            log(&format!("update failed: {e}"));
            notify("Update failed", &e);
        }
    });
}

pub(crate) fn spawn_auto_check() {
    spawn("update-check", || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        let mut notified = String::new();
        loop {
            let mut wait = CHECK_EVERY;
            if crate::config::gamepad_settings().update_check && !BUSY.load(Ordering::SeqCst) {
                match check() {
                    Ok(Check::Available(release)) if release.version != notified => {
                        log(&format!("v{} available", release.version));
                        notify(
                            &format!("warmUP companion v{} is available", release.version),
                            "Run warmup-companion update to install it.",
                        );
                        notified = release.version;
                    }
                    Ok(Check::NoLinuxBuild(version)) => {
                        log(&format!("v{version} has no Linux build"));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log(&format!("check failed: {e}"));
                        wait = RETRY_AFTER;
                    }
                }
            }
            std::thread::sleep(wait);
        }
    });
}

fn install(release: Release, options: &Options) -> Result<Outcome, String> {
    let expected = sidecar_sha(&get_string(&release.sha_url)?).ok_or("empty checksum file")?;
    fs::create_dir_all(&options.dir).map_err(|e| format!("{}: {e}", options.dir.display()))?;
    let new = options.dir.join(NEW_NAME);
    let _ = fs::remove_file(&new);
    log(&format!(
        "downloading v{} to {}",
        release.version,
        new.display()
    ));
    if let Err(e) = verified_download(&release.bin_url, &new, &expected) {
        let _ = fs::remove_file(&new);
        return Err(e);
    }
    if options.dry_run {
        let _ = fs::remove_file(&new);
        return Ok(Outcome::Verified(release));
    }
    let bin = options.dir.join(BIN_NAME);
    if let Err(e) = keep_previous(&options.dir) {
        let _ = fs::remove_file(&new);
        return Err(e);
    }
    if let Err(e) = fs::rename(&new, &bin) {
        let _ = fs::remove_file(&new);
        return Err(format!("replace {}: {e}", bin.display()));
    }
    log(&format!(
        "installed v{} at {}",
        release.version,
        bin.display()
    ));
    let restarted = restart(options.restart).map_err(|e| {
        format!(
            "installed v{} but the restart failed: {e}\n{}",
            release.version,
            rollback_hint(&options.dir)
        )
    })?;
    Ok(Outcome::Installed { release, restarted })
}

pub(crate) fn rollback_hint(dir: &Path) -> String {
    let old = dir.join(OLD_NAME);
    if !old.is_file() {
        return "No previous version was kept.".into();
    }
    format!(
        "To go back to the previous version run: mv {} {} && systemctl --user try-restart {UNIT_NAME}",
        old.display(),
        dir.join(BIN_NAME).display()
    )
}

fn keep_previous(dir: &Path) -> Result<(), String> {
    let bin = dir.join(BIN_NAME);
    let old = dir.join(OLD_NAME);
    let _ = fs::remove_file(&old);
    if !bin.is_file() {
        return Ok(());
    }
    fs::hard_link(&bin, &old)
        .or_else(|_| fs::copy(&bin, &old).map(|_| ()))
        .map_err(|e| format!("keep the current version as {}: {e}", old.display()))
}

fn verified_download(url: &str, path: &Path, expected: &str) -> Result<(), String> {
    fetch_to(url, path)?;
    let actual = sha256_file(path)?;
    if actual != expected {
        return Err(format!(
            "checksum mismatch\ngot      {actual}\nexpected {expected}"
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

fn restart(mode: Restart) -> Result<bool, String> {
    if mode == Restart::Skip || !crate::paths::systemd_user_unit_file().is_file() {
        return Ok(false);
    }
    let running = crate::install::systemctl(&["--user", "is-active", UNIT_NAME])
        .is_ok_and(|state| state.trim() == "active");
    if !running {
        return Ok(false);
    }
    if mode == Restart::Queue {
        crate::install::systemctl_ok(&["--user", "--no-block", "try-restart", UNIT_NAME])?;
        return Ok(true);
    }
    crate::install::systemctl_ok(&["--user", "try-restart", UNIT_NAME])?;
    std::thread::sleep(SETTLE);
    let state = crate::install::systemctl(&["--user", "is-active", UNIT_NAME])?;
    if state.trim() != "active" {
        return Err(format!(
            "the new version is {} after the restart",
            state.trim()
        ));
    }
    Ok(true)
}

fn curl() -> Command {
    let mut command = Command::new("curl");
    command
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--connect-timeout", "15", "-A"])
        .arg(format!("warmup-companion/{CURRENT}"));
    command
}

fn run_curl(mut command: Command, url: &str) -> Result<Vec<u8>, String> {
    let output = command
        .arg(url)
        .output()
        .map_err(|e| format!("curl is needed to update: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn get_string(url: &str) -> Result<String, String> {
    let mut command = curl();
    command.args(["--max-time", "30"]);
    String::from_utf8(run_curl(command, url)?).map_err(|e| format!("{url}: {e}"))
}

fn fetch_to(url: &str, path: &Path) -> Result<(), String> {
    let mut command = curl();
    command.args(["--max-time", "600", "-o"]).arg(path);
    run_curl(command, url).map(|_| ())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "sha256sum: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    sidecar_sha(&String::from_utf8_lossy(&output.stdout)).ok_or("sha256sum printed nothing".into())
}

fn notify(summary: &str, body: &str) {
    if let Err(e) = crate::linux_notify::notify(summary, body) {
        log(&format!("notification failed: {e}"));
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(f);
}

fn log(msg: &str) {
    crate::install::log_line(&format!("update: {msg}"));
}

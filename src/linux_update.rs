use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) use crate::release_feed::CURRENT;
use crate::release_feed::{
    asset, download_url, is_newer, page_url, release_notes, sidecar_sha, tag_version, LATEST_URL,
};

const BIN_NAME: &str = "warmup-companion";
const NEW_NAME: &str = "warmup-companion.new";
const OLD_NAME: &str = "warmup-companion.old";
const UNIT_NAME: &str = "warmup-companion.service";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const SETTLE: Duration = Duration::from_secs(3);
const MIN_VERIFY: Duration = Duration::from_millis(900);
const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Phase {
    Idle,
    Checking,
    Available,
    Downloading { got: u64, total: u64 },
    Verifying,
    Installing,
    UpToDate,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) phase: Phase,
    pub(crate) release: Option<Release>,
    pub(crate) last_checked: Option<String>,
}

static STATE: Mutex<Snapshot> = Mutex::new(Snapshot {
    phase: Phase::Idle,
    release: None,
    last_checked: None,
});
static BUSY: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static TRACKING: AtomicBool = AtomicBool::new(false);

pub(crate) fn snapshot() -> Snapshot {
    STATE.lock().map(|s| s.clone()).unwrap_or(Snapshot {
        phase: Phase::Idle,
        release: None,
        last_checked: None,
    })
}

pub(crate) fn available() -> Option<(String, String)> {
    let s = snapshot();
    if matches!(s.phase, Phase::UpToDate | Phase::Checking) {
        return None;
    }
    s.release.map(|r| (r.version, r.page))
}

pub(crate) fn last_checked() -> Option<String> {
    snapshot().last_checked
}

fn update_state(f: impl FnOnce(&mut Snapshot)) {
    let changed = match STATE.lock() {
        Ok(mut s) => {
            let before = s.clone();
            f(&mut s);
            *s != before
        }
        Err(_) => false,
    };
    if changed {
        crate::linux_tray::refresh();
    }
}

fn set_phase(phase: Phase) {
    update_state(|s| s.phase = phase);
}

fn record(check: &Check) {
    let (_, _, _, hour, minute, _) = crate::time_util::local_now();
    update_state(|s| {
        s.last_checked = Some(format!("today, {hour:02}:{minute:02}"));
        match check {
            Check::Available(release) => {
                s.release = Some(release.clone());
                s.phase = Phase::Available;
            }
            Check::UpToDate(_) | Check::NoLinuxBuild(_) => {
                s.release = None;
                s.phase = Phase::UpToDate;
            }
        }
    });
}

pub(crate) fn open() {
    match snapshot().phase {
        Phase::Idle | Phase::UpToDate | Phase::Failed(_) => start_check(),
        _ => {}
    }
    crate::win::update_window::show();
}

pub(crate) fn start_check() {
    if BUSY.load(Ordering::SeqCst) {
        return;
    }
    set_phase(Phase::Checking);
    spawn("update-check-now", || match check() {
        Ok(result) => {
            if let Check::NoLinuxBuild(version) = &result {
                log(&format!("v{version} has no Linux build"));
            }
            record(&result);
        }
        Err(e) => {
            log(&format!("check failed: {e}"));
            set_phase(Phase::Failed(e));
        }
    });
}

pub(crate) fn start_install() {
    let Some(release) = snapshot().release else {
        return start_check();
    };
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    CANCEL.store(false, Ordering::SeqCst);
    TRACKING.store(true, Ordering::SeqCst);
    set_phase(Phase::Downloading {
        got: 0,
        total: release.size,
    });
    spawn("update-install", move || {
        let version = release.version.clone();
        let result = install(release, &Options::installed());
        TRACKING.store(false, Ordering::SeqCst);
        BUSY.store(false, Ordering::SeqCst);
        match result {
            Ok(Outcome::Installed {
                restarted: false, ..
            }) => {
                notify(
                    "Update installed",
                    &format!("v{version} starts the next time the companion starts."),
                );
            }
            Ok(_) => {}
            Err(_) if CANCEL.swap(false, Ordering::SeqCst) => {
                log("download cancelled");
                set_phase(Phase::Available);
            }
            Err(e) => {
                log(&format!("install v{version} failed: {e}"));
                set_phase(Phase::Failed(e));
            }
        }
    });
}

pub(crate) fn cancel() {
    if matches!(snapshot().phase, Phase::Downloading { .. }) {
        CANCEL.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn open_release_page() {
    if let Some(page) = snapshot().release.map(|r| r.page) {
        open_url(&page);
    }
}

fn open_url(page: &str) {
    if let Err(e) = Command::new("xdg-open").arg(page).spawn() {
        log(&format!("open {page} failed: {e}"));
    }
}

fn show_update_notification(version: String) {
    spawn("update-notification", move || {
        let action = crate::linux_notify::notify_with_actions(
            &format!("Warmup Companion v{version} is available"),
            "Install it now, or later from the tray under Updates.",
            &[("default", "Open"), ("install", "Install"), ("later", "Later")],
        );
        match action.as_deref() {
            Ok("install") => {
                if matches!(snapshot().phase, Phase::Available) {
                    start_install();
                    crate::win::update_window::show();
                } else {
                    open();
                }
            }
            Ok("default") => open(),
            Ok(_) => {}
            Err(e) => log(&format!("notification failed: {e}")),
        }
    });
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Release {
    pub(crate) version: String,
    pub(crate) notes: Vec<String>,
    pub(crate) page: String,
    pub(crate) size: u64,
    pub(crate) sha: String,
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
        sha: String::new(),
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

pub(crate) fn spawn_auto_check() {
    spawn("update-check", || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        let mut notified = String::new();
        loop {
            let mut wait = CHECK_EVERY;
            if crate::config::gamepad_settings().update_check && !BUSY.load(Ordering::SeqCst) {
                let result = check();
                if let Ok(checked) = &result {
                    record(checked);
                }
                match result {
                    Ok(Check::Available(release)) if release.version != notified => {
                        log(&format!("v{} available", release.version));
                        show_update_notification(release.version.clone());
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
    let actual = match verified_download(&release.bin_url, &new, &expected, release.size) {
        Ok(actual) => actual,
        Err(e) => {
            let _ = fs::remove_file(&new);
            return Err(e);
        }
    };
    if TRACKING.load(Ordering::SeqCst) {
        update_state(|s| {
            if let Some(r) = s.release.as_mut() {
                r.sha = actual;
            }
            s.phase = Phase::Installing;
        });
        std::thread::sleep(Duration::from_millis(800));
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

fn verified_download(url: &str, path: &Path, expected: &str, size: u64) -> Result<String, String> {
    fetch_to(url, path, size)?;
    let tracking = TRACKING.load(Ordering::SeqCst);
    if tracking {
        set_phase(Phase::Verifying);
    }
    let started = Instant::now();
    let actual = sha256_file(path)?;
    if tracking {
        std::thread::sleep(MIN_VERIFY.saturating_sub(started.elapsed()));
    }
    if actual != expected {
        return Err(format!(
            "checksum mismatch\ngot      {actual}\nexpected {expected}"
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    Ok(actual)
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

fn fetch_to(url: &str, path: &Path, size: u64) -> Result<(), String> {
    let mut command = curl();
    command
        .args(["--max-time", "600", "-o"])
        .arg(path)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("curl is needed to update: {e}"))?;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("curl: {e}"))? {
            break status;
        }
        if CANCEL.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        if TRACKING.load(Ordering::SeqCst) {
            let got = fs::metadata(path).map_or(0, |m| m.len());
            set_phase(Phase::Downloading {
                got,
                total: size.max(got),
            });
        }
        std::thread::sleep(POLL);
    };
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
        }
        return Err(format!("{url}: {}", stderr.trim()));
    }
    Ok(())
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

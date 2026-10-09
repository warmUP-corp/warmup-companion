use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::{HSTRING, PCWSTR};
use windows::Foundation::Uri;
use windows::Security::Cryptography::Core::HashAlgorithmProvider;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::Streams::{Buffer, IBuffer, InputStreamOptions};
use windows::Web::Http::{HttpClient, HttpCompletionOption};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, INFINITE,
    SYNCHRONIZATION_ACCESS_RIGHTS,
};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

const LATEST_URL: &str =
    "https://api.github.com/repos/warmUP-corp/warmup-companion/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/warmUP-corp/warmup-companion/releases/download/";
const PAGE_PREFIX: &str = "https://github.com/warmUP-corp/warmup-companion/releases/";
const SETUP_NAME: &str = "warmup-companion-setup.exe";
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const CHUNK: u32 = 64 * 1024;
const MIN_VERIFY: Duration = Duration::from_millis(900);
const OPEN_EVENT: &str = r"Local\WarmupCompanionUpdateOpen";
const INSTALL_EVENT: &str = r"Local\WarmupCompanionUpdateInstall";
const EVENT_SDDL: &str = "D:(A;;GA;;;SY)(A;;0x00100002;;;IU)";
const EVENT_MODIFY_STATE: u32 = 0x0002;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Release {
    pub(crate) version: String,
    pub(crate) notes: Vec<String>,
    pub(crate) size: u64,
    pub(crate) page: String,
    pub(crate) sha: String,
    setup_url: String,
    sha_url: String,
}

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

pub(crate) fn snapshot() -> Snapshot {
    STATE.lock().map(|s| s.clone()).unwrap_or(Snapshot {
        phase: Phase::Idle,
        release: None,
        last_checked: None,
    })
}

pub(crate) fn available_version() -> Option<String> {
    let s = snapshot();
    if matches!(s.phase, Phase::UpToDate | Phase::Checking) {
        return None;
    }
    s.release.map(|r| r.version)
}

fn set_phase(phase: Phase) {
    if let Ok(mut s) = STATE.lock() {
        s.phase = phase;
    }
}

pub(crate) fn spawn_auto_check() {
    spawn("update-events", watch_events);
    spawn("update-check", || {
        std::thread::sleep(FIRST_CHECK_DELAY);
        let mut notified = String::new();
        loop {
            let mut wait = CHECK_EVERY;
            if crate::config::gamepad_settings().update_check && !BUSY.load(Ordering::SeqCst) {
                match check() {
                    Ok(Some(rel)) if rel.version != notified => {
                        log(&format!("v{} available", rel.version));
                        spawn_toast(&rel.version);
                        notified = rel.version;
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
    spawn("update-check-now", || {
        if let Err(e) = check() {
            log(&format!("check failed: {e}"));
            set_phase(Phase::Failed(e));
        }
    });
}

pub(crate) fn start_install() {
    let Some(rel) = snapshot().release else {
        return start_check();
    };
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    CANCEL.store(false, Ordering::SeqCst);
    set_phase(Phase::Downloading {
        got: 0,
        total: rel.size,
    });
    spawn("update-install", move || {
        if let Err(e) = download_and_install(&rel) {
            BUSY.store(false, Ordering::SeqCst);
            if CANCEL.swap(false, Ordering::SeqCst) {
                log("download cancelled");
                set_phase(Phase::Available);
            } else {
                log(&format!("install v{} failed: {e}", rel.version));
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
    let Some(page) = snapshot().release.map(|r| r.page) else {
        return;
    };
    let explorer = Path::new(r"C:\Windows\explorer.exe");
    match crate::win::speech_input::spawn_as_user(explorer, &format!("\"{page}\"")) {
        Ok(h) => unsafe {
            let _ = CloseHandle(h);
        },
        Err(e) => log(&format!("open release page failed: {e}")),
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(f);
}

fn check() -> Result<Option<Release>, String> {
    let json: serde_json::Value =
        serde_json::from_str(&get_string(LATEST_URL)?).map_err(|e| format!("bad reply: {e}"))?;
    let version = json["tag_name"]
        .as_str()
        .ok_or("reply has no tag_name")?
        .trim_start_matches('v')
        .to_string();
    let asset = |name: &str| {
        json["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"] == name)
            .cloned()
    };
    let url_of = |a: &serde_json::Value| {
        a["browser_download_url"]
            .as_str()
            .filter(|u| u.starts_with(DOWNLOAD_PREFIX))
            .map(str::to_string)
    };
    let newer = is_newer(&version, CURRENT);
    let rel = if newer {
        let setup = asset(SETUP_NAME).ok_or("release has no installer")?;
        let sha = asset(&format!("{SETUP_NAME}.sha256")).ok_or("release has no checksum")?;
        Some(Release {
            notes: release_notes(json["body"].as_str().unwrap_or_default()),
            size: setup["size"].as_u64().unwrap_or(0),
            page: json["html_url"]
                .as_str()
                .filter(|u| u.starts_with(PAGE_PREFIX))
                .unwrap_or(PAGE_PREFIX)
                .to_string(),
            sha: String::new(),
            setup_url: url_of(&setup).ok_or("installer URL is not on GitHub")?,
            sha_url: url_of(&sha).ok_or("checksum URL is not on GitHub")?,
            version,
        })
    } else {
        None
    };
    if let Ok(mut s) = STATE.lock() {
        s.release = rel.clone();
        s.last_checked = Some(stamp());
        s.phase = if newer {
            Phase::Available
        } else {
            Phase::UpToDate
        };
    }
    Ok(rel)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u32, u32, u32)> {
        let mut p = v.split('.').map(|s| s.parse().ok());
        Some((p.next()??, p.next()??, p.next()??))
    }
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

fn release_notes(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(|l| match l.strip_prefix("**").and_then(|r| r.split_once("**")) {
            Some((head, _)) => head.trim_end_matches('.').to_string(),
            None => l.split(". ").next().unwrap_or(l).replace('`', ""),
        })
        .filter(|l| !l.is_empty())
        .take(3)
        .collect()
}

fn stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!("today, {:02}:{:02}", t.wHour, t.wMinute)
}

fn download_and_install(rel: &Release) -> Result<(), String> {
    let expected = get_string(&rel.sha_url)?
        .split_whitespace()
        .next()
        .ok_or("empty checksum file")?
        .to_ascii_lowercase();
    let bytes = download(&rel.setup_url, rel.size)?;
    set_phase(Phase::Verifying);
    let started = std::time::Instant::now();
    let actual = sha256_hex(&bytes)?;
    std::thread::sleep(MIN_VERIFY.saturating_sub(started.elapsed()));
    if actual != expected {
        return Err(format!(
            "checksum mismatch\ngot      {}\nexpected {}",
            short_hash(&actual),
            short_hash(&expected)
        ));
    }
    if let Ok(mut s) = STATE.lock() {
        if let Some(r) = s.release.as_mut() {
            r.sha = actual;
        }
        s.phase = Phase::Installing;
    }
    let dir = Path::new(crate::install::DATA_DIR).join("updates");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let setup = dir.join(format!("warmup-companion-setup-{}.exe", rel.version));
    std::fs::write(&setup, &bytes).map_err(|e| format!("{}: {e}", setup.display()))?;
    std::thread::sleep(Duration::from_millis(800));
    log(&format!("running {} /S", setup.display()));
    std::process::Command::new(&setup)
        .arg("/S")
        .spawn()
        .map_err(|e| format!("start installer: {e}"))?;
    Ok(())
}

fn short_hash(h: &str) -> String {
    if h.len() > 12 {
        format!("{}…{}", &h[..4], &h[h.len() - 4..])
    } else {
        h.to_string()
    }
}

fn client() -> Result<HttpClient, String> {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let c = HttpClient::new().map_err(err)?;
    c.DefaultRequestHeaders()
        .and_then(|h| h.UserAgent())
        .and_then(|ua| ua.TryParseAdd(&HSTRING::from(format!("warmup-companion/{CURRENT}"))))
        .map_err(err)?;
    Ok(c)
}

fn uri(url: &str) -> Result<Uri, String> {
    Uri::CreateUri(&HSTRING::from(url)).map_err(err)
}

fn get_string(url: &str) -> Result<String, String> {
    let s = client()?
        .GetStringAsync(&uri(url)?)
        .and_then(|op| op.get())
        .map_err(|e| format!("{url}: {}", err(e)))?;
    Ok(s.to_string())
}

fn download(url: &str, size: u64) -> Result<Vec<u8>, String> {
    let fail = |e: windows::core::Error| format!("download: {}", err(e));
    let resp = client()?
        .GetWithOptionAsync(&uri(url)?, HttpCompletionOption::ResponseHeadersRead)
        .and_then(|op| op.get())
        .map_err(fail)?;
    if !resp.IsSuccessStatusCode().map_err(fail)? {
        return Err(format!(
            "download: HTTP {}",
            resp.StatusCode().map(|c| c.0).unwrap_or_default()
        ));
    }
    let stream = resp
        .Content()
        .and_then(|c| c.ReadAsInputStreamAsync())
        .and_then(|op| op.get())
        .map_err(fail)?;
    let mut out = Vec::with_capacity(size as usize);
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let chunk: IBuffer = stream
            .ReadAsync(&Buffer::Create(CHUNK).map_err(fail)?, CHUNK, InputStreamOptions::Partial)
            .and_then(|op| op.get())
            .map_err(fail)?;
        if chunk.Length().map_err(fail)? == 0 {
            break;
        }
        let mut part = windows::core::Array::<u8>::new();
        CryptographicBuffer::CopyToByteArray(&chunk, &mut part).map_err(fail)?;
        out.extend_from_slice(&part);
        set_phase(Phase::Downloading {
            got: out.len() as u64,
            total: size.max(out.len() as u64),
        });
    }
    Ok(out)
}

fn sha256_hex(bytes: &[u8]) -> Result<String, String> {
    let hash = CryptographicBuffer::CreateFromByteArray(bytes)
        .and_then(|b| {
            HashAlgorithmProvider::OpenAlgorithm(&HSTRING::from("SHA256"))?.HashData(&b)
        })
        .and_then(|h| CryptographicBuffer::EncodeToHexString(&h))
        .map_err(err)?;
    Ok(hash.to_string().to_ascii_lowercase())
}

fn spawn_toast(version: &str) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match crate::win::speech_input::spawn_as_user(&exe, &format!("--update-toast-helper {version}"))
    {
        Ok(h) => unsafe {
            let _ = CloseHandle(h);
        },
        Err(e) => log(&format!("toast spawn failed: {e}")),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn create_event(name: &str) -> Option<HANDLE> {
    let sddl = wide(EVENT_SDDL);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), 1, &mut sd, None)
        .ok()?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let name = wide(name);
    let event = CreateEventW(Some(&sa), false, false, PCWSTR(name.as_ptr())).ok();
    let _ = LocalFree(HLOCAL(sd.0));
    event
}

fn watch_events() {
    let events = unsafe { [create_event(OPEN_EVENT), create_event(INSTALL_EVENT)] };
    let [Some(open_ev), Some(install_ev)] = events else {
        log("could not create toast events");
        return;
    };
    loop {
        let hit = unsafe { WaitForMultipleObjects(&[open_ev, install_ev], false, INFINITE) };
        if hit == WAIT_OBJECT_0 {
            open();
        } else if hit.0 == WAIT_OBJECT_0.0 + 1 {
            if matches!(snapshot().phase, Phase::Available) {
                start_install();
                crate::win::update_window::show();
            } else {
                open();
            }
        } else {
            return;
        }
    }
}

pub(crate) fn signal_from_url(url: &str) {
    let name = wide(if url.contains("install") {
        INSTALL_EVENT
    } else {
        OPEN_EVENT
    });
    unsafe {
        match OpenEventW(
            SYNCHRONIZATION_ACCESS_RIGHTS(EVENT_MODIFY_STATE),
            false,
            PCWSTR(name.as_ptr()),
        ) {
            Ok(h) => {
                let _ = SetEvent(h);
                let _ = CloseHandle(h);
            }
            Err(e) => log(&format!("toast action: {e}")),
        }
    }
}

fn err(e: windows::core::Error) -> String {
    e.message().to_string()
}

fn log(msg: &str) {
    crate::install::log_line(&format!("update: {msg}"));
}

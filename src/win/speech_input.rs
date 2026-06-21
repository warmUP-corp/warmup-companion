//! VK mic key speech input (offline, whisper.cpp).
//!
//! The companion worker runs as SYSTEM (winlogon's duplicated token) inside the
//! active console session — see `service::launch_worker_in_active_session`. That
//! token has no per-user microphone consent, so the worker does NOT capture audio
//! itself. On the mic toggle it spawns a short-lived helper process as the *real
//! logged-in user* (`--speech-helper`, launched via `WTSQueryUserToken` +
//! `CreateProcessAsUserW`, mirroring `main::spawn_warmup_as_active_user`). The
//! helper has the user's normal mic consent, captures dictation, and injects the
//! recognized text with `SendInput` on `winsta0\default` — it *is* the user.
//! Toggling the mic again kills the helper.
//!
//! Recognition itself is whisper.cpp, shipped as an *optional* downloaded sidecar
//! (`whisper-server.exe` + a GGML model under `C:\ProgramData\WarmupVk\speech\`),
//! not linked into this binary. The Mic key is hidden unless both are present
//! (see [`available`]), so speech is a true opt-in install. The helper streams
//! each utterance to a resident `whisper-server` over loopback; the server keeps
//! the model in memory, so only the first dictation of a session pays the load.
//!
//! Recognition is blocked on the Winlogon/secure desktop anyway (the caller in
//! `vk_nav::start_voice_input` bails on `logon_focus::is_active`), so the helper
//! only ever runs on the default desktop where the user has audio + consent.

use std::sync::Mutex;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetExitCodeProcess, TerminateProcess, CREATE_NEW_PROCESS_GROUP,
    CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    STARTUPINFOW,
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

/// What the mic UI should show: the live helper's phase while it's running, else
/// None (idle). Keyed off the live helper (not the toggle), so the "transcribing"
/// halo persists after the user stops — until the text lands and the helper exits.
pub fn voice_ui_phase() -> Option<String> {
    let alive = HELPER
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| process_alive(h.process)))
        .unwrap_or(false);
    alive.then(|| current_phase().unwrap_or_else(|| "starting".to_string()))
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
        let startup = STARTUPINFOW {
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
            &startup,
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
// Recognition (`--speech-helper`, runs as the real user). Built only with the
// `speech` feature; without it the Mic key stays hidden and the helper no-ops.
// ---------------------------------------------------------------------------

#[cfg(feature = "speech")]
pub use engine::{
    available, current_phase, list_mics, mic_choice, request_stop, run_blocking, set_mic_choice,
    set_vk_language,
};

#[cfg(not(feature = "speech"))]
pub fn available() -> bool {
    false
}

#[cfg(not(feature = "speech"))]
pub fn list_mics() -> Vec<String> {
    Vec::new()
}

#[cfg(not(feature = "speech"))]
pub fn mic_choice() -> Option<String> {
    None
}

#[cfg(not(feature = "speech"))]
pub fn set_mic_choice(_name: &str) {}

#[cfg(not(feature = "speech"))]
pub fn set_vk_language(_de: bool) {}

#[cfg(not(feature = "speech"))]
pub fn current_phase() -> Option<String> {
    None
}

#[cfg(not(feature = "speech"))]
pub fn request_stop() {}

/// `available()` throttled to ~3s. The renderer asks every frame whether the mic
/// key is live; `available()` stats the filesystem, so cache it. A model dropped
/// in mid-session shows up within a few seconds — fast enough, no per-frame I/O.
pub fn available_cached() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let now = Instant::now();
    if let Ok(mut g) = CACHE.lock() {
        if let Some((t, v)) = *g {
            if now.duration_since(t) < Duration::from_secs(3) {
                return v;
            }
        }
        let v = available();
        *g = Some((now, v));
        return v;
    }
    available()
}

#[cfg(not(feature = "speech"))]
pub fn run_blocking() -> Result<(), String> {
    Err("speech feature not built into this binary".into())
}

#[cfg(feature = "speech")]
mod engine {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{Sample, SampleFormat};
    use windows::Win32::Globalization::GetUserDefaultLocaleName;

    /// Where the optional installer drops the whisper sidecar + model.
    const SPEECH_DIR: &str = r"C:\ProgramData\WarmupVk\speech";
    const SERVER_EXE: &str = "whisper-server.exe";
    const HOST: &str = "127.0.0.1";
    /// Loopback port for the resident whisper-server (uncommon to avoid clashes).
    const PORT: u16 = 17181;

    /// Safety cap: if the user never stops, transcribe + exit after this long so a
    /// runaway recording can't grow without bound.
    const MAX_RECORD_S: f32 = 180.0;

    fn server_path() -> PathBuf {
        Path::new(SPEECH_DIR).join(SERVER_EXE)
    }

    /// The model to load, in priority order:
    /// 1. `$WARMUP_WHISPER_MODEL` — explicit full-path override (power users).
    /// 2. `model.txt` — the filename the installer selected. Authoritative even
    ///    when several `*.bin` files are present (otherwise "first *.bin" wins and
    ///    silently ignores the user's choice).
    /// 3. Fallback: any `*.bin` in the speech dir.
    fn model_path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("WARMUP_WHISPER_MODEL") {
            let p = PathBuf::from(p);
            return p.is_file().then_some(p);
        }
        if let Ok(name) = std::fs::read_to_string(Path::new(SPEECH_DIR).join("model.txt")) {
            let chosen = Path::new(SPEECH_DIR).join(name.trim());
            if chosen.is_file() {
                return Some(chosen);
            }
        }
        std::fs::read_dir(SPEECH_DIR)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bin")))
    }

    /// True only if both the whisper sidecar and a model are installed.
    pub fn available() -> bool {
        server_path().is_file() && model_path().is_some()
    }

    fn mic_choice_path() -> PathBuf {
        Path::new(SPEECH_DIR).join("mic.txt")
    }

    /// Selected input-device name (substring), or None for the system default.
    /// Stored in the shared ProgramData speech dir so the SYSTEM-context tray can
    /// set it and this user-session helper can read it — their %LOCALAPPDATA% differ.
    pub fn mic_choice() -> Option<String> {
        std::fs::read_to_string(mic_choice_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Persist the mic choice (empty = system default).
    pub fn set_mic_choice(name: &str) {
        let path = mic_choice_path();
        if name.trim().is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, name.trim());
        }
    }

    /// Input device names cpal can see, for the tray mic picker.
    pub fn list_mics() -> Vec<String> {
        cpal::default_host()
            .input_devices()
            .map(|it| it.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    /// The chosen input device (by name substring) or the system default. The
    /// common reason dictation looks dead is the default being a virtual/silent
    /// device (e.g. "Steam Streaming Microphone") — the picker is the fix.
    fn pick_device(host: &cpal::Host) -> Option<cpal::Device> {
        if let Some(want) = mic_choice() {
            if let Ok(devices) = host.input_devices() {
                for d in devices {
                    if d.name().map(|n| n.contains(&want)).unwrap_or(false) {
                        return Some(d);
                    }
                }
            }
        }
        host.default_input_device()
    }

    fn lang_path() -> PathBuf {
        Path::new(SPEECH_DIR).join("lang.txt")
    }

    /// Worker-side (SYSTEM): record the language the user picked via the VK's
    /// DE/ENG toggle. This user-session helper reads it per utterance, so toggling
    /// the keyboard language drives recognition live (no server restart).
    pub fn set_vk_language(de: bool) {
        let _ = std::fs::write(lang_path(), if de { "de" } else { "en" });
    }

    /// Helper-side: the VK language, or None to fall back to the system locale.
    fn vk_language() -> Option<String> {
        std::fs::read_to_string(lang_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn status_path() -> PathBuf {
        Path::new(SPEECH_DIR).join("rt").join("status")
    }

    /// Helper-side: publish the current phase so the worker's UI can show it. The
    /// `rt` dir is created Users-writable by the installer (helper is non-elevated);
    /// best-effort, so a missing dir just means the UI shows the default state.
    fn set_phase(phase: &str) {
        let _ = std::fs::write(status_path(), phase);
    }

    /// Worker-side: the helper's current phase ("starting" | "listening" |
    /// "transcribing"), for the mic-key UI. None when not set.
    pub fn current_phase() -> Option<String> {
        std::fs::read_to_string(status_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn stop_path() -> PathBuf {
        Path::new(SPEECH_DIR).join("rt").join("stop")
    }

    /// Worker-side: ask the running helper to finish — it transcribes the whole
    /// recording, injects it, then exits. Graceful, NOT a kill (a kill would lose
    /// the recording).
    pub fn request_stop() {
        let _ = std::fs::write(stop_path(), "1");
    }

    fn stop_requested() -> bool {
        stop_path().exists()
    }

    fn clear_stop() {
        let _ = std::fs::remove_file(stop_path());
    }

    fn addr() -> SocketAddr {
        SocketAddr::new(HOST.parse().expect("valid loopback ip"), PORT)
    }

    fn server_up() -> bool {
        TcpStream::connect_timeout(&addr(), Duration::from_millis(300)).is_ok()
    }

    /// All logical cores — measured ~3x faster than whisper-server's default and
    /// the difference between ~2s and ~0.6s per utterance on a base model.
    fn threads() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }

    /// Recognition language. `auto` works but runs an extra detection pass (~2x
    /// latency) and misfires on short clips, so default to the user's system
    /// locale (e.g. `de-DE` -> `de`). Override with `$WARMUP_WHISPER_LANG`
    /// (set it to `auto` for mixed-language dictation).
    fn language() -> String {
        if let Ok(l) = std::env::var("WARMUP_WHISPER_LANG") {
            let l = l.trim();
            if !l.is_empty() {
                return l.to_string();
            }
        }
        let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 1 {
            let name = String::from_utf16_lossy(&buf[..(n as usize - 1)]);
            if let Some(prim) = name.split('-').next().filter(|p| !p.is_empty()) {
                return prim.to_ascii_lowercase();
            }
        }
        "auto".to_string()
    }

    /// Ensure a resident whisper-server is listening. Spawns it detached (so it
    /// survives the helper being killed on mic toggle-off and keeps the model
    /// loaded across toggles) and waits for it to bind + load the model once.
    fn ensure_server() -> Result<(), String> {
        if server_up() {
            return Ok(());
        }
        let model = model_path().ok_or("no whisper model present")?;
        let exe = server_path();
        if !exe.is_file() {
            return Err(format!("whisper sidecar not installed: {}", exe.display()));
        }
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new(&exe)
            .arg("-m")
            .arg(&model)
            .arg("--host")
            .arg(HOST)
            .arg("--port")
            .arg(PORT.to_string())
            .arg("-t")
            .arg(threads().to_string())
            .arg("--language")
            .arg(language())
            .arg("-nt")
            // ponytail: detached + no window so it outlives this helper and stays
            // resident. No idle shutdown; it holds the model (~200MB) until logoff.
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("spawn whisper-server: {e}"))?;

        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline {
            if server_up() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err("whisper-server did not become ready (model load timed out)".into())
    }

    /// POST one WAV to the resident server's `/inference` and return the text.
    /// Hand-rolled multipart over a plain socket — no HTTP crate needed.
    fn transcribe(wav: &[u8]) -> Result<String, String> {
        let boundary = "----warmupvkmic";
        let mut body = Vec::with_capacity(wav.len() + 256);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
                 filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(wav);
        body.extend_from_slice(b"\r\n");
        // Recognition language: the VK DE/ENG toggle wins, else the system locale.
        let lang = vk_language().unwrap_or_else(language);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n{lang}\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; \
                 name=\"response_format\"\r\n\r\njson\r\n--{boundary}--\r\n"
            )
            .as_bytes(),
        );

        let head = format!(
            "POST /inference HTTP/1.1\r\nHost: {HOST}:{PORT}\r\n\
             Content-Type: multipart/form-data; boundary={boundary}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );

        let mut stream =
            TcpStream::connect_timeout(&addr(), Duration::from_secs(2)).map_err(|e| format!("connect: {e}"))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(&body))
            .map_err(|e| format!("send: {e}"))?;

        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).map_err(|e| format!("recv: {e}"))?;
        let text = String::from_utf8_lossy(&resp);
        // Robust against chunked framing: pull the JSON object out by braces.
        let json = match (text.find('{'), text.rfind('}')) {
            (Some(a), Some(b)) if b > a => &text[a..=b],
            _ => return Err(format!("no JSON in response: {}", text.lines().next().unwrap_or(""))),
        };
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("parse: {e}"))?;
        Ok(v["text"].as_str().unwrap_or("").trim().to_string())
    }

    /// Average interleaved frames of any sample type down to mono f32.
    fn push_mono<T>(data: &[T], channels: usize, buf: &std::sync::Mutex<Vec<f32>>)
    where
        T: Sample,
        f32: cpal::FromSample<T>,
    {
        let ch = channels.max(1);
        if let Ok(mut b) = buf.lock() {
            for frame in data.chunks(ch) {
                let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
                b.push(sum / ch as f32);
            }
        }
    }

    /// Linear-resample mono f32 to 16 kHz (whisper's required rate).
    fn resample_16k(input: &[f32], in_rate: u32) -> Vec<f32> {
        if in_rate == 16_000 || input.is_empty() {
            return input.to_vec();
        }
        let ratio = 16_000.0 / in_rate as f32;
        let out_len = (input.len() as f32 * ratio) as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src = i as f32 / ratio;
            let idx = src as usize;
            let frac = src - idx as f32;
            let a = input.get(idx).copied().unwrap_or(0.0);
            let b = input.get(idx + 1).copied().unwrap_or(a);
            out.push(a + (b - a) * frac);
        }
        out
    }

    /// 16 kHz mono 16-bit PCM WAV bytes from f32 samples.
    fn wav_16k_mono(samples: &[f32]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + samples.len() * 2);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
        w.extend_from_slice(&1u16.to_le_bytes()); // format = PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
        w.extend_from_slice(&16_000u32.to_le_bytes()); // sample rate
        w.extend_from_slice(&(16_000u32 * 2).to_le_bytes()); // byte rate
        w.extend_from_slice(&2u16.to_le_bytes()); // block align
        w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            w.extend_from_slice(&v.to_le_bytes());
        }
        w
    }

    fn log(msg: &str) {
        // SYSTEM service.log — best-effort; the data dir is ACL-locked, so a
        // non-elevated user helper's write here silently fails.
        crate::install::log_line(msg);
        // Mirror to a user-writable log so the real (user-session) helper's run is
        // visible regardless of the data-dir lockdown.
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            let dir = Path::new(&base).join("WarmupVk");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("speech-helper.log");
            if std::fs::metadata(&path).map(|m| m.len() > 512_000).unwrap_or(false) {
                let _ = std::fs::remove_file(&path);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{msg}");
            }
        }
    }

    /// Capture the mic, segment utterances by silence, transcribe each via the
    /// resident server, and inject the text. Blocks until idle auto-stop; the
    /// worker kills this process for a manual toggle-off.
    pub fn run_blocking() -> Result<(), String> {
        let host = cpal::default_host();
        let device = pick_device(&host).ok_or("no input device (microphone)")?;
        log(&format!(
            "speech: mic = {}",
            device.name().unwrap_or_else(|_| "<unknown>".into())
        ));
        let supported = device
            .default_input_config()
            .map_err(|e| format!("default_input_config: {e}"))?;
        let in_rate = supported.sample_rate().0;
        let channels = supported.channels() as usize;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let err_fn = |e: cpal::StreamError| log(&format!("mic stream error: {e}"));

        macro_rules! build {
            ($t:ty) => {{
                let buf = buf.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[$t], _: &cpal::InputCallbackInfo| push_mono(data, channels, &buf),
                    err_fn,
                    None,
                )
            }};
        }
        let stream = match sample_format {
            SampleFormat::F32 => build!(f32),
            SampleFormat::I16 => build!(i16),
            SampleFormat::I32 => build!(i32),
            SampleFormat::I8 => build!(i8),
            SampleFormat::U16 => build!(u16),
            SampleFormat::U8 => build!(u8),
            other => return Err(format!("unsupported mic sample format: {other:?}")),
        }
        .map_err(|e| format!("build_input_stream: {e}"))?;
        stream.play().map_err(|e| format!("mic stream play: {e}"))?;

        // Record the WHOLE monologue and transcribe once, on stop — the user
        // finishes their thought, then gets the text (no mid-sentence injection).
        // Audio also buffers here while the model loads the first time.
        clear_stop();
        set_phase("starting");
        ensure_server()?;
        set_phase("listening");
        log("speech: recording (whisper-server ready)");

        let tick = Duration::from_millis(100);
        let mut all: Vec<f32> = Vec::new();
        let mut peak = 0.0f32;
        let mut since_log = 0.0f32;
        loop {
            std::thread::sleep(tick);
            let new: Vec<f32> = {
                let mut b = buf.lock().map_err(|_| "mic buffer poisoned")?;
                std::mem::take(&mut *b)
            };
            if !new.is_empty() {
                let rms = (new.iter().map(|x| x * x).sum::<f32>() / new.len() as f32).sqrt();
                peak = peak.max(rms);
            }
            all.extend_from_slice(&new);
            let secs = all.len() as f32 / in_rate as f32;
            since_log += tick.as_secs_f32();
            if since_log >= 1.5 {
                log(&format!("speech: recording {secs:.0}s peak_rms={peak:.3}"));
                peak = 0.0;
                since_log = 0.0;
            }
            // Stop only when the user toggles voice off (or the safety cap).
            if stop_requested() || secs >= MAX_RECORD_S {
                break;
            }
        }

        set_phase("transcribing");
        let secs = all.len() as f32 / in_rate as f32;
        if !all.is_empty() {
            let wav = wav_16k_mono(&resample_16k(&all, in_rate));
            match transcribe(&wav) {
                Ok(t) if !t.is_empty() => {
                    log(&format!("speech: heard ({secs:.1}s) \"{t}\""));
                    crate::vk_nav::send_text_direct(&format!("{t} "));
                }
                Ok(_) => log(&format!("speech: heard ({secs:.1}s) but transcript empty")),
                Err(e) => log(&format!("speech transcribe failed: {e}")),
            }
        }
        // Clear the phase so the mic UI returns to idle as this helper exits.
        let _ = std::fs::remove_file(status_path());
        log("speech: done");
        Ok(())
    }
}

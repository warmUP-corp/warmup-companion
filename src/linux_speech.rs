use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[allow(dead_code)]
#[path = "win/coding_vocab.rs"]
mod coding_vocab;
use cpal::{FromSample, Sample, SampleFormat};

const MAX_RECORD_S: f32 = 180.0;
const AUTO_STOP_S: f32 = 2.5;
const PORT: u16 = 17182;
const RUNTIME: &str = "libonnxruntime.so";
const ORT_VERSION: &str = "1.24.4";
const PARAKEET_REPO: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const PARAKEET_REV: &str = "main";
const PARAKEET_FILES: [&str; 3] = [
    "encoder-model.int8.onnx",
    "decoder_joint-model.int8.onnx",
    "vocab.txt",
];
const REDUX_REPO: &str = "eschmidbauer/parakeet-redux-onnx";
const REDUX_REV: &str = "1c285ba78d6f1880803f65355133a5ad3559a407";
const REDUX_FILES: [&str; 3] = ["encoder-model.onnx", "decoder_joint-model.onnx", "vocab.txt"];
const MODEL_IDS: [&str; 2] = ["parakeet", "parakeet-redux"];
const DOWNLOAD_POLL: Duration = Duration::from_millis(250);

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static DISCARD: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(0);
static LEVEL: AtomicU32 = AtomicU32::new(0);
static VOICE_OFF_APPLIED: AtomicBool = AtomicBool::new(false);
static VOICE_OFF_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn visual_active() -> bool {
    PHASE.load(Ordering::SeqCst) != 0 || running()
}

pub fn phase() -> u8 {
    PHASE.load(Ordering::SeqCst)
}

pub fn level() -> f32 {
    f32::from_bits(LEVEL.load(Ordering::SeqCst)).clamp(0.0, 1.0)
}

pub fn available() -> bool {
    model_dir().is_some() && onnx_runtime().is_some()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mic {
    pub name: String,
    pub label: String,
}

pub fn speech_dir() -> PathBuf {
    crate::paths::data_dir().join("speech")
}

fn mic_choice_path() -> PathBuf {
    speech_dir().join("mic.txt")
}

pub fn mic_choice() -> Option<String> {
    std::fs::read_to_string(mic_choice_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

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

pub fn list_mics() -> Vec<Mic> {
    let Ok(output) = Command::new("pactl")
        .args(["-f", "json", "list", "sources"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(serde_json::Value::Array(sources)) = serde_json::from_slice(&output.stdout) else {
        return Vec::new();
    };
    sources
        .iter()
        .filter(|source| {
            source["properties"]["media.class"]
                .as_str()
                .is_some_and(|class| class.starts_with("Audio/Source"))
        })
        .filter_map(|source| {
            let name = source["name"].as_str()?.to_string();
            let label = source["description"]
                .as_str()
                .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| name.clone());
            Some(Mic { name, label })
        })
        .collect()
}

struct MicStream {
    _cpal: Option<cpal::Stream>,
    _alsa: Option<AlsaCapture>,
}

struct AlsaCapture {
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AlsaCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

static MIC_WARNED: Mutex<Option<String>> = Mutex::new(None);

fn warn_mic_fallback(want: &str, reason: &str) {
    crate::install::log_line(&format!(
        "speech: chosen mic {want} unavailable ({reason}), recording from the default input"
    ));
    let Ok(mut warned) = MIC_WARNED.lock() else {
        return;
    };
    if warned.as_deref() == Some(want) {
        return;
    }
    *warned = Some(want.to_string());
    let label = list_mics()
        .into_iter()
        .find(|m| m.name == want)
        .map(|m| m.label)
        .unwrap_or_else(|| want.to_string());
    let _ = crate::linux_notify::notify(
        "Voice typing uses the default microphone",
        &format!("{label} could not be opened, so dictation records from the default input."),
    );
}

fn open_chosen_mic(
    want: &str,
    buf: &std::sync::Arc<Mutex<Vec<f32>>>,
) -> Result<(AlsaCapture, u32), String> {
    if want.contains('"') || want.contains('\\') {
        return Err("unsupported source name".into());
    }
    let mut errors = Vec::new();
    for pcm in [
        format!("pipewire:NODE=\"{want}\""),
        format!("pulse:DEVICE=\"{want}\""),
    ] {
        match open_alsa_capture(&pcm, buf.clone()) {
            Ok(opened) => {
                crate::install::log_line(&format!(
                    "speech: source = {want} via {}",
                    pcm.split(':').next().unwrap_or("")
                ));
                return Ok(opened);
            }
            Err(error) => errors.push(error),
        }
    }
    Err(errors.join("; "))
}

fn open_alsa_capture(
    pcm_name: &str,
    buf: std::sync::Arc<Mutex<Vec<f32>>>,
) -> Result<(AlsaCapture, u32), String> {
    use alsa::pcm::{Access, Format, HwParams, PCM};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    let name = pcm_name.to_string();
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("warmup-mic".into())
        .spawn(move || {
            let opened = (|| -> Result<(PCM, u32), alsa::Error> {
                let pcm = PCM::new(&name, alsa::Direction::Capture, false)?;
                {
                    let hwp = HwParams::any(&pcm)?;
                    hwp.set_access(Access::RWInterleaved)?;
                    hwp.set_format(Format::float())?;
                    hwp.set_channels(1)?;
                    hwp.set_rate_near(48_000, alsa::ValueOr::Nearest)?;
                    hwp.set_period_size_near(960, alsa::ValueOr::Nearest)?;
                    pcm.hw_params(&hwp)?;
                }
                let rate = pcm.hw_params_current()?.get_rate()?;
                pcm.start()?;
                Ok((pcm, rate))
            })();
            let (pcm, rate) = match opened {
                Ok(opened) => opened,
                Err(error) => {
                    let _ = tx.send(Err(format!("{name}: {error}")));
                    return;
                }
            };
            let io = match pcm.io_f32() {
                Ok(io) => io,
                Err(error) => {
                    let _ = tx.send(Err(format!("{name}: {error}")));
                    return;
                }
            };
            let _ = tx.send(Ok(rate));
            let mut chunk = [0f32; 960];
            while !flag.load(Ordering::SeqCst) {
                match io.readi(&mut chunk) {
                    Ok(frames) => push_mono(&chunk[..frames], 1, &buf),
                    Err(error) => {
                        if pcm.try_recover(error, true).is_err() {
                            crate::install::log_line(&format!("mic stream error: {error}"));
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|error| error.to_string())?;
    match rx.recv() {
        Ok(Ok(rate)) => Ok((
            AlsaCapture {
                stop,
                thread: Some(thread),
            },
            rate,
        )),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(format!("{pcm_name}: capture thread ended"))
        }
    }
}

fn open_mic(buf: &std::sync::Arc<Mutex<Vec<f32>>>) -> Result<(MicStream, u32), String> {
    if let Some(want) = mic_choice() {
        if list_mics().iter().any(|m| m.name == want) {
            match open_chosen_mic(&want, buf) {
                Ok((capture, rate)) => {
                    let stream = MicStream {
                        _cpal: None,
                        _alsa: Some(capture),
                    };
                    return Ok((stream, rate));
                }
                Err(error) => warn_mic_fallback(&want, &error),
            }
        } else {
            crate::install::log_line(&format!(
                "speech: chosen mic {want} is not connected, recording from the default input"
            ));
        }
    }
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no microphone is available".to_string())?;
    crate::install::log_line(&format!(
        "speech: mic = {}",
        device.name().unwrap_or_else(|_| "<unknown>".into())
    ));
    let supported = device
        .default_input_config()
        .map_err(|error| format!("default_input_config: {error}"))?;
    let in_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let err_fn = |error: cpal::StreamError| {
        crate::install::log_line(&format!("mic stream error: {error}"));
    };
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
        SampleFormat::F64 => build!(f64),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I8 => build!(i8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U8 => build!(u8),
        other => return Err(format!("unsupported mic sample format: {other:?}")),
    }
    .map_err(|error| format!("build_input_stream: {error}"))?;
    stream
        .play()
        .map_err(|error| format!("mic stream play: {error}"))?;
    let stream = MicStream {
        _cpal: Some(stream),
        _alsa: None,
    };
    Ok((stream, in_rate))
}

fn engine_path() -> PathBuf {
    speech_dir().join("engine.txt")
}

pub fn engine() -> String {
    std::fs::read_to_string(engine_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| MODEL_IDS.contains(&s.as_str()))
        .unwrap_or_else(|| "parakeet".to_string())
}

pub fn set_engine(name: &str) {
    let next = if name == "parakeet-redux" {
        "parakeet-redux"
    } else {
        "parakeet"
    };
    let changed = engine() != next;
    if let Some(parent) = engine_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(engine_path(), next);
    if changed {
        quit_server();
    }
}

pub fn model_installed(id: &str) -> bool {
    let dir = speech_dir().join(id);
    MODEL_IDS.contains(&id) && has_model(&dir) && runtime_for(&dir).is_some()
}

pub fn parakeet_available() -> bool {
    model_installed("parakeet")
}

pub fn redux_available() -> bool {
    model_installed("parakeet-redux")
}

pub fn download_model(id: &str, progress: impl Fn(u8)) -> Result<(), String> {
    let (repo, rev, files): (&str, &str, &[&str]) = match id {
        "parakeet" => (PARAKEET_REPO, PARAKEET_REV, &PARAKEET_FILES),
        "parakeet-redux" => (REDUX_REPO, REDUX_REV, &REDUX_FILES),
        _ => return Err(format!("unknown voice model {id}")),
    };
    let dir = speech_dir().join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut jobs: Vec<(String, PathBuf)> = files
        .iter()
        .filter(|f| !dir.join(f).is_file())
        .map(|f| {
            (
                format!("https://huggingface.co/{repo}/resolve/{rev}/{f}?download=true"),
                dir.join(f),
            )
        })
        .collect();
    let runtime = dir.join(RUNTIME);
    let sibling = MODEL_IDS
        .iter()
        .map(|other| speech_dir().join(other).join(RUNTIME))
        .find(|p| *p != runtime && p.is_file());
    let archive = dir.join("onnxruntime.tgz");
    let need_runtime = !runtime.is_file();
    if need_runtime {
        match &sibling {
            Some(from) => {
                std::fs::copy(from, &runtime).map_err(|e| format!("{}: {e}", runtime.display()))?;
            }
            None => jobs.push((ort_url()?, archive.clone())),
        }
    }
    let sizes: Vec<u64> = jobs.iter().map(|(url, _)| remote_size(url)).collect();
    let total = sizes.iter().sum::<u64>().max(1);
    let mut done = 0u64;
    progress(0);
    for ((url, path), size) in jobs.iter().zip(&sizes) {
        crate::install::log_line(&format!("speech: downloading {url}"));
        fetch(url, path, |got| {
            progress((((done + got) * 100) / total).min(99) as u8);
        })?;
        done += size;
    }
    if archive.is_file() {
        let unpacked = unpack_runtime(&archive, &dir);
        let _ = std::fs::remove_file(&archive);
        unpacked?;
    }
    if !model_installed(id) {
        return Err(format!("{} is incomplete after the download", dir.display()));
    }
    set_engine(id);
    progress(100);
    Ok(())
}

fn ort_url() -> Result<String, String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => return Err(format!("no ONNX Runtime build for {other}")),
    };
    Ok(format!(
        "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-linux-{arch}-{ORT_VERSION}.tgz"
    ))
}

fn curl() -> Command {
    let mut command = Command::new("curl");
    command
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--connect-timeout", "15", "-A", "warmup-companion"]);
    command
}

fn remote_size(url: &str) -> u64 {
    let Ok(output) = curl().args(["-I", url]).stderr(Stdio::null()).output() else {
        return 0;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
        .last()
        .unwrap_or(0)
}

fn fetch(url: &str, path: &Path, on_progress: impl Fn(u64)) -> Result<(), String> {
    let part = path.with_extension("part");
    let _ = std::fs::remove_file(&part);
    let mut child = curl()
        .arg("-o")
        .arg(&part)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl is needed to download voice models: {e}"))?;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("curl: {e}"))? {
            break status;
        }
        on_progress(std::fs::metadata(&part).map_or(0, |m| m.len()));
        std::thread::sleep(DOWNLOAD_POLL);
    };
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let _ = std::fs::remove_file(&part);
        return Err(format!("{url}: {}", stderr.trim()));
    }
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn unpack_runtime(archive: &Path, dir: &Path) -> Result<(), String> {
    let staging = dir.join("onnxruntime-unpack");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let pattern = format!("*/lib/{RUNTIME}.{ORT_VERSION}");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .args(["--wildcards", &pattern])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let found = std::fs::read_dir(&staging)
        .ok()
        .and_then(|mut entries| entries.find_map(|e| e.ok()))
        .map(|e| e.path().join("lib").join(format!("{RUNTIME}.{ORT_VERSION}")))
        .filter(|p| p.is_file());
    let result = match (status.success(), found) {
        (true, Some(lib)) => std::fs::rename(&lib, dir.join(RUNTIME))
            .map_err(|e| format!("{}: {e}", dir.join(RUNTIME).display())),
        _ => Err(format!("{} has no {RUNTIME}", archive.display())),
    };
    let _ = std::fs::remove_dir_all(&staging);
    result
}

pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

pub fn unload_engine() {
    if running() {
        crate::install::log_line("game started during dictation; voice engine stays loaded");
        return;
    }
    quit_server();
    std::thread::sleep(Duration::from_millis(500));
    if server_up() {
        quit_server();
    }
}

pub fn enforce_voice_enabled() {
    if crate::config::voice_enabled() {
        if VOICE_OFF_APPLIED.swap(false, Ordering::SeqCst) {
            VOICE_OFF_GENERATION.fetch_add(1, Ordering::SeqCst);
        }
        return;
    }
    if VOICE_OFF_APPLIED.swap(true, Ordering::SeqCst) {
        return;
    }
    let generation = VOICE_OFF_GENERATION.load(Ordering::SeqCst);
    crate::vk_nav::set_voice_input_active(false);
    crate::install::log_line(
        "voice transcription disabled; discarding any recording and unloading the engine",
    );
    DISCARD.store(true, Ordering::SeqCst);
    request_stop();
    if std::thread::Builder::new()
        .name("voice-unload".into())
        .spawn(move || {
            if VOICE_OFF_GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            quit_server();
            std::thread::sleep(Duration::from_millis(500));
            if VOICE_OFF_GENERATION.load(Ordering::SeqCst) != generation
                || crate::config::voice_enabled()
            {
                return;
            }
            if server_up() {
                quit_server();
            }
            if VOICE_OFF_GENERATION.load(Ordering::SeqCst) == generation
                && !crate::config::voice_enabled()
                && server_up()
            {
                VOICE_OFF_APPLIED.store(false, Ordering::SeqCst);
            }
        })
        .is_err()
    {
        VOICE_OFF_APPLIED.store(false, Ordering::SeqCst);
    }
}

fn quit_server() {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr(), Duration::from_millis(200)) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    let _ = stream.write_all(&u32::MAX.to_le_bytes());
}

pub fn toggle() {
    if running() {
        crate::install::log_line("vk voice input: OFF (finishing — transcribe on stop)");
        request_stop();
        crate::vk_nav::set_voice_input_active(false);
        return;
    }
    if !available() {
        let message = "Dictation needs a voice model. Download Parakeet in Control Center under Voice.";
        crate::install::log_line(message);
        let _ = crate::linux_notify::notify("Warmup Companion", message);
        return;
    }
    crate::install::log_line("vk voice input: ON");
    crate::vk_nav::set_voice_input_active(true);
    crate::linux_nimbus::ensure_running();
    #[cfg(feature = "gamepad")]
    crate::linux_voice::show();
    if let Err(error) = start() {
        crate::vk_nav::set_voice_input_active(false);
        set_phase(0);
        crate::install::log_line(&format!("voice input failed: {error}"));
        let _ = crate::linux_notify::notify("Warmup Companion", &error);
    }
}

pub fn run_server() -> Result<(), String> {
    #[cfg(feature = "parakeet")]
    {
        return engine::serve();
    }
    #[cfg(not(feature = "parakeet"))]
    {
        Err("the Parakeet voice engine is not built into this binary".into())
    }
}

fn start() -> Result<(), String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    STOP.store(false, Ordering::SeqCst);
    DISCARD.store(false, Ordering::SeqCst);
    set_phase(1);
    std::thread::Builder::new()
        .name("warmup-dictation".into())
        .spawn(|| {
            if let Err(error) = record_and_type() {
                crate::install::log_line(&format!("voice input: {error}"));
                let _ = crate::linux_notify::notify("Warmup Companion", &error);
            }
            set_phase(0);
            LEVEL.store(0.0f32.to_bits(), Ordering::SeqCst);
            RUNNING.store(false, Ordering::SeqCst);
            crate::vk_nav::set_voice_input_active(false);
        })
        .map_err(|error| {
            RUNNING.store(false, Ordering::SeqCst);
            set_phase(0);
            error.to_string()
        })?;
    Ok(())
}

fn set_phase(phase: u8) {
    PHASE.store(phase, Ordering::SeqCst);
}

struct MediaPause {
    players: Vec<String>,
}

impl MediaPause {
    fn hold() -> Self {
        let players = mpris_pause_playing();
        if !players.is_empty() {
            crate::install::log_line(&format!("speech: paused {} media session(s)", players.len()));
        }
        Self { players }
    }
}

impl Drop for MediaPause {
    fn drop(&mut self) {
        if self.players.is_empty() {
            return;
        }
        let n = mpris_resume(&self.players);
        if n > 0 {
            crate::install::log_line(&format!("speech: resumed {n} media session(s)"));
        }
    }
}

fn busctl(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("busctl")
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn mpris_names(list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            name.starts_with("org.mpris.MediaPlayer2.").then(|| name.to_string())
        })
        .collect()
}

fn playback_playing(property: &str) -> bool {
    property.split('"').nth(1).is_some_and(|status| status == "Playing")
}

fn mpris_pause_playing() -> Vec<String> {
    let Some(list) = busctl(&["list", "--no-pager"]) else {
        return Vec::new();
    };
    let mut paused = Vec::new();
    for name in mpris_names(&list) {
        let Some(status) = busctl(&[
            "get-property",
            &name,
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
            "PlaybackStatus",
        ]) else {
            continue;
        };
        if !playback_playing(&status) {
            continue;
        }
        if busctl(&[
            "call",
            &name,
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
            "Pause",
        ])
        .is_some()
        {
            paused.push(name);
        }
    }
    paused
}

fn mpris_resume(players: &[String]) -> usize {
    let mut resumed = 0usize;
    for name in players {
        if busctl(&[
            "call",
            name,
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
            "Play",
        ])
        .is_some()
        {
            resumed += 1;
        }
    }
    resumed
}

fn set_level(level: f32) {
    LEVEL.store(level.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
}

fn record_and_type() -> Result<(), String> {
    let _media = MediaPause::hold();
    ensure_server()?;
    let buf = std::sync::Arc::new(Mutex::new(Vec::<f32>::new()));
    let (stream, in_rate) = open_mic(&buf)?;
    let ready = std::thread::spawn(wait_ready);
    let tick = Duration::from_millis(50);
    let dt = tick.as_secs_f32();
    let mut all = Vec::<f32>::new();
    let mut noise = 0.0f32;
    let mut warmup = 16u32;
    let mut had_speech = false;
    let mut silence_s = 0.0f32;
    loop {
        std::thread::sleep(tick);
        let new = {
            let mut guard = buf.lock().map_err(|_| "mic buffer poisoned".to_string())?;
            std::mem::take(&mut *guard)
        };
        let rms = if new.is_empty() {
            0.0
        } else {
            (new.iter().map(|sample| sample * sample).sum::<f32>() / new.len() as f32).sqrt()
        };
        all.extend_from_slice(&new);
        let secs = all.len() as f32 / in_rate as f32;
        if warmup > 0 {
            noise = if noise == 0.0 { rms } else { noise * 0.6 + rms * 0.4 };
            warmup -= 1;
        }
        let gate = (noise * 1.6).max(0.012);
        let voiced = rms > gate;
        if voiced {
            had_speech = true;
            silence_s = 0.0;
        } else {
            silence_s += dt;
            noise = noise * 0.97 + rms * 0.03;
        }
        let target = ((rms - noise) / gate.max(0.02)).clamp(0.0, 1.0);
        set_level(target);
        let auto_stop = had_speech && silence_s >= AUTO_STOP_S;
        if STOP.load(Ordering::SeqCst) || auto_stop || secs >= MAX_RECORD_S {
            if auto_stop {
                crate::install::log_line(&format!("speech: auto-stop after {AUTO_STOP_S}s silence"));
            }
            break;
        }
    }
    drop(stream);
    set_phase(2);
    set_level(0.0);
    if DISCARD.load(Ordering::SeqCst) {
        crate::install::log_line("speech: recording discarded");
        return Ok(());
    }
    let secs = all.len() as f32 / in_rate as f32;
    if all.is_empty() || secs < 0.3 {
        crate::install::log_line("speech: nothing recorded");
        return Ok(());
    }
    ready
        .join()
        .map_err(|_| "voice engine thread panicked".to_string())?
        .map_err(|error| format!("voice engine: {error}"))?;
    if DISCARD.load(Ordering::SeqCst) {
        crate::install::log_line("speech: recording discarded");
        return Ok(());
    }
    let samples = resample_16k(&all, in_rate);
    let heard = transcribe_via_server(&samples)?;
    let clean = coding_vocab::current().fix(&clean_transcript(&heard));
    if clean.is_empty() {
        crate::install::log_line(&format!("speech: heard ({secs:.1}s) non-speech, skipped"));
        return Ok(());
    }
    crate::install::log_line(&format!("speech: heard ({secs:.1}s) \"{clean}\""));
    crate::vk_nav::send_text_direct(&format!("{clean} "));
    Ok(())
}

fn has_model(dir: &Path) -> bool {
    let encoder = dir.join("encoder-model.int8.onnx").is_file() || dir.join("encoder-model.onnx").is_file();
    let decoder = dir.join("decoder_joint-model.int8.onnx").is_file()
        || dir.join("decoder_joint-model.onnx").is_file()
        || dir.join("decoder_joint.onnx").is_file();
    encoder && decoder && dir.join("vocab.txt").is_file()
}

fn model_dir() -> Option<PathBuf> {
    let classic = speech_dir().join("parakeet");
    let redux = speech_dir().join("parakeet-redux");
    if engine() == "parakeet-redux" && has_model(&redux) {
        return Some(redux);
    }
    if has_model(&classic) {
        return Some(classic);
    }
    if has_model(&redux) {
        return Some(redux);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let bundled = PathBuf::from(home)
            .join(".local/share/voxtype/models/parakeet-tdt-0.6b-v3-int8");
        if has_model(&bundled) {
            return Some(bundled);
        }
    }
    None
}

fn onnx_runtime() -> Option<PathBuf> {
    runtime_for(&model_dir()?)
}

fn runtime_for(dir: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let beside = dir.join(RUNTIME);
    if beside.is_file() {
        return Some(beside);
    }
    [
        "/usr/lib/libonnxruntime.so",
        "/usr/lib64/libonnxruntime.so",
        "/usr/lib/voxtype/cuda-13/libonnxruntime.so",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

fn addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], PORT))
}

fn server_up() -> bool {
    TcpStream::connect_timeout(&addr(), Duration::from_millis(200)).is_ok()
}

fn ensure_server() -> Result<(), String> {
    if server_up() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    Command::new(exe)
        .arg("--parakeet-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("voice engine: {error}"))?;
    crate::install::log_line("speech: starting the voice engine");
    Ok(())
}

fn wait_ready() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if server_up() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Err("the voice engine did not become ready".into())
}

fn transcribe_via_server(samples: &[f32]) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&addr(), Duration::from_secs(2))
        .map_err(|error| format!("connect: {error}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut buf = Vec::with_capacity(4 + samples.len() * 4);
    buf.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for sample in samples {
        buf.extend_from_slice(&sample.to_le_bytes());
    }
    stream
        .write_all(&buf)
        .map_err(|error| format!("send: {error}"))?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut resp = Vec::new();
    stream
        .read_to_end(&mut resp)
        .map_err(|error| format!("recv: {error}"))?;
    Ok(String::from_utf8_lossy(&resp).trim().to_string())
}

fn clean_transcript(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_mono<T>(data: &[T], channels: usize, buf: &Mutex<Vec<f32>>)
where
    T: Sample,
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    if let Ok(mut samples) = buf.lock() {
        for frame in data.chunks(channels) {
            let sum: f32 = frame.iter().copied().map(f32::from_sample).sum();
            samples.push(sum / channels as f32);
        }
    }
}

fn resample_16k(input: &[f32], in_rate: u32) -> Vec<f32> {
    if in_rate == 16_000 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = 16_000.0 / in_rate as f32;
    let out_len = (input.len() as f32 * ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for index in 0..out_len {
        let src = index as f32 / ratio;
        let idx = src as usize;
        let frac = src - idx as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(feature = "parakeet")]
mod engine {
    use super::{addr, model_dir, onnx_runtime, PORT};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    use parakeet_rs::Transcriber;

    pub fn serve() -> Result<(), String> {
        let dir = model_dir().ok_or("the Parakeet model is not installed")?;
        let runtime = onnx_runtime().ok_or("ONNX Runtime was not found")?;
        std::env::set_var("ORT_DYLIB_PATH", &runtime);
        crate::install::log_line(&format!(
            "parakeet-server: loading {}",
            dir.display()
        ));
        let mut model = parakeet_rs::ParakeetTDT::from_pretrained(&dir, None)
            .map_err(|error| format!("parakeet load: {error}"))?;
        crate::install::log_line("parakeet: model loaded on CPU");
        let listener = TcpListener::bind(addr()).map_err(|error| format!("parakeet-server bind {PORT}: {error}"))?;
        crate::install::log_line(&format!("parakeet-server: ready on {}", addr()));
        for stream in listener.incoming() {
            match stream {
                Ok(mut socket) => {
                    if let Err(error) = handle(&mut socket, &mut model) {
                        crate::install::log_line(&format!("parakeet-server: {error}"));
                    }
                }
                Err(error) => crate::install::log_line(&format!("parakeet-server: accept: {error}")),
            }
        }
        Ok(())
    }

    fn handle(socket: &mut TcpStream, model: &mut parakeet_rs::ParakeetTDT) -> Result<(), String> {
        let mut lenb = [0u8; 4];
        if socket.read_exact(&mut lenb).is_err() {
            return Ok(());
        }
        let count = u32::from_le_bytes(lenb);
        if count == u32::MAX {
            crate::install::log_line("parakeet-server: unload requested, exiting");
            std::process::exit(0);
        }
        let count = count as usize;
        if count == 0 {
            return Ok(());
        }
        if count > 16_000_000 {
            return Err(format!("sample count too large: {count}"));
        }
        let mut bytes = vec![0u8; count * 4];
        socket
            .read_exact(&mut bytes)
            .map_err(|error| format!("read samples: {error}"))?;
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        let text = model
            .transcribe_samples(samples, 16_000, 1, None)
            .map(|result| result.text)
            .map_err(|error| format!("parakeet: {error}"))?;
        socket
            .write_all(text.as_bytes())
            .map_err(|error| format!("write: {error}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_transcript, mpris_names, playback_playing};

    #[test]
    fn mpris_list_keeps_players_only() {
        let names = mpris_names(
            "org.freedesktop.DBus 1 dbus\norg.mpris.MediaPlayer2.chromium.instance1 2 chromium\n",
        );
        assert_eq!(names, vec!["org.mpris.MediaPlayer2.chromium.instance1".to_string()]);
    }

    #[test]
    fn playback_status_is_playing() {
        assert!(playback_playing("s \"Playing\"\n"));
        assert!(!playback_playing("s \"Paused\"\n"));
    }

    #[test]
    fn annotations_are_not_typed() {
        assert_eq!(clean_transcript("[BLANK_AUDIO] (noise)"), "");
        assert_eq!(clean_transcript("hello [Music] there"), "hello there");
    }
}

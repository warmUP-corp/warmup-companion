use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat};

const MAX_RECORD_S: f32 = 180.0;
const AUTO_STOP_S: f32 = 2.5;
const PORT: u16 = 17182;

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

pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

pub fn unload_engine() {
    DISCARD.store(true, Ordering::SeqCst);
    request_stop();
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
        let message = "Dictation needs the Parakeet model and ONNX Runtime next to it.";
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
    let buf = std::sync::Arc::new(Mutex::new(Vec::<f32>::new()));
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
    let clean = clean_transcript(&heard);
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
    let own = crate::paths::data_dir().join("speech").join("parakeet");
    if has_model(&own) {
        return Some(own);
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

fn cpu_onnx(path: PathBuf) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    if path.to_string_lossy().to_ascii_lowercase().contains("cuda") {
        return None;
    }
    Some(path)
}

fn onnx_runtime() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        if let Some(path) = cpu_onnx(PathBuf::from(path)) {
            return Some(path);
        }
    }
    if let Some(dir) = model_dir() {
        if let Some(path) = cpu_onnx(dir.join("libonnxruntime.so")) {
            return Some(path);
        }
    }
    [
        "/usr/lib/libonnxruntime.so",
        "/usr/lib64/libonnxruntime.so",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find_map(cpu_onnx)
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

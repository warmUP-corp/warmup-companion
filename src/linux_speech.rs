use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat};

const MAX_RECORD_S: f32 = 180.0;
const AUTO_STOP_S: f32 = 2.5;

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn available() -> bool {
    Command::new("voxtype")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

pub fn toggle() {
    if running() {
        crate::install::log_line("vk voice input: OFF (finishing — transcribe on stop)");
        request_stop();
        crate::vk_nav::set_voice_input_active(false);
        return;
    }
    if !available() {
        let message = "Dictation needs voxtype, with a Whisper or Parakeet model installed.";
        crate::install::log_line(message);
        let _ = crate::linux_notify::notify("Warmup Companion", message);
        return;
    }
    crate::install::log_line("vk voice input: ON");
    crate::vk_nav::set_voice_input_active(true);
    let _ = crate::linux_notify::notify("Warmup Companion", "Listening. Pause when you are done.");
    if let Err(error) = start() {
        crate::vk_nav::set_voice_input_active(false);
        crate::install::log_line(&format!("voice input failed: {error}"));
        let _ = crate::linux_notify::notify("Warmup Companion", &error);
    }
}

fn start() -> Result<(), String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    STOP.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("warmup-dictation".into())
        .spawn(|| {
            if let Err(error) = record_and_type() {
                crate::install::log_line(&format!("voice input: {error}"));
                let _ = crate::linux_notify::notify("Warmup Companion", &error);
            }
            RUNNING.store(false, Ordering::SeqCst);
            crate::vk_nav::set_voice_input_active(false);
        })
        .map_err(|error| {
            RUNNING.store(false, Ordering::SeqCst);
            error.to_string()
        })?;
    Ok(())
}

fn record_and_type() -> Result<(), String> {
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

    let tick = Duration::from_millis(50);
    let dt = tick.as_secs_f32();
    let mut all: Vec<f32> = Vec::new();
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
        if rms > gate {
            had_speech = true;
            silence_s = 0.0;
        } else {
            silence_s += dt;
            noise = noise * 0.97 + rms * 0.03;
        }
        let auto_stop = had_speech && silence_s >= AUTO_STOP_S;
        if STOP.load(Ordering::SeqCst) || auto_stop || secs >= MAX_RECORD_S {
            if auto_stop {
                crate::install::log_line(&format!("speech: auto-stop after {AUTO_STOP_S}s silence"));
            }
            break;
        }
    }
    drop(stream);

    let secs = all.len() as f32 / in_rate as f32;
    if all.is_empty() || secs < 0.3 {
        crate::install::log_line("speech: nothing recorded");
        return Ok(());
    }
    let samples = resample_16k(&all, in_rate);
    let path = wav_path()?;
    std::fs::write(&path, wav_16k_mono(&samples)).map_err(|error| error.to_string())?;
    let heard = transcribe(&path)?;
    let _ = std::fs::remove_file(&path);
    let clean = clean_transcript(&heard);
    if clean.is_empty() {
        crate::install::log_line(&format!("speech: heard ({secs:.1}s) non-speech, skipped"));
        return Ok(());
    }
    crate::install::log_line(&format!("speech: heard ({secs:.1}s) \"{clean}\""));
    crate::vk_nav::send_text_direct(&format!("{clean} "));
    Ok(())
}

fn wav_path() -> Result<PathBuf, String> {
    let dir = crate::paths::state_dir().join("speech");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join("utterance.wav"))
}

fn transcribe(path: &std::path::Path) -> Result<String, String> {
    let output = Command::new("voxtype")
        .arg("-q")
        .arg("transcribe")
        .arg(path)
        .output()
        .map_err(|error| format!("voxtype: {error}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "voxtype failed: {}",
            err.lines().next().unwrap_or("transcription failed")
        ));
    }
    Ok(transcript_from_voxtype(&String::from_utf8_lossy(&output.stdout)))
}

fn transcript_from_voxtype(stdout: &str) -> String {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with("Loading ")
                && !line.starts_with("Audio format:")
                && !line.starts_with("Processing ")
        })
        .unwrap_or("")
        .to_string()
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

fn wav_16k_mono(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&16_000u32.to_le_bytes());
    bytes.extend_from_slice(&(16_000u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{clean_transcript, transcript_from_voxtype};

    #[test]
    fn voxtype_stdout_keeps_the_transcript_line() {
        let stdout = "Loading audio file: \"/tmp/a.wav\"\nAudio format: 16000 Hz, 1 channel(s), Int\nProcessing 16000 samples (1.00s)...\n\nhello world\n";
        assert_eq!(transcript_from_voxtype(stdout), "hello world");
    }

    #[test]
    fn annotations_are_not_typed() {
        assert_eq!(clean_transcript("[BLANK_AUDIO] (noise)"), "");
        assert_eq!(clean_transcript("hello [Music] there"), "hello there");
    }
}

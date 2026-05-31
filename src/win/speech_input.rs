//! Background Windows speech recognition for the VK mic key.

use std::sync::Mutex;

use windows::core::HSTRING;
use windows::Foundation::{EventRegistrationToken, TimeSpan, TypedEventHandler};
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs, SpeechContinuousRecognitionMode,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint,
    SpeechRecognizer,
};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

const AUTO_STOP_SILENCE_TIMEOUT: TimeSpan = TimeSpan {
    // Windows TimeSpan is measured in 100ns ticks. Keep dictation open long
    // enough for gamepad use instead of dropping the mic after a short pause.
    Duration: 10 * 60 * 10_000_000,
};

struct SpeechSession {
    recognizer: SpeechRecognizer,
    session: SpeechContinuousRecognitionSession,
    result_token: EventRegistrationToken,
    completed_token: EventRegistrationToken,
}

static SPEECH: Mutex<Option<SpeechSession>> = Mutex::new(None);

pub fn is_active() -> bool {
    SPEECH.lock().map(|s| s.is_some()).unwrap_or(false)
}

pub fn start() -> Result<(), String> {
    let mut active = SPEECH
        .lock()
        .map_err(|_| "speech session lock poisoned".to_string())?;
    if active.is_some() {
        return Ok(());
    }

    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    let recognizer = SpeechRecognizer::new().map_err(|e| format!("SpeechRecognizer: {e}"))?;
    let constraint = SpeechRecognitionTopicConstraint::Create(
        SpeechRecognitionScenario::Dictation,
        &HSTRING::new(),
    )
    .map_err(|e| format!("SpeechRecognitionTopicConstraint: {e}"))?;
    recognizer
        .Constraints()
        .map_err(|e| format!("SpeechRecognizer.Constraints: {e}"))?
        .Append(&constraint)
        .map_err(|e| format!("SpeechRecognizer constraints append: {e}"))?;
    let compile = recognizer
        .CompileConstraintsAsync()
        .map_err(|e| format!("CompileConstraintsAsync: {e}"))?
        .get()
        .map_err(|e| format!("CompileConstraintsAsync.get: {e}"))?;
    if compile
        .Status()
        .map_err(|e| format!("compile status: {e}"))?
        != SpeechRecognitionResultStatus::Success
    {
        return Err(format!(
            "speech constraint compile failed: {:?}",
            compile.Status().unwrap_or_default()
        ));
    }

    let session = recognizer
        .ContinuousRecognitionSession()
        .map_err(|e| format!("ContinuousRecognitionSession: {e}"))?;
    let _ = session.SetAutoStopSilenceTimeout(AUTO_STOP_SILENCE_TIMEOUT);

    let result_token = session
        .ResultGenerated(&TypedEventHandler::new(
            move |_sender: &Option<SpeechContinuousRecognitionSession>,
                  args: &Option<SpeechContinuousRecognitionResultGeneratedEventArgs>| {
                if let Some(args) = args {
                    if let Ok(result) = args.Result() {
                        if result.Status()? == SpeechRecognitionResultStatus::Success {
                            let text = result.Text()?.to_string();
                            if !text.trim().is_empty() {
                                crate::vk_nav::send_text_direct(&format!("{} ", text.trim()));
                            }
                        }
                    }
                }
                Ok(())
            },
        ))
        .map_err(|e| format!("ResultGenerated: {e}"))?;

    let completed_token = session
        .Completed(&TypedEventHandler::new(
            move |_sender: &Option<SpeechContinuousRecognitionSession>,
                  args: &Option<SpeechContinuousRecognitionCompletedEventArgs>| {
                if let Some(args) = args {
                    match args.Status() {
                        Ok(status) => {
                            crate::install::log_line(&format!("speech input completed: {status:?}"))
                        }
                        Err(e) => crate::install::log_line(&format!(
                            "speech input completed: status unavailable ({e})"
                        )),
                    }
                }
                crate::vk_nav::set_voice_input_active(false);
                Ok(())
            },
        ))
        .map_err(|e| format!("Completed: {e}"))?;

    session
        .StartWithModeAsync(SpeechContinuousRecognitionMode::Default)
        .map_err(|e| format!("StartWithModeAsync: {e}"))?
        .get()
        .map_err(|e| format!("StartWithModeAsync.get: {e}"))?;

    *active = Some(SpeechSession {
        recognizer,
        session,
        result_token,
        completed_token,
    });
    Ok(())
}

pub fn stop() {
    let session = SPEECH.lock().ok().and_then(|mut active| active.take());
    let Some(session) = session else {
        return;
    };

    let _ = session.session.RemoveResultGenerated(session.result_token);
    let _ = session.session.RemoveCompleted(session.completed_token);
    let _ = session.session.StopAsync().and_then(|a| a.get());
    let _ = session.recognizer.Close();
}

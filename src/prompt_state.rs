use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::win::controller_tips::{self, Gate, TipsState};

pub const TICK_INTERVAL: Duration = Duration::from_millis(250);
pub const CONNECTED_VISUAL_DURATION: Duration = Duration::from_millis(2400);
pub const MORPH_DURATION: Duration = Duration::from_millis(420);
pub const DEBUG_REPLAY_PERIOD: Duration = Duration::from_millis(6500);
pub const DEBUG_REPLAY_CONNECT_AT: Duration = Duration::from_millis(1500);

static TIPS_REPLAY: AtomicBool = AtomicBool::new(false);

pub fn request_controller_tips() {
    TIPS_REPLAY.store(true, Ordering::SeqCst);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptVisual {
    Ready,
    NoPad,
    Connected,
    Listening,
    Transcribing,
    VoiceBorder,
    Starting,
    Tips(u8),
}

const TIPS_CODE_BASE: isize = 100;

impl PromptVisual {
    pub fn code(self) -> isize {
        match self {
            PromptVisual::Ready => 1,
            PromptVisual::NoPad => 2,
            PromptVisual::Connected => 3,
            PromptVisual::Listening => 4,
            PromptVisual::Transcribing => 5,
            PromptVisual::Starting => 6,
            PromptVisual::VoiceBorder => 7,
            PromptVisual::Tips(i) => TIPS_CODE_BASE + i as isize,
        }
    }

    pub fn from_code(code: isize) -> Self {
        match code {
            2 => PromptVisual::NoPad,
            3 => PromptVisual::Connected,
            4 => PromptVisual::Listening,
            5 => PromptVisual::Transcribing,
            6 => PromptVisual::Starting,
            7 => PromptVisual::VoiceBorder,
            n if (TIPS_CODE_BASE..TIPS_CODE_BASE + 256).contains(&n) => {
                PromptVisual::Tips((n - TIPS_CODE_BASE) as u8)
            }
            _ => PromptVisual::Ready,
        }
    }

    pub fn tip_index(self) -> Option<usize> {
        match self {
            PromptVisual::Tips(i) => Some(i as usize),
            _ => None,
        }
    }

    pub fn from_voice_phase(phase: &str) -> Self {
        match phase {
            "transcribing" => PromptVisual::Transcribing,
            "starting" => PromptVisual::Starting,
            _ => PromptVisual::Listening,
        }
    }
}

pub fn debug_replay_visual(elapsed: Duration) -> PromptVisual {
    let phase = Duration::from_nanos((elapsed.as_nanos() % DEBUG_REPLAY_PERIOD.as_nanos()) as u64);
    if phase < DEBUG_REPLAY_CONNECT_AT {
        PromptVisual::NoPad
    } else if phase < DEBUG_REPLAY_CONNECT_AT + CONNECTED_VISUAL_DURATION {
        PromptVisual::Connected
    } else {
        PromptVisual::Ready
    }
}

pub fn visual_is_voice(visual: PromptVisual) -> bool {
    matches!(
        visual,
        PromptVisual::Starting
            | PromptVisual::Listening
            | PromptVisual::VoiceBorder
            | PromptVisual::Transcribing
    )
}

pub fn visual_is_tips(visual: PromptVisual) -> bool {
    visual.tip_index().is_some()
}

pub fn fullscreen_border(visual: PromptVisual) -> bool {
    visual_is_voice(visual)
}

pub fn userland_only(visual: PromptVisual) -> bool {
    visual_is_voice(visual) || visual_is_tips(visual)
}

pub fn same_voice_pill(from: PromptVisual, to: PromptVisual) -> bool {
    visual_is_voice(from) && visual_is_voice(to)
}

pub fn can_morph_between(from: PromptVisual, to: PromptVisual) -> bool {
    from != to
        && !visual_is_voice(from)
        && !visual_is_voice(to)
        && (matches!(from, PromptVisual::Connected) || matches!(to, PromptVisual::Connected))
}

pub fn raw_morph_progress(start: Instant, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / MORPH_DURATION.as_secs_f32())
        .clamp(0.0, 1.0)
}

pub fn eased_morph_progress(start: Instant, now: Instant) -> f32 {
    let t = raw_morph_progress(start, now);
    t * t * (3.0 - 2.0 * t)
}

pub fn voice_overlay_visual(phase: Option<&str>, vk_open: bool) -> Option<PromptVisual> {
    let phase = phase?;
    if vk_open {
        Some(PromptVisual::VoiceBorder)
    } else {
        Some(PromptVisual::from_voice_phase(phase))
    }
}

pub fn connected_card_title(label: &str) -> String {
    let l = label.trim();
    let low = l.to_ascii_lowercase();
    if l.is_empty() || low == "none" {
        "Controller".to_string()
    } else if low.contains("slot") {
        if low.contains("hid") {
            "PlayStation Controller".to_string()
        } else {
            "Xbox Controller".to_string()
        }
    } else {
        l.to_string()
    }
}

pub struct TickInputs<'a> {
    pub connected: bool,
    pub vk_open: bool,
    pub voice: Option<&'a str>,
    pub default_desktop: bool,
    pub on_winlogon: bool,
    pub userland_debug: bool,
}

pub struct PromptState {
    pub last_tick: Instant,
    pub last_connected: bool,
    pub connected_visual_until: Option<Instant>,
    pub connected_card_shown: bool,
    pub debug_epoch: Instant,
    pub tips: TipsState,
}

impl Default for PromptState {
    fn default() -> Self {
        Self {
            last_tick: crate::time_util::stale(TICK_INTERVAL),
            last_connected: false,
            connected_visual_until: None,
            connected_card_shown: false,
            debug_epoch: Instant::now(),
            tips: TipsState::default(),
        }
    }
}

impl PromptState {
    pub fn tick_due(&mut self) -> Option<Instant> {
        if self.last_tick.elapsed() < TICK_INTERVAL {
            return None;
        }
        let now = Instant::now();
        self.last_tick = now;
        Some(now)
    }

    pub fn update_connected_visual(&mut self, connected: bool, now: Instant) {
        if connected && !self.last_connected && !self.connected_card_shown {
            self.connected_visual_until = Some(now + CONNECTED_VISUAL_DURATION);
            self.connected_card_shown = true;
        } else if !connected {
            self.connected_visual_until = None;
        }
        self.last_connected = connected;
    }

    pub fn visual(
        &mut self,
        now: Instant,
        inputs: &TickInputs,
        signin_hints: impl FnOnce() -> bool,
        game: impl FnOnce() -> bool,
        tips_file: &Path,
        log: &dyn Fn(&str),
    ) -> Option<PromptVisual> {
        self.update_connected_visual(inputs.connected, now);
        let connected_intro_active = self.connected_visual_until.is_some_and(|until| now < until);
        let tip = self.step_tips(now, inputs, game, tips_file, log);
        if inputs.voice.is_some() {
            voice_overlay_visual(inputs.voice, inputs.vk_open)
        } else if inputs.userland_debug {
            Some(debug_replay_visual(now.duration_since(self.debug_epoch)))
        } else if inputs.on_winlogon {
            if inputs.vk_open || !signin_hints() {
                None
            } else if connected_intro_active {
                Some(PromptVisual::Connected)
            } else if inputs.connected {
                Some(PromptVisual::Ready)
            } else {
                None
            }
        } else {
            tip.map(|i| PromptVisual::Tips(i as u8))
        }
    }

    fn step_tips(
        &mut self,
        now: Instant,
        inputs: &TickInputs,
        game: impl FnOnce() -> bool,
        tips_file: &Path,
        log: &dyn Fn(&str),
    ) -> Option<usize> {
        if TIPS_REPLAY.swap(false, Ordering::SeqCst) {
            self.tips.request_replay();
        }
        let voice = inputs.voice.is_some();
        let calm = inputs.default_desktop && !inputs.vk_open && !voice && !self.tips.idle(inputs.connected);
        let game = calm && game();
        let gate = Gate {
            default_desktop: inputs.default_desktop,
            vk_open: inputs.vk_open,
            voice,
            game,
        };
        let current = env!("CARGO_PKG_VERSION");
        let step = self.tips.step(now, inputs.connected, gate, || {
            controller_tips::should_show(
                controller_tips::read_stored_version(tips_file).as_deref(),
                current,
            )
        });
        if step.started {
            log("prompt ui: controller tips started");
            if let Err(e) = controller_tips::write_stored_version(tips_file, current) {
                log(&format!("prompt ui: controller tips state write failed: {e}"));
            }
        }
        step.cue
    }
}

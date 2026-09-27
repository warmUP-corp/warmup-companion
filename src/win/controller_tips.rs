use std::path::Path;
use std::time::{Duration, Instant};

pub const CUE_DURATION: Duration = Duration::from_millis(3500);
pub const STATE_FILE: &str = r"C:\ProgramData\WarmupVk\controller-tips.version";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TipToken {
    Button(&'static str),
    Text(&'static str),
    Plus,
    Dot,
}

use TipToken::{Button, Dot, Plus, Text};

pub static CUES: [&[TipToken]; 5] = [
    &[Button("L3"), Text("Open the keyboard")],
    &[Button("R3"), Text("Dictate \u{2014} tap to talk")],
    &[
        Button("LB"),
        Plus,
        Button("RB"),
        Plus,
        Button("RT"),
        Text("Screenshot"),
        Dot,
        Button("LB"),
        Plus,
        Button("RB"),
        Plus,
        Button("LT"),
        Text("Window screenshot"),
    ],
    &[Button("SELECT"), Plus, Button("Y"), Text("Paste")],
    &[
        Text("In games: hold"),
        Button("SELECT"),
        Plus,
        Button("LB"),
        Text("Screenshot"),
    ],
];

pub fn cue_count() -> usize {
    CUES.len()
}

pub fn cue(index: usize) -> &'static [TipToken] {
    CUES.get(index).copied().unwrap_or(&[])
}

pub fn should_show(stored: Option<&str>, current: &str) -> bool {
    stored.map(str::trim) != Some(current.trim())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Gate {
    pub default_desktop: bool,
    pub vk_open: bool,
    pub voice: bool,
    pub game: bool,
}

impl Gate {
    pub fn clear(self) -> bool {
        self.default_desktop && !self.vk_open && !self.voice && !self.game
    }
}

pub fn cue_at(elapsed: Duration) -> Option<usize> {
    let index = (elapsed.as_millis() / CUE_DURATION.as_millis()) as usize;
    (index < cue_count()).then_some(index)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Trigger {
    Connect,
    Manual,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TipsStep {
    pub cue: Option<usize>,
    pub started: bool,
}

#[derive(Debug, Default)]
pub struct TipsState {
    pending: Option<Trigger>,
    started_at: Option<Instant>,
    last_connected: bool,
}

impl TipsState {
    pub fn request_replay(&mut self) {
        self.started_at = None;
        self.pending = Some(Trigger::Manual);
    }

    pub fn running(&self) -> bool {
        self.started_at.is_some()
    }

    pub fn idle(&self, connected: bool) -> bool {
        self.pending.is_none() && !self.running() && !(connected && !self.last_connected)
    }

    pub fn step(
        &mut self,
        now: Instant,
        connected: bool,
        gate: Gate,
        due: impl FnOnce() -> bool,
    ) -> TipsStep {
        if connected && !self.last_connected && self.pending.is_none() && !self.running() && due() {
            self.pending = Some(Trigger::Connect);
        }
        self.last_connected = connected;

        if let Some(started) = self.started_at {
            if !gate.clear() {
                self.started_at = None;
                return TipsStep::default();
            }
            let cue = cue_at(now.saturating_duration_since(started));
            if cue.is_none() {
                self.started_at = None;
            }
            return TipsStep {
                cue,
                started: false,
            };
        }

        let ready = match self.pending {
            Some(Trigger::Connect) => connected && gate.clear(),
            Some(Trigger::Manual) => gate.clear(),
            None => false,
        };
        if !ready {
            return TipsStep::default();
        }
        self.pending = None;
        self.started_at = Some(now);
        TipsStep {
            cue: Some(0),
            started: true,
        }
    }
}

pub fn read_stored_version(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn write_stored_version(path: &Path, version: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: Gate = Gate {
        default_desktop: true,
        vk_open: false,
        voice: false,
        game: false,
    };

    #[test]
    fn show_once_decision() {
        assert!(should_show(None, "0.6.2"));
        assert!(should_show(Some("0.6.1"), "0.6.2"));
        assert!(!should_show(Some("0.6.2"), "0.6.2"));
        assert!(!should_show(Some("0.6.2\r\n"), "0.6.2"));
    }

    #[test]
    fn gate_blocks_winlogon_vk_voice_and_game() {
        assert!(OPEN.clear());
        assert!(!Gate {
            default_desktop: false,
            ..OPEN
        }
        .clear());
        assert!(!Gate {
            vk_open: true,
            ..OPEN
        }
        .clear());
        assert!(!Gate {
            voice: true,
            ..OPEN
        }
        .clear());
        assert!(!Gate { game: true, ..OPEN }.clear());
    }

    #[test]
    fn waits_for_gate_then_starts() {
        let mut s = TipsState::default();
        let t0 = Instant::now();
        let winlogon = Gate {
            default_desktop: false,
            ..OPEN
        };
        assert_eq!(s.step(t0, true, winlogon, || true), TipsStep::default());
        let game = Gate { game: true, ..OPEN };
        assert_eq!(s.step(t0, true, game, || true), TipsStep::default());
        let step = s.step(t0, true, OPEN, || panic!("due is only read on connect"));
        assert_eq!(
            step,
            TipsStep {
                cue: Some(0),
                started: true
            }
        );
    }

    #[test]
    fn skips_when_version_already_shown() {
        let mut s = TipsState::default();
        let t0 = Instant::now();
        assert_eq!(s.step(t0, true, OPEN, || false), TipsStep::default());
        assert!(!s.running());
    }

    #[test]
    fn interruption_hides_and_does_not_replay() {
        let mut s = TipsState::default();
        let t0 = Instant::now();
        assert!(s.step(t0, true, OPEN, || true).started);
        let vk = Gate {
            vk_open: true,
            ..OPEN
        };
        assert_eq!(s.step(t0 + CUE_DURATION, true, vk, || true).cue, None);
        assert_eq!(s.step(t0 + CUE_DURATION * 2, true, OPEN, || true).cue, None);
        assert!(!s.running());
    }

    #[test]
    fn manual_replay_runs_without_a_new_connect() {
        let mut s = TipsState::default();
        let t0 = Instant::now();
        assert_eq!(s.step(t0, false, OPEN, || true), TipsStep::default());
        s.request_replay();
        assert!(s.step(t0, false, OPEN, || true).started);
    }

    #[test]
    fn cue_timeline() {
        assert_eq!(cue_at(Duration::ZERO), Some(0));
        assert_eq!(cue_at(CUE_DURATION - Duration::from_millis(1)), Some(0));
        assert_eq!(cue_at(CUE_DURATION), Some(1));
        assert_eq!(
            cue_at(CUE_DURATION * 4 + Duration::from_millis(10)),
            Some(4)
        );
        assert_eq!(cue_at(CUE_DURATION * 5), None);

        let mut s = TipsState::default();
        let t0 = Instant::now();
        s.step(t0, true, OPEN, || true);
        assert_eq!(
            s.step(t0 + CUE_DURATION * 2, true, OPEN, || true).cue,
            Some(2)
        );
        assert_eq!(s.step(t0 + CUE_DURATION * 5, true, OPEN, || true).cue, None);
        assert!(!s.running());
    }

    #[test]
    fn every_cue_uses_known_buttons() {
        let buttons: Vec<&str> = CUES
            .iter()
            .flat_map(|c| c.iter())
            .filter_map(|t| match t {
                Button(b) => Some(*b),
                _ => None,
            })
            .collect();
        for b in ["L3", "R3", "LB", "RB", "RT", "LT", "SELECT", "Y"] {
            assert!(buttons.contains(&b), "{b}");
        }
        assert_eq!(cue(0)[0], Button("L3"));
        assert_eq!(cue(9), &[] as &[TipToken]);
    }
}

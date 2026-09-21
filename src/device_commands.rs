//! Bounded queue of HID device writes (LED / rumble) drained by the gamepad loop.

use crate::gamepad_backend::PadCommand;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

static DEVICE_CMDS: OnceLock<Mutex<VecDeque<PadCommand>>> = OnceLock::new();
const DEVICE_CMD_CAP: usize = 64;

fn device_cmds() -> &'static Mutex<VecDeque<PadCommand>> {
    DEVICE_CMDS.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Queue a device write command (bounded; oldest dropped first).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn push_device_command(cmd: PadCommand) {
    if let Ok(mut q) = device_cmds().lock() {
        if q.len() >= DEVICE_CMD_CAP {
            q.pop_front();
        }
        q.push_back(cmd);
    }
}

fn coalesce_led_commands<I>(cmds: I) -> Vec<PadCommand>
where
    I: IntoIterator<Item = PadCommand>,
{
    let mut out = Vec::new();
    let mut last_led = None;
    for cmd in cmds {
        match cmd {
            PadCommand::Led { .. } => last_led = Some(cmd),
            _ => out.push(cmd),
        }
    }
    if let Some(cmd) = last_led {
        out.push(cmd);
    }
    out
}

/// Drain queued device write commands (LED/rumble) for the gamepad loop to apply.
pub fn drain_device_commands() -> Vec<PadCommand> {
    device_cmds()
        .lock()
        .map(|mut q| coalesce_led_commands(q.drain(..)))
        .unwrap_or_default()
}

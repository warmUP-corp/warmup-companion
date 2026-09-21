//! XInput DLL load, keystroke mapping, and secure-thread poll tick.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::PCSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT,
    XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_RIGHT_SHOULDER,
    XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::hid_gamepad::PadSample;
use crate::pad_decode::{norm_thumb, trigger_edges, LEFT_DEADZONE, RIGHT_DEADZONE};
use crate::xusb_ioctl::XusbDevice;

use super::raw_hid::report_hex;
use super::service_log;
use super::types::{
    ERROR_DEVICE_NOT_CONNECTED, ERROR_EMPTY, ERROR_SUCCESS, SLOTS, XInputGetKeystrokeFn,
    XInputGetStateFn, XInputKeystroke, XINPUT_KEYSTROKE_KEYDOWN, XINPUT_KEYSTROKE_KEYUP, PollState,
    SecureMsg, VK_PAD_A, VK_PAD_B, VK_PAD_DPAD_DOWN, VK_PAD_DPAD_LEFT, VK_PAD_DPAD_RIGHT,
    VK_PAD_DPAD_UP, VK_PAD_LSHOULDER, VK_PAD_RSHOULDER, VK_PAD_X, VK_PAD_Y,
};
use super::{INJECT_HOLD_ACTIVE, INJECT_HOLD_TICKS};

pub(crate) fn key_to_mask(vk: u16) -> Option<u16> {
    Some(match vk {
        VK_PAD_DPAD_UP => XINPUT_GAMEPAD_DPAD_UP.0,
        VK_PAD_DPAD_DOWN => XINPUT_GAMEPAD_DPAD_DOWN.0,
        VK_PAD_DPAD_LEFT => XINPUT_GAMEPAD_DPAD_LEFT.0,
        VK_PAD_DPAD_RIGHT => XINPUT_GAMEPAD_DPAD_RIGHT.0,
        VK_PAD_A => XINPUT_GAMEPAD_A.0,
        VK_PAD_B => XINPUT_GAMEPAD_B.0,
        VK_PAD_X => XINPUT_GAMEPAD_X.0,
        VK_PAD_Y => XINPUT_GAMEPAD_Y.0,
        VK_PAD_LSHOULDER => XINPUT_GAMEPAD_LEFT_SHOULDER.0,
        VK_PAD_RSHOULDER => XINPUT_GAMEPAD_RIGHT_SHOULDER.0,
        _ => return None,
    })
}

pub(crate) fn load_xinput_api() -> (
    Option<HMODULE>,
    Option<XInputGetStateFn>,
    Option<XInputGetKeystrokeFn>,
) {
    unsafe {
        for (name, dll) in [
            ("xinput1_4.dll", b"xinput1_4.dll\0".as_ptr()),
            ("xinput1_3.dll", b"xinput1_3.dll\0".as_ptr()),
        ] {
            let Ok(module) = LoadLibraryA(PCSTR(dll)) else {
                continue;
            };
            let proc = GetProcAddress(module, PCSTR(100usize as *const u8))
                .or_else(|| GetProcAddress(module, PCSTR(b"XInputGetState\0".as_ptr())));
            let Some(proc) = proc else {
                continue;
            };
            let get_state: XInputGetStateFn = std::mem::transmute(proc);
            let get_keystroke = GetProcAddress(module, PCSTR(b"XInputGetKeystroke\0".as_ptr()))
                .map(|p| std::mem::transmute::<_, XInputGetKeystrokeFn>(p));
            let label = format!("{name} ordinal 100/GetState");
            service_log(&format!(
                "XInput loader: {label}; keystroke={}",
                get_keystroke.is_some()
            ));
            return (Some(module), Some(get_state), get_keystroke);
        }
    }
    service_log("XInput loader: failed");
    (None, None, None)
}
pub(crate) fn poll_xinput_tick(state: &mut PollState) {
    // Re-enumerate XUSB pads while we have none — a controller connected after the
    // worker started (common at the lock screen) was missed by the one-shot
    // `open_all`, leaving only the foreground-gated DLL (always zeroed here).
    // Throttled to ~1s; only log on success so an empty scan doesn't spam.
    if state.xusb.is_empty() && state.last_xusb_scan.elapsed() >= Duration::from_secs(1) {
        state.last_xusb_scan = Instant::now();
        let (devices, log) = XusbDevice::open_all();
        if !devices.is_empty() {
            state.xusb = devices;
            for line in log {
                let _ = state.tx.send(SecureMsg::Error(line));
            }
        }
    }

    // Drop unplugged HID readers, and re-enumerate while we have none (a pad
    // connected after spawn — common at the lock screen — was missed by the
    // one-shot open in secure_poll_main).
    state.hid_readers.retain(|r| !r.is_dead());
    // Re-enumerate quickly while we have no readers so a replugged pad recovers in
    // ~150ms, not ~1s. Gated on is_empty(), so this only runs when there's nothing
    // to read — never during normal streaming.
    if state.hid_readers.is_empty() && state.last_hid_scan.elapsed() >= Duration::from_millis(150) {
        state.last_hid_scan = Instant::now();
        let (readers, log) = crate::hid_reader::HidReader::open_all();
        if !readers.is_empty() {
            state.hid_readers = readers;
            for line in log {
                let _ = state.tx.send(SecureMsg::Error(line));
            }
        }
    }

    // Drain each reader's overlapped read; keep the freshest non-neutral sample
    // (a pad streaming neutral frames shouldn't blank a just-pressed button from
    // another). `last_hid` then feeds the hid_authoritative slot-0 path below.
    let mut hid_sample: Option<PadSample> = None;
    for reader in state.hid_readers.iter_mut() {
        if let Some(sample) = reader.poll() {
            let replace = match hid_sample {
                None => true,
                Some(prev) => prev.buttons == 0 && prev.lt == 0 && prev.rt == 0,
            };
            if replace {
                hid_sample = Some(sample);
            }
        }
    }
    if let Some(sample) = hid_sample {
        state.last_hid = sample;
    }
    state.hid_readers.retain(|r| !r.is_dead());

    let mut connected = [false; 4];
    let mut states: [Option<XINPUT_GAMEPAD>; 4] = [None; 4];
    let mut errs = [0u32; 4];
    let mut packets = [0u32; 4];
    for slot in 0..SLOTS {
        let mut s = XINPUT_STATE::default();
        let err = unsafe { (state.get_state)(slot, &mut s) };
        errs[slot as usize] = err;
        if err == ERROR_SUCCESS {
            connected[slot as usize] = true;
            states[slot as usize] = Some(s.Gamepad);
            packets[slot as usize] = s.dwPacketNumber;
        } else if err != ERROR_DEVICE_NOT_CONNECTED
            && state.last_slot_err_log.elapsed() >= Duration::from_secs(1)
        {
            state.last_slot_err_log = Instant::now();
            let _ = state.tx.send(SecureMsg::Error(format!(
                "XInputGetState({slot}) error {err}"
            )));
        }
        // XInputGetKeystroke is foreground-gated like GetState. When physical XUSB
        // pads are open we read buttons directly from the driver below (no
        // foreground needed), so skip the gated path entirely — otherwise it would
        // fight `prev_buttons` with the XUSB edges.
        if state.xusb.is_empty() {
            if let Some(get_keystroke) = state.get_keystroke {
                state.keystroke_events +=
                    secure_poll_keystrokes(&state.tx, get_keystroke, slot, &mut state.prev_buttons);
            }
        }
    }

    // Direct XUSB read — authoritative on Winlogon, where DLL get_state is
    // gated to a neutral state by the foreground focus check. Pick the first
    // device that responds; mark its slot connected even if the DLL did not.
    let mut xusb_rep = None;
    let mut xusb_idx = None;
    for (i, dev) in state.xusb.iter().enumerate() {
        if let Some(rep) = dev.poll() {
            xusb_rep = Some(rep);
            xusb_idx = Some(i);
            break;
        }
    }
    if let Some(i) = xusb_idx {
        if i < SLOTS as usize {
            connected[i] = true;
        }
    }
    // Prune unplugged XUSB pads so `state.xusb` empties when the Xbox pad is
    // removed. Otherwise a stale entry keeps xusb non-empty, which blocks
    // `hid_is_authoritative` from ever handing slot 0 to a PlayStation pad
    // connected after it (the Xbox→PlayStation switch hang).
    if state.xusb.iter().any(|d| d.is_disconnected()) {
        state.xusb.retain(|d| !d.is_disconnected());
        if state.xusb.is_empty() {
            state.last_xusb = None;
            let _ = state.tx.send(SecureMsg::Error(
                "XUSB: pad disconnected; slot freed".into(),
            ));
        }
    }
    // Keep the last good report: a connected pad occasionally returns no bytes for
    // a single poll; replacing with None would flash a neutral (all-released) frame
    // and emit spurious button-up edges.
    if xusb_rep.is_some() {
        state.last_xusb = xusb_rep;
    }
    // HID is authoritative for PlayStation/non-XUSB pads. Xbox controllers can
    // also surface Raw Input HID devices; do not let that shadow the direct XUSB
    // path, which is the reliable secure-desktop source for Xbox.
    let hid_authoritative = hid_is_authoritative(state.hid_readers.len(), state.xusb.len());
    if hid_authoritative {
        connected[0] = true;
    }
    if hid_authoritative != state.hid_active_prev {
        state.hid_active_prev = hid_authoritative;
        let _ = state.tx.send(SecureMsg::HidActive(hid_authoritative));
    }

    if INJECT_HOLD_ACTIVE.load(Ordering::Relaxed) {
        let ticks = INJECT_HOLD_TICKS.load(Ordering::Relaxed);
        if ticks > 0 {
            INJECT_HOLD_TICKS.store(ticks - 1, Ordering::Relaxed);
            let _ = state.tx.send(SecureMsg::Axes((0.0, 0.0, 0.0, 0.0)));
            return;
        }

        INJECT_HOLD_ACTIVE.store(false, Ordering::Relaxed);
        state.last_xusb = None;
        for (slot, prev) in state.prev_buttons.iter_mut().enumerate() {
            if *prev != 0 {
                let old = *prev;
                *prev = 0;
                let _ = state.tx.send(SecureMsg::Buttons {
                    slot: slot as u32,
                    prev: old,
                    cur: 0,
                });
            }
        }
        let _ = state.tx.send(SecureMsg::Axes((0.0, 0.0, 0.0, 0.0)));
        return;
    }

    // Surface the raw XUSB report in the debug overlay so byte offsets can be
    // confirmed against live presses on Winlogon (the parsed `buttons` offset is
    // provisional — see xusb_ioctl::parse_report).
    // Verify the foreground experiment: show XInput slot-0 buttons + whether our
    // process currently owns the foreground window. If `fg=ours` and `xinput` goes
    // non-zero on a press, the gate is defeated and the existing edge path delivers it.
    {
        let xin = states[0].map(|p| p.wButtons.0).unwrap_or(0);
        let fg = unsafe { GetForegroundWindow() };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
        let ours = pid == unsafe { GetCurrentProcessId() };
        // Live press-test signal: show BOTH sources + an XUSB raw fingerprint that
        // changes whenever any report byte moves. On a press one of these must move,
        // or the pad stream is not reaching us at all (desktop/foreground gate).
        // Fingerprint the INPUT bytes only — skip header (0..5) and the free-running
        // counter at bytes 5..7, which tick every report even with zero input (that
        // was the "weird xfp stream"). xfp now moves only on a real button/stick/
        // trigger change, so a frozen xfp during a press == no live input reaching us.
        let (xusb_btn, xfp) = match &state.last_xusb {
            Some(r) => (
                r.buttons,
                r.raw
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i >= 7)
                    .fold(0u32, |a, (_, &b)| a.wrapping_mul(31).wrapping_add(b as u32)),
            ),
            None => (0u16, 0u32),
        };
        let hid = state.last_hid;
        crate::debug_state::set_detail(format!(
            "xinput=0x{xin:04x} xusb=0x{xusb_btn:04x} xfp=0x{xfp:08x} hid=0x{:04x}/{}:{} L({:.2},{:.2}) R({:.2},{:.2}) fg={}",
            hid.buttons,
            hid.lt,
            hid.rt,
            hid.lx,
            hid.ly,
            hid.rx,
            hid.ry,
            if ours { "ours" } else { "LogonUI" }
        ));
    }

    state.iter_count += 1;
    let probe_due = state.last_probe_log.elapsed() >= Duration::from_secs(2);
    if state.iter_count == 1 || probe_due {
        state.last_probe_log = Instant::now();
        let summary: Vec<String> = (0..SLOTS as usize)
            .map(|i| format!("{}:{}", i, errs[i]))
            .collect();
        let mut raw = String::new();
        for i in 0..SLOTS as usize {
            if let Some(pad) = states[i] {
                raw.push_str(&format!(
                    " s{i}:pkt={} btn=0x{:04x} lt={} rt={} lx={} ly={} rx={} ry={}",
                    packets[i],
                    pad.wButtons.0,
                    pad.bLeftTrigger,
                    pad.bRightTrigger,
                    pad.sThumbLX,
                    pad.sThumbLY,
                    pad.sThumbRX,
                    pad.sThumbRY,
                ));
            }
        }
        let xusb_str = match &state.last_xusb {
            Some(rep) => {
                let full: String = rep
                    .raw
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    " xusb:btn=0x{:04x} lt={} rt={} lx={} ly={} rx={} ry={} len={} raw=[{}]",
                    rep.buttons,
                    rep.left_trigger,
                    rep.right_trigger,
                    rep.thumb_lx,
                    rep.thumb_ly,
                    rep.thumb_rx,
                    rep.thumb_ry,
                    rep.raw.len(),
                    full,
                )
            }
            None => " xusb:none".into(),
        };
        let _ = state.tx.send(SecureMsg::Error(format!(
            "probe iter={} errs=[{}]{raw} keystroke_events={} hid:btn=0x{:04x} lt={} rt={} lx={:.2} ly={:.2} rx={:.2} ry={:.2} raw=[{}]{xusb_str}",
            state.iter_count,
            summary.join(","),
            state.keystroke_events,
            state.last_hid.buttons,
            state.last_hid.lt,
            state.last_hid.rt,
            state.last_hid.lx,
            state.last_hid.ly,
            state.last_hid.rx,
            state.last_hid.ry,
            // Prefer the direct-read bytes (the working secure-desktop path);
            // fall back to the WM_INPUT report, which never arrives on Winlogon.
            state
                .hid_readers
                .first()
                .map(|r| report_hex(r.last_report()))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| report_hex(&state.last_raw_report)),
        )));
    }

    let slots_changed = connected
        .iter()
        .zip(state.connected_prev.iter())
        .any(|(a, b)| a != b);
    if slots_changed || state.last_status.elapsed() >= Duration::from_secs(30) {
        state.last_status = Instant::now();
        state.connected_prev = connected;
        let _ = state.tx.send(SecureMsg::Slots(connected));
    }

    let slot = match state.active_slot {
        Some(slot) if connected[slot as usize] => Some(slot),
        _ => {
            state.active_slot = (0..SLOTS).find(|i| connected[*i as usize]);
            state.active_slot
        }
    };

    let Some(slot) = slot else {
        if state.last_no_pad.elapsed() >= Duration::from_secs(15) {
            state.last_no_pad = Instant::now();
            let _ = state.tx.send(SecureMsg::NoController);
        }
        return;
    };

    if hid_authoritative && slot == 0 {
        let prev = state.prev_buttons[0];
        let buttons = state.last_hid.buttons;
        if prev != buttons {
            state.prev_buttons[0] = buttons;
            let _ = state.tx.send(SecureMsg::Buttons {
                slot: 0,
                prev,
                cur: buttons,
            });
        }
        let mut trig = Vec::new();
        trigger_edges(
            &mut state.prev_trigger_left,
            &mut state.prev_trigger_right,
            state.last_hid.lt,
            state.last_hid.rt,
            &mut trig,
        );
        for edge in trig {
            let _ = state.tx.send(SecureMsg::Trigger(edge));
        }
        let _ = state.tx.send(SecureMsg::Axes((
            state.last_hid.lx,
            state.last_hid.ly,
            state.last_hid.rx,
            state.last_hid.ry,
        )));
        return;
    }

    // The DLL state may be absent (slot connected only via the direct XUSB read,
    // which the foreground gate doesn't touch) — default to neutral, never panic.
    let pad = states[slot as usize].unwrap_or_default();

    // Authoritative input source. On the secure desktop the DLL `pad` reads
    // neutral (btn=0) even while our anchor holds foreground — the invisible anchor
    // does not actually grant LogonUI's input rights — so the XUSB-direct read is
    // the only source that carries live buttons there. Prefer XUSB whenever a pad
    // is open; fall back to the DLL only when no XUSB pad is present. (During an
    // inject burst the anchor yields foreground for ~48ms and XUSB may briefly
    // freeze, but the user is not navigating then, so that is harmless.)
    let xusb = state.last_xusb.clone().filter(|_| !state.xusb.is_empty());
    let (buttons, lx, ly, rx, ry, lt, rt) = match &xusb {
        Some(r) => (
            r.buttons,
            r.thumb_lx,
            r.thumb_ly,
            r.thumb_rx,
            r.thumb_ry,
            r.left_trigger,
            r.right_trigger,
        ),
        None => (
            pad.wButtons.0,
            pad.sThumbLX,
            pad.sThumbLY,
            pad.sThumbRX,
            pad.sThumbRY,
            pad.bLeftTrigger,
            pad.bRightTrigger,
        ),
    };

    let mut trig = Vec::new();
    trigger_edges(
        &mut state.prev_trigger_left,
        &mut state.prev_trigger_right,
        lt,
        rt,
        &mut trig,
    );
    for edge in trig {
        let _ = state.tx.send(SecureMsg::Trigger(edge));
    }

    // Button edges from the authoritative source — handles press AND release, so it
    // owns `prev_buttons` outright (the foreground-gated keystroke path is skipped
    // above whenever an XUSB pad is open).
    let idx = slot as usize;
    let prev = state.prev_buttons[idx];
    if prev != buttons {
        state.prev_buttons[idx] = buttons;
        let _ = state.tx.send(SecureMsg::Buttons {
            slot,
            prev,
            cur: buttons,
        });
    }

    let stick_active = lx.abs() > LEFT_DEADZONE || ly.abs() > LEFT_DEADZONE;
    let axes = if stick_active {
        (
            norm_thumb(lx, LEFT_DEADZONE),
            norm_thumb(ly, LEFT_DEADZONE),
            norm_thumb(rx, RIGHT_DEADZONE),
            norm_thumb(ry, RIGHT_DEADZONE),
        )
    } else {
        (
            state.last_hid.lx,
            state.last_hid.ly,
            state.last_hid.rx,
            state.last_hid.ry,
        )
    };
    let _ = state.tx.send(SecureMsg::Axes(axes));
}

pub(crate) fn hid_is_authoritative(hid_device_count: usize, xusb_device_count: usize) -> bool {
    hid_device_count > 0 && xusb_device_count == 0
}

#[allow(dead_code)]
fn secure_poll_keystrokes(
    tx: &mpsc::Sender<SecureMsg>,
    get_keystroke: XInputGetKeystrokeFn,
    slot: u32,
    prev_buttons: &mut [u16; 4],
) -> u64 {
    let mut events = 0u64;
    for _ in 0..16 {
        let mut key = XInputKeystroke::default();
        let err = unsafe { get_keystroke(slot, 0, &mut key) };
        if err == ERROR_EMPTY || err == ERROR_DEVICE_NOT_CONNECTED {
            break;
        }
        if err != ERROR_SUCCESS {
            let _ = tx.send(SecureMsg::Error(format!(
                "XInputGetKeystroke({slot}) error {err}"
            )));
            break;
        }
        events += 1;
        let Some(mask) = key_to_mask(key.virtual_key) else {
            continue;
        };
        let idx = slot as usize;
        let prev = prev_buttons[idx];
        let mut cur = prev;
        if key.flags & XINPUT_KEYSTROKE_KEYDOWN != 0 {
            cur |= mask;
        }
        if key.flags & XINPUT_KEYSTROKE_KEYUP != 0 {
            cur &= !mask;
        }
        if prev != cur {
            prev_buttons[idx] = cur;
            service_log(&format!(
                "HID secure: keystroke slot {slot} vk=0x{:04x} -> 0x{cur:04x}",
                key.virtual_key
            ));
            let _ = tx.send(SecureMsg::Buttons { slot, prev, cur });
        }
    }
    events
}

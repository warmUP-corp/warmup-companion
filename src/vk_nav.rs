//! Gamepad-driven VK focus + full PC QWERTY grid.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(feature = "gamepad")]
use crate::gamepad_backend::Button;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_BACK, VK_END, VK_RETURN, VK_SPACE, VK_TAB,
};

#[derive(Clone)]
pub enum KeyAction {
    Char(char),
    Vk(VIRTUAL_KEY),
    /// Shift: RT or the on-screen Shift key (web `toggleShift`: one-shot,
    /// double-tap promotes to sticky caps).
    Shift,
    /// Symbol layer: LT or the on-screen `&123` key (web `toggleSymbols`).
    Symbols,
    /// Previous prediction-strip candidate (`<` key).
    PredictPrev,
    /// Next prediction-strip candidate (`>` key).
    PredictNext,
    /// Start background Windows speech recognition.
    VoiceInput,
    /// Dismiss the on-screen keyboard.
    CloseVk,
}

#[derive(Clone)]
pub struct KeyCell {
    pub label: String,
    /// Shifted symbol shown above the primary label (number row, etc.).
    pub sublabel: Option<String>,
    pub action: KeyAction,
    /// Width in key-units (1.0 = normal key).
    pub span: f32,
}

impl KeyCell {
    fn ch(c: char) -> Self {
        KeyCell {
            label: c.to_string(),
            sublabel: None,
            action: KeyAction::Char(c),
            span: 1.0,
        }
    }
    /// Character key with web semantics: `lower`/`upper`/`symbol` resolved by the
    /// active layer; the corner accent shows the symbol (or, on the symbol layer,
    /// the uppercase letter), matching `resolveVirtualKeyboardKeyAccent`.
    fn tri(lower: char, upper: char, symbol: char, layer: Layer) -> Self {
        let (c, accent) = match layer {
            Layer::Lower => (lower, symbol),
            Layer::Upper => (upper, symbol),
            Layer::Symbol => (symbol, upper),
        };
        KeyCell {
            label: c.to_string(),
            sublabel: Some(accent.to_string()),
            action: KeyAction::Char(c),
            span: 1.0,
        }
    }
    fn named(label: &str, action: KeyAction, span: f32) -> Self {
        KeyCell {
            label: label.to_string(),
            sublabel: None,
            action,
            span,
        }
    }
    fn vk(label: &str, vk: VIRTUAL_KEY, span: f32) -> Self {
        KeyCell {
            label: label.to_string(),
            sublabel: None,
            action: KeyAction::Vk(vk),
            span,
        }
    }
}

#[derive(Clone)]
pub struct KeyRow {
    pub keys: Vec<KeyCell>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyPos {
    pub row: usize,
    pub col: usize,
}

/// Active render layer, mirroring the web VK's `VirtualKeyboardLayer`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Lower,
    Upper,
    Symbol,
}

struct NavState {
    pos: KeyPos,
    layer: Layer,
    /// Upper layer reverts after one character unless promoted by double-tap.
    one_shot_shift: bool,
    /// Symbol layer reverts after one character unless promoted by double-tap.
    one_shot_symbol: bool,
    last_shift_at: Option<Instant>,
    last_symbol_at: Option<Instant>,
    /// QWERTZ letter rows (web `de-DE` language toggle on L3).
    lang_de: bool,
    voice_input: bool,
    rows: Vec<KeyRow>,
    #[cfg(feature = "gamepad")]
    hold_button: Option<Button>,
    hold_count: u32,
    hold_deadline: Option<Instant>,
    /// Held input button (A/B/Y) auto-repeating its action.
    repeat_key: Option<RepeatKey>,
    repeat_deadline: Option<Instant>,
}

static NAV: Mutex<NavState> = Mutex::new(NavState {
    pos: KeyPos { row: 0, col: 0 },
    layer: Layer::Lower,
    one_shot_shift: false,
    one_shot_symbol: false,
    last_shift_at: None,
    last_symbol_at: None,
    lang_de: false,
    voice_input: false,
    rows: Vec::new(),
    #[cfg(feature = "gamepad")]
    hold_button: None,
    hold_count: 0,
    hold_deadline: None,
    repeat_key: None,
    repeat_deadline: None,
});

/// Which held button drives the key auto-repeat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepeatKey {
    /// A/Touchpad held on the focused key.
    Activate,
    /// B held — backspace.
    Backspace,
    /// Y held — space.
    Space,
}

const HOLD_INITIAL: Duration = Duration::from_millis(250);
const HOLD_REPEAT: Duration = Duration::from_millis(70);
/// Second shift/symbol tap inside this window promotes one-shot to sticky
/// (web `DOUBLE_TAP_SHIFT_MS`).
const DOUBLE_TAP_STICKY: Duration = Duration::from_millis(400);

/// Edge action keys are 1.45 key-units wide, the space bar 5.15 — same flex
/// ratios as the web layout (`LEFT_ACTION_KEY_WIDTH` / `action-space`).
const SPAN_ACTION: f32 = 1.45;
const SPAN_SPACE: f32 = 5.15;

const TOP_LETTERS_EN: [char; 10] = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'];
const TOP_LETTERS_DE: [char; 10] = ['q', 'w', 'e', 'r', 't', 'z', 'u', 'i', 'o', 'p'];
const MID_LETTERS: [char; 9] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];
const BOTTOM_LETTERS_EN: [char; 7] = ['z', 'x', 'c', 'v', 'b', 'n', 'm'];
const BOTTOM_LETTERS_DE: [char; 7] = ['y', 'x', 'c', 'v', 'b', 'n', 'm'];

const TOP_SYMBOLS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
const MID_SYMBOLS: [char; 9] = ['@', '#', '$', '%', '&', '-', '+', '(', ')'];
const BOTTOM_SYMBOLS: [char; 7] = ['!', '?', '.', ',', ':', ';', '_'];

/// Quick-insert chips (web `PROFILE_QUICK_INSERTS.text`) — insert the same
/// character on every layer.
const QUICK_INSERTS: [char; 4] = ['-', '\'', '.', ','];

/// Four-row web VK card layout (`createVirtualKeyboardLayoutForLanguage`).
fn build_web_layout(layer: Layer, lang_de: bool) -> Vec<KeyRow> {
    let t = |lower: char, symbol: char| {
        let upper = lower.to_uppercase().next().unwrap_or(lower);
        KeyCell::tri(lower, upper, symbol, layer)
    };
    let top = if lang_de {
        TOP_LETTERS_DE
    } else {
        TOP_LETTERS_EN
    };
    let bottom = if lang_de {
        BOTTOM_LETTERS_DE
    } else {
        BOTTOM_LETTERS_EN
    };

    let mut row_top = vec![KeyCell::named("Esc", KeyAction::CloseVk, SPAN_ACTION)];
    row_top.extend(top.iter().zip(TOP_SYMBOLS).map(|(&l, s)| t(l, s)));
    row_top.push(KeyCell::vk("Backspace", VK_BACK, SPAN_ACTION));

    let mut row_mid = vec![KeyCell::vk("Tab", VK_TAB, SPAN_ACTION)];
    row_mid.extend(MID_LETTERS.iter().zip(MID_SYMBOLS).map(|(&l, s)| t(l, s)));
    row_mid.push(KeyCell::tri('\'', '"', '/', layer));
    row_mid.push(KeyCell::vk("Enter", VK_RETURN, SPAN_ACTION));

    let mut row_bottom = vec![KeyCell::named("Shift", KeyAction::Shift, SPAN_ACTION)];
    row_bottom.extend(bottom.iter().zip(BOTTOM_SYMBOLS).map(|(&l, s)| t(l, s)));
    row_bottom.push(KeyCell::tri(';', ':', '[', layer));
    row_bottom.push(KeyCell::tri('.', '!', ']', layer));
    // No dedicated close key: L3 toggles the keyboard open/closed, and the Esc
    // key in the top row covers on-grid dismissal.
    row_bottom.push(KeyCell::tri('?', '/', '\\', layer));

    let mut space = KeyCell::vk("Space", VK_SPACE, SPAN_SPACE);
    // Language badge on the space bar (web shows ENG/DE next to the L3 hint).
    space.sublabel = Some(if lang_de { "DE" } else { "ENG" }.to_string());
    let mut row_utility = vec![
        KeyCell::named("&123", KeyAction::Symbols, SPAN_ACTION),
        KeyCell::ch(QUICK_INSERTS[0]),
        KeyCell::ch(QUICK_INSERTS[1]),
        KeyCell::ch(QUICK_INSERTS[2]),
        space,
    ];
    // Mic key only when offline dictation is installed (whisper sidecar + model);
    // otherwise it's hidden, so an install that skipped speech shows no dead key.
    if crate::win::speech_input::available() {
        row_utility.push(KeyCell::named("Mic", KeyAction::VoiceInput, SPAN_ACTION));
    }
    row_utility.extend([
        KeyCell::ch(QUICK_INSERTS[3]),
        KeyCell::named("<", KeyAction::PredictPrev, SPAN_ACTION),
        KeyCell::named(">", KeyAction::PredictNext, SPAN_ACTION),
    ]);

    vec![
        KeyRow { keys: row_top },
        KeyRow { keys: row_mid },
        KeyRow { keys: row_bottom },
        KeyRow { keys: row_utility },
    ]
}

fn rebuild(nav: &mut NavState) {
    nav.rows = build_web_layout(nav.layer, nav.lang_de);
    clamp_pos(nav);
}

fn clamp_pos(nav: &mut NavState) {
    if nav.rows.is_empty() {
        nav.pos = KeyPos::default();
        return;
    }
    if nav.pos.row >= nav.rows.len() {
        nav.pos.row = nav.rows.len() - 1;
    }
    let cols = nav.rows[nav.pos.row].keys.len();
    if cols == 0 {
        nav.pos.col = 0;
    } else if nav.pos.col >= cols {
        nav.pos.col = cols - 1;
    }
}

/// Reset focus when the keyboard opens. Web parity: text fields open on the
/// upper layer (`resolveInitialVirtualKeyboardLayer`) with focus on the `a` key
/// (`getInitialVirtualKeyboardSelection` -> rows[1].keys[1]).
pub fn reset_selection() {
    if let Ok(mut nav) = NAV.lock() {
        nav.layer = Layer::Upper;
        // One-shot, not sticky: the opening uppercase layer reverts after the
        // first character (sentence case), like a fresh shift tap.
        nav.one_shot_shift = true;
        nav.one_shot_symbol = false;
        nav.last_shift_at = None;
        nav.last_symbol_at = None;
        nav.pos = KeyPos { row: 1, col: 1 };
        #[cfg(feature = "gamepad")]
        {
            nav.hold_button = None;
        }
        nav.hold_count = 0;
        nav.hold_deadline = None;
        nav.repeat_key = None;
        nav.repeat_deadline = None;
        rebuild(&mut nav);
    }
    crate::vk_predict::reset();
}

pub fn selection() -> KeyPos {
    NAV.lock().map(|n| n.pos).unwrap_or_default()
}

pub fn rows_snapshot() -> Vec<KeyRow> {
    NAV.lock().map(|n| n.rows.clone()).unwrap_or_default()
}

pub fn selected_key() -> Option<KeyCell> {
    let nav = NAV.lock().ok()?;
    nav.rows.get(nav.pos.row)?.keys.get(nav.pos.col).cloned()
}

pub fn voice_input_active() -> bool {
    NAV.lock().map(|n| n.voice_input).unwrap_or(false)
}

pub fn set_voice_input_active(active: bool) {
    if let Ok(mut nav) = NAV.lock() {
        nav.voice_input = active;
    }
    request_ui_repaint();
}

/// `(shift, caps)` for the renderer: shift = upper layer active; caps = upper
/// layer promoted to sticky (web `shiftEnabled && !oneShotShift`).
pub fn modifier_state() -> (bool, bool) {
    NAV.lock()
        .map(|n| {
            let up = n.layer == Layer::Upper;
            (up, up && !n.one_shot_shift)
        })
        .unwrap_or_default()
}

/// `(start, end)` of a key inside its row, normalized to `0.0..=1.0` of the row's
/// total span. The renderer flex-scales every row to the same pixel width, so rows
/// with different span totals only line up in normalized units, not raw key-units.
fn span_bounds(row: &KeyRow, col: usize) -> (f32, f32) {
    let total: f32 = row.keys.iter().map(|k| k.span).sum();
    let total = total.max(f32::EPSILON);
    let start: f32 = row.keys[..col].iter().map(|k| k.span).sum();
    (start / total, (start + row.keys[col].span) / total)
}

/// Nearest key in `target_row` by the web's scoring (`chooseNearestNode`):
/// `|center distance| - overlap * 0.65` over normalized row-relative units.
fn nearest_col(rows: &[KeyRow], from: KeyPos, target_row: usize) -> usize {
    let (cur_start, cur_end) = span_bounds(&rows[from.row], from.col);
    let cur_center = (cur_start + cur_end) / 2.0;
    let mut best = 0usize;
    let mut best_score = f32::INFINITY;
    for col in 0..rows[target_row].keys.len() {
        let (start, end) = span_bounds(&rows[target_row], col);
        let overlap = (cur_end.min(end) - cur_start.max(start)).max(0.0);
        let score = ((start + end) / 2.0 - cur_center).abs() - overlap * 0.65;
        if score < best_score {
            best_score = score;
            best = col;
        }
    }
    best
}

#[cfg(feature = "gamepad")]
pub fn move_selection(dir: Button) -> bool {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return false,
    };
    if nav.rows.is_empty() {
        return false;
    }
    let mut pos = nav.pos;
    // Web `moveVirtualKeyboardSelection`: LEFT/RIGHT stop at row edges (no
    // wrap, no cross-row snake); UP/DOWN pick the nearest key by span center.
    let changed = match dir {
        Button::Left if pos.col > 0 => {
            pos.col -= 1;
            true
        }
        Button::Right if pos.col + 1 < nav.rows[pos.row].keys.len() => {
            pos.col += 1;
            true
        }
        Button::Up if pos.row > 0 => {
            pos.col = nearest_col(&nav.rows, pos, pos.row - 1);
            pos.row -= 1;
            true
        }
        Button::Down if pos.row + 1 < nav.rows.len() => {
            pos.col = nearest_col(&nav.rows, pos, pos.row + 1);
            pos.row += 1;
            true
        }
        _ => false,
    };
    if changed {
        nav.pos = pos;
    }
    changed
}

#[cfg(feature = "gamepad")]
pub fn tick_dpad_hold(now: Instant) -> bool {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let Some(btn) = nav.hold_button else {
        return false;
    };
    let Some(deadline) = nav.hold_deadline else {
        return false;
    };
    if now < deadline {
        return false;
    }
    nav.hold_count += 1;
    nav.hold_deadline = Some(now + HOLD_REPEAT);
    drop(nav);
    let moved = move_selection(btn);
    refocus_after_nav_move();
    moved
}

/// Returns whether the press actually moved the selection (false at a grid
/// edge), so the caller can fire a haptic tick only on a real move.
#[cfg(feature = "gamepad")]
pub fn dpad_pressed(dir: Button) -> bool {
    let mut nav = match NAV.lock() {
        Ok(n) => n,
        Err(_) => return false,
    };
    nav.hold_button = Some(dir);
    nav.hold_count = 0;
    nav.hold_deadline = Some(Instant::now() + HOLD_INITIAL);
    drop(nav);
    let moved = move_selection(dir);
    refocus_after_nav_move();
    moved
}

/// Only character and virtual-key keys auto-repeat; toggles (Shift, &123,
/// language, voice, close) and chip cycling fire once per press.
fn key_repeats(key: &KeyCell) -> bool {
    matches!(key.action, KeyAction::Char(_) | KeyAction::Vk(_))
}

/// Arm auto-repeat for a held button. For `Activate`, only when the focused
/// key produces input. Call after the first action already fired.
pub fn repeat_pressed(kind: RepeatKey) {
    if kind == RepeatKey::Activate && !selected_key().as_ref().is_some_and(key_repeats) {
        return;
    }
    if let Ok(mut nav) = NAV.lock() {
        nav.repeat_key = Some(kind);
        nav.repeat_deadline = Some(Instant::now() + HOLD_INITIAL);
    }
}

pub fn repeat_released(kind: RepeatKey) {
    if let Ok(mut nav) = NAV.lock() {
        if nav.repeat_key == Some(kind) {
            nav.repeat_key = None;
            nav.repeat_deadline = None;
        }
    }
}

/// Fire the held button's action when its repeat deadline passes. Driven from
/// the gamepad poll alongside [`tick_dpad_hold`].
pub fn tick_key_repeat(now: Instant) -> bool {
    let kind = {
        let Ok(mut nav) = NAV.lock() else {
            return false;
        };
        let Some(kind) = nav.repeat_key else {
            return false;
        };
        let Some(deadline) = nav.repeat_deadline else {
            return false;
        };
        if now < deadline {
            return false;
        }
        nav.repeat_deadline = Some(now + HOLD_REPEAT);
        kind
    };
    match kind {
        RepeatKey::Activate => {
            // Selection may have moved mid-hold; stop if the key under focus
            // no longer repeats (e.g. d-pad onto Shift while holding A).
            if let Some(key) = selected_key().filter(key_repeats) {
                activate_key(&key);
            } else {
                repeat_released(RepeatKey::Activate);
                return false;
            }
        }
        RepeatKey::Backspace => backspace(),
        RepeatKey::Space => space(),
    }
    true
}

#[cfg(feature = "gamepad")]
pub fn dpad_released(dir: Button) {
    let Ok(mut nav) = NAV.lock() else {
        return;
    };
    if nav.hold_button == Some(dir) {
        nav.hold_button = None;
        nav.hold_count = 0;
        nav.hold_deadline = None;
    }
}

pub fn activate_selection() {
    if let Some(key) = selected_key() {
        activate_key(&key);
    }
}

/// Web `toggleShift`: off -> one-shot upper; double-tap inside 400ms -> sticky
/// caps; any further tap -> lower.
pub fn toggle_shift() {
    if let Ok(mut nav) = NAV.lock() {
        let now = Instant::now();
        if nav.layer != Layer::Upper {
            nav.layer = Layer::Upper;
            nav.one_shot_shift = true;
            nav.one_shot_symbol = false;
        } else if nav.one_shot_shift
            && nav
                .last_shift_at
                .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_STICKY)
        {
            nav.one_shot_shift = false;
        } else {
            nav.layer = Layer::Lower;
            nav.one_shot_shift = false;
        }
        nav.last_shift_at = Some(now);
        rebuild(&mut nav);
    }
    request_ui_repaint();
}

/// Web `toggleSymbols`: same one-shot/double-tap-sticky cycle for the symbol layer.
pub fn toggle_symbols() {
    if let Ok(mut nav) = NAV.lock() {
        let now = Instant::now();
        if nav.layer != Layer::Symbol {
            nav.layer = Layer::Symbol;
            nav.one_shot_symbol = true;
            nav.one_shot_shift = false;
        } else if nav.one_shot_symbol
            && nav
                .last_symbol_at
                .is_some_and(|t| now.duration_since(t) < DOUBLE_TAP_STICKY)
        {
            nav.one_shot_symbol = false;
        } else {
            nav.layer = Layer::Lower;
            nav.one_shot_symbol = false;
        }
        nav.last_symbol_at = Some(now);
        rebuild(&mut nav);
    }
    request_ui_repaint();
}

/// Web L3: flip between QWERTY (en-US) and QWERTZ (de-DE) letter rows.
pub fn toggle_language() {
    let mut de = false;
    if let Ok(mut nav) = NAV.lock() {
        nav.lang_de = !nav.lang_de;
        de = nav.lang_de;
        rebuild(&mut nav);
    }
    // Drive whisper recognition with the VK language (live; helper reads it per utterance).
    crate::win::speech_input::set_vk_language(de);
    request_ui_repaint();
}

/// Layer reset after an insert (web `insertText`): one-shot layers revert to
/// lower after one character or space; double-tap-promoted sticky layers
/// (caps / sticky symbols) persist until toggled off.
fn after_insert() {
    if let Ok(mut nav) = NAV.lock() {
        let reset = match nav.layer {
            Layer::Upper => nav.one_shot_shift,
            Layer::Symbol => nav.one_shot_symbol,
            Layer::Lower => false,
        };
        if reset {
            nav.layer = Layer::Lower;
            nav.one_shot_shift = false;
            nav.one_shot_symbol = false;
            rebuild(&mut nav);
        }
    }
    request_ui_repaint();
}

pub fn activate_key(key: &KeyCell) {
    match &key.action {
        KeyAction::Char(c) => {
            send_unicode(&[*c as u16]);
            crate::vk_predict::on_char(*c);
            after_insert();
        }
        KeyAction::Vk(vk) => {
            notify_vk_key(*vk);
            inject_vk(*vk);
            if *vk == VK_SPACE {
                after_insert();
            }
        }
        KeyAction::Shift => toggle_shift(),
        KeyAction::Symbols => toggle_symbols(),
        KeyAction::PredictPrev => {
            let _ = crate::vk_predict::cycle_prev();
            request_ui_repaint();
        }
        KeyAction::PredictNext => {
            let _ = crate::vk_predict::cycle_next();
            request_ui_repaint();
        }
        KeyAction::VoiceInput => start_voice_input(),
        KeyAction::CloseVk => crate::win::vk_ui::request_hide(),
    }
}

pub fn send_text_direct(text: &str) {
    // Log the real inject target up front, to the user-writable log, so we can see
    // exactly where a transcript went — including when the guard below skips it.
    let (hwnd, exe, class, title) = foreground_info();
    let warmup_browser_foreground = foreground_is_warmup_browser_info(&exe, &title);
    let skip_warmup = exe.eq_ignore_ascii_case("warmup.exe") && !warmup_browser_foreground;
    diag_inject_log(&format!(
        "send_text_direct: chars={} fg=0x{hwnd:x} exe='{exe}' class='{class}' title='{title}' browser_active={} browser_fg={warmup_browser_foreground} skip_warmup={skip_warmup}",
        text.chars().count(),
        crate::pipe_server::browser_active()
    ));
    // Never dump a dictated transcript into the warmUP launcher's own UI. Browser windows
    // are allowed because they are intentionally acting as desktop/browser text targets.
    if skip_warmup {
        crate::install::log_line("voice inject skipped: warmUP launcher is foreground");
        return;
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    send_unicode(&units);
}

/// `(hwnd_as_usize, exe_basename, window_class, window_title)` of the current foreground
/// window. Used both to keep dictation out of warmUP and to log the real inject
/// target — the speech helper injects from its own process, so this is the only
/// way to see where a transcript actually went.
fn foreground_info() -> (usize, String, String, String) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId,
    };
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return (0, String::new(), String::new(), String::new());
        }
        let hwnd = fg.0 as usize;
        let mut cbuf = [0u16; 128];
        let n = GetClassNameW(fg, &mut cbuf);
        let class = String::from_utf16_lossy(&cbuf[..n.max(0) as usize]);
        let title_len = GetWindowTextLengthW(fg);
        let title = if title_len > 0 {
            let mut tbuf = vec![0u16; title_len as usize + 1];
            let n = GetWindowTextW(fg, &mut tbuf);
            String::from_utf16_lossy(&tbuf[..n.max(0) as usize])
        } else {
            String::new()
        };
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let mut exe = String::new();
        if pid != 0 {
            if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut buf = [0u16; 1024];
                let mut len = buf.len() as u32;
                let ok = QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_FORMAT(0),
                    windows::core::PWSTR(buf.as_mut_ptr()),
                    &mut len,
                )
                .is_ok();
                let _ = CloseHandle(process);
                if ok && len > 0 {
                    exe = std::path::Path::new(&String::from_utf16_lossy(&buf[..len as usize]))
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                }
            }
        }
        (hwnd, exe, class, title)
    }
}

/// True if the OS foreground window belongs to the warmUP desktop app.
fn foreground_is_warmup_browser_info(exe: &str, title: &str) -> bool {
    crate::pipe_server::browser_active()
        && exe.eq_ignore_ascii_case("warmup.exe")
        && (title.eq_ignore_ascii_case("warmUP Browser")
            || title.eq_ignore_ascii_case("warmUP Browser Overlay"))
}

pub(crate) fn foreground_is_warmup_browser() -> bool {
    let (_, exe, _, title) = foreground_info();
    foreground_is_warmup_browser_info(&exe, &title)
}

/// Append a line to a user-session-writable inject log. `service.log` is
/// ACL-locked to SYSTEM, so an inject done from the user-session speech helper
/// leaves no trail there — this mirror makes it visible.
fn diag_inject_log(msg: &str) {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let dir = std::path::Path::new(&base).join("WarmupVk");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("inject.log");
    if std::fs::metadata(&path)
        .map(|m| m.len() > 512_000)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{msg}");
    }
}

/// Real Text-commit adapter (CONTEXT.md "Text commit"): one batched `SendInput`
/// of `del` backspaces followed by the word's Unicode events. Commit is
/// userland-only, so the Winlogon focus-collapse lead is a no-op here.
pub struct SendInputSink;

impl crate::vk_commit::TextSink for SendInputSink {
    fn replace(&mut self, del: usize, ins: &str) -> std::io::Result<()> {
        let collapse = focus_for_inject();
        let units: Vec<u16> = ins.encode_utf16().collect();
        let mut batch: Vec<INPUT> = Vec::with_capacity(2 + del * 2 + units.len() * 2);
        push_collapse(&mut batch, collapse);
        for _ in 0..del {
            batch.push(vk_event(VK_BACK, false));
            batch.push(vk_event(VK_BACK, true));
        }
        for unit in units {
            batch.push(unicode_event(unit, false));
            batch.push(unicode_event(unit, true));
        }
        let sent = unsafe { SendInput(&batch, std::mem::size_of::<INPUT>() as i32) };
        suppress_native_keyboard_after_winlogon_inject(collapse);
        if sent as usize == batch.len() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "SendInput inserted {sent}/{} events",
                batch.len()
            )))
        }
    }
}

fn notify_vk_key(vk: VIRTUAL_KEY) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_SPACE};
    if vk == VK_BACK {
        crate::vk_predict::on_backspace();
    } else if vk == VK_SPACE {
        crate::vk_predict::on_space();
    } else {
        // Return and every other committed key act as a word boundary.
        crate::vk_predict::on_boundary();
    }
}

fn request_ui_repaint() {
    crate::win::vk_ui::request_repaint();
}

pub fn backspace() {
    crate::vk_predict::on_backspace();
    inject_vk(VK_BACK);
}

pub fn space() {
    crate::vk_predict::on_space();
    inject_vk(VK_SPACE);
    after_insert();
}

pub fn enter() {
    crate::vk_predict::on_boundary();
    inject_vk(VK_RETURN);
}

pub fn start_voice_input() {
    if crate::win::logon_focus::is_active() {
        crate::install::log_line("vk voice input ignored on Winlogon");
        return;
    }

    // Toggle off only if a helper is actually running — it transcribes the whole
    // monologue and injects it, then exits. After an auto-stop the helper has already
    // exited, so a stale "active" flag falls through to a fresh start.
    if voice_input_active() && crate::win::speech_input::helper_alive() {
        crate::install::log_line("vk voice input: OFF (finishing — transcribe on stop)");
        crate::win::speech_input::request_stop();
        set_voice_input_active(false);
        return;
    }

    // Don't start dictation when warmUP's launcher is focused — the transcript would
    // land in its UI. Browser windows are different: they are warmUP-owned OS windows
    // but behave like normal desktop/browser text entry, so allow R3 there only when
    // the explicit browser mode is set AND the foreground warmUP window is the browser.
    let (_, exe, _, title) = foreground_info();
    if exe.eq_ignore_ascii_case("warmup.exe") && !foreground_is_warmup_browser_info(&exe, &title) {
        crate::install::log_line("vk voice input ignored: warmUP launcher is foreground");
        return;
    }

    // Turn on: the worker runs as SYSTEM with no mic consent, so recognition runs
    // in a helper process launched as the real logged-in user (see speech_input).
    crate::install::log_line("vk voice input: ON (spawning helper)");
    set_voice_input_active(true);
    // Pin the current VK language for the helper before it starts.
    let de = NAV.lock().map(|n| n.lang_de).unwrap_or(false);
    crate::win::speech_input::set_vk_language(de);
    if let Err(e) = crate::win::speech_input::start_helper() {
        set_voice_input_active(false);
        crate::install::log_line(&format!("speech helper start failed: {e}"));
    }
}

/// Focus the credential field for an inject. Returns true on Winlogon, where the
/// edit selects its entire contents on focus — the caller must then lead its
/// `SendInput` batch with a caret-to-end (`VK_END`) so the collapse and the key
/// land in the *same* injection. Two separate sends let the target re-select
/// (or re-process focus) between them, so every key after the first overwrites
/// the selection. False off Winlogon, where normal caret rules apply.
fn focus_for_inject() -> bool {
    crate::win::logon_focus::focus_password_field()
}

#[cfg(feature = "gamepad")]
fn refocus_after_nav_move() {
    let _ = crate::win::logon_focus::focus_password_field();
}

/// Build a virtual-key down (or up) `INPUT`.
fn vk_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Build a Unicode-scancode down (or up) `INPUT`.
fn unicode_event(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Push a caret-to-end (`VK_END`) down+up onto a Winlogon inject batch so the
/// select-on-focus selection collapses in the same injection as the key. No-op
/// off Winlogon.
fn push_collapse(batch: &mut Vec<INPUT>, on_winlogon: bool) {
    if on_winlogon {
        batch.push(vk_event(VK_END, false));
        batch.push(vk_event(VK_END, true));
    }
}

fn inject_vk(vk: VIRTUAL_KEY) {
    let collapse = focus_for_inject();
    let mut batch: Vec<INPUT> = Vec::with_capacity(4);
    push_collapse(&mut batch, collapse);
    batch.push(vk_event(vk, false));
    batch.push(vk_event(vk, true));
    unsafe {
        let _ = SendInput(&batch, std::mem::size_of::<INPUT>() as i32);
    }
    suppress_native_keyboard_after_winlogon_inject(collapse);
}

/// Chunk size + gap for paced injection. Console hosts (conhost / Windows
/// Terminal) silently drop a single large burst of synthetic KEYEVENTF_UNICODE
/// events — they drain the input queue slower than GUI apps, so a whole dictated
/// sentence sent at once never lands in a terminal (a single VK keypress does:
/// it's one event, paced by the human). Calibration knob: if a terminal still
/// drops dictated text, shrink INJECT_CHUNK or raise INJECT_GAP.
const INJECT_CHUNK: usize = 8;
const INJECT_GAP: Duration = Duration::from_millis(6);

fn send_unicode(units: &[u16]) {
    let collapse = focus_for_inject();
    // One char (a VK keypress) is a single chunk → one SendInput, no added
    // latency. Long injects (voice dictation) pace out so terminals keep up.
    let groups: Vec<&[u16]> = units.chunks(INJECT_CHUNK.max(1)).collect();
    let mut sent = 0i32;
    for (i, group) in groups.iter().enumerate() {
        let mut batch: Vec<INPUT> = Vec::with_capacity(group.len() * 2 + 2);
        if i == 0 {
            push_collapse(&mut batch, collapse);
        }
        for &unit in *group {
            batch.push(unicode_event(unit, false));
            batch.push(unicode_event(unit, true));
        }
        sent += unsafe { SendInput(&batch, std::mem::size_of::<INPUT>() as i32) } as i32;
        if i + 1 < groups.len() {
            std::thread::sleep(INJECT_GAP);
        }
    }
    suppress_native_keyboard_after_winlogon_inject(collapse);
    // Userland-typing diagnostic: when off Winlogon, SendInput should land in the
    // foreground app. Log the event count actually inserted + the loop thread's
    // desktop + foreground window so a misrouted inject (wrong desktop /
    // not-foreground / blocked) is visible in the log.
    #[cfg(feature = "gamepad")]
    if !crate::win::logon_focus::is_active() {
        let (hwnd, exe, class, title) = foreground_info();
        let desk = crate::win::current_desktop_name().unwrap_or_default();
        let line = format!(
            "vk inject(userland): units={} SendInput->{sent} desktop={desk} fg=0x{hwnd:x} exe='{exe}' class='{class}' title='{title}'",
            units.len()
        );
        // service.log (gamepad helper can write it) + a user-writable mirror (the
        // speech helper can't write service.log, so its injects only show here).
        crate::install::log_line(&line);
        diag_inject_log(&line);
    }
}

fn suppress_native_keyboard_after_winlogon_inject(on_winlogon: bool) {
    if on_winlogon {
        crate::win::native_keyboard::suppress_for(Duration::from_millis(300));
    }
}

#[allow(dead_code)]
fn active_langid() -> u32 {
    let hkl = unsafe { GetKeyboardLayout(0) };
    (hkl.0 as usize as u32) & 0xffff
}

#[cfg(all(test, feature = "gamepad"))]
mod tests {
    use super::*;
    use crate::gamepad_backend::Button;

    #[test]
    fn move_reports_real_moves_not_edges() {
        // The typing-loop haptic ticks only when the cursor actually moves. A
        // press into a row edge must report `false` so it stays silent — a
        // phantom buzz at every wall would be worse than no haptics.
        //
        // Seed NAV directly (not via reset_selection) so this test never touches
        // the shared vk_predict global that the predict tests run against.
        {
            let mut nav = NAV.lock().unwrap();
            nav.layer = Layer::Upper;
            nav.pos = KeyPos { row: 1, col: 1 }; // 'a' in the middle row
            rebuild(&mut nav);
        }
        assert!(
            move_selection(Button::Left),
            "interior move should report moved"
        );
        assert!(
            !move_selection(Button::Left),
            "at the left edge there is no move"
        );
    }
}

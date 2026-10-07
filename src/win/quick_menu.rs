use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_D, VK_OEM_PLUS, VK_R, VK_TAB};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetCursorPos, GetForegroundWindow, GetWindowRect, IsWindowVisible,
};

use super::desktop_window::{self, DesktopApp, DesktopWindowThread};
use super::mono_ui::Icon;
use super::quick_menu_sys as sys;
use super::tray_menu::{self, lucide, Action, Entry, Placement, Reply, Spec};
use crate::gamepad_backend::{Button, ButtonChange};

pub(crate) const GUIDE_HOLD: Duration = Duration::from_millis(500);
pub(crate) const ROOT_W: f32 = 360.0;
pub(crate) const SUB_W: f32 = 220.0;
const TV_ZOOM: f32 = 1.5;
const REFRESH_MS: u32 = 1000;

const CMD_MEDIA: usize = 1;
const CMD_VOLUME: usize = 2;
const CMD_BRIGHTNESS: usize = 3;
const CMD_OUTPUT_SETTINGS: usize = 4;
const CMD_SCREENSHOT: usize = 10;
const CMD_RECORD: usize = 11;
const CMD_SWITCHER: usize = 12;
const CMD_DESKTOP: usize = 13;
const CMD_CLOSE_WINDOW: usize = 14;
const CMD_KEYBOARD: usize = 15;
const CMD_DICTATE: usize = 16;
const CMD_MAGNIFIER: usize = 17;
const CMD_CENTER: usize = 18;
const CMD_LOCK: usize = 19;
const CMD_SLEEP: usize = 20;
const CMD_RESTART: usize = 21;
const CMD_SHUTDOWN: usize = 22;
const CMD_SIGN_OUT: usize = 23;
const CMD_CONFIRM: usize = 24;
const CMD_CANCEL_CONFIRM: usize = 25;
const CMD_OUTPUT_BASE: usize = 100;

const VOLUME: &str = lucide!(
    r#"<polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>"#
);
const VOLUME_X: &str = lucide!(
    r#"<polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><line x1="22" x2="16" y1="9" y2="15"/><line x1="16" x2="22" y1="9" y2="15"/>"#
);
const SUN: &str = lucide!(
    r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#
);
const HEADPHONES: &str = lucide!(
    r#"<path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3"/>"#
);
const CAMERA: &str = lucide!(
    r#"<path d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z"/><circle cx="12" cy="13" r="3"/>"#
);
const CIRCLE_DOT: &str =
    lucide!(r#"<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="1"/>"#);
const LAYOUT_GRID: &str = lucide!(
    r#"<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>"#
);
const MONITOR: &str = lucide!(
    r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>"#
);
const SQUARE_X: &str = lucide!(
    r#"<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>"#
);
const KEYBOARD: &str = lucide!(
    r#"<path d="M10 8h.01"/><path d="M12 12h.01"/><path d="M14 8h.01"/><path d="M16 12h.01"/><path d="M18 8h.01"/><path d="M6 8h.01"/><path d="M7 16h10"/><path d="M8 12h.01"/><rect width="20" height="16" x="2" y="4" rx="2"/>"#
);
const MIC: &str = lucide!(
    r#"<path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" x2="12" y1="19" y2="22"/>"#
);
const ZOOM_IN: &str = lucide!(
    r#"<circle cx="11" cy="11" r="8"/><line x1="21" x2="16.65" y1="21" y2="16.65"/><line x1="11" x2="11" y1="8" y2="14"/><line x1="8" x2="14" y1="11" y2="11"/>"#
);
const LOCK: &str = lucide!(
    r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#
);
const POWER: &str = lucide!(r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#);
const MOON: &str = lucide!(r#"<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>"#);
const ROTATE_CW: &str = lucide!(
    r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#
);
const LOG_OUT: &str = lucide!(
    r#"<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" x2="9" y1="12" y2="12"/>"#
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuideHold {
    since: Option<Instant>,
    fired: bool,
}

impl GuideHold {
    pub(crate) fn rewrite(
        &mut self,
        changes: Vec<ButtonChange>,
        allowed: bool,
        now: Instant,
    ) -> (Vec<ButtonChange>, bool) {
        if !allowed {
            *self = Self::default();
            return (changes, false);
        }
        let mut out = Vec::with_capacity(changes.len() + 1);
        for c in changes {
            if c.button != Button::Guide {
                out.push(c);
                continue;
            }
            if c.pressed {
                self.since = Some(now);
                self.fired = false;
                continue;
            }
            let held = self.since.take().is_some();
            if !std::mem::take(&mut self.fired) {
                if held {
                    out.push(ButtonChange {
                        button: Button::Guide,
                        pressed: true,
                    });
                }
                out.push(c);
            }
        }
        let open = match self.since {
            Some(since) if !self.fired && now.duration_since(since) >= GUIDE_HOLD => {
                self.fired = true;
                true
            }
            _ => false,
        };
        (out, open)
    }
}

static THREAD: OnceLock<Mutex<Option<DesktopWindowThread>>> = OnceLock::new();

pub(crate) fn show() {
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if guard.is_none() {
        match desktop_window::spawn(QuickApp) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("quick menu: spawn failed: {e}"));
                return;
            }
        }
    }
    if let Some(t) = guard.as_ref() {
        super::controller_center::set_menu_owns_pad(true);
        if t.show(LPARAM(0)).is_err() {
            super::controller_center::set_menu_owns_pad(false);
        }
    }
}

struct QuickApp;

impl DesktopApp for QuickApp {
    const THREAD_NAME: &'static str = "warmup-quick-menu";
    const CLASS_NAME: PCWSTR = w!("WarmupQuickMenuHost");
    const BG_COLOR: u32 = 0;
    const WNDPROC: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = host_proc;

    fn on_thread_start(&mut self) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        Ok(())
    }

    fn on_show(&mut self, _lparam: LPARAM) {
        open_menu();
    }

    fn on_hide(&mut self) {
        tray_menu::close();
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

struct QState {
    prev: HWND,
    confirm: Option<usize>,
    brightness: Option<(f32, sys::BrightnessSource)>,
    playstation: bool,
}

fn pad_is_playstation() -> bool {
    let s = crate::debug_state::snapshot();
    !s.connected || s.name.is_empty() || super::vk_renderer::is_playstation_label(&s.name)
}

fn local_time() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

pub(crate) fn subtitle(time: &str, playstation: bool) -> String {
    format!(
        "{time} \u{00B7} Hold {} to open",
        if playstation { "PS" } else { "Guide" }
    )
}

pub(crate) fn accels(playstation: bool) -> [&'static str; 4] {
    if playstation {
        ["L1 + R1", "L3", "R3", "Create + Options"]
    } else {
        ["LB + RB", "LS", "RS", "View + Menu"]
    }
}

pub(crate) fn volume_text(level: f32, muted: bool) -> String {
    if muted {
        "Muted".to_string()
    } else {
        format!("{}", (level * 100.0).round() as i32)
    }
}

fn media_entry(now: Option<sys::NowPlaying>) -> Entry {
    match now {
        Some(np) => {
            let subtitle = match (np.artist.is_empty(), np.app.is_empty()) {
                (false, false) => format!("{} \u{00B7} {}", np.artist, np.app),
                (false, true) => np.artist,
                (true, false) => np.app,
                (true, true) => String::new(),
            };
            Entry::Media {
                title: np.title,
                subtitle,
                playing: np.playing,
                cmd: CMD_MEDIA,
            }
        }
        None => Entry::Media {
            title: "Nothing playing".to_string(),
            subtitle: "Controls send media keys".to_string(),
            playing: false,
            cmd: CMD_MEDIA,
        },
    }
}

fn power_entries(confirm: Option<usize>) -> Vec<Entry> {
    match confirm {
        Some(cmd) => {
            let (label, icon) = if cmd == CMD_RESTART {
                ("Restart now", ROTATE_CW)
            } else {
                ("Shut down now", POWER)
            };
            vec![
                Entry::command(label, CMD_CONFIRM)
                    .with_icon(Icon::Svg(icon))
                    .with_accel("Confirm"),
                Entry::command("Cancel", CMD_CANCEL_CONFIRM).with_icon(Icon::Close),
            ]
        }
        None => vec![
            Entry::command("Sleep", CMD_SLEEP).with_icon(Icon::Svg(MOON)),
            Entry::command("Restart", CMD_RESTART).with_icon(Icon::Svg(ROTATE_CW)),
            Entry::command("Shut down", CMD_SHUTDOWN).with_icon(Icon::Svg(POWER)),
            Entry::command("Sign out", CMD_SIGN_OUT).with_icon(Icon::Svg(LOG_OUT)),
        ],
    }
}

pub(crate) struct Snapshot {
    pub(crate) media: Option<sys::NowPlaying>,
    pub(crate) volume: Option<(f32, bool)>,
    pub(crate) brightness: Option<f32>,
    pub(crate) outputs: Vec<sys::Output>,
    pub(crate) voice: bool,
    pub(crate) playstation: bool,
    pub(crate) confirm: Option<usize>,
}

pub(crate) fn entries(s: &Snapshot) -> Vec<Entry> {
    let [shot, kb, dictate, center] = accels(s.playstation);
    let mut root = vec![
        Entry::Header,
        Entry::Separator,
        media_entry(s.media.clone()),
    ];
    if let Some((level, muted)) = s.volume {
        root.push(Entry::Slider {
            label: "Volume".to_string(),
            icon: Icon::Svg(if muted { VOLUME_X } else { VOLUME }),
            value: if muted { 0.0 } else { level },
            text: volume_text(level, muted),
            cmd: CMD_VOLUME,
        });
    }
    if let Some(level) = s.brightness {
        root.push(Entry::Slider {
            label: "Brightness".to_string(),
            icon: Icon::Svg(SUN),
            value: level,
            text: format!("{}", (level * 100.0).round() as i32),
            cmd: CMD_BRIGHTNESS,
        });
    }
    let output = if s.outputs.is_empty() {
        Entry::command("Output", CMD_OUTPUT_SETTINGS)
    } else {
        let current = s
            .outputs
            .iter()
            .find(|o| o.current)
            .map(|o| sys::short_output_name(&o.name));
        let sub = s
            .outputs
            .iter()
            .enumerate()
            .map(|(i, o)| Entry::toggle(o.name.clone(), CMD_OUTPUT_BASE + i, o.current))
            .collect();
        let e = Entry::submenu("Output", sub);
        match current {
            Some(name) => e.with_accel(name),
            None => e,
        }
    };
    root.push(output.with_icon(Icon::Svg(HEADPHONES)));
    root.extend([
        Entry::Separator,
        Entry::command("Screenshot", CMD_SCREENSHOT)
            .with_icon(Icon::Svg(CAMERA))
            .with_accel(shot),
        Entry::command("Record clip", CMD_RECORD).with_icon(Icon::Svg(CIRCLE_DOT)),
        Entry::command("App switcher", CMD_SWITCHER).with_icon(Icon::Svg(LAYOUT_GRID)),
        Entry::command("Show desktop", CMD_DESKTOP).with_icon(Icon::Svg(MONITOR)),
        Entry::command("Close window", CMD_CLOSE_WINDOW).with_icon(Icon::Svg(SQUARE_X)),
        Entry::Separator,
        Entry::command("Keyboard", CMD_KEYBOARD)
            .with_icon(Icon::Svg(KEYBOARD))
            .with_accel(kb),
        Entry::command("Dictate", CMD_DICTATE)
            .with_icon(Icon::Svg(MIC))
            .with_accel(dictate)
            .disabled(!s.voice),
        Entry::command("Magnifier", CMD_MAGNIFIER).with_icon(Icon::Svg(ZOOM_IN)),
        Entry::command("Controller Center", CMD_CENTER)
            .with_icon(Icon::Gamepad)
            .with_accel(center),
        Entry::Separator,
        Entry::command("Lock", CMD_LOCK).with_icon(Icon::Svg(LOCK)),
        Entry::submenu("Power", power_entries(s.confirm)).with_icon(Icon::Svg(POWER)),
    ]);
    root
}

fn snapshot(st: &QState) -> Snapshot {
    Snapshot {
        media: sys::now_playing(),
        volume: sys::volume(),
        brightness: st.brightness.map(|(v, _)| v),
        outputs: sys::outputs(),
        voice: crate::config::voice_enabled(),
        playstation: st.playstation,
        confirm: st.confirm,
    }
}

fn after(f: impl FnOnce() + Send + 'static) -> Reply {
    Reply::Close(Some(Box::new(move || {
        let _ = std::thread::Builder::new()
            .name("quick-menu-action".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                f();
            });
    })))
}

fn handle(st: &mut QState, action: Action) -> Reply {
    let rebuild = |st: &QState| Reply::Replace(entries(&snapshot(st)));
    match action {
        Action::Run(CMD_MEDIA) | Action::Media(CMD_MEDIA, 0) => {
            sys::media_command(0);
            Reply::Stay
        }
        Action::Media(_, delta) => {
            sys::media_command(delta);
            Reply::Stay
        }
        Action::Run(CMD_VOLUME) => {
            sys::toggle_mute();
            rebuild(st)
        }
        Action::Step(CMD_VOLUME, steps) => {
            if let Some((level, _)) = sys::volume() {
                sys::set_volume(sys::step_volume(level, steps));
            }
            rebuild(st)
        }
        Action::Set(CMD_VOLUME, level) => {
            sys::set_volume(level);
            rebuild(st)
        }
        Action::Step(CMD_BRIGHTNESS, steps) => {
            if let Some((level, source)) = st.brightness {
                let next = sys::step_brightness(level, steps);
                if sys::set_brightness(source, st.prev, next) {
                    st.brightness = Some((next, source));
                }
            }
            rebuild(st)
        }
        Action::Set(CMD_BRIGHTNESS, level) => {
            if let Some((_, source)) = st.brightness {
                if sys::set_brightness(source, st.prev, level) {
                    st.brightness = Some((level, source));
                }
            }
            rebuild(st)
        }
        Action::Run(CMD_OUTPUT_SETTINGS) => after(sys::open_sound_settings),
        Action::Run(cmd) if cmd >= CMD_OUTPUT_BASE => after(sys::open_sound_settings),
        Action::Run(CMD_SCREENSHOT) => after(sys::screenshot),
        Action::Run(CMD_RECORD) => after(|| sys::win_alt_chord(VK_R)),
        Action::Run(CMD_SWITCHER) => after(|| sys::win_chord(VK_TAB)),
        Action::Run(CMD_DESKTOP) => after(|| sys::win_chord(VK_D)),
        Action::Run(CMD_MAGNIFIER) => after(|| sys::win_chord(VK_OEM_PLUS)),
        Action::Run(CMD_CLOSE_WINDOW) => {
            let target = st.prev.0 as isize;
            after(move || sys::close_window(HWND(target as *mut _)))
        }
        Action::Run(CMD_KEYBOARD) => after(|| crate::pipe_server::request_native_vk(true)),
        Action::Run(CMD_DICTATE) => after(|| {
            if crate::config::voice_enabled() {
                crate::vk_nav::start_voice_input();
            }
        }),
        Action::Run(CMD_CENTER) => after(super::controller_center::show),
        Action::Run(CMD_LOCK) => after(sys::lock),
        Action::Run(CMD_SLEEP) => after(sys::sleep),
        Action::Run(CMD_SIGN_OUT) => after(sys::sign_out),
        Action::Run(cmd @ (CMD_RESTART | CMD_SHUTDOWN)) => {
            st.confirm = Some(cmd);
            rebuild(st)
        }
        Action::Run(CMD_CANCEL_CONFIRM) => {
            st.confirm = None;
            rebuild(st)
        }
        Action::Run(CMD_CONFIRM) => match st.confirm.take() {
            Some(CMD_RESTART) => after(sys::restart),
            Some(_) => after(sys::shut_down),
            None => rebuild(st),
        },
        _ => Reply::Stay,
    }
}

fn center_point(prev: HWND) -> POINT {
    unsafe {
        let mut r = RECT::default();
        if !prev.is_invalid()
            && IsWindowVisible(prev).as_bool()
            && GetWindowRect(prev, &mut r).is_ok()
            && r.right > r.left
        {
            return POINT {
                x: (r.left + r.right) / 2,
                y: (r.top + r.bottom) / 2,
            };
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        pt
    }
}

pub(crate) fn zoom_for(tv: bool) -> f32 {
    if tv {
        TV_ZOOM
    } else {
        1.0
    }
}

fn open_menu() {
    let prev = unsafe { GetForegroundWindow() };
    sys::refresh_media();
    let playstation = pad_is_playstation();
    let state = Rc::new(RefCell::new(QState {
        prev,
        confirm: None,
        brightness: sys::brightness(prev),
        playstation,
    }));
    let root = entries(&snapshot(&state.borrow()));
    let for_handler = state.clone();
    let for_refresh = state.clone();
    let spec = Spec {
        root,
        root_w: ROOT_W,
        sub_w: SUB_W,
        title: "Quick menu".to_string(),
        subtitle: subtitle(&local_time(), playstation),
        zoom: zoom_for(super::vk_ui::vk_look().tv),
        placement: Placement::Center(center_point(prev)),
        handler: Box::new(move |action| handle(&mut for_handler.borrow_mut(), action)),
        refresh: Some((
            REFRESH_MS,
            Box::new(move || {
                sys::refresh_media();
                Some(entries(&snapshot(&for_refresh.borrow())))
            }),
        )),
        on_close: None,
    };
    tray_menu::open(spec);
    if !tray_menu::is_open() {
        super::controller_center::set_menu_owns_pad(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::mono_ui::rect;
    use crate::win::tray_menu::{entry_rects, panel_height, step_selectable};

    fn change(button: Button, pressed: bool) -> ButtonChange {
        ButtonChange { button, pressed }
    }

    fn kinds(v: &[ButtonChange]) -> Vec<(Button, bool)> {
        v.iter().map(|c| (c.button, c.pressed)).collect()
    }

    #[test]
    fn short_guide_press_is_replayed_on_release_and_hold_opens_once() {
        let t0 = Instant::now();
        let mut h = GuideHold::default();
        let (out, open) = h.rewrite(vec![change(Button::Guide, true)], true, t0);
        assert!(out.is_empty() && !open);
        let (out, open) = h.rewrite(
            vec![change(Button::Guide, false)],
            true,
            t0 + Duration::from_millis(200),
        );
        assert!(!open);
        assert_eq!(
            kinds(&out),
            vec![(Button::Guide, true), (Button::Guide, false)]
        );

        let (_, open) = h.rewrite(vec![change(Button::Guide, true)], true, t0);
        assert!(!open);
        let (_, open) = h.rewrite(Vec::new(), true, t0 + Duration::from_millis(499));
        assert!(!open);
        let (_, open) = h.rewrite(Vec::new(), true, t0 + GUIDE_HOLD);
        assert!(open);
        let (_, open) = h.rewrite(Vec::new(), true, t0 + Duration::from_millis(900));
        assert!(!open);
        let (out, _) = h.rewrite(
            vec![change(Button::Guide, false), change(Button::A, true)],
            true,
            t0 + Duration::from_secs(1),
        );
        assert_eq!(kinds(&out), vec![(Button::A, true)]);

        let mut off = GuideHold::default();
        let (out, open) = off.rewrite(vec![change(Button::Guide, true)], false, t0);
        assert_eq!(kinds(&out), vec![(Button::Guide, true)]);
        assert!(!open);
        let (out, _) = h.rewrite(
            vec![change(Button::Guide, true), change(Button::Guide, false)],
            true,
            t0,
        );
        assert_eq!(
            kinds(&out),
            vec![(Button::Guide, true), (Button::Guide, false)]
        );
    }

    fn sample() -> Snapshot {
        Snapshot {
            media: Some(sys::NowPlaying {
                title: "Midnight City".into(),
                artist: "M83".into(),
                app: "Spotify".into(),
                playing: true,
            }),
            volume: Some((0.62, false)),
            brightness: Some(0.8),
            outputs: vec![
                sys::Output {
                    name: "Headphones (WH-1000XM4)".into(),
                    current: true,
                },
                sys::Output {
                    name: "Speakers".into(),
                    current: false,
                },
            ],
            voice: true,
            playstation: true,
            confirm: None,
        }
    }

    #[test]
    fn rows_match_the_design_layout() {
        let root = entries(&sample());
        assert_eq!(panel_height(&root), 668.0);
        let rows = entry_rects(&root, ROOT_W);
        let ys: Vec<f32> = rows.iter().map(|r| r.y).collect();
        assert_eq!(
            ys,
            vec![
                6.0, 58.0, 71.0, 119.0, 155.0, 191.0, 227.0, 240.0, 276.0, 312.0, 348.0, 384.0,
                420.0, 433.0, 469.0, 505.0, 541.0, 577.0, 590.0, 626.0
            ]
        );
        assert_eq!(rows[2], rect(6.0, 71.0, 348.0, 48.0));
        match &root[2] {
            Entry::Media {
                subtitle, playing, ..
            } => {
                assert_eq!(subtitle, "M83 \u{00B7} Spotify");
                assert!(*playing);
            }
            other => panic!("{other:?}"),
        }
        match &root[3] {
            Entry::Slider { text, value, .. } => {
                assert_eq!(text, "62");
                assert_eq!(*value, 0.62);
            }
            other => panic!("{other:?}"),
        }
        match &root[5] {
            Entry::Item { accel, sub, .. } => {
                assert_eq!(accel.as_deref(), Some("Headphones"));
                assert_eq!(sub.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        let power = &root[19];
        match power {
            Entry::Item { sub, .. } => assert_eq!(panel_height(sub), 156.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rows_hide_and_disable_with_missing_backends() {
        let mut s = sample();
        s.brightness = None;
        s.voice = false;
        s.media = None;
        s.volume = Some((0.4, true));
        let root = entries(&s);
        assert_eq!(root.len(), 19);
        assert!(matches!(&root[2], Entry::Media { title, .. } if title == "Nothing playing"));
        assert!(
            matches!(&root[3], Entry::Slider { text, value, .. } if text == "Muted" && *value == 0.0)
        );
        let dictate = root
            .iter()
            .position(|e| matches!(e, Entry::Item { label, .. } if label == "Dictate"))
            .unwrap();
        assert!(!root[dictate].selectable());
        assert_eq!(
            step_selectable(&root, Some(dictate - 1), true),
            Some(dictate + 1)
        );
        let confirm = entries(&Snapshot {
            confirm: Some(CMD_RESTART),
            ..sample()
        });
        match confirm.last() {
            Some(Entry::Item { sub, .. }) => {
                assert!(matches!(&sub[0], Entry::Item { label, .. } if label == "Restart now"));
                assert_eq!(sub.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn navigation_reaches_slider_rows_and_labels_follow_the_pad() {
        let root = entries(&sample());
        assert_eq!(step_selectable(&root, None, true), Some(2));
        assert_eq!(step_selectable(&root, Some(2), true), Some(3));
        assert_eq!(step_selectable(&root, Some(3), true), Some(4));
        assert_eq!(step_selectable(&root, Some(5), true), Some(7));
        assert_eq!(accels(false)[3], "View + Menu");
        assert_eq!(subtitle("21:47", true), "21:47 \u{00B7} Hold PS to open");
        assert_eq!(volume_text(0.625, false), "63");
        assert_eq!(zoom_for(true), 1.5);
    }
}

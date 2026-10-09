use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, LoadCursorW, PostMessageW,
    RegisterClassW, SetForegroundWindow, SetTimer, ShowWindow, HMENU, IDC_ARROW, SW_SHOW,
    WA_INACTIVE, WM_ACTIVATE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::mono_ui::*;

const CLASS: windows::core::PCWSTR = w!("WarmupTrayMenu");
const TIMER_ID: usize = 7;
const TIMER_MS: u32 = 33;

pub(crate) const PANEL_PAD: f32 = 6.0;
pub(crate) const ROW_H: f32 = 36.0;
pub(crate) const MEDIA_H: f32 = 48.0;
pub(crate) const HEADER_H: f32 = 52.0;
pub(crate) const SEP_H: f32 = 13.0;
pub(crate) const ROOT_W: f32 = 344.0;
pub(crate) const SUB_W: f32 = 240.0;
pub(crate) const OVERLAP: f32 = 4.0;
pub(crate) const PANEL_R: f32 = 16.0;
pub(crate) const SLIDER_TRACK_X: f32 = 130.0;
pub(crate) const SLIDER_VALUE_W: f32 = 28.0;
pub(crate) const SLIDER_THUMB: f32 = 12.0;
pub(crate) const MEDIA_CONTROLS_W: f32 = 76.0;
const SHADOW_DY: f32 = 16.0;
const SHADOW_SIGMA: f32 = 20.0;
const MARGIN: f32 = 48.0;
pub(crate) const MAX_W: f32 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    None,
    Check,
    Radio,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Entry {
    Header,
    Separator,
    Item {
        label: String,
        accel: Option<String>,
        mark: Mark,
        icon: Option<Icon>,
        disabled: bool,
        dot: bool,
        cmd: usize,
        sub: Vec<Entry>,
    },
    Slider {
        label: String,
        icon: Icon,
        value: f32,
        text: String,
        cmd: usize,
    },
    Media {
        title: String,
        subtitle: String,
        playing: bool,
        cmd: usize,
    },
}

impl Entry {
    pub(crate) fn command(label: impl Into<String>, cmd: usize) -> Self {
        Self::marked(label, cmd, Mark::None)
    }

    pub(crate) fn toggle(label: impl Into<String>, cmd: usize, on: bool) -> Self {
        Self::marked(label, cmd, if on { Mark::Check } else { Mark::None })
    }

    pub(crate) fn radio(label: impl Into<String>, cmd: usize, on: bool) -> Self {
        Self::marked(label, cmd, if on { Mark::Radio } else { Mark::None })
    }

    fn marked(label: impl Into<String>, cmd: usize, mark: Mark) -> Self {
        Entry::Item {
            label: label.into(),
            accel: None,
            mark,
            icon: None,
            disabled: false,
            dot: false,
            cmd,
            sub: Vec::new(),
        }
    }

    pub(crate) fn submenu(label: impl Into<String>, sub: Vec<Entry>) -> Self {
        Entry::Item {
            label: label.into(),
            accel: None,
            mark: Mark::None,
            icon: None,
            disabled: false,
            dot: false,
            cmd: 0,
            sub,
        }
    }

    pub(crate) fn with_dot(mut self, on: bool) -> Self {
        if let Entry::Item { dot, .. } = &mut self {
            *dot = on;
        }
        self
    }

    pub(crate) fn with_accel(mut self, text: impl Into<String>) -> Self {
        if let Entry::Item { accel, .. } = &mut self {
            *accel = Some(text.into());
        }
        self
    }

    pub(crate) fn with_icon(mut self, glyph: Icon) -> Self {
        if let Entry::Item { icon, .. } = &mut self {
            *icon = Some(glyph);
        }
        self
    }

    pub(crate) fn disabled(mut self, off: bool) -> Self {
        if let Entry::Item { disabled, .. } = &mut self {
            *disabled = off;
        }
        self
    }

    fn height(&self) -> f32 {
        match self {
            Entry::Header => HEADER_H,
            Entry::Separator => SEP_H,
            Entry::Media { .. } => MEDIA_H,
            Entry::Item { .. } | Entry::Slider { .. } => ROW_H,
        }
    }

    pub(crate) fn selectable(&self) -> bool {
        match self {
            Entry::Item { disabled, .. } => !disabled,
            Entry::Slider { .. } | Entry::Media { .. } => true,
            _ => false,
        }
    }

    fn adjustable(&self) -> bool {
        matches!(self, Entry::Slider { .. } | Entry::Media { .. })
    }

    fn children(&self) -> Option<&[Entry]> {
        match self {
            Entry::Item { sub, .. } if !sub.is_empty() => Some(sub),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Placement {
    Anchor(POINT),
    Center(POINT),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    Run(usize),
    Step(usize, i32),
    Set(usize, f32),
    Media(usize, i32),
}

pub(crate) enum Reply {
    Stay,
    Replace(Vec<Entry>),
    Close(Option<Box<dyn FnOnce()>>),
}

pub(crate) type Handler = Box<dyn FnMut(Action) -> Reply>;
pub(crate) type Refresh = Box<dyn FnMut() -> Option<Vec<Entry>>>;

pub(crate) struct Spec {
    pub(crate) root: Vec<Entry>,
    pub(crate) root_w: f32,
    pub(crate) sub_w: f32,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) zoom: f32,
    pub(crate) placement: Placement,
    pub(crate) handler: Handler,
    pub(crate) refresh: Option<(u32, Refresh)>,
    pub(crate) on_close: Option<Box<dyn FnOnce()>>,
}

pub(crate) fn panel_height(entries: &[Entry]) -> f32 {
    2.0 * PANEL_PAD + entries.iter().map(Entry::height).sum::<f32>()
}

pub(crate) fn entry_rects(entries: &[Entry], width: f32) -> Vec<Rect> {
    let mut y = PANEL_PAD;
    entries
        .iter()
        .map(|e| {
            let r = rect(PANEL_PAD, y, width - 2.0 * PANEL_PAD, e.height());
            y += e.height();
            r
        })
        .collect()
}

pub(crate) fn item_row_width(label_w: f32, accel_w: Option<f32>, sub: bool) -> f32 {
    10.0 + 16.0
        + 10.0
        + label_w
        + accel_w.map_or(0.0, |a| 16.0 + a)
        + if sub { 10.0 + 16.0 } else { 0.0 }
        + 10.0
}

pub(crate) fn header_row_width(title_w: f32, status_w: f32) -> f32 {
    10.0 + 18.0 + 10.0 + title_w + 16.0 + (10.0 + 8.0 + 6.0 + status_w + 10.0) + 10.0
}

pub(crate) fn fit_panel_width(row_widths: impl IntoIterator<Item = f32>, min: f32) -> f32 {
    let widest = row_widths.into_iter().fold(0.0, f32::max) + 2.0 * PANEL_PAD;
    widest.ceil().clamp(min, MAX_W.max(min))
}

pub(crate) fn ellipsize(text: &str, max_w: f32, mut measure: impl FnMut(&str) -> f32) -> String {
    if measure(text) <= max_w {
        return text.to_string();
    }
    let cuts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, cuts.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate = format!(
            "{}\u{2026}",
            text[..cuts[mid.min(cuts.len() - 1)]].trim_end()
        );
        if mid < cuts.len() && measure(&candidate) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    format!(
        "{}\u{2026}",
        text[..cuts.get(lo).copied().unwrap_or(0)].trim_end()
    )
}

pub(crate) fn slider_track(row: Rect) -> Rect {
    let x = row.x + SLIDER_TRACK_X;
    let right = row.x + row.w - 10.0 - SLIDER_VALUE_W - 10.0;
    rect(x, row.y + 12.0, right - x, 12.0)
}

pub(crate) fn slider_thumb_x(track: Rect, value: f32) -> f32 {
    track.x + value.clamp(0.0, 1.0) * (track.w - SLIDER_THUMB)
}

pub(crate) fn slider_value_at(track: Rect, x: f32) -> f32 {
    ((x - track.x - SLIDER_THUMB / 2.0) / (track.w - SLIDER_THUMB)).clamp(0.0, 1.0)
}

pub(crate) fn media_buttons(row: Rect) -> [Rect; 3] {
    let x0 = row.x + row.w - 10.0 - MEDIA_CONTROLS_W;
    let y = row.y + (row.h - 16.0) / 2.0;
    [
        rect(x0, y, 16.0, 16.0),
        rect(x0 + 30.0, y, 16.0, 16.0),
        rect(x0 + 60.0, y, 16.0, 16.0),
    ]
}

pub(crate) fn place_root(anchor: (f32, f32), size: (f32, f32), work: Rect) -> (f32, f32) {
    let (w, h) = size;
    let mut x = anchor.0;
    if x + w > work.x + work.w {
        x = anchor.0 - w;
    }
    let mut y = anchor.1 - h;
    if y < work.y {
        y = anchor.1;
    }
    x = x.clamp(work.x, (work.x + work.w - w).max(work.x));
    y = y.clamp(work.y, (work.y + work.h - h).max(work.y));
    (x, y)
}

pub(crate) fn place_center(size: (f32, f32), work: Rect) -> (f32, f32) {
    (
        (work.x + (work.w - size.0) / 2.0).max(work.x).round(),
        (work.y + (work.h - size.1) / 2.0).max(work.y).round(),
    )
}

pub(crate) fn place_sub(
    parent: Rect,
    row_top: f32,
    size: (f32, f32),
    work: Rect,
    overlap: f32,
) -> (f32, f32) {
    let (w, h) = size;
    let mut x = parent.x + parent.w - overlap;
    if x + w > work.x + work.w {
        x = parent.x - w + overlap;
    }
    let mut y = row_top - PANEL_PAD;
    if y + h > work.y + work.h {
        y = work.y + work.h - h;
    }
    y = y.max(work.y);
    x = x.max(work.x);
    (x, y)
}

pub(crate) fn step_selectable(entries: &[Entry], from: Option<usize>, down: bool) -> Option<usize> {
    let n = entries.len();
    if n == 0 || !entries.iter().any(Entry::selectable) {
        return None;
    }
    let mut i = match from {
        Some(i) => i,
        None if down => n - 1,
        None => 0,
    };
    for _ in 0..n {
        i = if down { (i + 1) % n } else { (i + n - 1) % n };
        if entries[i].selectable() {
            return Some(i);
        }
    }
    from
}

pub(crate) fn level_entries<'a>(root: &'a [Entry], path: &[usize], level: usize) -> &'a [Entry] {
    let mut cur = root;
    for &i in &path[..level] {
        match cur.get(i).and_then(Entry::children) {
            Some(c) => cur = c,
            None => return &[],
        }
    }
    cur
}

struct Menu {
    hwnd: HWND,
    scale: f32,
    work: Rect,
    spec: Spec,
    path: Vec<usize>,
    focus: Vec<Option<usize>>,
    panels: Vec<Rect>,
    origin: (f32, f32),
    theme: Theme,
    status: (bool, String),
    update: Option<String>,
    gfx: Option<Gfx>,
    widths: Vec<f32>,
    ticks: u32,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    static CLASS_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn monitor_at(pt: POINT) -> (f32, Rect) {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let r = info.rcWork;
        (
            dx.max(96) as f32 / 96.0,
            rect(
                r.left as f32,
                r.top as f32,
                (r.right - r.left) as f32,
                (r.bottom - r.top) as f32,
            ),
        )
    }
}

pub(crate) fn tray_spec(owner: HWND, at: POINT, root: Vec<Entry>, build: &str) -> Spec {
    Spec {
        root,
        root_w: ROOT_W,
        sub_w: SUB_W,
        title: "warmUP Companion".to_string(),
        subtitle: build.to_string(),
        zoom: 1.0,
        placement: Placement::Anchor(at),
        handler: Box::new(move |action| match action {
            Action::Run(cmd) => Reply::Close(Some(Box::new(move || unsafe {
                let _ = PostMessageW(owner, WM_COMMAND, WPARAM(cmd), LPARAM(0));
            }))),
            _ => Reply::Stay,
        }),
        refresh: None,
        on_close: None,
    }
}

pub(crate) fn show(owner: HWND, at: POINT, root: Vec<Entry>, build: &str) {
    open(tray_spec(owner, at, root, build));
}

pub(crate) fn is_open() -> bool {
    MENU.with(|m| m.try_borrow().map(|m| m.is_some()).unwrap_or(true))
}

pub(crate) fn open(mut spec: Spec) {
    close();
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        if let Placement::Anchor(_) = spec.placement {
            let mut pt = POINT::default();
            if windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt).is_ok() {
                spec.placement = Placement::Anchor(pt);
            }
        }
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        if !CLASS_READY.with(|c| c.replace(true)) {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: CLASS,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("warmUP Companion"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            HMENU::default(),
            windows::Win32::Foundation::HINSTANCE(instance.0),
            None,
        ) else {
            return;
        };
        let at = match spec.placement {
            Placement::Anchor(p) | Placement::Center(p) => p,
        };
        let (dpi, work) = monitor_at(at);
        let mut menu = Menu {
            hwnd,
            scale: dpi * spec.zoom,
            work,
            spec,
            path: Vec::new(),
            focus: vec![None],
            panels: Vec::new(),
            origin: (0.0, 0.0),
            theme: current_theme(),
            status: super::controller_center::pad_status(),
            update: crate::updater::available_version().map(|v| format!("v{v}")),
            gfx: Gfx::new().ok(),
            widths: Vec::new(),
            ticks: 0,
        };
        if matches!(menu.spec.placement, Placement::Center(_)) {
            menu.focus[0] = step_selectable(&menu.spec.root, None, true);
        }
        layout(&mut menu);
        MENU.with(|m| *m.borrow_mut() = Some(menu));
        super::controller_center::set_menu_owns_pad(true);
        with_menu(paint);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
    }
}

pub(crate) fn close() {
    let menu = MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.take()));
    if let Some(mut menu) = menu {
        super::controller_center::set_menu_owns_pad(false);
        unsafe {
            let _ = KillTimer(menu.hwnd, TIMER_ID);
            let _ = DestroyWindow(menu.hwnd);
        }
        if let Some(f) = menu.spec.on_close.take() {
            f();
        }
    }
}

fn with_menu(f: impl FnOnce(&mut Menu)) {
    MENU.with(|m| {
        if let Ok(mut b) = m.try_borrow_mut() {
            if let Some(menu) = b.as_mut() {
                f(menu);
            }
        }
    });
}

unsafe fn measure_panel(g: &mut Gfx, m: &Menu, entries: &[Entry], min: f32) -> f32 {
    let mut rows = Vec::with_capacity(entries.len());
    for e in entries {
        match e {
            Entry::Header => {
                let title = g.measure(&m.spec.title, 14.0, true).max(g.measure(
                    &m.spec.subtitle,
                    11.0,
                    false,
                ));
                rows.push(header_row_width(title, g.measure(&m.status.1, 12.0, false)));
            }
            Entry::Item {
                label, accel, sub, ..
            } => {
                let accel_w = accel.as_deref().map(|a| g.measure(a, 12.0, false));
                rows.push(item_row_width(
                    g.measure(label, 14.0, false),
                    accel_w,
                    !sub.is_empty(),
                ));
            }
            _ => {}
        }
    }
    fit_panel_width(rows, min)
}

fn measure_widths(m: &mut Menu) {
    let levels = m.path.len() + 1;
    let mut gfx = m.gfx.take();
    let widths = (0..levels)
        .map(|level| {
            let min = if level == 0 {
                m.spec.root_w
            } else {
                m.spec.sub_w
            };
            let entries = level_entries(&m.spec.root, &m.path, level);
            match gfx.as_mut() {
                Some(g) => unsafe { measure_panel(g, m, entries, min) },
                None => min,
            }
        })
        .collect();
    m.gfx = gfx;
    m.widths = widths;
}

fn layout(m: &mut Menu) {
    measure_widths(m);
    let s = m.scale;
    let mut panels = Vec::new();
    let root_size = (m.widths[0] * s, panel_height(&m.spec.root) * s);
    let (x, y) = match m.spec.placement {
        Placement::Anchor(p) => place_root((p.x as f32, p.y as f32), root_size, m.work),
        Placement::Center(_) => place_center(root_size, m.work),
    };
    panels.push(rect(x, y, root_size.0, root_size.1));
    for level in 1..=m.path.len() {
        let entries = level_entries(&m.spec.root, &m.path, level);
        let parent = panels[level - 1];
        let parent_entries = level_entries(&m.spec.root, &m.path, level - 1);
        let rows = entry_rects(parent_entries, m.widths[level - 1]);
        let row_top = parent.y + rows[m.path[level - 1]].y * s;
        let size = (m.widths[level] * s, panel_height(entries) * s);
        let (sx, sy) = place_sub(parent, row_top, size, m.work, OVERLAP * s);
        panels.push(rect(sx, sy, size.0, size.1));
    }
    let margin = MARGIN * s;
    let min_x = panels.iter().map(|p| p.x).fold(f32::MAX, f32::min) - margin;
    let min_y = panels.iter().map(|p| p.y).fold(f32::MAX, f32::min) - margin;
    m.origin = (min_x.floor(), min_y.floor());
    m.panels = panels;
    m.focus.resize(m.path.len() + 1, None);
}

fn surface(m: &Menu) -> (i32, i32) {
    let margin = MARGIN * m.scale;
    let max_x = m.panels.iter().map(|p| p.x + p.w).fold(0.0, f32::max) + margin;
    let max_y = m.panels.iter().map(|p| p.y + p.h).fold(0.0, f32::max) + margin;
    (
        (max_x - m.origin.0).ceil() as i32,
        (max_y - m.origin.1).ceil() as i32,
    )
}

fn paint(m: &mut Menu) {
    let mut gfx = match m.gfx.take() {
        Some(g) => g,
        None => match unsafe { Gfx::new() } {
            Ok(g) => g,
            Err(e) => {
                crate::install::log_line(&format!("menu: gfx: {e}"));
                return;
            }
        },
    };
    if let Err(e) = unsafe { render(m, &mut gfx) } {
        crate::install::log_line(&format!("menu: render: {e}"));
        return;
    }
    m.gfx = Some(gfx);
}

unsafe fn render(m: &Menu, g: &mut Gfx) -> Result<(), String> {
    let s = m.scale;
    g.begin(surface(m))?;
    let cards: Vec<Rect> = m
        .panels
        .iter()
        .map(|p| {
            rect(
                (p.x - m.origin.0).round(),
                (p.y - m.origin.1 + SHADOW_DY * s).round(),
                p.w.round(),
                p.h.round(),
            )
        })
        .collect();
    g.draw_shadow(&cards, PANEL_R * s, SHADOW_SIGMA * s);
    for (level, panel) in m.panels.iter().enumerate() {
        g.transform(s, panel.x - m.origin.0, panel.y - m.origin.1);
        draw_panel(m, g, level);
    }
    g.present(
        m.hwnd,
        Some(POINT {
            x: m.origin.0 as i32,
            y: m.origin.1 as i32,
        }),
    )
}

unsafe fn draw_header(m: &Menu, g: &mut Gfx, r: Rect) {
    let t = m.theme;
    let s = m.scale;
    g.icon(
        Icon::Logo,
        rect(r.x + 10.0, r.y + 17.0, 18.0, 18.0),
        t.text,
        1.0,
        s,
    );
    g.text(
        &m.spec.title,
        rect(r.x + 38.0, r.y + 10.0, 170.0, 17.0),
        14.0,
        true,
        t.text,
        1.0,
        Align::Left,
    );
    g.text(
        &m.spec.subtitle,
        rect(r.x + 38.0, r.y + 29.0, 170.0, 13.0),
        11.0,
        false,
        t.dim,
        DIM_ALPHA,
        Align::Left,
    );
    if let Some(v) = &m.update {
        let tw = g.measure(v, 12.0, true);
        let pw = 10.0 + 12.0 + 6.0 + tw + 10.0;
        let pill = rect(r.x + r.w - 10.0 - pw, r.y + 14.0, pw, 24.0);
        g.fill(pill, 12.0, t.accent, 0.15);
        g.icon(
            Icon::Download,
            rect(pill.x + 10.0, pill.y + 6.0, 12.0, 12.0),
            t.accent,
            1.0,
            s,
        );
        g.text(
            v,
            rect(pill.x + 28.0, pill.y, tw + 4.0, pill.h),
            12.0,
            true,
            t.accent,
            1.0,
            Align::Left,
        );
        return;
    }
    let tw = g.measure(&m.status.1, 12.0, false);
    let pw = 10.0 + 8.0 + 6.0 + tw + 10.0;
    let pill = rect(r.x + r.w - 10.0 - pw, r.y + 14.0, pw, 24.0);
    g.fill(pill, 12.0, t.tile, 1.0);
    g.circle(
        pill.x + 14.0,
        pill.y + 12.0,
        4.0,
        if m.status.0 { 0x0058D130 } else { t.idle_dot },
    );
    g.text(
        &m.status.1,
        rect(pill.x + 24.0, pill.y, tw + 4.0, pill.h),
        12.0,
        false,
        t.dim,
        DIM_ALPHA,
        Align::Left,
    );
}

unsafe fn draw_panel(m: &Menu, g: &mut Gfx, level: usize) {
    let t = m.theme;
    let s = m.scale;
    let entries = level_entries(&m.spec.root, &m.path, level);
    let w = m.widths.get(level).copied().unwrap_or(m.spec.sub_w);
    let box_ = rect(0.0, 0.0, w, panel_height(entries));
    g.fill(box_, PANEL_R, t.bg, 1.0);
    g.ring(box_, PANEL_R, 1.0, t.line, t.line_alpha);
    let focus = m.focus.get(level).copied().flatten();
    for (i, (e, r)) in entries.iter().zip(entry_rects(entries, w)).enumerate() {
        let open = m.path.get(level) == Some(&i);
        let hot = focus == Some(i) || open;
        if hot && e.selectable() {
            g.fill(r, 8.0, t.hi, 1.0);
        }
        g.rt.PushAxisAlignedClip(
            &r.d2d(),
            windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
        );
        let (icon_c, icon_a) = if hot {
            (t.text, 1.0)
        } else {
            (t.dim, DIM_ALPHA)
        };
        match e {
            Entry::Header => draw_header(m, g, r),
            Entry::Separator => {
                g.fill(
                    rect(r.x + 10.0, r.y + 6.0, r.w - 20.0, 1.0),
                    0.0,
                    t.line,
                    t.line_alpha,
                );
            }
            Entry::Item {
                label,
                accel,
                mark,
                icon,
                disabled,
                dot,
                sub,
                ..
            } => {
                let indicator = rect(r.x + 10.0, r.y + 10.0, 16.0, 16.0);
                match (mark, icon) {
                    (Mark::Check, _) => g.icon(Icon::Check, indicator, t.accent, 1.0, s),
                    (Mark::Radio, _) => g.circle(r.x + 18.0, r.y + 18.0, 4.0, t.accent),
                    (Mark::None, Some(glyph)) => g.icon(
                        *glyph,
                        indicator,
                        icon_c,
                        if *disabled { DIM_ALPHA * 0.6 } else { icon_a },
                        s,
                    ),
                    _ => {}
                }
                let (lc, la) = if *disabled {
                    (t.dim, DIM_ALPHA)
                } else {
                    (t.text, 1.0)
                };
                let accel_w = accel.as_deref().map(|a| g.measure(a, 12.0, false));
                let reserve = item_row_width(0.0, accel_w, !sub.is_empty()) - 36.0;
                let label_w = (r.w - 36.0 - reserve).max(0.0);
                let shown = ellipsize(label, label_w, |t| g.measure(t, 14.0, false));
                g.text(
                    &shown,
                    rect(r.x + 36.0, r.y, label_w + 2.0, r.h),
                    14.0,
                    false,
                    lc,
                    la,
                    Align::Left,
                );
                let accel_right = if sub.is_empty() {
                    r.x + r.w - 10.0
                } else {
                    r.x + r.w - 36.0
                };
                if let Some(a) = accel {
                    g.text(
                        a,
                        rect(accel_right - 160.0, r.y, 160.0, r.h),
                        12.0,
                        false,
                        t.dim,
                        DIM_ALPHA,
                        Align::Right,
                    );
                }
                if *dot {
                    g.circle(accel_right - 4.0, r.y + r.h / 2.0, 4.0, t.accent);
                }
                if !sub.is_empty() {
                    g.icon(
                        Icon::ChevronRight,
                        rect(r.x + r.w - 26.0, r.y + 10.0, 16.0, 16.0),
                        icon_c,
                        icon_a,
                        s,
                    );
                }
            }
            Entry::Slider {
                label,
                icon,
                value,
                text,
                ..
            } => {
                g.icon(
                    *icon,
                    rect(r.x + 10.0, r.y + 10.0, 16.0, 16.0),
                    icon_c,
                    icon_a,
                    s,
                );
                g.text(
                    label,
                    rect(r.x + 36.0, r.y, 84.0, r.h),
                    14.0,
                    false,
                    t.text,
                    1.0,
                    Align::Left,
                );
                let track = slider_track(r);
                let thumb = slider_thumb_x(track, *value);
                let mid = track.y + 4.0;
                if thumb > track.x {
                    g.fill(rect(track.x, mid, thumb - track.x, 4.0), 2.0, t.accent, 1.0);
                }
                let rest = thumb + SLIDER_THUMB;
                if rest < track.x + track.w {
                    g.fill(
                        rect(rest, mid, track.x + track.w - rest, 4.0),
                        2.0,
                        t.hi,
                        1.0,
                    );
                }
                g.circle(thumb + 6.0, track.y + 6.0, 6.0, 0x00FF_FFFF);
                g.text(
                    text,
                    rect(r.x + r.w - 10.0 - 60.0, r.y, 60.0, r.h),
                    12.0,
                    false,
                    t.dim,
                    DIM_ALPHA,
                    Align::Right,
                );
            }
            Entry::Media {
                title,
                subtitle,
                playing,
                ..
            } => {
                g.icon(
                    Icon::Svg(MUSIC),
                    rect(r.x + 10.0, r.y + 16.0, 16.0, 16.0),
                    icon_c,
                    icon_a,
                    s,
                );
                let text_w = r.w - 36.0 - 10.0 - MEDIA_CONTROLS_W - 10.0;
                let title = ellipsize(title, text_w, |t| g.measure(t, 14.0, false));
                let subtitle = ellipsize(subtitle, text_w, |t| g.measure(t, 11.0, false));
                g.text(
                    &title,
                    rect(r.x + 36.0, r.y + 8.0, text_w, 17.0),
                    14.0,
                    false,
                    t.text,
                    1.0,
                    Align::Left,
                );
                g.text(
                    &subtitle,
                    rect(r.x + 36.0, r.y + 27.0, text_w, 13.0),
                    11.0,
                    false,
                    t.dim,
                    DIM_ALPHA,
                    Align::Left,
                );
                let [prev, play, next] = media_buttons(r);
                g.icon(Icon::Svg(SKIP_BACK), prev, t.dim, DIM_ALPHA, s);
                g.icon(
                    Icon::Svg(if *playing { PAUSE } else { PLAY }),
                    play,
                    t.text,
                    1.0,
                    s,
                );
                g.icon(Icon::Svg(SKIP_FORWARD), next, t.dim, DIM_ALPHA, s);
            }
        }
        g.rt.PopAxisAlignedClip();
    }
}

macro_rules! lucide {
    ($body:literal) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        )
    };
}
pub(crate) use lucide;

const MUSIC: &str = lucide!(
    r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#
);
const SKIP_BACK: &str =
    lucide!(r#"<polygon points="19 20 9 12 19 4 19 20"/><line x1="5" x2="5" y1="19" y2="5"/>"#);
const SKIP_FORWARD: &str =
    lucide!(r#"<polygon points="5 4 15 12 5 20 5 4"/><line x1="19" x2="19" y1="5" y2="19"/>"#);
const PAUSE: &str = lucide!(
    r#"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"#
);
const PLAY: &str = lucide!(r#"<polygon points="6 3 20 12 6 21 6 3"/>"#);

enum Hit {
    Row(usize, usize, Rect),
    Panel,
    Outside,
}

fn hit(m: &Menu, sx: f32, sy: f32) -> (Hit, f32, f32) {
    for level in (0..m.panels.len()).rev() {
        let p = m.panels[level];
        if !p.contains(sx, sy) {
            continue;
        }
        let (x, y) = ((sx - p.x) / m.scale, (sy - p.y) / m.scale);
        let entries = level_entries(&m.spec.root, &m.path, level);
        for (i, r) in entry_rects(
            entries,
            m.widths.get(level).copied().unwrap_or(m.spec.sub_w),
        )
        .iter()
        .enumerate()
        {
            if r.contains(x, y) && entries[i].selectable() {
                return (Hit::Row(level, i, *r), x, y);
            }
        }
        return (Hit::Panel, x, y);
    }
    (Hit::Outside, 0.0, 0.0)
}

enum Act {
    Stay,
    Close(Option<Box<dyn FnOnce()>>),
}

fn open_sub(m: &mut Menu, level: usize, i: usize, focus_first: bool) {
    m.path.truncate(level);
    let entries = level_entries(&m.spec.root, &m.path, level);
    if entries.get(i).and_then(Entry::children).is_some() {
        m.path.push(i);
        m.focus.truncate(level + 1);
        m.focus.push(None);
        if focus_first {
            let sub = level_entries(&m.spec.root, &m.path, level + 1);
            m.focus[level + 1] = step_selectable(sub, None, true);
        }
    }
    layout(m);
}

fn replace_root(m: &mut Menu, root: Vec<Entry>) {
    m.spec.root = root;
    let mut depth = 0;
    while depth < m.path.len() {
        let entries = level_entries(&m.spec.root, &m.path, depth);
        if entries
            .get(m.path[depth])
            .and_then(Entry::children)
            .is_none()
        {
            break;
        }
        depth += 1;
    }
    m.path.truncate(depth);
    m.focus.truncate(depth + 1);
    for level in 0..m.focus.len() {
        let entries = level_entries(&m.spec.root, &m.path, level);
        if let Some(i) = m.focus[level] {
            if !entries.get(i).is_some_and(Entry::selectable) {
                m.focus[level] = step_selectable(entries, Some(i.min(entries.len())), true)
                    .or_else(|| step_selectable(entries, None, true));
            }
        }
    }
    layout(m);
}

fn dispatch(m: &mut Menu, action: Action) -> Act {
    match (m.spec.handler)(action) {
        Reply::Stay => Act::Stay,
        Reply::Replace(root) => {
            replace_root(m, root);
            Act::Stay
        }
        Reply::Close(after) => Act::Close(after),
    }
}

fn entry_cmd(e: &Entry) -> usize {
    match e {
        Entry::Item { cmd, .. } | Entry::Slider { cmd, .. } | Entry::Media { cmd, .. } => *cmd,
        _ => 0,
    }
}

fn activate(m: &mut Menu, level: usize, i: usize) -> Act {
    let entries = level_entries(&m.spec.root, &m.path, level);
    match entries.get(i) {
        Some(e @ Entry::Item { .. }) if e.children().is_some() => {
            open_sub(m, level, i, true);
            Act::Stay
        }
        Some(e) if e.selectable() => {
            let cmd = entry_cmd(e);
            dispatch(m, Action::Run(cmd))
        }
        _ => Act::Stay,
    }
}

fn click(m: &mut Menu, level: usize, i: usize, row: Rect, x: f32) -> Act {
    let entries = level_entries(&m.spec.root, &m.path, level);
    match entries.get(i) {
        Some(Entry::Slider { cmd, .. }) => {
            let cmd = *cmd;
            let v = slider_value_at(slider_track(row), x);
            dispatch(m, Action::Set(cmd, v))
        }
        Some(Entry::Media { cmd, .. }) => {
            let cmd = *cmd;
            let [prev, play, next] = media_buttons(row);
            let zone = |r: Rect| r.inset(-7.0).contains(x, r.y + 8.0);
            if zone(prev) {
                dispatch(m, Action::Media(cmd, -1))
            } else if zone(next) {
                dispatch(m, Action::Media(cmd, 1))
            } else if zone(play) || x < play.x {
                dispatch(m, Action::Media(cmd, 0))
            } else {
                Act::Stay
            }
        }
        _ => activate(m, level, i),
    }
}

fn hover(m: &mut Menu, level: usize, i: usize) {
    m.focus.truncate(level + 1);
    m.focus.resize(level + 1, None);
    m.focus[level] = Some(i);
    let entries = level_entries(&m.spec.root, &m.path, level);
    let has_sub = entries.get(i).and_then(Entry::children).is_some();
    if m.path.get(level) == Some(&i) {
        return;
    }
    m.path.truncate(level);
    if has_sub {
        open_sub(m, level, i, false);
    } else {
        layout(m);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Back,
    Close,
}

fn focus_level(m: &Menu) -> usize {
    let level = m.path.len().min(m.focus.len() - 1);
    (0..=level)
        .rev()
        .find(|l| m.focus.get(*l).copied().flatten().is_some())
        .unwrap_or(0)
}

fn nav(m: &mut Menu, key: NavKey) -> Act {
    if key == NavKey::Close {
        return Act::Close(None);
    }
    let level = focus_level(m);
    let entries = level_entries(&m.spec.root, &m.path, level);
    let cur = m.focus[level];
    if let (Some(i), NavKey::Left | NavKey::Right) = (cur, key) {
        if entries[i].adjustable() {
            let delta = if key == NavKey::Right { 1 } else { -1 };
            let cmd = entry_cmd(&entries[i]);
            let action = if matches!(entries[i], Entry::Media { .. }) {
                Action::Media(cmd, delta)
            } else {
                Action::Step(cmd, delta)
            };
            return dispatch(m, action);
        }
    }
    match key {
        NavKey::Up | NavKey::Down => {
            m.focus[level] = step_selectable(entries, cur, key == NavKey::Down);
            m.path.truncate(level);
            m.focus.truncate(level + 1);
            layout(m);
            Act::Stay
        }
        NavKey::Right | NavKey::Enter => match cur {
            Some(i) if key == NavKey::Enter || entries[i].children().is_some() => {
                activate(m, level, i)
            }
            _ => Act::Stay,
        },
        NavKey::Left | NavKey::Back | NavKey::Close => {
            if level == 0 && m.path.is_empty() {
                return if key == NavKey::Back {
                    Act::Close(None)
                } else {
                    Act::Stay
                };
            }
            let parent = if m.path.len() > level {
                level
            } else {
                level - 1
            };
            m.path.truncate(parent);
            m.focus.truncate(parent + 1);
            layout(m);
            Act::Stay
        }
    }
}

fn finish(act: Act) {
    match act {
        Act::Stay => with_menu(paint),
        Act::Close(after) => {
            close();
            if let Some(f) = after {
                f();
            }
        }
    }
}

fn client_to_screen(m: &Menu, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x + m.origin.0, y + m.origin.1)
}

pub(crate) fn pad_key(button: &str) -> Option<NavKey> {
    Some(match button {
        "UP" => NavKey::Up,
        "DOWN" => NavKey::Down,
        "LEFT" => NavKey::Left,
        "RIGHT" => NavKey::Right,
        "A" => NavKey::Enter,
        "B" => NavKey::Back,
        "GUIDE" => NavKey::Close,
        _ => return None,
    })
}

fn tick(m: &mut Menu) -> bool {
    m.ticks += 1;
    let Some((every, refresh)) = m.spec.refresh.as_mut() else {
        return false;
    };
    let every = (*every / TIMER_MS).max(1);
    if m.ticks % every != 0 {
        return false;
    }
    match refresh() {
        Some(root) if root != m.spec.root => {
            replace_root(m, root);
            true
        }
        _ => false,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut changed = false;
            with_menu(|m| {
                let (x, y) = client_to_screen(m, lparam);
                if let (Hit::Row(level, i, _), _, _) = hit(m, x, y) {
                    if m.focus.get(level).copied().flatten() != Some(i) {
                        hover(m, level, i);
                        changed = true;
                    }
                }
            });
            if changed {
                finish(Act::Stay);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut act = Act::Stay;
            with_menu(|m| {
                let (sx, sy) = client_to_screen(m, lparam);
                act = match hit(m, sx, sy) {
                    (Hit::Row(level, i, row), x, _) => {
                        hover(m, level, i);
                        click(m, level, i, row, x)
                    }
                    (Hit::Panel, _, _) => Act::Stay,
                    (Hit::Outside, _, _) => Act::Close(None),
                };
            });
            finish(act);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = match wparam.0 as u16 {
                v if v == VK_UP.0 => Some(NavKey::Up),
                v if v == VK_DOWN.0 => Some(NavKey::Down),
                v if v == VK_LEFT.0 => Some(NavKey::Left),
                v if v == VK_RIGHT.0 => Some(NavKey::Right),
                v if v == VK_RETURN.0 || v == VK_SPACE.0 => Some(NavKey::Enter),
                v if v == VK_ESCAPE.0 => Some(NavKey::Back),
                _ => None,
            };
            if let Some(key) = key {
                let mut act = Act::Stay;
                with_menu(|m| act = nav(m, key));
                finish(act);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            let mut repaint = false;
            with_menu(|m| repaint = tick(m));
            if repaint {
                finish(Act::Stay);
            }
            for (button, pressed) in super::controller_center::take_menu_pad_edges() {
                if !pressed {
                    continue;
                }
                if let Some(key) = pad_key(button) {
                    let mut act = Act::Stay;
                    with_menu(|m| act = nav(m, key));
                    let closing = matches!(act, Act::Close(_));
                    finish(act);
                    if closing {
                        break;
                    }
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE if (wparam.0 & 0xffff) as u32 == WA_INACTIVE => {
            if MENU.with(|m| m.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.hwnd)))
                == Some(hwnd)
            {
                close();
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Entry> {
        vec![
            Entry::Header,
            Entry::Separator,
            Entry::toggle("Pause", 1, false),
            Entry::submenu(
                "Keyboard",
                vec![
                    Entry::toggle("Compact", 2, true),
                    Entry::submenu("Style", vec![Entry::radio("Mono", 3, true)]),
                ],
            ),
            Entry::Separator,
            Entry::command("Exit", 9),
        ]
    }

    #[test]
    fn panel_geometry_matches_the_design() {
        let root = sample();
        assert_eq!(panel_height(&root), 12.0 + 52.0 + 13.0 * 2.0 + 36.0 * 3.0);
        let rows = entry_rects(&root, ROOT_W);
        assert_eq!(rows[0], rect(6.0, 6.0, 332.0, 52.0));
        assert_eq!(rows[2], rect(6.0, 71.0, 332.0, 36.0));
        assert_eq!(rows[3].y, 107.0);
        assert_eq!(rows[5].y, 156.0);
    }

    #[test]
    fn root_opens_above_the_cursor_and_flips_at_screen_edges() {
        let work = rect(0.0, 0.0, 1920.0, 1040.0);
        assert_eq!(
            place_root((1700.0, 1030.0), (344.0, 500.0), work),
            (1356.0, 530.0)
        );
        assert_eq!(
            place_root((100.0, 1030.0), (344.0, 500.0), work),
            (100.0, 530.0)
        );
        assert_eq!(
            place_root((100.0, 200.0), (344.0, 500.0), work),
            (100.0, 200.0)
        );
        assert_eq!(
            place_root((1900.0, 1100.0), (344.0, 500.0), work),
            (1556.0, 540.0)
        );
        assert_eq!(
            place_center((360.0, 668.0), rect(1920.0, 0.0, 1920.0, 1040.0)),
            (2700.0, 186.0)
        );
    }

    #[test]
    fn submenus_open_right_and_flip_left_or_up() {
        let work = rect(0.0, 0.0, 1920.0, 1040.0);
        let parent = rect(100.0, 300.0, 344.0, 500.0);
        assert_eq!(
            place_sub(parent, 400.0, (240.0, 200.0), work, OVERLAP),
            (440.0, 394.0)
        );
        let right = rect(1600.0, 300.0, 344.0, 500.0);
        assert_eq!(
            place_sub(right, 400.0, (240.0, 200.0), work, OVERLAP),
            (1364.0, 394.0)
        );
        assert_eq!(
            place_sub(parent, 950.0, (240.0, 200.0), work, OVERLAP),
            (440.0, 840.0)
        );
    }

    #[test]
    fn navigation_skips_headers_and_separators_and_wraps() {
        let root = sample();
        assert_eq!(step_selectable(&root, None, true), Some(2));
        assert_eq!(step_selectable(&root, Some(2), true), Some(3));
        assert_eq!(step_selectable(&root, Some(3), true), Some(5));
        assert_eq!(step_selectable(&root, Some(5), true), Some(2));
        assert_eq!(step_selectable(&root, Some(2), false), Some(5));
        assert_eq!(step_selectable(&[Entry::Header], None, true), None);
        assert_eq!(level_entries(&root, &[3], 1).len(), 2);
        assert_eq!(
            level_entries(&root, &[3, 1], 2),
            &[Entry::radio("Mono", 3, true)]
        );
        assert!(level_entries(&root, &[2], 1).is_empty());
        let disabled = vec![
            Entry::command("A", 1),
            Entry::command("B", 2).disabled(true),
            Entry::command("C", 3),
        ];
        assert_eq!(step_selectable(&disabled, Some(0), true), Some(2));
    }

    #[test]
    fn panels_grow_to_their_widest_label_and_ellipsize_past_the_cap() {
        assert_eq!(item_row_width(100.0, Some(60.0), true), 248.0);
        assert_eq!(item_row_width(100.0, None, false), 146.0);
        assert_eq!(fit_panel_width([100.0], SUB_W), SUB_W);
        assert_eq!(fit_panel_width([200.0, 400.0], SUB_W), 412.0);
        assert_eq!(fit_panel_width([900.0], SUB_W), MAX_W);
        assert_eq!(fit_panel_width([], ROOT_W), ROOT_W);
        let label = "Guide-only input while a game is running";
        let width = |t: &str| t.chars().count() as f32 * 7.0;
        let wide = item_row_width(width(label), None, false);
        assert!(fit_panel_width([wide], SUB_W) >= wide + 2.0 * PANEL_PAD);
        assert_eq!(ellipsize(label, 400.0, width), label);
        let cut = ellipsize(label, 120.0, width);
        assert!(cut.ends_with('\u{2026}'));
        assert!(width(&cut) <= 120.0);
        assert!(cut.len() > 3);
        assert_eq!(ellipsize("abc", 1.0, width), "\u{2026}");
    }

    #[test]
    fn right_edge_cursor_keeps_root_and_flipped_flyout_on_screen() {
        let work = rect(0.0, 0.0, 1920.0, 1040.0);
        let s = 1.5;
        let root = (ROOT_W * s, 600.0);
        let (rx, ry) = place_root((1915.0, 1000.0), root, work);
        assert_eq!((rx, ry), (1915.0 - ROOT_W * s, 400.0));
        assert!(rx + root.0 <= 1920.0);
        let parent = rect(rx, ry, root.0, root.1);
        let sub = (420.0 * s, 150.0);
        let (sx, sy) = place_sub(parent, ry + 300.0, sub, work, OVERLAP * s);
        assert_eq!(sx, rx - sub.0 + OVERLAP * s);
        assert!(sx + sub.0 <= rx + OVERLAP * s + 0.01);
        assert_eq!(sy, ry + 300.0 - PANEL_PAD);
        let (lx, _) = place_sub(
            rect(10.0, 0.0, 500.0, 500.0),
            0.0,
            (1500.0, 100.0),
            work,
            6.0,
        );
        assert_eq!(lx, 0.0);
        let (cx, cy) = place_root((1919.0, 5.0), (500.0, 2000.0), work);
        assert!(cx >= 0.0 && cx + 500.0 <= 1920.0 && cy == 0.0);
    }

    #[test]
    fn slider_and_media_geometry() {
        let row = rect(6.0, 119.0, 348.0, 36.0);
        let track = slider_track(row);
        assert_eq!(track, rect(136.0, 131.0, 170.0, 12.0));
        assert_eq!(slider_thumb_x(track, 0.0), 136.0);
        assert_eq!(slider_thumb_x(track, 1.0), 136.0 + 158.0);
        for v in [0.0f32, 0.25, 0.62, 1.0] {
            let x = slider_thumb_x(track, v) + 6.0;
            assert!((slider_value_at(track, x) - v).abs() < 1e-4);
        }
        let media = rect(6.0, 71.0, 348.0, 48.0);
        let [prev, play, next] = media_buttons(media);
        assert_eq!(prev, rect(268.0, 87.0, 16.0, 16.0));
        assert_eq!(play.x, 298.0);
        assert_eq!(next.x + next.w, 344.0);
        assert_eq!(
            Entry::Media {
                title: String::new(),
                subtitle: String::new(),
                playing: false,
                cmd: 0
            }
            .height(),
            48.0
        );
    }
}

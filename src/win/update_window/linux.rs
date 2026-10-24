use std::sync::{Mutex, OnceLock};

use super::*;
use crate::linux_panel::{self, Flow, Key, PanelApp, PanelThread, Spec};

pub(super) use crate::vk_theme::current_theme;

pub(super) type HWND = ();

pub(super) struct VirtualKey(pub(super) u16);

pub(super) const VK_TAB: VirtualKey = VirtualKey(0x09);
pub(super) const VK_RETURN: VirtualKey = VirtualKey(0x0D);
pub(super) const VK_ESCAPE: VirtualKey = VirtualKey(0x1B);
pub(super) const VK_SPACE: VirtualKey = VirtualKey(0x20);
pub(super) const VK_LEFT: VirtualKey = VirtualKey(0x25);
pub(super) const VK_UP: VirtualKey = VirtualKey(0x26);
pub(super) const VK_RIGHT: VirtualKey = VirtualKey(0x27);
pub(super) const VK_DOWN: VirtualKey = VirtualKey(0x28);

const SPIN_SEGMENTS: usize = 48;
const SPIN_SWEEP_DEG: f32 = 288.0;

static THREAD: OnceLock<Mutex<Option<PanelThread>>> = OnceLock::new();

pub(crate) fn show() {
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if guard.is_none() {
        let spec = Spec {
            thread: "warmup-update-window",
            title: "warmUP Companion update",
            app_id: "warmup-companion",
        };
        match linux_panel::spawn(spec, Box::new(UpdatePanel)) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("update window: spawn failed: {e}"));
                return;
            }
        }
    }
    if let Some(t) = guard.as_ref() {
        let _ = t.show();
    }
}

pub(super) fn place(_ui: &Ui) {}

pub(super) unsafe fn draw_spinner(ui: &Ui, g: &mut Gfx, r: Rect) {
    let angle = (ui.spin as f32 * SPIN_DEG) % 360.0;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let radius = r.w * 9.0 / 24.0;
    let width = r.w * 2.0 / 24.0;
    let point = |i: usize| {
        let a = (angle + SPIN_SWEEP_DEG * i as f32 / SPIN_SEGMENTS as f32).to_radians();
        (cx + radius * a.cos(), cy + radius * a.sin())
    };
    for i in 0..SPIN_SEGMENTS {
        let (x0, y0) = point(i);
        let (x1, y1) = point(i + 1);
        g.line(x0, y0, x1, y1, width, ui.theme.accent, 1.0);
    }
    g.circle(point(0).0, point(0).1, width / 2.0, ui.theme.accent);
    g.circle(
        point(SPIN_SEGMENTS).0,
        point(SPIN_SEGMENTS).1,
        width / 2.0,
        ui.theme.accent,
    );
}

fn flow(close: bool) -> Flow {
    if close {
        Flow::Close
    } else {
        Flow::Paint
    }
}

struct UpdatePanel;

impl PanelApp for UpdatePanel {
    fn size(&self) -> (u32, u32) {
        let h = UI
            .with(|u| u.borrow().as_ref().map(|ui| win_h(&ui.snap)))
            .unwrap_or_else(|| win_h(&updater::snapshot()));
        (WIN_W as u32, h.ceil() as u32)
    }

    fn open(&mut self, scale: f32) {
        crate::win::controller_center::set_menu_owns_pad(true);
        if UI.with(|u| u.borrow().is_some()) {
            with_ui(|ui| {
                ui.scale = scale;
                ui.playstation = pad_is_playstation();
                ui.theme = current_theme();
                refresh(ui, true);
            });
            return;
        }
        let snap = updater::snapshot();
        let ui = Ui {
            hwnd: (),
            scale,
            focus: default_focus(&snap),
            snap,
            auto_check: crate::config::gamepad_settings().update_check,
            theme: current_theme(),
            gfx: Gfx::new(1, 1).ok(),
            playstation: pad_is_playstation(),
            spin: 0,
            shown: 0.0,
        };
        UI.with(|u| *u.borrow_mut() = Some(ui));
    }

    fn close(&mut self) {
        crate::win::controller_center::set_menu_owns_pad(false);
        UI.with(|u| u.borrow_mut().take());
    }

    fn rescale(&mut self, scale: f32) {
        with_ui(|ui| ui.scale = scale);
    }

    fn caption(&mut self, x: f32, y: f32) -> bool {
        (0.0..WIN_W).contains(&x) && (0.0..TITLE_H).contains(&y) && !CLOSE.inset(-7.0).contains(x, y)
    }

    fn press(&mut self, x: f32, y: f32) -> Flow {
        let mut close = false;
        with_ui(|ui| close = click(ui, x, y));
        flow(close)
    }

    fn key(&mut self, key: Key) -> Flow {
        let Some(vk) = key.vk else {
            return Flow::Idle;
        };
        let mut close = false;
        with_ui(|ui| close = key_down(ui, vk));
        flow(close)
    }

    fn tick(&mut self) -> Flow {
        let mut result = Flow::Idle;
        with_ui(|ui| {
            let (dirty, close) = tick(ui);
            result = if close {
                Flow::Close
            } else if dirty {
                Flow::Paint
            } else {
                Flow::Idle
            };
        });
        result
    }

    fn paint(&mut self, g: &mut Gfx, scale: f32) {
        with_ui(|ui| unsafe {
            g.transform(scale, 0.0, 0.0);
            draw(ui, g, win_h(&ui.snap));
        });
    }
}

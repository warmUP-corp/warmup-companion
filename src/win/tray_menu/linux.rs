use std::sync::{Mutex, OnceLock};

use super::*;
use crate::linux_panel::{self, Flow, Key, PanelApp, PanelThread};

pub(crate) type HWND = ();

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct POINT {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

type Build = fn(POINT) -> Spec;

static THREAD: OnceLock<Mutex<Option<PanelThread>>> = OnceLock::new();
static REQUEST: Mutex<Option<(Build, POINT)>> = Mutex::new(None);

thread_local! {
    static AFTER: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

pub(crate) fn open_with(build: Build, at: POINT) {
    if let Ok(mut r) = REQUEST.lock() {
        *r = Some((build, at));
    }
    let slot = THREAD.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if guard.as_ref().is_some_and(|t| !t.is_alive()) {
        crate::install::log_line("menu: panel thread is gone, starting it again");
        *guard = None;
    }
    if guard.is_none() {
        let spec = linux_panel::Spec {
            thread: "warmup-tray-menu",
            title: "warmUP Companion",
            app_id: "warmup-companion-menu",
        };
        match linux_panel::spawn(spec, Box::new(MenuPanel::default())) {
            Ok(t) => *guard = Some(t),
            Err(e) => {
                crate::install::log_line(&format!("menu: spawn failed: {e}"));
                return;
            }
        }
    }
    if let Some(t) = guard.as_ref() {
        let _ = t.hide();
        let _ = t.show();
    }
}

fn close() {
    let menu = MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.take()));
    if let Some(mut menu) = menu {
        crate::win::controller_center::set_menu_owns_pad(false);
        if let Some(f) = menu.spec.on_close.take() {
            f();
        }
    }
    if let Some(f) = AFTER.with(|a| a.borrow_mut().take()) {
        f();
    }
}

fn flow(act: Act) -> Flow {
    match act {
        Act::Stay => Flow::Paint,
        Act::Close(after) => {
            AFTER.with(|a| *a.borrow_mut() = after);
            Flow::Close
        }
    }
}

fn nav_key(vk: u16) -> Option<NavKey> {
    Some(match vk {
        0x26 => NavKey::Up,
        0x28 => NavKey::Down,
        0x25 => NavKey::Left,
        0x27 => NavKey::Right,
        0x0D | 0x20 => NavKey::Enter,
        0x1B => NavKey::Back,
        _ => return None,
    })
}

#[derive(Default)]
struct MenuPanel {
    size: (u32, u32),
    at: POINT,
    scale: f32,
}

impl MenuPanel {
    fn place(&self, m: &mut Menu) {
        let s = self.scale;
        m.scale = s * m.spec.zoom;
        m.work = rect(0.0, 0.0, self.size.0 as f32 * s, self.size.1 as f32 * s);
        let at = POINT {
            x: (self.at.x as f32 * s).round() as i32,
            y: (self.at.y as f32 * s).round() as i32,
        };
        m.spec.placement = match m.spec.placement {
            Placement::Anchor(_) => Placement::Anchor(at),
            Placement::Center(_) => Placement::Center(at),
        };
        layout(m);
    }
}

impl PanelApp for MenuPanel {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn overlay(&self) -> bool {
        true
    }

    fn resize(&mut self, w: u32, h: u32) {
        self.size = (w, h);
        let this = &*self;
        with_menu(|m| this.place(m));
    }

    fn frame_rect(&self) -> Option<(i32, i32, u32, u32)> {
        MENU.with(|m| {
            let m = m.try_borrow().ok()?;
            let m = m.as_ref()?;
            let (w, h) = surface(m);
            Some((m.origin.0 as i32, m.origin.1 as i32, w as u32, h as u32))
        })
    }

    fn open(&mut self, scale: f32) {
        close();
        self.scale = scale;
        let Some((build, at)) = REQUEST.lock().ok().and_then(|mut r| r.take()) else {
            return;
        };
        self.at = at;
        let spec = build(at);
        let mut menu = Menu {
            hwnd: (),
            scale,
            work: rect(0.0, 0.0, 0.0, 0.0),
            spec,
            path: Vec::new(),
            focus: vec![None],
            panels: Vec::new(),
            origin: (0.0, 0.0),
            theme: crate::vk_theme::current_theme(),
            status: crate::win::controller_center::pad_status(),
            update: crate::linux_update::available().map(|(v, _)| format!("v{v}")),
            gfx: Gfx::new(1, 1).ok(),
            widths: Vec::new(),
            ticks: 0,
        };
        if matches!(menu.spec.placement, Placement::Center(_)) {
            menu.focus[0] = step_selectable(&menu.spec.root, None, true);
        }
        self.place(&mut menu);
        MENU.with(|m| *m.borrow_mut() = Some(menu));
        crate::win::controller_center::set_menu_owns_pad(true);
    }

    fn close(&mut self) {
        close();
    }

    fn rescale(&mut self, scale: f32) {
        self.scale = scale;
        let this = &*self;
        with_menu(|m| this.place(m));
    }

    fn press(&mut self, x: f32, y: f32) -> Flow {
        let s = self.scale;
        let mut act = Act::Stay;
        with_menu(|m| {
            act = match hit(m, x * s, y * s) {
                (Hit::Row(level, i, row), hx, _) => {
                    hover(m, level, i);
                    click(m, level, i, row, hx)
                }
                (Hit::Panel, _, _) => Act::Stay,
                (Hit::Outside, _, _) => Act::Close(None),
            };
        });
        flow(act)
    }

    fn motion(&mut self, x: f32, y: f32) -> Flow {
        let s = self.scale;
        let mut changed = false;
        with_menu(|m| {
            if let (Hit::Row(level, i, _), _, _) = hit(m, x * s, y * s) {
                if m.focus.get(level).copied().flatten() != Some(i) {
                    hover(m, level, i);
                    changed = true;
                }
            }
        });
        if changed {
            Flow::Paint
        } else {
            Flow::Idle
        }
    }

    fn key(&mut self, key: Key) -> Flow {
        let Some(nav_key) = key.vk.and_then(nav_key) else {
            return Flow::Idle;
        };
        let mut act = Act::Stay;
        with_menu(|m| act = nav(m, nav_key));
        flow(act)
    }

    fn tick(&mut self) -> Flow {
        let mut result = Flow::Idle;
        with_menu(|m| {
            if tick(m) {
                result = Flow::Paint;
            }
        });
        for (button, pressed) in crate::win::controller_center::take_menu_pad_edges() {
            if !pressed {
                continue;
            }
            if let Some(key) = pad_key(button) {
                let mut act = Act::Stay;
                with_menu(|m| act = nav(m, key));
                match flow(act) {
                    Flow::Close => return Flow::Close,
                    f => result = f,
                }
            }
        }
        result
    }

    fn paint(&mut self, g: &mut Gfx, _scale: f32) {
        MENU.with(|m| {
            if let Ok(m) = m.try_borrow() {
                if let Some(m) = m.as_ref() {
                    unsafe { draw_menu(m, g) };
                }
            }
        });
    }
}

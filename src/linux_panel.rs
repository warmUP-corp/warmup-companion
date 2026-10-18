use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use smithay_client_toolkit::activation::{ActivationHandler, ActivationState, RequestData};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RepeatInfo};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer, BTN_LEFT,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shell::xdg::{XdgShell, XdgSurface};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_activation, delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};

use crate::mono_ui::Gfx;

const TICK: Duration = Duration::from_millis(33);
const IDLE_WAIT: Duration = Duration::from_millis(500);

const MOD_CTRL: u8 = 1;
const MOD_SHIFT: u8 = 2;
const MOD_ALT: u8 = 4;
const MOD_LOGO: u8 = 8;

static MODS: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mods {
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
    pub(crate) logo: bool,
}

pub(crate) fn mods() -> Mods {
    let m = MODS.load(Ordering::SeqCst);
    Mods {
        ctrl: m & MOD_CTRL != 0,
        shift: m & MOD_SHIFT != 0,
        alt: m & MOD_ALT != 0,
        logo: m & MOD_LOGO != 0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    pub(crate) vk: Option<u16>,
    pub(crate) text: Option<String>,
    pub(crate) mods: Mods,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    Idle,
    Paint,
    Close,
}

pub(crate) trait PanelApp: Send {
    fn size(&self) -> (u32, u32);
    fn open(&mut self, scale: f32);
    fn close(&mut self);
    fn rescale(&mut self, _scale: f32) {}
    fn caption(&mut self, _x: f32, _y: f32) -> bool {
        false
    }
    fn press(&mut self, x: f32, y: f32) -> Flow;
    fn motion(&mut self, _x: f32, _y: f32) -> Flow {
        Flow::Idle
    }
    fn release(&mut self) -> Flow {
        Flow::Idle
    }
    fn wheel(&mut self, _delta: i32) -> Flow {
        Flow::Idle
    }
    fn key(&mut self, key: Key) -> Flow;
    fn tick(&mut self) -> Flow;
    fn paint(&mut self, g: &mut Gfx, scale: f32);
}

pub(crate) struct Spec {
    pub(crate) thread: &'static str,
    pub(crate) title: &'static str,
    pub(crate) app_id: &'static str,
}

enum Cmd {
    Show,
    Hide,
    Quit,
}

pub(crate) struct PanelThread {
    tx: Sender<Cmd>,
    wake: UnixStream,
    join: Option<JoinHandle<()>>,
}

pub(crate) fn spawn(spec: Spec, app: Box<dyn PanelApp>) -> Result<PanelThread, String> {
    let (tx, rx) = mpsc::channel();
    let (wake_read, wake) = UnixStream::pair().map_err(|e| e.to_string())?;
    wake_read.set_nonblocking(true).map_err(|e| e.to_string())?;
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let name = spec.thread;
    let join = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || run(spec, app, rx, wake_read, ready_tx))
        .map_err(|e| format!("{name}: {e}"))?;
    ready_rx
        .recv()
        .map_err(|_| format!("{name} exited before ready"))??;
    Ok(PanelThread {
        tx,
        wake,
        join: Some(join),
    })
}

impl PanelThread {
    fn post(&self, cmd: Cmd) -> Result<(), String> {
        self.tx.send(cmd).map_err(|_| "panel thread is gone".to_string())?;
        std::io::Write::write_all(&mut &self.wake, &[1]).map_err(|e| e.to_string())
    }

    pub(crate) fn show(&self) -> Result<(), String> {
        self.post(Cmd::Show)
    }

    pub(crate) fn hide(&self) -> Result<(), String> {
        self.post(Cmd::Hide)
    }
}

impl Drop for PanelThread {
    fn drop(&mut self) {
        let _ = self.post(Cmd::Quit);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(
    spec: Spec,
    app: Box<dyn PanelApp>,
    rx: Receiver<Cmd>,
    wake: UnixStream,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    let setup = Connection::connect_to_env()
        .map_err(|e| format!("wayland: {e}"))
        .and_then(|conn| {
            let (globals, queue) =
                registry_queue_init::<Host>(&conn).map_err(|e| format!("wayland: {e}"))?;
            let host = Host::new(&conn, &globals, &queue.handle(), spec, app)?;
            Ok((conn, queue, host))
        });
    let (conn, mut queue, mut host) = match setup {
        Ok(v) => {
            let _ = ready.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let qh = queue.handle();
    loop {
        let mut quit = false;
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Show => host.show(&qh),
                Cmd::Hide => host.hide(),
                Cmd::Quit => quit = true,
            }
        }
        if quit {
            host.hide();
            break;
        }
        if let Err(e) = queue.dispatch_pending(&mut host) {
            crate::install::log_line(&format!("{}: wayland: {e}", host.spec.thread));
            break;
        }
        host.pump(&qh);
        if let Err(e) = queue.flush() {
            crate::install::log_line(&format!("{}: wayland: {e}", host.spec.thread));
            break;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let wait = host.wait();
        let (wayland_ready, wake_ready) = {
            let mut fds = [
                nix::poll::PollFd::new(guard.connection_fd(), nix::poll::PollFlags::POLLIN),
                nix::poll::PollFd::new(wake.as_fd(), nix::poll::PollFlags::POLLIN),
            ];
            let ms = wait.as_millis().min(u16::MAX as u128) as u16;
            let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(ms));
            (
                fds[0].any().unwrap_or(false),
                fds[1].any().unwrap_or(false),
            )
        };
        if wayland_ready {
            if let Err(e) = guard.read() {
                crate::install::log_line(&format!("{}: wayland: {e}", host.spec.thread));
                break;
            }
        } else {
            drop(guard);
        }
        if wake_ready {
            let mut buf = [0u8; 64];
            let _ = (&wake).read(&mut buf);
        }
    }
    host.app.close();
    drop(host);
    drop(queue);
    let _ = conn.flush();
}

struct Repeat {
    raw: u32,
    key: Key,
    next: Instant,
}

struct Host {
    spec: Spec,
    app: Box<dyn PanelApp>,
    conn: Connection,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    shm: Shm,
    activation: Option<ActivationState>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional_mgr: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    pool: Option<SlotPool>,
    window: Option<Window>,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    buffer: Option<Buffer>,
    gfx: Option<Gfx>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer>,
    seat: Option<wl_seat::WlSeat>,
    serial: Option<u32>,
    preferred: Option<f64>,
    int_scale: i32,
    scale: f32,
    configured: bool,
    opened: bool,
    dirty: bool,
    frame_pending: bool,
    next_tick: Instant,
    pointer_at: (f32, f32),
    repeat_info: Option<(Duration, Duration)>,
    repeat: Option<Repeat>,
}

impl Host {
    fn new(
        conn: &Connection,
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
        spec: Spec,
        app: Box<dyn PanelApp>,
    ) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|e| e.to_string())?;
        let xdg = XdgShell::bind(globals, qh).map_err(|e| format!("xdg shell: {e}"))?;
        let shm = Shm::bind(globals, qh).map_err(|e| e.to_string())?;
        Ok(Host {
            spec,
            app,
            conn: conn.clone(),
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            compositor,
            xdg,
            shm,
            activation: ActivationState::bind(globals, qh).ok(),
            viewporter: globals.bind(qh, 1..=1, ()).ok(),
            fractional_mgr: globals.bind(qh, 1..=1, ()).ok(),
            pool: None,
            window: None,
            viewport: None,
            fractional: None,
            buffer: None,
            gfx: None,
            keyboard: None,
            pointer: None,
            seat: None,
            serial: None,
            preferred: None,
            int_scale: 1,
            scale: 1.0,
            configured: false,
            opened: false,
            dirty: false,
            frame_pending: false,
            next_tick: Instant::now(),
            pointer_at: (0.0, 0.0),
            repeat_info: Some((Duration::from_millis(600), Duration::from_millis(40))),
            repeat: None,
        })
    }

    fn show(&mut self, qh: &QueueHandle<Self>) {
        if let Some(window) = &self.window {
            self.activate(qh, window.wl_surface().clone());
            self.dirty = true;
            return;
        }
        let (w, h) = self.app.size();
        let surface = self.compositor.create_surface(qh);
        let window = self
            .xdg
            .create_window(surface, WindowDecorations::RequestClient, qh);
        window.set_title(self.spec.title);
        window.set_app_id(self.spec.app_id);
        window.set_min_size(Some((w, h)));
        window.set_max_size(Some((w, h)));
        self.viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(window.wl_surface(), qh, ()));
        self.fractional = self
            .fractional_mgr
            .as_ref()
            .map(|m| m.get_fractional_scale(window.wl_surface(), qh, ()));
        window.commit();
        let surface = window.wl_surface().clone();
        self.window = Some(window);
        self.configured = false;
        self.opened = false;
        self.frame_pending = false;
        self.activate(qh, surface);
    }

    fn activate(&self, qh: &QueueHandle<Self>, surface: wl_surface::WlSurface) {
        if let Some(activation) = &self.activation {
            activation.request_token(
                qh,
                RequestData {
                    seat_and_serial: self.seat.clone().zip(self.serial),
                    surface: Some(surface),
                    app_id: Some(self.spec.app_id.to_string()),
                },
            );
        }
    }

    fn hide(&mut self) {
        self.repeat = None;
        if self.window.is_none() {
            return;
        }
        if self.opened {
            self.opened = false;
            self.app.close();
        }
        self.buffer = None;
        if let Some(f) = self.fractional.take() {
            f.destroy();
        }
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        self.window = None;
        self.pool = None;
        self.configured = false;
        self.frame_pending = false;
        self.dirty = false;
        let _ = self.conn.flush();
    }

    fn effective_scale(&self) -> f32 {
        match (self.preferred, &self.viewport) {
            (Some(s), Some(_)) => s.max(1.0) as f32,
            _ => self.int_scale.max(1) as f32,
        }
    }

    fn sync_scale(&mut self) {
        let next = self.effective_scale();
        if (next - self.scale).abs() > f32::EPSILON {
            self.scale = next;
            if self.opened {
                self.app.rescale(next);
            }
            self.dirty = true;
        }
    }

    fn apply(&mut self, flow: Flow) {
        match flow {
            Flow::Idle => {}
            Flow::Paint => self.dirty = true,
            Flow::Close => self.hide(),
        }
    }

    fn pump(&mut self, qh: &QueueHandle<Self>) {
        if !self.opened {
            return;
        }
        let now = Instant::now();
        if now >= self.next_tick {
            self.next_tick = now + TICK;
            let flow = self.app.tick();
            self.apply(flow);
        }
        if let Some(r) = &mut self.repeat {
            if let (true, Some((_, every))) = (now >= r.next, self.repeat_info) {
                r.next = now + every;
                let key = r.key.clone();
                let flow = self.app.key(key);
                self.apply(flow);
            }
        }
        if self.dirty && self.configured && !self.frame_pending {
            self.draw(qh);
        }
    }

    fn wait(&self) -> Duration {
        if !self.opened {
            return IDLE_WAIT;
        }
        let now = Instant::now();
        let mut at = self.next_tick;
        if let Some(r) = &self.repeat {
            at = at.min(r.next);
        }
        at.saturating_duration_since(now).max(Duration::from_millis(1))
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let Some(window) = &self.window else {
            return;
        };
        let (w, h) = self.app.size();
        let s = self.scale;
        let (pw, ph) = (
            (w as f32 * s).ceil() as u32,
            (h as f32 * s).ceil() as u32,
        );
        if self.gfx.is_none() {
            match Gfx::new(pw, ph) {
                Ok(g) => self.gfx = Some(g),
                Err(e) => {
                    crate::install::log_line(&format!("{}: gfx: {e}", self.spec.thread));
                    return;
                }
            }
        }
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        gfx.begin(pw, ph);
        self.app.paint(gfx, s);
        let stride = pw as i32 * 4;
        if self.pool.is_none() {
            match SlotPool::new((pw * ph * 4) as usize, &self.shm) {
                Ok(p) => self.pool = Some(p),
                Err(e) => {
                    crate::install::log_line(&format!("{}: shm: {e}", self.spec.thread));
                    return;
                }
            }
        }
        let Some(pool) = self.pool.as_mut() else {
            return;
        };
        let buffer = match self.buffer.take() {
            Some(b) if b.height() == ph as i32 && b.stride() == stride && pool.canvas(&b).is_some() => b,
            _ => match pool.create_buffer(pw as i32, ph as i32, stride, wl_shm::Format::Argb8888) {
                Ok((b, _)) => b,
                Err(_) => return,
            },
        };
        let Some(canvas) = pool.canvas(&buffer) else {
            return;
        };
        let pixels = gfx.pixels();
        let n = canvas.len().min(pixels.len());
        for (dst, src) in canvas[..n]
            .chunks_exact_mut(4)
            .zip(pixels[..n].chunks_exact(4))
        {
            dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
        }
        let surface = window.wl_surface();
        match &self.viewport {
            Some(viewport) => {
                surface.set_buffer_scale(1);
                viewport.set_destination(w as i32, h as i32);
            }
            None => surface.set_buffer_scale(s.round().max(1.0) as i32),
        }
        window.set_window_geometry(0, 0, w, h);
        surface.damage_buffer(0, 0, pw as i32, ph as i32);
        surface.frame(qh, surface.clone());
        if let Err(e) = buffer.attach_to(surface) {
            crate::install::log_line(&format!("{}: buffer: {e}", self.spec.thread));
            return;
        }
        window.commit();
        self.buffer = Some(buffer);
        self.frame_pending = true;
        self.dirty = false;
    }

    fn key_event(&mut self, event: &KeyEvent, pressed: bool) {
        if !self.opened {
            return;
        }
        if !pressed {
            if self.repeat.as_ref().is_some_and(|r| r.raw == event.raw_code) {
                self.repeat = None;
            }
            return;
        }
        let key = Key {
            vk: vk_of(event.keysym, event.raw_code),
            text: event.utf8.clone().filter(|t| !t.is_empty()),
            mods: mods(),
        };
        if key.vk.is_none() && key.text.is_none() {
            return;
        }
        let modifier = matches!(key.vk, Some(0x10..=0x12 | 0x5B | 0x5C));
        self.repeat = match self.repeat_info {
            Some((delay, _)) if !modifier => Some(Repeat {
                raw: event.raw_code,
                key: key.clone(),
                next: Instant::now() + delay,
            }),
            _ => None,
        };
        let flow = self.app.key(key);
        self.apply(flow);
    }
}

fn vk_of(sym: Keysym, raw: u32) -> Option<u16> {
    if let Some(c) = sym.key_char() {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        if c == ' ' {
            return Some(0x20);
        }
    }
    let vk = match sym {
        Keysym::Return | Keysym::KP_Enter => 0x0D,
        Keysym::Escape => 0x1B,
        Keysym::Tab | Keysym::ISO_Left_Tab => 0x09,
        Keysym::BackSpace => 0x08,
        Keysym::Delete | Keysym::KP_Delete => 0x2E,
        Keysym::Left | Keysym::KP_Left => 0x25,
        Keysym::Up | Keysym::KP_Up => 0x26,
        Keysym::Right | Keysym::KP_Right => 0x27,
        Keysym::Down | Keysym::KP_Down => 0x28,
        Keysym::Prior | Keysym::KP_Prior => 0x21,
        Keysym::Next | Keysym::KP_Next => 0x22,
        Keysym::End | Keysym::KP_End => 0x23,
        Keysym::Home | Keysym::KP_Home => 0x24,
        Keysym::Shift_L | Keysym::Shift_R => 0x10,
        Keysym::Control_L | Keysym::Control_R => 0x11,
        Keysym::Alt_L | Keysym::Alt_R => 0x12,
        Keysym::Super_L => 0x5B,
        Keysym::Super_R => 0x5C,
        s if (Keysym::F1.raw()..=Keysym::F12.raw()).contains(&s.raw()) => {
            0x70 + (s.raw() - Keysym::F1.raw()) as u16
        }
        _ => match raw {
            2..=10 => 0x31 + (raw - 2) as u16,
            11 => 0x30,
            _ => return None,
        },
    };
    Some(vk)
}

impl CompositorHandler for Host {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        self.int_scale = factor.max(1);
        self.sync_scale();
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if self.window.as_ref().map(|w| w.wl_surface()) != Some(surface) {
            return;
        }
        self.frame_pending = false;
        if self.dirty && self.configured {
            self.draw(qh);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        if self.preferred.is_none() {
            if let Some(info) = self.output_state.info(output) {
                self.int_scale = info.scale_factor.max(1);
                self.sync_scale();
            }
        }
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Host {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for Host {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.hide();
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &Window,
        _: WindowConfigure,
        _: u32,
    ) {
        self.configured = true;
        self.dirty = true;
        if !self.opened {
            self.opened = true;
            self.scale = self.effective_scale();
            self.next_tick = Instant::now() + TICK;
            self.app.open(self.scale);
        }
        if !self.frame_pending {
            self.draw(qh);
        }
    }
}

impl ActivationHandler for Host {
    type RequestData = RequestData;

    fn new_token(&mut self, token: String, data: &Self::RequestData) {
        let (Some(activation), Some(surface)) = (&self.activation, &data.surface) else {
            return;
        };
        if self
            .window
            .as_ref()
            .is_some_and(|w| w.wl_surface() == surface)
        {
            activation.activate::<Host>(surface, token);
        }
    }
}

impl SeatHandler for Host {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme(qh, &seat, self.shm.wl_shm(), surface, ThemeSpec::default())
                .ok();
        }
        self.seat = Some(seat);
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.pointer().release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for Host {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        serial: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.serial = Some(serial);
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.repeat = None;
        MODS.store(0, Ordering::SeqCst);
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        self.serial = Some(serial);
        self.key_event(&event, true);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key_event(&event, false);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        m: Modifiers,
        _: u32,
    ) {
        let bits = (m.ctrl as u8 * MOD_CTRL)
            | (m.shift as u8 * MOD_SHIFT)
            | (m.alt as u8 * MOD_ALT)
            | (m.logo as u8 * MOD_LOGO);
        MODS.store(bits, Ordering::SeqCst);
    }

    fn update_repeat_info(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        info: RepeatInfo,
    ) {
        self.repeat_info = match info {
            RepeatInfo::Repeat { rate, delay } => Some((
                Duration::from_millis(delay as u64),
                Duration::from_millis((1000 / rate.get().max(1)) as u64),
            )),
            RepeatInfo::Disable => None,
        };
        if self.repeat_info.is_none() {
            self.repeat = None;
        }
    }
}

impl PointerHandler for Host {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(window) = &self.window else {
            return;
        };
        let surface = window.wl_surface().clone();
        for event in events {
            if event.surface != surface || !self.opened {
                continue;
            }
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            self.pointer_at = (x, y);
            let flow = match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.serial = Some(serial);
                    if let Some(p) = &self.pointer {
                        let _ = p.set_cursor(conn, CursorIcon::Default);
                    }
                    Flow::Idle
                }
                PointerEventKind::Leave { .. } => Flow::Idle,
                PointerEventKind::Motion { .. } => self.app.motion(x, y),
                PointerEventKind::Press { button, serial, .. } if button == BTN_LEFT => {
                    self.serial = Some(serial);
                    if self.app.caption(x, y) {
                        if let (Some(w), Some(seat)) = (&self.window, &self.seat) {
                            w.move_(seat, serial);
                        }
                        Flow::Idle
                    } else {
                        self.app.press(x, y)
                    }
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    self.app.release()
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let steps = if vertical.discrete != 0 {
                        vertical.discrete
                    } else {
                        (vertical.absolute / 15.0).round() as i32
                    };
                    if steps == 0 {
                        Flow::Idle
                    } else {
                        self.app.wheel(-steps * 120)
                    }
                }
                _ => Flow::Idle,
            };
            self.apply(flow);
        }
    }
}

impl ShmHandler for Host {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Host {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Host);
delegate_output!(Host);
delegate_shm!(Host);
delegate_seat!(Host);
delegate_keyboard!(Host);
delegate_pointer!(Host);
delegate_xdg_shell!(Host);
delegate_xdg_window!(Host);
delegate_activation!(Host);
delegate_registry!(Host);

impl Dispatch<wp_viewporter::WpViewporter, ()> for Host {
    fn event(
        _: &mut Self,
        _: &wp_viewporter::WpViewporter,
        _: wp_viewporter::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_viewport::WpViewport, ()> for Host {
    fn event(
        _: &mut Self,
        _: &wp_viewport::WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()> for Host {
    fn event(
        _: &mut Self,
        _: &wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _: wp_fractional_scale_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Host {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.preferred = Some(scale as f64 / 120.0);
            state.sync_scale();
        }
    }
}

use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT};
use smithay_client_toolkit::seat::touch::TouchHandler;
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_touch, registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_pointer, wl_seat, wl_surface, wl_touch};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};

use crate::config::{vk_layout_mode, vk_style, VkLayoutMode, VkStyle};
use crate::mono_ui::Gfx;

#[path = "linux_vk_draw.rs"]
mod draw;
#[path = "linux_vk_gpu.rs"]
mod gpu;

static KEYBOARD_OPEN: AtomicBool = AtomicBool::new(false);
static KEYBOARD_GONE: AtomicBool = AtomicBool::new(false);
static OUTPUT_IS_TV: AtomicBool = AtomicBool::new(false);
const SHEET_HINT_FOR: Duration = Duration::from_secs(3);
const SHOW_SLIDE_MS: f32 = 180.0;
const HIDE_SLIDE_MS: f32 = 130.0;
const TIPS_W: u32 = 1240;
const TIPS_H: u32 = 180;
const TIPS_MARGIN_BOTTOM: i32 = 72;
const TIPS_FRAME_MS: u16 = 16;
static OVERLAY_STARTED: AtomicBool = AtomicBool::new(false);
const PROMPT_BG: u32 = 0x00141414;
const PROMPT_BORDER: u32 = 0x00997b4c;
const PROMPT_TEXT: u32 = 0x00FFFFFF;

pub fn look() -> crate::vk_paint::VkLook {
    let tv = match crate::config::vk_display() {
        crate::config::VkDisplay::Tv => true,
        crate::config::VkDisplay::Desk => false,
        crate::config::VkDisplay::Auto => OUTPUT_IS_TV.load(Ordering::SeqCst),
    };
    crate::vk_paint::VkLook { style: vk_style(), tv }
}

pub fn keyboard_open() -> bool {
    KEYBOARD_OPEN.load(Ordering::SeqCst)
}

pub fn keyboard_gone() -> bool {
    KEYBOARD_GONE.load(Ordering::SeqCst)
}

#[cfg(feature = "speech")]
pub fn paint_voice_orb(
    g: &mut Gfx,
    side: u32,
    slot: f32,
    phase: u8,
    level: f32,
    t: f32,
    nimbus: Option<(&[u8], u32)>,
    accent: u32,
) -> bool {
    draw::paint_voice_orb(g, side, slot, draw::Voice { phase, level, t }, nimbus, accent)
}

#[cfg(feature = "speech")]
pub fn voice_accent() -> u32 {
    let look = crate::vk_paint::VkLook { style: vk_style(), tv: false };
    draw::themed_palette(look, crate::vk_theme::is_dark_theme(), &crate::config::keyboard_theme()).accent
}

#[cfg(feature = "speech")]
pub fn paint_voice_screen(g: &mut Gfx, width: u32, height: u32, phase: u8, t: f32, accent: u32) -> bool {
    draw::paint_voice_screen(g, width, height, draw::Voice { phase, level: 0.0, t }, accent)
}

#[cfg(feature = "speech")]
pub fn draw_argb(pixels: &[u8]) -> Vec<u8> {
    draw::to_argb(pixels)
}

static STOP: AtomicBool = AtomicBool::new(false);
static WAKE: Mutex<Option<UnixStream>> = Mutex::new(None);

pub struct Overlay {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Overlay {
    pub fn open(lock_screen: bool) -> Result<Self, String> {
        if lock_screen {
            crate::install::log_line(
                "the lock screen cannot show the keyboard; opening it on the user session",
            );
        }
        crate::vk_nav::reset_selection();
        STOP.store(false, Ordering::SeqCst);
        KEYBOARD_GONE.store(false, Ordering::SeqCst);
        let (wake_read, wake_write) = UnixStream::pair().map_err(|error| error.to_string())?;
        wake_read.set_nonblocking(true).map_err(|error| error.to_string())?;
        if let Ok(mut slot) = WAKE.lock() {
            *slot = Some(wake_write);
        }
        let thread = std::thread::Builder::new()
            .name("warmup-vk".into())
            .spawn(move || run_keyboard(wake_read))
            .map_err(|error| error.to_string())?;
        Ok(Self { thread: Some(thread) })
    }

    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        STOP.store(true, Ordering::SeqCst);
        poke();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut slot) = WAKE.lock() {
            *slot = None;
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn request_repaint() {
    poke();
}

fn poke() {
    let Ok(mut slot) = WAKE.lock() else { return };
    if let Some(wake) = slot.as_mut() {
        let _ = std::io::Write::write_all(wake, &[1]);
    }
}

pub fn show_prompt_once() {
    if !crate::config::prompt_overlay_enabled() || OVERLAY_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("warmup-prompt-overlay".into())
        .spawn(run_overlay)
        .ok();
}

fn run_keyboard(wake: UnixStream) {
    if let Err(error) = keyboard_loop(wake) {
        crate::install::log_line(&format!("on-screen keyboard: {error}"));
    }
    if !STOP.load(Ordering::SeqCst) {
        crate::install::log_line("on-screen keyboard closed on its own");
        KEYBOARD_GONE.store(true, Ordering::SeqCst);
    }
}

fn keyboard_loop(wake: UnixStream) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Shell>(&conn).map_err(|error| error.to_string())?;
    let qh = queue.handle();
    let display = conn.display().id().as_ptr() as *mut std::ffi::c_void;
    let mut shell = Shell::new(&globals, &qh, display, conn.clone())?;
    KEYBOARD_OPEN.store(true, Ordering::SeqCst);
    struct KeyboardShown;
    impl Drop for KeyboardShown {
        fn drop(&mut self) {
            KEYBOARD_OPEN.store(false, Ordering::SeqCst);
        }
    }
    let _keyboard_shown = KeyboardShown;
    shell.layer.commit();
    let mut wayland_dead = false;
    loop {
        if STOP.load(Ordering::SeqCst) || shell.exit {
            break;
        }
        if let Err(error) = queue.dispatch_pending(&mut shell) {
            crate::install::log_line(&format!("keyboard wayland: {error}"));
            wayland_dead = true;
            break;
        }
        #[cfg(feature = "speech")]
        if shell.configured {
            let gen = crate::linux_nimbus::frame_generation();
            let active = crate::linux_speech::visual_active();
            if active && (gen != shell.voice_gen || !shell.voice_on || !crate::linux_nimbus::has_frame()) {
                shell.voice_gen = gen;
                shell.voice_on = true;
                shell.dirty = true;
            } else if !active && shell.voice_on {
                shell.voice_gen = 0;
                shell.voice_on = false;
                shell.dirty = true;
            }
        }
        let hint = shell.sheet_hint(shell.look());
        if shell.configured && shell.hint_shown != hint {
            shell.hint_shown = hint;
            shell.dirty = true;
        }
        if shell.configured && shell.dark != crate::vk_theme::is_dark_theme() {
            shell.dark = !shell.dark;
            shell.dirty = true;
        }
        if shell.dirty && shell.configured && !shell.frame_pending {
            shell.draw(&qh);
        }
        if let Err(error) = queue.flush() {
            crate::install::log_line(&format!("keyboard wayland: {error}"));
            wayland_dead = true;
            break;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let (wayland_ready, wake_ready) = {
            let mut fds = [
                nix::poll::PollFd::new(guard.connection_fd(), nix::poll::PollFlags::POLLIN),
                nix::poll::PollFd::new(wake.as_fd(), nix::poll::PollFlags::POLLIN),
            ];
            #[cfg(feature = "speech")]
            let wait = if crate::linux_speech::visual_active() { 33u16 } else { 500 };
            #[cfg(not(feature = "speech"))]
            let wait = 500u16;
            let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(wait));
            (
                fds[0].any().unwrap_or(false),
                fds[1].any().unwrap_or(false),
            )
        };
        if wayland_ready && guard.read().is_err() {
            crate::install::log_line("keyboard wayland: read failed");
            wayland_dead = true;
            break;
        }
        if wake_ready {
            let mut buf = [0u8; 32];
            let _ = (&wake).read(&mut buf);
            shell.dirty = true;
        }
    }
    if !wayland_dead && !shell.exit && shell.configured && (shell.gpu_presented || shell.buffer.is_some()) {
        wayland_dead = !slide_out(&mut shell, &mut queue, &qh);
    }
    shell.gpu.take();
    if !wayland_dead {
        if let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(proxy) = shell.layer.kind() {
            proxy.destroy();
        }
        let _ = queue.roundtrip(&mut shell);
    }
    drop(shell);
    drop(queue);
    let _ = conn.flush();
    Ok(())
}

fn slide_out(shell: &mut Shell, queue: &mut wayland_client::EventQueue<Shell>, qh: &QueueHandle<Shell>) -> bool {
    let started = std::time::Instant::now();
    let deadline = started + Duration::from_millis(HIDE_SLIDE_MS as u64 + 120);
    shell.slide = Some((started, true));
    shell.slide_waiting = false;
    shell.dirty = true;
    loop {
        if queue.dispatch_pending(shell).is_err() {
            return false;
        }
        if !shell.slide_hidden().1 || std::time::Instant::now() >= deadline {
            return true;
        }
        if shell.dirty && !shell.frame_pending {
            shell.draw(qh);
        }
        if queue.flush().is_err() {
            return false;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [nix::poll::PollFd::new(guard.connection_fd(), nix::poll::PollFlags::POLLIN)];
        let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(16u16));
        if fds[0].any().unwrap_or(false) && guard.read().is_err() {
            return false;
        }
    }
}

struct Shell {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    pointer: Option<wl_pointer::WlPointer>,
    touch: Option<wl_touch::WlTouch>,
    shm: Shm,
    pool: SlotPool,
    compositor: CompositorState,
    layer_shell: LayerShell,
    viewporter: Option<wp_viewporter::WpViewporter>,
    scale_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    layer: LayerSurface,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<f64>,
    output: Option<wl_output::WlOutput>,
    logical_w: u32,
    logical_h: u32,
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    exit: bool,
    gfx: Gfx,
    buffer: Option<Buffer>,
    ps5: bool,
    glide: draw::Glide,
    placed: Option<(bool, u32, u32, i32)>,
    display: *mut std::ffi::c_void,
    conn: Connection,
    gpu: Option<gpu::Presenter>,
    gpu_failed: bool,
    gpu_presented: bool,
    gpu_errors: u32,
    dark: bool,
    opened_at: std::time::Instant,
    hint_shown: bool,
    voice_gen: u64,
    voice_on: bool,
    voice_level: f32,
    voice_level_at: Option<std::time::Instant>,
    voice_started: Option<std::time::Instant>,
    slide: Option<(std::time::Instant, bool)>,
    slide_waiting: bool,
}

impl Shell {
    fn new(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
        display: *mut std::ffi::c_void,
        conn: Connection,
    ) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|error| error.to_string())?;
        let layer_shell = LayerShell::bind(globals, qh).map_err(|_| {
            "this compositor has no layer shell, so there is no on-screen keyboard".to_string()
        })?;
        let shm = Shm::bind(globals, qh).map_err(|error| error.to_string())?;
        let viewporter = globals.bind::<wp_viewporter::WpViewporter, _, _>(qh, 1..=1, ()).ok();
        let scale_manager = globals
            .bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(qh, 1..=1, ())
            .ok();
        let (layer, viewport) = open_layer(
            &compositor,
            &layer_shell,
            viewporter.as_ref(),
            scale_manager.as_ref(),
            qh,
        );
        let pool = SlotPool::new(1920 * 400 * 4, &shm).map_err(|error| error.to_string())?;
        let mut shell = Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            seat_state: SeatState::new(globals, qh),
            pointer: None,
            touch: None,
            shm,
            pool,
            compositor,
            layer_shell,
            viewporter,
            scale_manager,
            layer,
            viewport,
            fractional: None,
            output: None,
            logical_w: 0,
            logical_h: 0,
            configured: false,
            dirty: true,
            frame_pending: false,
            exit: false,
            gfx: Gfx::new(1, 1)?,
            buffer: None,
            ps5: ps5_pad(),
            glide: draw::Glide::default(),
            placed: None,
            display,
            conn,
            gpu: None,
            gpu_failed: false,
            gpu_presented: false,
            gpu_errors: 0,
            dark: crate::vk_theme::is_dark_theme(),
            opened_at: std::time::Instant::now(),
            hint_shown: false,
            voice_gen: 0,
            voice_on: false,
            voice_level: 0.0,
            voice_level_at: None,
            voice_started: None,
            slide: None,
            slide_waiting: false,
        };
        shell.apply_layout();
        Ok(shell)
    }

    fn apply_layout(&mut self) {
        let (output_w, output_h) = self.output_size();
        let keyboard_docked = matches!(vk_layout_mode(), VkLayoutMode::Docked);
        let (width, height) = desired_size(output_w, output_h, self.look());
        let docked = keyboard_docked;
        let margin = 0;
        let key = (docked, width, height, margin);
        if self.placed == Some(key) {
            return;
        }
        self.placed = Some(key);
        self.logical_w = width;
        self.logical_h = height;
        if docked {
            self.layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            self.layer.set_exclusive_zone(height as i32);
            self.layer.set_margin(0, 0, 0, 0);
            self.layer.set_size(0, height);
        } else {
            self.layer.set_anchor(Anchor::BOTTOM);
            self.layer.set_exclusive_zone(0);
            self.layer.set_margin(0, 0, margin, 0);
            self.layer.set_size(width, height);
        }
    }

    fn output_size(&self) -> (f32, f32) {
        let Some(output) = &self.output else {
            return (1920.0, 1080.0);
        };
        let Some(info) = self.output_state.info(output) else {
            return (1920.0, 1080.0);
        };
        if let Some((width, height)) = info.logical_size {
            if width > 0 && height > 0 {
                return (width as f32, height as f32);
            }
        }
        let (pixels_w, pixels_h) = info
            .modes
            .iter()
            .find(|mode| mode.current)
            .map(|mode| mode.dimensions)
            .unwrap_or((1920, 1080));
        let scale = info.scale_factor.max(1) as f32;
        (pixels_w as f32 / scale, pixels_h as f32 / scale)
    }

    fn fresh_surface(&mut self, qh: &QueueHandle<Self>) {
        let (layer, viewport) = open_layer(
            &self.compositor,
            &self.layer_shell,
            self.viewporter.as_ref(),
            self.scale_manager.as_ref(),
            qh,
        );
        if let Some(old) = self.viewport.take() {
            old.destroy();
        }
        let old = std::mem::replace(&mut self.layer, layer);
        if let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(proxy) = old.kind() {
            proxy.destroy();
        }
        drop(old);
        self.viewport = viewport;
        self.buffer = None;
        self.configured = false;
        self.frame_pending = false;
        self.placed = None;
        self.dirty = true;
        self.apply_layout();
        self.layer.commit();
    }

    fn sheet_hint(&self, look: crate::vk_paint::VkLook) -> bool {
        look.tv_layout()
            && self.opened_at.elapsed() < SHEET_HINT_FOR
            && crate::config::gamepad_settings().vk_side_tips
            && !crate::win::shortcut_sheet::hint_retired()
    }

    fn look(&self) -> crate::vk_paint::VkLook {
        if let Some(info) = self.output.as_ref().and_then(|output| self.output_state.info(output)) {
            OUTPUT_IS_TV.store(crate::vk_theme::is_tv(info.physical_size.0, info.physical_size.1), Ordering::SeqCst);
        }
        look()
    }

    fn buffer_scale(&self) -> f64 {
        if let Some(scale) = self.fractional {
            return scale.max(1.0);
        }
        let Some(output) = &self.output else {
            return 1.0;
        };
        self.output_state
            .info(output)
            .map(|info| info.scale_factor.max(1) as f64)
            .unwrap_or(1.0)
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        self.apply_layout();
        let scale = self.buffer_scale() as f32;
        let rows = crate::vk_nav::rows_snapshot();
        let look = self.look();
        let strip = crate::vk_predict::strip(
            crate::vk_paint::strip_slots(look),
            look.style == VkStyle::Mono,
        );
        let pressed = crate::vk_nav::press_feedback().map(|(pos, at)| {
            (
                pos,
                crate::vk_motion::press_scale(at.elapsed().as_secs_f32() * 1000.0),
            )
        });
        let (shift, caps, symbol, symbol_locked) = crate::vk_nav::modifier_state();
        let floating = !matches!(vk_layout_mode(), VkLayoutMode::Docked);
        let dark = crate::vk_theme::is_dark_theme();
        let theme = crate::config::keyboard_theme();
        let (scale_w, ui) = self.key_scales(look, &rows);
        let voice = self.voice_state();
        if self.slide.is_none() && !self.gpu_presented && self.buffer.is_none() {
            self.slide_waiting = true;
        }
        let (hidden, sliding) = self.slide_hidden();
        let slide = (hidden * self.logical_h as f32 * scale).round();
        let frame = draw::Frame {
            logical_w: self.logical_w as f32,
            logical_h: self.logical_h as f32,
            scale,
            scale_w,
            ui,
            rows,
            sel: crate::vk_nav::selection(),
            style: look.style,
            tv: look.tv,
            dark,
            pal: draw::themed_palette(look, dark, &theme),
            voice_available: crate::config::gamepad_settings().voice_enabled && speech_ready(),
            theme,
            strip,
            pressed,
            ps5: self.ps5,
            shift,
            caps,
            symbol,
            symbol_locked,
            floating,
            sheet: crate::vk_sheet::shown(),
            sheet_hint: self.sheet_hint(look),
            voice,
            slide,
        };
        if self.logical_w == 0 || self.logical_h == 0 {
            return;
        }
        if !self.gpu_failed {
            match self.present_gpu(qh, &frame) {
                Ok(()) => {
                    self.gpu_errors = 0;
                    self.gpu_presented = true;
                    self.dirty |= sliding;
                    return;
                }
                Err(error) => {
                    crate::install::log_line(&format!("keyboard gpu: {error}"));
                    self.gpu = None;
                    if !self.gpu_presented {
                        crate::install::log_line("keyboard: drawing in software on a new surface");
                        self.gpu_failed = true;
                        self.fresh_surface(qh);
                        return;
                    }
                    self.gpu_errors += 1;
                    if self.gpu_errors >= 3 {
                        crate::install::log_line("keyboard closed: the GPU keeps failing");
                        self.exit = true;
                    } else {
                        self.dirty = true;
                    }
                    return;
                }
            }
        }
        draw::paint_frame(&mut self.gfx, &frame, &mut self.glide);
        if frame.voice.is_some() {
            let nimbus = nimbus_frame();
            draw::paint_voice_key(&mut self.gfx, &frame, &self.glide, nimbus.as_ref().map(|(px, bytes)| (bytes.as_slice(), *px)));
        }
        let mut pixels = draw::to_argb(&self.gfx.pixels());
        let buffer_w = self.gfx.width() as i32;
        let buffer_h = self.gfx.height() as i32;
        draw::slide_down(&mut pixels, buffer_w as u32, slide as u32);
        let stride = buffer_w * 4;
        let Ok((buffer, canvas)) = self.pool.create_buffer(
            buffer_w,
            buffer_h,
            stride,
            wayland_client::protocol::wl_shm::Format::Argb8888,
        ) else {
            return;
        };
        let copy = pixels.len().min(canvas.len());
        canvas[..copy].copy_from_slice(&pixels[..copy]);
        let surface = self.layer.wl_surface();
        if let Some(viewport) = &self.viewport {
            let _ = surface.set_buffer_scale(1);
            viewport.set_destination(self.logical_w as i32, self.logical_h as i32);
        } else {
            let _ = surface.set_buffer_scale(self.buffer_scale().round().max(1.0) as i32);
        }
        surface.damage_buffer(0, 0, buffer_w, buffer_h);
        surface.frame(qh, surface.clone());
        self.frame_pending = true;
        self.dirty = self.glide.animating || sliding;
        if let Err(error) = buffer.attach_to(surface) {
            crate::install::log_line(&format!("keyboard buffer: {error}"));
            return;
        }
        self.buffer = Some(buffer);
        self.layer.commit();
    }

    fn present_gpu(&mut self, qh: &QueueHandle<Self>, frame: &draw::Frame) -> Result<(), String> {
        let _ = self.conn.flush();
        if self.gpu.is_none() {
            let surface = self.layer.wl_surface().id().as_ptr() as *mut std::ffi::c_void;
            self.gpu = Some(gpu::Presenter::open(self.display, surface)?);
        }
        let spot = draw::voice_key(frame);
        let nimbus = spot.as_ref().and_then(|_| nimbus_frame());
        let fallback = match (&spot, &nimbus) {
            (Some(spot), None) => draw::paint_voice_fallback(&mut self.gfx, frame, spot),
            _ => None,
        };
        let orb = spot.map(|spot| match (&nimbus, &fallback) {
            (Some((px, bytes)), _) => gpu::Orb {
                image: Some((bytes.as_slice(), *px)),
                ball: spot.ball,
                spot,
            },
            (None, Some((px, bytes, ball))) => gpu::Orb {
                image: Some((bytes.as_slice(), *px)),
                ball: *ball,
                spot,
            },
            (None, None) => gpu::Orb { image: None, ball: spot.ball, spot },
        });
        self.queue_gpu_frame(qh);
        let presenter = self.gpu.as_mut().ok_or("keyboard gpu is gone")?;
        presenter.present(frame, &mut self.gfx, &mut self.glide, orb.as_ref())?;
        self.frame_pending = true;
        self.dirty = self.glide.animating;
        Ok(())
    }

    fn key_scales(&self, look: crate::vk_paint::VkLook, rows: &[crate::vk_nav::KeyRow]) -> (f32, f32) {
        let layout = draw::layout_scale(look, self.output_size().1, rows);
        (
            draw::REF_MON_W * layout,
            draw::ui_scale() * layout / draw::design_scale(look.style),
        )
    }

    fn slide_hidden(&mut self) -> (f32, bool) {
        if self.slide_waiting {
            return (1.0, true);
        }
        let Some((at, hiding)) = self.slide else {
            return (0.0, false);
        };
        let total = if hiding { HIDE_SLIDE_MS } else { SHOW_SLIDE_MS };
        let t = at.elapsed().as_secs_f32() * 1000.0 / total;
        let eased = crate::vk_motion::ease_out_cubic(t);
        if hiding {
            (eased, t < 1.0)
        } else {
            if t >= 1.0 {
                self.slide = None;
            }
            (1.0 - eased, t < 1.0)
        }
    }

    fn click(&mut self, surface: &wl_surface::WlSurface, x: f32, y: f32) {
        if surface != self.layer.wl_surface() || !self.configured {
            return;
        }
        let look = self.look();
        if crate::vk_sheet::shown() && look.tv_layout() {
            return;
        }
        let rows = crate::vk_nav::rows_snapshot();
        let (scale_w, ui) = self.key_scales(look, &rows);
        match draw::hit_test(self.logical_w as f32, self.logical_h as f32, scale_w, ui, &rows, look, x, y) {
            Some(draw::Hit::Slot(slot)) => {
                let mut sink = crate::vk_nav::SendInputSink;
                if crate::vk_predict::commit_slot(slot, &mut sink).is_some() {
                    crate::vk_nav::after_insert();
                }
            }
            Some(draw::Hit::Key(pos, key)) => crate::vk_nav::activate_at(pos, &key),
            None => return,
        }
        self.dirty = true;
    }

    fn voice_state(&mut self) -> Option<draw::Voice> {
        #[cfg(feature = "speech")]
        if crate::linux_speech::visual_active() {
            let now = std::time::Instant::now();
            let started = *self.voice_started.get_or_insert(now);
            let target = crate::linux_speech::level();
            let dt_ms = self
                .voice_level_at
                .map(|at| now.duration_since(at).as_secs_f32() * 1000.0)
                .unwrap_or(0.0);
            self.voice_level_at = Some(now);
            self.voice_level = if dt_ms > 0.0 {
                crate::vk_motion::smooth_level(self.voice_level, target, dt_ms)
            } else {
                target
            };
            return Some(draw::Voice {
                phase: crate::linux_speech::phase(),
                level: self.voice_level,
                t: now.duration_since(started).as_secs_f32(),
            });
        }
        self.voice_started = None;
        self.voice_level_at = None;
        self.voice_level = 0.0;
        None
    }

    fn queue_gpu_frame(&self, qh: &QueueHandle<Self>) {
        let surface = self.layer.wl_surface();
        if let Some(viewport) = &self.viewport {
            let _ = surface.set_buffer_scale(1);
            viewport.set_destination(self.logical_w as i32, self.logical_h as i32);
        } else {
            let _ = surface.set_buffer_scale(self.buffer_scale().round().max(1.0) as i32);
        }
        surface.frame(qh, surface.clone());
        let _ = self.conn.flush();
    }
}

impl SeatHandler for Shell {
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
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
        if capability == Capability::Touch && self.touch.is_none() {
            self.touch = self.seat_state.get_touch(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
        if capability == Capability::Touch {
            if let Some(touch) = self.touch.take() {
                touch.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for Shell {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let PointerEventKind::Press { button, .. } = event.kind {
                if button == BTN_LEFT {
                    self.click(&event.surface, event.position.0 as f32, event.position.1 as f32);
                }
            }
        }
    }
}

impl TouchHandler for Shell {
    fn down(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _: u32,
        _: u32,
        surface: wl_surface::WlSurface,
        _: i32,
        position: (f64, f64),
    ) {
        self.click(&surface, position.0 as f32, position.1 as f32);
    }

    fn up(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, _: u32, _: i32) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, _: i32, _: (f64, f64)) {}

    fn shape(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64, _: f64) {}

    fn orientation(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64) {}

    fn cancel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch) {}
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.gpu.take();
    }
}

fn open_layer(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    viewporter: Option<&wp_viewporter::WpViewporter>,
    scale_manager: Option<&wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    qh: &QueueHandle<Shell>,
) -> (LayerSurface, Option<wp_viewport::WpViewport>) {
    let surface = compositor.create_surface(qh);
    let layer = layer_shell.create_layer_surface(
        qh,
        surface,
        Layer::Overlay,
        Some("warmup-companion-keyboard"),
        None,
    );
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    let viewport = viewporter.map(|porter| porter.get_viewport(layer.wl_surface(), qh, ()));
    if let Some(manager) = scale_manager {
        let _ = manager.get_fractional_scale(layer.wl_surface(), qh, ());
    }
    (layer, viewport)
}

fn desired_size(output_w: f32, output_h: f32, look: crate::vk_paint::VkLook) -> (u32, u32) {
    let rows = crate::vk_nav::rows_snapshot();
    let docked = matches!(vk_layout_mode(), VkLayoutMode::Docked);
    draw::card_size(output_w, output_h, docked, &rows, look)
}

fn nimbus_frame() -> Option<(u32, Vec<u8>)> {
    #[cfg(feature = "speech")]
    {
        crate::linux_nimbus::latest_frame()
    }
    #[cfg(not(feature = "speech"))]
    {
        None
    }
}

fn speech_ready() -> bool {
    #[cfg(feature = "speech")]
    {
        crate::linux_speech::available()
    }
    #[cfg(not(feature = "speech"))]
    {
        false
    }
}

fn ps5_pad() -> bool {
    std::fs::read_to_string("/proc/bus/input/devices")
        .unwrap_or_default()
        .contains("DualSense")
        || std::fs::read_to_string("/proc/bus/input/devices")
            .unwrap_or_default()
            .contains("Wireless Controller")
}

fn run_overlay() {
    let mut state = crate::prompt_state::PromptState::default();
    let tips_file = crate::paths::state_dir().join("controller-tips.version");
    let mut layer: Option<(Connection, wayland_client::EventQueue<Tips>, Tips)> = None;
    let log = |line: &str| crate::install::log_line(line);
    loop {
        if let Some(now) = state.tick_due() {
            let snapshot = crate::debug_state::snapshot();
            let voice = voice_phase_name();
            let inputs = crate::prompt_state::TickInputs {
                connected: snapshot.connected,
                vk_open: keyboard_open(),
                voice,
                default_desktop: true,
                on_winlogon: false,
                userland_debug: false,
            };
            let visual = state.visual(
                now,
                &inputs,
                || false,
                || crate::pipe_server::game_active() || crate::linux_game_detect::standalone_game_active_cached(),
                &tips_file,
                &log,
            );
            let cue = visual.and_then(|visual| visual.tip_index());
            match (cue, layer.as_mut()) {
                (Some(index), Some((_, _, tips))) => tips.show_cue(index, now),
                (Some(index), None) => match open_tips(index, &snapshot.name) {
                    Ok(opened) => {
                        log("prompt ui: shown (controller tips)");
                        layer = Some(opened);
                    }
                    Err(error) => log(&format!("prompt ui: controller tips: {error}")),
                },
                (None, Some((_, _, tips))) => tips.hide(now),
                (None, None) => {}
            }
        }
        let Some((conn, queue, tips)) = layer.as_mut() else {
            std::thread::sleep(crate::prompt_state::TICK_INTERVAL);
            continue;
        };
        if !pump_tips(conn, queue, tips) || tips.gone() {
            if let Some((conn, mut queue, mut tips)) = layer.take() {
                if let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(proxy) = tips.layer.kind() {
                    proxy.destroy();
                }
                let _ = queue.roundtrip(&mut tips);
                drop(tips);
                drop(queue);
                let _ = conn.flush();
                log("prompt ui: hidden");
            }
        }
    }
}

fn voice_phase_name() -> Option<&'static str> {
    #[cfg(feature = "speech")]
    {
        if !crate::linux_speech::visual_active() {
            return None;
        }
        Some(match crate::linux_speech::phase() {
            2 => "transcribing",
            1 => "listening",
            _ => "starting",
        })
    }
    #[cfg(not(feature = "speech"))]
    {
        None
    }
}

fn open_tips(
    index: usize,
    name: &str,
) -> Result<(Connection, wayland_client::EventQueue<Tips>, Tips), String> {
    let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, queue) = registry_queue_init::<Tips>(&conn).map_err(|error| error.to_string())?;
    let qh = queue.handle();
    let tips = Tips::new(&globals, &qh, index, name)?;
    tips.layer.commit();
    Ok((conn, queue, tips))
}

fn pump_tips(conn: &Connection, queue: &mut wayland_client::EventQueue<Tips>, tips: &mut Tips) -> bool {
    let qh = queue.handle();
    let deadline = std::time::Instant::now() + crate::prompt_state::TICK_INTERVAL;
    while std::time::Instant::now() < deadline && !tips.gone() {
        if queue.dispatch_pending(tips).is_err() {
            return false;
        }
        if tips.exit {
            return false;
        }
        if tips.configured && (!tips.frame_pending || tips.frame_stale()) {
            tips.draw(&qh);
        }
        if conn.flush().is_err() {
            return false;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [nix::poll::PollFd::new(guard.connection_fd(), nix::poll::PollFlags::POLLIN)];
        let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(TIPS_FRAME_MS));
        if fds[0].any().unwrap_or(false) && guard.read().is_err() {
            return false;
        }
    }
    true
}

struct Tips {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    exit: bool,
    configured: bool,
    frame_pending: bool,
    frame_at: std::time::Instant,
    scale: i32,
    gfx: Gfx,
    buffer: Option<Buffer>,
    controller_label: String,
    started: std::time::Instant,
    shown_at: std::time::Instant,
    hide_at: Option<std::time::Instant>,
    cue: usize,
    morph: Option<(usize, std::time::Instant)>,
}

impl Tips {
    fn new(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
        cue: usize,
        name: &str,
    ) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|error| error.to_string())?;
        let layer_shell = LayerShell::bind(globals, qh)
            .map_err(|_| "this compositor has no layer shell, so the controller tips are not shown".to_string())?;
        let shm = Shm::bind(globals, qh).map_err(|error| error.to_string())?;
        let surface = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("warmup-companion-tips"),
            None,
        );
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_anchor(Anchor::BOTTOM);
        layer.set_exclusive_zone(0);
        layer.set_margin(0, 0, TIPS_MARGIN_BOTTOM, 0);
        layer.set_size(TIPS_W, TIPS_H);
        if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&compositor) {
            layer.wl_surface().set_input_region(Some(region.wl_region()));
        }
        let pool = SlotPool::new((TIPS_W * TIPS_H * 4 * 4) as usize, &shm).map_err(|error| error.to_string())?;
        let name = name.trim();
        let controller_label = if name.is_empty() || name.eq_ignore_ascii_case("none") {
            "Xbox Wireless Controller".to_string()
        } else {
            name.to_string()
        };
        let now = std::time::Instant::now();
        Ok(Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            shm,
            pool,
            layer,
            exit: false,
            configured: false,
            frame_pending: false,
            frame_at: now,
            scale: 1,
            gfx: Gfx::new(1, 1)?,
            buffer: None,
            controller_label,
            started: now,
            shown_at: now,
            hide_at: None,
            cue,
            morph: None,
        })
    }

    fn show_cue(&mut self, index: usize, now: std::time::Instant) {
        if self.hide_at.take().is_some() {
            self.shown_at = now;
        }
        if index != self.cue {
            self.morph = Some((self.cue, now));
            self.cue = index;
        }
    }

    fn hide(&mut self, now: std::time::Instant) {
        if self.hide_at.is_none() {
            self.hide_at = Some(now);
        }
    }

    fn gone(&self) -> bool {
        self.hide_at
            .is_some_and(|at| at.elapsed().as_secs_f32() * 1000.0 >= crate::vk_motion::VOICE_EXIT_MS)
    }

    fn frame_stale(&self) -> bool {
        self.frame_at.elapsed() > Duration::from_millis(250)
    }

    fn transition(&self, now: std::time::Instant) -> (f32, f32) {
        let ms = |at: std::time::Instant| now.saturating_duration_since(at).as_secs_f32() * 1000.0;
        let (mut alpha, scale) = crate::vk_motion::voice_enter(ms(self.shown_at));
        if let Some(at) = self.hide_at {
            alpha *= crate::vk_motion::voice_exit(ms(at));
        }
        (alpha, scale)
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let now = std::time::Instant::now();
        let (alpha, scale) = self.transition(now);
        let morph = self.morph.and_then(|(from, start)| {
            (crate::prompt_state::raw_morph_progress(start, now) < 1.0)
                .then(|| (from, crate::prompt_state::eased_morph_progress(start, now)))
        });
        if morph.is_none() {
            self.morph = None;
        }
        let theme = crate::config::keyboard_theme();
        let pill = crate::vk_paint::TipsPill {
            bg: theme.bg.unwrap_or(PROMPT_BG),
            border: theme.border.or(theme.accent).unwrap_or(PROMPT_BORDER),
            text: theme.text.unwrap_or(PROMPT_TEXT),
            controller_label: &self.controller_label,
            cue: crate::win::controller_tips::cue(self.cue),
            prev_cue: morph.map(|(from, _)| crate::win::controller_tips::cue(from)),
            morph_t: morph.map(|(_, t)| t).unwrap_or(1.0),
            index: self.cue,
            count: crate::win::controller_tips::cue_count(),
            alpha,
            scale,
            style: vk_style(),
        };
        let t = self.started.elapsed().as_secs_f32();
        if !draw::paint_tips_pill(&mut self.gfx, &pill, TIPS_W as f32, TIPS_H as f32, self.scale as f32, t) {
            return;
        }
        let pixels = draw::to_argb(&self.gfx.pixels());
        let buffer_w = self.gfx.width() as i32;
        let buffer_h = self.gfx.height() as i32;
        let Ok((buffer, canvas)) = self.pool.create_buffer(
            buffer_w,
            buffer_h,
            buffer_w * 4,
            wayland_client::protocol::wl_shm::Format::Argb8888,
        ) else {
            return;
        };
        let copy = pixels.len().min(canvas.len());
        canvas[..copy].copy_from_slice(&pixels[..copy]);
        let surface = self.layer.wl_surface();
        surface.set_buffer_scale(self.scale);
        surface.damage_buffer(0, 0, buffer_w, buffer_h);
        surface.frame(qh, surface.clone());
        if buffer.attach_to(surface).is_ok() {
            self.buffer = Some(buffer);
            self.layer.commit();
            self.frame_pending = true;
            self.frame_at = now;
        }
    }
}

impl CompositorHandler for Shell {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
        self.dirty = true;
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if surface != self.layer.wl_surface() {
            return;
        }
        self.frame_pending = false;
        if self.slide_waiting {
            self.slide_waiting = false;
            self.slide = Some((std::time::Instant::now(), false));
            self.dirty = true;
        }
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
        self.output = Some(output.clone());
        self.dirty = true;
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

impl OutputHandler for Shell {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.dirty = true;
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Shell {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let _ = conn.flush();
        let docked = self.placed.is_some_and(|(docked, _, _, _)| docked);
        let mut changed = !self.configured;
        if docked && configure.new_size.0 > 0 && configure.new_size.0 != self.logical_w {
            self.logical_w = configure.new_size.0;
            changed = true;
        }
        if docked && configure.new_size.1 > 0 && configure.new_size.1 != self.logical_h {
            self.logical_h = configure.new_size.1;
            changed = true;
        }
        self.configured = true;
        if changed {
            self.dirty = true;
        }
        if self.dirty && !self.frame_pending {
            self.draw(qh);
        }
    }
}

impl ShmHandler for Shell {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Shell {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Shell);
delegate_output!(Shell);
delegate_shm!(Shell);
delegate_layer!(Shell);
delegate_registry!(Shell);
delegate_seat!(Shell);
delegate_pointer!(Shell);
delegate_touch!(Shell);

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Shell {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let next = scale as f64 / 120.0;
            if state.fractional != Some(next) {
                crate::install::log_line(&format!("keyboard buffer scale {next}"));
            }
            state.fractional = Some(next);
            state.dirty = true;
        }
    }
}

impl Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()> for Shell {
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

impl Dispatch<wp_viewport::WpViewport, ()> for Shell {
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

impl Dispatch<wp_viewporter::WpViewporter, ()> for Shell {
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

impl CompositorHandler for Tips {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        self.scale = factor.max(1);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
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

impl OutputHandler for Tips {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Tips {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.configured = true;
        if !self.frame_pending {
            self.draw(qh);
        }
    }
}

impl ShmHandler for Tips {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Tips {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Tips);
delegate_output!(Tips);
delegate_shm!(Tips);
delegate_layer!(Tips);
delegate_registry!(Tips);

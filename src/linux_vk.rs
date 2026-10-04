use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};

use crate::config::{vk_layout_mode, vk_style, VkLayoutMode, VkStyle};

#[path = "linux_vk_draw.rs"]
mod draw;

#[cfg(feature = "speech")]
pub fn draw_voice_surface(
    label: &str,
    hint: &str,
    orb: Option<&[u8]>,
    orb_px: u32,
) -> Option<tiny_skia::Pixmap> {
    draw::draw_voice_pill(label, hint, orb, orb_px)
}

#[cfg(feature = "speech")]
pub fn draw_argb(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    draw::to_argb(pixmap)
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
    if std::env::var_os("WARMUP_VK_PROMPT").is_some_and(|value| value == "0") {
        return;
    }
    let path = crate::paths::state_dir().join(".keyboard_prompt_shown");
    if path.is_file() {
        return;
    }
    std::thread::Builder::new()
        .name("warmup-vk-prompt".into())
        .spawn(move || {
            if run_prompt().is_ok() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(path, b"1");
            }
        })
        .ok();
}

fn run_keyboard(wake: UnixStream) {
    if let Err(error) = keyboard_loop(wake) {
        crate::install::log_line(&format!("on-screen keyboard: {error}"));
    }
}

fn keyboard_loop(wake: UnixStream) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Shell>(&conn).map_err(|error| error.to_string())?;
    let qh = queue.handle();
    let mut shell = Shell::new(&globals, &qh)?;
    shell.layer.commit();
    loop {
        if STOP.load(Ordering::SeqCst) || shell.exit {
            break;
        }
        let _ = queue.dispatch_pending(&mut shell);
        if shell.dirty && shell.configured && !shell.frame_pending {
            shell.draw(&qh);
        }
        let _ = queue.flush();
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let (wayland_ready, wake_ready) = {
            let mut fds = [
                nix::poll::PollFd::new(guard.connection_fd(), nix::poll::PollFlags::POLLIN),
                nix::poll::PollFd::new(wake.as_fd(), nix::poll::PollFlags::POLLIN),
            ];
            let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(500u16));
            (
                fds[0].any().unwrap_or(false),
                fds[1].any().unwrap_or(false),
            )
        };
        if wayland_ready {
            let _ = guard.read();
        }
        if wake_ready {
            let mut buf = [0u8; 32];
            let _ = (&wake).read(&mut buf);
            shell.dirty = true;
        }
    }
    if let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(proxy) = shell.layer.kind() {
        proxy.destroy();
    }
    let _ = queue.roundtrip(&mut shell);
    drop(shell);
    drop(queue);
    let _ = conn.flush();
    Ok(())
}

struct Shell {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
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
    fonts: cosmic_text::FontSystem,
    cache: cosmic_text::SwashCache,
    buffer: Option<Buffer>,
    ps5: bool,
    glide: draw::Glide,
    placed: Option<(bool, u32, u32, i32)>,
}

impl Shell {
    fn new(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
    ) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|error| error.to_string())?;
        let layer_shell = LayerShell::bind(globals, qh).map_err(|_| {
            "this compositor has no layer shell, so there is no on-screen keyboard".to_string()
        })?;
        let shm = Shm::bind(globals, qh).map_err(|error| error.to_string())?;
        let surface = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("warmup-companion-keyboard"),
            None,
        );
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let viewport = globals
            .bind::<wp_viewporter::WpViewporter, _, _>(qh, 1..=1, ())
            .ok()
            .map(|porter| porter.get_viewport(layer.wl_surface(), qh, ()));
        if let Ok(manager) = globals
            .bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(qh, 1..=1, ())
        {
            let _ = manager.get_fractional_scale(layer.wl_surface(), qh, ());
        }
        let pool = SlotPool::new(1920 * 400 * 4, &shm).map_err(|error| error.to_string())?;
        let mut shell = Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            shm,
            pool,
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
            fonts: draw::font_system(),
            cache: cosmic_text::SwashCache::new(),
            buffer: None,
            ps5: ps5_pad(),
            glide: draw::Glide::default(),
            placed: None,
        };
        shell.apply_layout();
        Ok(shell)
    }

    fn apply_layout(&mut self) {
        let (output_w, output_h) = self.output_size();
        let keyboard_docked = matches!(vk_layout_mode(), VkLayoutMode::Docked);
        let (width, height) = desired_size(output_w, output_h);
        let docked = keyboard_docked;
        let margin = if keyboard_docked { 0 } else { draw::floating_bottom_margin(output_h) };
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
        let scale = if self.viewport.is_some() {
            1.0
        } else {
            self.buffer_scale() as f32
        };
        let rows = crate::vk_nav::rows_snapshot();
        let mono = matches!(vk_style(), VkStyle::Mono);
        let strip = crate::vk_predict::strip(if mono { 3 } else { 7 }, mono);
        let pressed = crate::vk_nav::press_feedback().map(|(pos, at)| {
            (
                pos,
                crate::vk_motion::press_scale(at.elapsed().as_secs_f32() * 1000.0),
            )
        });
        let (shift, caps, symbol, symbol_locked) = crate::vk_nav::modifier_state();
        let floating = !matches!(vk_layout_mode(), VkLayoutMode::Docked);
        let frame = draw::Frame {
            logical_w: self.logical_w as f32,
            logical_h: self.logical_h as f32,
            scale,
            scale_w: draw::layout_scale_w(vk_style()),
            rows,
            sel: crate::vk_nav::selection(),
            style: vk_style(),
            theme: crate::config::keyboard_theme(),
            strip,
            pressed,
            ps5: self.ps5,
            shift,
            caps,
            symbol,
            symbol_locked,
            floating,
            sheet: crate::vk_sheet::shown(),
        };
        let Some(mut pixmap) = draw::draw(&frame, &mut self.fonts, &mut self.cache, &mut self.glide) else {
            return;
        };
        #[cfg(feature = "speech")]
        if crate::linux_speech::visual_active() {
            if let Some((px, bytes)) = crate::linux_nimbus::latest_frame() {
                draw::paint_voice_key(&mut pixmap, &frame, &bytes, px);
            }
        }
        let pixels = draw::to_argb(&pixmap);
        let buffer_w = pixmap.width() as i32;
        let buffer_h = pixmap.height() as i32;
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
        self.dirty = self.glide.animating;
        if let Err(error) = buffer.attach_to(surface) {
            crate::install::log_line(&format!("keyboard buffer: {error}"));
            return;
        }
        self.buffer = Some(buffer);
        self.layer.commit();
    }
}

fn desired_size(output_w: f32, output_h: f32) -> (u32, u32) {
    let rows = crate::vk_nav::rows_snapshot();
    let docked = matches!(vk_layout_mode(), VkLayoutMode::Docked);
    draw::card_size(output_w, output_h, docked, &rows, vk_style())
}

fn ps5_pad() -> bool {
    std::fs::read_to_string("/proc/bus/input/devices")
        .unwrap_or_default()
        .contains("DualSense")
        || std::fs::read_to_string("/proc/bus/input/devices")
            .unwrap_or_default()
            .contains("Wireless Controller")
}

fn run_prompt() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Prompt>(&conn).map_err(|error| error.to_string())?;
    let qh = queue.handle();
    let mut prompt = Prompt::new(&globals, &qh)?;
    prompt.layer.commit();
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while std::time::Instant::now() < deadline && !prompt.exit {
        let _ = queue.dispatch_pending(&mut prompt);
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let remain = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(400) as u16;
        let mut fds = [nix::poll::PollFd::new(
            guard.connection_fd(),
            nix::poll::PollFlags::POLLIN,
        )];
        let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(remain));
        if fds[0].any().unwrap_or(false) {
            let _ = guard.read();
        }
    }
    if let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(proxy) = prompt.layer.kind() {
        proxy.destroy();
    }
    let _ = queue.roundtrip(&mut prompt);
    drop(prompt);
    drop(queue);
    let _ = conn.flush();
    Ok(())
}

struct Prompt {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    exit: bool,
    drawn: bool,
    fonts: cosmic_text::FontSystem,
    cache: cosmic_text::SwashCache,
    buffer: Option<Buffer>,
}

impl Prompt {
    fn new(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
    ) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|error| error.to_string())?;
        let layer_shell = LayerShell::bind(globals, qh).map_err(|_| {
            "this compositor has no layer shell, so the keyboard prompt is not shown".to_string()
        })?;
        let shm = Shm::bind(globals, qh).map_err(|error| error.to_string())?;
        let surface = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("warmup-companion-prompt"),
            None,
        );
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_anchor(Anchor::BOTTOM);
        layer.set_exclusive_zone(0);
        layer.set_margin(0, 0, 28, 0);
        layer.set_size(520, 72);
        let pool = SlotPool::new(520 * 72 * 4 * 4, &shm).map_err(|error| error.to_string())?;
        Ok(Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            shm,
            pool,
            layer,
            exit: false,
            drawn: false,
            fonts: draw::font_system(),
            cache: cosmic_text::SwashCache::new(),
            buffer: None,
            })
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let Some(pixmap) = draw::draw_prompt(
            "Press L3 to open the keyboard",
            1.0,
            &mut self.fonts,
            &mut self.cache,
        ) else {
            return;
        };
        let pixels = draw::to_argb(&pixmap);
        let buffer_w = pixmap.width() as i32;
        let buffer_h = pixmap.height() as i32;
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
        surface.damage_buffer(0, 0, buffer_w, buffer_h);
        surface.frame(qh, surface.clone());
        if buffer.attach_to(surface).is_ok() {
            self.buffer = Some(buffer);
            self.layer.commit();
            self.drawn = true;
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

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.dirty {
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
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
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
    registry_handlers![OutputState];
}

delegate_compositor!(Shell);
delegate_output!(Shell);
delegate_shm!(Shell);
delegate_layer!(Shell);
delegate_registry!(Shell);

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

impl CompositorHandler for Prompt {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

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

impl OutputHandler for Prompt {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Prompt {
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
        if !self.drawn {
            self.draw(qh);
        }
    }
}

impl ShmHandler for Prompt {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Prompt {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Prompt);
delegate_output!(Prompt);
delegate_shm!(Prompt);
delegate_layer!(Prompt);
delegate_registry!(Prompt);

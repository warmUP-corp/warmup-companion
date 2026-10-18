use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure, SurfaceKind,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, QueueHandle};

static SHOWN: AtomicBool = AtomicBool::new(false);

pub fn show() {
    if SHOWN.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-voice-pill".into())
        .spawn(|| {
            if let Err(error) = run() {
                crate::install::log_line(&format!("voice pill: {error}"));
            }
            SHOWN.store(false, Ordering::SeqCst);
        });
}

fn run() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Pill>(&conn).map_err(|error| error.to_string())?;
    let qh = queue.handle();
    let mut pill = Pill::new(&globals, &qh)?;
    pill.border.commit();
    let mut wayland_dead = false;
    while crate::linux_speech::visual_active() && !wayland_dead {
        if let Err(error) = queue.dispatch_pending(&mut pill) {
            crate::install::log_line(&format!("voice pill wayland: {error}"));
            wayland_dead = true;
            break;
        }
        if pill.border_ready {
            pill.draw_border(&qh);
            pill.place_orb();
        }
        if pill.orb_ready {
            pill.sync_orb(&qh);
        }
        if let Err(error) = queue.flush() {
            crate::install::log_line(&format!("voice pill wayland: {error}"));
            wayland_dead = true;
            break;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [nix::poll::PollFd::new(
            guard.connection_fd(),
            nix::poll::PollFlags::POLLIN,
        )];
        let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(33u16));
        if fds[0].any().unwrap_or(false) && guard.read().is_err() {
            crate::install::log_line("voice pill wayland: read failed");
            wayland_dead = true;
            break;
        }
    }
    if !wayland_dead {
        destroy_layer(&pill.orb);
        destroy_layer(&pill.border);
        let _ = queue.roundtrip(&mut pill);
    }
    drop(pill);
    drop(queue);
    let _ = conn.flush();
    Ok(())
}

fn destroy_layer(layer: &LayerSurface) {
    if let SurfaceKind::Wlr(proxy) = layer.kind() {
        proxy.destroy();
    }
}

fn pass_through(compositor: &CompositorState, layer: &LayerSurface) {
    if let Ok(region) = smithay_client_toolkit::compositor::Region::new(compositor) {
        layer.wl_surface().set_input_region(Some(region.wl_region()));
    }
}

struct Pill {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    border_pool: SlotPool,
    orb_pool: SlotPool,
    border: LayerSurface,
    orb: LayerSurface,
    border_buffer: Option<Buffer>,
    orb_buffer: Option<Buffer>,
    border_ready: bool,
    orb_ready: bool,
    border_size: (u32, u32),
    orb_size: (u32, u32),
    border_phase: u8,
    border_drawn: bool,
    orb_placed: Option<(u32, i32)>,
    orb_visible: bool,
    level: f32,
    level_at: Option<Instant>,
    shown_gen: u64,
    gfx: crate::mono_ui::Gfx,
}

impl Pill {
    fn new(globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<Self>) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|error| error.to_string())?;
        let layer_shell = LayerShell::bind(globals, qh).map_err(|_| "this compositor has no layer shell".to_string())?;
        let shm = Shm::bind(globals, qh).map_err(|error| error.to_string())?;
        let border_surface = compositor.create_surface(qh);
        let border = layer_shell.create_layer_surface(
            qh,
            border_surface,
            Layer::Overlay,
            Some("warmup-companion-voice"),
            None,
        );
        border.set_keyboard_interactivity(KeyboardInteractivity::None);
        border.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        border.set_exclusive_zone(0);
        border.set_margin(0, 0, 0, 0);
        border.set_size(0, 0);
        pass_through(&compositor, &border);
        let orb_surface = compositor.create_surface(qh);
        let orb = layer_shell.create_layer_surface(
            qh,
            orb_surface,
            Layer::Overlay,
            Some("warmup-companion-voice-orb"),
            None,
        );
        orb.set_keyboard_interactivity(KeyboardInteractivity::None);
        orb.set_anchor(Anchor::RIGHT);
        orb.set_exclusive_zone(0);
        pass_through(&compositor, &orb);
        let border_pool = SlotPool::new(48 * 1024 * 1024, &shm).map_err(|error| error.to_string())?;
        let orb_pool = SlotPool::new(1024 * 1024, &shm).map_err(|error| error.to_string())?;
        Ok(Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            shm,
            border_pool,
            orb_pool,
            border,
            orb,
            border_buffer: None,
            orb_buffer: None,
            border_ready: false,
            orb_ready: false,
            border_size: (0, 0),
            orb_size: (0, 0),
            border_phase: 255,
            border_drawn: false,
            orb_placed: None,
            orb_visible: false,
            level: 0.0,
            level_at: None,
            shown_gen: u64::MAX,
            gfx: crate::mono_ui::Gfx::new(1, 1)?,
        })
    }

    fn draw_border(&mut self, qh: &QueueHandle<Self>) {
        let (width, height) = self.border_size;
        if !self.border_ready || width == 0 || height == 0 {
            return;
        }
        let phase = crate::linux_speech::phase();
        if self.border_drawn && self.border_phase == phase {
            return;
        }
        let accent = crate::config::keyboard_theme()
            .accent
            .unwrap_or(crate::vk_paint::rgb(0xA6D1FF));
        if !crate::linux_vk::paint_voice_screen(&mut self.gfx, width, height, phase, 0.0, false, None, 0, accent) {
            return;
        }
        let pixels = crate::linux_vk::draw_argb(self.gfx.pixels());
        let size = (self.gfx.width(), self.gfx.height());
        if !self.upload(&pixels, size, qh, true) {
            return;
        }
        self.border_phase = phase;
        self.border_drawn = true;
    }

    fn place_orb(&mut self) {
        let (width, height) = self.border_size;
        if width == 0 || height == 0 {
            return;
        }
        let scale = (height as f32 / 1080.0).clamp(0.85, 2.0);
        let slot = (96.0 * scale).min((width.min(height) as f32) * 0.9).max(1.0);
        let margin = (48.0 * scale).min((width as f32 - slot).max(0.0));
        let side = (slot * 1.25).round().max(1.0) as u32;
        let center = margin + slot * 0.5;
        let right = (center - side as f32 * 0.5).round().max(0.0) as i32;
        if self.orb_placed == Some((side, right)) {
            return;
        }
        self.orb_placed = Some((side, right));
        self.orb.set_margin(0, right, 0, 0);
        self.orb.set_size(side, side);
        self.orb.commit();
    }

    fn sync_orb(&mut self, qh: &QueueHandle<Self>) {
        if crate::linux_vk::keyboard_open() {
            self.hide_orb();
            return;
        }
        let gen = crate::linux_nimbus::frame_generation();
        if self.orb_visible && self.shown_gen == gen {
            return;
        }
        self.draw_orb(qh);
    }

    fn draw_orb(&mut self, qh: &QueueHandle<Self>) {
        let side = self.orb_size.0.max(self.orb_size.1);
        if !self.orb_ready || side == 0 {
            return;
        }
        let Some((px, bytes)) = crate::linux_nimbus::latest_frame() else {
            return;
        };
        if px == 0 {
            return;
        }
        let now = Instant::now();
        let dt_ms = self.level_at.map(|t| now.duration_since(t).as_secs_f32() * 1000.0).unwrap_or(0.0);
        self.level_at = Some(now);
        let target = crate::linux_speech::level();
        self.level = if dt_ms > 0.0 {
            crate::vk_motion::smooth_level(self.level, target, dt_ms)
        } else {
            target
        };
        let phase = crate::linux_speech::phase();
        let Some(pixmap) = crate::linux_vk::paint_voice_orb(side, phase, self.level, &bytes, px) else {
            return;
        };
        let pixels = crate::linux_vk::draw_argb(pixmap.data());
        if self.upload(&pixels, (pixmap.width(), pixmap.height()), qh, false) {
            self.shown_gen = crate::linux_nimbus::frame_generation();
            self.orb_visible = true;
        }
    }

    fn hide_orb(&mut self) {
        if !self.orb_visible {
            return;
        }
        self.orb.wl_surface().attach(None, 0, 0);
        self.orb.commit();
        self.orb_visible = false;
        self.shown_gen = u64::MAX;
    }

    fn upload(&mut self, pixels: &[u8], size: (u32, u32), qh: &QueueHandle<Self>, border: bool) -> bool {
        let buffer_w = size.0 as i32;
        let buffer_h = size.1 as i32;
        let pool = if border { &mut self.border_pool } else { &mut self.orb_pool };
        let Ok((buffer, canvas)) = pool.create_buffer(
            buffer_w,
            buffer_h,
            buffer_w * 4,
            wayland_client::protocol::wl_shm::Format::Argb8888,
        ) else {
            return false;
        };
        let copy = pixels.len().min(canvas.len());
        canvas[..copy].copy_from_slice(&pixels[..copy]);
        let layer = if border { &self.border } else { &self.orb };
        let surface = layer.wl_surface();
        surface.damage_buffer(0, 0, buffer_w, buffer_h);
        surface.frame(qh, surface.clone());
        if buffer.attach_to(surface).is_err() {
            return false;
        }
        layer.commit();
        if border {
            self.border_buffer = Some(buffer);
        } else {
            self.orb_buffer = Some(buffer);
        }
        true
    }
}

impl CompositorHandler for Pill {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wayland_client::protocol::wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wayland_client::protocol::wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wayland_client::protocol::wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wayland_client::protocol::wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wayland_client::protocol::wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for Pill {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Pill {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    fn configure(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let _ = conn.flush();
        if layer.wl_surface() == self.border.wl_surface() {
            if configure.new_size.0 > 0 && configure.new_size.1 > 0 && self.border_size != configure.new_size {
                self.border_size = configure.new_size;
                self.border_drawn = false;
            }
            self.border_ready = true;
            self.draw_border(qh);
            self.place_orb();
        } else {
            if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
                self.orb_size = configure.new_size;
            }
            self.orb_ready = true;
            self.shown_gen = u64::MAX;
            if !crate::linux_vk::keyboard_open() {
                self.draw_orb(qh);
            }
        }
    }
}

impl ShmHandler for Pill {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Pill {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Pill);
delegate_output!(Pill);
delegate_shm!(Pill);
delegate_layer!(Pill);
delegate_registry!(Pill);

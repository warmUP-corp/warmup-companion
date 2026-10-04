use std::sync::atomic::{AtomicBool, Ordering};

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
    pill.layer.commit();
    while crate::linux_speech::visual_active() {
        let _ = queue.dispatch_pending(&mut pill);
        pill.draw(&qh);
        let _ = queue.flush();
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [nix::poll::PollFd::new(
            guard.connection_fd(),
            nix::poll::PollFlags::POLLIN,
        )];
        let _ = nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(33u16));
        if fds[0].any().unwrap_or(false) {
            let _ = guard.read();
        }
    }
    if let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(proxy) = pill.layer.kind() {
        proxy.destroy();
    }
    let _ = queue.roundtrip(&mut pill);
    drop(pill);
    drop(queue);
    let _ = conn.flush();
    Ok(())
}

struct Pill {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    buffer: Option<Buffer>,
    frames: crate::linux_vk::VoiceFrames,
    shown_gen: u64,
    shown_label: String,
    shown_hint: String,
}

impl Pill {
    fn new(globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<Self>) -> Result<Self, String> {
        let compositor = CompositorState::bind(globals, qh).map_err(|error| error.to_string())?;
        let layer_shell = LayerShell::bind(globals, qh).map_err(|_| "this compositor has no layer shell".to_string())?;
        let shm = Shm::bind(globals, qh).map_err(|error| error.to_string())?;
        let surface = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("warmup-companion-voice"), None);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_anchor(Anchor::RIGHT);
        layer.set_exclusive_zone(0);
        layer.set_margin(0, 40, 0, 0);
        layer.set_size(420, 104);
        let pool = SlotPool::new(420 * 104 * 4 * 2, &shm).map_err(|error| error.to_string())?;
        Ok(Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            shm,
            pool,
            layer,
            buffer: None,
            frames: crate::linux_vk::VoiceFrames::new(),
            shown_gen: u64::MAX,
            shown_label: String::new(),
            shown_hint: String::new(),
        })
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let label = match crate::linux_speech::phase() {
            2 => "Transcribing",
            _ => "Listening",
        };
        let hint = if crate::linux_speech::phase() == 2 {
            "One moment"
        } else {
            "R3 to finish"
        };
        let gen = crate::linux_nimbus::frame_generation();
        if self.buffer.is_some() && self.shown_gen == gen && self.shown_label == label && self.shown_hint == hint {
            return;
        }
        let orb = crate::linux_nimbus::latest_frame();
        let Some(pixmap) = self.frames.paint(label, hint, orb.as_ref().map(|(_, bytes)| bytes.as_slice()), orb.as_ref().map(|(px, _)| *px).unwrap_or(0)) else {
            return;
        };
        let pixels = crate::linux_vk::draw_argb(&pixmap);
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
            self.shown_gen = gen;
            self.shown_label = label.to_string();
            self.shown_hint = hint.to_string();
            self.layer.commit();
        }
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
    fn configure(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &LayerSurface, _: LayerSurfaceConfigure, _: u32) {
        self.draw(qh);
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

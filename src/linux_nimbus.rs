use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RT: u32 = 160;
const STEPS: i32 = 56;
const LIGHT_STEPS: i32 = 4;
const DENSITY_OCT: i32 = 4;
const WHITE_LIFT: f32 = 0.35;
const HUE_SPREAD: f32 = 0.6;
const NIMBUS_BODY: f32 = 0.55;
const FRAME: Duration = Duration::from_millis(16);
#[cfg(any(test, not(feature = "gamepad")))]
const DEFAULT_ACCENT: u32 = 0x00ffd1a6;

#[repr(C)]
#[derive(Clone, Copy)]
struct NimbusCb {
    res_x: f32,
    res_y: f32,
    input: f32,
    output: f32,
    speed: f32,
    cam_dist: f32,
    focal: f32,
    radius: f32,
    scale: f32,
    churn: f32,
    threshold: f32,
    edge_soft: f32,
    density: f32,
    absorb: f32,
    shadow_absorb: f32,
    shadow_lift: f32,
    aniso: f32,
    light_spin: f32,
    power: f32,
    ambient: f32,
    exposure: f32,
    alpha_gain: f32,
    swirl: f32,
    _pad1: f32,
    light: [f32; 4],
    shadow: [f32; 4],
}

struct Clock {
    noise_phase: f32,
    light_phase: f32,
    swirl_phase: f32,
    age: f32,
    level: f32,
    level_at: Option<Instant>,
    last: Option<Instant>,
}

static LATEST: OnceLock<Mutex<Option<(u32, Vec<u8>)>>> = OnceLock::new();
static FRAME_GEN: AtomicU64 = AtomicU64::new(0);
static ANIMATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn ensure_running() {
    if ANIMATING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("warmup-nimbus".into())
        .spawn(|| {
            let mut clock = Clock::new();
            let mut cpu = vec![0u8; (RT * RT * 4) as usize];
            let mut gpu = Gpu::open();
            while crate::linux_speech::visual_active() {
                let started = Instant::now();
                let cb = clock.advance(
                    started,
                    crate::linux_speech::phase(),
                    crate::linux_speech::level(),
                    accent(),
                );
                let mut failed = false;
                let rendered = match gpu.as_mut() {
                    Some(gpu) => match gpu.tick(&cb) {
                        Ok(frame) => frame,
                        Err(error) => {
                            crate::install::log_line(&format!("nimbus gpu: {error}"));
                            failed = true;
                            None
                        }
                    },
                    None => {
                        render_cpu(&cb, &mut cpu);
                        Some(cpu.clone())
                    }
                };
                if failed {
                    gpu = None;
                }
                if let Some(frame) = rendered {
                    if let Ok(mut slot) = frame_slot().lock() {
                        *slot = Some((RT, frame));
                    }
                    FRAME_GEN.fetch_add(1, Ordering::Relaxed);
                    #[cfg(feature = "gamepad")]
                    crate::linux_vk::request_repaint();
                }
                let rest = FRAME.saturating_sub(started.elapsed());
                if !rest.is_zero() {
                    std::thread::sleep(rest);
                }
            }
            if let Ok(mut slot) = frame_slot().lock() {
                *slot = None;
            }
            FRAME_GEN.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "gamepad")]
            crate::linux_vk::request_repaint();
            ANIMATING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
}

fn accent() -> u32 {
    #[cfg(feature = "gamepad")]
    {
        crate::linux_vk::voice_accent()
    }
    #[cfg(not(feature = "gamepad"))]
    {
        DEFAULT_ACCENT
    }
}

pub fn latest_frame() -> Option<(u32, Vec<u8>)> {
    frame_slot().lock().ok().and_then(|guard| guard.clone())
}

pub fn has_frame() -> bool {
    frame_slot().lock().map(|guard| guard.is_some()).unwrap_or(false)
}

pub fn frame_generation() -> u64 {
    FRAME_GEN.load(Ordering::Relaxed)
}

fn frame_slot() -> &'static Mutex<Option<(u32, Vec<u8>)>> {
    LATEST.get_or_init(|| Mutex::new(None))
}

impl Clock {
    fn new() -> Self {
        Self {
            noise_phase: 0.0,
            light_phase: 0.0,
            swirl_phase: 0.0,
            age: 0.0,
            level: 0.0,
            level_at: None,
            last: None,
        }
    }

    fn think_pulse(&self) -> f32 {
        0.5 + 0.5 * (self.age * 2.4).sin()
    }

    fn smoothed_level(&mut self, target: f32, now: Instant) -> f32 {
        let dt_ms = self
            .level_at
            .map(|t| now.duration_since(t).as_secs_f32() * 1000.0)
            .unwrap_or(0.0);
        self.level_at = Some(now);
        self.level = if dt_ms > 0.0 {
            crate::vk_motion::smooth_level(self.level, target, dt_ms)
        } else {
            target.clamp(0.0, 1.0)
        };
        self.level
    }

    fn advance(&mut self, now: Instant, phase: u8, raw_level: f32, accent: u32) -> NimbusCb {
        let level_in = self.smoothed_level(raw_level, now);
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).as_secs_f32().min(0.05))
            .unwrap_or(1.0 / 60.0);
        self.last = Some(now);
        self.age += dt;
        let level = if phase == 1 { level_in.clamp(0.0, 1.0) } else { 0.0 };
        let pulse = match phase {
            1 => 0.5 + 0.5 * level_in.clamp(0.0, 1.0),
            2 => self.think_pulse().clamp(0.0, 1.0),
            _ => 0.5,
        };
        let noise_rate = match phase {
            1 => 0.22 + 0.2 * level,
            2 => 0.4 + 0.22 * pulse,
            _ => 0.22,
        };
        self.noise_phase += dt * noise_rate;
        self.light_phase += dt * 0.08;
        let swirl_rate = match phase {
            1 => 0.15 + 0.4 * level,
            2 => 0.30,
            _ => 0.15,
        };
        self.swirl_phase = (self.swirl_phase + dt * swirl_rate).rem_euclid(std::f32::consts::TAU);
        nimbus_cb(pulse, self.noise_phase, self.light_phase, self.swirl_phase, accent)
    }
}

fn nimbus_cb(pulse: f32, noise_phase: f32, light_phase: f32, swirl_phase: f32, accent: u32) -> NimbusCb {
    let glow = pulse;
    NimbusCb {
        res_x: RT as f32,
        res_y: RT as f32,
        input: NIMBUS_BODY,
        output: pulse,
        speed: noise_phase,
        cam_dist: 4.4,
        focal: 1.8,
        radius: 2.0,
        scale: 1.15,
        churn: glow,
        threshold: 0.40 - 0.16 * glow,
        edge_soft: 0.5,
        density: 1.35 + 2.6 * glow,
        absorb: 0.72,
        shadow_absorb: 1.5,
        shadow_lift: 0.7,
        aniso: 0.4,
        light_spin: light_phase,
        power: 0.8 + 0.45 * glow,
        ambient: 0.06 + 0.08 * glow,
        exposure: 0.50 + 0.20 * glow,
        alpha_gain: 1.0,
        swirl: swirl_phase,
        _pad1: 0.0,
        light: theme_light(accent),
        shadow: theme_shade(accent),
    }
}

fn theme_light(accent: u32) -> [f32; 4] {
    let rgb = boost_chroma(accent, 1.6);
    [rgb[0], rgb[1], rgb[2], 1.0]
}

fn theme_shade(accent: u32) -> [f32; 4] {
    let rgb = boost_chroma(accent, 1.25);
    [rgb[0] * 0.7, rgb[1] * 0.7, rgb[2] * 0.7, 1.0]
}

fn boost_chroma(c: u32, sat: f32) -> [f32; 3] {
    let r = chan(c, 0);
    let g = chan(c, 8);
    let b = chan(c, 16);
    let max = r.max(g).max(b).max(1.0e-4);
    let pull = |x: f32| (max + (x - max) * sat).clamp(0.0, 1.0);
    [pull(r), pull(g), pull(b)]
}

fn chan(c: u32, shift: u32) -> f32 {
    ((c >> shift) & 0xff) as f32 / 255.0
}

fn render_cpu(cb: &NimbusCb, out: &mut [u8]) {
    let row = (RT * 4) as usize;
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
    let rows_per = (RT as usize).div_ceil(workers);
    std::thread::scope(|scope| {
        for (chunk, band) in out.chunks_mut(rows_per * row).enumerate() {
            scope.spawn(move || {
                for (dy, line) in band.chunks_mut(row).enumerate() {
                    let y = (chunk * rows_per + dy) as f32;
                    for x in 0..RT as usize {
                        let c = ps_main(cb, [x as f32 + 0.5, y + 0.5]);
                        for k in 0..4 {
                            line[x * 4 + k] = unorm8(c[k]);
                        }
                    }
                }
            });
        }
    });
}

fn unorm8(x: f32) -> u8 {
    (saturate(x) * 255.0 + 0.5).floor() as u8
}

fn tanh3(x: [f32; 3]) -> [f32; 3] {
    let t = |v: f32| {
        let v = v.clamp(-10.0, 10.0);
        let e = (2.0 * v).exp();
        (e - 1.0) / (e + 1.0)
    };
    [t(x[0]), t(x[1]), t(x[2])]
}

fn hue_rotate(c: [f32; 3], a: f32) -> [f32; 3] {
    let k = [0.57735, 0.57735, 0.57735];
    let ca = a.cos();
    add(add(mul(c, ca), mul(cross(k, c), a.sin())), mul(k, dot(k, c) * (1.0 - ca)))
}

fn phase_hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    (1.0 - g2) / (1.0 + g2 - 2.0 * g * c).max(0.0001).powf(1.5)
}

fn rotate_axis(p: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let s = angle.sin();
    let c = angle.cos();
    add(add(mul(p, c), mul(cross(axis, p), s)), mul(axis, dot(axis, p) * (1.0 - c)))
}

fn density(u: &NimbusCb, p: [f32; 3], nimbus_density: f32) -> f32 {
    let shell = 1.0 - length(p) / u.radius;
    if shell <= 0.0 {
        return 0.0;
    }
    let voice = saturate(u.churn);
    let pulse = saturate(u.output);
    let self_fold = 0.55 + 0.3 * (u.speed * 2.2).sin();
    let amp = self_fold + 0.5 * voice + 0.95 * pulse;
    let axis = normalize([0.35 * u.swirl.sin(), 1.0, 0.35 * u.swirl.cos()]);
    let mut swirled = rotate_axis(p, axis, u.swirl);
    swirled = add(
        swirled,
        mul(
            [(u.swirl + 1.7).sin(), (u.swirl * 2.0 + 0.4).cos(), (u.swirl + 2.9).sin()],
            0.05,
        ),
    );
    let mut q = mul(swirled, u.scale * (1.0 + 0.16 * voice + 0.5 * pulse));
    let mut f = 1.0f32;
    for _ in 0..DENSITY_OCT {
        q = add(
            q,
            mul(
                [(q[1] * f + u.speed).cos(), (q[2] * f + u.speed).cos(), (q[0] * f + u.speed).cos()],
                amp / f,
            ),
        );
        f *= 1.8;
    }
    let n = (q[0].sin() + q[1].sin() + q[2].sin()) / 3.0 * 0.5 + 0.5;
    let thresh = lerp(u.threshold, u.threshold * 0.45, pulse);
    let clump = smoothstep(thresh, 1.0, n);
    clump * shell.powf(u.edge_soft) * nimbus_density
}

fn nimbus_render(u: &NimbusCb, frag: [f32; 2], nimbus_power: f32, nimbus_density: f32) -> [f32; 4] {
    let res = [u.res_x, u.res_y];
    let fy = res[1] - frag[1];
    let m = res[0].min(res[1]);
    let uv = [(2.0 * frag[0] - res[0]) / m, (2.0 * fy - res[1]) / m];
    let ro = [0.0, 0.0, -u.cam_dist];
    let rd = normalize([uv[0], uv[1], u.focal]);
    let l = normalize([u.light_spin.cos() * 0.7, 0.45, u.light_spin.sin() * 0.35 + 0.65]);
    let phase = phase_hg(dot(rd, l), u.aniso);
    let t_start = (u.cam_dist - u.radius).max(0.0);
    let dt = (2.0 * u.radius) / STEPS as f32;
    let mut transmit = 1.0f32;
    let mut scattered = [0.0f32; 3];
    let ray_dist = length(cross(rd, ro));
    if ray_dist < u.radius {
        for i in 0..STEPS {
            let t = t_start + (i as f32 + 0.5) * dt;
            let p = add(ro, mul(rd, t));
            let dn = density(u, p, nimbus_density);
            if dn > 0.001 {
                let mut shadow = 1.0f32;
                let lstep = u.radius / LIGHT_STEPS as f32;
                for s in 1..=LIGHT_STEPS {
                    let lp = add(p, mul(l, (s as f32 - 0.5) * lstep));
                    shadow *= (-density(u, lp, nimbus_density) * lstep * u.shadow_absorb).exp();
                }
                let hue = HUE_SPREAD * (dot(p, [0.8, 0.5, -0.3]) * 1.4 + u.swirl).sin();
                let light = saturate3(hue_rotate([u.light[0], u.light[1], u.light[2]], hue));
                let shade = [u.shadow[0], u.shadow[1], u.shadow[2]];
                let lit = lerp3(mul(shade, u.shadow_lift), light, shadow);
                scattered = add(scattered, mul(lit, transmit * dn * dt * phase * nimbus_power));
                transmit *= (-dn * dt * u.absorb).exp();
                if transmit < 0.01 {
                    break;
                }
            }
        }
    }
    let body = 1.0 - transmit;
    scattered = add(scattered, mul([u.light[0], u.light[1], u.light[2]], body * u.ambient));
    let rad = u.radius;
    let cover = smoothstep(rad + 0.05, rad - 0.14, ray_dist);
    let rgb = add(mul([u.shadow[0], u.shadow[1], u.shadow[2]], cover), scattered);
    [rgb[0], rgb[1], rgb[2], cover]
}

fn ps_main(u: &NimbusCb, pos: [f32; 2]) -> [f32; 4] {
    let nimbus_power = u.power;
    let nimbus_density = u.density * (1.0 + 0.35 * u.input);
    let acc = nimbus_render(u, pos, nimbus_power, nimbus_density);
    let raw = mul([acc[0], acc[1], acc[2]], u.exposure);
    let peak = raw[0].max(raw[1]).max(raw[2].max(0.0001));
    let hue = mul(raw, tanh3([peak, peak, peak])[0] / peak);
    let col = lerp3(hue, tanh3(raw), WHITE_LIFT * saturate(u.churn));
    let a = saturate(acc[3] * u.alpha_gain);
    [col[0], col[1], col[2], a]
}

fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn saturate3(c: [f32; 3]) -> [f32; 3] {
    [saturate(c[0]), saturate(c[1]), saturate(c[2])]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = saturate((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

fn length(p: [f32; 3]) -> f32 {
    (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt()
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn mul(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn normalize(p: [f32; 3]) -> [f32; 3] {
    mul(p, 1.0 / length(p))
}

struct ReadSlot {
    buffer: wgpu::Buffer,
}

struct Inflight {
    index: usize,
    recv: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    slots: [ReadSlot; 2],
    inflight: Option<Inflight>,
    write: usize,
}

impl Gpu {
    fn open() -> Option<Self> {
        match Self::try_open() {
            Ok(gpu) => Some(gpu),
            Err(error) => {
                crate::install::log_line(&format!("nimbus staying on the CPU: {error}"));
                None
            }
        }
    }

    fn try_open() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| "no Vulkan adapter".to_string())?;
        crate::install::log_line(&format!(
            "nimbus: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        ));
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("nimbus"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|error| error.to_string())?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nimbus"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nimbus-params"),
            size: std::mem::size_of::<NimbusCb>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nimbus"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nimbus"),
            layout: &bind,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nimbus"),
            bind_group_layouts: &[&bind],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nimbus"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nimbus"),
            size: wgpu::Extent3d {
                width: RT,
                height: RT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let row = align256(RT * 4);
        let read_buffer = |device: &wgpu::Device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nimbus-read"),
                size: row as u64 * RT as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let slots = [ReadSlot { buffer: read_buffer(&device) }, ReadSlot { buffer: read_buffer(&device) }];
        let _ = group;
        Ok(Self {
            device,
            queue,
            pipeline,
            params,
            texture,
            view,
            slots,
            inflight: None,
            write: 0,
        })
    }

    fn tick(&mut self, cb: &NimbusCb) -> Result<Option<Vec<u8>>, String> {
        self.device.poll(wgpu::Maintain::Poll);
        let ready = self.collect()?;
        if self.inflight.is_none() {
            self.submit(cb)?;
        }
        Ok(ready)
    }

    #[cfg(test)]
    fn render(&mut self, cb: &NimbusCb) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        loop {
            if let Some(frame) = self.tick(cb)? {
                return Ok(frame);
            }
            if started.elapsed() > Duration::from_millis(800) {
                return Err("nimbus frame timed out".into());
            }
            if started.elapsed() > Duration::from_millis(40) {
                self.device.poll(wgpu::Maintain::Wait);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn collect(&mut self) -> Result<Option<Vec<u8>>, String> {
        let status = match self.inflight.as_ref() {
            None => return Ok(None),
            Some(inflight) => inflight.recv.try_recv(),
        };
        match status {
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("nimbus readback dropped".into()),
            Ok(Err(error)) => Err(error.to_string()),
            Ok(Ok(())) => {
                let inflight = self.inflight.take().ok_or_else(|| "nimbus readback missing".to_string())?;
                let packed = copy_slot(&self.slots[inflight.index].buffer);
                self.slots[inflight.index].buffer.unmap();
                Ok(Some(packed))
            }
        }
    }

    fn submit(&mut self, cb: &NimbusCb) -> Result<(), String> {
        self.queue.write_buffer(&self.params, 0, cb_bytes(cb));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.params.as_entire_binding(),
            }],
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("nimbus") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nimbus"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        let index = self.write;
        self.write ^= 1;
        let row = align256(RT * 4);
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &self.slots[index].buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(RT),
                },
            },
            wgpu::Extent3d {
                width: RT,
                height: RT,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let (send, recv) = std::sync::mpsc::channel();
        self.slots[index].buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
        self.inflight = Some(Inflight { index, recv });
        Ok(())
    }
}

fn copy_slot(buffer: &wgpu::Buffer) -> Vec<u8> {
    let row = align256(RT * 4);
    let mapped = buffer.slice(..).get_mapped_range();
    let mut packed = vec![0u8; (RT * RT * 4) as usize];
    for y in 0..RT {
        let src = (y * row) as usize;
        let dst = (y * RT * 4) as usize;
        packed[dst..dst + (RT * 4) as usize].copy_from_slice(&mapped[src..src + (RT * 4) as usize]);
    }
    packed
}

fn align256(value: u32) -> u32 {
    (value + 255) & !255
}

fn cb_bytes(cb: &NimbusCb) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            (cb as *const NimbusCb) as *const u8,
            std::mem::size_of::<NimbusCb>(),
        )
    }
}

const SHADER: &str = r#"
struct Nimbus {
    uResX: f32,
    uResY: f32,
    uInput: f32,
    uOutput: f32,
    uP_speed: f32,
    uP_camDist: f32,
    uP_focal: f32,
    uP_radius: f32,
    uP_scale: f32,
    uP_churn: f32,
    uP_threshold: f32,
    uP_edgeSoft: f32,
    uP_density: f32,
    uP_absorb: f32,
    uP_shadowAbsorb: f32,
    uP_shadowLift: f32,
    uP_aniso: f32,
    uP_lightSpin: f32,
    uP_power: f32,
    uP_ambient: f32,
    uP_exposure: f32,
    uP_alphaGain: f32,
    uP_swirl: f32,
    _pad1: f32,
    uC_light: vec4<f32>,
    uC_shadow: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Nimbus;

const STEPS: i32 = 56;
const LIGHT_STEPS: i32 = 4;
const DENSITY_OCT: i32 = 4;
const WHITE_LIFT: f32 = 0.35;
const HUE_SPREAD: f32 = 0.6;

fn saturate1(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}

fn saturate3(x: vec3<f32>) -> vec3<f32> {
    return clamp(x, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn smoothstep1(e0: f32, e1: f32, x: f32) -> f32 {
    let t = saturate1((x - e0) / (e1 - e0));
    return t * t * (3.0 - 2.0 * t);
}

fn tanh3(xin: vec3<f32>) -> vec3<f32> {
    let x = clamp(xin, vec3<f32>(-10.0), vec3<f32>(10.0));
    let e = exp(2.0 * x);
    return (e - 1.0) / (e + 1.0);
}

fn hueRotate(c: vec3<f32>, a: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735, 0.57735, 0.57735);
    let ca = cos(a);
    return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

fn phaseHG(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / pow(max(1.0 + g2 - 2.0 * g * c, 0.0001), 1.5);
}

fn rotateAxis(p: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return p * c + cross(axis, p) * s + axis * dot(axis, p) * (1.0 - c);
}

fn density(p: vec3<f32>, nimbusDensity: f32) -> f32 {
    let shell = 1.0 - length(p) / u.uP_radius;
    if (shell <= 0.0) {
        return 0.0;
    }

    let voice = saturate1(u.uP_churn);
    let pulse = saturate1(u.uOutput);
    let selfFold = 0.55 + 0.3 * sin(u.uP_speed * 2.2);
    let amp = selfFold + 0.5 * voice + 0.95 * pulse;
    let axis = normalize(vec3<f32>(0.35 * sin(u.uP_swirl), 1.0, 0.35 * cos(u.uP_swirl)));
    var swirled = rotateAxis(p, axis, u.uP_swirl);
    swirled += vec3<f32>(
        sin(u.uP_swirl + 1.7),
        cos(u.uP_swirl * 2.0 + 0.4),
        sin(u.uP_swirl + 2.9)
    ) * 0.05;
    var q = swirled * (u.uP_scale * (1.0 + 0.16 * voice + 0.5 * pulse));
    var f = 1.0;
    for (var k = 0; k < DENSITY_OCT; k++) {
        q += cos(q.yzx * f + u.uP_speed) * amp / f;
        f *= 1.8;
    }

    let n = (sin(q.x) + sin(q.y) + sin(q.z)) / 3.0 * 0.5 + 0.5;
    let thresh = mix(u.uP_threshold, u.uP_threshold * 0.45, pulse);
    let clump = smoothstep1(thresh, 1.0, n);
    return clump * pow(shell, u.uP_edgeSoft) * nimbusDensity;
}

fn nimbusRender(fragIn: vec2<f32>, nimbusPower: f32, nimbusDensity: f32) -> vec4<f32> {
    let uRes = vec2<f32>(u.uResX, u.uResY);
    let fragCoord = vec2<f32>(fragIn.x, uRes.y - fragIn.y);
    let uv = (2.0 * fragCoord - uRes) / min(uRes.x, uRes.y);
    let ro = vec3<f32>(0.0, 0.0, -u.uP_camDist);
    let rd = normalize(vec3<f32>(uv, u.uP_focal));

    let L = normalize(vec3<f32>(
        cos(u.uP_lightSpin) * 0.7,
        0.45,
        sin(u.uP_lightSpin) * 0.35 + 0.65
    ));
    let phase = phaseHG(dot(rd, L), u.uP_aniso);

    let tStart = max(u.uP_camDist - u.uP_radius, 0.0);
    let dt = (2.0 * u.uP_radius) / f32(STEPS);

    var T = 1.0;
    var scattered = vec3<f32>(0.0, 0.0, 0.0);

    for (var i = 0; i < STEPS; i++) {
        let t = tStart + (f32(i) + 0.5) * dt;
        let p = ro + rd * t;
        let dn = density(p, nimbusDensity);
        if (dn > 0.001) {
            var shadow = 1.0;
            let lstep = u.uP_radius / f32(LIGHT_STEPS);
            for (var s = 1; s <= LIGHT_STEPS; s++) {
                let lp = p + L * (f32(s) - 0.5) * lstep;
                shadow *= exp(-density(lp, nimbusDensity) * lstep * u.uP_shadowAbsorb);
            }
            let hue = HUE_SPREAD * sin(dot(p, vec3<f32>(0.8, 0.5, -0.3)) * 1.4 + u.uP_swirl);
            let light = saturate3(hueRotate(u.uC_light.rgb, hue));
            let lit = mix(u.uC_shadow.rgb * u.uP_shadowLift, light, shadow);
            scattered += T * dn * dt * lit * phase * nimbusPower;
            T *= exp(-dn * dt * u.uP_absorb);
            if (T < 0.01) {
                break;
            }
        }
    }

    let body = 1.0 - T;
    scattered += u.uC_light.rgb * body * u.uP_ambient;

    let rayDist = length(cross(rd, ro));
    let rad = u.uP_radius;
    let cover = smoothstep1(rad + 0.05, rad - 0.14, rayDist);
    let rgb = u.uC_shadow.rgb * cover + scattered;
    return vec4<f32>(rgb, cover);
}

@vertex
fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    return vec4<f32>(xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let nimbusPower = u.uP_power;
    let nimbusDensity = u.uP_density * (1.0 + 0.35 * u.uInput);
    let acc = nimbusRender(pos.xy, nimbusPower, nimbusDensity);
    let raw = acc.rgb * u.uP_exposure;
    let peak = max(max(raw.r, raw.g), max(raw.b, 0.0001));
    let hue = raw / peak * tanh3(vec3<f32>(peak, peak, peak)).x;
    let col = mix(hue, tanh3(raw), WHITE_LIFT * saturate1(u.uP_churn));
    let a = saturate1(acc.a * u.uP_alphaGain);
    return vec4<f32>(col, a);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vulkan_nimbus_renders_on_the_gpu() {
        let Ok(mut gpu) = Gpu::try_open() else {
            return;
        };
        let started = Instant::now();
        let frame = gpu.render(&nimbus_cb(0.85, 0.3, 0.1, 0.4, DEFAULT_ACCENT)).expect("frame");
        let elapsed = started.elapsed();
        let lit = frame.chunks_exact(4).filter(|px| px[3] > 8).count();
        assert!(lit > 40, "lit pixels {lit}");
        assert!(elapsed.as_millis() < 500, "gpu frame took {elapsed:?}");
        let started = Instant::now();
        for _ in 0..12 {
            gpu.tick(&nimbus_cb(0.7, 0.3, 0.1, 0.4, DEFAULT_ACCENT)).expect("tick");
        }
        let burst = started.elapsed();
        assert!(burst.as_millis() < 80, "nimbus ticks stalled the GPU for {burst:?}");
    }

    #[test]
    fn one_frame_has_a_cloud() {
        let mut frame = vec![0u8; (RT * RT * 4) as usize];
        let cb = Clock::new().advance(Instant::now(), 1, 0.6, DEFAULT_ACCENT);
        let started = Instant::now();
        render_cpu(&cb, &mut frame);
        let elapsed = started.elapsed();
        let lit = frame.chunks_exact(4).filter(|px| px[3] > 8).count();
        assert!(lit > 40, "lit pixels {lit}");
        assert!(elapsed.as_millis() < 800, "nimbus frame took {elapsed:?}");
    }
}

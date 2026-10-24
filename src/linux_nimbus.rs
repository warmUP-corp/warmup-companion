use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RT: u32 = 72;
const STEPS: i32 = 36;
const LIGHT_STEPS: i32 = 3;
const DENSITY_OCT: i32 = 4;
const WHITE_LIFT: f32 = 0.35;

struct Params {
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
    light: [f32; 3],
    shadow: [f32; 3],
}

struct Animator {
    noise: f32,
    light: f32,
    swirl: f32,
    age: f32,
    last: Option<Instant>,
    frame: Vec<u8>,
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
            let mut anim = Animator::new();
            let mut gpu = Gpu::open();
            while crate::linux_speech::visual_active() {
                let phase = crate::linux_speech::phase();
                let level = crate::linux_speech::level();
                let accent = [0.65, 0.82, 1.0];
                let started = Instant::now();
                let mut failed = false;
                let rendered = match gpu.as_mut() {
                    Some(gpu) => match gpu.tick(phase, level, accent) {
                        Ok(frame) => frame.map(|pixels| (GPU_RT, pixels)),
                        Err(error) => {
                            crate::install::log_line(&format!("nimbus gpu: {error}"));
                            failed = true;
                            None
                        }
                    },
                    None => Some((RT, anim.render(phase, level, accent))),
                };
                if failed {
                    gpu = None;
                }
                if let Some((px, frame)) = rendered {
                    if let Ok(mut slot) = frame_slot().lock() {
                        *slot = Some((px, frame));
                    }
                    FRAME_GEN.fetch_add(1, Ordering::Relaxed);
                    #[cfg(feature = "gamepad")]
                    crate::linux_vk::request_repaint();
                }
                let rest = Duration::from_millis(33).saturating_sub(started.elapsed());
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

impl Animator {
    fn new() -> Self {
        Self {
            noise: 0.0,
            light: 0.0,
            swirl: 0.0,
            age: 0.0,
            last: None,
            frame: vec![0; (RT * RT * 4) as usize],
        }
    }

    fn render(&mut self, phase: u8, level: f32, accent: [f32; 3]) -> Vec<u8> {
        let now = Instant::now();
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).as_secs_f32().min(0.05))
            .unwrap_or(1.0 / 30.0);
        self.last = Some(now);
        self.age += dt;
        let level = level.clamp(0.0, 1.0);
        let pulse = match phase {
            2 => {
                let think = 0.5 + 0.5 * (self.age * 2.4).sin();
                think.clamp(0.0, 1.0)
            }
            1 => 0.5 + 0.5 * level,
            _ => 0.5,
        };
        let noise_rate = match phase {
            1 => 0.22 + 0.2 * level,
            2 => 0.4 + 0.22 * pulse,
            _ => 0.22,
        };
        self.noise += dt * noise_rate;
        self.light += dt * 0.08;
        let swirl_rate = match phase {
            1 => 0.15 + 0.4 * level,
            2 => 0.30,
            _ => 0.15,
        };
        self.swirl = (self.swirl + dt * swirl_rate).rem_euclid(std::f32::consts::TAU);
        let glow = pulse;
        let params = Params {
            input: 0.55,
            output: pulse,
            speed: self.noise,
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
            light_spin: self.light,
            power: 0.8 + 0.45 * glow,
            ambient: 0.06 + 0.08 * glow,
            exposure: 0.50 + 0.20 * glow,
            alpha_gain: 1.0,
            swirl: self.swirl,
            light: accent,
            shadow: [accent[0] * 0.7, accent[1] * 0.7, accent[2] * 0.7],
        };
        render_into(&params, &mut self.frame);
        self.frame.clone()
    }
}

fn render_into(p: &Params, out: &mut [u8]) {
    let res = RT as f32;
    for y in 0..RT {
        for x in 0..RT {
            let (r, g, b, a) = pixel(p, x as f32 + 0.5, y as f32 + 0.5, res);
            let i = ((y * RT + x) * 4) as usize;
            let pa = a.clamp(0.0, 1.0);
            out[i] = (r.clamp(0.0, 1.0) * pa * 255.0) as u8;
            out[i + 1] = (g.clamp(0.0, 1.0) * pa * 255.0) as u8;
            out[i + 2] = (b.clamp(0.0, 1.0) * pa * 255.0) as u8;
            out[i + 3] = (pa * 255.0) as u8;
        }
    }
}

fn pixel(p: &Params, fx: f32, fy: f32, res: f32) -> (f32, f32, f32, f32) {
    let fy = res - fy;
    let uvx = (2.0 * fx - res) / res;
    let uvy = (2.0 * fy - res) / res;
    let ro = [0.0, 0.0, -p.cam_dist];
    let rd = normalize([uvx, uvy, p.focal]);
    let light_dir = normalize([
        p.light_spin.cos() * 0.7,
        0.45,
        p.light_spin.sin() * 0.35 + 0.65,
    ]);
    let phase = phase_hg(dot(rd, light_dir), p.aniso);
    let ray_dist = length(cross(rd, ro));
    if ray_dist > p.radius + 0.08 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let t_start = (p.cam_dist - p.radius).max(0.0);
    let dt = (2.0 * p.radius) / STEPS as f32;
    let mut transmit = 1.0f32;
    let mut scattered = [0.0f32; 3];
    let nimbus_power = p.power;
    let nimbus_density = p.density * (1.0 + 0.35 * p.input);
    for i in 0..STEPS {
        let t = t_start + (i as f32 + 0.5) * dt;
        let pos = add(ro, mul(rd, t));
        let dn = density(p, pos, nimbus_density);
        if dn > 0.001 {
            let mut shadow = 1.0f32;
            let lstep = p.radius / LIGHT_STEPS as f32;
            for s in 1..=LIGHT_STEPS {
                let lp = add(pos, mul(light_dir, (s as f32 - 0.5) * lstep));
                shadow *= (-density(p, lp, nimbus_density) * lstep * p.shadow_absorb).exp();
            }
            let hue = 0.6 * (dot(pos, [0.8, 0.5, -0.3]) * 1.4 + p.swirl).sin();
            let light = hue_rotate(p.light, hue);
            let lit = lerp3(mul(p.shadow, p.shadow_lift), light, shadow);
            let gain = transmit * dn * dt * phase * nimbus_power;
            scattered = add(scattered, mul(lit, gain));
            transmit *= (-dn * dt * p.absorb).exp();
            if transmit < 0.01 {
                break;
            }
        }
    }
    let body = 1.0 - transmit;
    scattered = add(scattered, mul(p.light, body * p.ambient));
    let cover = smoothstep(p.radius + 0.05, p.radius - 0.14, ray_dist);
    let mut rgb = add(mul(p.shadow, cover), scattered);
    rgb = mul(rgb, p.exposure);
    let peak = rgb[0].max(rgb[1]).max(rgb[2]).max(0.0001);
    let tone = tanh_f(peak);
    let hue = mul(rgb, tone / peak);
    let lifted = [
        tanh_f(rgb[0]),
        tanh_f(rgb[1]),
        tanh_f(rgb[2]),
    ];
    let col = lerp3(hue, lifted, WHITE_LIFT * saturate(p.churn));
    let alpha = saturate(cover * p.alpha_gain);
    (col[0], col[1], col[2], alpha)
}

fn density(p: &Params, pos: [f32; 3], nimbus_density: f32) -> f32 {
    let shell = 1.0 - length(pos) / p.radius;
    if shell <= 0.0 {
        return 0.0;
    }
    let voice = saturate(p.churn);
    let pulse = saturate(p.output);
    let amp = 0.55 + 0.3 * (p.speed * 2.2).sin() + 0.5 * voice + 0.95 * pulse;
    let axis = normalize([0.35 * p.swirl.sin(), 1.0, 0.35 * p.swirl.cos()]);
    let mut swirled = rotate_axis(pos, axis, p.swirl);
    swirled = add(
        swirled,
        mul(
            [
                (p.swirl + 1.7).sin(),
                (p.swirl * 2.0 + 0.4).cos(),
                (p.swirl + 2.9).sin(),
            ],
            0.05,
        ),
    );
    let mut q = mul(swirled, p.scale * (1.0 + 0.16 * voice + 0.5 * pulse));
    let mut f = 1.0f32;
    for _ in 0..DENSITY_OCT {
        q = add(
            q,
            mul(
                [
                    (q[1] * f + p.speed).cos(),
                    (q[2] * f + p.speed).cos(),
                    (q[0] * f + p.speed).cos(),
                ],
                amp / f,
            ),
        );
        f *= 1.8;
    }
    let n = (q[0].sin() + q[1].sin() + q[2].sin()) / 3.0 * 0.5 + 0.5;
    let thresh = lerp(p.threshold, p.threshold * 0.45, pulse);
    let clump = smoothstep(thresh, 1.0, n);
    clump * shell.powf(p.edge_soft) * nimbus_density
}

fn phase_hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    (1.0 - g2) / (1.0 + g2 - 2.0 * g * c).max(0.0001).powf(1.5)
}

fn rotate_axis(pos: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let s = angle.sin();
    let c = angle.cos();
    add(
        add(mul(pos, c), mul(cross(axis, pos), s)),
        mul(axis, dot(axis, pos) * (1.0 - c)),
    )
}

fn hue_rotate(color: [f32; 3], angle: f32) -> [f32; 3] {
    let k = [0.57735, 0.57735, 0.57735];
    let ca = angle.cos();
    add(
        add(mul(color, ca), mul(cross(k, color), angle.sin())),
        mul(k, dot(k, color) * (1.0 - ca)),
    )
}

fn tanh_f(x: f32) -> f32 {
    let x = x.clamp(-10.0, 10.0);
    let e = (2.0 * x).exp();
    (e - 1.0) / (e + 1.0)
}

fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
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
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn mul(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn normalize(p: [f32; 3]) -> [f32; 3] {
    let len = length(p).max(1.0e-6);
    mul(p, 1.0 / len)
}

const GPU_RT: u32 = 128;

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
    noise: f32,
    light: f32,
    swirl: f32,
    age: f32,
    last: Option<Instant>,
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
            size: std::mem::size_of::<GpuParams>() as u64,
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
                width: GPU_RT,
                height: GPU_RT,
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
        let row = align256(GPU_RT * 4);
        let read_buffer = |device: &wgpu::Device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nimbus-read"),
                size: row as u64 * GPU_RT as u64,
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
            noise: 0.0,
            light: 0.0,
            swirl: 0.0,
            age: 0.0,
            last: None,
        })
    }

    fn tick(&mut self, phase: u8, level: f32, accent: [f32; 3]) -> Result<Option<Vec<u8>>, String> {
        self.device.poll(wgpu::Maintain::Poll);
        let ready = self.collect()?;
        if self.inflight.is_none() {
            self.submit(phase, level, accent)?;
        }
        Ok(ready)
    }

    fn render(&mut self, phase: u8, level: f32, accent: [f32; 3]) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        loop {
            if let Some(frame) = self.tick(phase, level, accent)? {
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

    fn submit(&mut self, phase: u8, level: f32, accent: [f32; 3]) -> Result<(), String> {
        let now = Instant::now();
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).as_secs_f32().min(0.05))
            .unwrap_or(1.0 / 30.0);
        self.last = Some(now);
        self.age += dt;
        let level = level.clamp(0.0, 1.0);
        let pulse = match phase {
            2 => (0.5 + 0.5 * (self.age * 2.4).sin()).clamp(0.0, 1.0),
            1 => 0.5 + 0.5 * level,
            _ => 0.5,
        };
        let noise_rate = match phase {
            1 => 0.22 + 0.2 * level,
            2 => 0.4 + 0.22 * pulse,
            _ => 0.22,
        };
        self.noise += dt * noise_rate;
        self.light += dt * 0.08;
        let swirl_rate = match phase {
            1 => 0.15 + 0.4 * level,
            2 => 0.30,
            _ => 0.15,
        };
        self.swirl = (self.swirl + dt * swirl_rate).rem_euclid(std::f32::consts::TAU);
        let params = GpuParams::from_mood(pulse, self.noise, self.light, self.swirl, accent);
        self.queue.write_buffer(&self.params, 0, bytemuck_bytes(&params));
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
        let row = align256(GPU_RT * 4);
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &self.slots[index].buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(GPU_RT),
                },
            },
            wgpu::Extent3d {
                width: GPU_RT,
                height: GPU_RT,
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
    let row = align256(GPU_RT * 4);
    let mapped = buffer.slice(..).get_mapped_range();
    let mut packed = vec![0u8; (GPU_RT * GPU_RT * 4) as usize];
    for y in 0..GPU_RT {
        let src = (y * row) as usize;
        let dst = (y * GPU_RT * 4) as usize;
        packed[dst..dst + (GPU_RT * 4) as usize].copy_from_slice(&mapped[src..src + (GPU_RT * 4) as usize]);
    }
    packed
}

fn align256(value: u32) -> u32 {
    (value + 255) & !255
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GpuParams {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    light: [f32; 4],
    shadow: [f32; 4],
}

impl GpuParams {
    fn from_mood(pulse: f32, speed: f32, light_spin: f32, swirl: f32, accent: [f32; 3]) -> Self {
        let glow = pulse;
        Self {
            a: [GPU_RT as f32, GPU_RT as f32, 0.55, pulse],
            b: [speed, 4.4, 1.8, 2.0],
            c: [1.15, glow, 0.40 - 0.16 * glow, 0.5],
            d: [1.35 + 2.6 * glow, 0.72, 1.5, 0.7],
            e: [0.4, light_spin, 0.8 + 0.45 * glow, 0.06 + 0.08 * glow],
            f: [0.50 + 0.20 * glow, 1.0, swirl, 0.0],
            light: [accent[0], accent[1], accent[2], 1.0],
            shadow: [accent[0] * 0.7, accent[1] * 0.7, accent[2] * 0.7, 1.0],
        }
    }
}

fn bytemuck_bytes(params: &GpuParams) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            (params as *const GpuParams) as *const u8,
            std::mem::size_of::<GpuParams>(),
        )
    }
}

const SHADER: &str = r#"
struct Params {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
    d: vec4<f32>,
    e: vec4<f32>,
    f: vec4<f32>,
    light: vec4<f32>,
    shadow: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Params;

fn saturate(x: f32) -> f32 { return clamp(x, 0.0, 1.0); }
fn lerp(a: f32, b: f32, t: f32) -> f32 { return a + (b - a) * t; }
fn lerp3(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> { return mix(a, b, t); }
fn smoothstep3(e0: f32, e1: f32, x: f32) -> f32 {
    let t = saturate((x - e0) / (e1 - e0));
    return t * t * (3.0 - 2.0 * t);
}
fn tanh1(x: f32) -> f32 {
    let c = clamp(x, -10.0, 10.0);
    let e = exp(2.0 * c);
    return (e - 1.0) / (e + 1.0);
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
fn hueRotate(col: vec3<f32>, ang: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735, 0.57735, 0.57735);
    let ca = cos(ang);
    return col * ca + cross(k, col) * sin(ang) + k * dot(k, col) * (1.0 - ca);
}
fn density(p: vec3<f32>, nimbusDensity: f32) -> f32 {
    let radius = u.b.w;
    let shell = 1.0 - length(p) / radius;
    if (shell <= 0.0) { return 0.0; }
    let voice = saturate(u.c.y);
    let pulse = saturate(u.a.w);
    let amp = 0.55 + 0.3 * sin(u.b.x * 2.2) + 0.5 * voice + 0.95 * pulse;
    let axis = normalize(vec3<f32>(0.35 * sin(u.f.z), 1.0, 0.35 * cos(u.f.z)));
    var swirled = rotateAxis(p, axis, u.f.z);
    swirled = swirled + vec3<f32>(sin(u.f.z + 1.7), cos(u.f.z * 2.0 + 0.4), sin(u.f.z + 2.9)) * 0.05;
    var q = swirled * (u.c.x * (1.0 + 0.16 * voice + 0.5 * pulse));
    var freq = 1.0;
    for (var k = 0; k < 4; k = k + 1) {
        q = q + cos(q.yzx * freq + vec3<f32>(u.b.x)) * amp / freq;
        freq = freq * 1.8;
    }
    let n = (sin(q.x) + sin(q.y) + sin(q.z)) / 3.0 * 0.5 + 0.5;
    let thresh = lerp(u.c.z, u.c.z * 0.45, pulse);
    let clump = smoothstep3(thresh, 1.0, n);
    return clump * pow(shell, u.c.w) * nimbusDensity;
}

@vertex
fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    return vec4<f32>(xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let res = vec2<f32>(u.a.x, u.a.y);
    let frag = vec2<f32>(pos.x, res.y - pos.y);
    let uv = (2.0 * frag - res) / min(res.x, res.y);
    let ro = vec3<f32>(0.0, 0.0, -u.b.y);
    let rd = normalize(vec3<f32>(uv, u.b.z));
    let L = normalize(vec3<f32>(cos(u.e.y) * 0.7, 0.45, sin(u.e.y) * 0.35 + 0.65));
    let phase = phaseHG(dot(rd, L), u.e.x);
    let rayDist = length(cross(rd, ro));
    let rad = u.b.w;
    if (rayDist > rad + 0.08) { return vec4<f32>(0.0); }
    let tStart = max(u.b.y - rad, 0.0);
    let dt = (2.0 * rad) / 48.0;
    var T = 1.0;
    var scattered = vec3<f32>(0.0);
    let nimbusPower = u.e.z;
    let nimbusDensity = u.d.x * (1.0 + 0.35 * u.a.z);
    for (var i = 0; i < 48; i = i + 1) {
        let t = tStart + (f32(i) + 0.5) * dt;
        let p = ro + rd * t;
        let dn = density(p, nimbusDensity);
        if (dn > 0.001) {
            var shadow = 1.0;
            let lstep = rad / 4.0;
            for (var s = 1; s <= 4; s = s + 1) {
                let lp = p + L * (f32(s) - 0.5) * lstep;
                shadow = shadow * exp(-density(lp, nimbusDensity) * lstep * u.d.z);
            }
            let hue = 0.6 * sin(dot(p, vec3<f32>(0.8, 0.5, -0.3)) * 1.4 + u.f.z);
            let light = saturate3(hueRotate(u.light.rgb, hue));
            let lit = lerp3(u.shadow.rgb * u.d.w, light, shadow);
            scattered = scattered + T * dn * dt * lit * phase * nimbusPower;
            T = T * exp(-dn * dt * u.d.y);
            if (T < 0.01) { break; }
        }
    }
    let body = 1.0 - T;
    scattered = scattered + u.light.rgb * body * u.e.w;
    let cover = smoothstep3(rad + 0.05, rad - 0.14, rayDist);
    var rgb = u.shadow.rgb * cover + scattered;
    rgb = rgb * u.f.x;
    let peak = max(max(rgb.r, rgb.g), max(rgb.b, 0.0001));
    let huev = rgb / peak * tanh1(peak);
    let col = lerp3(huev, vec3<f32>(tanh1(rgb.r), tanh1(rgb.g), tanh1(rgb.b)), 0.35 * saturate(u.c.y));
    return vec4<f32>(col, saturate(cover * u.f.y));
}
fn saturate3(c: vec3<f32>) -> vec3<f32> { return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)); }
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
        let frame = gpu.render(1, 0.7, [0.65, 0.82, 1.0]).expect("frame");
        let elapsed = started.elapsed();
        let lit = frame.chunks_exact(4).filter(|px| px[3] > 8).count();
        assert!(lit > 40, "lit pixels {lit}");
        assert!(elapsed.as_millis() < 500, "gpu frame took {elapsed:?}");
        let started = Instant::now();
        for _ in 0..12 {
            gpu.tick(1, 0.4, [0.65, 0.82, 1.0]).expect("tick");
        }
        let burst = started.elapsed();
        assert!(burst.as_millis() < 80, "nimbus ticks stalled the GPU for {burst:?}");
    }

    #[test]
    fn one_frame_has_a_cloud() {
        let mut anim = Animator::new();
        let started = Instant::now();
        let frame = anim.render(1, 0.6, [0.65, 0.82, 1.0]);
        let elapsed = started.elapsed();
        let lit = frame.chunks_exact(4).filter(|px| px[3] > 8).count();
        assert!(lit > 40, "lit pixels {lit}");
        assert!(elapsed.as_millis() < 800, "nimbus frame took {elapsed:?}");
    }
}

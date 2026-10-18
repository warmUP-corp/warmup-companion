use std::collections::HashMap;
use std::time::Instant;

use super::draw::{self, Frame, Glide};
use crate::mono_ui::Gfx;

pub struct Orb<'a> {
    pub pixels: &'a [u8],
    pub px: u32,
    pub x: f32,
    pub y: f32,
    pub size: f32,
}

pub struct Presenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    base: Option<Image>,
    keys: HashMap<(usize, usize), KeyGpu>,
    orb: Image,
    orb_live: bool,
    target_size: (u32, u32),
    stamp: u64,
    uniform: wgpu::Buffer,
    format: wgpu::TextureFormat,
    alpha: wgpu::CompositeAlphaMode,
    present_mode: wgpu::PresentMode,
    shown: u32,
    mode: Mode,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Keys,
    Sheet,
    Full,
}

struct Image {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

struct KeyGpu {
    image: Image,
    patch: Patch,
}

#[derive(Clone, Copy)]
struct Patch {
    dest_x: f32,
    dest_y: f32,
    width: f32,
    height: f32,
    body_x: f32,
    body_y: f32,
    body_w: f32,
    body_h: f32,
    radius: f32,
    ring: [f32; 4],
    ring_w: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Uniforms {
    tex: [f32; 4],
    key: [f32; 4],
    body: [f32; 4],
    ring: [f32; 4],
    extra: [f32; 4],
    orb: [f32; 4],
}

impl Presenter {
    pub fn open(display: *mut std::ffi::c_void, window: *mut std::ffi::c_void) -> Result<Self, String> {
        let display = std::ptr::NonNull::new(display).ok_or_else(|| "keyboard display is missing".to_string())?;
        let window = std::ptr::NonNull::new(window).ok_or_else(|| "keyboard surface is missing".to_string())?;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: wgpu::rwh::RawDisplayHandle::Wayland(wgpu::rwh::WaylandDisplayHandle::new(display)),
                raw_window_handle: wgpu::rwh::RawWindowHandle::Wayland(wgpu::rwh::WaylandWindowHandle::new(window)),
            })
        }
        .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| "no Vulkan adapter for the keyboard".to_string())?;
        let adapter_limit = adapter.limits().max_texture_dimension_2d;
        let mut limits = wgpu::Limits::default();
        if adapter_limit < limits.max_texture_dimension_2d {
            limits.max_texture_dimension_2d = adapter_limit;
        }
        if limits.max_texture_dimension_2d < 3072 {
            return Err(format!("GPU textures stop at {}", limits.max_texture_dimension_2d));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("keyboard"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|error| error.to_string())?;
        let caps = surface.get_capabilities(&adapter);
        let format = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ]
        .into_iter()
        .find(|format| caps.formats.contains(format))
        .ok_or_else(|| "the keyboard surface has no color format".to_string())?;
        let alpha = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ]
        .into_iter()
        .find(|mode| caps.alpha_modes.contains(mode))
        .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };
        crate::install::log_line(&format!(
            "keyboard: {} ({:?}) texture {} {:?} {:?}",
            adapter.get_info().name,
            adapter.get_info().backend,
            device.limits().max_texture_dimension_2d,
            alpha,
            present_mode
        ));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("keyboard"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("keyboard"),
            entries: &[
                tex_entry(0),
                samp_entry(1),
                tex_entry(2),
                samp_entry(3),
                tex_entry(4),
                samp_entry(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("keyboard"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("keyboard"),
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
                    format,
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
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("keyboard-uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let orb = upload(&device, &queue, "keyboard-orb", &[0, 0, 0, 0], 1, 1);
        Ok(Self {
            surface,
            device,
            queue,
            pipeline,
            layout,
            nearest,
            linear,
            base: None,
            keys: HashMap::new(),
            orb,
            orb_live: false,
            target_size: (0, 0),
            stamp: 0,
            uniform,
            format,
            alpha,
            present_mode,
            shown: 0,
            mode: Mode::Keys,
        })
    }

    pub fn present(
        &mut self,
        frame: &Frame,
        g: &mut Gfx,
        glide: &mut Glide,
        orb: Option<&Orb<'_>>,
    ) -> Result<(), String> {
        let width = (frame.logical_w * frame.scale.max(1.0)).round().max(1.0) as u32;
        let height = (frame.logical_h * frame.scale.max(1.0)).round().max(1.0) as u32;
        self.configure(width, height)?;
        let target = self.acquire()?;
        let started = Instant::now();
        let key_id = (frame.sel.row, frame.sel.col);
        let pressed_elsewhere = frame.pressed.is_some_and(|(pos, _)| pos != frame.sel);
        let mut full = frame.sheet || pressed_elsewhere;
        let mut raster = false;
        if !full {
            if self.mode != Mode::Keys {
                self.mode = Mode::Keys;
                self.base = None;
                self.stamp = 0;
                self.keys.clear();
            }
            let stamp = draw::keyboard_stamp(frame);
            let strip_live = strip_live(glide, frame);
            if strip_live || self.base.is_none() || self.stamp != stamp {
                draw::paint_base(g, frame, glide);
                self.base = Some(upload(&self.device, &self.queue, "keyboard-base", &g.pixels(), g.width(), g.height()));
                self.keys.clear();
                self.stamp = if strip_live { !stamp } else { stamp };
                raster = true;
            }
            if !self.keys.contains_key(&key_id) {
                match draw::paint_selected_key(g, frame, 1.0) {
                    Some(patch) => {
                        let image = upload(&self.device, &self.queue, "keyboard-key", &g.pixels(), g.width(), g.height());
                        self.keys.insert(
                            key_id,
                            KeyGpu {
                                image,
                                patch: Patch {
                                    dest_x: patch.dest_x,
                                    dest_y: patch.dest_y,
                                    width: patch.width as f32,
                                    height: patch.height as f32,
                                    body_x: patch.body_x,
                                    body_y: patch.body_y,
                                    body_w: patch.body_w,
                                    body_h: patch.body_h,
                                    radius: patch.radius,
                                    ring: patch.ring,
                                    ring_w: patch.ring_w,
                                },
                            },
                        );
                        raster = true;
                    }
                    None => full = true,
                }
            }
        }
        if full {
            let mode = if frame.sheet { Mode::Sheet } else { Mode::Full };
            let stamp = full_stamp(frame, glide);
            if self.mode != mode || self.base.is_none() || self.stamp != stamp {
                draw::paint_frame(g, frame, glide);
                self.base = Some(upload(&self.device, &self.queue, "keyboard-frame", &g.pixels(), g.width(), g.height()));
                self.keys.clear();
                raster = true;
            }
            self.stamp = stamp;
            self.mode = mode;
        }
        if let Some(orb) = orb {
            if orb.px > 0 && orb.pixels.len() >= (orb.px * orb.px * 4) as usize {
                self.orb = upload(&self.device, &self.queue, "keyboard-orb", orb.pixels, orb.px, orb.px);
                self.orb_live = true;
            }
        } else if self.orb_live {
            self.orb = upload(&self.device, &self.queue, "keyboard-orb", &[0, 0, 0, 0], 1, 1);
            self.orb_live = false;
        }
        let raster_ms = started.elapsed().as_secs_f32() * 1000.0;
        let patch = if full {
            Patch {
                dest_x: 0.0,
                dest_y: 0.0,
                width: 1.0,
                height: 1.0,
                body_x: 0.0,
                body_y: 0.0,
                body_w: 1.0,
                body_h: 1.0,
                radius: 0.0,
                ring: [0.0; 4],
                ring_w: 0.0,
            }
        } else {
            self.keys.get(&key_id).ok_or_else(|| "missing selected key".to_string())?.patch
        };
        let press = frame.pressed.filter(|(pos, _)| *pos == frame.sel).map(|(_, scale)| scale).unwrap_or(1.0);
        let show_key = if full { 0.0 } else { 1.0 };
        let orb_box = orb
            .filter(|_| self.orb_live)
            .map(|orb| [orb.x, orb.y, orb.size, orb.size])
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let uniforms = Uniforms {
            tex: [width as f32, height as f32, 0.0, 0.0],
            key: [patch.dest_x, patch.dest_y, patch.width, patch.height],
            body: [patch.body_x, patch.body_y, patch.body_w, patch.body_h],
            ring: patch.ring,
            extra: [patch.ring_w, patch.radius, show_key, press],
            orb: orb_box,
        };
        self.queue.write_buffer(&self.uniform, 0, bytes_of(&uniforms));
        let gpu_at = Instant::now();
        let base = self.base.as_ref().ok_or_else(|| "missing keyboard texture".to_string())?;
        let key_fallback;
        let key_view = if full {
            key_fallback = None;
            &self.orb.view
        } else {
            key_fallback = Some(self.keys.get(&key_id).ok_or_else(|| "missing selected key".to_string())?);
            &key_fallback.as_ref().unwrap().image.view
        };
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("keyboard"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&base.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.nearest),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(key_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.nearest),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.orb.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.linear),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });
        let view = target.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("keyboard") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("keyboard"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
        self.queue.submit(Some(encoder.finish()));
        let suboptimal = target.suboptimal;
        target.present();
        if suboptimal {
            self.target_size = (0, 0);
        }
        let gpu_ms = gpu_at.elapsed().as_secs_f32() * 1000.0;
        if self.shown < 4 {
            let kind = if raster { "raster" } else { "cached" };
            crate::install::log_line(&format!(
                "keyboard gpu {width}x{height} {kind} {raster_ms:.2}ms present {gpu_ms:.2}ms"
            ));
            self.shown += 1;
        }
        let strip_moving = glide
            .strip_at
            .is_some_and(|at| at.elapsed().as_secs_f32() * 1000.0 < crate::vk_motion::STRIP_ENTER_MS);
        let pressing = frame.pressed.is_some_and(|(_, scale)| scale < 0.999);
        glide.animating = strip_moving || pressing;
        Ok(())
    }

    fn configure(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self.target_size == (width, height) {
            return Ok(());
        }
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(format!("keyboard {width}x{height} exceeds the GPU surface ({max})"));
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            width,
            height,
            present_mode: self.present_mode,
            alpha_mode: self.alpha,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        let surface = &self.surface;
        let device = &self.device;
        let configured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            surface.configure(device, &config);
        }));
        if let Err(payload) = configured {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_string()))
                .unwrap_or_else(|| "surface configure failed".to_string());
            return Err(message);
        }
        self.target_size = (width, height);
        self.base = None;
        self.keys.clear();
        self.stamp = 0;
        Ok(())
    }

    fn acquire(&mut self) -> Result<wgpu::SurfaceTexture, String> {
        match self.surface.get_current_texture() {
            Ok(texture) => Ok(texture),
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                let (width, height) = self.target_size;
                self.target_size = (0, 0);
                self.configure(width, height)?;
                self.surface.get_current_texture().map_err(|error| error.to_string())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

fn full_stamp(frame: &Frame, glide: &Glide) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    draw::keyboard_stamp(frame).hash(&mut hasher);
    frame.sheet.hash(&mut hasher);
    if !frame.sheet {
        frame.sel.row.hash(&mut hasher);
        frame.sel.col.hash(&mut hasher);
        frame.pressed.map(|(pos, scale)| (pos.row, pos.col, scale.to_bits())).hash(&mut hasher);
        strip_live(glide, frame).then(Instant::now).hash(&mut hasher);
        glide
            .ring
            .map(|(l, t, r, b)| [l.to_bits(), t.to_bits(), r.to_bits(), b.to_bits()])
            .hash(&mut hasher);
    }
    hasher.finish()
}

fn strip_live(glide: &Glide, frame: &Frame) -> bool {
    frame.strip.is_some()
        && match glide.strip_at {
            None => true,
            Some(at) => at.elapsed().as_secs_f32() * 1000.0 < crate::vk_motion::STRIP_ENTER_MS,
        }
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, pixels: &[u8], width: u32, height: u32) -> Image {
    let width = width.max(1);
    let height = height.max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (padded, stride) = pad_rows(pixels, width, height);
    queue.write_texture(
        texture.as_image_copy(),
        &padded,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Image { texture, view, width, height }
}

fn pad_rows(pixels: &[u8], width: u32, height: u32) -> (Vec<u8>, u32) {
    let row = width.max(1) * 4;
    let stride = (row + 255) & !255;
    if stride == row && pixels.len() >= (row * height) as usize {
        return (pixels.to_vec(), row);
    }
    let mut out = vec![0u8; stride as usize * height.max(1) as usize];
    for y in 0..height.max(1) {
        let src = (y * row) as usize;
        let dst = (y * stride) as usize;
        let end = (src + row as usize).min(pixels.len());
        if src < end {
            out[dst..dst + (end - src)].copy_from_slice(&pixels[src..end]);
        }
    }
    (out, stride)
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn samp_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn bytes_of(value: &Uniforms) -> &[u8] {
    unsafe { std::slice::from_raw_parts((value as *const Uniforms) as *const u8, std::mem::size_of::<Uniforms>()) }
}

const SHADER: &str = r#"
struct Uniforms {
    tex: vec4<f32>,
    key: vec4<f32>,
    body: vec4<f32>,
    ring: vec4<f32>,
    extra: vec4<f32>,
    orb: vec4<f32>,
}
@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var base_samp: sampler;
@group(0) @binding(2) var key_tex: texture_2d<f32>;
@group(0) @binding(3) var key_samp: sampler;
@group(0) @binding(4) var orb_tex: texture_2d<f32>;
@group(0) @binding(5) var orb_samp: sampler;
@group(0) @binding(6) var<uniform> u: Uniforms;

@vertex
fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    return vec4<f32>(xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

fn sd_round(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p - size * 0.5) - size * 0.5 + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / max(u.tex.xy, vec2<f32>(1.0));
    var color = textureSample(base_tex, base_samp, uv);
    let press = max(u.extra.w, 0.9);
    let center = u.body.xy + u.body.zw * 0.5;
    let sample_at = center + (pos.xy - center) / press;
    let local = sample_at - u.key.xy;
    if (u.extra.z > 0.5 && all(local >= vec2<f32>(0.0)) && all(local < u.key.zw)) {
        let key = textureSample(key_tex, key_samp, local / max(u.key.zw, vec2<f32>(1.0)));
        color = over(color, key);
    }
    let dist = sd_round(pos.xy - u.body.xy, u.body.zw, u.extra.y);
    let half = max(u.extra.x * 0.5, 0.5);
    let band = abs(dist + half) - half;
    let ring_a = (1.0 - smoothstep(0.0, 1.25, band)) * u.ring.a;
    color = over(color, vec4<f32>(u.ring.rgb * ring_a, ring_a));
    let orb_local = pos.xy - u.orb.xy;
    if (u.orb.z > 1.5 && all(orb_local >= vec2<f32>(0.0)) && all(orb_local < u.orb.zw)) {
        let orb = textureSample(orb_tex, orb_samp, orb_local / max(u.orb.zw, vec2<f32>(1.0)));
        color = over(color, orb);
    }
    return color;
}
"#;

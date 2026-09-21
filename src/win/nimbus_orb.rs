//! SHDR-21 Nimbus (orbkit, MIT) drawn with the D3D11 device the VK already owns.
//!
//! The React/WebGL original marches a sphere-bounded density field: Beer-Lambert
//! transmittance, a short shadow march, and a Henyey-Greenstein phase so the
//! lit limb blooms. Step counts match that shader. The render target stays
//! small — the cloud is soft, and D2D upscales it into the key or the overlay.

use std::time::Instant;

use windows::core::s;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap1, ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1,
};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::{
    ID3DBlob, ID3DInclude, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11Texture2D, ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER,
    D3D11_BIND_RENDER_TARGET, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ, D3D11_CULL_NONE,
    D3D11_FILL_SOLID, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_RASTERIZER_DESC,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

/// Square render target. 160 is enough for a soft volume blit up to ~480px.
const RT: u32 = 160;

/// Orb state. The body stays the same size and the same theme color.
/// State shows up as how the cloud moves.
pub enum NimbusMood {
    /// Helper is up, mic not capturing yet. Slow drift.
    Idle,
    /// User is talking. Swirl and light speed follow `level` (0..1).
    Speaking { level: f32 },
    /// Recording stopped. A steady, quicker orbit — not driven by the mic.
    Thinking,
}

/// Shared body size. Density in the shader scales with this, so every state
/// uses the same value.
const NIMBUS_BODY: f32 = 0.55;

/// 24 floats then two float4s. 96 is 16-byte aligned, so the colors land where
/// FXC packs `float4` without inserting padding.
#[repr(C)]
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
    _pad0: f32,
    _pad1: f32,
    light: [f32; 4],
    shadow: [f32; 4],
}

/// Synthesized "thinking" volumes from the orbkit driver. Transcription has no
/// live mic, so the cloud breathes on this instead of speech energy.
pub fn thinking_volumes(t: f32) -> (f32, f32) {
    let base = 0.38 + 0.07 * (t * 0.7).sin();
    let wander = 0.05 * (t * 2.1).sin() * (t * 0.37 + 1.2).sin();
    let input = (base + wander).clamp(0.0, 1.0);
    let output = (0.48 + 0.12 * (t * 1.05 + 0.6).sin()).clamp(0.0, 1.0);
    (input, output)
}

const SHADER: &str = r#"
cbuffer Nimbus : register(b0) {
    float uResX;
    float uResY;
    float uInput;
    float uOutput;
    float uP_speed;
    float uP_camDist;
    float uP_focal;
    float uP_radius;
    float uP_scale;
    float uP_churn;
    float uP_threshold;
    float uP_edgeSoft;
    float uP_density;
    float uP_absorb;
    float uP_shadowAbsorb;
    float uP_shadowLift;
    float uP_aniso;
    float uP_lightSpin;
    float uP_power;
    float uP_ambient;
    float uP_exposure;
    float uP_alphaGain;
    float _pad0;
    float _pad1;
    float4 uC_light;
    float4 uC_shadow;
};

#define STEPS 56
#define LIGHT_STEPS 4
#define DENSITY_OCT 4

float3 tanh3(float3 x) {
    x = clamp(x, -10.0, 10.0);
    float3 e = exp(2.0 * x);
    return (e - 1.0) / (e + 1.0);
}

float phaseHG(float c, float g) {
    float g2 = g * g;
    return (1.0 - g2) / pow(max(1.0 + g2 - 2.0 * g * c, 0.0001), 1.5);
}

float density(float3 p, float animTime, float nimbusDensity) {
    float shell = 1.0 - length(p) / uP_radius;
    if (shell <= 0.0) return 0.0;

    float3 q = p * uP_scale;
    float f = 1.0;
    [unroll]
    for (int k = 0; k < DENSITY_OCT; k++) {
        q += cos(q.yzx * f + animTime * uP_churn) / f;
        f *= 1.8;
    }

    float n = (sin(q.x) + sin(q.y) + sin(q.z)) / 3.0 * 0.5 + 0.5;
    float clump = smoothstep(uP_threshold, 1.0, n);
    return clump * pow(shell, uP_edgeSoft) * nimbusDensity;
}

float4 nimbusRender(float2 fragCoord, float animTime, float nimbusPower, float nimbusDensity) {
    float2 uRes = float2(uResX, uResY);
    // GL fragcoord is bottom-left; D3D is top-left. Flip so the light orbit matches.
    fragCoord.y = uRes.y - fragCoord.y;
    float2 uv = (2.0 * fragCoord - uRes) / min(uRes.x, uRes.y);
    float3 ro = float3(0.0, 0.0, -uP_camDist);
    float3 rd = normalize(float3(uv, uP_focal));

    // Positive z: the light sits behind the cloud, where forward scatter blooms.
    float3 L = normalize(float3(
        cos(animTime * uP_lightSpin) * 0.7,
        0.45,
        sin(animTime * uP_lightSpin) * 0.35 + 0.65
    ));
    float phase = phaseHG(dot(rd, L), uP_aniso);

    float tStart = max(uP_camDist - uP_radius, 0.0);
    float dt = (2.0 * uP_radius) / float(STEPS);

    float T = 1.0;
    float3 scattered = float3(0.0, 0.0, 0.0);

    [loop]
    for (int i = 0; i < STEPS; i++) {
        float t = tStart + (float(i) + 0.5) * dt;
        float3 p = ro + rd * t;
        float dn = density(p, animTime, nimbusDensity);
        if (dn > 0.001) {
            float shadow = 1.0;
            float lstep = uP_radius / float(LIGHT_STEPS);
            [loop]
            for (int s = 1; s <= LIGHT_STEPS; s++) {
                float3 lp = p + L * (float(s) - 0.5) * lstep;
                shadow *= exp(-density(lp, animTime, nimbusDensity) * lstep * uP_shadowAbsorb);
            }
            // Shadow once, inside the mix. A second multiply kills the cool colour.
            float3 lit = lerp(uC_shadow.rgb * uP_shadowLift, uC_light.rgb, shadow);
            scattered += T * dn * dt * lit * phase * nimbusPower;
            T *= exp(-dn * dt * uP_absorb);
            if (T < 0.01) break;
        }
    }

    float body = 1.0 - T;
    scattered += uC_shadow.rgb * body * uP_ambient;
    return float4(scattered, body);
}

struct VsOut { float4 pos : SV_Position; };

VsOut vs_main(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

float4 ps_main(float4 pos : SV_Position) : SV_Target {
    float nimbusPower = uP_power * (0.7 + 0.9 * uOutput);
    float nimbusDensity = uP_density * (1.0 + 0.35 * uInput);
    float4 acc = nimbusRender(pos.xy, uP_speed, nimbusPower, nimbusDensity);
    float3 col = tanh3(acc.rgb * uP_exposure);
    float a = saturate(acc.a * uP_alphaGain);
    // Scattered light is already the premultiplied colour. Do not scale by alpha.
    return float4(col, a);
}
"#;

pub struct NimbusOrb {
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cb: ID3D11Buffer,
    raster: ID3D11RasterizerState,
    tex: ID3D11Texture2D,
    /// CPU copy of `tex`. D2D must not sample the live render target — rewriting
    /// it each frame blanks the bitmap the keyboard is showing (a one-frame flash,
    /// then the old orb).
    staging: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    bitmap: ID2D1Bitmap1,
    anim_speed: f32,
    churn: f32,
    spin: f32,
    clock: f32,
    vol_in: f32,
    vol_out: f32,
    last: Option<Instant>,
}

impl NimbusOrb {
    pub unsafe fn create(device: &ID3D11Device, d2d: &ID2D1DeviceContext) -> Result<Self, String> {
        let ctx = device
            .GetImmediateContext()
            .map_err(|e| format!("GetImmediateContext: {e}"))?;
        let vs_bc = compile_shader(SHADER, s!("vs_main"), s!("vs_4_0"))?;
        let ps_bc = compile_shader(SHADER, s!("ps_main"), s!("ps_4_0"))?;
        let mut vs = None;
        device
            .CreateVertexShader(&vs_bc, None, Some(&mut vs))
            .map_err(|e| format!("CreateVertexShader: {e}"))?;
        let mut ps = None;
        device
            .CreatePixelShader(&ps_bc, None, Some(&mut ps))
            .map_err(|e| format!("CreatePixelShader: {e}"))?;
        let vs = vs.ok_or("CreateVertexShader returned null")?;
        let ps = ps.ok_or("CreatePixelShader returned null")?;

        let cb_desc = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of::<NimbusCb>() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        let mut cb = None;
        device
            .CreateBuffer(&cb_desc, None, Some(&mut cb))
            .map_err(|e| format!("CreateBuffer: {e}"))?;
        let cb = cb.ok_or("CreateBuffer returned null")?;

        let rs_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            DepthClipEnable: BOOL(0),
            ..Default::default()
        };
        let mut raster = None;
        device
            .CreateRasterizerState(&rs_desc, Some(&mut raster))
            .map_err(|e| format!("CreateRasterizerState: {e}"))?;
        let raster = raster.ok_or("CreateRasterizerState returned null")?;

        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: RT,
            Height: RT,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut tex = None;
        device
            .CreateTexture2D(&tex_desc, None, Some(&mut tex))
            .map_err(|e| format!("CreateTexture2D: {e}"))?;
        let tex = tex.ok_or("CreateTexture2D returned null")?;
        let mut rtv = None;
        device
            .CreateRenderTargetView(&tex, None, Some(&mut rtv))
            .map_err(|e| format!("CreateRenderTargetView: {e}"))?;
        let rtv = rtv.ok_or("CreateRenderTargetView returned null")?;

        let stage_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..tex_desc
        };
        let mut staging = None;
        device
            .CreateTexture2D(&stage_desc, None, Some(&mut staging))
            .map_err(|e| format!("CreateTexture2D staging: {e}"))?;
        let staging = staging.ok_or("CreateTexture2D staging returned null")?;

        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap = d2d
            .CreateBitmap(
                D2D_SIZE_U {
                    width: RT,
                    height: RT,
                },
                None,
                0,
                &props,
            )
            .map_err(|e| format!("CreateBitmap: {e}"))?;

        let (vol_in, vol_out) = thinking_volumes(0.0);
        let anim_speed = 0.1 + (1.0 - (vol_out - 1.0).powi(2)) * 0.9;
        Ok(Self {
            ctx,
            vs,
            ps,
            cb,
            raster,
            tex,
            staging,
            rtv,
            bitmap,
            anim_speed,
            churn: 0.16,
            spin: 0.04,
            clock: 0.0,
            vol_in,
            vol_out,
            last: None,
        })
    }

    pub fn bitmap(&self) -> &ID2D1Bitmap1 {
        &self.bitmap
    }

    /// Advance the clock and draw one frame into the bitmap.
    /// The caller must not be inside a D2D `BeginDraw` — this uses the same
    /// immediate context.
    pub unsafe fn render(
        &mut self,
        now: Instant,
        mood: NimbusMood,
        accent: u32,
    ) -> Result<(), String> {
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).as_secs_f32().min(0.05))
            .unwrap_or(1.0 / 60.0);
        self.last = Some(now);
        // Same body in every state. Voice and transcription change the
        // motion, not the color and not the size.
        self.vol_in = NIMBUS_BODY;
        self.vol_out = NIMBUS_BODY;
        let level = match mood {
            NimbusMood::Speaking { level } => level.clamp(0.0, 1.0),
            _ => 0.0,
        };
        // The light only drifts. Voice stirs the noise field.
        let (speed_t, churn_t, spin_t) = match mood {
            NimbusMood::Idle => (0.14, 0.06, 0.02),
            NimbusMood::Speaking { .. } => (
                0.16 + 0.14 * level,
                0.06 + 0.85 * level,
                0.02 + 0.03 * level,
            ),
            NimbusMood::Thinking => {
                let pulse = 0.5 + 0.5 * (self.clock * 0.22).sin();
                (0.48 + 0.12 * pulse, 0.28, 0.055)
            }
        };
        let k = 1.0 - (-dt * 8.0).exp();
        let k_noise = 1.0 - (-dt * 16.0).exp();
        self.anim_speed += (speed_t - self.anim_speed) * k;
        self.churn += (churn_t - self.churn) * k_noise;
        self.spin += (spin_t - self.spin) * k;
        self.clock += dt * self.anim_speed * 10.0;
        let cb = NimbusCb {
            res_x: RT as f32,
            res_y: RT as f32,
            input: self.vol_in,
            output: self.vol_out,
            speed: self.clock,
            cam_dist: 4.4,
            focal: 1.8,
            radius: 2.0,
            // Finer noise and a higher threshold so the sphere breaks into
            // wisps instead of filling in as one colour.
            scale: 1.15,
            churn: self.churn,
            threshold: 0.32,
            edge_soft: 0.5,
            density: 2.2,
            absorb: 0.72,
            shadow_absorb: 1.5,
            shadow_lift: 0.48,
            aniso: 0.4,
            light_spin: self.spin,
            power: 1.15,
            ambient: 0.08,
            exposure: 0.68,
            alpha_gain: 1.0,
            _pad0: 0.0,
            _pad1: 0.0,
            light: theme_light(accent),
            shadow: theme_shadow(accent),
        };
        self.ctx
            .UpdateSubresource(&self.cb, 0, None, &cb as *const NimbusCb as *const _, 0, 0);
        self.ctx
            .OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
        self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: RT as f32,
            Height: RT as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]));
        self.ctx.RSSetState(&self.raster);
        self.ctx.IASetInputLayout(None);
        self.ctx
            .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.ctx.VSSetShader(&self.vs, None);
        self.ctx.PSSetShader(&self.ps, None);
        self.ctx
            .PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
        self.ctx
            .ClearRenderTargetView(&self.rtv, &[0.0, 0.0, 0.0, 0.0]);
        self.ctx.Draw(3, 0);
        self.ctx.OMSetRenderTargets(None, None);
        self.ctx.Flush();
        self.ctx.CopyResource(&self.staging, &self.tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE {
            pData: std::ptr::null_mut(),
            RowPitch: 0,
            DepthPitch: 0,
        };
        self.ctx
            .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("Map nimbus: {e}"))?;
        let copied = self
            .bitmap
            .CopyFromMemory(None, mapped.pData, mapped.RowPitch);
        self.ctx.Unmap(&self.staging, 0);
        copied.map_err(|e| format!("CopyFromMemory: {e}"))?;
        Ok(())
    }
}

unsafe fn compile_shader(
    src: &str,
    entry: windows::core::PCSTR,
    target: windows::core::PCSTR,
) -> Result<Vec<u8>, String> {
    let mut code: Option<ID3DBlob> = None;
    let mut errs: Option<ID3DBlob> = None;
    if let Err(e) = D3DCompile(
        src.as_ptr() as *const _,
        src.len(),
        None,
        None,
        None::<&ID3DInclude>,
        entry,
        target,
        D3DCOMPILE_OPTIMIZATION_LEVEL3,
        0,
        &mut code,
        Some(&mut errs),
    ) {
        let msg = errs.as_ref().map(blob_text).unwrap_or_default();
        return Err(format!("D3DCompile: {e}: {msg}"));
    }
    let code = code.ok_or("D3DCompile returned no blob")?;
    let ptr = code.GetBufferPointer() as *const u8;
    let len = code.GetBufferSize();
    if ptr.is_null() || len == 0 {
        return Err("D3DCompile blob was empty".into());
    }
    Ok(std::slice::from_raw_parts(ptr, len).to_vec())
}

/// Lit wisps stay the accent. Mixing toward white is what was blowing it out.
fn theme_light(accent: u32) -> [f32; 4] {
    [chan(accent, 0), chan(accent, 8), chan(accent, 16), 1.0]
}

/// Shade: the same accent, only a little deeper. A heavy black mix turned the
/// cloud into one dark solid.
fn theme_shadow(accent: u32) -> [f32; 4] {
    let c = mix_byte(accent, 0, 0.38);
    [chan(c, 0), chan(c, 8), chan(c, 16), 1.0]
}

fn chan(c: u32, shift: u32) -> f32 {
    ((c >> shift) & 0xff) as f32 / 255.0
}

fn mix_byte(fg: u32, bg: u32, toward_bg: f32) -> u32 {
    let t = toward_bg.clamp(0.0, 1.0);
    let blend = |shift: u32| {
        let f = ((fg >> shift) & 0xff) as f32;
        let b = ((bg >> shift) & 0xff) as f32;
        (f + (b - f) * t).round() as u32
    };
    blend(0) | (blend(8) << 8) | (blend(16) << 16)
}

fn blob_text(blob: &ID3DBlob) -> String {
    unsafe {
        let ptr = blob.GetBufferPointer();
        let len = blob.GetBufferSize();
        if ptr.is_null() || len == 0 {
            return String::new();
        }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, len);
        String::from_utf8_lossy(bytes).trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_buffer_matches_hlsl_packing() {
        assert_eq!(std::mem::size_of::<NimbusCb>(), 128);
        assert_eq!(std::mem::size_of::<NimbusCb>() % 16, 0);
    }

    #[test]
    fn thinking_volumes_stay_inside_the_shader_range() {
        for i in 0..40 {
            let (input, output) = thinking_volumes(i as f32 * 0.37);
            assert!((0.0..=1.0).contains(&input), "{input}");
            assert!((0.0..=1.0).contains(&output), "{output}");
        }
        let (input, _) = thinking_volumes(0.0);
        assert!(input > 0.2, "thinking should not start dark, got {input}");
    }

    #[test]
    fn nimbus_shader_compiles() {
        unsafe {
            compile_shader(SHADER, s!("vs_main"), s!("vs_4_0")).expect("vs");
            compile_shader(SHADER, s!("ps_main"), s!("ps_4_0")).expect("ps");
        }
    }

    #[test]
    fn warp_frame_is_a_soft_cloud() {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct2D::{
            D2D1CreateFactory, ID2D1Factory1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
            D2D1_FACTORY_TYPE_SINGLE_THREADED,
        };
        use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_USAGE_STAGING,
        };
        use windows::Win32::Graphics::Dxgi::IDXGIDevice;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut device = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .expect("warp device");
            let device = device.expect("warp device null");
            let dxgi: IDXGIDevice = device.cast().expect("dxgi");
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).expect("d2d factory");
            let d2d_device = factory.CreateDevice(&dxgi).expect("d2d device");
            let d2d = d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .expect("d2d context");
            let mut orb = NimbusOrb::create(&device, &d2d).expect("nimbus");
            orb.render(Instant::now(), NimbusMood::Thinking, 0x00e5881e)
                .expect("render");

            let desc = super::D3D11_TEXTURE2D_DESC {
                Width: RT,
                Height: RT,
                MipLevels: 1,
                ArraySize: 1,
                Format: super::DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: super::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..Default::default()
            };
            let mut staging = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .expect("staging");
            let staging = staging.expect("staging null");
            orb.ctx.CopyResource(&staging, &orb.tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE {
                pData: std::ptr::null_mut(),
                RowPitch: 0,
                DepthPitch: 0,
            };
            orb.ctx
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .expect("map");
            let sample = |x: u32, y: u32| -> u8 {
                let row = mapped.pData.add(y as usize * mapped.RowPitch as usize) as *const u8;
                *row.add(x as usize * 4 + 3)
            };
            let mut lit = 0u32;
            for y in 0..RT {
                for x in 0..RT {
                    if sample(x, y) > 16 {
                        lit += 1;
                    }
                }
            }
            let corner = sample(0, 0);
            let centre = sample(RT / 2, RT / 2);
            orb.ctx.Unmap(&staging, 0);
            let frac = lit as f32 / (RT * RT) as f32;
            assert!(corner < 16, "corner should be empty, alpha {corner}");
            assert!(centre > 20, "centre should be the cloud, alpha {centre}");
            assert!(
                frac > 0.02 && frac < 0.9,
                "cloud coverage {frac} (lit {lit})"
            );
        }
    }
}

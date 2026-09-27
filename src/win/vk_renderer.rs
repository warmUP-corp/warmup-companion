//! D3D11 + DXGI composition swapchain + D2D + DirectComposition renderer.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::time::Instant;

use windows::core::{w, Interface};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, HWND, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FILL_MODE_ALTERNATE, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Factory, ID2D1Factory1,
    ID2D1Geometry, ID2D1Layer, ID2D1RenderTarget, ID2D1Resource, ID2D1SolidColorBrush,
    D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_ROUNDED_RECT, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_TEXT_ALIGNMENT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::nimbus_orb::{drive_scale, mood_drive, NimbusMood, NimbusOrb, ORB_FRAME_FILL};
use super::controller_tips::TipToken;
use super::shortcut_sheet::{self, Line};
use crate::config::VkStyle;
use crate::vk_nav::{KeyAction, KeyCell, KeyPos, KeyRow};

/// GDI `COLORREF` (`0x00BBGGRR`) -> D2D color.
fn colorref(c: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (c & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: ((c >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn colorref_alpha(c: u32, alpha: f32) -> D2D1_COLOR_F {
    let mut col = colorref(c);
    col.a = alpha;
    col
}

fn colorref_hex(c: u32) -> String {
    let r = c & 0xff;
    let g = (c >> 8) & 0xff;
    let b = (c >> 16) & 0xff;
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn colorref_mix(fg: u32, bg: u32, amount: f32) -> u32 {
    let amount = amount.clamp(0.0, 1.0);
    let blend = |shift: u32| {
        let f = ((fg >> shift) & 0xff_u32) as f32;
        let b = ((bg >> shift) & 0xff_u32) as f32;
        (b + (f - b) * amount).round() as u32
    };
    blend(0) | (blend(8) << 8) | (blend(16) << 16)
}

pub fn mix_color(fg: u32, bg: u32, amount: f32) -> u32 {
    colorref_mix(fg, bg, amount)
}

/// Rotate the hue of a COLORREF (0x00BBGGRR) by `deg`, keeping saturation and
/// lightness. Lets the voice orb fan a single theme accent into a few related
/// tints so it flows like the old multicolor blob but stays on-theme.
fn shift_hue(c: u32, deg: f32) -> u32 {
    let r = (c & 0xff) as f32 / 255.0;
    let g = ((c >> 8) & 0xff) as f32 / 255.0;
    let b = ((c >> 16) & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    let (mut h, s) = if d < 1e-6 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s)
    };
    h = (h + deg).rem_euclid(360.0);
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = chroma * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - chroma * 0.5;
    let (r1, g1, b1) = match (h / 60.0) as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let to = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u32;
    to(r1) | (to(g1) << 8) | (to(b1) << 16)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_rect(a: D2D_RECT_F, b: D2D_RECT_F, t: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: lerp(a.left, b.left, t),
        top: lerp(a.top, b.top, t),
        right: lerp(a.right, b.right, t),
        bottom: lerp(a.bottom, b.bottom, t),
    }
}

fn configure_d2d_quality(ctx: &ID2D1DeviceContext) {
    // Default D2D text path can look aliased on our DXGI composition target.
    unsafe { ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE) };
    unsafe { ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
}

/// Identity transform for the D2D device context.
const IDENTITY: Matrix3x2 = Matrix3x2 {
    M11: 1.0,
    M12: 0.0,
    M21: 0.0,
    M22: 1.0,
    M31: 0.0,
    M32: 0.0,
};

/// Uniform scale `s` about `(cx, cy)`, for press feedback on a single key.
fn scale_about(s: f32, cx: f32, cy: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: cx * (1.0 - s),
        M32: cy * (1.0 - s),
    }
}

fn translate(dx: f32, dy: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: dx,
        M32: dy,
    }
}

const BLUR_SHADOW_STEPS: usize = 6;

fn blur_shadow_layers(blur: f32, alpha: f32) -> [(f32, f32); BLUR_SHADOW_STEPS] {
    let n = BLUR_SHADOW_STEPS as f32;
    let per = 1.0 - (1.0 - alpha.clamp(0.0, 1.0)).powf(1.0 / n);
    let mut out = [(0.0, 0.0); BLUR_SHADOW_STEPS];
    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f32 / (n - 1.0);
        *slot = (blur * (-0.25 + 0.75 * t), per);
    }
    out
}

unsafe fn draw_blur_shadow(
    ctx: &ID2D1DeviceContext,
    rect: D2D_RECT_F,
    radius: f32,
    dy: f32,
    blur: f32,
    alpha: f32,
) -> Result<(), String> {
    for (spread, a) in blur_shadow_layers(blur, alpha) {
        let brush = solid_brush(ctx, colorref_alpha(0x000000, a))?;
        let r = (radius + spread).max(0.0);
        ctx.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: rect.left - spread,
                    top: rect.top - spread + dy,
                    right: rect.right + spread,
                    bottom: rect.bottom + spread + dy,
                },
                radiusX: r,
                radiusY: r,
            },
            &brush,
        );
    }
    Ok(())
}

fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: radius,
        radiusY: radius,
    }
}

fn deflate(rect: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left + d,
        top: rect.top + d,
        right: rect.right - d,
        bottom: rect.bottom - d,
    }
}

fn band_at(rect: D2D_RECT_F, cy: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left,
        top: cy - h * 0.5,
        right: rect.right,
        bottom: cy + h * 0.5,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VkPalette {
    pub bg: u32,
    pub key: u32,
    pub key_action: u32,
    pub accent: u32,
    pub sel_ring: u32,
    pub text: u32,
    pub text_dim: u32,
    pub sel_text: u32,
    pub border: u32,
    pub panel_stroke: u32,
    pub panel_stroke_alpha: f32,
    pub chip_sel: u32,
}

const fn rgb(v: u32) -> u32 {
    let r = (v >> 16) & 0xff;
    let g = (v >> 8) & 0xff;
    let b = v & 0xff;
    (b << 16) | (g << 8) | r
}

pub fn style_palette(style: VkStyle, dark: bool) -> VkPalette {
    match (style, dark) {
        (VkStyle::Normal, true) => VkPalette {
            bg: rgb(0x15161C),
            key: rgb(0x2A2B36),
            key_action: rgb(0x1F2029),
            accent: rgb(0xA6D1FF),
            sel_ring: rgb(0xDCEBFF),
            text: rgb(0xFFFFFF),
            text_dim: rgb(0xFFFFFF),
            sel_text: rgb(0x1A2233),
            border: rgb(0x3A3B47),
            panel_stroke: rgb(0xFFFFFF),
            panel_stroke_alpha: 0x12 as f32 / 255.0,
            chip_sel: rgb(0x3A3C4A),
        },
        (VkStyle::Normal, false) => VkPalette {
            bg: rgb(0xF3F4F7),
            key: rgb(0xFFFFFF),
            key_action: rgb(0xE3E5EB),
            accent: rgb(0x0E80C7),
            sel_ring: rgb(0x7CC0EA),
            text: rgb(0x111217),
            text_dim: rgb(0x111217),
            sel_text: rgb(0xFFFFFF),
            border: rgb(0xD3D6DE),
            panel_stroke: rgb(0x000000),
            panel_stroke_alpha: 0x12 as f32 / 255.0,
            chip_sel: rgb(0xDADDE5),
        },
        (VkStyle::Mono, true) => VkPalette {
            bg: rgb(0x1C1C1E),
            key: rgb(0x3A3A3C),
            key_action: rgb(0x2C2C2E),
            accent: rgb(0xC9BCFF),
            sel_ring: rgb(0xB6A0FF),
            text: rgb(0xFFFFFF),
            text_dim: rgb(0xEBEBF5),
            sel_text: rgb(0x211047),
            border: rgb(0x545458),
            panel_stroke: rgb(0xFFFFFF),
            panel_stroke_alpha: 0x14 as f32 / 255.0,
            chip_sel: rgb(0x3A3A3C),
        },
        (VkStyle::Mono, false) => VkPalette {
            bg: rgb(0xD1D3D9),
            key: rgb(0xFFFFFF),
            key_action: rgb(0xABB0BA),
            accent: rgb(0x7E6AD8),
            sel_ring: rgb(0x7E6AD8),
            text: rgb(0x000000),
            text_dim: rgb(0x3C3C43),
            sel_text: rgb(0xFFFFFF),
            border: rgb(0x3C3C43),
            panel_stroke: rgb(0x000000),
            panel_stroke_alpha: 0x14 as f32 / 255.0,
            chip_sel: rgb(0xFFFFFF),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StripLook {
    Pills,
    Columns,
}

pub struct StyleSpec {
    design_kh: f32,
    key_aspect: f32,
    gap: f32,
    key_radius: f32,
    pad_x: f32,
    pad_y: f32,
    panel_radius: f32,
    families: &'static [&'static str],
    label_px: f32,
    label_weight: DWRITE_FONT_WEIGHT,
    word_px: f32,
    word_weight: DWRITE_FONT_WEIGHT,
    number_px: f32,
    number_alpha: f32,
    number_cy: f32,
    label_cy: f32,
    caption_cy: f32,
    space_icon_cy: f32,
    space_label: Option<&'static str>,
    space_label_px: f32,
    trim_symbols_label: bool,
    icon_px: f32,
    icon_large_px: f32,
    space_icon_px: f32,
    hint_badge: f32,
    hint_inset: f32,
    key_shadow_dy: f32,
    key_shadow_alpha: f32,
    sel_shadow: Option<(f32, f32, f32)>,
    sel_ring_w: f32,
    strip: StripLook,
    chip_slots: usize,
    strip_bar_h: f32,
    chip_px: f32,
    chip_sel_weight: DWRITE_FONT_WEIGHT,
    chip_text_alpha: f32,
    strip_button_w: f32,
    strip_button_h: f32,
    strip_button_gap: f32,
    strip_hint_px: f32,
    chips_h: f32,
    chips_pad: f32,
    chips_gap: f32,
    separator_h: f32,
    separator_alpha: f32,
}

static NORMAL_SPEC: StyleSpec = StyleSpec {
    design_kh: 102.0,
    key_aspect: 102.0 / 138.0,
    gap: 6.0,
    key_radius: 12.0,
    pad_x: 17.0,
    pad_y: 18.0,
    panel_radius: 25.0,
    families: &["Noto Sans", "Segoe UI"],
    label_px: 38.0,
    label_weight: DWRITE_FONT_WEIGHT_MEDIUM,
    word_px: 26.0,
    word_weight: DWRITE_FONT_WEIGHT_MEDIUM,
    number_px: 14.0,
    number_alpha: 0x8C as f32 / 255.0,
    number_cy: 25.0,
    label_cy: 60.5,
    caption_cy: 23.0,
    space_icon_cy: 62.5,
    space_label: None,
    space_label_px: 22.0,
    trim_symbols_label: false,
    icon_px: 36.0,
    icon_large_px: 40.0,
    space_icon_px: 52.0,
    hint_badge: 36.0,
    hint_inset: 8.0,
    key_shadow_dy: 1.0,
    key_shadow_alpha: 0x99 as f32 / 255.0,
    sel_shadow: None,
    sel_ring_w: 2.0,
    strip: StripLook::Pills,
    chip_slots: 7,
    strip_bar_h: 112.0,
    chip_px: 20.0,
    chip_sel_weight: DWRITE_FONT_WEIGHT_SEMI_BOLD,
    chip_text_alpha: 0xB3 as f32 / 255.0,
    strip_button_w: 60.0,
    strip_button_h: 48.0,
    strip_button_gap: 18.0,
    strip_hint_px: 36.0,
    chips_h: 60.0,
    chips_pad: 6.0,
    chips_gap: 4.0,
    separator_h: 0.0,
    separator_alpha: 0.0,
};

const MONO_FAMILIES: &[&str] = &["Inter", "SF Pro Text", "Segoe UI Variable Text", "Segoe UI"];

static MONO_SPEC: StyleSpec = StyleSpec {
    design_kh: 100.0,
    key_aspect: 100.0 / 137.0,
    gap: 8.0,
    key_radius: 12.0,
    pad_x: 13.0,
    pad_y: 13.0,
    panel_radius: 24.0,
    families: MONO_FAMILIES,
    label_px: 32.0,
    label_weight: DWRITE_FONT_WEIGHT_NORMAL,
    word_px: 24.0,
    word_weight: DWRITE_FONT_WEIGHT_NORMAL,
    number_px: 14.0,
    number_alpha: 0x99 as f32 / 255.0,
    number_cy: 30.5,
    label_cy: 58.5,
    caption_cy: 23.0,
    space_icon_cy: 50.0,
    space_label: Some("space"),
    space_label_px: 22.0,
    trim_symbols_label: true,
    icon_px: 34.0,
    icon_large_px: 34.0,
    space_icon_px: 34.0,
    hint_badge: 36.0,
    hint_inset: 8.0,
    key_shadow_dy: 1.0,
    key_shadow_alpha: 0x99 as f32 / 255.0,
    sel_shadow: Some((8.0, 24.0, 0x80 as f32 / 255.0)),
    sel_ring_w: 2.0,
    strip: StripLook::Columns,
    chip_slots: 7,
    strip_bar_h: 72.0,
    chip_px: 22.0,
    chip_sel_weight: DWRITE_FONT_WEIGHT_NORMAL,
    chip_text_alpha: 0x99 as f32 / 255.0,
    strip_button_w: 40.0,
    strip_button_h: 0.0,
    strip_button_gap: 0.0,
    strip_hint_px: 32.0,
    chips_h: 72.0,
    chips_pad: 0.0,
    chips_gap: 0.0,
    separator_h: 28.0,
    separator_alpha: 0x99 as f32 / 255.0,
};

pub fn style_spec(style: VkStyle) -> &'static StyleSpec {
    match style {
        VkStyle::Normal => &NORMAL_SPEC,
        VkStyle::Mono => &MONO_SPEC,
    }
}

struct PromptSpec {
    families: &'static [&'static str],
    no_pad_label: &'static str,
    status_label: &'static str,
    radius: f32,
    pill_h: f32,
    pill_bottom: f32,
    pill_pad_x: f32,
    pill_gap: f32,
    pill_fill: u32,
    pill_fill_alpha: f32,
    no_pad_stroke_alpha: f32,
    ready_stroke_alpha: f32,
    pill_shadow: (f32, f32, f32),
    card_w: f32,
    card_h: f32,
    card_bottom: f32,
    card_pad_y: f32,
    card_gap: f32,
    card_fill: u32,
    card_fill_alpha: f32,
    card_stroke_alpha: f32,
    card_shadow: (f32, f32, f32),
    label_px: f32,
    title_px: f32,
    title_h: f32,
    status_px: f32,
    status_h: f32,
    status_gap: f32,
    dim_text: u32,
    dim_alpha: f32,
    gamepad_px: f32,
    chip_px: f32,
    check_px: f32,
    stage_w: f32,
    stage_h: f32,
    stage_glow_alpha: f32,
    art_w: f32,
    art_h: f32,
}

static MONO_PROMPT: PromptSpec = PromptSpec {
    families: MONO_FAMILIES,
    no_pad_label: "Connect a controller",
    status_label: "Connected",
    radius: 44.0,
    pill_h: 88.0,
    pill_bottom: 26.0,
    pill_pad_x: 40.0,
    pill_gap: 12.0,
    pill_fill: rgb(0x2C2C2E),
    pill_fill_alpha: 0xB3 as f32 / 255.0,
    no_pad_stroke_alpha: 0x14 as f32 / 255.0,
    ready_stroke_alpha: 0x33 as f32 / 255.0,
    pill_shadow: (10.0, 30.0, 0x66 as f32 / 255.0),
    card_w: 460.0,
    card_h: 331.0,
    card_bottom: 50.0,
    card_pad_y: 24.0,
    card_gap: 8.0,
    card_fill: rgb(0x1C1C1E),
    card_fill_alpha: 0xCC as f32 / 255.0,
    card_stroke_alpha: 0x1A as f32 / 255.0,
    card_shadow: (20.0, 60.0, 0x80 as f32 / 255.0),
    label_px: 26.0,
    title_px: 26.0,
    title_h: 36.0,
    status_px: 20.0,
    status_h: 30.0,
    status_gap: 8.0,
    dim_text: rgb(0xEBEBF5),
    dim_alpha: 0x99 as f32 / 255.0,
    gamepad_px: 30.0,
    chip_px: 48.0,
    check_px: 22.0,
    stage_w: 320.0,
    stage_h: 200.0,
    stage_glow_alpha: 0x1A as f32 / 255.0,
    art_w: 250.0,
    art_h: 190.0,
};

fn prompt_spec(style: VkStyle) -> Option<&'static PromptSpec> {
    match style {
        VkStyle::Normal => None,
        VkStyle::Mono => Some(&MONO_PROMPT),
    }
}

#[derive(Clone)]
struct PromptFonts {
    label: IDWriteTextFormat,
    title: IDWriteTextFormat,
    status: IDWriteTextFormat,
}

pub fn strip_slots(style: VkStyle) -> usize {
    style_spec(style).chip_slots
}

fn ref_key_h(scale: f32, style: VkStyle) -> f32 {
    REF_KEY_W * scale.max(0.05) * style_spec(style).key_aspect
}

fn ref_unit(scale: f32, style: VkStyle) -> f32 {
    ref_key_h(scale, style) / style_spec(style).design_kh
}

pub fn strip_band_height(scale: f32, style: VkStyle) -> f32 {
    let spec = style_spec(style);
    (spec.strip_bar_h + spec.gap - spec.pad_y) * ref_unit(scale, style)
}

pub fn floating_pad(scale: f32, style: VkStyle) -> (f32, f32) {
    let spec = style_spec(style);
    let u = ref_unit(scale, style);
    (
        spec.pad_x * u + FLOATING_PANEL_INSET,
        spec.pad_y * u + FLOATING_PANEL_INSET,
    )
}

fn is_action_key(action: &KeyAction) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN};
    matches!(action, KeyAction::Vk(vk) if *vk == VK_BACK || *vk == VK_RETURN)
}

fn styled_glyph(spec: &StyleSpec, action: &KeyAction, glyph: String) -> String {
    match action {
        KeyAction::Symbols if spec.trim_symbols_label => {
            glyph.trim_start_matches('?').to_string()
        }
        _ => glyph,
    }
}

#[derive(Clone)]
struct StyleFonts {
    style: VkStyle,
    size_key: i32,
    label: IDWriteTextFormat,
    word: IDWriteTextFormat,
    number: IDWriteTextFormat,
    space: IDWriteTextFormat,
    chip: IDWriteTextFormat,
    chip_sel: IDWriteTextFormat,
}


pub struct VkRenderer {
    width: u32,
    height: u32,
    swapchain: IDXGISwapChain1,
    d2d_context: ID2D1DeviceContext,
    /// Owns the swapchain buffer. Must be dropped before `ResizeBuffers`, or the
    /// resize fails and every later draw is left with no target (the last pill
    /// frame stays on screen).
    d2d_target: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    text_format: IDWriteTextFormat,
    glyph_format: IDWriteTextFormat,
    /// Small font for sublabels, badges, and the legend strip.
    hint_format: IDWriteTextFormat,
    style_fonts: Option<StyleFonts>,
    /// Fixed large font for the connect/keyboard prompt pills (10-foot UI).
    prompt_format: IDWriteTextFormat,
    prompt_fonts: Option<PromptFonts>,
    icon_cache: HashMap<IconCacheKey, ID2D1Bitmap1>,
    controller_art_cache: HashMap<ControllerArtCacheKey, (ID2D1Bitmap1, u32, u32)>,
    prompt_started: Instant,
    /// Gliding focus-ring rect (client px) + the previous draw time, so the ring
    /// eases toward the selected key frame-rate-independently. `None` until the
    /// first frame, where it snaps to the selection.
    anim_sel: Option<D2D_RECT_F>,
    last_draw: Option<Instant>,
    /// When the suggestion strip last went from hidden to shown, driving its
    /// short fade/rise entrance. `None` while hidden (exit is instant: the strip
    /// comes and goes once per word, so motion there adds nothing).
    strip_shown_at: Option<Instant>,
    /// Mic-level envelope state (`vk_motion::smooth_level`), shared by the mic
    /// key orb and the dictation pill so both breathe the same way.
    level: f32,
    level_at: Option<Instant>,
    /// shdr-21 cloud, created on the first transcription frame. `nimbus_failed`
    /// sticks so a missing compiler doesn't retry every frame; the ellipse orb
    /// stays as the fallback.
    nimbus: Option<NimbusOrb>,
    nimbus_failed: bool,
    sheet_alpha: f32,
    sheet_tick: Option<Instant>,
    text_formats: std::cell::RefCell<HashMap<(VkStyle, i32), IDWriteTextFormat>>,
    d3d: ID3D11Device,
    _d2d_device: ID2D1Device,
    _dcomp_device: IDCompositionDevice,
    // Keep the composition target + visual alive for the window's lifetime. Dropping
    // them releases the HWND<->visual binding, so the window shows nothing.
    _comp_target: Option<IDCompositionTarget>,
    _visual: Option<IDCompositionVisual>,
}

pub const REF_MON_W: f32 = 1920.0;
const REF_KEY_W: f32 = 92.0;
/// Time constant for the focus ring gliding to the selected key. Small =
/// snappy (~90 ms settle); the fill stays instant so labels never tear.
const SEL_GLIDE_TAU: f32 = 0.045;

/// Hairline the floating panel is inset from the window so its antialiased
/// stroke is never clipped.
const FLOATING_PANEL_INSET: f32 = 1.0;

pub const STRIP_BAND_H: f32 = 67.0;

const BAR_H: f32 = 80.0;
const BAR_GLYPH: f32 = 38.0;
const BAR_SHARE: f32 = 44.0;
const BAR_GAP: f32 = 6.0;
const BAR_LABEL_GAP: f32 = 6.0;
const BAR_LABEL_PX: f32 = 19.0;
const BAR_CONNECTOR_PX: f32 = 17.0;
const BAR_PREFIX_PX: f32 = 20.0;
const BAR_SEP_PX: f32 = 18.0;
const SHEET_PAD_TOP: f32 = 16.0;
const SHEET_PAD_X: f32 = 24.0;
const SHEET_PAD_BOTTOM: f32 = 16.0;
const SHEET_COL_GAP: f32 = 40.0;
const SHEET_ROW_H: f32 = 70.0;
const SHEET_HEADER_PX: f32 = 18.0;
const SHEET_ACTION_PX: f32 = 19.0;
const SHEET_TOKEN_PX: f32 = 17.0;
const SHEET_GLYPH: f32 = 28.0;
const SHEET_SHARE: f32 = 32.0;
const SHEET_TOKEN_GAP: f32 = 8.0;
const SHEET_ACTION_GAP: f32 = 24.0;
const SHEET_TRAIL: f32 = 24.0;
const LEGEND_MIN_TEXT_PX: f32 = 11.0;
const LEGEND_SEP: &str = "\u{00B7}";
const ALPHA_SECONDARY: f32 = 0xA6 as f32 / 255.0;
const ALPHA_SEP: f32 = 0x66 as f32 / 255.0;
const ALPHA_LINE: f32 = 0x14 as f32 / 255.0;

fn text_px(design: f32, u: f32) -> f32 {
    (design * u).max(LEGEND_MIN_TEXT_PX)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ComboStyle {
    glyph: f32,
    share: f32,
    gap: f32,
    prefix_px: f32,
    connector_px: f32,
}

impl ComboStyle {
    fn bar(u: f32) -> Self {
        Self {
            glyph: BAR_GLYPH * u,
            share: BAR_SHARE * u,
            gap: BAR_GAP * u,
            prefix_px: text_px(BAR_PREFIX_PX, u),
            connector_px: text_px(BAR_CONNECTOR_PX, u),
        }
    }

    fn sheet(u: f32) -> Self {
        Self {
            glyph: SHEET_GLYPH * u,
            share: SHEET_SHARE * u,
            gap: SHEET_TOKEN_GAP * u,
            prefix_px: text_px(SHEET_TOKEN_PX, u),
            connector_px: text_px(SHEET_TOKEN_PX, u),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ComboPart {
    Icon(VkIcon, bool),
    Prefix(&'static str),
    Connector(&'static str),
}

fn combo_parts(tokens: &[TipToken], family: ControllerIconFamily) -> Vec<ComboPart> {
    tokens
        .iter()
        .filter_map(|&t| match t {
            TipToken::Button(b) => family.hint_icon(b).map(|i| ComboPart::Icon(i, b == "SELECT")),
            TipToken::Text(t) if t == "/" => Some(ComboPart::Connector(t)),
            TipToken::Text(t) => Some(ComboPart::Prefix(t)),
            TipToken::Plus => Some(ComboPart::Connector("+")),
            TipToken::Dot => Some(ComboPart::Connector(LEGEND_SEP)),
        })
        .collect()
}

type Measure<'a> = &'a dyn Fn(&str, f32) -> f32;

fn part_width(part: &ComboPart, cs: &ComboStyle, measure: Measure) -> f32 {
    match *part {
        ComboPart::Icon(_, true) => cs.share,
        ComboPart::Icon(_, false) => cs.glyph,
        ComboPart::Prefix(t) => measure(t, cs.prefix_px),
        ComboPart::Connector(t) => measure(t, cs.connector_px),
    }
}

fn combo_width(parts: &[ComboPart], cs: &ComboStyle, measure: Measure) -> f32 {
    parts.iter().map(|p| part_width(p, cs, measure)).sum::<f32>()
        + cs.gap * parts.len().saturating_sub(1) as f32
}

pub fn legend_band_height(scale: f32, style: VkStyle) -> f32 {
    ((BAR_H - style_spec(style).pad_y) * ref_unit(scale, style)).max(0.0)
}

fn tip_item(token: TipToken, family: ControllerIconFamily) -> Option<TipItem> {
    match token {
        TipToken::Button(b) => family.hint_icon(b).map(TipItem::Icon),
        TipToken::Text(t) => Some(TipItem::Text(t, false)),
        TipToken::Plus => Some(TipItem::Text("+", true)),
        TipToken::Dot => Some(TipItem::Text(LEGEND_SEP, true)),
    }
}

fn bar_item_width(
    row: &shortcut_sheet::SheetRow,
    family: ControllerIconFamily,
    u: f32,
    measure: Measure,
) -> f32 {
    combo_width(&combo_parts(row.keys, family), &ComboStyle::bar(u), measure)
        + BAR_LABEL_GAP * u
        + measure(row.label, text_px(BAR_LABEL_PX, u))
}

fn bar_sep_width(u: f32, measure: Measure) -> f32 {
    2.0 * BAR_GAP * u + measure(LEGEND_SEP, text_px(BAR_SEP_PX, u))
}

#[derive(Clone, Debug, PartialEq)]
struct BarLayout {
    items: Vec<(usize, f32, f32)>,
    sep_w: f32,
}

fn bar_layout(widths: &[f32], sep_w: f32, left: f32, right: f32) -> BarLayout {
    let kept = shortcut_sheet::fit_legend(widths, sep_w, right - left);
    let total = kept.iter().map(|&i| widths[i]).sum::<f32>()
        + sep_w * kept.len().saturating_sub(1) as f32;
    let mut x = (left + right - total) * 0.5;
    let items = kept
        .into_iter()
        .map(|i| {
            let at = x;
            x += widths[i] + sep_w;
            (i, at, widths[i])
        })
        .collect();
    BarLayout { items, sep_w }
}

fn legend_span(rects: &[KeyRect], client_w: f32, pad_x: f32) -> (f32, f32) {
    let (l, r) = rects
        .iter()
        .fold((pad_x, client_w - pad_x), |(l, r), kr| (l.min(kr.left), r.max(kr.right)));
    if l > r {
        (0.0, client_w)
    } else {
        (l.max(0.0), r.min(client_w))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SheetMetrics {
    u: f32,
    cs: ComboStyle,
    row_h: f32,
    header_px: f32,
    action_px: f32,
    action_gap: f32,
    trail: f32,
    col_gap: f32,
}

impl SheetMetrics {
    fn at(u: f32) -> Self {
        Self {
            u,
            cs: ComboStyle::sheet(u),
            row_h: SHEET_ROW_H * u,
            header_px: text_px(SHEET_HEADER_PX, u),
            action_px: text_px(SHEET_ACTION_PX, u),
            action_gap: SHEET_ACTION_GAP * u,
            trail: SHEET_TRAIL * u,
            col_gap: SHEET_COL_GAP * u,
        }
    }
}

struct SheetColumn {
    lines: Vec<Line>,
    combo_w: f32,
    width: f32,
}

fn sheet_columns(family: ControllerIconFamily, m: &SheetMetrics, measure: Measure) -> Vec<SheetColumn> {
    shortcut_sheet::columns()
        .into_iter()
        .map(|lines| {
            let mut combo_w = 0.0f32;
            let mut label_w = 0.0f32;
            let mut header_w = 0.0f32;
            for line in &lines {
                match line {
                    Line::Row(row) => {
                        combo_w = combo_w.max(combo_width(&combo_parts(row.keys, family), &m.cs, measure));
                        label_w = label_w.max(measure(row.label, m.action_px));
                    }
                    Line::Title(t) => header_w = header_w.max(measure(t, m.header_px)),
                    Line::Blank => {}
                }
            }
            let width = (combo_w + m.action_gap + label_w + m.trail).max(header_w);
            SheetColumn {
                lines,
                combo_w,
                width,
            }
        })
        .collect()
}

fn sheet_block_size(total: f32, m: &SheetMetrics) -> (f32, f32) {
    let lines = shortcut_sheet::max_lines() as f32;
    (
        total + 2.0 * SHEET_PAD_X * m.u,
        (SHEET_PAD_TOP + SHEET_PAD_BOTTOM) * m.u + lines * m.row_h,
    )
}

fn sheet_total_width(cols: &[SheetColumn], m: &SheetMetrics) -> f32 {
    let n = cols.len();
    let divider = if n >= 2 { m.col_gap + 1.0 } else { 0.0 };
    cols.iter().map(|c| c.width).sum::<f32>() + m.col_gap * n.saturating_sub(1) as f32 + divider
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum VkIcon {
    Backspace,
    Enter,
    Mic,
    MicOff,
    Space,
    Shift,
    ShiftFilled,
    Caps,
    CapsFilled,
    /// Caret-move arrow keys (Lucide chevrons).
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    /// Generic controller image for the connection card.
    Gamepad,
    Check,
    /// Left-stick click chips keep their native colors (no `currentColor`),
    /// extracted from the controller-icon atlas.
    L3Ps5,
    L3Xbox,
    R3Ps5,
    R3Xbox,
    /// Select/Start chips (PS5 Share/Options, Xbox View/Menu).
    SelectPs5,
    SelectXbox,
    StartPs5,
    StartXbox,
    Ps5Cross,
    Ps5Circle,
    Ps5Square,
    Ps5Triangle,
    Ps5L1,
    Ps5R1,
    Ps5L2,
    Ps5R2,
    XboxA,
    XboxB,
    XboxX,
    XboxY,
    XboxLb,
    XboxRb,
    XboxLt,
    XboxRt,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct IconCacheKey {
    icon: VkIcon,
    px: u32,
    color: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ControllerArt {
    DualSense,
    XboxOne,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ControllerArtCacheKey {
    art: ControllerArt,
}

impl ControllerArt {
    fn from_label(label: &str) -> Option<Self> {
        let l = label.to_ascii_lowercase();
        if l.contains("dualsense")
            || l.contains("dualshock")
            || l.contains("playstation")
            || l.contains("ps5")
            || l.contains("ps4")
            // Winlogon reads PlayStation pads via the direct-HID path ("HID slot N").
            || l.contains("hid slot")
        {
            Some(Self::DualSense)
        } else if l.contains("xbox") || l.contains("xinput") {
            Some(Self::XboxOne)
        } else {
            None
        }
    }

    fn png(self) -> &'static [u8] {
        match self {
            Self::DualSense => {
                include_bytes!("../../assets/controller-models/dualsense-controller.png")
            }
            Self::XboxOne => {
                include_bytes!("../../assets/controller-models/xbox-one-controller.png")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ControllerIconFamily {
    Ps5,
    Xbox,
}

impl ControllerIconFamily {
    fn from_label(label: &str) -> Self {
        let l = label.to_ascii_lowercase();
        if l.contains("dualsense")
            || l.contains("dualshock")
            || l.contains("playstation")
            || l.contains("ps5")
            || l.contains("ps4")
            || l.contains("hid slot")
        {
            Self::Ps5
        } else {
            Self::Xbox
        }
    }

    fn l3_icon(self) -> VkIcon {
        match self {
            Self::Ps5 => VkIcon::L3Ps5,
            Self::Xbox => VkIcon::L3Xbox,
        }
    }

    fn hint_icon(self, hint: &str) -> Option<VkIcon> {
        match (self, hint) {
            (Self::Ps5, "A") => Some(VkIcon::Ps5Cross),
            (Self::Ps5, "B") => Some(VkIcon::Ps5Circle),
            (Self::Ps5, "X") => Some(VkIcon::Ps5Square),
            (Self::Ps5, "Y") => Some(VkIcon::Ps5Triangle),
            (Self::Ps5, "LB") => Some(VkIcon::Ps5L1),
            (Self::Ps5, "RB") => Some(VkIcon::Ps5R1),
            (Self::Ps5, "LT") => Some(VkIcon::Ps5L2),
            (Self::Ps5, "RT") => Some(VkIcon::Ps5R2),
            (Self::Ps5, "L3") => Some(VkIcon::L3Ps5),
            (Self::Ps5, "R3") => Some(VkIcon::R3Ps5),
            (Self::Ps5, "SELECT") => Some(VkIcon::SelectPs5),
            (Self::Ps5, "START") => Some(VkIcon::StartPs5),
            (Self::Xbox, "A") => Some(VkIcon::XboxA),
            (Self::Xbox, "B") => Some(VkIcon::XboxB),
            (Self::Xbox, "X") => Some(VkIcon::XboxX),
            (Self::Xbox, "Y") => Some(VkIcon::XboxY),
            (Self::Xbox, "LB") => Some(VkIcon::XboxLb),
            (Self::Xbox, "RB") => Some(VkIcon::XboxRb),
            (Self::Xbox, "LT") => Some(VkIcon::XboxLt),
            (Self::Xbox, "RT") => Some(VkIcon::XboxRt),
            (Self::Xbox, "L3") => Some(VkIcon::L3Xbox),
            (Self::Xbox, "R3") => Some(VkIcon::R3Xbox),
            (Self::Xbox, "SELECT") => Some(VkIcon::SelectXbox),
            (Self::Xbox, "START") => Some(VkIcon::StartXbox),
            _ => None,
        }
    }
}

/// Longest edge (px) the cached controller bitmap is prefiltered down to. The
/// card draws the art at roughly 110px; this keeps a few times that for HiDPI
/// headroom while still being far enough below the ~1254px source that the GPU's
/// final resample has no high frequencies left to alias.
const CONTROLLER_ART_MAX_EDGE: u32 = 384;

/// Area-averaging (box filter) downscale of a premultiplied-BGRA buffer. Returns
/// the source unchanged when it already fits within `max_edge`. Averaging in
/// premultiplied space is correct for images with transparency, so edges stay
/// clean. Runs once per controller art (results are cached).
fn downscale_bgra_premul(src: &[u8], sw: u32, sh: u32, max_edge: u32) -> (Vec<u8>, u32, u32) {
    let long_edge = sw.max(sh);
    if long_edge <= max_edge || sw == 0 || sh == 0 {
        return (src.to_vec(), sw, sh);
    }
    let scale = max_edge as f32 / long_edge as f32;
    let tw = ((sw as f32 * scale).round() as u32).max(1);
    let th = ((sh as f32 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for ty in 0..th {
        let y0 = (ty * sh / th) as usize;
        let y1 = (((ty + 1) * sh / th).max(ty * sh / th + 1).min(sh)) as usize;
        for tx in 0..tw {
            let x0 = (tx * sw / tw) as usize;
            let x1 = (((tx + 1) * sw / tw).max(tx * sw / tw + 1).min(sw)) as usize;
            let (mut b, mut g, mut r, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in y0..y1 {
                let row = sy * sw as usize * 4;
                for sx in x0..x1 {
                    let i = row + sx * 4;
                    b += src[i] as u32;
                    g += src[i + 1] as u32;
                    r += src[i + 2] as u32;
                    a += src[i + 3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = (ty as usize * tw as usize + tx as usize) * 4;
            out[o] = (b / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (r / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    (out, tw, th)
}

impl VkIcon {
    fn svg(self) -> &'static str {
        match self {
            VkIcon::Backspace => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 5a2 2 0 0 0-1.344.519l-6.328 5.74a1 1 0 0 0 0 1.481l6.328 5.741A2 2 0 0 0 10 19h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2z"/><path d="m12 9 6 6"/><path d="m18 9-6 6"/></svg>"#
            }
            VkIcon::Enter => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 4v7a4 4 0 0 1-4 4H4"/><path d="m9 10-5 5 5 5"/></svg>"#
            }
            VkIcon::Mic => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19v3"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><rect x="9" y="2" width="6" height="13" rx="3"/></svg>"#
            }
            VkIcon::MicOff => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19v3"/><path d="M15 9.34V5a3 3 0 0 0-5.68-1.33"/><path d="M16.95 16.95A7 7 0 0 1 5 12v-2"/><path d="M18.89 13.23A7 7 0 0 0 19 12v-2"/><path d="m2 2 20 20"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12"/></svg>"#
            }
            VkIcon::Space => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 17v1c0 .5-.5 1-1 1H3c-.5 0-1-.5-1-1v-1"/></svg>"#
            }
            VkIcon::Shift => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 16a1 1 0 0 0 1-1v-2a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-6.939-6.939a1.207 1.207 0 0 0-1.708 0l-6.94 6.94a.707.707 0 0 0 .5 1.206H8a1 1 0 0 1 1 1v2a1 1 0 0 0 1 1z"/><path d="M9 20h6"/></svg>"#
            }
            VkIcon::ShiftFilled => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 16a1 1 0 0 0 1-1v-2a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-6.939-6.939a1.207 1.207 0 0 0-1.708 0l-6.94 6.94a.707.707 0 0 0 .5 1.206H8a1 1 0 0 1 1 1v2a1 1 0 0 0 1 1z"/><path d="M9 20h6" fill="none"/></svg>"#
            }
            VkIcon::Caps => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 19a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1v-6a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-7.086-7.086a1 1 0 0 0-1.414 0l-7.086 7.086a.707.707 0 0 0 .5 1.207H8a1 1 0 0 1 1 1z"/></svg>"#
            }
            VkIcon::CapsFilled => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 19a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1v-6a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-7.086-7.086a1 1 0 0 0-1.414 0l-7.086 7.086a.707.707 0 0 0 .5 1.207H8a1 1 0 0 1 1 1z"/></svg>"#
            }
            VkIcon::ChevronLeft => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 18-6-6 6-6"/></svg>"#
            }
            VkIcon::ChevronRight => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>"#
            }
            VkIcon::ChevronUp => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m18 15-6-6-6 6"/></svg>"#
            }
            VkIcon::ChevronDown => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>"#
            }
            VkIcon::Gamepad => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round"><line x1="6" x2="10" y1="12" y2="12"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="15" x2="15.01" y1="13" y2="13"/><line x1="18" x2="18.01" y1="11" y2="11"/><rect width="20" height="12" x="2" y="6" rx="4"/><path d="M6 18v1a2 2 0 0 0 4 0v-1"/><path d="M14 18v1a2 2 0 0 0 4 0v-1"/></svg>"#
            }
            VkIcon::Check => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>"#
            }
            // Native-colored chips have no `currentColor`, so the palette swap in
            // `draw_svg_icon` is a no-op and they keep their controller look.
            VkIcon::L3Ps5 => include_str!("../../controller-icons/p5_l3_click.svg"),
            VkIcon::L3Xbox => include_str!("../../controller-icons/x_l3_click.svg"),
            VkIcon::R3Ps5 => include_str!("../../controller-icons/p5_r3_click.svg"),
            VkIcon::R3Xbox => include_str!("../../controller-icons/x_r3_click.svg"),
            VkIcon::SelectPs5 => include_str!("../../controller-icons/p5_share.svg"),
            VkIcon::SelectXbox => include_str!("../../controller-icons/x_menu_view.svg"),
            VkIcon::StartPs5 => include_str!("../../controller-icons/p5_options.svg"),
            VkIcon::StartXbox => include_str!("../../controller-icons/x_menu_menu.svg"),
            VkIcon::Ps5Cross => include_str!("../../controller-icons/p5_face_cross_colored.svg"),
            VkIcon::Ps5Circle => include_str!("../../controller-icons/p5_face_circle_colored.svg"),
            VkIcon::Ps5Square => include_str!("../../controller-icons/p5_face_square_colored.svg"),
            VkIcon::Ps5Triangle => {
                include_str!("../../controller-icons/p5_face_triangle_colored.svg")
            }
            VkIcon::Ps5L1 => include_str!("../../controller-icons/p5_shoulder_l1.svg"),
            VkIcon::Ps5R1 => include_str!("../../controller-icons/p5_shoulder_r1.svg"),
            VkIcon::Ps5L2 => include_str!("../../controller-icons/p5_trigger_l2.svg"),
            VkIcon::Ps5R2 => include_str!("../../controller-icons/p5_trigger_r2.svg"),
            VkIcon::XboxA => include_str!("../../controller-icons/x_face_a_colored.svg"),
            VkIcon::XboxB => include_str!("../../controller-icons/x_face_b_colored.svg"),
            VkIcon::XboxX => include_str!("../../controller-icons/x_face_x_colored.svg"),
            VkIcon::XboxY => include_str!("../../controller-icons/x_face_y_colored.svg"),
            VkIcon::XboxLb => include_str!("../../controller-icons/x_shoulder_lb.svg"),
            VkIcon::XboxRb => include_str!("../../controller-icons/x_shoulder_rb.svg"),
            VkIcon::XboxLt => include_str!("../../controller-icons/x_trigger_lt.svg"),
            VkIcon::XboxRt => include_str!("../../controller-icons/x_trigger_rt.svg"),
        }
    }

    fn is_controller_tip(self) -> bool {
        matches!(
            self,
            VkIcon::L3Ps5
                | VkIcon::L3Xbox
                | VkIcon::R3Ps5
                | VkIcon::R3Xbox
                | VkIcon::SelectPs5
                | VkIcon::SelectXbox
                | VkIcon::StartPs5
                | VkIcon::StartXbox
                | VkIcon::Ps5Cross
                | VkIcon::Ps5Circle
                | VkIcon::Ps5Square
                | VkIcon::Ps5Triangle
                | VkIcon::Ps5L1
                | VkIcon::Ps5R1
                | VkIcon::Ps5L2
                | VkIcon::Ps5R2
                | VkIcon::XboxA
                | VkIcon::XboxB
                | VkIcon::XboxX
                | VkIcon::XboxY
                | VkIcon::XboxLb
                | VkIcon::XboxRb
                | VkIcon::XboxLt
                | VkIcon::XboxRt
        )
    }
}

/// Natural bounding box `(width, height)` of the key grid at `scale_w`, excluding
/// card padding and top chrome. Lets the floating card be sized to wrap keys that
/// render at the same scale as the docked bar.
pub fn grid_size(scale_w: f32, rows: &[KeyRow], style: VkStyle) -> (f32, f32) {
    let (kw, kh, gap) = key_metrics(scale_w, f32::INFINITY, rows, 0.0, style);
    let grid_w = rows
        .iter()
        .map(|r| row_pixel_width(r, kw, gap))
        .fold(0.0f32, f32::max);
    let n = rows.len() as f32;
    let block_h = n * kh + (n - 1.0).max(0.0) * gap;
    (grid_w, block_h)
}
/// Key width in px for a span of `n` key-units (`FUN_00463bd0`: span×keyW + (span−1)×gap).
fn key_width(kw: f32, gap: f32, span: f32) -> f32 {
    span * kw + (span - 1.0).max(0.0) * gap
}

/// One key's on-screen rect (logical px). Single source of layout truth shared by
/// [`VkRenderer::draw`] and `vk_ui::hit_test` so clicks always land on what's drawn.
pub struct KeyRect {
    pub pos: KeyPos,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl KeyRect {
    fn rect(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StripGeom {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    unit: f32,
}

impl StripGeom {
    fn above(rects: &[KeyRect], spec: &StyleSpec, unit: f32) -> Self {
        if rects.is_empty() {
            return Self::default();
        }
        let left = rects.iter().map(|r| r.left).fold(f32::INFINITY, f32::min);
        let right = rects.iter().map(|r| r.right).fold(f32::NEG_INFINITY, f32::max);
        let grid_top = rects.iter().map(|r| r.top).fold(f32::INFINITY, f32::min);
        let bottom = grid_top - spec.gap * unit;
        let top = (bottom - spec.strip_bar_h * unit).max(0.0);
        Self {
            left,
            top,
            right,
            bottom,
            unit,
        }
    }

    fn cy(&self) -> f32 {
        (self.top + self.bottom) * 0.5
    }

    fn pill_frame(&self, spec: &StyleSpec) -> PillFrame {
        let u = self.unit;
        let cy = self.cy();
        let button_w = spec.strip_button_w * u;
        let button_h = spec.strip_button_h * u;
        let button_gap = spec.strip_button_gap * u;
        let button = |left: f32| D2D_RECT_F {
            left,
            top: cy - button_h * 0.5,
            right: left + button_w,
            bottom: cy + button_h * 0.5,
        };
        let prev = button(self.left);
        let next = button(self.right - button_w);
        let chips_h = spec.chips_h * u;
        let chips = D2D_RECT_F {
            left: prev.right + button_gap,
            top: cy - chips_h * 0.5,
            right: next.left - button_gap,
            bottom: cy + chips_h * 0.5,
        };
        let inner = deflate(chips, spec.chips_pad * u);
        PillFrame {
            prev,
            next,
            chips,
            inner,
            chip_r: ((inner.bottom - inner.top) * 0.5).max(0.0),
        }
    }

    fn pill_slot_rects(&self, spec: &StyleSpec, visible: &[String]) -> Vec<(usize, D2D_RECT_F)> {
        let frame = self.pill_frame(spec);
        if frame.chips.right - frame.chips.left < 1.0 {
            return Vec::new();
        }
        let inner = frame.inner;
        let chip_gap = spec.chips_gap * self.unit;
        let filled: Vec<usize> = visible
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.is_empty())
            .map(|(i, _)| i)
            .collect();
        let n = filled.len() as f32;
        let chip_w = ((inner.right - inner.left) - chip_gap * (n - 1.0)) / n;
        filled
            .into_iter()
            .enumerate()
            .map(|(k, slot)| {
                let left = inner.left + k as f32 * (chip_w + chip_gap);
                (
                    slot,
                    D2D_RECT_F {
                        left,
                        top: inner.top,
                        right: left + chip_w,
                        bottom: inner.bottom,
                    },
                )
            })
            .collect()
    }

    fn column_hints(&self, spec: &StyleSpec, engaged: bool) -> Vec<(&'static str, D2D_RECT_F)> {
        let lane = spec.strip_button_w * self.unit;
        let side = spec.strip_hint_px * self.unit;
        let cy = self.cy();
        let left = square_about(self.left + lane * 0.5, cy, side);
        let right = square_about(self.right - lane * 0.5, cy, side);
        if engaged {
            vec![("LB", left), ("RB", right)]
        } else {
            vec![("SELECT", left)]
        }
    }

    fn column_rects(&self, spec: &StyleSpec, slots: usize) -> Vec<D2D_RECT_F> {
        let lane = spec.strip_button_w * self.unit;
        let left = self.left + lane;
        let right = (self.right - lane).max(left);
        let slots = slots.max(1);
        let col_w = (right - left) / slots as f32;
        (0..slots)
            .map(|i| D2D_RECT_F {
                left: left + i as f32 * col_w,
                top: self.top,
                right: left + (i + 1) as f32 * col_w,
                bottom: self.bottom,
            })
            .collect()
    }

    fn slot_rects(&self, spec: &StyleSpec, visible: &[String]) -> Vec<(usize, D2D_RECT_F)> {
        match spec.strip {
            StripLook::Pills => self.pill_slot_rects(spec, visible),
            StripLook::Columns => self
                .column_rects(spec, visible.len())
                .into_iter()
                .enumerate()
                .filter(|(i, _)| visible.get(*i).is_some_and(|w| !w.is_empty()))
                .collect(),
        }
    }
}

struct PillFrame {
    prev: D2D_RECT_F,
    next: D2D_RECT_F,
    chips: D2D_RECT_F,
    inner: D2D_RECT_F,
    chip_r: f32,
}

pub fn strip_hit_slot(
    client_w: f32,
    client_h: f32,
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    style: VkStyle,
    visible: &[String],
    x: f32,
    y: f32,
) -> Option<usize> {
    let spec = style_spec(style);
    let rects = key_rects(client_w, client_h, scale_w, rows, top_inset, style);
    let key_h = rects.first().map(|kr| kr.bottom - kr.top)?;
    let geom = StripGeom::above(&rects, spec, key_h / spec.design_kh);
    geom.slot_rects(spec, visible)
        .into_iter()
        .find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
        .map(|(slot, _)| slot)
}

fn key_icon(action: &KeyAction, shift: bool) -> Option<(VkIcon, bool)> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_BACK, VK_DOWN, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP,
    };
    match action {
        KeyAction::Vk(vk) if *vk == VK_BACK => Some((VkIcon::Backspace, true)),
        KeyAction::Vk(vk) if *vk == VK_RETURN => Some((VkIcon::Enter, true)),
        KeyAction::Vk(vk) if *vk == VK_LEFT => Some((VkIcon::ChevronLeft, false)),
        KeyAction::Vk(vk) if *vk == VK_RIGHT => Some((VkIcon::ChevronRight, false)),
        KeyAction::Vk(vk) if *vk == VK_UP => Some((VkIcon::ChevronUp, false)),
        KeyAction::Vk(vk) if *vk == VK_DOWN => Some((VkIcon::ChevronDown, false)),
        KeyAction::PredictPrev => Some((VkIcon::ChevronLeft, false)),
        KeyAction::PredictNext => Some((VkIcon::ChevronRight, false)),
        KeyAction::Shift => Some((shift_icon(shift), false)),
        _ => None,
    }
}

/// Compute every key's rect for the given client size + layout rows. Each key's
/// width is `span * kw` so the wide space bar covers several key-units.
fn row_pixel_width(row: &KeyRow, kw: f32, gap: f32) -> f32 {
    row.keys
        .iter()
        .map(|k| key_width(kw, gap, k.span))
        .sum::<f32>()
        + gap * (row.keys.len().saturating_sub(1) as f32)
}

pub fn key_rects(
    client_w: f32,
    client_h: f32,
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    style: VkStyle,
) -> Vec<KeyRect> {
    let (kw, kh, gap) = key_metrics(scale_w, client_h, rows, top_inset, style);
    let n = rows.len() as f32;
    let block_h = n * kh + (n - 1.0).max(0.0) * gap;
    let mut top = top_inset + ((client_h - top_inset - block_h) / 2.0).max(0.0);
    // Widest row sets the block width (same span sum can differ in pixel width by gap count).
    let grid_w = rows
        .iter()
        .map(|r| row_pixel_width(r, kw, gap))
        .fold(0.0f32, f32::max);
    let grid_left = (client_w - grid_w) / 2.0;
    let mut out = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        // Flex-grow parity with the web rows: distribute the row's slack across
        // every key proportionally so all rows share the same left/right edge.
        let gaps_w = gap * (row.keys.len().saturating_sub(1) as f32);
        let row_keys_w = (row_pixel_width(row, kw, gap) - gaps_w).max(1.0);
        let scale = (grid_w - gaps_w).max(1.0) / row_keys_w;
        let mut left = grid_left;
        for (ci, key) in row.keys.iter().enumerate() {
            let w = key_width(kw, gap, key.span) * scale;
            out.push(KeyRect {
                pos: KeyPos { row: ri, col: ci },
                left,
                top,
                right: left + w,
                bottom: top + kh,
            });
            left += w + gap;
        }
        top += kh + gap;
    }
    out
}

/// Shift/caps captured for one frame, so the glyph loop never re-reads global
/// nav state mid-draw. Same `VkModifiers` + same rows -> same pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VkModifiers {
    pub shift: bool,
    pub caps: bool,
    pub symbol: bool,
    pub symbol_locked: bool,
}

/// One immutable snapshot of everything the VK renderer needs for a frame.
/// `render_frame` assembles it from a single logical read of nav/predict state;
/// `draw` consumes only `&VkFrame` and performs no global reads, so the
/// selection/glyph-branch logic is testable without a NAV lock or a D2D device.
/// Voice helper phase, for the phase-coded mic halo (so startup/transcribe don't
/// look identical to idle listening).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoicePhase {
    Starting,
    Listening,
    Transcribing,
}

pub struct VkFrame<'a> {
    pub pal: &'a VkPalette,
    pub rows: &'a [KeyRow],
    pub sel: KeyPos,
    pub key_glyph: fn(&KeyCell) -> (String, bool),
    pub key_hint: fn(&KeyCell) -> Option<&'static str>,
    pub top_inset: f32,
    pub scale_w: f32,
    pub candidates: Option<&'a crate::vk_predict::StripState>,
    pub floating: bool,
    pub modifiers: VkModifiers,
    /// Key that just fired and its current press-feedback scale (`vk_motion::press_scale`),
    /// `None` once settled. The fill, label and badge dip together about the
    /// key's centre; the layout rects never move.
    pub pressed: Option<(KeyPos, f32)>,
    pub controller_label: &'a str,
    /// Voice input is reachable on this surface (false on Winlogon, where
    /// LocalSystem has no mic consent). Drives the mic key's truthful state.
    pub voice_available: bool,
    /// Voice recognition is currently listening.
    pub voice_active: bool,
    /// Helper phase, for a phase-coded mic halo (only read when `voice_active`).
    pub voice_phase: VoicePhase,
    /// Live mic energy, 0..1, used by the mic-key voice glow.
    pub voice_level: f32,
    pub ui_scale: f32,
    pub style: VkStyle,
    pub shortcut_sheet: bool,
    pub legend: bool,
}

/// Glyph for the Shift key (the Shift-action key reflects `shift`).
fn shift_icon(shift: bool) -> VkIcon {
    if shift {
        VkIcon::CapsFilled
    } else {
        VkIcon::Caps
    }
}

/// Glyph for sticky caps (kept for the Shift key's promoted state).
#[allow(dead_code)]
fn caps_icon(caps: bool) -> VkIcon {
    if caps {
        VkIcon::ShiftFilled
    } else {
        VkIcon::Shift
    }
}

impl VkRenderer {
    pub unsafe fn create(hwnd: HWND) -> Result<Self, String> {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).map_err(|e| format!("GetClientRect: {e}"))?;
        let width = (client.right - client.left).max(1) as u32;
        let height = (client.bottom - client.top).max(1) as u32;
        let on_secure = crate::win::surface::thread().is_some_and(|s| s.is_winlogon());
        Self::build(width, height, Some(hwnd), on_secure)
    }

    unsafe fn build(
        width: u32,
        height: u32,
        hwnd: Option<HWND>,
        prefer_warp: bool,
    ) -> Result<Self, String> {
        // NVIDIA's D3D11 user-mode driver (nvwgf2umx.dll) faults with 0xC0000005
        // when driven on the Winlogon secure desktop — the GPU context there is
        // unreliable (confirmed via minidump). On the secure desktop, render with
        // the WARP software rasterizer, which never loads the vendor UMD. Userland
        // keeps hardware for perf. Either way, fall back to the other on failure.
        let d3d = create_d3d_device(prefer_warp)?;
        let dxgi_device: IDXGIDevice = d3d.cast().map_err(|e| format!("IDXGIDevice: {e}"))?;

        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
            .map_err(|e| format!("CreateDXGIFactory2: {e}"))?;

        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let swapchain = factory
            .CreateSwapChainForComposition(&dxgi_device, &desc, None)
            .map_err(|e| format!("CreateSwapChainForComposition: {e}"))?;

        let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory: {e}"))?;
        let d2d_device = d2d_factory
            .CreateDevice(&dxgi_device)
            .map_err(|e| format!("ID2D1Factory1::CreateDevice: {e}"))?;
        let d2d_context = d2d_device
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .map_err(|e| format!("CreateDeviceContext: {e}"))?;
        configure_d2d_quality(&d2d_context);

        let d2d_target = bind_d2d_target(&d2d_context, &swapchain)?;

        let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)
            .map_err(|e| format!("DCompositionCreateDevice: {e}"))?;
        let (comp_target, visual) = match hwnd {
            Some(hwnd) => {
                let comp_target = dcomp_device
                    .CreateTargetForHwnd(hwnd, true)
                    .map_err(|e| format!("CreateTargetForHwnd: {e}"))?;
                let visual = dcomp_device
                    .CreateVisual()
                    .map_err(|e| format!("CreateVisual: {e}"))?;
                visual
                    .SetContent(&swapchain)
                    .map_err(|e| format!("SetContent: {e}"))?;
                comp_target
                    .SetRoot(&visual)
                    .map_err(|e| format!("SetRoot: {e}"))?;
                dcomp_device
                    .Commit()
                    .map_err(|e| format!("DComp Commit: {e}"))?;
                (Some(comp_target), Some(visual))
            }
            None => (None, None),
        };

        let dwrite = create_dwrite()?;
        let mut fonts: Option<IDWriteFontCollection> = None;
        dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let locale = user_locale_name();
        // Font scales with the docked bar height so labels fill the larger keys
        // (bar ~384px @1080p -> ~32px labels).
        let label_px = (height as f32 / 12.0).clamp(14.0, 48.0);
        // Segoe UI labels; Segoe MDL2 Assets when icon row enabled.
        let text_format = dwrite
            .CreateTextFormat(
                w!("Segoe UI"),
                &fonts,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                label_px,
                &locale,
            )
            .map_err(|e| format!("CreateTextFormat (Segoe UI): {e}"))?;
        let glyph_format = dwrite
            .CreateTextFormat(
                w!("Segoe UI Symbol"),
                &fonts,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                label_px * 1.1,
                &locale,
            )
            .map_err(|e| format!("CreateTextFormat (Segoe UI Symbol): {e}"))?;
        let hint_format = dwrite
            .CreateTextFormat(
                w!("Segoe UI"),
                &fonts,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                (label_px * 0.5).clamp(10.0, 20.0),
                &locale,
            )
            .map_err(|e| format!("CreateTextFormat (hint): {e}"))?;
        // Fixed large font for the connect/keyboard prompt pills. The pill window
        // is short, so `label_px` floors at 14; this is ~2x that so the prompt
        // reads on a TV across the room (10-foot UI).
        let prompt_format = dwrite
            .CreateTextFormat(
                w!("Segoe UI"),
                &fonts,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                28.0,
                &locale,
            )
            .map_err(|e| format!("CreateTextFormat (prompt): {e}"))?;

        // Centre labels in their key rects (DWrite defaults to top-left).
        for f in [&text_format, &glyph_format] {
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        // Badges/legend: horizontally centred, anchored to the top of their rect.
        let _ = hint_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = hint_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);

        Ok(Self {
            width,
            height,
            swapchain,
            d2d_context,
            d2d_target: Some(d2d_target),
            dwrite,
            text_format,
            glyph_format,
            hint_format,
            style_fonts: None,
            prompt_format,
            prompt_fonts: None,
            icon_cache: HashMap::new(),
            controller_art_cache: HashMap::new(),
            prompt_started: Instant::now(),
            anim_sel: None,
            last_draw: None,
            strip_shown_at: None,
            level: 0.0,
            level_at: None,
            nimbus: None,
            nimbus_failed: false,
            sheet_alpha: 0.0,
            sheet_tick: None,
            text_formats: std::cell::RefCell::new(HashMap::new()),
            d3d,
            _d2d_device: d2d_device,
            _dcomp_device: dcomp_device,
            _comp_target: comp_target,
            _visual: visual,
        })
    }

    pub unsafe fn resize(&mut self, hwnd: HWND) -> Result<(), String> {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).map_err(|e| format!("GetClientRect: {e}"))?;
        let width = (client.right - client.left).max(1) as u32;
        let height = (client.bottom - client.top).max(1) as u32;
        if width == self.width && height == self.height {
            return Ok(());
        }
        // The bitmap is an extra ref on the back buffer. ResizeBuffers returns
        // DXGI_ERROR_INVALID_CALL while any ref is alive, and SetTarget(None)
        // alone does not drop this one.
        self.d2d_context.SetTarget(None);
        let _ = self.d2d_context.Flush(None, None);
        self.d2d_target = None;
        if let Err(e) = self.swapchain.ResizeBuffers(
            0,
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_SWAP_CHAIN_FLAG(0),
        ) {
            if let Ok(bmp) = bind_d2d_target(&self.d2d_context, &self.swapchain) {
                self.d2d_target = Some(bmp);
            }
            return Err(format!("ResizeBuffers: {e}"));
        }
        self.width = width;
        self.height = height;
        self.d2d_target = Some(bind_d2d_target(&self.d2d_context, &self.swapchain)?);
        Ok(())
    }

    unsafe fn ensure_style_fonts(&mut self, style: VkStyle, key_h: f32) -> Result<(), String> {
        let size_key = (key_h * 4.0).round() as i32;
        if self
            .style_fonts
            .as_ref()
            .is_some_and(|f| f.style == style && f.size_key == size_key)
        {
            return Ok(());
        }
        let mut fonts: Option<IDWriteFontCollection> = None;
        self.dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let locale = user_locale_name();
        let spec = style_spec(style);
        let family = resolve_family(&fonts, spec.families);
        let u = key_h.max(1.0) / spec.design_kh;
        let dwrite = &self.dwrite;
        let make = |weight: DWRITE_FONT_WEIGHT, px: f32| -> Result<IDWriteTextFormat, String> {
            let f = dwrite
                .CreateTextFormat(
                    &family,
                    &fonts,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    (px * u).max(1.0),
                    &locale,
                )
                .map_err(|e| format!("CreateTextFormat ({family}): {e}"))?;
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            Ok(f)
        };
        let built = StyleFonts {
            style,
            size_key,
            label: make(spec.label_weight, spec.label_px)?,
            word: make(spec.word_weight, spec.word_px)?,
            number: make(DWRITE_FONT_WEIGHT_NORMAL, spec.number_px)?,
            space: make(DWRITE_FONT_WEIGHT_NORMAL, spec.space_label_px)?,
            chip: make(DWRITE_FONT_WEIGHT_NORMAL, spec.chip_px)?,
            chip_sel: make(spec.chip_sel_weight, spec.chip_px)?,
        };
        self.style_fonts = Some(built);
        Ok(())
    }

    unsafe fn draw_svg_icon(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
    ) -> Result<(), String> {
        self.draw_svg_icon_alpha(icon, rect, color, 1.0)
    }

    unsafe fn draw_svg_icon_alpha(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
        opacity: f32,
    ) -> Result<(), String> {
        let h = rect.bottom - rect.top;
        let draw_px = match icon {
            icon if icon.is_controller_tip() => (h * 0.98).round().clamp(24.0, h.max(64.0)),
            _ => (h * 0.5).round().clamp(16.0, 96.0),
        };
        self.draw_svg_icon_sized(icon, rect, color, opacity, draw_px)
    }

    unsafe fn draw_svg_icon_sized(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
        opacity: f32,
        draw_px: f32,
    ) -> Result<(), String> {
        let draw_px = draw_px.round().clamp(12.0, 192.0);
        let raster_px = match icon {
            icon if icon.is_controller_tip() => (draw_px * 3.0).round().clamp(54.0, (draw_px * 3.0).max(192.0)),
            _ => draw_px,
        } as u32;
        let key = IconCacheKey {
            icon,
            px: raster_px,
            color,
        };
        if !self.icon_cache.contains_key(&key) {
            let svg = icon.svg().replace("currentColor", &colorref_hex(color));
            let opt = resvg::usvg::Options::default();
            let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &opt)
                .map_err(|e| format!("parse svg icon {icon:?}: {e}"))?;
            let mut pixmap = resvg::tiny_skia::Pixmap::new(raster_px, raster_px)
                .ok_or_else(|| format!("alloc svg icon pixmap {raster_px}x{raster_px}"))?;
            let source_px = if icon.is_controller_tip() { 32.0 } else { 24.0 };
            let scale = raster_px as f32 / source_px;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );

            let mut bgra = pixmap.data().to_vec();
            for px in bgra.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: ManuallyDrop::new(None),
            };
            let bitmap = self
                .d2d_context
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: raster_px,
                        height: raster_px,
                    },
                    Some(bgra.as_ptr() as *const core::ffi::c_void),
                    raster_px * 4,
                    &props,
                )
                .map_err(|e| format!("CreateBitmap svg icon {icon:?}: {e}"))?;
            self.icon_cache.insert(key, bitmap);
        }

        let bitmap = self
            .icon_cache
            .get(&key)
            .ok_or_else(|| format!("missing svg icon cache {icon:?}"))?;
        let size = draw_px;
        let dest = D2D_RECT_F {
            left: (rect.left + rect.right - size) * 0.5,
            top: (rect.top + rect.bottom - size) * 0.5,
            right: (rect.left + rect.right + size) * 0.5,
            bottom: (rect.top + rect.bottom + size) * 0.5,
        };
        self.d2d_context.DrawBitmap(
            bitmap,
            Some(&dest),
            opacity.clamp(0.0, 1.0),
            D2D1_INTERPOLATION_MODE_LINEAR,
            None,
            None,
        );
        Ok(())
    }

    /// Decode + downscale the controller PNG for `controller_label` into the
    /// bitmap cache now, off the animation path. The 1254px PNG takes long
    /// enough to decode that doing it lazily on the first card frame stutters.
    pub unsafe fn preload_controller_art(&mut self, controller_label: &str) -> Result<(), String> {
        match ControllerArt::from_label(controller_label) {
            Some(art) => self.ensure_controller_art(art),
            None => Ok(()),
        }
    }

    unsafe fn ensure_controller_art(&mut self, art: ControllerArt) -> Result<(), String> {
        let key = ControllerArtCacheKey { art };
        if !self.controller_art_cache.contains_key(&key) {
            let decoder = png::Decoder::new(std::io::Cursor::new(art.png()));
            let mut reader = decoder
                .read_info()
                .map_err(|e| format!("decode controller art {art:?}: {e}"))?;
            let out_size = reader
                .output_buffer_size()
                .ok_or_else(|| format!("controller art {art:?}: unknown decoded size"))?;
            let mut decoded = vec![0; out_size];
            let info = reader
                .next_frame(&mut decoded)
                .map_err(|e| format!("read controller art {art:?}: {e}"))?;
            let bytes = &decoded[..info.buffer_size()];
            let mut bgra = Vec::with_capacity((info.width * info.height * 4) as usize);
            match info.color_type {
                png::ColorType::Rgba => {
                    for px in bytes.chunks_exact(4) {
                        let a = px[3] as u16;
                        let premul = |c: u8| ((c as u16 * a + 127) / 255) as u8;
                        bgra.extend_from_slice(&[
                            premul(px[2]),
                            premul(px[1]),
                            premul(px[0]),
                            px[3],
                        ]);
                    }
                }
                png::ColorType::Rgb => {
                    for px in bytes.chunks_exact(3) {
                        bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                }
                other => {
                    return Err(format!(
                        "controller art {art:?}: unsupported PNG color {other:?}"
                    ))
                }
            }

            // Prefilter to a modest size so the final GPU resample down to the
            // ~110px card slot has no aliasing-prone high frequencies left.
            let (bgra, art_w, art_h) =
                downscale_bgra_premul(&bgra, info.width, info.height, CONTROLLER_ART_MAX_EDGE);

            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: ManuallyDrop::new(None),
            };
            let bitmap = self
                .d2d_context
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: art_w,
                        height: art_h,
                    },
                    Some(bgra.as_ptr() as *const core::ffi::c_void),
                    art_w * 4,
                    &props,
                )
                .map_err(|e| format!("CreateBitmap controller art {art:?}: {e}"))?;
            self.controller_art_cache
                .insert(key, (bitmap, art_w, art_h));
        }
        Ok(())
    }

    unsafe fn draw_controller_art_alpha(
        &mut self,
        art: ControllerArt,
        rect: D2D_RECT_F,
        opacity: f32,
    ) -> Result<(), String> {
        self.ensure_controller_art(art)?;
        let key = ControllerArtCacheKey { art };
        let (bitmap, width, height) = self
            .controller_art_cache
            .get(&key)
            .ok_or_else(|| format!("missing controller art cache {art:?}"))?;
        let source_aspect = *width as f32 / *height as f32;
        let fit_w = rect.right - rect.left;
        let fit_h = rect.bottom - rect.top;
        let (draw_w, draw_h) = if fit_w / fit_h > source_aspect {
            (fit_h * source_aspect, fit_h)
        } else {
            (fit_w, fit_w / source_aspect)
        };
        let dest = D2D_RECT_F {
            left: (rect.left + rect.right - draw_w) * 0.5,
            top: (rect.top + rect.bottom - draw_h) * 0.5,
            right: (rect.left + rect.right + draw_w) * 0.5,
            bottom: (rect.top + rect.bottom + draw_h) * 0.5,
        };
        self.d2d_context.DrawBitmap(
            bitmap,
            Some(&dest),
            opacity.clamp(0.0, 1.0),
            D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            None,
            None,
        );
        Ok(())
    }

    /// Advance the mic-level envelope toward `target` for this frame.
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

    unsafe fn draw_voice_orb(
        &mut self,
        accent: u32,
        level: f32,
        transcribing: bool,
        cx: f32,
        cy: f32,
        unit: f32,
        alpha_scale: f32,
    ) -> Result<(), String> {
        let t = self.prompt_started.elapsed().as_secs_f32();
        let amp = if transcribing {
            (t * std::f32::consts::TAU * crate::vk_motion::TRANSCRIBE_PULSE_HZ).sin() * 0.5 + 0.5
        } else {
            level.clamp(0.0, 1.0)
        };
        let idle = (t * std::f32::consts::TAU * 0.25).sin() * 0.5 + 0.5;
        let energy = amp.max(idle * 0.10).clamp(0.0, 1.0);
        let max_r = unit * 0.92;

        let blobs: [(u32, f32, f32); 4] = [
            (shift_hue(accent, -34.0), 0.0, 0.85),
            (shift_hue(accent, -10.0), 2.1, 1.10),
            (shift_hue(accent, 16.0), 4.2, 0.70),
            (shift_hue(accent, 38.0), 1.0, 1.30),
        ];
        let drift = unit * (0.05 + 0.08 * energy);
        let base_r = unit * (0.42 + 0.24 * energy);
        let wob = 0.5 + 1.5 * energy;
        let jitter = |seed: f32| {
            let sp = 0.6 + 1.8 * energy;
            ((seed * 2.3999632 + t * sp).sin() + (seed * 5.197 - t * sp * 0.62).sin()) * 0.5 * wob
        };
        const LAYERS: usize = 11;
        for (bi, (color, phase, freq)) in blobs.into_iter().enumerate() {
            let ang = t * freq * 0.45 + phase;
            let bx = cx + ang.cos() * drift;
            let by = cy + ang.sin() * drift;
            let dist = ((bx - cx).powi(2) + (by - cy).powi(2)).sqrt();
            let r = (base_r * (0.90 + 0.12 * (ang * 1.3).sin()))
                .min(max_r - dist)
                .max(unit * 0.12);
            let brush = solid_brush(&self.d2d_context, colorref(color))?;
            let seed0 = bi as f32 * 9.71;
            for k in 0..LAYERS {
                let kf = k as f32 / (LAYERS - 1) as f32;
                let s = seed0 + k as f32;
                let rr = r * (1.0 - 0.80 * kf) * (1.0 + 0.20 * jitter(s));
                let jx = bx + unit * 0.06 * jitter(s + 1.3);
                let jy = by + unit * 0.06 * jitter(s + 7.7);
                brush.SetOpacity(((0.13 + 0.08 * energy) * (0.22 + 0.78 * kf * kf)) * alpha_scale);
                self.d2d_context.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x: jx, y: jy },
                        radiusX: rr,
                        radiusY: rr,
                    },
                    &brush,
                );
            }
        }

        let core_brush = solid_brush(&self.d2d_context, colorref(0x00FFFFFF))?;
        for k in 0..LAYERS {
            let kf = k as f32 / (LAYERS - 1) as f32;
            let rr = unit * (0.04 + (0.10 + 0.08 * energy) * (1.0 - kf));
            core_brush.SetOpacity(((0.022 + 0.09 * energy) * kf * kf) * alpha_scale);
            self.d2d_context.FillEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: cx, y: cy },
                    radiusX: rr,
                    radiusY: rr,
                },
                &core_brush,
            );
        }
        Ok(())
    }

    pub unsafe fn draw(&mut self, frame: &VkFrame) -> Result<(), String> {
        self.draw_frame(frame)?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))?;
        Ok(())
    }

    unsafe fn draw_frame(&mut self, frame: &VkFrame) -> Result<(), String> {
        let VkFrame {
            pal,
            rows,
            sel,
            key_glyph,
            key_hint,
            top_inset,
            scale_w,
            candidates,
            floating,
            modifiers,
            pressed,
            controller_label,
            voice_available,
            voice_active,
            voice_phase,
            voice_level,
            ui_scale,
            style,
            shortcut_sheet: sheet_on,
            legend,
        } = *frame;
        let spec = style_spec(style);
        let controller_icons = ControllerIconFamily::from_label(controller_label);
        let cw = self.width as f32;
        let ch = self.height as f32;
        let now = Instant::now();
        // The cloud uses the immediate context, so it has to land before D2D
        // opens its draw on that same context. Listening and transcription both
        // use it; the ellipse orb is only the fallback if the shader never comes up.
        let voice_level = if voice_active {
            self.smoothed_level(voice_level, now)
        } else {
            0.0
        };
        if voice_active {
            self.prepare_nimbus(now, nimbus_mood(voice_phase, voice_level), pal.accent);
        }

        let legend_h = if legend {
            legend_band_height(ui_scale, style)
        } else {
            0.0
        };
        let rects = key_rects(cw, ch - legend_h, scale_w, rows, top_inset, style);
        let key_h = rects
            .first()
            .map(|kr| kr.bottom - kr.top)
            .unwrap_or_else(|| ref_key_h(ui_scale, style));
        let unit = key_h / spec.design_kh;
        self.ensure_style_fonts(style, key_h)?;
        let fonts = self
            .style_fonts
            .clone()
            .ok_or_else(|| "style fonts missing".to_string())?;

        self.d2d_context.BeginDraw();
        // Per-key press transforms below are scoped; start every frame clean in
        // case a previous frame bailed mid-key.
        self.d2d_context.SetTransform(&IDENTITY);

        // Suggestion strip entrance clock: arm on hidden->shown, drop on hide.
        match (candidates.is_some(), self.strip_shown_at) {
            (true, None) => self.strip_shown_at = Some(now),
            (false, Some(_)) => self.strip_shown_at = None,
            _ => {}
        }
        let (strip_alpha, strip_dy) = self
            .strip_shown_at
            .map(|t| crate::vk_motion::strip_enter(now.duration_since(t).as_secs_f32() * 1000.0))
            .unwrap_or((1.0, 0.0));

        // Ease the focus ring toward the selected key. The accent fill still snaps
        // (so each key's label colour is unambiguous); only the bright ring glides,
        // which reads as the cursor moving. Frame-rate-independent via dt.
        if let Some(tgt) = rects
            .iter()
            .find(|kr| kr.pos.row == sel.row && kr.pos.col == sel.col)
            .map(|kr| kr.rect())
        {
            let dt = self
                .last_draw
                .map(|t| now.duration_since(t).as_secs_f32())
                .unwrap_or(0.0);
            self.last_draw = Some(now);
            self.anim_sel = Some(match self.anim_sel {
                Some(cur) if dt > 0.0 => {
                    let t = (1.0 - (-dt / SEL_GLIDE_TAU).exp()).clamp(0.0, 1.0);
                    D2D_RECT_F {
                        left: lerp(cur.left, tgt.left, t),
                        top: lerp(cur.top, tgt.top, t),
                        right: lerp(cur.right, tgt.right, t),
                        bottom: lerp(cur.bottom, tgt.bottom, t),
                    }
                }
                _ => tgt, // first frame (or no dt): snap onto the selection
            });
        }

        if floating {
            // Floating layout emulates the webview VK card. The window is already sized to wrap
            // the chips + keys (see `vk_dock_rect`), so the rounded panel fills the whole client
            // area minus a hairline for the antialiased stroke; content is clipped to it.
            self.d2d_context.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            let radius = spec.panel_radius * unit;
            let panel = D2D_RECT_F {
                left: FLOATING_PANEL_INSET,
                top: FLOATING_PANEL_INSET,
                right: cw - FLOATING_PANEL_INSET,
                bottom: ch - FLOATING_PANEL_INSET,
            };
            let bg_brush = solid_brush(&self.d2d_context, colorref(pal.bg))?;
            let panel_border = solid_brush(
                &self.d2d_context,
                colorref_alpha(pal.panel_stroke, pal.panel_stroke_alpha),
            )?;
            self.d2d_context
                .FillRoundedRectangle(&rounded(panel, radius), &bg_brush);
            self.d2d_context.DrawRoundedRectangle(
                &rounded(deflate(panel, 0.5), (radius - 0.5).max(0.0)),
                &panel_border,
                1.0,
                None,
            );
            self.d2d_context.PushAxisAlignedClip(
                &panel,
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        } else {
            self.d2d_context.Clear(Some(&colorref(pal.bg)));
        }

        let key_brush = solid_brush(&self.d2d_context, colorref(pal.key))?;
        let action_brush = solid_brush(&self.d2d_context, colorref(pal.key_action))?;
        let accent_brush = solid_brush(&self.d2d_context, colorref(pal.accent))?;
        let text_brush = solid_brush(&self.d2d_context, colorref(pal.text))?;
        let sel_text_brush = solid_brush(&self.d2d_context, colorref(pal.sel_text))?;
        let dim_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(pal.text_dim, spec.number_alpha),
        )?;
        let sel_dim_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(pal.sel_text, spec.number_alpha),
        )?;
        let shadow_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(0x000000, spec.key_shadow_alpha),
        )?;
        let sel_ring_brush = solid_brush(&self.d2d_context, colorref(pal.sel_ring))?;
        let radius = spec.key_radius * unit;

        for kr in &rects {
            let key = &rows[kr.pos.row].keys[kr.pos.col];
            let selected = sel.row == kr.pos.row && sel.col == kr.pos.col;
            // Press feedback: the key that just fired dips (0.96 -> 1.0) about its
            // own centre, so the interface visibly acknowledges every keystroke.
            let press = pressed
                .filter(|(p, _)| p.row == kr.pos.row && p.col == kr.pos.col)
                .map(|(_, s)| s);
            if let Some(s) = press {
                self.d2d_context.SetTransform(&scale_about(
                    s,
                    (kr.left + kr.right) * 0.5,
                    (kr.top + kr.bottom) * 0.5,
                ));
            }
            let key_rect = kr.rect();
            let rect = rounded(key_rect, radius);
            let action_key = is_action_key(&key.action);
            if selected {
                if let Some((dy, blur, alpha)) = spec.sel_shadow {
                    draw_blur_shadow(
                        &self.d2d_context,
                        key_rect,
                        radius,
                        dy * unit,
                        blur * unit,
                        alpha,
                    )?;
                }
            } else if spec.key_shadow_dy > 0.0 {
                let dy = (spec.key_shadow_dy * unit).max(1.0);
                self.d2d_context.FillRoundedRectangle(
                    &rounded(
                        D2D_RECT_F {
                            top: key_rect.top + dy,
                            bottom: key_rect.bottom + dy,
                            ..key_rect
                        },
                        radius,
                    ),
                    &shadow_brush,
                );
            }
            let modifier_active = !selected
                && ((matches!(key.action, KeyAction::Shift) && modifiers.shift)
                    || (matches!(key.action, KeyAction::Symbols) && modifiers.symbol));
            let modifier_tint_brush;
            let (fill, fill_color, label_brush, dim) = if selected {
                (&accent_brush, pal.accent, &sel_text_brush, &sel_dim_brush)
            } else if modifier_active {
                let tint_color = colorref_mix(pal.accent, pal.key, 0.35);
                modifier_tint_brush = solid_brush(&self.d2d_context, colorref(tint_color))?;
                (&modifier_tint_brush, tint_color, &text_brush, &dim_brush)
            } else if action_key {
                (&action_brush, pal.key_action, &text_brush, &dim_brush)
            } else {
                (&key_brush, pal.key, &text_brush, &dim_brush)
            };
            let label_color = if selected { pal.sel_text } else { pal.text };
            self.d2d_context.FillRoundedRectangle(&rect, fill);

            let locked = (matches!(key.action, KeyAction::Shift) && modifiers.caps)
                || (matches!(key.action, KeyAction::Symbols) && modifiers.symbol_locked);
            if locked {
                let pill_w = ((key_rect.right - key_rect.left) * 0.28).min(24.0 * unit);
                let pill_h = (3.0 * unit).max(2.0);
                let cx = (key_rect.left + key_rect.right) * 0.5;
                let bottom = key_rect.bottom - 5.0 * unit;
                let pill_rect = D2D_RECT_F {
                    left: cx - pill_w * 0.5,
                    right: cx + pill_w * 0.5,
                    top: bottom - pill_h,
                    bottom,
                };
                let pill_color = if selected { pal.sel_text } else { pal.accent };
                let pill_brush = solid_brush(&self.d2d_context, colorref(pill_color))?;
                self.d2d_context
                    .FillRoundedRectangle(&rounded(pill_rect, pill_h * 0.5), &pill_brush);
            }

            let is_space = matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_SPACE);
            if let Some(sub) = key.sublabel.as_deref().filter(|_| !is_space) {
                self.d2d_context.DrawText(
                    &wide(sub),
                    &fonts.number,
                    &band_at(key_rect, key_rect.top + spec.number_cy * unit, key_h),
                    dim,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            let icon_px = spec.icon_px * unit;
            if matches!(key.action, KeyAction::VoiceInput) {
                // Tell the truth: dimmed mic-off when voice can't run here — on
                // Winlogon, or in userland with the optional whisper model not
                // installed. Otherwise a live mic, accent + a breathing halo while
                // it's actually listening.
                if !voice_available {
                    let disabled_color = colorref_mix(label_color, fill_color, 0.42);
                    self.draw_svg_icon_sized(VkIcon::MicOff, key_rect, disabled_color, 1.0, icon_px)?;
                } else if voice_active {
                    let cx = (rect.rect.left + rect.rect.right) * 0.5;
                    let cy = (rect.rect.top + rect.rect.bottom) * 0.5;
                    let side = (rect.rect.right - rect.rect.left)
                        .min(rect.rect.bottom - rect.rect.top)
                        .max(1.0);
                    let transcribing = matches!(voice_phase, VoicePhase::Transcribing);
                    self.d2d_context.PushAxisAlignedClip(
                        &rect.rect,
                        windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    );
                    // The cloud fills its square, so keep it inside the old blob's
                    // footprint instead of edge to edge on the key.
                    let ball = side * ORB_FRAME_FILL * orb_scale(voice_phase, voice_level, self.think_pulse());
                    let drew_nimbus = self.draw_nimbus_at(&square_about(cx, cy, ball), 1.0);
                    if !drew_nimbus {
                        self.draw_voice_orb(
                            pal.accent,
                            voice_level,
                            transcribing,
                            cx,
                            cy,
                            (side * 0.56).max(1.0),
                            1.25,
                        )?;
                    }
                    self.d2d_context.PopAxisAlignedClip();
                    let halo_alpha = match voice_phase {
                        VoicePhase::Starting => 0.18,
                        VoicePhase::Listening => 0.28 + 0.34 * voice_level.clamp(0.0, 1.0),
                        VoicePhase::Transcribing => {
                            let t = self.prompt_started.elapsed().as_secs_f32();
                            let pulse = 0.5 + 0.5 * (t * 2.4).sin();
                            0.28 + 0.42 * pulse
                        }
                    };
                    let halo =
                        solid_brush(&self.d2d_context, colorref_alpha(pal.accent, halo_alpha))?;
                    self.d2d_context
                        .DrawRoundedRectangle(&rect, &halo, 2.0, None);
                } else {
                    self.draw_svg_icon_sized(VkIcon::Mic, key_rect, label_color, 1.0, icon_px)?;
                }
            } else if is_space {
                let space_px = spec.space_icon_px * unit;
                if let Some(text) = spec.space_label {
                    self.d2d_context.DrawText(
                        &wide(text),
                        &fonts.space,
                        &key_rect,
                        label_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                } else if let Some(sub) = key.sublabel.as_deref() {
                    self.d2d_context.DrawText(
                        &wide(sub),
                        &fonts.number,
                        &band_at(key_rect, key_rect.top + spec.caption_cy * unit, key_h),
                        dim,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                    let icon_rect =
                        band_at(key_rect, key_rect.top + spec.space_icon_cy * unit, key_h);
                    self.draw_svg_icon_sized(VkIcon::Space, icon_rect, label_color, 1.0, space_px)?;
                } else {
                    self.draw_svg_icon_sized(VkIcon::Space, key_rect, label_color, 1.0, space_px)?;
                }
            } else if let Some((icon, large)) = key_icon(&key.action, modifiers.shift) {
                let px = if large {
                    spec.icon_large_px * unit
                } else {
                    icon_px
                };
                self.draw_svg_icon_sized(icon, key_rect, label_color, 1.0, px)?;
            } else {
                let (glyph, symbol_font) = key_glyph(key);
                let glyph = styled_glyph(spec, &key.action, glyph);
                if !glyph.is_empty() {
                    let format = if symbol_font {
                        &self.glyph_format
                    } else if glyph.chars().count() > 1 {
                        &fonts.word
                    } else {
                        &fonts.label
                    };
                    let label_rect = if key.sublabel.is_some() {
                        band_at(key_rect, key_rect.top + spec.label_cy * unit, key_h)
                    } else {
                        key_rect
                    };
                    self.d2d_context.DrawText(
                        &wide(&glyph),
                        format,
                        &label_rect,
                        label_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            if let Some(hint) = key_hint(key) {
                let badge_size = spec.hint_badge * unit;
                let inset = spec.hint_inset * unit;
                let badge = D2D_RECT_F {
                    left: kr.left + inset,
                    top: kr.top + inset,
                    right: kr.left + inset + badge_size,
                    bottom: kr.top + inset + badge_size,
                };
                if let Some(icon) = controller_icons.hint_icon(hint) {
                    self.draw_svg_icon(icon, badge, pal.text)?;
                } else {
                    let badge_brush = if selected {
                        &sel_text_brush
                    } else {
                        &accent_brush
                    };
                    self.d2d_context.DrawText(
                        &wide(hint),
                        &self.hint_format,
                        &badge,
                        badge_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
            if press.is_some() {
                self.d2d_context.SetTransform(&IDENTITY);
            }
        }

        // Gliding focus ring on top of the keys, at the eased position. When the
        // focused key is the one dipping, the ring dips with it so the whole
        // button presses as one piece.
        if let Some(ring) = self.anim_sel {
            let ring_w = (spec.sel_ring_w * unit).max(1.0);
            let rr = rounded(
                deflate(ring, ring_w * 0.5),
                (radius - ring_w * 0.5).max(0.0),
            );
            let ring_press = pressed
                .filter(|(p, _)| p.row == sel.row && p.col == sel.col)
                .map(|(_, s)| s);
            if let Some(s) = ring_press {
                self.d2d_context.SetTransform(&scale_about(
                    s,
                    (ring.left + ring.right) * 0.5,
                    (ring.top + ring.bottom) * 0.5,
                ));
            }
            self.d2d_context
                .DrawRoundedRectangle(&rr, &sel_ring_brush, ring_w, None);
            if ring_press.is_some() {
                self.d2d_context.SetTransform(&IDENTITY);
            }
        }

        if legend {
            let lu = ref_unit(ui_scale, style);
            let (left, right) = legend_span(&rects, cw, floating_pad(ui_scale, style).0);
            self.draw_legend(pal, style, controller_icons, lu, ch - BAR_H * lu, left, right, 1.0)?;
        }

        drop(key_brush);
        drop(action_brush);
        drop(accent_brush);
        drop(text_brush);
        drop(sel_text_brush);

        // Suggestion pill last, so it floats on top of the keys (and the ring).
        if let Some(strip) = candidates {
            let geom = StripGeom::above(&rects, spec, unit);
            self.d2d_context.SetTransform(&translate(0.0, strip_dy));
            let result = self.draw_strip(
                spec,
                pal,
                &fonts,
                strip,
                geom,
                strip_alpha.clamp(0.0, 1.0),
                controller_icons,
            );
            self.d2d_context.SetTransform(&IDENTITY);
            result?;
        }

        let sheet_dt = self
            .sheet_tick
            .map(|t| now.duration_since(t).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        self.sheet_tick = Some(now);
        self.sheet_alpha = shortcut_sheet::fade_step(self.sheet_alpha, sheet_on, sheet_dt);
        if self.sheet_alpha > 0.0 {
            let (panel, radius) = if floating {
                (
                    D2D_RECT_F {
                        left: FLOATING_PANEL_INSET,
                        top: FLOATING_PANEL_INSET,
                        right: cw - FLOATING_PANEL_INSET,
                        bottom: ch - FLOATING_PANEL_INSET,
                    },
                    Some(spec.panel_radius * unit),
                )
            } else {
                (
                    D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: cw,
                        bottom: ch,
                    },
                    None,
                )
            };
            self.draw_shortcut_sheet(
                pal,
                style,
                controller_icons,
                unit,
                panel,
                radius,
                self.sheet_alpha,
            )?;
        }

        if floating {
            self.d2d_context.PopAxisAlignedClip();
        }

        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        Ok(())
    }

    unsafe fn text_format(&self, style: VkStyle, px: f32) -> Result<IDWriteTextFormat, String> {
        let key = (style, (px * 4.0).round() as i32);
        if let Some(f) = self.text_formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let mut fonts: Option<IDWriteFontCollection> = None;
        self.dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let family = resolve_family(&fonts, style_spec(style).families);
        let f = self
            .dwrite
            .CreateTextFormat(
                &family,
                &fonts,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px.max(1.0),
                &user_locale_name(),
            )
            .map_err(|e| format!("CreateTextFormat ({family}): {e}"))?;
        let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        self.text_formats.borrow_mut().insert(key, f.clone());
        Ok(f)
    }

    unsafe fn measure_px(&self, style: VkStyle, text: &str, px: f32) -> f32 {
        self.text_format(style, px)
            .map(|f| self.measure_text(text, &f))
            .unwrap_or(0.0)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_text_px(
        &mut self,
        style: VkStyle,
        text: &str,
        px: f32,
        left: f32,
        band: D2D_RECT_F,
        color: u32,
        alpha: f32,
    ) -> Result<f32, String> {
        let format = self.text_format(style, px)?;
        let w = self.measure_text(text, &format);
        let brush = solid_brush(&self.d2d_context, colorref_alpha(color, alpha))?;
        self.d2d_context.DrawText(
            &wide(text),
            &format,
            &D2D_RECT_F {
                left,
                right: left + w,
                ..band
            },
            &brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        Ok(w)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_combo(
        &mut self,
        style: VkStyle,
        parts: &[ComboPart],
        cs: &ComboStyle,
        left: f32,
        band: D2D_RECT_F,
        text: u32,
        alpha: f32,
    ) -> Result<f32, String> {
        let mut x = left;
        for part in parts {
            let w = match *part {
                ComboPart::Icon(icon, share) => {
                    let w = if share { cs.share } else { cs.glyph };
                    let slot = D2D_RECT_F {
                        left: x,
                        right: x + w,
                        ..band
                    };
                    self.draw_svg_icon_sized(icon, slot, text, alpha, w)?;
                    w
                }
                ComboPart::Prefix(t) => {
                    self.draw_text_px(style, t, cs.prefix_px, x, band, text, ALPHA_SECONDARY * alpha)?
                }
                ComboPart::Connector(t) => self.draw_text_px(
                    style,
                    t,
                    cs.connector_px,
                    x,
                    band,
                    text,
                    ALPHA_SECONDARY * alpha,
                )?,
            };
            x += w + cs.gap;
        }
        Ok((x - cs.gap - left).max(0.0))
    }

    unsafe fn bar_layout_for(
        &self,
        style: VkStyle,
        family: ControllerIconFamily,
        u: f32,
        left: f32,
        right: f32,
    ) -> BarLayout {
        let measure = |t: &str, px: f32| self.measure_px(style, t, px);
        let widths: Vec<f32> = shortcut_sheet::LEGEND
            .iter()
            .map(|row| bar_item_width(row, family, u, &measure))
            .collect();
        bar_layout(&widths, bar_sep_width(u, &measure), left, right)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_legend(
        &mut self,
        pal: &VkPalette,
        style: VkStyle,
        family: ControllerIconFamily,
        u: f32,
        top: f32,
        left: f32,
        right: f32,
        alpha: f32,
    ) -> Result<(), String> {
        let layout = self.bar_layout_for(style, family, u, left, right);
        let band = D2D_RECT_F {
            left,
            top,
            right,
            bottom: top + BAR_H * u,
        };
        let cs = ComboStyle::bar(u);
        for (n, &(i, start, _)) in layout.items.iter().enumerate() {
            if n > 0 {
                let sep_left = start - layout.sep_w + BAR_GAP * u;
                self.draw_text_px(
                    style,
                    LEGEND_SEP,
                    text_px(BAR_SEP_PX, u),
                    sep_left,
                    band,
                    pal.text,
                    ALPHA_SEP * alpha,
                )?;
            }
            let row = &shortcut_sheet::LEGEND[i];
            let parts = combo_parts(row.keys, family);
            let w = self.draw_combo(style, &parts, &cs, start, band, pal.text, alpha)?;
            self.draw_text_px(
                style,
                row.label,
                text_px(BAR_LABEL_PX, u),
                start + w + BAR_LABEL_GAP * u,
                band,
                pal.text,
                alpha,
            )?;
        }
        Ok(())
    }

    unsafe fn sheet_fit(
        &self,
        style: VkStyle,
        family: ControllerIconFamily,
        unit: f32,
        panel: D2D_RECT_F,
    ) -> (SheetMetrics, Vec<SheetColumn>, f32) {
        let measure = |t: &str, px: f32| self.measure_px(style, t, px);
        let avail_w = (panel.right - panel.left).max(1.0);
        let avail_h = (panel.bottom - panel.top).max(1.0);
        let mut u = unit.max(1e-3);
        let mut m = SheetMetrics::at(u);
        let mut cols = sheet_columns(family, &m, &measure);
        let mut total = sheet_total_width(&cols, &m);
        for _ in 0..3 {
            let (w, h) = sheet_block_size(total, &m);
            let fit = (avail_w / w.max(1.0)).min(avail_h / h.max(1.0));
            if (fit - 1.0).abs() < 0.002 {
                break;
            }
            u = (u * fit).max(0.05);
            m = SheetMetrics::at(u);
            cols = sheet_columns(family, &m, &measure);
            total = sheet_total_width(&cols, &m);
        }
        while u > 0.05 {
            let (w, h) = sheet_block_size(total, &m);
            if w <= avail_w + 0.5 && h <= avail_h + 0.5 {
                break;
            }
            u *= 0.98;
            m = SheetMetrics::at(u);
            cols = sheet_columns(family, &m, &measure);
            total = sheet_total_width(&cols, &m);
        }
        (m, cols, total)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_shortcut_sheet(
        &mut self,
        pal: &VkPalette,
        style: VkStyle,
        family: ControllerIconFamily,
        unit: f32,
        panel: D2D_RECT_F,
        radius: Option<f32>,
        alpha: f32,
    ) -> Result<(), String> {
        let r = radius.unwrap_or(0.0);
        let fill = solid_brush(&self.d2d_context, colorref_alpha(pal.bg, alpha))?;
        self.d2d_context.FillRoundedRectangle(&rounded(panel, r), &fill);
        if radius.is_some() {
            let stroke = solid_brush(
                &self.d2d_context,
                colorref_alpha(pal.panel_stroke, pal.panel_stroke_alpha * alpha),
            )?;
            self.d2d_context.DrawRoundedRectangle(
                &rounded(deflate(panel, 0.5), (r - 0.5).max(0.0)),
                &stroke,
                1.0,
                None,
            );
        }

        let (m, cols, total) = self.sheet_fit(style, family, unit, panel);
        let lines = shortcut_sheet::max_lines() as f32;
        let block_h = (SHEET_PAD_TOP + SHEET_PAD_BOTTOM) * m.u + lines * m.row_h;
        let top = panel.top
            + ((panel.bottom - panel.top - block_h) * 0.5).max(0.0)
            + SHEET_PAD_TOP * m.u;
        let line_brush = solid_brush(&self.d2d_context, colorref_alpha(pal.text, ALPHA_LINE * alpha))?;
        let mut x = (panel.left + panel.right - total) * 0.5;
        let n = cols.len();
        for (ci, col) in cols.iter().enumerate() {
            for (i, line) in col.lines.iter().enumerate() {
                let band = D2D_RECT_F {
                    left: x,
                    top: top + i as f32 * m.row_h,
                    right: x + col.width,
                    bottom: top + (i as f32 + 1.0) * m.row_h,
                };
                match line {
                    Line::Title(t) => {
                        self.draw_text_px(
                            style,
                            t,
                            m.header_px,
                            x,
                            band,
                            pal.text,
                            ALPHA_SECONDARY * alpha,
                        )?;
                    }
                    Line::Row(row) => {
                        let parts = combo_parts(row.keys, family);
                        self.draw_combo(style, &parts, &m.cs, x, band, pal.text, alpha)?;
                        self.draw_text_px(
                            style,
                            row.label,
                            m.action_px,
                            x + col.combo_w + m.action_gap,
                            band,
                            pal.text,
                            alpha,
                        )?;
                    }
                    Line::Blank => {}
                }
                if shortcut_sheet::hairline_below(&col.lines, i) {
                    let y = (band.bottom - 1.0).round();
                    self.d2d_context.FillRectangle(
                        &D2D_RECT_F {
                            left: x.round(),
                            top: y,
                            right: (x + col.width).round(),
                            bottom: y + 1.0,
                        },
                        &line_brush,
                    );
                }
            }
            x += col.width + m.col_gap;
            if n >= 2 && ci == n - 2 {
                let dx = x.round();
                self.d2d_context.FillRectangle(
                    &D2D_RECT_F {
                        left: dx,
                        top: top.round(),
                        right: dx + 1.0,
                        bottom: (top + lines * m.row_h).round(),
                    },
                    &line_brush,
                );
                x += 1.0 + m.col_gap;
            }
        }
        Ok(())
    }

    unsafe fn draw_strip(
        &mut self,
        spec: &StyleSpec,
        pal: &VkPalette,
        fonts: &StyleFonts,
        strip: &crate::vk_predict::StripState,
        geom: StripGeom,
        alpha: f32,
        icons: ControllerIconFamily,
    ) -> Result<(), String> {
        if alpha <= 0.0 || geom.bottom - geom.top < 1.0 || strip.visible.iter().all(|w| w.is_empty())
        {
            return Ok(());
        }
        match spec.strip {
            StripLook::Pills => self.draw_strip_pills(spec, pal, fonts, strip, geom, alpha, icons),
            StripLook::Columns => {
                self.draw_strip_columns(spec, pal, fonts, strip, geom, alpha, icons)
            }
        }
    }

    unsafe fn draw_strip_pills(
        &mut self,
        spec: &StyleSpec,
        pal: &VkPalette,
        fonts: &StyleFonts,
        strip: &crate::vk_predict::StripState,
        geom: StripGeom,
        alpha: f32,
        icons: ControllerIconFamily,
    ) -> Result<(), String> {
        let u = geom.unit;
        let cy = geom.cy();
        let button_h = spec.strip_button_h * u;
        let hint_px = spec.strip_hint_px * u;
        let fill = solid_brush(&self.d2d_context, colorref_alpha(pal.key_action, alpha))?;
        let stroke = solid_brush(&self.d2d_context, colorref_alpha(pal.border, alpha))?;
        let frame = geom.pill_frame(spec);
        let mut buttons = vec![(frame.prev, if strip.engaged { "LB" } else { "SELECT" })];
        if strip.engaged {
            buttons.push((frame.next, "RB"));
        }
        for (rect, hint) in buttons {
            let shape = rounded(rect, button_h * 0.5);
            self.d2d_context.FillRoundedRectangle(&shape, &fill);
            self.d2d_context.DrawRoundedRectangle(
                &rounded(deflate(rect, 0.5), (button_h * 0.5 - 0.5).max(0.0)),
                &stroke,
                1.0,
                None,
            );
            if let Some(icon) = icons.hint_icon(hint) {
                let cx = (rect.left + rect.right) * 0.5;
                self.draw_svg_icon_alpha(icon, square_about(cx, cy, hint_px), pal.text, alpha)?;
            }
        }

        let chips = frame.chips;
        if chips.right - chips.left < 1.0 {
            return Ok(());
        }
        let chip_r = frame.chip_r;
        let chips_r = crate::vk_motion::concentric_radius(chip_r, spec.chips_pad * u);
        self.d2d_context
            .FillRoundedRectangle(&rounded(chips, chips_r), &fill);
        self.d2d_context.DrawRoundedRectangle(
            &rounded(deflate(chips, 0.5), (chips_r - 0.5).max(0.0)),
            &stroke,
            1.0,
            None,
        );
        let sel_fill = solid_brush(&self.d2d_context, colorref_alpha(pal.chip_sel, alpha))?;
        let text = solid_brush(&self.d2d_context, colorref_alpha(pal.text, alpha))?;
        let dim = solid_brush(
            &self.d2d_context,
            colorref_alpha(pal.text, spec.chip_text_alpha * alpha),
        )?;
        for (slot_index, slot) in geom.pill_slot_rects(spec, &strip.visible) {
            let word = &strip.visible[slot_index];
            let selected = strip.engaged && slot_index == strip.highlight_slot;
            if selected {
                self.d2d_context
                    .FillRoundedRectangle(&rounded(slot, chip_r), &sel_fill);
            }
            let label = D2D_RECT_F {
                left: slot.left + chip_r * 0.5,
                right: slot.right - chip_r * 0.5,
                ..slot
            };
            self.d2d_context.DrawText(
                &wide(word),
                if selected { &fonts.chip_sel } else { &fonts.chip },
                &label,
                if selected { &text } else { &dim },
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    unsafe fn draw_strip_columns(
        &mut self,
        spec: &StyleSpec,
        pal: &VkPalette,
        fonts: &StyleFonts,
        strip: &crate::vk_predict::StripState,
        geom: StripGeom,
        alpha: f32,
        icons: ControllerIconFamily,
    ) -> Result<(), String> {
        let u = geom.unit;
        let cy = geom.cy();
        let sep_h = spec.separator_h * u;
        let separator = solid_brush(
            &self.d2d_context,
            colorref_alpha(pal.border, spec.separator_alpha * alpha),
        )?;
        let text = solid_brush(&self.d2d_context, colorref_alpha(pal.text, alpha))?;
        let dim = solid_brush(
            &self.d2d_context,
            colorref_alpha(pal.text_dim, spec.chip_text_alpha * alpha),
        )?;
        let sel_fill = solid_brush(&self.d2d_context, colorref_alpha(pal.chip_sel, alpha))?;
        for (hint, rect) in geom.column_hints(spec, strip.engaged) {
            if let Some(icon) = icons.hint_icon(hint) {
                self.draw_svg_icon_sized(
                    icon,
                    rect,
                    pal.text_dim,
                    alpha,
                    rect.bottom - rect.top,
                )?;
            }
        }
        let columns = geom.column_rects(spec, strip.visible.len());
        let sel_h = (sep_h * 1.6).min(geom.bottom - geom.top);
        for (i, (word, col)) in strip.visible.iter().zip(columns).enumerate() {
            if i > 0 {
                let x = col.left.round();
                self.d2d_context.FillRectangle(
                    &D2D_RECT_F {
                        left: x - 0.5,
                        top: cy - sep_h * 0.5,
                        right: x + 0.5,
                        bottom: cy + sep_h * 0.5,
                    },
                    &separator,
                );
            }
            if word.is_empty() {
                continue;
            }
            let selected = strip.engaged && i == strip.highlight_slot;
            let pad = 8.0 * u;
            if selected {
                let pill = D2D_RECT_F {
                    left: col.left + pad * 0.5,
                    top: cy - sel_h * 0.5,
                    right: col.right - pad * 0.5,
                    bottom: cy + sel_h * 0.5,
                };
                self.d2d_context
                    .FillRoundedRectangle(&rounded(pill, sel_h * 0.5), &sel_fill);
            }
            self.d2d_context.DrawText(
                &wide(word),
                if selected { &fonts.chip_sel } else { &fonts.chip },
                &D2D_RECT_F {
                    left: col.left + pad,
                    right: col.right - pad,
                    ..col
                },
                if selected { &text } else { &dim },
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    /// Render a simple diagnostic panel through the full D3D11/D2D/DirectComposition
    /// path: cleared background, an accent test border (verifies fills + strokes), and
    /// left-aligned text lines. Used by the Winlogon debug overlay to confirm the
    /// composition pipeline works on the secure desktop.
    pub unsafe fn draw_debug(
        &mut self,
        bg: u32,
        accent: u32,
        lines: &[(u32, String)],
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        // Left-align text for the panel (the keyboard path centres it).
        let _ = self
            .text_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = self
            .text_format
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);

        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&colorref(bg)));

        let accent_brush = solid_brush(&self.d2d_context, colorref(accent))?;
        let border = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 4.0,
                top: 4.0,
                right: cw - 4.0,
                bottom: ch - 4.0,
            },
            radiusX: 8.0,
            radiusY: 8.0,
        };
        self.d2d_context
            .DrawRoundedRectangle(&border, &accent_brush, 2.0, None);

        let mut y = 12.0;
        for (color, line) in lines {
            let text_brush = solid_brush(&self.d2d_context, colorref(*color))?;
            let rect = D2D_RECT_F {
                left: 16.0,
                top: y,
                right: cw - 12.0,
                bottom: y + 26.0,
            };
            let wide: Vec<u16> = line.encode_utf16().collect();
            self.d2d_context.DrawText(
                &wide,
                &self.text_format,
                &rect,
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            y += 26.0;
        }

        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))?;
        Ok(())
    }

    /// Measure a text run's width in DIPs at the given format.
    unsafe fn measure_text(&self, text: &str, format: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout: Option<IDWriteTextLayout> = self
            .dwrite
            .CreateTextLayout(&wide, format, 1.0e6, 1.0e6)
            .ok();
        let Some(layout) = layout else { return 0.0 };
        let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        let mut m = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_err() {
            return 0.0;
        }
        m.widthIncludingTrailingWhitespace
    }

    /// Prompt pill ⇄ controller connection card on one fixed-size surface.
    /// `card_t`: 0 = "Press [L3] for keyboard" pill hugging the bottom band,
    /// 1 = AirPods-style card with the controller art and its name above.
    /// The window never moves or resizes during the morph; everything is laid
    /// out in canvas pixels so nothing drifts while the panel grows upward.
    pub unsafe fn draw_prompt_card(&mut self, p: &PromptCard) -> Result<(), String> {
        let PromptCard {
            bg,
            border,
            text_color,
            pill_border,
            pill_text,
            prefix,
            suffix,
            show_l3,
            title,
            controller_label,
            card_t,
            style: _,
        } = *p;
        if let Some(spec) = prompt_spec(p.style) {
            return self.draw_prompt_card_spec(p, spec);
        }
        let cw = self.width as f32;
        let ch = self.height as f32;
        let card_t = card_t.clamp(0.0, 1.0);
        // Pill text leaves first, then the panel grows, then title + art arrive:
        // the two text layers never sit on top of each other.
        let prompt_alpha = 1.0 - (card_t / 0.3).clamp(0.0, 1.0);
        let content_alpha = ((card_t - 0.35) / 0.65).clamp(0.0, 1.0);
        let content_alpha = content_alpha * content_alpha * (3.0 - 2.0 * content_alpha);
        // Frame colours cross from the pill's (possibly muted) set to the card's.
        let border = colorref_mix(border, pill_border, 1.0 - card_t);
        let identity = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };

        let t = self.prompt_started.elapsed().as_secs_f32();
        let pulse = (t * lerp(0.33, 0.72, card_t)).fract();
        let pulse_alpha = (1.0 - pulse).powi(2);
        // The idle pill breathes very slightly about its own centre; the card holds still.
        let breathe = ((t * std::f32::consts::TAU * 0.33).sin() * 0.5 + 0.5) * 0.015;
        let scale = 1.0 - (0.015 - breathe) * prompt_alpha;
        let pill_cy = ch - PROMPT_PILL_H * 0.5;
        self.d2d_context.SetTransform(&Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: cw * 0.5 * (1.0 - scale),
            M32: pill_cy * (1.0 - scale),
        });

        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        // Pill: bottom band minus a hairline for the antialiased stroke.
        let pill = D2D_RECT_F {
            left: 4.0,
            top: ch - PROMPT_PILL_H + 4.0,
            right: cw - 4.0,
            bottom: ch - 4.0,
        };
        // Card: narrow panel hugging the art, bottom edge shared with the pill so
        // the morph grows upward; the controller name floats above it.
        let card_w = (cw * 0.55).min(cw - 8.0);
        let card_top = ch * 0.21;
        let card = D2D_RECT_F {
            left: (cw - card_w) * 0.5,
            top: card_top,
            right: (cw + card_w) * 0.5,
            bottom: ch - 8.0,
        };
        let panel = lerp_rect(pill, card, card_t);
        let radius = PROMPT_PILL_H * 0.5 - 2.0;
        let rounded = D2D1_ROUNDED_RECT {
            rect: panel,
            radiusX: radius,
            radiusY: radius,
        };
        let glow = colorref_mix(0x00FFFFFF, border, lerp(0.38, 0.45, card_t));
        let bg_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(bg, lerp(1.0, 0.94, card_t)),
        )?;
        let halo_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(glow, lerp(0.30, 0.22, card_t) * pulse_alpha),
        )?;
        let border_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(glow, lerp(1.0, 0.84, card_t)),
        )?;
        self.d2d_context.FillRoundedRectangle(&rounded, &bg_brush);
        self.d2d_context.DrawRoundedRectangle(
            &rounded,
            &halo_brush,
            2.0 + lerp(8.0, 12.0, card_t) * pulse,
            None,
        );
        self.d2d_context.DrawRoundedRectangle(
            &rounded,
            &border_brush,
            lerp(1.5, 1.2, card_t),
            None,
        );

        if prompt_alpha > 0.01 {
            let _ = self
                .prompt_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = self
                .prompt_format
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let chip = (PROMPT_PILL_H * 0.70).clamp(26.0, 96.0);
            let gap = 12.0;
            let w_prefix = self.measure_text(prefix, &self.prompt_format);
            let w_suffix = self.measure_text(suffix, &self.prompt_format);
            let total = if show_l3 {
                w_prefix + gap + chip + gap + w_suffix
            } else {
                w_prefix
            };
            let mut x = ((cw - total) * 0.5).max(0.0);
            let band_top = ch - PROMPT_PILL_H;
            let text_brush =
                solid_brush(&self.d2d_context, colorref_alpha(pill_text, prompt_alpha))?;
            let pre: Vec<u16> = prefix.encode_utf16().collect();
            self.d2d_context.DrawText(
                &pre,
                &self.prompt_format,
                &D2D_RECT_F {
                    left: x,
                    top: band_top,
                    right: x + w_prefix,
                    bottom: ch,
                },
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            if show_l3 {
                x += w_prefix + gap;
                let chip_rect = D2D_RECT_F {
                    left: x,
                    top: pill_cy - chip * 0.5,
                    right: x + chip,
                    bottom: pill_cy + chip * 0.5,
                };
                let icon = ControllerIconFamily::from_label(controller_label).l3_icon();
                self.draw_svg_icon_alpha(icon, chip_rect, pill_text, prompt_alpha)?;
                x += chip + gap;
            }
            if !suffix.is_empty() {
                let suf: Vec<u16> = suffix.encode_utf16().collect();
                self.d2d_context.DrawText(
                    &suf,
                    &self.prompt_format,
                    &D2D_RECT_F {
                        left: x,
                        top: band_top,
                        right: x + w_suffix,
                        bottom: ch,
                    },
                    &text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }

        if content_alpha > 0.01 {
            let _ = self
                .text_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = self
                .text_format
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            // Single line above the card; the shared format wraps by default and
            // the short band would clip all but the first word.
            let _ = self
                .text_format
                .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let title_w: Vec<u16> = title.encode_utf16().collect();
            let text_brush =
                solid_brush(&self.d2d_context, colorref_alpha(text_color, content_alpha))?;
            // Name rises into place from just below its resting slot.
            let name_rise = 10.0 * (1.0 - content_alpha);
            self.d2d_context.DrawText(
                &title_w,
                &self.text_format,
                &D2D_RECT_F {
                    left: 0.0,
                    top: 16.0 + name_rise,
                    right: cw,
                    bottom: card_top - 12.0 + name_rise,
                },
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            let _ = self.text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);

            let image_cx = cw * 0.5;
            let image_cy = (card.top + card.bottom) * 0.5;
            let image_scale = 0.90 + 0.10 * content_alpha;
            let ring_brush = solid_brush(
                &self.d2d_context,
                colorref_alpha(glow, 0.20 * content_alpha * pulse_alpha),
            )?;
            let ring_hw = 148.0 * image_scale + 20.0 * pulse;
            let ring_hh = 120.0 * image_scale + 20.0 * pulse;
            self.d2d_context.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: image_cx - ring_hw,
                        top: image_cy - ring_hh,
                        right: image_cx + ring_hw,
                        bottom: image_cy + ring_hh,
                    },
                    radiusX: 104.0 * image_scale + 20.0 * pulse,
                    radiusY: 104.0 * image_scale + 20.0 * pulse,
                },
                &ring_brush,
                2.0,
                None,
            );
            let img_hw = 124.0 * image_scale;
            let img_hh = 108.0 * image_scale;
            let image_rect = D2D_RECT_F {
                left: image_cx - img_hw,
                top: image_cy - img_hh,
                right: image_cx + img_hw,
                bottom: image_cy + img_hh,
            };
            if let Some(art) = ControllerArt::from_label(controller_label) {
                self.draw_controller_art_alpha(art, image_rect, content_alpha)?;
            } else {
                self.draw_svg_icon_alpha(VkIcon::Gamepad, image_rect, text_color, content_alpha)?;
            }
        }

        self.d2d_context.SetTransform(&identity);
        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))?;
        Ok(())
    }

    unsafe fn tip_items(
        &self,
        tokens: &[TipToken],
        family: ControllerIconFamily,
        format: &IDWriteTextFormat,
        chip_px: f32,
        gap: f32,
    ) -> (Vec<(TipItem, f32)>, f32) {
        let items: Vec<(TipItem, f32)> = tokens
            .iter()
            .filter_map(|&token| tip_item(token, family))
            .map(|item| {
                let w = match item {
                    TipItem::Icon(_) => chip_px,
                    TipItem::Text(t, _) => self.measure_text(t, format),
                };
                (item, w)
            })
            .collect();
        let w = items.iter().map(|i| i.1).sum::<f32>()
            + gap * items.len().saturating_sub(1) as f32;
        (items, w)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_tip_row(
        &mut self,
        items: &[(TipItem, f32)],
        left: f32,
        band: D2D_RECT_F,
        gap: f32,
        format: &IDWriteTextFormat,
        text: u32,
        dim: u32,
        alpha: f32,
    ) -> Result<(), String> {
        if alpha <= 0.01 {
            return Ok(());
        }
        let text_brush = solid_brush(&self.d2d_context, colorref_alpha(text, alpha))?;
        let dim_brush = solid_brush(&self.d2d_context, colorref_alpha(dim, alpha * 0.6))?;
        let mut x = left;
        for (item, w) in items {
            let slot = D2D_RECT_F {
                left: x,
                top: band.top,
                right: x + w,
                bottom: band.bottom,
            };
            match *item {
                TipItem::Icon(icon) => self.draw_svg_icon_sized(icon, slot, text, alpha, *w)?,
                TipItem::Text(t, faint) => self.d2d_context.DrawText(
                    &wide(t),
                    format,
                    &slot,
                    if faint { &dim_brush } else { &text_brush },
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                ),
            }
            x += w + gap;
        }
        Ok(())
    }

    pub unsafe fn draw_tips(&mut self, p: &TipsPill) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        let spec = prompt_spec(p.style);
        let family = ControllerIconFamily::from_label(p.controller_label);
        let (label, counter_format) = match spec {
            Some(spec) => {
                let fonts = self.ensure_prompt_fonts(spec)?;
                (fonts.label, fonts.status)
            }
            None => {
                let _ = self
                    .prompt_format
                    .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                let _ = self
                    .prompt_format
                    .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                let _ = self
                    .text_format
                    .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                (self.prompt_format.clone(), self.text_format.clone())
            }
        };
        let (pill_h, pill_bottom, radius, chip, gap, pad_x) = match spec {
            Some(s) => (
                s.pill_h,
                s.pill_bottom,
                s.radius,
                s.chip_px,
                s.pill_gap,
                s.pill_pad_x,
            ),
            None => (96.0, 6.0, 46.0, 56.0, 12.0, 36.0),
        };
        let (text, dim) = match spec {
            Some(s) => (0x00FFFFFF, s.dim_text),
            None => (p.text, mix_color(p.text, p.bg, 0.58)),
        };

        let counter = format!("{}/{}", p.index + 1, p.count.max(1));
        let counter_w = self.measure_text(&counter, &counter_format);
        let counter_gap = gap * 2.0;
        let (items, content_w) = self.tip_items(p.cue, family, &label, chip, gap);
        let prev = p
            .prev_cue
            .map(|tokens| self.tip_items(tokens, family, &label, chip, gap));
        let morph_t = if prev.is_some() {
            p.morph_t.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let pill_w_for = |w: f32| (w + counter_gap + counter_w + pad_x * 2.0).min(cw - 8.0);
        let pill_w = match &prev {
            Some((_, prev_w)) => lerp(pill_w_for(*prev_w), pill_w_for(content_w), morph_t),
            None => pill_w_for(content_w),
        };
        let bottom = ch - pill_bottom;
        let pill = D2D_RECT_F {
            left: (cw - pill_w) * 0.5,
            top: bottom - pill_h,
            right: (cw + pill_w) * 0.5,
            bottom,
        };
        let pill_cy = bottom - pill_h * 0.5;
        let alpha = p.alpha.clamp(0.0, 1.0);

        self.d2d_context
            .SetTransform(&scale_about(p.scale, cw * 0.5, pill_cy));
        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let shape = rounded(pill, radius);
        match spec {
            Some(s) => {
                let (dy, blur, a) = s.pill_shadow;
                self.draw_outer_shadow(&shape, (dy, blur, a * alpha))?;
                let fill = solid_brush(
                    &self.d2d_context,
                    colorref_alpha(s.pill_fill, s.pill_fill_alpha * alpha),
                )?;
                self.d2d_context.FillRoundedRectangle(&shape, &fill);
                let stroke = solid_brush(
                    &self.d2d_context,
                    colorref_alpha(0x00FFFFFF, s.ready_stroke_alpha * alpha),
                )?;
                self.d2d_context.DrawRoundedRectangle(
                    &rounded(deflate(pill, 0.5), radius - 0.5),
                    &stroke,
                    1.0,
                    None,
                );
            }
            None => {
                let t = self.prompt_started.elapsed().as_secs_f32();
                let pulse = (t * 0.33).fract();
                let pulse_alpha = (1.0 - pulse).powi(2);
                let glow = colorref_mix(0x00FFFFFF, p.border, 0.38);
                let bg = solid_brush(&self.d2d_context, colorref_alpha(p.bg, alpha))?;
                let halo = solid_brush(
                    &self.d2d_context,
                    colorref_alpha(glow, 0.30 * pulse_alpha * alpha),
                )?;
                let edge = solid_brush(&self.d2d_context, colorref_alpha(glow, alpha))?;
                self.d2d_context.FillRoundedRectangle(&shape, &bg);
                self.d2d_context
                    .DrawRoundedRectangle(&shape, &halo, 2.0 + 8.0 * pulse, None);
                self.d2d_context.DrawRoundedRectangle(&shape, &edge, 1.5, None);
            }
        }

        let counter_left = pill.right - pad_x - counter_w;
        let row_right = counter_left - counter_gap;
        let row_left = pill.left + pad_x;
        let centred = |w: f32| row_left + ((row_right - row_left - w) * 0.5).max(0.0);
        let old_alpha = 1.0 - (morph_t / 0.3).clamp(0.0, 1.0);
        let new_alpha = ((morph_t - 0.35) / 0.65).clamp(0.0, 1.0);
        let new_alpha = if prev.is_some() {
            new_alpha * new_alpha * (3.0 - 2.0 * new_alpha)
        } else {
            1.0
        };
        if let Some((prev_items, prev_w)) = &prev {
            self.draw_tip_row(
                prev_items,
                centred(*prev_w),
                pill,
                gap,
                &label,
                text,
                dim,
                old_alpha * alpha,
            )?;
        }
        self.draw_tip_row(
            &items,
            centred(content_w),
            pill,
            gap,
            &label,
            text,
            dim,
            new_alpha * alpha,
        )?;

        let divider = solid_brush(&self.d2d_context, colorref_alpha(dim, 0.25 * alpha))?;
        let divider_x = counter_left - counter_gap * 0.5;
        self.d2d_context.DrawLine(
            D2D_POINT_2F {
                x: divider_x,
                y: pill_cy - pill_h * 0.22,
            },
            D2D_POINT_2F {
                x: divider_x,
                y: pill_cy + pill_h * 0.22,
            },
            &divider,
            1.0,
            None,
        );
        let counter_brush = solid_brush(&self.d2d_context, colorref_alpha(dim, 0.8 * alpha))?;
        self.d2d_context.DrawText(
            &wide(&counter),
            &counter_format,
            &D2D_RECT_F {
                left: counter_left,
                top: pill.top,
                right: counter_left + counter_w,
                bottom: pill.bottom,
            },
            &counter_brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        if spec.is_none() {
            let _ = self.text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
        }

        self.d2d_context.SetTransform(&IDENTITY);
        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))?;
        Ok(())
    }

    unsafe fn ensure_prompt_fonts(&mut self, spec: &PromptSpec) -> Result<PromptFonts, String> {
        if let Some(fonts) = &self.prompt_fonts {
            return Ok(fonts.clone());
        }
        let mut fonts: Option<IDWriteFontCollection> = None;
        self.dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let locale = user_locale_name();
        let family = resolve_family(&fonts, spec.families);
        let dwrite = &self.dwrite;
        let make = |weight: DWRITE_FONT_WEIGHT,
                    px: f32,
                    align: DWRITE_TEXT_ALIGNMENT|
         -> Result<IDWriteTextFormat, String> {
            let f = dwrite
                .CreateTextFormat(
                    &family,
                    &fonts,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    px,
                    &locale,
                )
                .map_err(|e| format!("CreateTextFormat ({family}): {e}"))?;
            let _ = f.SetTextAlignment(align);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            Ok(f)
        };
        let built = PromptFonts {
            label: make(
                DWRITE_FONT_WEIGHT_MEDIUM,
                spec.label_px,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?,
            title: make(
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                spec.title_px,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?,
            status: make(
                DWRITE_FONT_WEIGHT_MEDIUM,
                spec.status_px,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?,
        };
        self.prompt_fonts = Some(built.clone());
        Ok(built)
    }

    unsafe fn draw_outer_shadow(
        &self,
        shape: &D2D1_ROUNDED_RECT,
        (dy, blur, alpha): (f32, f32, f32),
    ) -> Result<(), String> {
        let resource: ID2D1Resource = self
            .d2d_context
            .cast()
            .map_err(|e| format!("d2d resource: {e}"))?;
        let factory: ID2D1Factory = resource
            .GetFactory()
            .map_err(|e| format!("d2d factory: {e}"))?;
        let reach = blur * 2.0 + dy.abs();
        let r = shape.rect;
        let outer = factory
            .CreateRectangleGeometry(&D2D_RECT_F {
                left: r.left - reach,
                top: r.top - reach,
                right: r.right + reach,
                bottom: r.bottom + reach,
            })
            .map_err(|e| format!("shadow rect: {e}"))?;
        let inner = factory
            .CreateRoundedRectangleGeometry(shape)
            .map_err(|e| format!("shadow shape: {e}"))?;
        let group = factory
            .CreateGeometryGroup(
                D2D1_FILL_MODE_ALTERNATE,
                &[
                    Some(outer.cast::<ID2D1Geometry>().map_err(|e| format!("{e}"))?),
                    Some(inner.cast::<ID2D1Geometry>().map_err(|e| format!("{e}"))?),
                ],
            )
            .map_err(|e| format!("shadow mask: {e}"))?;
        let mut params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: D2D_RECT_F {
                left: f32::MIN,
                top: f32::MIN,
                right: f32::MAX,
                bottom: f32::MAX,
            },
            geometricMask: ManuallyDrop::new(Some(
                group.cast::<ID2D1Geometry>().map_err(|e| format!("{e}"))?,
            )),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: IDENTITY,
            opacity: 1.0,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        self.d2d_context.PushLayer(&params, None::<&ID2D1Layer>);
        let drawn = draw_blur_shadow(&self.d2d_context, r, shape.radiusX, dy, blur, alpha);
        self.d2d_context.PopLayer();
        ManuallyDrop::drop(&mut params.geometricMask);
        drawn
    }

    unsafe fn draw_prompt_card_spec(
        &mut self,
        p: &PromptCard,
        spec: &PromptSpec,
    ) -> Result<(), String> {
        let fonts = self.ensure_prompt_fonts(spec)?;
        let cw = self.width as f32;
        let ch = self.height as f32;
        let card_t = p.card_t.clamp(0.0, 1.0);
        let prompt_alpha = 1.0 - (card_t / 0.3).clamp(0.0, 1.0);
        let content_alpha = ((card_t - 0.35) / 0.65).clamp(0.0, 1.0);
        let content_alpha = content_alpha * content_alpha * (3.0 - 2.0 * content_alpha);
        let t = self.prompt_started.elapsed().as_secs_f32();
        let breathe = ((t * std::f32::consts::TAU * 0.33).sin() * 0.5 + 0.5) * 0.015;
        let scale = 1.0 - (0.015 - breathe) * prompt_alpha;
        let white = 0x00FFFFFF;

        let (items, pill_text, pill_text_alpha, stroke_alpha) = if p.show_l3 {
            let chip = ControllerIconFamily::from_label(p.controller_label).l3_icon();
            (
                vec![
                    (None, p.prefix.trim(), 0.0),
                    (Some(chip), "", spec.chip_px),
                    (None, p.suffix.trim(), 0.0),
                ],
                white,
                1.0,
                spec.ready_stroke_alpha,
            )
        } else {
            (
                vec![
                    (Some(VkIcon::Gamepad), "", spec.gamepad_px),
                    (None, spec.no_pad_label, 0.0),
                ],
                spec.dim_text,
                spec.dim_alpha,
                spec.no_pad_stroke_alpha,
            )
        };
        let items: Vec<(Option<VkIcon>, &str, f32)> = items
            .into_iter()
            .filter(|(icon, text, _)| icon.is_some() || !text.is_empty())
            .map(|(icon, text, w)| match icon {
                Some(_) => (icon, text, w),
                None => (icon, text, self.measure_text(text, &fonts.label)),
            })
            .collect();
        let content_w = items.iter().map(|i| i.2).sum::<f32>()
            + spec.pill_gap * items.len().saturating_sub(1) as f32;
        let pill_w = (content_w + spec.pill_pad_x * 2.0).min(cw - 8.0);
        let pill_bottom = ch - spec.pill_bottom;
        let pill = D2D_RECT_F {
            left: (cw - pill_w) * 0.5,
            top: pill_bottom - spec.pill_h,
            right: (cw + pill_w) * 0.5,
            bottom: pill_bottom,
        };
        let pill_cy = pill_bottom - spec.pill_h * 0.5;
        let card_bottom = ch - spec.card_bottom;
        let card_w = spec.card_w.min(cw - 8.0);
        let card = D2D_RECT_F {
            left: (cw - card_w) * 0.5,
            top: (card_bottom - spec.card_h).max(0.0),
            right: (cw + card_w) * 0.5,
            bottom: card_bottom,
        };
        let panel = lerp_rect(pill, card, card_t);
        let shape = rounded(panel, spec.radius);
        let (pdy, pblur, palpha) = spec.pill_shadow;
        let (cdy, cblur, calpha) = spec.card_shadow;
        let shadow = (
            lerp(pdy, cdy, card_t),
            lerp(pblur, cblur, card_t),
            lerp(palpha, calpha, card_t),
        );
        let fill = colorref_alpha(
            colorref_mix(spec.card_fill, spec.pill_fill, card_t),
            lerp(spec.pill_fill_alpha, spec.card_fill_alpha, card_t),
        );
        let stroke = colorref_alpha(white, lerp(stroke_alpha, spec.card_stroke_alpha, card_t));

        self.d2d_context
            .SetTransform(&scale_about(scale, cw * 0.5, pill_cy));
        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        self.draw_outer_shadow(&shape, shadow)?;
        let fill_brush = solid_brush(&self.d2d_context, fill)?;
        self.d2d_context.FillRoundedRectangle(&shape, &fill_brush);
        let stroke_brush = solid_brush(&self.d2d_context, stroke)?;
        self.d2d_context.DrawRoundedRectangle(
            &rounded(deflate(panel, 0.5), spec.radius - 0.5),
            &stroke_brush,
            1.0,
            None,
        );

        if prompt_alpha > 0.01 {
            let text_brush = solid_brush(
                &self.d2d_context,
                colorref_alpha(pill_text, pill_text_alpha * prompt_alpha),
            )?;
            let mut x = (cw - content_w) * 0.5;
            for (icon, text, w) in items {
                let slot = D2D_RECT_F {
                    left: x,
                    top: pill.top,
                    right: x + w,
                    bottom: pill.bottom,
                };
                match icon {
                    Some(icon) => {
                        let opacity = if icon.is_controller_tip() {
                            prompt_alpha
                        } else {
                            pill_text_alpha * prompt_alpha
                        };
                        self.draw_svg_icon_sized(icon, slot, pill_text, opacity, w)?;
                    }
                    None => self.d2d_context.DrawText(
                        &wide(text),
                        &fonts.label,
                        &slot,
                        &text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    ),
                }
                x += w + spec.pill_gap;
            }
        }

        if content_alpha > 0.01 {
            let rise = 10.0 * (1.0 - content_alpha);
            let title_top = card.top + spec.card_pad_y;
            let title_brush = solid_brush(&self.d2d_context, colorref_alpha(white, content_alpha))?;
            self.d2d_context.DrawText(
                &wide(p.title),
                &fonts.title,
                &D2D_RECT_F {
                    left: card.left,
                    top: title_top + rise,
                    right: card.right,
                    bottom: title_top + spec.title_h + rise,
                },
                &title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            let stage_top = title_top + spec.title_h + spec.card_gap;
            let stage_cy = stage_top + spec.stage_h * 0.5;
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: colorref_alpha(white, spec.stage_glow_alpha),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: colorref_alpha(white, 0.0),
                },
            ];
            let collection = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.d2d_context,
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .map_err(|e| format!("stage gradient: {e}"))?;
            let center = D2D_POINT_2F {
                x: cw * 0.5,
                y: stage_cy,
            };
            let glow = self
                .d2d_context
                .CreateRadialGradientBrush(
                    &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center,
                        gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
                        radiusX: spec.stage_w * 0.5,
                        radiusY: spec.stage_h * 0.5,
                    },
                    None,
                    &collection,
                )
                .map_err(|e| format!("stage brush: {e}"))?;
            glow.SetOpacity(content_alpha);
            self.d2d_context.FillEllipse(
                &D2D1_ELLIPSE {
                    point: center,
                    radiusX: spec.stage_w * 0.5,
                    radiusY: spec.stage_h * 0.5,
                },
                &glow,
            );

            let image_scale = 0.90 + 0.10 * content_alpha;
            let art_rect = D2D_RECT_F {
                left: center.x - spec.art_w * 0.5 * image_scale,
                top: stage_cy - spec.art_h * 0.5 * image_scale,
                right: center.x + spec.art_w * 0.5 * image_scale,
                bottom: stage_cy + spec.art_h * 0.5 * image_scale,
            };
            if let Some(art) = ControllerArt::from_label(p.controller_label) {
                self.draw_controller_art_alpha(art, art_rect, content_alpha)?;
            } else {
                self.draw_svg_icon_alpha(VkIcon::Gamepad, art_rect, white, content_alpha)?;
            }

            let status_top = stage_top + spec.stage_h + spec.card_gap;
            let status_w = self.measure_text(spec.status_label, &fonts.status);
            let row_w = spec.check_px + spec.status_gap + status_w;
            let x = (cw - row_w) * 0.5;
            let row = |left: f32, w: f32| D2D_RECT_F {
                left,
                top: status_top,
                right: left + w,
                bottom: status_top + spec.status_h,
            };
            self.draw_svg_icon_sized(
                VkIcon::Check,
                row(x, spec.check_px),
                white,
                content_alpha,
                spec.check_px,
            )?;
            let status_brush = solid_brush(
                &self.d2d_context,
                colorref_alpha(spec.dim_text, spec.dim_alpha * content_alpha),
            )?;
            self.d2d_context.DrawText(
                &wide(spec.status_label),
                &fonts.status,
                &row(x + spec.check_px + spec.status_gap, status_w),
                &status_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }

        self.d2d_context.SetTransform(&IDENTITY);
        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))?;
        Ok(())
    }

    /// Compile (once) and draw the cloud into its bitmap.
    fn prepare_nimbus(&mut self, now: Instant, mood: NimbusMood, accent: u32) {
        if self.nimbus_failed {
            return;
        }
        if self.nimbus.is_none() {
            match unsafe { NimbusOrb::create(&self.d3d, &self.d2d_context) } {
                Ok(orb) => self.nimbus = Some(orb),
                Err(e) => {
                    self.fail_nimbus(&e);
                    return;
                }
            }
        }
        if let Some(orb) = self.nimbus.as_mut() {
            if let Err(e) = unsafe { orb.render(now, mood, accent) } {
                self.fail_nimbus(&e);
            }
        }
    }

    fn fail_nimbus(&mut self, err: &str) {
        self.nimbus = None;
        self.nimbus_failed = true;
        if crate::config::service_mode() {
            crate::install::log_line(&format!("vk renderer: nimbus orb unavailable: {err}"));
        }
    }

    /// Blit the latest cloud. False when the shader never came up, so the caller
    /// can fall back to the ellipse orb.
    fn think_pulse(&self) -> f32 {
        self.nimbus.as_ref().map(|orb| orb.think_pulse()).unwrap_or(0.0)
    }

    fn draw_nimbus_at(&self, dest: &D2D_RECT_F, opacity: f32) -> bool {
        let Some(orb) = self.nimbus.as_ref() else {
            return false;
        };
        if opacity <= 0.01 {
            return true;
        }
        unsafe {
            self.d2d_context.DrawBitmap(
                orb.bitmap(),
                Some(dest),
                opacity.clamp(0.0, 1.0),
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
        }
        true
    }

    /// Glow sitting on the screen edge. The outside of the band is the screen
    /// rectangle, so the corner wedges are filled. Only the inner edge has a
    /// small radius.
    unsafe fn draw_display_border(&self, accent: u32, alpha: f32) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        let scale = (ch / 1080.0).clamp(0.85, 2.0);
        let radius = 8.0 * scale;
        // How far the glow reaches in from the bezel, faintest first. Each band
        // starts at the screen edge, so the corners stay filled.
        let layers: [(f32, f32); 7] = [
            (22.0, 0.02),
            (14.0, 0.028),
            (9.0, 0.038),
            (5.5, 0.05),
            (3.2, 0.065),
            (1.8, 0.08),
            (1.0, 0.07),
        ];
        for (depth, layer_alpha) in layers {
            let brush = solid_brush(
                &self.d2d_context,
                colorref_alpha(accent, layer_alpha * alpha),
            )?;
            fill_edge_band(&self.d2d_context, cw, ch, depth * scale, radius, &brush)?;
        }
        Ok(())
    }

    /// Dictation pill for the keyboard-closed case: the audio-reactive orb on the
    /// left, a phase title beside it, and — while listening — the controller's
    /// R3 glyph with "Stop" so the exit is always on screen. Every phase has a
    /// static cue (title, hint, orb brightness); motion is never the only signal.
    ///
    /// Transcription replaces the pill: only the nimbus cloud (when `show_orb`)
    /// and a border around the whole window, which the overlay sizes to the
    /// display. The border is the static cue; the cloud is the motion.
    pub unsafe fn draw_voice(&mut self, pill: &VoicePill) -> Result<(), String> {
        let VoicePill {
            bg,
            border: _,
            accent,
            text,
            level,
            phase,
            controller_label,
            alpha,
            scale,
            label_alpha,
            show_orb,
        } = *pill;
        let cw = self.width as f32;
        let ch = self.height as f32;
        let now = Instant::now();
        let alpha = alpha.clamp(0.0, 1.0);
        let label_alpha = label_alpha.clamp(0.0, 1.0) * alpha;
        let level = self.smoothed_level(level, now);
        let _ = (bg, text, controller_label, scale, label_alpha);
        // Same frame and the same cloud size. Talking speeds up with the mic;
        // transcription keeps a steady orbit. The palette does not change.
        if show_orb {
            self.prepare_nimbus(now, nimbus_mood(phase, level), accent);
        }

        {
            self.d2d_context.BeginDraw();
            self.d2d_context.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            self.d2d_context.SetTransform(&IDENTITY);
            let border_alpha = if matches!(phase, VoicePhase::Transcribing) {
                let t = self.prompt_started.elapsed().as_secs_f32();
                let pulse = 0.5 + 0.5 * (t * 2.4).sin();
                alpha * (0.8 + 0.7 * pulse)
            } else {
                alpha
            };
            self.draw_display_border(accent, border_alpha.min(1.0))?;
            if show_orb {
                let slot = scale_about_center(
                    nimbus_slot(cw, ch),
                    ORB_FRAME_FILL * orb_scale(phase, level, self.think_pulse()),
                );
                if !self.draw_nimbus_at(&slot, alpha) {
                    let unit = ((slot.right - slot.left) * 0.42).max(1.0);
                    self.draw_voice_orb(
                        accent,
                        0.0,
                        true,
                        (slot.left + slot.right) * 0.5,
                        (slot.top + slot.bottom) * 0.5,
                        unit,
                        alpha,
                    )?;
                }
            }
            self.d2d_context
                .EndDraw(None, None)
                .map_err(|e| format!("EndDraw: {e}"))?;
            self.swapchain
                .Present(1, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("Present: {e}"))?;
            Ok(())
        }
    }
}

/// Height of the "Press [L3]" pill band at the bottom of the prompt canvas.
pub const PROMPT_PILL_H: f32 = 116.0;

/// Inputs for [`VkRenderer::draw_prompt_card`].
#[derive(Clone, Copy)]
pub struct PromptCard<'a> {
    pub bg: u32,
    /// Card border / glow and title colour (always the full theme colours).
    pub border: u32,
    pub text_color: u32,
    /// Pill border / text; the muted "no pad" look passes dimmed colours here.
    pub pill_border: u32,
    pub pill_text: u32,
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub show_l3: bool,
    pub title: &'a str,
    pub controller_label: &'a str,
    /// 0 = prompt pill, 1 = connection card.
    pub card_t: f32,
    pub style: VkStyle,
}

#[derive(Clone, Copy)]
pub struct TipsPill<'a> {
    pub bg: u32,
    pub border: u32,
    pub text: u32,
    pub controller_label: &'a str,
    pub cue: &'a [TipToken],
    pub prev_cue: Option<&'a [TipToken]>,
    pub morph_t: f32,
    pub index: usize,
    pub count: usize,
    pub alpha: f32,
    pub scale: f32,
    pub style: VkStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TipItem {
    Icon(VkIcon),
    Text(&'static str, bool),
}

/// Everything one frame of the dictation pill needs. `alpha`/`scale` carry the
/// enter/exit transition; `label_alpha` fades a freshly swapped phase title in.
pub struct VoicePill<'a> {
    pub bg: u32,
    pub border: u32,
    pub accent: u32,
    pub text: u32,
    /// Raw published mic level (0..1); the renderer applies the envelope.
    pub level: f32,
    pub phase: VoicePhase,
    pub controller_label: &'a str,
    pub alpha: f32,
    pub scale: f32,
    pub label_alpha: f32,
    /// Transcription only. False when the keyboard's mic key is already the
    /// cloud, so the fullscreen overlay draws just the display border.
    pub show_orb: bool,
}

/// Screen rectangle with a rounded-rect hole. The hole's corner radius fills
/// nothing at the bezel: the outer path is square, so the corner wedge is paint.
unsafe fn fill_edge_band(
    ctx: &ID2D1DeviceContext,
    cw: f32,
    ch: f32,
    depth: f32,
    radius: f32,
    brush: &ID2D1SolidColorBrush,
) -> Result<(), String> {
    let depth = depth.max(1.0);
    let radius = radius.min(depth).min(cw * 0.5).min(ch * 0.5).max(0.0);
    let resource: ID2D1Resource = ctx
        .cast()
        .map_err(|e| format!("d2d resource: {e}"))?;
    let factory: ID2D1Factory = resource
        .GetFactory()
        .map_err(|e| format!("d2d factory: {e}"))?;
    let geometry = factory
        .CreatePathGeometry()
        .map_err(|e| format!("path: {e}"))?;
    let sink = geometry.Open().map_err(|e| format!("path open: {e}"))?;
    sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
    let p = |x: f32, y: f32| D2D_POINT_2F { x, y };
    sink.BeginFigure(p(0.0, 0.0), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLine(p(cw, 0.0));
    sink.AddLine(p(cw, ch));
    sink.AddLine(p(0.0, ch));
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);

    let left = depth;
    let top = depth;
    let right = (cw - depth).max(left + 1.0);
    let bottom = (ch - depth).max(top + 1.0);
    let r = radius;
    sink.BeginFigure(p(left + r, top), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLine(p(right - r, top));
    let arc = |x: f32, y: f32| D2D1_ARC_SEGMENT {
        point: p(x, y),
        size: D2D_SIZE_F {
            width: r,
            height: r,
        },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    };
    sink.AddArc(&arc(right, top + r));
    sink.AddLine(p(right, bottom - r));
    sink.AddArc(&arc(right - r, bottom));
    sink.AddLine(p(left + r, bottom));
    sink.AddArc(&arc(left, bottom - r));
    sink.AddLine(p(left, top + r));
    sink.AddArc(&arc(left + r, top));
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close().map_err(|e| format!("path close: {e}"))?;
    ctx.FillGeometry(&geometry, brush, None);
    Ok(())
}

fn nimbus_mood(phase: VoicePhase, level: f32) -> NimbusMood {
    match phase {
        VoicePhase::Starting => NimbusMood::Idle,
        VoicePhase::Listening => NimbusMood::Speaking { level },
        VoicePhase::Transcribing => NimbusMood::Thinking,
    }
}

fn orb_scale(phase: VoicePhase, level: f32, think_pulse: f32) -> f32 {
    drive_scale(mood_drive(&nimbus_mood(phase, level), think_pulse))
}

fn scale_about_center(rect: D2D_RECT_F, scale: f32) -> D2D_RECT_F {
    let cx = (rect.left + rect.right) * 0.5;
    let cy = (rect.top + rect.bottom) * 0.5;
    let hw = (rect.right - rect.left) * 0.5 * scale;
    let hh = (rect.bottom - rect.top) * 0.5 * scale;
    D2D_RECT_F {
        left: cx - hw,
        top: cy - hh,
        right: cx + hw,
        bottom: cy + hh,
    }
}

fn square_about(cx: f32, cy: f32, side: f32) -> D2D_RECT_F {
    let h = side * 0.5;
    D2D_RECT_F {
        left: cx - h,
        top: cy - h,
        right: cx + h,
        bottom: cy + h,
    }
}

/// Square for the transcription cloud: right edge, vertically centred. The
/// cloud fills this square, so 96px at 1080p matches the old pill indicator
/// instead of the 240px first cut.
fn nimbus_slot(cw: f32, ch: f32) -> D2D_RECT_F {
    let scale = (ch / 1080.0).clamp(0.85, 2.0);
    let size = (96.0 * scale).min(cw.min(ch) * 0.9).max(1.0);
    let margin = (48.0 * scale).min((cw - size).max(0.0));
    let left = (cw - margin - size).max(0.0);
    let top = ((ch - size) * 0.5).max(0.0);
    D2D_RECT_F {
        left,
        top,
        right: left + size,
        bottom: top + size,
    }
}

/// Create the D3D11 device, preferring WARP (software) on the secure desktop to
/// dodge the NVIDIA UMD crash, hardware otherwise. Falls back to the other driver
/// type if the preferred one fails to create.
unsafe fn create_d3d_device(prefer_warp: bool) -> Result<ID3D11Device, String> {
    let order: [D3D_DRIVER_TYPE; 2] = if prefer_warp {
        [D3D_DRIVER_TYPE_WARP, D3D_DRIVER_TYPE_HARDWARE]
    } else {
        [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP]
    };
    let feature_levels = [D3D_FEATURE_LEVEL_11_0];
    let mut last = String::from("no driver attempted");
    for driver in order {
        let mut d3d: Option<ID3D11Device> = None;
        match D3D11CreateDevice(
            None,
            driver,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut d3d as *mut _),
            None,
            None,
        ) {
            Ok(()) => {
                if let Some(d) = d3d {
                    let kind = if driver == D3D_DRIVER_TYPE_WARP {
                        "WARP (software)"
                    } else {
                        "hardware"
                    };
                    if crate::config::service_mode() {
                        crate::install::log_line(&format!("vk renderer: D3D11 device = {kind}"));
                    }
                    return Ok(d);
                }
                last = "D3D11CreateDevice returned null".to_string();
            }
            Err(e) => last = format!("{e}"),
        }
    }
    Err(format!("D3D11CreateDevice (all driver types): {last}"))
}

unsafe fn bind_d2d_target(
    ctx: &ID2D1DeviceContext,
    swapchain: &IDXGISwapChain1,
) -> Result<ID2D1Bitmap1, String> {
    let surface: IDXGISurface = swapchain
        .GetBuffer(0)
        .map_err(|e| format!("GetBuffer: {e}"))?;
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        ..Default::default()
    };
    let bitmap = ctx
        .CreateBitmapFromDxgiSurface(&surface, Some(&props))
        .map_err(|e| format!("CreateBitmapFromDxgiSurface: {e}"))?;
    ctx.SetTarget(&bitmap);
    Ok(bitmap)
}

unsafe fn solid_brush(
    ctx: &ID2D1DeviceContext,
    color: D2D1_COLOR_F,
) -> Result<ID2D1SolidColorBrush, String> {
    ctx.CreateSolidColorBrush(&color, None)
        .map_err(|e| format!("CreateSolidColorBrush: {e}"))
}

unsafe fn resolve_family(
    fonts: &IDWriteFontCollection,
    families: &[&str],
) -> windows::core::HSTRING {
    for name in families {
        let family = windows::core::HSTRING::from(*name);
        let mut index = 0u32;
        let mut exists = BOOL(0);
        if fonts.FindFamilyName(&family, &mut index, &mut exists).is_ok() && exists.as_bool() {
            return family;
        }
    }
    windows::core::HSTRING::from("Segoe UI")
}

unsafe fn create_dwrite() -> Result<IDWriteFactory, String> {
    DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| format!("DWriteCreateFactory: {e}"))
}

/// `CreateTextFormat` rejects a null locale on some builds; use the user default.
fn user_locale_name() -> windows::core::HSTRING {
    let mut buf = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if len > 1 {
        let end = (len - 1) as usize;
        String::from_utf16_lossy(&buf[..end])
    } else {
        "en-US".to_string()
    }
    .into()
}

fn key_metrics(
    scale_w: f32,
    client_h: f32,
    rows: &[KeyRow],
    top_inset: f32,
    style: VkStyle,
) -> (f32, f32, f32) {
    let spec = style_spec(style);
    let scale = (scale_w / REF_MON_W).max(0.05);
    let mut kw = REF_KEY_W * scale;
    let mut kh = kw * spec.key_aspect;
    let mut gap = kh * spec.gap / spec.design_kh;
    let n = rows.len().max(1) as f32;
    // Fit below top chrome (chips when active); shrink if rows overflow.
    let avail = (client_h - top_inset - kh * 0.25).max(1.0);
    let block = n * kh + (n - 1.0) * gap;
    if block > avail {
        let s = avail / block;
        kh *= s;
        gap *= s;
        kw = kh / spec.key_aspect;
    }
    (kw, kh, gap)
}

#[cfg(test)]
pub const FULL_SIZE_SCALE: f32 = 1.0 / crate::config::COMPACT_BAR_SCALE;

#[cfg(test)]
impl VkRenderer {
    pub unsafe fn offscreen(width: u32, height: u32) -> Result<Self, String> {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        Self::build(width.max(1), height.max(1), None, true)
    }

    pub unsafe fn render_bgra(&mut self, frame: &VkFrame) -> Result<(u32, u32, Vec<u8>), String> {
        if frame.shortcut_sheet {
            self.sheet_alpha = 1.0;
        }
        if frame.candidates.is_some() {
            self.strip_shown_at = Instant::now().checked_sub(std::time::Duration::from_secs(2));
        }
        self.draw_frame(frame)?;
        self.readback()
    }

    pub unsafe fn render_design(
        &mut self,
        style: VkStyle,
        sheet: bool,
    ) -> Result<(u32, u32, Vec<u8>), String> {
        let pal = style_palette(style, true);
        let family = ControllerIconFamily::Ps5;
        let (w, h) = (self.width as f32, self.height as f32);
        self.d2d_context.BeginDraw();
        self.d2d_context.SetTransform(&IDENTITY);
        self.d2d_context.Clear(Some(&colorref(pal.bg)));
        let drawn = if sheet {
            let panel = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w,
                bottom: h,
            };
            self.draw_shortcut_sheet(&pal, style, family, 1.0, panel, None, 1.0)
        } else {
            self.draw_legend(&pal, style, family, 1.0, 0.0, 0.0, w, 1.0)
        };
        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        drawn?;
        self.readback()
    }

    unsafe fn readback(&self) -> Result<(u32, u32, Vec<u8>), String> {
        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE,
            D3D11_MAP_READ, D3D11_RESOURCE_MISC_FLAG, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
        };
        let tex: ID3D11Texture2D = self
            .swapchain
            .GetBuffer(0)
            .map_err(|e| format!("GetBuffer: {e}"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.BindFlags = D3D11_BIND_FLAG(0).0 as u32;
        desc.MiscFlags = D3D11_RESOURCE_MISC_FLAG(0).0 as u32;
        let mut staging = None;
        self.d3d
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .map_err(|e| format!("staging: {e}"))?;
        let staging = staging.ok_or("staging null")?;
        let ctx = self
            .d3d
            .GetImmediateContext()
            .map_err(|e| format!("context: {e}"))?;
        ctx.CopyResource(&staging, &tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("map: {e}"))?;
        let (w, h) = (desc.Width, desc.Height);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let row = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
            std::ptr::copy_nonoverlapping(row, px.as_mut_ptr().add(y * w as usize * 4), w as usize * 4);
        }
        ctx.Unmap(&staging, 0);
        Ok((w, h, px))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &D2D_RECT_F, b: &D2D_RECT_F) -> bool {
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    #[test]
    fn legend_and_sheet_follow_display_scaling() {
        for style in [VkStyle::Normal, VkStyle::Mono] {
            for dpi in [1.25_f32, 1.5, 2.0] {
                let base = legend_band_height(1.0, style);
                assert!((legend_band_height(dpi, style) - base * dpi).abs() < 0.01);
                let (u1, u2) = (ref_unit(1.0, style), ref_unit(dpi, style));
                assert!((u2 - u1 * dpi).abs() < 1e-4);
                let (b1, b2) = (ComboStyle::bar(u1), ComboStyle::bar(u2));
                assert!((b2.glyph - b1.glyph * dpi).abs() < 0.01);
                let (s1, s2) = (SheetMetrics::at(u1), SheetMetrics::at(u2));
                assert!((s2.row_h - s1.row_h * dpi).abs() < 0.01);
                assert!((s2.header_px - s1.header_px * dpi).abs() < 0.01);
                assert!((s2.action_px - s1.action_px * dpi).abs() < 0.01);
            }
        }
    }

    #[test]
    fn mono_columns_never_overlap_the_hint_glyphs() {
        let spec = style_spec(VkStyle::Mono);
        for unit in [0.5_f32, 1.0, 1.75, 3.0] {
            let geom = StripGeom {
                left: 20.0,
                top: 10.0,
                right: 20.0 + 900.0 * unit,
                bottom: 10.0 + spec.strip_bar_h * unit,
                unit,
            };
            let cols = geom.column_rects(spec, 3);
            let w0 = cols[0].right - cols[0].left;
            assert!(cols.iter().all(|c| ((c.right - c.left) - w0).abs() < 1e-3));
            for engaged in [false, true] {
                let hints = geom.column_hints(spec, engaged);
                assert_eq!(hints.len(), if engaged { 2 } else { 1 });
                for (_, h) in &hints {
                    assert!(h.right - h.left > 0.0);
                    assert!(h.left >= geom.left && h.right <= geom.right);
                    for c in &cols {
                        assert!(!overlaps(h, c), "unit {unit} engaged {engaged}");
                    }
                }
            }
            let visible = vec!["a".to_string(), "b".to_string(), String::new()];
            let hit = geom.slot_rects(spec, &visible);
            assert_eq!(hit.len(), 2);
            assert_eq!(hit[0].1, cols[0]);
            assert_eq!(hit[1].1, cols[1]);
        }
    }

    fn bar_check(scale: f32, style: VkStyle, client_w: f32, family: ControllerIconFamily) -> (Vec<&'static str>, BarLayout, f32, f32) {
        let rows = crate::vk_nav::rows_for_test();
        let (grid_w, block_h) = grid_size(REF_MON_W * scale, &rows, style);
        let pad_x = floating_pad(scale, style).0;
        let cw = client_w.min(grid_w + 2.0 * pad_x);
        let rects = key_rects(cw, block_h + 200.0, REF_MON_W * scale, &rows, 0.0, style);
        let (l, r) = legend_span(&rects, cw, pad_x);
        unsafe {
            let rd = VkRenderer::offscreen(cw as u32, 100).expect("offscreen renderer");
            let layout = rd.bar_layout_for(style, family, ref_unit(scale, style), l, r);
            let labels = layout
                .items
                .iter()
                .map(|&(i, _, _)| shortcut_sheet::LEGEND[i].label)
                .collect();
            (labels, layout, l, r)
        }
    }

    fn assert_bar_clear(layout: &BarLayout, l: f32, r: f32, what: &str) {
        for w in layout.items.windows(2) {
            let (_, a, aw) = w[0];
            let (_, b, _) = w[1];
            assert!((b - (a + aw + layout.sep_w)).abs() < 0.01, "{what}: {:?}", layout.items);
            assert!(b > a + aw, "{what}: overlap {:?}", layout.items);
        }
        if layout.items.len() > 1 {
            let (_, first, _) = layout.items[0];
            let &(_, last, lw) = layout.items.last().unwrap();
            assert!(first >= l - 0.01 && last + lw <= r + 0.01, "{what}: outside {l}..{r}");
            let mid = (first + last + lw) * 0.5;
            assert!((mid - (l + r) * 0.5).abs() < 0.5, "{what}: not centred");
        }
    }

    #[test]
    fn bar_fits_whole_at_full_size_and_drops_the_middle_when_narrow() {
        let all: Vec<&str> = shortcut_sheet::LEGEND.iter().map(|r| r.label).collect();
        for family in [ControllerIconFamily::Ps5, ControllerIconFamily::Xbox] {
            for style in [VkStyle::Normal, VkStyle::Mono] {
                let what = format!("{style:?} {family:?}");
                let (labels, layout, l, r) = bar_check(FULL_SIZE_SCALE, style, 1920.0, family);
                assert_eq!(labels, all, "{what}");
                assert_bar_clear(&layout, l, r, &what);
                let (_, first, _) = layout.items[0];
                assert!(first > l + 1.0, "{what}: the bar is a compact centred group");
                for (scale, width, must_drop) in [(1.0, 1920.0, false), (FULL_SIZE_SCALE, 900.0, true)] {
                    let (kept, layout, l, r) = bar_check(scale, style, width, family);
                    assert_bar_clear(&layout, l, r, &what);
                    if !must_drop && kept == all {
                        continue;
                    }
                    assert!(kept.len() < all.len(), "{what} {kept:?}");
                    assert_eq!(kept.first(), Some(&"Type"), "{what}");
                    assert_eq!(kept.last(), Some(&"All shortcuts"), "{what}");
                }
            }
        }
    }

    #[test]
    fn bar_layout_centres_one_compact_group() {
        let layout = bar_layout(&[10.0, 20.0, 30.0], 5.0, 0.0, 200.0);
        assert_eq!(layout.items, vec![(0, 65.0, 10.0), (1, 80.0, 20.0), (2, 105.0, 30.0)]);
        let tight = bar_layout(&[10.0, 20.0, 30.0], 5.0, 0.0, 50.0);
        assert_eq!(tight.items.iter().map(|i| i.0).collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn design_sizes_scale_with_the_unit_and_keep_the_text_floor() {
        let bar = ComboStyle::bar(1.0);
        assert_eq!((bar.glyph, bar.share, bar.gap), (38.0, 44.0, 6.0));
        assert_eq!((bar.prefix_px, bar.connector_px), (20.0, 17.0));
        assert_eq!((text_px(BAR_LABEL_PX, 1.0), text_px(BAR_SEP_PX, 1.0), BAR_H), (19.0, 18.0, 80.0));
        let sheet = SheetMetrics::at(1.0);
        assert_eq!((sheet.cs.glyph, sheet.cs.share, sheet.cs.gap), (28.0, 32.0, 8.0));
        assert_eq!((sheet.header_px, sheet.action_px, sheet.cs.prefix_px), (18.0, 19.0, 17.0));
        assert_eq!((sheet.row_h, sheet.col_gap, sheet.action_gap), (70.0, 40.0, 24.0));
        for u in [0.3, 0.667, 1.0, 1.5] {
            assert!(ComboStyle::bar(u).connector_px >= LEGEND_MIN_TEXT_PX);
            assert!(SheetMetrics::at(u).header_px >= LEGEND_MIN_TEXT_PX);
        }
        for style in [VkStyle::Normal, VkStyle::Mono] {
            let u = ref_unit(1.0, style);
            let h = legend_band_height(1.0, style) + style_spec(style).pad_y * u;
            assert!((h - BAR_H * u).abs() < 1e-3);
        }
    }

    #[test]
    fn sheet_columns_match_the_design_and_never_clip() {
        let family = ControllerIconFamily::Ps5;
        unsafe {
            let rd = VkRenderer::offscreen(1600, 546).expect("offscreen");
            let measure = |t: &str, px: f32| rd.measure_px(VkStyle::Normal, t, px);
            let m = SheetMetrics::at(1.0);
            let cols = sheet_columns(family, &m, &measure);
            let combos: Vec<f32> = cols.iter().map(|c| c.combo_w).collect();
            for (got, want) in combos.iter().zip([86.0, 183.0, 140.0]) {
                assert!((got - want).abs() <= 8.0, "combo widths {combos:?}");
            }
            let total = sheet_total_width(&cols, &m);
            assert!(total <= 1600.0 - 2.0 * SHEET_PAD_X, "{total}");
            for col in &cols {
                for line in &col.lines {
                    if let Line::Row(row) = line {
                        let w = combo_width(&combo_parts(row.keys, family), &m.cs, &measure);
                        assert!(w <= col.combo_w + 0.01);
                        let label = measure(row.label, m.action_px);
                        assert!(col.combo_w + m.action_gap + label <= col.width + 0.01);
                    }
                }
            }
            for style in [VkStyle::Normal, VkStyle::Mono] {
                let rows = crate::vk_nav::rows_for_test();
                let (grid_w, block_h) = grid_size(REF_MON_W, &rows, style);
                let (pad_x, pad_y) = floating_pad(1.0, style);
                let panel = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: grid_w + 2.0 * pad_x,
                    bottom: strip_band_height(1.0, style) + block_h + 2.0 * pad_y + legend_band_height(1.0, style),
                };
                let (m, cols, total) = rd.sheet_fit(style, family, ref_unit(1.0, style), panel);
                let (bw, bh) = sheet_block_size(total, &m);
                assert!(bw <= panel.right + 1.0 && bh <= panel.bottom + 1.0, "{style:?} {bw}x{bh}");
                let fill = (bw / panel.right).max(bh / panel.bottom);
                assert!(fill >= 0.99, "{style:?} fills only {fill}");
                let lines = shortcut_sheet::max_lines() as f32;
                let content = (total / panel.right).max(lines * m.row_h / panel.bottom);
                assert!(content >= 0.92, "{style:?} content fills only {content}");
                assert!(m.u >= ref_unit(1.0, style) * 1.1, "{style:?} scale {}", m.u);
                assert!(m.u > ref_unit(1.0, style), "{style:?} sheet grows past the design size");
                assert_eq!(cols.len(), 3);
                let (m2, _, total2) = rd.sheet_fit(
                    style,
                    family,
                    ref_unit(1.5, style),
                    D2D_RECT_F { right: panel.right * 1.5, bottom: panel.bottom * 1.5, ..panel },
                );
                assert!((m2.u - m.u * 1.5).abs() < m.u * 0.05, "{style:?} {} vs {}", m2.u, m.u);
                assert!((total2 - total * 1.5).abs() < total * 0.05);
            }
        }
    }

    #[test]
    fn legend_uses_the_pad_family_svg_glyphs() {
        let ps = ControllerIconFamily::from_label("DualSense Wireless Controller");
        let xb = ControllerIconFamily::from_label("Xbox Wireless Controller");
        let icons = |family: ControllerIconFamily, label: &str| -> Vec<VkIcon> {
            let row = shortcut_sheet::LEGEND
                .iter()
                .find(|r| r.label == label)
                .expect(label);
            combo_parts(row.keys, family)
                .into_iter()
                .filter_map(|p| match p {
                    ComboPart::Icon(i, _) => Some(i),
                    _ => None,
                })
                .collect()
        };
        let expect: [(&str, &[VkIcon], &[VkIcon]); 9] = [
            ("Type", &[VkIcon::Ps5Cross], &[VkIcon::XboxA]),
            ("Delete", &[VkIcon::Ps5Circle], &[VkIcon::XboxB]),
            (
                "Move caret",
                &[VkIcon::Ps5L1, VkIcon::Ps5R1],
                &[VkIcon::XboxLb, VkIcon::XboxRb],
            ),
            ("Shift", &[VkIcon::Ps5R2], &[VkIcon::XboxRt]),
            ("Dictate", &[VkIcon::R3Ps5], &[VkIcon::R3Xbox]),
            (
                "Paste",
                &[VkIcon::SelectPs5, VkIcon::Ps5Triangle],
                &[VkIcon::SelectXbox, VkIcon::XboxY],
            ),
            ("Suggestions", &[VkIcon::SelectPs5], &[VkIcon::SelectXbox]),
            ("Close keyboard", &[VkIcon::L3Ps5], &[VkIcon::L3Xbox]),
            ("All shortcuts", &[VkIcon::SelectPs5], &[VkIcon::SelectXbox]),
        ];
        for (label, p, x) in expect {
            assert_eq!(icons(ps, label), p, "{label}");
            assert_eq!(icons(xb, label), x, "{label}");
        }
        let paste = combo_parts(shortcut_sheet::LEGEND[5].keys, ps);
        assert_eq!(
            paste,
            vec![
                ComboPart::Icon(VkIcon::SelectPs5, true),
                ComboPart::Connector("+"),
                ComboPart::Icon(VkIcon::Ps5Triangle, false),
            ]
        );
        let tap = combo_parts(shortcut_sheet::LEGEND[6].keys, xb);
        assert_eq!(tap[0], ComboPart::Prefix("Tap"));
    }

    #[test]
    fn every_legend_and_sheet_button_is_an_svg_icon_for_both_families() {
        let families = [ControllerIconFamily::Ps5, ControllerIconFamily::Xbox];
        let sheet = super::super::shortcut_sheet::GROUPS
            .iter()
            .flat_map(|g| g.rows.iter());
        for row in sheet.chain(shortcut_sheet::LEGEND.iter()) {
            for &token in row.keys {
                if let TipToken::Button(b) = token {
                    for family in families {
                        let icon = family.hint_icon(b).expect(b);
                        assert!(icon.is_controller_tip(), "{b} {family:?}");
                        assert!(icon.svg().contains("<svg"), "{b} {family:?}");
                        assert_eq!(tip_item(token, family), Some(TipItem::Icon(icon)));
                        assert!(combo_parts(&[token], family) == vec![ComboPart::Icon(icon, b == "SELECT")]);
                    }
                }
            }
        }
    }

    #[test]
    fn strip_band_grows_with_the_same_factor_as_the_keys() {
        for style in [VkStyle::Normal, VkStyle::Mono] {
            let one = strip_band_height(1.0, style);
            assert!((strip_band_height(2.0, style) - one * 2.0).abs() < 1e-3);
            assert!(strip_band_height(0.75, style) < one);
        }
    }

    #[test]
    fn modifier_glyphs_map_verbatim() {
        // Non-obvious crossed mapping the renderer must preserve:
        // the Shift key reflects `shift`; the CapsLock key reflects `caps`.
        assert_eq!(shift_icon(true), VkIcon::CapsFilled);
        assert_eq!(shift_icon(false), VkIcon::Caps);
        assert_eq!(caps_icon(true), VkIcon::ShiftFilled);
        assert_eq!(caps_icon(false), VkIcon::Shift);
    }

    #[test]
    fn controller_art_matches_device_families() {
        assert_eq!(
            ControllerArt::from_label("DualSense Wireless Controller"),
            Some(ControllerArt::DualSense)
        );
        assert_eq!(
            ControllerArt::from_label("Xbox Wireless Controller"),
            Some(ControllerArt::XboxOne)
        );
        // Backend slot labels (secure desktop): HID = PlayStation, XInput = Xbox.
        assert_eq!(
            ControllerArt::from_label("HID slot 0"),
            Some(ControllerArt::DualSense)
        );
        assert_eq!(
            ControllerArt::from_label("XInput slot 0"),
            Some(ControllerArt::XboxOne)
        );
        assert_eq!(ControllerArt::from_label("none"), None);
    }

    #[test]
    fn controller_tips_use_the_pad_family_glyphs() {
        let ps = ControllerIconFamily::from_label("DualSense Wireless Controller");
        let xb = ControllerIconFamily::from_label("Xbox Wireless Controller");
        let expect = [
            ("L3", VkIcon::L3Ps5, VkIcon::L3Xbox),
            ("R3", VkIcon::R3Ps5, VkIcon::R3Xbox),
            ("LB", VkIcon::Ps5L1, VkIcon::XboxLb),
            ("RB", VkIcon::Ps5R1, VkIcon::XboxRb),
            ("RT", VkIcon::Ps5R2, VkIcon::XboxRt),
            ("LT", VkIcon::Ps5L2, VkIcon::XboxLt),
            ("SELECT", VkIcon::SelectPs5, VkIcon::SelectXbox),
            ("Y", VkIcon::Ps5Triangle, VkIcon::XboxY),
        ];
        for (button, ps_icon, xb_icon) in expect {
            assert_eq!(ps.hint_icon(button), Some(ps_icon), "{button}");
            assert_eq!(xb.hint_icon(button), Some(xb_icon), "{button}");
        }
        for cue in super::super::controller_tips::CUES.iter() {
            for token in cue.iter() {
                if let TipToken::Button(b) = token {
                    assert!(ps.hint_icon(b).is_some(), "{b}");
                    assert!(xb.hint_icon(b).is_some(), "{b}");
                }
            }
        }
    }

    #[test]
    fn shortcut_sheet_rows_use_the_pad_family_glyphs() {
        let ps = ControllerIconFamily::from_label("DualShock 4 Wireless Controller");
        let xb = ControllerIconFamily::from_label("Xbox Wireless Controller");
        let icons = |family: ControllerIconFamily, label: &str| -> Vec<VkIcon> {
            super::super::shortcut_sheet::GROUPS
                .iter()
                .flat_map(|g| g.rows.iter())
                .find(|r| r.label == label)
                .expect(label)
                .keys
                .iter()
                .filter_map(|t| match t {
                    TipToken::Button(b) => Some(family.hint_icon(b).expect(b)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(icons(ps, "Copy"), [VkIcon::SelectPs5, VkIcon::Ps5Square]);
        assert_eq!(icons(xb, "Copy"), [VkIcon::SelectXbox, VkIcon::XboxX]);
        assert_eq!(
            icons(ps, "Clear field"),
            [VkIcon::SelectPs5, VkIcon::Ps5Circle]
        );
        assert_eq!(
            icons(xb, "Clear field"),
            [VkIcon::SelectXbox, VkIcon::XboxB]
        );
        assert_eq!(
            icons(ps, "Screenshot"),
            [VkIcon::Ps5L1, VkIcon::Ps5R1, VkIcon::Ps5R2]
        );
        assert_eq!(
            icons(xb, "Screenshot"),
            [VkIcon::XboxLb, VkIcon::XboxRb, VkIcon::XboxRt]
        );
        assert_eq!(icons(ps, "Close keyboard"), [VkIcon::L3Ps5]);
        assert_eq!(icons(xb, "Dictate"), [VkIcon::R3Xbox]);
        for b in super::super::shortcut_sheet::buttons() {
            let (p, x) = (ps.hint_icon(b), xb.hint_icon(b));
            assert!(p.is_some() && x.is_some(), "{b}");
            assert_ne!(p, x, "{b}");
        }
    }

    #[test]
    fn controller_icons_use_ps5_for_playstation_and_xbox_as_generic() {
        assert_eq!(
            ControllerIconFamily::from_label("DualSense Wireless Controller"),
            ControllerIconFamily::Ps5
        );
        assert_eq!(
            ControllerIconFamily::from_label("HID slot 0"),
            ControllerIconFamily::Ps5
        );
        assert_eq!(
            ControllerIconFamily::from_label("Xbox Wireless Controller"),
            ControllerIconFamily::Xbox
        );
        assert_eq!(
            ControllerIconFamily::from_label("Nintendo Pro Controller"),
            ControllerIconFamily::Xbox
        );
        assert_eq!(
            ControllerIconFamily::from_label("none"),
            ControllerIconFamily::Xbox
        );
        assert_eq!(
            ControllerIconFamily::Ps5.hint_icon("X"),
            Some(VkIcon::Ps5Square)
        );
        assert_eq!(
            ControllerIconFamily::Xbox.hint_icon("X"),
            Some(VkIcon::XboxX)
        );
        assert_eq!(ControllerIconFamily::Xbox.hint_icon("unknown"), None);
    }

    #[test]
    fn normal_key_radius_matches_the_design() {
        let spec = style_spec(VkStyle::Normal);
        assert_eq!(spec.key_radius, 12.0);
        let unit = ref_unit(1.0, VkStyle::Normal);
        let kh = ref_key_h(1.0, VkStyle::Normal);
        assert!((spec.key_radius * unit - 12.0 * kh / 102.0).abs() < 1e-4);
    }

    #[test]
    fn normal_strip_chips_are_concentric_with_their_track() {
        let spec = style_spec(VkStyle::Normal);
        let chip_r = (spec.chips_h - spec.chips_pad * 2.0) * 0.5;
        assert_eq!(
            crate::vk_motion::concentric_radius(chip_r, spec.chips_pad),
            spec.chips_h * 0.5
        );
    }

    #[test]
    fn design_frames_are_1468_wide() {
        for style in [VkStyle::Normal, VkStyle::Mono] {
            let spec = style_spec(style);
            let key_w = spec.design_kh / spec.key_aspect;
            let frame = 10.0 * key_w + 9.0 * spec.gap + 2.0 * spec.pad_x;
            assert!((frame - 1468.0).abs() < 0.5, "{style:?}: {frame}");
        }
    }

    #[test]
    fn strip_bar_sits_in_the_band_above_the_floating_grid() {
        for style in [VkStyle::Normal, VkStyle::Mono] {
            let spec = style_spec(style);
            let rows = crate::vk_nav::rows_for_test();
            let (grid_w, block_h) = grid_size(REF_MON_W, &rows, style);
            let (pad_x, pad_y) = floating_pad(1.0, style);
            let chrome = strip_band_height(1.0, style);
            let cw = grid_w + pad_x * 2.0;
            let ch = (chrome + block_h + pad_y * 2.0).ceil();
            let rects = key_rects(cw, ch, REF_MON_W, &rows, chrome, style);
            let kh = rects[0].bottom - rects[0].top;
            assert!((kh - ref_key_h(1.0, style)).abs() < 1e-3);
            let unit = kh / spec.design_kh;
            let geom = StripGeom::above(&rects, spec, unit);
            assert!((geom.top - FLOATING_PANEL_INSET).abs() < 1.0, "{style:?} {geom:?}");
            assert!((geom.bottom - geom.top - spec.strip_bar_h * unit).abs() < 1e-3);
            assert!((geom.left - pad_x).abs() < 0.5 && (cw - geom.right - pad_x).abs() < 0.5);
        }
    }

    #[test]
    fn mono_style_relabels_keys_like_ios() {
        let mono = style_spec(VkStyle::Mono);
        let normal = style_spec(VkStyle::Normal);
        assert_eq!(styled_glyph(mono, &KeyAction::Char('q'), "q".into()), "q");
        assert_eq!(styled_glyph(mono, &KeyAction::Char('Q'), "Q".into()), "Q");
        assert_eq!(styled_glyph(normal, &KeyAction::Char('q'), "q".into()), "q");
        assert_eq!(styled_glyph(mono, &KeyAction::Symbols, "?123".into()), "123");
        assert_eq!(styled_glyph(normal, &KeyAction::Symbols, "?123".into()), "?123");
        assert_eq!(styled_glyph(mono, &KeyAction::Char(';'), ";".into()), ";");
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN, VK_SPACE};
        assert!(is_action_key(&KeyAction::Vk(VK_BACK)));
        assert!(is_action_key(&KeyAction::Vk(VK_RETURN)));
        for letter_fill in [
            KeyAction::Shift,
            KeyAction::Symbols,
            KeyAction::PredictPrev,
            KeyAction::PredictNext,
            KeyAction::VoiceInput,
            KeyAction::Char('a'),
            KeyAction::Char(';'),
            KeyAction::Vk(VK_SPACE),
        ] {
            assert!(!is_action_key(&letter_fill));
        }
        let rows = crate::vk_nav::rows_for_test();
        let dark: usize = rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .filter(|k| is_action_key(&k.action))
            .count();
        assert_eq!(dark, 2);
        assert_eq!(strip_slots(VkStyle::Normal), 7);
        assert_eq!(strip_slots(VkStyle::Mono), 7);
    }

    #[test]
    fn blur_shadow_layers_compose_to_the_design_alpha() {
        let layers = blur_shadow_layers(24.0, 0.5);
        let mut clear = 1.0f32;
        let mut last = f32::NEG_INFINITY;
        for (spread, a) in layers {
            assert!(spread > last);
            last = spread;
            clear *= 1.0 - a;
        }
        assert!(((1.0 - clear) - 0.5).abs() < 1e-4);
        assert!(layers[BLUR_SHADOW_STEPS - 1].0 <= 12.0 + 1e-4);
    }

    #[test]
    fn only_mono_swaps_the_prompt_look() {
        assert!(prompt_spec(VkStyle::Normal).is_none());
        let mono = prompt_spec(VkStyle::Mono).expect("mono prompt spec");
        assert_eq!(mono.radius * 2.0, mono.pill_h);
        assert_eq!((mono.card_w, mono.card_h), (460.0, 331.0));
        assert_eq!(mono.families, style_spec(VkStyle::Mono).families);
        for (bottom, (dy, blur, _)) in [
            (mono.pill_bottom, mono.pill_shadow),
            (mono.card_bottom, mono.card_shadow),
        ] {
            assert!(bottom >= dy + blur * 0.5);
        }
        let card_stack = mono.card_pad_y * 2.0
            + mono.title_h
            + mono.stage_h
            + mono.status_h
            + mono.card_gap * 2.0;
        assert!(card_stack <= mono.card_h);
    }

    #[test]
    fn press_transform_scales_about_the_key_centre() {
        let m = scale_about(0.96, 100.0, 50.0);
        // Centre is a fixed point: (100, 50) -> (100, 50).
        let x = m.M11 * 100.0 + m.M21 * 50.0 + m.M31;
        let y = m.M12 * 100.0 + m.M22 * 50.0 + m.M32;
        assert!((x - 100.0).abs() < 1e-4 && (y - 50.0).abs() < 1e-4);
        // A corner moves inward toward the centre.
        let cx = m.M11 * 146.0 + m.M31;
        assert!(cx < 146.0 && cx > 100.0);
        assert_eq!(scale_about(1.0, 3.0, 4.0).M31, 0.0);
        assert_eq!(translate(0.0, 4.0).M32, 4.0);
    }

    #[test]
    fn transcription_cloud_sits_on_the_right_of_a_1080p_frame() {
        let slot = nimbus_slot(1920.0, 1080.0);
        let w = slot.right - slot.left;
        let h = slot.bottom - slot.top;
        assert!((w - 96.0).abs() < 1.0 && (h - w).abs() < 1.0);
        assert!((slot.right - (1920.0 - 48.0)).abs() < 1.0);
        assert!((slot.top - (1080.0 - 96.0) * 0.5).abs() < 1.0);
        // A short window can't push the square off the top or the right.
        let rest = orb_scale(VoicePhase::Starting, 0.0, 0.0);
        assert!((rest - 0.88).abs() < 1e-4);
        assert!((orb_scale(VoicePhase::Listening, 0.0, 0.0) - rest).abs() < 1e-4);
        assert!(orb_scale(VoicePhase::Listening, 1.0, 0.0) > rest);
        assert!((orb_scale(VoicePhase::Transcribing, 0.0, 0.5) - rest).abs() < 1e-4);
        assert!(orb_scale(VoicePhase::Transcribing, 0.0, 1.0) > rest);
        let peak = orb_scale(VoicePhase::Listening, 1.0, 0.0);
        assert!((peak - orb_scale(VoicePhase::Transcribing, 0.0, 1.0)).abs() < 1e-4);
        assert!(ORB_FRAME_FILL * peak <= 1.0);
        let tiny = nimbus_slot(100.0, 80.0);
        assert!(tiny.left >= 0.0 && tiny.top >= 0.0);
        assert!(tiny.right <= 100.0 + 0.5 && tiny.bottom <= 80.0 + 0.5);
    }

    #[test]
    fn downscale_box_filter_shrinks_and_averages() {
        // 2x2 premultiplied-BGRA source averaged to a single pixel.
        // Pixels: (b,g,r,a) = (0,0,0,0), (40,40,40,40), (80,80,80,80), (120,120,120,120)
        let src = vec![
            0, 0, 0, 0, 40, 40, 40, 40, 80, 80, 80, 80, 120, 120, 120, 120,
        ];
        let (out, w, h) = downscale_bgra_premul(&src, 2, 2, 1);
        assert_eq!((w, h), (1, 1));
        // Mean of {0,40,80,120} = 60 in every channel.
        assert_eq!(out, vec![60, 60, 60, 60]);
    }

    #[test]
    fn downscale_passes_through_when_already_small() {
        let src = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let (out, w, h) = downscale_bgra_premul(&src, 2, 1, 64);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, src);
    }
}

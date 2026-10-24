//! D3D11 + DXGI composition swapchain + D2D + DirectComposition renderer.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::time::Instant;

use windows::core::{w, Interface};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FILL_MODE_ALTERNATE, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Factory, ID2D1Factory1,
    ID2D1Resource, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_ROUNDED_RECT, D2D1_SWEEP_DIRECTION_CLOCKWISE,
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
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_FAR,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
};
#[cfg(feature = "vk-panels")]
use windows::Win32::Graphics::DirectWrite::{IDWriteTextLayout, DWRITE_TEXT_METRICS};
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

#[cfg(test)]
use super::controller_tips::TipToken;
#[cfg(test)]
use crate::vk_paint::{tip_item, TipItem};
use super::nimbus_orb::{drive_scale, mood_drive, NimbusMood, NimbusOrb, ORB_FRAME_FILL};
use super::shortcut_sheet;
#[cfg(feature = "vk-panels")]
use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP;
#[cfg(test)]
use super::shortcut_sheet::Line;
use crate::config::VkStyle;
use crate::mono_ui::{Gfx, user_locale_name};
#[cfg(test)]
use crate::vk_nav::KeyAction;
use crate::vk_nav::{KeyCell, KeyPos, KeyRow};
#[cfg(test)]
use crate::vk_paint::{caps_icon, is_action_key, shift_icon, styled_glyph};
#[cfg(feature = "vk-panels")]
pub(crate) use crate::vk_paint::{icon_draw_px, paint_icon};
pub(crate) use crate::vk_paint::{
    is_playstation_label, paint_focus_ring, paint_key,
    paint_strip, ControllerIconFamily, Edges, KeyState, KeyStyle, StripGeom, StripPaint, VkIcon,
};
#[cfg(test)]
use crate::vk_paint::{bar_layout, legend_layout, BarLayout, BAR_LABEL_PX, BAR_SEP_PX};
pub(crate) use crate::vk_paint::{legend_span, paint_legend, LegendPaint, BAR_H};
#[cfg(test)]
use crate::vk_paint::{combo_parts, combo_width, text_px, ComboPart, ComboStyle, SHEET_PAD_X};
pub use crate::vk_paint::legend_band_height;
pub(crate) use crate::vk_paint::{
    paint_modern_sheet, paint_sheet_hint, paint_shortcut_sheet, SheetPaint, MS_SCALE,
};
#[cfg(test)]
use crate::vk_paint::{
    sheet_block_size, sheet_columns, sheet_combo_style, sheet_fit, sheet_total_width,
    side_combo_style, SheetColumn, SheetMetrics,
};
pub use crate::vk_geom::KeyRect;
pub use crate::vk_paint::{
    FLOATING_PANEL_INSET,
    MONO_FAMILIES,
    PromptSpec,
    StripLook,
    StyleSpec,
    VkLook,
    VkPalette,
    colorref_mix,
    floating_pad,
    lerp,
    mix_color,
    prompt_spec,
    ref_key_h,
    ref_unit,
    strip_band_height,
    strip_slots,
    style_palette,
    style_spec,
};

#[cfg(feature = "vk-panels")]
#[path = "vk_panels.rs"]
pub(crate) mod vk_panels;

/// GDI `COLORREF` (`0x00BBGGRR`) -> D2D color.
fn colorref(c: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (c & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: ((c >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

#[cfg(feature = "vk-panels")]
fn colorref_alpha(c: u32, alpha: f32) -> D2D1_COLOR_F {
    let mut col = colorref(c);
    col.a = alpha;
    col
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

#[cfg(feature = "vk-panels")]
fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: radius,
        radiusY: radius,
    }
}

#[cfg(feature = "vk-panels")]
fn deflate(rect: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left + d,
        top: rect.top + d,
        right: rect.right - d,
        bottom: rect.bottom - d,
    }
}

#[cfg(feature = "vk-panels")]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[derive(Clone)]
struct StyleFonts {
    look: VkLook,
    size_key: i32,
    label: IDWriteTextFormat,
    word: IDWriteTextFormat,
    number: IDWriteTextFormat,
    space: IDWriteTextFormat,
    chip: IDWriteTextFormat,
    chip_sel: IDWriteTextFormat,
    label_corner: IDWriteTextFormat,
}


pub struct VkRenderer {
    width: u32,
    height: u32,
    dip_scale: f32,
    swapchain: IDXGISwapChain1,
    d2d_context: ID2D1DeviceContext,
    gfx: Gfx,
    /// Owns the swapchain buffer. Must be dropped before `ResizeBuffers`, or the
    /// resize fails and every later draw is left with no target (the last pill
    /// frame stays on screen).
    d2d_target: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    text_format: IDWriteTextFormat,
    glyph_format: IDWriteTextFormat,
    glyph_px: f32,
    /// Small font for sublabels, badges, and the legend strip.
    hint_format: IDWriteTextFormat,
    hint_px: f32,
    style_fonts: Option<StyleFonts>,
    /// Fixed large font for the connect/keyboard prompt pills (10-foot UI).
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
/// Time constant for the focus ring gliding to the selected key. Small =
/// snappy (~90 ms settle); the fill stays instant so labels never tear.
const SEL_GLIDE_TAU: f32 = 0.045;

/// Hairline the floating panel is inset from the window so its antialiased
/// stroke is never clipped.

pub const STRIP_BAND_H: f32 = 67.0;


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ControllerArtCacheKey {
    art: ControllerArt,
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

/// Natural bounding box `(width, height)` of the key grid at `scale_w`, excluding
/// card padding and top chrome. Lets the floating card be sized to wrap keys that
/// render at the same scale as the docked bar.
pub fn grid_size(scale_w: f32, rows: &[KeyRow], look: impl Into<VkLook>) -> (f32, f32) {
    let spec = style_spec(look);
    crate::vk_geom::grid_size(scale_w, rows, spec.key_aspect, spec.gap, spec.design_kh)
}

#[cfg(test)]
pub fn fitted_key_height(
    scale_w: f32,
    client_h: f32,
    rows: &[KeyRow],
    top_inset: f32,
    look: impl Into<VkLook>,
) -> f32 {
    key_metrics(scale_w, client_h, rows, top_inset, look).1
}

pub fn dock_height_for_full_keys(
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    look: impl Into<VkLook>,
) -> f32 {
    let spec = style_spec(look);
    crate::vk_geom::dock_height_for_full_keys(
        scale_w,
        rows,
        top_inset,
        spec.key_aspect,
        spec.gap,
        spec.design_kh,
    )
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

pub fn strip_hit_slot(
    client_w: f32,
    client_h: f32,
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    look: impl Into<VkLook>,
    visible: &[String],
    x: f32,
    y: f32,
) -> Option<usize> {
    let look = look.into();
    let spec = style_spec(look);
    let rects = key_rects(client_w, client_h, scale_w, rows, top_inset, look);
    let key_h = rects.first().map(|kr| kr.bottom - kr.top)?;
    let geom = StripGeom::above(&rects, spec, key_h / spec.design_kh);
    geom.slot_rects(spec, visible)
        .into_iter()
        .find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
        .map(|(slot, _)| slot)
}

pub fn key_rects(
    client_w: f32,
    client_h: f32,
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    look: impl Into<VkLook>,
) -> Vec<KeyRect> {
    let spec = style_spec(look);
    crate::vk_geom::key_rects(
        client_w,
        client_h,
        scale_w,
        rows,
        top_inset,
        spec.key_aspect,
        spec.gap,
        spec.design_kh,
    )
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
    pub tv: bool,
    pub shortcut_sheet: bool,
    pub legend: bool,
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
            dip_scale: 1.0,
            swapchain,
            d2d_context: d2d_context.clone(),
            d2d_target: Some(d2d_target),
            gfx: Gfx::wrap(&d2d_context)?,
            dwrite,
            text_format,
            glyph_format,
            glyph_px: label_px * 1.1,
            hint_format,
            hint_px: (label_px * 0.5).clamp(10.0, 20.0),
            style_fonts: None,
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
        self.resize_to(width, height)
    }

    pub fn set_dip_scale(&mut self, scale: f32) {
        self.dip_scale = if scale.is_finite() {
            scale.max(1.0)
        } else {
            1.0
        };
    }

    unsafe fn set_transform(&mut self, m: &Matrix3x2) {
        self.d2d_context.SetTransform(m);
        self.gfx.transform(m.M11, m.M31, m.M32);
    }

    unsafe fn dip_canvas(&self) -> (f32, f32) {
        let dpi = 96.0 * self.dip_scale;
        self.d2d_context.SetDpi(dpi, dpi);
        (
            self.width as f32 / self.dip_scale,
            self.height as f32 / self.dip_scale,
        )
    }

    pub unsafe fn resize_to(&mut self, width: u32, height: u32) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
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

    unsafe fn ensure_style_fonts(&mut self, look: VkLook, key_h: f32) -> Result<(), String> {
        let size_key = (key_h * 4.0).round() as i32;
        if self
            .style_fonts
            .as_ref()
            .is_some_and(|f| f.look == look && f.size_key == size_key)
        {
            return Ok(());
        }
        let mut fonts: Option<IDWriteFontCollection> = None;
        self.dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let locale = user_locale_name();
        let spec = style_spec(look);
        let family = resolve_family(&fonts, spec.families);
        let u = key_h.max(1.0) / spec.design_kh;
        let dwrite = &self.dwrite;
        let make = |weight: i32, px: f32| -> Result<IDWriteTextFormat, String> {
            let f = dwrite
                .CreateTextFormat(
                    &family,
                    &fonts,
                    DWRITE_FONT_WEIGHT(weight),
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
            look,
            size_key,
            label: make(spec.label_weight, spec.label_px)?,
            word: make(spec.word_weight, spec.word_px)?,
            number: make(400, spec.number_px)?,
            space: make(400, spec.space_label_px)?,
            chip: make(400, spec.chip_px)?,
            chip_sel: make(spec.chip_sel_weight, spec.chip_px)?,
            label_corner: {
                let f = make(spec.label_weight, spec.label_px)?;
                let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_FAR);
                f
            },
        };
        self.style_fonts = Some(built);
        Ok(())
    }

    #[cfg(feature = "vk-panels")]
    unsafe fn draw_svg_icon(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
    ) -> Result<(), String> {
        self.draw_svg_icon_alpha(icon, rect, color, 1.0)
    }

    #[cfg(feature = "vk-panels")]
    unsafe fn draw_svg_icon_alpha(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
        opacity: f32,
    ) -> Result<(), String> {
        let draw_px = icon_draw_px(icon, rect.bottom - rect.top);
        self.draw_svg_icon_sized(icon, rect, color, opacity, draw_px)
    }

    #[cfg(feature = "vk-panels")]
    unsafe fn draw_svg_icon_sized(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
        opacity: f32,
        draw_px: f32,
    ) -> Result<(), String> {
        let edges = Edges {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        paint_icon(&mut self.gfx, icon, edges, color, opacity, draw_px, self.dip_scale);
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


    pub unsafe fn present_clear(&mut self) -> Result<(), String> {
        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))
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
            tv,
            shortcut_sheet: sheet_on,
            legend,
        } = *frame;
        let look = VkLook { style, tv };
        let spec = style_spec(look);
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

        let legend_h = if legend && !look.tv_layout() {
            legend_band_height(ui_scale, look)
        } else {
            0.0
        };
        let rects = key_rects(cw, ch - legend_h, scale_w, rows, top_inset, look);
        let key_h = rects
            .first()
            .map(|kr| kr.bottom - kr.top)
            .unwrap_or_else(|| ref_key_h(ui_scale, look));
        let unit = key_h / spec.design_kh;
        self.ensure_style_fonts(look, key_h)?;
        #[cfg(feature = "vk-panels")]
        let fonts = self
            .style_fonts
            .clone()
            .ok_or_else(|| "style fonts missing".to_string())?;

        self.d2d_context.BeginDraw();
        // Per-key press transforms below are scoped; start every frame clean in
        // case a previous frame bailed mid-key.
        self.set_transform(&IDENTITY);

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
            crate::vk_paint::paint_floating_panel(
                &mut self.gfx,
                crate::mono_ui::Rect {
                    x: panel.left,
                    y: panel.top,
                    w: panel.right - panel.left,
                    h: panel.bottom - panel.top,
                },
                radius,
                1.0,
                pal,
            );
            self.d2d_context.PushAxisAlignedClip(
                &panel,
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        } else {
            self.d2d_context.Clear(Some(&colorref(pal.bg)));
        }

        #[cfg(feature = "vk-panels")]
        let radius = spec.key_radius * unit;
        #[cfg(feature = "vk-panels")]
        let panel_open = vk_panels::is_open();
        #[cfg(not(feature = "vk-panels"))]
        let panel_open = false;
        #[cfg(feature = "vk-panels")]
        if panel_open {
            self.draw_panel(
                pal,
                spec,
                &fonts,
                &rects,
                unit,
                radius,
                style,
                controller_icons,
            )?;
        }

        let ks = KeyStyle {
            look,
            pal,
            unit,
            key_h,
            dip_scale: self.dip_scale,
            glyph_px: self.glyph_px,
            hint_px: self.hint_px,
            icons: controller_icons,
        };
        let think = self.think_pulse();
        let voice_t = self.prompt_started.elapsed().as_secs_f32();
        let nimbus = self.nimbus.as_ref();
        let ctx = &self.d2d_context;
        let gfx = &mut self.gfx;
        let mut voice_paint = |g: &mut Gfx, k: Edges, radius: f32| {
            let cx = (k.left + k.right) * 0.5;
            let cy = (k.top + k.bottom) * 0.5;
            let side = (k.right - k.left).min(k.bottom - k.top).max(1.0);
            let transcribing = matches!(voice_phase, VoicePhase::Transcribing);
            g.push_clip(k.rect(), true);
            let ball = side * ORB_FRAME_FILL * orb_scale(voice_phase, voice_level, think);
            let drew_nimbus = match nimbus {
                Some(orb) => {
                    ctx.DrawBitmap(
                        orb.bitmap(),
                        Some(&square_about(cx, cy, ball)),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                    true
                }
                None => false,
            };
            if !drew_nimbus {
                crate::vk_paint::paint_voice_orb(
                    g,
                    pal.accent,
                    voice_level,
                    transcribing,
                    cx,
                    cy,
                    (side * 0.56).max(1.0),
                    1.25,
                    voice_t,
                );
            }
            g.pop_clip();
            let halo_alpha = match voice_phase {
                VoicePhase::Starting => 0.18,
                VoicePhase::Listening => 0.28 + 0.34 * voice_level.clamp(0.0, 1.0),
                VoicePhase::Transcribing => {
                    let pulse = 0.5 + 0.5 * (voice_t * 2.4).sin();
                    0.28 + 0.42 * pulse
                }
            };
            g.ring(
                crate::mono_ui::Rect {
                    x: k.left - 1.0,
                    y: k.top - 1.0,
                    w: k.right - k.left + 2.0,
                    h: k.bottom - k.top + 2.0,
                },
                radius + 1.0,
                2.0,
                pal.accent,
                halo_alpha,
            );
        };
        for kr in rects.iter().filter(|_| !panel_open) {
            let key = &rows[kr.pos.row].keys[kr.pos.col];
            let st = KeyState {
                selected: sel.row == kr.pos.row && sel.col == kr.pos.col,
                press_scale: pressed
                    .filter(|(p, _)| p.row == kr.pos.row && p.col == kr.pos.col)
                    .map(|(_, s)| s),
                shift: modifiers.shift,
                caps: modifiers.caps,
                symbols: modifiers.symbol,
                symbols_locked: modifiers.symbol_locked,
                voice_available,
                tonal_row: kr.pos.row == 0 && rows.len() == 5,
                glyph: key_glyph(key),
                hint: key_hint(key),
            };
            let voice: Option<&mut dyn FnMut(&mut Gfx, Edges, f32)> = if voice_active {
                Some(&mut voice_paint)
            } else {
                None
            };
            paint_key(gfx, key, kr.edges(), &ks, &st, voice);
        }

        if let Some(ring) = self.anim_sel.filter(|_| !panel_open) {
            let ring_press = pressed
                .filter(|(p, _)| p.row == sel.row && p.col == sel.col)
                .map(|(_, s)| s);
            paint_focus_ring(
                &mut self.gfx,
                Edges {
                    left: ring.left,
                    top: ring.top,
                    right: ring.right,
                    bottom: ring.bottom,
                },
                &ks,
                ring_press,
            );
        }

        if legend && !look.tv_layout() {
            let lu = ref_unit(ui_scale, look);
            let (left, right) = legend_span(&rects, cw, floating_pad(ui_scale, look).0);
            self.draw_legend(
                pal,
                style,
                controller_icons,
                lu,
                ch - BAR_H * lu,
                left,
                right,
                1.0,
            )?;
        }

        // Suggestion pill last, so it floats on top of the keys (and the ring).
        if let Some(strip) = candidates.filter(|_| !panel_open) {
            let geom = StripGeom::above(&rects, spec, unit);
            self.set_transform(&translate(0.0, strip_dy));
            let result = self.draw_strip(
                spec,
                pal,
                strip,
                geom,
                strip_alpha.clamp(0.0, 1.0),
                controller_icons,
            );
            self.set_transform(&IDENTITY);
            result?;
        }

        if look.tv_layout() {
            self.sheet_alpha = if sheet_on { 1.0 } else { 0.0 };

            if floating {
                self.d2d_context.PopAxisAlignedClip();
            }

            let strip_has_words =
                candidates.is_some_and(|s| s.visible.iter().any(|w| !w.is_empty()));
            if legend && !rects.is_empty() && !strip_has_words {
                let l = rects.iter().map(|kr| kr.left).fold(f32::INFINITY, f32::min);
                let r = rects
                    .iter()
                    .map(|kr| kr.right)
                    .fold(f32::NEG_INFINITY, f32::max);
                let t = rects.iter().map(|kr| kr.top).fold(f32::INFINITY, f32::min);
                let su = ref_unit(ui_scale, look);
                self.draw_sheet_hint(pal, style, controller_icons, su, (l + r) * 0.5, t, cw)?;
            }

            if self.sheet_alpha > 0.0 {
                let full = D2D_RECT_F {
                    left: FLOATING_PANEL_INSET,
                    top: FLOATING_PANEL_INSET,
                    right: cw - FLOATING_PANEL_INSET,
                    bottom: ch - FLOATING_PANEL_INSET,
                };
                let radius = floating.then_some(spec.panel_radius * unit);
                self.draw_modern_sheet(
                    pal,
                    style,
                    controller_icons,
                    ref_unit(ui_scale, look) * MS_SCALE,
                    full,
                    radius,
                    self.sheet_alpha,
                )?;
            }
        } else {
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
        }

        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_sheet_hint(
        &mut self,
        pal: &VkPalette,
        style: VkStyle,
        family: ControllerIconFamily,
        u: f32,
        center_x: f32,
        keys_top: f32,
        client_w: f32,
    ) -> Result<(), String> {
        let sp = SheetPaint {
            pal,
            style,
            family,
            dip_scale: self.dip_scale,
        };
        paint_sheet_hint(&mut self.gfx, &sp, u, center_x, keys_top, client_w);
        Ok(())
    }


    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_modern_sheet(
        &mut self,
        pal: &VkPalette,
        style: VkStyle,
        family: ControllerIconFamily,
        unit: f32,
        panel: D2D_RECT_F,
        radius: Option<f32>,
        alpha: f32,
    ) -> Result<(), String> {
        let sp = SheetPaint {
            pal,
            style,
            family,
            dip_scale: self.dip_scale,
        };
        let panel = Edges {
            left: panel.left,
            top: panel.top,
            right: panel.right,
            bottom: panel.bottom,
        };
        paint_modern_sheet(&mut self.gfx, &sp, unit, panel, radius, alpha);
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

    #[cfg(test)]
    unsafe fn measure_px(&self, style: VkStyle, text: &str, px: f32) -> f32 {
        crate::vk_paint::paint_measure_px(&self.gfx, style, text, px)
    }

    #[cfg(test)]
    unsafe fn bar_layout_for(
        &self,
        style: VkStyle,
        family: ControllerIconFamily,
        u: f32,
        left: f32,
        right: f32,
    ) -> BarLayout {
        legend_layout(
            &self.gfx,
            &shortcut_sheet::shown_legend(),
            style,
            family,
            u,
            left,
            right,
        )
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
        let lp = LegendPaint {
            pal,
            style,
            family,
            u,
            top,
            left,
            right,
            alpha,
            dip_scale: self.dip_scale,
        };
        paint_legend(&mut self.gfx, &shortcut_sheet::shown_legend(), &lp);
        Ok(())
    }

    #[cfg(test)]
    unsafe fn sheet_fit(
        &self,
        style: VkStyle,
        family: ControllerIconFamily,
        unit: f32,
        panel: D2D_RECT_F,
    ) -> (SheetMetrics, Vec<SheetColumn>, f32) {
        let panel = Edges {
            left: panel.left,
            top: panel.top,
            right: panel.right,
            bottom: panel.bottom,
        };
        sheet_fit(&self.gfx, style, family, unit, panel)
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
        let sp = SheetPaint {
            pal,
            style,
            family,
            dip_scale: self.dip_scale,
        };
        let panel = Edges {
            left: panel.left,
            top: panel.top,
            right: panel.right,
            bottom: panel.bottom,
        };
        paint_shortcut_sheet(&mut self.gfx, &sp, unit, panel, radius, alpha);
        Ok(())
    }

    unsafe fn draw_strip(
        &mut self,
        spec: &StyleSpec,
        pal: &VkPalette,
        strip: &crate::vk_predict::StripState,
        geom: StripGeom,
        alpha: f32,
        icons: ControllerIconFamily,
    ) -> Result<(), String> {
        let sp = StripPaint {
            spec,
            pal,
            alpha,
            icons,
            select_retired: spec.strip == StripLook::Chips && shortcut_sheet::hint_retired(),
            dip_scale: self.dip_scale,
        };
        paint_strip(&mut self.gfx, &sp, strip, &geom);
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
        let (cw, ch) = self.dip_canvas();
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
    #[cfg(feature = "vk-panels")]
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
        let (cw, ch) = self.dip_canvas();
        let art = match ControllerArt::from_label(p.controller_label) {
            Some(art) => {
                self.ensure_controller_art(art)?;
                self.controller_art_cache
                    .get(&ControllerArtCacheKey { art })
                    .map(|(bitmap, w, h)| (bitmap.clone(), *w, *h))
            }
            None => None,
        };
        let canvas = crate::vk_paint::PromptCanvas {
            cw,
            ch,
            dpr: 1.0,
            t: self.prompt_started.elapsed().as_secs_f32(),
            title_px: self.text_format.GetFontSize(),
            dip_scale: self.dip_scale,
        };
        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        let mut paint_art = |g: &mut Gfx, _: ControllerArt, rect: crate::mono_ui::Rect, alpha: f32| {
            if let Some((bitmap, w, h)) = &art {
                let dest = crate::vk_paint::fit_rect(rect, *w as f32, *h as f32);
                g.draw_bitmap(bitmap, dest, alpha.clamp(0.0, 1.0), true);
            }
        };
        crate::vk_paint::paint_prompt_card(&mut self.gfx, p, &canvas, &mut paint_art)?;
        self.set_transform(&IDENTITY);
        self.d2d_context
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        self.swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(|e| format!("Present: {e}"))?;
        Ok(())
    }

    pub unsafe fn draw_tips(&mut self, p: &TipsPill) -> Result<(), String> {
        let (cw, ch) = self.dip_canvas();
        let canvas = crate::vk_paint::PromptCanvas {
            cw,
            ch,
            dpr: 1.0,
            t: self.prompt_started.elapsed().as_secs_f32(),
            title_px: self.text_format.GetFontSize(),
            dip_scale: self.dip_scale,
        };
        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        crate::vk_paint::paint_tips(&mut self.gfx, p, &canvas)?;
        self.set_transform(&IDENTITY);
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

    fn think_pulse(&self) -> f32 {
        self.nimbus
            .as_ref()
            .map(|orb| orb.think_pulse())
            .unwrap_or(0.0)
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
        let (cw, ch) = self.dip_canvas();
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
            self.set_transform(&IDENTITY);
            let t = self.prompt_started.elapsed().as_secs_f32();
            let orb_fill =
                show_orb.then(|| ORB_FRAME_FILL * orb_scale(phase, level, self.think_pulse()));
            let nimbus = self.nimbus.as_ref();
            let ctx = &self.d2d_context;
            let mut paint_nimbus = |_: &mut Gfx, slot: crate::mono_ui::Rect, opacity: f32| {
                let Some(orb) = nimbus else {
                    return false;
                };
                if opacity > 0.01 {
                    ctx.DrawBitmap(
                        orb.bitmap(),
                        Some(&D2D_RECT_F {
                            left: slot.x,
                            top: slot.y,
                            right: slot.x + slot.w,
                            bottom: slot.y + slot.h,
                        }),
                        opacity.clamp(0.0, 1.0),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                }
                true
            };
            crate::vk_paint::paint_voice_screen(
                &mut self.gfx,
                cw,
                ch,
                accent,
                alpha,
                matches!(phase, VoicePhase::Transcribing),
                t,
                orb_fill,
                &mut paint_nimbus,
            )?;
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

pub(crate) use crate::vk_paint::ControllerArt;
pub use crate::vk_paint::{PromptCard, TipsPill};

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
    let resource: ID2D1Resource = ctx.cast().map_err(|e| format!("d2d resource: {e}"))?;
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
#[cfg(test)]
fn nimbus_slot(cw: f32, ch: f32) -> D2D_RECT_F {
    let r = crate::vk_paint::voice_slot(cw, ch);
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
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

pub(crate) unsafe fn resolve_family(
    fonts: &IDWriteFontCollection,
    families: &[&str],
) -> windows::core::HSTRING {
    for name in families {
        let family = windows::core::HSTRING::from(*name);
        let mut index = 0u32;
        let mut exists = BOOL(0);
        if fonts
            .FindFamilyName(&family, &mut index, &mut exists)
            .is_ok()
            && exists.as_bool()
        {
            return family;
        }
    }
    windows::core::HSTRING::from("Segoe UI")
}

unsafe fn create_dwrite() -> Result<IDWriteFactory, String> {
    DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| format!("DWriteCreateFactory: {e}"))
}

/// `CreateTextFormat` rejects a null locale on some builds; use the user default.
fn key_metrics(
    scale_w: f32,
    client_h: f32,
    rows: &[KeyRow],
    top_inset: f32,
    look: impl Into<VkLook>,
) -> (f32, f32, f32) {
    let spec = style_spec(look);
    crate::vk_geom::key_metrics(
        scale_w,
        client_h,
        rows,
        top_inset,
        spec.key_aspect,
        spec.gap,
        spec.design_kh,
    )
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
        self.set_transform(&IDENTITY);
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
            std::ptr::copy_nonoverlapping(
                row,
                px.as_mut_ptr().add(y * w as usize * 4),
                w as usize * 4,
            );
        }
        ctx.Unmap(&staging, 0);
        Ok((w, h, px))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vk_paint::{BLUR_SHADOW_STEPS, LEGEND_MIN_TEXT_PX, blur_shadow_layers};

    fn overlaps(a: &Edges, b: &Edges) -> bool {
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
    fn sheet_and_hint_follow_display_scaling() {
        for style in [VkStyle::Normal, VkStyle::Mono, VkStyle::Modern] {
            for dpi in [1.25_f32, 1.5, 2.0] {
                let (u1, u2) = (ref_unit(1.0, style), ref_unit(dpi, style));
                assert!((u2 - u1 * dpi).abs() < 1e-4);
                let (s1, s2) = (sheet_combo_style(u1), sheet_combo_style(u2));
                assert!((s2.glyph - s1.glyph * dpi).abs() < 0.01);
                let (h1, h2) = (side_combo_style(u1), side_combo_style(u2));
                assert!((h2.glyph - h1.glyph * dpi).abs() < 0.01);
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

    fn bar_check(
        scale: f32,
        style: VkStyle,
        client_w: f32,
        family: ControllerIconFamily,
    ) -> (Vec<&'static str>, BarLayout, f32, f32) {
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
            assert!(
                (b - (a + aw + layout.sep_w)).abs() < 0.01,
                "{what}: {:?}",
                layout.items
            );
            assert!(b > a + aw, "{what}: overlap {:?}", layout.items);
        }
        if layout.items.len() > 1 {
            let (_, first, _) = layout.items[0];
            let &(_, last, lw) = layout.items.last().unwrap();
            assert!(
                first >= l - 0.01 && last + lw <= r + 0.01,
                "{what}: outside {l}..{r}"
            );
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
                assert!(
                    first > l + 1.0,
                    "{what}: the bar is a compact centred group"
                );
                for (scale, width, must_drop) in
                    [(1.0, 1920.0, false), (FULL_SIZE_SCALE, 900.0, true)]
                {
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
        assert_eq!(
            layout.items,
            vec![(0, 65.0, 10.0), (1, 80.0, 20.0), (2, 105.0, 30.0)]
        );
        let tight = bar_layout(&[10.0, 20.0, 30.0], 5.0, 0.0, 50.0);
        assert_eq!(
            tight.items.iter().map(|i| i.0).collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[test]
    fn design_sizes_scale_with_the_unit_and_keep_the_text_floor() {
        let bar = ComboStyle::bar(1.0);
        assert_eq!((bar.glyph, bar.share, bar.gap), (38.0, 44.0, 6.0));
        assert_eq!((bar.prefix_px, bar.connector_px), (20.0, 17.0));
        assert_eq!(
            (text_px(BAR_LABEL_PX, 1.0), text_px(BAR_SEP_PX, 1.0), BAR_H),
            (19.0, 18.0, 80.0)
        );
        let sheet = SheetMetrics::at(1.0);
        assert_eq!(
            (sheet.cs.glyph, sheet.cs.share, sheet.cs.gap),
            (28.0, 32.0, 8.0)
        );
        assert_eq!(
            (sheet.header_px, sheet.action_px, sheet.cs.prefix_px),
            (18.0, 19.0, 17.0)
        );
        assert_eq!(
            (sheet.row_h, sheet.col_gap, sheet.action_gap),
            (70.0, 40.0, 24.0)
        );
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
                    bottom: strip_band_height(1.0, style)
                        + block_h
                        + 2.0 * pad_y
                        + legend_band_height(1.0, style),
                };
                let (m, cols, total) = rd.sheet_fit(style, family, ref_unit(1.0, style), panel);
                let (bw, bh) = sheet_block_size(total, &m);
                assert!(
                    bw <= panel.right + 1.0 && bh <= panel.bottom + 1.0,
                    "{style:?} {bw}x{bh}"
                );
                let fill = (bw / panel.right).max(bh / panel.bottom);
                assert!(fill >= 0.99, "{style:?} fills only {fill}");
                let lines = shortcut_sheet::max_lines() as f32;
                let content = (total / panel.right).max(lines * m.row_h / panel.bottom);
                assert!(content >= 0.92, "{style:?} content fills only {content}");
                assert!(m.u >= ref_unit(1.0, style) * 1.1, "{style:?} scale {}", m.u);
                assert!(
                    m.u > ref_unit(1.0, style),
                    "{style:?} sheet grows past the design size"
                );
                assert_eq!(cols.len(), 3);
                let (m2, _, total2) = rd.sheet_fit(
                    style,
                    family,
                    ref_unit(1.5, style),
                    D2D_RECT_F {
                        right: panel.right * 1.5,
                        bottom: panel.bottom * 1.5,
                        ..panel
                    },
                );
                assert!(
                    (m2.u - m.u * 1.5).abs() < m.u * 0.05,
                    "{style:?} {} vs {}",
                    m2.u,
                    m.u
                );
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
    fn sheet_hint_uses_the_pad_family_svg_glyphs() {
        let ps = ControllerIconFamily::from_label("DualSense Wireless Controller");
        let xb = ControllerIconFamily::from_label("Xbox Wireless Controller");
        let hint = shortcut_sheet::SHEET_HINT.keys;
        assert_eq!(
            combo_parts(hint, ps),
            vec![
                ComboPart::Prefix("Hold"),
                ComboPart::Icon(VkIcon::SelectPs5, true)
            ]
        );
        assert_eq!(
            combo_parts(hint, xb),
            vec![
                ComboPart::Prefix("Hold"),
                ComboPart::Icon(VkIcon::SelectXbox, true)
            ]
        );
    }

    #[test]
    fn every_legend_and_sheet_button_is_an_svg_icon_for_both_families() {
        let families = [ControllerIconFamily::Ps5, ControllerIconFamily::Xbox];
        let sheet = super::super::shortcut_sheet::GROUPS
            .iter()
            .chain(super::super::shortcut_sheet::MODERN_GROUPS.iter())
            .flat_map(|g| g.rows.iter());
        for row in sheet
            .chain(shortcut_sheet::LEGEND.iter())
            .chain(std::iter::once(&shortcut_sheet::SHEET_HINT))
        {
            for &token in row.keys {
                if let TipToken::Button(b) = token {
                    for family in families {
                        let icon = family.hint_icon(b).expect(b);
                        assert!(icon.is_controller_tip(), "{b} {family:?}");
                        assert!(icon.svg().contains("<svg"), "{b} {family:?}");
                        assert_eq!(tip_item(token, family), Some(TipItem::Icon(icon)));
                        assert!(
                            combo_parts(&[token], family)
                                == vec![ComboPart::Icon(icon, b == "SELECT")]
                        );
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
            ControllerIconFamily::from_label("Wireless Controller"),
            ControllerIconFamily::Ps5
        );
        assert_eq!(
            ControllerIconFamily::from_label("Sony Interactive Entertainment Controller"),
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

    fn blank_rows(n: usize) -> Vec<crate::vk_nav::KeyRow> {
        (0..n)
            .map(|_| crate::vk_nav::KeyRow { keys: Vec::new() })
            .collect()
    }

    #[test]
    fn modern_dock_keeps_the_same_key_size_as_normal() {
        let scale_w = REF_MON_W * 1.25;
        let normal = blank_rows(4);
        let modern = blank_rows(5);
        let (nw, nh, _) = key_metrics(scale_w, f32::INFINITY, &normal, 0.0, VkStyle::Normal);
        let (mw, mh, _) = key_metrics(scale_w, f32::INFINITY, &modern, 0.0, VkStyle::Modern);
        assert!((nw - mw).abs() < 0.01);
        assert!((nh - mh).abs() < 0.01);
        let top = strip_band_height(1.25, VkStyle::Modern);
        let dock = dock_height_for_full_keys(scale_w, &modern, top, VkStyle::Modern);
        let fitted = fitted_key_height(scale_w, dock, &modern, top, VkStyle::Modern);
        assert!((fitted - mh).abs() < 0.5, "{fitted} vs {mh}");
        let squeezed = fitted_key_height(scale_w, 384.0, &modern, top, VkStyle::Modern);
        assert!(squeezed < mh * 0.9);
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
            assert!(
                (geom.top - FLOATING_PANEL_INSET).abs() < 1.0,
                "{style:?} {geom:?}"
            );
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
        assert_eq!(
            styled_glyph(mono, &KeyAction::Symbols, "?123".into()),
            "123"
        );
        assert_eq!(
            styled_glyph(normal, &KeyAction::Symbols, "?123".into()),
            "?123"
        );
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

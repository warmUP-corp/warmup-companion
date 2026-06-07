//! D3D11 + DXGI composition swapchain + D2D + DirectComposition — Joyxoff `FUN_0041e670`.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::time::Instant;

use windows::core::{w, Interface};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1,
    ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_ROUNDED_RECT,
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
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_METRICS,
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

fn configure_d2d_quality(ctx: &ID2D1DeviceContext) {
    // Default D2D text path can look aliased on our DXGI composition target.
    let _ = unsafe { ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE) };
    let _ = unsafe { ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
}

fn chip_width(word: &str) -> f32 {
    let n = word.chars().count() as f32;
    (n * 7.8 + CHIP_PAD_X * 2.0).clamp(CHIP_MIN_W, 200.0)
}

unsafe fn draw_candidate_strip(
    ctx: &ID2D1DeviceContext,
    cw: f32,
    strip: &crate::vk_predict::StripState,
    key_brush: &ID2D1SolidColorBrush,
    accent_brush: &ID2D1SolidColorBrush,
    text_brush: &ID2D1SolidColorBrush,
    sel_text_brush: &ID2D1SolidColorBrush,
    chip_format: &IDWriteTextFormat,
    pal: &VkPalette,
) -> Result<(), String> {
    let mut widths = [0.0f32; 3];
    let mut count = 0usize;
    for (i, word) in strip.visible.iter().enumerate() {
        if word.is_empty() {
            continue;
        }
        widths[i] = chip_width(word);
        count += 1;
    }
    if count == 0 {
        return Ok(());
    }

    let total_w: f32 = widths.iter().sum::<f32>() + CHIP_GAP * (count.saturating_sub(1) as f32);
    let mut x = (cw - total_w) / 2.0;
    let outline = solid_brush(&ctx, colorref_alpha(pal.text, 0.28))?;
    let radius = CHIP_H * 0.42;

    for (i, word) in strip.visible.iter().enumerate() {
        if word.is_empty() {
            continue;
        }
        let w = widths[i];
        let selected = strip.engaged && i == strip.highlight_slot;
        let rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: CHIP_TOP,
                right: x + w,
                bottom: CHIP_TOP + CHIP_H,
            },
            radiusX: radius,
            radiusY: radius,
        };
        let (fill, label) = if selected {
            (accent_brush, sel_text_brush)
        } else {
            (key_brush, text_brush)
        };
        ctx.FillRoundedRectangle(&rect, fill);
        if !selected {
            ctx.DrawRoundedRectangle(&rect, &outline, 1.25, None);
        }
        let label_rect = D2D_RECT_F {
            left: rect.rect.left + CHIP_LABEL_INSET_X,
            top: rect.rect.top + CHIP_LABEL_INSET_Y,
            right: rect.rect.right - CHIP_LABEL_INSET_X,
            bottom: rect.rect.bottom - CHIP_LABEL_INSET_Y,
        };
        let wide: Vec<u16> = word.encode_utf16().collect();
        ctx.DrawText(
            &wide,
            chip_format,
            &label_rect,
            label,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        x += w + CHIP_GAP;
    }
    Ok(())
}

pub struct VkPalette {
    pub bg: u32,
    pub key: u32,
    pub accent: u32,
    pub text: u32,
    /// Label colour on the selected key (Joyxoff inverts it — `DAT_004a4964`).
    pub sel_text: u32,
    /// Key outline colour (matches the webview VK border).
    pub border: u32,
}

pub struct VkRenderer {
    width: u32,
    height: u32,
    swapchain: IDXGISwapChain1,
    d2d_context: ID2D1DeviceContext,
    d2d_target: ID2D1Bitmap1,
    dwrite: IDWriteFactory,
    text_format: IDWriteTextFormat,
    glyph_format: IDWriteTextFormat,
    /// Small font for sublabels, badges, and the legend strip.
    hint_format: IDWriteTextFormat,
    /// Fixed-size labels on prediction chips (not scaled with key height).
    chip_format: IDWriteTextFormat,
    sublabel_format: IDWriteTextFormat,
    icon_cache: HashMap<IconCacheKey, ID2D1Bitmap1>,
    prompt_started: Instant,
    _d3d: ID3D11Device,
    _d2d_device: ID2D1Device,
    _dcomp_device: IDCompositionDevice,
    // Keep the composition target + visual alive for the window's lifetime. Dropping
    // them releases the HWND<->visual binding, so the window shows nothing.
    _comp_target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

/// Joyxoff reference metrics on a 1920px-wide monitor: 92x68 px keys, 4 px gap,
/// 6.8 px corner radius (`_DAT_00494d5c`/`_DAT_00494d4c`/`_DAT_00494cb0`/`_DAT_00494ce8`).
const REF_MON_W: f32 = 1920.0;
const REF_KEY_W: f32 = 92.0;
const KEY_ASPECT: f32 = 68.0 / 92.0;
const REF_GAP: f32 = 4.0;
/// Corner radius as a fraction of key height (Joyxoff 6.8/68).
const RADIUS_FRAC: f32 = 6.8 / 68.0;
/// Prefix-completion candidate strip (reclaims former legend/tooltip row).
pub const CANDIDATE_STRIP_H: f32 = 70.0;

/// Uniform padding between the floating card's rounded edge and its key grid.
pub const FLOATING_PAD: f32 = 18.0;

const CHIP_H: f32 = 48.0;
const CHIP_GAP: f32 = 10.0;
const CHIP_PAD_X: f32 = 14.0;
const CHIP_MIN_W: f32 = 58.0;
const CHIP_TOP: f32 = 11.0;
const CHIP_LABEL_INSET_X: f32 = 8.0;
const CHIP_LABEL_INSET_Y: f32 = 4.0;
/// Chip label size in DIPs — independent of key label scaling.
const CHIP_FONT_PX: f32 = 14.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum VkIcon {
    Backspace,
    Close,
    Enter,
    MicOff,
    Space,
    Paste,
    Shift,
    ShiftFilled,
    Caps,
    CapsFilled,
    /// Caret-move arrow keys (Lucide chevrons).
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    /// PlayStation L3 (left-stick click) chip — keeps its native colors (no
    /// `currentColor`), extracted from the controller-icon atlas.
    L3,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct IconCacheKey {
    icon: VkIcon,
    px: u32,
    color: u32,
}

impl VkIcon {
    fn svg(self) -> &'static str {
        match self {
            VkIcon::Backspace => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 5a2 2 0 0 0-1.344.519l-6.328 5.74a1 1 0 0 0 0 1.481l6.328 5.741A2 2 0 0 0 10 19h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2z"/><path d="m12 9 6 6"/><path d="m18 9-6 6"/></svg>"#
            }
            VkIcon::Close => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#
            }
            VkIcon::Enter => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 4v7a4 4 0 0 1-4 4H4"/><path d="m9 10-5 5 5 5"/></svg>"#
            }
            VkIcon::MicOff => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19v3"/><path d="M15 9.34V5a3 3 0 0 0-5.68-1.33"/><path d="M16.95 16.95A7 7 0 0 1 5 12v-2"/><path d="M18.89 13.23A7 7 0 0 0 19 12v-2"/><path d="m2 2 20 20"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12"/></svg>"#
            }
            VkIcon::Space => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 17v1c0 .5-.5 1-1 1H3c-.5 0-1-.5-1-1v-1"/></svg>"#
            }
            VkIcon::Paste => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 14h10"/><path d="M16 4h2a2 2 0 0 1 2 2v1.344"/><path d="m17 18 4-4-4-4"/><path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 1.793-1.113"/><rect x="8" y="2" width="8" height="4" rx="1"/></svg>"#
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
            // Native-colored chip; has no `currentColor`, so the palette swap in
            // `draw_svg_icon` is a no-op and it keeps its PlayStation look.
            VkIcon::L3 => include_str!("../../controller-icons/l3.svg"),
        }
    }
}

/// Top chrome always reserved so keys do not shift when chips appear.
pub fn top_chrome_inset() -> f32 {
    CANDIDATE_STRIP_H
}

/// Natural bounding box `(width, height)` of the key grid at `scale_w`, excluding
/// card padding and top chrome. Lets the floating card be sized to wrap keys that
/// render at the same scale as the docked bar.
pub fn grid_size(scale_w: f32, rows: &[KeyRow]) -> (f32, f32) {
    let (kw, kh, gap) = key_metrics(scale_w, f32::INFINITY, rows, 0.0);
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

/// Compute every key's rect for the given client size + layout rows. Each key's
/// width is `span * kw` so the wide space bar covers several key-units (Joyxoff
/// `FUN_00463bd0` width = span*keyW + (span-1)*gap).
fn row_pixel_width(row: &KeyRow, kw: f32, gap: f32) -> f32 {
    row.keys
        .iter()
        .map(|k| key_width(kw, gap, k.span))
        .sum::<f32>()
        + gap * (row.keys.len().saturating_sub(1) as f32)
}

/// Which key in each row absorbs width slack so every row shares the same left/right edge.
fn row_stretch_key(row_index: usize, key_count: usize) -> usize {
    match row_index {
        0 => key_count - 1, // Backspace
        1 => 0,             // Tab
        2 => key_count - 1, // Enter
        3 => key_count - 1, // Right Shift
        4 => 0,             // Space
        _ => key_count.saturating_sub(1),
    }
}

/// `scale_w` drives key size (always the monitor width, so floating keys match the
/// docked bar); `client_w`/`client_h` drive centering within the target window.
pub fn key_rects(
    client_w: f32,
    client_h: f32,
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
) -> Vec<KeyRect> {
    let (kw, kh, gap) = key_metrics(scale_w, client_h, rows, top_inset);
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
        let row_w = row_pixel_width(row, kw, gap);
        let extra = grid_w - row_w;
        let stretch = row_stretch_key(ri, row.keys.len());
        let mut left = grid_left;
        for (ci, key) in row.keys.iter().enumerate() {
            let mut w = key_width(kw, gap, key.span);
            if ci == stretch {
                w = (w + extra).max(kw);
            }
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
}

/// One immutable snapshot of everything the VK renderer needs for a frame.
/// `render_frame` assembles it from a single logical read of nav/predict state;
/// `draw` consumes only `&VkFrame` and performs no global reads, so the
/// selection/glyph-branch logic is testable without a NAV lock or a D2D device.
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
}

/// Glyph for the Shift key (the Shift-action key reflects `shift`).
fn shift_icon(shift: bool) -> VkIcon {
    if shift {
        VkIcon::CapsFilled
    } else {
        VkIcon::Caps
    }
}

/// Glyph for the CapsLock key (the CapsLock-action key reflects `caps`).
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

        // NVIDIA's D3D11 user-mode driver (nvwgf2umx.dll) faults with 0xC0000005
        // when driven on the Winlogon secure desktop — the GPU context there is
        // unreliable (confirmed via minidump). On the secure desktop, render with
        // the WARP software rasterizer, which never loads the vendor UMD. Userland
        // keeps hardware for perf. Either way, fall back to the other on failure.
        let on_secure = crate::win::surface::thread().is_some_and(|s| s.is_winlogon());
        let d3d = create_d3d_device(on_secure)?;
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
        // Joyxoff `FUN_00463130`: Segoe UI labels; Segoe MDL2 Assets when icon row enabled.
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
        let chip_format = dwrite
            .CreateTextFormat(
                w!("Segoe UI"),
                &fonts,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                CHIP_FONT_PX,
                &locale,
            )
            .map_err(|e| format!("CreateTextFormat (chip): {e}"))?;
        let sublabel_format = dwrite
            .CreateTextFormat(
                w!("Segoe UI"),
                &fonts,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                (label_px * 0.55).clamp(10.0, 22.0),
                &locale,
            )
            .map_err(|e| format!("CreateTextFormat (sublabel): {e}"))?;

        // Centre labels in their key rects (DWrite defaults to top-left).
        for f in [&text_format, &glyph_format] {
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        // Badges/legend: horizontally centred, anchored to the top of their rect.
        let _ = hint_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = hint_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        let _ = chip_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = chip_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = sublabel_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = sublabel_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);

        Ok(Self {
            width,
            height,
            swapchain,
            d2d_context,
            d2d_target,
            dwrite,
            text_format,
            glyph_format,
            hint_format,
            chip_format,
            sublabel_format,
            icon_cache: HashMap::new(),
            prompt_started: Instant::now(),
            _d3d: d3d,
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
        self.d2d_context.SetTarget(None);
        self.swapchain
            .ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )
            .map_err(|e| format!("ResizeBuffers: {e}"))?;
        self.width = width;
        self.height = height;
        self.d2d_target = bind_d2d_target(&self.d2d_context, &self.swapchain)?;
        Ok(())
    }

    unsafe fn draw_svg_icon(
        &mut self,
        icon: VkIcon,
        rect: D2D_RECT_F,
        color: u32,
    ) -> Result<(), String> {
        let h = rect.bottom - rect.top;
        let draw_px = match icon {
            VkIcon::L3 => (h * 0.88).round().clamp(24.0, 64.0),
            _ => (h * 0.5).round().clamp(16.0, 96.0),
        };
        let raster_px = match icon {
            VkIcon::L3 => (draw_px * 3.0).round().clamp(72.0, 192.0),
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
            let scale = raster_px as f32 / 24.0;
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
            1.0,
            D2D1_INTERPOLATION_MODE_LINEAR,
            None,
            None,
        );
        Ok(())
    }

    pub unsafe fn draw(&mut self, frame: &VkFrame) -> Result<(), String> {
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
        } = *frame;
        let cw = self.width as f32;
        let ch = self.height as f32;

        self.d2d_context.BeginDraw();

        let rects = key_rects(cw, ch, scale_w, rows, top_inset);

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
            let radius = (ch * 0.06).clamp(14.0, 30.0);
            let panel = D2D_RECT_F {
                left: 1.0,
                top: 1.0,
                right: cw - 1.0,
                bottom: ch - 1.0,
            };
            let rounded = D2D1_ROUNDED_RECT {
                rect: panel,
                radiusX: radius,
                radiusY: radius,
            };
            let bg_brush = solid_brush(&self.d2d_context, colorref(pal.bg))?;
            let panel_border = solid_brush(&self.d2d_context, colorref(pal.border))?;
            self.d2d_context.FillRoundedRectangle(&rounded, &bg_brush);
            self.d2d_context
                .DrawRoundedRectangle(&rounded, &panel_border, 1.5, None);
            self.d2d_context.PushAxisAlignedClip(
                &panel,
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        } else {
            self.d2d_context.Clear(Some(&colorref(pal.bg)));
        }

        let key_brush = solid_brush(&self.d2d_context, colorref(pal.key))?;
        let accent_brush = solid_brush(&self.d2d_context, colorref(pal.accent))?;
        let text_brush = solid_brush(&self.d2d_context, colorref(pal.text))?;
        let sel_text_brush = solid_brush(&self.d2d_context, colorref(pal.sel_text))?;
        let border_brush = solid_brush(&self.d2d_context, colorref(pal.border))?;

        if let Some(strip) = candidates {
            draw_candidate_strip(
                &self.d2d_context,
                cw,
                strip,
                &key_brush,
                &accent_brush,
                &text_brush,
                &sel_text_brush,
                &self.chip_format,
                pal,
            )?;
        }

        for kr in &rects {
            let key = &rows[kr.pos.row].keys[kr.pos.col];
            let selected = sel.row == kr.pos.row && sel.col == kr.pos.col;
            // Radius scales with key height (Joyxoff 6.8px @ 68px key).
            let radius = (kr.bottom - kr.top) * RADIUS_FRAC;
            let rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: kr.left,
                    top: kr.top,
                    right: kr.right,
                    bottom: kr.bottom,
                },
                radiusX: radius,
                radiusY: radius,
            };
            // Selected key: solid accent fill + inverted label (Joyxoff `FUN_004676b0`).
            let (fill, label_brush) = if selected {
                (&accent_brush, &sel_text_brush)
            } else {
                (&key_brush, &text_brush)
            };
            let label_color = if selected { pal.sel_text } else { pal.text };
            self.d2d_context.FillRoundedRectangle(&rect, fill);
            // Outline non-selected keys to match the webview VK border; the selected key keeps a
            // clean accent fill.
            if !selected {
                self.d2d_context
                    .DrawRoundedRectangle(&rect, &border_brush, 1.25, None);
            }

            if let Some(sub) = &key.sublabel {
                let kh = kr.bottom - kr.top;
                let sub_rect = D2D_RECT_F {
                    left: kr.left + 2.0,
                    top: kr.top + 2.0,
                    right: kr.right - 2.0,
                    bottom: kr.top + kh * 0.45,
                };
                let w: Vec<u16> = sub.encode_utf16().collect();
                self.d2d_context.DrawText(
                    &w,
                    &self.sublabel_format,
                    &sub_rect,
                    label_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            if matches!(key.action, KeyAction::VoiceInput) {
                let disabled_color = colorref_mix(label_color, pal.key, 0.42);
                self.draw_svg_icon(VkIcon::MicOff, rect.rect, disabled_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_SPACE)
            {
                self.draw_svg_icon(VkIcon::Space, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_BACK)
            {
                self.draw_svg_icon(VkIcon::Backspace, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN)
            {
                self.draw_svg_icon(VkIcon::Enter, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_LEFT)
            {
                self.draw_svg_icon(VkIcon::ChevronLeft, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_RIGHT)
            {
                self.draw_svg_icon(VkIcon::ChevronRight, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_UP)
            {
                self.draw_svg_icon(VkIcon::ChevronUp, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Vk(vk) if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_DOWN)
            {
                self.draw_svg_icon(VkIcon::ChevronDown, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Paste) {
                self.draw_svg_icon(VkIcon::Paste, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::CloseVk) {
                self.draw_svg_icon(VkIcon::Close, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Shift) {
                self.draw_svg_icon(shift_icon(modifiers.shift), rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::CapsLock) {
                self.draw_svg_icon(caps_icon(modifiers.caps), rect.rect, label_color)?;
            } else {
                let (glyph, symbol_font) = key_glyph(key);
                if !glyph.is_empty() {
                    let format = if symbol_font {
                        &self.glyph_format
                    } else {
                        &self.text_format
                    };
                    let kh = kr.bottom - kr.top;
                    let label_rect = if key.sublabel.is_some() {
                        D2D_RECT_F {
                            left: rect.rect.left,
                            top: kr.top + kh * 0.35,
                            right: rect.rect.right,
                            bottom: rect.rect.bottom,
                        }
                    } else {
                        rect.rect
                    };
                    let wide: Vec<u16> = glyph.encode_utf16().collect();
                    self.d2d_context.DrawText(
                        &wide,
                        format,
                        &label_rect,
                        label_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            // Per-key controller-button badge in the top-left corner.
            if let Some(hint) = key_hint(key) {
                let kh = kr.bottom - kr.top;
                let badge = D2D_RECT_F {
                    left: kr.left + 2.0,
                    top: kr.top + 2.0,
                    right: kr.right - 2.0,
                    bottom: kr.top + kh * 0.5,
                };
                let badge_brush = if selected {
                    &sel_text_brush
                } else {
                    &accent_brush
                };
                let w: Vec<u16> = hint.encode_utf16().collect();
                self.d2d_context.DrawText(
                    &w,
                    &self.hint_format,
                    &badge,
                    badge_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }

        drop(key_brush);
        drop(accent_brush);
        drop(text_brush);
        drop(sel_text_brush);

        if floating {
            self.d2d_context.PopAxisAlignedClip();
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

    /// Measure a text run's width in DIPs at [`Self::text_format`].
    unsafe fn measure_text(&self, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout: Option<IDWriteTextLayout> = self
            .dwrite
            .CreateTextLayout(&wide, &self.text_format, f32::MAX, f32::MAX)
            .ok();
        let Some(layout) = layout else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_err() {
            return 0.0;
        }
        m.widthIncludingTrailingWhitespace
    }

    /// Draw an AirPods-style connection card with a shaded controller model.
    /// Kept D2D-only so the secure-desktop service path does not need asset IO or
    /// a separate 3D runtime.
    pub unsafe fn draw_connected_prompt(
        &mut self,
        bg: u32,
        border: u32,
        text_color: u32,
        title: &str,
        status: &str,
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        let _ = self
            .text_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = self
            .text_format
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = self
            .hint_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = self
            .hint_format
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let t = self.prompt_started.elapsed().as_secs_f32();
        let intro = (t / 0.62).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - intro).powi(3);
        let pulse = (t * 0.72).fract();
        let pulse_alpha = (1.0 - pulse).powi(2);
        let scale = 0.90 + 0.10 * eased;
        let transform = Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: cw * (1.0 - scale) * 0.5,
            M32: ch * (1.0 - scale) * 0.5,
        };
        self.d2d_context.SetTransform(&transform);

        let panel = D2D_RECT_F {
            left: 5.0,
            top: 5.0,
            right: cw - 5.0,
            bottom: ch - 5.0,
        };
        let rounded = D2D1_ROUNDED_RECT {
            rect: panel,
            radiusX: 28.0,
            radiusY: 28.0,
        };
        let glow = colorref_mix(0x00FFFFFF, border, 0.45);
        let bg_brush = solid_brush(&self.d2d_context, colorref_alpha(bg, 0.94))?;
        let border_brush = solid_brush(&self.d2d_context, colorref_alpha(glow, 0.84))?;
        let halo_brush = solid_brush(&self.d2d_context, colorref_alpha(glow, 0.22 * pulse_alpha))?;
        self.d2d_context.FillRoundedRectangle(&rounded, &bg_brush);
        self.d2d_context
            .DrawRoundedRectangle(&rounded, &halo_brush, 3.0 + 12.0 * pulse, None);
        self.d2d_context
            .DrawRoundedRectangle(&rounded, &border_brush, 1.2, None);

        let model_cx = cw * 0.5;
        let model_y = 54.0 - 7.0 * (1.0 - eased);
        let shadow = solid_brush(&self.d2d_context, colorref_alpha(0x00000000, 0.26))?;
        let body = solid_brush(
            &self.d2d_context,
            colorref(colorref_mix(0x00FFFFFF, bg, 0.70)),
        )?;
        let face = solid_brush(
            &self.d2d_context,
            colorref(colorref_mix(0x00FFFFFF, bg, 0.86)),
        )?;
        let dim = solid_brush(&self.d2d_context, colorref_alpha(text_color, 0.34))?;
        let accent = solid_brush(&self.d2d_context, colorref(glow))?;

        let shadow_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx - 78.0,
                top: model_y + 50.0,
                right: model_cx + 78.0,
                bottom: model_y + 66.0,
            },
            radiusX: 18.0,
            radiusY: 18.0,
        };
        self.d2d_context.FillRoundedRectangle(&shadow_rect, &shadow);

        let left_grip = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx - 106.0,
                top: model_y + 7.0,
                right: model_cx - 42.0,
                bottom: model_y + 77.0,
            },
            radiusX: 30.0,
            radiusY: 30.0,
        };
        let right_grip = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx + 42.0,
                top: model_y + 7.0,
                right: model_cx + 106.0,
                bottom: model_y + 77.0,
            },
            radiusX: 30.0,
            radiusY: 30.0,
        };
        let center = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx - 68.0,
                top: model_y,
                right: model_cx + 68.0,
                bottom: model_y + 61.0,
            },
            radiusX: 26.0,
            radiusY: 26.0,
        };
        self.d2d_context.FillRoundedRectangle(&left_grip, &body);
        self.d2d_context.FillRoundedRectangle(&right_grip, &body);
        self.d2d_context.FillRoundedRectangle(&center, &face);
        self.d2d_context
            .DrawRoundedRectangle(&center, &border_brush, 1.0, None);

        let round_dot = |x: f32, y: f32, r: f32| D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x - r,
                top: y - r,
                right: x + r,
                bottom: y + r,
            },
            radiusX: r,
            radiusY: r,
        };
        self.d2d_context
            .FillRoundedRectangle(&round_dot(model_cx - 34.0, model_y + 49.0, 14.0), &dim);
        self.d2d_context
            .FillRoundedRectangle(&round_dot(model_cx + 34.0, model_y + 49.0, 14.0), &dim);
        for (x, y) in [
            (model_cx + 75.0, model_y + 25.0),
            (model_cx + 91.0, model_y + 39.0),
            (model_cx + 59.0, model_y + 39.0),
            (model_cx + 75.0, model_y + 53.0),
        ] {
            self.d2d_context
                .FillRoundedRectangle(&round_dot(x, y, 7.0), &accent);
        }
        let dpad_h = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx - 91.0,
                top: model_y + 35.0,
                right: model_cx - 55.0,
                bottom: model_y + 47.0,
            },
            radiusX: 6.0,
            radiusY: 6.0,
        };
        let dpad_v = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx - 79.0,
                top: model_y + 23.0,
                right: model_cx - 67.0,
                bottom: model_y + 59.0,
            },
            radiusX: 6.0,
            radiusY: 6.0,
        };
        self.d2d_context.FillRoundedRectangle(&dpad_h, &dim);
        self.d2d_context.FillRoundedRectangle(&dpad_v, &dim);

        let ring = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: model_cx - 124.0 - 18.0 * pulse,
                top: model_y - 16.0 - 18.0 * pulse,
                right: model_cx + 124.0 + 18.0 * pulse,
                bottom: model_y + 90.0 + 18.0 * pulse,
            },
            radiusX: 58.0 + 18.0 * pulse,
            radiusY: 58.0 + 18.0 * pulse,
        };
        self.d2d_context
            .DrawRoundedRectangle(&ring, &halo_brush, 2.0, None);

        let title_w: Vec<u16> = title.encode_utf16().collect();
        let status_w: Vec<u16> = status.encode_utf16().collect();
        let text_brush = solid_brush(&self.d2d_context, colorref(text_color))?;
        let sub_brush = solid_brush(&self.d2d_context, colorref_alpha(text_color, 0.70))?;
        self.d2d_context.DrawText(
            &title_w,
            &self.text_format,
            &D2D_RECT_F {
                left: 24.0,
                top: ch - 78.0,
                right: cw - 24.0,
                bottom: ch - 42.0,
            },
            &text_brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        self.d2d_context.DrawText(
            &status_w,
            &self.hint_format,
            &D2D_RECT_F {
                left: 24.0,
                top: ch - 42.0,
                right: cw - 24.0,
                bottom: ch - 16.0,
            },
            &sub_brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );

        let identity = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };
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

    /// Draw the "Press [L3] to open keyboard" prompt: a rounded pill filling the
    /// client area, with `prefix` · L3 chip · `suffix` laid out left→right and
    /// centered. The L3 chip keeps its native colors; text uses `text_color`.
    pub unsafe fn draw_prompt(
        &mut self,
        bg: u32,
        border: u32,
        text_color: u32,
        prefix: &str,
        suffix: &str,
        show_l3: bool,
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        // Segments flow left to right, top-aligned to a shared baseline band.
        let _ = self
            .text_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = self
            .text_format
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let t = self.prompt_started.elapsed().as_secs_f32();
        let pulse = (t * 0.33).fract();
        let pulse_alpha = (1.0 - pulse).powi(2);
        let scale_phase = (t * std::f32::consts::TAU * 0.33).sin() * 0.5 + 0.5;
        let scale = 0.985 + 0.015 * scale_phase;
        let transform = Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: cw * (1.0 - scale) * 0.5,
            M32: ch * (1.0 - scale) * 0.5,
        };
        self.d2d_context.SetTransform(&transform);

        // Rounded pill fills the window minus a hairline for the antialiased stroke.
        let radius = (ch * 0.5 - 2.0).max(8.0);
        let panel = D2D_RECT_F {
            left: 4.0,
            top: 4.0,
            right: cw - 4.0,
            bottom: ch - 4.0,
        };
        let rounded = D2D1_ROUNDED_RECT {
            rect: panel,
            radiusX: radius,
            radiusY: radius,
        };
        let glow = colorref_mix(0x00FFFFFF, border, 0.38);
        let bg_brush = solid_brush(&self.d2d_context, colorref(bg))?;
        let glow_brush = solid_brush(&self.d2d_context, colorref_alpha(glow, 0.30 * pulse_alpha))?;
        let border_brush = solid_brush(&self.d2d_context, colorref(glow))?;
        self.d2d_context.FillRoundedRectangle(&rounded, &bg_brush);
        self.d2d_context
            .DrawRoundedRectangle(&rounded, &glow_brush, 2.0 + 8.0 * pulse, None);
        self.d2d_context
            .DrawRoundedRectangle(&rounded, &border_brush, 1.5, None);

        // Chip is a square sized to the pill height; text runs sit either side.
        let chip = (ch * 0.70).clamp(26.0, 48.0);
        let gap = 8.0;
        let w_prefix = self.measure_text(prefix);
        let w_suffix = self.measure_text(suffix);
        let total = if show_l3 {
            w_prefix + gap + chip + gap + w_suffix
        } else {
            w_prefix
        };
        let mut x = ((cw - total) * 0.5).max(0.0);
        let text_brush = solid_brush(&self.d2d_context, colorref(text_color))?;

        // Prefix.
        let pre: Vec<u16> = prefix.encode_utf16().collect();
        self.d2d_context.DrawText(
            &pre,
            &self.text_format,
            &D2D_RECT_F {
                left: x,
                top: 0.0,
                right: x + w_prefix,
                bottom: ch,
            },
            &text_brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        if show_l3 {
            x += w_prefix + gap;

            // L3 chip (native colors; the passed color is ignored by the no-op swap).
            let chip_rect = D2D_RECT_F {
                left: x,
                top: (ch - chip) * 0.5,
                right: x + chip,
                bottom: (ch + chip) * 0.5,
            };
            self.draw_svg_icon(VkIcon::L3, chip_rect, text_color)?;
            x += chip + gap;
        }

        // Suffix.
        if !suffix.is_empty() {
            let suf: Vec<u16> = suffix.encode_utf16().collect();
            self.d2d_context.DrawText(
                &suf,
                &self.text_format,
                &D2D_RECT_F {
                    left: x,
                    top: 0.0,
                    right: x + w_suffix,
                    bottom: ch,
                },
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }

        let identity = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };
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

/// Returns `(key_width, key_height, gap)` in px. Keys are sized from the window
/// width at the Joyxoff 92px reference (scaled by `client_w/1920`), holding the
/// 92:68 aspect, then shrunk to fit all rows in the docked bar's height.
fn key_metrics(scale_w: f32, client_h: f32, rows: &[KeyRow], top_inset: f32) -> (f32, f32, f32) {
    let scale = (scale_w / REF_MON_W).max(0.05);
    let mut kw = REF_KEY_W * scale;
    let mut gap = REF_GAP * scale;
    let mut kh = kw * KEY_ASPECT;
    let n = rows.len().max(1) as f32;
    // Fit below top chrome (chips when active); shrink if rows overflow.
    let avail = (client_h - top_inset - kh * 0.25).max(1.0);
    let block = n * kh + (n - 1.0) * gap;
    if block > avail {
        let s = avail / block;
        kh *= s;
        gap *= s;
        kw = kh / KEY_ASPECT;
    }
    (kw, kh, gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_glyphs_map_verbatim() {
        // Non-obvious crossed mapping the renderer must preserve:
        // the Shift key reflects `shift`; the CapsLock key reflects `caps`.
        assert_eq!(shift_icon(true), VkIcon::CapsFilled);
        assert_eq!(shift_icon(false), VkIcon::Caps);
        assert_eq!(caps_icon(true), VkIcon::ShiftFilled);
        assert_eq!(caps_icon(false), VkIcon::Shift);
    }
}

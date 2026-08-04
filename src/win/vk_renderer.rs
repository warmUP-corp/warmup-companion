//! D3D11 + DXGI composition swapchain + D2D + DirectComposition renderer.

use std::collections::{HashMap, HashSet};
use std::mem::ManuallyDrop;
use std::time::Instant;

use windows::core::{w, Interface};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1,
    ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE,
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

use crate::vk_nav::{KeyAction, KeyCell, KeyPos, KeyRow};
#[cfg(feature = "gamepad")]
use crate::{
    controller_shortcuts::{
        DesktopActionKind, LaunchableApp, Shortcut, WorkspaceWindowCandidate, MAPPABLE_BUTTONS,
    },
    gamepad_backend::Button,
};

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

fn chip_width(word: &str) -> f32 {
    let n = word.chars().count() as f32;
    (n * 7.8 + CHIP_PAD_X * 2.0).clamp(CHIP_MIN_W, 200.0)
}

unsafe fn draw_candidate_strip(
    ctx: &ID2D1DeviceContext,
    cw: f32,
    strip: &crate::vk_predict::StripState,
    accent_brush: &ID2D1SolidColorBrush,
    text_brush: &ID2D1SolidColorBrush,
    sel_text_brush: &ID2D1SolidColorBrush,
    chip_format: &IDWriteTextFormat,
    hint_format: &IDWriteTextFormat,
    pal: &VkPalette,
    controller_icons: ControllerIconFamily,
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
    let chips_left = (cw - total_w) / 2.0;
    let outline = solid_brush(ctx, colorref_alpha(pal.text, 0.22))?;
    let hint_fill = solid_brush(ctx, colorref_alpha(pal.text, 0.10))?;
    let hint_text = solid_brush(ctx, colorref_alpha(pal.text, 0.72))?;

    // One pill in the band reserved above the keys (the key layout leaves room, so
    // the keyboard never shifts). Elevated surface + border + a soft offset shadow
    // so it reads as a distinct suggestion bar sitting above the keys.
    let pill = D2D_RECT_F {
        left: chips_left - CHIP_PAD_X,
        top: CHIP_TOP,
        right: chips_left + total_w + CHIP_PAD_X,
        bottom: CHIP_TOP + CHIP_H,
    };
    let pill_radius = (pill.bottom - pill.top) * 0.5;
    let rounded = |r: D2D_RECT_F| D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: pill_radius,
        radiusY: pill_radius,
    };
    let shadow = solid_brush(ctx, colorref_alpha(0x000000, 0.30))?;
    ctx.FillRoundedRectangle(
        &rounded(D2D_RECT_F {
            top: pill.top + 3.0,
            bottom: pill.bottom + 3.0,
            ..pill
        }),
        &shadow,
    );
    let surface = solid_brush(ctx, colorref(mix_color(0xFFFFFF, pal.key, 0.10)))?;
    let border = solid_brush(ctx, colorref(pal.border))?;
    ctx.FillRoundedRectangle(&rounded(pill), &surface);
    ctx.DrawRoundedRectangle(&rounded(pill), &border, 1.25, None);

    // LB / RB cycle hints flank the pill.
    draw_shortcut_pill(
        ctx,
        "LB",
        controller_icons.hint_icon("LB"),
        pill.left - HINT_PILL_W - HINT_GAP,
        &hint_fill,
        &outline,
        &hint_text,
        hint_format,
    )?;
    draw_shortcut_pill(
        ctx,
        "RB",
        controller_icons.hint_icon("RB"),
        pill.right + HINT_GAP,
        &hint_fill,
        &outline,
        &hint_text,
        hint_format,
    )?;

    // Words inside the pill; the highlighted one gets an accent inner fill.
    let inner_radius = CHIP_H * 0.42;
    let mut x = chips_left;
    for (i, word) in strip.visible.iter().enumerate() {
        if word.is_empty() {
            continue;
        }
        let w = widths[i];
        let selected = strip.engaged && i == strip.highlight_slot;
        let slot = D2D_RECT_F {
            left: x,
            top: CHIP_TOP,
            right: x + w,
            bottom: CHIP_TOP + CHIP_H,
        };
        if selected {
            ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: slot,
                    radiusX: inner_radius,
                    radiusY: inner_radius,
                },
                accent_brush,
            );
        }
        let label = if selected { sel_text_brush } else { text_brush };
        let label_rect = D2D_RECT_F {
            left: slot.left + CHIP_LABEL_INSET_X,
            top: slot.top + CHIP_LABEL_INSET_Y,
            right: slot.right - CHIP_LABEL_INSET_X,
            bottom: slot.bottom - CHIP_LABEL_INSET_Y,
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

unsafe fn draw_shortcut_pill(
    ctx: &ID2D1DeviceContext,
    label: &str,
    icon: Option<VkIcon>,
    x: f32,
    fill: &ID2D1SolidColorBrush,
    outline: &ID2D1SolidColorBrush,
    text: &ID2D1SolidColorBrush,
    format: &IDWriteTextFormat,
) -> Result<(), String> {
    if x < 0.0 {
        return Ok(());
    }
    let rect = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: x,
            top: HINT_TOP,
            right: x + HINT_PILL_W,
            bottom: HINT_TOP + HINT_PILL_H,
        },
        radiusX: HINT_PILL_H * 0.5,
        radiusY: HINT_PILL_H * 0.5,
    };
    ctx.FillRoundedRectangle(&rect, fill);
    ctx.DrawRoundedRectangle(&rect, outline, 1.0, None);
    if let Some(icon) = icon {
        draw_uncached_svg_icon(ctx, icon, rect.rect)?;
    } else {
        let wide: Vec<u16> = label.encode_utf16().collect();
        ctx.DrawText(
            &wide,
            format,
            &rect.rect,
            text,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
    Ok(())
}

unsafe fn draw_uncached_svg_icon(
    ctx: &ID2D1DeviceContext,
    icon: VkIcon,
    rect: D2D_RECT_F,
) -> Result<(), String> {
    let h = rect.bottom - rect.top;
    let draw_px = (h * 0.94).round().clamp(24.0, 64.0);
    let raster_px = (draw_px * 3.0).round().clamp(54.0, 192.0) as u32;
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(icon.svg().as_bytes(), &opt)
        .map_err(|e| format!("parse shortcut icon {icon:?}: {e}"))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(raster_px, raster_px)
        .ok_or_else(|| format!("alloc shortcut icon pixmap {raster_px}x{raster_px}"))?;
    let scale = raster_px as f32 / 32.0;
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
    let bitmap = ctx
        .CreateBitmap(
            D2D_SIZE_U {
                width: raster_px,
                height: raster_px,
            },
            Some(bgra.as_ptr() as *const core::ffi::c_void),
            raster_px * 4,
            &props,
        )
        .map_err(|e| format!("CreateBitmap shortcut icon {icon:?}: {e}"))?;
    let dest = D2D_RECT_F {
        left: (rect.left + rect.right - draw_px) * 0.5,
        top: (rect.top + rect.bottom - draw_px) * 0.5,
        right: (rect.left + rect.right + draw_px) * 0.5,
        bottom: (rect.top + rect.bottom + draw_px) * 0.5,
    };
    ctx.DrawBitmap(
        &bitmap,
        Some(&dest),
        1.0,
        D2D1_INTERPOLATION_MODE_LINEAR,
        None,
        None,
    );
    Ok(())
}

pub struct VkPalette {
    pub bg: u32,
    pub key: u32,
    pub accent: u32,
    pub text: u32,
    /// Label colour on the selected key.
    pub sel_text: u32,
    /// Key outline colour (matches the webview VK border).
    pub border: u32,
}

pub struct VkRenderer {
    width: u32,
    height: u32,
    swapchain: IDXGISwapChain1,
    d2d_context: ID2D1DeviceContext,
    d2d_target: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    text_format: IDWriteTextFormat,
    glyph_format: IDWriteTextFormat,
    /// Small font for sublabels, badges, and the legend strip.
    hint_format: IDWriteTextFormat,
    /// Fixed-size labels on prediction chips (not scaled with key height).
    chip_format: IDWriteTextFormat,
    sublabel_format: IDWriteTextFormat,
    /// Fixed large font for the connect/keyboard prompt pills (10-foot UI).
    prompt_format: IDWriteTextFormat,
    icon_cache: HashMap<IconCacheKey, ID2D1Bitmap1>,
    controller_art_cache: HashMap<ControllerArtCacheKey, (ID2D1Bitmap1, u32, u32)>,
    controller_model_cache: HashMap<ControllerArt, (ID2D1Bitmap1, u32, u32)>,
    prompt_started: Instant,
    /// Gliding focus-ring rect (client px) + the previous draw time, so the ring
    /// eases toward the selected key frame-rate-independently. `None` until the
    /// first frame, where it snaps to the selection.
    anim_sel: Option<D2D_RECT_F>,
    last_draw: Option<Instant>,
    _d3d: ID3D11Device,
    _d2d_device: ID2D1Device,
    _dcomp_device: IDCompositionDevice,
    // Keep the composition target + visual alive for the window's lifetime. Dropping
    // them releases the HWND<->visual binding, so the window shows nothing.
    _comp_target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

/// Reference metrics on a 1920px-wide monitor: 92x68 px keys, 4 px gap,
/// 6.8 px corner radius.
const REF_MON_W: f32 = 1920.0;
const REF_KEY_W: f32 = 92.0;
const KEY_ASPECT: f32 = 68.0 / 92.0;
const REF_GAP: f32 = 4.0;
/// Corner radius as a fraction of key height (6.8/68).
const RADIUS_FRAC: f32 = 6.8 / 68.0;
/// Time constant for the focus ring gliding to the selected key. Small =
/// snappy (~90 ms settle); the fill stays instant so labels never tear.
const SEL_GLIDE_TAU: f32 = 0.045;

/// Uniform padding between the floating card's rounded edge and its key grid.
pub const FLOATING_PAD: f32 = 18.0;

const CHIP_H: f32 = 48.0;
const CHIP_GAP: f32 = 10.0;
const CHIP_PAD_X: f32 = 14.0;
const CHIP_MIN_W: f32 = 58.0;
const CHIP_TOP: f32 = 11.0;
/// Band reserved above the key grid for the suggestion pill, so chips sit ABOVE
/// the keyboard (not over the keys). Constant, so the keys never shift; sized to
/// the pill (`CHIP_TOP + CHIP_H`) plus a gap before the first key row.
pub const STRIP_BAND_H: f32 = CHIP_TOP + CHIP_H + 8.0;
const CHIP_LABEL_INSET_X: f32 = 8.0;
const CHIP_LABEL_INSET_Y: f32 = 4.0;
/// Chip label size in DIPs — independent of key label scaling.
const CHIP_FONT_PX: f32 = 14.0;
const HINT_PILL_W: f32 = 40.0;
const HINT_PILL_H: f32 = 32.0;
const HINT_TOP: f32 = CHIP_TOP + (CHIP_H - HINT_PILL_H) * 0.5;
const HINT_GAP: f32 = 12.0;
const KEY_HINT_BADGE_MAX: f32 = 38.0;
const KEY_HINT_BADGE_INSET: f32 = 7.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum VkIcon {
    Backspace,
    Close,
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

    fn svg(self) -> &'static str {
        match self {
            Self::DualSense => {
                include_str!("../../assets/controller-models/dualsense-controller.svg")
            }
            Self::XboxOne => include_str!("../../assets/controller-models/xbox-controller.svg"),
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
const CONTROLLER_ART_MAX_EDGE: u32 = 768;

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
            VkIcon::Close => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#
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
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round"><line x1="6" x2="10" y1="12" y2="12"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="15" x2="15.01" y1="13" y2="13"/><line x1="18" x2="18.01" y1="11" y2="11"/><rect width="20" height="12" x="2" y="6" rx="4"/><path d="M6 18v1a2 2 0 0 0 4 0v-1"/><path d="M14 18v1a2 2 0 0 0 4 0v-1"/></svg>"#
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
/// width is `span * kw` so the wide space bar covers several key-units.
fn row_pixel_width(row: &KeyRow, kw: f32, gap: f32) -> f32 {
    row.keys
        .iter()
        .map(|k| key_width(kw, gap, k.span))
        .sum::<f32>()
        + gap * (row.keys.len().saturating_sub(1) as f32)
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
}

/// One immutable snapshot of everything the VK renderer needs for a frame.
/// `render_frame` assembles it from a single logical read of nav/predict state;
/// `draw` consumes only `&VkFrame` and performs no global reads, so the
/// selection/glyph-branch logic is testable without a NAV lock or a D2D device.
/// Voice helper phase, for the phase-coded mic halo (so startup/transcribe don't
/// look identical to idle listening).
#[derive(Clone, Copy, PartialEq, Eq)]
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
}

/// One clickable controller input in the native Controller Center.
#[cfg(feature = "gamepad")]
pub struct ControllerCenterBinding<'a> {
    pub button: Button,
    pub action: &'a str,
    pub pressed: bool,
}

#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerCenterStep {
    Trigger,
    Action,
    Configure,
}

/// Immutable data for one Controller Center repaint.
#[cfg(feature = "gamepad")]
pub struct ControllerCenterFrame<'a> {
    pub palette: &'a VkPalette,
    pub connected: bool,
    pub controller_label: &'a str,
    pub input: &'a str,
    pub battery_percent: i32,
    pub charging: bool,
    pub wired: bool,
    pub axes: (f32, f32, f32, f32),
    pub bindings: &'a [ControllerCenterBinding<'a>],
    pub selected: Option<Button>,
    /// Optional held modifier. `None` means the selected trigger is a normal
    /// single-button mapping.
    pub selected_hold: Option<Button>,
    pub wizard_pending: Option<Button>,
    pub wizard_step: Option<ControllerCenterStep>,
    pub wizard_action: Option<DesktopActionKind>,
    pub wizard_shortcut: Option<Shortcut>,
    pub launch_target: &'a str,
    pub app_query: &'a str,
    pub apps: &'a [LaunchableApp],
    pub app_matches: &'a [usize],
    pub app_selected: Option<usize>,
    pub app_scroll: usize,
    pub workspace_name: &'a str,
    pub workspace_candidates: &'a [WorkspaceWindowCandidate],
    pub workspace_selected_ids: &'a HashSet<isize>,
    pub workspace_scroll: usize,
    pub command_text: &'a str,
    pub wizard_notice: &'a str,
    pub deadzone: f32,
}

/// Target returned by the shared Controller Center layout. Keeping hit testing
/// beside its draw geometry prevents a clicked label from drifting from its pill.
#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerCenterHit {
    Button(Button),
    Deadzone(u8),
    Action(DesktopActionKind),
    TriggerCapture,
    ShortcutCapture,
    AppSearch,
    CommandInput,
    AppRow(usize),
    AppScrollUp,
    AppScrollDown,
    WorkspaceName,
    WorkspaceRow(usize),
    WorkspaceScrollUp,
    WorkspaceScrollDown,
    Continue,
    Back,
    Cancel,
    Save,
    Clear,
}

#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerCenterHitState {
    pub step: ControllerCenterStep,
    pub action: Option<DesktopActionKind>,
    pub app_rows: usize,
    pub app_can_scroll_up: bool,
    pub app_can_scroll_down: bool,
    pub workspace_rows: usize,
    pub workspace_can_scroll_up: bool,
    pub workspace_can_scroll_down: bool,
}

#[cfg(feature = "gamepad")]
const CENTER_PAGE_PAD: f32 = 22.0;
#[cfg(feature = "gamepad")]
const CENTER_HEADER_BOTTOM: f32 = 68.0;
#[cfg(feature = "gamepad")]
const CENTER_STAGE_GAP: f32 = 16.0;
#[cfg(feature = "gamepad")]
const CENTER_RAIL_HEADER_H: f32 = 26.0;
#[cfg(feature = "gamepad")]
const CENTER_CARD_GAP: f32 = 6.0;
#[cfg(feature = "gamepad")]
const CENTER_CARD_MIN_H: f32 = 28.0;
#[cfg(feature = "gamepad")]
const CENTER_CARD_MAX_H: f32 = 38.0;
#[cfg(feature = "gamepad")]
const CENTER_DRAWER_MIN_H: f32 = 240.0;
#[cfg(feature = "gamepad")]
const CENTER_DRAWER_MAX_H: f32 = 248.0;
#[cfg(feature = "gamepad")]
const CENTER_DEADZONE_MAX: f32 = 0.60;

#[cfg(feature = "gamepad")]
fn controller_center_drawer_rect(width: f32, height: f32) -> D2D_RECT_F {
    let drawer_h = (height * 0.32).clamp(CENTER_DRAWER_MIN_H, CENTER_DRAWER_MAX_H);
    let bottom = (height - 16.0).max(0.0);
    let top = (bottom - drawer_h).max(CENTER_HEADER_BOTTOM + 112.0);
    D2D_RECT_F {
        left: CENTER_PAGE_PAD - 6.0,
        top,
        right: (width - CENTER_PAGE_PAD + 6.0).max(CENTER_PAGE_PAD),
        bottom,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_stage_rect(width: f32, height: f32) -> D2D_RECT_F {
    let drawer = controller_center_drawer_rect(width, height);
    let top = CENTER_HEADER_BOTTOM + 14.0;
    let bottom = (drawer.top - CENTER_STAGE_GAP).max(top + 150.0);
    D2D_RECT_F {
        left: CENTER_PAGE_PAD,
        top,
        right: (width - CENTER_PAGE_PAD).max(CENTER_PAGE_PAD + 1.0),
        bottom,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_card_width(width: f32) -> f32 {
    (width * 0.235).clamp(224.0, 300.0)
}

#[cfg(feature = "gamepad")]
fn controller_center_card_height(width: f32, height: f32) -> f32 {
    let stage = controller_center_stage_rect(width, height);
    let available = (stage.bottom - stage.top - CENTER_RAIL_HEADER_H - 8.0)
        .max(CENTER_CARD_MIN_H * 9.0 + CENTER_CARD_GAP * 8.0);
    ((available - CENTER_CARD_GAP * 8.0) / 9.0).clamp(CENTER_CARD_MIN_H, CENTER_CARD_MAX_H)
}

#[cfg(feature = "gamepad")]
fn controller_center_art_rect(width: f32, height: f32) -> D2D_RECT_F {
    let stage = controller_center_stage_rect(width, height);
    let card_w = controller_center_card_width(width);
    D2D_RECT_F {
        left: stage.left + card_w + CENTER_STAGE_GAP,
        top: stage.top,
        right: stage.right - card_w - CENTER_STAGE_GAP,
        bottom: stage.bottom,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_model_rect(width: f32, height: f32) -> D2D_RECT_F {
    let art = controller_center_art_rect(width, height);
    let available = D2D_RECT_F {
        left: art.left + 18.0,
        top: art.top + 42.0,
        right: (art.right - 18.0).max(art.left + 19.0),
        bottom: (art.bottom - 44.0).max(art.top + 43.0),
    };
    let available_w = available.right - available.left;
    let available_h = available.bottom - available.top;
    let aspect = 5.0 / 3.0;
    let (model_w, model_h) = if available_w / available_h > aspect {
        (available_h * aspect, available_h)
    } else {
        (available_w, available_w / aspect)
    };
    D2D_RECT_F {
        left: (available.left + available.right - model_w) * 0.5,
        top: (available.top + available.bottom - model_h) * 0.5,
        right: (available.left + available.right + model_w) * 0.5,
        bottom: (available.top + available.bottom + model_h) * 0.5,
    }
}

#[cfg(feature = "gamepad")]
#[derive(Clone, Copy, Debug, PartialEq)]
struct ControllerModelPoint {
    x: f32,
    y: f32,
}

#[cfg(feature = "gamepad")]
fn controller_button_position(button: Button, playstation: bool) -> ControllerModelPoint {
    let (x, y) = match button {
        Button::Lt => (0.31, 0.25),
        Button::Lb => (0.29, 0.34),
        Button::Select => (0.41, 0.40),
        Button::L3 => {
            if playstation {
                (0.37, 0.70)
            } else {
                (0.36, 0.57)
            }
        }
        Button::Up => {
            if playstation {
                (0.25, 0.50)
            } else {
                (0.25, 0.72)
            }
        }
        Button::Left => {
            if playstation {
                (0.20, 0.56)
            } else {
                (0.20, 0.77)
            }
        }
        Button::Right => {
            if playstation {
                (0.30, 0.56)
            } else {
                (0.30, 0.77)
            }
        }
        Button::Down => {
            if playstation {
                (0.25, 0.62)
            } else {
                (0.25, 0.84)
            }
        }
        Button::Touchpad => {
            if playstation {
                (0.50, 0.35)
            } else {
                (0.50, 0.45)
            }
        }
        Button::Rt => (0.69, 0.25),
        Button::Rb => (0.71, 0.34),
        Button::Start => (0.59, 0.40),
        Button::R3 => {
            if playstation {
                (0.63, 0.70)
            } else {
                (0.64, 0.72)
            }
        }
        Button::Y => (0.75, 0.43),
        Button::X => (0.69, 0.52),
        Button::B => (0.81, 0.52),
        Button::A => (0.75, 0.61),
        Button::Guide => {
            if playstation {
                (0.50, 0.54)
            } else {
                (0.50, 0.49)
            }
        }
    };
    ControllerModelPoint { x, y }
}

#[cfg(feature = "gamepad")]
fn controller_button_size(button: Button, unit: f32) -> (f32, f32) {
    match button {
        Button::Lt | Button::Lb | Button::Rt | Button::Rb => (unit * 0.18, unit * 0.075),
        Button::Up | Button::Left | Button::Right | Button::Down => (unit * 0.085, unit * 0.085),
        Button::L3 | Button::R3 => (unit * 0.22, unit * 0.22),
        Button::Touchpad => (unit * 0.29, unit * 0.10),
        Button::Select | Button::Start => (unit * 0.13, unit * 0.075),
        Button::Guide => (unit * 0.11, unit * 0.11),
        _ => (unit * 0.115, unit * 0.115),
    }
}

#[cfg(feature = "gamepad")]
fn controller_button_rect(rect: D2D_RECT_F, button: Button, playstation: bool) -> D2D_RECT_F {
    let unit = (rect.right - rect.left)
        .min(rect.bottom - rect.top)
        .max(1.0);
    let point = controller_button_position(button, playstation);
    let (width, height) = controller_button_size(button, unit);
    let cx = rect.left + (rect.right - rect.left) * point.x;
    let cy = rect.top + (rect.bottom - rect.top) * point.y;
    D2D_RECT_F {
        left: cx - width * 0.5,
        top: cy - height * 0.5,
        right: cx + width * 0.5,
        bottom: cy + height * 0.5,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_button_hit(x: f32, y: f32, width: f32, height: f32) -> Option<Button> {
    let model = controller_center_model_rect(width, height);
    let mut closest = None;
    for &button in MAPPABLE_BUTTONS {
        for playstation in [false, true] {
            let rect = controller_button_rect(model, button, playstation);
            if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
                let center_x = (rect.left + rect.right) * 0.5;
                let center_y = (rect.top + rect.bottom) * 0.5;
                let distance = (x - center_x).powi(2) + (y - center_y).powi(2);
                if closest.map_or(true, |(_, best)| distance < best) {
                    closest = Some((button, distance));
                }
            }
        }
    }
    closest.map(|(button, _)| button)
}

#[cfg(feature = "gamepad")]
fn controller_visual_label(button: Button, playstation: bool) -> &'static str {
    match button {
        Button::A if playstation => "×",
        Button::B if playstation => "○",
        Button::X if playstation => "□",
        Button::Y if playstation => "△",
        Button::Up => "↑",
        Button::Left => "←",
        Button::Right => "→",
        Button::Down => "↓",
        Button::Select if playstation => "Create",
        Button::Select => "View",
        Button::Start if playstation => "Options",
        Button::Start => "Menu",
        Button::Guide if playstation => "PS",
        Button::Guide => "Xbox",
        Button::Touchpad => "Touch",
        _ => controller_button_label(button, if playstation { "DualSense" } else { "Xbox" }),
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_card_rect(button: Button, width: f32, height: f32) -> Option<D2D_RECT_F> {
    let index = MAPPABLE_BUTTONS
        .iter()
        .position(|candidate| *candidate == button)?;
    let card_w = controller_center_card_width(width);
    let card_h = controller_center_card_height(width, height);
    let stage = controller_center_stage_rect(width, height);
    let left = if index < 9 {
        stage.left
    } else {
        stage.right - card_w
    };
    let top = stage.top + CENTER_RAIL_HEADER_H + (index % 9) as f32 * (card_h + CENTER_CARD_GAP);
    Some(D2D_RECT_F {
        left,
        top,
        right: left + card_w,
        bottom: top + card_h,
    })
}

#[cfg(feature = "gamepad")]
fn controller_center_badge_width(button: Button, controller_label: &str) -> f32 {
    match controller_button_label(button, controller_label).len() {
        0..=2 => 52.0,
        3..=5 => 62.0,
        _ => 78.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_deadzone_rect(width: f32, height: f32) -> D2D_RECT_F {
    let drawer = controller_center_drawer_rect(width, height);
    let slider_w = (width * 0.24).clamp(220.0, 350.0);
    let right = drawer.right - 16.0;
    let left = (right - slider_w).max(drawer.left + 160.0);
    D2D_RECT_F {
        left,
        top: drawer.top + 216.0,
        right,
        bottom: drawer.top + 232.0,
    }
}

#[cfg(feature = "gamepad")]
const CENTER_WIZARD_ROWS: usize = 3;

#[cfg(feature = "gamepad")]
fn controller_center_wizard_content_rect(width: f32, height: f32) -> D2D_RECT_F {
    let drawer = controller_center_drawer_rect(width, height);
    let slider = controller_center_deadzone_rect(width, height);
    D2D_RECT_F {
        left: drawer.left + 16.0,
        top: drawer.top + 42.0,
        right: (slider.left - 28.0).max(drawer.left + 240.0),
        bottom: drawer.bottom - 48.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_wizard_action_rect(index: usize, width: f32, height: f32) -> D2D_RECT_F {
    let content = controller_center_wizard_content_rect(width, height);
    let gap = 8.0;
    let slot_w = ((content.right - content.left - gap * 2.0) / 3.0).max(1.0);
    let x = content.left + index.min(2) as f32 * (slot_w + gap);
    let top = content.top + 30.0;
    D2D_RECT_F {
        left: x,
        top,
        right: x + slot_w,
        bottom: top + 56.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_wizard_trigger_rect(width: f32, height: f32) -> D2D_RECT_F {
    let content = controller_center_wizard_content_rect(width, height);
    D2D_RECT_F {
        left: content.left,
        top: content.top + 30.0,
        right: content.right,
        bottom: content.top + 82.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_wizard_field_rect(width: f32, height: f32) -> D2D_RECT_F {
    let content = controller_center_wizard_content_rect(width, height);
    D2D_RECT_F {
        left: content.left,
        top: content.top + 26.0,
        right: content.right,
        bottom: content.top + 56.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_wizard_list_row_rect(row: usize, width: f32, height: f32) -> D2D_RECT_F {
    let content = controller_center_wizard_content_rect(width, height);
    let top = content.top + 60.0 + row.min(CENTER_WIZARD_ROWS - 1) as f32 * 30.0;
    D2D_RECT_F {
        left: content.left,
        top,
        right: content.right - 30.0,
        bottom: top + 26.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_wizard_scroll_rect(up: bool, width: f32, height: f32) -> D2D_RECT_F {
    let content = controller_center_wizard_content_rect(width, height);
    let top = content.top + 60.0 + if up { 0.0 } else { 60.0 };
    D2D_RECT_F {
        left: content.right - 24.0,
        top,
        right: content.right,
        bottom: top + 26.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_wizard_footer_button_rect(
    index: usize,
    width: f32,
    height: f32,
) -> D2D_RECT_F {
    let drawer = controller_center_drawer_rect(width, height);
    let content = controller_center_wizard_content_rect(width, height);
    let (x, w) = match index {
        0 => (drawer.left + 16.0, 72.0),
        1 => (drawer.left + 96.0, 72.0),
        2 => (drawer.left + 176.0, 132.0),
        _ => ((content.right - 100.0).max(drawer.left + 320.0), 100.0),
    };
    let top = drawer.bottom - 42.0;
    D2D_RECT_F {
        left: x,
        top,
        right: x + w,
        bottom: drawer.bottom - 10.0,
    }
}

#[cfg(feature = "gamepad")]
fn controller_center_action_rect(index: usize, width: f32, height: f32) -> D2D_RECT_F {
    controller_center_wizard_action_rect(index, width, height)
}

#[cfg(feature = "gamepad")]
pub fn controller_center_hit(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    selected: Option<Button>,
) -> Option<ControllerCenterHit> {
    for &button in MAPPABLE_BUTTONS {
        let rect = controller_center_card_rect(button, width, height)?;
        if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
            return Some(ControllerCenterHit::Button(button));
        }
    }
    if let Some(button) = controller_center_button_hit(x, y, width, height) {
        return Some(ControllerCenterHit::Button(button));
    }
    let _ = selected;
    for (index, kind) in [
        DesktopActionKind::Shortcut,
        DesktopActionKind::Launch,
        DesktopActionKind::Workspace,
    ]
    .into_iter()
    .enumerate()
    {
        let rect = controller_center_action_rect(index, width, height);
        if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
            return Some(ControllerCenterHit::Action(kind));
        }
    }
    let clear = controller_center_wizard_footer_button_rect(2, width, height);
    if x >= clear.left && x <= clear.right && y >= clear.top && y <= clear.bottom {
        return Some(ControllerCenterHit::Clear);
    }
    let slider = controller_center_deadzone_rect(width, height);
    if x >= slider.left && x <= slider.right && y >= slider.top - 12.0 && y <= slider.bottom + 12.0
    {
        let value = ((x - slider.left) / (slider.right - slider.left)).clamp(0.0, 1.0);
        return Some(ControllerCenterHit::Deadzone(
            (value * CENTER_DEADZONE_MAX * 100.0).round() as u8,
        ));
    }
    None
}

#[cfg(feature = "gamepad")]
pub fn controller_center_hit_with_wizard(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    selected: Option<Button>,
    state: ControllerCenterHitState,
) -> Option<ControllerCenterHit> {
    if let Some(hit) = controller_center_hit(x, y, width, height, selected) {
        if matches!(
            hit,
            ControllerCenterHit::Button(_) | ControllerCenterHit::Deadzone(_)
        ) {
            return Some(hit);
        }
    }
    let clear = controller_center_wizard_footer_button_rect(2, width, height);
    if x >= clear.left && x <= clear.right && y >= clear.top && y <= clear.bottom {
        return Some(ControllerCenterHit::Clear);
    }
    let back = controller_center_wizard_footer_button_rect(0, width, height);
    if x >= back.left && x <= back.right && y >= back.top && y <= back.bottom {
        if state.step != ControllerCenterStep::Trigger {
            return Some(ControllerCenterHit::Back);
        }
        return None;
    }
    let cancel = controller_center_wizard_footer_button_rect(1, width, height);
    if x >= cancel.left && x <= cancel.right && y >= cancel.top && y <= cancel.bottom {
        return Some(ControllerCenterHit::Cancel);
    }
    let primary = controller_center_wizard_footer_button_rect(3, width, height);
    if x >= primary.left && x <= primary.right && y >= primary.top && y <= primary.bottom {
        return Some(if state.step == ControllerCenterStep::Configure {
            ControllerCenterHit::Save
        } else {
            ControllerCenterHit::Continue
        });
    }
    match state.step {
        ControllerCenterStep::Trigger => {
            let rect = controller_center_wizard_trigger_rect(width, height);
            (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom)
                .then_some(ControllerCenterHit::TriggerCapture)
        }
        ControllerCenterStep::Action => {
            for (index, kind) in [
                DesktopActionKind::Shortcut,
                DesktopActionKind::Launch,
                DesktopActionKind::Workspace,
            ]
            .into_iter()
            .enumerate()
            {
                let rect = controller_center_wizard_action_rect(index, width, height);
                if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
                    return Some(ControllerCenterHit::Action(kind));
                }
            }
            None
        }
        ControllerCenterStep::Configure => match state.action {
            Some(DesktopActionKind::Shortcut) => {
                let rect = controller_center_wizard_field_rect(width, height);
                (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom)
                    .then_some(ControllerCenterHit::ShortcutCapture)
            }
            Some(DesktopActionKind::Launch) => {
                let field = controller_center_wizard_field_rect(width, height);
                if x >= field.left && x <= field.right && y >= field.top && y <= field.bottom {
                    return Some(ControllerCenterHit::AppSearch);
                }
                for row in 0..CENTER_WIZARD_ROWS.min(state.app_rows) {
                    let rect = controller_center_wizard_list_row_rect(row, width, height);
                    if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
                        return Some(ControllerCenterHit::AppRow(row));
                    }
                }
                let up = controller_center_wizard_scroll_rect(true, width, height);
                if state.app_can_scroll_up
                    && x >= up.left
                    && x <= up.right
                    && y >= up.top
                    && y <= up.bottom
                {
                    return Some(ControllerCenterHit::AppScrollUp);
                }
                let down = controller_center_wizard_scroll_rect(false, width, height);
                if state.app_can_scroll_down
                    && x >= down.left
                    && x <= down.right
                    && y >= down.top
                    && y <= down.bottom
                {
                    return Some(ControllerCenterHit::AppScrollDown);
                }
                None
            }
            Some(DesktopActionKind::Workspace) => {
                let field = controller_center_wizard_field_rect(width, height);
                if x >= field.left && x <= field.right && y >= field.top && y <= field.bottom {
                    return Some(ControllerCenterHit::WorkspaceName);
                }
                for row in 0..CENTER_WIZARD_ROWS.min(state.workspace_rows) {
                    let rect = controller_center_wizard_list_row_rect(row, width, height);
                    if x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom {
                        return Some(ControllerCenterHit::WorkspaceRow(row));
                    }
                }
                let up = controller_center_wizard_scroll_rect(true, width, height);
                if state.workspace_can_scroll_up
                    && x >= up.left
                    && x <= up.right
                    && y >= up.top
                    && y <= up.bottom
                {
                    return Some(ControllerCenterHit::WorkspaceScrollUp);
                }
                let down = controller_center_wizard_scroll_rect(false, width, height);
                if state.workspace_can_scroll_down
                    && x >= down.left
                    && x <= down.right
                    && y >= down.top
                    && y <= down.bottom
                {
                    return Some(ControllerCenterHit::WorkspaceScrollDown);
                }
                None
            }
            Some(DesktopActionKind::Command) => {
                let rect = controller_center_wizard_field_rect(width, height);
                (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom)
                    .then_some(ControllerCenterHit::CommandInput)
            }
            None => None,
        },
    }
}

#[cfg(feature = "gamepad")]
fn controller_button_label(button: Button, controller_label: &str) -> &'static str {
    let ps = ControllerIconFamily::from_label(controller_label) == ControllerIconFamily::Ps5;
    if !ps {
        return button.as_str();
    }
    match button {
        Button::A => "Cross",
        Button::B => "Circle",
        Button::X => "Square",
        Button::Y => "Triangle",
        Button::Lb => "L1",
        Button::Rb => "R1",
        Button::Lt => "L2",
        Button::Rt => "R2",
        Button::Select => "Create",
        Button::Start => "Options",
        Button::Guide => "PS",
        _ => button.as_str(),
    }
}

#[cfg(feature = "gamepad")]
unsafe fn draw_center_text(
    context: &ID2D1DeviceContext,
    text: &str,
    format: &IDWriteTextFormat,
    rect: &D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    context.DrawText(
        &wide,
        format,
        rect,
        brush,
        D2D1_DRAW_TEXT_OPTIONS_CLIP,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

#[cfg(feature = "gamepad")]
unsafe fn draw_center_wizard(
    context: &ID2D1DeviceContext,
    width: f32,
    height: f32,
    drawer: &D2D1_ROUNDED_RECT,
    frame: &ControllerCenterFrame,
    button: Button,
    key_brush: &ID2D1SolidColorBrush,
    accent_brush: &ID2D1SolidColorBrush,
    ring_brush: &ID2D1SolidColorBrush,
    border_brush: &ID2D1SolidColorBrush,
    text_brush: &ID2D1SolidColorBrush,
    sel_text_brush: &ID2D1SolidColorBrush,
    muted_brush: &ID2D1SolidColorBrush,
    hint_format: &IDWriteTextFormat,
    chip_format: &IDWriteTextFormat,
) -> Result<(), String> {
    let content = controller_center_wizard_content_rect(width, height);
    let step = frame.wizard_step.unwrap_or(ControllerCenterStep::Trigger);
    let trigger_label = frame.selected_hold.map_or_else(
        || controller_button_label(button, frame.controller_label).to_string(),
        |hold| {
            format!(
                "{} + {}",
                controller_button_label(hold, frame.controller_label),
                controller_button_label(button, frame.controller_label)
            )
        },
    );

    let _ = hint_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    let _ = chip_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
    draw_center_text(
        context,
        &format!("SET UP SHORTCUT · {trigger_label}"),
        hint_format,
        &D2D_RECT_F {
            left: drawer.rect.left + 16.0,
            top: drawer.rect.top + 9.0,
            right: content.right - 250.0,
            bottom: drawer.rect.top + 31.0,
        },
        text_brush,
    );
    draw_center_text(
        context,
        "1 Trigger   ›   2 Action   ›   3 Configure",
        hint_format,
        &D2D_RECT_F {
            left: content.right - 250.0,
            top: drawer.rect.top + 10.0,
            right: content.right,
            bottom: drawer.rect.top + 31.0,
        },
        muted_brush,
    );

    let draw_box = |rect: D2D_RECT_F, selected: bool, enabled: bool| {
        let rounded = D2D1_ROUNDED_RECT {
            rect,
            radiusX: 8.0,
            radiusY: 8.0,
        };
        context.FillRoundedRectangle(
            &rounded,
            if !enabled {
                border_brush
            } else if selected {
                accent_brush
            } else {
                key_brush
            },
        );
        context.DrawRoundedRectangle(
            &rounded,
            if selected { ring_brush } else { border_brush },
            if selected { 2.0 } else { 1.0 },
            None,
        );
    };

    match step {
        ControllerCenterStep::Trigger => {
            draw_center_text(
                context,
                "TRIGGER · capture from the live controller",
                hint_format,
                &D2D_RECT_F {
                    left: content.left,
                    top: content.top,
                    right: content.right,
                    bottom: content.top + 22.0,
                },
                muted_brush,
            );
            let rect = controller_center_wizard_trigger_rect(width, height);
            draw_box(rect, true, true);
            let captured = format!("Captured: {trigger_label}");
            draw_center_text(
                context,
                &captured,
                chip_format,
                &D2D_RECT_F {
                    left: rect.left + 12.0,
                    top: rect.top + 5.0,
                    right: rect.right - 12.0,
                    bottom: rect.top + 28.0,
                },
                sel_text_brush,
            );
            let capture_hint = frame.wizard_pending.map_or_else(
                || "Press and release a button alone, or hold one and press another for a directed chord.".to_string(),
                |pending| {
                    format!(
                        "{} held · release to confirm single, or press another button for a chord",
                        controller_button_label(pending, frame.controller_label)
                    )
                },
            );
            draw_center_text(
                context,
                &capture_hint,
                hint_format,
                &D2D_RECT_F {
                    left: rect.left + 12.0,
                    top: rect.top + 29.0,
                    right: rect.right - 12.0,
                    bottom: rect.bottom - 5.0,
                },
                if frame.wizard_pending.is_some() {
                    text_brush
                } else {
                    muted_brush
                },
            );
            draw_center_text(
                context,
                if frame.wizard_notice.is_empty() {
                    "Nothing is saved during trigger capture. Continue when the trigger is right."
                } else {
                    frame.wizard_notice
                },
                hint_format,
                &D2D_RECT_F {
                    left: content.left,
                    top: content.top + 88.0,
                    right: content.right,
                    bottom: content.top + 111.0,
                },
                if frame.wizard_notice.is_empty() {
                    muted_brush
                } else {
                    text_brush
                },
            );
        }
        ControllerCenterStep::Action => {
            draw_center_text(
                context,
                "ACTION · choose one desktop behavior",
                hint_format,
                &D2D_RECT_F {
                    left: content.left,
                    top: content.top,
                    right: content.right,
                    bottom: content.top + 22.0,
                },
                muted_brush,
            );
            let choices = [
                (
                    DesktopActionKind::Shortcut,
                    "Keyboard shortcut",
                    "Capture a Windows key combination",
                ),
                (
                    DesktopActionKind::Launch,
                    "Open app",
                    "Choose an installed app or target",
                ),
                (
                    DesktopActionKind::Workspace,
                    "Restore workspace",
                    "Save visible window positions and sizes",
                ),
            ];
            for (index, (kind, title, description)) in choices.into_iter().enumerate() {
                let rect = controller_center_wizard_action_rect(index, width, height);
                let selected = frame.wizard_action == Some(kind);
                draw_box(rect, selected, true);
                draw_center_text(
                    context,
                    title,
                    hint_format,
                    &D2D_RECT_F {
                        left: rect.left + 6.0,
                        top: rect.top + 5.0,
                        right: rect.right - 6.0,
                        bottom: rect.top + 26.0,
                    },
                    if selected { sel_text_brush } else { text_brush },
                );
                draw_center_text(
                    context,
                    description,
                    hint_format,
                    &D2D_RECT_F {
                        left: rect.left + 6.0,
                        top: rect.top + 29.0,
                        right: rect.right - 6.0,
                        bottom: rect.bottom - 4.0,
                    },
                    if selected {
                        sel_text_brush
                    } else {
                        muted_brush
                    },
                );
            }
            if frame.wizard_action == Some(DesktopActionKind::Command) {
                draw_center_text(
                    context,
                    "Existing Command mapping loaded for compatibility · Clear or Continue to edit it.",
                    hint_format,
                    &D2D_RECT_F {
                        left: content.left,
                        top: content.top + 94.0,
                        right: content.right,
                        bottom: content.top + 117.0,
                    },
                    muted_brush,
                );
            }
        }
        ControllerCenterStep::Configure => match frame.wizard_action {
            Some(DesktopActionKind::Shortcut) => {
                draw_center_text(
                    context,
                    "CONFIGURE · keyboard shortcut",
                    hint_format,
                    &D2D_RECT_F {
                        left: content.left,
                        top: content.top,
                        right: content.right,
                        bottom: content.top + 22.0,
                    },
                    muted_brush,
                );
                let rect = controller_center_wizard_field_rect(width, height);
                draw_box(rect, true, true);
                draw_center_text(
                    context,
                    frame
                        .wizard_shortcut
                        .map(|shortcut| shortcut.display())
                        .as_deref()
                        .unwrap_or("Press a key combination here"),
                    chip_format,
                    &rect,
                    if frame.wizard_shortcut.is_some() {
                        sel_text_brush
                    } else {
                        muted_brush
                    },
                );
                draw_center_text(
                    context,
                    if frame.wizard_notice.is_empty() {
                        "Modifier-only keys are ignored. Capture is pending until you press Save."
                    } else {
                        frame.wizard_notice
                    },
                    hint_format,
                    &D2D_RECT_F {
                        left: content.left,
                        top: content.top + 63.0,
                        right: content.right,
                        bottom: content.top + 87.0,
                    },
                    if frame.wizard_notice.is_empty() {
                        muted_brush
                    } else {
                        text_brush
                    },
                );
            }
            Some(DesktopActionKind::Launch) => {
                draw_center_text(
                    context,
                    "CONFIGURE · choose an app",
                    hint_format,
                    &D2D_RECT_F {
                        left: content.left,
                        top: content.top,
                        right: content.right,
                        bottom: content.top + 22.0,
                    },
                    muted_brush,
                );
                let field = controller_center_wizard_field_rect(width, height);
                draw_box(field, true, true);
                let query = if frame.app_query.is_empty() && !frame.launch_target.is_empty() {
                    format!("Existing target: {}", frame.launch_target)
                } else if frame.app_query.is_empty() {
                    "Search app name or target".to_string()
                } else {
                    frame.app_query.to_string()
                };
                let query_is_placeholder =
                    frame.app_query.is_empty() && frame.launch_target.is_empty();
                draw_center_text(
                    context,
                    &query,
                    chip_format,
                    &D2D_RECT_F {
                        left: field.left + 12.0,
                        top: field.top,
                        right: field.right - 12.0,
                        bottom: field.bottom,
                    },
                    if query_is_placeholder {
                        muted_brush
                    } else {
                        text_brush
                    },
                );
                let indices = frame.app_matches;
                for row in 0..CENTER_WIZARD_ROWS {
                    let rect = controller_center_wizard_list_row_rect(row, width, height);
                    let Some(index) = indices.get(frame.app_scroll + row).copied() else {
                        break;
                    };
                    let selected = frame.app_selected == Some(index);
                    draw_box(rect, selected, true);
                    let app = &frame.apps[index];
                    let label = format!(
                        "{}{}  ·  {}",
                        if selected { "✓ " } else { "" },
                        app.name,
                        app.target
                    );
                    draw_center_text(
                        context,
                        &label,
                        hint_format,
                        &D2D_RECT_F {
                            left: rect.left + 8.0,
                            top: rect.top,
                            right: rect.right - 8.0,
                            bottom: rect.bottom,
                        },
                        if selected { sel_text_brush } else { text_brush },
                    );
                }
                let indices_len = indices.len();
                for up in [true, false] {
                    let rect = controller_center_wizard_scroll_rect(up, width, height);
                    let enabled = if up {
                        frame.app_scroll > 0
                    } else {
                        frame.app_scroll + CENTER_WIZARD_ROWS < indices_len
                    };
                    draw_box(rect, false, enabled);
                    draw_center_text(
                        context,
                        if up { "↑" } else { "↓" },
                        chip_format,
                        &rect,
                        if enabled { text_brush } else { muted_brush },
                    );
                }
                if indices.is_empty() {
                    draw_center_text(
                        context,
                        if frame.wizard_notice.is_empty() {
                            "No matching apps. Clear the search, or go Back, then Continue to refresh."
                        } else {
                            frame.wizard_notice
                        },
                        hint_format,
                        &D2D_RECT_F {
                            left: content.left,
                            top: content.top + 64.0,
                            right: content.right - 30.0,
                            bottom: content.top + 92.0,
                        },
                        muted_brush,
                    );
                }
            }
            Some(DesktopActionKind::Workspace) => {
                draw_center_text(
                    context,
                    if frame.wizard_notice.is_empty() {
                        "CONFIGURE · restore current window positions and sizes"
                    } else {
                        frame.wizard_notice
                    },
                    hint_format,
                    &D2D_RECT_F {
                        left: content.left,
                        top: content.top,
                        right: content.right,
                        bottom: content.top + 22.0,
                    },
                    muted_brush,
                );
                let field = controller_center_wizard_field_rect(width, height);
                draw_box(field, true, true);
                let selected_count = frame.workspace_selected_ids.len();
                let name = if frame.workspace_name.is_empty() {
                    format!(
                        "Workspace name · {selected_count} window{} selected",
                        if selected_count == 1 { "" } else { "s" }
                    )
                } else {
                    format!("{} · {selected_count} selected", frame.workspace_name)
                };
                draw_center_text(
                    context,
                    &name,
                    chip_format,
                    &D2D_RECT_F {
                        left: field.left + 12.0,
                        top: field.top,
                        right: field.right - 12.0,
                        bottom: field.bottom,
                    },
                    if frame.workspace_name.is_empty() {
                        muted_brush
                    } else {
                        text_brush
                    },
                );
                for row in 0..CENTER_WIZARD_ROWS {
                    let rect = controller_center_wizard_list_row_rect(row, width, height);
                    let Some(candidate) =
                        frame.workspace_candidates.get(frame.workspace_scroll + row)
                    else {
                        break;
                    };
                    let selected = frame.workspace_selected_ids.contains(&candidate.id);
                    draw_box(rect, selected, true);
                    let label = format!(
                        "{}{}  ·  {}",
                        if selected { "✓ " } else { "" },
                        if candidate.title.is_empty() {
                            "Untitled window"
                        } else {
                            &candidate.title
                        },
                        candidate.executable
                    );
                    draw_center_text(
                        context,
                        &label,
                        hint_format,
                        &D2D_RECT_F {
                            left: rect.left + 8.0,
                            top: rect.top,
                            right: rect.right - 8.0,
                            bottom: rect.bottom,
                        },
                        if selected { sel_text_brush } else { text_brush },
                    );
                }
                for up in [true, false] {
                    let rect = controller_center_wizard_scroll_rect(up, width, height);
                    let enabled = if up {
                        frame.workspace_scroll > 0
                    } else {
                        frame.workspace_scroll + CENTER_WIZARD_ROWS
                            < frame.workspace_candidates.len()
                    };
                    draw_box(rect, false, enabled);
                    draw_center_text(
                        context,
                        if up { "↑" } else { "↓" },
                        chip_format,
                        &rect,
                        if enabled { text_brush } else { muted_brush },
                    );
                }
            }
            Some(DesktopActionKind::Command) => {
                draw_center_text(
                    context,
                    "CONFIGURE · existing Command mapping",
                    hint_format,
                    &D2D_RECT_F {
                        left: content.left,
                        top: content.top,
                        right: content.right,
                        bottom: content.top + 22.0,
                    },
                    muted_brush,
                );
                let field = controller_center_wizard_field_rect(width, height);
                draw_box(field, true, true);
                draw_center_text(
                    context,
                    if frame.command_text.is_empty() {
                        "Existing command"
                    } else {
                        frame.command_text
                    },
                    chip_format,
                    &field,
                    text_brush,
                );
            }
            None => {}
        },
    }

    let footer = controller_center_wizard_footer_button_rect(0, width, height);
    let footer_specs = [
        (footer, "Back", step != ControllerCenterStep::Trigger),
        (
            controller_center_wizard_footer_button_rect(1, width, height),
            "Cancel",
            true,
        ),
        (
            controller_center_wizard_footer_button_rect(2, width, height),
            "Clear existing",
            true,
        ),
        (
            controller_center_wizard_footer_button_rect(3, width, height),
            if step == ControllerCenterStep::Configure {
                "Save"
            } else {
                "Continue"
            },
            step != ControllerCenterStep::Action || frame.wizard_action.is_some(),
        ),
    ];
    for (rect, label, enabled) in footer_specs {
        draw_box(rect, false, enabled);
        draw_center_text(
            context,
            label,
            hint_format,
            &rect,
            if enabled { text_brush } else { muted_brush },
        );
    }
    Ok(())
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
        let _ = hint_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let _ = chip_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = chip_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = chip_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let _ = sublabel_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = sublabel_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        let _ = sublabel_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);

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
            chip_format,
            sublabel_format,
            prompt_format,
            icon_cache: HashMap::new(),
            controller_art_cache: HashMap::new(),
            controller_model_cache: HashMap::new(),
            prompt_started: Instant::now(),
            anim_sel: None,
            last_draw: None,
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
        self.d2d_target = None;
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
        self.d2d_target = Some(bind_d2d_target(&self.d2d_context, &self.swapchain)?);
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
            icon if icon.is_controller_tip() => (h * 0.98).round().clamp(24.0, 64.0),
            _ => (h * 0.5).round().clamp(16.0, 96.0),
        };
        let raster_px = match icon {
            icon if icon.is_controller_tip() => (draw_px * 3.0).round().clamp(54.0, 192.0),
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

    unsafe fn draw_controller_art(
        &mut self,
        art: ControllerArt,
        rect: D2D_RECT_F,
    ) -> Result<(), String> {
        self.draw_controller_art_alpha(art, rect, 1.0)
    }

    unsafe fn draw_controller_art_alpha(
        &mut self,
        art: ControllerArt,
        rect: D2D_RECT_F,
        opacity: f32,
    ) -> Result<(), String> {
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

    #[cfg(feature = "gamepad")]
    unsafe fn draw_controller_model_alpha(
        &mut self,
        art: ControllerArt,
        rect: D2D_RECT_F,
        opacity: f32,
    ) -> Result<(), String> {
        const MODEL_W: u32 = 1000;
        const MODEL_H: u32 = 600;
        if !self.controller_model_cache.contains_key(&art) {
            let opt = resvg::usvg::Options::default();
            let tree = resvg::usvg::Tree::from_data(art.svg().as_bytes(), &opt)
                .map_err(|e| format!("parse controller model {art:?}: {e}"))?;
            let mut pixmap = resvg::tiny_skia::Pixmap::new(MODEL_W, MODEL_H)
                .ok_or_else(|| format!("alloc controller model pixmap {MODEL_W}x{MODEL_H}"))?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(1.0, 1.0),
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
                        width: MODEL_W,
                        height: MODEL_H,
                    },
                    Some(bgra.as_ptr() as *const core::ffi::c_void),
                    MODEL_W * 4,
                    &props,
                )
                .map_err(|e| format!("CreateBitmap controller model {art:?}: {e}"))?;
            self.controller_model_cache
                .insert(art, (bitmap, MODEL_W, MODEL_H));
        }

        let (bitmap, width, height) = self
            .controller_model_cache
            .get(&art)
            .ok_or_else(|| format!("missing controller model cache {art:?}"))?;
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

    #[cfg(feature = "gamepad")]
    unsafe fn draw_controller_model_overlay(
        &self,
        model: D2D_RECT_F,
        frame: &ControllerCenterFrame,
        family: ControllerIconFamily,
        surface_brush: &ID2D1SolidColorBrush,
        key_brush: &ID2D1SolidColorBrush,
        border_brush: &ID2D1SolidColorBrush,
        accent_brush: &ID2D1SolidColorBrush,
        active_brush: &ID2D1SolidColorBrush,
        ring_brush: &ID2D1SolidColorBrush,
        text_brush: &ID2D1SolidColorBrush,
        sel_text_brush: &ID2D1SolidColorBrush,
    ) {
        let playstation = family == ControllerIconFamily::Ps5;
        let pressed = |button: Button| {
            frame
                .bindings
                .iter()
                .any(|binding| binding.button == button && binding.pressed)
        };
        let selected = |button: Button| frame.selected == Some(button);

        for &button in MAPPABLE_BUTTONS {
            if matches!(button, Button::L3 | Button::R3) {
                continue;
            }
            let rect = controller_button_rect(model, button, playstation);
            let is_pressed = pressed(button);
            let is_selected = selected(button);
            let fill = if is_selected {
                accent_brush
            } else if is_pressed {
                active_brush
            } else {
                key_brush
            };
            let outline = if is_pressed || is_selected {
                ring_brush
            } else {
                border_brush
            };
            let label = if is_pressed || is_selected {
                sel_text_brush
            } else {
                text_brush
            };
            let stroke = if is_pressed || is_selected { 2.4 } else { 1.0 };
            let text = controller_visual_label(button, playstation);

            if matches!(
                button,
                Button::A | Button::B | Button::X | Button::Y | Button::Guide
            ) {
                let ellipse = D2D1_ELLIPSE {
                    point: D2D_POINT_2F {
                        x: (rect.left + rect.right) * 0.5,
                        y: (rect.top + rect.bottom) * 0.5,
                    },
                    radiusX: (rect.right - rect.left) * 0.5,
                    radiusY: (rect.bottom - rect.top) * 0.5,
                };
                self.d2d_context.FillEllipse(&ellipse, fill);
                self.d2d_context
                    .DrawEllipse(&ellipse, outline, stroke, None);
            } else {
                let rounded = D2D1_ROUNDED_RECT {
                    rect,
                    radiusX: (rect.bottom - rect.top) * 0.35,
                    radiusY: (rect.bottom - rect.top) * 0.35,
                };
                self.d2d_context.FillRoundedRectangle(&rounded, fill);
                self.d2d_context
                    .DrawRoundedRectangle(&rounded, outline, stroke, None);
            }
            draw_center_text(&self.d2d_context, text, &self.chip_format, &rect, label);
        }

        for (button, axis) in [
            (Button::L3, (frame.axes.0, frame.axes.1)),
            (Button::R3, (frame.axes.2, frame.axes.3)),
        ] {
            let rect = controller_button_rect(model, button, playstation);
            let base = D2D1_ELLIPSE {
                point: D2D_POINT_2F {
                    x: (rect.left + rect.right) * 0.5,
                    y: (rect.top + rect.bottom) * 0.5,
                },
                radiusX: (rect.right - rect.left) * 0.5,
                radiusY: (rect.bottom - rect.top) * 0.5,
            };
            self.d2d_context.FillEllipse(&base, surface_brush);
            self.d2d_context.DrawEllipse(&base, border_brush, 1.5, None);

            let x = axis.0.clamp(-1.0, 1.0);
            let y = axis.1.clamp(-1.0, 1.0);
            let center_x = base.point.x + x * base.radiusX * 0.52;
            let center_y = base.point.y - y * base.radiusY * 0.52;
            let knob = D2D1_ELLIPSE {
                point: D2D_POINT_2F {
                    x: center_x,
                    y: center_y,
                },
                radiusX: base.radiusX * 0.48,
                radiusY: base.radiusY * 0.48,
            };
            let is_pressed = pressed(button);
            let is_selected = selected(button);
            let fill = if is_selected {
                accent_brush
            } else if is_pressed {
                active_brush
            } else {
                accent_brush
            };
            self.d2d_context.FillEllipse(&knob, fill);
            self.d2d_context.DrawEllipse(
                &knob,
                if is_pressed || is_selected {
                    ring_brush
                } else {
                    border_brush
                },
                if is_pressed || is_selected { 2.4 } else { 1.0 },
                None,
            );
            let label = if is_pressed || is_selected {
                sel_text_brush
            } else {
                text_brush
            };
            draw_center_text(
                &self.d2d_context,
                controller_visual_label(button, playstation),
                &self.hint_format,
                &rect,
                label,
            );
        }
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
            (t * std::f32::consts::TAU * 1.1).sin() * 0.5 + 0.5
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
            controller_label,
            voice_available,
            voice_active,
            voice_phase,
            voice_level,
        } = *frame;
        let controller_icons = ControllerIconFamily::from_label(controller_label);
        let cw = self.width as f32;
        let ch = self.height as f32;

        self.d2d_context.BeginDraw();

        let rects = key_rects(cw, ch, scale_w, rows, top_inset);

        // Ease the focus ring toward the selected key. The accent fill still snaps
        // (so each key's label colour is unambiguous); only the bright ring glides,
        // which reads as the cursor moving. Frame-rate-independent via dt.
        if let Some(tgt) = rects
            .iter()
            .find(|kr| kr.pos.row == sel.row && kr.pos.col == sel.col)
            .map(|kr| D2D_RECT_F {
                left: kr.left,
                top: kr.top,
                right: kr.right,
                bottom: kr.bottom,
            })
        {
            let now = Instant::now();
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
        // Bright accent ring on the selected key — lifts it off the grid so the
        // cursor reads at a glance, not by fill colour alone. (A blurred outer
        // glow would need extra composition layers; deferred — the ring carries it.)
        let sel_ring_brush = solid_brush(
            &self.d2d_context,
            colorref(mix_color(0xFFFFFF, pal.accent, 0.5)),
        )?;

        for kr in &rects {
            let key = &rows[kr.pos.row].keys[kr.pos.col];
            let selected = sel.row == kr.pos.row && sel.col == kr.pos.col;
            // Radius scales with key height (6.8px @ 68px key).
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
            // Selected key: solid accent fill + inverted label.
            let (fill, label_brush) = if selected {
                (&accent_brush, &sel_text_brush)
            } else {
                (&key_brush, &text_brush)
            };
            let label_color = if selected { pal.sel_text } else { pal.text };
            self.d2d_context.FillRoundedRectangle(&rect, fill);
            // Non-selected keys get the subtle webview border; the selected key's
            // ring is drawn after the loop so it can glide between keys.
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
                // Tell the truth: dimmed mic-off when voice can't run here — on
                // Winlogon, or in userland with the optional whisper model not
                // installed. Otherwise a live mic, accent + a breathing halo while
                // it's actually listening.
                if !voice_available {
                    let disabled_color = colorref_mix(label_color, pal.key, 0.42);
                    self.draw_svg_icon(VkIcon::MicOff, rect.rect, disabled_color)?;
                } else if voice_active {
                    let cx = (rect.rect.left + rect.rect.right) * 0.5;
                    let cy = (rect.rect.top + rect.rect.bottom) * 0.5;
                    let unit = ((rect.rect.right - rect.rect.left)
                        .min(rect.rect.bottom - rect.rect.top)
                        * 0.56)
                        .max(1.0);
                    self.d2d_context.PushAxisAlignedClip(
                        &rect.rect,
                        windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    );
                    self.draw_voice_orb(
                        pal.accent,
                        voice_level,
                        matches!(voice_phase, VoicePhase::Transcribing),
                        cx,
                        cy,
                        unit,
                        1.25,
                    )?;
                    self.d2d_context.PopAxisAlignedClip();
                    let halo_alpha = match voice_phase {
                        VoicePhase::Starting => 0.18,
                        VoicePhase::Listening => 0.28,
                        VoicePhase::Transcribing => 0.46,
                    };
                    let halo =
                        solid_brush(&self.d2d_context, colorref_alpha(pal.accent, halo_alpha))?;
                    self.d2d_context
                        .DrawRoundedRectangle(&rect, &halo, 2.0, None);
                } else {
                    self.draw_svg_icon(VkIcon::Mic, rect.rect, label_color)?;
                }
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
            } else if matches!(key.action, KeyAction::PredictPrev) {
                self.draw_svg_icon(VkIcon::ChevronLeft, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::PredictNext) {
                self.draw_svg_icon(VkIcon::ChevronRight, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::CloseVk) && key.label.is_empty() {
                // The labeled close key ("Esc") falls through to the text path.
                self.draw_svg_icon(VkIcon::Close, rect.rect, label_color)?;
            } else if matches!(key.action, KeyAction::Shift) {
                self.draw_svg_icon(shift_icon(modifiers.shift), rect.rect, label_color)?;
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

            // Per-key controller-button badge in the top-left corner. Keep this
            // footprint fixed so controller glyphs do not collide with key glyphs.
            if let Some(hint) = key_hint(key) {
                let key_h = kr.bottom - kr.top;
                let badge_size = (key_h * 0.48).clamp(30.0, KEY_HINT_BADGE_MAX);
                let badge = D2D_RECT_F {
                    left: kr.left + KEY_HINT_BADGE_INSET,
                    top: kr.top + KEY_HINT_BADGE_INSET,
                    right: kr.left + KEY_HINT_BADGE_INSET + badge_size,
                    bottom: kr.top + KEY_HINT_BADGE_INSET + badge_size,
                };
                if let Some(icon) = controller_icons.hint_icon(hint) {
                    self.draw_svg_icon(icon, badge, pal.text)?;
                } else {
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
        }

        // Gliding focus ring on top of the keys, at the eased position.
        if let Some(ring) = self.anim_sel {
            let radius = (ring.bottom - ring.top) * RADIUS_FRAC;
            let rr = D2D1_ROUNDED_RECT {
                rect: ring,
                radiusX: radius,
                radiusY: radius,
            };
            self.d2d_context
                .DrawRoundedRectangle(&rr, &sel_ring_brush, 2.5, None);
        }

        // Suggestion pill last, so it floats on top of the keys (and the ring).
        if let Some(strip) = candidates {
            draw_candidate_strip(
                &self.d2d_context,
                cw,
                strip,
                &accent_brush,
                &text_brush,
                &sel_text_brush,
                &self.chip_format,
                &self.hint_format,
                pal,
                controller_icons,
            )?;
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

    /// Draw the native Controller Center. It intentionally reuses the keyboard
    /// renderer and bundled controller art, so it has no separate GUI runtime.
    #[cfg(feature = "gamepad")]
    pub unsafe fn draw_controller_center(
        &mut self,
        frame: &ControllerCenterFrame,
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        let pal = frame.palette;
        let stage = controller_center_stage_rect(cw, ch);
        let art_rect = controller_center_art_rect(cw, ch);
        let drawer_rect = controller_center_drawer_rect(cw, ch);
        let card_w = controller_center_card_width(cw);
        let surface = colorref_mix(pal.text, pal.key, 0.045);
        let muted = colorref_mix(pal.text, pal.bg, 0.62);

        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&colorref(pal.bg)));

        let surface_brush = solid_brush(&self.d2d_context, colorref(surface))?;
        let key_brush = solid_brush(&self.d2d_context, colorref(pal.key))?;
        let border_brush = solid_brush(&self.d2d_context, colorref(pal.border))?;
        let accent_brush = solid_brush(&self.d2d_context, colorref(pal.accent))?;
        let active_brush = solid_brush(
            &self.d2d_context,
            colorref(mix_color(0xffffff, pal.accent, 0.55)),
        )?;
        let ring_brush = solid_brush(
            &self.d2d_context,
            colorref(mix_color(0xffffff, pal.accent, 0.42)),
        )?;
        let text_brush = solid_brush(&self.d2d_context, colorref(pal.text))?;
        let sel_text_brush = solid_brush(&self.d2d_context, colorref(pal.sel_text))?;
        let muted_brush = solid_brush(&self.d2d_context, colorref(muted))?;

        let _ = self
            .prompt_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = self
            .prompt_format
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        draw_center_text(
            &self.d2d_context,
            "CONTROLLER CENTER",
            &self.prompt_format,
            &D2D_RECT_F {
                left: CENTER_PAGE_PAD,
                top: 10.0,
                right: (cw * 0.55).min(cw - 300.0),
                bottom: 44.0,
            },
            &text_brush,
        );

        let _ = self
            .chip_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        draw_center_text(
            &self.d2d_context,
            "Desktop mappings · paused automatically while a game owns the pad",
            &self.chip_format,
            &D2D_RECT_F {
                left: CENTER_PAGE_PAD + 1.0,
                top: 43.0,
                right: (cw * 0.64).min(cw - 280.0),
                bottom: 64.0,
            },
            &muted_brush,
        );

        let status = if frame.connected {
            let battery = if frame.wired {
                "Wired".to_string()
            } else if frame.battery_percent >= 0 {
                format!(
                    "{}%{}",
                    frame.battery_percent,
                    if frame.charging { " · charging" } else { "" }
                )
            } else {
                "Battery unknown".to_string()
            };
            let label = if frame.controller_label.is_empty() {
                "Controller"
            } else {
                frame.controller_label
            };
            format!("{} · {}", label, battery)
        } else {
            "Offline · mapping remains available".to_string()
        };
        let status_w = (cw * 0.30).clamp(280.0, 430.0);
        let status_left = (cw - status_w - CENTER_PAGE_PAD).max(CENTER_PAGE_PAD);
        let status_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: status_left,
                top: 16.0,
                right: (cw - CENTER_PAGE_PAD).max(status_left + status_w),
                bottom: 54.0,
            },
            radiusX: 19.0,
            radiusY: 19.0,
        };
        self.d2d_context
            .FillRoundedRectangle(&status_rect, &surface_brush);
        self.d2d_context.DrawRoundedRectangle(
            &status_rect,
            if frame.connected {
                &accent_brush
            } else {
                &border_brush
            },
            1.25,
            None,
        );
        let _ = self
            .chip_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        draw_center_text(
            &self.d2d_context,
            &status,
            &self.chip_format,
            &status_rect.rect,
            if frame.connected {
                &text_brush
            } else {
                &muted_brush
            },
        );

        let separator = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: CENTER_PAGE_PAD,
                top: CENTER_HEADER_BOTTOM,
                right: (cw - CENTER_PAGE_PAD).max(CENTER_PAGE_PAD + 2.0),
                bottom: CENTER_HEADER_BOTTOM + 1.0,
            },
            radiusX: 1.0,
            radiusY: 1.0,
        };
        self.d2d_context
            .FillRoundedRectangle(&separator, &border_brush);

        let _ = self
            .hint_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        for (label, left) in [("INPUTS", stage.left), ("ACTIONS", stage.right - card_w)] {
            draw_center_text(
                &self.d2d_context,
                label,
                &self.hint_format,
                &D2D_RECT_F {
                    left,
                    top: stage.top,
                    right: left + card_w,
                    bottom: stage.top + 22.0,
                },
                &muted_brush,
            );
        }

        let art_panel = D2D1_ROUNDED_RECT {
            rect: art_rect,
            radiusX: 22.0,
            radiusY: 22.0,
        };
        self.d2d_context
            .FillRoundedRectangle(&art_panel, &surface_brush);
        self.d2d_context
            .DrawRoundedRectangle(&art_panel, &border_brush, 1.0, None);
        draw_center_text(
            &self.d2d_context,
            if frame.connected {
                "LIVE CONTROLLER"
            } else {
                "CONTROLLER PREVIEW"
            },
            &self.hint_format,
            &D2D_RECT_F {
                left: art_rect.left,
                top: art_rect.top + 11.0,
                right: art_rect.right,
                bottom: art_rect.top + 31.0,
            },
            &accent_brush,
        );
        let art =
            ControllerArt::from_label(frame.controller_label).unwrap_or(ControllerArt::DualSense);
        let family = match art {
            ControllerArt::DualSense => ControllerIconFamily::Ps5,
            ControllerArt::XboxOne => ControllerIconFamily::Xbox,
        };
        let model_label = if frame.connected {
            match family {
                ControllerIconFamily::Ps5 => "PLAYSTATION · DUALSENSE",
                ControllerIconFamily::Xbox => "XBOX CONTROLLER",
            }
        } else {
            "Connect a controller to see live input"
        };
        draw_center_text(
            &self.d2d_context,
            model_label,
            &self.chip_format,
            &D2D_RECT_F {
                left: art_rect.left + 18.0,
                top: art_rect.top + 31.0,
                right: art_rect.right - 18.0,
                bottom: art_rect.top + 52.0,
            },
            &muted_brush,
        );
        let model_rect = controller_center_model_rect(cw, ch);
        self.draw_controller_model_alpha(
            art,
            model_rect,
            if frame.connected { 1.0 } else { 0.72 },
        )?;
        self.draw_controller_model_overlay(
            model_rect,
            frame,
            family,
            &surface_brush,
            &key_brush,
            &border_brush,
            &accent_brush,
            &active_brush,
            &ring_brush,
            &text_brush,
            &sel_text_brush,
        );
        let telemetry = if frame.connected {
            format!(
                "L {:>+.2}, {:>+.2}     R {:>+.2}, {:>+.2}",
                frame.axes.0, frame.axes.1, frame.axes.2, frame.axes.3
            )
        } else {
            "Mapping is available while the controller is offline".to_string()
        };
        draw_center_text(
            &self.d2d_context,
            &telemetry,
            &self.chip_format,
            &D2D_RECT_F {
                left: art_rect.left,
                top: art_rect.bottom - 34.0,
                right: art_rect.right,
                bottom: art_rect.bottom - 10.0,
            },
            &muted_brush,
        );

        for binding in frame.bindings {
            let Some(rect) = controller_center_card_rect(binding.button, cw, ch) else {
                continue;
            };
            let selected = frame.selected == Some(binding.button);
            let rounded = D2D1_ROUNDED_RECT {
                rect,
                radiusX: 9.0,
                radiusY: 9.0,
            };
            let fill = if selected {
                &accent_brush
            } else if binding.pressed {
                &active_brush
            } else {
                &key_brush
            };
            let label_brush = if binding.pressed || selected {
                &sel_text_brush
            } else {
                &text_brush
            };
            self.d2d_context.FillRoundedRectangle(&rounded, fill);
            self.d2d_context.DrawRoundedRectangle(
                &rounded,
                if selected { &ring_brush } else { &border_brush },
                if selected { 2.0 } else { 1.25 },
                None,
            );
            let badge_width = controller_center_badge_width(binding.button, frame.controller_label);
            let badge = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: rect.left + 6.0,
                    top: rect.top + 5.0,
                    right: (rect.left + 6.0 + badge_width)
                        .min(rect.right - 54.0)
                        .max(rect.left + 40.0),
                    bottom: rect.bottom - 5.0,
                },
                radiusX: 7.0,
                radiusY: 7.0,
            };
            self.d2d_context.FillRoundedRectangle(
                &badge,
                if binding.pressed || selected {
                    &surface_brush
                } else {
                    &accent_brush
                },
            );
            draw_center_text(
                &self.d2d_context,
                controller_button_label(binding.button, frame.controller_label),
                &self.chip_format,
                &badge.rect,
                if binding.pressed || selected {
                    &text_brush
                } else {
                    &sel_text_brush
                },
            );
            if binding.pressed {
                let pressed_mark = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: rect.left + 2.0,
                        top: rect.top + 5.0,
                        right: rect.left + 5.0,
                        bottom: rect.bottom - 5.0,
                    },
                    radiusX: 2.0,
                    radiusY: 2.0,
                };
                self.d2d_context
                    .FillRoundedRectangle(&pressed_mark, &accent_brush);
            }
            let _ = self
                .chip_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            draw_center_text(
                &self.d2d_context,
                binding.action,
                &self.chip_format,
                &D2D_RECT_F {
                    left: badge.rect.right + 8.0,
                    top: rect.top,
                    right: rect.right - 10.0,
                    bottom: rect.bottom,
                },
                label_brush,
            );
            let _ = self
                .chip_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        }

        let drawer = D2D1_ROUNDED_RECT {
            rect: drawer_rect,
            radiusX: 18.0,
            radiusY: 18.0,
        };
        self.d2d_context
            .FillRoundedRectangle(&drawer, &surface_brush);
        self.d2d_context
            .DrawRoundedRectangle(&drawer, &border_brush, 1.0, None);

        if let Some(button) = frame.selected {
            draw_center_wizard(
                &self.d2d_context,
                cw,
                ch,
                &drawer,
                frame,
                button,
                &key_brush,
                &accent_brush,
                &ring_brush,
                &border_brush,
                &text_brush,
                &sel_text_brush,
                &muted_brush,
                &self.hint_format,
                &self.chip_format,
            )?;
        } else {
            let _ = self
                .chip_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            draw_center_text(
                &self.d2d_context,
                "Select a control to set up a keyboard shortcut, app, or workspace.",
                &self.chip_format,
                &D2D_RECT_F {
                    left: drawer.rect.left + 18.0,
                    top: drawer.rect.top + 22.0,
                    right: controller_center_deadzone_rect(cw, ch).left - 26.0,
                    bottom: drawer.rect.top + 47.0,
                },
                &muted_brush,
            );
            if !frame.input.is_empty() {
                draw_center_text(
                    &self.d2d_context,
                    frame.input,
                    &self.chip_format,
                    &D2D_RECT_F {
                        left: drawer.rect.left + 18.0,
                        top: drawer.rect.top + 58.0,
                        right: controller_center_deadzone_rect(cw, ch).left - 26.0,
                        bottom: drawer.rect.top + 81.0,
                    },
                    &muted_brush,
                );
            }
            let _ = self
                .chip_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        }

        let slider = controller_center_deadzone_rect(cw, ch);
        let slider_label = format!(
            "Cursor deadzone: {:.0}%",
            frame.deadzone.clamp(0.0, CENTER_DEADZONE_MAX) * 100.0
        );
        draw_center_text(
            &self.d2d_context,
            &slider_label,
            &self.chip_format,
            &D2D_RECT_F {
                left: slider.left,
                top: slider.top - 24.0,
                right: slider.right,
                bottom: slider.top - 3.0,
            },
            &muted_brush,
        );
        let slider_rr = D2D1_ROUNDED_RECT {
            rect: slider,
            radiusX: 8.0,
            radiusY: 8.0,
        };
        self.d2d_context
            .FillRoundedRectangle(&slider_rr, &border_brush);
        let filled = D2D_RECT_F {
            right: (slider.left
                + (slider.right - slider.left)
                    * (frame.deadzone.clamp(0.0, CENTER_DEADZONE_MAX) / CENTER_DEADZONE_MAX))
                .max(slider.left + 1.0),
            ..slider
        };
        let filled_rr = D2D1_ROUNDED_RECT {
            rect: filled,
            radiusX: 8.0,
            radiusY: 8.0,
        };
        self.d2d_context
            .FillRoundedRectangle(&filled_rr, &accent_brush);

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
            .CreateTextLayout(&wide, format, f32::MAX, f32::MAX)
            .ok();
        let Some(layout) = layout else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_err() {
            return 0.0;
        }
        m.widthIncludingTrailingWhitespace
    }

    /// One-surface morph between the small "open keyboard" pill and the
    /// controller connection card. `card_t`: 0 = prompt, 1 = card.
    pub unsafe fn draw_prompt_card_morph(
        &mut self,
        bg: u32,
        border: u32,
        text_color: u32,
        prefix: &str,
        suffix: &str,
        show_l3: bool,
        title: &str,
        controller_label: &str,
        card_t: f32,
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        let card_t = card_t.clamp(0.0, 1.0);
        let prompt_alpha = 1.0 - card_t;
        let card_alpha = card_t;
        let identity = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };
        self.d2d_context.SetTransform(&identity);

        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let elapsed = self.prompt_started.elapsed().as_secs_f32();
        let pulse = (elapsed * 0.54).fract();
        let pulse_alpha = (1.0 - pulse).powi(2);
        let prompt_panel = D2D_RECT_F {
            left: 4.0,
            top: 4.0,
            right: cw - 4.0,
            bottom: ch - 4.0,
        };
        // Card interior was tuned for a 210px card; scale with window height so the
        // morph lands on the same size as `draw_connected_prompt`.
        let s = ch / 210.0;
        let card_top = (44.0 * s).min(ch - 14.0).max(4.0);
        let card_w = (196.0 * s).min(cw - 10.0).max(20.0);
        let card_left = (cw - card_w) * 0.5;
        let card_panel = D2D_RECT_F {
            left: card_left,
            top: card_top,
            right: cw - card_left,
            bottom: ch - 5.0 * s,
        };
        let panel = lerp_rect(prompt_panel, card_panel, card_t);
        let rounded = D2D1_ROUNDED_RECT {
            rect: panel,
            radiusX: lerp((ch * 0.5 - 2.0).max(8.0), 28.0 * s, card_t),
            radiusY: lerp((ch * 0.5 - 2.0).max(8.0), 28.0 * s, card_t),
        };
        let glow = colorref_mix(0x00FFFFFF, border, lerp(0.38, 0.45, card_t));
        let bg_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(bg, lerp(1.0, 0.94, card_t)),
        )?;
        let glow_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(glow, lerp(0.30, 0.22, card_t) * pulse_alpha),
        )?;
        let border_brush = solid_brush(
            &self.d2d_context,
            colorref_alpha(glow, lerp(1.0, 0.84, card_t)),
        )?;
        self.d2d_context.FillRoundedRectangle(&rounded, &bg_brush);
        self.d2d_context
            .DrawRoundedRectangle(&rounded, &glow_brush, 2.0 + 10.0 * pulse, None);
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
            let chip = (ch * 0.70).clamp(26.0, 96.0);
            let gap = 12.0;
            let w_prefix = self.measure_text(prefix, &self.prompt_format);
            let w_suffix = self.measure_text(suffix, &self.prompt_format);
            let total = if show_l3 {
                w_prefix + gap + chip + gap + w_suffix
            } else {
                w_prefix
            };
            let mut x = ((cw - total) * 0.5).max(0.0);
            let text_brush =
                solid_brush(&self.d2d_context, colorref_alpha(text_color, prompt_alpha))?;
            let pre: Vec<u16> = prefix.encode_utf16().collect();
            self.d2d_context.DrawText(
                &pre,
                &self.prompt_format,
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
                let chip_rect = D2D_RECT_F {
                    left: x,
                    top: (ch - chip) * 0.5,
                    right: x + chip,
                    bottom: (ch + chip) * 0.5,
                };
                let icon = ControllerIconFamily::from_label(controller_label).l3_icon();
                self.draw_svg_icon_alpha(icon, chip_rect, text_color, prompt_alpha)?;
                x += chip + gap;
            }
            if !suffix.is_empty() {
                let suf: Vec<u16> = suffix.encode_utf16().collect();
                self.d2d_context.DrawText(
                    &suf,
                    &self.prompt_format,
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
        }

        if card_alpha > 0.01 {
            let _ = self
                .text_format
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = self
                .text_format
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = self
                .text_format
                .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let title_w: Vec<u16> = title.encode_utf16().collect();
            let text_brush =
                solid_brush(&self.d2d_context, colorref_alpha(text_color, card_alpha))?;
            let name_bottom = (card_top - 6.0 * s).max(12.0);
            self.d2d_context.DrawText(
                &title_w,
                &self.text_format,
                &D2D_RECT_F {
                    left: 0.0,
                    top: 8.0 * s,
                    right: cw,
                    bottom: name_bottom,
                },
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            let _ = self.text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);

            let image_cx = cw * 0.5;
            let image_cy = (panel.top + panel.bottom) * 0.5 - 6.0 * (1.0 - card_t);
            let image_scale = (0.88 + 0.12 * card_t) * s;
            let ring_brush = solid_brush(
                &self.d2d_context,
                colorref_alpha(glow, 0.20 * card_alpha * pulse_alpha),
            )?;
            let ring = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: image_cx - 74.0 * image_scale - 10.0 * pulse,
                    top: image_cy - 60.0 * image_scale - 10.0 * pulse,
                    right: image_cx + 74.0 * image_scale + 10.0 * pulse,
                    bottom: image_cy + 60.0 * image_scale + 10.0 * pulse,
                },
                radiusX: 52.0 * image_scale + 10.0 * pulse,
                radiusY: 52.0 * image_scale + 10.0 * pulse,
            };
            self.d2d_context
                .DrawRoundedRectangle(&ring, &ring_brush, 2.0, None);
            let image_rect = D2D_RECT_F {
                left: image_cx - 62.0 * image_scale,
                top: image_cy - 54.0 * image_scale,
                right: image_cx + 62.0 * image_scale,
                bottom: image_cy + 54.0 * image_scale,
            };
            if let Some(art) = ControllerArt::from_label(controller_label) {
                self.draw_controller_art_alpha(art, image_rect, card_alpha)?;
            } else {
                self.draw_svg_icon_alpha(VkIcon::Gamepad, image_rect, text_color, card_alpha)?;
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

    /// Draw an AirPods-style connection card with a controller image.
    /// Kept D2D-only so the secure-desktop service path does not need asset IO or
    /// a separate 3D runtime.
    pub unsafe fn draw_connected_prompt(
        &mut self,
        bg: u32,
        border: u32,
        text_color: u32,
        title: &str,
        controller_label: &str,
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

        // Leave a transparent band at the top so the controller name renders
        // *outside* (above) the card. The card itself is a narrow pill that only
        // hugs the controller art; it is centred in the (wider) window so the name
        // above has room to render without clipping.
        // Interior geometry was tuned for a 210px-tall card; scale it with the
        // actual window height so a bigger card enlarges the art/ring/name too.
        let s = ch / 210.0;
        let card_top = 44.0 * s;
        let card_w = (196.0 * s).min(cw - 10.0);
        let card_left = (cw - card_w) * 0.5;
        let panel = D2D_RECT_F {
            left: card_left,
            top: card_top,
            right: cw - card_left,
            bottom: ch - 5.0 * s,
        };
        let rounded = D2D1_ROUNDED_RECT {
            rect: panel,
            radiusX: 28.0 * s,
            radiusY: 28.0 * s,
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

        let image_cx = cw * 0.5;
        // The controller name floats *above* the card on the transparent top band;
        // the artwork is the sole content of the card, centred in it.
        let name_top = 8.0 * s;
        let name_bottom = card_top - 6.0 * s;
        let image_cy = (panel.top + panel.bottom) * 0.5 - 6.0 * (1.0 - eased);

        let ring = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: image_cx - (74.0 + 10.0 * pulse) * s,
                top: image_cy - (60.0 + 10.0 * pulse) * s,
                right: image_cx + (74.0 + 10.0 * pulse) * s,
                bottom: image_cy + (60.0 + 10.0 * pulse) * s,
            },
            radiusX: (52.0 + 10.0 * pulse) * s,
            radiusY: (52.0 + 10.0 * pulse) * s,
        };
        self.d2d_context
            .DrawRoundedRectangle(&ring, &halo_brush, 2.0, None);
        let image_rect = D2D_RECT_F {
            left: image_cx - 62.0 * s,
            top: image_cy - 54.0 * s,
            right: image_cx + 62.0 * s,
            bottom: image_cy + 54.0 * s,
        };
        if let Some(art) = ControllerArt::from_label(controller_label) {
            self.draw_controller_art(art, image_rect)?;
        } else {
            self.draw_svg_icon(VkIcon::Gamepad, image_rect, text_color)?;
        }

        let title_w: Vec<u16> = title.encode_utf16().collect();
        let text_brush = solid_brush(&self.d2d_context, colorref(text_color))?;
        // The name is a single line above the card; without this it word-wraps and
        // the short band clips all but the first word. Restore wrap after (the
        // format is shared with the keyboard label renderer).
        let _ = self
            .text_format
            .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        self.d2d_context.DrawText(
            &title_w,
            &self.text_format,
            &D2D_RECT_F {
                left: 0.0,
                top: name_top,
                right: cw,
                bottom: name_bottom,
            },
            &text_brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        let _ = self.text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);

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
        controller_label: &str,
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        // Segments flow left to right, top-aligned to a shared baseline band.
        let _ = self
            .prompt_format
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = self
            .prompt_format
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
        let chip = (ch * 0.70).clamp(26.0, 96.0);
        let gap = 12.0;
        let w_prefix = self.measure_text(prefix, &self.prompt_format);
        let w_suffix = self.measure_text(suffix, &self.prompt_format);
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
            &self.prompt_format,
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
            let icon = ControllerIconFamily::from_label(controller_label).l3_icon();
            self.draw_svg_icon(icon, chip_rect, text_color)?;
            x += chip + gap;
        }

        // Suffix.
        if !suffix.is_empty() {
            let suf: Vec<u16> = suffix.encode_utf16().collect();
            self.d2d_context.DrawText(
                &suf,
                &self.prompt_format,
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

    /// Subtle, audio-reactive voice glow for the right-edge overlay: soft concentric
    /// rings + a core dot that grow/brighten with `level` (live mic energy, 0..1).
    /// `transcribing` swaps the live level for a gentle auto-pulse while it works.
    pub unsafe fn draw_voice(
        &mut self,
        accent: u32,
        level: f32,
        transcribing: bool,
    ) -> Result<(), String> {
        let cw = self.width as f32;
        let ch = self.height as f32;
        self.d2d_context.BeginDraw();
        self.d2d_context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let cx = cw * 0.5;
        let cy = ch * 0.5;
        let unit = cw.min(ch) * 0.5;
        self.draw_voice_orb(accent, level, transcribing, cx, cy, unit, 1.0)?;

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
/// width at the 92px reference (scaled by `client_w/1920`), holding the
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
    fn generic_gamepad_icon_matches_the_renderer_unit_size() {
        let svg = VkIcon::Gamepad.svg();
        assert!(svg.contains("width=\"24\" height=\"24\""));
    }

    #[test]
    fn controller_models_are_embedded_valid_svg() {
        for art in [ControllerArt::DualSense, ControllerArt::XboxOne] {
            assert!(resvg::usvg::Tree::from_data(
                art.svg().as_bytes(),
                &resvg::usvg::Options::default(),
            )
            .is_ok());
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

    #[cfg(feature = "gamepad")]
    #[test]
    fn controller_center_hit_regions_match_the_drawn_cards_and_slider() {
        let width = 1120.0;
        let height = 760.0;
        let lt = controller_center_card_rect(Button::Lt, width, height).unwrap();
        assert_eq!(
            controller_center_hit(
                (lt.left + lt.right) * 0.5,
                (lt.top + lt.bottom) * 0.5,
                width,
                height,
                None,
            ),
            Some(ControllerCenterHit::Button(Button::Lt))
        );
        let guide = controller_center_card_rect(Button::Guide, width, height).unwrap();
        assert_eq!(
            controller_center_hit(
                (guide.left + guide.right) * 0.5,
                (guide.top + guide.bottom) * 0.5,
                width,
                height,
                None,
            ),
            Some(ControllerCenterHit::Button(Button::Guide))
        );
        let launch = controller_center_action_rect(1, width, height);
        assert_eq!(
            controller_center_hit(
                (launch.left + launch.right) * 0.5,
                (launch.top + launch.bottom) * 0.5,
                width,
                height,
                Some(Button::A),
            ),
            Some(ControllerCenterHit::Action(DesktopActionKind::Launch))
        );
        let slider = controller_center_deadzone_rect(width, height);
        assert_eq!(
            controller_center_hit(
                slider.left + (slider.right - slider.left) * 0.5,
                slider.top + (slider.bottom - slider.top) * 0.5,
                width,
                height,
                None,
            ),
            Some(ControllerCenterHit::Deadzone(30))
        );
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn controller_center_diagram_hits_both_layout_positions() {
        let width = 1120.0;
        let height = 760.0;
        let model = controller_center_model_rect(width, height);
        for &button in MAPPABLE_BUTTONS {
            for playstation in [false, true] {
                let rect = controller_button_rect(model, button, playstation);
                assert_eq!(
                    controller_center_hit(
                        (rect.left + rect.right) * 0.5,
                        (rect.top + rect.bottom) * 0.5,
                        width,
                        height,
                        None,
                    ),
                    Some(ControllerCenterHit::Button(button)),
                    "diagram hit missed {button:?} playstation={playstation}"
                );
            }
        }
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn controller_center_model_rect_matches_svg_aspect_at_resizes() {
        for (width, height) in [(1120.0, 760.0), (800.0, 620.0)] {
            let rect = controller_center_model_rect(width, height);
            let aspect = (rect.right - rect.left) / (rect.bottom - rect.top);
            assert!(
                (aspect - 5.0 / 3.0).abs() < 0.001,
                "{width}x{height}: {aspect}"
            );
        }
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn controller_center_wizard_hits_each_visible_primary_control() {
        let width = 1120.0;
        let height = 760.0;
        let center = |rect: D2D_RECT_F| {
            (
                (rect.left + rect.right) * 0.5,
                (rect.top + rect.bottom) * 0.5,
            )
        };
        let trigger_state = ControllerCenterHitState {
            step: ControllerCenterStep::Trigger,
            action: None,
            app_rows: 0,
            app_can_scroll_up: false,
            app_can_scroll_down: false,
            workspace_rows: 0,
            workspace_can_scroll_up: false,
            workspace_can_scroll_down: false,
        };
        let trigger = center(controller_center_wizard_trigger_rect(width, height));
        assert_eq!(
            controller_center_hit_with_wizard(
                trigger.0,
                trigger.1,
                width,
                height,
                Some(Button::A),
                trigger_state,
            ),
            Some(ControllerCenterHit::TriggerCapture)
        );
        let continue_button = center(controller_center_wizard_footer_button_rect(
            3, width, height,
        ));
        assert_eq!(
            controller_center_hit_with_wizard(
                continue_button.0,
                continue_button.1,
                width,
                height,
                Some(Button::A),
                trigger_state,
            ),
            Some(ControllerCenterHit::Continue)
        );

        let action_state = ControllerCenterHitState {
            step: ControllerCenterStep::Action,
            action: Some(DesktopActionKind::Launch),
            ..trigger_state
        };
        let action = center(controller_center_wizard_action_rect(1, width, height));
        assert_eq!(
            controller_center_hit_with_wizard(
                action.0,
                action.1,
                width,
                height,
                Some(Button::A),
                action_state,
            ),
            Some(ControllerCenterHit::Action(DesktopActionKind::Launch))
        );
        for (index, expected) in [
            (0, ControllerCenterHit::Back),
            (1, ControllerCenterHit::Cancel),
            (2, ControllerCenterHit::Clear),
        ] {
            let rect = center(controller_center_wizard_footer_button_rect(
                index, width, height,
            ));
            assert_eq!(
                controller_center_hit_with_wizard(
                    rect.0,
                    rect.1,
                    width,
                    height,
                    Some(Button::A),
                    action_state,
                ),
                Some(expected)
            );
        }

        let configure_state = ControllerCenterHitState {
            step: ControllerCenterStep::Configure,
            action: Some(DesktopActionKind::Launch),
            app_rows: 4,
            app_can_scroll_down: true,
            ..trigger_state
        };
        let search = center(controller_center_wizard_field_rect(width, height));
        assert_eq!(
            controller_center_hit_with_wizard(
                search.0,
                search.1,
                width,
                height,
                Some(Button::A),
                configure_state,
            ),
            Some(ControllerCenterHit::AppSearch)
        );
        let app = center(controller_center_wizard_list_row_rect(0, width, height));
        assert_eq!(
            controller_center_hit_with_wizard(
                app.0,
                app.1,
                width,
                height,
                Some(Button::A),
                configure_state,
            ),
            Some(ControllerCenterHit::AppRow(0))
        );
        let down = center(controller_center_wizard_scroll_rect(false, width, height));
        assert_eq!(
            controller_center_hit_with_wizard(
                down.0,
                down.1,
                width,
                height,
                Some(Button::A),
                configure_state,
            ),
            Some(ControllerCenterHit::AppScrollDown)
        );
        let workspace_state = ControllerCenterHitState {
            action: Some(DesktopActionKind::Workspace),
            workspace_rows: 4,
            workspace_can_scroll_down: true,
            ..configure_state
        };
        let workspace_row = center(controller_center_wizard_list_row_rect(0, width, height));
        assert_eq!(
            controller_center_hit_with_wizard(
                workspace_row.0,
                workspace_row.1,
                width,
                height,
                Some(Button::A),
                workspace_state,
            ),
            Some(ControllerCenterHit::WorkspaceRow(0))
        );
        let save = center(controller_center_wizard_footer_button_rect(
            3, width, height,
        ));
        assert_eq!(
            controller_center_hit_with_wizard(
                save.0,
                save.1,
                width,
                height,
                Some(Button::A),
                configure_state,
            ),
            Some(ControllerCenterHit::Save)
        );
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn controller_center_wizard_draw_regions_fit_practical_window_sizes() {
        for (width, height) in [(1120.0, 760.0), (800.0, 620.0)] {
            let drawer = controller_center_drawer_rect(width, height);
            let content = controller_center_wizard_content_rect(width, height);
            let footer = controller_center_wizard_footer_button_rect(3, width, height);
            let last_row = controller_center_wizard_list_row_rect(2, width, height);
            assert!(content.right < controller_center_deadzone_rect(width, height).left);
            assert!(last_row.bottom < footer.top);
            assert!(footer.bottom <= drawer.bottom);
            assert!(controller_center_wizard_action_rect(2, width, height).bottom < footer.top);
        }
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

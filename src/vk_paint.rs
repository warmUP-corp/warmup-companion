use crate::config::VkStyle;
use crate::mono_ui::{Gfx, Rect};

pub fn colorref_hex(c: u32) -> String {
    let r = c & 0xff;
    let g = (c >> 8) & 0xff;
    let b = (c >> 16) & 0xff;
    format!("#{r:02X}{g:02X}{b:02X}")
}

pub fn colorref_mix(fg: u32, bg: u32, amount: f32) -> u32 {
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
pub fn shift_hue(c: u32, deg: f32) -> u32 {
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


pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}


pub const BLUR_SHADOW_STEPS: usize = 6;

pub fn blur_shadow_layers(blur: f32, alpha: f32) -> [(f32, f32); BLUR_SHADOW_STEPS] {
    let n = BLUR_SHADOW_STEPS as f32;
    let per = 1.0 - (1.0 - alpha.clamp(0.0, 1.0)).powf(1.0 / n);
    let mut out = [(0.0, 0.0); BLUR_SHADOW_STEPS];
    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f32 / (n - 1.0);
        *slot = (blur * (-0.25 + 0.75 * t), per);
    }
    out
}


#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VkPalette {
    pub bg: u32,
    pub key: u32,
    pub key_action: u32,
    pub key_enter: u32,
    pub enter_text: u32,
    pub mod_tint: Option<(u32, u32)>,
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

pub const fn rgb(v: u32) -> u32 {
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
            key_enter: rgb(0x1F2029),
            enter_text: rgb(0xFFFFFF),
            mod_tint: None,
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
            key_enter: rgb(0xE3E5EB),
            enter_text: rgb(0x111217),
            mod_tint: None,
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
            key_enter: rgb(0x2C2C2E),
            enter_text: rgb(0xFFFFFF),
            mod_tint: None,
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
            key_enter: rgb(0xABB0BA),
            enter_text: rgb(0x000000),
            mod_tint: None,
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
        (VkStyle::Modern, true) => VkPalette {
            bg: rgb(0x1B1F26),
            key: rgb(0x2E333B),
            key_action: rgb(0x3A404A),
            key_enter: rgb(0x3F3E54),
            enter_text: rgb(0xE8EAED),
            mod_tint: Some((rgb(0x433F5C), rgb(0xC9B8FF))),
            accent: rgb(0xE8EAED),
            sel_ring: rgb(0xFFFFFF),
            text: rgb(0xE8EAED),
            text_dim: rgb(0xBDC1C6),
            sel_text: rgb(0x1B1F26),
            border: rgb(0x4A505A),
            panel_stroke: rgb(0xFFFFFF),
            panel_stroke_alpha: 0x14 as f32 / 255.0,
            chip_sel: rgb(0xE8EAED),
        },
        (VkStyle::Modern, false) => VkPalette {
            bg: rgb(0xF1F3F4),
            key: rgb(0xFFFFFF),
            key_action: rgb(0xE8EAED),
            key_enter: rgb(0xECE7F5),
            enter_text: rgb(0x202124),
            mod_tint: Some((rgb(0xE4DCF8), rgb(0x7B61D9))),
            accent: rgb(0x202124),
            sel_ring: rgb(0x202124),
            text: rgb(0x202124),
            text_dim: rgb(0x5F6368),
            sel_text: rgb(0xFFFFFF),
            border: rgb(0xDADCE0),
            panel_stroke: rgb(0x000000),
            panel_stroke_alpha: 0x12 as f32 / 255.0,
            chip_sel: rgb(0x202124),
        },
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripLook {
    Pills,
    Columns,
    Chips,
}


#[derive(Clone, Copy)]
pub struct StyleSpec {
    pub(crate) design_kh: f32,
    pub(crate) key_aspect: f32,
    pub(crate) gap: f32,
    pub(crate) key_radius: f32,
    pub(crate) pad_x: f32,
    pub(crate) pad_y: f32,
    pub(crate) panel_radius: f32,
    pub(crate) families: &'static [&'static str],
    pub(crate) label_px: f32,
    pub(crate) label_weight: i32,
    pub(crate) word_px: f32,
    pub(crate) word_weight: i32,
    pub(crate) number_px: f32,
    pub(crate) number_alpha: f32,
    pub(crate) number_cy: f32,
    pub(crate) corner_sublabel: bool,
    pub(crate) tonal_function_keys: bool,
    pub(crate) label_cy: f32,
    pub(crate) caption_cy: f32,
    pub(crate) space_icon_cy: f32,
    pub(crate) space_label: Option<&'static str>,
    pub(crate) space_label_px: f32,
    pub(crate) trim_symbols_label: bool,
    pub(crate) icon_px: f32,
    pub(crate) icon_large_px: f32,
    pub(crate) space_icon_px: f32,
    pub(crate) hint_badge: f32,
    pub(crate) hint_inset: f32,
    pub(crate) key_shadow_dy: f32,
    pub(crate) key_shadow_alpha: f32,
    pub(crate) sel_shadow: Option<(f32, f32, f32)>,
    pub(crate) sel_ring_w: f32,
    pub(crate) strip: StripLook,
    pub(crate) chip_slots: usize,
    pub(crate) strip_bar_h: f32,
    pub(crate) chip_px: f32,
    pub(crate) chip_sel_weight: i32,
    pub(crate) chip_text_alpha: f32,
    pub(crate) strip_button_w: f32,
    pub(crate) strip_button_h: f32,
    pub(crate) strip_button_gap: f32,
    pub(crate) strip_hint_px: f32,
    pub(crate) chips_h: f32,
    pub(crate) chips_pad: f32,
    pub(crate) chips_gap: f32,
    pub(crate) separator_h: f32,
    pub(crate) separator_alpha: f32,
}


pub static NORMAL_SPEC: StyleSpec = StyleSpec {
    design_kh: 102.0,
    key_aspect: 102.0 / 138.0,
    gap: 6.0,
    key_radius: 12.0,
    pad_x: 17.0,
    pad_y: 18.0,
    panel_radius: 25.0,
    families: &["Noto Sans", "Segoe UI"],
    label_px: 38.0,
    label_weight: 500,
    word_px: 26.0,
    word_weight: 500,
    number_px: 14.0,
    number_alpha: 0x8C as f32 / 255.0,
    number_cy: 25.0,
    corner_sublabel: false,
    tonal_function_keys: false,
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
    chip_sel_weight: 600,
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


pub const MONO_FAMILIES: &[&str] =
    &["Inter", "SF Pro Text", "Segoe UI Variable Text", "Segoe UI"];


pub static MONO_SPEC: StyleSpec = StyleSpec {
    design_kh: 100.0,
    key_aspect: 100.0 / 137.0,
    gap: 8.0,
    key_radius: 12.0,
    pad_x: 13.0,
    pad_y: 13.0,
    panel_radius: 24.0,
    families: MONO_FAMILIES,
    label_px: 32.0,
    label_weight: 400,
    word_px: 24.0,
    word_weight: 400,
    number_px: 14.0,
    number_alpha: 0x99 as f32 / 255.0,
    number_cy: 30.5,
    corner_sublabel: false,
    tonal_function_keys: false,
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
    chip_sel_weight: 400,
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


pub const MODERN_FAMILIES: &[&str] = &[
    "Roboto",
    "Google Sans",
    "Segoe UI Variable Text",
    "Segoe UI",
];


pub static MODERN_SPEC: StyleSpec = StyleSpec {
    design_kh: 102.0,
    key_aspect: 102.0 / 138.0,
    gap: 19.0,
    key_radius: 10.0,
    pad_x: 20.0,
    pad_y: 20.0,
    panel_radius: 20.0,
    families: MODERN_FAMILIES,
    label_px: 53.0,
    label_weight: 400,
    word_px: 34.0,
    word_weight: 500,
    number_px: 29.0,
    number_alpha: 0xB3 as f32 / 255.0,
    number_cy: 25.0,
    corner_sublabel: true,
    tonal_function_keys: true,
    label_cy: 63.0,
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
    key_shadow_dy: 0.0,
    key_shadow_alpha: 0.0,
    sel_shadow: Some((6.0, 22.0, 0x8C as f32 / 255.0)),
    sel_ring_w: 2.0,
    strip: StripLook::Chips,
    chip_slots: 5,
    strip_bar_h: 128.0,
    chip_px: 38.0,
    chip_sel_weight: 500,
    chip_text_alpha: 0xE6 as f32 / 255.0,
    strip_button_w: 60.0,
    strip_button_h: 48.0,
    strip_button_gap: 18.0,
    strip_hint_px: 36.0,
    chips_h: 94.0,
    chips_pad: 0.0,
    chips_gap: 16.0,
    separator_h: 0.0,
    separator_alpha: 0.0,
};


pub const fn tv_spec(base: &StyleSpec) -> StyleSpec {
    StyleSpec {
        key_radius: base.key_radius,
        panel_radius: base.panel_radius,
        families: base.families,
        label_weight: base.label_weight,
        word_weight: base.word_weight,
        number_alpha: base.number_alpha,
        tonal_function_keys: base.tonal_function_keys,
        caption_cy: base.caption_cy,
        space_icon_cy: base.space_icon_cy,
        space_label: base.space_label,
        space_label_px: base.space_label_px,
        trim_symbols_label: base.trim_symbols_label,
        key_shadow_dy: base.key_shadow_dy,
        key_shadow_alpha: base.key_shadow_alpha,
        sel_shadow: base.sel_shadow,
        sel_ring_w: base.sel_ring_w,
        strip: base.strip,
        chip_sel_weight: base.chip_sel_weight,
        chip_text_alpha: base.chip_text_alpha,
        strip_button_w: base.strip_button_w,
        strip_button_h: base.strip_button_h,
        strip_button_gap: base.strip_button_gap,
        chips_pad: base.chips_pad,
        chips_gap: base.chips_gap,
        separator_h: base.separator_h,
        separator_alpha: base.separator_alpha,
        ..MODERN_SPEC
    }
}

pub static NORMAL_TV_SPEC: StyleSpec = tv_spec(&NORMAL_SPEC);
pub static MONO_TV_SPEC: StyleSpec = tv_spec(&MONO_SPEC);


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VkLook {
    pub style: VkStyle,
    pub tv: bool,
}

impl VkLook {
    pub fn tv_layout(self) -> bool {
        self.tv || self.style == VkStyle::Modern
    }
}

impl From<VkStyle> for VkLook {
    fn from(style: VkStyle) -> Self {
        Self { style, tv: false }
    }
}


pub fn style_spec(look: impl Into<VkLook>) -> &'static StyleSpec {
    let look = look.into();
    match (look.style, look.tv) {
        (VkStyle::Normal, false) => &NORMAL_SPEC,
        (VkStyle::Normal, true) => &NORMAL_TV_SPEC,
        (VkStyle::Mono, false) => &MONO_SPEC,
        (VkStyle::Mono, true) => &MONO_TV_SPEC,
        (VkStyle::Modern, _) => &MODERN_SPEC,
    }
}


pub struct PromptSpec {
    pub(crate) families: &'static [&'static str],
    pub(crate) no_pad_label: &'static str,
    pub(crate) status_label: &'static str,
    pub(crate) radius: f32,
    pub(crate) pill_h: f32,
    pub(crate) pill_bottom: f32,
    pub(crate) pill_pad_x: f32,
    pub(crate) pill_gap: f32,
    pub(crate) pill_fill: u32,
    pub(crate) pill_fill_alpha: f32,
    pub(crate) no_pad_stroke_alpha: f32,
    pub(crate) ready_stroke_alpha: f32,
    pub(crate) pill_shadow: (f32, f32, f32),
    pub(crate) card_w: f32,
    pub(crate) card_h: f32,
    pub(crate) card_bottom: f32,
    pub(crate) card_pad_y: f32,
    pub(crate) card_gap: f32,
    pub(crate) card_fill: u32,
    pub(crate) card_fill_alpha: f32,
    pub(crate) card_stroke_alpha: f32,
    pub(crate) card_shadow: (f32, f32, f32),
    pub(crate) label_px: f32,
    pub(crate) title_px: f32,
    pub(crate) title_h: f32,
    pub(crate) status_px: f32,
    pub(crate) status_h: f32,
    pub(crate) status_gap: f32,
    pub(crate) dim_text: u32,
    pub(crate) dim_alpha: f32,
    pub(crate) gamepad_px: f32,
    pub(crate) chip_px: f32,
    pub(crate) check_px: f32,
    pub(crate) stage_w: f32,
    pub(crate) stage_h: f32,
    pub(crate) stage_glow_alpha: f32,
    pub(crate) art_w: f32,
    pub(crate) art_h: f32,
}


pub static MONO_PROMPT: PromptSpec = PromptSpec {
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


pub fn prompt_spec(style: VkStyle) -> Option<&'static PromptSpec> {
    match style {
        VkStyle::Normal | VkStyle::Modern => None,
        VkStyle::Mono => Some(&MONO_PROMPT),
    }
}


pub fn strip_slots(look: impl Into<VkLook>) -> usize {
    style_spec(look).chip_slots
}


pub fn ref_key_h(scale: f32, look: impl Into<VkLook>) -> f32 {
    REF_KEY_W * scale.max(0.05) * style_spec(look).key_aspect
}


pub fn ref_unit(scale: f32, look: impl Into<VkLook>) -> f32 {
    let look = look.into();
    ref_key_h(scale, look) / style_spec(look).design_kh
}


pub fn strip_band_height(scale: f32, look: impl Into<VkLook>) -> f32 {
    let look = look.into();
    let spec = style_spec(look);
    let pad = if look.tv_layout() {
        spec.pad_y
    } else {
        -spec.pad_y
    };
    (spec.strip_bar_h + spec.gap + pad) * ref_unit(scale, look)
}


pub fn floating_pad(scale: f32, look: impl Into<VkLook>) -> (f32, f32) {
    let look = look.into();
    let spec = style_spec(look);
    let u = ref_unit(scale, look);
    (
        spec.pad_x * u + FLOATING_PANEL_INSET,
        spec.pad_y * u + FLOATING_PANEL_INSET,
    )
}


pub const REF_KEY_W: f32 = 92.0;


pub const FLOATING_PANEL_INSET: f32 = 1.0;


pub const LEGEND_MIN_TEXT_PX: f32 = 11.0;


pub fn text_px(design: f32, u: f32) -> f32 {
    (design * u).max(LEGEND_MIN_TEXT_PX)
}

pub unsafe fn paint_svg_icon(
    g: &mut Gfx,
    svg: &str,
    raster_px: u32,
    tint: u32,
    dst: Rect,
    alpha: f32,
    source_px: f32,
) {
    g.draw_svg_scaled(svg, raster_px, tint, source_px, dst, alpha);
}

pub fn fit_rect(dst: Rect, src_w: f32, src_h: f32) -> Rect {
    if src_w <= 0.0 || src_h <= 0.0 {
        return dst;
    }
    let (fit_w, fit_h) = if dst.w / dst.h > src_w / src_h {
        (dst.h * src_w / src_h, dst.h)
    } else {
        (dst.w, dst.w * src_h / src_w)
    };
    Rect {
        x: (dst.x + dst.x + dst.w - fit_w) * 0.5,
        y: (dst.y + dst.y + dst.h - fit_h) * 0.5,
        w: fit_w,
        h: fit_h,
    }
}

pub unsafe fn paint_voice_orb(
    g: &mut Gfx,
    accent: u32,
    level: f32,
    transcribing: bool,
    cx: f32,
    cy: f32,
    unit: f32,
    alpha_scale: f32,
    elapsed: f32,
) {
    let t = elapsed;
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
        let seed0 = bi as f32 * 9.71;
        for k in 0..LAYERS {
            let kf = k as f32 / (LAYERS - 1) as f32;
            let s = seed0 + k as f32;
            let rr = r * (1.0 - 0.80 * kf) * (1.0 + 0.20 * jitter(s));
            let jx = bx + unit * 0.06 * jitter(s + 1.3);
            let jy = by + unit * 0.06 * jitter(s + 7.7);
            let a = ((0.13 + 0.08 * energy) * (0.22 + 0.78 * kf * kf)) * alpha_scale;
            g.fill_ellipse(jx, jy, rr, rr, color, a);
        }
    }
    for k in 0..LAYERS {
        let kf = k as f32 / (LAYERS - 1) as f32;
        let rr = unit * (0.04 + (0.10 + 0.08 * energy) * (1.0 - kf));
        let a = ((0.022 + 0.09 * energy) * kf * kf) * alpha_scale;
        g.fill_ellipse(cx, cy, rr, rr, 0x00FFFFFF, a);
    }
}

pub unsafe fn paint_outer_shadow(
    g: &mut Gfx,
    shape: Rect,
    radius: f32,
    dy: f32,
    blur: f32,
    alpha: f32,
) -> Result<(), String> {
    g.push_shadow_layer(shape, radius, dy, blur)?;
    for (spread, a) in blur_shadow_layers(blur, alpha) {
        let r = (radius + spread).max(0.0);
        g.fill(
            Rect {
                x: shape.x - spread,
                y: shape.y - spread + dy,
                w: shape.w + spread * 2.0,
                h: shape.h + spread * 2.0,
            },
            r,
            0x000000,
            a,
        );
    }
    g.pop_layer();
    Ok(())
}

pub unsafe fn paint_display_border(
    g: &mut Gfx,
    cw: f32,
    ch: f32,
    accent: u32,
    alpha: f32,
) -> Result<(), String> {
    let scale = (ch / 1080.0).clamp(0.85, 2.0);
    let radius = 8.0 * scale;
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
        g.fill_edge_band(cw, ch, depth * scale, radius, accent, layer_alpha * alpha)?;
    }
    Ok(())
}

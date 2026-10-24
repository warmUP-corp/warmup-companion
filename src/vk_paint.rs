use crate::config::VkStyle;
use crate::mono_ui::{Align, Gfx, Rect, VAlign};
use crate::vk_nav::{KeyAction, KeyCell};

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
    strip_bar_h: 94.0,
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


pub const PROMPT_PILL_H: f32 = 116.0;
pub const PROMPT_LABEL_PX: f32 = 28.0;
pub const PROMPT_FAMILIES: &[&str] = &["Segoe UI", "Noto Sans", "DejaVu Sans"];
pub const CONTROLLER_ART_PX: u32 = 1254;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ControllerArt {
    DualSense,
    XboxOne,
}

impl ControllerArt {
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        let l = label.to_ascii_lowercase();
        if l.contains("dualsense")
            || l.contains("dualshock")
            || l.contains("playstation")
            || l.contains("ps5")
            || l.contains("ps4")
            || l.contains("hid slot")
            || l.contains("sony")
            || l == "wireless controller"
        {
            Some(Self::DualSense)
        } else if l.contains("xbox") || l.contains("xinput") {
            Some(Self::XboxOne)
        } else {
            None
        }
    }

    pub(crate) fn png(self) -> &'static [u8] {
        match self {
            Self::DualSense => include_bytes!("../assets/controller-models/dualsense-controller.png"),
            Self::XboxOne => include_bytes!("../assets/controller-models/xbox-one-controller.png"),
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::DualSense => "controller-art-dualsense",
            Self::XboxOne => "controller-art-xbox-one",
        }
    }
}

#[derive(Clone, Copy)]
pub struct PromptCard<'a> {
    pub bg: u32,
    pub border: u32,
    pub text_color: u32,
    pub pill_border: u32,
    pub pill_text: u32,
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub show_l3: bool,
    pub title: &'a str,
    pub controller_label: &'a str,
    pub card_t: f32,
    pub style: VkStyle,
}

pub struct PromptCanvas {
    pub cw: f32,
    pub ch: f32,
    pub dpr: f32,
    pub t: f32,
    pub title_px: f32,
    pub dip_scale: f32,
}

pub type ArtPainter<'a> = &'a mut dyn FnMut(&mut Gfx, ControllerArt, Rect, f32);

fn lerp_xywh(a: Rect, b: Rect, t: f32) -> Rect {
    Rect {
        x: lerp(a.x, b.x, t),
        y: lerp(a.y, b.y, t),
        w: lerp(a.w, b.w, t),
        h: lerp(a.h, b.h, t),
    }
}

fn edges_of(r: Rect) -> Edges {
    Edges {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

unsafe fn stroke_centered(g: &mut Gfx, r: Rect, radius: f32, width: f32, c: u32, alpha: f32) {
    let h = width * 0.5;
    g.ring(
        Rect {
            x: r.x - h,
            y: r.y - h,
            w: r.w + width,
            h: r.h + width,
        },
        radius + h,
        width,
        c,
        alpha,
    );
}

unsafe fn scale_about_point(g: &mut Gfx, c: &PromptCanvas, s: f32, cx: f32, cy: f32) {
    g.transform(s * c.dpr, cx * (1.0 - s) * c.dpr, cy * (1.0 - s) * c.dpr);
}

unsafe fn paint_prompt_art(
    g: &mut Gfx,
    p: &PromptCard,
    rect: Rect,
    tint: u32,
    alpha: f32,
    dip_scale: f32,
    art: ArtPainter<'_>,
) {
    match ControllerArt::from_label(p.controller_label) {
        Some(kind) => art(g, kind, rect, alpha),
        None => {
            let edges = edges_of(rect);
            paint_icon(g, VkIcon::Gamepad, edges, tint, alpha, icon_draw_px(VkIcon::Gamepad, rect.h), dip_scale);
        }
    }
}

pub unsafe fn paint_prompt_card(
    g: &mut Gfx,
    p: &PromptCard,
    c: &PromptCanvas,
    art: ArtPainter<'_>,
) -> Result<(), String> {
    if let Some(spec) = prompt_spec(p.style) {
        return paint_prompt_card_spec(g, p, spec, c, art);
    }
    let (cw, ch) = (c.cw, c.ch);
    let card_t = p.card_t.clamp(0.0, 1.0);
    let prompt_alpha = 1.0 - (card_t / 0.3).clamp(0.0, 1.0);
    let content_alpha = ((card_t - 0.35) / 0.65).clamp(0.0, 1.0);
    let content_alpha = content_alpha * content_alpha * (3.0 - 2.0 * content_alpha);
    let border = colorref_mix(p.border, p.pill_border, 1.0 - card_t);
    let t = c.t;
    let pulse = (t * lerp(0.33, 0.72, card_t)).fract();
    let pulse_alpha = (1.0 - pulse).powi(2);
    let breathe = ((t * std::f32::consts::TAU * 0.33).sin() * 0.5 + 0.5) * 0.015;
    let scale = 1.0 - (0.015 - breathe) * prompt_alpha;
    let pill_cy = ch - PROMPT_PILL_H * 0.5;
    scale_about_point(g, c, scale, cw * 0.5, pill_cy);

    let pill = Rect {
        x: 4.0,
        y: ch - PROMPT_PILL_H + 4.0,
        w: cw - 8.0,
        h: PROMPT_PILL_H - 8.0,
    };
    let card_w = (cw * 0.55).min(cw - 8.0);
    let card_top = ch * 0.21;
    let card = Rect {
        x: (cw - card_w) * 0.5,
        y: card_top,
        w: card_w,
        h: ch - 8.0 - card_top,
    };
    let panel = lerp_xywh(pill, card, card_t);
    let radius = PROMPT_PILL_H * 0.5 - 2.0;
    let glow = colorref_mix(0x00FFFFFF, border, lerp(0.38, 0.45, card_t));
    g.fill(panel, radius, p.bg, lerp(1.0, 0.94, card_t));
    stroke_centered(
        g,
        panel,
        radius,
        2.0 + lerp(8.0, 12.0, card_t) * pulse,
        glow,
        lerp(0.30, 0.22, card_t) * pulse_alpha,
    );
    stroke_centered(g, panel, radius, lerp(1.5, 1.2, card_t), glow, lerp(1.0, 0.84, card_t));

    if prompt_alpha > 0.01 {
        let chip = (PROMPT_PILL_H * 0.70).clamp(26.0, 96.0);
        let gap = 12.0;
        let measure = |g: &Gfx, s: &str| g.measure_full(s, PROMPT_LABEL_PX, 600, PROMPT_FAMILIES);
        let w_prefix = measure(g, p.prefix);
        let w_suffix = measure(g, p.suffix);
        let total = if p.show_l3 {
            w_prefix + gap + chip + gap + w_suffix
        } else {
            w_prefix
        };
        let mut x = ((cw - total) * 0.5).max(0.0);
        let band_top = ch - PROMPT_PILL_H;
        let label = |g: &Gfx, s: &str, x: f32, w: f32| {
            g.text_full(
                s,
                Rect {
                    x,
                    y: band_top,
                    w,
                    h: PROMPT_PILL_H,
                },
                PROMPT_LABEL_PX,
                600,
                PROMPT_FAMILIES,
                p.pill_text,
                prompt_alpha,
                Align::Left,
                VAlign::Center,
                false,
            )
        };
        label(g, p.prefix, x, w_prefix);
        if p.show_l3 {
            x += w_prefix + gap;
            let chip_rect = Rect {
                x,
                y: pill_cy - chip * 0.5,
                w: chip,
                h: chip,
            };
            let icon = ControllerIconFamily::from_label(p.controller_label).l3_icon();
            paint_icon(
                g,
                icon,
                edges_of(chip_rect),
                p.pill_text,
                prompt_alpha,
                icon_draw_px(icon, chip),
                c.dip_scale,
            );
            x += chip + gap;
        }
        if !p.suffix.is_empty() {
            label(g, p.suffix, x, w_suffix);
        }
    }

    if content_alpha > 0.01 {
        let name_rise = 10.0 * (1.0 - content_alpha);
        g.text_full(
            p.title,
            Rect {
                x: 0.0,
                y: 16.0 + name_rise,
                w: cw,
                h: card_top - 28.0,
            },
            c.title_px,
            600,
            PROMPT_FAMILIES,
            p.text_color,
            content_alpha,
            Align::Center,
            VAlign::Center,
            false,
        );
        let image_cx = cw * 0.5;
        let image_cy = card.y + card.h * 0.5;
        let image_scale = 0.90 + 0.10 * content_alpha;
        let ring_hw = 148.0 * image_scale + 20.0 * pulse;
        let ring_hh = 120.0 * image_scale + 20.0 * pulse;
        stroke_centered(
            g,
            Rect {
                x: image_cx - ring_hw,
                y: image_cy - ring_hh,
                w: ring_hw * 2.0,
                h: ring_hh * 2.0,
            },
            104.0 * image_scale + 20.0 * pulse,
            2.0,
            glow,
            0.20 * content_alpha * pulse_alpha,
        );
        let img_hw = 124.0 * image_scale;
        let img_hh = 108.0 * image_scale;
        let image_rect = Rect {
            x: image_cx - img_hw,
            y: image_cy - img_hh,
            w: img_hw * 2.0,
            h: img_hh * 2.0,
        };
        paint_prompt_art(g, p, image_rect, p.text_color, content_alpha, c.dip_scale, art);
    }
    g.transform(c.dpr, 0.0, 0.0);
    Ok(())
}

unsafe fn paint_prompt_card_spec(
    g: &mut Gfx,
    p: &PromptCard,
    spec: &PromptSpec,
    c: &PromptCanvas,
    art: ArtPainter<'_>,
) -> Result<(), String> {
    let (cw, ch) = (c.cw, c.ch);
    let card_t = p.card_t.clamp(0.0, 1.0);
    let prompt_alpha = 1.0 - (card_t / 0.3).clamp(0.0, 1.0);
    let content_alpha = ((card_t - 0.35) / 0.65).clamp(0.0, 1.0);
    let content_alpha = content_alpha * content_alpha * (3.0 - 2.0 * content_alpha);
    let t = c.t;
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
            vec![(Some(VkIcon::Gamepad), "", spec.gamepad_px), (None, spec.no_pad_label, 0.0)],
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
            None => (icon, text, g.measure_full(text, spec.label_px, 500, spec.families)),
        })
        .collect();
    let content_w =
        items.iter().map(|i| i.2).sum::<f32>() + spec.pill_gap * items.len().saturating_sub(1) as f32;
    let pill_w = (content_w + spec.pill_pad_x * 2.0).min(cw - 8.0);
    let pill_bottom = ch - spec.pill_bottom;
    let pill = Rect {
        x: (cw - pill_w) * 0.5,
        y: pill_bottom - spec.pill_h,
        w: pill_w,
        h: spec.pill_h,
    };
    let pill_cy = pill_bottom - spec.pill_h * 0.5;
    let card_bottom = ch - spec.card_bottom;
    let card_w = spec.card_w.min(cw - 8.0);
    let card_top = (card_bottom - spec.card_h).max(0.0);
    let card = Rect {
        x: (cw - card_w) * 0.5,
        y: card_top,
        w: card_w,
        h: card_bottom - card_top,
    };
    let panel = lerp_xywh(pill, card, card_t);
    let (pdy, pblur, palpha) = spec.pill_shadow;
    let (cdy, cblur, calpha) = spec.card_shadow;
    let fill = colorref_mix(spec.card_fill, spec.pill_fill, card_t);
    let fill_alpha = lerp(spec.pill_fill_alpha, spec.card_fill_alpha, card_t);

    scale_about_point(g, c, scale, cw * 0.5, pill_cy);
    paint_outer_shadow(
        g,
        panel,
        spec.radius,
        lerp(pdy, cdy, card_t),
        lerp(pblur, cblur, card_t),
        lerp(palpha, calpha, card_t),
    )?;
    g.fill(panel, spec.radius, fill, fill_alpha);
    g.ring(panel, spec.radius, 1.0, white, lerp(stroke_alpha, spec.card_stroke_alpha, card_t));

    if prompt_alpha > 0.01 {
        let mut x = (cw - content_w) * 0.5;
        for (icon, text, w) in items {
            let slot = Rect {
                x,
                y: pill.y,
                w,
                h: pill.h,
            };
            match icon {
                Some(icon) => {
                    let opacity = if icon.is_controller_tip() {
                        prompt_alpha
                    } else {
                        pill_text_alpha * prompt_alpha
                    };
                    paint_icon(g, icon, edges_of(slot), pill_text, opacity, w, c.dip_scale);
                }
                None => g.text_full(
                    text,
                    slot,
                    spec.label_px,
                    500,
                    spec.families,
                    pill_text,
                    pill_text_alpha * prompt_alpha,
                    Align::Left,
                    VAlign::Center,
                    false,
                ),
            }
            x += w + spec.pill_gap;
        }
    }

    if content_alpha > 0.01 {
        let rise = 10.0 * (1.0 - content_alpha);
        let title_top = card.y + spec.card_pad_y;
        g.text_full(
            p.title,
            Rect {
                x: card.x,
                y: title_top + rise,
                w: card.w,
                h: spec.title_h,
            },
            spec.title_px,
            600,
            spec.families,
            white,
            content_alpha,
            Align::Center,
            VAlign::Center,
            false,
        );
        let stage_top = title_top + spec.title_h + spec.card_gap;
        let stage_cy = stage_top + spec.stage_h * 0.5;
        g.radial_fill(
            cw * 0.5,
            stage_cy,
            spec.stage_w * 0.5,
            spec.stage_h * 0.5,
            &[(0.0, white, spec.stage_glow_alpha), (1.0, white, 0.0)],
            content_alpha,
        );
        let image_scale = 0.90 + 0.10 * content_alpha;
        let art_rect = Rect {
            x: cw * 0.5 - spec.art_w * 0.5 * image_scale,
            y: stage_cy - spec.art_h * 0.5 * image_scale,
            w: spec.art_w * image_scale,
            h: spec.art_h * image_scale,
        };
        paint_prompt_art(g, p, art_rect, white, content_alpha, c.dip_scale, art);

        let status_top = stage_top + spec.stage_h + spec.card_gap;
        let status_w = g.measure_full(spec.status_label, spec.status_px, 500, spec.families);
        let row_w = spec.check_px + spec.status_gap + status_w;
        let x = (cw - row_w) * 0.5;
        let row = |left: f32, w: f32| Rect {
            x: left,
            y: status_top,
            w,
            h: spec.status_h,
        };
        paint_icon(
            g,
            VkIcon::Check,
            edges_of(row(x, spec.check_px)),
            white,
            content_alpha,
            spec.check_px,
            c.dip_scale,
        );
        g.text_full(
            spec.status_label,
            row(x + spec.check_px + spec.status_gap, status_w),
            spec.status_px,
            500,
            spec.families,
            spec.dim_text,
            spec.dim_alpha * content_alpha,
            Align::Left,
            VAlign::Center,
            false,
        );
    }
    g.transform(c.dpr, 0.0, 0.0);
    Ok(())
}

pub struct TipsPill<'a> {
    pub bg: u32,
    pub border: u32,
    pub text: u32,
    pub controller_label: &'a str,
    pub cue: &'a [crate::win::controller_tips::TipToken],
    pub prev_cue: Option<&'a [crate::win::controller_tips::TipToken]>,
    pub morph_t: f32,
    pub index: usize,
    pub count: usize,
    pub alpha: f32,
    pub scale: f32,
    pub style: VkStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TipItem {
    Icon(VkIcon),
    Text(&'static str, bool),
}

pub(crate) fn tip_item(token: crate::win::controller_tips::TipToken, family: ControllerIconFamily) -> Option<TipItem> {
    use crate::win::controller_tips::TipToken;
    match token {
        TipToken::Button(b) => family.hint_icon(b).map(TipItem::Icon),
        TipToken::Text(t) => Some(TipItem::Text(t, false)),
        TipToken::Plus => Some(TipItem::Text("+", true)),
        TipToken::Dot => Some(TipItem::Text(LEGEND_SEP, true)),
    }
}

#[derive(Clone, Copy)]
struct TipFont {
    px: f32,
    weight: i32,
    families: &'static [&'static str],
}

unsafe fn tip_items(
    g: &Gfx,
    tokens: &[crate::win::controller_tips::TipToken],
    family: ControllerIconFamily,
    font: TipFont,
    chip_px: f32,
    gap: f32,
) -> (Vec<(TipItem, f32)>, f32) {
    let items: Vec<(TipItem, f32)> = tokens
        .iter()
        .filter_map(|&token| tip_item(token, family))
        .map(|item| {
            let w = match item {
                TipItem::Icon(_) => chip_px,
                TipItem::Text(t, _) => g.measure_full(t, font.px, font.weight, font.families),
            };
            (item, w)
        })
        .collect();
    let w = items.iter().map(|i| i.1).sum::<f32>() + gap * items.len().saturating_sub(1) as f32;
    (items, w)
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_tip_row(
    g: &mut Gfx,
    items: &[(TipItem, f32)],
    left: f32,
    band: Rect,
    gap: f32,
    font: TipFont,
    text: u32,
    dim: u32,
    alpha: f32,
    dip_scale: f32,
) {
    if alpha <= 0.01 {
        return;
    }
    let mut x = left;
    for (item, w) in items {
        let slot = Rect {
            x,
            y: band.y,
            w: *w,
            h: band.h,
        };
        match *item {
            TipItem::Icon(icon) => paint_icon(g, icon, edges_of(slot), text, alpha, *w, dip_scale),
            TipItem::Text(t, faint) => g.text_full(
                t,
                slot,
                font.px,
                font.weight,
                font.families,
                if faint { dim } else { text },
                if faint { alpha * 0.6 } else { alpha },
                Align::Left,
                VAlign::Center,
                false,
            ),
        }
        x += w + gap;
    }
}

pub unsafe fn paint_tips(g: &mut Gfx, p: &TipsPill, c: &PromptCanvas) -> Result<(), String> {
    let (cw, ch) = (c.cw, c.ch);
    let spec = prompt_spec(p.style);
    let family = ControllerIconFamily::from_label(p.controller_label);
    let (label, counter_font) = match spec {
        Some(s) => (
            TipFont {
                px: s.label_px,
                weight: 500,
                families: s.families,
            },
            TipFont {
                px: s.status_px,
                weight: 500,
                families: s.families,
            },
        ),
        None => (
            TipFont {
                px: PROMPT_LABEL_PX,
                weight: 600,
                families: PROMPT_FAMILIES,
            },
            TipFont {
                px: c.title_px,
                weight: 600,
                families: PROMPT_FAMILIES,
            },
        ),
    };
    let (pill_h, pill_bottom, radius, chip, gap, pad_x) = match spec {
        Some(s) => (s.pill_h, s.pill_bottom, s.radius, s.chip_px, s.pill_gap, s.pill_pad_x),
        None => (96.0, 6.0, 46.0, 56.0, 12.0, 36.0),
    };
    let (text, dim) = match spec {
        Some(s) => (0x00FFFFFF, s.dim_text),
        None => (p.text, mix_color(p.text, p.bg, 0.58)),
    };

    let counter = format!("{}/{}", p.index + 1, p.count.max(1));
    let counter_w = g.measure_full(&counter, counter_font.px, counter_font.weight, counter_font.families);
    let counter_gap = gap * 2.0;
    let (items, content_w) = tip_items(g, p.cue, family, label, chip, gap);
    let prev = p.prev_cue.map(|tokens| tip_items(g, tokens, family, label, chip, gap));
    let morph_t = if prev.is_some() { p.morph_t.clamp(0.0, 1.0) } else { 1.0 };
    let pill_w_for = |w: f32| (w + counter_gap + counter_w + pad_x * 2.0).min(cw - 8.0);
    let pill_w = match &prev {
        Some((_, prev_w)) => lerp(pill_w_for(*prev_w), pill_w_for(content_w), morph_t),
        None => pill_w_for(content_w),
    };
    let bottom = ch - pill_bottom;
    let pill = Rect {
        x: (cw - pill_w) * 0.5,
        y: bottom - pill_h,
        w: pill_w,
        h: pill_h,
    };
    let pill_cy = bottom - pill_h * 0.5;
    let alpha = p.alpha.clamp(0.0, 1.0);

    scale_about_point(g, c, p.scale, cw * 0.5, pill_cy);
    match spec {
        Some(s) => {
            let (dy, blur, a) = s.pill_shadow;
            paint_outer_shadow(g, pill, radius, dy, blur, a * alpha)?;
            g.fill(pill, radius, s.pill_fill, s.pill_fill_alpha * alpha);
            g.ring(pill, radius, 1.0, 0x00FFFFFF, s.ready_stroke_alpha * alpha);
        }
        None => {
            let pulse = (c.t * 0.33).fract();
            let pulse_alpha = (1.0 - pulse).powi(2);
            let glow = colorref_mix(0x00FFFFFF, p.border, 0.38);
            g.fill(pill, radius, p.bg, alpha);
            stroke_centered(g, pill, radius, 2.0 + 8.0 * pulse, glow, 0.30 * pulse_alpha * alpha);
            stroke_centered(g, pill, radius, 1.5, glow, alpha);
        }
    }

    let counter_left = pill.x + pill.w - pad_x - counter_w;
    let row_right = counter_left - counter_gap;
    let row_left = pill.x + pad_x;
    let centred = |w: f32| row_left + ((row_right - row_left - w) * 0.5).max(0.0);
    let old_alpha = 1.0 - (morph_t / 0.3).clamp(0.0, 1.0);
    let new_alpha = ((morph_t - 0.35) / 0.65).clamp(0.0, 1.0);
    let new_alpha = if prev.is_some() {
        new_alpha * new_alpha * (3.0 - 2.0 * new_alpha)
    } else {
        1.0
    };
    if let Some((prev_items, prev_w)) = &prev {
        paint_tip_row(g, prev_items, centred(*prev_w), pill, gap, label, text, dim, old_alpha * alpha, c.dip_scale);
    }
    paint_tip_row(g, &items, centred(content_w), pill, gap, label, text, dim, new_alpha * alpha, c.dip_scale);

    let divider_x = counter_left - counter_gap * 0.5;
    g.line(
        divider_x,
        pill_cy - pill_h * 0.22,
        divider_x,
        pill_cy + pill_h * 0.22,
        1.0,
        dim,
        0.25 * alpha,
    );
    g.text_full(
        &counter,
        Rect {
            x: counter_left,
            y: pill.y,
            w: counter_w,
            h: pill.h,
        },
        counter_font.px,
        counter_font.weight,
        counter_font.families,
        dim,
        0.8 * alpha,
        Align::Left,
        VAlign::Center,
        false,
    );
    g.transform(c.dpr, 0.0, 0.0);
    Ok(())
}

pub fn voice_slot(cw: f32, ch: f32) -> Rect {
    let scale = (ch / 1080.0).clamp(0.85, 2.0);
    let size = (96.0 * scale).min(cw.min(ch) * 0.9).max(1.0);
    let margin = (48.0 * scale).min((cw - size).max(0.0));
    let left = (cw - margin - size).max(0.0);
    let top = ((ch - size) * 0.5).max(0.0);
    Rect {
        x: left,
        y: top,
        w: size,
        h: size,
    }
}

pub fn scale_rect_about_center(r: Rect, scale: f32) -> Rect {
    let cx = r.x + r.w * 0.5;
    let cy = r.y + r.h * 0.5;
    let w = r.w * scale;
    let h = r.h * scale;
    Rect {
        x: cx - w * 0.5,
        y: cy - h * 0.5,
        w,
        h,
    }
}

pub type NimbusPainter<'a> = &'a mut dyn FnMut(&mut Gfx, Rect, f32) -> bool;

pub unsafe fn paint_voice_cloud(g: &mut Gfx, slot: Rect, accent: u32, alpha: f32, t: f32, nimbus: NimbusPainter<'_>) {
    if !nimbus(g, slot, alpha) {
        let unit = (slot.w * 0.42).max(1.0);
        paint_voice_orb(g, accent, 0.0, true, slot.x + slot.w * 0.5, slot.y + slot.h * 0.5, unit, alpha, t);
    }
}

pub fn voice_border_alpha(alpha: f32, transcribing: bool, t: f32) -> f32 {
    if transcribing {
        let pulse = 0.5 + 0.5 * (t * 2.4).sin();
        alpha * (0.8 + 0.7 * pulse)
    } else {
        alpha
    }
}

pub unsafe fn paint_voice_screen(
    g: &mut Gfx,
    cw: f32,
    ch: f32,
    accent: u32,
    alpha: f32,
    transcribing: bool,
    t: f32,
    orb_fill: Option<f32>,
    nimbus: NimbusPainter<'_>,
) -> Result<(), String> {
    paint_display_border(g, cw, ch, accent, voice_border_alpha(alpha, transcribing, t).min(1.0))?;
    if let Some(fill) = orb_fill {
        let slot = scale_rect_about_center(voice_slot(cw, ch), fill);
        paint_voice_cloud(g, slot, accent, alpha, t, nimbus);
    }
    Ok(())
}

pub unsafe fn paint_floating_panel(g: &mut Gfx, panel: Rect, radius: f32, stroke_w: f32, pal: &VkPalette) {
    g.fill(panel, radius, pal.bg, 1.0);
    g.ring(panel, radius, stroke_w, pal.panel_stroke, pal.panel_stroke_alpha);
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
        0.0
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
    svg: &'static str,
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

pub unsafe fn paint_blur_strokes(
    g: &Gfx,
    r: Rect,
    radius: f32,
    dy: f32,
    blur: f32,
    alpha: f32,
) {
    for (spread, a) in blur_shadow_layers(blur, alpha) {
        let rad = (radius + spread).max(0.0);
        g.fill(
            Rect {
                x: r.x - spread,
                y: r.y - spread + dy,
                w: r.w + spread * 2.0,
                h: r.h + spread * 2.0,
            },
            rad,
            0x000000,
            a,
        );
    }
}

pub unsafe fn paint_voice_orb(
    g: &Gfx,
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
    g: &Gfx,
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
    g: &Gfx,
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

pub unsafe fn paint_measure_px(g: &Gfx, style: VkStyle, text: &str, px: f32) -> f32 {
    let spec = style_spec(style);
    g.measure_full(text, px, 400, spec.families)
}

pub unsafe fn paint_text_px(
    g: &Gfx,
    style: VkStyle,
    text: &str,
    px: f32,
    left: f32,
    band: Rect,
    color: u32,
    alpha: f32,
) -> f32 {
    let spec = style_spec(style);
    let w = g.measure_full(text, px, 400, spec.families);
    g.text_full(
        text,
        Rect {
            x: left,
            y: band.y,
            w,
            h: band.h,
        },
        px,
        400,
        spec.families,
        color,
        alpha,
        crate::mono_ui::Align::Center,
        crate::mono_ui::VAlign::Center,
        false,
    );
    w
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum VkIcon {
    Backspace,
    Enter,
    Mic,
    MicOff,
    Space,
    Shift,
    ShiftFilled,
    Caps,
    CapsFilled,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    Gamepad,
    Check,
    L3Ps5,
    L3Xbox,
    R3Ps5,
    R3Xbox,
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
pub(crate) enum ControllerIconFamily {
    Ps5,
    Xbox,
}

#[cfg_attr(not(feature = "gamepad"), allow(dead_code))]
pub(crate) fn is_playstation_label(label: &str) -> bool {
    ControllerIconFamily::from_label(label) == ControllerIconFamily::Ps5
}

impl ControllerIconFamily {
    pub(crate) fn from_label(label: &str) -> Self {
        let l = label.to_ascii_lowercase();
        if l.contains("dualsense")
            || l.contains("dualshock")
            || l.contains("playstation")
            || l.contains("ps5")
            || l.contains("ps4")
            || l.contains("hid slot")
            || l.contains("sony")
            || l == "wireless controller"
        {
            Self::Ps5
        } else {
            Self::Xbox
        }
    }

    pub(crate) fn l3_icon(self) -> VkIcon {
        match self {
            Self::Ps5 => VkIcon::L3Ps5,
            Self::Xbox => VkIcon::L3Xbox,
        }
    }

    pub(crate) fn hint_icon(self, hint: &str) -> Option<VkIcon> {
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

impl VkIcon {
    pub(crate) fn svg(self) -> &'static str {
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
            VkIcon::L3Ps5 => include_str!("../controller-icons/p5_l3_click.svg"),
            VkIcon::L3Xbox => include_str!("../controller-icons/x_l3_click.svg"),
            VkIcon::R3Ps5 => include_str!("../controller-icons/p5_r3_click.svg"),
            VkIcon::R3Xbox => include_str!("../controller-icons/x_r3_click.svg"),
            VkIcon::SelectPs5 => include_str!("../controller-icons/p5_share.svg"),
            VkIcon::SelectXbox => include_str!("../controller-icons/x_menu_view.svg"),
            VkIcon::StartPs5 => include_str!("../controller-icons/p5_options.svg"),
            VkIcon::StartXbox => include_str!("../controller-icons/x_menu_menu.svg"),
            VkIcon::Ps5Cross => include_str!("../controller-icons/p5_face_cross_colored.svg"),
            VkIcon::Ps5Circle => include_str!("../controller-icons/p5_face_circle_colored.svg"),
            VkIcon::Ps5Square => include_str!("../controller-icons/p5_face_square_colored.svg"),
            VkIcon::Ps5Triangle => {
                include_str!("../controller-icons/p5_face_triangle_colored.svg")
            }
            VkIcon::Ps5L1 => include_str!("../controller-icons/p5_shoulder_l1.svg"),
            VkIcon::Ps5R1 => include_str!("../controller-icons/p5_shoulder_r1.svg"),
            VkIcon::Ps5L2 => include_str!("../controller-icons/p5_trigger_l2.svg"),
            VkIcon::Ps5R2 => include_str!("../controller-icons/p5_trigger_r2.svg"),
            VkIcon::XboxA => include_str!("../controller-icons/x_face_a_colored.svg"),
            VkIcon::XboxB => include_str!("../controller-icons/x_face_b_colored.svg"),
            VkIcon::XboxX => include_str!("../controller-icons/x_face_x_colored.svg"),
            VkIcon::XboxY => include_str!("../controller-icons/x_face_y_colored.svg"),
            VkIcon::XboxLb => include_str!("../controller-icons/x_shoulder_lb.svg"),
            VkIcon::XboxRb => include_str!("../controller-icons/x_shoulder_rb.svg"),
            VkIcon::XboxLt => include_str!("../controller-icons/x_trigger_lt.svg"),
            VkIcon::XboxRt => include_str!("../controller-icons/x_trigger_rt.svg"),
        }
    }

    pub(crate) fn is_controller_tip(self) -> bool {
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

pub(crate) fn key_icon(action: &KeyAction, shift: bool) -> Option<(VkIcon, bool)> {
    use crate::vk_key::*;
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

pub(crate) fn shift_icon(shift: bool) -> VkIcon {
    if shift {
        VkIcon::CapsFilled
    } else {
        VkIcon::Caps
    }
}

#[allow(dead_code)]
pub(crate) fn caps_icon(caps: bool) -> VkIcon {
    if caps {
        VkIcon::ShiftFilled
    } else {
        VkIcon::Shift
    }
}

pub(crate) fn is_action_key(action: &KeyAction) -> bool {
    use crate::vk_key::*;
    matches!(action, KeyAction::Vk(vk) if *vk == VK_BACK || *vk == VK_RETURN)
}

pub(crate) fn is_function_key(action: &KeyAction) -> bool {
    use crate::vk_key::*;
    matches!(
        action,
        KeyAction::Shift
            | KeyAction::Symbols
            | KeyAction::PredictPrev
            | KeyAction::PredictNext
            | KeyAction::VoiceInput
    ) || matches!(action, KeyAction::Vk(vk) if *vk == VK_BACK || *vk == VK_SPACE)
}

pub(crate) fn is_enter_key(action: &KeyAction) -> bool {
    use crate::vk_key::*;
    matches!(action, KeyAction::Vk(vk) if *vk == VK_RETURN)
}

pub(crate) fn styled_glyph(spec: &StyleSpec, action: &KeyAction, glyph: String) -> String {
    match action {
        KeyAction::Symbols if spec.trim_symbols_label => glyph.trim_start_matches('?').to_string(),
        _ => glyph,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Edges {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
}

impl Edges {
    pub(crate) fn rect(self) -> Rect {
        Rect {
            x: self.left,
            y: self.top,
            w: self.right - self.left,
            h: self.bottom - self.top,
        }
    }

    pub(crate) fn deflate(self, d: f32) -> Edges {
        Edges {
            left: self.left + d,
            top: self.top + d,
            right: self.right - d,
            bottom: self.bottom - d,
        }
    }

    pub(crate) fn band_at(self, cy: f32, h: f32) -> Edges {
        Edges {
            left: self.left,
            top: cy - h * 0.5,
            right: self.right,
            bottom: cy + h * 0.5,
        }
    }

    pub(crate) fn corner_band(self, spec: &StyleSpec, unit: f32) -> Edges {
        let inset = spec.hint_inset * unit;
        let h = spec.number_px * 1.5 * unit;
        let w = spec.number_px * 2.0 * unit;
        Edges {
            left: self.right - inset - w,
            top: self.top + inset * 0.5,
            right: self.right - inset * 0.5,
            bottom: self.top + inset * 0.5 + h,
        }
    }
}

pub(crate) fn key_glyph(key: &KeyCell) -> (String, bool) {
    match &key.action {
        KeyAction::Shift => (key.label.clone(), false),
        KeyAction::VoiceInput => (String::new(), false),
        KeyAction::Vk(vk) if *vk == crate::vk_key::VK_SPACE => (String::new(), false),
        _ => (key.label.clone(), false),
    }
}

pub(crate) fn key_hint(key: &KeyCell) -> Option<&'static str> {
    use crate::vk_key::{VK_BACK, VK_RETURN, VK_SPACE};
    match &key.action {
        KeyAction::Vk(vk) if *vk == VK_BACK => Some("B"),
        KeyAction::Vk(vk) if *vk == VK_RETURN => Some("START"),
        KeyAction::Vk(vk) if *vk == VK_SPACE => Some("Y"),
        KeyAction::Shift => Some("RT"),
        KeyAction::Symbols => Some("LT"),
        KeyAction::PredictPrev => Some("LB"),
        KeyAction::PredictNext => Some("RB"),
        KeyAction::VoiceInput => Some("R3"),
        _ => None,
    }
}

pub(crate) fn icon_draw_px(icon: VkIcon, h: f32) -> f32 {
    match icon {
        icon if icon.is_controller_tip() => (h * 0.98).round().clamp(24.0, h.max(64.0)),
        _ => (h * 0.5).round().clamp(16.0, 96.0),
    }
}

pub(crate) unsafe fn paint_icon(
    g: &mut Gfx,
    icon: VkIcon,
    rect: Edges,
    color: u32,
    opacity: f32,
    draw_px: f32,
    dip_scale: f32,
) {
    let draw_px = draw_px.round().clamp(12.0, 192.0);
    let raster_px = match icon {
        icon if icon.is_controller_tip() => (draw_px * 3.0)
            .round()
            .clamp(54.0, (draw_px * 3.0).max(192.0)),
        _ => (draw_px * dip_scale).round(),
    } as u32;
    let source_px = if icon.is_controller_tip() { 32.0 } else { 24.0 };
    let size = draw_px;
    paint_svg_icon(
        g,
        icon.svg(),
        raster_px,
        color,
        Rect {
            x: (rect.left + rect.right - size) * 0.5,
            y: (rect.top + rect.bottom - size) * 0.5,
            w: size,
            h: size,
        },
        opacity.clamp(0.0, 1.0),
        source_px,
    );
}

pub(crate) struct KeyStyle<'a> {
    pub(crate) look: VkLook,
    pub(crate) pal: &'a VkPalette,
    pub(crate) unit: f32,
    pub(crate) key_h: f32,
    pub(crate) dip_scale: f32,
    pub(crate) glyph_px: f32,
    pub(crate) hint_px: f32,
    pub(crate) icons: ControllerIconFamily,
}

impl KeyStyle<'_> {
    pub(crate) fn radius(&self) -> f32 {
        style_spec(self.look).key_radius * self.unit
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct KeyState<'a> {
    pub(crate) selected: bool,
    pub(crate) press_scale: Option<f32>,
    pub(crate) shift: bool,
    pub(crate) caps: bool,
    pub(crate) symbols: bool,
    pub(crate) symbols_locked: bool,
    pub(crate) voice_available: bool,
    pub(crate) tonal_row: bool,
    pub(crate) glyph: (String, bool),
    pub(crate) hint: Option<&'a str>,
}

unsafe fn press_about(g: &mut Gfx, s: f32, cx: f32, cy: f32) {
    g.transform(s, cx * (1.0 - s), cy * (1.0 - s));
}

pub(crate) unsafe fn paint_key(
    g: &mut Gfx,
    key: &KeyCell,
    key_rect: Edges,
    ks: &KeyStyle,
    st: &KeyState,
    voice: Option<&mut dyn FnMut(&mut Gfx, Edges, f32)>,
) {
    let spec = style_spec(ks.look);
    let pal = ks.pal;
    let unit = ks.unit;
    let key_h = ks.key_h;
    let radius = ks.radius();
    let selected = st.selected;
    let press = st.press_scale;
    if let Some(s) = press {
        press_about(
            g,
            s,
            (key_rect.left + key_rect.right) * 0.5,
            (key_rect.top + key_rect.bottom) * 0.5,
        );
    }
    let action_key = is_action_key(&key.action);
    if selected {
        if let Some((dy, blur, alpha)) = spec.sel_shadow {
            paint_blur_strokes(g, key_rect.rect(), radius, dy * unit, blur * unit, alpha);
        }
    } else if spec.key_shadow_dy > 0.0 {
        let dy = (spec.key_shadow_dy * unit).max(1.0);
        g.fill(
            Rect {
                x: key_rect.left,
                y: key_rect.top + dy,
                w: key_rect.right - key_rect.left,
                h: key_rect.bottom - key_rect.top,
            },
            radius,
            0x000000,
            spec.key_shadow_alpha,
        );
    }
    let modifier_active = !selected
        && ((matches!(key.action, KeyAction::Shift) && st.shift)
            || (matches!(key.action, KeyAction::Symbols) && st.symbols));
    let modern = ks.look.style == VkStyle::Modern;
    let fill_color = if selected {
        pal.accent
    } else if modifier_active || (press.is_some() && pal.mod_tint.is_some()) {
        pal.mod_tint
            .map(|(fill, _)| fill)
            .unwrap_or_else(|| colorref_mix(pal.accent, pal.key, 0.35))
    } else if modern && is_enter_key(&key.action) {
        pal.key_enter
    } else if action_key
        || (spec.tonal_function_keys && (is_function_key(&key.action) || st.tonal_row))
    {
        pal.key_action
    } else {
        pal.key
    };
    let dim_color = if selected { pal.sel_text } else { pal.text_dim };
    let label_color = if selected {
        pal.sel_text
    } else if modern && is_enter_key(&key.action) && !modifier_active {
        pal.enter_text
    } else {
        pal.text
    };
    g.fill(key_rect.rect(), radius, fill_color, 1.0);

    let locked = (matches!(key.action, KeyAction::Shift) && st.caps)
        || (matches!(key.action, KeyAction::Symbols) && st.symbols_locked);
    if locked {
        let pill_w = ((key_rect.right - key_rect.left) * 0.28).min(24.0 * unit);
        let pill_h = (3.0 * unit).max(2.0);
        let cx = (key_rect.left + key_rect.right) * 0.5;
        let bottom = key_rect.bottom - 5.0 * unit;
        let pill_color = match (selected, pal.mod_tint) {
            (true, _) => pal.sel_text,
            (false, Some((_, bar))) => bar,
            (false, None) => pal.accent,
        };
        g.fill(
            Rect {
                x: cx - pill_w * 0.5,
                y: bottom - pill_h,
                w: pill_w,
                h: pill_h,
            },
            pill_h * 0.5,
            pill_color,
            1.0,
        );
    }

    let is_space = matches!(key.action, KeyAction::Vk(vk) if vk == crate::vk_key::VK_SPACE);
    if let Some(sub) = key.sublabel.as_deref().filter(|_| !is_space) {
        let band = if spec.corner_sublabel {
            key_rect.corner_band(spec, unit)
        } else {
            key_rect.band_at(key_rect.top + spec.number_cy * unit, key_h)
        };
        g.text_full(
            sub,
            band.rect(),
            spec.number_px * unit,
            400,
            spec.families,
            dim_color,
            spec.number_alpha,
            Align::Center,
            VAlign::Center,
            false,
        );
    }

    let icon_px = spec.icon_px * unit;
    if matches!(key.action, KeyAction::VoiceInput) {
        if !st.voice_available {
            let disabled_color = colorref_mix(label_color, fill_color, 0.42);
            paint_icon(g, VkIcon::MicOff, key_rect, disabled_color, 1.0, icon_px, ks.dip_scale);
        } else if let Some(voice) = voice {
            voice(g, key_rect, radius);
        } else {
            paint_icon(g, VkIcon::Mic, key_rect, label_color, 1.0, icon_px, ks.dip_scale);
        }
    } else if is_space {
        let space_px = spec.space_icon_px * unit;
        if let Some(text) = spec.space_label {
            g.text_full(
                text,
                key_rect.rect(),
                spec.space_label_px * unit,
                400,
                spec.families,
                label_color,
                1.0,
                Align::Center,
                VAlign::Center,
                false,
            );
        } else if let Some(sub) = key.sublabel.as_deref() {
            let band = key_rect.band_at(key_rect.top + spec.caption_cy * unit, key_h);
            g.text_full(
                sub,
                band.rect(),
                spec.number_px * unit,
                400,
                spec.families,
                dim_color,
                spec.number_alpha,
                Align::Center,
                VAlign::Center,
                false,
            );
            let icon_rect = key_rect.band_at(key_rect.top + spec.space_icon_cy * unit, key_h);
            paint_icon(g, VkIcon::Space, icon_rect, label_color, 1.0, space_px, ks.dip_scale);
        } else {
            paint_icon(g, VkIcon::Space, key_rect, label_color, 1.0, space_px, ks.dip_scale);
        }
    } else if let Some((icon, large)) = key_icon(&key.action, st.shift) {
        let px = if large {
            spec.icon_large_px * unit
        } else {
            icon_px
        };
        paint_icon(g, icon, key_rect, label_color, 1.0, px, ks.dip_scale);
    } else {
        let (glyph, symbol_font) = st.glyph.clone();
        let glyph = styled_glyph(spec, &key.action, glyph);
        if !glyph.is_empty() {
            let corner = spec.corner_sublabel && key.sublabel.is_some() && !symbol_font;
            let (weight, families, halign, valign, wrap) = if corner {
                (spec.label_weight, spec.families, Align::Left, VAlign::Bottom, false)
            } else if symbol_font {
                (
                    400,
                    &["Segoe UI Symbol"] as &[&str],
                    Align::Center,
                    VAlign::Center,
                    true,
                )
            } else if glyph.chars().count() > 1 {
                (spec.word_weight, spec.families, Align::Center, VAlign::Center, false)
            } else {
                (spec.label_weight, spec.families, Align::Center, VAlign::Center, false)
            };
            let px = if symbol_font {
                ks.glyph_px
            } else if glyph.chars().count() > 1 {
                spec.word_px * unit
            } else {
                spec.label_px * unit
            };
            let label_rect = if corner {
                key_rect.deflate(spec.hint_inset * 1.5 * unit)
            } else if key.sublabel.is_some() {
                key_rect.band_at(key_rect.top + spec.label_cy * unit, key_h)
            } else {
                key_rect
            };
            g.text_full(
                &glyph,
                label_rect.rect(),
                px,
                weight,
                families,
                label_color,
                1.0,
                halign,
                valign,
                wrap,
            );
        }
    }

    if let Some(hint) = st.hint {
        let badge_size = spec.hint_badge * unit;
        let inset = spec.hint_inset * unit;
        let badge = Edges {
            left: key_rect.left + inset,
            top: key_rect.top + inset,
            right: key_rect.left + inset + badge_size,
            bottom: key_rect.top + inset + badge_size,
        };
        if let Some(icon) = ks.icons.hint_icon(hint) {
            let px = icon_draw_px(icon, badge.bottom - badge.top);
            paint_icon(g, icon, badge, pal.text, 1.0, px, ks.dip_scale);
        } else {
            let badge_color = if selected { pal.sel_text } else { pal.accent };
            g.text_full(
                hint,
                badge.rect(),
                ks.hint_px,
                600,
                &["Segoe UI"],
                badge_color,
                1.0,
                Align::Center,
                VAlign::Top,
                true,
            );
        }
    }
    if press.is_some() {
        g.transform(1.0, 0.0, 0.0);
    }
}

pub(crate) unsafe fn paint_focus_ring(
    g: &mut Gfx,
    ring: Edges,
    ks: &KeyStyle,
    press_scale: Option<f32>,
) {
    let spec = style_spec(ks.look);
    let ring_w = (spec.sel_ring_w * ks.unit).max(1.0);
    if let Some(s) = press_scale {
        press_about(g, s, (ring.left + ring.right) * 0.5, (ring.top + ring.bottom) * 0.5);
    }
    g.ring(ring.rect(), ks.radius(), ring_w, ks.pal.sel_ring, 1.0);
    if press_scale.is_some() {
        g.transform(1.0, 0.0, 0.0);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StripGeom {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
    pub(crate) unit: f32,
}

impl StripGeom {
    pub(crate) fn above(rects: &[crate::vk_geom::KeyRect], spec: &StyleSpec, unit: f32) -> Self {
        if rects.is_empty() {
            return Self::default();
        }
        let left = rects.iter().map(|r| r.left).fold(f32::INFINITY, f32::min);
        let right = rects
            .iter()
            .map(|r| r.right)
            .fold(f32::NEG_INFINITY, f32::max);
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

    pub(crate) fn cy(&self) -> f32 {
        (self.top + self.bottom) * 0.5
    }

    pub(crate) fn pill_frame(&self, spec: &StyleSpec) -> PillFrame {
        let u = self.unit;
        let cy = self.cy();
        let button_w = spec.strip_button_w * u;
        let button_h = spec.strip_button_h * u;
        let button_gap = spec.strip_button_gap * u;
        let button = |left: f32| Edges {
            left,
            top: cy - button_h * 0.5,
            right: left + button_w,
            bottom: cy + button_h * 0.5,
        };
        let prev = button(self.left);
        let next = button(self.right - button_w);
        let chips_h = spec.chips_h * u;
        let chips = Edges {
            left: prev.right + button_gap,
            top: cy - chips_h * 0.5,
            right: next.left - button_gap,
            bottom: cy + chips_h * 0.5,
        };
        let inner = chips.deflate(spec.chips_pad * u);
        PillFrame {
            prev,
            next,
            chips,
            inner,
            chip_r: ((inner.bottom - inner.top) * 0.5).max(0.0),
        }
    }

    pub(crate) fn pill_slot_rects(&self, spec: &StyleSpec, visible: &[String]) -> Vec<(usize, Edges)> {
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
                    Edges {
                        left,
                        top: inner.top,
                        right: left + chip_w,
                        bottom: inner.bottom,
                    },
                )
            })
            .collect()
    }

    pub(crate) fn column_hints(&self, spec: &StyleSpec, engaged: bool) -> Vec<(&'static str, Edges)> {
        let lane = spec.strip_button_w * self.unit;
        let side = spec.strip_hint_px * self.unit;
        let cy = self.cy();
        let left = Edges::square(self.left + lane * 0.5, cy, side);
        let right = Edges::square(self.right - lane * 0.5, cy, side);
        if engaged {
            vec![("LB", left), ("RB", right)]
        } else {
            vec![("SELECT", left)]
        }
    }

    pub(crate) fn column_rects(&self, spec: &StyleSpec, slots: usize) -> Vec<Edges> {
        let lane = spec.strip_button_w * self.unit;
        let left = self.left + lane;
        let right = (self.right - lane).max(left);
        let slots = slots.max(1);
        let col_w = (right - left) / slots as f32;
        (0..slots)
            .map(|i| Edges {
                left: left + i as f32 * col_w,
                top: self.top,
                right: left + (i + 1) as f32 * col_w,
                bottom: self.bottom,
            })
            .collect()
    }

    pub(crate) fn chip_rects(&self, spec: &StyleSpec, slots: usize) -> Vec<Edges> {
        let half_gap = spec.chips_gap * self.unit * 0.5;
        let chip_h = (spec.chips_h * self.unit).min(self.bottom - self.top);
        let cy = self.cy();
        self.column_rects(spec, slots)
            .into_iter()
            .map(|lane| Edges {
                left: lane.left + half_gap,
                top: cy - chip_h * 0.5,
                right: (lane.right - half_gap).max(lane.left + half_gap),
                bottom: cy + chip_h * 0.5,
            })
            .collect()
    }

    pub(crate) fn slot_rects(&self, spec: &StyleSpec, visible: &[String]) -> Vec<(usize, Edges)> {
        match spec.strip {
            StripLook::Pills => self.pill_slot_rects(spec, visible),
            StripLook::Chips => self
                .chip_rects(spec, visible.len())
                .into_iter()
                .enumerate()
                .filter(|(i, _)| visible.get(*i).is_some_and(|w| !w.is_empty()))
                .collect(),
            StripLook::Columns => self
                .column_rects(spec, visible.len())
                .into_iter()
                .enumerate()
                .filter(|(i, _)| visible.get(*i).is_some_and(|w| !w.is_empty()))
                .collect(),
        }
    }
}

pub(crate) struct PillFrame {
    pub(crate) prev: Edges,
    pub(crate) next: Edges,
    pub(crate) chips: Edges,
    pub(crate) inner: Edges,
    pub(crate) chip_r: f32,
}

impl crate::vk_geom::KeyRect {
    pub(crate) fn edges(&self) -> Edges {
        Edges {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

impl Edges {
    pub(crate) fn square(cx: f32, cy: f32, side: f32) -> Edges {
        let h = side * 0.5;
        Edges {
            left: cx - h,
            top: cy - h,
            right: cx + h,
            bottom: cy + h,
        }
    }
}

pub(crate) struct StripPaint<'a> {
    pub(crate) spec: &'a StyleSpec,
    pub(crate) pal: &'a VkPalette,
    pub(crate) alpha: f32,
    pub(crate) icons: ControllerIconFamily,
    pub(crate) select_retired: bool,
    pub(crate) dip_scale: f32,
}

unsafe fn strip_word(
    g: &mut Gfx,
    sp: &StripPaint,
    geom: &StripGeom,
    word: &str,
    rect: Edges,
    selected: bool,
    color: u32,
    alpha: f32,
) {
    let spec = sp.spec;
    g.text_clip(
        word,
        rect.rect(),
        (spec.chip_px * geom.unit).max(1.0),
        if selected { spec.chip_sel_weight } else { 400 },
        spec.families,
        color,
        alpha,
        Align::Center,
        VAlign::Center,
        false,
    );
}

pub(crate) unsafe fn paint_strip(
    g: &mut Gfx,
    sp: &StripPaint,
    strip: &crate::vk_predict::StripState,
    geom: &StripGeom,
) {
    if sp.alpha <= 0.0
        || geom.bottom - geom.top < 1.0
        || strip.visible.iter().all(|w| w.is_empty())
    {
        return;
    }
    match sp.spec.strip {
        StripLook::Pills => paint_strip_pills(g, sp, strip, geom),
        StripLook::Columns => paint_strip_columns(g, sp, strip, geom),
        StripLook::Chips => paint_strip_chips(g, sp, strip, geom),
    }
}

unsafe fn paint_strip_chips(
    g: &mut Gfx,
    sp: &StripPaint,
    strip: &crate::vk_predict::StripState,
    geom: &StripGeom,
) {
    let (spec, pal, alpha) = (sp.spec, sp.pal, sp.alpha);
    for (hint, rect) in geom
        .column_hints(spec, strip.engaged)
        .into_iter()
        .filter(|(h, _)| !(sp.select_retired && *h == "SELECT"))
    {
        if let Some(icon) = sp.icons.hint_icon(hint) {
            paint_icon(g, icon, rect, pal.text, alpha, rect.bottom - rect.top, sp.dip_scale);
        }
    }
    if !strip.engaged {
        let lane = spec.strip_button_w * geom.unit;
        let side = spec.strip_hint_px * geom.unit;
        let arrow = Edges::square(geom.right - lane * 0.5, geom.cy(), side);
        paint_icon(g, VkIcon::ChevronRight, arrow, pal.text, alpha, side, sp.dip_scale);
    }
    let pad = spec.chips_gap * geom.unit;
    for (i, chip) in geom.slot_rects(spec, &strip.visible) {
        let selected = strip.engaged && i == strip.highlight_slot;
        let r = (chip.bottom - chip.top) * 0.5;
        if selected {
            if let Some((dy, blur, a)) = spec.sel_shadow {
                paint_blur_strokes(g, chip.rect(), r, dy * geom.unit, blur * geom.unit, a * alpha);
            }
        }
        g.fill(chip.rect(), r, if selected { pal.accent } else { pal.key }, alpha);
        let (color, a) = if selected {
            (pal.sel_text, alpha)
        } else {
            (pal.text, spec.chip_text_alpha * alpha)
        };
        let label = Edges {
            left: chip.left + pad,
            right: chip.right - pad,
            ..chip
        };
        strip_word(g, sp, geom, &strip.visible[i], label, selected, color, a);
    }
}

unsafe fn paint_strip_pills(
    g: &mut Gfx,
    sp: &StripPaint,
    strip: &crate::vk_predict::StripState,
    geom: &StripGeom,
) {
    let (spec, pal, alpha) = (sp.spec, sp.pal, sp.alpha);
    let u = geom.unit;
    let cy = geom.cy();
    let button_h = spec.strip_button_h * u;
    let hint_px = spec.strip_hint_px * u;
    let frame = geom.pill_frame(spec);
    let mut buttons = vec![(frame.prev, if strip.engaged { "LB" } else { "SELECT" })];
    if strip.engaged {
        buttons.push((frame.next, "RB"));
    }
    for (rect, hint) in buttons {
        g.fill(rect.rect(), button_h * 0.5, pal.key_action, alpha);
        g.ring(rect.rect(), button_h * 0.5, 1.0, pal.border, alpha);
        if let Some(icon) = sp.icons.hint_icon(hint) {
            let cx = (rect.left + rect.right) * 0.5;
            let square = Edges::square(cx, cy, hint_px);
            let px = icon_draw_px(icon, square.bottom - square.top);
            paint_icon(g, icon, square, pal.text, alpha, px, sp.dip_scale);
        }
    }
    let chips = frame.chips;
    if chips.right - chips.left < 1.0 {
        return;
    }
    let chip_r = frame.chip_r;
    let chips_r = crate::vk_motion::concentric_radius(chip_r, spec.chips_pad * u);
    g.fill(chips.rect(), chips_r, pal.key_action, alpha);
    g.ring(chips.rect(), chips_r, 1.0, pal.border, alpha);
    for (slot_index, slot) in geom.pill_slot_rects(spec, &strip.visible) {
        let word = &strip.visible[slot_index];
        let selected = strip.engaged && slot_index == strip.highlight_slot;
        if selected {
            g.fill(slot.rect(), chip_r, pal.chip_sel, alpha);
        }
        let label = Edges {
            left: slot.left + chip_r * 0.5,
            right: slot.right - chip_r * 0.5,
            ..slot
        };
        let a = if selected {
            alpha
        } else {
            spec.chip_text_alpha * alpha
        };
        strip_word(g, sp, geom, word, label, selected, pal.text, a);
    }
}

unsafe fn paint_strip_columns(
    g: &mut Gfx,
    sp: &StripPaint,
    strip: &crate::vk_predict::StripState,
    geom: &StripGeom,
) {
    let (spec, pal, alpha) = (sp.spec, sp.pal, sp.alpha);
    let u = geom.unit;
    let cy = geom.cy();
    let sep_h = spec.separator_h * u;
    for (hint, rect) in geom.column_hints(spec, strip.engaged) {
        if let Some(icon) = sp.icons.hint_icon(hint) {
            paint_icon(g, icon, rect, pal.text_dim, alpha, rect.bottom - rect.top, sp.dip_scale);
        }
    }
    let columns = geom.column_rects(spec, strip.visible.len());
    let sel_h = (sep_h * 1.6).min(geom.bottom - geom.top);
    for (i, (word, col)) in strip.visible.iter().zip(columns).enumerate() {
        if i > 0 {
            let x = col.left.round();
            let sep = Edges {
                left: x - 0.5,
                top: cy - sep_h * 0.5,
                right: x + 0.5,
                bottom: cy + sep_h * 0.5,
            };
            g.fill_rect(sep.rect(), pal.border, spec.separator_alpha * alpha);
        }
        if word.is_empty() {
            continue;
        }
        let selected = strip.engaged && i == strip.highlight_slot;
        let pad = 8.0 * u;
        if selected {
            let pill = Edges {
                left: col.left + pad * 0.5,
                top: cy - sel_h * 0.5,
                right: col.right - pad * 0.5,
                bottom: cy + sel_h * 0.5,
            };
            g.fill(pill.rect(), sel_h * 0.5, pal.chip_sel, alpha);
        }
        let (color, a) = if selected {
            (pal.text, alpha)
        } else {
            (pal.text_dim, spec.chip_text_alpha * alpha)
        };
        let label = Edges {
            left: col.left + pad,
            right: col.right - pad,
            ..col
        };
        strip_word(g, sp, geom, word, label, selected, color, a);
    }
}

pub(crate) const BAR_H: f32 = 80.0;
pub(crate) const BAR_GLYPH: f32 = 38.0;
pub(crate) const BAR_SHARE: f32 = 44.0;
pub(crate) const BAR_GAP: f32 = 6.0;
pub(crate) const BAR_LABEL_GAP: f32 = 6.0;
pub(crate) const BAR_LABEL_PX: f32 = 19.0;
pub(crate) const BAR_CONNECTOR_PX: f32 = 17.0;
pub(crate) const BAR_PREFIX_PX: f32 = 20.0;
pub(crate) const BAR_SEP_PX: f32 = 18.0;
pub(crate) const SHEET_PAD_TOP: f32 = 16.0;
pub(crate) const SHEET_PAD_X: f32 = 24.0;
pub(crate) const SHEET_PAD_BOTTOM: f32 = 16.0;
pub(crate) const SHEET_COL_GAP: f32 = 40.0;
pub(crate) const SHEET_ROW_H: f32 = 70.0;
pub(crate) const SHEET_HEADER_PX: f32 = 18.0;
pub(crate) const SHEET_ACTION_PX: f32 = 19.0;
pub(crate) const SHEET_TOKEN_PX: f32 = 17.0;
pub(crate) const SHEET_GLYPH: f32 = 28.0;
pub(crate) const SHEET_SHARE: f32 = 32.0;
pub(crate) const SHEET_TOKEN_GAP: f32 = 8.0;
pub(crate) const SHEET_ACTION_GAP: f32 = 24.0;
pub(crate) const SHEET_TRAIL: f32 = 24.0;
pub(crate) const LEGEND_SEP: &str = "\u{00B7}";
pub(crate) const ALPHA_SECONDARY: f32 = 0xA6 as f32 / 255.0;
pub(crate) const ALPHA_SEP: f32 = 0x66 as f32 / 255.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ComboStyle {
    pub(crate) glyph: f32,
    pub(crate) share: f32,
    pub(crate) gap: f32,
    pub(crate) prefix_px: f32,
    pub(crate) connector_px: f32,
}

impl ComboStyle {
    pub(crate) fn bar(u: f32) -> Self {
        Self {
            glyph: BAR_GLYPH * u,
            share: BAR_SHARE * u,
            gap: BAR_GAP * u,
            prefix_px: text_px(BAR_PREFIX_PX, u),
            connector_px: text_px(BAR_CONNECTOR_PX, u),
        }
    }

    pub(crate) fn sheet(u: f32) -> Self {
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
pub(crate) enum ComboPart {
    Icon(VkIcon, bool),
    Prefix(&'static str),
    Connector(&'static str),
}

pub(crate) fn combo_parts(tokens: &[crate::win::controller_tips::TipToken], family: ControllerIconFamily) -> Vec<ComboPart> {
    tokens
        .iter()
        .filter_map(|&t| match t {
            crate::win::controller_tips::TipToken::Button(b) => family
                .hint_icon(b)
                .map(|i| ComboPart::Icon(i, b == "SELECT")),
            crate::win::controller_tips::TipToken::Text(t) if t == "/" => Some(ComboPart::Connector(t)),
            crate::win::controller_tips::TipToken::Text(t) => Some(ComboPart::Prefix(t)),
            crate::win::controller_tips::TipToken::Plus => Some(ComboPart::Connector("+")),
            crate::win::controller_tips::TipToken::Dot => Some(ComboPart::Connector(LEGEND_SEP)),
        })
        .collect()
}

pub(crate) type Measure<'a> = &'a dyn Fn(&str, f32) -> f32;

pub(crate) fn part_width(part: &ComboPart, cs: &ComboStyle, measure: Measure) -> f32 {
    match *part {
        ComboPart::Icon(_, true) => cs.share,
        ComboPart::Icon(_, false) => cs.glyph,
        ComboPart::Prefix(t) => measure(t, cs.prefix_px),
        ComboPart::Connector(t) => measure(t, cs.connector_px),
    }
}

pub(crate) fn combo_width(parts: &[ComboPart], cs: &ComboStyle, measure: Measure) -> f32 {
    parts
        .iter()
        .map(|p| part_width(p, cs, measure))
        .sum::<f32>()
        + cs.gap * parts.len().saturating_sub(1) as f32
}

pub fn legend_band_height(scale: f32, look: impl Into<VkLook>) -> f32 {
    let look = look.into();
    ((BAR_H - style_spec(look).pad_y) * ref_unit(scale, look)).max(0.0)
}

pub(crate) fn bar_item_width(
    row: &crate::win::shortcut_sheet::SheetRow,
    family: ControllerIconFamily,
    u: f32,
    measure: Measure,
) -> f32 {
    combo_width(&combo_parts(row.keys, family), &ComboStyle::bar(u), measure)
        + BAR_LABEL_GAP * u
        + measure(row.label, text_px(BAR_LABEL_PX, u))
}

pub(crate) fn bar_sep_width(u: f32, measure: Measure) -> f32 {
    2.0 * BAR_GAP * u + measure(LEGEND_SEP, text_px(BAR_SEP_PX, u))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BarLayout {
    pub(crate) items: Vec<(usize, f32, f32)>,
    pub(crate) sep_w: f32,
}

pub(crate) fn bar_layout(widths: &[f32], sep_w: f32, left: f32, right: f32) -> BarLayout {
    let kept = crate::win::shortcut_sheet::fit_legend(widths, sep_w, right - left);
    let total =
        kept.iter().map(|&i| widths[i]).sum::<f32>() + sep_w * kept.len().saturating_sub(1) as f32;
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

pub(crate) fn legend_span(rects: &[crate::vk_geom::KeyRect], client_w: f32, pad_x: f32) -> (f32, f32) {
    let (l, r) = rects.iter().fold((pad_x, client_w - pad_x), |(l, r), kr| {
        (l.min(kr.left), r.max(kr.right))
    });
    if l > r {
        (0.0, client_w)
    } else {
        (l.max(0.0), r.min(client_w))
    }
}

pub(crate) unsafe fn paint_combo(
    g: &mut Gfx,
    style: VkStyle,
    parts: &[ComboPart],
    cs: &ComboStyle,
    left: f32,
    band: Edges,
    text: u32,
    alpha: f32,
    dip_scale: f32,
) -> f32 {
    let mut x = left;
    for part in parts {
        let w = match *part {
            ComboPart::Icon(icon, share) => {
                let w = if share { cs.share } else { cs.glyph };
                let slot = Edges {
                    left: x,
                    right: x + w,
                    ..band
                };
                paint_icon(g, icon, slot, text, alpha, w, dip_scale);
                w
            }
            ComboPart::Prefix(t) => paint_text_px(
                g,
                style,
                t,
                cs.prefix_px,
                x,
                band.rect(),
                text,
                ALPHA_SECONDARY * alpha,
            ),
            ComboPart::Connector(t) => paint_text_px(
                g,
                style,
                t,
                cs.connector_px,
                x,
                band.rect(),
                text,
                ALPHA_SECONDARY * alpha,
            ),
        };
        x += w + cs.gap;
    }
    (x - cs.gap - left).max(0.0)
}

pub(crate) unsafe fn legend_layout(
    g: &Gfx,
    rows: &[crate::win::shortcut_sheet::SheetRow],
    style: VkStyle,
    family: ControllerIconFamily,
    u: f32,
    left: f32,
    right: f32,
) -> BarLayout {
    let measure = |t: &str, px: f32| paint_measure_px(g, style, t, px);
    let widths: Vec<f32> = rows
        .iter()
        .map(|row| bar_item_width(row, family, u, &measure))
        .collect();
    bar_layout(&widths, bar_sep_width(u, &measure), left, right)
}

pub(crate) struct LegendPaint<'a> {
    pub(crate) pal: &'a VkPalette,
    pub(crate) style: VkStyle,
    pub(crate) family: ControllerIconFamily,
    pub(crate) u: f32,
    pub(crate) top: f32,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) alpha: f32,
    pub(crate) dip_scale: f32,
}

pub(crate) unsafe fn paint_legend(
    g: &mut Gfx,
    legend: &[crate::win::shortcut_sheet::SheetRow],
    lp: &LegendPaint,
) {
    let (pal, style, family, u, alpha) = (lp.pal, lp.style, lp.family, lp.u, lp.alpha);
    let layout = legend_layout(g, legend, style, family, u, lp.left, lp.right);
    let band = Edges {
        left: lp.left,
        top: lp.top,
        right: lp.right,
        bottom: lp.top + BAR_H * u,
    };
    let cs = ComboStyle::bar(u);
    for (n, &(i, start, _)) in layout.items.iter().enumerate() {
        if n > 0 {
            let sep_left = start - layout.sep_w + BAR_GAP * u;
            paint_text_px(
                g,
                style,
                LEGEND_SEP,
                text_px(BAR_SEP_PX, u),
                sep_left,
                band.rect(),
                pal.text,
                ALPHA_SEP * alpha,
            );
        }
        let row = &legend[i];
        let parts = combo_parts(row.keys, family);
        let w = paint_combo(g, style, &parts, &cs, start, band, pal.text, alpha, lp.dip_scale);
        paint_text_px(
            g,
            style,
            row.label,
            text_px(BAR_LABEL_PX, u),
            start + w + BAR_LABEL_GAP * u,
            band.rect(),
            pal.text,
            alpha,
        );
    }
}

pub(crate) const ALPHA_LINE: f32 = 0x14 as f32 / 255.0;

pub(crate) const SIDE_PAD: f32 = 24.0;
pub(crate) const SIDE_LABEL_PX: f32 = 38.0;
pub(crate) const SIDE_GLYPH: f32 = 48.0;
pub(crate) const SIDE_SHARE: f32 = 56.0;
pub(crate) const SIDE_TOKEN_GAP: f32 = 8.0;
pub(crate) const SIDE_LABEL_GAP: f32 = 16.0;
pub(crate) const SIDE_FILL_ALPHA: f32 = 0x99 as f32 / 255.0;

pub(crate) const HINT_H: f32 = 88.0;
pub(crate) const MS_COLS: usize = 3;
pub(crate) const MS_SCALE: f32 = 3.0;
pub(crate) const MS_PAD: f32 = 32.0;
pub(crate) const MS_GAP: f32 = 20.0;
pub(crate) const MS_TILE_PAD: f32 = 26.0;
pub(crate) const MS_TITLE_PX: f32 = 24.0;
pub(crate) const MS_TITLE_H: f32 = 48.0;
pub(crate) const MS_LABEL_PX: f32 = 26.0;
pub(crate) const MS_LABEL_GAP: f32 = 20.0;
pub(crate) const MS_ROW_H: f32 = 62.0;
pub(crate) const MS_GLYPH: f32 = 44.0;
pub(crate) const MS_SHARE: f32 = 50.0;
pub(crate) const MS_RADIUS: f32 = 16.0;


pub(crate) fn sheet_combo_style(u: f32) -> ComboStyle {
    ComboStyle {
        glyph: MS_GLYPH * u,
        share: MS_SHARE * u,
        gap: SIDE_TOKEN_GAP * u,
        prefix_px: text_px(MS_LABEL_PX, u),
        connector_px: text_px(MS_LABEL_PX, u),
    }
}

pub(crate) fn side_combo_style(u: f32) -> ComboStyle {
    ComboStyle {
        glyph: SIDE_GLYPH * u,
        share: SIDE_SHARE * u,
        gap: SIDE_TOKEN_GAP * u,
        prefix_px: text_px(SIDE_LABEL_PX, u),
        connector_px: text_px(SIDE_LABEL_PX, u),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SheetMetrics {
    pub(crate) u: f32,
    pub(crate) cs: ComboStyle,
    pub(crate) row_h: f32,
    pub(crate) header_px: f32,
    pub(crate) action_px: f32,
    pub(crate) action_gap: f32,
    pub(crate) trail: f32,
    pub(crate) col_gap: f32,
}

impl SheetMetrics {
    pub(crate) fn at(u: f32) -> Self {
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

pub(crate) struct SheetColumn {
    pub(crate) lines: Vec<crate::win::shortcut_sheet::Line>,
    pub(crate) combo_w: f32,
    pub(crate) width: f32,
}

pub(crate) fn sheet_columns(
    family: ControllerIconFamily,
    m: &SheetMetrics,
    measure: Measure,
) -> Vec<SheetColumn> {
    crate::win::shortcut_sheet::shown_columns()
        .into_iter()
        .map(|lines| {
            let mut combo_w = 0.0f32;
            let mut label_w = 0.0f32;
            let mut header_w = 0.0f32;
            for line in &lines {
                match line {
                    crate::win::shortcut_sheet::Line::Row(row) => {
                        combo_w = combo_w.max(combo_width(
                            &combo_parts(row.keys, family),
                            &m.cs,
                            measure,
                        ));
                        label_w = label_w.max(measure(row.label, m.action_px));
                    }
                    crate::win::shortcut_sheet::Line::Title(t) => header_w = header_w.max(measure(t, m.header_px)),
                    crate::win::shortcut_sheet::Line::Blank => {}
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

pub(crate) fn sheet_block_size(total: f32, m: &SheetMetrics) -> (f32, f32) {
    let lines = crate::win::shortcut_sheet::shown_line_count() as f32;
    (
        total + 2.0 * SHEET_PAD_X * m.u,
        (SHEET_PAD_TOP + SHEET_PAD_BOTTOM) * m.u + lines * m.row_h,
    )
}

pub(crate) fn sheet_total_width(cols: &[SheetColumn], m: &SheetMetrics) -> f32 {
    let n = cols.len();
    let divider = if n >= 2 { m.col_gap + 1.0 } else { 0.0 };
    cols.iter().map(|c| c.width).sum::<f32>() + m.col_gap * n.saturating_sub(1) as f32 + divider
}

pub(crate) struct SheetPaint<'a> {
    pub(crate) pal: &'a VkPalette,
    pub(crate) style: VkStyle,
    pub(crate) family: ControllerIconFamily,
    pub(crate) dip_scale: f32,
}

pub(crate) unsafe fn paint_sheet_hint(
    g: &mut Gfx,
    sp: &SheetPaint,
    u: f32,
    center_x: f32,
    keys_top: f32,
    client_w: f32,
) {
    let (pal, style) = (sp.pal, sp.style);
    let hint = &crate::win::shortcut_sheet::SHEET_HINT;
    let cs = side_combo_style(u);
    let parts = combo_parts(hint.keys, sp.family);
    let label_px = text_px(SIDE_LABEL_PX, u);
    let combo_w = {
        let measure = |t: &str, px: f32| paint_measure_px(g, style, t, px);
        combo_width(&parts, &cs, &measure)
    };
    let label_w = paint_measure_px(g, style, hint.label, label_px);
    let pad = SIDE_PAD * u;
    let h = HINT_H * u;
    let w = (combo_w + SIDE_LABEL_GAP * u + label_w + 2.0 * pad).min(client_w);
    let left = (center_x - w * 0.5).clamp(0.0, (client_w - w).max(0.0));
    let top = ((keys_top - h) * 0.5).max(0.0);
    let pill = Edges {
        left,
        top,
        right: left + w,
        bottom: top + h,
    };
    let r = h * 0.5;
    g.fill(pill.rect(), r, pal.key, 1.0);
    g.ring(pill.rect(), r, 1.0, pal.panel_stroke, pal.panel_stroke_alpha * 2.0);
    let x = pill.left + pad;
    paint_combo(g, style, &parts, &cs, x, pill, pal.text, 1.0, sp.dip_scale);
    paint_text_px(
        g,
        style,
        hint.label,
        label_px,
        x + combo_w + SIDE_LABEL_GAP * u,
        pill.rect(),
        pal.text,
        1.0,
    );
}

pub(crate) unsafe fn modern_sheet_layout(
    g: &Gfx,
    style: VkStyle,
    family: ControllerIconFamily,
    unit: f32,
    panel: Edges,
    groups: &[(&'static str, Vec<crate::win::shortcut_sheet::SheetRow>)],
) -> (f32, Vec<f32>) {
    let cols = MS_COLS as f32;
    let grid_rows = groups.len().div_ceil(MS_COLS);
    let avail_w = (panel.right - panel.left).max(1.0);
    let avail_h = (panel.bottom - panel.top).max(1.0);
    let row_heights = |u: f32| -> Vec<f32> {
        (0..grid_rows)
            .map(|gr| {
                let n = groups
                    .iter()
                    .skip(gr * MS_COLS)
                    .take(MS_COLS)
                    .map(|(_, rows)| rows.len())
                    .max()
                    .unwrap_or(0) as f32;
                (MS_TITLE_H + 2.0 * MS_TILE_PAD) * u + n * MS_ROW_H * u
            })
            .collect()
    };
    let needed_h = |u: f32| {
        row_heights(u).iter().sum::<f32>()
            + (2.0 * MS_PAD + MS_GAP * (grid_rows as f32 - 1.0).max(0.0)) * u
    };
    let needed_w = |u: f32| {
        let measure = |t: &str, px: f32| paint_measure_px(g, style, t, px);
        let cs = sheet_combo_style(u);
        let tile = groups
            .iter()
            .map(|(_, rows)| {
                let combo = rows
                    .iter()
                    .map(|r| combo_width(&combo_parts(r.keys, family), &cs, &measure))
                    .fold(0.0f32, f32::max);
                let label = rows
                    .iter()
                    .map(|r| measure(r.label, text_px(MS_LABEL_PX, u)))
                    .fold(0.0f32, f32::max);
                combo + MS_LABEL_GAP * u + label + 2.0 * MS_TILE_PAD * u
            })
            .fold(0.0f32, f32::max);
        tile * cols + (2.0 * MS_PAD + MS_GAP * (cols - 1.0)) * u
    };
    let mut u = unit.max(1e-3);
    for _ in 0..4 {
        let fit = (avail_h / needed_h(u)).min(avail_w / needed_w(u));
        if fit >= 1.0 {
            break;
        }
        u *= fit * 0.995;
    }
    (u, row_heights(u))
}

pub(crate) unsafe fn paint_modern_sheet(
    g: &mut Gfx,
    sp: &SheetPaint,
    unit: f32,
    panel: Edges,
    radius: Option<f32>,
    alpha: f32,
) {
    let (pal, style, family) = (sp.pal, sp.style, sp.family);
    let r = radius.unwrap_or(0.0);
    g.fill(panel.rect(), r, pal.bg, alpha);
    let groups: Vec<(&'static str, Vec<crate::win::shortcut_sheet::SheetRow>)> =
        crate::win::shortcut_sheet::MODERN_GROUPS
            .iter()
            .map(|group| (group.title, crate::win::shortcut_sheet::shown_rows(group)))
            .collect();
    let (u, row_heights) = modern_sheet_layout(g, style, family, unit, panel, &groups);
    let pad = MS_PAD * u;
    let gap = MS_GAP * u;
    let total_h =
        row_heights.iter().sum::<f32>() + gap * (row_heights.len() as f32 - 1.0).max(0.0);
    let tile_w =
        (panel.right - panel.left - 2.0 * pad - gap * (MS_COLS as f32 - 1.0)) / MS_COLS as f32;
    let mut y = panel.top + ((panel.bottom - panel.top - total_h) * 0.5).max(pad);
    let tile_r = MS_RADIUS * u;
    let cs = sheet_combo_style(u);
    let label_px = text_px(MS_LABEL_PX, u);
    for (gr, &row_h) in row_heights.iter().enumerate() {
        for (gc, (title, rows)) in groups.iter().skip(gr * MS_COLS).take(MS_COLS).enumerate() {
            let left = panel.left + pad + gc as f32 * (tile_w + gap);
            let tile = Edges {
                left,
                top: y,
                right: left + tile_w,
                bottom: y + row_h,
            };
            g.fill(tile.rect(), tile_r, pal.key, SIDE_FILL_ALPHA * alpha);
            let inner = tile.deflate(MS_TILE_PAD * u);
            let title_band = Edges {
                bottom: inner.top + MS_TITLE_H * u,
                ..inner
            };
            paint_text_px(
                g,
                style,
                title,
                text_px(MS_TITLE_PX, u),
                inner.left,
                title_band.rect(),
                pal.text,
                ALPHA_SECONDARY * alpha,
            );
            let parts: Vec<Vec<ComboPart>> =
                rows.iter().map(|r| combo_parts(r.keys, family)).collect();
            let combo_w = {
                let measure = |t: &str, px: f32| paint_measure_px(g, style, t, px);
                parts
                    .iter()
                    .map(|p| combo_width(p, &cs, &measure))
                    .fold(0.0f32, f32::max)
            };
            for (i, (row, p)) in rows.iter().zip(&parts).enumerate() {
                let band = Edges {
                    top: title_band.bottom + i as f32 * MS_ROW_H * u,
                    bottom: title_band.bottom + (i as f32 + 1.0) * MS_ROW_H * u,
                    ..inner
                };
                paint_combo(g, style, p, &cs, inner.left, band, pal.text, alpha, sp.dip_scale);
                paint_text_px(
                    g,
                    style,
                    row.label,
                    label_px,
                    inner.left + combo_w + MS_LABEL_GAP * u,
                    band.rect(),
                    pal.text,
                    alpha,
                );
            }
        }
        y += row_h + gap;
    }
}

pub(crate) unsafe fn sheet_fit(
    g: &Gfx,
    style: VkStyle,
    family: ControllerIconFamily,
    unit: f32,
    panel: Edges,
) -> (SheetMetrics, Vec<SheetColumn>, f32) {
    let measure = |t: &str, px: f32| paint_measure_px(g, style, t, px);
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

pub(crate) unsafe fn paint_shortcut_sheet(
    g: &mut Gfx,
    sp: &SheetPaint,
    unit: f32,
    panel: Edges,
    radius: Option<f32>,
    alpha: f32,
) {
    use crate::win::shortcut_sheet::{self, Line};
    let (pal, style, family) = (sp.pal, sp.style, sp.family);
    let r = radius.unwrap_or(0.0);
    g.fill(panel.rect(), r, pal.bg, alpha);
    if radius.is_some() {
        g.ring(panel.rect(), r, 1.0, pal.panel_stroke, pal.panel_stroke_alpha * alpha);
    }
    let (m, cols, total) = sheet_fit(g, style, family, unit, panel);
    let lines = shortcut_sheet::shown_line_count() as f32;
    let block_h = (SHEET_PAD_TOP + SHEET_PAD_BOTTOM) * m.u + lines * m.row_h;
    let top =
        panel.top + ((panel.bottom - panel.top - block_h) * 0.5).max(0.0) + SHEET_PAD_TOP * m.u;
    let mut x = (panel.left + panel.right - total) * 0.5;
    let n = cols.len();
    for (ci, col) in cols.iter().enumerate() {
        for (i, line) in col.lines.iter().enumerate() {
            let band = Edges {
                left: x,
                top: top + i as f32 * m.row_h,
                right: x + col.width,
                bottom: top + (i as f32 + 1.0) * m.row_h,
            };
            match line {
                Line::Title(t) => {
                    paint_text_px(
                        g,
                        style,
                        t,
                        m.header_px,
                        x,
                        band.rect(),
                        pal.text,
                        ALPHA_SECONDARY * alpha,
                    );
                }
                Line::Row(row) => {
                    let parts = combo_parts(row.keys, family);
                    paint_combo(g, style, &parts, &m.cs, x, band, pal.text, alpha, sp.dip_scale);
                    paint_text_px(
                        g,
                        style,
                        row.label,
                        m.action_px,
                        x + col.combo_w + m.action_gap,
                        band.rect(),
                        pal.text,
                        alpha,
                    );
                }
                Line::Blank => {}
            }
            if shortcut_sheet::hairline_below(&col.lines, i) {
                let y = (band.bottom - 1.0).round();
                let hair = Edges {
                    left: x.round(),
                    top: y,
                    right: (x + col.width).round(),
                    bottom: y + 1.0,
                };
                g.fill_rect(hair.rect(), pal.text, ALPHA_LINE * alpha);
            }
        }
        x += col.width + m.col_gap;
        if n >= 2 && ci == n - 2 {
            let dx = x.round();
            let divider = Edges {
                left: dx,
                top: top.round(),
                right: dx + 1.0,
                bottom: (top + lines * m.row_h).round(),
            };
            g.fill_rect(divider.rect(), pal.text, ALPHA_LINE * alpha);
            x += 1.0 + m.col_gap;
        }
    }
}

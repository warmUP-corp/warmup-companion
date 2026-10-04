use std::time::Instant;

use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::config::{KeyboardTheme, VkStyle};
use crate::vk_geom::{self, KeyRect};
use crate::vk_key::{VK_BACK, VK_DOWN, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP};
use crate::vk_motion::{self, concentric_radius};
use crate::vk_nav::{KeyAction, KeyPos, KeyRow};
use crate::vk_predict::StripState;

pub const REF_MON_W: f32 = 1920.0;
const REF_KEY_W: f32 = 92.0;

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
const SHEET_W: f32 = 1600.0;
const SHEET_H: f32 = 546.0;
const SHEET_PAD_TOP: f32 = 24.0;
const SHEET_PAD_BOTTOM: f32 = 32.0;
const SHEET_COL_GAP: f32 = 72.0;
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
const VK_KB_REF_H: f32 = 384.0;

#[allow(dead_code)]
fn sheet_metrics_are_the_windows_ones() -> f32 {
    LEGEND_MIN_TEXT_PX
        + LEGEND_SEP.len() as f32
        + ALPHA_SECONDARY
        + ALPHA_SEP
        + BAR_GLYPH
        + BAR_SHARE
        + BAR_GAP
        + BAR_LABEL_GAP
        + BAR_LABEL_PX
        + BAR_CONNECTOR_PX
        + BAR_PREFIX_PX
        + BAR_SEP_PX
        + BAR_H
}

struct Spec {
    mono: bool,
    design_kh: f32,
    key_aspect: f32,
    gap: f32,
    key_radius: f32,
    pad_x: f32,
    pad_y: f32,
    panel_radius: f32,
    label_px: f32,
    label_weight: Weight,
    word_px: f32,
    word_weight: Weight,
    number_px: f32,
    number_alpha: f32,
    number_cy: f32,
    label_cy: f32,
    caption_cy: f32,
    space_icon_cy: f32,
    space_label: Option<&'static str>,
    space_label_px: f32,
    trim_symbols: bool,
    icon_px: f32,
    icon_large_px: f32,
    space_icon_px: f32,
    hint_badge: f32,
    hint_inset: f32,
    key_shadow_dy: f32,
    key_shadow_alpha: f32,
    sel_shadow: Option<(f32, f32, f32)>,
    sel_ring_w: f32,
    strip_bar_h: f32,
    chip_px: f32,
    chip_sel_weight: Weight,
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

fn spec_for(style: VkStyle) -> Spec {
    match style {
        VkStyle::Mono => Spec {
            mono: true,
            design_kh: 100.0,
            key_aspect: 100.0 / 137.0,
            gap: 8.0,
            key_radius: 12.0,
            pad_x: 13.0,
            pad_y: 13.0,
            panel_radius: 24.0,
            label_px: 32.0,
            label_weight: Weight::NORMAL,
            word_px: 24.0,
            word_weight: Weight::NORMAL,
            number_px: 14.0,
            number_alpha: 0x99 as f32 / 255.0,
            number_cy: 30.5,
            label_cy: 58.5,
            caption_cy: 23.0,
            space_icon_cy: 50.0,
            space_label: Some("space"),
            space_label_px: 22.0,
            trim_symbols: true,
            icon_px: 34.0,
            icon_large_px: 34.0,
            space_icon_px: 34.0,
            hint_badge: 36.0,
            hint_inset: 8.0,
            key_shadow_dy: 1.0,
            key_shadow_alpha: 0x99 as f32 / 255.0,
            sel_shadow: Some((8.0, 24.0, 0x80 as f32 / 255.0)),
            sel_ring_w: 2.0,
            strip_bar_h: 72.0,
            chip_px: 22.0,
            chip_sel_weight: Weight::NORMAL,
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
        },
        VkStyle::Normal => Spec {
            mono: false,
            design_kh: 102.0,
            key_aspect: 102.0 / 138.0,
            gap: 6.0,
            key_radius: 12.0,
            pad_x: 17.0,
            pad_y: 18.0,
            panel_radius: 25.0,
            label_px: 38.0,
            label_weight: Weight::MEDIUM,
            word_px: 26.0,
            word_weight: Weight::MEDIUM,
            number_px: 14.0,
            number_alpha: 0x8C as f32 / 255.0,
            number_cy: 25.0,
            label_cy: 60.5,
            caption_cy: 23.0,
            space_icon_cy: 62.5,
            space_label: None,
            space_label_px: 22.0,
            trim_symbols: false,
            icon_px: 36.0,
            icon_large_px: 40.0,
            space_icon_px: 52.0,
            hint_badge: 36.0,
            hint_inset: 8.0,
            key_shadow_dy: 1.0,
            key_shadow_alpha: 0x99 as f32 / 255.0,
            sel_shadow: None,
            sel_ring_w: 2.0,
            strip_bar_h: 112.0,
            chip_px: 20.0,
            chip_sel_weight: Weight::SEMIBOLD,
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
        },
    }
}

#[derive(Clone, Copy)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

struct Pal {
    bg: Rgb,
    key: Rgb,
    key_action: Rgb,
    accent: Rgb,
    sel_ring: Rgb,
    text: Rgb,
    text_dim: Rgb,
    sel_text: Rgb,
    border: Rgb,
    panel_stroke: Rgb,
    panel_stroke_alpha: f32,
    chip_sel: Rgb,
}

pub struct Frame {
    pub logical_w: f32,
    pub logical_h: f32,
    pub scale: f32,
    pub scale_w: f32,
    pub rows: Vec<KeyRow>,
    pub sel: KeyPos,
    pub style: VkStyle,
    pub theme: KeyboardTheme,
    pub strip: Option<StripState>,
    pub pressed: Option<(KeyPos, f32)>,
    pub ps5: bool,
    pub shift: bool,
    pub caps: bool,
    pub symbol: bool,
    pub symbol_locked: bool,
    pub floating: bool,
    pub sheet: bool,
}

pub struct Glide {
    pub ring: Option<(f32, f32, f32, f32)>,
    pub last: Option<Instant>,
    pub strip_at: Option<Instant>,
    pub animating: bool,
    base: Option<(u64, Pixmap)>,
}

impl Default for Glide {
    fn default() -> Self {
        Self {
            ring: None,
            last: None,
            strip_at: None,
            animating: false,
            base: None,
        }
    }
}

pub fn font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(include_bytes!("../assets/fonts/Selawik-Regular.ttf").to_vec());
    db.load_font_data(include_bytes!("../assets/fonts/Selawik-Bold.ttf").to_vec());
    db.set_sans_serif_family("Selawik");
    db.set_serif_family("Selawik");
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}

pub fn ui_scale() -> f32 {
    crate::config::vk_bar_scale() / crate::config::COMPACT_BAR_SCALE
}

pub fn floating_bottom_margin(output_h: f32) -> i32 {
    ((output_h.max(0.0) * 0.04).round() as i32).clamp(28, 80)
}

pub fn layout_scale(style: VkStyle) -> f32 {
    let key_w = match style {
        VkStyle::Normal => 138.0,
        VkStyle::Mono => 137.0,
    };
    key_w / REF_KEY_W
}

pub fn layout_scale_w(style: VkStyle) -> f32 {
    REF_MON_W * layout_scale(style)
}

pub fn card_size(output_w: f32, output_h: f32, docked: bool, rows: &[KeyRow], style: VkStyle) -> (u32, u32) {
    let spec = spec_for(style);
    let scale = layout_scale(style);
    let scale_w = REF_MON_W * scale;
    let (grid_w, block_h) = vk_geom::grid_size(scale_w, rows, spec.key_aspect, spec.gap, spec.design_kh);
    let u = ref_unit(scale, &spec);
    let chrome = (spec.strip_bar_h + spec.gap - spec.pad_y) * u;
    let legend = legend_band_height(scale, &spec);
    let (pad_x, pad_y) = (spec.pad_x * u, spec.pad_y * u);
    let height = (chrome + block_h + pad_y * 2.0 + legend).ceil().clamp(160.0, output_h.max(160.0));
    if docked {
        (output_w.max(640.0) as u32, height as u32)
    } else {
        let width = (grid_w + pad_x * 2.0).round().min(output_w.max(480.0));
        (width as u32, height as u32)
    }
}

#[cfg(test)]
fn sheet_size(output_w: f32, output_h: f32) -> (u32, u32) {
    let fit = (output_w / SHEET_W).min(output_h / SHEET_H).min(1.0).max(0.35);
    (
        (SHEET_W * fit).round().max(320.0) as u32,
        (SHEET_H * fit).round().max(160.0) as u32,
    )
}

fn legend_band_height(scale: f32, spec: &Spec) -> f32 {
    60.0 * ref_unit(scale, spec)
}

fn ref_unit(scale: f32, spec: &Spec) -> f32 {
    REF_KEY_W * scale.max(0.05) * spec.key_aspect / spec.design_kh
}

pub fn draw(
    frame: &Frame,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    glide: &mut Glide,
) -> Option<Pixmap> {
    let _ = sheet_metrics_are_the_windows_ones();
    if frame.sheet {
        glide.base = None;
        glide.animating = false;
        return draw_sheet_frame(frame, fonts, cache);
    }
    let now = Instant::now();
    if strip_blocking(glide, frame, now) {
        glide.base = None;
        return render_keyboard(frame, fonts, cache, glide, true);
    }
    let stamp = keyboard_stamp(frame);
    if glide.base.as_ref().map(|(s, _)| *s) != Some(stamp) {
        let mut base = render_keyboard(frame, fonts, cache, glide, false)?;
        if let Some(copy) = clone_pixmap(&base) {
            glide.base = Some((stamp, copy));
        }
        paint_live(&mut base, frame, fonts, cache, glide, false);
        return Some(base);
    }
    let mut shown = glide.base.as_ref().and_then(|(_, pixmap)| clone_pixmap(pixmap))?;
    paint_live(&mut shown, frame, fonts, cache, glide, true);
    Some(shown)
}

fn render_keyboard(
    frame: &Frame,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    glide: &mut Glide,
    highlight: bool,
) -> Option<Pixmap> {
    let scale = frame.scale.max(1.0);
    let width = (frame.logical_w * scale).round().max(1.0) as u32;
    let height = (frame.logical_h * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let spec = spec_for(frame.style);
    let pal = palette(frame.style, &frame.theme);
    let lu = ref_unit(ui_scale(), &spec);
    let top_inset = (spec.strip_bar_h + spec.gap - spec.pad_y) * lu;
    let legend_h = legend_band_height(ui_scale(), &spec);
    let rects = vk_geom::key_rects(
        frame.logical_w,
        (frame.logical_h - legend_h).max(40.0),
        frame.scale_w,
        &frame.rows,
        top_inset,
        spec.key_aspect,
        spec.gap,
        spec.design_kh,
    );
    let key_h = rects.first().map(|r| r.bottom - r.top).unwrap_or(spec.design_kh);
    let unit = key_h / spec.design_kh;
    let radius = spec.key_radius * unit;
    let now = Instant::now();
    let target = rects.iter().find(|r| r.pos == frame.sel).map(|r| (r.left, r.top, r.right, r.bottom));
    if let Some(tgt) = target {
        glide.last = Some(now);
        glide.ring = Some(tgt);
    }
    let panel = if frame.floating {
        let panel_r = spec.panel_radius * unit;
        let x = FLOATING_PANEL_INSET * scale;
        let y = FLOATING_PANEL_INSET * scale;
        let w = (frame.logical_w - 2.0 * FLOATING_PANEL_INSET) * scale;
        let h = (frame.logical_h - 2.0 * FLOATING_PANEL_INSET) * scale;
        round_rect(&mut pixmap, x, y, w, h, panel_r * scale, pal.bg, 255);
        stroke_rect(
            &mut pixmap,
            x + 0.5 * scale,
            y + 0.5 * scale,
            (w - scale).max(1.0),
            (h - scale).max(1.0),
            (panel_r - 0.5).max(0.0) * scale,
            pal.panel_stroke,
            (pal.panel_stroke_alpha * 255.0) as u8,
            scale.max(1.0),
        );
        Some((x, y, w, h, panel_r * scale))
    } else {
        fill(&mut pixmap, pal.bg, 255);
        None
    };
    for rect in &rects {
        let selected = highlight && rect.pos == frame.sel;
        let press = if highlight {
            frame.pressed.filter(|(p, _)| *p == rect.pos).map(|(_, s)| s).unwrap_or(1.0)
        } else {
            1.0
        };
        paint_key(&mut pixmap, fonts, cache, frame, &spec, &pal, rect, unit, key_h, scale, radius, selected, press, true);
    }
    if highlight {
        stroke_ring(&mut pixmap, glide, &spec, &pal, unit, scale, radius);
    }
    let (strip_alpha, strip_dy) = if frame.strip.is_some() {
        if glide.strip_at.is_none() {
            glide.strip_at = Some(now);
        }
        let elapsed = glide
            .strip_at
            .map(|t| now.duration_since(t).as_secs_f32() * 1000.0)
            .unwrap_or(0.0);
        vk_motion::strip_enter(elapsed)
    } else {
        glide.strip_at = None;
        (1.0, 0.0)
    };
    if let Some(strip) = &frame.strip {
        draw_strip(&mut pixmap, fonts, cache, &spec, &pal, strip, &rects, unit, scale, frame.ps5, strip_dy, strip_alpha);
    }
    draw_legend(&mut pixmap, fonts, cache, &spec, &pal, &rects, frame.logical_w, frame.logical_h, lu, scale, frame.ps5);
    if let Some((x, y, w, h, radius)) = panel {
        clip_round(&mut pixmap, x, y, w, h, radius);
    }
    let strip_moving = glide
        .strip_at
        .is_some_and(|t| t.elapsed().as_secs_f32() * 1000.0 < vk_motion::STRIP_ENTER_MS);
    let pressing = frame.pressed.is_some_and(|(_, s)| s < 0.999);
    glide.animating = strip_moving || pressing;
    Some(pixmap)
}

#[derive(Clone, Copy)]
enum Leg {
    Btn(&'static str),
    Word(&'static str),
    Plus,
    Slash,
}

const LEGEND_ROWS: &[(&[Leg], &str)] = &[
    (&[Leg::Btn("A")], "Type"),
    (&[Leg::Btn("B")], "Delete"),
    (&[Leg::Btn("LB"), Leg::Slash, Leg::Btn("RB")], "Move caret"),
    (&[Leg::Btn("RT")], "Shift"),
    (&[Leg::Btn("R3")], "Dictate"),
    (&[Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("Y")], "Paste"),
    (&[Leg::Word("Tap"), Leg::Btn("SELECT")], "Suggestions"),
    (&[Leg::Btn("L3")], "Close keyboard"),
    (&[Leg::Word("Hold"), Leg::Btn("SELECT")], "All shortcuts"),
];

fn text_px(design: f32, u: f32) -> f32 {
    (design * u).max(LEGEND_MIN_TEXT_PX)
}

fn fit_legend(widths: &[f32], sep: f32, avail: f32) -> Vec<usize> {
    let mut kept: Vec<usize> = (0..widths.len()).collect();
    let total = |kept: &[usize]| {
        kept.iter().map(|&i| widths[i]).sum::<f32>() + sep * kept.len().saturating_sub(1) as f32
    };
    while kept.len() > 1 && total(&kept) > avail {
        kept.remove((kept.len() - 1) / 2);
    }
    kept
}

fn leg_part_width(part: &Leg, u: f32, fonts: &mut FontSystem) -> f32 {
    match *part {
        Leg::Btn("SELECT") => 28.0 * u,
        Leg::Btn(_) => 24.0 * u,
        Leg::Word(word) => measure_label(fonts, word, 15.0 * u),
        Leg::Plus => measure_label(fonts, "+", 13.0 * u),
        Leg::Slash => measure_label(fonts, "/", 13.0 * u),
    }
}

fn legend_item_width(parts: &[Leg], label: &str, u: f32, fonts: &mut FontSystem) -> f32 {
    let gaps = 6.0 * u * parts.len().saturating_sub(1) as f32;
    let body: f32 = parts.iter().map(|part| leg_part_width(part, u, fonts)).sum();
    body + gaps + 6.0 * u + measure_label(fonts, label, 15.0 * u)
}

fn draw_legend(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    _spec: &Spec,
    pal: &Pal,
    _rects: &[KeyRect],
    frame_w: f32,
    frame_h: f32,
    u: f32,
    scale: f32,
    ps5: bool,
) {
    let pad_x = 16.0 * u;
    let left = pad_x;
    let right = (frame_w - pad_x).max(left);
    let dot_w = measure_label(fonts, LEGEND_SEP, 15.0 * u);
    let sep_w = 10.0 * u + dot_w + 10.0 * u;
    let widths: Vec<f32> = LEGEND_ROWS
        .iter()
        .map(|(parts, label)| legend_item_width(parts, label, u, fonts))
        .collect();
    let kept = fit_legend(&widths, sep_w, (right - left).max(1.0));
    let total = kept.iter().map(|&i| widths[i]).sum::<f32>() + sep_w * kept.len().saturating_sub(1) as f32;
    let mut x = (left + right - total) * 0.5;
    let top = frame_h - 60.0 * u;
    let bottom = frame_h;
    let secondary = (ALPHA_SECONDARY * 255.0) as u8;
    let sep_alpha = (ALPHA_SEP * 255.0) as u8;
    let label_px = 15.0 * u;
    for (n, &index) in kept.iter().enumerate() {
        if n > 0 {
            let sep_left = x - sep_w + 10.0 * u;
            let sep_w_text = measure_label(fonts, LEGEND_SEP, 15.0 * u);
            text_aligned(
                pixmap,
                fonts,
                cache,
                LEGEND_SEP,
                (sep_left, top, sep_left + sep_w_text, bottom),
                15.0 * u * scale,
                Weight::NORMAL,
                pal.text,
                sep_alpha,
                scale,
                Align::Left,
            );
        }
        let (parts, label) = LEGEND_ROWS[index];
        let mut cursor = x;
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                cursor += 6.0 * u;
            }
            let w = leg_part_width(part, u, fonts);
            match *part {
                Leg::Btn(hint) => {
                    if let Some(svg) = hint_svg(ps5, hint) {
                        blit_svg(pixmap, svg, cursor, top, cursor + w, bottom, w, scale, None, 255);
                    }
                }
                Leg::Word(_) | Leg::Plus | Leg::Slash => {
                    let shown = match *part {
                        Leg::Word(word) => word,
                        Leg::Plus => "+",
                        Leg::Slash => "/",
                        Leg::Btn(_) => "",
                    };
                    text_aligned(
                        pixmap,
                        fonts,
                        cache,
                        shown,
                        (cursor, top, cursor + w, bottom),
                        (if matches!(*part, Leg::Word(_)) { 15.0 } else { 13.0 }) * u * scale,
                        Weight::NORMAL,
                        pal.text,
                        secondary,
                        scale,
                        Align::Left,
                    );
                }
            }
            cursor += w;
        }
        let label_w = measure_label(fonts, label, label_px);
        let label_x = cursor + 6.0 * u;
        text_aligned(
            pixmap,
            fonts,
            cache,
            label,
            (label_x, top, label_x + label_w, bottom),
            label_px * scale,
            Weight::NORMAL,
            pal.text,
            255,
            scale,
            Align::Left,
        );
        x += widths[index] + sep_w;
    }
}

fn strip_blocking(glide: &Glide, frame: &Frame, now: Instant) -> bool {
    if frame.strip.is_none() {
        return false;
    }
    match glide.strip_at {
        None => true,
        Some(t) => now.saturating_duration_since(t).as_secs_f32() * 1000.0 < vk_motion::STRIP_ENTER_MS,
    }
}

pub(crate) fn keyboard_stamp(frame: &Frame) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    frame.logical_w.to_bits().hash(&mut hasher);
    frame.logical_h.to_bits().hash(&mut hasher);
    frame.scale.to_bits().hash(&mut hasher);
    frame.scale_w.to_bits().hash(&mut hasher);
    ui_scale().to_bits().hash(&mut hasher);
    matches!(frame.style, VkStyle::Mono).hash(&mut hasher);
    frame.ps5.hash(&mut hasher);
    frame.shift.hash(&mut hasher);
    frame.caps.hash(&mut hasher);
    frame.symbol.hash(&mut hasher);
    frame.symbol_locked.hash(&mut hasher);
    frame.floating.hash(&mut hasher);
    frame.theme.bg.hash(&mut hasher);
    frame.theme.key.hash(&mut hasher);
    frame.theme.accent.hash(&mut hasher);
    frame.theme.text.hash(&mut hasher);
    frame.theme.sel_text.hash(&mut hasher);
    frame.theme.border.hash(&mut hasher);
    for row in &frame.rows {
        for key in &row.keys {
            key.label.hash(&mut hasher);
            key.sublabel.hash(&mut hasher);
            key.span.to_bits().hash(&mut hasher);
            match &key.action {
                KeyAction::Char(c) => {
                    0u8.hash(&mut hasher);
                    c.hash(&mut hasher);
                }
                KeyAction::Vk(vk) => {
                    1u8.hash(&mut hasher);
                    vk.0.hash(&mut hasher);
                }
                KeyAction::Shift => 2u8.hash(&mut hasher),
                KeyAction::Symbols => 3u8.hash(&mut hasher),
                KeyAction::PredictPrev => 4u8.hash(&mut hasher),
                KeyAction::PredictNext => 5u8.hash(&mut hasher),
                KeyAction::VoiceInput => 6u8.hash(&mut hasher),
            }
        }
    }
    if let Some(strip) = &frame.strip {
        true.hash(&mut hasher);
        strip.visible.hash(&mut hasher);
        strip.highlight_slot.hash(&mut hasher);
        strip.engaged.hash(&mut hasher);
    } else {
        false.hash(&mut hasher);
    }
    hasher.finish()
}

fn clone_pixmap(src: &Pixmap) -> Option<Pixmap> {
    let size = tiny_skia::IntSize::from_wh(src.width(), src.height())?;
    Pixmap::from_vec(src.data().to_vec(), size)
}

fn paint_key(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    frame: &Frame,
    spec: &Spec,
    pal: &Pal,
    rect: &KeyRect,
    unit: f32,
    key_h: f32,
    scale: f32,
    radius: f32,
    selected: bool,
    press: f32,
    shadow: bool,
) {
    let Some(key) = frame.rows.get(rect.pos.row).and_then(|row| row.keys.get(rect.pos.col)) else { return };
    let (left, top, right, bottom) = scaled_about(rect, press);
    let action = is_action(&key.action);
    let modifier = !selected
        && ((matches!(key.action, KeyAction::Shift) && frame.shift && frame.caps)
            || (matches!(key.action, KeyAction::Symbols) && frame.symbol && frame.symbol_locked));
    let (fill_c, label_c) = if selected {
        (pal.accent, pal.sel_text)
    } else if modifier {
        (mix(pal.key, pal.accent, 0.35), pal.text)
    } else if action {
        (pal.key_action, pal.text)
    } else {
        (pal.key, pal.text)
    };
    if shadow {
        if selected {
            if let Some((dy, blur, alpha)) = spec.sel_shadow {
                blur_shadow(pixmap, left * scale, top * scale, (right - left) * scale, (bottom - top) * scale, radius * scale, dy * unit * scale, blur * unit * scale, alpha);
            }
        } else if spec.key_shadow_dy > 0.0 {
            let dy = (spec.key_shadow_dy * unit).max(1.0);
            round_rect(pixmap, left * scale, (top + dy) * scale, (right - left) * scale, (bottom - top) * scale, radius * scale, Rgb { r: 0, g: 0, b: 0 }, (spec.key_shadow_alpha * 255.0) as u8);
        }
    }
    round_rect(pixmap, left * scale, top * scale, (right - left) * scale, (bottom - top) * scale, radius * scale, fill_c, 255);
    let locked = (matches!(key.action, KeyAction::Shift) && frame.caps) || (matches!(key.action, KeyAction::Symbols) && frame.symbol_locked);
    if locked {
        let pill_w = ((right - left) * 0.28).min(24.0 * unit);
        let pill_h = (3.0 * unit).max(2.0);
        let cx = (left + right) * 0.5;
        let yb = bottom - 5.0 * unit;
        round_rect(pixmap, (cx - pill_w * 0.5) * scale, (yb - pill_h) * scale, pill_w * scale, pill_h * scale, pill_h * 0.5 * scale, if selected { pal.sel_text } else { pal.accent }, 255);
    }
    let is_space = matches!(key.action, KeyAction::Vk(vk) if vk == VK_SPACE);
    let dim = if selected { pal.sel_text } else { pal.text_dim };
    let dim_a = (spec.number_alpha * 255.0) as u8;
    if let Some(sub) = key.sublabel.as_deref().filter(|_| !is_space) {
        let band = band_at(left, top, right, spec.number_cy * unit, key_h);
        text(pixmap, fonts, cache, sub, band, spec.number_px * unit * scale, Weight::NORMAL, dim, dim_a, scale);
    }
    if matches!(key.action, KeyAction::VoiceInput) {
        blit_svg(pixmap, MIC, left, top, right, bottom, spec.icon_px * unit, scale, Some(label_c), 255);
    } else if is_space {
        if let Some(word) = spec.space_label {
            text(pixmap, fonts, cache, word, (left, top, right, bottom), spec.space_label_px * unit * scale, Weight::NORMAL, label_c, 255, scale);
        } else if let Some(sub) = key.sublabel.as_deref() {
            let cap = band_at(left, top, right, spec.caption_cy * unit, key_h);
            text(pixmap, fonts, cache, sub, cap, spec.number_px * unit * scale, Weight::MEDIUM, dim, dim_a, scale);
            let icon = band_at(left, top, right, spec.space_icon_cy * unit, key_h);
            blit_svg(pixmap, SPACE, icon.0, icon.1, icon.2, icon.3, spec.space_icon_px * unit, scale, Some(label_c), 255);
        }
    } else if let Some((svg, large)) = key_icon(&key.action, frame.caps) {
        let px = if large { spec.icon_large_px } else { spec.icon_px } * unit;
        blit_svg(pixmap, svg, left, top, right, bottom, px, scale, Some(label_c), 255);
    } else {
        let mut glyph = key.label.clone();
        if spec.trim_symbols && matches!(key.action, KeyAction::Symbols) {
            glyph = glyph.trim_start_matches('?').to_string();
        }
        if !glyph.is_empty() {
            let weight = if glyph.chars().count() > 1 { spec.word_weight } else { spec.label_weight };
            let px = if glyph.chars().count() > 1 { spec.word_px } else { spec.label_px };
            let band = if key.sublabel.is_some() {
                band_at(left, top, right, spec.label_cy * unit, key_h)
            } else {
                (left, top, right, bottom)
            };
            text(pixmap, fonts, cache, &glyph, band, px * unit * scale, weight, label_c, 255, scale);
        }
    }
    if let Some(hint) = key_hint(&key.action).filter(|_| !spec.mono) {
        let badge = spec.hint_badge * unit;
        let inset = spec.hint_inset * unit;
        let bx = left + inset;
        let by = top + inset;
        if let Some(svg) = hint_svg(frame.ps5, hint) {
            blit_svg(pixmap, svg, bx, by, bx + badge, by + badge, badge, scale, None, 255);
        }
    }
}

fn stroke_ring(pixmap: &mut Pixmap, glide: &Glide, spec: &Spec, pal: &Pal, unit: f32, scale: f32, radius: f32) {
    let Some((l, t, r, b)) = glide.ring else { return };
    let ring_w = (spec.sel_ring_w * unit).max(1.0);
    stroke_rect(pixmap, (l + ring_w * 0.5) * scale, (t + ring_w * 0.5) * scale, (r - l - ring_w) * scale, (b - t - ring_w) * scale, (radius - ring_w * 0.5).max(0.0) * scale, pal.sel_ring, 255, ring_w * scale);
}

fn erase_key(pixmap: &mut Pixmap, rect: &KeyRect, spec: &Spec, pal: &Pal, unit: f32, scale: f32) {
    let dy = (spec.key_shadow_dy * unit).max(0.0);
    round_rect(pixmap, rect.left * scale, rect.top * scale, (rect.right - rect.left) * scale, (rect.bottom - rect.top + dy) * scale, 0.0, pal.bg, 255);
}

fn scene(frame: &Frame) -> (Spec, Pal, Vec<KeyRect>, f32, f32, f32, f32) {
    let scale = frame.scale.max(1.0);
    let spec = spec_for(frame.style);
    let pal = palette(frame.style, &frame.theme);
    let lu = ref_unit(ui_scale(), &spec);
    let top_inset = (spec.strip_bar_h + spec.gap - spec.pad_y) * lu;
    let legend_h = legend_band_height(ui_scale(), &spec);
    let rects = vk_geom::key_rects(
        frame.logical_w,
        (frame.logical_h - legend_h).max(40.0),
        frame.scale_w,
        &frame.rows,
        top_inset,
        spec.key_aspect,
        spec.gap,
        spec.design_kh,
    );
    let key_h = rects.first().map(|r| r.bottom - r.top).unwrap_or(spec.design_kh);
    let unit = key_h / spec.design_kh;
    let radius = spec.key_radius * unit;
    (spec, pal, rects, unit, scale, radius, key_h)
}

fn advance_glide(glide: &mut Glide, frame: &Frame, target: Option<(f32, f32, f32, f32)>, now: Instant) {
    if let Some(tgt) = target {
        glide.last = Some(now);
        glide.ring = Some(tgt);
    }
    let strip_moving = glide.strip_at.is_some_and(|t| t.elapsed().as_secs_f32() * 1000.0 < vk_motion::STRIP_ENTER_MS);
    let pressing = frame.pressed.is_some_and(|(_, scale)| scale < 0.999);
    glide.animating = strip_moving || pressing;
}

fn paint_live(pixmap: &mut Pixmap, frame: &Frame, fonts: &mut FontSystem, cache: &mut SwashCache, glide: &mut Glide, step: bool) {
    let (spec, pal, rects, unit, scale, radius, key_h) = scene(frame);
    let target = rects.iter().find(|r| r.pos == frame.sel).map(|r| (r.left, r.top, r.right, r.bottom));
    if step {
        advance_glide(glide, frame, target, Instant::now());
    }
    if let Some(sel_index) = rects.iter().position(|r| r.pos == frame.sel) {
        erase_key(pixmap, &rects[sel_index], &spec, &pal, unit, scale);
        let press = frame.pressed.filter(|(p, _)| *p == rects[sel_index].pos).map(|(_, s)| s).unwrap_or(1.0);
        paint_key(pixmap, fonts, cache, frame, &spec, &pal, &rects[sel_index], unit, key_h, scale, radius, true, press, true);
        if let Some((pos, press)) = frame.pressed {
            if pos != rects[sel_index].pos {
                if let Some(other) = rects.iter().find(|rect| rect.pos == pos) {
                    erase_key(pixmap, other, &spec, &pal, unit, scale);
                    paint_key(pixmap, fonts, cache, frame, &spec, &pal, other, unit, key_h, scale, radius, false, press, true);
                }
            }
        }
    }
    stroke_ring(pixmap, glide, &spec, &pal, unit, scale, radius);
    if frame.floating {
        let panel_r = spec.panel_radius * unit * scale;
        let x = FLOATING_PANEL_INSET * scale;
        let y = FLOATING_PANEL_INSET * scale;
        let w = (frame.logical_w - 2.0 * FLOATING_PANEL_INSET) * scale;
        let h = (frame.logical_h - 2.0 * FLOATING_PANEL_INSET) * scale;
        clip_round(pixmap, x, y, w, h, panel_r);
    }
}

fn draw_sheet_frame(frame: &Frame, fonts: &mut FontSystem, cache: &mut SwashCache) -> Option<Pixmap> {
    let (spec, pal, _, unit, scale, _, _) = scene(frame);
    let width = (frame.logical_w * scale).round().max(1.0) as u32;
    let height = (frame.logical_h * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let panel = if frame.floating {
        let panel_r = spec.panel_radius * unit;
        let x = FLOATING_PANEL_INSET * scale;
        let y = FLOATING_PANEL_INSET * scale;
        let w = (frame.logical_w - 2.0 * FLOATING_PANEL_INSET) * scale;
        let h = (frame.logical_h - 2.0 * FLOATING_PANEL_INSET) * scale;
        round_rect(&mut pixmap, x, y, w, h, panel_r * scale, rgb(0x1F1F1F), 255);
        stroke_rect(
            &mut pixmap,
            x + 0.5 * scale,
            y + 0.5 * scale,
            (w - scale).max(1.0),
            (h - scale).max(1.0),
            (panel_r - 0.5).max(0.0) * scale,
            pal.panel_stroke,
            (pal.panel_stroke_alpha * 255.0) as u8,
            scale.max(1.0),
        );
        Some((x, y, w, h, panel_r * scale))
    } else {
        fill(&mut pixmap, rgb(0x1F1F1F), 255);
        None
    };
    paint_sheet(&mut pixmap, fonts, cache, frame.logical_w, frame.logical_h, scale, frame.ps5);
    if let Some((x, y, w, h, radius)) = panel {
        clip_round(&mut pixmap, x, y, w, h, radius);
    }
    Some(pixmap)
}

enum SheetLine {
    Title(&'static str),
    Blank,
    Row(&'static [Leg], &'static str),
}

struct SheetCol {
    lines: Vec<SheetLine>,
    combo: f32,
    width: f32,
}

fn sheet_part_width(part: &Leg, fonts: &mut FontSystem) -> f32 {
    match *part {
        Leg::Btn("SELECT") => SHEET_SHARE,
        Leg::Btn(_) => SHEET_GLYPH,
        Leg::Word(word) => measure_label(fonts, word, SHEET_TOKEN_PX),
        Leg::Plus => measure_label(fonts, "+", SHEET_TOKEN_PX),
        Leg::Slash => measure_label(fonts, "/", SHEET_TOKEN_PX),
    }
}

fn sheet_combo_width(parts: &[Leg], fonts: &mut FontSystem) -> f32 {
    parts.iter().map(|part| sheet_part_width(part, fonts)).sum::<f32>() + SHEET_TOKEN_GAP * parts.len().saturating_sub(1) as f32
}

fn sheet_columns(fonts: &mut FontSystem) -> [SheetCol; 3] {
    const L3: &[Leg] = &[Leg::Btn("L3")];
    const R3: &[Leg] = &[Leg::Btn("R3")];
    const TAP: &[Leg] = &[Leg::Word("Tap"), Leg::Btn("SELECT")];
    const COPY: &[Leg] = &[Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("X")];
    const PASTE: &[Leg] = &[Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("Y")];
    const CLEAR: &[Leg] = &[Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("B")];
    const CARET: &[Leg] = &[Leg::Btn("LB"), Leg::Slash, Leg::Btn("RB")];
    const JUMP: &[Leg] = &[Leg::Word("Hold"), Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("LB"), Leg::Slash, Leg::Btn("RB")];
    const SHIFT_K: &[Leg] = &[Leg::Btn("RT")];
    const SYMBOLS: &[Leg] = &[Leg::Btn("LT")];
    const LAYOUT: &[Leg] = &[Leg::Btn("X")];
    const SHOT: &[Leg] = &[Leg::Btn("LB"), Leg::Plus, Leg::Btn("RB"), Leg::Plus, Leg::Btn("RT")];
    const WINDOW: &[Leg] = &[Leg::Btn("LB"), Leg::Plus, Leg::Btn("RB"), Leg::Plus, Leg::Btn("LT")];
    const OPEN: &[Leg] = &[Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("LB"), Leg::Plus, Leg::Btn("X")];
    const GAME_SHOT: &[Leg] = &[Leg::Word("Hold"), Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("LB")];
    const RECORD: &[Leg] = &[Leg::Word("Hold"), Leg::Btn("SELECT"), Leg::Plus, Leg::Btn("RB")];
    let raw = [
        vec![
            SheetLine::Title("Keyboard"),
            SheetLine::Row(L3, "Close keyboard"),
            SheetLine::Row(R3, "Dictate"),
            SheetLine::Row(TAP, "Suggestions"),
            SheetLine::Row(COPY, "Copy"),
            SheetLine::Row(PASTE, "Paste"),
            SheetLine::Row(CLEAR, "Clear field"),
        ],
        vec![
            SheetLine::Blank,
            SheetLine::Row(CARET, "Move caret"),
            SheetLine::Row(JUMP, "Jump a word"),
            SheetLine::Row(SHIFT_K, "Shift"),
            SheetLine::Row(SYMBOLS, "Symbols"),
            SheetLine::Row(LAYOUT, "Switch layout"),
        ],
        vec![
            SheetLine::Title("Keyboard closed"),
            SheetLine::Row(SHOT, "Screenshot"),
            SheetLine::Row(WINDOW, "Window screenshot"),
            SheetLine::Row(OPEN, "Open warmUP"),
            SheetLine::Title("In games"),
            SheetLine::Row(GAME_SHOT, "Screenshot"),
            SheetLine::Row(RECORD, "Record (warmUP)"),
        ],
    ];
    raw.map(|lines| {
        let mut combo = 0.0f32;
        let mut label = 0.0f32;
        let mut header = 0.0f32;
        for line in &lines {
            match line {
                SheetLine::Row(parts, text) => {
                    combo = combo.max(sheet_combo_width(parts, fonts));
                    label = label.max(measure_label(fonts, text, SHEET_ACTION_PX));
                }
                SheetLine::Title(text) => header = header.max(measure_label(fonts, text, SHEET_HEADER_PX)),
                SheetLine::Blank => {}
            }
        }
        let width = (combo + SHEET_ACTION_GAP + label + SHEET_TRAIL).max(header);
        SheetCol { lines, combo, width }
    })
}

fn sheet_total(cols: &[SheetCol]) -> f32 {
    let n = cols.len();
    cols.iter().map(|col| col.width).sum::<f32>() + SHEET_COL_GAP * n as f32 + if n >= 2 { 1.0 } else { 0.0 }
}

fn sheet_hairline(lines: &[SheetLine], index: usize) -> bool {
    matches!(lines.get(index), Some(SheetLine::Row(_, _))) && matches!(lines.get(index + 1), Some(SheetLine::Row(_, _)))
}

fn leg_word(part: &Leg) -> &'static str {
    match *part {
        Leg::Word(word) => word,
        Leg::Plus => "+",
        Leg::Slash => "/",
        Leg::Btn(_) => "",
    }
}

fn paint_sheet(pixmap: &mut Pixmap, fonts: &mut FontSystem, cache: &mut SwashCache, frame_w: f32, frame_h: f32, scale: f32, ps5: bool) {
    let cols = sheet_columns(fonts);
    let total = sheet_total(&cols);
    let rows = cols.iter().map(|col| col.lines.len()).max().unwrap_or(1);
    let design_h = SHEET_PAD_TOP + rows as f32 * SHEET_ROW_H + SHEET_PAD_BOTTOM;
    let u = (frame_w / SHEET_W).min(frame_h / design_h).clamp(0.35, 1.0);
    let row_h = SHEET_ROW_H * u;
    let top = SHEET_PAD_TOP * u;
    let block_h = rows as f32 * row_h;
    let mut x = (frame_w - total * u) * 0.5;
    let count = cols.len();
    let muted = (ALPHA_SECONDARY * 255.0) as u8;
    let line_a = (ALPHA_LINE * 255.0) as u8;
    let white = rgb(0xFFFFFF);
    for (ci, col) in cols.iter().enumerate() {
        let col_w = col.width * u;
        for (index, line) in col.lines.iter().enumerate() {
            let y = top + index as f32 * row_h;
            match line {
                SheetLine::Title(title) => {
                    text_aligned(pixmap, fonts, cache, title, (x, y, x + col_w, y + row_h), SHEET_HEADER_PX * u * scale, Weight::NORMAL, white, muted, scale, Align::Left);
                }
                SheetLine::Row(parts, label) => {
                    let mut cursor = x;
                    for (part_index, part) in parts.iter().enumerate() {
                        if part_index > 0 {
                            cursor += SHEET_TOKEN_GAP * u;
                        }
                        let w = sheet_part_width(part, fonts) * u;
                        match *part {
                            Leg::Btn(hint) => {
                                if let Some(svg) = hint_svg(ps5, hint) {
                                    blit_svg(pixmap, svg, cursor, y, cursor + w, y + row_h, w, scale, None, 255);
                                }
                            }
                            Leg::Word(_) | Leg::Plus | Leg::Slash => {
                                text_aligned(pixmap, fonts, cache, leg_word(part), (cursor, y, cursor + w, y + row_h), SHEET_TOKEN_PX * u * scale, Weight::NORMAL, white, muted, scale, Align::Left);
                            }
                        }
                        cursor += w;
                    }
                    let label_x = x + col.combo * u + SHEET_ACTION_GAP * u;
                    text_aligned(pixmap, fonts, cache, label, (label_x, y, x + col_w, y + row_h), SHEET_ACTION_PX * u * scale, Weight::NORMAL, white, 255, scale, Align::Left);
                }
                SheetLine::Blank => {}
            }
            if sheet_hairline(&col.lines, index) {
                round_rect(pixmap, x * scale, (y + row_h - u) * scale, col_w * scale, u * scale, 0.0, white, line_a);
            }
        }
        x += col_w;
        if ci + 1 == count {
            break;
        }
        x += SHEET_COL_GAP * u;
        if ci + 2 == count {
            round_rect(pixmap, x * scale, top * scale, u * scale, block_h * scale, 0.0, white, line_a);
            x += u + SHEET_COL_GAP * u;
        }
    }
}

#[cfg(test)]
fn sheet_layout(fonts: &mut FontSystem) -> (f32, [f32; 3]) {
    let cols = sheet_columns(fonts);
    (sheet_total(&cols), [cols[0].combo, cols[1].combo, cols[2].combo])
}


fn draw_strip(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    spec: &Spec,
    pal: &Pal,
    strip: &StripState,
    rects: &[KeyRect],
    unit: f32,
    scale: f32,
    ps5: bool,
    dy: f32,
    fade: f32,
) {
    if rects.is_empty() || strip.visible.iter().all(|w| w.is_empty()) || fade <= 0.0 {
        return;
    }
    let fade_u8 = |a: f32| (a.clamp(0.0, 1.0) * fade * 255.0) as u8;
    let left = rects.iter().map(|r| r.left).fold(f32::INFINITY, f32::min);
    let right = rects.iter().map(|r| r.right).fold(f32::NEG_INFINITY, f32::max);
    let grid_top = rects.iter().map(|r| r.top).fold(f32::INFINITY, f32::min);
    let bottom = grid_top - spec.gap * unit + dy;
    let top = (bottom - spec.strip_bar_h * unit).max(0.0);
    let cy = (top + bottom) * 0.5;
    if spec.mono {
        let inner_l = left;
        let inner_r = right;
        let n = strip.visible.len().max(1) as f32;
        let col_w = (inner_r - inner_l) / n;
        let sep_h = spec.separator_h * unit;
        for (i, word) in strip.visible.iter().enumerate() {
            let x0 = inner_l + i as f32 * col_w;
            let x1 = x0 + col_w;
            if i > 0 {
                round_rect(pixmap, (x0 - 0.5) * scale, (cy - sep_h * 0.5) * scale, scale.max(1.0), sep_h * scale, 0.0, pal.border, fade_u8(spec.separator_alpha));
            }
            if word.is_empty() {
                continue;
            }
            let selected = i == strip.highlight_slot;
            let alpha = fade_u8(if selected { 1.0 } else { spec.chip_text_alpha });
            let color = if selected { pal.text } else { pal.text_dim };
            text(pixmap, fonts, cache, word, (x0, top, x1, bottom), spec.chip_px * unit * scale, if selected { spec.chip_sel_weight } else { Weight::NORMAL }, color, alpha, scale);
        }
        return;
    }
    let button_w = spec.strip_button_w * unit;
    let button_h = spec.strip_button_h * unit;
    let gap = spec.strip_button_gap * unit;
    let prev = (left, cy - button_h * 0.5, left + button_w, cy + button_h * 0.5);
    let next = (right - button_w, cy - button_h * 0.5, right, cy + button_h * 0.5);
    let chips = (prev.2 + gap, cy - spec.chips_h * unit * 0.5, next.0 - gap, cy + spec.chips_h * unit * 0.5);
    let inner_t = chips.1 + spec.chips_pad * unit;
    let inner_b = chips.3 - spec.chips_pad * unit;
    let chip_r = ((inner_b - inner_t) * 0.5).max(0.0);
    let chips_r = concentric_radius(chip_r, spec.chips_pad * unit);
    round_rect(pixmap, chips.0 * scale, chips.1 * scale, (chips.2 - chips.0) * scale, (chips.3 - chips.1) * scale, chips_r * scale, pal.key_action, fade_u8(1.0));
    stroke_rect(pixmap, (chips.0 + 0.5) * scale, (chips.1 + 0.5) * scale, (chips.2 - chips.0 - 1.0) * scale, (chips.3 - chips.1 - 1.0) * scale, (chips_r - 0.5).max(0.0) * scale, pal.border, fade_u8(1.0), scale.max(1.0));
    for (rect, hint) in [(prev, "LB"), (next, "RB")] {
        let rad = button_h * 0.5;
        round_rect(pixmap, rect.0 * scale, rect.1 * scale, (rect.2 - rect.0) * scale, (rect.3 - rect.1) * scale, rad * scale, pal.key_action, fade_u8(1.0));
        stroke_rect(pixmap, (rect.0 + 0.5) * scale, (rect.1 + 0.5) * scale, (rect.2 - rect.0 - 1.0) * scale, (rect.3 - rect.1 - 1.0) * scale, (rad - 0.5).max(0.0) * scale, pal.border, fade_u8(1.0), scale.max(1.0));
        if let Some(svg) = hint_svg(ps5, hint) {
            let px = 30.0 * unit;
            let cx = (rect.0 + rect.2) * 0.5;
            let half = px * 0.5;
            blit_svg(pixmap, svg, cx - half, cy - half, cx + half, cy + half, px, scale, None, fade_u8(1.0));
        }
    }
    let inner_l = chips.0 + spec.chips_pad * unit;
    let inner_r = chips.2 - spec.chips_pad * unit;
    let filled: Vec<usize> = strip.visible.iter().enumerate().filter(|(_, w)| !w.is_empty()).map(|(i, _)| i).collect();
    let n = filled.len().max(1) as f32;
    let chip_gap = spec.chips_gap * unit;
    let chip_w = ((inner_r - inner_l) - chip_gap * (n - 1.0)) / n;
    for (k, slot) in filled.into_iter().enumerate() {
        let x0 = inner_l + k as f32 * (chip_w + chip_gap);
        let word = &strip.visible[slot];
        let selected = slot == strip.highlight_slot;
        if selected {
            round_rect(pixmap, x0 * scale, inner_t * scale, chip_w * scale, (inner_b - inner_t) * scale, chip_r * scale, pal.chip_sel, fade_u8(1.0));
        }
        let alpha = fade_u8(if selected { 1.0 } else { spec.chip_text_alpha });
        let weight = if selected { spec.chip_sel_weight } else { Weight::NORMAL };
        let label = (x0 + chip_r * 0.5, inner_t, x0 + chip_w - chip_r * 0.5, inner_b);
        text(pixmap, fonts, cache, word, label, spec.chip_px * unit * scale, weight, pal.text, alpha, scale);
    }
}

fn palette(style: VkStyle, theme: &KeyboardTheme) -> Pal {
    let mut pal = match style {
        VkStyle::Normal => Pal {
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
        VkStyle::Mono => Pal {
            bg: rgb(0x1C1C1E),
            key: rgb(0x3A3A3C),
            key_action: rgb(0x2C2C2E),
            accent: rgb(0xFFFFFF),
            sel_ring: rgb(0xFFFFFF),
            text: rgb(0xFFFFFF),
            text_dim: rgb(0xEBEBF5),
            sel_text: rgb(0x000000),
            border: rgb(0x545458),
            panel_stroke: rgb(0xFFFFFF),
            panel_stroke_alpha: 0x14 as f32 / 255.0,
            chip_sel: rgb(0x3A3A3C),
        },
    };
    if let Some(v) = theme.bg {
        pal.bg = from_colorref(v);
    }
    if let Some(v) = theme.key {
        pal.key = from_colorref(v);
        pal.key_action = from_colorref(v);
        pal.chip_sel = from_colorref(v);
    }
    if let Some(v) = theme.accent {
        pal.accent = from_colorref(v);
        pal.sel_ring = mix(rgb(0xFFFFFF), from_colorref(v), 0.5);
    }
    if let Some(v) = theme.text {
        pal.text = from_colorref(v);
        pal.text_dim = from_colorref(v);
    }
    if let Some(v) = theme.sel_text {
        pal.sel_text = from_colorref(v);
    }
    if let Some(v) = theme.border {
        pal.border = from_colorref(v);
        pal.panel_stroke = from_colorref(v);
        pal.panel_stroke_alpha = 1.0;
    }
    pal
}

fn rgb(v: u32) -> Rgb {
    Rgb { r: ((v >> 16) & 0xff) as u8, g: ((v >> 8) & 0xff) as u8, b: (v & 0xff) as u8 }
}

fn from_colorref(color: u32) -> Rgb {
    Rgb { r: (color & 0xff) as u8, g: ((color >> 8) & 0xff) as u8, b: ((color >> 16) & 0xff) as u8 }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

fn scaled_about(rect: &KeyRect, press: f32) -> (f32, f32, f32, f32) {
    let cx = (rect.left + rect.right) * 0.5;
    let cy = (rect.top + rect.bottom) * 0.5;
    let s = press.clamp(0.9, 1.0);
    (
        cx + (rect.left - cx) * s,
        cy + (rect.top - cy) * s,
        cx + (rect.right - cx) * s,
        cy + (rect.bottom - cy) * s,
    )
}

fn band_at(left: f32, top: f32, right: f32, cy: f32, h: f32) -> (f32, f32, f32, f32) {
    let y = top + cy;
    (left, y - h * 0.5, right, y + h * 0.5)
}

fn is_action(action: &KeyAction) -> bool {
    matches!(action, KeyAction::Vk(vk) if *vk == VK_BACK || *vk == VK_RETURN)
}

fn key_hint(action: &KeyAction) -> Option<&'static str> {
    match action {
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

fn key_icon(action: &KeyAction, shift: bool) -> Option<(&'static str, bool)> {
    match action {
        KeyAction::Vk(vk) if *vk == VK_BACK => Some((BACKSPACE, true)),
        KeyAction::Vk(vk) if *vk == VK_RETURN => Some((ENTER, true)),
        KeyAction::Vk(vk) if *vk == VK_LEFT => Some((CHEV_L, false)),
        KeyAction::Vk(vk) if *vk == VK_RIGHT => Some((CHEV_R, false)),
        KeyAction::Vk(vk) if *vk == VK_UP => Some((CHEV_U, false)),
        KeyAction::Vk(vk) if *vk == VK_DOWN => Some((CHEV_D, false)),
        KeyAction::PredictPrev => Some((CHEV_L, false)),
        KeyAction::PredictNext => Some((CHEV_R, false)),
        KeyAction::Shift => Some((if shift { CAPS_FILL } else { SHIFT }, false)),
        _ => None,
    }
}

fn hint_svg(ps5: bool, hint: &str) -> Option<&'static str> {
    Some(match (ps5, hint) {
        (true, "A") => include_str!("../controller-icons/p5_face_cross_colored.svg"),
        (true, "B") => include_str!("../controller-icons/p5_face_circle_colored.svg"),
        (true, "X") => include_str!("../controller-icons/p5_face_square_colored.svg"),
        (true, "Y") => include_str!("../controller-icons/p5_face_triangle_colored.svg"),
        (true, "L3") => include_str!("../controller-icons/p5_l3_click.svg"),
        (true, "SELECT") => include_str!("../controller-icons/p5_share.svg"),
        (true, "LB") => include_str!("../controller-icons/p5_shoulder_l1.svg"),
        (true, "RB") => include_str!("../controller-icons/p5_shoulder_r1.svg"),
        (true, "LT") => include_str!("../controller-icons/p5_trigger_l2.svg"),
        (true, "RT") => include_str!("../controller-icons/p5_trigger_r2.svg"),
        (true, "R3") => include_str!("../controller-icons/p5_r3_click.svg"),
        (true, "START") => include_str!("../controller-icons/p5_options.svg"),
        (false, "A") => include_str!("../controller-icons/x_face_a_colored.svg"),
        (false, "B") => include_str!("../controller-icons/x_face_b_colored.svg"),
        (false, "X") => include_str!("../controller-icons/x_face_x_colored.svg"),
        (false, "Y") => include_str!("../controller-icons/x_face_y_colored.svg"),
        (false, "L3") => include_str!("../controller-icons/x_l3_click.svg"),
        (false, "SELECT") => include_str!("../controller-icons/x_menu_view.svg"),
        (false, "LB") => include_str!("../controller-icons/x_shoulder_lb.svg"),
        (false, "RB") => include_str!("../controller-icons/x_shoulder_rb.svg"),
        (false, "LT") => include_str!("../controller-icons/x_trigger_lt.svg"),
        (false, "RT") => include_str!("../controller-icons/x_trigger_rt.svg"),
        (false, "R3") => include_str!("../controller-icons/x_r3_click.svg"),
        (false, "START") => include_str!("../controller-icons/x_menu_menu.svg"),
        _ => return None,
    })
}

const SHIFT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 16a1 1 0 0 0 1-1v-2a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-6.939-6.939a1.207 1.207 0 0 0-1.708 0l-6.94 6.94a.707.707 0 0 0 .5 1.206H8a1 1 0 0 1 1 1v2a1 1 0 0 0 1 1z"/><path d="M9 20h6"/></svg>"#;
const CAPS_FILL: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 19a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1v-6a1 1 0 0 1 1-1h3.293a.707.707 0 0 0 .5-1.207l-7.086-7.086a1 1 0 0 0-1.414 0l-7.086 7.086a.707.707 0 0 0 .5 1.207H8a1 1 0 0 1 1 1z"/></svg>"#;
const BACKSPACE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 5a2 2 0 0 0-1.344.519l-6.328 5.74a1 1 0 0 0 0 1.481l6.328 5.741A2 2 0 0 0 10 19h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2z"/><path d="m12 9 6 6"/><path d="m18 9-6 6"/></svg>"#;
const ENTER: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 4v7a4 4 0 0 1-4 4H4"/><path d="m9 10-5 5 5 5"/></svg>"#;
const MIC: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19v3"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><rect x="9" y="2" width="6" height="13" rx="3"/></svg>"#;
const SPACE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 17v1c0 .5-.5 1-1 1H3c-.5 0-1-.5-1-1v-1"/></svg>"#;
const CHEV_L: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 18-6-6 6-6"/></svg>"#;
const CHEV_R: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>"#;
const CHEV_U: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m18 15-6-6-6 6"/></svg>"#;
const CHEV_D: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>"#;

fn fill(pixmap: &mut Pixmap, color: Rgb, alpha: u8) {
    if let Some(rect) = Rect::from_xywh(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, alpha);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn round_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb, alpha: u8) {
    let Some(path) = rounded(x, y, w, h, radius) else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, alpha);
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb, alpha: u8, width: f32) {
    let Some(path) = rounded(x, y, w, h, radius) else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, alpha);
    let stroke = Stroke { width: width.max(1.0), ..Stroke::default() };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    if w < 1.0 || h < 1.0 {
        return None;
    }
    let radius = radius.min(w * 0.5).min(h * 0.5).max(0.0);
    let k = radius * 0.55228475;
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(x + w - radius, y);
    path.cubic_to(x + w - radius + k, y, x + w, y + radius - k, x + w, y + radius);
    path.line_to(x + w, y + h - radius);
    path.cubic_to(x + w, y + h - radius + k, x + w - radius + k, y + h, x + w - radius, y + h);
    path.line_to(x + radius, y + h);
    path.cubic_to(x + radius - k, y + h, x, y + h - radius + k, x, y + h - radius);
    path.line_to(x, y + radius);
    path.cubic_to(x, y + radius - k, x + radius - k, y, x + radius, y);
    path.close();
    path.finish()
}

fn blur_shadow(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, dy: f32, blur: f32, alpha: f32) {
    let steps = 6.0;
    let per = 1.0 - (1.0 - alpha.clamp(0.0, 1.0)).powf(1.0 / steps);
    for i in 0..6 {
        let t = i as f32 / (steps - 1.0);
        let spread = blur * (-0.25 + 0.75 * t);
        round_rect(
            pixmap,
            x - spread,
            y - spread + dy,
            w + spread * 2.0,
            h + spread * 2.0,
            (radius + spread).max(0.0),
            Rgb { r: 0, g: 0, b: 0 },
            (per * 255.0) as u8,
        );
    }
}

fn clip_round(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let data = pixmap.data_mut();
    let r = radius.max(0.0);
    for py in 0..height {
        let (start, end) = row_inside(py, x, y, w, h, r, width);
        clear_span(data, width, py, 0, start);
        clear_span(data, width, py, end, width);
    }
}

fn row_inside(py: i32, x: f32, y: f32, w: f32, h: f32, r: f32, width: i32) -> (i32, i32) {
    let sample = py as f32 + 0.5;
    if sample < y || sample > y + h {
        return (0, 0);
    }
    let mut inset = 0.0;
    if sample < y + r {
        let dy = y + r - sample;
        let inner = (r * r - dy * dy).max(0.0).sqrt();
        inset = r - inner;
    } else if sample > y + h - r {
        let dy = sample - (y + h - r);
        let inner = (r * r - dy * dy).max(0.0).sqrt();
        inset = r - inner;
    }
    let start = (x + inset).ceil() as i32;
    let end = (x + w - inset).floor() as i32;
    (start.clamp(0, width), end.clamp(0, width))
}

fn clear_span(data: &mut [u8], width: i32, py: i32, from: i32, to: i32) {
    if to <= from || py < 0 {
        return;
    }
    let row = (py as u32 * width as u32) as usize * 4;
    let start = row + from.max(0) as usize * 4;
    let end = row + to.max(from) as usize * 4;
    if end <= data.len() {
        data[start..end].fill(0);
    }
}

fn text(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    value: &str,
    band: (f32, f32, f32, f32),
    px: f32,
    weight: Weight,
    color: Rgb,
    alpha: u8,
    scale: f32,
) {
    text_aligned(pixmap, fonts, cache, value, band, px, weight, color, alpha, scale, Align::Center);
}

fn measure_label(fonts: &mut FontSystem, value: &str, px: f32) -> f32 {
    if value.is_empty() || px < 1.0 {
        return 0.0;
    }
    let metrics = Metrics::new(px, px * 1.2);
    let mut buffer = Buffer::new(fonts, metrics);
    buffer.set_size(fonts, None, None);
    buffer.set_wrap(fonts, Wrap::None);
    let attrs = Attrs::new().family(Family::Name("Selawik")).weight(Weight::NORMAL);
    buffer.set_text(fonts, value, &attrs, Shaping::Advanced);
    buffer.shape_until_scroll(fonts, false);
    buffer.layout_runs().map(|run| run.line_w).fold(0.0, f32::max)
}

fn text_aligned(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    value: &str,
    band: (f32, f32, f32, f32),
    px: f32,
    weight: Weight,
    color: Rgb,
    alpha: u8,
    scale: f32,
    align: Align,
) {
    if value.is_empty() || px < 4.0 {
        return;
    }
    let (x, y, r, b) = (band.0 * scale, band.1 * scale, band.2 * scale, band.3 * scale);
    let w = r - x;
    let h = b - y;
    if w < 2.0 || h < 2.0 {
        return;
    }
    let metrics = Metrics::new(px, px * 1.2);
    let mut buffer = Buffer::new(fonts, metrics);
    buffer.set_size(fonts, Some(w), Some(h));
    buffer.set_wrap(fonts, Wrap::None);
    let attrs = Attrs::new().family(Family::Name("Selawik")).weight(weight);
    buffer.set_text(fonts, value, &attrs, Shaping::Advanced);
    for line in &mut buffer.lines {
        line.set_align(Some(align));
    }
    buffer.shape_until_scroll(fonts, false);
    let mut block_top = 0.0;
    let mut block_bottom = px * 1.2;
    let mut seen = false;
    for run in buffer.layout_runs() {
        if !seen {
            block_top = run.line_top;
            seen = true;
        }
        block_bottom = run.line_top + run.line_height;
    }
    let block = (block_bottom - block_top).max(1.0);
    let dy = ((h - block) * 0.5 - block_top).round() as i32;
    let paint = Color::rgba(color.r, color.g, color.b, alpha);
    let clip = (x as i32, y as i32, r as i32, b as i32);
    buffer.draw(fonts, cache, paint, |gx, gy, gw, gh, pixel| {
        let px0 = x as i32 + gx;
        let py0 = y as i32 + dy + gy;
        if px0 + gw as i32 <= clip.0 || py0 + gh as i32 <= clip.1 || px0 >= clip.2 || py0 >= clip.3 {
            return;
        }
        blend_straight(pixmap, px0, py0, pixel);
    });
}

fn blend_straight(pixmap: &mut Pixmap, x: i32, y: i32, pixel: Color) {
    if pixel.a() == 0 {
        return;
    }
    let src_a = pixel.a() as f32 / 255.0;
    blend_premul(
        pixmap,
        x,
        y,
        (pixel.r() as f32 * src_a).round() as u8,
        (pixel.g() as f32 * src_a).round() as u8,
        (pixel.b() as f32 * src_a).round() as u8,
        pixel.a(),
    );
}

fn blend_premul(pixmap: &mut Pixmap, x: i32, y: i32, pr: u8, pg: u8, pb: u8, pa: u8) {
    if x < 0 || y < 0 || pa == 0 {
        return;
    }
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    if x >= pw || y >= ph {
        return;
    }
    let width = pixmap.width();
    let data = pixmap.data_mut();
    let index = ((y as u32 * width) + x as u32) as usize * 4;
    let src_a = pa as f32 / 255.0;
    let inv = 1.0 - src_a;
    data[index] = (pr as f32 + data[index] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    data[index + 1] = (pg as f32 + data[index + 1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    data[index + 2] = (pb as f32 + data[index + 2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    data[index + 3] = (pa as f32 + data[index + 3] as f32 * inv).round().clamp(0.0, 255.0) as u8;
}

fn blit_svg(pixmap: &mut Pixmap, svg: &str, x0: f32, y0: f32, x1: f32, y1: f32, size: f32, scale: f32, tint: Option<Rgb>, alpha: u8) {
    if alpha == 0 {
        return;
    }
    let px = (size * scale).round().max(8.0) as u32;
    let Some(bytes) = cached_svg(svg, px, tint) else { return };
    let cx = (x0 + x1) * 0.5 * scale;
    let cy = (y0 + y1) * 0.5 * scale;
    let dest_x = (cx - px as f32 * 0.5).round() as i32;
    let dest_y = (cy - px as f32 * 0.5).round() as i32;
    let fade = alpha as f32 / 255.0;
    for row in 0..px {
        for col in 0..px {
            let index = ((row * px + col) * 4) as usize;
            blend_premul(
                pixmap,
                dest_x + col as i32,
                dest_y + row as i32,
                (bytes[index] as f32 * fade).round() as u8,
                (bytes[index + 1] as f32 * fade).round() as u8,
                (bytes[index + 2] as f32 * fade).round() as u8,
                (bytes[index + 3] as f32 * fade).round() as u8,
            );
        }
    }
}

fn cached_svg(svg: &str, px: u32, tint: Option<Rgb>) -> Option<std::sync::Arc<Vec<u8>>> {
    use std::sync::{Mutex, OnceLock};
    type Key = (usize, usize, u32, u32);
    static CACHE: OnceLock<Mutex<std::collections::HashMap<Key, std::sync::Arc<Vec<u8>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let tint_key = tint.map(|c| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32).unwrap_or(0);
    let key = (svg.as_ptr() as usize, svg.len(), px, tint_key);
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return Some(hit.clone());
        }
    }
    let factor = if tint.is_none() { 3u32 } else { 1u32 };
    let colored = if let Some(c) = tint {
        svg.replace("currentColor", &format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b))
    } else {
        svg.to_string()
    };
    let tree = resvg::usvg::Tree::from_data(colored.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let raster = px.saturating_mul(factor);
    let mut icon = resvg::tiny_skia::Pixmap::new(raster, raster)?;
    let tw = tree.size().width().max(1.0);
    let th = tree.size().height().max(1.0);
    let s = raster as f32 / tw.max(th);
    let dx = (raster as f32 - tw * s) * 0.5;
    let dy = (raster as f32 - th * s) * 0.5;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_row(s, 0.0, 0.0, s, dx, dy),
        &mut icon.as_mut(),
    );
    let src = icon.data();
    let area = (factor * factor) as f32;
    let mut bytes = vec![0u8; (px * px * 4) as usize];
    for row in 0..px {
        for col in 0..px {
            let mut acc = [0.0f32; 4];
            for sy in 0..factor {
                for sx in 0..factor {
                    let index = (((row * factor + sy) * raster + col * factor + sx) * 4) as usize;
                    acc[0] += src[index] as f32;
                    acc[1] += src[index + 1] as f32;
                    acc[2] += src[index + 2] as f32;
                    acc[3] += src[index + 3] as f32;
                }
            }
            let out = ((row * px + col) * 4) as usize;
            bytes[out] = (acc[0] / area).round() as u8;
            bytes[out + 1] = (acc[1] / area).round() as u8;
            bytes[out + 2] = (acc[2] / area).round() as u8;
            bytes[out + 3] = (acc[3] / area).round() as u8;
        }
    }
    let bytes = std::sync::Arc::new(bytes);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, bytes.clone());
    }
    Some(bytes)
}

pub struct KeyPatch {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub dest_x: f32,
    pub dest_y: f32,
    pub body_x: f32,
    pub body_y: f32,
    pub body_w: f32,
    pub body_h: f32,
    pub radius: f32,
    pub ring: [f32; 4],
    pub ring_w: f32,
}

pub fn raster_unselected(
    frame: &Frame,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    glide: &mut Glide,
) -> Option<Pixmap> {
    render_keyboard(frame, fonts, cache, glide, false)
}

pub fn voice_spot(frame: &Frame) -> Option<(f32, f32, f32)> {
    let (_spec, _pal, rects, _unit, scale, _radius, _key_h) = scene(frame);
    let rect = rects.iter().find(|rect| {
        frame
            .rows
            .get(rect.pos.row)
            .and_then(|row| row.keys.get(rect.pos.col))
            .is_some_and(|key| matches!(key.action, KeyAction::VoiceInput))
    })?;
    let side = (rect.right - rect.left).min(rect.bottom - rect.top) * 0.86 * scale;
    let cx = (rect.left + rect.right) * 0.5 * scale;
    let cy = (rect.top + rect.bottom) * 0.5 * scale;
    Some((cx - side * 0.5, cy - side * 0.5, side))
}

pub fn raster_selected_key(
    frame: &Frame,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    press: f32,
) -> Option<KeyPatch> {
    let (spec, pal, rects, unit, scale, radius, key_h) = scene(frame);
    let rect = rects.iter().find(|rect| rect.pos == frame.sel)?;
    let pad = 8.0;
    let local = KeyRect {
        pos: rect.pos,
        left: pad,
        top: pad,
        right: pad + (rect.right - rect.left),
        bottom: pad + (rect.bottom - rect.top),
    };
    let width = (local.right * scale).ceil().max(1.0) as u32;
    let height = (local.bottom * scale).ceil().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    paint_key(
        &mut pixmap,
        fonts,
        cache,
        frame,
        &spec,
        &pal,
        &local,
        unit,
        key_h,
        scale,
        radius,
        true,
        press,
        true,
    );
    Some(KeyPatch {
        pixels: pixmap.data().to_vec(),
        width,
        height,
        dest_x: (rect.left - pad) * scale,
        dest_y: (rect.top - pad) * scale,
        body_x: rect.left * scale,
        body_y: rect.top * scale,
        body_w: (rect.right - rect.left) * scale,
        body_h: (rect.bottom - rect.top) * scale,
        radius: radius * scale,
        ring: [pal.sel_ring.r as f32 / 255.0, pal.sel_ring.g as f32 / 255.0, pal.sel_ring.b as f32 / 255.0, 1.0],
        ring_w: (spec.sel_ring_w * unit * scale).max(1.0),
    })
}

pub fn paint_voice_key(pixmap: &mut Pixmap, frame: &Frame, orb: &[u8], orb_px: u32) {
    let (_spec, pal, rects, _unit, scale, _radius, _key_h) = scene(frame);
    let Some(rect) = rects.iter().find(|rect| {
        frame
            .rows
            .get(rect.pos.row)
            .and_then(|row| row.keys.get(rect.pos.col))
            .is_some_and(|key| matches!(key.action, KeyAction::VoiceInput))
    }) else {
        return;
    };
    let side = (rect.right - rect.left).min(rect.bottom - rect.top) * 0.86;
    let cx = (rect.left + rect.right) * 0.5;
    let cy = (rect.top + rect.bottom) * 0.5;
    blit_premul(
        pixmap,
        orb,
        orb_px,
        (cx - side * 0.5) * scale,
        (cy - side * 0.5) * scale,
        side * scale,
    );
    stroke_rect(
        pixmap,
        rect.left * scale,
        rect.top * scale,
        (rect.right - rect.left) * scale,
        (rect.bottom - rect.top) * scale,
        ((rect.bottom - rect.top) * 0.16) * scale,
        pal.accent,
        180,
        (2.0 * scale).max(1.0),
    );
}

pub struct VoicePainter {
    fonts: FontSystem,
    cache: SwashCache,
    label: String,
    hint: String,
    base: Option<Pixmap>,
}

impl VoicePainter {
    pub fn new() -> Self {
        Self {
            fonts: font_system(),
            cache: SwashCache::new(),
            label: String::new(),
            hint: String::new(),
            base: None,
        }
    }

    pub fn paint(&mut self, label: &str, hint: &str, orb: Option<&[u8]>, orb_px: u32) -> Option<Pixmap> {
        if self.base.is_none() || self.label != label || self.hint != hint {
            self.base = draw_voice_pill_with(label, hint, None, 0, &mut self.fonts, &mut self.cache);
            self.label = label.to_string();
            self.hint = hint.to_string();
        }
        let mut frame = clone_pixmap(self.base.as_ref()?)?;
        if let Some(orb) = orb {
            blit_premul(&mut frame, orb, orb_px, 16.0, 16.0, 72.0);
        }
        Some(frame)
    }
}

pub fn draw_voice_pill(label: &str, hint: &str, orb: Option<&[u8]>, orb_px: u32) -> Option<Pixmap> {
    let mut fonts = font_system();
    let mut cache = SwashCache::new();
    draw_voice_pill_with(label, hint, orb, orb_px, &mut fonts, &mut cache)
}

fn draw_voice_pill_with(
    label: &str,
    hint: &str,
    orb: Option<&[u8]>,
    orb_px: u32,
    mut fonts: &mut FontSystem,
    mut cache: &mut SwashCache,
) -> Option<Pixmap> {
    let scale = 1.0f32;
    let mut pixmap = Pixmap::new(420, 104)?;
    round_rect(&mut pixmap, 0.0, 0.0, 420.0, 104.0, 28.0, rgb(0x141414), 242);
    if let Some(orb) = orb {
        blit_premul(&mut pixmap, orb, orb_px, 16.0, 16.0, 72.0);
    }
    text_aligned(
        &mut pixmap,
        &mut fonts,
        &mut cache,
        label,
        (104.0, 18.0, 400.0, 58.0),
        26.0 * scale,
        Weight::NORMAL,
        rgb(0xFFFFFF),
        255,
        scale,
        Align::Left,
    );
    text_aligned(
        &mut pixmap,
        &mut fonts,
        &mut cache,
        hint,
        (104.0, 54.0, 400.0, 90.0),
        16.0 * scale,
        Weight::NORMAL,
        rgb(0xFFFFFF),
        0xA6,
        scale,
        Align::Left,
    );
    Some(pixmap)
}

fn blit_premul(pixmap: &mut Pixmap, src: &[u8], src_px: u32, x: f32, y: f32, size: f32) {
    if src_px == 0 || size < 1.0 {
        return;
    }
    let width = pixmap.width();
    let height = pixmap.height();
    let dest = pixmap.data_mut();
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let side = size.ceil() as i32;
    for row in 0..side {
        let dy = y0 + row;
        if dy < 0 || dy >= height as i32 {
            continue;
        }
        let sy = ((row as f32 / size) * src_px as f32) as u32;
        if sy >= src_px {
            continue;
        }
        for col in 0..side {
            let dx = x0 + col;
            if dx < 0 || dx >= width as i32 {
                continue;
            }
            let sx = ((col as f32 / size) * src_px as f32) as u32;
            if sx >= src_px {
                continue;
            }
            let si = ((sy * src_px + sx) * 4) as usize;
            if si + 3 >= src.len() {
                continue;
            }
            let sa = src[si + 3] as u32;
            if sa == 0 {
                continue;
            }
            let di = ((dy as u32 * width + dx as u32) * 4) as usize;
            let inv = 255 - sa;
            for channel in 0..3 {
                let blended = src[si + channel] as u32 + dest[di + channel] as u32 * inv / 255;
                dest[di + channel] = blended.min(255) as u8;
            }
            dest[di + 3] = (sa + dest[di + 3] as u32 * inv / 255).min(255) as u8;
        }
    }
}

pub fn to_argb(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.data().chunks_exact(4) {
        out.push(px[2]);
        out.push(px[1]);
        out.push(px[0]);
        out.push(px[3]);
    }
    out
}

pub fn draw_prompt(text_value: &str, scale: f32, fonts: &mut FontSystem, cache: &mut SwashCache) -> Option<Pixmap> {
    let scale = scale.max(1.0);
    let logical_w = 520.0;
    let logical_h = 72.0;
    let mut pixmap = Pixmap::new((logical_w * scale) as u32, (logical_h * scale) as u32)?;
    fill(&mut pixmap, rgb(0x15161C), 235);
    text(&mut pixmap, fonts, cache, text_value, (16.0, 16.0, logical_w - 16.0, logical_h - 16.0), 22.0 * scale, Weight::MEDIUM, rgb(0xFFFFFF), 255, 1.0);
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::{draw, font_system, Frame, Glide};
    use crate::config::{KeyboardTheme, VkStyle};
    use crate::vk_nav::{rows_for_test, KeyPos};

    #[test]
    fn keyboard_frame_uses_the_windows_palette() {
        let mut fonts = font_system();
        let mut cache = cosmic_text::SwashCache::new();
        let mut frame = Frame {
            logical_w: 1200.0,
            logical_h: 460.0,
            scale: 1.0,
            scale_w: 1920.0 * 1.25,
            rows: rows_for_test(),
            sel: KeyPos { row: 1, col: 1 },
            style: VkStyle::Normal,
            theme: KeyboardTheme::default(),
            strip: None,
            pressed: None,
            ps5: true,
            shift: true,
            caps: false,
            symbol: false,
            symbol_locked: false,
            floating: true,
            sheet: false,
        };
        let mut glide = Glide::default();
        let started = std::time::Instant::now();
        let pixmap = draw(&frame, &mut fonts, &mut cache, &mut glide).expect("pixmap");
        let cold = started.elapsed();
        assert!(pixmap.width() > 1000);
        assert!(pixmap.data().iter().any(|byte| *byte != 0));
        frame.sel = KeyPos { row: 0, col: 0 };
        let started = std::time::Instant::now();
        let moved = draw(&frame, &mut fonts, &mut cache, &mut glide).expect("moved");
        let warm = started.elapsed();
        let mut same = 0usize;
        let mut total = 0usize;
        for (left, right) in pixmap.data().chunks_exact(4).zip(moved.data().chunks_exact(4)) {
            total += 1;
            if left == right {
                same += 1;
            }
        }
        assert!(same != total);
        assert!(same * 10 > total * 8, "selection repaint changed {same}/{total}");
        if cold > std::time::Duration::from_millis(30) {
            assert!(warm * 3 < cold, "cold {cold:?} warm {warm:?}");
        }
        let mut sheet = frame;
        sheet.sheet = true;
        sheet.logical_w = 1600.0;
        sheet.logical_h = 546.0;
        let covered = draw(&sheet, &mut fonts, &mut cache, &mut Glide::default()).expect("sheet");
        assert_eq!((covered.width(), covered.height()), (1600, 546));
        let inset = (12 * 1600 + 80) * 4;
        assert_eq!(&covered.data()[inset..inset + 4], &[0x1F, 0x1F, 0x1F, 0xFF]);
    }

    #[test]
    fn shortcut_overlay_matches_the_pen_frame() {
        let (w, h) = super::sheet_size(3072.0, 1728.0);
        assert_eq!((w, h), (1600, 546));
        let mut fonts = font_system();
        let (total, combos) = super::sheet_layout(&mut fonts);
        for (got, want) in combos.iter().zip([86.0, 183.0, 140.0]) {
            assert!((got - want).abs() <= 16.0, "combos {combos:?} total {total}");
        }
        assert!(total < 1600.0 - 96.0, "content {total} leaves the pen's side inset");
        let mut cache = cosmic_text::SwashCache::new();
        let frame = Frame {
            logical_w: 1600.0,
            logical_h: 546.0,
            scale: 1.0,
            scale_w: 1920.0,
            rows: rows_for_test(),
            sel: KeyPos { row: 0, col: 0 },
            style: VkStyle::Normal,
            theme: KeyboardTheme::default(),
            strip: None,
            pressed: None,
            ps5: true,
            shift: false,
            caps: false,
            symbol: false,
            symbol_locked: false,
            floating: true,
            sheet: true,
        };
        let pixmap = draw(&frame, &mut fonts, &mut cache, &mut Glide::default()).expect("overlay");
        assert!(pixmap.data().chunks_exact(4).any(|px| px[0] != 0x1F || px[1] != 0x1F || px[2] != 0x1F));
        let mut docked = frame;
        docked.sheet = false;
        docked.logical_w = 3072.0;
        docked.logical_h = 577.0;
        docked.scale = 1.25;
        docked.floating = false;
        docked.style = VkStyle::Mono;
        docked.sel = KeyPos { row: 1, col: 1 };
        let mut glide = Glide::default();
        let cold_at = std::time::Instant::now();
        let _ = draw(&docked, &mut fonts, &mut cache, &mut glide).expect("docked");
        let cold = cold_at.elapsed();
        docked.sel = KeyPos { row: 1, col: 2 };
        let warm_at = std::time::Instant::now();
        let _ = draw(&docked, &mut fonts, &mut cache, &mut glide).expect("docked move");
        let warm = warm_at.elapsed();
        assert!(warm < std::time::Duration::from_millis(120), "docked highlight cold {cold:?} warm {warm:?}");
    }
}

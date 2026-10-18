use std::time::Instant;

use cosmic_text::Weight;
use crate::mono_ui::Align;
use tiny_skia::Pixmap;

use crate::config::{KeyboardTheme, VkStyle};
use crate::mono_ui::Gfx;
use crate::vk_paint::{VkLook, VkPalette};
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

type Spec = crate::vk_paint::StyleSpec;

fn spec_for(look: VkLook) -> &'static Spec {
    crate::vk_paint::style_spec(look)
}

fn weight(value: i32) -> Weight {
    Weight(value.clamp(1, 1000) as u16)
}

fn is_columns(spec: &Spec) -> bool {
    spec.strip == crate::vk_paint::StripLook::Columns
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
    pub tv: bool,
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

impl Frame {
    pub fn look(&self) -> VkLook {
        VkLook {
            style: self.style,
            tv: self.tv,
        }
    }
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

pub fn ui_scale() -> f32 {
    crate::config::vk_bar_scale() / crate::config::COMPACT_BAR_SCALE
}

pub fn floating_bottom_margin(output_h: f32) -> i32 {
    ((output_h.max(0.0) * 0.04).round() as i32).clamp(28, 80)
}

pub fn layout_scale(style: VkStyle) -> f32 {
    let key_w = match style {
        VkStyle::Normal | VkStyle::Modern => 138.0,
        VkStyle::Mono => 137.0,
    };
    key_w / REF_KEY_W
}

pub fn layout_scale_w(style: VkStyle) -> f32 {
    REF_MON_W * layout_scale(style)
}

pub fn card_size(output_w: f32, output_h: f32, docked: bool, rows: &[KeyRow], look: VkLook) -> (u32, u32) {
    let spec = spec_for(look);
    let scale = layout_scale(look.style);
    let scale_w = REF_MON_W * scale;
    let (grid_w, block_h) = vk_geom::grid_size(scale_w, rows, spec.key_aspect, spec.gap, spec.design_kh);
    let chrome = crate::vk_paint::strip_band_height(scale, look);
    let legend = legend_band_height(scale, look);
    let (pad_x, pad_y) = crate::vk_paint::floating_pad(scale, look);
    let full = if docked && look.tv_layout() {
        let (_, kh, _) =
            vk_geom::key_metrics(scale_w, f32::INFINITY, rows, 0.0, spec.key_aspect, spec.gap, spec.design_kh);
        chrome + block_h + kh * 0.25
    } else {
        chrome + block_h + pad_y * 2.0 + legend
    };
    let height = full.ceil().clamp(160.0, output_h.max(160.0));
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

fn legend_band_height(scale: f32, look: VkLook) -> f32 {
    if look.tv_layout() {
        return 0.0;
    }
    ((BAR_H - spec_for(look).pad_y) * crate::vk_paint::ref_unit(scale, look)).max(0.0)
}

pub fn frame_px(frame: &Frame) -> (u32, u32) {
    let scale = frame.scale.max(1.0);
    (
        (frame.logical_w * scale).round().max(1.0) as u32,
        (frame.logical_h * scale).round().max(1.0) as u32,
    )
}

pub fn paint_frame(g: &mut Gfx, frame: &Frame, glide: &mut Glide) {
    let _ = sheet_metrics_are_the_windows_ones();
    let (width, height) = frame_px(frame);
    g.begin(width, height);
    if frame.sheet {
        glide.base = None;
        glide.animating = false;
        draw_sheet_frame(g, frame);
        return;
    }
    let now = Instant::now();
    if strip_blocking(glide, frame, now) {
        glide.base = None;
        render_keyboard(g, frame, glide, true);
        return;
    }
    let stamp = keyboard_stamp(frame);
    let cached = glide
        .base
        .as_ref()
        .filter(|(s, base)| *s == stamp && base.data().len() == g.pixels().len());
    if let Some((_, base)) = cached {
        g.canvas().0.data_mut().copy_from_slice(base.data());
        paint_live(g, frame, glide, true);
        return;
    }
    render_keyboard(g, frame, glide, false);
    if let Some(copy) = clone_pixmap(g.canvas().0) {
        glide.base = Some((stamp, copy));
    }
    paint_live(g, frame, glide, false);
}

fn render_keyboard(
    g: &mut Gfx,
    frame: &Frame,
    glide: &mut Glide,
    highlight: bool,
) {
    let scale = frame.scale.max(1.0);
    let look = frame.look();
    let spec = spec_for(look);
    let pal = palette(look, &frame.theme);
    let lu = crate::vk_paint::ref_unit(ui_scale(), look);
    let top_inset = crate::vk_paint::strip_band_height(ui_scale(), look);
    let legend_h = legend_band_height(ui_scale(), look);
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
        round_rect(g, x, y, w, h, panel_r * scale, pal.bg, 255);
        stroke_rect(
            g,
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
        fill(g, pal.bg, 255);
        None
    };
    for rect in &rects {
        let selected = highlight && rect.pos == frame.sel;
        let press = if highlight {
            frame.pressed.filter(|(p, _)| *p == rect.pos).map(|(_, s)| s).unwrap_or(1.0)
        } else {
            1.0
        };
        paint_key(g, frame, &spec, &pal, rect, unit, key_h, scale, radius, selected, press, true);
    }
    if highlight {
        stroke_ring(g, glide, &spec, &pal, unit, scale, radius);
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
        draw_strip(g, &spec, &pal, strip, &rects, unit, scale, frame.ps5, strip_dy, strip_alpha);
    }
    if !look.tv_layout() {
        draw_legend(g, spec, &pal, &rects, frame.logical_w, frame.logical_h, lu, scale, frame.ps5);
    }
    if let Some((x, y, w, h, radius)) = panel {
        clip_round(g, x, y, w, h, radius);
    }
    let strip_moving = glide
        .strip_at
        .is_some_and(|t| t.elapsed().as_secs_f32() * 1000.0 < vk_motion::STRIP_ENTER_MS);
    let pressing = frame.pressed.is_some_and(|(_, s)| s < 0.999);
    glide.animating = strip_moving || pressing;
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

fn leg_part_width(part: &Leg, u: f32, g: &mut Gfx, family: &[&str]) -> f32 {
    match *part {
        Leg::Btn("SELECT") => BAR_SHARE * u,
        Leg::Btn(_) => BAR_GLYPH * u,
        Leg::Word(word) => measure_label(g, family, word, text_px(BAR_PREFIX_PX, u)),
        Leg::Plus => measure_label(g, family, "+", text_px(BAR_CONNECTOR_PX, u)),
        Leg::Slash => measure_label(g, family, "/", text_px(BAR_CONNECTOR_PX, u)),
    }
}

fn legend_item_width(parts: &[Leg], label: &str, u: f32, g: &mut Gfx, family: &[&str]) -> f32 {
    let gaps = BAR_GAP * u * parts.len().saturating_sub(1) as f32;
    let body: f32 = parts.iter().map(|part| leg_part_width(part, u, g, family)).sum();
    body + gaps + BAR_LABEL_GAP * u + measure_label(g, family, label, text_px(BAR_LABEL_PX, u))
}

fn draw_legend(
    g: &mut Gfx,
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
    let family = _spec.families;
    let dot_w = measure_label(g, family, LEGEND_SEP, text_px(BAR_SEP_PX, u));
    let sep_w = BAR_GAP * u + dot_w + BAR_GAP * u;
    let widths: Vec<f32> = LEGEND_ROWS
        .iter()
        .map(|(parts, label)| legend_item_width(parts, label, u, g, family))
        .collect();
    let kept = fit_legend(&widths, sep_w, (right - left).max(1.0));
    let total = kept.iter().map(|&i| widths[i]).sum::<f32>() + sep_w * kept.len().saturating_sub(1) as f32;
    let mut x = (left + right - total) * 0.5;
    let top = frame_h - BAR_H * u;
    let bottom = frame_h;
    let secondary = (ALPHA_SECONDARY * 255.0) as u8;
    let sep_alpha = (ALPHA_SEP * 255.0) as u8;
    let label_px = text_px(BAR_LABEL_PX, u);
    for (n, &index) in kept.iter().enumerate() {
        if n > 0 {
            let sep_left = x - sep_w + BAR_GAP * u;
            let sep_w_text = measure_label(g, family, LEGEND_SEP, text_px(BAR_SEP_PX, u));
            text_aligned(
                g,
                LEGEND_SEP,
                (sep_left, top, sep_left + sep_w_text, bottom),
                text_px(BAR_SEP_PX, u) * scale,
                Weight::NORMAL,
                pal.text,
                sep_alpha,
                scale,
                Align::Left,
                family,
            );
        }
        let (parts, label) = LEGEND_ROWS[index];
        let mut cursor = x;
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                cursor += BAR_GAP * u;
            }
            let w = leg_part_width(part, u, g, family);
            match *part {
                Leg::Btn(hint) => {
                    if let Some(svg) = hint_svg(ps5, hint) {
                        blit_svg(g, svg, cursor, top, cursor + w, bottom, w, scale, None, 255);
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
                        g,
                        shown,
                        (cursor, top, cursor + w, bottom),
                        (if matches!(*part, Leg::Word(_)) { text_px(BAR_PREFIX_PX, u) } else { text_px(BAR_CONNECTOR_PX, u) }) * scale,
                        Weight::NORMAL,
                        pal.text,
                        secondary,
                        scale,
                        Align::Left,
                        family,
                    );
                }
            }
            cursor += w;
        }
        let label_w = measure_label(g, family, label, label_px);
        let label_x = cursor + BAR_LABEL_GAP * u;
        text_aligned(
            g,
            label,
            (label_x, top, label_x + label_w, bottom),
            label_px * scale,
            Weight::NORMAL,
            pal.text,
            255,
            scale,
            Align::Left,
            family,
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
    g: &mut Gfx,
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
                blur_shadow(g, left * scale, top * scale, (right - left) * scale, (bottom - top) * scale, radius * scale, dy * unit * scale, blur * unit * scale, alpha);
            }
        } else if spec.key_shadow_dy > 0.0 {
            let dy = (spec.key_shadow_dy * unit).max(1.0);
            round_rect(g, left * scale, (top + dy) * scale, (right - left) * scale, (bottom - top) * scale, radius * scale, Rgb { r: 0, g: 0, b: 0 }, (spec.key_shadow_alpha * 255.0) as u8);
        }
    }
    round_rect(g, left * scale, top * scale, (right - left) * scale, (bottom - top) * scale, radius * scale, fill_c, 255);
    let locked = (matches!(key.action, KeyAction::Shift) && frame.caps) || (matches!(key.action, KeyAction::Symbols) && frame.symbol_locked);
    if locked {
        let pill_w = ((right - left) * 0.28).min(24.0 * unit);
        let pill_h = (3.0 * unit).max(2.0);
        let cx = (left + right) * 0.5;
        let yb = bottom - 5.0 * unit;
        round_rect(g, (cx - pill_w * 0.5) * scale, (yb - pill_h) * scale, pill_w * scale, pill_h * scale, pill_h * 0.5 * scale, if selected { pal.sel_text } else { pal.accent }, 255);
    }
    let is_space = matches!(key.action, KeyAction::Vk(vk) if vk == VK_SPACE);
    let dim = if selected { pal.sel_text } else { pal.text_dim };
    let dim_a = (spec.number_alpha * 255.0) as u8;
    if let Some(sub) = key.sublabel.as_deref().filter(|_| !is_space) {
        let band = band_at(left, top, right, spec.number_cy * unit, key_h);
        text(g, sub, band, spec.number_px * unit * scale, Weight::NORMAL, dim, dim_a, scale, spec.families);
    }
    if matches!(key.action, KeyAction::VoiceInput) {
        blit_svg(g, MIC, left, top, right, bottom, spec.icon_px * unit, scale, Some(label_c), 255);
    } else if is_space {
        if let Some(word) = spec.space_label {
            text(g, word, (left, top, right, bottom), spec.space_label_px * unit * scale, Weight::NORMAL, label_c, 255, scale, spec.families);
        } else if let Some(sub) = key.sublabel.as_deref() {
            let cap = band_at(left, top, right, spec.caption_cy * unit, key_h);
            text(g, sub, cap, spec.number_px * unit * scale, Weight::NORMAL, dim, dim_a, scale, spec.families);
            let icon = band_at(left, top, right, spec.space_icon_cy * unit, key_h);
            blit_svg(g, SPACE, icon.0, icon.1, icon.2, icon.3, spec.space_icon_px * unit, scale, Some(label_c), 255);
        }
    } else if let Some((svg, large)) = key_icon(&key.action, frame.caps) {
        let px = if large { spec.icon_large_px } else { spec.icon_px } * unit;
        blit_svg(g, svg, left, top, right, bottom, px, scale, Some(label_c), 255);
    } else {
        let mut glyph = key.label.clone();
        if spec.trim_symbols_label && matches!(key.action, KeyAction::Symbols) {
            glyph = glyph.trim_start_matches('?').to_string();
        }
        if !glyph.is_empty() {
            let weight = weight(if glyph.chars().count() > 1 { spec.word_weight } else { spec.label_weight });
            let px = if glyph.chars().count() > 1 { spec.word_px } else { spec.label_px };
            let band = if key.sublabel.is_some() {
                band_at(left, top, right, spec.label_cy * unit, key_h)
            } else {
                (left, top, right, bottom)
            };
            text(g, &glyph, band, px * unit * scale, weight, label_c, 255, scale, spec.families);
        }
    }
    if let Some(hint) = key_hint(&key.action) {
        let badge = spec.hint_badge * unit;
        let inset = spec.hint_inset * unit;
        let bx = left + inset;
        let by = top + inset;
        if let Some(svg) = hint_svg(frame.ps5, hint) {
            blit_svg(g, svg, bx, by, bx + badge, by + badge, badge, scale, None, 255);
        }
    }
}

fn stroke_ring(g: &mut Gfx, glide: &Glide, spec: &Spec, pal: &Pal, unit: f32, scale: f32, radius: f32) {
    let Some((l, t, r, b)) = glide.ring else { return };
    let ring_w = (spec.sel_ring_w * unit).max(1.0);
    stroke_rect(g, (l + ring_w * 0.5) * scale, (t + ring_w * 0.5) * scale, (r - l - ring_w) * scale, (b - t - ring_w) * scale, (radius - ring_w * 0.5).max(0.0) * scale, pal.sel_ring, 255, ring_w * scale);
}

fn erase_key(g: &mut Gfx, rect: &KeyRect, spec: &Spec, pal: &Pal, unit: f32, scale: f32) {
    let dy = (spec.key_shadow_dy * unit).max(0.0);
    round_rect(g, rect.left * scale, rect.top * scale, (rect.right - rect.left) * scale, (rect.bottom - rect.top + dy) * scale, 0.0, pal.bg, 255);
}

fn scene(frame: &Frame) -> (&'static Spec, Pal, Vec<KeyRect>, f32, f32, f32, f32) {
    let scale = frame.scale.max(1.0);
    let look = frame.look();
    let spec = spec_for(look);
    let pal = palette(look, &frame.theme);
    let top_inset = crate::vk_paint::strip_band_height(ui_scale(), look);
    let legend_h = legend_band_height(ui_scale(), look);
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

fn paint_live(g: &mut Gfx, frame: &Frame, glide: &mut Glide, step: bool) {
    let (spec, pal, rects, unit, scale, radius, key_h) = scene(frame);
    let target = rects.iter().find(|r| r.pos == frame.sel).map(|r| (r.left, r.top, r.right, r.bottom));
    if step {
        advance_glide(glide, frame, target, Instant::now());
    }
    if let Some(sel_index) = rects.iter().position(|r| r.pos == frame.sel) {
        erase_key(g, &rects[sel_index], &spec, &pal, unit, scale);
        let press = frame.pressed.filter(|(p, _)| *p == rects[sel_index].pos).map(|(_, s)| s).unwrap_or(1.0);
        paint_key(g, frame, &spec, &pal, &rects[sel_index], unit, key_h, scale, radius, true, press, true);
        if let Some((pos, press)) = frame.pressed {
            if pos != rects[sel_index].pos {
                if let Some(other) = rects.iter().find(|rect| rect.pos == pos) {
                    erase_key(g, other, &spec, &pal, unit, scale);
                    paint_key(g, frame, &spec, &pal, other, unit, key_h, scale, radius, false, press, true);
                }
            }
        }
    }
    stroke_ring(g, glide, &spec, &pal, unit, scale, radius);
    if frame.floating {
        let panel_r = spec.panel_radius * unit * scale;
        let x = FLOATING_PANEL_INSET * scale;
        let y = FLOATING_PANEL_INSET * scale;
        let w = (frame.logical_w - 2.0 * FLOATING_PANEL_INSET) * scale;
        let h = (frame.logical_h - 2.0 * FLOATING_PANEL_INSET) * scale;
        clip_round(g, x, y, w, h, panel_r);
    }
}

fn draw_sheet_frame(g: &mut Gfx, frame: &Frame) {
    let (spec, pal, _, unit, scale, _, _) = scene(frame);
    let panel = if frame.floating {
        let panel_r = spec.panel_radius * unit;
        let x = FLOATING_PANEL_INSET * scale;
        let y = FLOATING_PANEL_INSET * scale;
        let w = (frame.logical_w - 2.0 * FLOATING_PANEL_INSET) * scale;
        let h = (frame.logical_h - 2.0 * FLOATING_PANEL_INSET) * scale;
        round_rect(g, x, y, w, h, panel_r * scale, rgb(0x1F1F1F), 255);
        stroke_rect(
            g,
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
        fill(g, rgb(0x1F1F1F), 255);
        None
    };
    paint_sheet(g, frame.logical_w, frame.logical_h, scale, frame.ps5);
    if let Some((x, y, w, h, radius)) = panel {
        clip_round(g, x, y, w, h, radius);
    }
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

fn sheet_part_width(part: &Leg, g: &mut Gfx) -> f32 {
    match *part {
        Leg::Btn("SELECT") => SHEET_SHARE,
        Leg::Btn(_) => SHEET_GLYPH,
        Leg::Word(word) => measure_label(g, &["Noto Sans"], word, SHEET_TOKEN_PX),
        Leg::Plus => measure_label(g, &["Noto Sans"], "+", SHEET_TOKEN_PX),
        Leg::Slash => measure_label(g, &["Noto Sans"], "/", SHEET_TOKEN_PX),
    }
}

fn sheet_combo_width(parts: &[Leg], g: &mut Gfx) -> f32 {
    parts.iter().map(|part| sheet_part_width(part, g)).sum::<f32>() + SHEET_TOKEN_GAP * parts.len().saturating_sub(1) as f32
}

fn sheet_columns(g: &mut Gfx) -> [SheetCol; 3] {
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
                    combo = combo.max(sheet_combo_width(parts, g));
                    label = label.max(measure_label(g, &["Noto Sans"], text, SHEET_ACTION_PX));
                }
                SheetLine::Title(text) => header = header.max(measure_label(g, &["Noto Sans"], text, SHEET_HEADER_PX)),
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

fn paint_sheet(g: &mut Gfx, frame_w: f32, frame_h: f32, scale: f32, ps5: bool) {
    let cols = sheet_columns(g);
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
                    text_aligned(g, title, (x, y, x + col_w, y + row_h), SHEET_HEADER_PX * u * scale, Weight::NORMAL, white, muted, scale, Align::Left, &["Noto Sans"]);
                }
                SheetLine::Row(parts, label) => {
                    let mut cursor = x;
                    for (part_index, part) in parts.iter().enumerate() {
                        if part_index > 0 {
                            cursor += SHEET_TOKEN_GAP * u;
                        }
                        let w = sheet_part_width(part, g) * u;
                        match *part {
                            Leg::Btn(hint) => {
                                if let Some(svg) = hint_svg(ps5, hint) {
                                    blit_svg(g, svg, cursor, y, cursor + w, y + row_h, w, scale, None, 255);
                                }
                            }
                            Leg::Word(_) | Leg::Plus | Leg::Slash => {
                                text_aligned(g, leg_word(part), (cursor, y, cursor + w, y + row_h), SHEET_TOKEN_PX * u * scale, Weight::NORMAL, white, muted, scale, Align::Left, &["Noto Sans"]);
                            }
                        }
                        cursor += w;
                    }
                    let label_x = x + col.combo * u + SHEET_ACTION_GAP * u;
                    text_aligned(g, label, (label_x, y, x + col_w, y + row_h), SHEET_ACTION_PX * u * scale, Weight::NORMAL, white, 255, scale, Align::Left, &["Noto Sans"]);
                }
                SheetLine::Blank => {}
            }
            if sheet_hairline(&col.lines, index) {
                round_rect(g, x * scale, (y + row_h - u) * scale, col_w * scale, u * scale, 0.0, white, line_a);
            }
        }
        x += col_w;
        if ci + 1 == count {
            break;
        }
        x += SHEET_COL_GAP * u;
        if ci + 2 == count {
            round_rect(g, x * scale, top * scale, u * scale, block_h * scale, 0.0, white, line_a);
            x += u + SHEET_COL_GAP * u;
        }
    }
}

#[cfg(test)]
fn sheet_layout(g: &mut Gfx) -> (f32, [f32; 3]) {
    let cols = sheet_columns(g);
    (sheet_total(&cols), [cols[0].combo, cols[1].combo, cols[2].combo])
}


fn draw_strip(
    g: &mut Gfx,
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
    if is_columns(spec) {
        let inner_l = left;
        let inner_r = right;
        let n = strip.visible.len().max(1) as f32;
        let col_w = (inner_r - inner_l) / n;
        let sep_h = spec.separator_h * unit;
        for (i, word) in strip.visible.iter().enumerate() {
            let x0 = inner_l + i as f32 * col_w;
            let x1 = x0 + col_w;
            if i > 0 {
                round_rect(g, (x0 - 0.5) * scale, (cy - sep_h * 0.5) * scale, scale.max(1.0), sep_h * scale, 0.0, pal.border, fade_u8(spec.separator_alpha));
            }
            if word.is_empty() {
                continue;
            }
            let selected = i == strip.highlight_slot;
            let alpha = fade_u8(if selected { 1.0 } else { spec.chip_text_alpha });
            let color = if selected { pal.text } else { pal.text_dim };
            text(g, word, (x0, top, x1, bottom), spec.chip_px * unit * scale, if selected { weight(spec.chip_sel_weight) } else { Weight::NORMAL }, color, alpha, scale, spec.families);
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
    round_rect(g, chips.0 * scale, chips.1 * scale, (chips.2 - chips.0) * scale, (chips.3 - chips.1) * scale, chips_r * scale, pal.key_action, fade_u8(1.0));
    stroke_rect(g, (chips.0 + 0.5) * scale, (chips.1 + 0.5) * scale, (chips.2 - chips.0 - 1.0) * scale, (chips.3 - chips.1 - 1.0) * scale, (chips_r - 0.5).max(0.0) * scale, pal.border, fade_u8(1.0), scale.max(1.0));
    for (rect, hint) in [(prev, "LB"), (next, "RB")] {
        let rad = button_h * 0.5;
        round_rect(g, rect.0 * scale, rect.1 * scale, (rect.2 - rect.0) * scale, (rect.3 - rect.1) * scale, rad * scale, pal.key_action, fade_u8(1.0));
        stroke_rect(g, (rect.0 + 0.5) * scale, (rect.1 + 0.5) * scale, (rect.2 - rect.0 - 1.0) * scale, (rect.3 - rect.1 - 1.0) * scale, (rad - 0.5).max(0.0) * scale, pal.border, fade_u8(1.0), scale.max(1.0));
        if let Some(svg) = hint_svg(ps5, hint) {
            let px = 30.0 * unit;
            let cx = (rect.0 + rect.2) * 0.5;
            let half = px * 0.5;
            blit_svg(g, svg, cx - half, cy - half, cx + half, cy + half, px, scale, None, fade_u8(1.0));
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
            round_rect(g, x0 * scale, inner_t * scale, chip_w * scale, (inner_b - inner_t) * scale, chip_r * scale, pal.chip_sel, fade_u8(1.0));
        }
        let alpha = fade_u8(if selected { 1.0 } else { spec.chip_text_alpha });
        let weight = if selected { weight(spec.chip_sel_weight) } else { Weight::NORMAL };
        let label = (x0 + chip_r * 0.5, inner_t, x0 + chip_w - chip_r * 0.5, inner_b);
        text(g, word, label, spec.chip_px * unit * scale, weight, pal.text, alpha, scale, spec.families);
    }
}

fn palette(look: VkLook, theme: &KeyboardTheme) -> Pal {
    let v = themed_palette(crate::vk_paint::style_palette(look.style, true), look.style, theme);
    Pal {
        bg: from_colorref(v.bg),
        key: from_colorref(v.key),
        key_action: from_colorref(v.key_action),
        accent: from_colorref(v.accent),
        sel_ring: from_colorref(v.sel_ring),
        text: from_colorref(v.text),
        text_dim: from_colorref(v.text_dim),
        sel_text: from_colorref(v.sel_text),
        border: from_colorref(v.border),
        panel_stroke: from_colorref(v.panel_stroke),
        panel_stroke_alpha: v.panel_stroke_alpha,
        chip_sel: from_colorref(v.chip_sel),
    }
}

pub fn accent(look: VkLook, theme: &KeyboardTheme) -> u32 {
    themed_palette(crate::vk_paint::style_palette(look.style, true), look.style, theme).accent
}

fn themed_palette(mut pal: VkPalette, style: VkStyle, theme: &KeyboardTheme) -> VkPalette {
    use crate::vk_paint::mix_color;
    if style == VkStyle::Modern {
        if let Some(v) = theme.bg {
            pal.bg = v;
        }
        if let Some(v) = theme.text {
            pal.text = v;
            pal.enter_text = v;
            pal.text_dim = mix_color(v, pal.bg, 0.8);
        }
        if let Some(v) = theme.sel_text {
            pal.sel_text = v;
        }
        if let Some(v) = theme.accent {
            pal.accent = v;
            pal.sel_ring = v;
            pal.chip_sel = v;
        }
        if let Some(v) = theme.key {
            pal.key = v;
            pal.key_action = mix_color(pal.text, v, 0.08);
        }
        if theme.key.is_some() || theme.accent.is_some() {
            if let Some(tint) = theme.accent.or(pal.mod_tint.map(|(_, bar)| bar)) {
                pal.key_enter = mix_color(tint, pal.key, 0.14);
                pal.mod_tint = Some((mix_color(tint, pal.key, 0.2), tint));
            }
        }
        if let Some(v) = theme.border {
            pal.border = v;
            pal.panel_stroke = v;
            pal.panel_stroke_alpha = 1.0;
        }
        return pal;
    }
    if style == VkStyle::Mono {
        if let Some(v) = theme.accent {
            pal.accent = mix_color(v, pal.key, 0.7);
            pal.sel_ring = v;
        }
        if let Some(v) = theme.sel_text {
            pal.sel_text = v;
        }
        return pal;
    }
    if let Some(v) = theme.bg {
        pal.bg = v;
    }
    if let Some(v) = theme.key {
        pal.key = v;
        pal.key_action = mix_color(0x000000, v, 0.26);
        pal.chip_sel = mix_color(0xFFFFFF, v, 0.10);
    }
    if let Some(v) = theme.accent {
        pal.accent = v;
        pal.sel_ring = mix_color(0xFFFFFF, v, 0.5);
    }
    if let Some(v) = theme.text {
        pal.text = v;
        pal.text_dim = v;
    }
    if let Some(v) = theme.sel_text {
        pal.sel_text = v;
    }
    if let Some(v) = theme.border {
        pal.border = v;
        pal.panel_stroke = v;
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

fn cref(color: Rgb) -> u32 {
    color.r as u32 | (color.g as u32) << 8 | (color.b as u32) << 16
}

fn device(x: f32, y: f32, w: f32, h: f32) -> crate::mono_ui::Rect {
    crate::mono_ui::rect(x, y, w, h)
}

fn fill(g: &mut Gfx, color: Rgb, alpha: u8) {
    unsafe { g.clear(cref(color), alpha as f32 / 255.0) };
}

fn round_rect(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb, alpha: u8) {
    unsafe { g.fill(device(x, y, w, h), radius, cref(color), alpha as f32 / 255.0) };
}

fn stroke_rect(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb, alpha: u8, width: f32) {
    let width = width.max(1.0);
    let half = width * 0.5;
    let outer = device(x - half, y - half, w + width, h + width);
    unsafe { g.ring(outer, radius + half, width, cref(color), alpha as f32 / 255.0) };
}

fn blur_shadow(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, radius: f32, dy: f32, blur: f32, alpha: f32) {
    unsafe { crate::vk_paint::paint_blur_strokes(g, device(x, y, w, h), radius, dy, blur, alpha) };
}

fn clip_round(g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    let pixmap = g.canvas().0;
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
    g: &mut Gfx,
    value: &str,
    band: (f32, f32, f32, f32),
    px: f32,
    weight: Weight,
    color: Rgb,
    alpha: u8,
    scale: f32,
    family: &[&str],
) {
    text_aligned(g, value, band, px, weight, color, alpha, scale, Align::Center, family);
}

fn measure_label(g: &mut Gfx, family: &[&str], value: &str, px: f32) -> f32 {
    unsafe { g.measure_full(value, px, 400, family) }
}

fn text_aligned(
    g: &mut Gfx,
    value: &str,
    band: (f32, f32, f32, f32),
    px: f32,
    weight: Weight,
    color: Rgb,
    alpha: u8,
    scale: f32,
    align: Align,
    family: &[&str],
) {
    if value.is_empty() || px < 4.0 {
        return;
    }
    let r = device(band.0 * scale, band.1 * scale, (band.2 - band.0) * scale, (band.3 - band.1) * scale);
    unsafe {
        g.text_full(
            value,
            r,
            px,
            weight.0 as i32,
            family,
            cref(color),
            alpha as f32 / 255.0,
            align,
            crate::mono_ui::VAlign::Center,
            false,
        )
    };
}

fn blit_svg(g: &mut Gfx, svg: &str, x0: f32, y0: f32, x1: f32, y1: f32, size: f32, scale: f32, tint: Option<Rgb>, alpha: u8) {
    if alpha == 0 {
        return;
    }
    let tip = tint.is_none();
    let draw_px = size.round().clamp(12.0, 192.0);
    let raster_px = if tip {
        (draw_px * scale * 3.0).round().clamp(54.0, (draw_px * scale * 3.0).max(192.0))
    } else {
        (draw_px * scale).round().max(1.0)
    } as u32;
    let source_px = if tip { 32.0 } else { 24.0 };
    let dst = crate::mono_ui::rect(
        ((x0 + x1) * 0.5 - draw_px * 0.5) * scale,
        ((y0 + y1) * 0.5 - draw_px * 0.5) * scale,
        draw_px * scale,
        draw_px * scale,
    );
    unsafe {
        crate::vk_paint::paint_svg_icon(
            g,
            svg,
            raster_px,
            tint.map(cref).unwrap_or(0),
            dst,
            alpha as f32 / 255.0,
            source_px,
        )
    };
}

pub struct KeyPatch {
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

pub fn paint_base(g: &mut Gfx, frame: &Frame, glide: &mut Glide) {
    let (width, height) = frame_px(frame);
    g.begin(width, height);
    render_keyboard(g, frame, glide, false);
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

pub fn paint_selected_key(g: &mut Gfx, frame: &Frame, press: f32) -> Option<KeyPatch> {
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
    g.begin(width, height);
    paint_key(
        g,
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

pub fn paint_voice_key(g: &mut Gfx, frame: &Frame, orb: &[u8], orb_px: u32) {
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
        g.canvas().0,
        orb,
        orb_px,
        (cx - side * 0.5) * scale,
        (cy - side * 0.5) * scale,
        side * scale,
    );
    stroke_rect(
        g,
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

pub fn voice_swell(phase: u8, level: f32) -> f32 {
    let drive = match phase {
        2 => {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f32())
                .unwrap_or(0.0);
            0.5 + 0.5 * (t * 2.4).sin()
        }
        1 => 0.5 + 0.5 * level.clamp(0.0, 1.0),
        _ => 0.5,
    };
    let scale = 0.70 + 0.36 * drive.clamp(0.0, 1.0);
    let rest = 0.70 + 0.36 * 0.5;
    scale / rest
}

pub fn paint_voice_orb(side: u32, phase: u8, level: f32, orb: &[u8], orb_px: u32) -> Option<Pixmap> {
    if side == 0 {
        return None;
    }
    let mut pixmap = Pixmap::new(side, side)?;
    let swell = voice_swell(phase, level);
    let drawn = (side as f32 * 0.94 * swell / 1.25).clamp(1.0, side as f32);
    let origin = (side as f32 - drawn) * 0.5;
    blit_premul(&mut pixmap, orb, orb_px, origin, origin, drawn);
    Some(pixmap)
}

pub fn paint_voice_screen(
    g: &mut Gfx,
    width: u32,
    height: u32,
    phase: u8,
    level: f32,
    show_orb: bool,
    orb: Option<&[u8]>,
    orb_px: u32,
    accent: u32,
) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    g.begin(width, height);
    let border_alpha = if phase == 2 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f32())
            .unwrap_or(0.0);
        let pulse = 0.5 + 0.5 * (t * 2.4).sin();
        0.8 + 0.7 * pulse
    } else {
        1.0
    };
    if unsafe {
        crate::vk_paint::paint_display_border(g, width as f32, height as f32, accent, border_alpha.min(1.0))
    }
    .is_err()
    {
        return false;
    }
    if show_orb {
        if let Some(orb) = orb {
            let (x, y, side) = voice_screen_slot(width as f32, height as f32, phase, level);
            blit_premul(g.canvas().0, orb, orb_px, x, y, side);
        }
    }
    true
}

fn voice_screen_slot(width: f32, height: f32, phase: u8, level: f32) -> (f32, f32, f32) {
    let scale = (height / 1080.0).clamp(0.85, 2.0);
    let size = (96.0 * scale).min(width.min(height) * 0.9).max(1.0);
    let margin = (48.0 * scale).min((width - size).max(0.0));
    let left = (width - margin - size).max(0.0);
    let top = ((height - size) * 0.5).max(0.0);
    let swell = voice_swell(phase, level);
    let side = (size * 0.94 * swell).max(1.0);
    let cx = left + size * 0.5;
    let cy = top + size * 0.5;
    (cx - side * 0.5, cy - side * 0.5, side)
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

pub fn to_argb(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for px in pixels.chunks_exact(4) {
        out.push(px[2]);
        out.push(px[1]);
        out.push(px[0]);
        out.push(px[3]);
    }
    out
}

pub fn paint_prompt(g: &mut Gfx, text_value: &str, scale: f32) {
    let scale = scale.max(1.0);
    let logical_w = 520.0;
    let logical_h = 72.0;
    g.begin((logical_w * scale) as u32, (logical_h * scale) as u32);
    fill(g, rgb(0x15161C), 235);
    text(g, text_value, (16.0, 16.0, logical_w - 16.0, logical_h - 16.0), 22.0 * scale, Weight::MEDIUM, rgb(0xFFFFFF), 255, 1.0, &["Noto Sans"]);
}

#[cfg(test)]
mod tests {
    use super::{paint_frame, Frame, Glide};
    use crate::mono_ui::Gfx;
    use crate::config::{KeyboardTheme, VkStyle};
    use crate::vk_nav::{rows_for_test, KeyPos};

    #[test]
    fn voice_swell_grows_with_the_mic() {
        let quiet = super::voice_swell(1, 0.0);
        let loud = super::voice_swell(1, 1.0);
        assert!((quiet - 1.0).abs() < 0.001, "{quiet}");
        assert!(loud > quiet + 0.15, "{loud}");
    }

    #[test]
    fn voice_screen_is_a_border_with_a_cloud() {
        let orb = vec![255u8, 255, 255, 255];
        let mut g = Gfx::new(1, 1).expect("gfx");
        assert!(super::paint_voice_screen(&mut g, 800, 480, 1, 1.0, true, Some(&orb), 1, 0xA6D1FF));
        assert_eq!((g.width(), g.height()), (800, 480));
        let edge = g.pixels()[3];
        let centre = g.pixels()[((240 * 800 + 400) * 4 + 3) as usize];
        assert!(edge > 0, "edge {edge}");
        assert_eq!(centre, 0);
    }

    #[test]
    fn keyboard_frame_uses_the_windows_palette() {
        let mut g = Gfx::new(1, 1).expect("gfx");
        let mut frame = Frame {
            logical_w: 1200.0,
            logical_h: 460.0,
            scale: 1.0,
            scale_w: 1920.0 * 1.25,
            rows: rows_for_test(),
            sel: KeyPos { row: 1, col: 1 },
            style: VkStyle::Normal,
            tv: false,
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
        paint_frame(&mut g, &frame, &mut glide);
        let pixmap = g.pixels().to_vec();
        let cold = started.elapsed();
        assert!(g.width() > 1000);
        assert!(pixmap.iter().any(|byte| *byte != 0));
        frame.sel = KeyPos { row: 0, col: 0 };
        let started = std::time::Instant::now();
        paint_frame(&mut g, &frame, &mut glide);
        let moved = g.pixels().to_vec();
        let warm = started.elapsed();
        let mut same = 0usize;
        let mut total = 0usize;
        for (left, right) in pixmap.chunks_exact(4).zip(moved.chunks_exact(4)) {
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
        paint_frame(&mut g, &sheet, &mut Glide::default());
        assert_eq!((g.width(), g.height()), (1600, 546));
        let inset = (12 * 1600 + 80) * 4;
        assert_eq!(&g.pixels()[inset..inset + 4], &[0x1F, 0x1F, 0x1F, 0xFF]);
    }

    #[test]
    fn shortcut_overlay_matches_the_pen_frame() {
        let (w, h) = super::sheet_size(3072.0, 1728.0);
        assert_eq!((w, h), (1600, 546));
        let mut g = Gfx::new(1, 1).expect("gfx");
        let (total, combos) = super::sheet_layout(&mut g);
        for (got, want) in combos.iter().zip([86.0, 183.0, 140.0]) {
            assert!((got - want).abs() <= 16.0, "combos {combos:?} total {total}");
        }
        assert!(total < 1600.0 - 96.0, "content {total} leaves the pen's side inset");
        let frame = Frame {
            logical_w: 1600.0,
            logical_h: 546.0,
            scale: 1.0,
            scale_w: 1920.0,
            rows: rows_for_test(),
            sel: KeyPos { row: 0, col: 0 },
            style: VkStyle::Normal,
            tv: false,
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
        paint_frame(&mut g, &frame, &mut Glide::default());
        assert!(g.pixels().chunks_exact(4).any(|px| px[0] != 0x1F || px[1] != 0x1F || px[2] != 0x1F));
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
        paint_frame(&mut g, &docked, &mut glide);
        let cold = cold_at.elapsed();
        docked.sel = KeyPos { row: 1, col: 2 };
        let warm_at = std::time::Instant::now();
        paint_frame(&mut g, &docked, &mut glide);
        let warm = warm_at.elapsed();
        assert!(warm < std::time::Duration::from_millis(120), "docked highlight cold {cold:?} warm {warm:?}");
    }
}

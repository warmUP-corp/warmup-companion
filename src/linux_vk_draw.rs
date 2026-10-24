use std::time::Instant;

use tiny_skia::Pixmap;

use crate::config::{KeyboardTheme, VkStyle};
use crate::mono_ui::Gfx;
use crate::vk_paint::VkLook;
use crate::vk_geom::{self, KeyRect};
use crate::vk_motion;
use crate::vk_nav::{KeyAction, KeyPos, KeyRow};
use crate::vk_predict::StripState;

pub const REF_MON_W: f32 = 1920.0;
const REF_KEY_W: f32 = 92.0;

const FLOATING_PANEL_INSET: f32 = 1.0;
const BAR_H: f32 = 80.0;
const BAR_GLYPH: f32 = 38.0;
const BAR_SHARE: f32 = 44.0;
const BAR_GAP: f32 = 6.0;
const BAR_LABEL_GAP: f32 = 6.0;
const BAR_LABEL_PX: f32 = 19.0;
const BAR_CONNECTOR_PX: f32 = 17.0;
const BAR_PREFIX_PX: f32 = 20.0;
const BAR_SEP_PX: f32 = 18.0;
const LEGEND_MIN_TEXT_PX: f32 = 11.0;
const LEGEND_SEP: &str = "\u{00B7}";
const ALPHA_SECONDARY: f32 = 0xA6 as f32 / 255.0;
const ALPHA_SEP: f32 = 0x66 as f32 / 255.0;

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



#[derive(Clone, Copy)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

struct Pal {
    bg: Rgb,
    sel_ring: Rgb,
}

pub struct Frame {
    pub logical_w: f32,
    pub logical_h: f32,
    pub scale: f32,
    pub scale_w: f32,
    pub ui: f32,
    pub rows: Vec<KeyRow>,
    pub sel: KeyPos,
    pub style: VkStyle,
    pub tv: bool,
    pub dark: bool,
    pub pal: crate::vk_paint::VkPalette,
    pub voice_available: bool,
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
    pub sheet_hint: bool,
    pub voice: Option<Voice>,
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

pub fn design_scale(style: VkStyle) -> f32 {
    let key_w = match style {
        VkStyle::Normal | VkStyle::Modern => 138.0,
        VkStyle::Mono => 137.0,
    };
    key_w / REF_KEY_W
}

pub fn layout_scale(look: VkLook, output_h: f32, rows: &[KeyRow]) -> f32 {
    let design = design_scale(look.style);
    if !look.tv_layout() {
        return design;
    }
    let unit = 1.0 / crate::config::COMPACT_BAR_SCALE;
    let capped = vk_geom::tv_capped_dpi(design / unit, output_h.round() as i32, rows, look, unit);
    (capped * unit).min(design)
}

pub fn card_size(output_w: f32, output_h: f32, docked: bool, rows: &[KeyRow], look: VkLook) -> (u32, u32) {
    let spec = spec_for(look);
    let scale = layout_scale(look, output_h, rows);
    let scale_w = REF_MON_W * scale;
    let (grid_w, block_h) = vk_geom::grid_size(scale_w, rows, spec.key_aspect, spec.gap, spec.design_kh);
    let chrome = crate::vk_paint::strip_band_height(scale, look);
    let legend = legend_band_height(scale, look);
    let (pad_x, pad_y) = crate::vk_paint::floating_pad(scale, look);
    let full = if docked && look.tv_layout() {
        vk_geom::dock_height_for_full_keys(scale_w, rows, chrome, spec.key_aspect, spec.gap, spec.design_kh)
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
    let pal = palette(frame);
    let lu = crate::vk_paint::ref_unit(frame.ui, look);
    let top_inset = crate::vk_paint::strip_band_height(frame.ui, look);
    let legend_h = legend_band_height(frame.ui, look);
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
        unsafe {
            crate::vk_paint::paint_floating_panel(
                g,
                crate::mono_ui::Rect { x, y, w, h },
                panel_r * scale,
                scale.max(1.0),
                &frame.pal,
            );
        }
        Some((x, y, w, h, panel_r * scale))
    } else {
        fill(g, pal.bg, 255);
        None
    };
    let ks = key_style(frame, unit, key_h, scale);
    for rect in &rects {
        let selected = highlight && rect.pos == frame.sel;
        let press = if highlight { press_of(frame, rect.pos) } else { None };
        paint_key(g, frame, &ks, rect, scale, selected, press);
    }
    if highlight {
        stroke_ring(g, glide, frame, &ks, scale);
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
        paint_strip(g, frame, strip, &rects, unit, scale, strip_dy, strip_alpha);
    }
    if !look.tv_layout() {
        paint_legend(g, frame, &rects, lu, scale);
    } else if frame.sheet_hint && !frame.strip.as_ref().is_some_and(|s| s.visible.iter().any(|w| !w.is_empty())) {
        paint_sheet_hint(g, frame, &rects, scale);
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







fn paint_legend(g: &mut Gfx, frame: &Frame, rects: &[KeyRect], lu: f32, scale: f32) {
    let look = frame.look();
        let pad_x = crate::vk_paint::floating_pad(frame.ui, look).0;
    let (left, right) = crate::vk_paint::legend_span(rects, frame.logical_w, pad_x);
    let lp = crate::vk_paint::LegendPaint {
        pal: &frame.pal,
        style: look.style,
        family: icon_family(frame),
        u: lu * scale,
        top: (frame.logical_h - crate::vk_paint::BAR_H * lu) * scale,
        left: left * scale,
        right: right * scale,
        alpha: 1.0,
        dip_scale: 1.0,
    };
    unsafe { crate::vk_paint::paint_legend(g, &crate::win::shortcut_sheet::shown_legend(), &lp) };
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
    frame.ui.to_bits().hash(&mut hasher);
    matches!(frame.style, VkStyle::Mono).hash(&mut hasher);
    frame.ps5.hash(&mut hasher);
    frame.shift.hash(&mut hasher);
    frame.caps.hash(&mut hasher);
    frame.symbol.hash(&mut hasher);
    frame.symbol_locked.hash(&mut hasher);
    frame.floating.hash(&mut hasher);
    frame.dark.hash(&mut hasher);
    frame.sheet_hint.hash(&mut hasher);
    frame.voice_available.hash(&mut hasher);
    frame.voice.is_some().hash(&mut hasher);
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

fn press_of(frame: &Frame, pos: KeyPos) -> Option<f32> {
    frame.pressed.filter(|(p, _)| *p == pos).map(|(_, s)| s)
}

fn key_style(frame: &Frame, unit: f32, key_h: f32, scale: f32) -> crate::vk_paint::KeyStyle<'_> {
    let look = frame.look();
    let label_px = spec_for(look).label_px * unit * scale;
    crate::vk_paint::KeyStyle {
        look,
        pal: &frame.pal,
        unit: unit * scale,
        key_h: key_h * scale,
        dip_scale: 1.0,
        glyph_px: label_px * 1.1,
        hint_px: (label_px * 0.5).clamp(10.0, 20.0),
        icons: icon_family(frame),
    }
}

fn icon_family(frame: &Frame) -> crate::vk_paint::ControllerIconFamily {
    if frame.ps5 {
        crate::vk_paint::ControllerIconFamily::Ps5
    } else {
        crate::vk_paint::ControllerIconFamily::Xbox
    }
}

fn device_edges(left: f32, top: f32, right: f32, bottom: f32, scale: f32) -> crate::vk_paint::Edges {
    crate::vk_paint::Edges {
        left: left * scale,
        top: top * scale,
        right: right * scale,
        bottom: bottom * scale,
    }
}

fn paint_key(
    g: &mut Gfx,
    frame: &Frame,
    ks: &crate::vk_paint::KeyStyle,
    rect: &KeyRect,
    scale: f32,
    selected: bool,
    press: Option<f32>,
) {
    let Some(key) = frame.rows.get(rect.pos.row).and_then(|row| row.keys.get(rect.pos.col)) else { return };
    let st = crate::vk_paint::KeyState {
        selected,
        press_scale: press,
        shift: frame.shift,
        caps: frame.caps,
        symbols: frame.symbol,
        symbols_locked: frame.symbol_locked,
        voice_available: frame.voice_available,
        tonal_row: rect.pos.row == 0 && frame.rows.len() == 5,
        glyph: crate::vk_paint::key_glyph(key),
        hint: crate::vk_paint::key_hint(key),
    };
    let edges = device_edges(rect.left, rect.top, rect.right, rect.bottom, scale);
    let mut voice_paint = |_: &mut Gfx, _: crate::vk_paint::Edges, _: f32| {};
    let voice: Option<&mut dyn FnMut(&mut Gfx, crate::vk_paint::Edges, f32)> =
        if frame.voice.is_some() { Some(&mut voice_paint) } else { None };
    unsafe { crate::vk_paint::paint_key(g, key, edges, ks, &st, voice) };
}

fn stroke_ring(g: &mut Gfx, glide: &Glide, frame: &Frame, ks: &crate::vk_paint::KeyStyle, scale: f32) {
    let Some((l, t, r, b)) = glide.ring else { return };
    unsafe { crate::vk_paint::paint_focus_ring(g, device_edges(l, t, r, b, scale), ks, press_of(frame, frame.sel)) };
}

fn erase_key(g: &mut Gfx, rect: &KeyRect, spec: &Spec, pal: &Pal, unit: f32, scale: f32) {
    let dy = (spec.key_shadow_dy * unit).max(0.0);
    round_rect(g, rect.left * scale, rect.top * scale, (rect.right - rect.left) * scale, (rect.bottom - rect.top + dy) * scale, 0.0, pal.bg, 255);
}

fn scene(frame: &Frame) -> (&'static Spec, Pal, Vec<KeyRect>, f32, f32, f32, f32) {
    let scale = frame.scale.max(1.0);
    let look = frame.look();
    let spec = spec_for(look);
    let pal = palette(frame);
    let top_inset = crate::vk_paint::strip_band_height(frame.ui, look);
    let legend_h = legend_band_height(frame.ui, look);
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
    let (spec, pal, rects, unit, scale, _, key_h) = scene(frame);
    let target = rects.iter().find(|r| r.pos == frame.sel).map(|r| (r.left, r.top, r.right, r.bottom));
    if step {
        advance_glide(glide, frame, target, Instant::now());
    }
    let ks = key_style(frame, unit, key_h, scale);
    if let Some(sel_index) = rects.iter().position(|r| r.pos == frame.sel) {
        erase_key(g, &rects[sel_index], &spec, &pal, unit, scale);
        let press = press_of(frame, rects[sel_index].pos);
        paint_key(g, frame, &ks, &rects[sel_index], scale, true, press);
        if let Some((pos, press)) = frame.pressed {
            if pos != rects[sel_index].pos {
                if let Some(other) = rects.iter().find(|rect| rect.pos == pos) {
                    erase_key(g, other, &spec, &pal, unit, scale);
                    paint_key(g, frame, &ks, other, scale, false, Some(press));
                }
            }
        }
    }
    stroke_ring(g, glide, frame, &ks, scale);
    if frame.floating {
        let panel_r = spec.panel_radius * unit * scale;
        let x = FLOATING_PANEL_INSET * scale;
        let y = FLOATING_PANEL_INSET * scale;
        let w = (frame.logical_w - 2.0 * FLOATING_PANEL_INSET) * scale;
        let h = (frame.logical_h - 2.0 * FLOATING_PANEL_INSET) * scale;
        clip_round(g, x, y, w, h, panel_r);
    }
}

fn sheet_paint<'a>(frame: &Frame, pal: &'a crate::vk_paint::VkPalette) -> crate::vk_paint::SheetPaint<'a> {
    crate::vk_paint::SheetPaint {
        pal,
        style: frame.style,
        family: icon_family(frame),
        dip_scale: 1.0,
    }
}

fn draw_sheet_frame(g: &mut Gfx, frame: &Frame) {
    let (spec, _, _, unit, scale, _, _) = scene(frame);
    let look = frame.look();
        let sp = sheet_paint(frame, &frame.pal);
    let (w, h) = (frame.logical_w * scale, frame.logical_h * scale);
    let inset = FLOATING_PANEL_INSET * scale;
    let inset_panel = crate::vk_paint::Edges {
        left: inset,
        top: inset,
        right: w - inset,
        bottom: h - inset,
    };
    let radius = frame.floating.then_some(spec.panel_radius * unit * scale);
    unsafe {
        if look.tv_layout() {
            let u = crate::vk_paint::ref_unit(frame.ui, look) * crate::vk_paint::MS_SCALE * scale;
            crate::vk_paint::paint_modern_sheet(g, &sp, u, inset_panel, radius, 1.0);
        } else {
            let panel = if frame.floating {
                inset_panel
            } else {
                crate::vk_paint::Edges {
                    left: 0.0,
                    top: 0.0,
                    right: w,
                    bottom: h,
                }
            };
            crate::vk_paint::paint_shortcut_sheet(g, &sp, unit * scale, panel, radius, 1.0);
        }
    }
}

fn paint_sheet_hint(g: &mut Gfx, frame: &Frame, rects: &[KeyRect], scale: f32) {
    if rects.is_empty() {
        return;
    }
    let left = rects.iter().map(|r| r.left).fold(f32::INFINITY, f32::min);
    let right = rects.iter().map(|r| r.right).fold(f32::NEG_INFINITY, f32::max);
    let top = rects.iter().map(|r| r.top).fold(f32::INFINITY, f32::min);
    let u = crate::vk_paint::ref_unit(frame.ui, frame.look());
        let sp = sheet_paint(frame, &frame.pal);
    unsafe {
        crate::vk_paint::paint_sheet_hint(
            g,
            &sp,
            u * scale,
            (left + right) * 0.5 * scale,
            top * scale,
            frame.logical_w * scale,
        )
    };
}












fn paint_strip(
    g: &mut Gfx,
    frame: &Frame,
    strip: &StripState,
    rects: &[KeyRect],
    unit: f32,
    scale: f32,
    dy: f32,
    alpha: f32,
) {
    let look = frame.look();
    let spec = spec_for(look);
        let above = crate::vk_paint::StripGeom::above(rects, spec, unit);
    let geom = crate::vk_paint::StripGeom {
        left: above.left * scale,
        top: above.top * scale,
        right: above.right * scale,
        bottom: above.bottom * scale,
        unit: above.unit * scale,
    };
    let sp = crate::vk_paint::StripPaint {
        spec,
        pal: &frame.pal,
        alpha: alpha.clamp(0.0, 1.0),
        icons: icon_family(frame),
        select_retired: spec.strip == crate::vk_paint::StripLook::Chips
            && crate::win::shortcut_sheet::hint_retired(),
        dip_scale: 1.0,
    };
    unsafe {
        g.transform(1.0, 0.0, dy * scale);
        crate::vk_paint::paint_strip(g, &sp, strip, &geom);
        g.transform(1.0, 0.0, 0.0);
    }
}

fn palette(frame: &Frame) -> Pal {
    let v = &frame.pal;
    Pal {
        bg: from_colorref(v.bg),
        sel_ring: from_colorref(v.sel_ring),
    }
}

pub fn themed_palette(look: VkLook, dark: bool, theme: &KeyboardTheme) -> crate::vk_paint::VkPalette {
    crate::vk_theme::themed_palette(crate::vk_paint::style_palette(look.style, dark), look.style, *theme)
}


fn from_colorref(color: u32) -> Rgb {
    Rgb { r: (color & 0xff) as u8, g: ((color >> 8) & 0xff) as u8, b: ((color >> 16) & 0xff) as u8 }
}









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

pub const ORB_FRAME_FILL: f32 = 0.94;

#[derive(Clone, Copy)]
pub struct Voice {
    pub phase: u8,
    pub level: f32,
    pub t: f32,
}

impl Voice {
    fn transcribing(&self) -> bool {
        self.phase == 2
    }

    fn pulse(&self) -> f32 {
        0.5 + 0.5 * (self.t * 2.4).sin()
    }

    pub fn orb_scale(&self) -> f32 {
        let drive = match self.phase {
            1 => 0.5 + 0.5 * self.level.clamp(0.0, 1.0),
            2 => self.pulse(),
            _ => 0.5,
        };
        0.70 + 0.36 * drive.clamp(0.0, 1.0)
    }

    fn halo_alpha(&self) -> f32 {
        match self.phase {
            1 => 0.28 + 0.34 * self.level.clamp(0.0, 1.0),
            2 => 0.28 + 0.42 * self.pulse(),
            _ => 0.18,
        }
    }
}

pub struct VoiceKey {
    pub key: crate::mono_ui::Rect,
    pub ball: crate::mono_ui::Rect,
    pub halo: crate::mono_ui::Rect,
    pub halo_radius: f32,
    pub halo_w: f32,
    pub halo_alpha: f32,
    pub accent: u32,
}

pub fn voice_key(frame: &Frame) -> Option<VoiceKey> {
    let voice = frame.voice?;
    let (spec, _pal, rects, unit, scale, _radius, _key_h) = scene(frame);
    let rect = rects.iter().find(|rect| {
        frame
            .rows
            .get(rect.pos.row)
            .and_then(|row| row.keys.get(rect.pos.col))
            .is_some_and(|key| matches!(key.action, KeyAction::VoiceInput))
    })?;
    let key = device_edges(rect.left, rect.top, rect.right, rect.bottom, scale).rect();
    let radius = spec.key_radius * unit * scale;
    let side = key.w.min(key.h).max(1.0);
    let ball = side * ORB_FRAME_FILL * voice.orb_scale();
    let cx = key.x + key.w * 0.5;
    let cy = key.y + key.h * 0.5;
    Some(VoiceKey {
        key,
        ball: crate::mono_ui::Rect {
            x: cx - ball * 0.5,
            y: cy - ball * 0.5,
            w: ball,
            h: ball,
        },
        halo: crate::mono_ui::Rect {
            x: key.x - scale,
            y: key.y - scale,
            w: key.w + 2.0 * scale,
            h: key.h + 2.0 * scale,
        },
        halo_radius: radius + scale,
        halo_w: 2.0 * scale,
        halo_alpha: voice.halo_alpha(),
        accent: frame.pal.accent,
    })
}

impl VoiceKey {
    pub fn halo_rgba(&self) -> [f32; 4] {
        let c = from_colorref(self.accent);
        let a = self.halo_alpha.clamp(0.0, 1.0);
        [c.r as f32 / 255.0 * a, c.g as f32 / 255.0 * a, c.b as f32 / 255.0 * a, a]
    }
}

pub fn paint_voice_key(g: &mut Gfx, frame: &Frame, nimbus: Option<(&[u8], u32)>) {
    let (Some(voice), Some(spot)) = (frame.voice, voice_key(frame)) else {
        return;
    };
    unsafe {
        g.push_clip(spot.key, true);
        if !draw_nimbus(g, nimbus, spot.ball, 1.0) {
            crate::vk_paint::paint_voice_orb(
                g,
                spot.accent,
                voice.level,
                voice.transcribing(),
                spot.ball.x + spot.ball.w * 0.5,
                spot.ball.y + spot.ball.h * 0.5,
                (spot.key.w.min(spot.key.h) * 0.56).max(1.0),
                1.25,
                voice.t,
            );
        }
        g.pop_clip();
        g.ring(spot.halo, spot.halo_radius, spot.halo_w, spot.accent, spot.halo_alpha);
    }
}

pub fn paint_voice_fallback(g: &mut Gfx, frame: &Frame, spot: &VoiceKey) -> Option<(u32, Vec<u8>, crate::mono_ui::Rect)> {
    let voice = frame.voice?;
    let side = spot.key.w.max(spot.key.h).ceil().max(1.0);
    let px = side as u32;
    g.begin(px, px);
    let c = side * 0.5;
    unsafe {
        crate::vk_paint::paint_voice_orb(
            g,
            spot.accent,
            voice.level,
            voice.transcribing(),
            c,
            c,
            (spot.key.w.min(spot.key.h) * 0.56).max(1.0),
            1.25,
            voice.t,
        );
    }
    let cx = spot.key.x + spot.key.w * 0.5;
    let cy = spot.key.y + spot.key.h * 0.5;
    let pixels = g.pixels().to_vec();
    Some((px, pixels, crate::mono_ui::Rect { x: cx - c, y: cy - c, w: side, h: side }))
}

fn draw_nimbus(g: &mut Gfx, nimbus: Option<(&[u8], u32)>, dst: crate::mono_ui::Rect, alpha: f32) -> bool {
    let Some((bytes, px)) = nimbus.filter(|(bytes, px)| *px > 0 && bytes.len() >= (*px as usize).pow(2) * 4) else {
        return false;
    };
    unsafe {
        g.set_image("nimbus", px, px, &bytes[..(px as usize).pow(2) * 4]);
        g.draw_image("nimbus", dst, alpha, false);
    }
    true
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
    let ks = key_style(frame, unit, key_h, scale);
    paint_key(g, frame, &ks, &local, scale, true, (press < 0.999).then_some(press));
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

pub fn paint_voice_orb(
    g: &mut Gfx,
    side: u32,
    slot: f32,
    voice: Voice,
    nimbus: Option<(&[u8], u32)>,
    accent: u32,
) -> bool {
    if side == 0 {
        return false;
    }
    g.begin(side, side);
    let c = side as f32 * 0.5;
    let room = crate::mono_ui::Rect {
        x: c - slot * 0.5,
        y: c - slot * 0.5,
        w: slot,
        h: slot,
    };
    let ball = crate::vk_paint::scale_rect_about_center(room, ORB_FRAME_FILL * voice.orb_scale());
    let mut paint_nimbus = |g: &mut Gfx, dst: crate::mono_ui::Rect, alpha: f32| draw_nimbus(g, nimbus, dst, alpha);
    unsafe { crate::vk_paint::paint_voice_cloud(g, ball, accent, 1.0, voice.t, &mut paint_nimbus) };
    true
}

pub fn paint_voice_screen(g: &mut Gfx, width: u32, height: u32, voice: Voice, accent: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    g.begin(width, height);
    let mut no_cloud = |_: &mut Gfx, _: crate::mono_ui::Rect, _: f32| false;
    unsafe {
        crate::vk_paint::paint_voice_screen(
            g,
            width as f32,
            height as f32,
            accent,
            1.0,
            voice.transcribing(),
            voice.t,
            None,
            &mut no_cloud,
        )
    }
    .is_ok()
}

pub fn paint_prompt_card(g: &mut Gfx, card: &crate::vk_paint::PromptCard, width: f32, height: f32, scale: f32, t: f32) -> bool {
    let scale = scale.max(1.0);
    g.begin((width * scale).round().max(1.0) as u32, (height * scale).round().max(1.0) as u32);
    unsafe { g.transform(scale, 0.0, 0.0) };
    let canvas = crate::vk_paint::PromptCanvas {
        cw: width,
        ch: height,
        dpr: scale,
        t,
        title_px: 35.0,
        dip_scale: scale,
    };
    let mut paint_art = |g: &mut Gfx, art: crate::vk_paint::ControllerArt, rect: crate::mono_ui::Rect, alpha: f32| {
        let px = crate::vk_paint::CONTROLLER_ART_PX as f32;
        let dest = crate::vk_paint::fit_rect(rect, px, px);
        unsafe { g.draw_art(art.key(), art.png(), 384, 384, 0.0, dest, alpha, true) };
    };
    let painted = unsafe { crate::vk_paint::paint_prompt_card(g, card, &canvas, &mut paint_art) }.is_ok();
    unsafe { g.transform(1.0, 0.0, 0.0) };
    painted
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


#[cfg(test)]
mod tests {
    use super::{paint_frame, Frame, Glide};
    use crate::mono_ui::Gfx;
    use crate::config::{KeyboardTheme, VkStyle};
    use crate::vk_nav::{rows_for_test, KeyPos};

    #[test]
    fn voice_swell_grows_with_the_mic() {
        let quiet = super::Voice { phase: 1, level: 0.0, t: 0.0 }.orb_scale();
        let loud = super::Voice { phase: 1, level: 1.0, t: 0.0 }.orb_scale();
        let idle = super::Voice { phase: 0, level: 0.0, t: 0.0 }.orb_scale();
        assert!((quiet - idle).abs() < 0.001, "{quiet}");
        assert!(loud > quiet + 0.15, "{loud}");
    }

    #[test]
    fn voice_screen_is_a_border_with_a_cloud() {
        let mut g = Gfx::new(1, 1).expect("gfx");
        assert!(super::paint_voice_screen(&mut g, 800, 480, super::Voice { phase: 1, level: 0.0, t: 0.0 }, 0xA6D1FF));
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
            ui: super::ui_scale(),
            rows: rows_for_test(),
            sel: KeyPos { row: 1, col: 1 },
            style: VkStyle::Normal,
            tv: false,
            dark: true,
            pal: crate::vk_paint::style_palette(VkStyle::Normal, true),
            voice_available: true,
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
            sheet_hint: false,
            voice: None,
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
        assert_eq!(&g.pixels()[inset..inset + 4], &[0x15, 0x16, 0x1C, 0xFF]);
    }

    #[test]
    fn shortcut_overlay_matches_the_pen_frame() {
        let mut g = Gfx::new(1, 1).expect("gfx");
        let frame = Frame {
            logical_w: 1600.0,
            logical_h: 546.0,
            scale: 1.0,
            scale_w: 1920.0,
            ui: super::ui_scale(),
            rows: rows_for_test(),
            sel: KeyPos { row: 0, col: 0 },
            style: VkStyle::Normal,
            tv: false,
            dark: true,
            pal: crate::vk_paint::style_palette(VkStyle::Normal, true),
            voice_available: true,
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
            sheet_hint: false,
            voice: None,
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

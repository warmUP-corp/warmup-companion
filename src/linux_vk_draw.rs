use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::config::{KeyboardTheme, VkStyle};
use crate::vk_geom::{self, KeyRect};
use crate::vk_nav::{KeyPos, KeyRow};
use crate::vk_predict::StripState;

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
}

#[derive(Clone, Copy)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

struct Palette {
    bg: Rgb,
    key: Rgb,
    accent: Rgb,
    text: Rgb,
    sel_text: Rgb,
    border: Rgb,
}

pub fn draw(frame: &Frame, fonts: &mut FontSystem, cache: &mut SwashCache) -> Option<Pixmap> {
    let scale = frame.scale.max(1.0);
    let width = (frame.logical_w * scale).round().max(1.0) as u32;
    let height = (frame.logical_h * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let pal = palette(&frame.theme);
    fill(&mut pixmap, pal.bg, 255);
    let metrics = vk_geom::style_metrics(matches!(frame.style, VkStyle::Mono));
    let legend_h = 44.0;
    let strip_h = if frame.strip.is_some() { 56.0 } else { 0.0 };
    let key_top = 8.0 + strip_h;
    let key_h = (frame.logical_h - key_top - legend_h).max(40.0);
    if let Some(strip) = &frame.strip {
        draw_strip(
            &mut pixmap,
            fonts,
            cache,
            strip,
            12.0,
            8.0,
            frame.logical_w - 24.0,
            48.0,
            scale,
            &pal,
        );
    }
    let rects = vk_geom::key_rects(
        frame.logical_w,
        key_h,
        frame.scale_w,
        &frame.rows,
        0.0,
        metrics.key_aspect,
        metrics.gap,
        metrics.design_kh,
    );
    for rect in &rects {
        let mut placed = KeyRect {
            pos: rect.pos,
            left: rect.left,
            top: rect.top + key_top,
            right: rect.right,
            bottom: rect.bottom + key_top,
        };
        if frame.pressed.is_some_and(|(pos, _)| pos == placed.pos) {
            let inset = 2.0;
            placed.left += inset;
            placed.top += inset;
            placed.right -= inset;
            placed.bottom -= inset;
        }
        let selected = placed.pos == frame.sel;
        let key = frame
            .rows
            .get(placed.pos.row)
            .and_then(|row| row.keys.get(placed.pos.col));
        let Some(key) = key else { continue };
        let fill_color = if selected { pal.accent } else { pal.key };
        let label_color = if selected { pal.sel_text } else { pal.text };
        round_rect(
            &mut pixmap,
            placed.left * scale,
            placed.top * scale,
            (placed.right - placed.left) * scale,
            (placed.bottom - placed.top) * scale,
            metrics.key_radius * scale,
            fill_color,
            255,
        );
        if selected {
            stroke_rect(
                &mut pixmap,
                placed.left * scale,
                placed.top * scale,
                (placed.right - placed.left) * scale,
                (placed.bottom - placed.top) * scale,
                metrics.key_radius * scale,
                pal.border,
            );
        }
        let label_px = ((placed.bottom - placed.top) * 0.38 * scale).clamp(12.0, 42.0);
        draw_text(
            &mut pixmap,
            fonts,
            cache,
            &key.label,
            placed.left * scale,
            (placed.top + (placed.bottom - placed.top) * 0.22) * scale,
            (placed.right - placed.left) * scale,
            (placed.bottom - placed.top) * 0.62 * scale,
            label_px,
            label_color,
        );
        if let Some(sub) = &key.sublabel {
            draw_text(
                &mut pixmap,
                fonts,
                cache,
                sub,
                placed.left * scale,
                placed.top * scale,
                (placed.right - placed.left) * scale,
                (placed.bottom - placed.top) * 0.28 * scale,
                (label_px * 0.45).max(10.0),
                pal.text,
            );
        }
    }
    draw_legend(
        &mut pixmap,
        fonts,
        cache,
        frame.ps5,
        12.0 * scale,
        (frame.logical_h - legend_h + 6.0) * scale,
        (frame.logical_w - 24.0) * scale,
        32.0 * scale,
        scale,
        &pal,
    );
    Some(pixmap)
}

pub fn draw_prompt(text: &str, scale: f32, fonts: &mut FontSystem, cache: &mut SwashCache) -> Option<Pixmap> {
    let scale = scale.max(1.0);
    let logical_w = 520.0;
    let logical_h = 72.0;
    let width = (logical_w * scale).round() as u32;
    let height = (logical_h * scale).round() as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let bg = Rgb { r: 0x15, g: 0x16, b: 0x1c };
    let text_color = Rgb { r: 0xff, g: 0xff, b: 0xff };
    fill(&mut pixmap, bg, 235);
    draw_text(
        &mut pixmap,
        fonts,
        cache,
        text,
        16.0 * scale,
        16.0 * scale,
        (logical_w - 32.0) * scale,
        40.0 * scale,
        22.0 * scale,
        text_color,
    );
    Some(pixmap)
}

fn palette(theme: &KeyboardTheme) -> Palette {
    let mut pal = Palette {
        bg: Rgb { r: 0x15, g: 0x16, b: 0x1c },
        key: Rgb { r: 0x2a, g: 0x2b, b: 0x36 },
        accent: Rgb { r: 0xa6, g: 0xd1, b: 0xff },
        text: Rgb { r: 0xff, g: 0xff, b: 0xff },
        sel_text: Rgb { r: 0x1a, g: 0x22, b: 0x33 },
        border: Rgb { r: 0x3a, g: 0x3b, b: 0x47 },
    };
    if let Some(color) = theme.bg {
        pal.bg = from_colorref(color);
    }
    if let Some(color) = theme.key {
        pal.key = from_colorref(color);
    }
    if let Some(color) = theme.accent {
        pal.accent = from_colorref(color);
    }
    if let Some(color) = theme.text {
        pal.text = from_colorref(color);
    }
    if let Some(color) = theme.sel_text {
        pal.sel_text = from_colorref(color);
    }
    if let Some(color) = theme.border {
        pal.border = from_colorref(color);
    }
    pal
}

fn from_colorref(color: u32) -> Rgb {
    Rgb {
        r: (color & 0xff) as u8,
        g: ((color >> 8) & 0xff) as u8,
        b: ((color >> 16) & 0xff) as u8,
    }
}

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

fn stroke_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb) {
    let Some(path) = rounded(x, y, w, h, radius) else { return };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, 255);
    let stroke = Stroke { width: 2.0, ..Stroke::default() };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    if w < 1.0 || h < 1.0 {
        return None;
    }
    let radius = radius.min(w * 0.5).min(h * 0.5).max(0.0);
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(x + w - radius, y);
    path.quad_to(x + w, y, x + w, y + radius);
    path.line_to(x + w, y + h - radius);
    path.quad_to(x + w, y + h, x + w - radius, y + h);
    path.line_to(x + radius, y + h);
    path.quad_to(x, y + h, x, y + h - radius);
    path.line_to(x, y + radius);
    path.quad_to(x, y, x + radius, y);
    path.close();
    path.finish()
}

fn draw_text(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    px: f32,
    color: Rgb,
) {
    if text.is_empty() || w < 2.0 || h < 2.0 {
        return;
    }
    let metrics = Metrics::new(px, px * 1.15);
    let mut buffer = Buffer::new(fonts, metrics);
    buffer.set_size(fonts, Some(w), Some(h));
    let attrs = Attrs::new().family(Family::SansSerif);
    buffer.set_text(fonts, text, &attrs, Shaping::Advanced);
    for line in &mut buffer.lines {
        line.set_align(Some(Align::Center));
    }
    buffer.shape_until_scroll(fonts, false);
    let paint_color = Color::rgb(color.r, color.g, color.b);
    buffer.draw(fonts, cache, paint_color, |gx, gy, _, _, pixel| {
        blend(pixmap, x as i32 + gx, y as i32 + gy, pixel);
    });
}

fn blend(pixmap: &mut Pixmap, x: i32, y: i32, pixel: Color) {
    if x < 0 || y < 0 {
        return;
    }
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    if x >= pw || y >= ph {
        return;
    }
    let alpha = pixel.a();
    if alpha == 0 {
        return;
    }
    let width = pixmap.width();
    let data = pixmap.data_mut();
    let index = ((y as u32 * width) + x as u32) as usize * 4;
    let src_a = alpha as f32 / 255.0;
    let dst_a = data[index + 3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    for (channel, src) in [pixel.r(), pixel.g(), pixel.b()].into_iter().enumerate() {
        let dst = data[index + channel] as f32 / 255.0;
        let out = (src as f32 / 255.0 * src_a + dst * dst_a * (1.0 - src_a)) / out_a;
        data[index + channel] = (out * 255.0).round() as u8;
    }
    data[index + 3] = (out_a * 255.0).round() as u8;
}

fn draw_strip(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    strip: &StripState,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    scale: f32,
    pal: &Palette,
) {
    let shown: Vec<&str> = strip.visible.iter().map(String::as_str).filter(|word| !word.is_empty()).collect();
    if shown.is_empty() {
        return;
    }
    let gap = 6.0 * scale;
    let chip_w = ((w * scale) - gap * (shown.len() as f32 - 1.0)) / shown.len() as f32;
    for (index, word) in shown.iter().enumerate() {
        let left = x * scale + index as f32 * (chip_w + gap);
        let selected = strip.visible.get(strip.highlight_slot).is_some_and(|label| label == *word);
        let fill_color = if selected { pal.accent } else { pal.key };
        let text = if selected { pal.sel_text } else { pal.text };
        round_rect(pixmap, left, y * scale, chip_w, h * scale, 10.0 * scale, fill_color, 255);
        draw_text(
            pixmap,
            fonts,
            cache,
            word,
            left,
            y * scale,
            chip_w,
            h * scale,
            16.0 * scale,
            text,
        );
    }
}

fn draw_legend(
    pixmap: &mut Pixmap,
    fonts: &mut FontSystem,
    cache: &mut SwashCache,
    ps5: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    scale: f32,
    pal: &Palette,
) {
    let cross = if ps5 {
        include_str!("../controller-icons/p5_face_cross_colored.svg")
    } else {
        include_str!("../controller-icons/x_face_a_colored.svg")
    };
    let circle = if ps5 {
        include_str!("../controller-icons/p5_face_circle_colored.svg")
    } else {
        include_str!("../controller-icons/x_face_b_colored.svg")
    };
    let icon = h.min(28.0 * scale);
    blit_svg(pixmap, cross, x, y, icon);
    draw_text(pixmap, fonts, cache, "Type", x + icon + 6.0, y, 80.0 * scale, h, 14.0 * scale, pal.text);
    let second = x + w * 0.28;
    blit_svg(pixmap, circle, second, y, icon);
    draw_text(
        pixmap,
        fonts,
        cache,
        "Delete",
        second + icon + 6.0,
        y,
        100.0 * scale,
        h,
        14.0 * scale,
        pal.text,
    );
}

fn blit_svg(pixmap: &mut Pixmap, svg: &str, x: f32, y: f32, size: f32) {
    let px = size.round().max(8.0) as u32;
    let Ok(tree) = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()) else {
        return;
    };
    let mut icon = match resvg::tiny_skia::Pixmap::new(px, px) {
        Some(icon) => icon,
        None => return,
    };
    let source = tree.size().width().max(tree.size().height()).max(1.0);
    let scale = px as f32 / source;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut icon.as_mut(),
    );
    let dest_x = x.round() as i32;
    let dest_y = y.round() as i32;
    let bytes = icon.data();
    for row in 0..px {
        for col in 0..px {
            let index = ((row * px + col) * 4) as usize;
            let color = Color::rgba(bytes[index], bytes[index + 1], bytes[index + 2], bytes[index + 3]);
            blend(pixmap, dest_x + col as i32, dest_y + row as i32, color);
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

#[cfg(test)]
mod tests {
    use super::draw;
    use crate::config::{KeyboardTheme, VkStyle};
    use crate::vk_nav::{rows_snapshot, KeyPos};

    #[test]
    fn keyboard_frame_is_not_blank() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut cache = cosmic_text::SwashCache::new();
        let frame = super::Frame {
            logical_w: 900.0,
            logical_h: 280.0,
            scale: 1.0,
            scale_w: 1920.0,
            rows: rows_snapshot(),
            sel: KeyPos { row: 0, col: 0 },
            style: VkStyle::Normal,
            theme: KeyboardTheme::default(),
            strip: None,
            pressed: None,
            ps5: true,
        };
        let pixmap = draw(&frame, &mut fonts, &mut cache).expect("pixmap");
        assert!(pixmap.data().iter().any(|byte| *byte != 0));
    }
}

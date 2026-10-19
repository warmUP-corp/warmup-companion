use std::cell::RefCell;
use std::collections::HashMap;

use cosmic_text::{Align as TextAlign, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder,
    PathSegment, Pixmap, PixmapPaint, Point, RadialGradient, Rect as SkRect, SpreadMode, Stroke,
    Transform,
};

use super::{
    Align, Icon, Rect, VAlign, cover_bgra, icon_svg, logo_bgra, shadow_alpha, svg_bgra,
    svg_bgra_scaled,
};

pub(crate) struct Gfx {
    pixmap: RefCell<Pixmap>,
    spare: Option<Pixmap>,
    fonts: RefCell<FontSystem>,
    swash: RefCell<SwashCache>,
    transform: Transform,
    masks: RefCell<Vec<Mask>>,
    icons: HashMap<(Icon, u32, u32), Pixmap>,
    svg_icons: HashMap<(usize, usize, u32, u32), Pixmap>,
    svg_scaled: HashMap<(usize, usize, u32, u32, u32), Pixmap>,
    art: HashMap<String, Pixmap>,
    images: HashMap<String, Pixmap>,
}

fn load_fonts() -> FontSystem {
    let mut db = fontdb::Database::new();
    for path in [
        "/usr/share/fonts/inter/Inter.ttc",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Medium.ttf",
        "/usr/share/fonts/noto/NotoSans-Bold.ttf",
    ] {
        if std::path::Path::new(path).is_file() {
            let _ = db.load_font_file(path);
        }
    }
    db.load_font_data(include_bytes!("../../assets/fonts/Selawik-Regular.ttf").to_vec());
    db.load_font_data(include_bytes!("../../assets/fonts/Selawik-Bold.ttf").to_vec());
    let sans = if db.faces().any(|face| face.families.iter().any(|(name, _)| name == "Inter")) {
        "Inter"
    } else {
        "Selawik"
    };
    db.set_sans_serif_family(sans);
    db.set_serif_family(sans);
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}

fn weight_for(weight: i32) -> Weight {
    Weight(weight.clamp(1, 1000) as u16)
}

fn split_color(c: u32, alpha: f32) -> (u8, u8, u8, u8) {
    (
        (c & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        ((c >> 16) & 0xff) as u8,
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn solid_paint(c: u32, alpha: f32) -> Paint<'static> {
    let (r, g, b, a) = split_color(c, alpha);
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

fn rounded_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<Path> {
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

fn blend_premul(pixmap: &mut Pixmap, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
    if x < 0 || y < 0 || x >= pixmap.width() as i32 || y >= pixmap.height() as i32 {
        return;
    }
    let Some(px) = pixmap.pixel(x as u32, y as u32) else {
        return;
    };
    let src_a = a as f32 / 255.0;
    let inv = 1.0 - src_a;
    let mix = |dst: u8, src: u8| (dst as f32 * inv + src as f32).round().clamp(0.0, 255.0) as u8;
    let a8 = (px.alpha() as f32 * inv + a as f32).round().clamp(0.0, 255.0) as u8;
    let w = pixmap.width() as usize;
    let i = (y as usize * w + x as usize) * 4;
    let data = pixmap.data_mut();
    data[i] = mix(data[i], r);
    data[i + 1] = mix(data[i + 1], g);
    data[i + 2] = mix(data[i + 2], b);
    data[i + 3] = a8;
}

fn blend_straight(pixmap: &mut Pixmap, x: i32, y: i32, pixel: cosmic_text::Color) {
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

fn bgra_to_rgba(mut bgra: Vec<u8>) -> Vec<u8> {
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    bgra
}

impl Gfx {
    pub(crate) fn new(w: u32, h: u32) -> Result<Self, String> {
        let pixmap =
            Pixmap::new(w.max(1), h.max(1)).ok_or("mono_ui pixmap alloc failed")?;
        Ok(Gfx {
            pixmap: RefCell::new(pixmap),
            spare: None,
            fonts: RefCell::new(load_fonts()),
            swash: RefCell::new(SwashCache::new()),
            transform: Transform::identity(),
            masks: RefCell::new(Vec::new()),
            icons: HashMap::new(),
            svg_icons: HashMap::new(),
            svg_scaled: HashMap::new(),
            art: HashMap::new(),
            images: HashMap::new(),
        })
    }

    pub(crate) fn begin(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        let current = self.pixmap.get_mut();
        if current.width() != w || current.height() != h {
            let reuse = self
                .spare
                .take_if(|spare| spare.width() == w && spare.height() == h);
            if let Some(pixmap) = reuse.or_else(|| Pixmap::new(w, h)) {
                self.spare = Some(std::mem::replace(current, pixmap));
            }
        }
        self.pixmap.borrow_mut().fill(Color::from_rgba8(0, 0, 0, 0));
        self.masks.borrow_mut().clear();
        self.transform = Transform::identity();
    }

    pub(crate) fn pixels(&self) -> std::cell::Ref<'_, [u8]> {
        std::cell::Ref::map(self.pixmap.borrow(), |p| p.data())
    }

    pub(crate) fn canvas(&mut self) -> (&mut Pixmap, &mut FontSystem, &mut SwashCache) {
        (self.pixmap.get_mut(), self.fonts.get_mut(), self.swash.get_mut())
    }

    pub(crate) fn width(&self) -> u32 {
        self.pixmap.borrow().width()
    }

    pub(crate) fn height(&self) -> u32 {
        self.pixmap.borrow().height()
    }

    pub(crate) unsafe fn transform(&mut self, scale: f32, dx: f32, dy: f32) {
        self.transform = Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    }

    fn device_rect(&self, r: Rect) -> (f32, f32, f32, f32) {
        let s = self.transform.sx;
        (
            r.x * s + self.transform.tx,
            r.y * s + self.transform.ty,
            (r.x + r.w) * s + self.transform.tx,
            (r.y + r.h) * s + self.transform.ty,
        )
    }

    fn push_mask_path(&self, path: &Path, rule: FillRule, antialiased: bool) {
        let (w, h) = {
            let pixmap = self.pixmap.borrow();
            (pixmap.width(), pixmap.height())
        };
        let mut mask = match self.masks.borrow().last() {
            Some(top) => top.clone(),
            None => {
                let mut full = Mask::new(w, h).unwrap_or_else(|| Mask::new(1, 1).unwrap());
                if let Some(rect) =
                    SkRect::from_xywh(0.0, 0.0, w as f32, h as f32)
                {
                    full.fill_path(
                        &PathBuilder::from_rect(rect),
                        FillRule::Winding,
                        true,
                        Transform::identity(),
                    );
                }
                full
            }
        };
        mask.intersect_path(path, rule, antialiased, self.transform);
        self.masks.borrow_mut().push(mask);
    }

    fn draw_text_runs(
        &self,
        value: &str,
        r: Rect,
        px: f32,
        weight: Weight,
        family: Family,
        c: u32,
        alpha: f32,
        halign: Align,
        valign: VAlign,
        wrap: bool,
    ) {
        if value.is_empty() || px < 1.0 {
            return;
        }
        let s = self.transform.sx;
        if !s.is_finite() || s <= 0.0 {
            return;
        }
        let (x, y, right, bottom) = self.device_rect(r);
        let w = right - x;
        let h = bottom - y;
        if w < 2.0 || h < 2.0 {
            return;
        }
        let metrics = Metrics::new(px * s, px * s);
        let mut fonts = self.fonts.borrow_mut();
        let mut buffer = Buffer::new(&mut fonts, metrics);
        buffer.set_size(&mut fonts, Some(w), Some(h));
        buffer.set_wrap(&mut fonts, if wrap { Wrap::Word } else { Wrap::None });
        let attrs = Attrs::new().family(family).weight(weight);
        buffer.set_text(&mut fonts, value, &attrs, Shaping::Advanced);
        let cosmic_align = match halign {
            Align::Left => TextAlign::Left,
            Align::Center => TextAlign::Center,
            Align::Right => TextAlign::Right,
        };
        for line in &mut buffer.lines {
            line.set_align(Some(cosmic_align));
        }
        buffer.shape_until_scroll(&mut fonts, false);
        let mut block_top = 0.0;
        let mut block_bottom = px * s;
        let mut seen = false;
        for run in buffer.layout_runs() {
            if !seen {
                block_top = run.line_top;
                seen = true;
            }
            block_bottom = run.line_top + run.line_height;
        }
        let block = (block_bottom - block_top).max(1.0);
        let dy = match valign {
            VAlign::Top => -block_top,
            VAlign::Center => (h - block) * 0.5 - block_top,
            VAlign::Bottom => (h - block) - block_top,
        }
        .round() as i32;
        let (cr, cg, cb, ca) = split_color(c, alpha);
        let paint = cosmic_text::Color::rgba(cr, cg, cb, ca);
        let clip = (x as i32, y as i32, right as i32, bottom as i32);
        let mut cache = self.swash.borrow_mut();
        let mut pixmap = self.pixmap.borrow_mut();
        buffer.draw(&mut *fonts, &mut *cache, paint, |gx, gy, gw, gh, pixel| {
            let px0 = x as i32 + gx;
            let py0 = y as i32 + dy + gy;
            if px0 + gw as i32 <= clip.0
                || py0 + gh as i32 <= clip.1
                || px0 >= clip.2
                || py0 >= clip.3
            {
                return;
            }
            blend_straight(&mut pixmap, px0, py0, pixel);
        });
    }

    fn shape_width(
        &self,
        value: &str,
        px: f32,
        weight: Weight,
        family: Family,
    ) -> f32 {
        if value.is_empty() || px < 1.0 {
            return 0.0;
        }
        let metrics = Metrics::new(px, px);
        let mut fonts = self.fonts.borrow_mut();
        let mut buffer = Buffer::new(&mut fonts, metrics);
        buffer.set_size(&mut fonts, None, None);
        buffer.set_wrap(&mut fonts, Wrap::None);
        let attrs = Attrs::new().family(family).weight(weight);
        buffer.set_text(&mut fonts, value, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut fonts, false);
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max)
    }

    fn draw_cached_pixmap(
        &mut self,
        key_pixmap: &Pixmap,
        dst: Rect,
        alpha: f32,
        cubic: bool,
    ) {
        let (w, h) = (key_pixmap.width() as f32, key_pixmap.height() as f32);
        if w < 1.0 || h < 1.0 {
            return;
        }
        let (x, y, right, bottom) = self.device_rect(dst);
        let (dw, dh) = (right - x, bottom - y);
        if dw < 1.0 || dh < 1.0 {
            return;
        }
        let mut paint = PixmapPaint::default();
        paint.opacity = alpha.clamp(0.0, 1.0);
        paint.quality = if cubic {
            FilterQuality::Bicubic
        } else {
            FilterQuality::Bilinear
        };
        let k = dw / w;
        self.pixmap.borrow_mut().draw_pixmap(
            0,
            0,
            key_pixmap.as_ref(),
            &paint,
            Transform::from_row(k, 0.0, 0.0, dh / h, x, y),
            self.masks.borrow().last(),
        );
    }

    fn cached_icon(&mut self, icon: Icon, px: u32, tint: u32) -> Option<Pixmap> {
        if !self.icons.contains_key(&(icon, px, tint)) {
            let bytes = match icon_svg(icon) {
                Some(svg) => svg_bgra(svg, px, tint)?,
                None => logo_bgra(px, tint)?,
            };
            let size = tiny_skia::IntSize::from_wh(px, px)?;
            let owned = Pixmap::from_vec(bgra_to_rgba(bytes), size)?;
            self.icons.insert((icon, px, tint), owned);
        }
        self.icons.get(&(icon, px, tint)).cloned()
    }

    fn cached_svg(&mut self, svg: &'static str, px: u32, tint: u32) -> Option<Pixmap> {
        let key = (svg.as_ptr() as usize, svg.len(), px, tint);
        if !self.svg_icons.contains_key(&key) {
            let bytes = svg_bgra(svg, px, tint)?;
            let size = tiny_skia::IntSize::from_wh(px, px)?;
            let owned = Pixmap::from_vec(bgra_to_rgba(bytes), size)?;
            self.svg_icons.insert(key, owned);
        }
        self.svg_icons.get(&key).cloned()
    }

    fn cached_art(&mut self, key: &str, png: &[u8], w: u32, h: u32, radius: f32) -> Option<Pixmap> {
        if !self.art.contains_key(key) {
            let bytes = cover_bgra(png, w, h, radius)?;
            let size = tiny_skia::IntSize::from_wh(w, h)?;
            let owned = Pixmap::from_vec(bgra_to_rgba(bytes), size)?;
            self.art.insert(key.to_string(), owned);
        }
        self.art.get(key).cloned()
    }

    pub(crate) unsafe fn measure(&self, s: &str, px: f32, bold: bool) -> f32 {
        let weight = weight_for(if bold { 600 } else { 400 });
        self.measure_full(s, px, weight.0 as i32, &["Inter", "Selawik"])
    }

    pub(crate) unsafe fn measure_full(
        &self,
        s: &str,
        px: f32,
        weight: i32,
        families: &[&str],
    ) -> f32 {
        let family = resolve_family(&self.fonts.borrow().db(), families);
        self.shape_width(s, px, weight_for(weight), family)
    }

    pub(crate) unsafe fn text(
        &self,
        s: &str,
        r: Rect,
        px: f32,
        bold: bool,
        c: u32,
        alpha: f32,
        align: Align,
    ) {
        let weight = weight_for(if bold { 600 } else { 400 });
        self.text_full(s, r, px, weight.0 as i32, &["Inter", "Selawik"], c, alpha, align, VAlign::Center, false);
    }

    pub(crate) unsafe fn text_full(
        &self,
        s: &str,
        r: Rect,
        px: f32,
        weight: i32,
        families: &[&str],
        c: u32,
        alpha: f32,
        halign: Align,
        valign: VAlign,
        wrap: bool,
    ) {
        let family = resolve_family(&self.fonts.borrow().db(), families);
        self.draw_text_runs(s, r, px, weight_for(weight), family, c, alpha, halign, valign, wrap);
    }

    pub(crate) unsafe fn text_clip(
        &mut self,
        s: &str,
        r: Rect,
        px: f32,
        weight: i32,
        families: &[&str],
        c: u32,
        alpha: f32,
        halign: Align,
        valign: VAlign,
        wrap: bool,
    ) {
        self.push_clip(r, false);
        self.text_full(s, r, px, weight, families, c, alpha, halign, valign, wrap);
        self.pop_clip();
    }

    pub(crate) unsafe fn fill(&self, r: Rect, radius: f32, c: u32, alpha: f32) {
        let Some(path) = rounded_path(r.x, r.y, r.w, r.h, radius) else {
            return;
        };
        let paint = solid_paint(c, alpha);
        self.pixmap.borrow_mut()
            .fill_path(&path, &paint, FillRule::Winding, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn fill_rect(&mut self, r: Rect, c: u32, alpha: f32) {
        let Some(rect) = SkRect::from_xywh(r.x, r.y, r.w, r.h) else {
            return;
        };
        let paint = solid_paint(c, alpha);
        self.pixmap.borrow_mut()
            .fill_rect(rect, &paint, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn gradient(
        &mut self,
        r: Rect,
        radius: f32,
        c: u32,
        alpha: (f32, f32),
        vertical: bool,
    ) {
        let (r0, g0, b0, a0) = split_color(c, alpha.0);
        let (r1, g1, b1, a1) = split_color(c, alpha.1);
        let (start, end) = if vertical {
            (Point::from_xy(r.x, r.y), Point::from_xy(r.x, r.y + r.h))
        } else {
            (Point::from_xy(r.x, r.y), Point::from_xy(r.x + r.w, r.y))
        };
        let Some(shader) = LinearGradient::new(
            start,
            end,
            vec![
                GradientStop::new(0.0, Color::from_rgba8(r0, g0, b0, a0)),
                GradientStop::new(1.0, Color::from_rgba8(r1, g1, b1, a1)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let Some(path) = rounded_path(r.x, r.y, r.w, r.h, radius) else {
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        self.pixmap.borrow_mut()
            .fill_path(&path, &paint, FillRule::Winding, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn ring(&mut self, r: Rect, radius: f32, width: f32, c: u32, alpha: f32) {
        let half = width / 2.0;
        let Some(path) = rounded_path(
            r.x + half,
            r.y + half,
            r.w - width,
            r.h - width,
            (radius - half).max(0.0),
        ) else {
            return;
        };
        let paint = solid_paint(c, alpha);
        let stroke = Stroke {
            width,
            ..Default::default()
        };
        self.pixmap.borrow_mut()
            .stroke_path(&path, &paint, &stroke, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn circle(&mut self, cx: f32, cy: f32, radius: f32, c: u32) {
        let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
            return;
        };
        let paint = solid_paint(c, 1.0);
        self.pixmap.borrow_mut()
            .fill_path(&path, &paint, FillRule::Winding, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn ellipse(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        width: f32,
        c: u32,
        alpha: f32,
    ) {
        let Some(rect) = SkRect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0) else {
            return;
        };
        let Some(path) = PathBuilder::from_oval(rect) else {
            return;
        };
        let paint = solid_paint(c, alpha);
        let stroke = Stroke {
            width,
            ..Default::default()
        };
        self.pixmap.borrow_mut()
            .stroke_path(&path, &paint, &stroke, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn fill_ellipse(
        &self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        c: u32,
        alpha: f32,
    ) {
        let Some(rect) = SkRect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0) else {
            return;
        };
        let Some(path) = PathBuilder::from_oval(rect) else {
            return;
        };
        let paint = solid_paint(c, alpha);
        self.pixmap.borrow_mut()
            .fill_path(&path, &paint, FillRule::Winding, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        c: u32,
        alpha: f32,
    ) {
        let mut builder = PathBuilder::new();
        builder.move_to(x0, y0);
        builder.line_to(x1, y1);
        let Some(path) = builder.finish() else {
            return;
        };
        let paint = solid_paint(c, alpha);
        let stroke = Stroke {
            width,
            ..Default::default()
        };
        self.pixmap.borrow_mut()
            .stroke_path(&path, &paint, &stroke, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn icon(&mut self, icon: Icon, r: Rect, tint: u32, alpha: f32, scale: f32) {
        let px = (r.w.max(r.h) * scale).round().max(1.0) as u32;
        let Some(card) = self.cached_icon(icon, px, tint) else {
            return;
        };
        self.draw_cached_pixmap(&card, r, alpha, false);
    }

    pub(crate) unsafe fn draw_svg(
        &mut self,
        svg: &'static str,
        px: u32,
        tint: u32,
        dst: Rect,
        alpha: f32,
        cubic: bool,
    ) {
        let Some(card) = self.cached_svg(svg, px, tint) else {
            return;
        };
        self.draw_cached_pixmap(&card, dst, alpha, cubic);
    }

    pub(crate) unsafe fn draw_svg_scaled(
        &mut self,
        svg: &'static str,
        px: u32,
        tint: u32,
        source_px: f32,
        dst: Rect,
        alpha: f32,
    ) {
        let key = (svg.as_ptr() as usize, svg.len(), px, tint, source_px.to_bits());
        if !self.svg_scaled.contains_key(&key) {
            let scale = px as f32 / source_px.max(1.0);
            let Some(bytes) = svg_bgra_scaled(svg, px, tint, scale) else {
                return;
            };
            let Some(size) = tiny_skia::IntSize::from_wh(px, px) else {
                return;
            };
            let Some(owned) = Pixmap::from_vec(bgra_to_rgba(bytes), size) else {
                return;
            };
            self.svg_scaled.insert(key, owned);
        }
        let Some(card) = self.svg_scaled.get(&key).cloned() else {
            return;
        };
        self.draw_cached_pixmap(&card, dst, alpha, false);
    }

    pub(crate) unsafe fn draw_art(
        &mut self,
        key: &str,
        png: &[u8],
        w: u32,
        h: u32,
        radius: f32,
        dst: Rect,
        alpha: f32,
        cubic: bool,
    ) {
        let Some(card) = self.cached_art(key, png, w, h, radius) else {
            return;
        };
        self.draw_cached_pixmap(&card, dst, alpha, cubic);
    }

    pub(crate) unsafe fn set_image(&mut self, key: &str, w: u32, h: u32, rgba_premul: &[u8]) {
        let Some(size) = tiny_skia::IntSize::from_wh(w, h) else {
            return;
        };
        if rgba_premul.len() != (w as usize) * (h as usize) * 4 {
            return;
        }
        if let Some(owned) = Pixmap::from_vec(rgba_premul.to_vec(), size) {
            self.images.insert(key.to_string(), owned);
        }
    }

    pub(crate) unsafe fn draw_image(&mut self, key: &str, dst: Rect, alpha: f32, cubic: bool) {
        let Some(card) = self.images.get(key).cloned() else {
            return;
        };
        self.draw_cached_pixmap(&card, dst, alpha, cubic);
    }

    pub(crate) unsafe fn draw_shadow(&mut self, cards: &[Rect], radius: f32, sigma: f32) {
        let pixmap = self.pixmap.borrow();
        let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
        let s = self.transform.sx;
        let mapped: Vec<Rect> = cards
            .iter()
            .map(|c| Rect {
                x: c.x * s + self.transform.tx,
                y: c.y * s + self.transform.ty,
                w: c.w * s,
                h: c.h * s,
            })
            .collect();
        let bytes = shadow_alpha(w, h, &mapped, radius, sigma);
        let mut rgba = vec![0u8; bytes.len() * 4];
        for (dst, a) in rgba.chunks_exact_mut(4).zip(bytes.iter()) {
            dst[3] = *a;
        }
        let Some(size) = tiny_skia::IntSize::from_wh(w as u32, h as u32) else {
            return;
        };
        let Some(card) = Pixmap::from_vec(rgba, size) else {
            return;
        };
        let mut paint = PixmapPaint::default();
        paint.quality = FilterQuality::Bilinear;
        self.pixmap.borrow_mut().draw_pixmap(
            0,
            0,
            card.as_ref(),
            &paint,
            Transform::identity(),
            self.masks.borrow().last(),
        );
    }

    pub(crate) unsafe fn push_clip(&mut self, r: Rect, antialiased: bool) {
        let Some(rect) = SkRect::from_xywh(r.x, r.y, r.w, r.h) else {
            return;
        };
        let path = PathBuilder::from_rect(rect);
        self.push_mask_path(&path, FillRule::Winding, antialiased);
    }

    pub(crate) unsafe fn pop_clip(&mut self) {
        self.masks.borrow_mut().pop();
    }

    pub(crate) unsafe fn push_shadow_layer(
        &self,
        shape: Rect,
        radius: f32,
        dy: f32,
        blur: f32,
    ) -> Result<(), String> {
        let reach = blur * 2.0 + dy.abs();
        let Some(outer) = SkRect::from_xywh(
            shape.x - reach,
            shape.y - reach,
            shape.w + reach * 2.0,
            shape.h + reach * 2.0,
        ) else {
            return Err("shadow layer rect".into());
        };
        let Some(inner) = rounded_path(shape.x, shape.y, shape.w, shape.h, radius) else {
            return Err("shadow layer shape".into());
        };
        let mut builder = PathBuilder::new();
        rect_contour(&mut builder, outer);
        append_path(&mut builder, &inner);
        let Some(mask_path) = builder.finish() else {
            return Err("shadow layer shape".into());
        };
        self.push_mask_path(&mask_path, FillRule::EvenOdd, true);
        Ok(())
    }

    pub(crate) unsafe fn pop_layer(&self) {
        self.masks.borrow_mut().pop();
    }

    pub(crate) unsafe fn radial_fill(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        stops: &[(f32, u32, f32)],
        alpha: f32,
    ) {
        if stops.is_empty() || rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let points: Vec<GradientStop> = stops
            .iter()
            .map(|(p, c, a)| {
                let (r, g, b, al) = split_color(*c, *a * alpha);
                GradientStop::new(*p, Color::from_rgba8(r, g, b, al))
            })
            .collect();
        let Some(shader) = RadialGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(0.0, 0.0),
            1.0,
            points,
            SpreadMode::Pad,
            Transform::from_row(rx, 0.0, 0.0, ry, cx, cy),
        ) else {
            return;
        };
        let Some(rect) = SkRect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0) else {
            return;
        };
        let Some(path) = PathBuilder::from_oval(rect) else {
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        self.pixmap.borrow_mut()
            .fill_path(&path, &paint, FillRule::Winding, self.transform, self.masks.borrow().last());
    }

    pub(crate) unsafe fn fill_edge_band(
        &self,
        cw: f32,
        ch: f32,
        depth: f32,
        radius: f32,
        c: u32,
        alpha: f32,
    ) -> Result<(), String> {
        let depth = depth.max(1.0);
        let radius = radius.min(depth).min(cw * 0.5).min(ch * 0.5).max(0.0);
        let Some(outer) = SkRect::from_xywh(0.0, 0.0, cw, ch) else {
            return Err("edge band rect".into());
        };
        let mut builder = PathBuilder::new();
        rect_contour(&mut builder, outer);
        if let Some(hole) = rounded_path(depth, depth, cw - depth * 2.0, ch - depth * 2.0, radius) {
            append_path(&mut builder, &hole);
        }
        let Some(path) = builder.finish() else {
            return Err("edge band path".into());
        };
        let paint = solid_paint(c, alpha);
        self.pixmap.borrow_mut()
            .fill_path(&path, &paint, FillRule::EvenOdd, self.transform, self.masks.borrow().last());
        Ok(())
    }

    pub(crate) unsafe fn clear(&mut self, c: u32, alpha: f32) {
        let (w, h) = {
            let pixmap = self.pixmap.borrow();
            (pixmap.width() as f32, pixmap.height() as f32)
        };
        self.fill_rect(Rect { x: 0.0, y: 0.0, w, h }, c, alpha);
    }
}

fn resolve_family<'a>(db: &fontdb::Database, families: &[&'a str]) -> Family<'a> {
    for name in families {
        if db.faces().any(|face| {
            face.families
                .iter()
                .any(|(family, _)| family == name)
        }) {
            return Family::Name(name);
        }
    }
    Family::SansSerif
}

fn rect_contour(builder: &mut PathBuilder, r: SkRect) {
    builder.move_to(r.left(), r.top());
    builder.line_to(r.right(), r.top());
    builder.line_to(r.right(), r.bottom());
    builder.line_to(r.left(), r.bottom());
    builder.close();
}

fn append_path(builder: &mut PathBuilder, path: &Path) {
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => builder.move_to(p.x, p.y),
            PathSegment::LineTo(p) => builder.line_to(p.x, p.y),
            PathSegment::QuadTo(a, b) => builder.quad_to(a.x, a.y, b.x, b.y),
            PathSegment::CubicTo(a, b, c) => builder.cubic_to(a.x, a.y, b.x, b.y, c.x, c.y),
            PathSegment::Close => builder.close(),
        }
    }
}

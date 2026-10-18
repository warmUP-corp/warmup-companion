#[cfg(windows)]
mod win;
#[cfg(windows)]
pub(crate) use win::{Gfx, current_theme};

#[cfg(not(windows))]
mod linux;
#[cfg(not(windows))]
#[allow(unused_imports)]
pub(crate) use linux::Gfx;

#[cfg(windows)]
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

pub(crate) const DIM_ALPHA: f32 = 0x99 as f32 / 255.0;
pub(crate) const SHADOW_ALPHA: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

pub(crate) const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub(crate) fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub(crate) fn inset(&self, d: f32) -> Rect {
        rect(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }

    #[cfg(windows)]
    pub(crate) fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Icon {
    Logo,
    Close,
    Settings,
    Mouse,
    Keyboard,
    Gamepad,
    Battery,
    Charging,
    Vibrate,
    Lightbulb,
    Monitor,
    Download,
    Upload,
    RotateCcw,
    Check,
    ChevronRight,
    ChevronLeft,
    ChevronDown,
    AppWindow,
    Globe,
    Plus,
    Mic,
    Trash,
    Pad(bool, usize),
    Svg(&'static str),
}

pub(crate) fn pad_svg(ps: bool, i: usize) -> &'static str {
    match (ps, i) {
        (_, 12) => include_str!("../../assets/controller-center/dpad-left.svg"),
        (_, 13) => include_str!("../../assets/controller-center/dpad-right.svg"),
        (_, 14) => include_str!("../../assets/controller-center/dpad-up.svg"),
        (_, 15) => include_str!("../../assets/controller-center/dpad-down.svg"),
        (true, 10) => include_str!("../../controller-icons/p5_share.svg"),
        (true, 11) => include_str!("../../controller-icons/p5_options.svg"),
        (false, 10) => include_str!("../../controller-icons/x_menu_view.svg"),
        (false, 11) => include_str!("../../controller-icons/x_menu_menu.svg"),
        (true, 0) => include_str!("../../controller-icons/p5_face_cross_colored.svg"),
        (true, 1) => include_str!("../../controller-icons/p5_face_circle_colored.svg"),
        (true, 2) => include_str!("../../controller-icons/p5_face_square_colored.svg"),
        (true, 3) => include_str!("../../controller-icons/p5_face_triangle_colored.svg"),
        (true, 4) => include_str!("../../controller-icons/p5_shoulder_l1.svg"),
        (true, 5) => include_str!("../../controller-icons/p5_shoulder_r1.svg"),
        (true, 6) => include_str!("../../controller-icons/p5_trigger_l2.svg"),
        (true, 7) => include_str!("../../controller-icons/p5_trigger_r2.svg"),
        (true, 8) => include_str!("../../controller-icons/p5_l3_click.svg"),
        (true, _) => include_str!("../../controller-icons/p5_r3_click.svg"),
        (false, 0) => include_str!("../../controller-icons/x_face_a_colored.svg"),
        (false, 1) => include_str!("../../controller-icons/x_face_b_colored.svg"),
        (false, 2) => include_str!("../../controller-icons/x_face_x_colored.svg"),
        (false, 3) => include_str!("../../controller-icons/x_face_y_colored.svg"),
        (false, 4) => include_str!("../../controller-icons/x_shoulder_lb.svg"),
        (false, 5) => include_str!("../../controller-icons/x_shoulder_rb.svg"),
        (false, 6) => include_str!("../../controller-icons/x_trigger_lt.svg"),
        (false, 7) => include_str!("../../controller-icons/x_trigger_rt.svg"),
        (false, 8) => include_str!("../../controller-icons/x_l3_click.svg"),
        (false, _) => include_str!("../../controller-icons/x_r3_click.svg"),
    }
}

pub(crate) fn icon_svg(icon: Icon) -> Option<&'static str> {
    Some(match icon {
        Icon::Logo => return None,
        Icon::Svg(svg) => svg,
        Icon::Close => include_str!("../../assets/controller-center/x.svg"),
        Icon::Settings => include_str!("../../assets/controller-center/settings.svg"),
        Icon::Mouse => include_str!("../../assets/controller-center/mouse.svg"),
        Icon::Keyboard => include_str!("../../assets/controller-center/keyboard.svg"),
        Icon::Gamepad => include_str!("../../assets/controller-center/gamepad-2.svg"),
        Icon::Battery => include_str!("../../assets/controller-center/battery-medium.svg"),
        Icon::Charging => include_str!("../../assets/controller-center/zap.svg"),
        Icon::Vibrate => include_str!("../../assets/controller-center/vibrate.svg"),
        Icon::Lightbulb => include_str!("../../assets/controller-center/lightbulb.svg"),
        Icon::Monitor => include_str!("../../assets/controller-center/monitor.svg"),
        Icon::Download => include_str!("../../assets/controller-center/download.svg"),
        Icon::Upload => include_str!("../../assets/controller-center/upload.svg"),
        Icon::RotateCcw => include_str!("../../assets/controller-center/rotate-ccw.svg"),
        Icon::Check => include_str!("../../assets/controller-center/check.svg"),
        Icon::ChevronRight => include_str!("../../assets/controller-center/chevron-right.svg"),
        Icon::ChevronLeft => include_str!("../../assets/controller-center/chevron-left.svg"),
        Icon::ChevronDown => include_str!("../../assets/controller-center/chevron-down.svg"),
        Icon::AppWindow => include_str!("../../assets/controller-center/app-window.svg"),
        Icon::Globe => include_str!("../../assets/controller-center/globe.svg"),
        Icon::Plus => include_str!("../../assets/controller-center/plus.svg"),
        Icon::Mic => include_str!("../../assets/controller-center/mic.svg"),
        Icon::Trash => include_str!("../../assets/controller-center/trash-2.svg"),
        Icon::Pad(ps, i) => pad_svg(ps, i),
    })
}

pub(crate) const LOGO_PNG: &[u8] = include_bytes!("../../assets/controller-center/warmup-logo.png");
#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) bg: u32,
    pub(crate) tile: u32,
    pub(crate) hi: u32,
    pub(crate) ring: u32,
    pub(crate) accent: u32,
    pub(crate) on_accent: u32,
    pub(crate) text: u32,
    pub(crate) dim: u32,
    pub(crate) line: u32,
    pub(crate) line_alpha: f32,
    pub(crate) toggle_off: u32,
    pub(crate) idle_dot: u32,
}

pub(crate) fn blur_lines(
    buf: &mut [f32],
    len: usize,
    step: usize,
    lines: usize,
    line_step: usize,
    r: usize,
) {
    let mut prefix = vec![0f32; len + 1];
    let norm = 1.0 / (2 * r + 1) as f32;
    for l in 0..lines {
        let base = l * line_step;
        for i in 0..len {
            prefix[i + 1] = prefix[i] + buf[base + i * step];
        }
        for i in 0..len {
            let lo = i.saturating_sub(r);
            let hi = (i + r + 1).min(len);
            buf[base + i * step] = (prefix[hi] - prefix[lo]) * norm;
        }
    }
}

pub(crate) fn rounded_inside(px: f32, py: f32, r: Rect, radius: f32) -> bool {
    if !r.contains(px, py) {
        return false;
    }
    let cx = px.clamp(r.x + radius, r.x + r.w - radius);
    let cy = py.clamp(r.y + radius, r.y + r.h - radius);
    (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius
}

pub(crate) fn shadow_alpha(w: usize, h: usize, cards: &[Rect], radius: f32, sigma: f32) -> Vec<u8> {
    let mut buf = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            if cards
                .iter()
                .any(|c| rounded_inside(x as f32 + 0.5, y as f32 + 0.5, *c, radius))
            {
                buf[y * w + x] = 1.0;
            }
        }
    }
    let r = sigma.round().max(1.0) as usize;
    for _ in 0..3 {
        blur_lines(&mut buf, w, 1, h, w, r);
        blur_lines(&mut buf, h, w, w, 1, r);
    }
    buf.iter()
        .map(|v| (v * SHADOW_ALPHA * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

pub(crate) fn svg_bgra(svg: &str, px: u32, tint: u32) -> Option<Vec<u8>> {
    let svg = svg.replace(
        "currentColor",
        &format!(
            "#{:02X}{:02X}{:02X}",
            tint & 0xff,
            (tint >> 8) & 0xff,
            (tint >> 16) & 0xff
        ),
    );
    let tree =
        resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let s = px as f32 / size.width().max(size.height());
    let dx = (px as f32 - size.width() * s) / 2.0;
    let dy = (px as f32 - size.height() * s) / 2.0;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(s, s).post_translate(dx, dy),
        &mut pixmap.as_mut(),
    );
    Some(
        pixmap
            .data()
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
    )
}

pub(crate) fn logo_bgra(px: u32, tint: u32) -> Option<Vec<u8>> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(LOGO_PNG))
        .read_info()
        .ok()?;
    let mut data = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut data).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    let (sw, sh) = (info.width as usize, info.height as usize);
    let side = sw.max(sh) as f32;
    let k = side / px as f32;
    let (ox, oy) = ((side - sw as f32) / 2.0, (side - sh as f32) / 2.0);
    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for ty in 0..px {
        for tx in 0..px {
            let (x0, y0) = (tx as f32 * k - ox, ty as f32 * k - oy);
            let mut sum = 0.0;
            let mut n = 0.0;
            for sy in (y0.floor().max(0.0) as usize)..((y0 + k).ceil().max(0.0) as usize).min(sh) {
                for sx in
                    (x0.floor().max(0.0) as usize)..((x0 + k).ceil().max(0.0) as usize).min(sw)
                {
                    sum += data[(sy * sw + sx) * 4 + 3] as f32;
                    n += 1.0;
                }
            }
            let a = if n > 0.0 { sum / n / 255.0 } else { 0.0 };
            let ch = |shift: u32| (((tint >> shift) & 0xff) as f32 * a).round() as u8;
            out.extend_from_slice(&[ch(16), ch(8), ch(0), (a * 255.0).round() as u8]);
        }
    }
    Some(out)
}

pub(crate) fn cover_bgra(png_bytes: &[u8], w: u32, h: u32, radius: f32) -> Option<Vec<u8>> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .ok()?;
    let mut data = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut data).ok()?;
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return None,
    };
    let (sw, sh) = (info.width as usize, info.height as usize);
    let k = (sw as f32 / w as f32).min(sh as f32 / h as f32);
    let (ox, oy) = (
        (sw as f32 - w as f32 * k) / 2.0,
        (sh as f32 - h as f32 * k) / 2.0,
    );
    let mask = rect(0.0, 0.0, w as f32, h as f32 + radius);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for ty in 0..h {
        for tx in 0..w {
            if !rounded_inside(tx as f32 + 0.5, ty as f32 + 0.5, mask, radius) {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let (x0, y0) = (ox + tx as f32 * k, oy + ty as f32 * k);
            let mut sum = [0.0f32; 4];
            let mut n = 0.0;
            for sy in (y0.floor() as usize)..((y0 + k).ceil() as usize).min(sh) {
                for sx in (x0.floor() as usize)..((x0 + k).ceil() as usize).min(sw) {
                    let p = &data[(sy * sw + sx) * ch..];
                    let a = if ch == 4 { p[3] as f32 / 255.0 } else { 1.0 };
                    sum[0] += p[2] as f32 * a;
                    sum[1] += p[1] as f32 * a;
                    sum[2] += p[0] as f32 * a;
                    sum[3] += a;
                    n += 1.0;
                }
            }
            let n = f32::max(n, 1.0);
            out.extend_from_slice(&[
                (sum[0] / n).round() as u8,
                (sum[1] / n).round() as u8,
                (sum[2] / n).round() as u8,
                (sum[3] / n * 255.0).round() as u8,
            ]);
        }
    }
    Some(out)
}

#[derive(Clone, Copy)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum VAlign {
    Top,
    Center,
    Bottom,
}

pub(crate) const FONT_NORMAL: i32 = 400;
pub(crate) const FONT_SEMI_BOLD: i32 = 600;


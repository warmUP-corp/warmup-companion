use std::collections::HashMap;

use windows::core::{w, HSTRING, Interface};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FILL_MODE_ALTERNATE, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
    D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1DeviceContext, ID2D1Factory,
    ID2D1Geometry, ID2D1Layer, ID2D1RenderTarget, ID2D1Resource, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_GAMMA_2_2,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_FAR, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, UpdateLayeredWindow, ULW_ALPHA};
use std::mem::ManuallyDrop;

use crate::win::vk_renderer::{self, VkPalette};

use super::{
    Align, DIM_ALPHA, Icon, Rect, Theme, VAlign, cover_bgra, icon_svg, logo_bgra, rect, shadow_alpha,
    svg_bgra, svg_bgra_scaled, FONT_NORMAL, FONT_SEMI_BOLD,
};

pub(crate) fn theme_from(pal: &VkPalette) -> Theme {
    Theme {
        bg: pal.bg,
        tile: pal.key_action,
        hi: pal.key,
        ring: pal.sel_ring,
        accent: pal.accent,
        on_accent: pal.sel_text,
        text: pal.text,
        dim: pal.text_dim,
        line: pal.panel_stroke,
        line_alpha: pal.panel_stroke_alpha,
        toggle_off: vk_renderer::mix_color(pal.text, pal.key, 0.07),
        idle_dot: vk_renderer::mix_color(pal.text_dim, pal.bg, DIM_ALPHA),
    }
}

pub(crate) fn current_theme() -> Theme {
    let dark = crate::win::vk_ui::is_dark_theme();
    theme_from(&crate::win::vk_ui::vk_palette(
        dark,
        crate::config::VkStyle::Mono,
    ))
}

pub(crate) fn color(c: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (c & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: ((c >> 16) & 0xff) as f32 / 255.0,
        a,
    }
}

pub(crate) struct Gfx {
    pub(crate) dwrite: IDWriteFactory,
    pub(crate) fonts: IDWriteFontCollection,
    pub(crate) family: HSTRING,
    pub(crate) rt: ID2D1RenderTarget,
    pub(crate) device: Option<ID2D1DeviceContext>,
    pub(crate) brush: ID2D1SolidColorBrush,
    pub(crate) formats: HashMap<(u32, i32, String, u8, u8, bool), IDWriteTextFormat>,
    pub(crate) icons: HashMap<(Icon, u32, u32), ID2D1Bitmap>,
    pub(crate) svg_icons: HashMap<(String, u32, u32), ID2D1Bitmap>,
    pub(crate) svg_scaled: HashMap<(String, u32, u32, u32), ID2D1Bitmap>,
    pub(crate) art: HashMap<String, ID2D1Bitmap>,
    pub(crate) images: HashMap<String, ID2D1Bitmap>,
    pub(crate) shadow: Option<((i32, i32, Vec<Rect>), ID2D1Bitmap)>,
    pub(crate) dc_rt: Option<ID2D1DCRenderTarget>,
    pub(crate) dc: HDC,
    pub(crate) bmp: HBITMAP,
    pub(crate) old: HGDIOBJ,
    pub(crate) size: (i32, i32),
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp);
                let _ = DeleteDC(self.dc);
            }
        }
    }
}

impl Gfx {
    pub(crate) unsafe fn new() -> Result<Self, String> {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory: {e}"))?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let dc_rt = factory
            .CreateDCRenderTarget(&props)
            .map_err(|e| format!("CreateDCRenderTarget: {e}"))?;
        dc_rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let brush = dc_rt
            .CreateSolidColorBrush(&color(0, 1.0), None)
            .map_err(|e| format!("CreateSolidColorBrush: {e}"))?;
        let rt: ID2D1RenderTarget = dc_rt
            .cast()
            .map_err(|e| format!("DCRenderTarget as target: {e}"))?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| format!("DWriteCreateFactory: {e}"))?;
        let mut fonts: Option<IDWriteFontCollection> = None;
        dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let family = vk_renderer::resolve_family(&fonts, vk_renderer::MONO_FAMILIES);
        Ok(Gfx {
            dwrite,
            fonts,
            family,
            rt,
            device: None,
            brush,
            formats: HashMap::new(),
            icons: HashMap::new(),
            svg_icons: HashMap::new(),
            svg_scaled: HashMap::new(),
            art: HashMap::new(),
            images: HashMap::new(),
            shadow: None,
            dc_rt: Some(dc_rt),
            dc: HDC::default(),
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            size: (0, 0),
        })
    }

    pub(crate) unsafe fn resolve(&self, families: &[&str]) -> HSTRING {
        for name in families {
            let family = HSTRING::from(*name);
            let mut index = 0u32;
            let mut exists = BOOL(0);
            if self
                .fonts
                .FindFamilyName(&family, &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
            {
                return family;
            }
        }
        HSTRING::from("Segoe UI")
    }

    pub(crate) unsafe fn ensure_surface(&mut self, size: (i32, i32)) -> Result<(), String> {
        if self.size == size && !self.dc.is_invalid() {
            return Ok(());
        }
        if !self.dc.is_invalid() {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
            self.dc = HDC::default();
        }
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0,
                biHeight: -size.1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("CreateDIBSection: {e}"))?;
        self.old = SelectObject(dc, bmp);
        self.dc = dc;
        self.bmp = bmp;
        self.size = size;
        Ok(())
    }

    pub(crate) unsafe fn format_full(
        &mut self,
        px: f32,
        weight: i32,
        family: &str,
        halign: Align,
        valign: VAlign,
        wrap: bool,
    ) -> Option<IDWriteTextFormat> {
        let key = (
            (px * 10.0).round() as u32,
            weight,
            family.to_string(),
            halign as u8,
            valign as u8,
            wrap,
        );
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        let halign_d2d = match halign {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        let valign_d2d = match valign {
            VAlign::Top => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            VAlign::Center => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            VAlign::Bottom => DWRITE_PARAGRAPH_ALIGNMENT_FAR,
        };
        let f = self
            .dwrite
            .CreateTextFormat(
                &HSTRING::from(family),
                &self.fonts,
                DWRITE_FONT_WEIGHT(weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px,
                w!("en-us"),
            )
            .ok()?;
        let _ = f.SetTextAlignment(halign_d2d);
        let _ = f.SetParagraphAlignment(valign_d2d);
        let _ = f.SetWordWrapping(if wrap {
            DWRITE_WORD_WRAPPING_WRAP
        } else {
            DWRITE_WORD_WRAPPING_NO_WRAP
        });
        self.formats.insert(key, f.clone());
        Some(f)
    }

    pub(crate) unsafe fn measure(&mut self, s: &str, px: f32, bold: bool) -> f32 {
        let family = self.family.to_string();
        let weight = if bold { FONT_SEMI_BOLD } else { FONT_NORMAL };
        let family: &str = &family;
        self.measure_full(s, px, weight, &[family])
    }

    pub(crate) unsafe fn measure_full(
        &mut self,
        s: &str,
        px: f32,
        weight: i32,
        families: &[&str],
    ) -> f32 {
        let family = self.resolve(families).to_string();
        let Some(f) =
            self.format_full(px, weight, &family, Align::Center, VAlign::Center, false)
        else {
            return 0.0;
        };
        let text: Vec<u16> = s.encode_utf16().collect();
        let Ok(layout) = self.dwrite.CreateTextLayout(&text, &f, 10_000.0, 100.0) else {
            return 0.0;
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_err() {
            return 0.0;
        }
        m.widthIncludingTrailingWhitespace
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn text(
        &mut self,
        s: &str,
        r: Rect,
        px: f32,
        bold: bool,
        c: u32,
        alpha: f32,
        align: Align,
    ) {
        let family = self.family.to_string();
        let weight = if bold { FONT_SEMI_BOLD } else { FONT_NORMAL };
        let Some(f) = self.format_full(px, weight, &family, align, VAlign::Center, false) else {
            return;
        };
        self.brush.SetColor(&color(c, alpha));
        let text: Vec<u16> = s.encode_utf16().collect();
        self.rt.DrawText(
            &text,
            &f,
            &r.d2d(),
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    pub(crate) unsafe fn fill(&self, r: Rect, radius: f32, c: u32, alpha: f32) {
        self.brush.SetColor(&color(c, alpha));
        self.rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: r.d2d(),
                radiusX: radius,
                radiusY: radius,
            },
            &self.brush,
        );
    }

    pub(crate) unsafe fn gradient(
        &self,
        r: Rect,
        radius: f32,
        c: u32,
        alpha: (f32, f32),
        vertical: bool,
    ) {
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: color(c, alpha.0),
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: color(c, alpha.1),
            },
        ];
        let Ok(stops) =
            self.rt
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
        else {
            return;
        };
        let end = if vertical {
            D2D_POINT_2F {
                x: r.x,
                y: r.y + r.h,
            }
        } else {
            D2D_POINT_2F {
                x: r.x + r.w,
                y: r.y,
            }
        };
        let Ok(brush) = self.rt.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: D2D_POINT_2F { x: r.x, y: r.y },
                endPoint: end,
            },
            None,
            &stops,
        ) else {
            return;
        };
        self.rt.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: r.d2d(),
                radiusX: radius,
                radiusY: radius,
            },
            &brush,
        );
    }

    pub(crate) unsafe fn ring(&self, r: Rect, radius: f32, width: f32, c: u32, alpha: f32) {
        self.brush.SetColor(&color(c, alpha));
        let half = width / 2.0;
        self.rt.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: r.inset(half).d2d(),
                radiusX: (radius - half).max(0.0),
                radiusY: (radius - half).max(0.0),
            },
            &self.brush,
            width,
            None,
        );
    }

    pub(crate) unsafe fn circle(&self, cx: f32, cy: f32, radius: f32, c: u32) {
        self.brush.SetColor(&color(c, 1.0));
        self.rt.FillEllipse(
            &D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: cx, y: cy },
                radiusX: radius,
                radiusY: radius,
            },
            &self.brush,
        );
    }

    pub(crate) unsafe fn icon(&mut self, icon: Icon, r: Rect, tint: u32, alpha: f32, scale: f32) {
        let px = (r.w.max(r.h) * scale).round().max(1.0) as u32;
        let key = (icon, px, tint);
        if !self.icons.contains_key(&key) {
            let pixels = match icon_svg(icon) {
                Some(svg) => svg_bgra(svg, px, tint),
                None => logo_bgra(px, tint),
            };
            let Some(pixels) = pixels else {
                return;
            };
            let Ok(bmp) = self.bitmap(px as i32, px as i32, &pixels) else {
                return;
            };
            self.icons.insert(key, bmp);
        }
        if let Some(bmp) = self.icons.get(&key) {
            self.rt.DrawBitmap(
                bmp,
                Some(&r.d2d()),
                alpha,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    pub(crate) unsafe fn bitmap(&self, w: i32, h: i32, bgra: &[u8]) -> Result<ID2D1Bitmap, String> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        self.rt
            .CreateBitmap(
                D2D_SIZE_U {
                    width: w as u32,
                    height: h as u32,
                },
                Some(bgra.as_ptr() as *const _),
                (w * 4) as u32,
                &props,
            )
            .map_err(|e| format!("CreateBitmap: {e}"))
    }

    pub(crate) unsafe fn draw_shadow(&mut self, cards: &[Rect], radius: f32, sigma: f32) {
        let (w, h) = self.size;
        let key = (w, h, cards.to_vec());
        if !matches!(&self.shadow, Some((k, _)) if *k == key) {
            let alpha = shadow_alpha(w as usize, h as usize, cards, radius, sigma);
            let bgra: Vec<u8> = alpha.iter().flat_map(|a| [0, 0, 0, *a]).collect();
            self.shadow = self.bitmap(w, h, &bgra).ok().map(|b| (key, b));
        }
        if let Some((_, bmp)) = &self.shadow {
            self.rt.DrawBitmap(
                bmp,
                Some(&rect(0.0, 0.0, w as f32, h as f32).d2d()),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    pub(crate) unsafe fn begin(&mut self, size: (i32, i32)) -> Result<(), String> {
        if self.dc_rt.is_none() {
            return Err("begin needs a DC target".into());
        }
        self.ensure_surface(size)?;
        let dc_rt = self.dc_rt.as_ref().ok_or("begin needs a DC target")?;
        let bounds = RECT {
            left: 0,
            top: 0,
            right: size.0,
            bottom: size.1,
        };
        dc_rt
            .BindDC(self.dc, &bounds)
            .map_err(|e| format!("BindDC: {e}"))?;
        self.rt.BeginDraw();
        self.rt.SetTransform(&Matrix3x2::identity());
        self.rt.Clear(Some(&color(0, 0.0)));
        Ok(())
    }

    pub(crate) unsafe fn transform(&self, scale: f32, dx: f32, dy: f32) {
        self.rt.SetTransform(&Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: dx,
            M32: dy,
        });
    }

    pub(crate) unsafe fn present(&mut self, hwnd: HWND, at: Option<POINT>) -> Result<(), String> {
        if self.dc_rt.is_none() {
            return Err("present needs a DC target".into());
        }
        self.rt
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))?;
        let pos = at.unwrap_or_else(|| {
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            POINT {
                x: wr.left,
                y: wr.top,
            }
        });
        let sz = SIZE {
            cx: self.size.0,
            cy: self.size.1,
        };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1,
        };
        let screen = GetDC(None);
        let res = UpdateLayeredWindow(
            hwnd,
            screen,
            Some(&pos),
            Some(&sz),
            self.dc,
            Some(&src),
            None,
            Some(&blend),
            ULW_ALPHA,
        );
        ReleaseDC(None, screen);
        res.map_err(|e| format!("UpdateLayeredWindow: {e}"))
    }

    pub(crate) unsafe fn fill_rect(&self, r: Rect, c: u32, alpha: f32) {
        self.brush.SetColor(&color(c, alpha));
        self.rt.FillRectangle(&r.d2d(), &self.brush);
    }

    pub(crate) unsafe fn ellipse(
        &self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        width: f32,
        c: u32,
        alpha: f32,
    ) {
        self.brush.SetColor(&color(c, alpha));
        self.rt.DrawEllipse(
            &D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: cx, y: cy },
                radiusX: rx,
                radiusY: ry,
            },
            &self.brush,
            width,
            None,
        );
    }

    pub(crate) unsafe fn line(
        &self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        c: u32,
        alpha: f32,
    ) {
        self.brush.SetColor(&color(c, alpha));
        self.rt.DrawLine(
            D2D_POINT_2F { x: x0, y: y0 },
            D2D_POINT_2F { x: x1, y: y1 },
            &self.brush,
            width,
            None,
        );
    }

    pub(crate) unsafe fn draw_bitmap(&self, bmp: &ID2D1Bitmap, dst: Rect, alpha: f32, cubic: bool) {
        if let Some(ctx) = self.device.as_ref() {
            ctx.DrawBitmap(
                bmp,
                Some(&dst.d2d()),
                alpha,
                if cubic {
                    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC
                } else {
                    D2D1_INTERPOLATION_MODE_LINEAR
                },
                None,
                None,
            );
        } else {
            self.rt.DrawBitmap(
                bmp,
                Some(&dst.d2d()),
                alpha,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    pub(crate) unsafe fn push_clip(&self, r: Rect, antialiased: bool) {
        self.rt.PushAxisAlignedClip(
            &r.d2d(),
            if antialiased {
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE
            } else {
                D2D1_ANTIALIAS_MODE_ALIASED
            },
        );
    }

    pub(crate) unsafe fn pop_clip(&self) {
        self.rt.PopAxisAlignedClip();
    }
}

#[allow(dead_code)]
impl Gfx {
    pub(crate) unsafe fn format(&mut self, px: f32, bold: bool) -> Option<IDWriteTextFormat> {
        let family = self.family.to_string();
        let weight = if bold { FONT_SEMI_BOLD } else { FONT_NORMAL };
        self.format_full(px, weight, &family, Align::Center, VAlign::Center, false)
    }

    pub(crate) unsafe fn wrap(device: &ID2D1DeviceContext) -> Result<Self, String> {
        let rt: ID2D1RenderTarget = device
            .cast()
            .map_err(|e| format!("DeviceContext as target: {e}"))?;
        let brush = rt
            .CreateSolidColorBrush(&color(0, 1.0), None)
            .map_err(|e| format!("CreateSolidColorBrush: {e}"))?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| format!("DWriteCreateFactory: {e}"))?;
        let mut fonts: Option<IDWriteFontCollection> = None;
        dwrite
            .GetSystemFontCollection(&mut fonts, false)
            .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
        let fonts = fonts.ok_or("GetSystemFontCollection returned null")?;
        let family = vk_renderer::resolve_family(&fonts, vk_renderer::MONO_FAMILIES);
        Ok(Gfx {
            dwrite,
            fonts,
            family,
            rt,
            device: Some(device.clone()),
            brush,
            formats: HashMap::new(),
            icons: HashMap::new(),
            svg_icons: HashMap::new(),
            svg_scaled: HashMap::new(),
            art: HashMap::new(),
            images: HashMap::new(),
            shadow: None,
            dc_rt: None,
            dc: HDC::default(),
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            size: (0, 0),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn text_full(
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
        let family = self.resolve(families).to_string();
        let Some(f) = self.format_full(px, weight, &family, halign, valign, wrap) else {
            return;
        };
        self.brush.SetColor(&color(c, alpha));
        let text: Vec<u16> = s.encode_utf16().collect();
        self.rt.DrawText(
            &text,
            &f,
            &r.d2d(),
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    pub(crate) unsafe fn begin_draw(&self) {
        self.rt.BeginDraw();
    }

    pub(crate) unsafe fn clear(&self, c: u32, alpha: f32) {
        self.rt.Clear(Some(&color(c, alpha)));
    }

    pub(crate) unsafe fn end_draw(&self) -> Result<(), String> {
        self.rt
            .EndDraw(None, None)
            .map_err(|e| format!("EndDraw: {e}"))
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
        self.brush.SetColor(&color(c, alpha));
        self.rt.FillEllipse(
            &D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: cx, y: cy },
                radiusX: rx,
                radiusY: ry,
            },
            &self.brush,
        );
    }

    pub(crate) unsafe fn svg_bitmap(
        &mut self,
        svg: &str,
        px: u32,
        tint: u32,
    ) -> Option<ID2D1Bitmap> {
        let key = (svg.to_string(), px, tint);
        if !self.svg_icons.contains_key(&key) {
            let bgra = svg_bgra(svg, px, tint)?;
            let bmp = self.bitmap(px as i32, px as i32, &bgra).ok()?;
            self.svg_icons.insert(key.clone(), bmp);
        }
        self.svg_icons.get(&key).cloned()
    }

    pub(crate) unsafe fn draw_svg_scaled(
        &mut self,
        svg: &str,
        px: u32,
        tint: u32,
        source_px: f32,
        dst: Rect,
        alpha: f32,
    ) {
        let key = (svg.to_string(), px, tint, source_px.to_bits());
        if !self.svg_scaled.contains_key(&key) {
            let scale = px as f32 / source_px.max(1.0);
            let Some(bgra) = svg_bgra_scaled(svg, px, tint, scale) else {
                return;
            };
            let Ok(bmp) = self.bitmap(px as i32, px as i32, &bgra) else {
                return;
            };
            self.svg_scaled.insert(key.clone(), bmp);
        }
        let Some(bmp) = self.svg_scaled.get(&key).cloned() else {
            return;
        };
        self.draw_bitmap(&bmp, dst, alpha, false);
    }

    pub(crate) unsafe fn cover_bitmap(
        &mut self,
        key: &str,
        png: &[u8],
        w: u32,
        h: u32,
        radius: f32,
    ) -> Option<ID2D1Bitmap> {
        if !self.art.contains_key(key) {
            let bgra = cover_bgra(png, w, h, radius)?;
            let bmp = self.bitmap(w as i32, h as i32, &bgra).ok()?;
            self.art.insert(key.to_string(), bmp);
        }
        self.art.get(key).cloned()
    }

    pub(crate) unsafe fn draw_svg(
        &mut self,
        svg: &str,
        px: u32,
        tint: u32,
        dst: Rect,
        alpha: f32,
        cubic: bool,
    ) {
        let Some(bmp) = self.svg_bitmap(svg, px, tint) else {
            return;
        };
        self.draw_bitmap(&bmp, dst, alpha, cubic);
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
        let Some(bmp) = self.cover_bitmap(key, png, w, h, radius) else {
            return;
        };
        self.draw_bitmap(&bmp, dst, alpha, cubic);
    }

    pub(crate) unsafe fn set_image(&mut self, key: &str, w: u32, h: u32, rgba: &[u8]) {
        if rgba.len() != (w as usize) * (h as usize) * 4 {
            return;
        }
        let bgra: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        if let Ok(bmp) = self.bitmap(w as i32, h as i32, &bgra) {
            self.images.insert(key.to_string(), bmp);
        }
    }

    pub(crate) unsafe fn draw_image(&mut self, key: &str, dst: Rect, alpha: f32, cubic: bool) {
        let Some(bmp) = self.images.get(key).cloned() else {
            return;
        };
        self.draw_bitmap(&bmp, dst, alpha, cubic);
    }

    pub(crate) unsafe fn push_shadow_layer(
        &self,
        shape: Rect,
        radius: f32,
        dy: f32,
        blur: f32,
    ) -> Result<(), String> {
        let ctx = self
            .device
            .as_ref()
            .ok_or("shadow layer needs a device target")?;
        let resource: ID2D1Resource = ctx.cast().map_err(|e| format!("d2d resource: {e}"))?;
        let factory: ID2D1Factory = resource
            .GetFactory()
            .map_err(|e| format!("d2d factory: {e}"))?;
        let reach = blur * 2.0 + dy.abs();
        let outer = factory
            .CreateRectangleGeometry(&D2D_RECT_F {
                left: shape.x - reach,
                top: shape.y - reach,
                right: shape.x + shape.w + reach,
                bottom: shape.y + shape.h + reach,
            })
            .map_err(|e| format!("shadow rect: {e}"))?;
        let inner = factory
            .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: shape.d2d(),
                radiusX: radius,
                radiusY: radius,
            })
            .map_err(|e| format!("shadow shape: {e}"))?;
        let group = factory
            .CreateGeometryGroup(
                D2D1_FILL_MODE_ALTERNATE,
                &[
                    Some(outer.cast::<ID2D1Geometry>().map_err(|e| format!("{e}"))?),
                    Some(inner.cast::<ID2D1Geometry>().map_err(|e| format!("{e}"))?),
                ],
            )
            .map_err(|e| format!("shadow mask: {e}"))?;
        let mut params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: D2D_RECT_F {
                left: f32::MIN,
                top: f32::MIN,
                right: f32::MAX,
                bottom: f32::MAX,
            },
            geometricMask: ManuallyDrop::new(Some(
                group.cast::<ID2D1Geometry>().map_err(|e| format!("{e}"))?,
            )),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        ctx.PushLayer(&params, None::<&ID2D1Layer>);
        ManuallyDrop::drop(&mut params.geometricMask);
        Ok(())
    }

    pub(crate) unsafe fn pop_layer(&self) {
        if let Some(ctx) = self.device.as_ref() {
            ctx.PopLayer();
        } else {
            self.rt.PopLayer();
        }
    }

    pub(crate) unsafe fn radial_fill(
        &self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        stops: &[(f32, u32, f32)],
        alpha: f32,
    ) {
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|(p, c, a)| D2D1_GRADIENT_STOP {
                position: *p,
                color: color(*c, *a * alpha),
            })
            .collect();
        let Ok(collection) = self.rt.CreateGradientStopCollection(
            &stops,
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        ) else {
            return;
        };
        let Ok(brush) = self.rt.CreateRadialGradientBrush(
            &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center: D2D_POINT_2F { x: cx, y: cy },
                gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
                radiusX: rx,
                radiusY: ry,
            },
            None,
            &collection,
        ) else {
            return;
        };
        self.rt.FillEllipse(
            &D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: cx, y: cy },
                radiusX: rx,
                radiusY: ry,
            },
            &brush,
        );
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
        let resource: ID2D1Resource = self
            .rt
            .cast()
            .map_err(|e| format!("d2d resource: {e}"))?;
        let factory: ID2D1Factory = resource
            .GetFactory()
            .map_err(|e| format!("d2d factory: {e}"))?;
        let brush = self
            .rt
            .CreateSolidColorBrush(&color(c, alpha), None)
            .map_err(|e| format!("CreateSolidColorBrush: {e}"))?;
        let geometry = factory
            .CreatePathGeometry()
            .map_err(|e| format!("path: {e}"))?;
        let sink = geometry.Open().map_err(|e| format!("path open: {e}"))?;
        sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
        let p = |x: f32, y: f32| D2D_POINT_2F { x, y };
        sink.BeginFigure(p(0.0, 0.0), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(p(cw, 0.0));
        sink.AddLine(p(cw, ch));
        sink.AddLine(p(0.0, ch));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        let left = depth;
        let top = depth;
        let right = (cw - depth).max(left + 1.0);
        let bottom = (ch - depth).max(top + 1.0);
        let r = radius;
        sink.BeginFigure(p(left + r, top), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(p(right - r, top));
        let arc = |x: f32, y: f32| D2D1_ARC_SEGMENT {
            point: p(x, y),
            size: D2D_SIZE_F {
                width: r,
                height: r,
            },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        sink.AddArc(&arc(right, top + r));
        sink.AddLine(p(right, bottom - r));
        sink.AddArc(&arc(right - r, bottom));
        sink.AddLine(p(left + r, bottom));
        sink.AddArc(&arc(left, bottom - r));
        sink.AddLine(p(left, top + r));
        sink.AddArc(&arc(left + r, top));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close().map_err(|e| format!("path close: {e}"))?;
        self.rt.FillGeometry(&geometry, &brush, None);
        Ok(())
    }
}

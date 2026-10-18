use std::collections::HashMap;

use windows::core::{w, HSTRING};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
    D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, UpdateLayeredWindow, ULW_ALPHA};

use crate::win::vk_renderer::{self, VkPalette};

use super::{Align, DIM_ALPHA, Icon, Rect, Theme, icon_svg, logo_bgra, rect, shadow_alpha, svg_bgra};

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
    pub(crate) rt: ID2D1DCRenderTarget,
    pub(crate) brush: ID2D1SolidColorBrush,
    pub(crate) formats: HashMap<(u32, bool), IDWriteTextFormat>,
    pub(crate) icons: HashMap<(Icon, u32, u32), ID2D1Bitmap>,
    pub(crate) shadow: Option<((i32, i32, Vec<Rect>), ID2D1Bitmap)>,
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
        let rt = factory
            .CreateDCRenderTarget(&props)
            .map_err(|e| format!("CreateDCRenderTarget: {e}"))?;
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
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
            brush,
            formats: HashMap::new(),
            icons: HashMap::new(),
            shadow: None,
            dc: HDC::default(),
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            size: (0, 0),
        })
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

    pub(crate) unsafe fn format(&mut self, px: f32, bold: bool) -> Option<IDWriteTextFormat> {
        let key = ((px * 10.0).round() as u32, bold);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        let f = self
            .dwrite
            .CreateTextFormat(
                &self.family,
                &self.fonts,
                if bold {
                    DWRITE_FONT_WEIGHT_SEMI_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px,
                w!("en-us"),
            )
            .ok()?;
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        self.formats.insert(key, f.clone());
        Some(f)
    }

    pub(crate) unsafe fn measure(&mut self, s: &str, px: f32, bold: bool) -> f32 {
        let Some(f) = self.format(px, bold) else {
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
        let Some(f) = self.format(px, bold) else {
            return;
        };
        let a: DWRITE_TEXT_ALIGNMENT = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        let _ = f.SetTextAlignment(a);
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
        self.ensure_surface(size)?;
        let bounds = RECT {
            left: 0,
            top: 0,
            right: size.0,
            bottom: size.1,
        };
        self.rt
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
}

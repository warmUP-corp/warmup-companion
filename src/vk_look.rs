use crate::config::{KeyboardTheme, VkStyle};
use crate::vk_paint::{mix_color, VkPalette};

const MONO_ACCENT_TONE: f32 = 0.7;
const MODERN_ACTION_TONE: f32 = 0.08;
const MODERN_ENTER_TONE: f32 = 0.14;
const MODERN_MOD_TONE: f32 = 0.2;
const MODERN_DIM_TONE: f32 = 0.8;
const TV_DIAGONAL_INCH_MIN: f32 = 40.0;
const TV_ASPECT_MAX: f32 = 2.0;

pub fn diagonal_inches_from_mm(width_mm: i32, height_mm: i32) -> f32 {
    if width_mm <= 0 || height_mm <= 0 {
        return 0.0;
    }
    let w = width_mm as f32;
    let h = height_mm as f32;
    (w * w + h * h).sqrt() / 25.4
}

pub fn is_tv(width_mm: i32, height_mm: i32) -> bool {
    if width_mm <= 0 || height_mm <= 0 {
        return false;
    }
    let aspect = width_mm as f32 / height_mm as f32;
    diagonal_inches_from_mm(width_mm, height_mm) >= TV_DIAGONAL_INCH_MIN && aspect <= TV_ASPECT_MAX
}

pub fn themed_palette(mut pal: VkPalette, style: VkStyle, theme: KeyboardTheme) -> VkPalette {
    if style == VkStyle::Modern {
        return modern_themed_palette(pal, theme);
    }
    if style == VkStyle::Mono {
        if let Some(v) = theme.accent {
            pal.accent = mix_color(v, pal.key, MONO_ACCENT_TONE);
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

fn modern_themed_palette(mut pal: VkPalette, theme: KeyboardTheme) -> VkPalette {
    if let Some(v) = theme.bg {
        pal.bg = v;
    }
    if let Some(v) = theme.text {
        pal.text = v;
        pal.enter_text = v;
        pal.text_dim = mix_color(v, pal.bg, MODERN_DIM_TONE);
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
        pal.key_action = mix_color(pal.text, v, MODERN_ACTION_TONE);
    }
    if theme.key.is_some() || theme.accent.is_some() {
        if let Some(tint) = theme.accent.or(pal.mod_tint.map(|(_, bar)| bar)) {
            pal.key_enter = mix_color(tint, pal.key, MODERN_ENTER_TONE);
            pal.mod_tint = Some((mix_color(tint, pal.key, MODERN_MOD_TONE), tint));
        }
    }
    if let Some(v) = theme.border {
        pal.border = v;
        pal.panel_stroke = v;
        pal.panel_stroke_alpha = 1.0;
    }
    pal
}

pub fn is_dark_theme() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    if let Ok(cache) = CACHE.lock() {
        if let Some((at, dark)) = *cache {
            if at.elapsed() < Duration::from_secs(5) {
                return dark;
            }
        }
    }
    let dark = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map_or(true, |out| !String::from_utf8_lossy(&out.stdout).contains("light"));
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), dark));
    }
    dark
}

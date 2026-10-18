use crate::mono_ui::{Theme, DIM_ALPHA};
use crate::vk_paint::{mix_color, style_palette, VkPalette};

const MONO_ACCENT_TONE: f32 = 0.7;
const MODERN_ACTION_TONE: f32 = 0.08;
const MODERN_ENTER_TONE: f32 = 0.14;
const MODERN_MOD_TONE: f32 = 0.2;
const MODERN_DIM_TONE: f32 = 0.8;

pub(crate) fn vk_palette(dark: bool, style: crate::config::VkStyle) -> VkPalette {
    themed_palette(
        style_palette(style, dark),
        style,
        crate::config::keyboard_theme(),
    )
}

pub(crate) fn themed_palette(
    mut pal: VkPalette,
    style: crate::config::VkStyle,
    theme: crate::config::KeyboardTheme,
) -> VkPalette {
    if style == crate::config::VkStyle::Modern {
        return modern_themed_palette(pal, theme);
    }
    if style == crate::config::VkStyle::Mono {
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

fn modern_themed_palette(mut pal: VkPalette, theme: crate::config::KeyboardTheme) -> VkPalette {
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
        toggle_off: mix_color(pal.text, pal.key, 0.07),
        idle_dot: mix_color(pal.text_dim, pal.bg, DIM_ALPHA),
    }
}

#[cfg(not(windows))]
pub(crate) fn is_dark_theme() -> bool {
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

#[cfg(not(windows))]
pub(crate) fn current_theme() -> Theme {
    theme_from(&vk_palette(is_dark_theme(), crate::config::VkStyle::Mono))
}

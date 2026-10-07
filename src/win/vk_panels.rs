use std::cell::RefCell;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_EMERGENCY_BREAK,
};

use super::*;
use crate::emoji_data::CATEGORIES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Clipboard,
    Emoji,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelInput {
    Up,
    Down,
    Left,
    Right,
    PrevTab,
    NextTab,
    Insert,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pick {
    Clip(String),
    Emoji(&'static str),
}

pub const EMOJI_COLS: usize = 12;
const PREVIEW_CHARS: usize = 240;
const HOLD_INITIAL: Duration = Duration::from_millis(250);
const HOLD_REPEAT: Duration = Duration::from_millis(70);

#[derive(Debug, PartialEq, Eq)]
pub struct EmojiRow {
    pub label: Option<&'static str>,
    pub emoji: Vec<&'static str>,
}

pub fn emoji_rows(recent: &[&'static str]) -> Vec<EmojiRow> {
    let mut rows = Vec::new();
    if !recent.is_empty() {
        rows.push(EmojiRow {
            label: Some("Recent"),
            emoji: recent.iter().take(EMOJI_COLS).copied().collect(),
        });
    }
    for cat in &CATEGORIES {
        for (i, chunk) in cat.emoji.chunks(EMOJI_COLS).enumerate() {
            rows.push(EmojiRow {
                label: (i == 0).then_some(cat.name),
                emoji: chunk.to_vec(),
            });
        }
    }
    rows
}

pub fn preview(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(PREVIEW_CHARS)
        .collect()
}

pub fn scroll_top(top: usize, sel: usize, visible: usize) -> usize {
    let visible = visible.max(1);
    if sel < top {
        sel
    } else if sel >= top + visible {
        sel + 1 - visible
    } else {
        top
    }
}

#[derive(Debug)]
pub struct Panel {
    pub open: bool,
    pub tab: Tab,
    pub clip_sel: usize,
    pub clip_top: usize,
    pub emoji_row: usize,
    pub emoji_col: usize,
    pub emoji_top: usize,
    pub recent: Vec<&'static str>,
    held: Option<(PanelInput, Instant)>,
}

impl Panel {
    pub const fn new() -> Self {
        Panel {
            open: false,
            tab: Tab::Clipboard,
            clip_sel: 0,
            clip_top: 0,
            emoji_row: 0,
            emoji_col: 0,
            emoji_top: 0,
            recent: Vec::new(),
            held: None,
        }
    }

    pub fn show(&mut self) {
        self.open = true;
        self.clip_sel = 0;
        self.clip_top = 0;
        self.held = None;
    }

    pub fn step(&mut self, input: PanelInput, clips: &[String]) -> Option<Pick> {
        match (self.tab, input) {
            (_, PanelInput::PrevTab | PanelInput::NextTab) => {
                self.tab = match self.tab {
                    Tab::Clipboard => Tab::Emoji,
                    Tab::Emoji => Tab::Clipboard,
                };
            }
            (Tab::Clipboard, PanelInput::Up) => self.clip_sel = self.clip_sel.saturating_sub(1),
            (Tab::Clipboard, PanelInput::Down) => {
                self.clip_sel = (self.clip_sel + 1).min(clips.len().saturating_sub(1));
            }
            (Tab::Clipboard, PanelInput::Insert) => {
                return clips.get(self.clip_sel).cloned().map(Pick::Clip);
            }
            (Tab::Clipboard, _) => {}
            (Tab::Emoji, input) => return self.step_emoji(input),
        }
        None
    }

    fn step_emoji(&mut self, input: PanelInput) -> Option<Pick> {
        let rows = emoji_rows(&self.recent);
        let last_row = rows.len().saturating_sub(1);
        self.emoji_row = self.emoji_row.min(last_row);
        let len = |r: usize| rows.get(r).map_or(1, |row| row.emoji.len());
        match input {
            PanelInput::Up => self.emoji_row = self.emoji_row.saturating_sub(1),
            PanelInput::Down => self.emoji_row = (self.emoji_row + 1).min(last_row),
            PanelInput::Left if self.emoji_col > 0 => self.emoji_col -= 1,
            PanelInput::Left if self.emoji_row > 0 => {
                self.emoji_row -= 1;
                self.emoji_col = len(self.emoji_row) - 1;
            }
            PanelInput::Right if self.emoji_col + 1 < len(self.emoji_row) => self.emoji_col += 1,
            PanelInput::Right if self.emoji_row < last_row => {
                self.emoji_row += 1;
                self.emoji_col = 0;
            }
            PanelInput::Insert => {
                let pick = rows
                    .get(self.emoji_row)
                    .and_then(|row| row.emoji.get(self.emoji_col))
                    .copied()?;
                self.remember_emoji(pick);
                return Some(Pick::Emoji(pick));
            }
            _ => {}
        }
        self.emoji_col = self.emoji_col.min(len(self.emoji_row) - 1);
        None
    }

    fn remember_emoji(&mut self, pick: &'static str) {
        let had_recent = !self.recent.is_empty();
        self.recent.retain(|e| *e != pick);
        self.recent.insert(0, pick);
        self.recent.truncate(EMOJI_COLS);
        if !had_recent {
            self.emoji_row += 1;
        }
    }

    fn hold(&mut self, input: PanelInput, now: Instant) {
        let repeats = !matches!(
            input,
            PanelInput::Insert | PanelInput::PrevTab | PanelInput::NextTab
        );
        self.held = repeats.then_some((input, now + HOLD_INITIAL));
    }

    fn due(&mut self, now: Instant) -> Option<PanelInput> {
        let (input, at) = self.held?;
        if now < at {
            return None;
        }
        self.held = Some((input, now + HOLD_REPEAT));
        Some(input)
    }
}

static PANEL: Mutex<Panel> = Mutex::new(Panel::new());

fn with<R>(f: impl FnOnce(&mut Panel) -> R) -> R {
    let mut panel = PANEL.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut panel)
}

pub fn is_open() -> bool {
    with(|p| p.open)
}

pub fn close() {
    if with(|p| std::mem::replace(&mut p.open, false)) {
        crate::win::vk_ui::request_repaint();
    }
}

pub fn toggle() {
    if crate::win::logon_focus::is_active() {
        return;
    }
    with(|p| {
        if p.open {
            p.open = false;
        } else {
            p.show();
        }
    });
    crate::win::vk_ui::request_repaint();
}

pub fn press(input: PanelInput) {
    let clips = crate::clipboard_history::items();
    let pick = with(|p| {
        p.hold(input, Instant::now());
        p.step(input, &clips)
    });
    apply(pick);
    crate::win::vk_ui::request_repaint();
}

pub fn release(input: PanelInput) {
    with(|p| {
        if p.held.is_some_and(|(held, _)| held == input) {
            p.held = None;
        }
    });
}

pub fn tick(now: Instant) -> bool {
    let clips = crate::clipboard_history::items();
    let fired = with(|p| {
        if !p.open {
            return None;
        }
        let input = p.due(now)?;
        p.step(input, &clips);
        Some(())
    });
    fired.is_some()
}

fn apply(pick: Option<Pick>) {
    match pick {
        Some(Pick::Clip(text)) => {
            close();
            if crate::clipboard_history::set_text(&text) {
                crate::vk_nav::paste_clipboard();
            }
        }
        Some(Pick::Emoji(e)) => crate::vk_nav::send_text_direct(e),
        None => {}
    }
}

thread_local! {
    static EMOJI_FORMATS: RefCell<HashMap<i32, IDWriteTextFormat>> = RefCell::new(HashMap::new());
}

impl VkRenderer {
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_panel(
        &mut self,
        pal: &VkPalette,
        spec: &StyleSpec,
        fonts: &StyleFonts,
        rects: &[KeyRect],
        unit: f32,
        radius: f32,
        style: VkStyle,
        icons: ControllerIconFamily,
    ) -> Result<(), String> {
        let Some(first) = rects.first() else {
            return Ok(());
        };
        let key_h = first.bottom - first.top;
        let area = D2D_RECT_F {
            left: rects.iter().map(|r| r.left).fold(f32::INFINITY, f32::min),
            top: rects.iter().map(|r| r.top).fold(f32::INFINITY, f32::min),
            right: rects
                .iter()
                .map(|r| r.right)
                .fold(f32::NEG_INFINITY, f32::max),
            bottom: rects
                .iter()
                .map(|r| r.bottom)
                .fold(f32::NEG_INFINITY, f32::max),
        };
        let strip = StripGeom::above(rects, spec, unit);
        let head_h = (key_h * 0.6).min((strip.bottom - strip.top) * 0.8).round();
        let head = D2D_RECT_F {
            top: strip.cy() - head_h * 0.5,
            bottom: strip.cy() + head_h * 0.5,
            ..area
        };
        let body = area;
        let tab = with(|p| p.tab);
        self.draw_panel_tabs(pal, fonts, head, radius, tab, icons)?;
        match tab {
            Tab::Clipboard => {
                self.draw_clip_list(pal, spec, fonts, body, key_h, unit, radius, style)
            }
            Tab::Emoji => self.draw_emoji_grid(pal, spec, body, key_h, unit, radius, style),
        }
    }

    unsafe fn draw_panel_tabs(
        &mut self,
        pal: &VkPalette,
        fonts: &StyleFonts,
        head: D2D_RECT_F,
        radius: f32,
        tab: Tab,
        icons: ControllerIconFamily,
    ) -> Result<(), String> {
        let h = head.bottom - head.top;
        let pad = h * 0.6;
        let labels = [("Clipboard", Tab::Clipboard), ("Emoji", Tab::Emoji)];
        let widths: Vec<f32> = labels
            .iter()
            .map(|(l, _)| self.measure_text(l, &fonts.word) + pad * 2.0)
            .collect();
        let spacing = h * 0.25;
        let total = widths.iter().sum::<f32>() + spacing;
        let cx = (head.left + head.right) * 0.5;
        let mut left = cx - total * 0.5;
        let first_left = left;
        for ((label, which), w) in labels.iter().zip(&widths) {
            let pill = D2D_RECT_F {
                left,
                right: left + w,
                ..head
            };
            let on = *which == tab;
            let (fill, text) = if on {
                (pal.accent, pal.sel_text)
            } else {
                (pal.key_action, pal.text)
            };
            let fill_brush = solid_brush(&self.d2d_context, colorref(fill))?;
            let text_brush = solid_brush(&self.d2d_context, colorref(text))?;
            self.d2d_context
                .FillRoundedRectangle(&rounded(pill, radius.min(h * 0.5)), &fill_brush);
            self.d2d_context.DrawText(
                &wide(label),
                &fonts.word,
                &pill,
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            left += w + spacing;
        }
        let badge = h * 0.9;
        let cy = (head.top + head.bottom) * 0.5;
        let slots = [("LB", first_left - spacing - badge), ("RB", left)];
        for (hint, x) in slots {
            if let Some(icon) = icons.hint_icon(hint) {
                let rect = D2D_RECT_F {
                    left: x,
                    top: cy - badge * 0.5,
                    right: x + badge,
                    bottom: cy + badge * 0.5,
                };
                self.draw_svg_icon(icon, rect, pal.text)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_clip_list(
        &mut self,
        pal: &VkPalette,
        spec: &StyleSpec,
        fonts: &StyleFonts,
        body: D2D_RECT_F,
        key_h: f32,
        unit: f32,
        radius: f32,
        style: VkStyle,
    ) -> Result<(), String> {
        let clips = crate::clipboard_history::items();
        if clips.is_empty() {
            let brush = solid_brush(&self.d2d_context, colorref_alpha(pal.text_dim, 0.8))?;
            self.d2d_context.DrawText(
                &wide("Copied text shows up here"),
                &fonts.word,
                &body,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            return Ok(());
        }
        let gap = spec.gap * unit;
        let row_h = key_h;
        let visible = (((body.bottom - body.top) + gap) / (row_h + gap))
            .floor()
            .max(1.0) as usize;
        let (sel, top) = with(|p| {
            p.clip_sel = p.clip_sel.min(clips.len() - 1);
            p.clip_top = scroll_top(p.clip_top, p.clip_sel, visible);
            (p.clip_sel, p.clip_top)
        });
        let px = (key_h * 0.25).max(11.0);
        let format = self.text_format(style, px)?;
        let mut y = body.top;
        for (i, clip) in clips.iter().enumerate().skip(top).take(visible) {
            let row = D2D_RECT_F {
                top: y,
                bottom: y + row_h,
                ..body
            };
            let on = i == sel;
            let fill = solid_brush(
                &self.d2d_context,
                colorref(if on { pal.accent } else { pal.key }),
            )?;
            self.d2d_context
                .FillRoundedRectangle(&rounded(row, radius), &fill);
            let text = solid_brush(
                &self.d2d_context,
                colorref(if on { pal.sel_text } else { pal.text }),
            )?;
            let inner = deflate(row, key_h * 0.14);
            let preview = wide(&preview(clip));
            let layout = self
                .dwrite
                .CreateTextLayout(
                    &preview,
                    &format,
                    (inner.right - inner.left).max(1.0),
                    (inner.bottom - inner.top).max(1.0),
                )
                .map_err(|e| format!("CreateTextLayout: {e}"))?;
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK);
            let _ = layout.SetMaxHeight(px * 2.7);
            if let Ok(sign) = self.dwrite.CreateEllipsisTrimmingSign(&format) {
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = layout.SetTrimming(&trimming, &sign);
            }
            let cy = (inner.top + inner.bottom) * 0.5;
            self.d2d_context.DrawTextLayout(
                D2D_POINT_2F {
                    x: inner.left,
                    y: cy - px * 1.35,
                },
                &layout,
                &text,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
            if on {
                self.draw_panel_ring(pal, spec, row, unit, radius)?;
            }
            y += row_h + gap;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_emoji_grid(
        &mut self,
        pal: &VkPalette,
        spec: &StyleSpec,
        body: D2D_RECT_F,
        key_h: f32,
        unit: f32,
        radius: f32,
        style: VkStyle,
    ) -> Result<(), String> {
        let (recent, sel_row, sel_col, top) =
            with(|p| (p.recent.clone(), p.emoji_row, p.emoji_col, p.emoji_top));
        let rows = emoji_rows(&recent);
        let width = body.right - body.left;
        let gutter = width * 0.14;
        let cell_w = (width - gutter) / EMOJI_COLS as f32;
        let cell_h = cell_w.min(key_h * 0.8);
        let visible = ((body.bottom - body.top) / cell_h).floor().max(1.0) as usize;
        let sel_row = sel_row.min(rows.len().saturating_sub(1));
        let top = scroll_top(top, sel_row, visible);
        with(|p| p.emoji_top = top);
        let label_px = (key_h * 0.2).max(11.0);
        let label_format = self.text_format(style, label_px)?;
        let emoji_format = self.emoji_format(cell_h * 0.58)?;
        let dim = solid_brush(&self.d2d_context, colorref_alpha(pal.text_dim, 0.75))?;
        let ink = solid_brush(&self.d2d_context, colorref(pal.text))?;
        let accent = solid_brush(&self.d2d_context, colorref(pal.accent))?;
        let cell_radius = radius.min(cell_h * 0.3);
        for (ri, row) in rows.iter().enumerate().skip(top).take(visible) {
            let y = body.top + (ri - top) as f32 * cell_h;
            if let Some(label) = row.label {
                let rect = D2D_RECT_F {
                    left: body.left,
                    top: y,
                    right: body.left + gutter - 8.0 * unit,
                    bottom: y + cell_h,
                };
                let text = wide(label);
                if let Ok(layout) = self.dwrite.CreateTextLayout(
                    &text,
                    &label_format,
                    rect.right - rect.left,
                    cell_h,
                ) {
                    let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                    self.d2d_context.DrawTextLayout(
                        D2D_POINT_2F {
                            x: rect.left,
                            y: rect.top,
                        },
                        &layout,
                        &dim,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    );
                }
            }
            for (ci, e) in row.emoji.iter().enumerate() {
                let x = body.left + gutter + ci as f32 * cell_w;
                let cell = D2D_RECT_F {
                    left: x,
                    top: y,
                    right: x + cell_w,
                    bottom: y + cell_h,
                };
                let on = ri == sel_row && ci == sel_col;
                let inset = deflate(cell, (2.0 * unit).max(1.0));
                if on {
                    self.d2d_context
                        .FillRoundedRectangle(&rounded(inset, cell_radius), &accent);
                }
                self.d2d_context.DrawText(
                    &wide(e),
                    &emoji_format,
                    &cell,
                    &ink,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                if on {
                    self.draw_panel_ring(pal, spec, inset, unit, cell_radius)?;
                }
            }
        }
        Ok(())
    }

    unsafe fn draw_panel_ring(
        &mut self,
        pal: &VkPalette,
        spec: &StyleSpec,
        rect: D2D_RECT_F,
        unit: f32,
        radius: f32,
    ) -> Result<(), String> {
        let ring_w = (spec.sel_ring_w * unit).max(1.0);
        let brush = solid_brush(&self.d2d_context, colorref(pal.sel_ring))?;
        self.d2d_context.DrawRoundedRectangle(
            &rounded(
                deflate(rect, ring_w * 0.5),
                (radius - ring_w * 0.5).max(0.0),
            ),
            &brush,
            ring_w,
            None,
        );
        Ok(())
    }

    unsafe fn emoji_format(&self, px: f32) -> Result<IDWriteTextFormat, String> {
        let key = (px * 4.0).round() as i32;
        if let Some(f) = EMOJI_FORMATS.with(|m| m.borrow().get(&key).cloned()) {
            return Ok(f);
        }
        let f = self
            .dwrite
            .CreateTextFormat(
                w!("Segoe UI Emoji"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px.max(1.0),
                &user_locale_name(),
            )
            .map_err(|e| format!("CreateTextFormat (Segoe UI Emoji): {e}"))?;
        let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        EMOJI_FORMATS.with(|m| m.borrow_mut().insert(key, f.clone()));
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clips(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("clip {i}")).collect()
    }

    #[test]
    fn shoulders_flip_between_the_two_tabs() {
        let mut p = Panel::new();
        p.show();
        assert_eq!(p.tab, Tab::Clipboard);
        p.step(PanelInput::NextTab, &[]);
        assert_eq!(p.tab, Tab::Emoji);
        p.step(PanelInput::PrevTab, &[]);
        assert_eq!(p.tab, Tab::Clipboard);
    }

    #[test]
    fn clipboard_selection_clamps_and_picks() {
        let mut p = Panel::new();
        let c = clips(3);
        p.step(PanelInput::Up, &c);
        assert_eq!(p.clip_sel, 0);
        for _ in 0..5 {
            p.step(PanelInput::Down, &c);
        }
        assert_eq!(p.clip_sel, 2);
        assert_eq!(
            p.step(PanelInput::Insert, &c),
            Some(Pick::Clip("clip 2".into()))
        );
        assert_eq!(Panel::new().step(PanelInput::Insert, &[]), None);
    }

    #[test]
    fn emoji_rows_are_chunked_per_category_with_one_label() {
        let rows = emoji_rows(&[]);
        assert_eq!(rows[0].label, Some("Smileys"));
        assert!(rows
            .iter()
            .all(|r| !r.emoji.is_empty() && r.emoji.len() <= EMOJI_COLS));
        let labels: Vec<&str> = rows.iter().filter_map(|r| r.label).collect();
        assert_eq!(
            labels,
            [
                "Smileys",
                "People",
                "Animals",
                "Food",
                "Activities",
                "Travel",
                "Objects",
                "Symbols"
            ]
        );
        let total: usize = rows.iter().map(|r| r.emoji.len()).sum();
        assert_eq!(
            total,
            CATEGORIES.iter().map(|c| c.emoji.len()).sum::<usize>()
        );
        assert_eq!(emoji_rows(&["🔥"])[0].label, Some("Recent"));
    }

    #[test]
    fn emoji_nav_wraps_rows_and_clamps_columns() {
        let mut p = Panel::new();
        p.tab = Tab::Emoji;
        p.step(PanelInput::Left, &[]);
        assert_eq!((p.emoji_row, p.emoji_col), (0, 0));
        for _ in 0..EMOJI_COLS {
            p.step(PanelInput::Right, &[]);
        }
        assert_eq!((p.emoji_row, p.emoji_col), (1, 0));
        p.step(PanelInput::Left, &[]);
        assert_eq!((p.emoji_row, p.emoji_col), (0, EMOJI_COLS - 1));
        let rows = emoji_rows(&[]);
        let short = rows
            .iter()
            .position(|r| r.emoji.len() < EMOJI_COLS)
            .unwrap();
        p.emoji_row = short - 1;
        p.step(PanelInput::Down, &[]);
        assert_eq!(p.emoji_col, rows[short].emoji.len() - 1);
    }

    #[test]
    fn inserting_an_emoji_fills_recent_and_keeps_the_selection_on_it() {
        let mut p = Panel::new();
        p.tab = Tab::Emoji;
        p.emoji_col = 2;
        let first = emoji_rows(&[])[0].emoji[2];
        assert_eq!(p.step(PanelInput::Insert, &[]), Some(Pick::Emoji(first)));
        assert_eq!(p.recent, [first]);
        assert_eq!(p.step(PanelInput::Insert, &[]), Some(Pick::Emoji(first)));
        assert_eq!(p.recent, [first]);
        p.emoji_row = 0;
        p.emoji_col = 0;
        assert_eq!(p.step(PanelInput::Insert, &[]), Some(Pick::Emoji(first)));
    }

    #[test]
    fn preview_collapses_whitespace_and_caps_length() {
        assert_eq!(preview("  a\r\n\tb   c "), "a b c");
        assert_eq!(preview(&"x".repeat(1000)).chars().count(), PREVIEW_CHARS);
    }

    #[test]
    fn scroll_keeps_the_selection_visible() {
        assert_eq!(scroll_top(0, 2, 4), 0);
        assert_eq!(scroll_top(0, 5, 4), 2);
        assert_eq!(scroll_top(3, 1, 4), 1);
        assert_eq!(scroll_top(0, 0, 0), 0);
    }

    #[test]
    fn panel_key_sits_after_the_mic_and_keeps_the_row_width() {
        for rows in [
            crate::vk_nav::rows_for_test(),
            crate::vk_nav::modern_rows_for_test(),
        ] {
            let bottom = &rows[rows.len() - 1].keys;
            let at = bottom
                .iter()
                .position(|k| matches!(k.action, crate::vk_nav::KeyAction::Panel))
                .expect("panel key");
            assert!(matches!(
                bottom[at - 1].action,
                crate::vk_nav::KeyAction::VoiceInput | crate::vk_nav::KeyAction::Vk(_)
            ));
            let span: f32 = bottom.iter().map(|k| k.span).sum();
            let top: f32 = rows[rows.len() - 2].keys.iter().map(|k| k.span).sum();
            assert_eq!(span, top);
        }
    }

    fn glyph(key: &crate::vk_nav::KeyCell) -> (String, bool) {
        (key.label.clone(), false)
    }

    fn no_hint(_: &crate::vk_nav::KeyCell) -> Option<&'static str> {
        None
    }

    fn write_png(path: &std::path::Path, w: u32, h: u32, bgra: &[u8]) {
        let rgba: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|p| {
                let a = p[3] as u32;
                let over = |c: u8| (c as u32 + 0x20 * (255 - a) / 255).min(255) as u8;
                [over(p[2]), over(p[1]), over(p[0]), 255]
            })
            .collect();
        let file = std::fs::File::create(path).expect("png file");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .expect("png header")
            .write_image_data(&rgba)
            .expect("png data");
    }

    #[test]
    fn renders_both_tabs_in_every_style() {
        let Some(dir) = std::env::var_os("VK_PANEL_PNG_DIR").map(std::path::PathBuf::from) else {
            return;
        };
        crate::clipboard_history::push_for_test("https://example.com/a/very/long/link/that/keeps/going/and/going/past/the/edge/of/the/row/so/it/has/to/wrap");
        crate::clipboard_history::push_for_test(
            "Meeting moved to 3pm.
Bring the slides and the controller.",
        );
        crate::clipboard_history::push_for_test("hunter2 is not a password");
        let looks = [
            (VkStyle::Normal, false, "normal"),
            (VkStyle::Mono, false, "mono"),
            (VkStyle::Modern, false, "modern"),
            (VkStyle::Normal, true, "normal-tv"),
        ];
        for (style, tv, name) in looks {
            let look = VkLook { style, tv };
            let rows = if look.tv_layout() {
                crate::vk_nav::modern_rows_for_test()
            } else {
                crate::vk_nav::rows_for_test()
            };
            let scale = 0.8;
            let scale_w = REF_MON_W * scale;
            let (grid_w, block_h) = grid_size(scale_w, &rows, look);
            let (pad_x, pad_y) = floating_pad(scale, look);
            let chrome = strip_band_height(scale, look);
            let w = grid_w + 2.0 * pad_x;
            let h = chrome + block_h + 2.0 * pad_y;
            let pal = style_palette(style, true);
            for tab in [Tab::Clipboard, Tab::Emoji] {
                with(|p| {
                    p.show();
                    p.tab = tab;
                    p.emoji_row = 1;
                    p.emoji_col = 3;
                });
                let frame = VkFrame {
                    pal: &pal,
                    rows: &rows,
                    sel: KeyPos { row: 1, col: 1 },
                    key_glyph: glyph,
                    key_hint: no_hint,
                    top_inset: chrome,
                    scale_w,
                    candidates: None,
                    floating: true,
                    modifiers: VkModifiers::default(),
                    pressed: None,
                    controller_label: "DualSense Wireless Controller",
                    voice_available: true,
                    voice_active: false,
                    voice_phase: VoicePhase::Listening,
                    voice_level: 0.0,
                    ui_scale: scale,
                    style,
                    tv,
                    shortcut_sheet: false,
                    legend: false,
                };
                let (pw, ph, px) = unsafe {
                    let mut r = VkRenderer::offscreen(w as u32, h as u32).expect("offscreen");
                    r.render_bgra(&frame).expect("render")
                };
                with(|p| p.open = false);
                let tab_name = if tab == Tab::Clipboard {
                    "clip"
                } else {
                    "emoji"
                };
                write_png(
                    &dir.join(format!("panel-{name}-{tab_name}.png")),
                    pw,
                    ph,
                    &px,
                );
            }
        }
    }

    #[test]
    fn held_direction_repeats_but_insert_does_not() {
        let mut p = Panel::new();
        let t = Instant::now();
        p.hold(PanelInput::Down, t);
        assert_eq!(p.due(t), None);
        assert_eq!(p.due(t + HOLD_INITIAL), Some(PanelInput::Down));
        assert_eq!(p.due(t + HOLD_INITIAL), None);
        assert_eq!(
            p.due(t + HOLD_INITIAL + HOLD_REPEAT),
            Some(PanelInput::Down)
        );
        p.hold(PanelInput::Insert, t);
        assert_eq!(p.due(t + HOLD_INITIAL), None);
    }
}

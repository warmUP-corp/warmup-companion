use crate::vk_nav::{KeyPos, KeyRow};

pub const REF_MON_W: f32 = 1920.0;
const REF_KEY_W: f32 = 92.0;

pub struct KeyRect {
    pub pos: KeyPos,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

pub struct StyleMetrics {
    pub key_aspect: f32,
    pub gap: f32,
    pub design_kh: f32,
    pub key_radius: f32,
    pub pad_x: f32,
    pub pad_y: f32,
}

pub fn style_metrics(mono: bool) -> StyleMetrics {
    if mono {
        StyleMetrics {
            key_aspect: 100.0 / 137.0,
            gap: 8.0,
            design_kh: 100.0,
            key_radius: 12.0,
            pad_x: 13.0,
            pad_y: 13.0,
        }
    } else {
        StyleMetrics {
            key_aspect: 102.0 / 138.0,
            gap: 6.0,
            design_kh: 102.0,
            key_radius: 12.0,
            pad_x: 17.0,
            pad_y: 18.0,
        }
    }
}

pub fn grid_size(
    scale_w: f32,
    rows: &[KeyRow],
    key_aspect: f32,
    gap_design: f32,
    design_kh: f32,
) -> (f32, f32) {
    let (kw, kh, gap) = key_metrics(scale_w, f32::INFINITY, rows, 0.0, key_aspect, gap_design, design_kh);
    let grid_w = rows
        .iter()
        .map(|row| row_pixel_width(row, kw, gap))
        .fold(0.0f32, f32::max);
    let n = rows.len() as f32;
    let block_h = n * kh + (n - 1.0).max(0.0) * gap;
    (grid_w, block_h)
}

pub const TV_MAX_HEIGHT_FRAC: f32 = 0.45;

pub fn dock_height_for_full_keys(
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    key_aspect: f32,
    gap_design: f32,
    design_kh: f32,
) -> f32 {
    let (_, block_h) = grid_size(scale_w, rows, key_aspect, gap_design, design_kh);
    let kh = key_metrics(scale_w, f32::INFINITY, rows, 0.0, key_aspect, gap_design, design_kh).1;
    top_inset + block_h + kh * 0.25
}

pub fn tv_capped_dpi(
    dpi: f32,
    full_h: i32,
    rows: &[KeyRow],
    look: crate::vk_paint::VkLook,
    unit_scale: f32,
) -> f32 {
    let spec = crate::vk_paint::style_spec(look);
    let h1 = dock_height_for_full_keys(
        REF_MON_W * unit_scale,
        rows,
        crate::vk_paint::strip_band_height(unit_scale, look),
        spec.key_aspect,
        spec.gap,
        spec.design_kh,
    );
    if h1 > 0.0 {
        dpi.min(TV_MAX_HEIGHT_FRAC * full_h.max(1) as f32 / h1)
    } else {
        dpi
    }
}

pub fn key_rects(
    client_w: f32,
    client_h: f32,
    scale_w: f32,
    rows: &[KeyRow],
    top_inset: f32,
    key_aspect: f32,
    gap_design: f32,
    design_kh: f32,
) -> Vec<KeyRect> {
    let (kw, kh, gap) = key_metrics(
        scale_w,
        client_h,
        rows,
        top_inset,
        key_aspect,
        gap_design,
        design_kh,
    );
    let n = rows.len() as f32;
    let block_h = n * kh + (n - 1.0).max(0.0) * gap;
    let mut top = top_inset + ((client_h - top_inset - block_h) / 2.0).max(0.0);
    let grid_w = rows
        .iter()
        .map(|row| row_pixel_width(row, kw, gap))
        .fold(0.0f32, f32::max);
    let grid_left = (client_w - grid_w) / 2.0;
    let mut out = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        let gaps_w = gap * (row.keys.len().saturating_sub(1) as f32);
        let row_keys_w = (row_pixel_width(row, kw, gap) - gaps_w).max(1.0);
        let scale = (grid_w - gaps_w).max(1.0) / row_keys_w;
        let mut left = grid_left;
        for (ci, key) in row.keys.iter().enumerate() {
            let w = key_width(kw, gap, key.span) * scale;
            out.push(KeyRect {
                pos: KeyPos { row: ri, col: ci },
                left,
                top,
                right: left + w,
                bottom: top + kh,
            });
            left += w + gap;
        }
        top += kh + gap;
    }
    out
}

fn key_width(kw: f32, gap: f32, span: f32) -> f32 {
    span * kw + (span - 1.0).max(0.0) * gap
}

fn row_pixel_width(row: &KeyRow, kw: f32, gap: f32) -> f32 {
    row.keys
        .iter()
        .map(|key| key_width(kw, gap, key.span))
        .sum::<f32>()
        + gap * (row.keys.len().saturating_sub(1) as f32)
}

pub fn key_metrics(
    scale_w: f32,
    client_h: f32,
    rows: &[KeyRow],
    top_inset: f32,
    key_aspect: f32,
    gap_design: f32,
    design_kh: f32,
) -> (f32, f32, f32) {
    let scale = (scale_w / REF_MON_W).max(0.05);
    let mut kw = REF_KEY_W * scale;
    let mut kh = kw * key_aspect;
    let mut gap = kh * gap_design / design_kh;
    let n = rows.len().max(1) as f32;
    let avail = (client_h - top_inset - kh * 0.25).max(1.0);
    let block = n * kh + (n - 1.0) * gap;
    if block > avail {
        let fit = avail / block;
        kh *= fit;
        gap *= fit;
        kw = kh / key_aspect;
    }
    (kw, kh, gap)
}

#[cfg(test)]
mod tests {
    use super::{grid_size, key_rects, style_metrics};
    use crate::vk_nav::rows_for_test;

    #[test]
    fn rows_share_one_width_and_do_not_overlap() {
        let rows = rows_for_test();
        let metrics = style_metrics(false);
        let (grid_w, grid_h) = grid_size(
            1920.0,
            &rows,
            metrics.key_aspect,
            metrics.gap,
            metrics.design_kh,
        );
        assert!(grid_w > 100.0);
        assert!(grid_h > 100.0);
        let rects = key_rects(
            grid_w + 40.0,
            grid_h + 40.0,
            1920.0,
            &rows,
            0.0,
            metrics.key_aspect,
            metrics.gap,
            metrics.design_kh,
        );
        let keys: usize = rows.iter().map(|row| row.keys.len()).sum();
        assert_eq!(rects.len(), keys);
        for pair in rects.windows(2) {
            if pair[0].pos.row == pair[1].pos.row {
                assert!(pair[0].right <= pair[1].left + 0.5);
            }
        }
    }
}

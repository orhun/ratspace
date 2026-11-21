use ratatui::layout::Rect;
use smithay::utils::{Buffer as BufferCoords, Rectangle};

use crate::font::GlyphMask;

/// Holds one changed terminal cell ready for pixel painting.
pub(super) struct PaintCell {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) columns: u16,
    pub(super) glyph: Option<crate::font::GlyphKey>,
    pub(super) foreground: [u8; 3],
    pub(super) background: [u8; 3],
}

/// Paints a cell background and blends its cached glyph mask.
pub(super) fn paint_cell(
    buffer: &mut [u8],
    pixel_width: usize,
    pixel_height: usize,
    cell_width: usize,
    cell_height: usize,
    paint: &PaintCell,
    glyph: Option<&GlyphMask>,
) {
    let x = usize::from(paint.x) * cell_width;
    let y = usize::from(paint.y) * cell_height;
    let width = cell_width * usize::from(paint.columns);
    fill_rect(
        buffer,
        pixel_width,
        pixel_height,
        x,
        y,
        width,
        cell_height,
        paint.background,
    );
    if let Some(glyph) = glyph {
        blend_mask(
            buffer,
            pixel_width,
            pixel_height,
            x,
            y,
            glyph,
            paint.foreground,
        );
    }
}

/// Fills a clipped rectangle in the retained RGBA buffer.
pub(super) fn fill_rect(
    buffer: &mut [u8],
    pixel_width: usize,
    pixel_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: [u8; 3],
) {
    let right = x.saturating_add(width).min(pixel_width);
    let bottom = y.saturating_add(height).min(pixel_height);
    for row in y.min(pixel_height)..bottom {
        for col in x.min(pixel_width)..right {
            let index = (row * pixel_width + col) * 4;
            buffer[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

/// Blends a glyph coverage mask over a cell's background color.
pub(super) fn blend_mask(
    buffer: &mut [u8],
    pixel_width: usize,
    pixel_height: usize,
    x: usize,
    y: usize,
    glyph: &GlyphMask,
    foreground: [u8; 3],
) {
    for mask_y in 0..glyph.height {
        let target_y = y + mask_y;
        if target_y >= pixel_height {
            break;
        }
        for mask_x in 0..glyph.width {
            let target_x = x + mask_x;
            if target_x >= pixel_width {
                break;
            }
            let coverage = glyph.alpha[mask_y * glyph.width + mask_x] as u32;
            if coverage == 0 {
                continue;
            }
            let index = (target_y * pixel_width + target_x) * 4;
            for channel in 0..3 {
                let background = buffer[index + channel] as u32;
                buffer[index + channel] =
                    ((foreground[channel] as u32 * coverage + background * (255 - coverage) + 127)
                        / 255) as u8;
            }
            buffer[index + 3] = 255;
        }
    }
}

/// Inverts the cell under a block cursor.
pub(super) fn invert_cell(
    buffer: &mut [u8],
    pixel_width: usize,
    pixel_height: usize,
    cell_width: usize,
    cell_height: usize,
    x: u16,
    y: u16,
) {
    let x = usize::from(x) * cell_width;
    let y = usize::from(y) * cell_height;
    for row in y..y.saturating_add(cell_height).min(pixel_height) {
        for col in x..x.saturating_add(cell_width).min(pixel_width) {
            let index = (row * pixel_width + col) * 4;
            buffer[index] = 255 - buffer[index];
            buffer[index + 1] = 255 - buffer[index + 1];
            buffer[index + 2] = 255 - buffer[index + 2];
        }
    }
}

/// Builds damage rectangles for each terminal row that changed.
pub(super) fn damage_rectangles(
    width: i32,
    height: i32,
    cell_height: usize,
    rows: &[u16],
) -> Vec<Rectangle<i32, BufferCoords>> {
    rows.iter()
        .map(|row| {
            let y = i32::from(*row) * cell_height as i32;
            Rectangle::new(
                (0, y).into(),
                (width, (height - y).min(cell_height as i32)).into(),
            )
        })
        .collect()
}

/// Returns the Ratatui cell dimensions for a physical output size.
pub(super) fn cell_area(width: i32, height: i32, cell_width: usize, cell_height: usize) -> Rect {
    Rect::new(
        0,
        0,
        (width.max(1) as usize / cell_width).max(1) as u16,
        (height.max(1) as usize / cell_height).max(1) as u16,
    )
}

/// Adds an in-bounds row to a damage list once.
pub(super) fn add_row(rows: &mut Vec<u16>, row: u16, row_count: u16) {
    if row < row_count && !rows.contains(&row) {
        rows.push(row);
    }
}

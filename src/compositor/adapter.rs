use std::time::Instant;

use cosmic_text::{
    Attrs, Buffer as TextBuffer, Color as TextColor, Family, FontSystem, Metrics, Shaping,
    Style as FontStyle, SwashCache, Weight,
};
use fux_vt::{Mode, Screen};
use ratatui::{
    buffer::{Buffer as RatatuiBuffer, Cell},
    layout::Rect,
    style::{Color, Modifier},
};

use crate::terminal::buffer::Buffer;

pub(super) const CELL_WIDTH: i32 = 16;
pub(super) const CELL_HEIGHT: i32 = 30;

const FONT_SIZE: f32 = 24.0;
const DEFAULT_FG: [u8; 3] = [220, 220, 220];
const DEFAULT_BG: [u8; 3] = [16, 16, 16];

/// Adapts Ratatui terminal cells into an RGBA pixel buffer.
pub(super) struct CellAdapter {
    fonts: FontSystem,
    cache: SwashCache,
}

impl CellAdapter {
    /// Creates a text rasterizer and its glyph cache.
    pub(super) fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    /// Rasterizes the terminal screen at the requested pixel dimensions.
    pub(super) fn render(&mut self, screen: &Screen, width: i32, height: i32) -> Vec<u8> {
        let started = Instant::now();
        let width = width.max(1);
        let height = height.max(1);
        let (cols, rows) = cell_size_for(width, height);
        let area = Rect::new(0, 0, cols, rows);
        let mut cells = RatatuiBuffer::empty(area);
        Buffer::new(screen, area).render(&mut cells);

        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        Self::draw_backgrounds(&cells, &mut pixels, width, height, cols, rows);
        tracing::debug!("rendered terminal cell backgrounds");
        self.draw_text(&cells, &mut pixels, width, height, cols, rows);
        tracing::debug!("rendered terminal glyphs");
        Self::draw_cursor(screen, &mut pixels, width, cols, rows);
        tracing::debug!(
            cols,
            rows,
            elapsed_ms = started.elapsed().as_millis(),
            "rasterized terminal screen"
        );
        pixels
    }

    /// Fills each terminal cell's background color into the pixel buffer.
    fn draw_backgrounds(
        cells: &RatatuiBuffer,
        pixels: &mut [u8],
        width: i32,
        height: i32,
        cols: u16,
        rows: u16,
    ) {
        let mut scanline = vec![0_u8; (width * 4) as usize];
        for pixel in scanline.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 255]);
        }
        for y in 0..height {
            let start = (y * width * 4) as usize;
            let end = start + scanline.len();
            pixels[start..end].copy_from_slice(&scanline);
        }

        for row in 0..rows {
            for col in 0..cols {
                let (_, [red, green, blue]) = cell_colors(&cells[(col, row)]);
                let start = (i32::from(col) * CELL_WIDTH * 4) as usize;
                let end = start + (CELL_WIDTH * 4) as usize;
                for pixel in scanline[start..end].chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[red, green, blue, 255]);
                }
            }

            let first_y = i32::from(row) * CELL_HEIGHT;
            let last_y = (first_y + CELL_HEIGHT).min(height);
            for y in first_y..last_y {
                let start = (y * width * 4) as usize;
                let end = start + scanline.len();
                pixels[start..end].copy_from_slice(&scanline);
            }
        }
    }

    /// Shapes and rasterizes terminal text with its cell styles.
    fn draw_text(
        &mut self,
        cells: &RatatuiBuffer,
        pixels: &mut [u8],
        width: i32,
        height: i32,
        cols: u16,
        rows: u16,
    ) {
        let mut spans: Vec<(String, Attrs<'static>)> = Vec::with_capacity(usize::from(rows) * 4);
        for row in 0..rows {
            let last_text_col =
                (0..cols).rfind(|col| !cells[(*col, row)].symbol().trim().is_empty());
            if let Some(last_text_col) = last_text_col {
                for col in 0..=last_text_col {
                    let cell = &cells[(col, row)];
                    let (foreground, _) = cell_colors(cell);
                    let attrs = Attrs::new()
                        .family(Family::Monospace)
                        .color(TextColor::rgb(foreground[0], foreground[1], foreground[2]))
                        .weight(if cell.modifier.contains(Modifier::BOLD) {
                            Weight::BOLD
                        } else {
                            Weight::NORMAL
                        })
                        .style(if cell.modifier.contains(Modifier::ITALIC) {
                            FontStyle::Italic
                        } else {
                            FontStyle::Normal
                        });
                    if let Some((previous_text, previous_attrs)) = spans.last_mut()
                        && *previous_attrs == attrs
                    {
                        previous_text.push_str(cell.symbol());
                    } else {
                        spans.push((cell.symbol().to_owned(), attrs));
                    }
                }
            }
            if row + 1 < rows {
                spans.push(("\n".to_owned(), Attrs::new().family(Family::Monospace)));
            }
        }

        let mut text =
            TextBuffer::new(&mut self.fonts, Metrics::new(FONT_SIZE, CELL_HEIGHT as f32));
        text.set_size(Some(width as f32), Some(height as f32));
        text.set_monospace_width(Some(CELL_WIDTH as f32));
        text.set_rich_text(
            spans
                .iter()
                .map(|(text, attrs)| (text.as_str(), attrs.clone())),
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        text.draw(
            &mut self.fonts,
            &mut self.cache,
            TextColor::rgb(DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2]),
            |x, y, glyph_width, glyph_height, color| {
                let rgba = color.as_rgba();
                for glyph_y in 0..glyph_height as i32 {
                    for glyph_x in 0..glyph_width as i32 {
                        blend_pixel(
                            pixels,
                            width,
                            height,
                            x + glyph_x,
                            y + glyph_y,
                            [rgba[0], rgba[1], rgba[2], rgba[3]],
                        );
                    }
                }
            },
        );
        Self::draw_cell_lines(cells, pixels, width, height, cols, rows);
    }

    /// Draws underline and strike-through decorations for terminal cells.
    fn draw_cell_lines(
        cells: &RatatuiBuffer,
        pixels: &mut [u8],
        width: i32,
        height: i32,
        cols: u16,
        rows: u16,
    ) {
        for row in 0..rows {
            for col in 0..cols {
                let cell = &cells[(col, row)];
                let underline = cell.modifier.contains(Modifier::UNDERLINED);
                let strike = cell.modifier.contains(Modifier::CROSSED_OUT);
                if underline || strike {
                    let (foreground, _) = cell_colors(cell);
                    let line_y = i32::from(row) * CELL_HEIGHT
                        + if underline {
                            CELL_HEIGHT - 2
                        } else {
                            CELL_HEIGHT / 2
                        };
                    for x in i32::from(col) * CELL_WIDTH..(i32::from(col) + 1) * CELL_WIDTH {
                        set_pixel(pixels, width, height, x, line_y, foreground);
                    }
                }
            }
        }
    }

    /// Inverts the visible terminal cursor cell in the pixel buffer.
    fn draw_cursor(screen: &Screen, pixels: &mut [u8], width: i32, cols: u16, rows: u16) {
        if !screen.mode(Mode::ShowCursor) {
            return;
        }
        let (row, col) = screen.cursor_position();
        if row >= rows || col >= cols {
            return;
        }
        let x = i32::from(col) * CELL_WIDTH;
        let y = i32::from(row) * CELL_HEIGHT;
        for py in y..y + CELL_HEIGHT {
            for px in x..x + CELL_WIDTH {
                let index = ((py * width + px) * 4) as usize;
                for channel in 0..3 {
                    pixels[index + channel] = 255_u8.saturating_sub(pixels[index + channel]);
                }
            }
        }
    }
}

/// Returns the number of terminal cells that fit in a pixel size.
pub(super) fn cell_size_for(width: i32, height: i32) -> (u16, u16) {
    (
        (width.max(1) / CELL_WIDTH).max(1) as u16,
        (height.max(1) / CELL_HEIGHT).max(1) as u16,
    )
}

/// Resolves a cell's foreground and background colors, including reverse video.
fn cell_colors(cell: &Cell) -> ([u8; 3], [u8; 3]) {
    let reversed = cell.modifier.contains(Modifier::REVERSED);
    let mut foreground = color(if reversed { cell.bg } else { cell.fg }, DEFAULT_FG);
    let background = color(if reversed { cell.fg } else { cell.bg }, DEFAULT_BG);
    if cell.modifier.contains(Modifier::DIM) {
        foreground.iter_mut().for_each(|channel| *channel /= 2);
    }
    (foreground, background)
}

/// Converts a Ratatui color into RGB values.
fn color(color: Color, reset: [u8; 3]) -> [u8; 3] {
    match color {
        Color::Reset => reset,
        Color::Black => [0, 0, 0],
        Color::Red => [205, 49, 49],
        Color::Green => [13, 188, 121],
        Color::Yellow => [229, 229, 16],
        Color::Blue => [36, 114, 200],
        Color::Magenta => [188, 63, 188],
        Color::Cyan => [17, 168, 205],
        Color::Gray => [229, 229, 229],
        Color::DarkGray => [102, 102, 102],
        Color::LightRed => [241, 76, 76],
        Color::LightGreen => [35, 209, 139],
        Color::LightYellow => [245, 245, 67],
        Color::LightBlue => [59, 142, 234],
        Color::LightMagenta => [214, 112, 214],
        Color::LightCyan => [41, 184, 219],
        Color::White => [255, 255, 255],
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(index) => indexed_color(index),
    }
}

/// Converts an ANSI indexed color into RGB values.
fn indexed_color(index: u8) -> [u8; 3] {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    if index < 16 {
        return ANSI[index as usize];
    }
    if index < 232 {
        let n = index - 16;
        let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
        return [component(n / 36), component((n / 6) % 6), component(n % 6)];
    }
    let gray = 8 + (index - 232) * 10;
    [gray, gray, gray]
}

/// Writes an opaque RGB pixel when the coordinates are inside the buffer.
fn set_pixel(pixels: &mut [u8], width: i32, height: i32, x: i32, y: i32, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
}

/// Alpha blends an RGBA glyph pixel over the existing buffer contents.
fn blend_pixel(pixels: &mut [u8], width: i32, height: i32, x: i32, y: i32, rgba: [u8; 4]) {
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let index = ((y * width + x) * 4) as usize;
    let alpha = u16::from(rgba[3]);
    for channel in 0..3 {
        pixels[index + channel] = ((u16::from(rgba[channel]) * alpha
            + u16::from(pixels[index + channel]) * (255 - alpha))
            / 255) as u8;
    }
}

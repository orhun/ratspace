use anyhow::{Context, Result, anyhow};
use ratatui::{
    buffer::{Buffer, CellDiffOption},
    style::Modifier,
};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    color::ColorResolver,
    font::{FontRenderer, GlyphKey},
    pixels::{self, PaintCell},
};

/// Rasterizes Ratatui cells into a damage-tracked Smithay memory buffer.
pub struct RatatuiSmithay {
    width: i32,
    height: i32,
    pixels: MemoryRenderBuffer,
    previous_cells: Buffer,
    font: FontRenderer,
    colors: ColorResolver,
    cursor: Option<(u16, u16)>,
    needs_full_redraw: bool,
}

impl RatatuiSmithay {
    /// Creates a renderer with a system font family and terminal colors.
    pub fn new(
        width: i32,
        height: i32,
        font_size: f32,
        font_family: impl Into<String>,
        default_foreground: [u8; 3],
        default_background: [u8; 3],
    ) -> Result<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let font = FontRenderer::new(font_size, font_family)?;
        let area = pixels::cell_area(width, height, font.cell_width(), font.cell_height());

        Ok(Self {
            width,
            height,
            pixels: MemoryRenderBuffer::new(
                Fourcc::Abgr8888,
                (width, height),
                1,
                Transform::Normal,
                None,
            ),
            previous_cells: Buffer::empty(area),
            font,
            colors: ColorResolver::new(default_foreground, default_background),
            cursor: None,
            needs_full_redraw: true,
        })
    }

    /// Returns how many terminal cells fit in the output dimensions.
    pub fn cell_size(&self) -> (u16, u16) {
        self.font.cell_size(self.width, self.height)
    }

    /// Resizes the pixel buffer and resets cell history for a full repaint.
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.pixels.render().resize((self.width, self.height));
        let area = pixels::cell_area(
            self.width,
            self.height,
            self.font.cell_width(),
            self.font.cell_height(),
        );
        self.previous_cells = Buffer::empty(area);
        self.cursor = None;
        self.needs_full_redraw = true;
    }

    /// Draws changed cell rows and updates the cursor overlay.
    pub fn draw(
        &mut self,
        cells: &Buffer,
        dirty_rows: &[u16],
        cursor: Option<(u16, u16)>,
    ) -> Result<()> {
        let (cols, rows) = self.cell_size();
        if cells.area.width != cols || cells.area.height != rows {
            return Err(anyhow!(
                "cell buffer size {}x{} does not match renderer size {}x{}",
                cells.area.width,
                cells.area.height,
                cols,
                rows
            ));
        }

        let cursor = cursor.filter(|(x, y)| *x < cols && *y < rows);
        let previous_cursor = self.cursor;
        let cursor_changed = previous_cursor != cursor;
        let full_redraw = self.needs_full_redraw;
        let mut rows_to_check = if full_redraw {
            (0..rows).collect::<Vec<_>>()
        } else {
            dirty_rows.to_vec()
        };
        if cursor_changed {
            if let Some((_, row)) = previous_cursor {
                pixels::add_row(&mut rows_to_check, row, rows);
            }
            if let Some((_, row)) = cursor {
                pixels::add_row(&mut rows_to_check, row, rows);
            }
        }
        rows_to_check.sort_unstable();
        rows_to_check.dedup();

        let mut paint_cells = Vec::new();
        let mut damaged_rows = Vec::new();
        for &y in &rows_to_check {
            if y >= rows {
                continue;
            }
            for x in 0..cols {
                let cell = &cells[(x, y)];
                let cursor_cell = cursor_changed
                    && (previous_cursor == Some((x, y))
                        || cursor == Some((x, y))
                        || (x + 1 < cols
                            && cell.symbol().width() > 1
                            && (previous_cursor == Some((x + 1, y))
                                || cursor == Some((x + 1, y)))));
                if !full_redraw && !cursor_cell && self.previous_cells[(x, y)] == *cell {
                    continue;
                }

                self.previous_cells[(x, y)] = cell.clone();
                let wide_continuation = x > 0 && cells[(x - 1, y)].symbol().width() > 1;
                if cell.diff_option == CellDiffOption::Skip || wide_continuation {
                    continue;
                }

                let columns = match cell.diff_option {
                    CellDiffOption::ForcedWidth(width) => width.get(),
                    _ => cell.symbol().width().clamp(1, u16::MAX as usize) as u16,
                }
                .min(cols.saturating_sub(x).max(1));
                let (foreground, background) = self.colors.cell_colors(cell);
                let modifiers = cell.modifier;
                let glyph = if modifiers.contains(Modifier::HIDDEN) {
                    None
                } else {
                    Some(GlyphKey {
                        text: Self::decorated_text(
                            cell.symbol(),
                            modifiers.contains(Modifier::UNDERLINED),
                            modifiers.contains(Modifier::CROSSED_OUT),
                        ),
                        bold: modifiers.contains(Modifier::BOLD),
                        italic: modifiers.contains(Modifier::ITALIC),
                        underline: modifiers.contains(Modifier::UNDERLINED),
                        strikeout: modifiers.contains(Modifier::CROSSED_OUT),
                        columns,
                    })
                };
                if let Some(glyph) = &glyph {
                    self.font.ensure_glyph(glyph)?;
                }
                paint_cells.push(PaintCell {
                    x,
                    y,
                    columns,
                    glyph,
                    foreground,
                    background,
                });
                pixels::add_row(&mut damaged_rows, y, rows);
            }
        }

        let cursor_overwritten = cursor.is_some_and(|(x, y)| {
            paint_cells.iter().any(|paint| {
                paint.y == y && x >= paint.x && x < paint.x.saturating_add(paint.columns)
            })
        });
        if cursor_changed {
            if let Some((_, row)) = previous_cursor {
                pixels::add_row(&mut damaged_rows, row, rows);
            }
            if let Some((_, row)) = cursor {
                pixels::add_row(&mut damaged_rows, row, rows);
            }
        } else if cursor_overwritten && let Some((_, row)) = cursor {
            pixels::add_row(&mut damaged_rows, row, rows);
        }

        if full_redraw {
            damaged_rows = (0..rows).collect();
        }
        if paint_cells.is_empty() && damaged_rows.is_empty() {
            self.cursor = cursor;
            return Ok(());
        }

        let damage = pixels::damage_rectangles(
            self.width,
            self.height,
            self.font.cell_height(),
            &damaged_rows,
        );
        let pixel_width = self.width as usize;
        let pixel_height = self.height as usize;
        let cell_width = self.font.cell_width();
        let cell_height = self.font.cell_height();
        let right_edge_backgrounds: Vec<_> = damaged_rows
            .iter()
            .map(|&row| (row, self.colors.cell_colors(&cells[(cols - 1, row)]).1))
            .collect();
        let bottom_edge_backgrounds: Vec<_> = if damaged_rows.contains(&(rows - 1)) {
            (0..cols)
                .map(|col| self.colors.cell_colors(&cells[(col, rows - 1)]).1)
                .collect()
        } else {
            Vec::new()
        };
        let font = &self.font;
        let pixels_buffer = &mut self.pixels;
        pixels_buffer
            .render()
            .draw(|buffer| {
                for paint in &paint_cells {
                    let glyph = paint.glyph.as_ref().and_then(|key| font.glyph(key));
                    pixels::paint_cell(
                        buffer,
                        pixel_width,
                        pixel_height,
                        cell_width,
                        cell_height,
                        paint,
                        glyph,
                    );
                }
                let grid_width = cell_width * usize::from(cols);
                for (row, background) in &right_edge_backgrounds {
                    pixels::fill_rect(
                        buffer,
                        pixel_width,
                        pixel_height,
                        grid_width,
                        usize::from(*row) * cell_height,
                        pixel_width.saturating_sub(grid_width),
                        cell_height,
                        *background,
                    );
                }
                let grid_height = cell_height * usize::from(rows);
                for (col, background) in bottom_edge_backgrounds.iter().enumerate() {
                    pixels::fill_rect(
                        buffer,
                        pixel_width,
                        pixel_height,
                        col * cell_width,
                        grid_height,
                        cell_width,
                        pixel_height.saturating_sub(grid_height),
                        *background,
                    );
                }
                if cursor_changed || cursor_overwritten || full_redraw {
                    if let Some((x, y)) = cursor {
                        pixels::invert_cell(
                            buffer,
                            pixel_width,
                            pixel_height,
                            cell_width,
                            cell_height,
                            x,
                            y,
                        );
                    }
                }
                Ok::<_, anyhow::Error>(damage)
            })
            .context("update Smithay terminal buffer")?;

        self.cursor = cursor;
        self.needs_full_redraw = false;
        Ok(())
    }

    /// Returns the Smithay buffer containing the latest terminal image.
    pub fn pixels(&self) -> &MemoryRenderBuffer {
        &self.pixels
    }

    /// Builds the text string with combining marks used for terminal decorations.
    fn decorated_text(text: &str, underline: bool, strikeout: bool) -> String {
        let mut output = String::with_capacity(text.len() * 2);
        for character in text.chars() {
            output.push(character);
            if strikeout {
                output.push('\u{0336}');
            }
            if underline {
                output.push('\u{0332}');
            }
        }
        output
    }
}

impl std::fmt::Debug for RatatuiSmithay {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RatatuiSmithay")
            .field("size", &(self.width, self.height))
            .field(
                "cell_size",
                &(self.font.cell_width(), self.font.cell_height()),
            )
            .field("font_family", &self.font.font_family())
            .field("font_size", &self.font.font_size())
            .finish_non_exhaustive()
    }
}

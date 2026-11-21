use anyhow::{Context, Result, anyhow};
use fontdb::{Database, Family, Query};
use fux_vt::{Mode, Screen};
use ratatui::{backend::Backend, buffer::Buffer as RatatuiBuffer, layout::Rect};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Rectangle, Transform},
};
use soft_ratatui::{CosmicText, RgbPixmap, SoftBackend};

use crate::{config, terminal::buffer::Buffer};

use super::palette::Palette;

/// Rasterizes changed Ratatui cells into a Smithay memory buffer.
pub(super) struct TerminalRenderer {
    width: i32,
    height: i32,
    pixels: MemoryRenderBuffer,
    rgba: Vec<u8>,
    backend: SoftBackend<CosmicText>,
    palette: Palette,
    previous_cursor: Option<(bool, u16, u16)>,
    needs_full_redraw: bool,
}

impl TerminalRenderer {
    /// Creates a software terminal renderer for the compositor output.
    pub(super) fn new(width: i32, height: i32) -> Result<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let palette = Palette::new()?;
        let font_data = Self::load_monospace_font()?;
        let mut backend =
            SoftBackend::<CosmicText>::new(1, 1, config::TERMINAL_FONT_SIZE as i32, &font_data);
        backend.cursor_config.blink = false;
        let (cols, rows) = Self::cell_size_for(&backend, width, height);
        Self::reset_soft_backend(&mut backend, cols, rows, &palette);

        let mut renderer = Self {
            width,
            height,
            pixels: MemoryRenderBuffer::new(
                Fourcc::Abgr8888,
                (width, height),
                1,
                Transform::Normal,
                None,
            ),
            rgba: vec![0; (width * height * 4) as usize],
            backend,
            palette,
            previous_cursor: None,
            needs_full_redraw: true,
        };
        renderer.fill_background();
        Ok(renderer)
    }

    /// Returns the output size in terminal cells.
    pub(super) fn cell_size(&self) -> (u16, u16) {
        (
            self.backend.buffer.area.width,
            self.backend.buffer.area.height,
        )
    }

    /// Resizes the soft backend and the Smithay output buffer.
    pub(super) fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        let (cols, rows) = Self::cell_size_for(&self.backend, self.width, self.height);
        Self::reset_soft_backend(&mut self.backend, cols, rows, &self.palette);
        self.pixels.render().resize((self.width, self.height));
        self.rgba.resize((self.width * self.height * 4) as usize, 0);
        self.fill_background();
        self.previous_cursor = None;
        self.needs_full_redraw = true;
    }

    /// Updates changed cells and uploads their rows to Smithay.
    pub(super) fn draw(&mut self, screen: &Screen) -> Result<()> {
        let (cols, rows) = self.cell_size();
        let area = Rect::new(0, 0, cols, rows);
        let mut cells = RatatuiBuffer::empty(area);
        Buffer::new(screen, area).render(&mut cells);
        self.palette.resolve_cells(&mut cells);

        let mut changed_cells = Vec::new();
        let mut dirty_rows = Vec::new();
        for row in 0..rows {
            let mut row_changed = false;
            for col in 0..cols {
                if cells[(col, row)] != self.backend.buffer[(col, row)] {
                    changed_cells.push((col, row, &cells[(col, row)]));
                    row_changed = true;
                }
            }
            if row_changed {
                dirty_rows.push(row);
            }
        }
        let cells_changed = !changed_cells.is_empty();
        let blinking_cells = !self.backend.always_redraw_list.is_empty();
        if cells_changed || blinking_cells {
            self.backend.draw(changed_cells.into_iter())?;
        }

        let (row, col) = screen.cursor_position();
        let visible = screen.mode(Mode::ShowCursor) && row < rows && col < cols;
        let cursor = (visible, row, col);
        let cursor_changed = self.previous_cursor != Some(cursor);
        if cursor_changed {
            if let Some((was_visible, previous_row, _)) = self.previous_cursor
                && was_visible
                && previous_row < rows
            {
                Self::add_dirty_row(&mut dirty_rows, previous_row);
            }
            if visible {
                Self::add_dirty_row(&mut dirty_rows, row);
            }
            self.previous_cursor = Some(cursor);
        }
        if cells_changed || blinking_cells || cursor_changed {
            Backend::set_cursor_position(&mut self.backend, (col, row))?;
            if visible {
                Backend::show_cursor(&mut self.backend)?;
            } else {
                Backend::hide_cursor(&mut self.backend)?;
            }
            Backend::flush(&mut self.backend)?;
        }

        for &(_, row) in &self.backend.always_redraw_list {
            if row < rows {
                Self::add_dirty_row(&mut dirty_rows, row);
            }
        }

        if self.needs_full_redraw {
            dirty_rows = (0..rows).collect();
            self.needs_full_redraw = false;
        }
        if dirty_rows.is_empty() {
            return Ok(());
        }

        self.copy_pixmap_rows(&dirty_rows);
        self.upload_rows(&dirty_rows)
    }

    /// Returns the memory buffer used by the terminal layer.
    pub(super) fn pixels(&self) -> &MemoryRenderBuffer {
        &self.pixels
    }

    /// Loads the system's preferred monospace font into owned font data.
    fn load_monospace_font() -> Result<Vec<u8>> {
        let mut fonts = Database::new();
        fonts.load_system_fonts();
        let families = [
            Family::Name(config::TERMINAL_FONT_FAMILY),
            Family::Monospace,
        ];
        let id = fonts
            .query(&Query {
                families: &families,
                ..Query::default()
            })
            .context("find a system monospace font")?;
        fonts
            .with_face_data(id, |data, _| data.to_vec())
            .ok_or_else(|| anyhow!("read system monospace font data"))
    }

    /// Measures how many soft-backend cells fit in the output pixels.
    fn cell_size_for(backend: &SoftBackend<CosmicText>, width: i32, height: i32) -> (u16, u16) {
        let cell_width = backend.char_width.max(1) as i32;
        let cell_height = backend.char_height.max(1) as i32;
        (
            (width / cell_width).max(1) as u16,
            (height / cell_height).max(1) as u16,
        )
    }

    /// Resets the soft backend without rasterizing its temporary empty cells.
    fn reset_soft_backend(
        backend: &mut SoftBackend<CosmicText>,
        cols: u16,
        rows: u16,
        palette: &Palette,
    ) {
        backend.buffer.resize(Rect::new(0, 0, cols, rows));
        backend.buffer.reset();
        for cell in &mut backend.buffer.content {
            cell.fg = palette.foreground();
            cell.bg = palette.background();
        }
        backend.rgb_pixmap = RgbPixmap::new(
            backend.char_width * usize::from(cols),
            backend.char_height * usize::from(rows),
        );
        backend.rgb_pixmap.fill(palette.background_rgb());
        backend.always_redraw_list.clear();
        backend.rendered_cursor = None;
        backend.cursor = false;
    }

    /// Adds a row to the dirty list once.
    fn add_dirty_row(rows: &mut Vec<u16>, row: u16) {
        if !rows.contains(&row) {
            rows.push(row);
        }
    }

    /// Fills the retained output with the configured terminal background.
    fn fill_background(&mut self) {
        let background = self.palette.background_rgb();
        for pixel in self.rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[background[0], background[1], background[2], 255]);
        }
    }

    /// Copies selected RGB pixmap rows into the retained RGBA output.
    fn copy_pixmap_rows(&mut self, rows: &[u16]) {
        let source_width = self.backend.get_pixmap_width();
        let source_height = self.backend.get_pixmap_height();
        let source = self.backend.get_pixmap_data();
        let cell_height = self.backend.char_height;
        let copy_width = source_width.min(self.width as usize);
        let background = self.palette.background_rgb();

        for &row in rows {
            for cell_y in 0..cell_height {
                let y = usize::from(row) * cell_height + cell_y;
                if y >= source_height || y >= self.height as usize {
                    continue;
                }
                let source_start = y * source_width * 3;
                let dest_start = y * self.width as usize * 4;
                let dest_end = dest_start + self.width as usize * 4;
                let destination = &mut self.rgba[dest_start..dest_end];
                for pixel in destination.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[background[0], background[1], background[2], 255]);
                }
                for (destination, source) in destination
                    .chunks_exact_mut(4)
                    .take(copy_width)
                    .zip(source[source_start..source_start + copy_width * 3].chunks_exact(3))
                {
                    destination.copy_from_slice(&[source[0], source[1], source[2], 255]);
                }
            }
        }
    }

    /// Uploads changed output rows and their damage regions to Smithay.
    fn upload_rows(&mut self, rows: &[u16]) -> Result<()> {
        let width = self.width;
        let height = self.height;
        let cell_height = self.backend.char_height as i32;
        let updates: Vec<_> = rows
            .iter()
            .map(|&row| {
                let y = i32::from(row) * cell_height;
                let row_height = (height - y).min(cell_height);
                let start = (y * width * 4) as usize;
                let end = start + (row_height * width * 4) as usize;
                let damage = Rectangle::new((0, y).into(), (width, row_height).into());
                (start, end, damage)
            })
            .collect();

        self.pixels
            .render()
            .draw(|buffer| {
                for (start, end, _) in &updates {
                    buffer[*start..*end].copy_from_slice(&self.rgba[*start..*end]);
                }
                Ok::<_, anyhow::Error>(updates.into_iter().map(|(_, _, rect)| rect).collect())
            })
            .context("update terminal render buffer")
    }
}

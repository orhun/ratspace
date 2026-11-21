use anyhow::Result;
use fux_vt::{Mark, Mode, Screen};
use ratatui::{buffer::Buffer as RatatuiBuffer, layout::Rect};
use ratatui_smithay::RatatuiSmithay;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

use crate::{config, terminal::buffer::Buffer};

use super::palette::Palette;

/// Adapts VT screen changes into the Ratatui and Smithay terminal renderer.
pub(super) struct TerminalRenderer {
    renderer: RatatuiSmithay,
    cells: RatatuiBuffer,
    palette: Palette,
    screen_mark: Option<Mark>,
    reverse_video: Option<bool>,
}

impl TerminalRenderer {
    /// Creates a font renderer and a cell buffer for one compositor output.
    pub(super) fn new(width: i32, height: i32) -> Result<Self> {
        let palette = Palette::new()?;
        let renderer = RatatuiSmithay::new(
            width,
            height,
            config::TERMINAL_FONT_SIZE,
            config::TERMINAL_FONT_FAMILY,
            palette.foreground_rgb(),
            palette.background_rgb(),
        )?;
        let cells = RatatuiBuffer::empty(Self::area(&renderer));

        Ok(Self {
            renderer,
            cells,
            palette,
            screen_mark: None,
            reverse_video: None,
        })
    }

    /// Returns the output size in terminal cells.
    pub(super) fn cell_size(&self) -> (u16, u16) {
        self.renderer.cell_size()
    }

    /// Resizes the Smithay pixel buffer and resets the retained cell state.
    pub(super) fn resize(&mut self, width: i32, height: i32) {
        self.renderer.resize(width, height);
        self.cells = RatatuiBuffer::empty(Self::area(&self.renderer));
        self.screen_mark = None;
        self.reverse_video = None;
    }

    /// Renders changed VT rows into the retained Smithay buffer.
    pub(super) fn draw(&mut self, screen: &Screen) -> Result<()> {
        let (cols, rows) = self.cell_size();
        let area = Rect::new(0, 0, cols, rows);
        let previous_mark = self.screen_mark;
        let reverse_video = screen.mode(Mode::ReverseVideo);
        let full_refresh = previous_mark
            .is_none_or(|mark| screen.full_refresh_since(mark) || screen.scrolled_since(mark) > 0)
            || self.reverse_video != Some(reverse_video);
        let window = screen.window();
        let changed_rows: Vec<_> = (0..rows)
            .filter(|row| {
                full_refresh
                    || previous_mark.is_some_and(|mark| {
                        window.row(*row).is_some_and(|row| row.changed_since(mark))
                    })
            })
            .collect();

        if !changed_rows.is_empty() {
            Buffer::new(screen, area).render_rows(&changed_rows, &mut self.cells);
            self.palette.resolve_rows(&mut self.cells, &changed_rows);
        }
        self.renderer.draw(
            &self.cells,
            &changed_rows,
            Self::visible_cursor(screen, cols, rows),
        )?;
        self.screen_mark = Some(screen.mark());
        self.reverse_video = Some(reverse_video);
        Ok(())
    }

    /// Returns the memory buffer used by the compositor output.
    pub(super) fn pixels(&self) -> &MemoryRenderBuffer {
        self.renderer.pixels()
    }

    /// Returns the terminal cell area matching the renderer's current size.
    fn area(renderer: &RatatuiSmithay) -> Rect {
        let (cols, rows) = renderer.cell_size();
        Rect::new(0, 0, cols, rows)
    }

    /// Returns the visible cursor in Ratatui x/y order.
    fn visible_cursor(screen: &Screen, cols: u16, rows: u16) -> Option<(u16, u16)> {
        let (row, col) = screen.cursor_position();
        (screen.mode(Mode::ShowCursor) && row < rows && col < cols).then_some((col, row))
    }
}

use anyhow::{Context, Result};
use fux_vt::Mode;
use ratatui::{
    DefaultTerminal,
    layout::{Position, Size},
};

use super::Terminal;

/// Initializes Ratatui and draws the emulated terminal screen.
pub struct Renderer {
    terminal: DefaultTerminal,
}

impl Renderer {
    /// Enters raw mode and the alternate screen through Ratatui.
    pub fn new() -> Result<Self> {
        let mut renderer = Self {
            terminal: ratatui::try_init().context("initialize Ratatui terminal")?,
        };
        renderer
            .terminal
            .clear()
            .context("clear Ratatui terminal")?;
        Ok(renderer)
    }

    /// Returns the host terminal's current column and row counts.
    pub fn size(&self) -> Result<(u16, u16)> {
        let Size { width, height } = self.terminal.size().context("read terminal size")?;
        Ok((width, height))
    }

    /// Draws one VT screen using Ratatui's buffer diffing.
    pub fn draw(&mut self, terminal: &Terminal) -> Result<()> {
        let screen = terminal.screen();
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                super::buffer::Buffer::new(screen, area).render(frame.buffer_mut());
                if screen.mode(Mode::ShowCursor) {
                    let (row, col) = screen.cursor_position();
                    if col < area.width && row < area.height {
                        frame.set_cursor_position(Position::new(col, row));
                    }
                }
            })
            .context("draw Ratatui frame")?;
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

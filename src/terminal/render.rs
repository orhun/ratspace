use anyhow::{Context, Result};
use fux_vt::{Attributes, Color as VtColor, Mode, Screen, UnderlineStyle};
use ratatui::{
    DefaultTerminal,
    buffer::Buffer,
    layout::{Position, Rect, Size},
    style::{Color, Modifier, Style},
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
                Self::draw_screen(screen, area, frame.buffer_mut());
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

    fn draw_screen(screen: &Screen, area: Rect, buffer: &mut Buffer) {
        let window = screen.window();
        for row in 0..area.height.min(window.rows()) {
            let Some(screen_row) = window.row(row) else {
                continue;
            };
            for col in 0..area.width.min(window.cols()) {
                let Some(cell) = screen_row.cell(usize::from(col)) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }

                let attributes = cell.attributes();
                let symbol = if attributes.hidden() {
                    " "
                } else {
                    cell.shown()
                };
                buffer.set_stringn(
                    area.x + col,
                    area.y + row,
                    symbol,
                    if cell.is_wide() { 2 } else { 1 },
                    Self::style(attributes, screen.mode(Mode::ReverseVideo)),
                );
            }
        }
    }

    fn style(attributes: Attributes, reverse_video: bool) -> Style {
        let style = Style::default()
            .fg(Self::color(attributes.foreground()))
            .bg(Self::color(attributes.background()));
        let mut modifiers = Modifier::empty();
        if attributes.bold() {
            modifiers |= Modifier::BOLD;
        }
        if attributes.dim() {
            modifiers |= Modifier::DIM;
        }
        if attributes.italic() {
            modifiers |= Modifier::ITALIC;
        }
        if attributes.underline_style() != UnderlineStyle::None {
            modifiers |= Modifier::UNDERLINED;
        }
        if attributes.strikeout() {
            modifiers |= Modifier::CROSSED_OUT;
        }
        if attributes.inverse() ^ reverse_video {
            modifiers |= Modifier::REVERSED;
        }
        style.add_modifier(modifiers)
    }

    fn color(color: VtColor) -> Color {
        match color {
            VtColor::Default => Color::Reset,
            VtColor::Idx(index) => Color::Indexed(index),
            VtColor::Rgb(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

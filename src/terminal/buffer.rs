use fux_vt::{Attributes, Color as VtColor, Mode, Screen, UnderlineStyle};
use ratatui::{
    buffer::Buffer as RatatuiBuffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

/// Adapts a VT screen into Ratatui buffer cells.
pub(super) struct Buffer<'a> {
    screen: &'a Screen,
    area: Rect,
}

impl<'a> Buffer<'a> {
    /// Creates a buffer renderer for the given VT screen and area.
    pub(super) fn new(screen: &'a Screen, area: Rect) -> Self {
        Self { screen, area }
    }

    /// Writes the VT screen's cells and styles into a Ratatui buffer.
    pub(super) fn render(&self, buffer: &mut RatatuiBuffer) {
        let window = self.screen.window();
        for row in 0..self.area.height.min(window.rows()) {
            let Some(screen_row) = window.row(row) else {
                continue;
            };
            for col in 0..self.area.width.min(window.cols()) {
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
                    self.area.x + col,
                    self.area.y + row,
                    symbol,
                    if cell.is_wide() { 2 } else { 1 },
                    Self::cell_style(attributes, self.screen.mode(Mode::ReverseVideo)),
                );
            }
        }
    }

    fn cell_style(attributes: Attributes, reverse_video: bool) -> Style {
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

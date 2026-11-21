use ratatui::{
    buffer::Cell,
    style::{Color, Modifier},
};

/// Resolves Ratatui cell styles to terminal RGB colors.
pub(super) struct ColorResolver {
    default_foreground: [u8; 3],
    default_background: [u8; 3],
}

impl ColorResolver {
    /// Creates a resolver with the terminal's default colors.
    pub(super) fn new(default_foreground: [u8; 3], default_background: [u8; 3]) -> Self {
        Self {
            default_foreground,
            default_background,
        }
    }

    /// Returns resolved foreground and background colors for a cell.
    pub(super) fn cell_colors(&self, cell: &Cell) -> ([u8; 3], [u8; 3]) {
        let mut foreground = Self::rgb(cell.fg, self.default_foreground);
        let mut background = Self::rgb(cell.bg, self.default_background);
        if cell.modifier.contains(Modifier::REVERSED) {
            std::mem::swap(&mut foreground, &mut background);
        }
        if cell.modifier.contains(Modifier::DIM) {
            foreground = [foreground[0] / 2, foreground[1] / 2, foreground[2] / 2];
        }
        if cell.modifier.contains(Modifier::HIDDEN) {
            foreground = background;
        }
        (foreground, background)
    }

    /// Resolves Ratatui colors to RGB values with the supplied default.
    fn rgb(color: Color, reset: [u8; 3]) -> [u8; 3] {
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
            Color::Rgb(red, green, blue) => [red, green, blue],
            Color::Indexed(index) => Self::indexed_rgb(index),
        }
    }

    /// Resolves an xterm indexed color to RGB.
    fn indexed_rgb(index: u8) -> [u8; 3] {
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
}

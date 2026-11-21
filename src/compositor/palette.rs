use anyhow::{Context, Result, anyhow};
use ratatui::{
    buffer::Buffer,
    style::{Color, Modifier},
};

use crate::config;

/// Holds the configured terminal colors used by the compositor.
pub(super) struct Palette {
    foreground: Color,
    background: Color,
    ansi: [Color; 16],
}

impl Palette {
    /// Parses terminal color strings once when the renderer starts.
    pub(super) fn new() -> Result<Self> {
        let foreground = Self::parse(config::TERMINAL_DEFAULT_FG)?;
        let background = Self::parse(config::TERMINAL_DEFAULT_BG)?;
        let ansi: Vec<_> = config::TERMINAL_ANSI_PALETTE
            .iter()
            .map(|hex| Self::parse(hex))
            .collect::<Result<_>>()?;
        let ansi = ansi
            .try_into()
            .map_err(|_| anyhow!("terminal ANSI palette must have 16 colors"))?;

        Ok(Self {
            foreground,
            background,
            ansi,
        })
    }

    /// Returns the configured default foreground color.
    pub(super) fn foreground(&self) -> Color {
        self.foreground
    }

    /// Returns the configured default background color.
    pub(super) fn background(&self) -> Color {
        self.background
    }

    /// Returns the configured default background as RGB components.
    pub(super) fn background_rgb(&self) -> [u8; 3] {
        Self::rgb(self.background)
    }

    /// Resolves Ratatui reset and indexed colors through the terminal palette.
    pub(super) fn resolve_cells(&self, cells: &mut Buffer) {
        for cell in &mut cells.content {
            let mut foreground = self.resolve_color(cell.fg, self.foreground);
            cell.bg = self.resolve_color(cell.bg, self.background);
            if cell.modifier.contains(Modifier::DIM) {
                foreground = Self::dim(foreground);
                cell.modifier.remove(Modifier::DIM);
            }
            cell.fg = foreground;
        }
    }

    /// Uses Ratatui's built-in parser for a configured hex color.
    fn parse(hex: &str) -> Result<Color> {
        hex.parse()
            .with_context(|| format!("parse terminal color {hex}"))
    }

    /// Resolves a Ratatui color through this terminal palette.
    fn resolve_color(&self, color: Color, reset: Color) -> Color {
        match color {
            Color::Reset => reset,
            Color::Black => self.ansi[0],
            Color::Red => self.ansi[1],
            Color::Green => self.ansi[2],
            Color::Yellow => self.ansi[3],
            Color::Blue => self.ansi[4],
            Color::Magenta => self.ansi[5],
            Color::Cyan => self.ansi[6],
            Color::Gray => self.ansi[7],
            Color::DarkGray => self.ansi[8],
            Color::LightRed => self.ansi[9],
            Color::LightGreen => self.ansi[10],
            Color::LightYellow => self.ansi[11],
            Color::LightBlue => self.ansi[12],
            Color::LightMagenta => self.ansi[13],
            Color::LightCyan => self.ansi[14],
            Color::White => self.ansi[15],
            Color::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
            Color::Indexed(index) => self.indexed_color(index),
        }
    }

    /// Converts an xterm indexed color into a Ratatui RGB color.
    fn indexed_color(&self, index: u8) -> Color {
        if index < 16 {
            return self.ansi[index as usize];
        }
        if index < 232 {
            let n = index - 16;
            let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            return Color::Rgb(component(n / 36), component((n / 6) % 6), component(n % 6));
        }
        let gray = 8 + (index - 232) * 10;
        Color::Rgb(gray, gray, gray)
    }

    /// Dims the foreground without changing the background color.
    fn dim(color: Color) -> Color {
        let Color::Rgb(red, green, blue) = color else {
            unreachable!("resolved terminal colors are RGB")
        };
        Color::Rgb(red / 2, green / 2, blue / 2)
    }

    /// Returns RGB components from a Ratatui RGB color.
    fn rgb(color: Color) -> [u8; 3] {
        let Color::Rgb(red, green, blue) = color else {
            unreachable!("configured terminal colors are RGB")
        };
        [red, green, blue]
    }
}

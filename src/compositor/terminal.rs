use anyhow::{Context, Result};
use fux_vt::Screen;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Rectangle, Transform},
};

use super::adapter::{self, CellAdapter};

/// Rasterizes terminal cells into a memory buffer for Smithay to composite.
pub(super) struct TerminalRenderer {
    width: i32,
    height: i32,
    pixels: MemoryRenderBuffer,
    adapter: CellAdapter,
}

impl TerminalRenderer {
    /// Creates a terminal layer sized to the compositor output.
    pub(super) fn new(width: i32, height: i32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            width,
            height,
            pixels: MemoryRenderBuffer::new(
                Fourcc::Abgr8888,
                (width, height),
                1,
                Transform::Normal,
                None,
            ),
            adapter: CellAdapter::new(),
        }
    }

    /// Returns the output size in terminal cells.
    pub(super) fn cell_size_for(width: i32, height: i32) -> (u16, u16) {
        adapter::cell_size_for(width, height)
    }

    /// Returns this layer's current size in terminal cells.
    pub(super) fn cell_size(&self) -> (u16, u16) {
        Self::cell_size_for(self.width, self.height)
    }

    /// Resizes the pixel buffer to match the output.
    pub(super) fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.pixels.render().resize((self.width, self.height));
    }

    /// Updates the output buffer from the current VT screen.
    pub(super) fn draw(&mut self, screen: &Screen) -> Result<()> {
        let pixels = self.adapter.render(screen, self.width, self.height);
        let width = self.width;
        let height = self.height;
        self.pixels
            .render()
            .draw(|buffer| {
                buffer.copy_from_slice(&pixels);
                Ok::<_, anyhow::Error>(vec![Rectangle::from_size((width, height).into())])
            })
            .context("update terminal render buffer")
    }

    /// Returns the memory buffer used by the terminal layer.
    pub(super) fn pixels(&self) -> &MemoryRenderBuffer {
        &self.pixels
    }
}

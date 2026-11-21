mod compositor;
mod config;
mod terminal;

use compositor::Compositor;
use terminal::{Terminal, pty::PtySession};

fn main() -> anyhow::Result<()> {
    println!("🐀🌌");

    let compositor = Compositor::new()?;
    let (cols, rows) = compositor.terminal_size();
    let screen = Terminal::new(cols, rows)?;
    let shell = PtySession::new(cols, rows, compositor.wayland_display())?;

    compositor.run(screen, shell)
}

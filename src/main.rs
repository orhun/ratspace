mod compositor;
mod config;
mod terminal;

use compositor::Compositor;
use terminal::{Terminal, pty::PtySession};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    println!("🐀🌌");
    tracing::info!("starting Ratspace");

    let compositor = Compositor::new()?;
    tracing::info!("starting shell");
    let (cols, rows) = compositor.terminal_size();
    let screen = Terminal::new(cols, rows)?;
    let shell = PtySession::new(cols, rows, compositor.wayland_display())?;

    compositor.run(screen, shell)
}

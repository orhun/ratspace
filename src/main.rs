mod compositor;
mod config;
mod terminal;

use compositor::Compositor;
use terminal::{Terminal, pty::PtySession};

fn main() -> anyhow::Result<()> {
    println!("🐀🌌");

    let compositor = match std::env::args().nth(1).as_deref() {
        Some("--tty") => create_tty_compositor()?,
        Some(argument) => anyhow::bail!("unknown argument: {argument}"),
        None => Compositor::new()?,
    };
    let (cols, rows) = compositor.terminal_size();
    let screen = Terminal::new(cols, rows)?;
    let shell = PtySession::new(cols, rows, compositor.wayland_display())?;

    compositor.run(screen, shell)
}

fn create_tty_compositor() -> anyhow::Result<Compositor> {
    #[cfg(feature = "tty")]
    {
        Compositor::new_tty()
    }
    #[cfg(not(feature = "tty"))]
    {
        anyhow::bail!("TTY output is disabled; rebuild Ratspace with --features tty")
    }
}

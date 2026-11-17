mod config;
mod terminal;

use terminal::{Terminal, app, pty::PtySession, render::Renderer};

fn main() -> anyhow::Result<()> {
    println!("🐀🌌");

    let mut renderer = Renderer::new()?;
    let (cols, rows) = renderer.size()?;
    let mut screen = Terminal::new(cols, rows)?;
    let mut shell = PtySession::new(cols, rows)?;

    app::run(&mut screen, &mut shell, &mut renderer)
}

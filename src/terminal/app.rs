use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event};

use crate::terminal::{
    Terminal, input,
    pty::{OutputPoll, PtySession},
    render::Renderer,
};

/// Runs the input, shell output, resize and rendering loop.
pub fn run(terminal: &mut Terminal, pty: &mut PtySession, renderer: &mut Renderer) -> Result<()> {
    renderer.draw(terminal)?;
    loop {
        if event::poll(Duration::from_millis(10)).context("poll terminal input")? {
            handle_event(
                event::read().context("read terminal input")?,
                terminal,
                pty,
                renderer,
            )?;
        }

        let mut changed = false;
        loop {
            match pty.poll_output() {
                OutputPoll::Data(bytes) => {
                    let replies = terminal.process_output(&bytes)?;
                    if !replies.is_empty() {
                        pty.write(&replies)?;
                    }
                    changed = true;
                }
                OutputPoll::Empty => break,
                OutputPoll::Closed => {
                    if changed {
                        renderer.draw(terminal)?;
                    }
                    pty.wait()?;
                    return Ok(());
                }
            }
        }
        if changed {
            renderer.draw(terminal)?;
        }
    }
}

/// Routes terminal input and resize events to the shell and renderer.
fn handle_event(
    event: Event,
    terminal: &mut Terminal,
    pty: &mut PtySession,
    renderer: &mut Renderer,
) -> Result<()> {
    match event {
        Event::Key(key) => {
            if let Some(bytes) = input::encode_key(key, terminal) {
                pty.write(&bytes)?;
            }
        }
        Event::Paste(text) => pty.write(&input::encode_paste(&text, terminal))?,
        Event::Resize(cols, rows) => {
            let report = terminal.resize(cols, rows)?;
            pty.resize(cols, rows)?;
            if let Some(report) = report {
                pty.write(&report)?;
            }
            renderer.draw(terminal)?;
        }
        _ => {}
    }
    Ok(())
}

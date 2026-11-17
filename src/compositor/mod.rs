mod adapter;
mod input;
mod state;
mod terminal;
mod winit;

use std::ffi::OsStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use smithay::reexports::calloop::{EventLoop, timer::Timer};
use state::State;
use terminal::TerminalRenderer;
use winit::WinitOutput;

use crate::terminal::{Terminal, pty::PtySession};

/// Owns Ratspace's Wayland server and its single nested output.
pub struct Compositor {
    event_loop: EventLoop<'static, State>,
    state: State,
}

impl Compositor {
    /// Creates the nested Winit output and a Wayland socket for child applications.
    pub fn new() -> Result<Self> {
        let mut event_loop = EventLoop::try_new().context("create compositor event loop")?;
        let display =
            smithay::reexports::wayland_server::Display::new().context("create Wayland display")?;
        let mut state = State::new(&mut event_loop, display)?;
        let output = WinitOutput::init(&mut event_loop, &mut state)?;
        let (width, height) = output.size();
        state.set_output_size(width, height, output.scale_factor())?;
        tracing::info!(
            wayland_display = %state.socket_name.to_string_lossy(),
            output_width = width,
            output_height = height,
            "compositor ready"
        );

        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_millis(10)),
                |_, _, state| {
                    if let Err(error) = state.poll_shell() {
                        tracing::error!(error = ?error, "shell PTY failed");
                        state.loop_signal.stop();
                    }
                    smithay::reexports::calloop::timer::TimeoutAction::ToDuration(
                        Duration::from_millis(10),
                    )
                },
            )
            .map_err(|error| anyhow!("watch shell PTY: {error:?}"))?;

        Ok(Self { event_loop, state })
    }

    /// Returns the initial shell size in terminal cells.
    pub fn terminal_size(&self) -> (u16, u16) {
        self.state.terminal_size()
    }

    /// Returns the Wayland socket name used by this compositor.
    pub fn wayland_display(&self) -> &OsStr {
        &self.state.socket_name
    }

    /// Starts the shell and runs the compositor until its output is closed.
    pub fn run(mut self, terminal: Terminal, pty: PtySession) -> Result<()> {
        let (width, height) = self.state.output_size();
        self.state.terminal_renderer = Some(TerminalRenderer::new(width, height));
        self.state.terminal = Some(terminal);
        self.state.pty = Some(pty);
        self.state.redraw_terminal()?;
        tracing::info!("running compositor event loop");

        self.event_loop
            .run(None, &mut self.state, |_| {})
            .context("run compositor event loop")?;
        tracing::info!("compositor event loop stopped");

        if self.state.shell_closed
            && let Some(pty) = &mut self.state.pty
        {
            pty.wait()?;
        }
        Ok(())
    }
}

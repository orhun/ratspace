mod input;
mod palette;
mod state;
mod terminal;
#[cfg(feature = "tty")]
mod tty;
mod winit;
mod xdg_shell;

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use smithay::reexports::calloop::{EventLoop, timer::Timer};

use crate::terminal::{Terminal, pty::PtySession};

use self::{state::State, terminal::TerminalRenderer, winit::WinitOutput};

/// Owns Ratspace's Wayland server, shell, and selected display backend.
pub struct Compositor {
    event_loop: EventLoop<'static, State>,
    state: State,
}

impl Compositor {
    /// Creates the nested output and a Wayland socket for the shell.
    pub fn new() -> Result<Self> {
        let (mut event_loop, mut state) = Self::initialize()?;
        let output = WinitOutput::init(&mut event_loop, &mut state)?;
        let (width, height) = output.size();
        Self::finish(event_loop, state, width, height, output.scale_factor())
    }

    /// Creates a DRM output and Wayland socket for running directly on a TTY.
    #[cfg(feature = "tty")]
    pub fn new_tty() -> Result<Self> {
        let (mut event_loop, mut state) = Self::initialize()?;
        let (width, height) = tty::TtyOutput::init(&mut event_loop, &mut state)?;
        Self::finish(event_loop, state, width, height, 1.0)
    }

    /// Creates the shared Wayland protocol state and compositor event loop.
    fn initialize() -> Result<(EventLoop<'static, State>, State)> {
        let mut event_loop = EventLoop::try_new().context("create compositor event loop")?;
        let display =
            smithay::reexports::wayland_server::Display::new().context("create Wayland display")?;
        let state = State::new(&mut event_loop, display)?;
        Ok((event_loop, state))
    }

    /// Initializes terminal rendering and the shared shell polling loop.
    fn finish(
        event_loop: EventLoop<'static, State>,
        mut state: State,
        width: i32,
        height: i32,
        scale_factor: f64,
    ) -> Result<Self> {
        state.set_output_size(width, height, scale_factor)?;
        state.terminal_renderer = Some(TerminalRenderer::new(width, height)?);
        state.request_redraw();

        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_millis(10)),
                |_, _, state| {
                    if let Err(error) = state.poll_shell() {
                        state.stop_with_error(error.context("shell PTY failed"));
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

    /// Returns the socket name used by this compositor's Wayland clients.
    pub fn wayland_display(&self) -> &std::ffi::OsStr {
        &self.state.socket_name
    }

    /// Starts the shell and runs the compositor until either one exits.
    pub fn run(mut self, terminal: Terminal, pty: PtySession) -> Result<()> {
        self.state.terminal = Some(terminal);
        self.state.pty = Some(pty);
        self.state.resize_terminal()?;
        self.state.request_redraw();

        self.event_loop
            .run(None, &mut self.state, |_| {})
            .context("run compositor event loop")?;

        if let Some(error) = self.state.failure.take() {
            return Err(error);
        }
        if self.state.shell_closed
            && let Some(pty) = &mut self.state.pty
        {
            pty.wait()?;
        }
        Ok(())
    }
}

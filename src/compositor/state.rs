use std::{ffi::OsString, rc::Rc, sync::Arc};

use anyhow::{Context, Result, anyhow};
use smithay::{
    delegate_compositor, delegate_output, delegate_seat, delegate_shm,
    input::{Seat, SeatHandler, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        wayland_server::{
            Client, Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    utils::SERIAL_COUNTER,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        output::{OutputHandler, OutputManagerState},
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};

use crate::terminal::{
    Terminal, input,
    pty::{OutputPoll, PtySession},
};

use super::{input as compositor_input, terminal::TerminalRenderer};

/// Holds Wayland protocol state and the shell terminal shown on the output.
pub(super) struct State {
    /// Socket name passed to the shell for Wayland client connections.
    pub(super) socket_name: OsString,
    /// Handle for publishing globals and managing connected clients.
    pub(super) display_handle: DisplayHandle,
    /// Signal used to stop the compositor event loop.
    pub(super) loop_signal: smithay::reexports::calloop::LoopSignal,
    /// Current output dimensions in physical pixels.
    output_size: (i32, i32),
    /// Wayland output advertised to connected clients.
    output: Option<smithay::output::Output>,
    /// State and globals for wl_compositor and wl_surface.
    compositor_state: CompositorState,
    /// State and globals for shared-memory client buffers.
    shm_state: ShmState,
    /// Keeps wl_output globals alive for the display lifetime.
    _output_manager_state: OutputManagerState,
    /// State for the Wayland keyboard seat.
    seat_state: SeatState<State>,
    /// Wayland seat advertised to clients.
    seat: Seat<State>,
    /// Callback that requests a frame from the host window.
    redraw_requester: Option<Rc<dyn Fn()>>,
    /// VT model receiving shell output and keyboard input.
    pub(super) terminal: Option<Terminal>,
    /// Pixel renderer for the shell's terminal cells.
    pub(super) terminal_renderer: Option<TerminalRenderer>,
    /// Shell process and its pseudoterminal.
    pub(super) pty: Option<PtySession>,
    /// Whether the shell PTY closed and caused the event loop to stop.
    pub(super) shell_closed: bool,
    /// First compositor or shell error raised by an event callback.
    pub(super) failure: Option<anyhow::Error>,
}

/// Creates Wayland globals and routes output, input, and shell events.
impl State {
    /// Builds the protocol state, client socket, and display event source.
    pub(super) fn new(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
    ) -> Result<Self> {
        let display_handle = display.handle();
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "ratspace");
        seat.add_keyboard(Default::default(), 200, 25)
            .context("create Wayland keyboard")?;

        let listening_socket =
            ListeningSocketSource::new_auto().context("create Wayland listening socket")?;
        let socket_name = listening_socket.socket_name().to_os_string();
        event_loop
            .handle()
            .insert_source(listening_socket, |stream, _, state| {
                if let Err(error) = state
                    .display_handle
                    .insert_client(stream, Arc::new(ClientState::default()))
                {
                    state.stop_with_error(anyhow!("accept Wayland client: {error}"));
                }
            })
            .map_err(|error| anyhow!("register Wayland listener: {error:?}"))?;
        event_loop
            .handle()
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // This event source owns the display for the compositor lifetime.
                    unsafe { display.get_mut().dispatch_clients(state)? };
                    Ok(PostAction::Continue)
                },
            )
            .context("register Wayland display")?;

        Ok(Self {
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
            output_size: (1, 1),
            output: None,
            compositor_state,
            shm_state,
            _output_manager_state: output_manager_state,
            seat_state,
            seat,
            redraw_requester: None,
            terminal: None,
            terminal_renderer: None,
            pty: None,
            shell_closed: false,
            failure: None,
        })
    }

    /// Returns the terminal cell dimensions for the current output.
    pub(super) fn terminal_size(&self) -> (u16, u16) {
        self.terminal_renderer
            .as_ref()
            .map(TerminalRenderer::cell_size)
            .expect("terminal renderer initialized with compositor output")
    }

    /// Resizes the output and updates the terminal and shell dimensions.
    pub(super) fn set_output_size(
        &mut self,
        width: i32,
        height: i32,
        scale_factor: f64,
    ) -> Result<()> {
        self.output_size = (width.max(1), height.max(1));
        if let Some(output) = &self.output {
            output.change_current_state(
                Some(smithay::output::Mode {
                    size: self.output_size.into(),
                    refresh: 60_000,
                }),
                None,
                Some(smithay::output::Scale::Fractional(scale_factor)),
                None,
            );
        }
        if let Some(renderer) = &mut self.terminal_renderer {
            renderer.resize(self.output_size.0, self.output_size.1);
            self.resize_terminal()?;
        }
        Ok(())
    }

    /// Registers the compositor's output with Smithay's output state.
    pub(super) fn install_output(&mut self, output: smithay::output::Output) {
        self.output = Some(output);
    }

    /// Stores the host-window callback used to schedule a compositor frame.
    pub(super) fn set_redraw_requester(&mut self, requester: Rc<dyn Fn()>) {
        self.redraw_requester = Some(requester);
    }

    /// Requests a frame from the host window when terminal content changes.
    pub(super) fn request_redraw(&self) {
        if let Some(requester) = &self.redraw_requester {
            requester();
        }
    }

    /// Drains available shell output, parses it, and redraws changed cells.
    pub(super) fn poll_shell(&mut self) -> Result<()> {
        let mut changed = false;
        loop {
            let Some(pty) = self.pty.as_ref() else { break };
            match pty.poll_output() {
                OutputPoll::Data(bytes) => {
                    let replies = self
                        .terminal
                        .as_mut()
                        .context("terminal not initialized")?
                        .process_output(&bytes)?;
                    if !replies.is_empty() {
                        self.pty
                            .as_mut()
                            .context("shell PTY not initialized")?
                            .write(&replies)?;
                    }
                    changed = true;
                }
                OutputPoll::Empty => break,
                OutputPoll::Closed => {
                    if changed {
                        self.redraw_terminal()?;
                        self.request_redraw();
                    }
                    self.shell_closed = true;
                    self.loop_signal.stop();
                    return Ok(());
                }
            }
        }
        if changed {
            self.redraw_terminal()?;
            self.request_redraw();
        }
        Ok(())
    }

    /// Redraws the terminal's current VT screen into its pixel buffer.
    pub(super) fn redraw_terminal(&mut self) -> Result<()> {
        let terminal = self.terminal.as_ref().context("terminal not initialized")?;
        self.terminal_renderer
            .as_mut()
            .context("terminal renderer not initialized")?
            .draw(terminal.screen())
    }

    /// Resizes the VT screen and the shell PTY to match the output.
    pub(super) fn resize_terminal(&mut self) -> Result<()> {
        let (cols, rows) = self.terminal_size();
        if let Some(terminal) = &mut self.terminal {
            if let Some(report) = terminal.resize(cols, rows)? {
                self.pty
                    .as_mut()
                    .context("shell PTY not initialized")?
                    .write(&report)?;
            }
        }
        if let Some(pty) = &self.pty {
            pty.resize(cols, rows)?;
        }
        if self.terminal.is_some() && self.terminal_renderer.is_some() {
            self.redraw_terminal()?;
        }
        Ok(())
    }

    /// Sends an error to the event loop and stops further processing.
    pub(super) fn stop_with_error(&mut self, error: anyhow::Error) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        self.loop_signal.stop();
    }

    /// Routes keyboard events to the shell while this step has no app surfaces.
    pub(super) fn process_input_event<I: smithay::backend::input::InputBackend>(
        &mut self,
        event: smithay::backend::input::InputEvent<I>,
    ) {
        use smithay::backend::input::{Event, InputEvent, KeyState, KeyboardKeyEvent};
        use smithay::input::keyboard::FilterResult;

        let InputEvent::Keyboard { event, .. } = event else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time(&event) as u32;
        let key_code = event.key_code();
        let key_state = event.state();
        let keyboard = self.seat.get_keyboard().expect("keyboard was created");
        keyboard.input::<(), _>(
            self,
            key_code,
            key_state,
            serial,
            time,
            |state, modifiers, sym| {
                if key_state == KeyState::Pressed {
                    if let Some(key) = compositor_input::crossterm_key(sym.modified_sym()) {
                        let mut key_modifiers = crossterm::event::KeyModifiers::empty();
                        if modifiers.shift {
                            key_modifiers |= crossterm::event::KeyModifiers::SHIFT;
                        }
                        if modifiers.ctrl {
                            key_modifiers |= crossterm::event::KeyModifiers::CONTROL;
                        }
                        if modifiers.alt {
                            key_modifiers |= crossterm::event::KeyModifiers::ALT;
                        }
                        let event = crossterm::event::KeyEvent::new(key, key_modifiers);
                        let write_error = if let (Some(terminal), Some(pty)) =
                            (&state.terminal, &mut state.pty)
                            && let Some(bytes) = input::encode_key(event, terminal)
                        {
                            pty.write(&bytes).err()
                        } else {
                            None
                        };
                        if let Some(error) = write_error {
                            state.stop_with_error(error.context("write keyboard input to shell"));
                        }
                    }
                }
                FilterResult::Intercept(())
            },
        );
    }
}

/// Releases client buffer resources when their Wayland buffers are destroyed.
impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

/// Implements surface lifecycle and damage tracking for Wayland clients.
impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
    }
}

/// Provides shared-memory buffers to Wayland clients.
impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

/// Tracks clients that bind the compositor's output.
impl OutputHandler for State {
    fn output_bound(&mut self, _output: smithay::output::Output, _wl_output: WlOutput) {}
}

/// Routes the Wayland keyboard seat; keyboard input remains with the shell.
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

/// Stores surface bookkeeping for each connected Wayland client.
#[derive(Default)]
struct ClientState {
    /// Smithay state associated with the client's compositor surfaces.
    compositor_state: CompositorClientState,
}

/// Receives Wayland client connection and disconnection notifications.
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// These macros connect the handlers above to Smithay's Wayland protocol dispatchers.
delegate_compositor!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_output!(State);

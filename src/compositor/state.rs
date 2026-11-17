use std::{ffi::OsString, sync::Arc, time::Instant};

use super::terminal::TerminalRenderer;
use crate::terminal::{
    Terminal, input,
    pty::{OutputPoll, PtySession},
};
use anyhow::{Context, Result, anyhow};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatHandler, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState, with_states},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};

/// Wayland protocol state and the shell surface displayed beneath client windows.
pub(super) struct State {
    /// Origin used to timestamp client frame callbacks.
    pub(super) start_time: Instant,
    /// Socket name passed to the shell for Wayland client connections.
    pub(super) socket_name: OsString,
    /// Handle for publishing globals and managing connected clients.
    pub(super) display_handle: DisplayHandle,
    /// Signal used to stop the compositor event loop.
    pub(super) loop_signal: smithay::reexports::calloop::LoopSignal,
    /// Mapped toplevel windows on the compositor's output.
    pub(super) space: Space<Window>,
    /// Tracks xdg popup surfaces belonging to mapped windows.
    pub(super) popups: PopupManager,
    /// Wayland output advertised to connected clients.
    pub(super) output: Option<smithay::output::Output>,
    /// Current output dimensions in physical pixels.
    pub(super) output_size: (i32, i32),

    /// State and globals for wl_compositor and wl_surface.
    pub(super) compositor_state: CompositorState,
    /// State and globals for xdg-shell surfaces.
    pub(super) xdg_shell_state: XdgShellState,
    /// State and globals for shared-memory client buffers.
    pub(super) shm_state: ShmState,
    /// Retains wl_output and xdg-output globals for the display lifetime.
    pub(super) output_manager_state: OutputManagerState,
    /// State for the Wayland keyboard and pointer seat.
    pub(super) seat_state: SeatState<State>,
    /// State for client clipboard and data-device requests.
    pub(super) data_device_state: DataDeviceState,
    /// Wayland seat advertised to clients.
    pub(super) seat: Seat<State>,

    /// VT model receiving shell output and keyboard input.
    pub(super) terminal: Option<Terminal>,
    /// Pixel renderer for the shell's terminal cells.
    pub(super) terminal_renderer: Option<TerminalRenderer>,
    /// Shell process and its pseudoterminal.
    pub(super) pty: Option<PtySession>,
    /// Whether the shell PTY closed and caused the event loop to stop.
    pub(super) shell_closed: bool,
}

/// Creates Wayland globals and routes compositor, input, and shell events.
impl State {
    /// Builds the protocol state, client socket, and display event source.
    pub(super) fn new(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
    ) -> Result<Self> {
        let display_handle = display.handle();
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "ratspace");
        seat.add_keyboard(Default::default(), 200, 25)
            .context("create Wayland keyboard")?;
        seat.add_pointer();

        let listening_socket =
            ListeningSocketSource::new_auto().context("create Wayland listening socket")?;
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();
        loop_handle
            .insert_source(listening_socket, |stream, _, state| {
                if let Err(error) = state
                    .display_handle
                    .insert_client(stream, Arc::new(ClientState::default()))
                {
                    tracing::warn!(error = %error, "failed to accept Wayland client");
                } else {
                    tracing::debug!("accepted Wayland client");
                }
            })
            .map_err(|error| anyhow!("register Wayland listener: {error:?}"))?;
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // The event loop owns this display source for its entire lifetime.
                    unsafe { display.get_mut().dispatch_clients(state)? };
                    Ok(PostAction::Continue)
                },
            )
            .context("register Wayland display")?;

        Ok(Self {
            start_time: Instant::now(),
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            popups: PopupManager::default(),
            output: None,
            output_size: (1, 1),
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
            terminal: None,
            terminal_renderer: None,
            pty: None,
            shell_closed: false,
        })
    }

    pub(super) fn terminal_size(&self) -> (u16, u16) {
        self.terminal_renderer
            .as_ref()
            .map(TerminalRenderer::cell_size)
            .unwrap_or_else(|| {
                TerminalRenderer::cell_size_for(self.output_size.0, self.output_size.1)
            })
    }

    pub(super) fn output_size(&self) -> (i32, i32) {
        self.output_size
    }

    pub(super) fn set_output_size(
        &mut self,
        width: i32,
        height: i32,
        scale_factor: f64,
    ) -> Result<()> {
        self.output_size = (width.max(1), height.max(1));
        if let (Some(output), Some(renderer)) = (&self.output, &mut self.terminal_renderer) {
            renderer.resize(self.output_size.0, self.output_size.1);
            output.change_current_state(
                Some(smithay::output::Mode {
                    size: self.output_size.into(),
                    refresh: 60_000,
                }),
                None,
                Some(smithay::output::Scale::Fractional(scale_factor)),
                None,
            );
            self.resize_terminal()?;
        }
        Ok(())
    }

    pub(super) fn install_output(&mut self, output: smithay::output::Output) {
        self.space.map_output(&output, (0, 0));
        self.output = Some(output);
    }

    pub(super) fn poll_shell(&mut self) -> Result<()> {
        let mut changed = false;
        loop {
            let Some(pty) = self.pty.as_ref() else { break };
            match pty.poll_output() {
                OutputPoll::Data(bytes) => {
                    tracing::debug!(bytes = bytes.len(), "received shell PTY output");
                    let replies = self
                        .terminal
                        .as_mut()
                        .context("terminal not initialized")?
                        .process_output(&bytes)?;
                    if !replies.is_empty() {
                        self.pty.as_mut().unwrap().write(&replies)?;
                    }
                    changed = true;
                }
                OutputPoll::Empty => break,
                OutputPoll::Closed => {
                    tracing::info!("shell PTY closed");
                    if changed {
                        self.redraw_terminal()?;
                    }
                    self.shell_closed = true;
                    self.loop_signal.stop();
                    return Ok(());
                }
            }
        }

        if changed {
            self.redraw_terminal()?;
        }
        Ok(())
    }

    pub(super) fn redraw_terminal(&mut self) -> Result<()> {
        let terminal = self.terminal.as_ref().context("terminal not initialized")?;
        self.terminal_renderer
            .as_mut()
            .context("terminal renderer not initialized")?
            .draw(terminal.screen())
    }

    pub(super) fn resize_terminal(&mut self) -> Result<()> {
        let (cols, rows) = self.terminal_size();
        let report = self
            .terminal
            .as_mut()
            .context("terminal not initialized")?
            .resize(cols, rows)?;
        let pty = self.pty.as_mut().context("shell PTY not initialized")?;
        pty.resize(cols, rows)?;
        if let Some(report) = report {
            pty.write(&report)?;
        }
        self.redraw_terminal()
    }

    pub(super) fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, point)| (surface, (point + location).to_f64()))
            })
    }

    pub(super) fn focus_surface(&mut self, surface: Option<WlSurface>) {
        let serial = SERIAL_COUNTER.next_serial();
        for window in self.space.elements() {
            let active = surface.as_ref().is_some_and(|surface| {
                window
                    .toplevel()
                    .is_some_and(|top| top.wl_surface() == surface)
            });
            window.set_activated(active);
            if let Some(toplevel) = window.toplevel()
                && has_initial_configure_sent(toplevel.wl_surface())
            {
                toplevel.send_pending_configure();
            }
        }
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, surface, serial);
    }

    pub(super) fn process_input_event<I: smithay::backend::input::InputBackend>(
        &mut self,
        event: smithay::backend::input::InputEvent<I>,
    ) {
        use smithay::backend::input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputEvent,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        };
        use smithay::input::{
            keyboard::FilterResult,
            pointer::{AxisFrame, ButtonEvent, MotionEvent},
        };

        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time(&event) as u32;
                let keyboard = self.seat.get_keyboard().unwrap();
                let shell_focus = keyboard.current_focus().is_none();
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    |state, modifiers, sym| {
                        if shell_focus {
                            if event.state() == smithay::backend::input::KeyState::Pressed {
                                if let Some(key) = super::input::crossterm_key(sym.modified_sym()) {
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
                                    if let (Some(terminal), Some(pty)) =
                                        (&state.terminal, &mut state.pty)
                                    {
                                        if let Some(bytes) = input::encode_key(event, terminal) {
                                            let _ = pty.write(&bytes);
                                        }
                                    }
                                }
                            }
                            FilterResult::Intercept(())
                        } else {
                            FilterResult::Forward
                        }
                    },
                );
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.output.as_ref() else {
                    return;
                };
                let output_geo = self.space.output_geometry(output).unwrap();
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().unwrap();
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time() as u32,
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    if let Some((window, _)) = self.space.element_under(pointer.current_location())
                    {
                        let window = window.clone();
                        self.space.raise_element(&window, true);
                        self.focus_surface(window.toplevel().map(|top| top.wl_surface().clone()));
                    } else {
                        self.focus_surface(None);
                    }
                }
                pointer.button(
                    self,
                    &ButtonEvent {
                        button: event.button_code(),
                        state: event.state(),
                        serial,
                        time: event.time() as u32,
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let mut frame = AxisFrame::new(event.time() as u32).source(event.source());
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0);
                    if amount != 0.0 {
                        frame = frame.value(axis, amount);
                    }
                }
                if event.source() == AxisSource::Finger {
                    for axis in [Axis::Horizontal, Axis::Vertical] {
                        if event.amount(axis) == Some(0.0) {
                            frame = frame.stop(axis);
                        }
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
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

    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        if let Some(window) = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|top| top.wl_surface() == surface)
            })
            .cloned()
        {
            window.on_commit();
            if !has_initial_configure_sent(surface) {
                window.toplevel().unwrap().send_configure();
            }
        }
    }
}

/// Provides shared-memory buffers to Wayland clients.
impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

/// Maps xdg toplevels and configures their popup surfaces.
impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::info!("Wayland toplevel created");
        let window = Window::new_wayland_window(surface);
        let wl_surface = window.toplevel().unwrap().wl_surface().clone();
        self.space.map_element(window, (0, 0), false);
        self.focus_surface(Some(wl_surface));
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface.clone()));
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        let _ = surface.send_configure();
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface();
        if self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .is_some_and(|focused| focused == *wl_surface)
        {
            self.focus_surface(None);
        }
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
    }
}

/// Routes keyboard focus and pointer focus through the Wayland seat.
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }
    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

/// Enables selection ownership for the seat's data device.
impl SelectionHandler for State {
    type SelectionUserData = ();
}
/// Provides the Wayland data-device state used for clipboard transfers.
impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
/// Accepts client initiated drag-and-drop requests.
impl ClientDndGrabHandler for State {}

/// Handles compositor initiated drag-and-drop data transfer.
impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: std::os::fd::OwnedFd, _seat: Seat<Self>) {}
}

/// Handles clients binding the compositor's Wayland output.
impl OutputHandler for State {
    fn output_bound(&mut self, output: smithay::output::Output, _wl_output: WlOutput) {
        tracing::debug!(
            output = %output.name(),
            xdg_output_available = self.output_manager_state.xdg_output_manager_global().is_some(),
            "client bound output"
        );
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
delegate_xdg_shell!(State);
delegate_compositor!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);

/// Checks whether the xdg toplevel has received its first configure event.
fn has_initial_configure_sent(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap()
            .initial_configure_sent
    })
}

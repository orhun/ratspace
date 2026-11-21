use std::{cell::RefCell, os::fd::OwnedFd, path::Path, rc::Rc, time::Duration};

use anyhow::{Context, Result, anyhow};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmNode, NodeType,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            element::{
                Kind, memory::MemoryRenderBufferRenderElement, surface::WaylandSurfaceRenderElement,
            },
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::primary_gpu,
    },
    desktop::space::{SpaceRenderElements, space_render_elements},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{EventLoop, LoopHandle},
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Transform},
};

use super::state::State;

smithay::backend::renderer::element::render_elements! {
    OutputElement<=GlesRenderer>;
    Terminal=MemoryRenderBufferRenderElement<GlesRenderer>,
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
}

type TtyDrmOutput =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type TtyDrmOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

const OUTPUT_CLEAR_COLOR: [f32; 4] = [0.04, 0.04, 0.04, 1.0];
const OUTPUT_NAME: &str = env!("CARGO_PKG_NAME");
const COLOR_FORMATS: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

/// Initializes one physical DRM output for a direct TTY session.
pub(super) struct TtyOutput;

/// Owns the DRM output state shared by rendering and session events.
struct TtyBackend {
    /// Keeps the libseat session active for the lifetime of the DRM device.
    _session: LibSeatSession,
    /// Manages DRM device activation and output buffer submission.
    output_manager: TtyDrmOutputManager,
    /// DRM output associated with the connected monitor.
    drm_output: TtyDrmOutput,
    /// GLES renderer bound to the DRM device's GBM buffers.
    renderer: GlesRenderer,
    /// Wayland output used for surface rendering and callbacks.
    output: Output,
    /// CRTC whose pageflip events complete our submitted frames.
    crtc: crtc::Handle,
    /// Whether a queued frame is waiting for its pageflip event.
    frame_pending: bool,
    /// Whether this process currently owns the active VT session.
    session_active: bool,
    /// Whether changes arrived while a frame was pending.
    needs_redraw: bool,
    /// Whether an idle render callback has already been queued.
    redraw_scheduled: bool,
}

/// Sets up libseat, the DRM output, input, and redraw event sources.
impl TtyOutput {
    /// Creates the first connected output and returns its physical pixel dimensions.
    pub(super) fn init(
        event_loop: &mut EventLoop<'static, State>,
        state: &mut State,
    ) -> Result<(i32, i32)> {
        let (mut session, session_notifier) =
            LibSeatSession::new().context("create libseat session for TTY output")?;
        let seat_name = session.seat();
        let device_path = primary_gpu(&seat_name)
            .context("find GPU for the current seat")?
            .context("no DRM GPU found for the current seat")?;
        let node = DrmNode::from_path(&device_path)
            .context("identify primary DRM device")?
            .node_with_type(NodeType::Primary)
            .context("query primary DRM node")?
            .context("GPU has no primary DRM node")?;
        let (drm_device, drm_notifier) = Self::open_drm_device(&mut session, &device_path)?;
        let (connector, crtc, drm_mode) = Self::find_connected_output(&drm_device)?;

        let gbm = GbmDevice::new(drm_device.device_fd().clone()).context("create GBM device")?;
        let egl_display =
            unsafe { EGLDisplay::new(gbm.clone()) }.context("create GBM EGL display")?;
        let egl_context =
            EGLContext::new(&egl_display).context("create EGL context for TTY output")?;
        let mut renderer = unsafe { GlesRenderer::new(egl_context) }
            .context("create GLES renderer for TTY output")?;
        let render_formats = renderer
            .egl_context()
            .dmabuf_render_formats()
            .clone()
            .into_iter();

        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let exporter = GbmFramebufferExporter::new(gbm.clone(), Some(node));
        let mut output_manager = DrmOutputManager::new(
            drm_device,
            allocator,
            exporter,
            Some(gbm),
            COLOR_FORMATS.iter().copied(),
            render_formats,
        );

        let output = Self::create_output(state, &connector, drm_mode);
        let drm_output = output_manager
            .initialize_output::<_, OutputElement>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                None,
                &mut renderer,
                &DrmOutputRenderElements::<GlesRenderer, OutputElement>::default(),
            )
            .map_err(|error| anyhow!("initialize DRM output: {error:?}"))?;
        let size = drm_mode.size();

        let backend = Rc::new(RefCell::new(TtyBackend {
            _session: session.clone(),
            output_manager,
            drm_output,
            renderer,
            output: output.clone(),
            crtc,
            frame_pending: false,
            session_active: session.is_active(),
            needs_redraw: false,
            redraw_scheduled: false,
        }));
        let loop_handle = event_loop.handle();

        let redraw_backend = Rc::clone(&backend);
        let redraw_handle = loop_handle.clone();
        state.set_redraw_requester(Rc::new(move || {
            Self::request_redraw(&redraw_handle, &redraw_backend);
        }));

        Self::insert_drm_events(event_loop, &loop_handle, &backend, drm_notifier);

        let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
            session.clone().into(),
        );
        libinput
            .udev_assign_seat(&seat_name)
            .map_err(|_| anyhow!("assign libinput to seat {seat_name}"))?;
        let input_backend = LibinputInputBackend::new(libinput.clone());
        event_loop
            .handle()
            .insert_source(input_backend, |event, _, state| {
                state.process_input_event(event);
            })
            .map_err(|error| anyhow!("register TTY input backend: {error:?}"))?;

        Self::insert_session_events(
            event_loop,
            &loop_handle,
            &backend,
            session_notifier,
            libinput,
        );

        Ok((size.0 as i32, size.1 as i32))
    }

    /// Opens the primary DRM node through the active libseat session.
    fn open_drm_device(
        session: &mut LibSeatSession,
        path: &Path,
    ) -> Result<(DrmDevice, DrmDeviceNotifier)> {
        let fd: OwnedFd = session
            .open(
                path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(|error| anyhow!("open {} through libseat: {error:?}", path.display()))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));
        DrmDevice::new(fd, true).context("initialize DRM device")
    }

    /// Chooses the first connected connector, compatible CRTC, and preferred mode.
    fn find_connected_output(
        device: &DrmDevice,
    ) -> Result<(
        connector::Info,
        crtc::Handle,
        smithay::reexports::drm::control::Mode,
    )> {
        let resources = device.resource_handles().context("read DRM resources")?;
        resources
            .connectors()
            .iter()
            .find_map(|handle| {
                let info = device.get_connector(*handle, true).ok()?;
                if info.state() != connector::State::Connected {
                    return None;
                }
                let mode = info
                    .modes()
                    .iter()
                    .copied()
                    .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                    .or_else(|| info.modes().first().copied())?;
                let current_crtc = info
                    .current_encoder()
                    .and_then(|encoder| device.get_encoder(encoder).ok())
                    .and_then(|encoder| encoder.crtc());
                let compatible_crtc = info.encoders().iter().find_map(|encoder| {
                    let encoder = device.get_encoder(*encoder).ok()?;
                    resources
                        .filter_crtcs(encoder.possible_crtcs())
                        .into_iter()
                        .next()
                });
                Some((info, current_crtc.or(compatible_crtc)?, mode))
            })
            .context("no connected DRM output with a compatible CRTC")
    }

    /// Creates and maps the Wayland output for the connected DRM monitor.
    fn create_output(
        state: &mut State,
        connector: &connector::Info,
        drm_mode: smithay::reexports::drm::control::Mode,
    ) -> Output {
        let output_name = format!(
            "{OUTPUT_NAME}-{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        let display_mode = Mode::from(drm_mode);
        let (width_mm, height_mm) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            output_name,
            PhysicalProperties {
                size: (width_mm as i32, height_mm as i32).into(),
                subpixel: Subpixel::from(connector.subpixel()),
                make: "Unknown".into(),
                model: "DRM output".into(),
            },
        );
        output.create_global::<State>(&state.display_handle);
        output.set_preferred(display_mode);
        output.change_current_state(
            Some(display_mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        state.install_output(output.clone());
        output
    }

    /// Connects DRM vblank events to frame completion and Wayland callbacks.
    fn insert_drm_events(
        event_loop: &mut EventLoop<'static, State>,
        loop_handle: &LoopHandle<'static, State>,
        backend: &Rc<RefCell<TtyBackend>>,
        notifier: smithay::backend::drm::DrmDeviceNotifier,
    ) {
        let drm_backend = Rc::clone(backend);
        let redraw_backend = Rc::clone(backend);
        let redraw_handle = loop_handle.clone();
        event_loop
            .handle()
            .insert_source(notifier, move |event, _, state| match event {
                DrmEvent::VBlank(crtc) => {
                    let output = {
                        let mut backend = drm_backend.borrow_mut();
                        if crtc != backend.crtc || !backend.frame_pending {
                            return;
                        }
                        if let Err(error) = backend.drm_output.frame_submitted() {
                            state.stop_with_error(anyhow!("complete DRM frame: {error:?}"));
                            return;
                        }
                        backend.frame_pending = false;
                        backend.output.clone()
                    };
                    Self::send_frame_callbacks(state, &output);
                    Self::schedule_pending_redraw(&redraw_handle, &redraw_backend);
                }
                DrmEvent::Error(error) => {
                    if drm_backend.borrow().session_active {
                        state.stop_with_error(anyhow!("DRM event failed: {error:?}"));
                    }
                }
            })
            .expect("register DRM event source");
    }

    /// Connects libseat activation events to DRM and libinput session state.
    fn insert_session_events(
        event_loop: &mut EventLoop<'static, State>,
        loop_handle: &LoopHandle<'static, State>,
        backend: &Rc<RefCell<TtyBackend>>,
        notifier: smithay::backend::session::libseat::LibSeatSessionNotifier,
        mut libinput: Libinput,
    ) {
        let session_backend = Rc::clone(backend);
        let redraw_backend = Rc::clone(backend);
        let redraw_handle = loop_handle.clone();
        event_loop
            .handle()
            .insert_source(notifier, move |event, _, state| match event {
                SessionEvent::PauseSession => {
                    libinput.suspend();
                    let mut backend = session_backend.borrow_mut();
                    backend.session_active = false;
                    backend.output_manager.pause();
                }
                SessionEvent::ActivateSession => {
                    if libinput.resume().is_err() {
                        state.stop_with_error(anyhow!("resume libinput after TTY switch"));
                        return;
                    }
                    let mut backend = session_backend.borrow_mut();
                    if let Err(error) = backend.output_manager.activate(false) {
                        state.stop_with_error(anyhow!("reactivate DRM output: {error:?}"));
                        return;
                    }
                    if backend.frame_pending {
                        if let Err(error) = backend.drm_output.frame_submitted() {
                            state.stop_with_error(anyhow!(
                                "complete pending DRM frame after VT switch: {error:?}"
                            ));
                            return;
                        }
                        backend.frame_pending = false;
                    }
                    backend.session_active = true;
                    drop(backend);
                    Self::request_redraw(&redraw_handle, &redraw_backend);
                }
            })
            .expect("register libseat event source");
    }

    /// Marks output content dirty and schedules a render when the output is ready.
    fn request_redraw(handle: &LoopHandle<'static, State>, backend: &Rc<RefCell<TtyBackend>>) {
        backend.borrow_mut().needs_redraw = true;
        Self::schedule_pending_redraw(handle, backend);
    }

    /// Schedules a marked repaint once the DRM output has no pageflip pending.
    fn schedule_pending_redraw(
        handle: &LoopHandle<'static, State>,
        backend: &Rc<RefCell<TtyBackend>>,
    ) {
        let should_schedule = {
            let mut backend = backend.borrow_mut();
            if !backend.session_active
                || !backend.needs_redraw
                || backend.frame_pending
                || backend.redraw_scheduled
            {
                false
            } else {
                backend.redraw_scheduled = true;
                true
            }
        };
        if should_schedule {
            let backend = Rc::clone(backend);
            handle.insert_idle(move |state| {
                if let Err(error) = backend.borrow_mut().render(state) {
                    state.stop_with_error(error.context("render TTY output"));
                }
            });
        }
    }

    /// Sends frame callbacks after output presentation or when no repaint was needed.
    fn send_frame_callbacks(state: &mut State, output: &Output) {
        state.space.elements().for_each(|window| {
            window.send_frame(
                output,
                state.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        });
        state.space.refresh();
        let _ = state.display_handle.flush_clients();
    }
}

/// Renders the terminal and native surfaces to the physical output.
impl TtyBackend {
    /// Renders and queues a frame when the output is ready.
    fn render(&mut self, state: &mut State) -> Result<()> {
        self.redraw_scheduled = false;
        if !self.session_active {
            self.needs_redraw = true;
            return Ok(());
        }
        if self.frame_pending {
            self.needs_redraw = true;
            return Ok(());
        }
        self.needs_redraw = false;

        let mut elements = Vec::new();
        let space_elements =
            space_render_elements(&mut self.renderer, [&state.space], &self.output, 1.0)
                .context("render Wayland surfaces for TTY output")?;
        elements.extend(space_elements.into_iter().map(OutputElement::Space));
        if let Some(terminal) = state.terminal_renderer.as_mut() {
            elements.push(OutputElement::Terminal(
                MemoryRenderBufferRenderElement::from_buffer(
                    &mut self.renderer,
                    (0.0, 0.0),
                    terminal.pixels(),
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
                .context("upload terminal buffer to DRM output")?,
            ));
        }

        let frame = self
            .drm_output
            .render_frame(
                &mut self.renderer,
                &elements,
                OUTPUT_CLEAR_COLOR,
                smithay::backend::drm::compositor::FrameFlags::empty(),
            )
            .map_err(|error| anyhow!("render DRM frame: {error:?}"))?;
        if frame.is_empty {
            drop(frame);
            TtyOutput::send_frame_callbacks(state, &self.output);
            return Ok(());
        }
        if frame.needs_sync()
            && let smithay::backend::drm::compositor::PrimaryPlaneElement::Swapchain(element) =
                &frame.primary_element
        {
            element
                .sync
                .wait()
                .context("wait for DRM frame rendering")?;
        }
        drop(frame);
        self.drm_output
            .queue_frame(())
            .map_err(|error| anyhow!("queue DRM frame: {error:?}"))?;
        self.frame_pending = true;
        Ok(())
    }
}

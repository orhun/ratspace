use std::{cell::RefCell, rc::Rc};

use anyhow::{Context, Result, anyhow};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::{Kind, memory::MemoryRenderBufferRenderElement},
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use super::state::State;

smithay::backend::renderer::element::render_elements! {
    OutputElement<=GlesRenderer>;
    Terminal=MemoryRenderBufferRenderElement<GlesRenderer>,
}

const OUTPUT_NAME: &str = env!("CARGO_PKG_NAME");
const OUTPUT_REFRESH: i32 = 60_000;
const OUTPUT_CLEAR_COLOR: [f32; 4] = [0.04, 0.04, 0.04, 1.0];

/// Owns the initial size of Ratspace's nested Winit output.
pub(super) struct WinitOutput {
    size: (i32, i32),
    scale_factor: f64,
}

/// Creates the nested window, advertises its output, and renders its events.
impl WinitOutput {
    /// Initializes a Winit window and registers it with the compositor event loop.
    pub(super) fn init(
        event_loop: &mut EventLoop<'static, State>,
        state: &mut State,
    ) -> Result<Self> {
        let (backend, winit_events) = winit::init::<GlesRenderer>()
            .map_err(|error| anyhow!("initialize nested Winit output: {error:?}"))?;
        let size = backend.window_size();
        let scale_factor = backend.scale_factor();
        let output = Self::create_output(state, size, scale_factor);
        let mut damage_tracker = OutputDamageTracker::from_output(&output);
        let backend = Rc::new(RefCell::new(backend));
        let redraw_backend = Rc::clone(&backend);
        state.set_redraw_requester(Rc::new(move || {
            redraw_backend.borrow().window().request_redraw();
        }));

        event_loop
            .handle()
            .insert_source(winit_events, move |event, _, state| {
                Self::handle_event(event, state, &mut backend.borrow_mut(), &mut damage_tracker);
            })
            .map_err(|error| anyhow!("register Winit event source: {error:?}"))?;

        Ok(Self {
            size: (size.w, size.h),
            scale_factor,
        })
    }

    /// Returns the nested window's initial pixel dimensions.
    pub(super) fn size(&self) -> (i32, i32) {
        self.size
    }

    /// Returns the nested window's display scale factor.
    pub(super) fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    /// Registers an output global and maps it at the origin.
    fn create_output(
        state: &mut State,
        size: smithay::utils::Size<i32, smithay::utils::Physical>,
        scale_factor: f64,
    ) -> Output {
        let output = Output::new(
            OUTPUT_NAME.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Ratspace".into(),
                model: "Winit".into(),
            },
        );
        output.create_global::<State>(&state.display_handle);
        let mode = Mode {
            size,
            refresh: OUTPUT_REFRESH,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Flipped180),
            Some(Scale::Fractional(scale_factor)),
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        state.install_output(output.clone());
        output
    }

    /// Dispatches a Winit event to input handling, resize handling, or rendering.
    fn handle_event(
        event: WinitEvent,
        state: &mut State,
        backend: &mut WinitGraphicsBackend<GlesRenderer>,
        damage_tracker: &mut OutputDamageTracker,
    ) {
        match event {
            WinitEvent::Resized { size, scale_factor } => {
                if let Err(error) = state.set_output_size(size.w, size.h, scale_factor) {
                    state.stop_with_error(error.context("resize compositor output"));
                } else {
                    backend.window().request_redraw();
                }
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                if let Err(error) = Self::render_frame(backend, state, damage_tracker) {
                    state.stop_with_error(error.context("render compositor output"));
                }
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            WinitEvent::Focus(_) => {}
        }
    }

    /// Renders the terminal buffer into the nested window and schedules another frame.
    fn render_frame(
        backend: &mut WinitGraphicsBackend<GlesRenderer>,
        state: &mut State,
        damage_tracker: &mut OutputDamageTracker,
    ) -> Result<()> {
        let size = backend.window_size();
        let damage = Rectangle::from_size(size);
        {
            let (renderer, mut framebuffer) = backend.bind().context("bind Winit framebuffer")?;
            let mut elements = Vec::new();
            if let Some(terminal) = state.terminal_renderer.as_mut() {
                elements.push(OutputElement::Terminal(
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        (0.0, 0.0),
                        terminal.pixels(),
                        None,
                        None,
                        None,
                        Kind::Unspecified,
                    )
                    .context("upload terminal buffer")?,
                ));
            }
            damage_tracker
                .render_output(renderer, &mut framebuffer, 0, &elements, OUTPUT_CLEAR_COLOR)
                .context("render terminal buffer")?;
        }

        let _ = state.display_handle.flush_clients();
        backend
            .submit(Some(&[damage]))
            .context("submit Winit frame")?;
        Ok(())
    }
}

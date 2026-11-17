use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::surface::WaylandSurfaceRenderElement,
            element::{Kind, memory::MemoryRenderBufferRenderElement},
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::space::{SpaceRenderElements, space_render_elements},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use super::state::State;

smithay::backend::renderer::element::render_elements! {
    OutputElement<=GlesRenderer>;
    Client=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    Terminal=MemoryRenderBufferRenderElement<GlesRenderer>,
}

const OUTPUT_NAME: &str = env!("CARGO_PKG_NAME");
const OUTPUT_MAKE: &str = "Ratspace";
const OUTPUT_MODEL: &str = "Winit";
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
        let (mut backend, winit_events) = winit::init::<GlesRenderer>()
            .map_err(|error| anyhow!("initialize nested Winit output: {error:?}"))?;
        backend.window().request_redraw();
        let size = backend.window_size();
        let scale_factor = backend.scale_factor();
        tracing::info!(
            width = size.w,
            height = size.h,
            scale_factor,
            "created nested Winit output"
        );
        let output = Self::create_output(state, size, scale_factor);
        let mut damage_tracker = OutputDamageTracker::from_output(&output);

        event_loop
            .handle()
            .insert_source(winit_events, move |event, _, state| {
                Self::handle_event(event, state, &mut backend, &output, &mut damage_tracker);
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
                make: OUTPUT_MAKE.into(),
                model: OUTPUT_MODEL.into(),
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

    fn handle_event(
        event: WinitEvent,
        state: &mut State,
        backend: &mut WinitGraphicsBackend<GlesRenderer>,
        output: &Output,
        damage_tracker: &mut OutputDamageTracker,
    ) {
        match event {
            WinitEvent::Resized { size, scale_factor } => {
                tracing::info!(
                    width = size.w,
                    height = size.h,
                    scale_factor,
                    "resized compositor output"
                );
                if let Err(error) = state.set_output_size(size.w, size.h, scale_factor) {
                    tracing::error!(error = ?error, "failed to resize compositor output");
                    state.loop_signal.stop();
                }
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                if let Err(error) = Self::render_frame(backend, state, output, damage_tracker) {
                    tracing::error!(error = ?error, "failed to render compositor output");
                    state.loop_signal.stop();
                }
            }
            WinitEvent::CloseRequested => {
                tracing::info!("compositor window closed");
                state.loop_signal.stop();
            }
            WinitEvent::Focus(focused) => tracing::debug!(focused, "compositor focus changed"),
        }
    }

    fn render_frame(
        backend: &mut WinitGraphicsBackend<GlesRenderer>,
        state: &mut State,
        output: &Output,
        damage_tracker: &mut OutputDamageTracker,
    ) -> Result<()> {
        let size = backend.window_size();
        let damage = Rectangle::from_size(size);
        {
            let (renderer, mut framebuffer) = backend.bind().context("bind Winit framebuffer")?;
            let mut elements = space_render_elements(renderer, [&state.space], output, 1.0)
                .context("collect client render elements")?
                .into_iter()
                .map(OutputElement::Client)
                .collect::<Vec<_>>();
            // Render elements are front-to-back, so keep the terminal behind client surfaces.
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
                .context("render terminal and client surfaces")?;
        }

        state.space.elements().for_each(|window| {
            window.send_frame(
                output,
                state.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        });
        state.space.refresh();
        state.popups.cleanup();
        let _ = state.display_handle.flush_clients();
        backend
            .submit(Some(&[damage]))
            .context("submit Winit frame")?;
        backend.window().request_redraw();
        Ok(())
    }
}

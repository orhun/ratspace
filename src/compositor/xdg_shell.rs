use smithay::{
    desktop::{Space, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Size},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

/// Sends the initial XDG configure when an application surface commits.
pub(super) fn handle_commit(space: &Space<Window>, surface: &WlSurface) {
    let Some(window) = space.elements().find(|window| {
        window
            .toplevel()
            .is_some_and(|toplevel| toplevel.wl_surface() == surface)
    }) else {
        return;
    };

    let initial_configure_sent = with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .expect("XDG toplevel surface state")
            .lock()
            .expect("XDG toplevel surface state lock")
            .initial_configure_sent
    });
    if !initial_configure_sent {
        let toplevel = window
            .toplevel()
            .expect("mapped Wayland window has an XDG toplevel");
        let size = space
            .outputs()
            .next()
            .and_then(|output| space.output_geometry(output))
            .map(|geometry| geometry.size);
        toplevel.with_pending_state(|state| state.size = size);
        toplevel.send_configure();
    }
}

/// Configures mapped toplevels to match the output's logical dimensions.
pub(super) fn resize_toplevels(space: &Space<Window>, size: Size<i32, Logical>) {
    for window in space.elements() {
        let Some(toplevel) = window.toplevel() else {
            continue;
        };
        toplevel.with_pending_state(|state| state.size = Some(size));
        toplevel.send_pending_configure();
    }
}

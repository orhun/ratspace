/// Name of the terminal type environment variable.
pub(crate) const TERM: &str = "TERM";
/// Terminal type expected by shell programs.
pub(crate) const TERM_VALUE: &str = "xterm-256color";

/// Name of the Wayland socket environment variable.
pub(crate) const WAYLAND_DISPLAY: &str = "WAYLAND_DISPLAY";
/// Name of the host X11 display environment variable.
pub(crate) const DISPLAY: &str = "DISPLAY";

/// Name of GTK's backend environment variable.
pub(crate) const GDK_BACKEND: &str = "GDK_BACKEND";
/// Selects Wayland for GTK applications.
pub(crate) const GDK_BACKEND_VALUE: &str = "wayland";
/// Name of Firefox's Wayland environment variable.
pub(crate) const MOZ_ENABLE_WAYLAND: &str = "MOZ_ENABLE_WAYLAND";
/// Enables Firefox's Wayland backend.
pub(crate) const MOZ_ENABLE_WAYLAND_VALUE: &str = "1";
/// Name of the session type environment variable.
pub(crate) const XDG_SESSION_TYPE: &str = "XDG_SESSION_TYPE";
/// Identifies child applications as Wayland clients.
pub(crate) const XDG_SESSION_TYPE_VALUE: &str = "wayland";

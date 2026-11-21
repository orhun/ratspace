/// Name of the terminal type environment variable.
pub(crate) const TERM: &str = "TERM";
/// Terminal type expected by shell programs.
pub(crate) const TERM_VALUE: &str = "xterm-256color";

/// Environment variable used to select the shell started by Ratspace.
pub(crate) const RATSPACE_SHELL: &str = "RATSPACE_SHELL";
/// Name of the shell environment variable passed to child programs.
pub(crate) const SHELL: &str = "SHELL";

/// Name of the Wayland display environment variable.
pub(crate) const WAYLAND_DISPLAY: &str = "WAYLAND_DISPLAY";

/// Font size used to rasterize terminal text.
pub(crate) const TERMINAL_FONT_SIZE: f32 = 24.0;
/// Preferred terminal font, with the system monospace font as a fallback.
pub(crate) const TERMINAL_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
/// Default terminal foreground color as a hex string.
pub(crate) const TERMINAL_DEFAULT_FG: &str = "#DCDCDC";
/// Default terminal background color as a hex string.
pub(crate) const TERMINAL_DEFAULT_BG: &str = "#101010";
/// Hex colors for the first sixteen ANSI indexed colors.
pub(crate) const TERMINAL_ANSI_PALETTE: [&str; 16] = [
    "#000000", "#CD3131", "#0DBC79", "#E5E510", "#2472C8", "#BC3FBC", "#11A8CD", "#E5E5E5",
    "#666666", "#F14C4C", "#23D18B", "#F5F543", "#3B8EEA", "#D670D6", "#29B8DB", "#FFFFFF",
];

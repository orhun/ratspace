mod buffer;

pub mod app;
pub mod input;
pub mod pty;
pub mod render;

use anyhow::{Context, Result};
use fux_vt::{Mode, Parser, Screen, Sink, Size, keys::encode::KeyMode};

/// Emulates the VT screen used by the shell running in Ratspace.
pub struct Terminal {
    emulator: Parser,
}

impl Terminal {
    /// Creates an empty VT screen with the given column and row counts.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        let size = Size::new(rows.max(1), cols.max(1)).context("make terminal size")?;
        let emulator = Parser::with_options(size, 0, fux_vt::Options::default())
            .context("create terminal emulator")?;
        Ok(Self { emulator })
    }

    /// Returns the current emulated screen for rendering.
    pub fn screen(&self) -> &Screen {
        self.emulator.screen()
    }

    /// Parses shell output and returns any terminal query replies.
    pub fn process_output(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut sink = ReplySink::default();
        let mut remaining = bytes;
        while !remaining.is_empty() || self.emulator.has_work_left() {
            match self
                .emulator
                .process_until_stop(remaining, &mut sink)
                .context("parse terminal output")?
            {
                Some((consumed, _)) => {
                    remaining = remaining
                        .get(consumed..)
                        .context("terminal parser consumed beyond output")?;
                    if consumed == 0 && !self.emulator.has_work_left() {
                        break;
                    }
                }
                None => break,
            }
        }
        Ok(sink.replies)
    }

    /// Resizes the VT screen and returns an optional in-band resize report.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<Option<Vec<u8>>> {
        let size = Size::new(rows.max(1), cols.max(1)).context("make terminal size")?;
        self.emulator
            .resize(size)
            .context("resize terminal emulator")?;
        Ok(self.emulator.resize_report())
    }

    /// Returns the key encoding mode requested by the running program.
    pub fn key_mode(&self) -> KeyMode {
        self.emulator.screen().key_mode()
    }

    /// Returns whether the running program requested bracketed paste.
    pub fn bracketed_paste(&self) -> bool {
        self.emulator.screen().mode(Mode::BracketedPaste)
    }
}

#[derive(Default)]
struct ReplySink {
    replies: Vec<u8>,
}

impl Sink for ReplySink {
    fn reply(&mut self, bytes: &[u8]) {
        self.replies.extend_from_slice(bytes);
    }
}

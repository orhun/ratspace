use std::{
    io::{Read, Write},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, anyhow};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Owns the shell PTY and relays its output to the app loop.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    reader_thread: Option<JoinHandle<()>>,
    child: ChildGuard,
}

/// The result of polling the shell PTY's output queue.
pub enum OutputPoll {
    /// Bytes are available to parse and render.
    Data(Vec<u8>),
    /// The reader has no new bytes yet.
    Empty,
    /// The shell closed its PTY output.
    Closed,
}

impl PtySession {
    /// Starts the user's default shell at the requested terminal size.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(pty_size(cols, rows))
            .context("create shell PTY")?;
        let mut reader = pair.master.try_clone_reader().context("open PTY reader")?;
        let writer = pair.master.take_writer().context("open PTY writer")?;
        let mut command = CommandBuilder::new_default_prog();
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .context("start default shell")?;
        drop(pair.slave);
        let child = ChildGuard::new(child);

        let (output_tx, output) = mpsc::channel();
        let reader_thread = thread::Builder::new()
            .name("ratspace-pty-reader".into())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            if output_tx.send(buffer[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .context("start PTY reader")?;

        Ok(Self {
            master: pair.master,
            writer,
            output,
            reader_thread: Some(reader_thread),
            child,
        })
    }

    /// Sends input or emulator replies to the shell.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("write to shell PTY")?;
        self.writer.flush().context("flush shell PTY")
    }

    /// Updates the shell's PTY dimensions.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(pty_size(cols, rows))
            .context("resize shell PTY")
    }

    /// Reads one queued output chunk without blocking.
    pub fn poll_output(&self) -> OutputPoll {
        match self.output.try_recv() {
            Ok(bytes) => OutputPoll::Data(bytes),
            Err(TryRecvError::Empty) => OutputPoll::Empty,
            Err(TryRecvError::Disconnected) => OutputPoll::Closed,
        }
    }

    /// Waits for the shell and reaps the PTY reader thread.
    pub fn wait(&mut self) -> Result<()> {
        self.child.wait()?;
        if let Some(reader_thread) = self.reader_thread.take() {
            reader_thread
                .join()
                .map_err(|_| anyhow!("PTY reader thread panicked"))?;
        }
        Ok(())
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

struct ChildGuard {
    child: Box<dyn Child + Send + Sync>,
    waited: bool,
}

impl ChildGuard {
    fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self {
            child,
            waited: false,
        }
    }

    fn wait(&mut self) -> Result<()> {
        self.child.wait().context("wait for shell")?;
        self.waited = true;
        Ok(())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.waited {
            let _ = self.child.kill();
        }
    }
}

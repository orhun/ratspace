use std::{
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, anyhow};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::config;

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
    /// Starts the selected shell at the requested terminal size.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(Self::pty_size(cols, rows))
            .context("create shell PTY")?;
        let mut reader = pair.master.try_clone_reader().context("open PTY reader")?;
        let writer = pair.master.take_writer().context("open PTY writer")?;
        let configured_shell = env::var_os(config::RATSPACE_SHELL);
        let mut command = if let Some(shell) = configured_shell.as_deref() {
            CommandBuilder::new(shell)
        } else {
            let default = CommandBuilder::new_default_prog();
            let shell = default.get_shell();
            anyhow::ensure!(
                !Self::is_current_executable(Path::new(&shell)),
                "the configured shell is Ratspace itself; set RATSPACE_SHELL to your real shell"
            );
            default
        };
        command.env(config::TERM, config::TERM_VALUE);
        if let Some(shell) = configured_shell.as_deref() {
            command.env(config::SHELL, shell);
            anyhow::ensure!(
                !Self::is_current_executable(Path::new(shell)),
                "{} points to Ratspace itself; set it to your real shell",
                config::RATSPACE_SHELL
            );
        }
        let child = pair
            .slave
            .spawn_command(command)
            .context("start shell in PTY")?;
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
            .resize(Self::pty_size(cols, rows))
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

    /// Builds a PTY size with nonzero cell dimensions.
    fn pty_size(cols: u16, rows: u16) -> PtySize {
        PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Checks whether a shell path resolves to the current Ratspace executable.
    fn is_current_executable(shell: &Path) -> bool {
        let Ok(current) = env::current_exe() else {
            return false;
        };
        let Some(shell) = Self::resolve_program(shell) else {
            return false;
        };
        Self::same_path(&current, &shell)
    }

    /// Resolves a program name through PATH when it has no directory component.
    fn resolve_program(program: &Path) -> Option<PathBuf> {
        if program.components().count() > 1 {
            return Some(program.to_path_buf());
        }
        env::split_paths(&env::var_os("PATH")?)
            .map(|directory| directory.join(program))
            .find(|path| path.is_file())
    }

    /// Compares paths after canonicalization when possible.
    fn same_path(left: &Path, right: &Path) -> bool {
        match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => left == right,
        }
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

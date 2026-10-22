//! A pane: one shell running in a PTY, plus the terminal emulator state
//! (alacritty_terminal) that its output is fed into.

use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::app::AppEvent;

pub type PaneId = usize;

/// Lines of scrollback kept per pane.
const SCROLLBACK: usize = 10_000;

/// Size handed to alacritty's `Term`. It only needs the `Dimensions` trait.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Receives events the emulator can't handle by itself.
#[derive(Clone)]
pub struct Listener {
    id: PaneId,
    writer: SharedWriter,
    tx: Sender<AppEvent>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            // Replies to queries such as "where is the cursor?" (DSR) or
            // "what are you?" (DA). Apps like vim block waiting for these.
            TermEvent::PtyWrite(text) => {
                let mut w = self.writer.lock().unwrap();
                let _ = w.write_all(text.as_bytes());
                let _ = w.flush();
            }
            TermEvent::Title(title) => {
                let _ = self.tx.send(AppEvent::Title(self.id, Some(title)));
            }
            TermEvent::ResetTitle => {
                let _ = self.tx.send(AppEvent::Title(self.id, None));
            }
            _ => {}
        }
    }
}

pub struct Pane {
    /// Title set by the program via OSC 0/2, if any.
    pub title: Option<String>,
    pub term: Arc<Mutex<Term<Listener>>>,
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: TermSize,
}

impl Pane {
    /// Spawn the user's login shell in a new PTY and start a reader thread
    /// that feeds its output into the emulator.
    pub fn spawn(id: PaneId, size: TermSize, tx: Sender<AppEvent>) -> Result<Pane> {
        let size = clamp_size(size);
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new_default_prog();
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        // We emulate an xterm-compatible terminal with truecolor, not the
        // outer terminal, so don't let apps think they are talking to Ghostty.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("HAUNT", "1");
        cmd.env_remove("TERM_PROGRAM");
        cmd.env_remove("TERM_PROGRAM_VERSION");

        let child = pair.slave.spawn_command(cmd).context("spawning shell")?;
        // Drop our copy of the slave side, so reads on the master hit EOF
        // once the shell exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer()?));

        let listener = Listener {
            id,
            writer: writer.clone(),
            tx: tx.clone(),
        };
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Config::default()
        };
        let term = Arc::new(Mutex::new(Term::new(config, &size, listener)));

        let term_for_reader = term.clone();
        thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut buf = [0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut term = term_for_reader.lock().unwrap();
                        parser.advance(&mut *term, &buf[..n]);
                        drop(term);
                        if tx.send(AppEvent::PaneOutput).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = tx.send(AppEvent::PaneExited(id));
        });

        Ok(Pane {
            title: None,
            term,
            writer,
            master: pair.master,
            child,
            size,
        })
    }

    /// Send raw bytes to the program (keyboard input, mouse reports, paste).
    pub fn write(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Resize both the PTY (so the program gets SIGWINCH) and the emulator
    /// grid. No-op if the size is unchanged.
    pub fn resize(&mut self, size: TermSize) {
        let size = clamp_size(size);
        if size == self.size {
            return;
        }
        self.size = size;
        let _ = self.master.resize(pty_size(size));
        self.term.lock().unwrap().resize(size);
    }

    pub fn scroll(&self, scroll: Scroll) {
        self.term.lock().unwrap().scroll_display(scroll);
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// alacritty's grid needs at least 2 columns and 1 line.
fn clamp_size(size: TermSize) -> TermSize {
    TermSize {
        cols: size.cols.max(2),
        rows: size.rows.max(1),
    }
}

fn pty_size(size: TermSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

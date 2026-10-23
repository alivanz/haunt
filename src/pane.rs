//! A pane: one shell running in a PTY, plus the terminal emulator state
//! (alacritty_terminal) that its output is fed into.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::app::AppEvent;
use crate::snapshot;

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

/// How to start a pane. The default is a fresh shell in haunt's directory.
#[derive(Default)]
pub struct SpawnOptions {
    /// Working directory for the shell.
    pub cwd: Option<PathBuf>,
    /// Text with ANSI codes to show above the first prompt (a restored
    /// session's history).
    pub history: Option<String>,
}

pub struct Pane {
    /// Title set by the program via OSC 0/2, if any.
    pub title: Option<String>,
    pub term: Arc<Mutex<Term<Listener>>>,
    /// Last main-screen text produced by `history()`. The reader thread also
    /// fills it just before a program switches to the alternate screen,
    /// since the main screen can't be read while vim/htop/... is open.
    history: Arc<Mutex<Option<String>>>,
    /// Set by the reader thread on new output; `history` is stale.
    changed: Arc<AtomicBool>,
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: TermSize,
}

impl Pane {
    /// Spawn the user's login shell in a new PTY and start a reader thread
    /// that feeds its output into the emulator.
    pub fn spawn(
        id: PaneId,
        size: TermSize,
        tx: Sender<AppEvent>,
        opts: SpawnOptions,
    ) -> Result<Pane> {
        let size = clamp_size(size);
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new_default_prog();
        // A saved directory may have been deleted since; fall back to ours.
        let cwd = opts
            .cwd
            .filter(|d| d.is_dir())
            .or_else(|| std::env::current_dir().ok());
        if let Some(cwd) = cwd {
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
        let mut term = Term::new(config, &size, listener);
        if let Some(text) = opts.history.filter(|t| !t.is_empty()) {
            // The shell is already running, but its output waits in the PTY
            // until the reader thread starts, so this lands above the prompt.
            let mut parser: Processor = Processor::new();
            parser.advance(&mut term, text.as_bytes());
            parser.advance(&mut term, "\r\n\x1b[2m── restored ──\x1b[0m\r\n".as_bytes());
        }
        let term = Arc::new(Mutex::new(term));
        let history = Arc::new(Mutex::new(None));
        let changed = Arc::new(AtomicBool::new(true));

        let term_for_reader = term.clone();
        let history_for_reader = history.clone();
        let changed_for_reader = changed.clone();
        thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut buf = [0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut term = term_for_reader.lock().unwrap();
                        let mut chunk = &buf[..n];
                        // Copy the main screen right before it gets hidden.
                        while let Some((start, len)) = find_alt_screen_enter(chunk) {
                            parser.advance(&mut *term, &chunk[..start]);
                            if !term.mode().contains(TermMode::ALT_SCREEN) {
                                let text = snapshot::to_ansi(term.grid());
                                *history_for_reader.lock().unwrap() = Some(text);
                            }
                            parser.advance(&mut *term, &chunk[start..start + len]);
                            chunk = &chunk[start + len..];
                        }
                        parser.advance(&mut *term, chunk);
                        drop(term);
                        changed_for_reader.store(true, Ordering::Relaxed);
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
            history,
            changed,
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

    /// Main-screen text (scrollback + screen, with colors) for saving.
    /// Re-rendered only when there was output since the last call.
    pub fn history(&self) -> String {
        let term = self.term.lock().unwrap();
        let mut cached = self.history.lock().unwrap();
        // While a full-screen program runs, the copy taken before it
        // started is the best we have.
        if !term.mode().contains(TermMode::ALT_SCREEN)
            && (self.changed.swap(false, Ordering::Relaxed) || cached.is_none())
        {
            *cached = Some(snapshot::to_ansi(term.grid()));
        }
        cached.clone().unwrap_or_default()
    }

    /// The shell's current working directory.
    pub fn cwd(&self) -> Option<PathBuf> {
        process_cwd(self.child.process_id()?)
    }
}

/// Find the first "switch to alternate screen" sequence (xterm modes 1049,
/// 1047 and 47), as (offset, length). A sequence split across two reads is
/// missed; the cost is a slightly older copy of the main screen.
fn find_alt_screen_enter(bytes: &[u8]) -> Option<(usize, usize)> {
    [&b"\x1b[?1049h"[..], b"\x1b[?1047h", b"\x1b[?47h"]
        .iter()
        .filter_map(|seq| {
            let pos = bytes.windows(seq.len()).position(|w| w == *seq)?;
            Some((pos, seq.len()))
        })
        .min()
}

#[cfg(target_os = "macos")]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::ffi::CStr;
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // vip_path is a NUL-terminated path stored as [[c_char; 32]; 32].
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    let path = path.to_str().ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(target_os = "linux")]
fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_alt_screen_switch() {
        assert_eq!(find_alt_screen_enter(b"abc\x1b[?1049hdef"), Some((3, 8)));
        assert_eq!(find_alt_screen_enter(b"\x1b[?47h"), Some((0, 6)));
        // The earliest one wins.
        assert_eq!(
            find_alt_screen_enter(b"x\x1b[?1047h\x1b[?1049h"),
            Some((1, 8))
        );
        assert_eq!(find_alt_screen_enter(b"\x1b[?1049l plain"), None);
    }

    #[test]
    fn cwd_of_this_process() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(process_cwd(std::process::id()), Some(here));
    }
}

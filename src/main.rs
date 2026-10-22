mod app;
mod input;
mod layout;
mod pane;
mod ui;

use std::io::{Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

fn main() -> Result<()> {
    // Restore the outer terminal before the panic message is printed, so
    // it's readable and the shell isn't left in raw mode. Exit right away
    // too: a panic in a PTY reader thread would otherwise leave the UI
    // running on a terminal we just restored.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
        std::process::exit(101);
    }));

    setup_terminal()?;
    // Restores on normal return and on `?` errors (and on unwinding panics).
    let _guard = RestoreGuard;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let size = terminal.size()?;
    app::App::new(ratatui::layout::Rect::new(0, 0, size.width, size.height))?.run(&mut terminal)
}

fn setup_terminal() -> Result<()> {
    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        // Kitty keyboard protocol, so Ctrl+Shift+T arrives as something
        // different from Ctrl+T. Pushed after entering the alternate screen,
        // since the terminal keeps a separate flag stack per screen.
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    Ok(())
}

/// Undo everything `setup_terminal` and the app did. Runs at most once, so
/// the keyboard flag stack isn't popped twice.
fn restore_terminal() {
    static RESTORED: AtomicBool = AtomicBool::new(false);
    if RESTORED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut out = stdout();
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        PopKeyboardEnhancementFlags,
        SetCursorStyle::DefaultUserShape,
        cursor::Show,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    let _ = out.flush();
}

struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

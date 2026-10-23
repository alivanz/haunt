//! Setting up and restoring the terminal haunt (or a `haunt attach` client)
//! runs in.

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

/// Switch the terminal to the UI and return a guard that switches it back
/// when dropped. Panics restore it too, and exit right away: a panic in a
/// background thread would otherwise leave the UI running on a terminal we
/// just restored.
pub fn setup() -> Result<RestoreGuard> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Restore before the message is printed, so it's readable and the
        // shell isn't left in raw mode.
        restore();
        default_hook(info);
        std::process::exit(101);
    }));

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
    Ok(RestoreGuard)
}

/// Undo everything `setup` and the UI did. Runs at most once, so the
/// keyboard flag stack isn't popped twice.
pub fn restore() {
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

/// Restores the terminal on normal return, on `?` errors and on unwinding.
pub struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        restore();
    }
}

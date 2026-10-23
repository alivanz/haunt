mod app;
mod input;
mod layout;
mod pane;
mod session;
mod snapshot;
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

const USAGE: &str = "\
usage: haunt [-s NAME]

  -s, --session NAME   restore session NAME if it was saved, and keep saving it
      --sessions       list saved sessions
  -h, --help           show this help";

#[derive(Debug, PartialEq)]
enum Command {
    /// Start haunt, optionally with a named session.
    Run(Option<String>),
    ListSessions,
    Help,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut session = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-s" | "--session" => {
                let name = args.next().ok_or("-s needs a session name")?;
                if !session::valid_name(&name) {
                    return Err(format!(
                        "invalid session name '{name}' (use letters, digits, '-', '_', '.')"
                    ));
                }
                session = Some(name);
            }
            "--sessions" => return Ok(Command::ListSessions),
            "-h" | "--help" => return Ok(Command::Help),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Command::Run(session))
}

fn list_sessions() {
    let sessions = session::list();
    if sessions.is_empty() {
        println!("No saved sessions. Start one with: haunt -s NAME");
        return;
    }
    let width = sessions.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (name, saved) in sessions {
        println!("{name:<width$}  saved {}", session::age(saved));
    }
    println!("\n(stored in {})", session::dir().display());
}

fn main() -> Result<()> {
    let command = parse_args(std::env::args().skip(1)).unwrap_or_else(|msg| {
        eprintln!("haunt: {msg}\n\n{USAGE}");
        std::process::exit(2);
    });
    // Everything that can fail with a message happens before the terminal
    // switches to the UI, so errors print normally.
    let open = match command {
        Command::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Command::ListSessions => {
            list_sessions();
            return Ok(());
        }
        Command::Run(None) => None,
        Command::Run(Some(name)) => {
            let lock = session::Lock::acquire(&name)?;
            let saved = session::load(&session::path(&name))?;
            Some(app::OpenSession { name, lock, saved })
        }
    };

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
    let screen = ratatui::layout::Rect::new(0, 0, size.width, size.height);
    app::App::new(screen, open)?.run(&mut terminal)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn arguments() {
        assert_eq!(parse(&[]), Ok(Command::Run(None)));
        assert_eq!(
            parse(&["-s", "work"]),
            Ok(Command::Run(Some("work".into())))
        );
        assert_eq!(
            parse(&["--session", "a.b"]),
            Ok(Command::Run(Some("a.b".into())))
        );
        assert_eq!(parse(&["--sessions"]), Ok(Command::ListSessions));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        assert!(parse(&["-s"]).is_err());
        assert!(parse(&["-s", "../x"]).is_err());
        assert!(parse(&["--nope"]).is_err());
    }
}

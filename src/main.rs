mod app;
mod client;
mod input;
mod layout;
mod pane;
mod protocol;
mod screen;
mod server;
mod session;
mod snapshot;
mod tty;
mod ui;

use anyhow::Result;
use ratatui::layout::Size;

use crate::app::{App, Output};

const USAGE: &str = "\
usage: haunt [COMMAND]

  haunt               start haunt; nothing is kept once it closes
  haunt attach NAME   show session NAME, starting it if needed (short: a)
  haunt ls            list running and saved sessions
  haunt kill NAME     stop session NAME and delete its saved copy
  haunt help          show this help

A session keeps running in the background: Ctrl+Shift+D detaches from it,
and closing the terminal does too. Ctrl+Shift+Q stops it, saving its tabs
and text for the next attach.";

#[derive(Debug, PartialEq)]
enum Command {
    /// Plain `haunt`, running in this terminal.
    Run,
    Attach(String),
    List,
    Kill(String),
    /// The background server for a session; started by `attach`.
    Server(String),
    Help,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let args: Vec<String> = args.collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let name = |name: &str| {
        if session::valid_name(name) {
            Ok(name.to_string())
        } else {
            Err(format!(
                "invalid session name '{name}' (use letters, digits, '-', '_', '.')"
            ))
        }
    };
    Ok(match args.as_slice() {
        [] => Command::Run,
        ["attach" | "a", n] => Command::Attach(name(n)?),
        ["ls" | "list"] => Command::List,
        ["kill", n] => Command::Kill(name(n)?),
        ["__server", n] => Command::Server(name(n)?),
        ["help" | "-h" | "--help"] => Command::Help,
        [cmd @ ("attach" | "a" | "kill")] => return Err(format!("{cmd} needs a session name")),
        _ => return Err(format!("unknown command '{}'", args.join(" "))),
    })
}

fn main() -> Result<()> {
    let command = parse_args(std::env::args().skip(1)).unwrap_or_else(|msg| {
        eprintln!("haunt: {msg}\n\n{USAGE}");
        std::process::exit(2);
    });
    match command {
        Command::Run => run_here(),
        Command::Attach(name) => client::attach(&name),
        Command::List => {
            list_sessions();
            Ok(())
        }
        Command::Kill(name) => client::kill(&name),
        Command::Server(name) => server::run(name),
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

/// Plain `haunt`: everything in this process, drawn on this terminal.
fn run_here() -> Result<()> {
    // Anything that can fail with a message happens before the terminal
    // switches to the UI, so errors print normally.
    let (cols, rows) = crossterm::terminal::size()?;
    let app = App::new(Size::new(cols, rows), Output::Local, None)?;
    let _restore = tty::setup()?;
    app.run()
}

fn list_sessions() {
    let sessions = session::list();
    if sessions.is_empty() {
        println!("No sessions. Start one with: haunt attach NAME");
        return;
    }
    let width = sessions.iter().map(|s| s.name.len()).max().unwrap_or(0);
    for s in sessions {
        let state = match (s.running, s.saved) {
            (Some(pid), _) => format!("running (pid {pid})"),
            (None, Some(saved)) => format!("saved {}", session::age(saved)),
            (None, None) => continue,
        };
        println!("{:<width$}  {state}", s.name);
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
        assert_eq!(parse(&[]), Ok(Command::Run));
        assert_eq!(
            parse(&["attach", "work"]),
            Ok(Command::Attach("work".into()))
        );
        assert_eq!(parse(&["a", "a.b"]), Ok(Command::Attach("a.b".into())));
        assert_eq!(parse(&["ls"]), Ok(Command::List));
        assert_eq!(parse(&["kill", "work"]), Ok(Command::Kill("work".into())));
        assert_eq!(parse(&["__server", "x"]), Ok(Command::Server("x".into())));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        assert!(parse(&["attach"]).is_err());
        assert!(parse(&["attach", "../x"]).is_err());
        assert!(parse(&["attach", "a", "b"]).is_err());
        assert!(parse(&["--nope"]).is_err());
    }
}

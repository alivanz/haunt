//! `haunt attach NAME` and `haunt kill NAME`: the commands that talk to a
//! session's server.

use std::fs;
use std::io::{BufReader, Read, Write, stdout};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::protocol::{self, ClientMsg, Hello, PROTOCOL, ServerMsg};
use crate::{server, session, tty};

/// How long a newly started server gets to open its socket.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `kill` waits for the server to finish.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// Show session `name` in this terminal, starting it first if it isn't
/// running. Returns when detached or when the session ends.
pub fn attach(name: &str) -> Result<()> {
    if std::env::var("HAUNT_SESSION").as_deref() == Ok(name) {
        bail!("you're already inside session '{name}'");
    }
    let stream = connect_or_start(name)?;
    let (cols, rows) = crossterm::terminal::size()?;
    let mut writer = stream.try_clone()?;
    let hello = Hello::Attach {
        protocol: PROTOCOL,
        cols,
        rows,
    };
    protocol::write_client(&mut writer, &ClientMsg::Hello(hello))?;

    let reason = {
        let _restore = tty::setup()?;
        // Keys, mouse, pastes and resizes go to the server as they happen.
        thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                if protocol::write_client(&mut writer, &ClientMsg::Event(ev)).is_err() {
                    break;
                }
            }
        });
        show(stream, name)
    };
    // Printed after the terminal is restored, so it stays visible.
    println!("{reason}");
    Ok(())
}

/// Copy the server's output to the terminal until it sends us away.
fn show(stream: UnixStream, name: &str) -> String {
    let mut reader = BufReader::new(stream);
    loop {
        match protocol::read_server(&mut reader) {
            Ok(ServerMsg::Output(bytes)) => {
                let mut out = stdout();
                if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                    return String::new();
                }
            }
            Ok(ServerMsg::Exit(reason)) => return reason,
            Err(_) => return format!("[lost connection to session '{name}']"),
        }
    }
}

fn connect_or_start(name: &str) -> Result<UnixStream> {
    let socket = session::socket_path(name);
    if let Ok(stream) = UnixStream::connect(&socket) {
        return Ok(stream);
    }
    let mut server = server::spawn(name)?;
    let started = Instant::now();
    loop {
        if let Ok(stream) = UnixStream::connect(&socket) {
            return Ok(stream);
        }
        if let Some(status) = server.try_wait()? {
            let log = fs::read_to_string(session::log_path(name)).unwrap_or_default();
            // The server's `main` prints "Error: ..." already.
            let log = log.trim().trim_start_matches("Error: ");
            if log.is_empty() {
                bail!("session '{name}' failed to start ({status})");
            }
            bail!("session '{name}' failed to start:\n{log}");
        }
        if started.elapsed() > START_TIMEOUT {
            bail!(
                "session '{name}' didn't start in time; see {}",
                session::log_path(name).display()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Stop session `name` if it's running, and delete its saved copy.
pub fn kill(name: &str) -> Result<()> {
    match UnixStream::connect(session::socket_path(name)) {
        Ok(mut stream) => {
            protocol::write_client(&mut stream, &ClientMsg::Hello(Hello::Kill))?;
            // The server closes the connection when it exits.
            stream.set_read_timeout(Some(KILL_TIMEOUT))?;
            let _ = stream.read(&mut [0u8; 1]);
            println!("killed session '{name}'");
        }
        Err(_) => {
            if let Some(pid) = session::running(name) {
                bail!("session '{name}' is running (pid {pid}) but not answering");
            }
            let path = session::path(name);
            if !path.exists() {
                bail!("no session named '{name}'");
            }
            session::delete(&path);
            println!("deleted saved session '{name}'");
        }
    }
    Ok(())
}

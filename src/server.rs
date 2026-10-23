//! The background server behind `haunt attach NAME`. It runs the session
//! (shells, layout, scrollback) with no terminal of its own, and shows it on
//! one attached client at a time through a Unix socket.

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Sender, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Result;
use ratatui::layout::Size;

use crate::app::{App, AppEvent, OpenSession, Output};
use crate::protocol::{self, ClientMsg, Hello, PROTOCOL, ServerMsg};
use crate::session;

/// Frames waiting to be written to a client. If a slow client lets this
/// fill up, new frames are dropped and the next one repaints everything.
const QUEUE: usize = 32;

/// A client that can't take any output for this long is cut off.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a new connection gets to say what it wants.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Size to lay panes out at until the first client says its size.
const DEFAULT_SIZE: Size = Size::new(80, 24);

/// Start `haunt __server NAME` in the background, detached from this
/// terminal so closing it doesn't stop the server.
pub fn spawn(name: &str) -> Result<Child> {
    fs::create_dir_all(session::dir())?;
    let log = File::create(session::log_path(name))?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("__server")
        .arg(name)
        // Lets `haunt attach` refuse to attach a session from inside itself.
        .env("HAUNT_SESSION", name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // A new session with no controlling terminal: when the terminal closes,
    // its hangup signal goes to the old session, not to us.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(cmd.spawn()?)
}

/// Run session `name` until it ends, is quit or is killed.
pub fn run(name: String) -> Result<()> {
    // Errors and panics go to stderr, which `spawn` pointed at the log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        std::process::exit(101);
    }));
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let lock = session::Lock::acquire(&name)?;
    let saved = session::load(&session::path(&name))?;
    session::private_runtime_dir()?;
    let socket = session::socket_path(&name);
    // We hold the lock, so a socket left here belongs to a dead server.
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    let _socket = RemoveOnDrop(socket);

    let open = OpenSession {
        name: name.clone(),
        lock,
        saved,
    };
    let app = App::new(DEFAULT_SIZE, Output::Server(None), Some(open))?;
    let events = app.sender();
    let accept_name = name.clone();
    thread::spawn(move || accept(listener, accept_name, events));
    app.run()?;
    // A clean exit: the log only matters when something went wrong.
    let _ = fs::remove_file(session::log_path(&name));
    Ok(())
}

fn accept(listener: UnixListener, name: String, events: Sender<AppEvent>) {
    for (id, stream) in (1..).zip(listener.incoming()) {
        let Ok(stream) = stream else { continue };
        let (name, events) = (name.clone(), events.clone());
        // Each connection gets a thread, so one that never says hello
        // doesn't hold up the rest.
        thread::spawn(move || {
            let _ = serve(id, stream, &name, events);
        });
    }
}

/// Handle one connection: check who it is and what it wants, then (for an
/// attach) pass its input to the app until it disconnects.
fn serve(id: u64, mut stream: UnixStream, name: &str, events: Sender<AppEvent>) -> io::Result<()> {
    if peer_uid(&stream)? != unsafe { libc::getuid() } {
        return Ok(());
    }
    stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
    let ClientMsg::Hello(hello) = protocol::read_client(&mut stream)? else {
        return Ok(());
    };
    stream.set_read_timeout(None)?;

    let (cols, rows) = match hello {
        Hello::Kill => {
            let _ = events.send(AppEvent::Kill(stream));
            return Ok(());
        }
        Hello::Attach {
            protocol: theirs, ..
        } if theirs != PROTOCOL => {
            let reason = format!(
                "session '{name}' is running a different version of haunt \
                 (protocol {PROTOCOL}, yours is {theirs}); `haunt kill {name}` stops it"
            );
            return protocol::write_server(&mut stream, &ServerMsg::Exit(reason));
        }
        Hello::Attach { cols, rows, .. } => (cols, rows),
    };

    let reader = stream.try_clone()?;
    let client = Client::start(id, Size::new(cols, rows), stream);
    if events.send(AppEvent::Attach(client)).is_err() {
        return Ok(());
    }
    // Only now start reading input, so none of it arrives before the
    // attach it belongs to.
    let mut reader = BufReader::new(reader);
    loop {
        match protocol::read_client(&mut reader) {
            Ok(ClientMsg::Event(ev)) => {
                if events.send(AppEvent::ClientInput(id, ev)).is_err() {
                    break;
                }
            }
            Ok(ClientMsg::Hello(_)) => {}
            Err(_) => {
                let _ = events.send(AppEvent::ClientGone(id));
                break;
            }
        }
    }
    Ok(())
}

/// The attached client, as seen by the app. Output goes through a queue to
/// a writer thread, so a slow client can't stall the session.
pub struct Client {
    pub id: u64,
    /// The client's terminal size when it attached.
    pub size: Size,
    tx: SyncSender<ServerMsg>,
    writer: JoinHandle<()>,
}

impl Client {
    fn start(id: u64, size: Size, mut stream: UnixStream) -> Client {
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let (tx, rx) = sync_channel(QUEUE);
        let writer = thread::spawn(move || {
            for msg in rx {
                if protocol::write_server(&mut stream, &msg).is_err() {
                    break;
                }
            }
            // Also ends the reading side in `serve`.
            let _ = stream.shutdown(Shutdown::Both);
        });
        Client {
            id,
            size,
            tx,
            writer,
        }
    }

    /// Queue a frame; false if it had to be dropped.
    pub fn send_output(&self, frame: Vec<u8>) -> bool {
        self.tx.try_send(ServerMsg::Output(frame)).is_ok()
    }

    /// Send the client away with a message to print. The returned thread
    /// finishes once everything queued was delivered (or the client is gone).
    pub fn close(self, reason: String) -> JoinHandle<()> {
        let _ = self.tx.try_send(ServerMsg::Exit(reason));
        // Dropping `tx` here lets the writer thread finish.
        self.writer
    }
}

/// The user id of the process on the other end of a Unix socket.
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let ok = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if ok != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (mut uid, mut gid) = (0, 0);
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_is_us() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::getuid() });
    }
}

//! Messages between a `haunt attach` client and the session server, sent
//! over a Unix socket.
//!
//! Each message is a frame: a 1-byte kind, a 4-byte big-endian length and
//! the payload. Screen output is sent as raw bytes; everything else is JSON.

use std::io::{self, Read, Write};

use crossterm::event::Event;
use serde::{Deserialize, Serialize};

/// Bump when the messages below change incompatibly. A client refuses to
/// attach to a server speaking another version.
pub const PROTOCOL: u32 = 1;

/// Frames bigger than this are treated as a broken connection.
const MAX_FRAME: usize = 64 << 20;

/// The first message on a connection says what the client wants. Keep it
/// compatible across versions, so `kill` always works.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Hello {
    Attach {
        protocol: u32,
        cols: u16,
        rows: u16,
    },
    /// Stop the session and delete its saved copy.
    Kill,
}

#[derive(Debug, PartialEq)]
pub enum ClientMsg {
    Hello(Hello),
    /// A key, mouse, paste or resize event from the client's terminal.
    Event(Event),
}

#[derive(Debug, PartialEq)]
pub enum ServerMsg {
    /// Bytes to write to the terminal as-is.
    Output(Vec<u8>),
    /// The client should restore its terminal, print this and exit.
    Exit(String),
}

const HELLO: u8 = b'H';
const EVENT: u8 = b'E';
const OUTPUT: u8 = b'O';
const EXIT: u8 = b'X';

fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    w.write_all(&frame)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len > MAX_FRAME {
        return Err(invalid("frame too big"));
    }
    let mut payload = vec![0; len];
    r.read_exact(&mut payload)?;
    Ok((head[0], payload))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub fn write_client(w: &mut impl Write, msg: &ClientMsg) -> io::Result<()> {
    let (kind, json) = match msg {
        ClientMsg::Hello(h) => (HELLO, serde_json::to_vec(h)?),
        ClientMsg::Event(e) => (EVENT, serde_json::to_vec(e)?),
    };
    write_frame(w, kind, &json)
}

pub fn read_client(r: &mut impl Read) -> io::Result<ClientMsg> {
    let (kind, payload) = read_frame(r)?;
    match kind {
        HELLO => Ok(ClientMsg::Hello(serde_json::from_slice(&payload)?)),
        EVENT => Ok(ClientMsg::Event(serde_json::from_slice(&payload)?)),
        _ => Err(invalid("unknown message from client")),
    }
}

pub fn write_server(w: &mut impl Write, msg: &ServerMsg) -> io::Result<()> {
    match msg {
        ServerMsg::Output(bytes) => write_frame(w, OUTPUT, bytes),
        ServerMsg::Exit(reason) => write_frame(w, EXIT, reason.as_bytes()),
    }
}

pub fn read_server(r: &mut impl Read) -> io::Result<ServerMsg> {
    let (kind, payload) = read_frame(r)?;
    match kind {
        OUTPUT => Ok(ServerMsg::Output(payload)),
        EXIT => Ok(ServerMsg::Exit(String::from_utf8_lossy(&payload).into())),
        _ => Err(invalid("unknown message from server")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn client_messages_roundtrip() {
        let msgs = [
            ClientMsg::Hello(Hello::Attach {
                protocol: PROTOCOL,
                cols: 120,
                rows: 40,
            }),
            ClientMsg::Hello(Hello::Kill),
            ClientMsg::Event(Event::Key(KeyEvent::new(
                KeyCode::Char('t'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ))),
            ClientMsg::Event(Event::Resize(80, 24)),
            ClientMsg::Event(Event::Paste("hi\nthere".into())),
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_client(&mut buf, m).unwrap();
        }
        let mut r = buf.as_slice();
        for m in msgs {
            assert_eq!(read_client(&mut r).unwrap(), m);
        }
        assert!(read_client(&mut r).is_err()); // EOF
    }

    #[test]
    fn server_messages_roundtrip() {
        let msgs = [
            ServerMsg::Output(b"\x1b[2J\x00\xff raw".to_vec()),
            ServerMsg::Exit("detached".into()),
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_server(&mut buf, m).unwrap();
        }
        let mut r = buf.as_slice();
        for m in msgs {
            assert_eq!(read_server(&mut r).unwrap(), m);
        }
    }

    #[test]
    fn rejects_garbage() {
        let mut r: &[u8] = b"Z\x00\x00\x00\x00";
        assert!(read_server(&mut r).is_err());
        let mut r: &[u8] = b"O\xff\xff\xff\xff";
        assert!(read_server(&mut r).is_err());
    }
}

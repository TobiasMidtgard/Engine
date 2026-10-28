//! The wire half of the script debugger: a [Debug Adapter Protocol] server on a local TCP port,
//! so VS Code (`crates/script/vscode`), Neovim (nvim-dap), Zed or any other DAP client can
//! attach to the running game. One client at a time; it runs on its own thread and only frames
//! messages, everything else happens on the thread that runs the scripts ([`crate::debugger`]).
//!
//! [Debug Adapter Protocol]: https://microsoft.github.io/debug-adapter-protocol/overview

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// What the connection thread hands to the script thread.
pub(crate) enum Incoming {
    Connected,
    Request(Value),
    Closed,
}

/// The listening server and the client connected to it, if any.
pub(crate) struct Link {
    pub port: u16,
    rx: Receiver<Incoming>,
    writer: Arc<Mutex<Option<TcpStream>>>,
    seq: AtomicI64,
}

/// Listens on `127.0.0.1:port` (0 picks a free port; [`Link::port`] says which).
pub(crate) fn listen(port: u16) -> std::io::Result<Link> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = channel();
    let writer: Arc<Mutex<Option<TcpStream>>> = Arc::default();
    let conn = writer.clone();
    std::thread::Builder::new()
        .name("script-debugger".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = stream.set_nodelay(true);
                let Ok(write_half) = stream.try_clone() else {
                    continue;
                };
                *conn.lock().unwrap_or_else(|p| p.into_inner()) = Some(write_half);
                if tx.send(Incoming::Connected).is_err() {
                    return;
                }
                let mut reader = BufReader::new(stream);
                while let Some(message) = read_message(&mut reader) {
                    if tx.send(Incoming::Request(message)).is_err() {
                        return;
                    }
                }
                *conn.lock().unwrap_or_else(|p| p.into_inner()) = None;
                if tx.send(Incoming::Closed).is_err() {
                    return;
                }
            }
        })?;
    Ok(Link {
        port,
        rx,
        writer,
        seq: AtomicI64::new(1),
    })
}

/// Reads one `Content-Length`-framed message; `None` when the client went away. A body that
/// isn't JSON comes back as `Null` (and is ignored).
pub(crate) fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
        {
            len = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; len?];
    r.read_exact(&mut body).ok()?;
    Some(serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Frames `message` for the wire.
pub(crate) fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

impl Link {
    /// The next message from the client, waiting up to `wait`.
    pub fn recv(&self, wait: Duration) -> Option<Incoming> {
        self.rx.recv_timeout(wait).ok()
    }

    pub fn connected(&self) -> bool {
        self.writer.lock().map(|w| w.is_some()).unwrap_or_default()
    }

    fn send(&self, mut message: Value) {
        message["seq"] = json!(self.seq.fetch_add(1, Ordering::Relaxed));
        let mut writer = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(stream) = writer.as_mut()
            && stream.write_all(&frame(&message)).is_err()
        {
            *writer = None;
        }
    }

    /// Answers `request` with a body, or fails it with a message the client shows.
    pub fn respond(&self, request: &Value, result: Result<Value, String>) {
        let mut message = json!({
            "type": "response",
            "request_seq": request["seq"],
            "command": request["command"],
            "success": result.is_ok(),
        });
        match result {
            Ok(Value::Null) => {}
            Ok(body) => message["body"] = body,
            Err(text) => {
                message["message"] = json!(text);
                message["body"] = json!({"error": {"id": 1, "format": text}});
            }
        }
        self.send(message);
    }

    pub fn event(&self, event: &str, body: Value) {
        let mut message = json!({"type": "event", "event": event});
        if !body.is_null() {
            message["body"] = body;
        }
        self.send(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_through_the_framing() {
        let a = json!({"seq": 1, "type": "request", "command": "initialize"});
        let b = json!({"seq": 2, "type": "request", "command": "threads", "text": "ünïcode"});
        let mut wire = frame(&a);
        wire.extend(frame(&b));
        let mut r = BufReader::new(&wire[..]);
        assert_eq!(read_message(&mut r), Some(a));
        assert_eq!(read_message(&mut r), Some(b));
        assert_eq!(read_message(&mut r), None);
    }
}

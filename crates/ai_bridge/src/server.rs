//! The TCP side of the bridge: one accept thread, and per connection a reader thread (parses
//! lines, checks the token, queues requests) and a writer thread (sends answers back). None of
//! these threads touch the [`World`](bevy_ecs::world::World); the main thread never waits on
//! the network.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy_ecs::prelude::*;
use serde_json::{Value, json};

use crate::protocol::{Reply, ReplyTo, Request, RpcError, codes, response_json};

/// Longest accepted request line (bytes). Larger requests are rejected and the connection is
/// closed.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

/// Most simultaneous TCP clients. Further connections get an error line and are closed.
pub const MAX_CLIENTS: u32 = 64;

/// State shared by the accept thread, the connection threads and the main thread.
pub(crate) struct ServerShared {
    queue: Sender<Request>,
    token: Option<String>,
    pub(crate) clients: AtomicU32,
    shutdown: AtomicBool,
    next_connection: AtomicU64,
    connections: Mutex<Vec<(u64, TcpStream)>>,
}

/// The running TCP server. Dropping it (with the app's world) stops the accept thread and
/// closes every client connection.
#[derive(Resource)]
pub struct BridgeServer {
    pub(crate) shared: Arc<ServerShared>,
    local_addr: SocketAddr,
}

impl BridgeServer {
    /// Bind `addr` and start accepting clients on a background thread.
    pub(crate) fn start(
        addr: &str,
        token: Option<String>,
        queue: Sender<Request>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        let shared = Arc::new(ServerShared {
            queue,
            token,
            clients: AtomicU32::new(0),
            shutdown: AtomicBool::new(false),
            next_connection: AtomicU64::new(1),
            connections: Mutex::new(Vec::new()),
        });
        let accept_shared = shared.clone();
        std::thread::Builder::new()
            .name("ai-bridge-accept".into())
            .spawn(move || accept_loop(listener, accept_shared))?;
        Ok(Self { shared, local_addr })
    }

    /// The address actually bound (the real port when port 0 was requested).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Connected TCP clients right now.
    pub fn clients(&self) -> u32 {
        self.shared.clients.load(Ordering::Relaxed)
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        // Wake the blocking accept() with a throwaway connection.
        let mut wake = self.local_addr;
        if wake.ip().is_unspecified() {
            wake.set_ip(match wake {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let _ = TcpStream::connect_timeout(&wake, Duration::from_millis(200));
        for (_, stream) in lock(&self.shared.connections).drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn accept_loop(listener: TcpListener, shared: Arc<ServerShared>) {
    for stream in listener.incoming() {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        match stream {
            Ok(stream) => {
                if let Err(e) = spawn_connection(stream, shared.clone()) {
                    tracing::warn!("AI bridge: could not start a client connection: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("AI bridge: accept failed: {e}");
                // Avoid spinning when out of file descriptors.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn spawn_connection(stream: TcpStream, shared: Arc<ServerShared>) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    if shared.clients.load(Ordering::SeqCst) >= MAX_CLIENTS {
        let mut stream = stream;
        let error = RpcError::new(
            codes::LIMIT_EXCEEDED,
            format!("the AI bridge already has {MAX_CLIENTS} clients; close one and retry"),
        );
        let mut line = serde_json::to_vec(&response_json(&Value::Null, &Err(error)))?;
        line.push(b'\n');
        let _ = stream.write_all(&line);
        return Ok(());
    }
    let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    let writer_stream = stream.try_clone()?;
    let registered_stream = stream.try_clone()?;

    let (tx, rx) = mpsc::channel::<Reply>();
    std::thread::Builder::new()
        .name(format!("ai-bridge-write-{id}"))
        .spawn(move || {
            let mut stream = writer_stream;
            for reply in rx {
                let Ok(mut line) = serde_json::to_vec(&response_json(&reply.id, &reply.result))
                else {
                    continue;
                };
                line.push(b'\n');
                if stream
                    .write_all(&line)
                    .and_then(|_| stream.flush())
                    .is_err()
                {
                    break;
                }
            }
        })?;

    // Registered only once the writer runs, so a failed spawn leaves nothing behind (the
    // reader-spawn failure below undoes it).
    {
        let mut connections = lock(&shared.connections);
        // `BridgeServer::drop` sets `shutdown` before it closes the registered connections, so a
        // connection registered after that would never be closed: close it here instead.
        if shared.shutdown.load(Ordering::SeqCst) {
            let _ = registered_stream.shutdown(Shutdown::Both);
        }
        connections.push((id, registered_stream));
    }
    shared.clients.fetch_add(1, Ordering::SeqCst);
    let reader_shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("ai-bridge-read-{id}"))
        .spawn(move || {
            let peer = stream
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            tracing::info!("AI bridge: client {id} connected from {peer}");
            let mut connection = Connection {
                shared: reader_shared.clone(),
                tx,
                authed: reader_shared.token.is_none(),
            };
            connection.read_loop(&stream);
            let _ = stream.shutdown(Shutdown::Read);
            lock(&reader_shared.connections).retain(|(c, _)| *c != id);
            reader_shared.clients.fetch_sub(1, Ordering::SeqCst);
            tracing::info!("AI bridge: client {id} disconnected");
        });
    if let Err(e) = spawned {
        lock(&shared.connections).retain(|(c, _)| *c != id);
        shared.clients.fetch_sub(1, Ordering::SeqCst);
        return Err(e);
    }
    Ok(())
}

/// One client's reader state.
struct Connection {
    shared: Arc<ServerShared>,
    tx: Sender<Reply>,
    authed: bool,
}

impl Connection {
    fn read_loop(&mut self, stream: &TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = (&mut reader)
                .take(MAX_REQUEST_BYTES as u64 + 1)
                .read_until(b'\n', &mut line);
            match read {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if line.len() > MAX_REQUEST_BYTES {
                self.reply(
                    Value::Null,
                    Err(RpcError::new(
                        codes::LIMIT_EXCEEDED,
                        format!(
                            "request is larger than {MAX_REQUEST_BYTES} bytes; the connection \
                             will be closed"
                        ),
                    )),
                );
                return;
            }
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return;
            }
            self.handle_line(&line);
        }
    }

    fn reply(&self, id: Value, result: Result<Value, RpcError>) {
        let _ = self.tx.send(Reply { id, result });
    }

    fn handle_line(&mut self, line: &[u8]) {
        let text = line.trim_ascii();
        if text.is_empty() {
            return;
        }
        let message: Value = match serde_json::from_slice(text) {
            Ok(v) => v,
            Err(e) => {
                return self.reply(
                    Value::Null,
                    Err(RpcError::new(
                        codes::PARSE_ERROR,
                        format!("parse error: {e}. Send one JSON-RPC object per line."),
                    )),
                );
            }
        };
        let Value::Object(object) = message else {
            let why = if message.is_array() {
                "batch requests are not supported; send one JSON-RPC object per line"
            } else {
                "expected a JSON-RPC object like {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"bridge.ping\"}"
            };
            return self.reply(Value::Null, Err(RpcError::new(codes::INVALID_REQUEST, why)));
        };
        // A missing id makes the message a notification: run it, send nothing back.
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            if let Some(id) = id {
                self.reply(
                    id,
                    Err(RpcError::new(
                        codes::INVALID_REQUEST,
                        "missing \"method\"; call bridge.methods to list what you can call",
                    )),
                );
            }
            return;
        };
        let params = match object.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(p) => p.clone(),
        };

        if method == "auth" || method == "bridge.auth" {
            let result = self.authenticate(&params);
            if let Some(id) = id {
                self.reply(id, result);
            }
            return;
        }
        if !self.authed {
            let token_ok = matches!(
                (object.get("token").and_then(Value::as_str), &self.shared.token),
                (Some(given), Some(expected)) if token_eq(given, expected)
            );
            if !token_ok {
                if let Some(id) = id {
                    self.reply(id, Err(unauthorized()));
                }
                return;
            }
        }

        let request = Request {
            method: method.to_string(),
            params,
            reply: id.map(|id| ReplyTo {
                id,
                tx: self.tx.clone(),
            }),
        };
        if let Err(mpsc::SendError(request)) = self.shared.queue.send(request)
            && let Some(reply) = request.reply
        {
            reply.send(Err(RpcError::shutting_down()));
        }
    }

    fn authenticate(&mut self, params: &Value) -> Result<Value, RpcError> {
        let Some(expected) = &self.shared.token else {
            self.authed = true;
            return Ok(json!({"ok": true, "token_required": false}));
        };
        match params.get("token").and_then(Value::as_str) {
            Some(given) if token_eq(given, expected) => {
                self.authed = true;
                Ok(json!({"ok": true, "token_required": true}))
            }
            _ => Err(unauthorized()),
        }
    }
}

fn unauthorized() -> RpcError {
    RpcError::new(
        codes::UNAUTHORIZED,
        "unauthorized: this bridge requires the ENGINE_BRIDGE_TOKEN value. Send \
         {\"method\":\"auth\",\"params\":{\"token\":\"...\"}} once, or add \"token\": \"...\" \
         to every request",
    )
}

/// Compare tokens without an early exit, so timing does not reveal how much matched.
fn token_eq(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abd", "abc"));
        assert!(!token_eq("ab", "abc"));
        assert!(!token_eq("", "abc"));
    }
}

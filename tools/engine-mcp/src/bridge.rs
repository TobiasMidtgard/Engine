//! Client for the engine's AI bridge (newline-delimited JSON-RPC 2.0 over TCP) and the game
//! process this server may have launched.
//!
//! One TCP connection is shared by every tool call; answers are matched to callers by id, so a
//! long `engine.step` does not hold up a `bridge.ping`. A dropped connection is re-opened on
//! the next call (the game may have been restarted).

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Environment variable the game reads for its bridge address.
pub const ENV_ADDRESS: &str = "ENGINE_BRIDGE";
/// Environment variable with the bridge token.
pub const ENV_TOKEN: &str = "ENGINE_BRIDGE_TOKEN";
/// Tells a launched game to exit when its stdin closes (that is, when this server goes away).
pub const ENV_EXIT_ON_STDIN_EOF: &str = "ENGINE_BRIDGE_EXIT_ON_STDIN_EOF";
/// Where the bridge listens unless told otherwise.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:7878";

/// Why a bridge call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// The game answered with a JSON-RPC error object.
    Rpc { code: i64, message: String },
    /// The game could not be reached, went away, or did not answer in time.
    Unavailable(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Rpc { code, message } => write!(f, "{message} (error {code})"),
            CallError::Unavailable(message) => f.write_str(message),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lost() -> CallError {
    CallError::Unavailable("lost the connection to the game (did it exit or crash?)".into())
}

/// Turn a JSON-RPC response object into a result.
pub fn parse_response(message: &Value) -> Result<Value, CallError> {
    if let Some(error) = message.get("error") {
        return Err(CallError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
        });
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| CallError::Unavailable(format!("malformed answer from the game: {message}")))
}

type Waiter = mpsc::Sender<Result<Value, CallError>>;

/// One open TCP connection to the bridge.
struct Connection {
    writer: Mutex<TcpStream>,
    pending: Mutex<HashMap<u64, Waiter>>,
    alive: AtomicBool,
    next_id: AtomicU64,
}

impl Connection {
    fn open(address: &str, timeout: Duration) -> Result<Arc<Self>, String> {
        let addresses: Vec<SocketAddr> = address
            .to_socket_addrs()
            .map_err(|e| format!("'{address}' is not a usable address ({e})"))?
            .collect();
        let mut last_error = format!("'{address}' resolved to no addresses");
        let mut stream = None;
        for a in addresses {
            match TcpStream::connect_timeout(&a, timeout) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last_error = e.to_string(),
            }
        }
        let stream = stream.ok_or(last_error)?;
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let connection = Arc::new(Connection {
            writer: Mutex::new(stream),
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            next_id: AtomicU64::new(1),
        });
        let weak = Arc::downgrade(&connection);
        std::thread::Builder::new()
            .name("bridge-reader".into())
            .spawn(move || read_loop(reader, weak))
            .map_err(|e| e.to_string())?;
        Ok(connection)
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(id, tx);
        if !self.is_alive() {
            lock(&self.pending).remove(&id);
            return Err(lost());
        }
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut line = message.to_string().into_bytes();
        line.push(b'\n');
        let written = {
            let mut writer = lock(&self.writer);
            writer.write_all(&line).and_then(|_| writer.flush())
        };
        if let Err(e) = written {
            self.alive.store(false, Ordering::SeqCst);
            lock(&self.pending).remove(&id);
            return Err(CallError::Unavailable(format!(
                "could not send {method} to the game: {e}"
            )));
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.pending).remove(&id);
                Err(CallError::Unavailable(format!(
                    "the game did not answer {method} within {} s (it may still finish it)",
                    timeout.as_secs()
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(lost()),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Unblocks the reader thread.
        let _ = lock(&self.writer).shutdown(Shutdown::Both);
    }
}

fn read_loop(stream: TcpStream, connection: Weak<Connection>) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(conn) = connection.upgrade() else {
            return;
        };
        let message: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => {
                eprintln!("engine-mcp: the bridge sent a line that is not JSON: {line}");
                continue;
            }
        };
        match message.get("id").and_then(Value::as_u64) {
            Some(id) => {
                if let Some(waiter) = lock(&conn.pending).remove(&id) {
                    let _ = waiter.send(parse_response(&message));
                }
            }
            None => eprintln!("engine-mcp: bridge message without a request id: {message}"),
        }
    }
    if let Some(conn) = connection.upgrade() {
        conn.alive.store(false, Ordering::SeqCst);
        for (_, waiter) in lock(&conn.pending).drain() {
            let _ = waiter.send(Err(lost()));
        }
    }
}

/// A free TCP port on 127.0.0.1 (bound and released, so the game can take it).
pub fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// A game started with `--launch`.
struct GameProcess {
    child: Child,
    /// Held open and never written: when this server exits for any reason the pipe closes and
    /// the game (told so through [`ENV_EXIT_ON_STDIN_EOF`]) shuts itself down.
    stdin: Option<ChildStdin>,
    command: String,
}

impl GameProcess {
    fn spawn(command: &str, address: &str, token: Option<&str>) -> Result<Self, String> {
        let mut cmd = shell_command(command);
        if let Some(token) = token {
            cmd.env(ENV_TOKEN, token);
        }
        cmd.env(ENV_ADDRESS, address)
            .env(ENV_EXIT_ON_STDIN_EOF, "1")
            .stdin(Stdio::piped())
            // stdout is the MCP channel: the game's output must go to stderr instead.
            .stdout(Stdio::from(std::io::stderr()))
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group, so shutdown reaches cargo and the game it starts.
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start `{command}`: {e}"))?;
        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            command: command.to_string(),
        })
    }

    fn exit_status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.exit_status().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

impl Drop for GameProcess {
    fn drop(&mut self) {
        if self.exit_status().is_some() {
            return;
        }
        eprintln!("engine-mcp: stopping `{}`", self.command);
        // Closing stdin asks the game to exit cleanly.
        drop(self.stdin.take());
        if self.wait_exit(Duration::from_secs(3)) {
            return;
        }
        #[cfg(unix)]
        {
            let group = format!("-{}", self.child.id());
            let _ = Command::new("kill")
                .args(["-TERM", "--", &group])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if self.wait_exit(Duration::from_secs(3)) {
                return;
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// The link to one game's bridge: connects lazily, reconnects after a drop, authenticates
/// with the token, and (with `--launch`) owns the game process.
pub struct BridgeLink {
    address: String,
    token: Option<String>,
    connection: Mutex<Option<Arc<Connection>>>,
    game: Mutex<Option<GameProcess>>,
    launched_at: Option<Instant>,
    startup_timeout: Duration,
}

impl BridgeLink {
    /// Talk to a game that is already running (or will be started by someone else).
    pub fn connect_to(address: impl Into<String>, token: Option<String>) -> Self {
        Self {
            address: address.into(),
            token,
            connection: Mutex::new(None),
            game: Mutex::new(None),
            launched_at: None,
            startup_timeout: Duration::from_secs(2),
        }
    }

    /// Start `command` through the shell with `ENGINE_BRIDGE=127.0.0.1:<free port>` (and the
    /// token, if any, as `ENGINE_BRIDGE_TOKEN`) and talk to it. Returns at once; calls wait up to
    /// `startup_timeout` for the bridge to come up.
    pub fn launch(
        command: &str,
        token: Option<String>,
        startup_timeout: Duration,
    ) -> Result<Self, String> {
        let port = free_port().map_err(|e| format!("could not find a free port: {e}"))?;
        let address = format!("127.0.0.1:{port}");
        let game = GameProcess::spawn(command, &address, token.as_deref())?;
        eprintln!(
            "engine-mcp: launched `{command}` (pid {}); its AI bridge will be on {address}",
            game.child.id()
        );
        Ok(Self {
            address,
            token,
            connection: Mutex::new(None),
            game: Mutex::new(Some(game)),
            launched_at: Some(Instant::now()),
            startup_timeout,
        })
    }

    /// The bridge address (`host:port`).
    pub fn address(&self) -> &str {
        &self.address
    }

    /// True if this server started the game.
    pub fn is_launched(&self) -> bool {
        self.launched_at.is_some()
    }

    /// How the launched game exited, if it did.
    fn game_exit(&self) -> Option<ExitStatus> {
        lock(&self.game).as_mut().and_then(GameProcess::exit_status)
    }

    fn game_running(&self) -> bool {
        lock(&self.game).is_some() && self.game_exit().is_none()
    }

    fn unreachable(&self, error: &str) -> CallError {
        let address = &self.address;
        let message = match (self.launched_at, self.game_exit()) {
            (Some(_), Some(status)) => format!(
                "the launched game exited ({status}); its log is in this server's stderr. \
                 Restart the MCP server to launch it again."
            ),
            (Some(started), None) => format!(
                "the game is still starting (launched {} s ago; a first `cargo run` can compile \
                 for minutes) and its AI bridge on {address} is not up yet ({error}). Try again \
                 shortly.",
                started.elapsed().as_secs()
            ),
            (None, _) => format!(
                "cannot reach the game's AI bridge at {address} ({error}). Start the game with \
                 {ENV_ADDRESS}={address} (it must add engine_ai_bridge::AiBridgePlugin), or run \
                 engine-mcp with --launch \"<command>\"."
            ),
        };
        CallError::Unavailable(message)
    }

    fn connection(&self) -> Result<Arc<Connection>, CallError> {
        let mut slot = lock(&self.connection);
        if let Some(c) = slot.as_ref()
            && c.is_alive()
        {
            return Ok(c.clone());
        }
        *slot = None;
        let connection = Connection::open(&self.address, Duration::from_secs(2))
            .map_err(|e| self.unreachable(&e))?;
        if let Some(token) = &self.token {
            connection
                .call("auth", json!({"token": token}), Duration::from_secs(30))
                .map_err(|e| {
                    CallError::Unavailable(format!(
                        "the game's AI bridge rejected the token from {ENV_TOKEN} or --token: {e}"
                    ))
                })?;
        }
        *slot = Some(connection.clone());
        Ok(connection)
    }

    /// Call a method if the bridge is reachable right now (no waiting for startup).
    pub fn try_call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        self.connection()?.call(method, params, timeout)
    }

    /// Call a method; if this server launched the game and it is still starting, wait for it
    /// (up to the startup timeout) first.
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, CallError> {
        match self.connection() {
            Ok(connection) => connection.call(method, params, timeout),
            Err(_) if self.game_running() => {
                let started = self.launched_at.unwrap_or_else(Instant::now);
                let left = self.startup_timeout.saturating_sub(started.elapsed());
                self.wait_ready(left.max(Duration::from_secs(1)))
                    .map_err(CallError::Unavailable)?;
                self.connection()?.call(method, params, timeout)
            }
            Err(e) => Err(e),
        }
    }

    /// Wait until the bridge answers `bridge.ping` (or the launched game exits).
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.game_exit() {
                return Err(format!(
                    "the launched game exited ({status}) before its AI bridge answered; its \
                     output is in this server's stderr"
                ));
            }
            // A connected game that does not run frames (stopped in a debugger, a modal dialog)
            // must not hold the caller past its own deadline.
            let ping_timeout = deadline
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(500), Duration::from_secs(30));
            match self.try_call("bridge.ping", json!({}), ping_timeout) {
                Ok(_) | Err(CallError::Rpc { .. }) => return Ok(()),
                Err(CallError::Unavailable(e)) => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "the game's AI bridge on {} did not answer within {} s: {e}",
                            self.address,
                            timeout.as_secs()
                        ));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Stop the launched game (if any) and close the connection.
    pub fn shutdown(&self) {
        lock(&self.connection).take();
        let game = lock(&self.game).take();
        drop(game);
    }
}

impl Drop for BridgeLink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_parse() {
        assert_eq!(
            parse_response(&json!({"jsonrpc": "2.0", "id": 1, "result": {"a": 1}})),
            Ok(json!({"a": 1}))
        );
        assert_eq!(
            parse_response(&json!({"id": 1, "error": {"code": -32000, "message": "bad"}})),
            Err(CallError::Rpc {
                code: -32000,
                message: "bad".into()
            })
        );
    }

    #[test]
    fn unreachable_bridge_explains_what_to_do() {
        // Port 0 always refuses connections (a released "free" port could be taken by another
        // process on a busy machine before we connect).
        let link = BridgeLink::connect_to("127.0.0.1:0", None);
        let err = link
            .call("bridge.ping", json!({}), Duration::from_secs(1))
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("ENGINE_BRIDGE=127.0.0.1:"), "{text}");
        assert!(text.contains("--launch"), "{text}");
    }

    #[test]
    fn wait_ready_keeps_its_deadline_when_the_game_does_not_answer() {
        // Accepts connections and reads requests but never answers, like a game stopped in a
        // debugger.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });
        let link = BridgeLink::connect_to(address, None);
        let started = Instant::now();
        let err = link.wait_ready(Duration::from_secs(1)).unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?}: {err}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn launch_reports_an_early_exit() {
        let link = BridgeLink::launch("exit 3", None, Duration::from_secs(5)).unwrap();
        let err = link.wait_ready(Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("exited"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn launched_process_sees_the_bridge_address() {
        // A stand-in "game" that prints its environment to stderr and waits for stdin to close.
        let link = BridgeLink::launch(
            "test -n \"$ENGINE_BRIDGE\" && test \"$ENGINE_BRIDGE_EXIT_ON_STDIN_EOF\" = 1 && cat >/dev/null",
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(link.address().starts_with("127.0.0.1:"));
        std::thread::sleep(Duration::from_millis(200));
        assert!(link.game_running(), "the stand-in exited early");
        let started = Instant::now();
        link.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "closing stdin did not stop it"
        );
    }
}

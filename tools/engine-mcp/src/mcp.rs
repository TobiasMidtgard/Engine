//! MCP (Model Context Protocol) message handling, written by hand over `serde_json`:
//! `initialize`, `notifications/initialized`, `ping`, `tools/list` and `tools/call`.
//!
//! Every engine API reported by the game's `bridge.methods` becomes a tool (`entity.spawn` →
//! `entity_spawn`). Three tools are always present: `screenshot` (returns the image itself),
//! `step` (advance N frames) and `call` (any method by its dotted name).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};

use crate::base64;
use crate::bridge::{BridgeLink, CallError};

/// The MCP revision this server implements.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Revisions a client may ask for; the answer echoes the client's choice when it is one of
/// these (the subset of MCP this server uses is the same in all of them).
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Names of the tools that exist even before the game answers.
const BUILTIN_TOOLS: &[&str] = &["screenshot", "step", "call"];

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

const INSTRUCTIONS: &str = "Drives a running game made with this engine through its AI bridge. \
Start with engine_describe: it lists every component and resource (with docs) and every API \
with an argument example. Inspect with entity_list / entity_get / resource_get; edit with \
entity_spawn / entity_set / resource_set / scene_spawn; control time with engine_pause, \
engine_resume and step (a paused game runs exactly N frames, then pauses again); look with \
screenshot; play with input_press / input_axis / input_release. Units are SI (meters, seconds, \
kilograms), Y is up, cameras look along -Z. Entities are addressed by numeric id or by name.";

/// JSON-RPC error codes used for protocol-level failures.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// Tunables for [`Server`].
#[derive(Debug, Clone)]
pub struct Settings {
    /// How long `tools/list` waits for the game before answering with the built-in tools only
    /// (a `notifications/tools/list_changed` follows once the game is up).
    pub list_wait: Duration,
    /// How long one tool call may take (a long `step` counts here).
    pub call_timeout: Duration,
    /// Where screenshots go when the caller gives no path. Removed by [`Server::shutdown`] if
    /// the server created it.
    pub screenshot_dir: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            list_wait: Duration::from_secs(2),
            call_timeout: Duration::from_secs(600),
            screenshot_dir: std::env::temp_dir().join(format!("engine-mcp-{}", std::process::id())),
        }
    }
}

/// Sends a server-initiated message (a notification) to the client.
pub type Notifier = Box<dyn Fn(Value) + Send + Sync>;

/// One engine method as reported by `bridge.methods`.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub name: String,
    pub summary: String,
    pub args_example: String,
}

/// A protocol-level failure (becomes a JSON-RPC error object).
#[derive(Debug)]
struct Failure {
    code: i64,
    message: String,
}

fn invalid_params(message: impl Into<String>) -> Failure {
    Failure {
        code: codes::INVALID_PARAMS,
        message: message.into(),
    }
}

enum Lookup {
    Found(String),
    Unknown,
    Unreachable(CallError),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The MCP server state. [`Server::handle`] is synchronous and thread-safe; the stdio loop runs
/// slow requests (`tools/call`, `tools/list`) on their own threads.
pub struct Server {
    bridge: BridgeLink,
    settings: Settings,
    notify: Notifier,
    /// Tool name → engine method, from the last `bridge.methods`.
    tools: Mutex<HashMap<String, String>>,
    /// A `tools/list` was answered without the engine's tools; send `list_changed` later.
    listed_without_engine: AtomicBool,
    shots: AtomicU64,
    /// `settings.screenshot_dir` was created by this server (and is removed on shutdown).
    made_screenshot_dir: AtomicBool,
}

/// `entity.spawn` → `entity_spawn`; anything outside `[A-Za-z0-9_-]` becomes `_`.
pub fn tool_name(method: &str) -> String {
    method
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Read the `bridge.methods` result.
pub fn parse_methods(result: &Value) -> Result<Vec<Method>, String> {
    let list = result
        .get("methods")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("bridge.methods returned no \"methods\" list: {result}"))?;
    Ok(list
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?.to_string();
            let summary = m
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args_example = match m.get("args_example") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => "{}".to_string(),
                Some(other) => other.to_string(),
            };
            Some(Method {
                name,
                summary,
                args_example,
            })
        })
        .collect())
}

/// The MCP tool for one engine method.
pub fn engine_tool(method: &Method) -> Value {
    let summary = method.summary.trim();
    let mut description = String::from(summary);
    if !description.is_empty() && !description.ends_with('.') {
        description.push('.');
    }
    description.push_str(&format!(
        "\nExample args: {}\n(engine API `{}`)",
        method.args_example, method.name
    ));
    json!({
        "name": tool_name(&method.name),
        "title": method.name,
        "description": description,
        "inputSchema": {"type": "object", "additionalProperties": true},
    })
}

fn builtin_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "screenshot",
            "title": "Screenshot",
            "description": "Render the game's current frame and return it as a PNG image, plus \
                a text line with the file path and size. Optional `path` keeps the PNG there \
                (relative paths resolve against this server's working directory); by default \
                it goes to a temp folder that is removed when this server exits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Where to save the PNG (must end in .png)."}
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "step",
            "title": "Step frames",
            "description": "Advance the game exactly `frames` frames (default 1) and return \
                when they have run, with the new frame number. A paused game (engine_pause) \
                runs those frames and pauses again, so you can advance it frame by frame and \
                take a screenshot in between.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "frames": {"type": "integer", "minimum": 0, "default": 1,
                               "description": "Frames to run."}
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "call",
            "title": "Call an engine API",
            "description": "Call any engine API by its dotted name, for example \
                {\"method\": \"entity.list\", \"params\": {\"name_contains\": \"Tree\"}}. \
                Useful for APIs registered after the tool list was made.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "method": {"type": "string", "description": "Dotted API name, e.g. weather.set."},
                    "params": {"type": "object", "description": "Arguments (JSON object)."}
                },
                "required": ["method"],
                "additionalProperties": false
            }
        }),
    ]
}

fn text_content(text: impl Into<String>, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}], "isError": is_error})
}

/// A tool result for a bridge answer: pretty JSON text, or an error the model can read.
pub fn tool_result(answer: Result<Value, CallError>) -> Value {
    match answer {
        Ok(value) => text_content(
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            false,
        ),
        Err(e) => text_content(format!("Error: {e}"), true),
    }
}

/// A JSON-RPC error response.
pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(PNG_SIGNATURE) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// True for requests that may wait on the game (run them off the stdin thread).
pub fn is_slow(message: &Value) -> bool {
    matches!(
        message.get("method").and_then(Value::as_str),
        Some("tools/call" | "tools/list")
    ) && message.get("id").is_some()
}

impl Server {
    pub fn new(bridge: BridgeLink, settings: Settings, notify: Notifier) -> Self {
        Self {
            bridge,
            settings,
            notify,
            tools: Mutex::new(HashMap::new()),
            listed_without_engine: AtomicBool::new(false),
            shots: AtomicU64::new(0),
            made_screenshot_dir: AtomicBool::new(false),
        }
    }

    /// Handle one message from the client; returns the response to send, if any.
    pub fn handle(&self, message: &Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error_response(
                &Value::Null,
                codes::INVALID_REQUEST,
                "expected a JSON-RPC object (batches are not supported)",
            ));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            if object.contains_key("result") || object.contains_key("error") {
                // A response to a server request; this server sends none, so ignore it.
                return None;
            }
            return Some(error_response(
                &id.unwrap_or(Value::Null),
                codes::INVALID_REQUEST,
                "missing \"method\"",
            ));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // Notifications (`notifications/initialized`, `notifications/cancelled`, …) need
            // no answer and change nothing here.
            return None;
        };
        Some(match self.dispatch(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(f) => error_response(&id, f.code, &f.message),
        })
    }

    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, Failure> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.list_tools()),
            "tools/call" => self.call_tool(params),
            _ => Err(Failure {
                code: codes::METHOD_NOT_FOUND,
                message: format!(
                    "method not found: {method}. This server offers tools only (tools/list, \
                     tools/call)."
                ),
            }),
        }
    }

    fn initialize(&self, params: &Value) -> Value {
        let version = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|v| SUPPORTED_VERSIONS.contains(v))
            .unwrap_or(PROTOCOL_VERSION);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {
                "name": "engine-mcp",
                "title": "Engine game bridge",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": INSTRUCTIONS,
        })
    }

    /// Ask the game for its methods and refresh the tool-name map.
    fn load_methods(&self, wait_for_startup: bool) -> Result<Vec<Method>, CallError> {
        let timeout = self.settings.call_timeout;
        let result = if wait_for_startup {
            self.bridge.call("bridge.methods", json!({}), timeout)?
        } else {
            self.bridge.try_call("bridge.methods", json!({}), timeout)?
        };
        let methods = parse_methods(&result).map_err(CallError::Unavailable)?;
        let mut map = lock(&self.tools);
        map.clear();
        for m in &methods {
            map.entry(tool_name(&m.name))
                .or_insert_with(|| m.name.clone());
        }
        Ok(methods)
    }

    fn list_tools(&self) -> Value {
        let mut tools = builtin_tools();
        let methods = self
            .bridge
            .wait_ready(self.settings.list_wait)
            .map_err(CallError::Unavailable)
            .and_then(|_| self.load_methods(false));
        match methods {
            Ok(methods) => {
                let mut seen: Vec<String> = BUILTIN_TOOLS.iter().map(|s| s.to_string()).collect();
                for method in &methods {
                    let name = tool_name(&method.name);
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name);
                    tools.push(engine_tool(method));
                }
            }
            Err(e) => {
                self.listed_without_engine.store(true, Ordering::SeqCst);
                eprintln!("engine-mcp: listing only the built-in tools for now: {e}");
            }
        }
        json!({"tools": tools})
    }

    fn method_for_tool(&self, name: &str) -> Lookup {
        if let Some(method) = lock(&self.tools).get(name) {
            return Lookup::Found(method.clone());
        }
        // Not in the last list (it may be stale, or was made before the game was up).
        match self.load_methods(true) {
            Ok(_) => match lock(&self.tools).get(name) {
                Some(method) => Lookup::Found(method.clone()),
                None => Lookup::Unknown,
            },
            Err(e) => Lookup::Unreachable(e),
        }
    }

    fn call_tool(&self, params: &Value) -> Result<Value, Failure> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_params("tools/call needs a tool \"name\""))?;
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(a) => a.clone(),
        };
        let timeout = self.settings.call_timeout;
        match name {
            "screenshot" => Ok(self.screenshot(&args)),
            "step" => {
                let frames = args.get("frames").cloned().unwrap_or(json!(1));
                Ok(tool_result(self.bridge.call(
                    "engine.step",
                    json!({"frames": frames}),
                    timeout,
                )))
            }
            "call" => {
                let method = args.get("method").and_then(Value::as_str).ok_or_else(|| {
                    invalid_params(
                        "the call tool needs {\"method\": \"entity.list\", \"params\": {...}}",
                    )
                })?;
                let call_params = match args.get("params") {
                    None | Some(Value::Null) => json!({}),
                    Some(p) => p.clone(),
                };
                Ok(tool_result(self.bridge.call(method, call_params, timeout)))
            }
            _ => match self.method_for_tool(name) {
                Lookup::Found(method) => Ok(tool_result(self.bridge.call(&method, args, timeout))),
                Lookup::Unknown => Err(invalid_params(format!(
                    "unknown tool: {name}. Call tools/list to see the available tools."
                ))),
                Lookup::Unreachable(e) => Ok(text_content(format!("Error: {e}"), true)),
            },
        }
    }

    fn screenshot_path(&self, args: &Value) -> Result<PathBuf, String> {
        if let Some(path) = args.get("path").and_then(Value::as_str) {
            let path = Path::new(path);
            // The game always writes PNG data and overwrites whatever is there; insisting on the
            // extension keeps a mistyped path from clobbering a source file.
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))
            {
                return Err(format!(
                    "\"path\" must end in .png (got '{}'); omit it to use a temp file",
                    path.display()
                ));
            }
            return Ok(if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|e| e.to_string())?
                    .join(path)
            });
        }
        let dir = &self.settings.screenshot_dir;
        if !dir.is_dir() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            self.made_screenshot_dir.store(true, Ordering::SeqCst);
        }
        let n = self.shots.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(dir.join(format!("shot-{n:04}.png")))
    }

    fn screenshot(&self, args: &Value) -> Value {
        let path = match self.screenshot_path(args) {
            Ok(p) => p,
            Err(e) => return text_content(format!("Error: {e}"), true),
        };
        // An older file at the path must not pass for a fresh capture; remember its timestamp
        // instead of deleting it (a failed capture then leaves the user's file alone).
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let before = modified(&path);
        let shown = path.to_string_lossy().to_string();
        let answer = self.bridge.call(
            "render.screenshot",
            json!({"path": shown}),
            self.settings.call_timeout,
        );
        // The game may report an error although it saved the PNG (render.screenshot answers
        // "the frame was not rendered" from the 17th capture of a session on), so a file the
        // call rewrote counts as a success too.
        let rewritten = modified(&path).is_some_and(|after| before != Some(after));
        let bytes = if answer.is_ok() || rewritten {
            std::fs::read(&path)
        } else {
            Err(std::io::Error::other("not written"))
        };
        match (answer, bytes) {
            (_, Ok(bytes)) if bytes.starts_with(PNG_SIGNATURE) => {
                let (width, height) = png_size(&bytes).unwrap_or((0, 0));
                json!({
                    "content": [
                        {"type": "image", "data": base64::encode(&bytes), "mimeType": "image/png"},
                        {"type": "text", "text": format!(
                            "Screenshot {width}x{height} saved to {shown} ({} bytes).",
                            bytes.len()
                        )},
                    ],
                    "isError": false,
                })
            }
            (Err(e), _) => text_content(format!("Error: render.screenshot failed: {e}"), true),
            (Ok(_), Err(e)) => text_content(
                format!(
                    "Error: the game reports it saved {shown}, but this server cannot read it \
                     ({e}). Is the game running on another machine?"
                ),
                true,
            ),
            (Ok(_), Ok(_)) => text_content(format!("Error: {shown} is not a PNG file"), true),
        }
    }

    /// Background duty: report when a launched game comes up, and send
    /// `notifications/tools/list_changed` once the game answers after a `tools/list` had to
    /// leave its tools out. Runs until the process exits.
    pub fn watch(&self, startup_timeout: Duration) {
        if self.bridge.is_launched() {
            match self.bridge.wait_ready(startup_timeout) {
                Ok(()) => eprintln!(
                    "engine-mcp: the game's AI bridge is up on {}",
                    self.bridge.address()
                ),
                Err(e) => eprintln!("engine-mcp: {e}"),
            }
        }
        loop {
            std::thread::sleep(Duration::from_millis(500));
            self.notify_if_tools_changed();
        }
    }

    /// Send `list_changed` if a tool list lacked the engine's tools and the game is up now.
    pub fn notify_if_tools_changed(&self) -> bool {
        if !self.listed_without_engine.load(Ordering::SeqCst) {
            return false;
        }
        let up = self
            .bridge
            .try_call("bridge.ping", json!({}), Duration::from_secs(10))
            .is_ok();
        if up && self.listed_without_engine.swap(false, Ordering::SeqCst) {
            (self.notify)(json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}));
            return true;
        }
        false
    }

    /// Stop the launched game (if any) and remove the temp screenshot folder this server
    /// created.
    pub fn shutdown(&self) {
        self.bridge.shutdown();
        if self.made_screenshot_dir.swap(false, Ordering::SeqCst) {
            let _ = std::fs::remove_dir_all(&self.settings.screenshot_dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// A stand-in for a game's AI bridge: answers `bridge.methods`, `bridge.ping`,
    /// `entity.spawn` (echoing its params), `engine.step` and `render.screenshot` (writes a tiny
    /// PNG), and records every request it receives.
    struct FakeBridge {
        address: String,
        seen: Arc<Mutex<Vec<Value>>>,
    }

    const TINY_PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
        0, 0, 4, 0, 0, 0, 3, 8, 6, 0, 0, 0,
    ];

    impl FakeBridge {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let log = log.clone();
                    std::thread::spawn(move || serve(stream, log));
                }
            });
            Self { address, seen }
        }

        fn requests(&self, method: &str) -> Vec<Value> {
            lock(&self.seen)
                .iter()
                .filter(|r| r["method"] == method)
                .cloned()
                .collect()
        }
    }

    fn serve(stream: std::net::TcpStream, log: Arc<Mutex<Vec<Value>>>) {
        let mut writer = stream.try_clone().unwrap();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { return };
            let request: Value = serde_json::from_str(&line).unwrap();
            lock(&log).push(request.clone());
            let params = request["params"].clone();
            let answer = match request["method"].as_str().unwrap_or("") {
                "bridge.ping" => json!({"result": {"pong": true, "frame": 7}}),
                "bridge.methods" => json!({"result": {"count": 3, "methods": [
                    {"name": "entity.spawn", "summary": "Spawn an entity from component JSON.",
                     "args_example": r#"{"name": "Crate", "components": {}}"#},
                    {"name": "bridge.ping", "summary": "Check the game is alive", "args_example": "{}"},
                    {"name": "engine.step", "summary": "Run frames", "args_example": r#"{"frames": 10}"#},
                ]}}),
                "entity.spawn" => json!({"result": {"id": 42, "echo": params}}),
                "engine.step" => json!({"result": {"frames": params["frames"], "frame": 10}}),
                "render.screenshot" => {
                    let path = params["path"].as_str().unwrap().to_string();
                    if path.contains("no-gpu") {
                        // Fails without writing anything.
                        json!({"error": {"code": -32000, "message": "no GPU"}})
                    } else if path.contains("saved-then-error") {
                        // Writes the PNG but reports an error (the render crate does this from
                        // the 17th capture of a session).
                        std::fs::write(&path, TINY_PNG).unwrap();
                        json!({"error": {"code": -32000, "message": "the frame was not rendered"}})
                    } else {
                        std::fs::write(&path, TINY_PNG).unwrap();
                        json!({"result": {"path": path, "width": 4, "height": 3}})
                    }
                }
                other => {
                    json!({"error": {"code": -32601, "message": format!("unknown method '{other}'")}})
                }
            };
            let mut response = answer;
            response["jsonrpc"] = json!("2.0");
            response["id"] = request["id"].clone();
            let mut out = response.to_string();
            out.push('\n');
            if writer.write_all(out.as_bytes()).is_err() {
                return;
            }
        }
    }

    fn server_for(address: &str) -> (Server, Arc<Mutex<Vec<Value>>>) {
        let notes = Arc::new(Mutex::new(Vec::new()));
        let sink = notes.clone();
        // One folder per server: tests run in parallel and shutdown removes it.
        static SERVERS: AtomicU64 = AtomicU64::new(0);
        let n = SERVERS.fetch_add(1, Ordering::SeqCst);
        let settings = Settings {
            list_wait: Duration::from_secs(2),
            call_timeout: Duration::from_secs(10),
            screenshot_dir: std::env::temp_dir()
                .join(format!("engine-mcp-test-{}-{n}", std::process::id())),
        };
        let server = Server::new(
            BridgeLink::connect_to(address, None),
            settings,
            Box::new(move |n| lock(&sink).push(n)),
        );
        (server, notes)
    }

    fn request(server: &Server, id: u64, method: &str, params: Value) -> Value {
        let response = server
            .handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .expect("requests get a response");
        assert_eq!(response["jsonrpc"], json!("2.0"));
        assert_eq!(response["id"], json!(id));
        response
    }

    #[test]
    fn initialize_and_ping() {
        let fake = FakeBridge::start();
        let (server, _) = server_for(&fake.address);
        let init = request(
            &server,
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}),
        );
        let result = &init["result"];
        assert_eq!(result["protocolVersion"], json!(PROTOCOL_VERSION));
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], json!("engine-mcp"));
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("engine_describe")
        );

        // Unknown versions get ours; known older ones are echoed.
        let old = request(
            &server,
            2,
            "initialize",
            json!({"protocolVersion": "2024-11-05"}),
        );
        assert_eq!(old["result"]["protocolVersion"], json!("2024-11-05"));
        let future = request(
            &server,
            3,
            "initialize",
            json!({"protocolVersion": "2099-01-01"}),
        );
        assert_eq!(future["result"]["protocolVersion"], json!(PROTOCOL_VERSION));

        // Notifications get no answer.
        assert!(
            server
                .handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
        assert_eq!(request(&server, 4, "ping", json!({}))["result"], json!({}));
        let missing = request(&server, 5, "resources/list", json!({}));
        assert_eq!(missing["error"]["code"], json!(codes::METHOD_NOT_FOUND));
        let garbage = server.handle(&json!([1, 2])).unwrap();
        assert_eq!(garbage["error"]["code"], json!(codes::INVALID_REQUEST));
    }

    #[test]
    fn tools_list_maps_engine_methods() {
        let fake = FakeBridge::start();
        let (server, _) = server_for(&fake.address);
        let list = request(&server, 1, "tools/list", json!({}));
        let tools = list["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "screenshot",
            "step",
            "call",
            "entity_spawn",
            "bridge_ping",
            "engine_step",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        let spawn = tools.iter().find(|t| t["name"] == "entity_spawn").unwrap();
        let description = spawn["description"].as_str().unwrap();
        assert!(
            description.starts_with("Spawn an entity from component JSON."),
            "{description}"
        );
        assert!(
            description.contains(r#"Example args: {"name": "Crate", "components": {}}"#),
            "{description}"
        );
        assert_eq!(
            spawn["inputSchema"],
            json!({"type": "object", "additionalProperties": true})
        );
        // Every tool name is valid for MCP clients.
        for name in names {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{name}"
            );
        }
    }

    #[test]
    fn tools_call_round_trip() {
        let fake = FakeBridge::start();
        let (server, _) = server_for(&fake.address);
        request(&server, 1, "tools/list", json!({}));

        let args =
            json!({"name": "Crate", "components": {"Transform": {"translation": [0, 2, 0]}}});
        let call = request(
            &server,
            2,
            "tools/call",
            json!({"name": "entity_spawn", "arguments": args}),
        );
        let result = &call["result"];
        assert_eq!(result["isError"], json!(false), "{call}");
        let text = result["content"][0]["text"].as_str().unwrap();
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value["id"], json!(42));
        assert_eq!(value["echo"], args, "arguments reach the engine unchanged");
        let sent = fake.requests("entity.spawn");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["params"], args);

        // The step tool maps to engine.step.
        let step = request(
            &server,
            3,
            "tools/call",
            json!({"name": "step", "arguments": {"frames": 5}}),
        );
        assert_eq!(step["result"]["isError"], json!(false), "{step}");
        assert_eq!(
            fake.requests("engine.step")[0]["params"],
            json!({"frames": 5})
        );

        // The call tool reaches any method; bridge errors become tool errors.
        let call_tool = request(
            &server,
            4,
            "tools/call",
            json!({"name": "call", "arguments": {"method": "weather.set", "params": {"rain": 1}}}),
        );
        assert_eq!(call_tool["result"]["isError"], json!(true));
        let text = call_tool["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown method 'weather.set'"), "{text}");

        // Unknown tools are a protocol error.
        let unknown = request(&server, 5, "tools/call", json!({"name": "no_such_tool"}));
        assert_eq!(unknown["error"]["code"], json!(codes::INVALID_PARAMS));
    }

    #[test]
    fn tools_call_works_before_tools_list() {
        let fake = FakeBridge::start();
        let (server, _) = server_for(&fake.address);
        let call = request(&server, 1, "tools/call", json!({"name": "bridge_ping"}));
        assert_eq!(call["result"]["isError"], json!(false), "{call}");
        assert!(
            call["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("pong")
        );
    }

    #[test]
    fn screenshot_returns_image_content() {
        let fake = FakeBridge::start();
        let (server, _) = server_for(&fake.address);
        let shot = request(&server, 1, "tools/call", json!({"name": "screenshot"}));
        let content = shot["result"]["content"].as_array().unwrap();
        assert_eq!(shot["result"]["isError"], json!(false), "{shot}");
        assert_eq!(content[0]["type"], json!("image"));
        assert_eq!(content[0]["mimeType"], json!("image/png"));
        assert_eq!(content[0]["data"], json!(base64::encode(TINY_PNG)));
        let text = content[1]["text"].as_str().unwrap();
        assert!(text.contains("4x3"), "{text}");
        assert!(
            text.contains(&format!("({} bytes)", TINY_PNG.len())),
            "{text}"
        );
        // The game was asked to write to an absolute temp path.
        let path = fake.requests("render.screenshot")[0]["params"]["path"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(Path::new(&path).is_absolute(), "{path}");
        assert!(text.contains(&path));

        // The temp folder goes away with the server.
        assert!(Path::new(&path).exists());
        server.shutdown();
        assert!(!server.settings.screenshot_dir.exists(), "{path}");
    }

    #[test]
    fn screenshot_never_destroys_an_existing_file() {
        let fake = FakeBridge::start();
        let (server, _) = server_for(&fake.address);
        let dir = std::env::temp_dir().join(format!("engine-mcp-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = |id: u64, path: &Path| {
            let args = json!({"path": path.to_string_lossy()});
            request(
                &server,
                id,
                "tools/call",
                json!({"name": "screenshot", "arguments": args}),
            )
        };

        // A path that is not a .png is refused before the game is asked.
        let notes = dir.join("notes.md");
        std::fs::write(&notes, "keep me").unwrap();
        let refused = shot(1, &notes);
        assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
        assert!(fake.requests("render.screenshot").is_empty());
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "keep me");

        // A failed capture leaves an older file in place and is reported as a failure.
        let old = dir.join("no-gpu.png");
        std::fs::write(&old, TINY_PNG).unwrap();
        let failed = shot(2, &old);
        assert_eq!(failed["result"]["isError"], json!(true), "{failed}");
        let text = failed["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("no GPU"), "{text}");
        assert!(old.exists(), "the failed capture deleted the user's file");

        // A PNG the game wrote counts even if the game then reported an error.
        let late = dir.join("saved-then-error.png");
        let _ = std::fs::remove_file(&late);
        let saved = shot(3, &late);
        assert_eq!(saved["result"]["isError"], json!(false), "{saved}");
        assert_eq!(saved["result"]["content"][0]["type"], json!("image"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_game_tools_list_has_builtins_and_later_changes() {
        // Reserve the port for the whole test: until `up` is set, the "game" hangs up on every
        // connection (like a game that is not ready); then it serves the bridge protocol.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let up = Arc::new(AtomicBool::new(false));
        let game_up = up.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if game_up.load(Ordering::SeqCst) {
                    std::thread::spawn(move || serve(stream, Arc::new(Mutex::new(Vec::new()))));
                }
            }
        });

        let (mut server, notes) = server_for(&address);
        server.settings.list_wait = Duration::from_millis(300);
        let list = request(&server, 1, "tools/list", json!({}));
        let names: Vec<String> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, BUILTIN_TOOLS);
        assert!(!server.notify_if_tools_changed(), "no game yet");

        // Calls report the problem as a tool error the model can read.
        let call = request(&server, 2, "tools/call", json!({"name": "entity_list"}));
        assert_eq!(call["result"]["isError"], json!(true), "{call}");

        // The game comes up: the client is told the tool list changed.
        up.store(true, Ordering::SeqCst);
        assert!(server.notify_if_tools_changed());
        assert_eq!(
            lock(&notes).as_slice(),
            &[json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})]
        );
        let list = request(&server, 3, "tools/list", json!({}));
        assert!(list["result"]["tools"].as_array().unwrap().len() > BUILTIN_TOOLS.len());
    }

    #[test]
    fn names_and_png_sizes() {
        assert_eq!(tool_name("entity.spawn"), "entity_spawn");
        assert_eq!(tool_name("weird name/with:chars"), "weird_name_with_chars");
        assert_eq!(png_size(TINY_PNG), Some((4, 3)));
        assert_eq!(png_size(b"not a png"), None);
        assert!(is_slow(&json!({"id": 1, "method": "tools/call"})));
        assert!(!is_slow(&json!({"id": 1, "method": "initialize"})));
    }
}

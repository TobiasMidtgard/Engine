//! JSON-RPC 2.0 types shared by the TCP server, the main-thread executor and
//! [`BridgeClient`](crate::BridgeClient).
//!
//! On the wire every message is one JSON object on one line:
//!
//! ```text
//! → {"jsonrpc":"2.0","id":1,"method":"entity.get","params":{"name":"Player"}}
//! ← {"jsonrpc":"2.0","id":1,"result":{"id":4294967297,"name":"Player","components":{…}}}
//! ← {"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"no entity named 'Playr'"}}
//! ```

use std::fmt;
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// JSON-RPC error codes used by the bridge.
pub mod codes {
    /// The line was not valid JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// Valid JSON but not a JSON-RPC request object (missing `method`, a batch array, …).
    pub const INVALID_REQUEST: i64 = -32600;
    /// No API function or bridge method with that name.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The arguments have the wrong shape (for bridge-level methods such as `engine.step`).
    pub const INVALID_PARAMS: i64 = -32602;
    /// The API function panicked. The game keeps running.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The API function returned an error (its message says what to fix).
    pub const API_ERROR: i64 = -32000;
    /// A token is required (`ENGINE_BRIDGE_TOKEN`) and was missing or wrong.
    pub const UNAUTHORIZED: i64 = -32001;
    /// The game stopped (or is stopping) before it could answer.
    pub const SHUTTING_DOWN: i64 = -32002;
    /// The request was too large or the server is full.
    pub const LIMIT_EXCEEDED: i64 = -32003;
}

/// A JSON-RPC error object: `{"code": -32000, "message": "...", "data": ...}`.
///
/// `message` is always a plain sentence that says what went wrong and, where possible, what
/// to do instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// One of [`codes`].
    pub code: i64,
    /// Human- and AI-readable explanation.
    pub message: String,
    /// Optional structured details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// An error with the given code and message and no data.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// An error returned by an API function ([`codes::API_ERROR`]).
    pub fn api(message: impl Into<String>) -> Self {
        Self::new(codes::API_ERROR, message)
    }

    /// The game stopped before answering ([`codes::SHUTTING_DOWN`]).
    pub fn shutting_down() -> Self {
        Self::new(
            codes::SHUTTING_DOWN,
            "the game stopped before it could answer; start it again and reconnect",
        )
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// The outcome of one request.
pub type RpcResult = Result<Value, RpcError>;

/// A finished request on its way back to whoever asked.
#[derive(Debug)]
pub(crate) struct Reply {
    pub id: Value,
    pub result: RpcResult,
}

/// Where to send the answer to a request (absent for JSON-RPC notifications).
#[derive(Debug)]
pub(crate) struct ReplyTo {
    pub id: Value,
    pub tx: Sender<Reply>,
}

impl ReplyTo {
    /// Deliver the result. Never blocks; a vanished receiver is ignored.
    pub fn send(self, result: RpcResult) {
        let _ = self.tx.send(Reply {
            id: self.id,
            result,
        });
    }
}

/// One request queued for the main thread.
#[derive(Debug)]
pub(crate) struct Request {
    pub method: String,
    pub params: Value,
    pub reply: Option<ReplyTo>,
}

/// The JSON-RPC response object for a result.
pub fn response_json(id: &Value, result: &RpcResult) -> Value {
    match result {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }
}

/// Parse a JSON-RPC response object back into a result (used by clients).
pub fn parse_response(message: &Value) -> RpcResult {
    if let Some(error) = message.get("error") {
        return Err(serde_json::from_value(error.clone()).unwrap_or_else(|_| {
            RpcError::new(
                codes::INTERNAL_ERROR,
                format!("malformed error object: {error}"),
            )
        }));
    }
    match message.get("result") {
        Some(result) => Ok(result.clone()),
        None => Err(RpcError::new(
            codes::INVALID_REQUEST,
            "response has neither \"result\" nor \"error\"",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_round_trip() {
        let ok = response_json(&json!(7), &Ok(json!({"a": 1})));
        assert_eq!(ok, json!({"jsonrpc": "2.0", "id": 7, "result": {"a": 1}}));
        assert_eq!(parse_response(&ok), Ok(json!({"a": 1})));

        let err = response_json(&json!("x"), &Err(RpcError::api("nope")));
        assert_eq!(err["error"]["code"], json!(codes::API_ERROR));
        assert!(err["error"].get("data").is_none());
        assert_eq!(parse_response(&err), Err(RpcError::api("nope")));
    }

    #[test]
    fn display_is_readable() {
        let e = RpcError::new(codes::UNAUTHORIZED, "send a token");
        assert_eq!(e.to_string(), "send a token (code -32001)");
    }
}

//! The request queue between other threads and the game's main thread, and the in-process
//! [`BridgeClient`] that uses it.

use std::cell::OnceCell;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

use bevy_ecs::prelude::*;
use serde_json::Value;

use crate::protocol::{Reply, ReplyTo, Request, RpcError, RpcResult};

/// The queue of pending requests. The TCP server threads and every [`BridgeClient`] push into
/// it; the bridge's frame-start system drains it on the main thread.
#[derive(Resource)]
pub struct BridgeQueue {
    pub(crate) tx: Sender<Request>,
    pub(crate) rx: Mutex<Receiver<Request>>,
}

impl Default for BridgeQueue {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx: Mutex::new(rx),
        }
    }
}

impl BridgeQueue {
    /// A client that sends requests into this queue.
    pub fn client(&self) -> BridgeClient {
        BridgeClient {
            tx: self.tx.clone(),
        }
    }

    /// Take up to `max` queued requests without blocking.
    pub(crate) fn drain(&self, max: usize) -> Vec<Request> {
        let rx = match self.rx.lock() {
            Ok(rx) => rx,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut out = Vec::new();
        while out.len() < max {
            match rx.try_recv() {
                Ok(request) => out.push(request),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

/// Calls engine APIs from any thread of the same process, through the same queue and the same
/// main-thread executor as TCP clients (so `engine.step` waits for frames here too). Use it in
/// tests, editors and embedded tools. No token is needed in-process.
///
/// ```no_run
/// # use engine_core::prelude::*;
/// # use engine_ai_bridge::{AiBridgePlugin, BridgeClient};
/// let mut app = App::new();
/// app.add_plugins(AiBridgePlugin::default());
/// let client = BridgeClient::from_world(&app.world).unwrap();
/// let worker = std::thread::spawn(move || client.call("engine.status", serde_json::json!({})));
/// while !worker.is_finished() {
///     app.update(); // the main thread answers while it runs frames
/// }
/// println!("{:?}", worker.join().unwrap());
/// ```
///
/// **Never call [`BridgeClient::call`] on the thread that runs `app.update()`**: the answer is
/// produced by the next frame, so it would wait forever. On that thread use
/// [`BridgeClient::request`] and poll the [`PendingReply`] between frames.
#[derive(Clone, Debug)]
pub struct BridgeClient {
    tx: Sender<Request>,
}

impl BridgeClient {
    /// A client for the bridge installed in this world (`None` if [`crate::AiBridgePlugin`]
    /// was not added).
    pub fn from_world(world: &World) -> Option<Self> {
        world.get_resource::<BridgeQueue>().map(BridgeQueue::client)
    }

    /// Queue a request and return immediately. The answer arrives during a later frame.
    pub fn request(&self, method: &str, params: Value) -> PendingReply {
        let (tx, rx) = mpsc::channel();
        let request = Request {
            method: method.to_string(),
            params,
            reply: Some(ReplyTo {
                id: Value::Null,
                tx,
            }),
        };
        // If the game is gone, the request (and its reply sender) is dropped and the pending
        // reply reports "shutting down".
        let _ = self.tx.send(request);
        PendingReply {
            rx,
            result: OnceCell::new(),
        }
    }

    /// Queue a request whose result nobody needs (a JSON-RPC notification).
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.tx.send(Request {
            method: method.to_string(),
            params,
            reply: None,
        });
    }

    /// Send a request and block until the main thread answers it. See the type docs for the
    /// one thread you must not call this from.
    pub fn call(&self, method: &str, params: Value) -> RpcResult {
        self.request(method, params).wait()
    }

    /// Like [`BridgeClient::call`] but gives up after `timeout` (the request may still run).
    pub fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> RpcResult {
        self.request(method, params).wait_timeout(timeout)
    }
}

/// The answer to a [`BridgeClient::request`], available once the main thread has run it.
#[derive(Debug)]
pub struct PendingReply {
    rx: Receiver<Reply>,
    /// The answer once it arrived, so [`PendingReply::try_get`] can be asked again.
    result: OnceCell<RpcResult>,
}

impl PendingReply {
    /// The result if it has arrived, without blocking. Once it has, every call returns it.
    pub fn try_get(&self) -> Option<RpcResult> {
        if let Some(result) = self.result.get() {
            return Some(result.clone());
        }
        let result = match self.rx.try_recv() {
            Ok(reply) => reply.result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(RpcError::shutting_down()),
        };
        Some(self.result.get_or_init(|| result).clone())
    }

    /// Block until the result arrives.
    pub fn wait(self) -> RpcResult {
        if let Some(result) = self.result.into_inner() {
            return result;
        }
        match self.rx.recv() {
            Ok(reply) => reply.result,
            Err(_) => Err(RpcError::shutting_down()),
        }
    }

    /// Block until the result arrives or `timeout` passes.
    pub fn wait_timeout(self, timeout: Duration) -> RpcResult {
        if let Some(result) = self.result.into_inner() {
            return result;
        }
        match self.rx.recv_timeout(timeout) {
            Ok(reply) => reply.result,
            Err(RecvTimeoutError::Timeout) => Err(RpcError::new(
                crate::protocol::codes::INTERNAL_ERROR,
                format!(
                    "no answer within {:.1} s; is the game running frames (app.update)?",
                    timeout.as_secs_f64()
                ),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(RpcError::shutting_down()),
        }
    }
}

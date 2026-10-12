//! # engine_ai_bridge
//!
//! Lets AI agents (and any other tool) drive a running game: every API function registered with
//! `App::register_api` (`engine.describe`, `entity.*`, `resource.*`, `scene.*`,
//! `render.screenshot`, `input.*`, module APIs…) is served as newline-delimited **JSON-RPC 2.0
//! over TCP**, plus a few bridge-level methods:
//!
//! | Method | Does |
//! |---|---|
//! | `bridge.ping` | `{"pong": true, "frame": n}` — answered by the main thread, so it proves frames run |
//! | `bridge.methods` | every callable method with summary and argument example |
//! | `engine.step {frames}` | answers after exactly `frames` more frames; a paused game runs them and pauses again |
//! | `engine.pause` / `engine.resume` | pause world time / restore the speed from before the pause |
//! | `engine.status` | frame, fps, paused, time scale, entity count, clients |
//! | `engine.exit {code}` | stop the game (sends `AppExit`) |
//! | `auth {token}` | unlock a connection when `ENGINE_BRIDGE_TOKEN` is set |
//!
//! ## Enabling
//!
//! `engine::DefaultPlugins` already includes [`AiBridgePlugin`] (as `AiBridgePlugin::default()`),
//! so games built on it have the bridge; apps without `DefaultPlugins` add the plugin
//! themselves. The TCP server starts when the `ENGINE_BRIDGE` environment variable is set
//! (`ENGINE_BRIDGE=127.0.0.1:7878`, `ENGINE_BRIDGE=1` for that default, `ENGINE_BRIDGE=0` or
//! `127.0.0.1:0` for a free port) or when the plugin settings say so
//! ([`AiBridgePlugin::listen`]). To change the settings of the copy in `DefaultPlugins`, replace
//! it: `DefaultPlugins.set(AiBridgePlugin::listen("127.0.0.1:0"))`. A second `AiBridgePlugin`
//! added after `DefaultPlugins` is ignored, settings included (the app logs a warning).
//!
//! The bound address is stored in [`BridgeInfo`] and logged. It binds to localhost unless
//! told otherwise. If `ENGINE_BRIDGE_TOKEN` (or
//! [`AiBridgePlugin::token`]) is set, every TCP request must carry `"token": "..."` or the
//! connection must first call `auth`.
//!
//! Without the TCP server the plugin still installs the request queue, so in-process
//! [`BridgeClient`]s work (tests, editors, embedding).
//!
//! ## Threading
//!
//! Network threads only parse and queue requests. The main thread runs them in an exclusive
//! system at the very start of the frame (`First`, before the clock advances) and hands the
//! answers back over channels, so a slow or stalled client never blocks the game loop. Running
//! before the clock means:
//!
//! - edits (spawns, input presses, resource patches) are seen by the whole frame — input
//!   systems, simulation, transform propagation and rendering — so a following
//!   `render.screenshot` shows them after one step;
//! - `engine.step` / `engine.pause` / `engine.resume` change `Time` before `time_system` runs, so
//!   stepping a paused game advances exactly the requested number of frames.
//!
//! Finished steps are answered from `Last`, once their frames have fully run.
//!
//! See `README.md` for a protocol walkthrough with `nc`.

mod client;
pub mod protocol;
mod server;
mod step;

use std::panic::AssertUnwindSafe;

use bevy_ecs::prelude::*;
use engine_core::diagnostics::Diagnostics;
use engine_core::prelude::*;
use engine_core::registry::{ApiRegistry, call_api, unknown_name_error};
use serde_json::{Value, json};

pub use client::{BridgeClient, BridgeQueue, PendingReply};
pub use protocol::{RpcError, RpcResult, codes};
pub use server::{BridgeServer, MAX_CLIENTS, MAX_REQUEST_BYTES};
pub use step::{MAX_STEP_FRAMES, StepControl};

use protocol::Request;

/// Address used when the bridge is enabled without one.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:7878";

/// Environment variable that enables the TCP server and sets its address.
pub const ENV_ADDRESS: &str = "ENGINE_BRIDGE";

/// Environment variable holding the token clients must send.
pub const ENV_TOKEN: &str = "ENGINE_BRIDGE_TOKEN";

/// When set (to anything but `0`/`false`), the game exits once its stdin closes. Launchers such
/// as `engine-mcp --launch` set it and keep a pipe open, so the game never outlives them. It
/// also marks the game as started for its bridge: if the bridge cannot listen, the game exits
/// (with code 1) instead of running where its launcher can never reach it.
pub const ENV_EXIT_ON_STDIN_EOF: &str = "ENGINE_BRIDGE_EXIT_ON_STDIN_EOF";

/// Seconds the game gets to exit cleanly after its launcher went away, before it is forced to.
const ORPHAN_EXIT_GRACE_SECS: u64 = 15;

/// Serves the engine API to AI agents and tools. See the [crate docs](crate).
///
/// `engine::DefaultPlugins` already includes `AiBridgePlugin::default()`, which is off unless
/// `ENGINE_BRIDGE` is set (the usual choice for games). To change its settings, replace the
/// group's copy; a second `AiBridgePlugin` added after `DefaultPlugins` is ignored:
///
/// ```ignore
/// use engine::ai_bridge::AiBridgePlugin;
/// use engine::prelude::*;
/// // Always on, on a free port (read BridgeInfo::address to find it):
/// App::new().add_plugins(DefaultPlugins.set(AiBridgePlugin::listen("127.0.0.1:0")));
/// // Always on, and TCP clients must send a token:
/// App::new().add_plugins(
///     DefaultPlugins.set(AiBridgePlugin::listen("127.0.0.1:0").with_token("secret")),
/// );
/// ```
///
/// Apps that don't use `DefaultPlugins` add the plugin themselves:
///
/// ```no_run
/// # use engine_core::prelude::*;
/// # use engine_ai_bridge::AiBridgePlugin;
/// // Off unless ENGINE_BRIDGE is set:
/// App::new().add_plugins(AiBridgePlugin::default());
/// // Always on, on a free port (read BridgeInfo::address to find it):
/// App::new().add_plugins(AiBridgePlugin::listen("127.0.0.1:0"));
/// ```
#[derive(Clone, Debug)]
pub struct AiBridgePlugin {
    /// Start the TCP server even when `ENGINE_BRIDGE` is not set.
    pub enabled: bool,
    /// Address to bind, `host:port` (port 0 picks a free one). `ENGINE_BRIDGE` overrides it,
    /// so a launcher (such as `engine-mcp --launch`) can choose the port.
    pub address: String,
    /// Token TCP clients must send. `ENGINE_BRIDGE_TOKEN` overrides it.
    pub token: Option<String>,
    /// Read `ENGINE_BRIDGE` and `ENGINE_BRIDGE_TOKEN` (default true). Tests turn this off so the
    /// environment cannot change them.
    pub read_env: bool,
    /// Most requests run per frame; the rest wait for the next frame (keeps frame time bounded
    /// when a client floods the bridge).
    pub max_requests_per_frame: usize,
}

impl Default for AiBridgePlugin {
    fn default() -> Self {
        Self {
            enabled: false,
            address: DEFAULT_ADDRESS.to_string(),
            token: None,
            read_env: true,
            max_requests_per_frame: 256,
        }
    }
}

impl AiBridgePlugin {
    /// Always listen on `address` (`ENGINE_BRIDGE` can still override the address).
    pub fn listen(address: impl Into<String>) -> Self {
        Self {
            enabled: true,
            address: address.into(),
            ..Default::default()
        }
    }

    /// Require this token from TCP clients.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Ignore `ENGINE_BRIDGE` / `ENGINE_BRIDGE_TOKEN` (for tests).
    pub fn without_env(mut self) -> Self {
        self.read_env = false;
        self
    }

    /// The address to bind (if the server should run) and the token, after applying the
    /// environment.
    fn resolve(&self) -> (Option<String>, Option<String>) {
        let mut address = self.enabled.then(|| self.address.clone());
        let mut token = self.token.clone().filter(|t| !t.is_empty());
        if self.read_env {
            if let Ok(value) = std::env::var(ENV_ADDRESS) {
                address = parse_env_address(&value, &self.address);
            }
            if let Ok(value) = std::env::var(ENV_TOKEN)
                && !value.is_empty()
            {
                token = Some(value);
            }
        }
        (address, token)
    }
}

/// Interpret `ENGINE_BRIDGE`: `""`/`1`/`true`/`on`/`yes` → the default address, `off`/`false`/
/// `no` → disabled, a bare port → `127.0.0.1:<port>`, anything else is used as the address.
pub fn parse_env_address(value: &str, default: &str) -> Option<String> {
    let v = value.trim();
    match v.to_ascii_lowercase().as_str() {
        "" | "1" | "true" | "on" | "yes" => Some(default.to_string()),
        "off" | "false" | "no" | "disabled" => None,
        _ if v.chars().all(|c| c.is_ascii_digit()) => Some(format!("127.0.0.1:{v}")),
        _ => Some(v.to_string()),
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !v.is_empty() && v != "0" && v != "false" && v != "no" && v != "off"
    })
}

/// Watch stdin on a background thread; when it closes (the launcher exited or crashed), ask the
/// game to exit, and force it if it does not within a grace period.
fn exit_when_stdin_closes(client: BridgeClient) {
    let spawned = std::thread::Builder::new()
        .name("ai-bridge-stdin".into())
        .spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            tracing::info!("AI bridge: stdin closed (the launcher went away); exiting");
            client.notify("engine.exit", json!({}));
            std::thread::sleep(std::time::Duration::from_secs(ORPHAN_EXIT_GRACE_SECS));
            tracing::warn!(
                "AI bridge: the game did not exit {ORPHAN_EXIT_GRACE_SECS} s after its launcher \
                 went away; forcing it"
            );
            std::process::exit(0);
        });
    if let Err(e) = spawned {
        tracing::warn!("AI bridge: could not watch stdin: {e}");
    }
}

/// Where the bridge is listening. Read it with `resource.get {"name": "BridgeInfo"}` or from
/// Rust after adding the plugin. Read-only: changing it has no effect.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgeInfo {
    /// True while the TCP server accepts clients.
    pub listening: bool,
    /// The bound `host:port` (the real port when port 0 was requested).
    pub address: Option<String>,
    /// TCP clients must authenticate with the token.
    pub token_required: bool,
    /// TCP clients connected right now.
    pub clients: u32,
    /// Requests executed since start (TCP and in-process).
    pub requests: u64,
    /// Why the server is not listening although it was asked to (for example "address in use").
    pub error: Option<String>,
}

/// Plugin-internal settings read by the systems.
#[derive(Resource, Debug, Clone, Copy)]
struct BridgeLimits {
    max_requests_per_frame: usize,
}

impl Plugin for AiBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BridgeQueue>()
            .init_resource::<StepControl>()
            .init_resource::<BridgeInfo>()
            .insert_resource(BridgeLimits {
                max_requests_per_frame: self.max_requests_per_frame.max(1),
            })
            .register_resource::<BridgeInfo>(
                "AI bridge state (read-only): listening, address (host:port), token_required, \
                 clients, requests served, error.",
            )
            .add_systems(
                First,
                bridge_frame_start_system.before(engine_core::time::time_system),
            )
            .add_systems(Last, bridge_frame_end_system);
        register_api(app);
        let launched = self.read_env && env_flag(ENV_EXIT_ON_STDIN_EOF);
        if launched {
            exit_when_stdin_closes(app.world.resource::<BridgeQueue>().client());
        }

        let (address, token) = self.resolve();
        let Some(address) = address else {
            return;
        };
        let queue = app.world.resource::<BridgeQueue>().tx.clone();
        let mut info = BridgeInfo {
            token_required: token.is_some(),
            ..Default::default()
        };
        match BridgeServer::start(&address, token.clone(), queue) {
            Ok(server) => {
                let bound = server.local_addr();
                tracing::info!(
                    "AI bridge listening on {bound} (JSON-RPC 2.0, one object per line{})",
                    if token.is_some() {
                        ", token required"
                    } else {
                        ""
                    }
                );
                if !bound.ip().is_loopback() && token.is_none() {
                    tracing::warn!(
                        "AI bridge is reachable from other machines at {bound} without a token; \
                         anyone who can connect can control the game. Set {ENV_TOKEN}."
                    );
                }
                info.listening = true;
                info.address = Some(bound.to_string());
                app.insert_resource(server);
            }
            Err(e) => {
                let message = format!(
                    "could not listen on {address}: {e}. Pick another port with \
                     {ENV_ADDRESS}=127.0.0.1:<port> (0 = any free port)."
                );
                tracing::error!("AI bridge {message}");
                if launched {
                    // The launcher waits for this bridge; a game it cannot reach is useless to
                    // it, and exiting tells it at once instead of after its startup timeout.
                    tracing::error!(
                        "AI bridge: {ENV_EXIT_ON_STDIN_EOF} is set (launched by a tool that \
                         drives the game through the bridge); exiting"
                    );
                    app.world.write_message(AppExit::Error(1));
                }
                info.error = Some(message);
            }
        }
        app.insert_resource(info);
    }
}

/// `First`, before the clock: run queued requests and count this frame for steps in progress.
pub fn bridge_frame_start_system(world: &mut World) {
    let max = world.resource::<BridgeLimits>().max_requests_per_frame;
    let batch = world.resource::<BridgeQueue>().drain(max);
    let executed = batch.len() as u64;
    for request in batch {
        execute_request(world, request);
    }
    step::count_frame(world);

    let clients = world
        .get_resource::<BridgeServer>()
        .map(BridgeServer::clients)
        .unwrap_or(0);
    let (stale, mut requests) = {
        let info = world.resource::<BridgeInfo>();
        (executed > 0 || info.clients != clients, info.requests)
    };
    if stale {
        // Only touch the resource when something changed, so change detection stays quiet.
        let mut info = world.resource_mut::<BridgeInfo>();
        info.requests += executed;
        info.clients = clients;
        requests = info.requests;
    }
    if let Some(mut diagnostics) = world.get_resource_mut::<Diagnostics>() {
        diagnostics.set("bridge.clients", clients as f64);
        diagnostics.set("bridge.requests", requests as f64);
    }
}

/// `Last`: answer `engine.step` requests whose frames have all run, and pause again if needed.
pub fn bridge_frame_end_system(world: &mut World) {
    step::finish_steps(world);
}

/// Run one request on the main thread.
fn execute_request(world: &mut World, request: Request) {
    let Request {
        method,
        params,
        reply,
    } = request;
    if method == "engine.step" {
        match step::parse_frames(&params) {
            Ok(frames) => step::begin_step(world, frames, reply),
            Err(e) => {
                if let Some(reply) = reply {
                    reply.send(Err(e));
                }
            }
        }
        return;
    }
    let result = call_method(world, &method, params);
    if let Some(reply) = reply {
        reply.send(result);
    }
}

/// Call a registered API function, turning unknown names, errors and panics into JSON-RPC
/// errors. A panicking API function does not take the game down.
pub fn call_method(world: &mut World, method: &str, params: Value) -> RpcResult {
    let known = world.resource::<ApiRegistry>().get(method).is_some();
    if !known {
        let registry = world.resource::<ApiRegistry>();
        let mut message =
            unknown_name_error("method", method, registry.list().map(|e| e.name.as_str()));
        if message.ends_with('?') {
            message.push_str(" Call bridge.methods to list every method.");
        }
        return Err(RpcError::new(codes::METHOD_NOT_FOUND, message));
    }
    match std::panic::catch_unwind(AssertUnwindSafe(|| call_api(world, method, params))) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(RpcError::api(message)),
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            tracing::error!("AI bridge: {method} panicked: {detail}");
            Err(RpcError::new(
                codes::INTERNAL_ERROR,
                format!(
                    "{method} panicked ({detail}). The game is still running; this is a bug in \
                     that API function"
                ),
            ))
        }
    }
}

fn status(world: &mut World) -> Value {
    let entities = world.entities().count_spawned();
    let time = world.resource::<Time>();
    let fixed = world.resource::<FixedTime>();
    let diagnostics = world.resource::<Diagnostics>();
    let control = world.resource::<StepControl>();
    let info = world.resource::<BridgeInfo>();
    json!({
        "frame": time.frame_count(),
        "fps": diagnostics.fps,
        "frame_ms": diagnostics.frame_ms,
        "paused": time.is_paused(),
        "time_scale": time.scale(),
        "speed": time.base_scale(),
        "elapsed_world_secs": time.elapsed_secs_f64(),
        "elapsed_real_secs": time.real_elapsed_secs_f64(),
        "fixed_tick": fixed.tick(),
        "entities": entities,
        "stepping_frames_remaining": control.frames_remaining(),
        "pause_after_step": control.pauses_after_steps(),
        "bridge": {"address": info.address, "clients": info.clients, "requests": info.requests},
    })
}

fn register_api(app: &mut App) {
    app.register_api(
        "bridge.ping",
        "Check the game is alive and running frames. Returns {pong: true, frame}.",
        "{}",
        |world, _| Ok(json!({"pong": true, "frame": world.resource::<Time>().frame_count()})),
    )
    .register_api(
        "bridge.methods",
        "List every callable method with its summary and an example of its arguments.",
        "{}",
        |world, _| {
            let registry = world.resource::<ApiRegistry>();
            let methods: Vec<Value> = registry
                .list()
                .map(|e| json!({"name": e.name, "summary": e.summary, "args_example": e.args_example}))
                .collect();
            Ok(json!({"count": methods.len(), "methods": methods}))
        },
    )
    .register_api(
        "engine.step",
        "Run exactly `frames` more frames (default 1), then answer. A paused game runs those \
         frames and pauses again, so you can advance it frame by frame. Over the bridge the \
         answer {frames, start_frame, frame, paused} arrives when they are done.",
        r#"{"frames": 10}"#,
        |world, args| {
            // Called directly (console, scripts): schedule the frames and return at once.
            let frames = step::parse_frames(&args).map_err(|e| e.message)?;
            step::begin_step(world, frames, None);
            Ok(json!({"scheduled_frames": frames}))
        },
    )
    .register_api(
        "engine.pause",
        "Pause world time (frames keep rendering). Use engine.step to advance while paused.",
        "{}",
        |world, _| Ok(step::pause(world)),
    )
    .register_api(
        "engine.resume",
        "Resume world time at the speed it had before engine.pause.",
        "{}",
        |world, _| Ok(step::resume(world)),
    )
    .register_api(
        "engine.exit",
        "Stop the game at the end of this frame (sends AppExit). code 0 = success.",
        r#"{"code": 0}"#,
        |world, args| {
            let code = args.get("code").and_then(Value::as_u64).unwrap_or(0);
            let exit = match code {
                0 => AppExit::Success,
                c => AppExit::Error(c.min(255) as u8),
            };
            world.write_message(exit);
            Ok(json!({"exiting": true}))
        },
    )
    .register_api(
        "engine.status",
        "Frame number, fps, paused, time scale, entity count, stepping progress, bridge clients.",
        "{}",
        |world, _| Ok(status(world)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::time::TimeSource;

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(AiBridgePlugin::default().without_env());
        app
    }

    #[test]
    fn env_address_forms() {
        let d = DEFAULT_ADDRESS;
        assert_eq!(parse_env_address("", d).as_deref(), Some(d));
        assert_eq!(parse_env_address("1", d).as_deref(), Some(d));
        assert_eq!(parse_env_address("true", d).as_deref(), Some(d));
        assert_eq!(parse_env_address("off", d), None);
        assert_eq!(parse_env_address("0", d).as_deref(), Some("127.0.0.1:0"));
        assert_eq!(
            parse_env_address("9000", d).as_deref(),
            Some("127.0.0.1:9000")
        );
        assert_eq!(
            parse_env_address(" 0.0.0.0:7000 ", d).as_deref(),
            Some("0.0.0.0:7000")
        );
    }

    #[test]
    fn disabled_by_default_without_env() {
        let app = app();
        let info = app.world.resource::<BridgeInfo>();
        assert!(!info.listening && info.address.is_none());
        assert!(app.world.get_resource::<BridgeServer>().is_none());
    }

    #[test]
    fn in_process_client_runs_apis() {
        let mut app = app();
        let client = BridgeClient::from_world(&app.world).unwrap();
        let ping = client.request("bridge.ping", json!({}));
        assert!(ping.try_get().is_none(), "answered before a frame ran");
        app.update();
        assert_eq!(ping.try_get().unwrap().unwrap()["pong"], json!(true));
        // Asking again gives the same answer (not "the game stopped").
        assert_eq!(ping.try_get().unwrap().unwrap()["pong"], json!(true));
        assert_eq!(ping.wait().unwrap()["pong"], json!(true));

        let missing = client.request("entity.spwan", json!({}));
        app.update();
        let err = missing.try_get().unwrap().unwrap_err();
        assert_eq!(err.code, codes::METHOD_NOT_FOUND);
        assert!(err.message.contains("Did you mean 'entity.spawn'"), "{err}");

        let bad = client.request("entity.get", json!({"name": "Nobody"}));
        app.update();
        let err = bad.try_get().unwrap().unwrap_err();
        assert_eq!(err.code, codes::API_ERROR);
        assert!(err.message.contains("no entity named 'Nobody'"), "{err}");
    }

    #[test]
    fn step_answers_after_exactly_n_frames_and_repauses() {
        let mut app = app();
        let client = BridgeClient::from_world(&app.world).unwrap();
        app.world.resource_mut::<Time>().pause();
        app.update_n(2);
        let before = app.world.resource::<Time>().elapsed_secs_f64();

        let step = client.request("engine.step", json!({"frames": 3}));
        app.update_n(2);
        assert!(step.try_get().is_none(), "answered after only 2 frames");
        assert!(!app.world.resource::<Time>().is_paused());
        app.update();
        let result = step.try_get().expect("answered after 3 frames").unwrap();
        assert_eq!(result["frames"], json!(3));
        assert_eq!(
            result["frame"].as_u64().unwrap() - result["start_frame"].as_u64().unwrap(),
            3
        );
        assert_eq!(result["paused"], json!(true));
        let time = app.world.resource::<Time>();
        assert!(time.is_paused());
        let advanced = time.elapsed_secs_f64() - before;
        assert!((advanced - 3.0 / 60.0).abs() < 1e-9, "advanced {advanced}");

        // Still paused afterwards: world time stands still.
        app.update_n(3);
        let after = app.world.resource::<Time>().elapsed_secs_f64();
        assert!((after - before - 3.0 / 60.0).abs() < 1e-9);
    }

    #[test]
    fn step_on_a_running_game_keeps_it_running() {
        let mut app = app();
        let client = BridgeClient::from_world(&app.world).unwrap();
        let step = client.request("engine.step", json!({"frames": 2}));
        app.update_n(2);
        assert_eq!(step.try_get().unwrap().unwrap()["paused"], json!(false));
        assert!(!app.world.resource::<Time>().is_paused());

        let zero = client.request("engine.step", json!({"frames": 0}));
        app.update();
        assert!(zero.try_get().unwrap().is_ok());

        let bad = client.request("engine.step", json!({"frames": -1}));
        app.update();
        assert_eq!(
            bad.try_get().unwrap().unwrap_err().code,
            codes::INVALID_PARAMS
        );
    }

    #[test]
    fn pause_and_resume_restore_speed() {
        let mut app = app();
        app.world.resource_mut::<Time>().set_base_scale(0.5);
        call_api(&mut app.world, "engine.pause", json!({})).unwrap();
        assert!(app.world.resource::<Time>().is_paused());
        call_api(&mut app.world, "engine.resume", json!({})).unwrap();
        assert_eq!(app.world.resource::<Time>().base_scale(), 0.5);

        // A pause requested during a step waits for the step to finish.
        call_api(&mut app.world, "engine.step", json!({"frames": 2})).unwrap();
        let r = call_api(&mut app.world, "engine.pause", json!({})).unwrap();
        assert_eq!(r["pause_after_step"], json!(true));
        app.update();
        assert!(!app.world.resource::<Time>().is_paused());
        app.update();
        assert!(app.world.resource::<Time>().is_paused());
    }

    #[test]
    fn pause_during_a_step_keeps_the_speed_for_resume() {
        let mut app = app();
        app.world.resource_mut::<Time>().set_base_scale(0.5);
        let client = BridgeClient::from_world(&app.world).unwrap();
        let step = client.request("engine.step", json!({"frames": 2}));
        let pause = client.request("engine.pause", json!({}));
        app.update();
        assert_eq!(
            pause.try_get().unwrap().unwrap()["pause_after_step"],
            json!(true)
        );
        app.update();
        assert_eq!(step.try_get().unwrap().unwrap()["paused"], json!(true));
        assert!(app.world.resource::<Time>().is_paused());

        let resume = call_api(&mut app.world, "engine.resume", json!({})).unwrap();
        assert_eq!(resume["speed"], json!(0.5));
        assert_eq!(app.world.resource::<Time>().base_scale(), 0.5);
    }

    #[test]
    fn status_and_methods() {
        let mut app = app();
        app.world.spawn(Name::new("A"));
        app.update();
        let status = call_api(&mut app.world, "engine.status", json!({})).unwrap();
        assert_eq!(status["frame"], json!(1));
        assert_eq!(status["paused"], json!(false));
        assert!(status["entities"].as_u64().unwrap() >= 1);
        let methods = call_api(&mut app.world, "bridge.methods", json!({})).unwrap();
        let names: Vec<&str> = methods["methods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        for expected in [
            "bridge.ping",
            "engine.step",
            "engine.status",
            "entity.spawn",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
    }

    #[test]
    fn exit_api_sends_app_exit() {
        let mut app = app();
        call_api(&mut app.world, "engine.exit", json!({"code": 3})).unwrap();
        assert_eq!(app.should_exit(), Some(AppExit::Error(3)));
    }

    #[test]
    fn panicking_api_is_reported_not_fatal() {
        let mut app = app();
        app.register_api("test.boom", "panics", "{}", |_, _| panic!("kaboom"));
        let client = BridgeClient::from_world(&app.world).unwrap();
        let pending = client.request("test.boom", json!({}));
        app.update();
        let err = pending.try_get().unwrap().unwrap_err();
        assert_eq!(err.code, codes::INTERNAL_ERROR);
        assert!(err.message.contains("kaboom"), "{err}");
        app.update();
    }
}

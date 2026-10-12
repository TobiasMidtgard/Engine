# engine_ai_bridge: AI bridge

**Owner:** Core.

Lets AI agents and tools drive a running game, windowed or headless. Every API function
registered with `app.register_api` is served as **newline-delimited JSON-RPC 2.0 over TCP**.
That covers `engine.describe`, `entity.*`, `resource.*`, `scene.*`, `time.*`,
`render.screenshot`, `input.*` and every module API. The bridge adds stepping, pause/resume
and status on top. [`tools/engine-mcp`](../../tools/engine-mcp) uses this bridge to make a
game available to MCP clients such as Claude Code.

## Enable it

```rust
app.add_plugins(engine_ai_bridge::AiBridgePlugin::default()); // on when ENGINE_BRIDGE is set
app.add_plugins(engine_ai_bridge::AiBridgePlugin::listen("127.0.0.1:0")); // always on, free port
```

| Setting | Effect |
|---|---|
| `ENGINE_BRIDGE=127.0.0.1:7878` | listen on that address (overrides the plugin's address) |
| `ENGINE_BRIDGE=1` / `true` / empty | listen on the default `127.0.0.1:7878` |
| `ENGINE_BRIDGE=0` or `127.0.0.1:0` | listen on a free port, which is logged and stored in `BridgeInfo.address` |
| `ENGINE_BRIDGE=9000` | listen on `127.0.0.1:9000` |
| `ENGINE_BRIDGE=off` | stay off even if the plugin says `enabled` |
| `ENGINE_BRIDGE_TOKEN=secret` | require the token (see below) |
| `ENGINE_BRIDGE_EXIT_ON_STDIN_EOF=1` | exit when stdin closes (launchers set this so the game never outlives them); also exit at once if the bridge cannot listen |
| `AiBridgePlugin { enabled, address, token, read_env, max_requests_per_frame }` | the same settings in code |

By default the bridge binds to localhost only. If you bind it to another interface without a
token, it logs a warning. The bound address is printed in the log
(`AI bridge listening on 127.0.0.1:7878`) and stored in the `BridgeInfo` resource
(`resource.get {"name": "BridgeInfo"}`).

When the TCP server is off, the plugin still installs the request queue, so the in-process
`BridgeClient` still works.

## Protocol

There is one JSON object per line in each direction, and any number of clients can connect at
once. Answers carry the request's `id`, and they can arrive out of order: an `engine.step` waits
for frames, while a `bridge.ping` sent after it does not. A request without an `id` is a
notification: it runs, and no answer is sent. Batches (JSON arrays) are not supported.

```sh
ENGINE_BRIDGE=127.0.0.1:7878 cargo run -p engine_ai_bridge --example bridge_demo -- --headless
```

In another terminal, run `nc 127.0.0.1 7878` and type requests, one per line. To send a batch
and exit, use `-N` (OpenBSD netcat), which closes the connection after the last answer:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"bridge.ping"}' \
  '{"jsonrpc":"2.0","id":2,"method":"entity.spawn","params":{"name":"Crate","components":{"Transform":{"translation":[0,2,0]}}}}' \
  '{"jsonrpc":"2.0","id":3,"method":"engine.step","params":{"frames":3}}' \
  '{"jsonrpc":"2.0","id":4,"method":"entity.spwan"}' \
  | nc -N 127.0.0.1 7878
```

```text
{"id":1,"jsonrpc":"2.0","result":{"frame":11,"pong":true}}
{"id":2,"jsonrpc":"2.0","result":{"id":4294967249}}
{"error":{"code":-32601,"message":"unknown method 'entity.spwan'. Did you mean 'entity.spawn'? Call bridge.methods to list every method."},"id":4,"jsonrpc":"2.0"}
{"id":3,"jsonrpc":"2.0","result":{"elapsed_world_secs":2.03,"frame":14,"frames":3,"paused":false,"start_frame":11}}
```

### Bridge-level methods

| Method | Params | Result |
|---|---|---|
| `bridge.ping` | `{}` | `{pong: true, frame}`. The main thread answers it, so a reply proves frames are running. |
| `bridge.methods` | `{}` | `{count, methods: [{name, summary, args_example}]}` for every callable method |
| `engine.step` | `{frames: n}` (default 1) | sent after exactly `n` more frames: `{frames, start_frame, frame, paused, elapsed_world_secs}` |
| `engine.pause` | `{}` | pauses world time (`Time::pause`). Frames keep running and rendering. |
| `engine.resume` | `{}` | restores the speed that was set before the pause |
| `engine.status` | `{}` | `{frame, fps, frame_ms, paused, time_scale, speed, elapsed_world_secs, fixed_tick, entities, stepping_frames_remaining, bridge}` |
| `engine.exit` | `{code: 0}` | stops the game at the end of the frame |
| `auth` | `{token}` | unlocks this connection when a token is required (handled by the network thread) |

`engine.step` on a **paused** game resumes time for exactly `n` frames, then pauses again
before the next frame starts. This lets an agent advance a frozen world frame by frame, with
`render.screenshot` in between. On a running game, `engine.step` just waits for `n` frames.
If `engine.pause` arrives during a step, the pause takes effect when the step finishes.
`engine.resume` cancels that pending pause.

### Errors

Errors are JSON-RPC error objects. Each message is a sentence that says what to fix.

| Code | Meaning |
|---|---|
| -32700 | the line is not JSON |
| -32600 | not a request object (missing `method`, or a batch) |
| -32601 | unknown method (includes a "did you mean" suggestion) |
| -32602 | bad params for a bridge-level method (`engine.step`) |
| -32603 | the API function panicked. The game keeps running and the panic is logged. |
| -32000 | the API function returned an error (its own message) |
| -32001 | a token is required and was missing or wrong |
| -32002 | the game is shutting down |
| -32003 | the request was larger than 16 MiB, or the bridge already has 64 clients |

### Token

With `ENGINE_BRIDGE_TOKEN` set, the bridge rejects any request without the token with error
-32001. A client can include `"token": "..."` in each request object, next to `"method"`, or call
`{"method": "auth", "params": {"token": "..."}}` once per connection.

## Threading model

A background thread accepts connections. Each connection has a reader thread, which parses
lines, checks the token and queues requests over a channel, and a writer thread. The main thread
drains the queue in an exclusive system that runs in `First`, **before** `time_system`:

- Edits such as spawns, `input.press` and resource patches are visible to the whole frame:
  input systems, simulation, transform propagation and rendering. After a spawn and one step,
  a screenshot shows the result.
- Pause, resume and step change `Time` before the clock advances, so stepping is exact.

Finished steps get their answers from `Last`, after their frames have fully rendered. The main
thread never blocks on the network: answers go back over unbounded channels to the writer
threads. At most `max_requests_per_frame` requests (default 256) run per frame. Dropping the
`App` stops the server and closes every connection.

## In-process client

```rust
let client = engine_ai_bridge::BridgeClient::from_world(&app.world).unwrap();
// From another thread (it blocks until a frame answers):
let status = client.call("engine.status", serde_json::json!({}))?;
// On the thread that runs app.update(): queue, run frames, poll.
let pending = client.request("engine.step", serde_json::json!({"frames": 3}));
app.update_n(3);
assert!(pending.try_get().is_some());
```

## Tests

- `cargo test -p engine_ai_bridge` runs the unit tests for stepping, pause and resume, errors,
  panics and exit.
- `tests/tcp.rs` uses a real TCP client thread while the main thread runs frames. It covers
  ping, spawn, list, step, input, error codes, pipelining, two concurrent clients, token
  rejection and shutdown. It also spawns a red ball over TCP and checks that the next
  screenshot shows it; that test is skipped with a note when there is no GPU.
- `tests/launched.rs` checks that a game started by a launcher exits when its bridge cannot
  listen (it runs the game in a child process, because it sets environment variables).

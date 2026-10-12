# engine-mcp

An [MCP](https://modelcontextprotocol.io) server that lets AI assistants such as Claude Code
drive a game built on this engine. Through it, an assistant can inspect and edit entities,
step time, press inputs and look at screenshots. It talks to the game's AI bridge
([`engine_ai_bridge`](../../crates/ai_bridge)) over TCP. It talks to the MCP client over
**stdio**: JSON-RPC 2.0, one message per line on stdin/stdout, with logs and the game's output
on stderr. The protocol is implemented by hand on `serde_json`, without an MCP SDK.

## Tools

- One tool for every engine API that the game reports through `bridge.methods`. Dots become
  underscores, so `entity.spawn` becomes `entity_spawn`, `engine.describe` becomes
  `engine_describe`, and so on. Each tool's description is the API summary plus
  `Example args: ...`. Its input schema is `{"type": "object", "additionalProperties": true}`.
- `screenshot` renders the current frame. It returns the PNG as MCP image content (base64,
  `image/png`) plus a text item with the file path and size. Optional `path` keeps the file
  there. Otherwise it goes to a temp folder.
- `step` `{frames}` advances exactly N frames and returns when they have run. A paused game
  runs those frames, then pauses again.
- `call` `{method, params}` calls any API by its dotted name, including APIs registered after
  the tool list was built.

The server's `initialize` answer includes short usage instructions. A good first call is
`engine_describe`, which lists every component, resource and API, with docs and examples.

## Running it

```sh
engine-mcp --launch "cargo run -q -p hello -- --headless"   # start the game itself
engine-mcp --connect 127.0.0.1:7878                         # use a game that is already running
engine-mcp                                                  # same, address from $ENGINE_BRIDGE or 127.0.0.1:7878
```

`--launch` picks a free local port and starts the command through the shell with
`ENGINE_BRIDGE=127.0.0.1:<port>`. The game's stdout is redirected to stderr, so it cannot
corrupt the protocol. When the bridge answers `bridge.ping`, the server reports that the game is
up. The server answers `initialize` immediately. `tools/list` waits up to `--list-wait` seconds
(15 by default with `--launch`). If the game is still compiling, `tools/list` returns the three
built-in tools first. The server then sends `notifications/tools/list_changed` once the game is
up. Tool calls wait for a starting game, up to `--startup-timeout`, which defaults to 600 s
because first builds are slow. When the client closes stdin, the server stops the game. The
game is launched with `ENGINE_BRIDGE_EXIT_ON_STDIN_EOF=1`, so it also exits by itself if the
server is killed.

Other flags: `--token` (or `ENGINE_BRIDGE_TOKEN`; it is passed to a launched game too),
`--call-timeout S`, `--list-wait S`, `--help`.

> **The game must add `engine_ai_bridge::AiBridgePlugin`.** It does nothing unless
> `ENGINE_BRIDGE` is set, so it is safe to leave in. Until `DefaultPlugins` includes it, add it
> to your game yourself. To try the setup right away, use the demo game:
> `--launch "cargo run -q -p engine_ai_bridge --example bridge_demo -- --headless"`.

Build once before the first launch so the client does not wait on a cold compile:
`cargo build -p engine-mcp -p hello`.

## Add it to Claude Code

Put a `.mcp.json` in the workspace root:

```json
{
  "mcpServers": {
    "engine": {
      "command": "cargo",
      "args": ["run", "-q", "-p", "engine-mcp", "--", "--launch", "cargo run -q -p hello -- --headless"]
    }
  }
}
```

You can also register it from the command line:
`claude mcp add engine -- cargo run -q -p engine-mcp -- --launch "cargo run -q -p hello -- --headless"`.

To attach to a game you run yourself, for example windowed with `ENGINE_BRIDGE=1 cargo run -p
hello`, use `"args": ["run", "-q", "-p", "engine-mcp", "--", "--connect", "127.0.0.1:7878"]`.

### Other MCP clients

Any client that starts stdio servers works. Use the same `command` and `args`. Clients
configure this under different keys (`mcpServers` in Claude Desktop, Cursor and others). Start
the client from the workspace directory, or run a prebuilt binary
(`target/debug/engine-mcp`) and pass `--launch` a command with an absolute
`--manifest-path`. Logs go to stderr, which most clients show in their MCP log view.

## Testing

`cargo test -p engine-mcp` runs the tests without starting a game. A fake bridge (a small
`TcpListener` in the test) answers `bridge.methods`, `entity.spawn`, `engine.step` and
`render.screenshot`. The tests check `initialize`, the `tools/list` mapping, `tools/call`
round trips, image content, the built-in-tools fallback with `list_changed`, and process
launch and shutdown.

To check the whole path by hand, start the server with `--launch` on the demo game and send:

```text
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"me","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"engine_status","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"screenshot","arguments":{}}}
```

# Engine Luau debugger for VS Code

Debugs the game's Luau scripts (`assets/scripts`) while the game runs: breakpoints with
conditions and hit counts, logpoints, step in, over and out, the call stack, locals, upvalues and
`self` (expandable), watches, hovers, and the debug console evaluating in the stopped frame.
The game listens; this extension only tells VS Code where (`crates/script/src/dap.rs`).

## Set up (once)

1. Install the **Luau Language Server** extension (`JohnnyMorganz.luau-lsp`), which gives `.luau`
   files their language (see `assets/scripts/README.md` for autocompletion).
2. In VS Code run **Developer: Install Extension from Location...** and pick this folder
   (`crates/script/vscode`). Or package it with `npx @vscode/vsce package` and install the
   `.vsix` with `code --install-extension engine-luau-debug-0.1.0.vsix`.
3. Add to `.vscode/launch.json`:

   ```json
   {
     "version": "0.2.0",
     "configurations": [
       { "type": "engine-luau", "request": "attach", "name": "Attach to the game's scripts", "port": 4711 }
     ]
   }
   ```

## Use

1. Start the game with the debugger listening:
   - Windows (PowerShell): `$env:ENGINE_SCRIPT_DEBUG=4711; .\showcase.exe`
   - Linux/macOS: `ENGINE_SCRIPT_DEBUG=4711 ./showcase`
   - or while it runs: `script.debug {"port": 4711}` from the console or the AI bridge.
2. Open the Engine folder in VS Code, set breakpoints in `assets/scripts/**/*.luau`, and start
   **Attach to the game's scripts** (F5).

While stopped the game stands still (it continues when you do), like a native debugger in Unity
or Unreal. Logpoints never stop. Breakpoints follow hot reloads: save the file and they move onto
the new code. Script `print`s and errors appear in the debug console.

## Other editors

Any Debug Adapter Protocol client can attach to the same port. Neovim (nvim-dap):

```lua
local dap = require("dap")
dap.adapters["engine-luau"] = { type = "server", host = "127.0.0.1", port = 4711 }
dap.configurations.luau = {
  { type = "engine-luau", request = "attach", name = "Attach to the game's scripts" },
}
```

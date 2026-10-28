# Scripts

Gameplay scripts in [Luau](https://luau.org), run by `engine_script` (crate docs:
`crates/script/src/lib.rs`; design and research: `engine-design/scripting-research.md`).
A `Script { path, props }` component runs a file from this folder on its entity; edit the file
while the game runs and it reloads within a quarter second, keeping the script's state.

## A script

```lua
--!strict
local Lantern = {}
Lantern.props = { color = vec3(1, 0.6, 0.3), period = 2 } -- defaults; Script.props override

function Lantern:start() -- once; may task.wait
	self.t = 0
end

function Lantern:update(dt: number) -- every frame, world seconds (slow motion aware)
	self.t += dt
	local glow = 0.6 + 0.4 * math.sin(self.t * 2 * math.pi / self.props.period)
	self.entity:set("PointLight", { color = self.props.color, intensity_lm = 900 * glow })
end

function Lantern:on_interact(by: Entity) -- the player pressed E on it; may task.wait
	vfx.spawn({ effect = "spell.fireball_impact", position = self.entity.world_position })
end

return Lantern
```

Every engine function is there as `module.verb(args)` (`fire.ignite`, `weather.set`,
`npc.spawn`, ...), plus `world.find/spawn`, entity handles, `vec3`/`quat`, the `task` library
(`wait`, `spawn`, `delay`, `defer`, `cancel`, `tween`, `wait_until`), `self:every`,
`self:after`, `ease.*`, `input.pressed`, and `require("lib/name")` for shared modules.

## Editor setup (autocomplete and type checking)

1. Install the **Luau Language Server** extension (`JohnnyMorganz.luau-lsp`) in VS Code.
2. Write the engine's type definitions from the running game, in the console or through the AI
   bridge: `script.types` (it writes `assets/scripts/engine.d.luau`, which git ignores).
3. Add to `.vscode/settings.json`:

   ```json
   {
     "luau-lsp.types.definitionFiles": ["assets/scripts/engine.d.luau"],
     "luau-lsp.platform.type": "standard",
     "luau-lsp.sourcemap.enabled": false
   }
   ```

## Live tools

`script.eval {code}` runs Luau in the live game (`return world.find("Lantern 3").position`),
`script.errors` and `script.log` show what scripts reported, `script.list` shows every file and
instance with its cost per frame, `script.edit` and `script.write` change files like an editor,
and `script.restart` starts an entity's script from scratch.

## Debugging

Start the game with `ENGINE_SCRIPT_DEBUG=4711` (or call `script.debug` while it runs) and attach
VS Code with the extension in `crates/script/vscode` (its README has the setup) or any other
Debug Adapter Protocol client. Breakpoints with conditions, hit counts and logpoints, stepping,
the call stack, variables, watches and the debug console all work in these files, and follow hot
reloads. "Uncaught errors" in VS Code's breakpoint list (on by default) stops where an error
nothing catches is raised, with the stack and locals still there. Without an editor,
`script.logpoint {path, line, message}` prints values from a line without stopping
(`"message": "glow = {glow}"`), and code boards show them beside the line. A code board also
marks the line the last error came from, with the message, until the file is fixed.

## Profiling

`script.profile {"seconds": 5}` records where the scripts spend their time while the game runs;
`script.profile {}` lists the functions and lines that took the most, with engine calls such as
`e:set` or `fire.ignite` as entries of their own. Code boards show each line's share beside it,
and a `ProfileBoard` draws the flame graph (the Script courtyard has one recording live).

## In this folder

- `showcase/courtyard.luau`: the Script courtyard's director (lanterns, orrery, the ritual).
- `showcase/lever.luau`: a lever that sends an event to another entity's script.

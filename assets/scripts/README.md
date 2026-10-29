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

## Many scripts at once: `parallel_update`

For hundreds of scripts that each move their own entity (a swarm, a crowd, flocks), use
`parallel_update(dt)` instead of `update`: those scripts run in actor VMs side by side, one per
core. In it a script reads the world as the frame's parallel part began, sees its own moves at
once, and its other changes (`e:set`, `e.name =`, `e:material`, `e:despawn`) apply right after.
An engine call that needs the whole world (`fire.ignite`, spawning) needs `task.synchronize()`
first; the rest of the function then runs after the parallel part, one script at a time.

```lua
function Wisp:parallel_update(dt: number)
	local at = self.entity.position
	self.velocity = self.velocity * (1 - 2.5 * dt) + (self:orbit_point() - at) * (6 * dt)
	self.entity.position = at + self.velocity * dt
	if self.entity.position.y > 30 then
		task.synchronize()
		vfx.spawn({ effect = "spell.starburst", position = self.entity.world_position })
	end
end
```

Each VM has its own globals and modules: talk to a script in another VM with `e:send`, or give
scripts the same `ScriptActor` number to keep them in one VM. The showcase swarm:
`showcase/wisp.luau`.

## In the editor

The editor's **Scripts** tab (beside Scene) edits these files: the folder with each file's
state (green: running, red: an error), colored code with line numbers, and the file's functions
to jump to. A syntax error is marked on its line as you type, before you save, and a call to an
engine function that doesn't exist is underlined with the nearest name. Completion (as you
type, or Ctrl+Space) knows every engine function with its summary and example arguments,
`vec3`, entities, `task`, `self`, hooks and component names. Scripts run when you press Play,
and every Play starts them afresh; Ctrl+S then hot reloads them with their state. While playing,
click a line's number to see its values live, and Profile shows each line's share of the script
time. The Output under the code shows prints and errors (click one to open its line), and its
command bar runs Luau in the game. With an entity selected, open its script, or run the open
file on it.

## Visual scripts (`.graph`)

A `.graph` file is a script drawn as nodes and wires, for people who would rather not write
code. Put it on an entity like a Luau file (`Script { path: "examples/pulse.graph" }`): the
engine compiles it to Luau and runs that, so hot reload, errors, the profiler and the debugger
work the same. Edit graphs in the editor's **Graphs** tab:

- Right-click (or Space) for the node search; drag from a pin into empty space to search only
  for nodes that can take that wire. Events are red, execution wires white, data wires colored
  by type.
- Variables (the script's own state) and props (settings each entity can override) are listed
  on the left; Get and Set put their nodes on the canvas.
- Mistakes show before you run: on the node, in Problems, and on the line of the Luau panel,
  which shows the code the graph runs as. A graph with mistakes doesn't load; the old one keeps
  running.
- While playing, the nodes that run light up, pulses run along their execution wires and every
  data wire shows its value. Ctrl+S hot reloads the graph in the running game.
- **To script** writes the graph's Luau as a `.luau` file, for when a graph outgrows nodes.

The file is JSON text with one node or wire per line, so changes diff and merge in git like
code. Every engine function is a node (`fire.ignite`, `weather.set`, ...), and every
component has Get and Set nodes.

## VS Code setup (autocomplete and type checking)

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
Debug Adapter Protocol client (to try it without starting the game:
`cargo run -p engine_script --example headless -- examples/float.luau`). Breakpoints with conditions, hit counts and logpoints, stepping,
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

What things cost (release build, one core; `math_speed` in the crate's tests): a `vec3` field
read is a plain table read and `a + b` a short Luau function, so vector math stays in the VM.
Engine calls (`e:set`, `world.find`, `e.position`) cross into Rust and cost more: read
`self.entity.position` once into a local, not once per use. For heavy number crunching on
directions or colors where `f32` is enough, Luau's own `vector` is faster still
(`vector.create(x, y, z)`, `vector.magnitude(v)`); `vec3(v)` turns one into a `vec3`. The
built-in libraries (`math`, `string`, `table`, `vector`) take Luau's fast path; `getfenv`,
`setfenv` and `loadstring` would switch it off (and reach into other scripts), so scripts don't
have them.

## In this folder

- `showcase/courtyard.luau`: the Script courtyard's director (lanterns, orrery, the ritual).
- `showcase/lever.luau`: a lever that sends an event to another entity's script.
- `showcase/wisp.luau`: one of the 320 wisps round the orrery (`parallel_update`, on every core).
- `examples/lamp.luau`: a lamp that breathes and flickers (the editor's starter courtyard has a
  lamp to put it on).
- `examples/float.luau`: a stone that floats and turns, with a curve from a module
  (`examples/lib/wave.luau`).
- `examples/pulse.graph`, `examples/hover.graph`: the same lamp and stone as visual scripts.
- `showcase/crystal.graph`: the courtyard's graph crystal (spins, changes color every 1.5 s,
  flares when used), drawn live on the board beside it.

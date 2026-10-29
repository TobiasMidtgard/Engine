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

Scripts are type checked as you type by Luau's own checker (the one Roblox Studio and luau-lsp
use; scripts here start with `--!strict`). A type error is underlined in red and a lint warning
(an unused local, a shadowed name) in yellow, with the message on hover and at the line's end;
**Problems** beside Output lists them all (click one to open it), the file list shows a ✖ count
and the status line says "✔ types" when a file is clean. Hover over a name to see its type,
`.` and `:` complete the members of that value's real type (`self.props.` lists the props), a
call shows its parameters with the current one in bold, and F12 or Ctrl+click goes to where a
name is defined. The checker knows what `self` is in a class script: the engine's part
(`entity`, `every`, `after`, `spawn`, the `ScriptBase` type), the class's methods and `props`,
and the fields its methods set, typed from what they are set to.

## Props in the Details panel

Select an entity with a script and the Details panel shows each of its props as a field, as
Unity's inspector shows a component's fields: numbers, sliders, colors, vectors, angles,
toggles, text, a list to choose from, a picker of the scene's entities, and tables that fold
out. The comment after a prop is its tooltip, and hints in it shape the field:

```lua
Beacon.props = {
	-- @group Light
	lumens = 2400, -- at the top of a pulse @range 0 8000 @unit lm
	color = vec3(0.35, 0.75, 1), -- linear RGB
	-- @group Signal
	mode = "pulse", -- how it shines @enum steady|pulse|blink
	target = "Stone", -- who hears it flash @entity
	every = 4, -- flashes between messages @int @min 1
}
```

| Hint | Field |
| --- | --- |
| `@range min max [step]` | a slider; values stay inside |
| `@min v`, `@max v`, `@step v` | a number kept above or below `v`, or moving in steps |
| `@int` | a whole number |
| `@unit text` | shown after the number (`lm`, `m`, `s`) |
| `@angle` | radians, shown in degrees |
| `@color` | a `vec3` as a color (props named `color`, `colour` or `tint` are colors anyway) |
| `@enum a\|b\|c` | a string chosen from a list |
| `@entity` | a string naming an entity, picked from the scene (`world.find(self.props.target)`) |
| `@hide` | not shown |
| `@group Name` | on a comment line of its own: the props below it fold under `Name` |

A value set in the panel belongs to that entity (its name turns bold blue, with a bar, as
Unity marks an override); ↺ or right-click puts the script's default back. While playing, a
change reaches the running script at once (`on_props()` is called), the script's own values
(`self.t`, ...) show live under its props, and **Keep after Stop** keeps what you tuned when
Stop puts the scene back. `examples/beacon.luau` uses every kind of field.

## Signals: wiring scripts together in the Details panel

A script says what it tells the world in a `signals` table, and fires one with `self:emit`:

```lua
Lever.signals = {
	pulled = "by: Entity", -- the arm went over; who pulled it
	reset = "", -- it can be pulled again
}

function Lever:on_interact(by: Entity)
	-- ... swing the arm ...
	self:emit("pulled", by)
end
```

Who hears it is set per object in the editor, like Godot's signal connections and Unity's
events: select the lever, and under **Signals** in its Script component press **+ Connect**,
pick a scene object and one of its script's methods (or **+ New method**, which writes
`function Door:on_pulled(by)` into that script and opens it). A connection can pass fixed
values instead of the signal's own (`2, "open"`) and fire only once. The object on the other
end lists it under **Heard from**, and its script shows a bolt beside the method; hover it to
see who calls it. One lever script then serves every lever: each one is wired to its own door.

Code can listen too: `lever:on("pulled", function(by) ... end)` (every time) and
`lever:once(...)` (the next time) return a connection with `disconnect()`, and
`task.wait_for(lever, "pulled")` waits for one and returns its values. Listeners run later in
the same frame, each as a task of its own, so they may wait and one that fails doesn't stop the
others. Emitting a signal the class doesn't declare is an error, and the type checker knows each
signal's values: `self:emit("pulled", 3)` is underlined as you type.

## State machines: a `states` table

Most gameplay objects are a few states: a gate is closed, opening, open or closing. Give the
class a `states` table and the engine runs it; the state's own work goes in methods named after
it:

```lua
Gate.states = {
	initial = "closed",
	states = {
		closed = { on = { interact = "opening" } },             -- E on it
		opening = { done = "open" },                            -- when enter_opening returns
		open = { after = { "hold", "closing" } },               -- a prop: seconds
		closing = { done = "closed", when = { blocked = "opening" } }, -- checked every frame
	},
}

function Gate:enter_opening()
	self:move_to(1) -- may wait (a tween): `done` follows when it returns
end
```

- Per state: `on = { event = "target" }` (or `{ to = "target", guard = "method" }`, or a list
  of those: the first whose guard holds), `after = { seconds or a prop's name, "target" }`,
  `done = "target"` and `when = { method_or_field = "target" }` (`"not blocked"` too).
  Transitions on the outer table fire from any state.
- `enter_<state>()`, `update_<state>(dt)` and `exit_<state>()` run as it starts, every frame
  and as it ends; leaving a state stops its enter work where it is. A state with `states` of
  its own is a group: its `initial` starts first, its hooks and transitions hold for all of
  them, and `history = true` resumes the child it was in.
- Events come in by themselves: E on the object is `interact`, a message or a wired signal of
  that name is that event. From code: `self:trigger("power")`, `self:go("open")`,
  `self:in_state("powered")`, `self:state_time()` and `self.state` (the innermost state).
- The type checker knows the names: `self:go("openning")` is underlined. A transition to a
  state that isn't there is an error on its line; a loop of `done` or `when` changes stops
  after 32 in a frame and says which. Hot reload keeps the state it is in.
- In the editor the **States** button beside a script draws its machine: while playing the
  current state is green with its time, `after` timers fill up, the transition just taken
  glows, and the log below says each change and why ("interact by Player"). Click a state or
  an arrow to go to its line. The Details panel shows the same for the selected object.
- Two things at once: a group with `parallel = true` runs all its states side by side
  (regions, usually groups of their own), as a turret's aim and its gun. Every region takes
  each event; a change inside one leaves the others where they are; leaving the group leaves
  them all. It has no `initial`. `self:in_state` asks about any region's state.

```lua
Turret.states = {
	initial = "active",
	states = {
		active = {
			parallel = true,
			on = { power = "off" },
			states = {
				aim = { initial = "scanning", states = {
					scanning = { when = { sees = "tracking" } },
					tracking = { when = { ["not sees"] = "scanning" } },
				} },
				gun = { initial = "ready", states = {
					ready = { when = { locked = "firing" } },
					firing = { done = "cooling" },
					cooling = { after = { "cooldown", "ready" } },
				} },
			},
		},
		off = { on = { power = "active" } },
	},
}
```

- Pausing on a state: right-click a state in the diagram, "Pause the game when it starts" (a
  red dot) or "when it ends" (a red ring). Play stops there, the object selected and the panel
  saying what got it there; Play goes on. Click a change in the log to see the machine as it
  was right after it (Earlier, Later, Live).
- In the world: a `StateBoard { target = "Gate" }` on a quad draws that object's machine live,
  as the editor does (the Script courtyard's portcullis has one).

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
- **Signals** and **Events** are listed there too, as Unreal's event dispatchers and custom
  events. A signal (its values typed as `count: number, by: Entity`) is fired by an **Emit**
  node and wired per object in the Details panel like a Luau script's. An event is a node of
  its own with its values as outputs: what a wired signal, a **Bind signal to** node (another
  object's signal, from now on or once), a **Call** node or a message of its name runs.
- Mistakes show before you run: on the node, in Problems, and on the line of the Luau panel,
  which shows the code the graph runs as. A graph with mistakes doesn't load; the old one keeps
  running. Luau's type checker reads the code too, and its errors (a value of the wrong type
  into a call, say) show on the node they come from.
- While playing, the nodes that run light up, pulses run along their execution wires and every
  data wire shows its value. Ctrl+S hot reloads the graph in the running game.
- **States** (in the toolbar) shows the graph's state machine instead of its nodes: add states,
  drag an arrow from one to another, and set its event, timer or condition in the sidebar; each
  state gets Enter, Update and Exit nodes, and **Go to** and **In?** nodes for the graph. It runs
  as a Luau script's `states` table does, and lights up the same while playing.
- **To script** writes the graph's Luau as a `.luau` file, for when a graph outgrows nodes.

The file is JSON text with one node or wire per line, so changes diff and merge in git like
code. Every engine function is a node (`fire.ignite`, `weather.set`, ...), and every
component has Get and Set nodes.

## Tests (`*.test.luau`)

A test file says what your scripts should do, and the engine checks it in the running game. It
reads like Jest (or Roblox's Jest Lua and TestEZ): `describe` groups tests, `it` is one test,
`expect` checks a value.

```lua
describe("Lamp", function()
	it("goes out when pressed", function(t)
		local lamp = t:spawn({ name = "Test lamp", components = {
			Transform = {}, PointLight = {}, MeshRenderer = {},
			Script = { path = "examples/lamp.luau" } } })
		t:press(lamp)                         -- E, as the player
		expect(lamp.script.lit).to_be(false)  -- its script's own state
	end)
end)
```

- Each test runs as a task in the game, one after another, so it can wait like gameplay does:
  `t:wait(seconds)`, `t:frames(n)`, `t:wait_until(check)`, `t:wait_for(entity, "signal")`.
- `t:spawn(spec)` makes an entity for this test only (with a Script component it waits until
  the script runs); it is gone after the test. `t:find(name)` finds one in the scene,
  `t:press(entity)` presses E on it, `t:listen(entity, "signal")` counts its signals.
- Checks: `to_be`, `to_equal` (tables all the way down), `to_be_near(x, within)` (numbers and
  vectors), `to_be_greater_than`, `to_be_less_than`, `to_be_at_least`, `to_be_at_most`,
  `to_be_truthy`, `to_be_falsy`, `to_be_nil`, `to_be_a("number")`, `to_contain`,
  `to_have_length`, `to_throw`; `expect(x).never.to_be_nil()` turns any of them around.
- `before_each` and `after_each` run around each test of their group; `it.skip` and
  `describe.skip` leave tests out, `it.only` runs only those. A test longer than 10 seconds of
  game time fails (`it(name, function(t) ... end, 30)` gives it longer).
- In the editor's Scripts tab, the **Tests** panel (beside Output and Problems) runs them all
  or the open file, presses Play for the run and Stop after, and lists every test with its
  time; a failed one says why and on which line (click it). In a test file, the mark left of
  each `it` and `describe` runs just that one, then shows ✔ or ✖.
- `script.test {}` runs them from the console or the AI bridge (`{"files": [...]}`, `{"file":
  ..., "line": 12}`, `{"filter": "lamp"}`, `{"stop": true}`); `script.tests` returns the
  results. The examples' tests also run in the engine's own test suite.

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
and `script.restart` starts an entity's script from scratch. `script.typecheck {path?, text?}`
type checks one file (or text as that file), or every script, and returns the problems.

## Debugging

In the editor's Scripts tab, click left of a line number for a breakpoint (right-click for one
that stops only when a condition holds; F9 on the caret's line). Press Play: when a script gets
there it waits on that line, the other scripts wait with it and the world's time stands still,
but the editor keeps running. The line gets a yellow arrow, each local's value shows beside the
line that set it, hovering a name shows its value, and the panel on the right has the call
stack (click a frame for its line and values), the variables (tables open up) and your watches.
F5 goes on, F10 steps over, F11 in, Shift+F11 out; ⏸ Break stops at the next line any script
runs, and Stop on errors stops where an error is raised. In the Graphs tab, F9 or a node's
right-click menu puts a breakpoint on a node, and Next node (F10) runs on to the next one.
`script.break {"path": ..., "lines": [31]}` does the same from the API or the AI bridge.

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
- `examples/beacon.luau`: a light that pulses, turns and signals another entity; its props
  show every kind of field in the Details panel.
- `examples/float.luau`: a stone that floats and turns, with a curve from a module
  (`examples/lib/wave.luau`).
- `examples/gate.luau`: a portcullis whose logic is all in its `states` table (E raises it, it
  closes after `hold` seconds, goes back up if someone is under it, and stops on a power cut);
  `examples/gate.test.luau` tests each of those.
- `examples/turret.luau`: a sentry turret with two regions at once (`parallel = true`): its
  aim sweeps or follows whoever comes near while its gun fires and cools down; power off and
  on starts both again. `examples/turret.test.luau` tests them.
- `examples/pulse.graph`, `examples/hover.graph`: the same lamp and stone as visual scripts.
- `examples/lamp.test.luau`, `examples/beacon.test.luau`, `examples/float.test.luau`: tests of
  the examples (and of the `wave` module), run in the game.
- `showcase/crystal.graph`: the courtyard's graph crystal (spins, changes color every 1.5 s,
  flares when used or when its `flare` event runs, then fires `flared`), drawn live on the
  board beside it; the lever's `pulled` signal is wired to its `flare`.

--!nonstrict
-- State machines for scripts: a class's `states` table, run by the engine (Harel statecharts,
-- as Unreal's StateTree, Unity's state graphs, PlayMaker and Godot's LimboHSM).
--
--   Gate.states = {
--       initial = "closed",
--       on = { power_cut = "dark" },             -- the outer table's transitions work in every state
--       states = {
--           closed = { on = { interact = "opening" } },
--           opening = { done = "open" },         -- when enter_opening has finished
--           open = { after = { 4, "closing" }, when = { blocked = "opening" } },
--           closing = { done = "closed" },
--           dark = { on = { power_back = "closed" } },
--       },
--   }
--
-- A state runs `enter_<name>` as a task when it starts (it may wait: `done` follows when it
-- returns), `update_<name>(dt)` every frame and `exit_<name>` when it ends; the state's table
-- can hold `enter`, `update` and `exit` functions instead. A state with `states` inside is a
-- group: entering it enters its `initial` one (or the one it was last in, with `history = true`).
-- Triggers: `on = { event = "to" }` (interactions are `interact`; messages, wired signals and
-- `self:trigger(event)` use their names), `after = { seconds, "to" }`, `done = "to"` and
-- `when = { condition = "to" }` (a method or field of the script, `"not lit"` too, checked every
-- frame). Events go from the innermost state out; the first that knows the event takes it. An
-- `on` entry may be `{ to = "x", guard = "condition" }` or a list of those, the first whose guard
-- holds wins. Runs as part of the script: the runtime calls `__states_start` after `start`,
-- `__states_tick` after `update`, `__states_event` for each event and `__states_reload` after a
-- hot reload. engine-design/scripting-research.md §19.

local Base = __script_base
local spawn_as = __task_spawn_as

local MAX_CHANGES = 32
local LOG_LENGTH = 64
local KEYS = {
	initial = true,
	history = true,
	on = true,
	after = true,
	done = true,
	when = true,
	states = true,
	enter = true,
	exit = true,
	update = true,
	doc = true,
}

local function sorted_keys(t: { [any]: any }): { string }
	local out = {}
	for k in t do
		if type(k) == "string" then
			table.insert(out, k)
		end
	end
	table.sort(out)
	return out
end

local function list(names: { string }): string
	return table.concat(names, ", ")
end

-- An `on` entry's choices: { { to, guard } }.
local function choices(v: any): { { to: any, guard: any } }?
	if type(v) == "string" then
		return { { to = v } }
	end
	if type(v) ~= "table" then
		return nil
	end
	if v.to ~= nil then
		return { { to = v.to, guard = v.guard } }
	end
	local out = {}
	for _, c in ipairs(v) do
		if type(c) == "string" then
			table.insert(out, { to = c })
		elseif type(c) == "table" and c.to ~= nil then
			table.insert(out, { to = c.to, guard = c.guard })
		else
			return nil
		end
	end
	return if #out > 0 then out else nil
end

-- Compiles a `states` table into nodes. Returns the machine, or nil, the problem and the name
-- or value it is about (the editor finds its line by it).
local function compile(def: any): (any, string?, string?)
	if type(def) ~= "table" then
		return nil, "states must be a table", nil
	end
	local nodes = {}
	local order = {}
	local function walk(name: string, d: any, parent: any, depth: number): (string?, string?)
		local node = {
			name = name,
			parent = parent,
			depth = depth,
			def = d,
			children = {},
			initial = nil,
		}
		nodes[name] = node
		table.insert(order, node)
		for _, k in sorted_keys(d) do
			if not KEYS[k] then
				local keys = sorted_keys(KEYS)
				return `state {name}: unknown key "{k}" (a state has {list(keys)}; states inside it go in its states table)`,
					k
			end
		end
		local kids = d.states
		if kids == nil then
			return nil
		end
		if type(kids) ~= "table" then
			return `state {name}: states must be a table of states`, name
		end
		local names = sorted_keys(kids)
		for _, child in names do
			local cd = kids[child]
			if type(cd) ~= "table" then
				return "state " .. child .. ": needs a table (" .. child .. " = {} is a state that does nothing)", child
			end
			if child == "root" then
				return `"root" is the name of the outer state; call this one something else`, child
			end
			if nodes[child] then
				return `there are two states named {child}: names must differ, even in different groups`,
					child
			end
			table.insert(node.children, child)
			local problem, about = walk(child, cd, node, depth + 1)
			if problem then
				return problem, about
			end
		end
		if #names == 0 then
			return nil
		end
		local first = d.initial
		if first == nil then
			if #names > 1 then
				local what = if name == "root" then "states" else `state {name}`
				return `{what} has {#names} states inside: say which comes first (initial = "{names[1]}")`,
					names[1]
			end
			first = names[1]
		end
		if type(first) ~= "string" or kids[first] == nil then
			local what = if name == "root" then "states" else `state {name}`
			return `{what}: initial "{tostring(first)}" isn't one of its states ({list(names)})`,
				tostring(first)
		end
		node.initial = first
		return nil
	end
	local problem, about = walk("root", def, nil, 0)
	if problem then
		return nil, problem, about
	end
	if #nodes.root.children == 0 then
		return nil, "states has no states (states = { closed = { ... }, ... })", nil
	end
	local all = sorted_keys(nodes)
	local events = {}
	local function target(from: string, to: any, what: string): (string?, string?)
		if type(to) ~= "string" or nodes[to] == nil or to == "root" then
			local names = {}
			for _, n in all do
				if n ~= "root" then
					table.insert(names, n)
				end
			end
			return `state {from}: {what} goes to "{tostring(to)}", which isn't a state ({list(names)})`,
				tostring(to)
		end
		return nil
	end
	for _, node in order do
		local d = node.def
		local name = node.name
		if d.on ~= nil then
			if type(d.on) ~= "table" then
				return nil, `state {name}: on must be a table of event = "state"`, name
			end
			for _, event in sorted_keys(d.on) do
				local cs = choices(d.on[event])
				if not cs then
					return nil,
						"state " .. name .. ": on." .. event .. ' must be a state\'s name, { to = "state", guard = "condition" } or a list of those',
						event
				end
				for _, c in cs do
					local p, about = target(name, c.to, `on {event}`)
					if p then
						return nil, p, about
					end
				end
				events[event] = true
			end
		end
		if d.after ~= nil then
			local a = d.after
			if type(a) ~= "table" or (type(a[1]) ~= "number" and type(a[1]) ~= "string") then
				return nil,
					"state " .. name .. ': after must be { seconds, "state" } (seconds a number or a prop\'s name)',
					name
			end
			local p, about = target(name, a[2], `after {a[1]} s`)
			if p then
				return nil, p, about
			end
		end
		if d.done ~= nil then
			local p, about = target(name, d.done, "done")
			if p then
				return nil, p, about
			end
		end
		if d.when ~= nil then
			if type(d.when) ~= "table" then
				return nil, `state {name}: when must be a table of condition = "state"`, name
			end
			for _, cond in sorted_keys(d.when) do
				local p, about = target(name, d.when[cond], `when {cond}`)
				if p then
					return nil, p, about
				end
			end
		end
		for _, hook in { "enter", "exit", "update" } do
			local h = d[hook]
			if h ~= nil and type(h) ~= "function" and type(h) ~= "string" then
				return nil, `state {name}: {hook} must be a function or a method's name`, name
			end
		end
	end
	return { nodes = nodes, order = order, root = nodes.root, events = events, def = def }
end

-- Compiled machines by `states` table (a reload brings a new table).
local compiled = setmetatable({}, { __mode = "k" })

local function machine_of(class: any): (any, string?, string?)
	local def = rawget(class, "states")
	local m = compiled[def]
	if m then
		return m
	end
	local problem, about
	m, problem, about = compile(def)
	if m then
		compiled[def] = m
	end
	return m, problem, about
end

-- The state's hook: its table's function, else the class's `<hook>_<name>` method.
local function hook_of(self: any, node: any, hook: string): any
	local h = node.def[hook]
	if type(h) == "string" then
		h = self[h]
	end
	if h == nil and node.name ~= "root" then
		h = self[`{hook}_{node.name}`]
	end
	return if type(h) == "function" then h else nil
end

-- `after`'s seconds: a number, or a prop's (or field's) name so the Details panel tunes it.
local function seconds(self: any, secs: any): number
	if type(secs) == "string" then
		local props = rawget(self, "props")
		local v = if type(props) == "table" and props[secs] ~= nil then props[secs] else self[secs]
		return if type(v) == "number" then v else math.huge
	end
	return secs
end

-- Whether a condition holds: a method's result, a field's value, `"not x"` the opposite.
local function holds(self: any, cond: any, ...: any): boolean
	if type(cond) == "function" then
		return cond(self, ...) and true or false
	end
	if type(cond) ~= "string" then
		return cond and true or false
	end
	local negate = false
	local name = cond
	if string.sub(name, 1, 4) == "not " then
		negate = true
		name = string.sub(name, 5)
	end
	local v = self[name]
	if type(v) == "function" then
		v = v(self, ...)
	end
	local yes = v and true or false
	return if negate then not yes else yes
end

local function log(run: any, from: string, to: string, why: string)
	table.insert(run.log, { t = run.clock, from = from, to = to, why = why })
	if #run.log > LOG_LENGTH then
		table.remove(run.log, 1)
	end
end

local settle

-- Enters `node`: its time starts, its enter hook starts as a task.
local function enter(self: any, run: any, node: any)
	table.insert(run.active, node)
	run.since[node.name] = run.clock
	run.visit[node.name] = run.serial
	run.finished[node.name] = nil
	local h = hook_of(self, node, "enter")
	if not h then
		run.finished[node.name] = true
		return
	end
	local visit = run.serial
	local name = node.name
	local t
	t = spawn_as(self.entity, function()
		h(self)
		if run.visit[name] == visit and run.running[name] == t then
			run.running[name] = nil
			run.finished[name] = true
			if not run.changing then
				settle(self, run)
			end
		end
	end)
	if coroutine.status(t) ~= "dead" then
		run.running[name] = t
	else
		-- Finished without waiting (its code above ran before `t` was set).
		run.finished[name] = true
	end
end

-- Leaves the innermost active state.
local function exit(self: any, run: any)
	local node = table.remove(run.active)
	local name = node.name
	local t = run.running[name]
	if t then
		run.running[name] = nil
		task.cancel(t)
	end
	run.finished[name] = nil
	run.visit[name] = nil
	local parent = node.parent
	if parent then
		run.last[parent.name] = name
	end
	local h = hook_of(self, node, "exit")
	if h then
		h(self)
	end
end

local function leaf(run: any): any
	return run.active[#run.active]
end

-- Enters `node`'s inner states down to a leaf: `initial`, or the last one with `history`
-- (unless `fresh`).
local function descend(self: any, run: any, node: any, fresh: boolean?)
	while #node.children > 0 do
		local d = node.def
		local next_name = node.initial
		local last = run.last[node.name]
		if not fresh and d.history and last and run.m.nodes[last] and run.m.nodes[last].parent == node then
			next_name = last
		end
		node = run.m.nodes[next_name]
		enter(self, run, node)
	end
end

local function is_active(run: any, name: string): boolean
	for _, n in run.active do
		if n.name == name then
			return true
		end
	end
	return false
end

-- Goes from `source` (an active state, the one whose transition fires) to the state named
-- `to`: leaves the states below the nearest group holding both, enters down to `to` and on
-- to a leaf.
local function change(self: any, run: any, source: any, to: string, why: string)
	run.changes += 1
	local from = leaf(run).name
	if run.changes > MAX_CHANGES then
		local path = {}
		for k = math.max(1, #run.log - 6), #run.log do
			table.insert(path, run.log[k].to)
		end
		error(
			`states: more than {MAX_CHANGES} changes in one frame ({table.concat(path, " → ")} → {to}): a loop of done or when transitions with nothing to wait for`,
			0
		)
	end
	local target = run.m.nodes[to]
	-- The nearest group holding both (never the source or the target themselves: going to a
	-- state you are in leaves it and enters it again).
	local ancestors = {}
	local n = source.parent
	while n do
		ancestors[n] = true
		n = n.parent
	end
	local lca = target.parent
	while lca and not ancestors[lca] do
		lca = lca.parent
	end
	lca = lca or run.m.root
	run.changing = true
	run.serial += 1
	while leaf(run) ~= lca do
		exit(self, run)
	end
	local path = {}
	n = target
	while n ~= lca do
		table.insert(path, 1, n)
		n = n.parent
	end
	for _, node in path do
		enter(self, run, node)
	end
	descend(self, run, target)
	run.changing = false
	self.state = leaf(run).name
	log(run, from, self.state, why)
end

-- `done` transitions of states whose enter task has finished, innermost first.
settle = function(self: any, run: any)
	local guard = 0
	while true do
		guard += 1
		if guard > MAX_CHANGES + 1 then
			return
		end
		local fired = false
		for k = #run.active, 1, -1 do
			local node = run.active[k]
			local to = node.def.done
			if to ~= nil and run.finished[node.name] then
				change(self, run, node, to, "done")
				fired = true
				break
			end
		end
		if not fired then
			return
		end
	end
end

local function start(self: any, clock: number, at: string?)
	local m, problem = machine_of(getmetatable(self).__index)
	if not m then
		error(`states: {problem}`, 0)
	end
	local run = {
		m = m,
		active = {},
		since = {},
		visit = {},
		finished = {},
		running = {},
		last = {},
		log = {},
		clock = clock,
		serial = 0,
		changes = 0,
		changing = true,
	}
	rawset(self, "__machine", run)
	table.insert(run.active, m.root)
	run.since.root = clock
	local first = m.nodes[at or m.root.initial]
	local path = {}
	local n = first
	while n ~= m.root do
		table.insert(path, 1, n)
		n = n.parent
	end
	for _, node in path do
		enter(self, run, node)
	end
	descend(self, run, first)
	run.changing = false
	self.state = leaf(run).name
	log(run, "", self.state, if at then "go" else "start")
	settle(self, run)
	return run
end

local function running(self: any): any
	return rawget(self, "__machine")
end

-- The engine's calls (the runtime's).

function Base:__states_start(clock: number)
	if running(self) then
		return
	end
	start(self, clock)
end

function Base:__states_tick(dt: number, clock: number)
	local run = running(self)
	if not run then
		return
	end
	run.clock = clock
	run.changes = 0
	-- Every active state's update, outermost first.
	for _, node in table.clone(run.active) do
		if run.visit[node.name] or node == run.m.root then
			local h = hook_of(self, node, "update")
			if h then
				h(self, dt)
			end
		end
	end
	-- Timers and conditions, innermost first; after a change, the new states' turn.
	for _ = 1, MAX_CHANGES + 1 do
		local fired = false
		for k = #run.active, 1, -1 do
			local node = run.active[k]
			local d = node.def
			local a = d.after
			local secs = a and seconds(self, a[1])
			if a and clock - (run.since[node.name] or clock) >= secs then
				change(self, run, node, a[2], `after {secs} s`)
				fired = true
				break
			end
			if d.when then
				for _, cond in sorted_keys(d.when) do
					if holds(self, cond) then
						change(self, run, node, d.when[cond], `when {cond}`)
						fired = true
						break
					end
				end
				if fired then
					break
				end
			end
		end
		if not fired then
			break
		end
		settle(self, run)
	end
end

-- An event: the innermost state that knows it changes state. Returns whether one did.
function Base:__states_event(event: string, ...: any): boolean
	local run = running(self)
	if not run then
		return false
	end
	for k = #run.active, 1, -1 do
		local node = run.active[k]
		local on = node.def.on
		local cs = on and choices(on[event])
		if cs then
			for _, c in cs do
				if c.guard == nil or holds(self, c.guard, ...) then
					local why = event
					local by = ...
					if type(by) == "userdata" then
						local ok, name = pcall(function()
							return by.name
						end)
						if ok and type(name) == "string" and name ~= "" then
							why = `{event} by {name}`
						end
					end
					change(self, run, node, c.to, why)
					settle(self, run)
					return true
				end
			end
		end
	end
	return false
end

-- Whether some state of the class's machine takes `event`.
function Base:__states_hears(event: string): boolean
	local m = machine_of(getmetatable(self).__index)
	return m ~= nil and m.events[event] == true
end

-- After a hot reload: the same states if they still exist (with their times), else the
-- nearest group that does, entered afresh.
function Base:__states_reload(clock: number)
	local run = running(self)
	if not run then
		-- States new in this version.
		start(self, clock)
		return
	end
	local m, problem = machine_of(getmetatable(self).__index)
	if not m then
		error(`states: {problem}`, 0)
	end
	if m == run.m then
		return
	end
	local old = run.active
	local kept = { m.root }
	for k = 2, #old do
		local node = m.nodes[old[k].name]
		if not node or node.parent ~= kept[#kept] then
			break
		end
		table.insert(kept, node)
	end
	run.m = m
	if #kept == #old and #m.nodes[old[#old].name].children == 0 then
		run.active = kept
		return
	end
	-- Leave what's gone (its tasks stop), then enter the rest afresh.
	for k = #old, #kept + 1, -1 do
		local name = old[k].name
		local t = run.running[name]
		if t then
			run.running[name] = nil
			task.cancel(t)
		end
		run.visit[name] = nil
		run.finished[name] = nil
	end
	run.active = kept
	local from = old[#old].name
	run.changing = true
	run.serial += 1
	local base = kept[#kept]
	if base == m.root then
		local first = m.nodes[m.root.initial]
		enter(self, run, first)
		base = first
	end
	descend(self, run, base, true)
	run.changing = false
	self.state = leaf(run).name
	log(run, from, self.state, "reload")
	settle(self, run)
end

-- What the editor shows of a running machine: active states, their times, the log.
function Base:__states_live(): any
	local run = running(self)
	if not run then
		return nil
	end
	local active = {}
	local since = {}
	local busy = {}
	for _, node in run.active do
		if node.name ~= "root" then
			table.insert(active, node.name)
			since[node.name] = run.since[node.name]
			if run.running[node.name] then
				busy[node.name] = true
			end
		end
	end
	local entries = {}
	for _, e in run.log do
		table.insert(entries, { t = e.t, from = e.from, to = e.to, why = e.why })
	end
	return {
		active = active,
		since = since,
		running = busy,
		clock = run.clock,
		log = entries,
		history = table.clone(run.last),
	}
end

-- A class's machine as plain data (no functions) for the editor: states and transitions, or
-- the problem with it.
function Base.__states_shape(class: any): any
	local m, problem, about = machine_of(class)
	if not m then
		return { problem = problem, about = about }
	end
	local states = {}
	local transitions = {}
	for _, node in m.order do
		if node.name ~= "root" then
			local d = node.def
			local hooks = {}
			for _, hook in { "enter", "update", "exit" } do
				local h = d[hook]
				if type(h) == "string" then
					h = class[h]
				end
				if h == nil then
					h = class[`{hook}_{node.name}`]
				end
				if type(h) == "function" then
					table.insert(hooks, hook)
				end
			end
			table.insert(states, {
				name = node.name,
				parent = if node.parent and node.parent.name ~= "root" then node.parent.name else nil,
				initial = node.initial,
				history = d.history and true or nil,
				doc = if type(d.doc) == "string" then d.doc else nil,
				hooks = hooks,
			})
		end
		local d = node.def
		local from = node.name
		if d.on then
			for _, event in sorted_keys(d.on) do
				for _, c in choices(d.on[event]) or {} do
					table.insert(transitions, {
						from = from,
						to = c.to,
						kind = "on",
						event = event,
						guard = if type(c.guard) == "string" then c.guard elseif c.guard then "(function)" else nil,
					})
				end
			end
		end
		if d.after then
			local secs = d.after[1]
			local prop = nil
			if type(secs) == "string" then
				prop = secs
				local props = rawget(class, "props")
				secs = if type(props) == "table" and type(props[prop]) == "number" then props[prop] else nil
			end
			table.insert(
				transitions,
				{ from = from, to = d.after[2], kind = "after", secs = secs, prop = prop }
			)
		end
		if d.done then
			table.insert(transitions, { from = from, to = d.done, kind = "done" })
		end
		if d.when then
			for _, cond in sorted_keys(d.when) do
				table.insert(transitions, { from = from, to = d.when[cond], kind = "when", cond = cond })
			end
		end
	end
	return { initial = m.root.initial, states = states, transitions = transitions }
end

-- What scripts call.

-- Goes to the state named `name` now (from the current one; its exits and enters run).
function Base:go(name: string)
	local run = running(self)
	if not run then
		-- Before the machine started (in `start`): it starts there.
		local m, problem = machine_of(getmetatable(self).__index)
		if not m then
			error(`states: {problem}`, 2)
		end
		if m.nodes[name] == nil or name == "root" then
			error(`no state "{name}" in states`, 2)
		end
		start(self, 0, name)
		return
	end
	if run.m.nodes[name] == nil or name == "root" then
		local names = {}
		for _, node in run.m.order do
			if node.name ~= "root" then
				table.insert(names, node.name)
			end
		end
		table.sort(names)
		error(`no state "{name}" in states ({list(names)})`, 2)
	end
	change(self, run, leaf(run), name, "go")
	settle(self, run)
end

-- Sends an event to the state machine, as an interaction or a message would. Returns whether
-- a state took it.
function Base:trigger(event: string, ...: any): boolean
	return (self :: any):__states_event(event, ...)
end

-- Whether the state named `name` is active (the current one or a group around it).
function Base:in_state(name: string): boolean
	local run = running(self)
	return run ~= nil and is_active(run, name)
end

-- Seconds since the current state (or the named active one) started.
function Base:state_time(name: string?): number
	local run = running(self)
	if not run then
		return 0
	end
	local at = run.since[name or leaf(run).name]
	return if at then run.clock - at else 0
end

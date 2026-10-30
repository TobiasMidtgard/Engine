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
-- group: entering it enters its `initial` one (or the one it was last in, with `history = true`;
-- with `history = "deep"` the states inside that one resume where they were too).
-- A group with `parallel = true` runs all its states at once (regions, each usually a group of
-- its own): an event reaches every region, and `self.state` is the first region's state.
-- A state with `final = true` ends its group: a group's `done` follows when it reaches one of its
-- final states (a parallel group's, when every region has), as XState's onDone and SCXML's
-- done.state; a final state at the outer level ends the machine (nothing changes it until `go`).
-- Triggers: `on = { event = "to" }` (interactions are `interact`; messages, wired signals and
-- `self:trigger(event)` use their names), `after = { seconds, "to" }`, `done = "to"` and
-- `when = { condition = "to" }` (a method or field of the script, `"not lit"` too, checked every
-- frame). Events go from the innermost state out; the first that knows the event takes it. An
-- `on` entry may be `{ to = "x", guard = "condition" }` or a list of those, the first whose guard
-- holds wins. An event (or `go`) sent while the machine is changing state, from an enter or exit
-- hook, waits until that change has settled: each change runs to completion first. Runs as part
-- of the script: the runtime calls `__states_start` after `start`, `__states_tick` after
-- `update`, `__states_event` for each event and `__states_reload` after a hot reload;
-- `__states_hit` tells it a breakpoint the editor set was reached. Each change keeps the
-- script's values as they were (the editor shows them when it scrubs back through the log).
-- engine-design/scripting-research.md §19, §20 and §21.

local Base = __script_base
local spawn_as = __task_spawn_as
local hit_break = __states_hit

local MAX_CHANGES = 32
local LOG_LENGTH = 64
-- The values a change keeps, at most.
local MAX_VALUES = 24
local KEYS = {
	initial = true,
	history = true,
	parallel = true,
	final = true,
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
		if d.final ~= nil and type(d.final) ~= "boolean" then
			return `state {name}: final must be true or false`, name
		end
		if d.final then
			if name == "root" then
				return "the outer table can't be final: put final = true on the state that ends the machine", nil
			end
			if d.states ~= nil then
				return `state {name} is final, so it has no states inside: it is where its group ends`, name
			end
			for _, k in { "on", "after", "when", "done" } do
				if d[k] ~= nil then
					return `state {name} is final, so it has no way out ({k}): reaching it ends its group`, name
				end
			end
		end
		local h = d.history
		if h ~= nil and h ~= true and h ~= false and h ~= "shallow" and h ~= "deep" then
			return `state {name}: history must be true (back to the state it was last in) or "deep" (and to where the states inside that one were)`,
				name
		end
		local kids = d.states
		if kids == nil then
			if h then
				return `state {name} has history but no states inside: history picks which of them it goes back to`,
					name
			end
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
		if d.parallel ~= nil and type(d.parallel) ~= "boolean" then
			return `state {name}: parallel must be true or false`, name
		end
		if d.parallel then
			if #names == 0 then
				return `state {name} is parallel but has no states inside (its regions, all running at once)`,
					name
			end
			if d.initial ~= nil then
				return `state {name} is parallel: all its states start together, so it has no initial`,
					name
			end
			if h == true or h == "shallow" then
				return `state {name} is parallel, so it enters all its states anyway: history = "deep" keeps where each of them was`,
					name
			end
			return nil
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
			if name == "root" then
				return nil, "the outer table has no done: a final state at the outer level ends the machine", nil
			end
			-- A group's done follows when it reaches a final state: it must have one to reach.
			if #node.children > 0 then
				local function finishes(n: any): boolean
					if #n.children == 0 then
						return n.def.final == true
					end
					for _, c in n.children do
						if n.def.parallel then
							if not finishes(nodes[c]) then
								return false
							end
						elseif nodes[c].def.final then
							return true
						end
					end
					return n.def.parallel == true
				end
				if node.def.parallel then
					for _, c in node.children do
						if not finishes(nodes[c]) then
							return nil,
								`state {name}: done follows when every region is in a final state, and {c} never is (give one of its states final = true)`,
								c
						end
					end
				elseif not finishes(node) then
					return nil,
						`state {name}: done follows when it reaches a final state, and none of its states is final (give one final = true)`,
						name
				end
			end
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

-- The running machine. `run.on` is the set of active states: the root, one state of each
-- active group, and every region of an active parallel group, so they form a tree.

-- The active states in document order (a group before the states inside it, a parallel group's
-- regions one after another).
local function actives(run: any): { any }
	local list = run.list
	if list then
		return list
	end
	list = {}
	for _, node in run.m.order do
		if run.on[node] then
			table.insert(list, node)
		end
	end
	run.list = list
	return list
end

local function is_active(run: any, name: string): boolean
	local node = run.m.nodes[name]
	return node ~= nil and run.on[node] == true
end

-- Whether `node` is `outer` or inside it.
local function within(node: any, outer: any): boolean
	local n = node
	while n do
		if n == outer then
			return true
		end
		n = n.parent
	end
	return false
end

-- The active states with no active state inside: the current state, or one per region.
local function leaves(run: any): { any }
	local out = {}
	local list = actives(run)
	for k, node in list do
		-- In document order a state's first active child comes right after it.
		local after = list[k + 1]
		if not (after and after.parent == node) then
			table.insert(out, node)
		end
	end
	return out
end

-- `self.state`: the current state (with parallel regions, the first region's).
local function current(run: any): any
	return leaves(run)[1]
end

-- The active state a transition from `node` leaves: `node`, or the state it is in.
local function leaf_under(run: any, node: any): any
	for _, l in leaves(run) do
		if within(l, node) then
			return l
		end
	end
	return node
end

-- The active leaf whose group is nearest `target` (what `go` changes from: only that region).
local function nearest(run: any, target: any): any
	local best, depth = nil, -1
	for _, l in leaves(run) do
		local seen = {}
		local n = l
		while n do
			seen[n] = true
			n = n.parent
		end
		n = target
		while n and not seen[n] do
			n = n.parent
		end
		local d = if n then n.depth else 0
		if d > depth then
			best, depth = l, d
		end
	end
	return best or run.m.root
end

local function snapshot(run: any): { string }
	local out = {}
	for _, node in actives(run) do
		if node.name ~= "root" then
			table.insert(out, node.name)
		end
	end
	return out
end

-- A number as the editor shows it: whole numbers plain, others to three decimals.
local function num(x: number): string
	if x == math.floor(x) and math.abs(x) < 1e15 then
		return string.format("%d", x)
	end
	local s = string.format("%.3f", x)
	s = string.gsub(s, "0+$", "")
	return s
end

-- A value as the editor shows it (nil: one it leaves out, as functions and tables).
local function shown(v: any): string?
	local kind = type(v)
	if kind == "number" then
		return num(v)
	elseif kind == "boolean" then
		return tostring(v)
	elseif kind == "string" then
		return if #v > 40 then `"{string.sub(v, 1, 39)}…"` else `"{v}"`
	elseif kind == "table" then
		local mt = getmetatable(v)
		if type(mt) == "table" and mt.__name == "vec3" then
			return `({num(v.x)}, {num(v.y)}, {num(v.z)})`
		end
		return nil
	elseif kind == "userdata" then
		-- An entity: its name.
		local ok, name = pcall(function()
			return v.name
		end)
		return if ok and type(name) == "string" and name ~= "" then name else nil
	end
	return nil
end

-- The script's values now, sorted by name: its props (`props.range`) and its own fields (not
-- `_private` ones), those `shown` can show (PlayMaker's Debug Flow shows them as they were at
-- each change).
local function values(self: any): { { string } }
	local out = {}
	local props = rawget(self, "props")
	if type(props) == "table" then
		for _, k in sorted_keys(props) do
			local s = shown(props[k])
			if s then
				table.insert(out, { `props.{k}`, s })
			end
		end
	end
	local own = {}
	for k in self do
		-- `_name` fields are the script's own business (and `__` ones the engine's).
		if type(k) == "string" and string.sub(k, 1, 1) ~= "_" and k ~= "props" and k ~= "entity" and k ~= "state" then
			table.insert(own, k)
		end
	end
	table.sort(own)
	for _, k in own do
		local s = shown(rawget(self, k))
		if s then
			table.insert(out, { k, s })
		end
	end
	while #out > MAX_VALUES do
		table.remove(out)
	end
	return out
end

-- A change, with every state active after it and the script's values (the editor shows the
-- machine as it was then).
local function log(self: any, run: any, from: string, to: string, why: string)
	table.insert(
		run.log,
		{ t = run.clock, from = from, to = to, why = why, active = snapshot(run), values = values(self) }
	)
	if #run.log > LOG_LENGTH then
		table.remove(run.log, 1)
	end
end

-- "interact by Player": the event, and who sent it when its first value is a named entity.
local function why_of(event: string, ...: any): string
	local by = ...
	if type(by) == "userdata" then
		local ok, name = pcall(function()
			return by.name
		end)
		if ok and type(name) == "string" and name ~= "" then
			return `{event} by {name}`
		end
	end
	return event
end

-- A breakpoint the editor set on this state (`__states_breaks`): the game pauses at the end
-- of the frame, as PlayMaker's and StateTree's do.
local function check_break(self: any, run: any, kind: string, name: string)
	local b = rawget(self, "__breaks")
	local set = b and b[kind]
	if set and set[name] then
		hit_break(self.entity, kind, name, run.why or "")
	end
end

local settle

-- Enters `node`: its time starts, its enter hook starts as a task.
local function enter(self: any, run: any, node: any)
	run.on[node] = true
	run.list = nil
	run.enters += 1
	run.since[node.name] = run.clock
	run.visit[node.name] = run.serial
	run.finished[node.name] = nil
	-- Its enter hook sees where it is (the change sets it again once every state is entered).
	self.state = current(run).name
	check_break(self, run, "enter", node.name)
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

-- Leaves the active state `node` (the states inside it have left already).
local function exit(self: any, run: any, node: any)
	run.on[node] = nil
	run.list = nil
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
	check_break(self, run, "exit", name)
	local h = hook_of(self, node, "exit")
	if h then
		h(self)
	end
end

-- Enters what's missing under the active `node`, down to leaves: a group's `initial` state (or
-- the one it was last in, with `history`, unless `fresh`) and every region of a parallel group.
-- `deep`: inside a group with `history = "deep"`, so every group resumes where it was.
local function fill(self: any, run: any, node: any, fresh: boolean?, deep: boolean?)
	if #node.children == 0 then
		return
	end
	local nodes = run.m.nodes
	local d = node.def
	deep = deep or d.history == "deep"
	if d.parallel then
		for _, name in node.children do
			local c = nodes[name]
			if not run.on[c] then
				enter(self, run, c)
			end
			fill(self, run, c, fresh, deep)
		end
		return
	end
	local inside = nil
	for _, name in node.children do
		if run.on[nodes[name]] then
			inside = nodes[name]
			break
		end
	end
	if not inside then
		local next_name = node.initial
		local last = run.last[node.name]
		if not fresh and (d.history or deep) and last and nodes[last] and nodes[last].parent == node then
			next_name = last
		end
		inside = nodes[next_name]
		enter(self, run, inside)
	end
	fill(self, run, inside, fresh, deep)
end

-- Whether the active `node` is done: a group in one of its final states (a parallel group with
-- every region done), a state on its own that is final.
local function complete(run: any, node: any): boolean
	local nodes = run.m.nodes
	if #node.children == 0 then
		return node.def.final == true
	end
	for _, name in node.children do
		local c = nodes[name]
		if node.def.parallel then
			if not complete(run, c) then
				return false
			end
		elseif run.on[c] then
			return c.def.final == true
		end
	end
	return node.def.parallel == true
end

-- Goes from `source` (an active state, the one whose transition fires) to the state named
-- `to`: leaves every active state below the nearest group holding both, enters down to `to`,
-- then fills in the rest (`initial` states, history, a parallel group's other regions). A change
-- between two regions of a parallel group starts all its regions again, as in SCXML.
local function change(self: any, run: any, source: any, to: string, why: string)
	run.changes += 1
	-- Where it was: the state it was in under `source` (a parallel group: the group, its
	-- regions each being in one).
	local from = if source.def.parallel then source.name else leaf_under(run, source).name
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
	run.why = why
	local list = actives(run)
	for k = #list, 1, -1 do
		local node = list[k]
		if node ~= lca and within(node, lca) then
			exit(self, run, node)
		end
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
	fill(self, run, lca)
	run.changing = false
	run.why = nil
	self.state = current(run).name
	log(self, run, from, leaf_under(run, target).name, why)
end

local deliver

-- What waited while the machine changed state: an event or a `go`.
local function dispatch(self: any, run: any, q: any)
	if q.go then
		change(self, run, nearest(run, run.m.nodes[q.go]), q.go, "go")
	else
		deliver(self, run, q.event, table.unpack(q.args, 1, q.args.n))
	end
end

-- After a change: `done` transitions, innermost first (of states whose enter task has
-- finished, of groups that reached a final state), then what waited for the change, one at a
-- time, each settling in turn. A final state at the outer level ends the machine.
settle = function(self: any, run: any)
	while true do
		if run.over then
			table.clear(run.queue)
			return
		end
		local fired = false
		local list = actives(run)
		for k = #list, 1, -1 do
			local node = list[k]
			local to = node.def.done
			if
				to ~= nil
				and run.on[node]
				and (if #node.children > 0 then complete(run, node) else run.finished[node.name])
			then
				change(self, run, node, to, "done")
				fired = true
				break
			end
		end
		if not fired then
			if complete(run, run.m.root) then
				run.over = true
			else
				local q = table.remove(run.queue, 1)
				if not q then
					return
				end
				dispatch(self, run, q)
			end
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
		on = {},
		list = nil,
		since = {},
		visit = {},
		finished = {},
		running = {},
		last = {},
		log = {},
		clock = clock,
		serial = 0,
		enters = 0,
		changes = 0,
		changing = true,
		why = if at then "go" else "start",
		-- Events and `go`s sent while it changed state, waiting for the change to settle.
		queue = {},
		-- It reached a final state at the outer level.
		over = false,
	}
	rawset(self, "__machine", run)
	run.on[m.root] = true
	run.since.root = clock
	if at then
		local path = {}
		local n = m.nodes[at]
		while n ~= m.root do
			table.insert(path, 1, n)
			n = n.parent
		end
		for _, node in path do
			enter(self, run, node)
		end
	end
	fill(self, run, m.root)
	run.changing = false
	run.why = nil
	self.state = current(run).name
	log(self, run, "", self.state, if at then "go" else "start")
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
	-- Every active state's update, outermost first (a state left by an earlier update skips).
	for _, node in actives(run) do
		if run.on[node] and (run.visit[node.name] or node == run.m.root) then
			local h = hook_of(self, node, "update")
			if h then
				h(self, dt)
			end
		end
	end
	if run.over then
		return
	end
	-- Timers and conditions, innermost first; after a change, the new states' turn.
	for _ = 1, MAX_CHANGES + 1 do
		local fired = false
		local list = actives(run)
		for k = #list, 1, -1 do
			local node = list[k]
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

-- An event: in each region, the innermost state that knows it changes state (a state that
-- several regions share takes it once). Returns whether one did.
deliver = function(self: any, run: any, event: string, ...: any): boolean
	local took = false
	local ls = leaves(run)
	local stamps = {}
	for _, l in ls do
		stamps[l] = run.visit[l.name]
	end
	local fired = {}
	for _, l in ls do
		-- Still where it was (an earlier region's change may have left it).
		if run.on[l] and run.visit[l.name] == stamps[l] then
			local node = l
			while node and not fired[node] do
				local on = node.def.on
				local cs = on and choices(on[event])
				local chosen = nil
				if cs then
					for _, c in cs do
						if c.guard == nil or holds(self, c.guard, ...) then
							chosen = c
							break
						end
					end
				end
				if chosen then
					fired[node] = true
					change(self, run, node, chosen.to, why_of(event, ...))
					took = true
					break
				end
				node = node.parent
			end
		end
	end
	return took
end

-- An event for the machine. While it is changing state (the event comes from an enter or exit
-- hook) it waits until the change has settled, and the answer is whether some state takes it.
function Base:__states_event(event: string, ...: any): boolean
	local run = running(self)
	if not run or run.over then
		return false
	end
	if run.changing then
		table.insert(run.queue, { event = event, args = table.pack(...) })
		return run.m.events[event] == true
	end
	local took = deliver(self, run, event, ...)
	if took then
		settle(self, run)
	end
	return took
end

-- Whether some state of the class's machine takes `event`.
function Base:__states_hears(event: string): boolean
	local m = machine_of(getmetatable(self).__index)
	return m ~= nil and m.events[event] == true
end

-- The editor's breakpoints on this script's states: { enter = { name = true }, exit = { ... } }.
function Base:__states_breaks(breaks: any)
	rawset(self, "__breaks", breaks)
end

-- After a hot reload: the same states if they still exist where they were (with their times);
-- what's gone is left and the groups it was in start again from their first state.
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
	local old = actives(run)
	local from = current(run).name
	local keep = { [m.root] = true }
	local kept = 1
	for _, o in old do
		local node = m.nodes[o.name]
		if o.parent and node and node.parent and node.parent.name == o.parent.name and keep[node.parent] then
			keep[node] = true
			kept += 1
		end
	end
	-- Leave what's gone (its tasks stop).
	for k = #old, 1, -1 do
		local o = old[k]
		local node = m.nodes[o.name]
		if o.parent and not (node and keep[node]) then
			local t = run.running[o.name]
			if t then
				run.running[o.name] = nil
				task.cancel(t)
			end
			run.visit[o.name] = nil
			run.finished[o.name] = nil
		end
	end
	run.m = m
	run.on = keep
	run.list = nil
	run.changing = true
	run.serial += 1
	run.why = "reload"
	local enters = run.enters
	fill(self, run, m.root, true)
	run.changing = false
	run.why = nil
	run.over = complete(run, m.root)
	self.state = current(run).name
	if run.enters ~= enters or kept ~= #old then
		log(self, run, from, self.state, "reload")
		settle(self, run)
	end
end

-- The current states (one per parallel region), for the rewind recorder each frame: nil
-- before the machine starts.
function Base:__states_now(): { string }?
	local run = running(self)
	if not run then
		return nil
	end
	local out = {}
	for _, l in leaves(run) do
		table.insert(out, l.name)
	end
	return out
end

-- What the editor shows of a running machine: active states (and which are current), their
-- times, the log.
function Base:__states_live(): any
	local run = running(self)
	if not run then
		return nil
	end
	local since = {}
	local busy = {}
	for _, node in actives(run) do
		if node.name ~= "root" then
			since[node.name] = run.since[node.name]
			if run.running[node.name] then
				busy[node.name] = true
			end
		end
	end
	local current_ones = {}
	for _, l in leaves(run) do
		table.insert(current_ones, l.name)
	end
	local entries = {}
	for _, e in run.log do
		table.insert(
			entries,
			{ t = e.t, from = e.from, to = e.to, why = e.why, active = e.active, values = e.values }
		)
	end
	-- Active groups in one of their final states (every region, for a parallel group).
	local done = {}
	for _, node in actives(run) do
		if node.name ~= "root" and #node.children > 0 and complete(run, node) then
			table.insert(done, node.name)
		end
	end
	return {
		active = snapshot(run),
		current = current_ones,
		since = since,
		running = busy,
		clock = run.clock,
		log = entries,
		history = table.clone(run.last),
		done = done,
		over = run.over,
		values = values(self),
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
				history = if d.history == "deep" then "deep" elseif d.history then true else nil,
				parallel = d.parallel and true or nil,
				final = d.final and true or nil,
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

-- Goes to the state named `name` now (from the state nearest it: its exits and enters run; from
-- an enter or exit hook, once the change it is part of has settled). It starts a machine that
-- reached a final state at the outer level again.
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
	if run.changing then
		-- From an enter or exit hook: once this change has settled.
		table.insert(run.queue, { go = name })
		return
	end
	-- Going somewhere starts a finished machine again.
	run.over = false
	run.changes = 0
	change(self, run, nearest(run, run.m.nodes[name]), name, "go")
	settle(self, run)
end

-- Sends an event to the state machine, as an interaction or a message would. Returns whether
-- a state took it.
function Base:trigger(event: string, ...: any): boolean
	return (self :: any):__states_event(event, ...)
end

-- Whether the state named `name` is active (the current one, a group around it, or with
-- parallel regions any region's).
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
	local at = run.since[name or current(run).name]
	return if at then run.clock - at else 0
end

--!nonstrict
-- Script tests, the Luau half. A test file (`*.test.luau`) says what the game should do with
-- describe / it / expect (Jest's shape, as Roblox's Jest Lua and TestEZ have it). Its tests run
-- one after another as tasks in the live game, so a test can spawn things, wait seconds or
-- frames, press E on something and check what the scripts did, as Unreal's functional tests and
-- Unity's play mode tests do. The runtime (testing.rs) loads every file with `session.globals`
-- in its environment, then runs `session.run` as a task; `report` hands it the results.

local Testing = {}

-- An expect that didn't hold (any other error is a test that broke).
local Fail = {}

local function fail(message: string)
	error(setmetatable({ message = message }, Fail), 0)
end

-- A value as a test message shows it: strings quoted, tables a level or two deep.
local function show(v: any, depth: number?): string
	local kind = type(v)
	if kind == "string" then
		return string.format("%q", v)
	elseif kind == "number" then
		if v == math.floor(v) and math.abs(v) < 1e15 then
			return string.format("%d", v)
		end
		return string.format("%.6g", v)
	elseif kind == "table" and vec3.is(v) then
		return tostring(v)
	elseif kind == "table" then
		local d = depth or 0
		if d >= 2 then
			return "{…}"
		end
		local parts = {}
		local n = 0
		for k, x in v do
			n += 1
			if n > 6 then
				table.insert(parts, "…")
				break
			end
			if k == n and type(k) == "number" then
				table.insert(parts, show(x, d + 1))
			elseif k ~= "n" then
				local key = if type(k) == "string" and string.match(k, "^[%a_][%w_]*$")
					then k
					else `[{show(k, d + 1)}]`
				table.insert(parts, `{key} = {show(x, d + 1)}`)
			end
		end
		return "{" .. table.concat(parts, ", ") .. "}"
	end
	return tostring(v)
end

-- The line of `path` that is running now: the innermost call from that file on the stack.
local function line_in(path: string): number?
	for level = 2, 60 do
		local source, line = debug.info(level, "sl")
		if source == nil then
			return nil
		end
		if (source == path or source == "@" .. path) and line and line > 0 then
			return line
		end
	end
	return nil
end

-- Two values the same all the way down (tables key by key; vectors and entities by ==).
local function same(a: any, b: any, seen: { [any]: any }?): boolean
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	local s = seen or {}
	if s[a] == b then
		return true
	end
	s[a] = b
	for k, x in a do
		if not same(x, b[k], s) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

local function is_vector(v: any): boolean
	return type(v) ~= "number" and vec3.is(v)
end

-- `expect(value)`: the matchers, and `.never` with each one turned around.
local function expect(value: any)
	local function matchers(negated: boolean)
		local m = {}
		-- Fails unless `holds` (or, under never, unless it doesn't).
		local function check(holds: boolean, says: string, says_never: string)
			if holds == negated then
				fail(if negated then says_never else says)
			end
		end
		local function number(name: string, v: any)
			if type(v) ~= "number" then
				fail(`{name} needs a number, got {show(v)}`)
			end
		end
		function m.to_be(expected: any)
			check(
				value == expected,
				`expected {show(expected)}, got {show(value)}`,
				`expected anything but {show(expected)}`
			)
		end
		function m.to_equal(expected: any)
			check(
				same(value, expected),
				`expected {show(expected)}, got {show(value)}`,
				`expected something other than {show(expected)}`
			)
		end
		function m.to_be_near(expected: any, within: number?)
			local tolerance = within or 0.001
			local off
			if is_vector(value) and is_vector(expected) then
				off = (value - expected):length()
			else
				number("to_be_near", value)
				number("to_be_near", expected)
				off = math.abs(value - expected)
			end
			check(
				off <= tolerance,
				`expected {show(expected)} (within {show(tolerance)}), got {show(value)}: off by {show(off)}`,
				`expected it farther than {show(tolerance)} from {show(expected)}, got {show(value)}`
			)
		end
		for _, c in
			{
				{ "to_be_greater_than", ">", function(a, b)
					return a > b
				end },
				{ "to_be_less_than", "<", function(a, b)
					return a < b
				end },
				{ "to_be_at_least", ">=", function(a, b)
					return a >= b
				end },
				{ "to_be_at_most", "<=", function(a, b)
					return a <= b
				end },
			}
		do
			local name, op, test = c[1], c[2], c[3]
			m[name] = function(limit: number)
				number(name, value)
				number(name, limit)
				check(
					test(value, limit),
					`expected {op} {show(limit)}, got {show(value)}`,
					`expected not {op} {show(limit)}, got {show(value)}`
				)
			end
		end
		function m.to_be_truthy()
			check(not not value, `expected a true value, got {show(value)}`, `expected false or nil, got {show(value)}`)
		end
		function m.to_be_falsy()
			check(not value, `expected false or nil, got {show(value)}`, `expected a true value, got {show(value)}`)
		end
		function m.to_be_nil()
			check(value == nil, `expected nil, got {show(value)}`, "expected a value, got nil")
		end
		function m.to_be_a(kind: string)
			local actual = if is_vector(value) then "Vec3" else typeof(value)
			check(actual == kind, `expected a {kind}, got a {actual}: {show(value)}`, `expected anything but a {kind}`)
		end
		function m.to_contain(item: any)
			local found = false
			if type(value) == "string" then
				found = string.find(value, tostring(item), 1, true) ~= nil
			elseif type(value) == "table" then
				for _, x in value do
					if same(x, item) then
						found = true
						break
					end
				end
			else
				fail(`to_contain needs a string or a table, got {show(value)}`)
			end
			check(found, `expected {show(value)} to contain {show(item)}`, `expected {show(value)} not to contain {show(item)}`)
		end
		function m.to_have_length(n: number)
			local length = #value
			check(length == n, `expected length {n}, got {length}: {show(value)}`, `expected a length other than {n}`)
		end
		function m.to_throw(text: string?)
			if type(value) ~= "function" then
				fail(`to_throw needs a function to call, got {show(value)}`)
			end
			local ok, err = pcall(value)
			local message = if ok then "" else tostring(if getmetatable(err) == Fail then err.message else err)
			local holds = not ok and (text == nil or string.find(message, text, 1, true) ~= nil)
			check(
				holds,
				if ok
					then "expected it to throw, and it didn't"
					else `expected it to throw {show(text)}, it threw {show(message)}`,
				`expected it not to throw, it threw {show(message)}`
			)
		end
		return m
	end
	local e = matchers(false)
	e.never = matchers(true)
	return e
end
Testing.expect = expect

-- One run's tests, from every file loaded into it. `report(kind, ...)` takes the results;
-- `press(target, by)` presses E on the target (an Interact message).
function Testing.session(report: (string, ...any) -> (), press: (any, any?) -> ())
	local session = { cases = {}, focused = false }

	local function block(name: string?, parent: any, line: number, skip: boolean, only: boolean)
		return {
			name = name,
			parent = parent,
			line = line,
			skip = skip or (parent ~= nil and parent.skip),
			only = only or (parent ~= nil and parent.only),
			before = {},
			after = {},
		}
	end

	-- `describe`, `it`, `expect` and the hooks for the file at `path` (its environment).
	function session.globals(path: string)
		local root = block(nil, nil, 0, false, false)
		local current = root
		local function describe_as(skip: boolean, only: boolean)
			return function(name: string, body: () -> ())
				if type(name) ~= "string" or type(body) ~= "function" then
					error('describe takes a name and a function: describe("Lamp", function() ... end)', 2)
				end
				local outer = current
				current = block(name, outer, line_in(path) or 0, skip, only)
				session.focused = session.focused or only
				body()
				current = outer
			end
		end
		local function it_as(skip: boolean, only: boolean)
			return function(name: string, body: (any) -> (), timeout: number?)
				if type(name) ~= "string" or type(body) ~= "function" then
					error('it takes a name and a function: it("lights up", function(t) ... end)', 2)
				end
				local names = { name }
				local b = current
				while b ~= nil and b.name ~= nil do
					table.insert(names, 1, b.name)
					b = b.parent
				end
				session.focused = session.focused or only
				table.insert(session.cases, {
					file = path,
					name = name,
					title = table.concat(names, " › "),
					line = line_in(path) or 0,
					body = body,
					block = current,
					skip = skip or current.skip,
					only = only or current.only,
					timeout = timeout or 10,
				})
			end
		end
		local function callable(f, skip, only)
			return setmetatable({ skip = skip, only = only }, {
				__call = function(_, ...)
					return f(...)
				end,
			})
		end
		return {
			describe = callable(describe_as(false, false), describe_as(true, false), describe_as(false, true)),
			it = callable(it_as(false, false), it_as(true, false), it_as(false, true)),
			before_each = function(f: (any) -> ())
				table.insert(current.before, f)
			end,
			after_each = function(f: (any) -> ())
				table.insert(current.after, f)
			end,
			expect = expect,
		}
	end

	-- The test's `t`: the world helpers, and what to clean up after it.
	local function context(case)
		local t = { name = case.title }
		local spawned, connections = {}, {}
		function t:spawn(spec)
			local e = world.spawn(spec)
			table.insert(spawned, e)
			local components = type(spec) == "table" and spec.components
			if type(components) == "table" and components.Script ~= nil then
				-- Its script starts next frame; wait for it, so the test can use it at once.
				task.wait_until(function()
					return e.script ~= nil
				end, 2)
			end
			return e
		end
		function t:find(name: string)
			local e = world.find(name)
			if e == nil then
				fail(`there is no entity named {show(name)} in the world`)
			end
			return e
		end
		function t:wait(seconds: number?): number
			return task.wait(seconds)
		end
		function t:frames(n: number?)
			for _ = 1, n or 1 do
				task.wait()
			end
		end
		function t:wait_until(check: () -> boolean, timeout: number?)
			local limit = timeout or 5
			if not task.wait_until(check, limit) then
				fail(`waited {show(limit)} s and it never came true`)
			end
		end
		function t:wait_for(entity, signal: string, timeout: number?)
			local limit = timeout or 5
			local got = nil
			local c = entity:once(signal, function(...)
				got = table.pack(...)
			end)
			task.wait_until(function()
				return got ~= nil
			end, limit)
			c.disconnect()
			if got == nil then
				fail(`{show(entity.name or tostring(entity))} didn't emit {show(signal)} within {show(limit)} s`)
			end
			return table.unpack(got, 1, got.n)
		end
		function t:press(entity, by)
			press(entity, by)
			-- Its on_interact runs at the start of next frame, before this goes on.
			task.wait()
		end
		function t:listen(entity, signal: string)
			local heard = { count = 0, calls = {}, last = nil }
			local c = entity:on(signal, function(...)
				local values = table.pack(...)
				heard.count += 1
				table.insert(heard.calls, values)
				heard.last = values
			end)
			table.insert(connections, c)
			return heard
		end
		local function cleanup()
			for _, c in connections do
				c.disconnect()
			end
			for i = #spawned, 1, -1 do
				local e = spawned[i]
				if e:exists() then
					e:despawn()
				end
			end
		end
		return t, cleanup
	end

	-- Whether `case` is among the picked: a line (of its `it` or a `describe` around it) and
	-- a filter on its title.
	local function picked(case, pick): boolean
		if pick.file ~= nil and case.file ~= pick.file then
			return false
		end
		if pick.filter ~= nil and not string.find(string.lower(case.title), string.lower(pick.filter), 1, true) then
			return false
		end
		if pick.line ~= nil then
			if case.line == pick.line then
				return true
			end
			local b = case.block
			while b ~= nil do
				if b.line == pick.line and b.name ~= nil then
					return true
				end
				b = b.parent
			end
			return false
		end
		return true
	end

	-- The before_each hooks outermost first, the after_each ones innermost first.
	local function hooks(case)
		local before, after = {}, {}
		local b = case.block
		while b ~= nil do
			for i = #b.before, 1, -1 do
				table.insert(before, 1, b.before[i])
			end
			for _, f in b.after do
				table.insert(after, f)
			end
			b = b.parent
		end
		return before, after
	end

	-- The run itself (a task): every picked test in file order, one at a time.
	function session.run(pick)
		local plan = {}
		for _, case in session.cases do
			if picked(case, pick) then
				case.skip = case.skip or (session.focused and not case.only)
				table.insert(plan, case)
			end
		end
		local rows = {}
		for _, case in plan do
			table.insert(rows, { file = case.file, title = case.title, line = case.line, skip = case.skip })
		end
		report("plan", rows)
		local since_yield = os.clock()
		for i, case in plan do
			if case.skip then
				continue
			end
			-- Tests that don't wait run back to back; this keeps a long list from holding a frame.
			if os.clock() - since_yield > 0.004 then
				task.wait()
				since_yield = os.clock()
			end
			local t, cleanup = context(case)
			local outcome = nil
			local finished = false
			local function caught(err)
				local message = if getmetatable(err) == Fail then err.message else tostring(err)
				return { message = message, line = line_in(case.file) }
			end
			local start_world, start_real = world.time(), os.clock()
			local frames = 0
			local before, after = hooks(case)
			local worker = task.spawn(function()
				local ok, err = xpcall(function()
					for _, f in before do
						f(t)
					end
					case.body(t)
				end, caught)
				local ok_after, err_after = xpcall(function()
					for _, f in after do
						f(t)
					end
				end, caught)
				outcome = if not ok then err elseif not ok_after then err_after else nil
				finished = true
			end)
			report("start", i, worker)
			while not finished do
				if world.time() - start_world > case.timeout or os.clock() - start_real > case.timeout + 30 then
					task.cancel(worker)
					outcome = {
						message = `didn't finish within {show(case.timeout)} s (a longer limit: it(name, function(t) ... end, 30))`,
						line = case.line,
					}
					break
				end
				task.wait()
				frames += 1
			end
			pcall(cleanup)
			since_yield = os.clock()
			report(
				"end",
				i,
				outcome == nil,
				outcome and outcome.message,
				outcome and outcome.line,
				world.time() - start_world,
				frames
			)
		end
		report("done")
	end

	return session
end

return Testing

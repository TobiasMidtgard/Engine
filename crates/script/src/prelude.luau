--!strict
-- The Luau half of the script runtime: the task library's waiting functions, the helpers every
-- script instance inherits, and easing curves. Runs once, before the VM is sandboxed.
-- `__task_spawn`, `__task_delay`, `__task_cancel` and `__script_emit` are the runtime's Rust
-- functions.

local yield = coroutine.yield
local spawn_task = __task_spawn
local delay_task = __task_delay
local cancel_task = __task_cancel
local stopped = __script_stopped
local in_parallel = __script_parallel
local emit_signal = __script_emit

-- A script the time limit stopped stays stopped: catching the stop (pcall in a loop) would
-- otherwise let an endless loop run on forever, so these pass it on.
local raw_pcall, raw_xpcall, raw_resume = pcall, xpcall, coroutine.resume
local function pass_stop(ok: boolean, ...: any): ...any
	if not ok and stopped() then
		error((...), 0)
	end
	return ok, ...
end
pcall = function(f, ...)
	return pass_stop(raw_pcall(f, ...))
end
xpcall = function(f, handler, ...)
	return pass_stop(raw_xpcall(f, handler, ...))
end
local co = table.clone(coroutine)
co.resume = function(thread, ...)
	return pass_stop(raw_resume(thread, ...))
end
coroutine = co

-- Gameplay over time. Waits count world seconds on the calling script's clock, so they slow
-- down in bullet time like everything else and follow the entity's LocalTimeScale. A task
-- belongs to the script that started it and stops when that script's entity goes away.
task = {}

-- Pauses this task for `seconds` (the next frame when nil or 0). Returns the seconds that
-- actually passed.
function task.wait(seconds: number?): number
	return yield("wait", seconds or 0)
end

-- Runs `f(...)` now as a new task, until its first wait. Returns the task, for task.cancel.
function task.spawn(f: (...any) -> ...any, ...: any): thread
	return spawn_task(f, ...)
end

-- Runs `f(...)` as a new task after `seconds`.
function task.delay(seconds: number, f: (...any) -> ...any, ...: any): thread
	return delay_task(seconds, f, ...)
end

-- Runs `f(...)` as a new task at the end of this frame's script work.
function task.defer(f: (...any) -> ...any, ...: any): thread
	return delay_task(0, f, ...)
end

-- In `parallel_update`: the rest of it runs after the parallel part of the frame, one script
-- at a time with the whole world (engine calls, spawning, reading what others changed).
-- Elsewhere it does nothing.
function task.synchronize()
	if in_parallel() then
		yield("sync")
	end
end

-- Stops a task started with spawn, delay or defer.
function task.cancel(t: thread)
	cancel_task(t)
end

-- Calls `step(alpha)` every frame for `seconds`, alpha going 0 to 1 through `ease` (linear when
-- nil), ending with exactly 1. Waits until it is done.
function task.tween(seconds: number, step: (number) -> (), ease: ((number) -> number)?)
	local t = 0
	step(0)
	while t < seconds do
		t += task.wait()
		local a = math.min(t / seconds, 1)
		step(if ease then ease(a) else a)
	end
end

-- Waits until `check()` returns true (checked once a frame), or `timeout` seconds. Returns
-- whether it came true.
function task.wait_until(check: () -> boolean, timeout: number?): boolean
	local t = 0
	while not check() do
		if timeout and t >= timeout then
			return false
		end
		t += task.wait()
	end
	return true
end

-- Waits until `entity`'s script emits `signal` (or `timeout` seconds pass), and returns the
-- signal's values (nothing on timeout). Roblox's `Signal:Wait()`.
function task.wait_for(entity: any, signal: string, timeout: number?): ...any
	local got: { n: number, [number]: any }? = nil
	local c = entity:once(signal, function(...)
		got = table.pack(...)
	end)
	task.wait_until(function()
		return got ~= nil
	end, timeout)
	c.disconnect()
	if got then
		return table.unpack(got, 1, got.n)
	end
	return
end

-- `ease` (easing curves for task.tween and tweens) is the runtime's (ease.rs).

-- What every script instance can call on `self`, besides its own class's methods.
local Base = {}
Base.__index = Base

-- Calls `self:<method>(...)` every `seconds` until the script stops. `method` is a method name
-- (looked up each time, so hot reload swaps it) or a function. Returns the task.
function Base:every(seconds: number, method: any, ...: any): thread
	local args = table.pack(...)
	return task.spawn(function()
		while true do
			task.wait(seconds)
			local f = if type(method) == "string" then (self :: any)[method] else method
			f(self, table.unpack(args, 1, args.n))
		end
	end)
end

-- Calls `self:<method>(...)` once after `seconds`. Returns the task.
function Base:after(seconds: number, method: any, ...: any): thread
	local args = table.pack(...)
	return task.delay(seconds, function()
		local f = if type(method) == "string" then (self :: any)[method] else method
		f(self, table.unpack(args, 1, args.n))
	end)
end

-- Fires one of the class's `signals` with its values: every method wired to it in the
-- editor's Details panel (the Script component's connections) and every `entity:on` listener
-- runs, later this frame, each as a task of its own.
function Base:emit(signal: string, ...: any)
	local declared = (self :: any).signals
	if type(declared) ~= "table" or declared[signal] == nil then
		local names = {}
		for name in (if type(declared) == "table" then declared else {}) :: any do
			table.insert(names, name)
		end
		table.sort(names)
		error(
			if #names == 0
				then `no signal "{signal}": the class declares no signals (a table named signals)`
				else `no signal "{signal}" in signals ({table.concat(names, ", ")})`,
			2
		)
	end
	emit_signal((self :: any).entity, signal, table.pack(...))
end

-- Spawns an entity as a child of this script's entity (removed with it).
function Base:spawn(spec: any): any
	return (self :: any).entity:spawn(spec)
end

__script_base = Base

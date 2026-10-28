--!strict
-- The Luau half of the script runtime: the task library's waiting functions, the helpers every
-- script instance inherits, and easing curves. Runs once, before the VM is sandboxed.
-- `__task_spawn`, `__task_delay` and `__task_cancel` are the runtime's Rust functions.

local yield = coroutine.yield
local spawn_task = __task_spawn
local delay_task = __task_delay
local cancel_task = __task_cancel
local stopped = __script_stopped

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

-- Easing curves for task.tween, alpha 0..1 in and out.
ease = {}
function ease.linear(t: number): number
	return t
end
function ease.in_quad(t: number): number
	return t * t
end
function ease.out_quad(t: number): number
	return 1 - (1 - t) * (1 - t)
end
function ease.in_out(t: number): number
	return if t < 0.5 then 2 * t * t else 1 - (-2 * t + 2) ^ 2 / 2
end
function ease.out_back(t: number): number
	local c1 = 1.70158
	local c3 = c1 + 1
	return 1 + c3 * (t - 1) ^ 3 + c1 * (t - 1) ^ 2
end
function ease.out_elastic(t: number): number
	if t == 0 or t == 1 then
		return t
	end
	return 2 ^ (-10 * t) * math.sin((t * 10 - 0.75) * (2 * math.pi) / 3) + 1
end
function ease.smooth(t: number): number
	return t * t * (3 - 2 * t)
end

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

-- Spawns an entity as a child of this script's entity (removed with it).
function Base:spawn(spec: any): any
	return (self :: any).entity:spawn(spec)
end

__script_base = Base

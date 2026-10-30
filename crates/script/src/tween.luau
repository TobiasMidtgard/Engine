--!strict
-- Tweens' handles (the runtime runs the tweens: tween.rs). `__tween_*` are its Rust functions.

local to_raw = __tween_to
local seq_raw = __tween_seq
local add_raw = __tween_add
local spring_raw = __tween_spring
local ctl = __tween_ctl
local info = __tween_info
local kill_raw = __tween_kill
local Base = __script_base

-- A running tween, sequence or spring.
local Tween = {}
local methods: { [string]: any } = {}
Tween.__index = function(self: any, key: string): any
	local m = methods[key]
	if m then
		return m
	end
	local playing, done, time, length, progress = info(rawget(self, "id"))
	if key == "playing" then
		return playing
	elseif key == "done" then
		return done
	elseif key == "time" then
		return time
	elseif key == "length" then
		return length
	elseif key == "progress" then
		return progress
	end
	return nil
end
Tween.__tostring = function(self: any): string
	return `tween {rawget(self, "id")}`
end

local function handle(id: number): any
	return setmetatable({ id = id }, Tween)
end

-- Pauses it where it is.
function methods.pause(self: any): any
	ctl(self.id, "pause")
	return self
end

-- Goes on after pause.
function methods.play(self: any): any
	ctl(self.id, "play")
	return self
end

-- Stops it where it is (complete: jumps to its end first, and calls `done`).
function methods.kill(self: any, complete: boolean?)
	ctl(self.id, if complete then "complete" else "kill")
end

-- From the start again (while it runs).
function methods.restart(self: any): any
	ctl(self.id, "restart")
	return self
end

-- Jumps to `seconds` into the current pass.
function methods.seek(self: any, seconds: number): any
	ctl(self.id, "seek", seconds)
	return self
end

-- Plays it `rate` times as fast (0.5: half speed).
function methods.speed(self: any, rate: number): any
	ctl(self.id, "speed", rate)
	return self
end

-- Waits (in a task) until it is done or killed.
function methods.wait(self: any)
	while not self.done do
		task.wait()
	end
end

-- Sequences (DOTween's Append, Join, Insert): `to` adds a step after the last one, `join`
-- runs with the last one added, `at` starts one at a time of its own.
function methods.to(self: any, target: any, props: any, seconds: number, opts: any?): any
	add_raw(self.id, "to", nil, target, props, seconds, opts)
	return self
end

function methods.join(self: any, target: any, props: any, seconds: number, opts: any?): any
	add_raw(self.id, "join", nil, target, props, seconds, opts)
	return self
end

function methods.at(self: any, time: number, target: any, props: any, seconds: number, opts: any?): any
	add_raw(self.id, "at", time, target, props, seconds, opts)
	return self
end

-- A pause before the next step.
function methods.interval(self: any, seconds: number): any
	add_raw(self.id, "interval", nil, nil, nil, seconds, nil)
	return self
end

-- Calls `f` (as a task) when the sequence gets here.
function methods.call(self: any, f: () -> ()): any
	add_raw(self.id, "call", nil, f, nil, nil, nil)
	return self
end

tween = {}

-- Moves `target`'s values (an entity's position, rotation, scale, "Component.field",
-- "material.field"; a table's fields) to `props` over `seconds`.
function tween.to(target: any, props: any, seconds: number, opts: any?): any
	return handle(to_raw(target, props, seconds, opts))
end

-- An empty sequence: add steps with :to, :join, :at, :interval and :call.
function tween.sequence(opts: any?): any
	return handle(seq_raw(opts))
end

-- Makes `target`'s values follow `props` like a spring (halflife: seconds to close half the
-- gap; bounce: 0 to 0.9). Calling it again for the same values changes their goal, keeping
-- their speed.
function tween.spring(target: any, props: any, opts: any?): any
	return handle(spring_raw(target, props, opts))
end

-- Stops the tweens moving `target` (or only its value named `value`). Returns how many.
function tween.kill(target: any, value: string?): number
	return kill_raw(target, value)
end

function Base:tween(target: any, props: any, seconds: number, opts: any?): any
	return handle(to_raw(target, props, seconds, opts))
end

function Base:sequence(opts: any?): any
	return handle(seq_raw(opts))
end

function Base:spring(target: any, props: any, opts: any?): any
	return handle(spring_raw(target, props, opts))
end

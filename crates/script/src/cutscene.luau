--!strict
-- Cutscenes' handles (the engine plays them: cutscene.rs). `__cutscene_*` are its Rust
-- functions.

local play_raw = __cutscene_play
local ctl = __cutscene_ctl
local info = __cutscene_info
local stop_all = __cutscene_stop_all

-- A cutscene playing.
local Cutscene = {}
local methods: { [string]: any } = {}
Cutscene.__index = function(self: any, key: string): any
	local m = methods[key]
	if m then
		return m
	end
	local playing, done, time, length, missing = info(rawget(self, "id"))
	if key == "playing" then
		return playing
	elseif key == "done" then
		return done
	elseif key == "time" then
		return time
	elseif key == "length" then
		return length
	elseif key == "missing" then
		return missing
	elseif key == "path" then
		return rawget(self, "file")
	end
	return nil
end
Cutscene.__tostring = function(self: any): string
	return `cutscene {rawget(self, "file")}`
end

-- Holds it where it is.
function methods.pause(self: any): any
	ctl(self.id, "pause")
	return self
end

-- Goes on after pause.
function methods.play(self: any): any
	ctl(self.id, "play")
	return self
end

-- Jumps to `at`: seconds, or a mark's name. Its objects are posed there at once; the events
-- it jumps over that are retro are sent.
function methods.seek(self: any, at: number | string): any
	ctl(self.id, "seek", at)
	return self
end

-- Plays it `rate` times as fast (negative: backward).
function methods.set_rate(self: any, rate: number): any
	ctl(self.id, "rate", rate)
	return self
end

-- Jumps to the end and stops (as Unreal's Go to End and Stop): its objects as it leaves
-- them, its retro events sent. For a skip button.
function methods.skip(self: any)
	ctl(self.id, "skip")
end

-- Stops it where it is (values put back if it restores).
function methods.stop(self: any)
	ctl(self.id, "stop")
end

-- Waits (in a task) until it ends, is stopped or skipped.
function methods.wait(self: any)
	while not self.done do
		task.wait()
	end
end

cutscene = {}

-- Plays the .cutscene file at `path` ("cutscenes/intro"). Options: rate, loop, from (seconds
-- or a mark), to, restore (put back what it changed when it ends), cameras (false: don't look
-- through its camera cuts), unscaled (real seconds), at (the entity whose children it looks
-- for names under; the calling script's by default), bind ({ Hero = entity }), done (called,
-- as a task, when it ends).
function cutscene.play(path: string, opts: any?): any
	local o = opts or {}
	local from = o.from
	local raw = o
	if type(from) == "string" then
		raw = table.clone(o)
		raw.from = nil
	end
	local h = setmetatable({ id = play_raw(path, raw), file = path }, Cutscene)
	if type(from) == "string" then
		h:seek(from)
	end
	if o.done then
		local done = o.done
		task.spawn(function()
			h:wait()
			done(h)
		end)
	end
	return h
end

-- Stops every cutscene playing.
function cutscene.stop_all()
	stop_all()
end

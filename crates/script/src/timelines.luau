--!strict
-- Timelines (Unreal's Timeline node): a class's `timelines` table declares them, each with
-- curves as tracks and named events; `self:timeline(name)` plays one; every frame it plays,
-- the class's `on_<name>(values, timeline)` gets each track's value, and each event it passes
-- calls the method of that name. Ends call `on_<name>_done(timeline)`.
--
--   Door.timelines = {
--     open = {
--       tracks = { swing = "curves/door_swing.curve", glow = { {0, 0}, {0.3, 1}, {1.6, 0.2} } },
--       events = { creak = 0.05, thud = 1.45 },
--     },
--   }
--   function Door:on_open(v) self.leaf.rotation = quat.euler(v.swing * 1.7) end
--   self:timeline("open"):play()    -- :reverse() closes it from wherever it is
--
-- The runtime calls `__timelines_tick(dt)` after `update` (and the state machine). Hot
-- reload rebuilds a timeline's tracks and keeps where it is.

local Base = __script_base
local make_curve = curve

type Track = { name: string, curve: any, source: string? }
type Event = { name: string, at: number }

local Timeline = {}
Timeline.__index = Timeline

local function build(decl: any, name: string): ({ Track }, { Event }, number)
	local tracks: { Track } = {}
	local length = 0
	if decl.tracks ~= nil and type(decl.tracks) ~= "table" then
		error(`timeline {name}: tracks is a table of curves`, 3)
	end
	for tname, src in (decl.tracks or {}) :: any do
		local ok, c = pcall(make_curve, src)
		if not ok then
			error(`timeline {name}, track {tname}: {c}`, 3)
		end
		table.insert(tracks, {
			name = tname,
			curve = c,
			source = if type(src) == "string" then c.path else nil,
		})
		length = math.max(length, c.last)
	end
	table.sort(tracks, function(a, b)
		return a.name < b.name
	end)
	local events: { Event } = {}
	for ename, at in (decl.events or {}) :: any do
		if type(at) == "table" then
			for _, t in at do
				table.insert(events, { name = ename, at = t })
			end
		else
			table.insert(events, { name = ename, at = at })
		end
	end
	table.sort(events, function(a, b)
		return a.at < b.at
	end)
	for _, e in events do
		length = math.max(length, e.at)
	end
	if type(decl.length) == "number" then
		length = decl.length
	end
	return tracks, events, length
end

-- The timeline's declaration (as the class has it now).
function Timeline._decl_now(self: any): any
	local decls = (self.owner :: any).timelines
	return if type(decls) == "table" then decls[self.name] else nil
end

-- Builds it again when the class changed (hot reload).
function Timeline._refresh(self: any)
	local decl = self:_decl_now()
	if decl == nil or decl == self._decl then
		return
	end
	self._decl = decl
	self.tracks, self.events, self.length = build(decl, self.name)
	self.loop = decl.loop == true
	if self.time > self.length then
		self.time = self.length
	end
end

-- Each track's value now.
function Timeline._values(self: any): { [string]: number }
	local values = self.values
	for _, t in self.tracks do
		values[t.name] = t.curve:at(self.time)
	end
	return values
end

-- Calls the owner's on_<name>(values, timeline).
function Timeline._update(self: any)
	local f = (self.owner :: any)["on_" .. self.name]
	local values = self:_values()
	if type(f) == "function" then
		f(self.owner, values, self)
	end
end

-- Calls the event methods between old and new (in the direction played).
function Timeline._fire(self: any, old: number, new: number)
	if #self.events == 0 or old == new then
		return
	end
	local forward = new > old
	local list = self.events
	local from, to, step = 1, #list, 1
	if not forward then
		from, to, step = #list, 1, -1
	end
	for i = from, to, step do
		local e = list[i]
		local hit = if forward then e.at > old and e.at <= new else e.at < old and e.at >= new
		if hit then
			local f = (self.owner :: any)[e.name]
			if type(f) == "function" then
				task.spawn(f, self.owner, self)
			end
		end
	end
end

function Timeline._advance(self: any, dt: number)
	self:_refresh()
	local old = self.time
	local new = old + dt * self.rate * self.direction
	local finished = false
	if self.direction > 0 and new >= self.length then
		if self.loop and self.length > 0 then
			self:_fire(old, self.length)
			new = (new - self.length) % self.length
			self:_fire(-1e-9, new)
		else
			self:_fire(old, self.length)
			new = self.length
			finished = true
		end
	elseif self.direction < 0 and new <= 0 then
		if self.loop and self.length > 0 then
			self:_fire(old, 0)
			new = self.length - (-new % self.length)
			self:_fire(self.length + 1e-9, new)
		else
			self:_fire(old, 0)
			new = 0
			finished = true
		end
	else
		self:_fire(old, new)
	end
	self.time = new
	self:_update()
	if finished then
		self.playing = false
		local f = (self.owner :: any)["on_" .. self.name .. "_done"]
		if type(f) == "function" then
			task.spawn(f, self.owner, self)
		end
	end
end

-- Plays forward from where it is.
function Timeline.play(self: any): any
	self:_refresh()
	self.direction = 1
	self.playing = true
	return self
end

-- Plays forward from the start.
function Timeline.play_from_start(self: any): any
	self:_refresh()
	self.time = 0
	return self:play()
end

-- Plays backward from where it is (a door closes from however open it is).
function Timeline.reverse(self: any): any
	self:_refresh()
	self.direction = -1
	self.playing = true
	return self
end

-- Plays backward from the end.
function Timeline.reverse_from_end(self: any): any
	self:_refresh()
	self.time = self.length
	return self:reverse()
end

-- Stops where it is.
function Timeline.stop(self: any): any
	self.playing = false
	return self
end

-- Jumps to `seconds` (firing the events passed when fire_events) and applies it.
function Timeline.seek(self: any, seconds: number, fire_events: boolean?): any
	self:_refresh()
	local new = math.clamp(seconds, 0, self.length)
	if fire_events then
		self:_fire(self.time, new)
	end
	self.time = new
	self:_update()
	return self
end

-- Plays `rate` times as fast.
function Timeline.set_rate(self: any, rate: number): any
	self.rate = rate
	return self
end

-- Waits (in a task) until it stops.
function Timeline.wait(self: any)
	while self.playing do
		task.wait()
	end
end

-- The timeline `name` from the class's `timelines` (made the first time).
function Base:timeline(name: string): any
	local list = rawget(self, "_timelines")
	if list == nil then
		list = {}
		rawset(self, "_timelines", list)
	end
	local tl = list[name]
	if tl == nil then
		local decls = (self :: any).timelines
		local decl = if type(decls) == "table" then decls[name] else nil
		if decl == nil then
			local names = {}
			for n in (if type(decls) == "table" then decls else {}) :: any do
				table.insert(names, n)
			end
			table.sort(names)
			error(
				if #names == 0
					then `no timeline "{name}": the class declares none (a table named timelines)`
					else `no timeline "{name}" in timelines ({table.concat(names, ", ")})`,
				2
			)
		end
		tl = setmetatable({
			owner = self,
			name = name,
			time = 0,
			rate = if type(decl.rate) == "number" then decl.rate else 1,
			direction = 1,
			playing = false,
			loop = false,
			values = {},
			tracks = {},
			events = {},
			length = 0,
		}, Timeline)
		list[name] = tl
		tl:_refresh()
	end
	return tl
end

-- After update: autoplay ones start, playing ones move on.
function Base:__timelines_tick(dt: number)
	local decls = (self :: any).timelines
	if type(decls) ~= "table" then
		return
	end
	if not rawget(self, "_timelines_began") then
		rawset(self, "_timelines_began", true)
		local names = {}
		for name, decl in decls :: any do
			if type(decl) == "table" and decl.autoplay then
				table.insert(names, name)
			end
		end
		table.sort(names)
		for _, name in names do
			self:timeline(name):play()
		end
	end
	local list = rawget(self, "_timelines")
	if list == nil then
		return
	end
	local names = {}
	for name, tl in list do
		if tl.playing then
			table.insert(names, name)
		end
	end
	table.sort(names)
	for _, name in names do
		list[name]:_advance(dt)
	end
end

-- For the editor: each timeline's place, tracks and events.
function Base:__timelines_now(): { any }
	local out = {}
	local decls = (self :: any).timelines
	if type(decls) ~= "table" then
		return out
	end
	local names = {}
	for name in decls :: any do
		table.insert(names, name)
	end
	table.sort(names)
	for _, name in names do
		local ok, tl = pcall(self.timeline, self, name)
		if ok then
			tl:_refresh()
			local tracks = {}
			for _, t in tl.tracks do
				table.insert(tracks, { name = t.name, curve = t.curve, value = t.curve:at(tl.time) })
			end
			local events = {}
			for _, e in tl.events do
				table.insert(events, { name = e.name, at = e.at })
			end
			table.insert(out, {
				name = name,
				time = tl.time,
				length = tl.length,
				playing = tl.playing,
				direction = tl.direction,
				tracks = tracks,
				events = events,
			})
		end
	end
	return out
end

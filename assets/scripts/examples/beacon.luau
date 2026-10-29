--!strict
-- A beacon: a light that pulses and turns its head, and fires a signal every few flashes. Put
-- it on an entity with a PointLight. Each prop below is a field in the editor's Details panel:
-- the hints after @ give it a slider, a unit, a group or a picker (see "Props in the Details
-- panel" in the scripts README); each signal can be wired there to any object's script.
local Beacon = {}
Beacon.props = {
	-- @group Light
	lumens = 2400, -- at the top of a pulse @range 0 8000 @unit lm
	color = vec3(0.35, 0.75, 1), -- linear RGB
	-- how it breathes
	pulse = {
		rate = 0.8, -- pulses a second @range 0.1 4 @unit Hz
		depth = 0.6, -- how far each one dims @range 0 1
	},
	-- @group Motion
	spin = 0.25, -- turns a second @range -2 2 0.05 @unit turns/s
	tilt = 0.2, -- how far the head leans @angle
	hover = vec3(0, 0.15, 0), -- how far it bobs, meters
	-- @group Signal
	mode = "pulse", -- how it shines @enum steady|pulse|blink
	every = 4, -- flashes between signals @int @min 1
	announce = true, -- say so in the output too
}

-- What it tells the scene. Wire a signal to a method of any object's script in the Details
-- panel (Signals), or listen in code: beacon:on("flashed", function(count) ... end).
Beacon.signals = {
	flashed = "count: number", -- every few flashes, with how many so far
}

function Beacon:start()
	self.t = 0
	self.home = self.entity.position
	self.phase = 0 -- pulses so far, and how far into this one
	self.flashes = 0
end

function Beacon:update(dt: number)
	self.t += dt
	local p = self.props
	self.phase += dt * p.pulse.rate
	local wave = 0.5 + 0.5 * math.sin(self.phase * 2 * math.pi)
	local level = if p.mode == "steady"
		then 1
		elseif p.mode == "blink" then (if wave > 0.5 then 1 else 0.05)
		else 1 - p.pulse.depth * (1 - wave)
	self.level = level
	self.entity:set("PointLight", { color = p.color, intensity_lm = p.lumens * level })
	self.entity:material({ emissive = p.color, emissive_strength = 2500 * level })
	self.entity.position = self.home + p.hover * math.sin(self.t * 1.3)
	self.entity.rotation = quat.axis_angle(vec3.up, self.t * p.spin * 2 * math.pi)
		* quat.axis_angle(vec3.right, p.tilt)
	-- A flash: each crest of the wave passed (a quarter into each pulse), counted from the
	-- phase so that a slow frame can't step over one.
	while self.flashes < math.floor(self.phase + 0.75) do
		self.flashes += 1
		if self.flashes % math.max(1, p.every) == 0 then
			self:emit("flashed", self.flashes)
			if p.announce then
				print(`{self.entity.name} flashed {self.flashes} times`)
			end
		end
	end
end

return Beacon

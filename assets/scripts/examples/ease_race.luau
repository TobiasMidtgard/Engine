--!strict
-- Tweens, sequences and a spring side by side. Six runners race down their lanes and back,
-- each with its own ease (a sequence each: there, a pause, back, a pause, forever); the
-- glowing ring loops round a square by a sequence of its own, and the orb follows it on a
-- spring (calling `spring` again each frame gives it a new goal and keeps its speed). Change
-- `bounce` or `halflife` while it runs. The editor's Curves tab lists every tween running.
-- See "Tweens and timelines" in the scripts README.
local Race = {}
Race.props = {
	length = 6, -- of the lanes @range 2 20 @unit m
	secs = 1.6, -- for a runner to go down its lane @range 0.2 6 @unit s
	halflife = 0.35, -- the orb's spring: seconds to close half the gap @range 0.02 2 @unit s
	bounce = 0.4, -- 0: no overshoot, near 1: wobbles long @range 0 0.95
}

Race.eases = { "linear", "out_quad", "in_out_cubic", "out_back", "out_elastic", "out_bounce" }

local COLORS = {
	vec3(0.6, 0.65, 0.7),
	vec3(0.3, 0.7, 1),
	vec3(0.75, 0.45, 1),
	vec3(1, 0.75, 0.25),
	vec3(1, 0.4, 0.3),
	vec3(0.35, 1, 0.45),
}

local RAIL = { base_color = { 0.06, 0.065, 0.07, 1 }, metallic = 0.7, perceptual_roughness = 0.4 }

local function part(name: string, shape: any, at: Vec3, material: any): any
	return {
		name = name,
		components = {
			Transform = { translation = at },
			MeshRenderer = { mesh = { shape = shape }, material = { inline = material } },
		},
	}
end

local function glow(c: Vec3, strength: number): any
	return { base_color = { c.x * 0.2, c.y * 0.2, c.z * 0.2, 1 }, emissive = { c.x, c.y, c.z }, emissive_strength = strength }
end

function Race:start()
	local p = self.props
	self.runners = {}
	for k, name in Race.eases do
		local x = (k - (#Race.eases + 1) / 2) * 0.8
		self.entity:spawn(part("Lane", { Cuboid = { size = { 0.08, 0.04, p.length + 0.6 } } }, vec3(x, 0.02, p.length / 2), RAIL))
		local runner = self.entity:spawn(part(name, { RoundedCuboid = { size = { 0.36, 0.36, 0.36 }, radius = 0.08, segments = 0 } }, vec3(x, 0.25, 0), glow(COLORS[k], 6)))
		table.insert(self.runners, runner)
		-- There, a pause, back, a pause; forever.
		self:sequence({ loops = -1, label = name })
			:to(runner, { position = vec3(x, 0.25, p.length) }, p.secs, { ease = name })
			:interval(0.5)
			:to(runner, { position = vec3(x, 0.25, 0) }, p.secs, { ease = name })
			:interval(0.5)
	end
	-- The ring goes round a square beside the lanes; the orb springs after it.
	local cx = (#Race.eases / 2 + 1.6) * 0.8
	local corners = { vec3(cx, 1, 0.5), vec3(cx + 2.5, 1, 0.5), vec3(cx + 2.5, 1, p.length - 0.5), vec3(cx, 1, p.length - 0.5) }
	self.ring = self.entity:spawn(part("Ring", { Torus = { radius = 0.32, tube = 0.045, segments = 32 } }, corners[1], glow(vec3(0.3, 1, 0.9), 8)))
	local square = self:sequence({ loops = -1, label = "ring" })
	for k = 2, #corners + 1 do
		square:to(self.ring, { position = corners[(k - 1) % #corners + 1] }, 1.2, { ease = "in_out_sine" }):interval(0.4)
	end
	self.orb = self.entity:spawn(part("Orb", { Sphere = { radius = 0.2, segments = 24 } }, corners[1], glow(vec3(1, 0.85, 0.4), 10)))
end

function Race:update(dt: number)
	self:spring(self.orb, { position = self.ring.position }, { halflife = self.props.halflife, bounce = self.props.bounce })
end

return Race

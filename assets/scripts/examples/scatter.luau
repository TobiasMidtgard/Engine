--!strict
-- Stones scattered round the entity. They show while you tune the props (`construct` builds
-- them in the editor too, and again as a prop changes), and the buttons in its Details panel
-- work on them before Play: Bake makes them scene objects of their own (saved with the scene,
-- each one yours to move or delete), Reseed tries another scatter, Clear takes the baked
-- stones away. Each button is one step Ctrl+Z undoes. See "Tool buttons and tool scripts" in
-- the scripts README.
local Scatter = {}
Scatter.props = {
	count = 18, -- stones @range 1 120 @int
	radius = 3.5, -- how far out they lie @range 0.5 20 @unit m @handle
	size = 0.5, -- a stone's size, give or take a third @range 0.05 2 @unit m
	moss = 0.3, -- the share of mossy stones @range 0 1
	seed = 1, -- which scatter of many @int @min 1
	baked = false, -- the stones are scene objects now (Bake) @hide
}

local STONE = { base_color = { 0.42, 0.4, 0.37, 1 }, perceptual_roughness = 0.9 }
local MOSS = { base_color = { 0.2, 0.29, 0.12, 1 }, perceptual_roughness = 0.95 }

type Stone = { at: Vec3, size: Vec3, yaw: number, mossy: boolean }

-- Numbers in [0, 1) from a seed: the same seed gives the same scatter (a Park-Miller
-- generator; `math.random` would differ run to run).
local function numbers(seed: number): () -> number
	local state = (math.floor(seed) * 7919 + 17) % 2147483647
	return function(): number
		state = (state * 48271) % 2147483647
		return state / 2147483647
	end
end

-- The stones the props make, round the entity on the ground.
local function stones(props: any): { Stone }
	local rand = numbers(props.seed)
	local out: { Stone } = {}
	for _ = 1, props.count do
		-- Evenly over the disc (the square root keeps the middle from crowding).
		local r = props.radius * math.sqrt(rand())
		local a = rand() * 2 * math.pi
		local s = props.size * (0.67 + 0.66 * rand())
		local flat = 0.45 + 0.35 * rand()
		table.insert(out, {
			at = vec3(math.cos(a) * r, s * flat * 0.3, math.sin(a) * r),
			size = vec3(s * (0.8 + 0.4 * rand()), s * flat, s * (0.8 + 0.4 * rand())),
			yaw = rand() * 2 * math.pi,
			mossy = rand() < props.moss,
		})
	end
	return out
end

local function spec(i: number, stone: Stone): any
	return {
		name = `Stone {i}`,
		components = {
			Transform = { translation = stone.at, rotation = quat.euler(stone.yaw), scale = stone.size },
			MeshRenderer = {
				mesh = { shape = { Sphere = { radius = 0.5, segments = 9 } } },
				material = { inline = if stone.mossy then MOSS else STONE },
			},
		},
	}
end

-- The scatter, until it is baked.
function Scatter:construct()
	if self.props.baked then
		return
	end
	for i, stone in stones(self.props) do
		self.entity:spawn(spec(i, stone))
	end
end

-- Makes the stones scene objects of their own, saved with the scene; move or delete any of
-- them. @button Bake
function Scatter:bake()
	if self.props.baked then
		warn("Scatter: already baked (Clear first for a new scatter)")
		return
	end
	for i, stone in stones(self.props) do
		self.entity:spawn(spec(i, stone))
	end
	self.props.baked = true
end

-- Another scatter: the next seed. @button Reseed
function Scatter:reseed()
	self.props.seed += 1
end

-- Takes the baked stones away, back to a scatter you can tune. @button Clear
function Scatter:clear()
	if not self.props.baked then
		return
	end
	for _, child in self.entity:children() do
		if string.find(child.name or "", "^Stone %d") then
			child:despawn()
		end
	end
	self.props.baked = false
end

-- How far out it scatters.
function Scatter:draw_gizmos()
	draw.circle(vec3(0, 0.03, 0), self.props.radius, { color = "lime", alpha = 0.7 })
end

return Scatter

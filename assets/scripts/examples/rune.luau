--!strict
-- A rune stone for examples/seal.luau, which spawns three: press E on one and it tells its seal
-- (the entity it stands under), whose state machine has a region for each rune; it glows while
-- the seal's machine is in its `lit` state, so the seal's states are all it goes by. See
-- "State machines" in the scripts README.
local Rune = {}
Rune.props = {
	event = "touch_left", -- what it sends its seal when pressed
	lit = "left_lit", -- the seal's state that lights it
	floor = false, -- a plate set in the floor rather than a standing stone
	mark = 1, -- which glyph it bears @range 1 3
}

local STONE = { base_color = { 0.3, 0.29, 0.28, 1 }, perceptual_roughness = 0.85 }
local GLYPH = { base_color = { 0.02, 0.03, 0.04, 1 }, emissive = { 0.3, 0.8, 1 }, emissive_strength = 0.6 }
local LIT = 40 -- the glyph's glow, lit
local DARK = 0.6

local function part(name: string, shape: any, at: Vec3, material: any, rotation: Quat?): any
	return {
		name = name,
		components = {
			Transform = { translation = at, rotation = rotation or quat.identity },
			MeshRenderer = { mesh = { shape = shape }, material = { inline = material } },
		},
	}
end

local function box(x: number, y: number, z: number): any
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.2, segments = 0 } }
end

-- The glyph's strokes, on a face whose normal is +z (a standing stone) or +y (a floor plate):
-- a ring and, by `mark`, one bar, a cross or a chevron.
local function strokes(mark: number): { { number } }
	if mark == 2 then
		return { { 0, 0, 0 }, { 0, 0, math.pi / 2 } }
	elseif mark == 3 then
		return { { -0.05, 0.02, 0.5 }, { 0.05, 0.02, -0.5 } }
	end
	return { { 0, 0, 0 } }
end

function Rune:start()
	local floor = self.props.floor
	-- A standing stone faces +z; a plate in the floor faces up.
	local face = if floor then quat.identity else quat.euler(0, math.pi / 2)
	local center = if floor then vec3(0, 0.085, 0) else vec3(0, 1.0, 0.215)
	if floor then
		self.entity:spawn(part("Plate", { Cylinder = { radius = 0.62, height = 0.08, segments = 40 } }, vec3(0, 0.04, 0), STONE))
	else
		self.entity:spawn(part("Stone", box(0.62, 1.5, 0.42), vec3(0, 0.75, 0), STONE))
		self.entity:spawn(part("Cap", { Cone = { radius = 0.44, height = 0.32, segments = 4 } }, vec3(0, 1.66, 0), STONE, quat.euler(math.pi / 4)))
	end
	local size = if floor then 1.9 else 1
	local glyph: { any } = {
		part("Ring", { Torus = { radius = 0.17 * size, tube = 0.022 * size, segments = 32 } }, vec3.zero, GLYPH, face),
	}
	for _, s in strokes(self.props.mark) do
		-- In the face's plane: x across, y up the face (for a plate, z toward the seal).
		local along = vec3(s[1] * size, if floor then 0 else s[2] * size, if floor then -s[2] * size else 0)
		local turn = if floor then quat.euler(s[3]) else quat.euler(0, 0, s[3])
		table.insert(glyph, part("Stroke", box(0.03 * size, if floor then 0.02 else 0.3 * size, if floor then 0.3 * size else 0.02), along, GLYPH, turn))
	end
	self.glyph = self.entity:spawn({
		name = "Glyph",
		components = { Transform = { translation = center } },
		children = glyph,
	})
	self.light = self.entity:spawn({
		name = "Rune light",
		components = {
			Transform = { translation = center + (if floor then vec3(0, 0.5, 0) else vec3(0, 0, 0.5)) },
			PointLight = { color = { 0.3, 0.8, 1 }, intensity_lm = 0, range = 5, radius = 0.05 },
		},
	})
	self.level = 0 -- 0 dark, 1 lit
	self._shown = -1
end

-- Pressed: it tells the seal, whose machine decides.
function Rune:on_interact()
	local seal = self.entity.parent
	if seal then
		seal:send(self.props.event)
	end
end

function Rune:update(dt: number)
	local seal = self.entity.parent
	local machine = if seal then seal.script else nil
	local goal = if machine and machine:in_state(self.props.lit) then 1 else 0
	self.level += (goal - self.level) * math.min(1, dt * 5)
	if math.abs(goal - self.level) < 0.01 then
		self.level = goal
	end
	-- New materials only when the glow has changed enough to see.
	local shown = math.floor(self.level * 40 + 0.5)
	if shown == self._shown then
		return
	end
	self._shown = shown
	for _, stroke in self.glyph:children() do
		stroke:material({ emissive_strength = DARK + (LIT - DARK) * self.level })
	end
	self.light:set("PointLight", { intensity_lm = 900 * self.level })
end

return Rune

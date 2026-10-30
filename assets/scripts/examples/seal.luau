--!strict
-- A rune seal run by a state machine with final states: three rune stones stand before a
-- round stone door, and the door sinks into the ground once all three are lit, in any order.
-- Each rune is a region of the parallel group `sealed`, going from dark to lit, and lit is a
-- final state: `sealed`'s `done` follows once every region is in one (XState's onDone,
-- SCXML's done.state), so no code counts the runes. The door stays down `hold` seconds, rises
-- and the runes go dark again. Put it on an empty entity: it builds its arch, its door and its
-- three rune stones (`construct`, so the editor shows them too; each runs examples/rune.luau:
-- press E on one and it tells the seal). Selected, it shows where each rune's touch reaches
-- and which socket it lights; drag `spread` in the scene view. See "State machines" and "In the
-- editor's view" in the scripts README.
local Seal = {}
Seal.props = {
	hold = 6, -- how long the door stays down @range 1 30 @unit s
	open_secs = 2.2, -- to sink or rise all the way @range 0.3 6 @unit s
	spread = 2.4, -- how far the side runes stand from the middle @range 1.5 5 @unit m @handle x
}

Seal.states = {
	initial = "sealed",
	states = {
		sealed = {
			parallel = true, -- a region per rune, each lit on its own
			done = "opening", -- once every rune is lit
			states = {
				left = {
					initial = "left_dark",
					states = { left_dark = { on = { touch_left = "left_lit" } }, left_lit = { final = true } },
				},
				middle = {
					initial = "middle_dark",
					states = { middle_dark = { on = { touch_middle = "middle_lit" } }, middle_lit = { final = true } },
				},
				right = {
					initial = "right_dark",
					states = { right_dark = { on = { touch_right = "right_lit" } }, right_lit = { final = true } },
				},
			},
		},
		opening = { done = "open" },
		open = { after = { "hold", "closing" } },
		closing = { done = "sealed" },
	},
}

local RUNES = { "left", "middle", "right" }
local STONE = { base_color = { 0.34, 0.33, 0.31, 1 }, perceptual_roughness = 0.82 }
local DOOR = { base_color = { 0.16, 0.165, 0.18, 1 }, metallic = 0.1, perceptual_roughness = 0.6 }
local RIM = { base_color = { 0.02, 0.03, 0.04, 1 }, emissive = { 0.3, 0.8, 1 }, emissive_strength = 1 }
local SOCKET = { base_color = { 0.02, 0.03, 0.04, 1 }, emissive = { 0.3, 0.8, 1 }, emissive_strength = 0.3 }
local GOLD = { base_color = { 0.05, 0.04, 0.02, 1 }, emissive = { 1, 0.72, 0.32 }, emissive_strength = 0 }
local CYAN = vec3(0.3, 0.8, 1)
local AMBER = vec3(1, 0.6, 0.15)
local DOOR_Y = 1.45

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
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.18, segments = 0 } }
end

-- Whether rune `name`'s region is lit. The state's name is built here, so the type checker
-- (which knows the machine's state names) can't check it: `machine` is untyped for that.
local function is_lit(machine: any, name: string): boolean
	return machine:in_state(`{name}_lit`)
end

-- Where each rune stands (the side ones `spread` from the middle), and whether it is a plate in
-- the floor rather than a standing stone.
local function spots(spread: number): { [string]: { at: Vec3, floor: boolean } }
	return {
		left = { at = vec3(-spread, 0, 0.9), floor = false },
		middle = { at = vec3(0, 0, 2.3), floor = true },
		right = { at = vec3(spread, 0, 0.9), floor = false },
	}
end

-- Its parts (in the editor whenever its props change, and before `start` in play).
function Seal:construct()
	-- The arch (its front, +z, faces the way the entity does) and the alcove behind the door.
	for _, x in { -1.55, 1.55 } do
		self.entity:spawn(part("Pillar", box(0.55, 3.3, 0.75), vec3(x, 1.65, 0), STONE))
	end
	self.entity:spawn(part("Lintel", box(3.9, 0.55, 0.85), vec3(0, 3.55, 0), STONE))
	self.entity:spawn(part("Back wall", box(2.7, 3.3, 0.3), vec3(0, 1.65, -0.75), STONE))
	self.gem = self.entity:spawn(part("Keystone gem", { Sphere = { radius = 0.11, segments = 16 } }, vec3(0, 3.55, 0.45), RIM))
	-- What the door hides: a golden crystal and its light.
	self.crystal = self.entity:spawn(part("Sanctum crystal", { Sphere = { radius = 0.24, segments = 6 } }, vec3(0, 1.5, -0.38), GOLD))
	self.glow = self.entity:spawn({
		name = "Sanctum light",
		components = {
			Transform = { translation = vec3(0, 1.6, 0.2) },
			PointLight = { color = { 1, 0.72, 0.32 }, intensity_lm = 0, range = 9, radius = 0.2 },
		},
	})
	-- The door: a disc of stone with a glowing rim and a socket for each rune.
	local upright = quat.euler(0, math.pi / 2)
	local face: { any } = {
		part("Disc", { Cylinder = { radius = 1.2, height = 0.2, segments = 48 } }, vec3.zero, DOOR, upright),
		part("Rim", { Torus = { radius = 1.08, tube = 0.045, segments = 64 } }, vec3(0, 0, 0.1), RIM, upright),
		part("Boss", { Torus = { radius = 0.3, tube = 0.06, segments = 32 } }, vec3(0, 0, 0.1), DOOR, upright),
	}
	for k, name in RUNES do
		local a = math.rad(150 - 60 * (k - 1)) -- left, top, right
		local at = vec3(math.cos(a) * 0.68, math.sin(a) * 0.68, 0.12)
		table.insert(face, part(`Socket {name}`, { Sphere = { radius = 0.1, segments = 12 } }, at, SOCKET))
	end
	self.door = self.entity:spawn({
		name = "Door",
		components = { Transform = { translation = vec3(0, DOOR_Y, 0.05) } },
		children = face,
	})
	self.rim = self.door:find("Rim") :: Entity
	self.sockets = {} :: { [string]: Entity }
	for _, name in RUNES do
		self.sockets[name] = self.door:find(`Socket {name}`) :: Entity
	end
	-- The rune stones: two standing at the sides, one set in the floor before the door.
	local where = spots(self.props.spread)
	for k, name in RUNES do
		local spot = where[name]
		local floor = spot.floor
		self.entity:spawn({
			name = `{string.upper(string.sub(name, 1, 1))}{string.sub(name, 2)} rune`,
			components = {
				Transform = { translation = spot.at },
				Interactable = { verb = "Touch", reach_m = 2.5, offset = { 0, if floor then 0.2 else 1, 0 } },
				Script = {
					path = "examples/rune.luau",
					props = { event = `touch_{name}`, lit = `{name}_lit`, floor = floor, mark = k },
				},
			},
		})
	end
end

function Seal:start()
	self.sink = 0 -- 0: the door up, 1: down in the ground
	self.runes_lit = 0
	self._shown = -1 -- what the glow shows (it changes materials only when it changes)
end

-- The door at `sink` (0 up, 1 down), with the light behind it.
function Seal:place(sink: number)
	self.sink = sink
	local shake = if sink > 0 and sink < 1 then 0.012 * math.sin(sink * 90) else 0
	self.door.position = vec3(shake, DOOR_Y - 2.95 * sink, 0.05)
	self.glow:set("PointLight", { intensity_lm = 2600 * sink })
	self.crystal:material({ emissive_strength = 30 * sink })
end

function Seal:move_to(goal: number)
	local from = self.sink
	local secs = math.abs(goal - from) * self.props.open_secs
	if secs > 0 then
		task.tween(secs, function(a: number)
			self:place(from + (goal - from) * a)
		end, ease.in_out)
	end
end

-- The sockets on the door and its rim show which runes the seal has (its regions' states).
function Seal:update(_dt: number)
	local lit = 0
	for _, name in RUNES do
		if is_lit(self, name) then
			lit += 1
		end
	end
	local moving = self:in_state("opening") or self:in_state("closing")
	local key = lit + (if moving then 10 elseif self:in_state("open") then 20 else 0)
	-- The story's fact: the keeper's conversation and its quest follow it.
	if lit ~= self.runes_lit then
		facts.runes_lit = lit
	end
	self.runes_lit = lit
	if key == self._shown then
		return
	end
	self._shown = key
	for _, name in RUNES do
		local on = is_lit(self, name) or self:in_state("opening") or self:in_state("open")
		self.sockets[name]:material({ emissive = CYAN, emissive_strength = if on then 45 else 0.3 })
	end
	local rim = if self:in_state("sealed") then 1 + 9 * lit else 40
	self.rim:material({ emissive_strength = rim })
	self.gem:material({ emissive = if moving then AMBER else CYAN, emissive_strength = if moving then 40 else 2 + 8 * lit })
end

-- Selected: how far each rune's touch reaches, and the socket on the door it lights (cyan once
-- lit), in the seal's own space.
function Seal:draw_gizmos_selected()
	local where = spots(self.props.spread)
	local door = DOOR_Y - 2.95 * (self.sink or 0) -- it sinks while open
	for k, name in RUNES do
		local spot = where[name]
		local color = if is_lit(self, name) then "cyan" else "white"
		draw.circle(spot.at + vec3(0, 0.03, 0), 2.5, { color = color, alpha = 0.5 })
		local a = math.rad(150 - 60 * (k - 1))
		local socket = vec3(math.cos(a) * 0.68, door + math.sin(a) * 0.68, 0.2)
		local top = spot.at + vec3(0, if spot.floor then 0.2 else 1.75, 0)
		draw.arrow(top, socket, { color = color, on_top = true })
	end
end

-- The states' own work: the door moves; the runes come from the regions.

function Seal:enter_opening()
	facts.seal_opened = true
	self:move_to(1)
end

function Seal:enter_closing()
	self:move_to(0)
end

return Seal

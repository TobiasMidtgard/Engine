--!strict
-- A portcullis run by a state machine: press E on it to raise or lower it; it comes down by
-- itself after `hold` seconds, goes back up if someone stands under it while it closes, and
-- stops where it is when its power is cut (send it "power", or wire a lever's signal to
-- "power"); with the power back it carries on from where it stopped. Put it on an empty entity
-- with an Interactable: it builds its posts and its bars. The states table is the whole logic;
-- the methods below only move the bars and light the lamp. See "State machines" in the
-- scripts README.
local Gate = {}
Gate.props = {
	width = 2.6, -- between the posts @range 1 6 @unit m
	height = 2.6, -- of the opening @range 1.5 5 @unit m
	lift_secs = 1.6, -- to raise or lower it all the way @range 0.2 6 @unit s
	hold = 4, -- how long it stays up @range 0.5 20 @unit s
	blocker = "Player", -- who it won't close on @entity
}

Gate.states = {
	initial = "powered",
	states = {
		powered = {
			initial = "closed",
			history = true, -- power back: on from the state it was in
			on = { power = "unpowered" },
			states = {
				closed = { on = { interact = "opening" } },
				opening = { done = "open", on = { interact = "closing" } },
				open = { after = { "hold", "closing" }, on = { interact = "closing" } },
				closing = { done = "closed", on = { interact = "opening" }, when = { blocked = "opening" } },
			},
		},
		unpowered = { on = { power = "powered" } },
	},
}

local IRON = { base_color = { 0.05, 0.048, 0.046, 1 }, metallic = 0.85, perceptual_roughness = 0.45 }
local STONE = { preset = "Marble" }
local LAMP = { base_color = { 0, 0, 0, 1 }, emissive = { 1, 0.2, 0.1 }, emissive_strength = 0 }
local COLORS = {
	closed = vec3(1, 0.12, 0.06),
	opening = vec3(1, 0.62, 0.1),
	open = vec3(0.2, 1, 0.3),
	closing = vec3(1, 0.62, 0.1),
	unpowered = vec3(0, 0, 0),
}

local function part(name: string, shape: any, at: Vec3, material: any): any
	return {
		name = name,
		components = {
			Transform = { translation = at },
			MeshRenderer = { mesh = { shape = shape }, material = { inline = material } },
		},
	}
end

local function box(x: number, y: number, z: number): any
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.2, segments = 0 } }
end

function Gate:start()
	local p = self.props
	local w, h = p.width, p.height
	self.entity:spawn(part("Left post", box(0.4, h + 0.5, 0.5), vec3(-w / 2 - 0.2, (h + 0.5) / 2, 0), STONE))
	self.entity:spawn(part("Right post", box(0.4, h + 0.5, 0.5), vec3(w / 2 + 0.2, (h + 0.5) / 2, 0), STONE))
	self.entity:spawn(part("Lintel", box(w + 1.2, 0.4, 0.6), vec3(0, h + 0.7, 0), STONE))
	local bars: { any } = {}
	local count = math.max(3, math.floor(w / 0.32))
	for k = 0, count do
		local x = -w / 2 + 0.12 + (w - 0.24) * k / count
		table.insert(bars, part("Bar", box(0.06, h, 0.06), vec3(x, h / 2, 0), IRON))
	end
	for _, y in { 0.25, h / 2, h - 0.2 } do
		table.insert(bars, part("Rail", box(w - 0.1, 0.06, 0.08), vec3(0, y, 0.02), IRON))
	end
	self.bars = self.entity:spawn({ name = "Portcullis", components = { Transform = {} }, children = bars })
	self.lamp = self.entity:spawn(part("Lamp", { Sphere = { radius = 0.11, segments = 16 } }, vec3(0, h + 1.02, 0.18), LAMP))
	self.light = self.entity:spawn({
		name = "Lamp light",
		components = {
			Transform = { translation = vec3(0, h + 1.02, 0.4) },
			PointLight = { color = { 1, 0.2, 0.1 }, intensity_lm = 0, range = 8, radius = 0.1 },
		},
	})
	self.lift = 0 -- 0 down, 1 up
	self.blink = 0
end

-- The bars at `lift` (0 down, 1 up).
function Gate:place(lift: number)
	self.lift = lift
	self.bars.position = vec3(0, lift * (self.props.height - 0.35), 0)
end

-- From where the bars are to `goal`, at the gate's speed.
function Gate:move_to(goal: number)
	local from = self.lift
	local secs = math.abs(goal - from) * self.props.lift_secs
	if secs > 0 then
		task.tween(secs, function(a: number)
			self:place(from + (goal - from) * a)
		end, ease.in_out)
	end
end

function Gate:shine(color: Vec3, level: number)
	self.lamp:material({ emissive = color, emissive_strength = 40 * level })
	self.light:set("PointLight", { color = color, intensity_lm = 600 * level })
end

-- Someone under it: the blocker within a meter of the opening.
function Gate:blocked(): boolean
	local player = world.find(self.props.blocker)
	if not player then
		return false
	end
	local d = player.position - self.entity.position
	return math.abs(d.x) < self.props.width / 2 and math.abs(d.z) < 1
end

-- The states' own work: enter_<state> runs when it starts (it may wait: `done` follows when it
-- returns), update_<state> every frame, exit_<state> when it ends.

function Gate:enter_closed()
	self:shine(COLORS.closed, 1)
end

function Gate:enter_opening()
	self:move_to(1)
end

function Gate:enter_open()
	self:shine(COLORS.open, 1)
end

function Gate:enter_closing()
	self:move_to(0)
end

-- While it moves the lamp blinks amber.
function Gate:update_powered(dt: number)
	if self:in_state("opening") or self:in_state("closing") then
		self.blink += dt
		self:shine(COLORS.opening, if self.blink % 0.5 < 0.25 then 1 else 0.15)
	end
end

function Gate:enter_unpowered()
	self:shine(COLORS.unpowered, 0)
end

return Gate

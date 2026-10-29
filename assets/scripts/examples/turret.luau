--!strict
-- A sentry turret run by a state machine with two regions at once (`parallel = true`): its aim
-- (sweeping, or turned on whoever comes near) and its gun (ready, firing, cooling down) each
-- keep a state of their own, side by side, and both leave together when its power is cut (send
-- it "power"; again to switch it back on, each region from its first state). Put it on an empty
-- entity: it builds its base, head and barrels (`construct`, so they show in the editor too),
-- and selected it shows its range, which you can drag, and how far it sweeps. See "State
-- machines" and "In the editor's view" in the scripts README.
local Turret = {}
Turret.props = {
	target = "Player", -- who it watches for @entity
	range = 6, -- how near they must come @range 1 20 @unit m @handle
	sweep = 50, -- how far it looks either way while scanning @range 0 170 @unit deg
	turn_rate = 150, -- how fast it turns @range 20 720 @unit deg/s
	cooldown = 0.7, -- between shots @range 0.1 5 @unit s
}

Turret.states = {
	initial = "active",
	states = {
		active = {
			parallel = true, -- its aim and its gun run side by side
			on = { power = "off" },
			states = {
				aim = {
					initial = "scanning",
					states = {
						scanning = { when = { sees = "tracking" } },
						tracking = { when = { ["not sees"] = "scanning" } },
					},
				},
				gun = {
					initial = "ready",
					states = {
						ready = { when = { locked = "firing" } },
						firing = { done = "cooling" },
						cooling = { after = { "cooldown", "ready" } },
					},
				},
			},
		},
		off = { on = { power = "active" } },
	},
}

local IRON = { base_color = { 0.06, 0.062, 0.066, 1 }, metallic = 0.8, perceptual_roughness = 0.4 }
local PAINT = { base_color = { 0.42, 0.45, 0.36, 1 }, metallic = 0.2, perceptual_roughness = 0.55 }
local GLOW = { base_color = { 0, 0, 0, 1 }, emissive = { 1, 1, 1 }, emissive_strength = 0 }
local FIRE = { base_color = { 0, 0, 0, 1 }, emissive = { 1, 0.7, 0.3 }, emissive_strength = 80 }
local EYE = {
	scanning = vec3(0.2, 1, 0.35),
	tracking = vec3(1, 0.15, 0.08),
}

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
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.25, segments = 0 } }
end

-- The direction of a turn of the head (0: the turret's forward, -z), flat.
local function heading(yaw: number): Vec3
	return vec3(-math.sin(yaw), 0, -math.cos(yaw))
end

-- An angle in -pi..pi.
local function wrap(a: number): number
	return (a + math.pi) % (2 * math.pi) - math.pi
end

-- Its parts (in the editor whenever its props change, and before `start` in play).
function Turret:construct()
	local along = quat.euler(0, math.pi / 2) -- a cylinder lying along the head's forward
	self.entity:spawn(part("Base", { Cylinder = { radius = 0.34, height = 0.7, segments = 24 } }, vec3(0, 0.35, 0), IRON))
	local barrels: { any } = {}
	for _, x in { -0.1, 0.1 } do
		table.insert(barrels, part("Barrel", { Cylinder = { radius = 0.045, height = 0.62, segments = 12 } }, vec3(x, 0, -0.5), IRON, along))
	end
	local head = {
		part("Body", box(0.56, 0.34, 0.62), vec3(0, 0, 0), PAINT),
		part("Eye", { Sphere = { radius = 0.055, segments = 12 } }, vec3(0, 0.08, -0.31), GLOW),
		{ name = "Barrels", components = { Transform = {} }, children = barrels },
	}
	self.head = self.entity:spawn({ name = "Head", components = { Transform = { translation = vec3(0, 0.9, 0) } }, children = head })
	self.barrels = self.head:find("Barrels")
	self.eye = self.head:find("Eye")
	self.light = self.head:spawn({
		name = "Muzzle light",
		components = {
			Transform = { translation = vec3(0, 0, -1) },
			PointLight = { color = { 1, 0.7, 0.3 }, intensity_lm = 0, range = 6, radius = 0.05 },
		},
	})
end

function Turret:start()
	self.flash = nil :: Entity?
	self.yaw = 0 -- the head's turn, radians (0: the turret's forward)
	self.sweep_t = 0
	self.shots = 0
end

-- Where the target is, as a turn of the head (radians), and how far.
function Turret:bearing(): (number, number)
	local who = world.find(self.props.target)
	if not who then
		return 0, math.huge
	end
	local d = self.entity.rotation:inverse():rotate(who.position - self.entity.position)
	return math.atan2(-d.x, -d.z), math.sqrt(d.x * d.x + d.z * d.z)
end

-- Turns the head toward `goal` at its turn rate.
function Turret:turn(goal: number, dt: number)
	local step = math.rad(self.props.turn_rate) * dt
	self.yaw = wrap(self.yaw + math.clamp(wrap(goal - self.yaw), -step, step))
	self.head.rotation = quat.euler(self.yaw)
end

-- Someone within range.
function Turret:sees(): boolean
	local _, far = self:bearing()
	return far < self.props.range
end

-- Turned on them, near enough to hit.
function Turret:locked(): boolean
	local goal = self:bearing()
	return self:in_state("tracking") and math.abs(wrap(goal - self.yaw)) < math.rad(4)
end

-- The aim region: sweeps while nobody is near, follows them once someone is.

function Turret:enter_scanning()
	self.eye:material({ emissive = EYE.scanning, emissive_strength = 30 })
end

function Turret:update_scanning(dt: number)
	self.sweep_t += dt
	self:turn(math.rad(self.props.sweep) * math.sin(self.sweep_t * 0.9), dt)
end

function Turret:enter_tracking()
	self.eye:material({ emissive = EYE.tracking, emissive_strength = 40 })
end

function Turret:update_tracking(dt: number)
	self:turn((self:bearing()), dt)
end

-- The gun region: fires once it is locked on, then cools down.

function Turret:enter_firing()
	self.shots += 1
	self.flash = self.head:spawn(part("Flash", { Sphere = { radius = 0.12, segments = 12 } }, vec3(0, 0, -0.86), FIRE))
	self.light:set("PointLight", { intensity_lm = 4000 })
	self.barrels.position = vec3(0, 0, 0.09) -- the kick
	task.wait(0.08)
	self:douse()
	task.tween(0.25, function(a: number)
		self.barrels.position = vec3(0, 0, 0.09 * (1 - a))
	end)
end

-- The flash out (also when the power is cut mid-shot: leaving a state stops its enter task).
function Turret:douse()
	if self.flash then
		self.flash:despawn()
		self.flash = nil
	end
	self.light:set("PointLight", { intensity_lm = 0 })
end

function Turret:exit_firing()
	self:douse()
end

-- Gizmos, in the turret's own space: what it covers, and while selected where it aims.

function Turret:draw_gizmos()
	local tracking = self:in_state("tracking")
	local color = if tracking then "red" elseif self:in_state("off") then "gray" else "orange"
	local range, half = self.props.range, math.rad(self.props.sweep)
	local ground = vec3(0, 0.05, 0)
	draw.circle(ground, range, { color = color, alpha = 0.6 })
	-- The wedge its scan sweeps.
	draw.line(ground, ground + heading(-half) * range, color)
	draw.line(ground, ground + heading(half) * range, color)
	draw.arc(ground, range, heading(-half), 2 * half, color)
end

function Turret:draw_gizmos_selected()
	-- Not running in the editor's edit mode: it looks straight ahead.
	local yaw = self.yaw or 0
	local eye = vec3(0, 0.98, 0)
	local color = if self:in_state("tracking") then "red" else "lime"
	draw.arrow(eye, eye + heading(yaw) * self.props.range, { color = color, on_top = true })
end

-- Off: both regions stop where they are, the eye dark.
function Turret:enter_off()
	self.eye:material({ emissive_strength = 0 })
end

return Turret

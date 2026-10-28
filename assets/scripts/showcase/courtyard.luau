--!strict
-- The Script courtyard's director: it builds the lantern ring and the orrery, runs them
-- every frame, and plays the ritual when the lever is pulled. Edit this file while the
-- game runs and the courtyard changes within a quarter second; the board shows the edit.

local Courtyard = {}

Courtyard.props = {
	lanterns = 12, -- round the ring
	radius = 11.5, -- meters from the middle
	color = vec3(1.0, 0.56, 0.22), -- the lanterns' light, linear RGB
	glow = 2600, -- lumens at the crest of the wave
	wave_speed = 0.11, -- turns of the wave per second
	spin = 0.05, -- the orrery's turns per second
	wisps = 320, -- in the swarm round the orrery (wisp.luau, run on every core)
}

function Courtyard:start()
	self.t = 0
	self.spin = self.props.spin
	self.flare = 0 -- extra glow while the ritual runs
	self:build()
end

function Courtyard:update(dt: number)
	self.t += dt
	local p = self.props
	for i, lamp in self.lanterns do
		local phase = (i - 1) / #self.lanterns - self.t * p.wave_speed
		local crest = (0.5 + 0.5 * math.cos(phase * 2 * math.pi)) ^ 6
		local glow = lamp.dim * (0.45 + 0.55 * crest + self.flare)
		lamp.bulb:material({ emissive = p.color, emissive_strength = 10 * glow })
		lamp.light:set("PointLight", { color = p.color, intensity_lm = p.glow * glow })
	end
	for i, ring in self.rings do
		ring:rotate_y(dt * self.spin * 2 * math.pi * (1 + 0.7 * i))
	end
end

-- After an edit: rebuild only when the lantern ring itself changed.
function Courtyard:on_reload()
	local p = self.props
	if #self.lanterns ~= p.lanterns or self.built_radius ~= p.radius or #self.wisps ~= p.wisps then
		self:build()
	end
	self.spin = p.spin
end

-- Pulling the lever sends "ritual" (lever.luau). One task, read top to bottom.
function Courtyard:ritual()
	if self.busy then
		return
	end
	self.busy = true
	if ui then -- the UI module may be left out of a game
		ui.notify({ title = "The courtyard stirs", text = "courtyard.luau: Courtyard:ritual()" })
	end
	for _, lamp in self.lanterns do -- out, one by one
		task.spawn(task.tween, 0.35, function(a)
			lamp.dim = 1 - a
		end)
		task.wait(0.08)
	end
	task.wait(0.6)
	for _, wisp in self.wisps do -- the swarm rises (wisp.luau, Wisp:ritual)
		wisp:send("ritual")
	end
	for i, lamp in self.lanterns do -- back in sequence, each with a flash
		vfx.spawn({ effect = "spell.charge", position = lamp.bulb.world_position, scale = 0.6 })
		task.spawn(task.tween, 0.5, function(a)
			lamp.dim = a
		end, ease.out_back)
		task.wait(0.12)
	end
	for i = 1, 4 do
		local brazier = world.find(`Brazier {i}`)
		if brazier then
			local top = brazier.world_position + vec3(0, 1.1, 0)
			vfx.spawn({ effect = "fire.torch", position = top, scale = 1.6, once = false, duration = 14 })
			vfx.spawn({ effect = "spell.fireball_impact", position = top, scale = 0.5 })
		end
	end
	task.tween(2.2, function(a) -- the orrery spins up and the lanterns flare
		self.spin = self.props.spin + 1.4 * a
		self.flare = 1.5 * a
	end, ease.in_quad)
	local core = self.core.world_position
	vfx.beam({ from = core, to = core + vec3(0, 60, 0), color = { 1.0, 0.7, 0.35 },
		width = 0.6, style = "energy", duration = 3 })
	vfx.spawn({ effect = "spell.starburst", position = core, scale = 0.7 })
	time.dilate({ key = "courtyard", scale = 0.3, hold_secs = 1.2 })
	task.wait(3)
	task.tween(4, function(a) -- and settles
		self.spin = self.props.spin + 1.4 * (1 - a)
		self.flare = 1.5 * (1 - a)
	end, ease.smooth)
	self.busy = false
end

-- Building: plain tables in the scene format, spawned as this entity's children.

local IRON = { base_color = { 0.05, 0.048, 0.046, 1 }, metallic = 0.85, perceptual_roughness = 0.5 }
local BRASS = { base_color = { 0.78, 0.56, 0.26, 1 }, metallic = 1, perceptual_roughness = 0.28 }
local GLASS = { base_color = { 1, 0.95, 0.85, 1 }, perceptual_roughness = 0.15 }
local MARBLE = { preset = "Marble" }

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
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.3, segments = 0 } }
end

local function lantern(i: number, at: Vec3): any
	local bars = {}
	for k = 0, 3 do
		local corner = vec3.from_angle(k * math.pi / 2 + math.pi / 4, 0.17)
		table.insert(bars, part("Bar", box(0.025, 0.4, 0.025), corner + vec3(0, 2.8, 0), IRON))
	end
	local spec: any = {
		name = `Lantern {i}`,
		components = { Transform = { translation = at } },
		children = {
			part("Post", { Cylinder = { radius = 0.055, height = 2.6, segments = 16 } }, vec3(0, 1.3, 0), IRON),
			part("Foot", { Cylinder = { radius = 0.16, height = 0.18, segments = 16 } }, vec3(0, 0.09, 0), IRON),
			part("Floor", box(0.32, 0.035, 0.32), vec3(0, 2.6, 0), IRON),
			part("Roof", { Cone = { radius = 0.27, height = 0.22, segments = 4 } }, vec3(0, 3.1, 0), IRON,
				quat.euler(math.pi / 4)),
			part("Bulb", { Sphere = { radius = 0.12, segments = 20 } }, vec3(0, 2.78, 0), GLASS),
			{
				name = "Light",
				components = {
					Transform = { translation = vec3(0, 2.78, 0) },
					PointLight = { color = { 1, 0.6, 0.3 }, intensity_lm = 0, range = 14, radius = 0.1 },
				},
			},
		},
	}
	for _, bar in bars do
		table.insert(spec.children, bar)
	end
	return spec
end

local RINGS = { { 1.2, 0.35, 0.1 }, { 1.7, -0.5, 1.2 }, { 2.3, 0.2, 2.3 } } -- radius, tilt, yaw
local CORE = 2.6 -- the core's height
local PLANETS = { { 0.35, 0.55, 0.9, 1 }, { 0.9, 0.42, 0.25, 1 }, { 0.55, 0.8, 0.45, 1 } }

local function orrery(): any
	local spec: any = {
		name = "Orrery",
		components = { Transform = {} },
		children = {
			part("Pedestal", { Cylinder = { radius = 0.5, height = 1.2, segments = 32 } }, vec3(0, 0.6, 0), MARBLE),
			part("Pedestal cap", { Cylinder = { radius = 0.64, height = 0.08, segments = 32 } }, vec3(0, 1.24, 0), BRASS),
			part("Stem", { Cylinder = { radius = 0.045, height = 0.95, segments = 12 } }, vec3(0, 1.75, 0), BRASS),
			part("Core", { Sphere = { radius = 0.42, segments = 32 } }, vec3(0, CORE, 0), {
				base_color = { 1, 0.85, 0.6, 1 },
				emissive = { 1, 0.62, 0.3 },
				emissive_strength = 60,
			}),
			{
				name = "Core light",
				components = {
					Transform = { translation = vec3(0, CORE, 0) },
					PointLight = { color = { 1, 0.7, 0.4 }, intensity_lm = 3000, range = 16, radius = 0.4 },
				},
			},
		},
	}
	for k, r in RINGS do
		local radius, tilt, yaw = r[1], r[2], r[3]
		local ring: any = part(`Ring {k}`, { Torus = { radius = radius, tube = 0.03, segments = 64 } }, vec3.zero, BRASS)
		ring.children = {
			part("Planet", { Sphere = { radius = 0.08 + 0.04 * k, segments = 16 } }, vec3(radius, 0, 0), {
				base_color = PLANETS[k],
				perceptual_roughness = 0.5,
			}),
		}
		table.insert(spec.children, {
			name = `Orbit {k}`,
			components = { Transform = { translation = vec3(0, CORE, 0), rotation = quat.euler(yaw, tilt) } },
			children = { ring },
		})
	end
	return spec
end

local WISP_GLOW = { { 1, 0.62, 0.3 }, { 1, 0.82, 0.5 }, { 0.55, 0.75, 1 } }

local function wisp(i: number): any
	local u = (i * 0.618034) % 1 -- spread evenly in radius and height
	local v = (i * 0.754878) % 1
	local radius, height = 3.2 + 6.3 * u, 1.3 + 4.2 * v
	local angle = i * 2.399963
	return {
		name = `Wisp {i}`,
		components = {
			Transform = { translation = vec3(math.cos(angle) * radius, height, math.sin(angle) * radius) },
			MeshRenderer = {
				mesh = { shape = { Sphere = { radius = 0.075, segments = 10 } } },
				material = { inline = {
					base_color = { 0, 0, 0, 1 },
					emissive = WISP_GLOW[i % 3 + 1],
					emissive_strength = 90,
				} },
			},
			Script = { path = "showcase/wisp.luau", props = { seed = i, radius = radius, height = height } },
		},
	}
end

function Courtyard:build()
	for _, lamp in self.lanterns or {} do
		lamp.post:despawn()
	end
	for _, w in self.wisps or {} do
		w:despawn()
	end
	if self.orrery then
		self.orrery:despawn()
	end
	local p = self.props
	self.lanterns = {}
	for i = 1, p.lanterns do
		local at = vec3.from_angle((i - 1) / p.lanterns * 2 * math.pi, p.radius)
		local post = self:spawn(lantern(i, at))
		table.insert(self.lanterns, { post = post, bulb = post:find("Bulb"), light = post:find("Light"), dim = 1 })
	end
	self.built_radius = p.radius
	self.orrery = self:spawn(orrery())
	self.core = self.orrery:find("Core")
	self.rings = {}
	for k = 1, #RINGS do
		table.insert(self.rings, self.orrery:find(`Ring {k}`))
	end
	self.wisps = {}
	for i = 1, p.wisps do
		table.insert(self.wisps, self:spawn(wisp(i)))
	end
end

return Courtyard

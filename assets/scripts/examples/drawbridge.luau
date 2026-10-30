--!strict
-- A drawbridge on a timeline: press E on it (or send it "toggle") to lower or raise it. Its
-- timeline `lower` swings the deck down along curves/drawbridge.curve (slow to start, a heavy
-- landing with a small bounce) while the winch wheels turn; events at times on it clank the
-- winch and thud the deck down. Raising plays it backward from wherever it is, so a bridge
-- pressed halfway turns round without a jump. The warning lamps blink by a tween while it
-- moves. Put it on an empty entity with an Interactable: it builds its towers, deck and chains.
-- Edit the curve in the editor's Curves tab while it plays. See "Tweens and timelines" in the
-- scripts README.
local Bridge = {}
Bridge.props = {
	span = 5, -- the deck's length @range 2 12 @unit m
	width = 2.4, -- the deck's width @range 1 6 @unit m
	speed = 1, -- how fast it lowers and rises @range 0.2 4
	auto = 0, -- lowers and rises by itself, waiting this long between (0: only when pressed) @range 0 30 @unit s
}

Bridge.timelines = {
	lower = {
		tracks = {
			-- 0 up to 1 down, drawn in the Curves tab.
			swing = "curves/drawbridge",
			-- The winch wheels' turns, as keys here.
			winch = { { 0, 0 }, { 0.3, 0.08 }, { 1.9, 1 } },
		},
		events = { clank = { 0.05, 1.1 }, thud = 1.9 },
	},
}

-- The deck leans back this far when raised.
local UP = -1.42

local STONE = { preset = "Marble" }
local WOOD = { base_color = { 0.36, 0.22, 0.12, 1 }, perceptual_roughness = 0.8 }
local IRON = { base_color = { 0.05, 0.048, 0.046, 1 }, metallic = 0.85, perceptual_roughness = 0.45 }
local LAMP = { base_color = { 0, 0, 0, 1 }, emissive = { 1, 0.45, 0.08 }, emissive_strength = 2 }

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
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.15, segments = 0 } }
end

function Bridge:start()
	local p = self.props
	local w, span = p.width, p.span
	local tower_x = w / 2 + 0.5
	for _, side in { -1, 1 } do
		self.entity:spawn(part("Tower", box(0.8, 3.8, 0.9), vec3(side * tower_x, 1.9, -0.55), STONE))
		self.entity:spawn(part("Cap", box(1.0, 0.3, 1.1), vec3(side * tower_x, 3.95, -0.55), STONE))
	end
	self.entity:spawn(part("Lintel", box(tower_x * 2 + 0.6, 0.45, 0.6), vec3(0, 3.6, -0.55), STONE))
	-- The far bank the deck lands on.
	self.entity:spawn(part("Landing", box(w + 1.2, 0.5, 1.4), vec3(0, 0.2, span + 0.55), STONE))
	self.entity:spawn(part("Near bank", box(w + 2.4, 0.5, 1.4), vec3(0, 0.2, -0.75), STONE))
	-- The deck turns about its hinge at the near edge.
	local planks: { any } = {}
	local count = math.floor(span / 0.5)
	for k = 0, count - 1 do
		table.insert(planks, part("Plank", box(w, 0.14, 0.46), vec3(0, 0, 0.25 + k * span / count), WOOD))
	end
	for _, x in { -w / 2 + 0.1, w / 2 - 0.1 } do
		table.insert(planks, part("Beam", box(0.14, 0.2, span), vec3(x, -0.1, span / 2), IRON))
	end
	self.deck = self.entity:spawn({
		name = "Deck",
		components = { Transform = { translation = vec3(0, 0.5, 0) } },
		children = planks,
	})
	-- Winch wheels on the towers, and the chains from them to the deck's far corners.
	self.wheels = {}
	self.chains = {}
	for _, side in { -1, 1 } do
		table.insert(
			self.wheels,
			self.entity:spawn({
				name = "Winch",
				components = { Transform = { translation = vec3(side * (tower_x - 0.45), 3.2, 0.0) } },
				children = {
					part("Wheel", { Cylinder = { radius = 0.32, height = 0.12, segments = 20 } }, vec3(0, 0, 0), IRON),
					part("Spoke", box(0.6, 0.06, 0.06), vec3(0, 0.07, 0), WOOD),
					part("Spoke", box(0.06, 0.06, 0.6), vec3(0, 0.07, 0), WOOD),
				},
			})
		)
		table.insert(
			self.chains,
			self.entity:spawn(part("Chain", { Cylinder = { radius = 0.035, height = 1, segments = 8 } }, vec3(0, 0, 0), IRON))
		)
	end
	-- Warning lamps on the caps.
	self.lamps = {}
	for _, side in { -1, 1 } do
		table.insert(
			self.lamps,
			self.entity:spawn(part("Lamp", { Sphere = { radius = 0.13, segments = 16 } }, vec3(side * tower_x, 4.25, -0.2), LAMP))
		)
	end
	self.clanks = 0
	self.thuds = 0
	self.down = false
	self:place(0, 0)
	if p.auto > 0 then
		task.spawn(function()
			while true do
				task.wait(p.auto)
				self:toggle()
				self:timeline("lower"):wait()
			end
		end)
	end
end

-- The deck at `swing` (0 up, 1 down), the wheels turned `winch` of the way.
function Bridge:place(swing: number, winch: number)
	self.swing = swing
	self.deck.rotation = quat.axis_angle(vec3(1, 0, 0), UP * (1 - swing))
	for _, wheel in self.wheels do
		wheel.rotation = quat.axis_angle(vec3(1, 0, 0), winch * math.pi * 6)
	end
	-- Each chain from its wheel to the deck's far corner, taut.
	local w, span = self.props.width, self.props.span
	local deck_at = self.deck.position
	local turn = self.deck.rotation
	for k, chain in self.chains do
		local side = if k == 1 then -1 else 1
		local from = self.wheels[k].position
		local to = deck_at + turn:rotate(vec3(side * (w / 2 - 0.1), 0.05, span - 0.2))
		local d = to - from
		chain.position = from + d * 0.5
		chain.rotation = quat.between(vec3(0, 1, 0), d)
		chain.scale = vec3(1, d:length(), 1)
	end
end

-- The timeline's tracks each frame it plays.
function Bridge:on_lower(v: { [string]: number })
	self:place(v.swing, v.winch)
end

-- Events on the timeline: the winch catches, the deck lands.
function Bridge:clank()
	self.clanks += 1
	print(`clank ({self.clanks})`)
end

function Bridge:thud(tl: Timeline)
	if tl.direction > 0 then
		self.thuds += 1
		print("thud")
	end
end

-- At either end: the lamps stop blinking.
function Bridge:on_lower_done()
	for _, lamp in self.lamps do
		tween.kill(lamp)
		self:tween(lamp, { ["material.emissive_strength"] = 2 }, 0.3)
	end
end

-- The lamps blink while it moves.
function Bridge:blink()
	for _, lamp in self.lamps do
		self:tween(lamp, { ["material.emissive_strength"] = 60 }, 0.25, { loops = -1, yoyo = true, ease = "in_out_sine" })
	end
end

-- Down if it was going up (or is up), up if it was going down. A message "toggle" calls it
-- too.
function Bridge:toggle()
	local tl = self:timeline("lower"):set_rate(self.props.speed)
	self.down = not self.down
	if self.down then
		tl:play()
	else
		tl:reverse()
	end
	self:blink()
end

-- Up at once, the lamps out (the tour's stops start from here).
function Bridge:raise_now()
	self:timeline("lower"):stop():seek(0)
	self.down = false
	for _, lamp in self.lamps do
		tween.kill(lamp)
		lamp:material({ emissive_strength = 2 })
	end
end

function Bridge:on_interact(by: Entity)
	self:toggle()
end

return Bridge

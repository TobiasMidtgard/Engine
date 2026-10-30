--!strict
-- The captain of the gate watch and his company, mustered in front of the courtyard gate.
-- Press E (Talk) and he asks who will speak for the company (dialogue/muster.yarn); you
-- answer by walking: hold your place in the rank, step past him to the front, lean in to his
-- ear, or slip behind him. The places are StorySpot entities in the scene, his children so they
-- move with him (in his space, facing +z toward the rank): "rank" in the gap of the rank
-- (0, 0, 3.2), "forward" past him on his left (1.45, 0, 0.15), "ear" at his right shoulder
-- (-0.62, 0, -0.18), "behind" at his back (0.1, 0, -1.35). The conversation's options name them
-- with #spot:<name>; the Story tab shows them from above, to drag where they belong. While you
-- choose, the view rises over the yard, the world slows and a ring glows at each spot;
-- `on_spot_near` lets him react as you come close. Its commands (<<call_to_order>>,
-- <<step_aside>>, <<nod>>, <<turn_around>>) are this script's methods. See "Story" in the
-- scripts README.
local Captain = {}
Captain.props = {
	conversation = "dialogue/muster", -- the .yarn file he talks from
	soldiers = 4, -- in the rank facing him, two either side of the gap @range 0 8
	spacing = 0.95, -- between them, meters @range 0.6 2
	rank = 3.2, -- from him to the rank, meters @range 2 6
	recruit = false, -- a recruit in the gap (who walks for the tour; leave it to the player)
	chooser = "", -- who walks to the spots (an entity's name); the player when empty
}

local STEEL = { base_color = { 0.56, 0.58, 0.62, 1 }, metallic = 1, perceptual_roughness = 0.32 }
local DARK_STEEL = { base_color = { 0.2, 0.21, 0.23, 1 }, metallic = 1, perceptual_roughness = 0.45 }
local LEATHER = { base_color = { 0.16, 0.1, 0.06, 1 }, perceptual_roughness = 0.75 }
local SKIN = { base_color = { 0.62, 0.44, 0.33, 1 }, perceptual_roughness = 0.6 }
local WOOD = { base_color = { 0.3, 0.2, 0.11, 1 }, perceptual_roughness = 0.7 }
local CRIMSON = { base_color = { 0.42, 0.04, 0.05, 1 }, perceptual_roughness = 0.85 }
local GOLD = { base_color = { 0.83, 0.62, 0.26, 1 }, metallic = 1, perceptual_roughness = 0.28 }
local TABARDS = {
	{ base_color = { 0.09, 0.16, 0.32, 1 }, perceptual_roughness = 0.9 }, -- the watch's blue
	{ base_color = { 0.08, 0.14, 0.28, 1 }, perceptual_roughness = 0.9 },
}
local RECRUIT = { base_color = { 0.17, 0.24, 0.12, 1 }, perceptual_roughness = 0.9 } -- still in green
local EYE = { base_color = { 0.05, 0.04, 0.04, 1 }, perceptual_roughness = 0.3 }

local function part(name: string, shape: any, at: Vec3, material: any, rotation: Quat?, scale: Vec3?): any
	return {
		name = name,
		components = {
			Transform = { translation = at, rotation = rotation or quat.identity, scale = scale or vec3.one },
			MeshRenderer = { mesh = { shape = shape }, material = { inline = material } },
		},
	}
end

local function box(x: number, y: number, z: number): any
	return { RoundedCuboid = { size = { x, y, z }, radius = math.min(x, y, z) * 0.35, segments = 0 } }
end

-- A head under a kettle helmet, turning about the neck (a conversation's watchers turn the
-- child named Head to follow whoever is choosing).
local function head(extra: { any }?): any
	local children = {
		part("Face", { Sphere = { radius = 0.11, segments = 20 } }, vec3(0, 0.08, 0.01), SKIN, nil, vec3(1, 1.12, 1)),
		part("Nose", { Cone = { radius = 0.018, height = 0.05, segments = 8 } }, vec3(0, 0.07, 0.125), SKIN, quat.euler(0, math.pi / 2, 0)),
		part("Left eye", { Sphere = { radius = 0.012, segments = 8 } }, vec3(-0.04, 0.1, 0.105), EYE),
		part("Right eye", { Sphere = { radius = 0.012, segments = 8 } }, vec3(0.04, 0.1, 0.105), EYE),
		part("Helmet", { Sphere = { radius = 0.13, segments = 20 } }, vec3(0, 0.14, 0), STEEL, nil, vec3(1, 0.72, 1.04)),
		part("Brim", { Torus = { radius = 0.17, tube = 0.016, segments = 32 } }, vec3(0, 0.12, 0), STEEL),
	}
	for _, x in extra or {} do
		table.insert(children, x)
	end
	return {
		name = "Head",
		components = { Transform = { translation = vec3(0, 1.52, 0) } },
		children = children,
	}
end

-- A figure facing +z: legs, a tabard over mail, arms, a head under a kettle helmet.
local function figure(name: string, at: Vec3, yaw: number, tabard: any, extra: { any }, crest: { any }?): any
	local children = {
		part("Left leg", { Capsule = { radius = 0.075, height = 0.78, segments = 12 } }, vec3(-0.1, 0.42, 0), LEATHER),
		part("Right leg", { Capsule = { radius = 0.075, height = 0.78, segments = 12 } }, vec3(0.1, 0.42, 0), LEATHER),
		part("Mail", box(0.42, 0.62, 0.24), vec3(0, 1.16, 0), DARK_STEEL),
		part("Tabard", box(0.4, 0.78, 0.27), vec3(0, 1.02, 0.005), tabard),
		part("Belt", box(0.44, 0.06, 0.29), vec3(0, 0.9, 0), LEATHER),
		part("Left arm", { Capsule = { radius = 0.058, height = 0.56, segments = 12 } }, vec3(-0.27, 1.14, 0.02), DARK_STEEL, quat.euler(0, 0.12, -0.12)),
		part("Right arm", { Capsule = { radius = 0.058, height = 0.56, segments = 12 } }, vec3(0.27, 1.14, 0.02), DARK_STEEL, quat.euler(0, 0.12, 0.12)),
		part("Neck", { Cylinder = { radius = 0.05, height = 0.1, segments = 12 } }, vec3(0, 1.5, 0), SKIN),
		head(crest),
	}
	for _, x in extra do
		table.insert(children, x)
	end
	return {
		name = name,
		components = { Transform = { translation = at, rotation = quat.euler(yaw, 0, 0) } },
		children = children,
	}
end

-- A soldier's spear and shield.
local function kit(): { any }
	return {
		part("Spear", { Cylinder = { radius = 0.017, height = 2.2, segments = 10 } }, vec3(0.33, 1.08, 0.08), WOOD),
		part("Spearhead", { Cone = { radius = 0.032, height = 0.2, segments = 4 } }, vec3(0.33, 2.28, 0.08), STEEL),
		part("Shield", { Cylinder = { radius = 0.3, height = 0.035, segments = 32 } }, vec3(-0.34, 1.02, 0.1), WOOD, quat.euler(0, math.pi / 2, 0)),
		part("Shield rim", { Torus = { radius = 0.3, tube = 0.016, segments = 32 } }, vec3(-0.34, 1.02, 0.1), DARK_STEEL, quat.euler(0, math.pi / 2, 0)),
		part("Boss", { Sphere = { radius = 0.06, segments = 12 } }, vec3(-0.34, 1.02, 0.13), STEEL, nil, vec3(1, 1, 0.6)),
	}
end

-- His figure, his rank and the recruit (in the editor too; the spots are the scene's).
function Captain:construct()
	local p = self.props
	self.body = self.entity:spawn(figure("Body", vec3.zero, 0, CRIMSON, {
		part("Breastplate", box(0.44, 0.42, 0.29), vec3(0, 1.2, 0.01), STEEL),
		part("Left pauldron", { Sphere = { radius = 0.1, segments = 16 } }, vec3(-0.25, 1.42, 0), STEEL, nil, vec3(1.1, 0.7, 1)),
		part("Right pauldron", { Sphere = { radius = 0.1, segments = 16 } }, vec3(0.25, 1.42, 0), STEEL, nil, vec3(1.1, 0.7, 1)),
		part("Cape", box(0.52, 1.12, 0.035), vec3(0, 0.98, -0.17), CRIMSON, quat.euler(0, -0.07, 0)),
		part("Gorget", { Torus = { radius = 0.075, tube = 0.02, segments = 24 } }, vec3(0, 1.49, 0), GOLD),
		part("Scabbard", { Cylinder = { radius = 0.025, height = 0.8, segments = 10 } }, vec3(-0.24, 0.72, 0.05), LEATHER, quat.euler(0, 0.25, 0.18)),
		part("Hilt", { Cylinder = { radius = 0.018, height = 0.2, segments = 10 } }, vec3(-0.3, 1.18, 0.16), GOLD, quat.euler(0, 0.25, 0.18)),
	}, {
		part("Plume", { Cone = { radius = 0.045, height = 0.32, segments = 12 } }, vec3(0, 0.34, -0.04), CRIMSON, quat.euler(0, -0.35, 0)),
	}))
	-- The rank faces him, a gap in the middle (the recruit's place, or yours).
	self.soldiers = {}
	local n = math.floor(p.soldiers)
	for i = 1, n do
		local k = i - (n + 1) / 2
		local side = if k < 0 then math.floor(k) else math.ceil(k)
		local x = side * p.spacing
		local s = self.entity:spawn(figure(`Soldier {i}`, vec3(x, 0, p.rank), math.pi, TABARDS[i % 2 + 1], kit()))
		table.insert(self.soldiers, s)
	end
	if p.recruit then
		self.entity:spawn(figure("Recruit", vec3(0, 0, p.rank), math.pi, RECRUIT, {}))
	end
end

-- Pressed (Talk): the muster's conversation, unless one is going on already.
function Captain:on_interact()
	self:talk()
end

function Captain:talk()
	if self.conversation and not self.conversation.done then
		return
	end
	self:reset()
	self.conversation = dialogue.start(self.props.conversation, "Start", {
		owner = self.entity,
		chooser = if self.props.chooser ~= "" then self.props.chooser else nil,
		speakers = {
			Captain = { title = "Captain of the gate watch", portrait = "portrait:pilot", color = "#ff8a6b" },
		},
	})
end

-- Back as he was before a conversation moved him.
function Captain:reset()
	self.body.position = vec3.zero
	self.body.rotation = quat.identity
end

-- As you step into a spot he notices; what this returns is his new line over his head.
function Captain:on_spot_near(name: string): string?
	self.noticed = name
	if name == "forward" then
		return "Careful, recruit. That is my place."
	elseif name == "ear" then
		return "Something to say? Quietly, then."
	elseif name == "behind" then
		return "I can hear you breathing back there."
	end
	return nil
end

-- <<call_to_order>>: the rank snaps straight, each soldier a beat after the last.
function Captain:call_to_order()
	for i, s in self.soldiers do
		task.delay(0.08 * i, function()
			self:tween(s, { position = s.position + vec3(0, 0.035, 0) }, 0.12, { loops = 2, yoyo = true, ease = "out_quad" })
		end)
	end
end

-- <<step_aside>>: he gives up the front, a step to his right and back; the talk waits.
function Captain:step_aside(): () -> boolean
	local t = self:tween(self.body, { position = vec3(-0.75, 0, -0.35) }, 0.9, { ease = "in_out_sine" })
	return function()
		return t.done
	end
end

-- <<nod>>: a small nod, as to a quiet word.
function Captain:nod()
	self:tween(self.body, { rotation = quat.euler(0, 0.12, 0) }, 0.4, { loops = 2, yoyo = true, ease = "in_out_sine" })
end

-- <<turn_around>>: he turns to face whoever is behind him; the talk waits.
function Captain:turn_around(): () -> boolean
	local t = self:tween(self.body, { rotation = quat.euler(math.pi, 0, 0) }, 0.7, { ease = "out_back" })
	return function()
		return t.done
	end
end

return Captain

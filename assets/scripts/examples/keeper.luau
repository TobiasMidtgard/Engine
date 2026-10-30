--!strict
-- The keeper of the rune seal: a robed figure with a crystal-topped staff who talks. Press E
-- (Talk) and its conversation (dialogue/keeper.yarn) opens in the conversation screen; the
-- commands in it that aren't built in are this script's methods (<<bow>>, <<raise_staff>>).
-- The quest it gives (quests/sealed_bridge.quest) moves on by itself: the seal's script sets
-- the facts $runes_lit and $seal_opened, and the quest's objectives are conditions on them.
-- It builds itself in `construct`, so the editor shows it before Play. See "Story" in the
-- scripts README.
local Keeper = {}
Keeper.props = {
	conversation = "dialogue/keeper", -- the .yarn file it talks from
	node = "Start", -- the node its conversations start at
	glow = 6, -- the staff crystal's glow at rest @range 0 40
}

local ROBE = { base_color = { 0.085, 0.1, 0.14, 1 }, perceptual_roughness = 0.95 }
local MANTLE = { base_color = { 0.16, 0.12, 0.09, 1 }, perceptual_roughness = 0.9 }
local SHADOW = { base_color = { 0.01, 0.01, 0.012, 1 }, perceptual_roughness = 1 }
local BRASS = { base_color = { 0.78, 0.56, 0.26, 1 }, metallic = 1, perceptual_roughness = 0.3 }
local WOOD = { base_color = { 0.22, 0.14, 0.08, 1 }, perceptual_roughness = 0.7 }
local CRYSTAL = { base_color = { 0.02, 0.03, 0.04, 1 }, emissive = { 0.3, 0.8, 1 }, emissive_strength = 6 }
local EYES = { base_color = { 0, 0, 0, 1 }, emissive = { 0.4, 0.85, 1 }, emissive_strength = 12 }

local function part(name: string, shape: any, at: Vec3, material: any, rotation: Quat?, scale: Vec3?): any
	return {
		name = name,
		components = {
			Transform = { translation = at, rotation = rotation or quat.identity, scale = scale or vec3.one },
			MeshRenderer = { mesh = { shape = shape }, material = { inline = material } },
		},
	}
end

-- Its figure (in the editor too): a robe and mantle, a deep hood with two points of light in
-- it, and the staff in its right hand. It faces its +z.
function Keeper:construct()
	self.body = self.entity:spawn({
		name = "Body",
		components = { Transform = {} },
		children = {
			part("Robe", { Cone = { radius = 0.5, height = 1.62, segments = 40 } }, vec3(0, 0.81, 0), ROBE),
			part("Hem", { Torus = { radius = 0.48, tube = 0.022, segments = 48 } }, vec3(0, 0.05, 0), BRASS),
			part("Mantle", { Cone = { radius = 0.36, height = 0.55, segments = 32 } }, vec3(0, 1.3, 0), MANTLE),
			part("Clasp", { Sphere = { radius = 0.035, segments = 12 } }, vec3(0, 1.36, 0.17), BRASS),
			part("Hood", { Sphere = { radius = 0.2, segments = 24 } }, vec3(0, 1.66, -0.02), MANTLE, nil, vec3(1, 1.18, 1.1)),
			part("Face", { Sphere = { radius = 0.14, segments = 20 } }, vec3(0, 1.63, 0.08), SHADOW),
			part("Eye", { Sphere = { radius = 0.013, segments = 8 } }, vec3(-0.045, 1.66, 0.205), EYES),
			part("Eye", { Sphere = { radius = 0.013, segments = 8 } }, vec3(0.045, 1.66, 0.205), EYES),
			-- Sleeves from the shoulders: the left hangs in front, the right reaches the staff.
			part("Left sleeve", { Capsule = { radius = 0.07, height = 0.42, segments = 12 } }, vec3(-0.16, 1.15, 0.09), MANTLE, quat.euler(0, -0.42, 0.2)),
			part("Right sleeve", { Capsule = { radius = 0.07, height = 0.42, segments = 12 } }, vec3(0.28, 1.17, 0.1), MANTLE, quat.euler(0, -0.5, 0.42)),
		},
	})
	self.staff = self.entity:spawn({
		name = "Staff",
		components = { Transform = { translation = vec3(0.36, 0, 0.2) } },
		children = {
			part("Shaft", { Cylinder = { radius = 0.022, height = 1.95, segments = 12 } }, vec3(0, 0.975, 0), WOOD),
			part("Collar", { Torus = { radius = 0.06, tube = 0.012, segments = 24 } }, vec3(0, 1.9, 0), BRASS),
			part("Crystal", { Sphere = { radius = 0.065, segments = 6 } }, vec3(0, 2.0, 0), CRYSTAL, nil, vec3(1, 1.5, 1)),
		},
	})
	self.crystal = self.staff:find("Crystal") :: Entity
	self.light = self.staff:spawn({
		name = "Staff light",
		components = {
			Transform = { translation = vec3(0, 2.0, 0) },
			PointLight = { color = { 0.3, 0.8, 1 }, intensity_lm = 160, range = 4, radius = 0.06 },
		},
	})
end

function Keeper:start()
	self.t = 0
	self.flare = 0 -- the staff's flare, 1 at its brightest
	self._shown = -1
end

-- Pressed (Talk): its conversation starts, unless one is going on already.
function Keeper:on_interact()
	self:talk()
end

-- Also from `script.send {"event": "talk"}`.
function Keeper:talk()
	if self.conversation and not self.conversation.done then
		return
	end
	self.conversation = dialogue.start(self.props.conversation, self.props.node, {
		-- Its commands run here, also when another script (a test, a trigger) calls talk().
		owner = self.entity,
		speakers = {
			Keeper = { title = "Keeper of the rune seal", portrait = "portrait:merchant", color = "#7fd4ff" },
		},
	})
end

-- <<bow>>: a slow nod of the whole figure, down and back up.
function Keeper:bow()
	self:tween(self.body, { rotation = quat.euler(0, 0.12, 0) }, 0.55, { loops = 2, yoyo = true, ease = "in_out_sine" })
end

-- <<raise_staff>>: the staff lifts and its crystal flares; the conversation waits for it.
function Keeper:raise_staff(): () -> boolean
	local done = false
	task.spawn(function()
		task.tween(0.5, function(a: number)
			self.staff.position = vec3(0.36, 0.25 * a, 0.2)
			self.flare = a
		end, ease.out_back)
		task.wait(0.6)
		task.tween(1.2, function(a: number)
			self.staff.position = vec3(0.36, 0.25 * (1 - a), 0.2)
			self.flare = 1 - a
		end, ease.in_out)
		done = true
	end)
	return function()
		return done
	end
end

-- Breathing, and the crystal's glow (new materials only when it changes enough to see).
function Keeper:update(dt: number)
	self.t += dt
	self.body.position = vec3(0, 0.008 * math.sin(self.t * 1.4), 0)
	local glow = self.props.glow * (1 + 0.15 * math.sin(self.t * 2.3)) + 54 * self.flare
	local shown = math.floor(glow * 2 + 0.5)
	if shown == self._shown then
		return
	end
	self._shown = shown
	self.crystal:material({ emissive_strength = glow })
	self.light:set("PointLight", { intensity_lm = 160 + 2400 * self.flare })
end

return Keeper

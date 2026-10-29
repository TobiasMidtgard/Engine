--!strict
-- A lamp that breathes and flickers like a gas flame. Put it on an entity with a PointLight
-- (the editor's starter courtyard has one: "Lamp"). Press E on it to put it out and light it.
local Lamp = {}
Lamp.props = {
	lumens = 1800, -- at full brightness @range 0 5000 @unit lm
	color = vec3(1, 0.72, 0.42),
	period = 2.4, -- seconds a breath @range 0.3 8 @unit s
	flicker = 0.18, -- how much the flame jitters @range 0 1
}

function Lamp:start()
	self.t = 0
	self.jitter = 0
	self.boost = 0
	self.lit = true
	print(`{self.entity.name} burns at {self.props.lumens} lm`)
	self:every(0.08, "shake")
end

function Lamp:shake()
	self.jitter = (math.random() - 0.5) * self.props.flicker
end

function Lamp:update(dt: number)
	self.t += dt
	local p = self.props
	local breath = 0.5 + 0.5 * math.sin(self.t * 2 * math.pi / p.period)
	self.boost = math.max(0, self.boost - dt * 2.5)
	local glow = if self.lit then 0.55 + 0.45 * breath + self.jitter + self.boost else 0
	self.entity:set("PointLight", { color = p.color, intensity_lm = p.lumens * glow })
	self.entity:material({ emissive = p.color, emissive_strength = 2000 * glow })
end

-- A bright burst that fades: wire a signal to it (a beacon's flash, a lever's pull).
function Lamp:flare(strength: number?)
	self.boost = math.max(self.boost, strength or 1)
end

function Lamp:on_interact(by)
	self.lit = not self.lit
	print(`{self.entity.name} {if self.lit then "lit" else "put out"}`)
end

return Lamp

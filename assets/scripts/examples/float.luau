--!strict
-- A stone that floats above the ground, bobbing and turning slowly. Its curve comes from a
-- module shared by the examples (lib/wave.luau).
local wave = require("./lib/wave")

local Float = {}
Float.props = { height = 0.9, bob = 0.25, speed = 0.6, spin = 25 } -- spin: degrees a second

function Float:start()
	self.home = self.entity.position
	self.t = 0
	self:every(2, "report")
end

function Float:update(dt: number)
	self.t += dt
	local p = self.props
	local lift = p.height + wave.bob(self.t, p.speed) * p.bob
	self.entity.position = self.home + vec3.up * lift
	self.entity:rotate_y(math.rad(p.spin) * dt)
end

function Float:report()
	print(`{self.entity.name} floats at {string.format("%.2f", self.entity.position.y)} m`)
end

return Float

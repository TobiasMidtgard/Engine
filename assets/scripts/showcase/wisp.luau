--!strict
-- One wisp of the courtyard swarm (the director spawns a few hundred, courtyard.luau).
-- `parallel_update` runs on the actor VMs, side by side on every core: each wisp reads the
-- world as the frame began and moves only itself, so it needs no locks and the swarm moves the
-- same however many cores share it. The director's ritual sends "ritual" and they rise.

local Wisp = {}

Wisp.props = {
	seed = 1, -- which wisp: where it starts on its orbit and how it drifts
	radius = 5, -- meters from the orrery's axis
	height = 3, -- meters above the floor
}

function Wisp:start()
	local s = self.props.seed
	self.angle = s * 2.399963 -- the golden angle: wisps spread evenly round the ring
	self.rate = (0.16 + 0.1 * math.sin(s * 12.9898)) * (if s % 3 == 0 then -1 else 1)
	self.t = s * 0.7
	self.rise = 0
	self.velocity = vec3.zero
end

function Wisp:parallel_update(dt: number)
	local p = self.props
	self.angle += self.rate * dt
	self.t += dt
	-- The ritual (rise 0 to 1) draws the swarm in round the beam and up it.
	local r = (p.radius + 0.5 * math.sin(self.t * 0.6 + p.seed)) * (1 - 0.55 * self.rise)
	local h = p.height + 0.4 * math.sin(self.t * 1.1 + p.seed * 0.3) + 6 * self.rise
	local target = vec3(math.cos(self.angle) * r, h, math.sin(self.angle) * r)
	local at = self.entity.position
	-- A spring toward the orbit with some drag: wisps lag and overshoot like moths at a lamp.
	self.velocity = self.velocity * (1 - 2.5 * dt) + (target - at) * (6 * dt)
	self.entity.position = at + self.velocity * dt
end

-- Sent by the director when the lever is pulled: up round the beam, then back down.
function Wisp:ritual()
	task.wait(self.props.seed % 40 * 0.02)
	task.tween(2.5, function(a)
		self.rise = a
	end, ease.in_out)
	task.wait(3)
	task.tween(4, function(a)
		self.rise = 1 - a
	end, ease.smooth)
end

return Wisp

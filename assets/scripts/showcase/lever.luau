--!strict
-- A lever: pressing E by it swings the arm over and fires `pulled`. What a pull does is wired
-- per lever in the editor's Details panel (Signals: an object and a method of its script), so
-- one script serves every lever.

local Lever = {}

Lever.props = {
	swing = 1.1, -- how far the arm swings @angle @range 0 3.14
	cooldown = 12, -- before it can be pulled again @min 0 @unit s
}

Lever.signals = {
	pulled = "by: Entity", -- the arm went over; who pulled it
	reset = "", -- the arm is back up and it can be pulled again
}

function Lever:start()
	-- The part that swings (a lever built without one swings whole).
	self.arm = self.entity:find("Lever arm") or self.entity
	self.rest = self.arm.rotation
end

function Lever:on_interact(by: Entity)
	if self.pulling then
		return
	end
	self.pulling = true
	self.entity:set("Interactable", { enabled = false })
	task.tween(0.35, function(a)
		self.arm.rotation = quat.axis_angle(vec3.right, self.props.swing * a) * self.rest
	end, ease.out_back)
	vfx.spawn({ effect = "impact.sparks", position = self.entity.world_position, scale = 0.4 })
	self:emit("pulled", by)
	task.wait(self.props.cooldown)
	task.tween(1.2, function(a)
		self.arm.rotation = quat.axis_angle(vec3.right, self.props.swing * (1 - a)) * self.rest
	end, ease.smooth)
	self.entity:set("Interactable", { enabled = true })
	self.pulling = false
	self:emit("reset")
end

return Lever

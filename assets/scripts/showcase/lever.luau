--!strict
-- A lever: pressing E by it swings the arm over and sends an event to another entity's
-- script. Props say which entity and which event, so one script serves every lever.

local Lever = {}

Lever.props = {
	-- @group What it sends
	target = "Courtyard director", -- the entity that hears it @entity
	event = "ritual", -- the message its script gets
	-- @group The arm
	swing = 1.1, -- how far the arm swings @angle @range 0 3.14
	cooldown = 12, -- before it can be pulled again @min 0 @unit s
}

function Lever:start()
	self.arm = self.entity:find("Lever arm")
	self.rest = self.arm.rotation
end

function Lever:on_interact(by: Entity)
	if self.pulling then
		return
	end
	self.pulling = true
	self.entity:set("Interactable", { enabled = false })
	local target = world.find(self.props.target)
	task.tween(0.35, function(a)
		self.arm.rotation = quat.axis_angle(vec3.right, self.props.swing * a) * self.rest
	end, ease.out_back)
	vfx.spawn({ effect = "impact.sparks", position = self.entity.world_position, scale = 0.4 })
	if target then
		target:send(self.props.event, { by = by })
	else
		warn(`no entity named "{self.props.target}" to send "{self.props.event}" to`)
	end
	task.wait(self.props.cooldown)
	task.tween(1.2, function(a)
		self.arm.rotation = quat.axis_angle(vec3.right, self.props.swing * (1 - a)) * self.rest
	end, ease.smooth)
	self.entity:set("Interactable", { enabled = true })
	self.pulling = false
end

return Lever

--!strict
-- Gives each selected object its own turn about the up axis and a size within a tenth of its
-- own, so copies of one stone, crate or tree stop looking alike.
local Tool = {}
Tool.title = "Turn and size randomly"

function Tool.run(selection: { Entity })
	for _, e in selection do
		e.rotation = quat.axis_angle(vec3.up, math.random() * 2 * math.pi) * e.rotation
		e.scale = e.scale * (0.9 + 0.2 * math.random())
	end
end

return Tool

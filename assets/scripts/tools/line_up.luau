--!strict
-- Lines the selected objects up, evenly spaced from the first you picked to the last, each
-- keeping its height.
local Tool = {}
Tool.title = "Line up"

function Tool.run(selection: { Entity })
	if #selection < 3 then
		warn("Line up: select three or more objects")
		return
	end
	local first, last = selection[1].position, selection[#selection].position
	for i, e in selection do
		local p = first:lerp(last, (i - 1) / (#selection - 1))
		e.position = vec3(p.x, e.position.y, p.z)
	end
end

return Tool

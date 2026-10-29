--!strict
-- Curves shared by the example scripts (a module: `require("./lib/wave")`).
local wave = {}

-- A smooth bob from -1 to 1, `speed` times a second.
function wave.bob(t: number, speed: number): number
	return math.sin(t * speed * 2 * math.pi)
end

-- From 0 to 1 and back, `speed` times a second.
function wave.pulse(t: number, speed: number): number
	return 0.5 - 0.5 * math.cos(t * speed * 2 * math.pi)
end

return wave

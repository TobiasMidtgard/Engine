--!strict
-- Tests for lamp.luau. They run in the live game (the editor's Tests panel, or script.test):
-- each one spawns a lamp of its own, presses E on it or calls it, waits frames or seconds and
-- checks what its script did. What a test spawns is gone after it.
local LAMP = {
	name = "Test lamp",
	components = {
		Transform = { translation = { 2, 1.5, 4 } },
		PointLight = { intensity_lm = 0 },
		MeshRenderer = { mesh = { shape = { Sphere = { radius = 0.15, segments = 0 } } } },
		Script = { path = "examples/lamp.luau" },
	},
}

local function light(lamp: Entity): number
	return lamp:get("PointLight").intensity_lm
end

describe("Lamp", function()
	it("starts lit and glowing", function(t)
		local lamp = t:spawn(LAMP)
		t:frames(2)
		expect(lamp.script.lit).to_be(true)
		expect(light(lamp)).to_be_greater_than(0)
	end)

	it("goes out when pressed, and lights again", function(t)
		local lamp = t:spawn(LAMP)
		t:press(lamp)
		expect(lamp.script.lit).to_be(false)
		t:frames(1)
		expect(light(lamp)).to_be(0)
		t:press(lamp)
		expect(lamp.script.lit).to_be(true)
	end)

	it("flares brighter, then settles back", function(t)
		local lamp = t:spawn(LAMP)
		t:frames(1)
		local before = light(lamp)
		lamp.script:flare(1)
		t:frames(1)
		local bright = light(lamp)
		expect(bright).to_be_greater_than(before * 1.4)
		t:wait(0.6)
		expect(light(lamp)).to_be_less_than(bright)
	end)

	it("breathes: dimmer and brighter over one period", function(t)
		local lamp = t:spawn(LAMP)
		local low, high = math.huge, 0
		t:wait_until(function()
			local now = light(lamp)
			low, high = math.min(low, now), math.max(high, now)
			return lamp.script.t >= lamp.script.props.period
		end)
		expect(low).to_be_less_than(high * 0.8)
	end)
end)

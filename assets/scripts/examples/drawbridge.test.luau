--!strict
-- Tests for drawbridge.luau's timeline: lowering along its curve with the events on the way,
-- and turning round halfway without a jump. (Tests run at 4 frames a second.)
local BRIDGE = {
	name = "Test bridge",
	components = {
		Transform = { translation = { 6, 0, -8 } },
		Interactable = {},
		Script = { path = "examples/drawbridge.luau", props = { speed = 2 } },
	},
}

local SLOW = {
	name = "Test bridge",
	components = {
		Transform = { translation = { 6, 0, -8 } },
		Interactable = {},
		Script = { path = "examples/drawbridge.luau", props = { speed = 0.5 } },
	},
}

local function playing(b: Entity): boolean
	return b.script:timeline("lower").playing
end

describe("Drawbridge", function()
	it("starts raised", function(t)
		local b = t:spawn(BRIDGE)
		expect(b.script.swing).to_be(0)
		expect(playing(b)).to_be(false)
	end)

	it("lowers when pressed, with two clanks and a thud", function(t)
		local b = t:spawn(BRIDGE)
		t:press(b)
		expect(playing(b)).to_be(true)
		t:wait_until(function()
			return not playing(b)
		end, 3)
		expect(b.script.swing).to_be_near(1)
		expect(b.script.clanks).to_be(2)
		expect(b.script.thuds).to_be(1)
	end)

	it("turns round halfway without a jump", function(t)
		local b = t:spawn(SLOW)
		t:press(b)
		t:wait(2)
		local was = b.script.swing
		expect(was).to_be_greater_than(0.2)
		t:press(b)
		t:wait(0.25)
		-- A frame on, backward: a little higher, not snapped up or down.
		expect(math.abs(b.script.swing - was)).to_be_less_than(0.25)
		t:wait_until(function()
			return not playing(b)
		end, 6)
		expect(b.script.swing).to_be_near(0)
		expect(b.script.thuds).to_be(0)
	end)
end)

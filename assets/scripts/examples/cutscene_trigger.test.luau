--!strict
-- Tests for cutscene_trigger.luau playing cutscenes/lift.cutscene: the platform rises on its
-- curve, the events reach the script, a second press skips to the end, and restore puts it
-- all back. (Tests run at 4 frames a second.)
local function stage(props: any): any
	return {
		name = "Test lift",
		components = {
			Transform = { translation = { -6, 0, 8 } },
			Interactable = {},
			Script = { path = "examples/cutscene_trigger.luau", props = props },
		},
		children = {
			{ name = "Platform", components = { Transform = {} } },
			{ name = "Gear", components = { Transform = { translation = { 1, 1, 0 } } } },
			{ name = "Beacon", components = { Transform = { translation = { 0, 4, 0 } } } },
		},
	}
end

describe("Cutscene trigger", function()
	it("plays when pressed: the platform rises and the events reach the script", function(t)
		local s = t:spawn(stage({}))
		local platform = s:find("Platform") :: Entity
		local finished = 0
		s:on("finished", function()
			finished += 1
		end)
		t:press(s)
		t:wait(1)
		expect(platform.position.y - s.position.y).to_be_greater_than(0.3)
		expect(platform.position.y - s.position.y).to_be_less_than(2.9)
		expect(s.script.cutscene.playing).to_be(true)
		t:wait_until(function()
			return s.script.cutscene.done
		end, 4)
		t:wait(0.5)
		expect(platform.position.y - s.position.y).to_be_near(3)
		expect(s.script.arrivals).to_be(1)
		expect(s.script.skipped).to_be(false)
		expect(finished).to_be(1)
	end)

	it("skips to the end on a second press, still sending its retro event", function(t)
		local s = t:spawn(stage({}))
		local platform = s:find("Platform") :: Entity
		t:press(s)
		t:wait(0.25)
		t:press(s)
		t:wait(0.5)
		expect(s.script.cutscene.done).to_be(true)
		expect(s.script.skipped).to_be(true)
		expect(platform.position.y - s.position.y).to_be_near(3)
		expect(s.script.arrivals).to_be(1)
	end)

	it("puts everything back when it restores", function(t)
		local s = t:spawn(stage({ restore = true }))
		local platform = s:find("Platform") :: Entity
		t:press(s)
		t:wait(1)
		expect(platform.position.y - s.position.y).to_be_greater_than(0.3)
		t:wait_until(function()
			return s.script.cutscene.done
		end, 4)
		t:wait(0.25)
		expect(platform.position.y - s.position.y).to_be_near(0)
	end)
end)

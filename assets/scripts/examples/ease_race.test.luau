--!strict
-- Tests for ease_race.luau: each runner eases down its lane in its own way and waits at the
-- end; the orb's spring brings it to the ring. (Tests run at 4 frames a second.)
local RACE = {
	name = "Test race",
	components = {
		Transform = { translation = { -8, 0, -8 } },
		Script = { path = "examples/ease_race.luau", props = { length = 4, secs = 1, halflife = 0.1 } },
	},
}

describe("Ease race", function()
	it("runs each runner down its lane with its ease", function(t)
		local race = t:spawn(RACE)
		local runners = race.script.runners
		expect(#runners).to_be(6)
		local function z(k: number): number
			return runners[k].position.z
		end
		t:wait(0.5)
		-- Early on: out_quad is ahead of linear and of in_out_cubic, which starts slow.
		expect(z(1)).to_be_greater_than(0.5)
		expect(z(1)).to_be_less_than(3)
		expect(z(2)).to_be_greater_than(z(1))
		expect(z(2)).to_be_greater_than(z(3))
		-- At the end, all of them, waiting there.
		t:wait(0.75)
		for k = 1, 6 do
			expect(z(k)).to_be_near(4, 0.01)
		end
	end)

	it("brings the orb to the ring", function(t)
		local race = t:spawn(RACE)
		t:wait(1.5)
		local d = (race.script.orb.position - race.script.ring.position):length()
		expect(d).to_be_less_than(0.3)
	end)
end)

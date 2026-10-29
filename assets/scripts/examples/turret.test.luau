--!strict
-- Tests for turret.luau's two regions: its aim and its gun each keep their own state, side by
-- side. Each spawns a turret of its own (and someone to watch for) and checks both regions.
local TURRET = {
	name = "Test turret",
	components = {
		Transform = { translation = { 8, 0, -6 } },
		Script = { path = "examples/turret.luau", props = { target = "Test intruder", range = 4, cooldown = 0.3 } },
	},
}

-- Someone `x` meters to the turret's right.
local function intruder(t: any, x: number): Entity
	return t:spawn({ name = "Test intruder", components = { Transform = { translation = { 8 + x, 0, -6 } } } })
end

describe("Turret", function()
	it("scans with its gun ready while no one is near", function(t)
		local turret = t:spawn(TURRET)
		expect(turret.script:in_state("scanning")).to_be(true)
		expect(turret.script:in_state("ready")).to_be(true)
		t:wait(0.5)
		expect(math.abs(turret.script.yaw)).to_be_greater_than(0.05)
		expect(turret.script.shots).to_be(0)
	end)

	it("turns on whoever comes near and fires, both regions at once", function(t)
		local turret = t:spawn(TURRET)
		intruder(t, 2)
		t:wait_until(function()
			return turret.script:in_state("tracking")
		end, 1)
		t:wait_until(function()
			return turret.script.shots > 0
		end, 3)
		-- Turned to its right (a quarter turn clockwise seen from above).
		expect(turret.script.yaw).to_be_near(-math.pi / 2, 0.1)
		expect(turret.script:in_state("tracking")).to_be(true)
		t:wait(1)
		expect(turret.script.shots).to_be_greater_than(1)
	end)

	it("goes back to scanning when they leave", function(t)
		local turret = t:spawn(TURRET)
		local who = intruder(t, 2)
		t:wait_until(function()
			return turret.script:in_state("tracking")
		end, 1)
		who.position = vec3(30, 0, -6)
		t:wait_until(function()
			return turret.script:in_state("scanning")
		end, 1)
		expect(turret.script:in_state("tracking")).to_be(false)
	end)

	it("switches off and on, each region from its start", function(t)
		local turret = t:spawn(TURRET)
		intruder(t, 2)
		t:wait_until(function()
			return turret.script:in_state("tracking")
		end, 1)
		turret.script:trigger("power")
		expect(turret.script.state).to_be("off")
		expect(turret.script:in_state("aim")).to_be(false)
		expect(turret.script:in_state("gun")).to_be(false)
		turret.script:trigger("power")
		expect(turret.script:in_state("active")).to_be(true)
		expect(turret.script:in_state("ready")).to_be(true)
	end)
end)

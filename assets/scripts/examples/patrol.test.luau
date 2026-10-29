--!strict
-- Tests for patrol.luau: it builds its drone before it starts, and flies the loop through its
-- waypoints at its speed.
local DRONE = {
	name = "Test patrol",
	components = {
		Transform = { translation = { 20, 0, -20 } },
		Script = {
			path = "examples/patrol.luau",
			props = { a = { 0, 2, 0 }, b = { 4, 2, 0 }, c = { 4, 2, 4 }, speed = 4 },
		},
	},
}

describe("Patrol", function()
	it("builds its drone at the first waypoint", function(t)
		local patrol = t:spawn(DRONE)
		local drone = patrol:find("Drone")
		expect(drone).never.to_be_nil()
		expect((drone :: Entity).position).to_be_near(vec3(0, 2, 0), 1.1)
	end)

	it("flies the loop at its speed", function(t)
		local patrol = t:spawn(DRONE)
		local drone = patrol:find("Drone") :: Entity
		-- 4 m along (at 4 m/s): at the second waypoint, give or take a frame's flight and its bob.
		t:wait_until(function()
			return patrol.script.flown >= 4
		end, 3)
		expect(drone.position).to_be_near(vec3(4, 2, 0), 1.1)
		t:wait_until(function()
			return patrol.script.flown >= 8
		end, 3)
		expect(drone.position).to_be_near(vec3(4, 2, 4), 1.1)
	end)
end)

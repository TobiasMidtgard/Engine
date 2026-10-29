--!strict
-- Tests for beacon.luau: its signal, its modes and its motion, each on a beacon of its own
-- with the props the test needs (a Script component's props override the defaults).
local function beacon(props: { [string]: any }?)
	return {
		name = "Test beacon",
		components = {
			Transform = { translation = { -2, 1.5, 4 } },
			PointLight = { intensity_lm = 0 },
			MeshRenderer = { mesh = { shape = { Sphere = { radius = 0.15, segments = 0 } } } },
			Script = { path = "examples/beacon.luau", props = props },
		},
	}
end

describe("Beacon", function()
	it("fires flashed with its count", function(t)
		local b = t:spawn(beacon({ every = 1, pulse = { rate = 2, depth = 0.6 } }))
		local count = t:wait_for(b, "flashed", 2)
		expect(count).to_be(1)
	end)

	it("fires every few flashes", function(t)
		local b = t:spawn(beacon({ every = 2, pulse = { rate = 2, depth = 0.6 } }))
		local heard = t:listen(b, "flashed")
		t:wait(2.2)
		expect(heard.count).to_be_at_least(2)
		for _, call in heard.calls do
			expect(call[1] % 2).to_be(0)
		end
	end)

	it("holds its full light when steady", function(t)
		local b = t:spawn(beacon({ mode = "steady" }))
		t:frames(3)
		expect(b.script.level).to_be(1)
	end)

	it("bobs around where it started", function(t)
		local b = t:spawn(beacon())
		t:frames(1)
		local home = b.script.home
		for _ = 1, 10 do
			t:wait(0.1)
			expect(b.position).to_be_near(home, 0.16)
		end
	end)
end)

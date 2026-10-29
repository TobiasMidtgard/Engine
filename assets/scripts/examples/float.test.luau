--!strict
-- Tests for float.luau and the module it uses (lib/wave.luau): a test file can require
-- modules and check them alone, as well as the scripts in the world.
local wave = require("./lib/wave")

describe("wave.bob", function()
	it("goes from -1 to 1", function()
		expect(wave.bob(0, 1)).to_be_near(0)
		expect(wave.bob(0.25, 1)).to_be_near(1)
		expect(wave.bob(0.75, 1)).to_be_near(-1)
	end)
end)

describe("Float", function()
	it("floats its height above where it started, give or take its bob", function(t)
		local stone = t:spawn({
			name = "Test float",
			components = {
				Transform = { translation = { 0, 0.5, 6 } },
				Script = { path = "examples/float.luau", props = { height = 2, bob = 0.25 } },
			},
		})
		t:frames(2)
		for _ = 1, 8 do
			t:wait(0.1)
			local lift = stone.position.y - 0.5
			expect(lift).to_be_at_least(1.75 - 0.001)
			expect(lift).to_be_at_most(2.25 + 0.001)
		end
	end)
end)

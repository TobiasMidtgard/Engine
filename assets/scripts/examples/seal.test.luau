--!strict
-- Tests for seal.luau's final states: each rune is a region that ends in a final state, and
-- the seal opens once all three have. Each spawns a seal of its own (with its three rune
-- stones), presses runes and checks the states it goes through.
local SEAL = {
	name = "Test seal",
	components = {
		Transform = { translation = { 12, 0, -14 } },
		Script = { path = "examples/seal.luau", props = { hold = 0.5, open_secs = 0.3 } },
	},
}

local function rune(seal: Entity, name: string): Entity
	return seal:find(`{name} rune`) :: Entity
end

describe("Seal", function()
	it("starts sealed, every rune dark", function(t)
		local seal = t:spawn(SEAL)
		for _, name in { "left", "middle", "right" } do
			expect(seal.script:in_state(`{name}_dark`)).to_be(true)
		end
		expect(seal.script.sink).to_be(0)
	end)

	it("waits for every rune, in any order", function(t)
		local seal = t:spawn(SEAL)
		t:press(rune(seal, "Right"))
		t:wait_until(function()
			return seal.script:in_state("right_lit")
		end, 1)
		t:press(rune(seal, "Left"))
		t:wait_until(function()
			return seal.script:in_state("left_lit")
		end, 1)
		-- Two of three regions in their final state: still sealed.
		t:wait(0.3)
		expect(seal.script:in_state("sealed")).to_be(true)
		expect(seal.script.runes_lit).to_be(2)
		t:press(rune(seal, "Middle"))
		t:wait_until(function()
			return seal.script.state == "opening"
		end, 1)
	end)

	it("sinks its door, holds it down and closes again, the runes dark", function(t)
		local seal = t:spawn(SEAL)
		for _, event in { "touch_left", "touch_middle", "touch_right" } do
			seal.script:trigger(event)
		end
		expect(seal.script.state).to_be("opening")
		t:wait_until(function()
			return seal.script.state == "open"
		end, 1)
		expect(seal.script.sink).to_be_near(1)
		t:wait_until(function()
			return seal.script:in_state("sealed")
		end, 2)
		expect(seal.script.sink).to_be_near(0)
		expect(seal.script:in_state("left_dark")).to_be(true)
	end)

	it("lights a rune stone while its rune is lit", function(t)
		local seal = t:spawn(SEAL)
		local left = rune(seal, "Left")
		seal.script:trigger("touch_left")
		t:wait(0.8)
		expect(left.script.level).to_be_greater_than(0.9)
		expect(rune(seal, "Right").script.level).to_be(0)
	end)
end)

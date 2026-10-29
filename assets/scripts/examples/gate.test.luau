--!strict
-- Tests for gate.luau's state machine. Each spawns a quick gate of its own, presses E on it or
-- sends it events, and checks the states it goes through (self.state) and where its bars are.
local GATE = {
	name = "Test gate",
	components = {
		Transform = { translation = { -4, 0, 8 } },
		Interactable = {},
		Script = { path = "examples/gate.luau", props = { lift_secs = 0.4, hold = 0.6, blocker = "Test walker" } },
	},
}

local function state(gate: Entity): string
	return gate.script.state
end

describe("Gate", function()
	it("starts closed, lamp red", function(t)
		local gate = t:spawn(GATE)
		expect(state(gate)).to_be("closed")
		expect(gate.script:in_state("powered")).to_be(true)
		expect(gate.script.lift).to_be(0)
	end)

	it("opens when pressed and closes by itself", function(t)
		local gate = t:spawn(GATE)
		t:press(gate)
		expect(state(gate)).to_be("opening")
		t:wait_until(function()
			return state(gate) == "open"
		end, 2)
		expect(gate.script.lift).to_be_near(1)
		t:wait_until(function()
			return state(gate) == "closed"
		end, 3)
		expect(gate.script.lift).to_be_near(0)
	end)

	it("turns back when pressed while it moves", function(t)
		local gate = t:spawn(GATE)
		t:press(gate)
		t:wait(0.15)
		t:press(gate)
		expect(state(gate)).to_be("closing")
		t:wait_until(function()
			return state(gate) == "closed"
		end, 2)
	end)

	it("stops without power and carries on after", function(t)
		local gate = t:spawn(GATE)
		t:press(gate)
		t:wait(0.15)
		gate.script:trigger("power")
		expect(state(gate)).to_be("unpowered")
		local stopped = gate.script.lift
		t:wait(0.3)
		expect(gate.script.lift).to_be(stopped)
		gate.script:trigger("power")
		expect(state(gate)).to_be("opening")
		t:wait_until(function()
			return state(gate) == "open"
		end, 2)
	end)

	it("won't close on someone under it", function(t)
		local gate = t:spawn(GATE)
		t:press(gate)
		t:wait_until(function()
			return state(gate) == "open"
		end, 2)
		local walker = t:spawn({ name = "Test walker", components = { Transform = { translation = { -4, 0, 8.3 } } } })
		-- Its `hold` runs out twice while the walker stands there: each time it starts to close,
		-- sees the walker and goes back up, so it never gets far down.
		for _ = 1, 16 do
			t:wait(0.1)
			expect(gate.script.lift).to_be_greater_than(0.5)
			expect(state(gate)).never.to_be("closed")
		end
		-- The walker gone, it closes after `hold`.
		walker.position = vec3(-4, 0, 14)
		t:wait_until(function()
			return state(gate) == "closed"
		end, 2)
	end)

	it("knows who is under it when turned", function(t)
		local gate = t:spawn({
			name = "Test gate",
			components = {
				Transform = { translation = { -4, 0, 8 }, rotation = quat.euler(math.pi / 2) },
				Interactable = {},
				Script = { path = "examples/gate.luau", props = { blocker = "Test walker" } },
			},
		})
		-- Turned a quarter, its opening runs along z: 1.2 m along it is under it, 1.2 m across
		-- it isn't.
		local walker = t:spawn({ name = "Test walker", components = { Transform = { translation = { -4, 0, 9.2 } } } })
		expect(gate.script:blocked()).to_be(true)
		walker.position = vec3(-2.8, 0, 8)
		expect(gate.script:blocked()).to_be(false)
	end)
end)

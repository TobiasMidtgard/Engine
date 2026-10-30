--!strict
-- Tests for keeper.luau and its conversation (dialogue/keeper.yarn): with no conversation
-- screen here, conversations run headless (lines to the log, picks by handle:choose). Each
-- test starts the story over: no facts, every quest inactive.
local KEEPER = {
	name = "Test keeper",
	components = { Transform = { translation = { 6, 0, -6 } }, Script = { path = "examples/keeper.luau" } },
}
local SEAL = {
	name = "Test seal",
	components = { Transform = { translation = { 12, 0, -6 } }, Script = { path = "examples/seal.luau", props = { hold = 30 } } },
}

local function fresh()
	api("story.facts", { clear = true })
	api("story.quests", { reset = true })
end

-- Waits until the conversation shows options (or ends), and returns them.
local function options(t: Test, c: Conversation): { string }
	t:wait_until(function()
		return c.options ~= nil or c.done
	end, 5)
	return c.options or {}
end

describe("Keeper", function()
	it("greets once, then by what has happened", function(t)
		fresh()
		local keeper = t:spawn(KEEPER)
		keeper.script:talk()
		local c: Conversation = keeper.script.conversation
		expect(c.speaker).to_be("Keeper")
		local first = options(t, c)
		expect(#first).to_be(4)
		expect(first[1]).to_be("Who sealed it?")
		expect(first[4]).to_be("I can read old runes.")
		c:choose(3)
		t:wait_until(function()
			return c.done
		end, 2)
		expect(c.error).to_be(nil)
		expect(c.line).to_be("Most do. The door does not mind.")
		-- The second time: the node group's `when: always` node, its line group.
		keeper.script:talk()
		local again: Conversation = keeper.script.conversation
		expect(again.line).to_be("Back again?")
		expect(options(t, again)[2]).to_be("I'll try the runes.")
		again:stop()
	end)

	it("reads the runes on a lore check, its odds shown", function(t)
		fresh()
		local keeper = t:spawn(KEEPER)
		keeper.script:talk()
		local c: Conversation = keeper.script.conversation
		options(t, c)
		local check = api("story.state", {}).current.checks[1]
		expect(check.option).to_be(4)
		expect(check.skill).to_be("lore")
		-- 2d6 + Lore 2 against 9: 21 ways in 36.
		expect(math.abs(check.chance - 21 / 36) < 1e-9).to_be(true)
		c:choose(4, true)
		t:wait_until(function()
			return c.done
		end, 2)
		expect(c.passed).to_be(true)
		expect(facts.read_runes).to_be(true)
		expect(c.line).to_be("You can? Then you know the third one lies. It is no stone at all.")
	end)

	it("gives the quest, which follows the seal's runes", function(t)
		fresh()
		local keeper = t:spawn(KEEPER)
		local seal = t:spawn(SEAL)
		keeper.script:talk()
		local c: Conversation = keeper.script.conversation
		options(t, c)
		c:choose(2) -- Can it be opened?
		-- Offer: <<raise_staff>> holds the conversation until the staff is down again.
		t:wait_until(function()
			return quests.state("sealed_bridge") == "active"
		end, 6)
		t:wait_until(function()
			return c.done
		end, 4)
		expect(c.taken[#c.taken]).to_be("Offer")
		local function step(n: number): any
			local all = api("story.quests", {})
			for _, q in all do
				if q.id == "sealed_bridge" then
					return q.objectives[n]
				end
			end
			return nil
		end
		seal.script:trigger("touch_left")
		seal.script:trigger("touch_right")
		t:wait_until(function()
			return step(1).progress == 2
		end, 2)
		expect(step(1).done).to_be(false)
		seal.script:trigger("touch_middle")
		t:wait_until(function()
			return step(1).done
		end, 2)
		expect(facts.seal_opened).to_be(true)
		expect(quests.state("sealed_bridge")).to_be("active")
		-- Back to the keeper: the open seal's node thanks you, and that ends the quest.
		keeper.script:talk()
		local thanks: Conversation = keeper.script.conversation
		local picks = options(t, thanks)
		expect(picks[1]).to_be("What now?")
		t:wait_until(function()
			return quests.state("sealed_bridge") == "completed"
		end, 1)
		expect(facts.courtyard_hero).to_be(true)
		thanks:choose(2)
		t:wait_until(function()
			return thanks.done
		end, 2)
		expect(thanks.line).to_be("Then go with the light at your back.")
	end)
end)

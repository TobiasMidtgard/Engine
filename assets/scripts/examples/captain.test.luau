--!strict
-- Tests for captain.luau and the muster (dialogue/muster.yarn), whose options are picked by
-- walking: the recruit it builds in the gap of the rank walks (its position set) to a spot
-- and stands in its ring until that option is taken. Headless here (no conversation screen),
-- but the spots, rings, slow motion, checks, barks and watchers work as in the game.
-- `dialogue.checks.next` decides a check instead of the dice.
local function spot(name: string, at: { number }, radius: number): any
	return {
		name = `Spot {name}`,
		components = { Transform = { translation = at }, StorySpot = { name = name, radius_m = radius } },
	}
end

-- The captain with his spots as children, as in a scene.
local CAPTAIN = {
	name = "Test captain",
	components = {
		Transform = { translation = { -20, 0, 20 } },
		Script = { path = "examples/captain.luau", props = { soldiers = 2, recruit = true } },
	},
	children = {
		spot("rank", { 0, 0, 3.2 }, 0.5),
		spot("forward", { 1.45, 0, 0.15 }, 0.6),
		spot("ear", { -0.62, 0, -0.18 }, 0.42),
		spot("behind", { 0.1, 0, -1.35 }, 0.55),
	},
}

-- The muster, once it waits for the recruit to walk.
local function muster(t: Test, captain: Entity): (Conversation, Entity)
	api("story.facts", { clear = true })
	local recruit = captain:find("Recruit") :: Entity
	local c = dialogue.start("dialogue/muster", "Start", { owner = captain, chooser = recruit, headless = true })
	t:wait_until(function()
		return c.waiting == "spots" or c.done
	end, 5)
	return c, recruit
end

-- The recruit into spot `name` (both are the captain's children).
local function walk(captain: Entity, recruit: Entity, name: string)
	recruit.position = (captain:find(`Spot {name}`) :: Entity).position
end

describe("Captain", function()
	it("asks who speaks, and you answer by walking to his ear", function(t)
		local captain = t:spawn(CAPTAIN)
		local c, recruit = muster(t, captain)
		expect(c.waiting).to_be("spots")
		expect(c.line).to_be("Who speaks for the company? Step up, or hold your place.")
		expect(#(c.options :: { string })).to_be(4)
		-- Standing in the rank, where it began, picks nothing by itself.
		t:wait(0.5)
		expect(c.waiting).to_be("spots")
		dialogue.checks.next = true
		walk(captain, recruit, "ear")
		t:wait_until(function()
			return c.done
		end, 5)
		expect(c.error).to_be(nil)
		expect(captain.script.noticed).to_be("ear")
		expect(facts.whispered).to_be(true)
		expect(c.taken[#c.taken]).to_be("Whisper")
		expect(c.passed).to_be(true)
		expect(c.line).to_be("If it works, it was my idea. If it fails, it was yours.")
	end)

	it("shows the odds over the rings, and a failed whisper is brushed off", function(t)
		local captain = t:spawn(CAPTAIN)
		local c, recruit = muster(t, captain)
		local open = api("story.state", {}).spots[1]
		-- 2d6 + Persuade 1 against 8: 21 ways in 36. Nerve 0 against 9: 10 in 36, and red.
		expect(open.options[3].check).to_be("PERSUADE 8 · 58%")
		expect(open.options[2].check).to_be("RED · NERVE 9 · 28%")
		dialogue.checks.next = false
		walk(captain, recruit, "ear")
		t:wait_until(function()
			return c.done
		end, 5)
		expect(c.passed).to_be(false)
		expect(facts.whispered).to_be(false)
		expect(c.line).to_be("Not now, recruit. Whatever it is, it keeps until after the parley.")
	end)

	it("mutters and watches as you step out of the rank", function(t)
		local captain = t:spawn(CAPTAIN)
		local c, recruit = muster(t, captain)
		-- He and his two soldiers follow the recruit.
		expect(#api("story.state", {}).spots[1].watching).to_be(3)
		dialogue.checks.next = false
		walk(captain, recruit, "forward")
		t:wait_until(function()
			for _, b in api("story.state", {}).barks do
				if b.speaker == "Soldier 2" and b.text == "He's not serious." then
					return true
				end
			end
			return false
		end, 2)
		t:wait_until(function()
			local open = api("story.state", {}).spots[1]
			return open ~= nil and math.abs(open.watching[1].head_deg) > 20
		end, 1)
		-- A red check failed: back in line, his regard for you lower, the rank laughing.
		t:wait_until(function()
			return c.done
		end, 6)
		expect(facts.overstepped).to_be(true)
		expect(facts.spoke_up).to_be(nil)
		expect(facts.regard_captain).to_be(-1)
		expect(c.line).to_be("Back in line, recruit. When I want a speaker, I will point at one.")
	end)

	it("turns around when you slip behind him", function(t)
		local captain = t:spawn(CAPTAIN)
		local c, recruit = muster(t, captain)
		walk(captain, recruit, "behind")
		t:wait_until(function()
			return c.done
		end, 6)
		expect(facts.drew_on_captain).to_be(true)
		expect(c.taken[#c.taken]).to_be("Behind")
		-- <<turn_around>> held the conversation until he faced the other way.
		local body = captain:find("Body") :: Entity
		expect((body.rotation * vec3(0, 0, 1)).z < -0.9).to_be(true)
	end)

	it("notes it when you are out of the rank as he starts", function(t)
		local captain = t:spawn(CAPTAIN)
		local recruit = captain:find("Recruit") :: Entity
		recruit.position = vec3(2.5, 0, 2)
		local c = dialogue.start("dialogue/muster", "Start", { owner = captain, chooser = recruit, headless = true })
		t:wait_until(function()
			return c.waiting == "spots"
		end, 5)
		local said = {}
		for _, l in api("story.state", {}).current.lines do
			table.insert(said, l.text)
		end
		expect(table.find(said, "You there, out of the line already? Back in it.") ~= nil).to_be(true)
		c:stop()
	end)
end)

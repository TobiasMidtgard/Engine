--!strict
-- Tests for captain.luau and the muster (dialogue/muster.yarn), whose options are picked by
-- walking: the recruit it builds in the gap of the rank walks (its position set) to a spot
-- and stands in its ring until that option is taken. Headless here (no conversation screen),
-- but the spots, rings and slow motion work as in the game.
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
		walk(captain, recruit, "ear")
		t:wait_until(function()
			return c.done
		end, 5)
		expect(c.error).to_be(nil)
		expect(captain.script.noticed).to_be("ear")
		expect(facts.whispered).to_be(true)
		expect(c.taken[#c.taken]).to_be("Whisper")
		expect(c.line).to_be("If it works, it was my idea. If it fails, it was yours.")
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

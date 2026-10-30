--!strict
-- Plays a cutscene when pressed (E); pressing again while it plays skips to its end, as a
-- skip button would (what it changes ends as it leaves it, and its retro events still go
-- out). Put it on the cutscene's player: the names its tracks bind are looked up below this
-- entity first, and its events with no object come here, calling the method of that name.
-- Make cutscenes in the editor's Sequencer tab. See "Cutscenes" in the scripts README.
local Trigger = {}
Trigger.props = {
	cutscene = "cutscenes/lift", -- the .cutscene file it plays
	once = false, -- only the first press plays it
	restore = false, -- put back everything it changed when it ends
	cameras = true, -- look through its camera cuts
}

Trigger.signals = {
	finished = "skipped: boolean", -- the cutscene ended; skipped if a press cut it short
}

function Trigger:start()
	self.plays = 0
	self.arrivals = 0
	self.skipped = false
end

function Trigger:on_interact(by: Entity)
	local cs = self.cutscene
	if cs and not cs.done then
		self.skipped = true
		cs:skip()
		return
	end
	if self.props.once and self.plays > 0 then
		return
	end
	self.plays += 1
	self.skipped = false
	self.cutscene = cutscene.play(self.props.cutscene, {
		restore = self.props.restore,
		cameras = self.props.cameras,
		done = function()
			self:emit("finished", self.skipped)
		end,
	})
end

-- The cutscene's events with no object come here.
function Trigger:departed()
	print("departed")
end

function Trigger:arrived(data: any)
	self.arrivals += 1
	print(`arrived at floor {data.floor}`)
end

return Trigger

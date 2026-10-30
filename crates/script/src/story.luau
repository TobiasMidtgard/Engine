--!nonstrict
-- Conversations and quest conditions: the Luau half of story.rs. A .yarn file compiles to a
-- module of nodes (story.rs's compile_yarn, one Luau line per Yarn line). A conversation
-- runs them in a coroutine that pauses whenever the player has to read or pick, and
-- `__story_tick` resumes it every frame. In the compiled code `S` is the conversation (its
-- lines, options and commands) and `F` is the facts.
-- `__story_*` and `__quest_*` are story.rs's Rust functions.

local module_of = __story_module
local version_of = __story_version
local visit = __story_visit
local once_seen = __story_once
local owner_now = __story_owner
local set_owner = __story_set_owner
local report = __story_report
local take = __story_take
local dt_now = __story_dt
local new_id = __story_new_id
local has_api = __story_has_api
local log_error = __story_error
local spots_begin = __story_spots_begin
local spots_poll = __story_spots_poll
local spots_end = __story_spots_end
local spots_say = __story_spots_say
local standing_at = __story_standing_at
local spots_roll = __story_spots_roll
local spots_left = __story_spots_left
local check_get = __story_check_get
local check_set = __story_check_set
local luck_op = __story_luck
local settings_now = __story_settings
local settings_set = __story_settings_set
local real_dt = __story_real_dt
local bark_say = __story_bark_say
local quest_module = __quest_module
local quest_ctl = __quest_ctl
local call_api = api
local say = print
local warn_log = warn
local F = facts
local cutscenes = cutscene
local yield = coroutine.yield
local resume = coroutine.resume
local status = coroutine.status
local frame_info = debug.info

-- What the runner yields when it waits for the player (task.wait yields "wait").
local PAUSE = table.freeze({})
-- What `<<stop>>` and `<<return>>` hand back up through the node's code.
local STOP = table.freeze({ stop = true })
local BACK = table.freeze({ back = true })

-- ------------------------------------------------------------------------- values

-- Yarn's operators. A fact that was never set counts as 0, "" or false next to a value of
-- that kind, so `$coins > 3` works before anything set $coins.
local Ops = {}

-- An error at the .yarn line that called (compiled lines carry their file's line).
local function fail(message: string)
	for level = 2, 30 do
		local src, line = frame_info(level, "sl")
		if src == nil then
			break
		end
		if string.sub(src, -5) == ".yarn" then
			local file = string.gsub(src, "^[@=]", "")
			error(`{file}:{line}: {message}`, 0)
		end
	end
	error(message, 0)
end

local function text_of(v: any): string
	if v == nil then
		return ""
	elseif type(v) == "number" then
		if v == math.floor(v) and math.abs(v) < 1e15 then
			return string.format("%d", v)
		end
		local s = string.format("%.4f", v)
		s = (string.gsub(s, "0+$", ""))
		return (string.gsub(s, "%.$", ""))
	end
	return tostring(v)
end
Ops.text = text_of

local function num(v: any): number
	if v == nil then
		return 0
	elseif type(v) == "number" then
		return v
	elseif type(v) == "boolean" then
		return if v then 1 else 0
	end
	local n = tonumber(v)
	if n == nil then
		fail(`"{text_of(v)}" isn't a number`)
	end
	return n :: number
end

local function blank_like(v: any): any
	local t = type(v)
	if t == "number" then
		return 0
	elseif t == "string" then
		return ""
	elseif t == "boolean" then
		return false
	end
	return nil
end

function Ops.eq(a: any, b: any): boolean
	if a == nil then
		a = blank_like(b)
	end
	if b == nil then
		b = blank_like(a)
	end
	return a == b
end

function Ops.lt(a: any, b: any): boolean
	if type(a) == "string" and type(b) == "string" then
		return a < b
	end
	return num(a) < num(b)
end

function Ops.le(a: any, b: any): boolean
	if type(a) == "string" and type(b) == "string" then
		return a <= b
	end
	return num(a) <= num(b)
end

-- `+` adds numbers and joins text.
function Ops.add(a: any, b: any): any
	if type(a) == "string" or type(b) == "string" then
		return text_of(a) .. text_of(b)
	end
	return num(a) + num(b)
end

function Ops.sub(a: any, b: any): number
	return num(a) - num(b)
end

function Ops.mul(a: any, b: any): number
	return num(a) * num(b)
end

function Ops.div(a: any, b: any): number
	local d = num(b)
	if d == 0 then
		fail("divided by zero")
	end
	return num(a) / d
end

function Ops.mod(a: any, b: any): number
	local d = num(b)
	if d == 0 then
		fail("% by zero")
	end
	return num(a) % d
end

function Ops.neg(a: any): number
	return -num(a)
end

function Ops.xor(a: any, b: any): boolean
	return (not a) ~= (not b)
end

-- Yarn's functions, plus the quests' states. More in `dialogue.functions`.
local Funcs: { [string]: (...any) -> any } = {
	visited = function(title: string): boolean
		return visit(title, false) > 0
	end,
	visited_count = function(title: string): number
		return visit(title, false)
	end,
	random = function(): number
		return math.random()
	end,
	random_range = function(a: number, b: number): number
		return math.random(math.floor(num(a)), math.floor(num(b)))
	end,
	dice = function(sides: number): number
		return math.random(1, math.max(1, math.floor(num(sides))))
	end,
	round = function(x: number): number
		return math.floor(num(x) + 0.5)
	end,
	round_places = function(x: number, places: number): number
		local m = 10 ^ num(places)
		return math.floor(num(x) * m + 0.5) / m
	end,
	floor = function(x: number): number
		return math.floor(num(x))
	end,
	ceil = function(x: number): number
		return math.ceil(num(x))
	end,
	inc = function(x: number): number
		return math.floor(num(x)) + 1
	end,
	dec = function(x: number): number
		return math.ceil(num(x)) - 1
	end,
	decimal = function(x: number): number
		local n = num(x)
		return n - (if n < 0 then math.ceil(n) else math.floor(n))
	end,
	int = function(x: number): number
		local n = num(x)
		return if n < 0 then math.ceil(n) else math.floor(n)
	end,
	string = text_of,
	number = function(x: any): number
		return tonumber(x) or 0
	end,
	bool = function(x: any): boolean
		return x ~= nil and x ~= false and x ~= 0 and x ~= ""
	end,
	quest_state = function(id: string): string
		return quest_ctl("state", id)
	end,
	quest_active = function(id: string): boolean
		return quest_ctl("state", id) == "active"
	end,
	quest_done = function(id: string): boolean
		return quest_ctl("state", id) == "completed"
	end,
}

-- `dialogue` is the UI module's namespace when it is there (dialogue.state, dialogue.next...).
local D = dialogue
if D == nil then
	D = {}
	dialogue = D
end
-- How each speaker shows in the conversation screen: { title, portrait, side, color }.
D.speakers = {}
-- The player's replies: { name, portrait }.
D.player = { name = "You" }
-- Commands for every conversation: `dialogue.commands.shake = function(strength) ... end`
-- runs `<<shake 2>>`. Return a function to make the conversation wait until it returns true.
D.commands = {}
-- Functions for conditions and text: `dialogue.functions.gold = function() ... end` makes
-- `{gold()}` and `<<if gold() > 3>>` work.
D.functions = {}
-- Skill checks (`#check:persuade:8` on an option). `dialogue.checks.skill = function(name,
-- chooser) return ... end` gives the chooser's skills (facts `$skill_<name>` otherwise);
-- `dialogue.checks.bonuses = function(skill, chooser) return { { 2, "Watch colours" } } end`
-- adds a game's own; `dialogue.checks.next = true` (or false) decides the next check, for
-- tests.
D.checks = {}

-- How checks roll: `dialogue.settings({ dice = "d20", difficulty = "easy", luck = false })`
-- (any of them); returns them all.
function D.settings(t: any?): any
	if t ~= nil then
		settings_set(t)
	end
	return settings_now()
end

function Ops:call(name: string, ...: any): any
	if name == "standing_at" then
		-- Where the conversation's chooser (the player) stands: `<<if standing_at("ear")>>`.
		local opts = rawget(self, "opts")
		return standing_at(opts and opts.chooser, text_of((...)))
	elseif name == "passed" then
		-- Whether the conversation's last skill check passed.
		return rawget(self, "passed") == true
	elseif name == "moment" then
		-- In a bark: what happened ("near", "out" or "hurry"; "" outside a walk-to choice).
		local opts = rawget(self, "opts")
		return opts and opts.moment or ""
	elseif name == "seconds_left" then
		-- Of the walk-to choice open now (a large number without a time limit).
		local opts = rawget(self, "opts")
		local id = rawget(self, "spots") or (opts and opts.choice)
		return (id and spots_left(id)) or 1e9
	end
	local f = Funcs[name] or D.functions[name]
	if f == nil then
		fail(`there is no function {name}() (Yarn's own, or one in dialogue.functions)`)
	end
	return f(...)
end

function Ops:has_once(key: string): boolean
	return once_seen(key, false)
end

-- True the first time (and remembers it): `<<once>>`, `<<if $x>> <<once>>` lines.
function Ops:once(key: string): boolean
	if once_seen(key, false) then
		return false
	end
	once_seen(key, true)
	return true
end

-- -------------------------------------------------------------------------- checks

-- The chance that `dice` ("2d6" or "d20") plus `mod` reaches `target`, with the automatic
-- pass and fail at the top and bottom (story.rs's check_chance).
local function chance_of(dice: string, mod: number, target: number): number
	local pass = 0
	if dice == "d20" then
		for r = 1, 20 do
			if r == 20 or (r ~= 1 and r + mod >= target) then
				pass += 1
			end
		end
		return pass / 20
	end
	for a = 1, 6 do
		for b = 1, 6 do
			if (a == 6 and b == 6) or (not (a == 1 and b == 1) and a + b + mod >= target) then
				pass += 1
			end
		end
	end
	return pass / 36
end

-- "persuade" -> "Persuade", "half_truth" -> "Half truth".
local function skill_title(skill: string): string
	local s = string.gsub(skill, "_", " ")
	return string.upper(string.sub(s, 1, 1)) .. string.sub(s, 2)
end

-- A name as a fact's: "Old Brannoc" -> "old_brannoc".
local function fact_name(name: string): string
	return (string.gsub(string.lower(name), "[^%w]+", "_"))
end

-- "PERSUADE 8 · 72%" (what a ring's label shows under its option).
local function check_label(spec: any): string
	local head = `{string.upper(skill_title(spec.skill))} {text_of(spec.target)} · {math.floor(spec.chance * 100 + 0.5)}%`
	return if spec.red then `RED · {head}` else head
end

-- "Persuade 8: rolled 3 + 5, +2 = 10, success".
local function roll_text(r: any): string
	local head = `{skill_title(r.skill)} {text_of(r.target)}`
	local result = if r.passed then "success" else "failure"
	if #r.dice == 0 then
		return `{head}: {result}{if r.how == "forced" then " (decided, not rolled)" else ""}`
	end
	local parts = {}
	for i, d in r.dice do
		parts[i] = text_of(d)
	end
	local mod = ""
	if r.modifier > 0 then
		mod = `, +{text_of(r.modifier)}`
	elseif r.modifier < 0 then
		mod = `, -{text_of(-r.modifier)}`
	end
	local why = ""
	if #r.dice == 2 and r.dice[1] == 6 and r.dice[2] == 6 then
		why = " (double six)"
	elseif #r.dice == 2 and r.dice[1] == 1 and r.dice[2] == 1 then
		why = " (double one)"
	elseif #r.dice == 1 and r.dice[1] == 20 then
		why = " (natural 20)"
	elseif #r.dice == 1 and r.dice[1] == 1 then
		why = " (natural 1)"
	end
	return `{head}: rolled {table.concat(parts, " + ")}{mod} = {text_of(r.total)}{why}, {result}`
end

-- ------------------------------------------------------------------- conversations

local Run = setmetatable({}, { __index = Ops })
Run.__index = Run

local ORDER: { any } = {}

local function speaker_style(c: any, name: string?): any
	local out: any = { name = name or "", side = "left" }
	if name ~= nil then
		local style = (c.opts.speakers and c.opts.speakers[name]) or D.speakers[name]
		if type(style) == "table" then
			for k, v in style do
				out[k] = v
			end
		end
	end
	return out
end

function Run:load()
	local v = if self.mod then version_of(self.path) else -1
	if self.mod == nil or v ~= self.ver then
		local m, ver, name = module_of(self.path)
		self.mod, self.ver, self.path = m, ver, name
		local by = {}
		for _, n in m.nodes do
			local list = by[n.title]
			if list == nil then
				list = {}
				by[n.title] = list
			end
			table.insert(list, n)
		end
		self.by = by
	end
end

-- The node to run for `title`: of a node group (several nodes with one title and when:
-- headers), the one whose conditions hold that tests the most, the least seen on a tie.
function Run:pick_node(title: string, quiet: boolean?): any
	self:load()
	local list = self.by[title]
	if list == nil then
		fail(`there is no node called '{title}' in {self.path}`)
	end
	if #list == 1 and #list[1].when == 0 then
		return list[1]
	end
	local best, best_score, best_seen = nil, -1, math.huge
	for _, n in list do
		local ok = true
		for _, w in n.when do
			if not w(self, F) then
				ok = false
				break
			end
		end
		if ok then
			local seen = visit(`@{self.path}:{n.line}`, false)
			if n.score > best_score or (n.score == best_score and seen < best_seen) then
				best, best_score, best_seen = n, n.score, seen
			end
		end
	end
	if best == nil and not quiet then
		fail(`no node called '{title}' fits now (every one's when: is false)`)
	end
	return best
end

function Run:run_node(title: string): any
	local n = self:pick_node(title)
	for _, key in n.once do
		once_seen(key, true)
	end
	self.node = title
	self.node_line = n.line
	table.insert(self.taken, title)
	if #self.taken > 64 then
		table.remove(self.taken, 1)
	end
	self.dirty = true
	self.jumps += 1
	if self.jumps > 200 then
		fail("200 jumps with nothing said: is there a loop of <<jump>>s?")
	end
	local r = n.run(self, F)
	visit(title, true)
	visit(`@{self.path}:{n.line}`, true)
	return r
end

-- Runs a node and the nodes it jumps to.
function Run:go(title: string): any
	local r = self:run_node(title)
	while type(r) == "table" and r.jump ~= nil do
		r = self:run_node(r.jump)
	end
	return r
end

function Run:jump(title: string): any
	return { jump = title }
end

function Run:stop(): any
	return STOP
end

function Run:back(): any
	return BACK
end

-- `<<detour Node>>`: runs it, then carries on here (unless it stops the conversation).
function Run:detour(title: string): any
	local here, here_line = self.node, self.node_line
	local r = self:go(title)
	self.node, self.node_line = here, here_line
	self.dirty = true
	if r == STOP then
		return STOP
	end
	return nil
end

-- The transcript the editor shows (the last 60 lines and picks).
function Run:remember(entry: any)
	table.insert(self.said, entry)
	if #self.said > 60 then
		table.remove(self.said, 1)
	end
end

function Run:line(speaker: string?, text: string, tags: any, n: number)
	self.jumps = 0
	local l = { speaker = speaker, text = text, tags = tags, line = n }
	self.last = l
	self:remember({ speaker = speaker, text = text, line = n })
	self.dirty = true
	if self.bark then
		-- Over the speaker's head while play goes on, then the next line (as Valve's
		-- follow-ups go once a line is said).
		local secs = math.clamp(1.1 + 0.05 * #text, 1.6, 6)
		bark_say(self.id, speaker or "", text, secs, self.owner)
		say(`  ({speaker or "?"}) {text}`)
		yield(PAUSE, "real", secs)
		return
	end
	if self.ui then
		table.insert(self.buf, l)
	else
		say(if speaker then `{speaker}: {text}` else text)
		if self.opts.step then
			-- One line at a time: on until a pick (story.choose with index 0 continues).
			yield(PAUSE, "reading")
		end
	end
end

-- The conversation screen (the UI module's): the lines said since it last showed, as one
-- conversation the player reads through; then the options (each leads to a line of the
-- player's, which tells the runner the pick) or a blank line to wait on ("h").
function Run:show(choices: { string }?, hold: boolean, checks: any?)
	local lines = self.buf
	self.buf = {}
	if #lines == 0 then
		-- Options straight after a pick or a command: the last line again, over them.
		lines = { self.last or { text = "" } }
	end
	local speakers: any = {}
	local nodes: any = {}
	local function key_of(name: string?): string
		local k = name or "_"
		if speakers[k] == nil then
			speakers[k] = speaker_style(self, name)
		end
		return k
	end
	for i, l in lines do
		nodes[`l{i}`] = {
			speaker = key_of(l.speaker),
			text = l.text,
			next = if i < #lines then `l{i + 1}` else nil,
		}
	end
	local last = nodes[`l{#lines}`]
	if choices then
		local me: any = { name = D.player.name or "You", side = "right" }
		for k, v in D.player do
			me[k] = v
		end
		me.side = "right"
		speakers.__player = me
		local list = {}
		for i, t in choices do
			list[i] = { text = t, next = `p{i}` }
			nodes[`p{i}`] = { speaker = "__player", text = t }
			-- A check: the screen shows its odds and rolls them; a failure goes to f<i>.
			local spec = checks and checks[i]
			if spec then
				list[i].tag = `{skill_title(spec.skill)} {text_of(spec.target)}{if spec.red then " · red" else ""}`
				list[i].chance = spec.chance
				list[i].fail = `f{i}`
				nodes[`f{i}`] = { speaker = "__player", text = t }
			end
		end
		last.choices = list
	elseif hold then
		last.next = "h"
		nodes.h = { speaker = last.speaker, text = "" }
	end
	self.lines_shown = lines
	self.shown = true
	call_api("ui.show", {
		screen = "dialogue",
		tree = { start = "l1", speakers = speakers, nodes = nodes },
	})
end

function Run:hide()
	if self.shown then
		self.shown = false
		pcall(call_api, "ui.hide", { screen = "dialogue" })
	end
end

-- Waits until the player has read the lines said so far (before a command or a wait).
function Run:read_first()
	if self.ui and #self.buf > 0 then
		self:show(nil, true)
		yield(PAUSE, "reading")
	end
end

-- `#spot:name` (or `#fallback`) among an option's tags: its value ("" for a bare tag).
local function tag_value(tags: any, key: string): string?
	if tags == nil then
		return nil
	end
	for _, t in tags do
		if t == key then
			return ""
		end
		local v = string.match(t, "^" .. key .. ":(.+)$")
		if v then
			return v
		end
	end
	return nil
end

-- Options with `#spot:name` tags are picked by walking there (spots.rs): the lines said
-- before them read first, the last stays up as the speaker's bubble, the player walks. The
-- option picked (from 1), or nil when they show as a menu after all.
function Run:walk(items: { any }, texts: { string }, checks: any): number?
	local list = {}
	local any = false
	for n, it in items do
		local spot = tag_value(it.tags, "spot")
		list[n] = {
			text = texts[n],
			spot = spot,
			fallback = tag_value(it.tags, "fallback") ~= nil,
			check = if checks[n] then check_label(checks[n]) else nil,
		}
		any = any or spot ~= nil
	end
	local o = self.spot_opts or {}
	local watch = self.watch or {}
	local barks = self.barks_node
	self.spot_opts, self.watch, self.barks_node = nil, nil, nil
	if not any then
		return nil
	end
	local prompt = self.last
	if self.ui then
		prompt = table.remove(self.buf) or self.last
		if #self.buf > 0 then
			self:show(nil, true)
			yield(PAUSE, "reading")
		end
		self.buf = {}
		self:hide()
	end
	local spec = {
		options = list,
		prompt = { speaker = prompt and prompt.speaker, text = prompt and prompt.text or "" },
		time = o.time,
		slow = o.slow,
		watch = if #watch > 0 then watch else nil,
	}
	local id, why = spots_begin(self.id, spec, self.opts.chooser, self.owner)
	if id == nil then
		warn_log(`{self.path}: {why}; the options show as a menu`)
		return nil
	end
	self.spots = id
	self.walk_barks = barks
	if not self.ui then
		say("  (walk to one)")
		for n, it in list do
			say(
				`  {n}. {it.text}{if it.spot then ` [{it.spot}]` else ""}{if it.fallback then " (fallback)" else ""}`
					.. (if it.check then `  [{it.check}]` else "")
			)
		end
	end
	self.dirty = true
	local i = yield(PAUSE, "spots", #texts, id)
	self.spots = nil
	self.walk_barks, self.bark_next = nil, nil
	self.walked = id
	spots_end(id, i)
	return i
end

-- Something happened in a walk-to choice with `<<barks Node>>`: the node's best fit is said
-- as barks. One at a time: a moment while one is being said waits (only the newest).
function Run:moment(kind: string)
	if self.walk_barks == nil then
		return
	end
	local b = self.barking
	if b and not b.done then
		self.bark_next = kind
		return
	end
	self.bark_next = nil
	self.barking = D.bark(self.path, self.walk_barks, {
		owner = self.owner,
		chooser = self.opts.chooser,
		moment = kind,
		choice = self.spots,
	})
end

-- A script's word when the player steps into a spot: the owner's `on_spot_near(name)` runs,
-- and what it returns (text) is the speaker's new line over their head.
function Run:near_spot(id: number, name: string)
	local s = self.owner and self.owner.script
	local m = s and s.on_spot_near
	if type(m) ~= "function" then
		return
	end
	local ok, said = pcall(m, s, name)
	if not ok then
		warn_log(`{self.path}: on_spot_near: {said}`)
	elseif type(said) == "string" then
		spots_say(id, said)
	end
end

local Options = {}
Options.__index = Options

function Run:options(): any
	return setmetatable({ c = self, items = {} }, Options)
end

function Options:add(text: string, ok: any, key: string?, tags: any, n: number, body: () -> any)
	if ok and (key == nil or not once_seen(key, false)) then
		table.insert(self.items, { text = text, key = key, tags = tags, line = n, body = body })
	end
end

-- The chooser's skill: dialogue.checks.skill's answer, or the fact $skill_<name> (0).
function Run:skill(name: string): number
	local f = D.checks.skill
	if type(f) == "function" then
		local ok, v = pcall(f, name, self.opts.chooser)
		if not ok then
			warn_log(`dialogue.checks.skill: {v}`)
		elseif tonumber(v) then
			return tonumber(v) :: number
		end
	end
	return tonumber(F[`skill_{name}`]) or 0
end

-- An option's skill check (`#check:skill:difficulty`, `#red`) as it stands now: the odds and
-- everything in them; { shut } for a failed white check until the skill is higher, { gone }
-- for a red one rolled before; nil without a check.
function Run:check_of(it: any): any
	local raw = tag_value(it.tags, "check")
	if raw == nil then
		return nil
	end
	local skill, dc = string.match(raw, "^([%w_]+):(%-?[%d%.]+)$")
	local difficulty = tonumber(dc)
	if skill == nil or difficulty == nil then
		fail(`#check:{raw}: write #check:skill:difficulty, like #check:persuade:8`)
	end
	local red = tag_value(it.tags, "red") ~= nil
	local key = `{self.path}:{it.line}`
	local value = self:skill(skill)
	local rec = check_get(key)
	if type(rec) == "table" then
		if rec.red then
			return { gone = true }
		elseif rec.shut ~= nil and value <= num(rec.shut) then
			return { shut = true, skill = skill, above = num(rec.shut) }
		end
	end
	local st = settings_now()
	local bonuses = {}
	for _, b in self.bonuses or {} do
		if b.skill == skill or b.skill == "all" then
			table.insert(bonuses, { n = b.n, why = b.why })
		end
	end
	local game = D.checks.bonuses
	if type(game) == "function" then
		local ok, list = pcall(game, skill, self.opts.chooser)
		if not ok then
			warn_log(`dialogue.checks.bonuses: {list}`)
		elseif type(list) == "table" then
			for _, b in list do
				local n = tonumber(b[1] or b.n)
				if n and n ~= 0 then
					table.insert(bonuses, { n = n, why = text_of(b[2] or b.why or "Bonus") })
				end
			end
		end
	end
	local who = self.last and self.last.speaker
	if who then
		local r = tonumber(F[`regard_{fact_name(who)}`])
		if r and r ~= 0 then
			table.insert(bonuses, { n = r, why = `{who}'s regard for you` })
		end
	end
	if st.luck then
		local l = luck_op("get")
		if l > 0 then
			table.insert(bonuses, { n = l, why = "Luck" })
		end
	end
	local mod = value
	for _, b in bonuses do
		mod += b.n
	end
	local target = difficulty + st.shift
	return {
		skill = skill,
		difficulty = difficulty,
		target = target,
		value = value,
		bonuses = bonuses,
		mod = mod,
		dice = st.dice,
		chance = chance_of(st.dice, mod, target),
		red = red,
		key = key,
	}
end

-- Rolls an option's check (or takes `forced`, or the result the conversation screen rolled),
-- remembers it (a red check is gone, a failed white one shut), and shows it: in the log, and
-- over the ring of a choice walked to (`choice`, option `i`) for a moment.
function Run:settle(spec: any, text: string, forced: boolean?, screen: boolean?, choice: number?, i: number)
	local r = {
		skill = spec.skill,
		option = text,
		dice = {},
		modifier = spec.mod,
		total = spec.mod,
		target = spec.target,
		passed = false,
		how = "",
	}
	if forced ~= nil then
		r.passed, r.how = forced, "forced"
	elseif screen ~= nil then
		r.passed, r.how = screen, "screen"
	elseif spec.dice == "d20" then
		local d = math.random(1, 20)
		r.dice, r.total = { d }, d + spec.mod
		r.passed = d == 20 or (d ~= 1 and r.total >= spec.target)
	else
		local a, b = math.random(1, 6), math.random(1, 6)
		r.dice, r.total = { a, b }, a + b + spec.mod
		r.passed = (a == 6 and b == 6) or (not (a == 1 and b == 1) and r.total >= spec.target)
	end
	if spec.red then
		check_set(spec.key, { red = true })
	elseif r.passed then
		check_set(spec.key, nil)
	else
		check_set(spec.key, { shut = spec.value })
	end
	luck_op(if r.passed then "pass" else "fail")
	self.passed = r.passed
	self.last_roll = r
	local said = roll_text(r)
	self:remember({ text = said, roll = true, passed = r.passed })
	self.dirty = true
	if not self.ui then
		say(`  [{said}]`)
	end
	if choice ~= nil then
		spots_roll(choice, i, if r.passed then "SUCCESS" else "FAILURE", said, r.passed)
		self:report()
		yield(PAUSE, "real", 1.6)
	end
end

-- Shows the options that are open and runs the one picked (nothing when none is open). An
-- option with a check rolls it once picked; `passed()` is how it went.
function Options:pick(): any
	local c = self.c
	if c.bark then
		fail("a bark can't have options (it is said while the player walks)")
	end
	local items, checks, infos, shut = {}, {}, {}, {}
	for _, it in self.items do
		local spec = c:check_of(it)
		if spec and spec.gone then
			-- A red check rolled before: that moment passed.
		elseif spec and spec.shut then
			table.insert(shut, { text = it.text, skill = spec.skill, above = spec.above })
		else
			table.insert(items, it)
			if spec then
				checks[#items] = spec
				table.insert(infos, {
					option = #items,
					skill = spec.skill,
					difficulty = spec.difficulty,
					target = spec.target,
					value = spec.value,
					bonuses = (function()
						local out = {}
						for k, b in spec.bonuses do
							out[k] = { b.n, b.why }
						end
						return if #out > 0 then out else nil
					end)(),
					dice = spec.dice,
					chance = spec.chance,
					red = spec.red,
				})
			end
		end
	end
	c.bonuses = nil
	c.shut_now = if #shut > 0 then shut else nil
	if #items == 0 then
		c.dirty = true
		return nil
	end
	local texts = {}
	for i, it in items do
		texts[i] = it.text
	end
	c.options_now = texts
	c.checks_now = if #infos > 0 then infos else nil
	c.dirty = true
	c.walked = nil
	local i = c:walk(items, texts, checks)
	if i == nil then
		if c.ui then
			c:show(texts, false, checks)
		else
			for n, t in texts do
				say(`  {n}. {t}{if checks[n] then `  [{check_label(checks[n])}]` else ""}`)
			end
		end
		i = yield(PAUSE, "options", #texts)
	end
	c.options_now = nil
	c.checks_now = nil
	c.shut_now = nil
	c.jumps = 0
	c.dirty = true
	local it = items[i]
	if it.key then
		once_seen(it.key, true)
	end
	c.picked = it.text
	c:remember({ text = it.text, choice = true, line = it.line })
	if not c.ui then
		say(`  > {it.text}`)
	end
	local forced, screen = c.forced, c.screen_result
	c.forced, c.screen_result = nil, nil
	local spec = checks[i]
	if spec then
		if forced == nil and D.checks.next ~= nil then
			forced = D.checks.next == true
			D.checks.next = nil
		end
		c:settle(spec, it.text, forced, screen, c.walked, i)
	end
	return it.body()
end

local Group = {}
Group.__index = Group

function Run:group(): any
	return setmetatable({ c = self, items = {} }, Group)
end

function Group:add(ok: any, score: number, key: string?, n: number, body: () -> any)
	if ok and (key == nil or not once_seen(key, false)) then
		table.insert(self.items, { score = score, key = key, line = n, body = body })
	end
end

-- A line group (`=>` lines): the one whose condition tests the most, the least seen on a tie.
function Group:run(): any
	local c = self.c
	local best, best_score, best_seen = nil, -1, math.huge
	for _, it in self.items do
		local seen = visit(`@{c.path}:{it.line}`, false)
		if it.score > best_score or (it.score == best_score and seen < best_seen) then
			best, best_score, best_seen = it, it.score, seen
		end
	end
	if best == nil then
		return nil
	end
	if best.key then
		once_seen(best.key, true)
	end
	visit(`@{c.path}:{best.line}`, true)
	return best.body()
end

-- Built-in commands: cutscene, quest, emit, hide. `c` is the conversation.
local Commands: { [string]: (c: any, ...any) -> any } = {
	-- `<<cutscene cutscenes/bridge>>` plays it (the screen hidden) and waits for its end;
	-- `<<cutscene path nowait>>` carries on at once.
	cutscene = function(c, path: string, how: string?): any
		c:hide()
		local h = cutscenes.play(path, if c.owner then { at = c.owner } else nil)
		if how ~= "nowait" then
			return function()
				return h.done
			end
		end
		return nil
	end,
	-- `<<quest start sealed_bridge>>`, `<<quest complete id>>`, `<<quest fail id>>`.
	quest = function(_, what: string, id: string): any
		if what ~= "start" and what ~= "complete" and what ~= "fail" then
			fail(`<<quest {what}>>: write <<quest start id>>, <<quest complete id>> or <<quest fail id>>`)
		end
		quest_ctl(what, id)
		return nil
	end,
	-- `<<emit signal values...>>`: the script that started the conversation emits a signal.
	emit = function(c, signal: string, ...: any): any
		local s = c.owner and c.owner.script
		if s == nil then
			fail("<<emit>> needs a conversation started by a script (dialogue.start)")
		end
		s:emit(signal, ...)
		return nil
	end,
	-- `<<hide>>`: hides the conversation screen until the next line.
	hide = function(c): any
		c:hide()
		return nil
	end,
}

-- `<<name args>>`: a built-in, one of dialogue.commands, or a method of the script that
-- started the conversation. A command that returns a function holds the conversation until
-- the function returns true.
function Run:command(name: string, args: { any }, n: number): any
	self.jumps = 0
	if name == "spots" then
		-- `<<spots 12 0.35>>`: the next options walked to have 12 s (then the #fallback),
		-- the world at 0.35 speed meanwhile. Nothing to read first: the lines stay up.
		self.spot_opts = { time = tonumber(args[1]), slow = tonumber(args[2]) }
		return nil
	elseif name == "bonus" then
		-- `<<bonus persuade 2 "You read the orders">>`: added to the next options' checks of
		-- that skill ("all" for every skill), with its reason shown.
		local skill, n = args[1], tonumber(args[2])
		if skill == nil or n == nil then
			fail(`<<bonus>>: write <<bonus persuade 2 "why">> (a skill or all, an amount, a reason)`)
		end
		self.bonuses = self.bonuses or {}
		table.insert(self.bonuses, {
			skill = text_of(skill),
			n = n,
			why = if args[3] ~= nil then text_of(args[3]) else "Bonus",
		})
		return nil
	elseif name == "watch" then
		-- `<<watch Soldier*>>`: they turn to follow the chooser through the next choice walked
		-- to, as the speaker does (`<<watch none>>`: nobody does).
		self.watch = self.watch or {}
		for _, a in args do
			table.insert(self.watch, text_of(a))
		end
		return nil
	elseif name == "barks" then
		-- `<<barks Murmur>>`: through the next choice walked to, node group Murmur is said as
		-- barks whenever the chooser steps into a spot or out of theirs, or time runs short.
		local title = text_of(args[1])
		self:load()
		if self.by[title] == nil then
			fail(`<<barks {title}>>: there is no node called '{title}' in {self.path}`)
		end
		self.barks_node = title
		return nil
	elseif name == "bark" then
		-- `<<bark Node>>`: said as barks once the lines before it are read; the conversation
		-- goes on at once.
		self:read_first()
		D.bark(self.path, text_of(args[1]), { owner = self.owner, chooser = self.opts.chooser })
		return nil
	end
	local f = Commands[name]
	if f == nil then
		local user = D.commands[name]
		if user ~= nil then
			f = function(_, ...)
				return user(...)
			end
		end
	end
	if f == nil then
		local s = self.owner and self.owner.script
		local m = s and s[name]
		if type(m) == "function" then
			f = function(_, ...)
				return m(s, ...)
			end
		end
	end
	if f == nil then
		fail(
			`<<{name}>>: no such command (a built-in, dialogue.commands.{name}, or a method of `
				.. "the script that started the conversation)"
		)
	end
	self:read_first()
	self.doing = name
	self.dirty = true
	local r = f(self, table.unpack(args))
	if type(r) == "function" then
		yield(PAUSE, "until", r)
	end
	self.doing = nil
	return nil
end

-- `<<wait 2>>`: the screen hidden for that many seconds of game time.
function Run:wait(seconds: any, n: number)
	self:read_first()
	self:hide()
	yield("wait", num(seconds))
end

function Run:main()
	if self.bark and self:pick_node(self.start, true) == nil then
		-- Nothing fits now: nothing is said (as a Valve query no rule matches).
		return
	end
	self:go(self.start)
	-- The last lines: the screen closes once the player reads past them.
	if self.ui and #self.buf > 0 then
		self:show(nil, false)
		yield(PAUSE, "closing")
	end
end

function Run:report()
	if not self.dirty then
		return
	end
	self.dirty = false
	local l = self.on_screen or self.last
	-- The field (`self.wait` is the <<wait>> command's method when there is none).
	local w = rawget(self, "wait")
	-- No dice (decided, not rolled): none rather than an empty table, which reads as a map.
	local roll = self.last_roll
	if roll and #roll.dice == 0 then
		roll = table.clone(roll)
		roll.dice = nil
	end
	report(self.id, {
		path = self.path,
		node = self.node or "",
		node_line = self.node_line or 0,
		speaker = l and l.speaker or "",
		text = l and l.text or "",
		options = if self.options_now and #self.options_now > 0 then self.options_now else nil,
		waiting = if self.done then "" elseif w == nil then "" else w.kind,
		taken = if #self.taken > 0 then self.taken else nil,
		lines = if #self.said > 0 then self.said else nil,
		done = self.done,
		error = self.error,
		bark = self.bark or nil,
		checks = self.checks_now,
		shut = self.shut_now,
		roll = roll,
	})
end

function Run:finish(err: string?)
	if self.done then
		return
	end
	self.done = true
	self.wait = nil
	self.options_now = nil
	if self.spots then
		spots_end(self.spots, nil)
		self.spots = nil
	end
	if err then
		self.error = err
		log_error(self.path, err)
	end
	self:hide()
	self.dirty = true
	self:report()
end

-- An error inside the conversation, with the .yarn line it happened at.
function Run:failed(e: any)
	local message = tostring(e)
	if not string.find(message, "%.yarn:%d+:") then
		for level = 0, 40 do
			local src, line = frame_info(self.co, level, "sl")
			if src == nil and level > 0 then
				break
			end
			src = string.gsub(src or "", "^[@=]", "")
			if string.sub(src, -5) == ".yarn" then
				message = `{src}:{line}: {message}`
				break
			end
		end
	end
	self:finish(message)
end

function Run:resume(...: any)
	local before = owner_now()
	set_owner(self.owner)
	local ok, a, b, c, d = resume(self.co, ...)
	set_owner(before)
	if not ok then
		self:failed(a)
		return
	end
	if status(self.co) == "dead" then
		self:finish(nil)
		return
	end
	if a == PAUSE then
		self.wait = { kind = b, n = c, check = c, choice = d }
	elseif a == "wait" then
		local secs = tonumber(b) or 0
		self.wait = { kind = "time", left = secs, total = secs }
	else
		-- task.synchronize and the like: on next frame.
		self.wait = { kind = "frame" }
	end
	self.dirty = true
end

-- What the screen shows now (nil once it closed).
local function screen_now(): any
	local ok, st = pcall(call_api, "dialogue.state", {})
	if ok then
		return st
	end
	return nil
end

function Run:follow(st: any)
	local i = st.node and tonumber(string.match(st.node, "^l(%d+)$"))
	local l = i and self.lines_shown and self.lines_shown[i]
	if l and l ~= self.on_screen then
		self.on_screen = l
		self.dirty = true
	end
end

function Run:step(dt: number)
	local pick, stop, forced = take(self.id)
	if self.next_pick then
		pick, self.next_pick = self.next_pick, nil
		forced, self.next_forced = self.next_forced, nil
	end
	if pick ~= nil then
		self.forced = forced
	end
	if stop or self.stopping then
		self:finish(nil)
		return
	end
	local w = rawget(self, "wait")
	local arg = nil
	if w then
		local kind = w.kind
		if kind == "time" then
			w.left -= dt
			if w.left > 0 then
				return
			end
			arg = w.total - w.left
		elseif kind == "real" then
			-- Real seconds (a bark's line, a roll shown), whatever the world's speed.
			w.n -= real_dt()
			if w.n > 0 then
				return
			end
		elseif kind == "options" then
			if pick == nil and self.ui then
				local st = screen_now()
				if st == nil then
					-- Closed under it (another screen took its place): the conversation ends.
					self:finish(nil)
					return
				end
				self:follow(st)
				-- p<i>: option i (its check passed, if it has one); f<i>: its check failed.
				local k, i = string.match(st.node or "", "^([pf])(%d+)$")
				pick = tonumber(i)
				if pick ~= nil then
					self.screen_result = k == "p"
				end
			end
			if pick == nil then
				return
			end
			if pick < 1 or pick > w.n then
				warn_log(`{self.path}: there is no option {pick} (1 to {w.n})`)
				return
			end
			arg = pick
		elseif kind == "spots" then
			if pick == nil then
				local p, near, moments = spots_poll(w.choice)
				if near then
					for _, name in near do
						self:near_spot(w.choice, name)
					end
				end
				if moments then
					-- Of several at once (a step from one spot straight into another), the last.
					self:moment(moments[#moments])
				end
				if self.bark_next and (self.barking == nil or self.barking.done) then
					self:moment(self.bark_next)
				end
				pick = p
			end
			if pick == nil then
				return
			end
			if pick < 1 or pick > w.n then
				warn_log(`{self.path}: there is no option {pick} (1 to {w.n})`)
				return
			end
			arg = pick
		elseif kind == "reading" or kind == "closing" then
			if self.ui then
				local st = screen_now()
				if st ~= nil and st.node ~= "h" then
					self:follow(st)
					return
				end
			elseif pick == nil then
				-- Headless, a line at a time: waits for a pick (any) to go on.
				return
			end
		elseif kind == "until" then
			local ok, done = pcall(w.check)
			if not ok then
				self:failed(done)
				return
			end
			if not done then
				return
			end
		end
	end
	self.wait = nil
	self.on_screen = nil
	self:resume(arg)
end

-- What `dialogue.start` returns.
local Handle = {}
local handle_methods: { [string]: any } = {}
Handle.__index = function(self: any, key: string): any
	local m = handle_methods[key]
	if m then
		return m
	end
	local c = rawget(self, "c")
	if key == "done" then
		return c.done
	elseif key == "node" then
		return c.node
	elseif key == "line" then
		local l = c.on_screen or c.last
		return l and l.text
	elseif key == "speaker" then
		local l = c.on_screen or c.last
		return l and l.speaker
	elseif key == "options" then
		return c.options_now and table.clone(c.options_now)
	elseif key == "waiting" then
		return if c.wait then c.wait.kind else nil
	elseif key == "error" then
		return c.error
	elseif key == "taken" then
		return table.clone(c.taken)
	elseif key == "path" then
		return c.path
	elseif key == "passed" then
		return c.passed == true
	elseif key == "roll" then
		return c.last_roll and table.clone(c.last_roll)
	end
	return nil
end
Handle.__tostring = function(self: any): string
	return `conversation {rawget(self, "c").path}`
end

-- Picks option `i` (1 is the first) when it shows options; `pass` (true or false) decides
-- its skill check instead of the dice.
function handle_methods.choose(self: any, i: number, pass: boolean?): any
	local c = rawget(self, "c")
	c.next_pick = i
	c.next_forced = pass
	return self
end

-- Ends it (the screen closes).
function handle_methods.stop(self: any)
	rawget(self, "c"):finish(nil)
end

-- Waits (in a task) until it ends; true unless it ended with an error.
function handle_methods.wait(self: any): boolean
	local c = rawget(self, "c")
	while not c.done do
		task.wait()
	end
	return c.error == nil
end

-- Starts the conversation at node `node` ("Start") of the .yarn file at `path`
-- ("dialogue/keeper"). Options: headless (lines to the log, picks by handle:choose or
-- story.choose, not the screen), step (headless, a line at a time: a pick goes on),
-- speakers ({ Keeper = { title, portrait, side } }), owner (the entity whose script runs its
-- commands; the calling script's), chooser (who walks to `#spot:` options, an entity or its
-- name; the player).
local function new_run(what: string, path: string, node: string?, opts: any?, bark: boolean): any
	local o = opts or {}
	if type(o.chooser) == "string" then
		local who = world.find(o.chooser)
		if who == nil then
			fail(`{what}: there is no entity '{o.chooser}' to be the chooser`)
		end
		o.chooser = who
	end
	local c = setmetatable({
		id = new_id(),
		path = path,
		start = node or "Start",
		opts = o,
		owner = if o.owner ~= nil then o.owner else owner_now(),
		ui = not bark and not o.headless and has_api("ui.show") and has_api("dialogue.state"),
		bark = bark,
		buf = {},
		taken = {},
		jumps = 0,
		said = {},
		dirty = true,
		done = false,
	}, Run)
	c:load()
	c.co = coroutine.create(function()
		c:main()
	end)
	table.insert(ORDER, c)
	-- The first lines show this frame.
	c:resume()
	c:report()
	return setmetatable({ id = c.id, c = c }, Handle)
end

function D.start(path: string, node: string?, opts: any?): any
	return new_run("dialogue.start", path, node, opts, false)
end

-- Says node `node` of `path` as barks: each line over its speaker's head (looked for among
-- the owner's children first, then everywhere) for a moment, one after another, while play
-- goes on. Of a node group, the one that fits best now; nil when none does. Options: owner
-- (whose script runs its commands; the calling script's), chooser (for standing_at()).
function D.bark(path: string, node: string, opts: any?): any
	local h = new_run("dialogue.bark", path, node, opts, true)
	if rawget(h, "c").done and #rawget(h, "c").said == 0 then
		return nil
	end
	return h
end

-- Whether a conversation is going on (barks aside).
function D.running(): boolean
	for _, c in ORDER do
		if not c.done and not c.bark then
			return true
		end
	end
	return false
end

-- ------------------------------------------------------------------------------ quests

quests = {}

-- Starts a quest (quests/<id>.quest) by hand; returns its state.
function quests.start(id: string): string
	return quest_ctl("start", id)
end

function quests.complete(id: string): string
	return quest_ctl("complete", id)
end

function quests.fail(id: string): string
	return quest_ctl("fail", id)
end

-- "inactive", "active", "completed" or "failed".
function quests.state(id: string): string
	return quest_ctl("state", id)
end

local quest_conditions: any = nil
local quest_version = 0
local Q = setmetatable({}, { __index = Ops })

-- Each quest's conditions now: { start, fail, done = {...}, n = {...} } or { error }.
local function eval_quests(ids: { string }): any
	local m, v = quest_module(quest_version)
	if m ~= nil then
		quest_conditions = m
	end
	quest_version = v
	local out = {}
	if quest_conditions == nil then
		return out
	end
	for _, id in ids do
		local q = quest_conditions[id]
		if q then
			local ok, r = pcall(function()
				local r: any = { done = {}, n = {} }
				if q.start_when then
					r.start = not not q.start_when(Q, F)
				end
				if q.fail_when then
					r.fail = not not q.fail_when(Q, F)
				end
				for i, o in q.objectives do
					r.done[i] = if o.done then not not o.done(Q, F) else false
					r.n[i] = if o.n then (tonumber(o.n(Q, F)) or 0) else 0
				end
				return r
			end)
			out[id] = if ok then r else { error = tostring(r) }
		end
	end
	return out
end

-- Once a frame (story.rs's run_story): the conversations go on, the quests' conditions are
-- evaluated (only that with quests_only). Returns { quests = {...}, live = { ids of the
-- conversations going } }.
function __story_tick(ids: { string }, quests_only: boolean?): any
	if quests_only then
		return { quests = eval_quests(ids) }
	end
	local dt = dt_now()
	for _, c in table.clone(ORDER) do
		if not c.done then
			c:step(dt)
		end
		c:report()
	end
	local live = {}
	local keep = {}
	for _, c in ORDER do
		if not c.done then
			table.insert(keep, c)
			table.insert(live, c.id)
		end
	end
	ORDER = keep
	return { quests = eval_quests(ids), live = live }
end

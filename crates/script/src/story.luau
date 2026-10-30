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

function Ops:call(name: string, ...: any): any
	if name == "standing_at" then
		-- Where the conversation's chooser (the player) stands: `<<if standing_at("ear")>>`.
		local opts = rawget(self, "opts")
		return standing_at(opts and opts.chooser, text_of((...)))
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
function Run:pick_node(title: string): any
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
	if best == nil then
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
function Run:show(choices: { string }?, hold: boolean)
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
function Run:walk(items: { any }, texts: { string }): number?
	local list = {}
	local any = false
	for n, it in items do
		local spot = tag_value(it.tags, "spot")
		list[n] = { text = texts[n], spot = spot, fallback = tag_value(it.tags, "fallback") ~= nil }
		any = any or spot ~= nil
	end
	local o = self.spot_opts or {}
	self.spot_opts = nil
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
	}
	local id, why = spots_begin(self.id, spec, self.opts.chooser, self.owner)
	if id == nil then
		warn_log(`{self.path}: {why}; the options show as a menu`)
		return nil
	end
	self.spots = id
	if not self.ui then
		say("  (walk to one)")
		for n, it in list do
			say(`  {n}. {it.text}{if it.spot then ` [{it.spot}]` else ""}{if it.fallback then " (fallback)" else ""}`)
		end
	end
	self.dirty = true
	local i = yield(PAUSE, "spots", #texts, id)
	self.spots = nil
	spots_end(id, i)
	return i
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

-- Shows the options that are open and runs the one picked (nothing when none is open).
function Options:pick(): any
	local items = self.items
	if #items == 0 then
		return nil
	end
	local c = self.c
	local texts = {}
	for i, it in items do
		texts[i] = it.text
	end
	c.options_now = texts
	c.dirty = true
	local i = c:walk(items, texts)
	if i == nil then
		if c.ui then
			c:show(texts, false)
		else
			for n, t in texts do
				say(`  {n}. {t}`)
			end
		end
		i = yield(PAUSE, "options", #texts)
	end
	c.options_now = nil
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
	local w = self.wait
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
	local pick, stop = take(self.id)
	if self.next_pick then
		pick, self.next_pick = self.next_pick, nil
	end
	if stop or self.stopping then
		self:finish(nil)
		return
	end
	local w = self.wait
	local arg = nil
	if w then
		local kind = w.kind
		if kind == "time" then
			w.left -= dt
			if w.left > 0 then
				return
			end
			arg = w.total - w.left
		elseif kind == "options" then
			if pick == nil and self.ui then
				local st = screen_now()
				if st == nil then
					-- Closed under it (another screen took its place): the conversation ends.
					self:finish(nil)
					return
				end
				self:follow(st)
				pick = st.node and tonumber(string.match(st.node, "^p(%d+)$"))
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
				local p, near = spots_poll(w.choice)
				if near then
					for _, name in near do
						self:near_spot(w.choice, name)
					end
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
	end
	return nil
end
Handle.__tostring = function(self: any): string
	return `conversation {rawget(self, "c").path}`
end

-- Picks option `i` (1 is the first) when it shows options.
function handle_methods.choose(self: any, i: number): any
	rawget(self, "c").next_pick = i
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
function D.start(path: string, node: string?, opts: any?): any
	local o = opts or {}
	if type(o.chooser) == "string" then
		local who = world.find(o.chooser)
		if who == nil then
			fail(`dialogue.start: there is no entity '{o.chooser}' to be the chooser`)
		end
		o.chooser = who
	end
	local c = setmetatable({
		id = new_id(),
		path = path,
		start = node or "Start",
		opts = o,
		owner = if o.owner ~= nil then o.owner else owner_now(),
		ui = not o.headless and has_api("ui.show") and has_api("dialogue.state"),
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

-- Whether a conversation is going on.
function D.running(): boolean
	for _, c in ORDER do
		if not c.done then
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

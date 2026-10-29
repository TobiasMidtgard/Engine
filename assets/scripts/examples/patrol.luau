--!strict
-- A patrol drone that flies a loop through three waypoints. The waypoints are props with
-- `@handle`, so the editor draws a handle at each in the scene view: drag them to reshape the
-- route (Unreal's "Show 3D Widget" on a vector variable). Its gizmos draw the route, in the
-- entity's own space, and while selected the way it flies. Put it on an empty entity; it builds
-- the drone (`construct`, so it shows in the editor too). See "In the editor's view" in the
-- scripts README.
local Patrol = {}
Patrol.props = {
	a = vec3(-3, 2.2, 0), -- first waypoint, from the entity @handle
	b = vec3(3, 2.6, -1), -- second waypoint @handle
	c = vec3(0, 3.2, -4), -- third waypoint @handle
	speed = 2.2, -- along the route @range 0.2 10 @unit m/s
}

local BODY = { base_color = { 0.1, 0.11, 0.13, 1 }, metallic = 0.7, perceptual_roughness = 0.35 }
local LAMP = { base_color = { 0.02, 0.03, 0.04, 1 }, emissive = { 0.3, 0.9, 1 }, emissive_strength = 30 }
local ROTOR = { base_color = { 0.5, 0.52, 0.55, 1 }, metallic = 0.9, perceptual_roughness = 0.3 }

local function part(name: string, shape: any, at: Vec3, material: any): any
	return {
		name = name,
		components = {
			Transform = { translation = at },
			MeshRenderer = { mesh = { shape = shape }, material = { inline = material } },
		},
	}
end

-- The waypoints in order.
local function route(self: any): { Vec3 }
	return { self.props.a, self.props.b, self.props.c }
end

-- The point `d` meters along the closed route, and the direction there.
local function along(points: { Vec3 }, d: number): (Vec3, Vec3)
	local total = 0
	for i, p in points do
		total += (points[i % #points + 1] - p):length()
	end
	d = if total > 0 then d % total else 0
	for i, p in points do
		local q = points[i % #points + 1]
		local leg = (q - p):length()
		if d <= leg and leg > 0 then
			return p:lerp(q, d / leg), (q - p):unit()
		end
		d -= leg
	end
	return points[1], vec3.forward
end

-- The drone: a body, four rotor rings and a lamp underneath.
function Patrol:construct()
	local rotors: { any } = {}
	for _, x in { -0.32, 0.32 } do
		for _, z in { -0.32, 0.32 } do
			table.insert(rotors, part("Rotor", { Torus = { radius = 0.16, tube = 0.018, segments = 24 } }, vec3(x, 0.06, z), ROTOR))
		end
	end
	table.insert(rotors, part("Body", { RoundedCuboid = { size = { 0.42, 0.14, 0.42 }, radius = 0.05, segments = 0 } }, vec3.zero, BODY))
	table.insert(rotors, part("Lamp", { Sphere = { radius = 0.07, segments = 12 } }, vec3(0, -0.09, 0), LAMP))
	self.drone = self.entity:spawn({
		name = "Drone",
		components = {
			Transform = { translation = self.props.a },
			PointLight = { color = { 0.3, 0.9, 1 }, intensity_lm = 500, range = 6, radius = 0.05 },
		},
		children = rotors,
	})
end

function Patrol:start()
	self.flown = 0 -- meters along the route
end

function Patrol:update(dt: number)
	self.flown += self.props.speed * dt
	local at, dir = along(route(self), self.flown)
	local bob = 0.06 * math.sin(self.flown * 2.5)
	self.drone.position = at + vec3(0, bob, 0)
	self.drone.rotation = quat.look(dir:flat():unit())
end

-- Its route, in its own space: always while gizmos show.
function Patrol:draw_gizmos()
	local points = route(self)
	draw.path(points, { color = "cyan", closed = true })
	for _, p in points do
		draw.point(p, { color = "cyan", size = 0.3 })
		-- Down to the ground, to see where it hovers over.
		draw.line(p, vec3(p.x, 0, p.z), { color = "cyan", alpha = 0.35 })
	end
end

-- Selected: which way it flies each leg, and where it is going now.
function Patrol:draw_gizmos_selected()
	local points = route(self)
	for i, p in points do
		local q = points[i % #points + 1]
		draw.arrow(p:lerp(q, 0.35), p:lerp(q, 0.65), { color = "yellow", on_top = true })
	end
	if self.flown then
		local at = along(points, self.flown)
		draw.sphere(at, 0.45, "yellow")
	end
end

return Patrol

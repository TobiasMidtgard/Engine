-- vec3 and quat, in Luau so reading a field and doing arithmetic never leave the VM (11x faster
-- field reads and 2-3x faster operators than the Rust userdata they replace; see math.rs).
-- Values are frozen tables: `v.x` is a plain field read, and `v.x = 1` is an error, so a vec3
-- passed around is never changed behind anyone's back. Components are f64 (positions far
-- from the origin keep their millimeters); quats are unit quaternions (x, y, z, w).
--
-- Loaded once per VM before the globals are sandboxed; returns the two metatables, which Rust
-- uses to recognize and make these values (math.rs).

-- seal(t): freezes `t` like `table.freeze`, without its checks (math.rs): those cost more
-- than making the table. Values are made inline, `seal(setmetatable({ x = .., .. }, Vec3))`,
-- rather than through a helper function: a Luau call costs about as much again. A table
-- literal copies a template with the keys already in place, which beats filling one from C.
local seal = ...
local freeze, setmetatable, getmetatable = table.freeze, setmetatable, getmetatable
local type, tonumber, error = type, tonumber, error
local sqrt, sin, cos = math.sqrt, math.sin, math.cos
local asin, acos, atan2 = math.asin, math.acos, math.atan2
local abs, min, max, clamp = math.abs, math.min, math.max, math.clamp

local Vec3 = { __name = "vec3" }
local V = {} -- vec3 methods
Vec3.__index = V
local Quat = { __name = "quat" }
local Q = {} -- quat methods
Quat.__index = Q

local function new(x: number, y: number, z: number)
	return seal(setmetatable({ x = x, y = y, z = z }, Vec3))
end

local ZERO = new(0, 0, 0)
local IDENTITY = seal(setmetatable({ x = 0, y = 0, z = 0, w = 1 }, Quat))

local function kind(v: any): string
	local mt = getmetatable(v)
	return if type(mt) == "table" and mt.__name then mt.__name else typeof(v)
end

-- A plain table {x, y, z} or {x = .., y = .., z = ..}, or Luau's own `vector`, as a vec3;
-- anything else is an error at the line that called `what`.
local function coerce(v: any, what: string)
	local mt = getmetatable(v)
	if mt == Vec3 then
		return v
	end
	local t = type(v)
	if t == "table" and mt == nil then
		return seal(setmetatable({
			x = tonumber(v.x or v[1]) or 0,
			y = tonumber(v.y or v[2]) or 0,
			z = tonumber(v.z or v[3]) or 0,
		}, Vec3))
	elseif t == "vector" then
		return seal(setmetatable({ x = v.x, y = v.y, z = v.z }, Vec3))
	end
	error(`{what}: expected a vec3 (or a table {"{x, y, z}"}), got {kind(v)}`, 3)
end

function Vec3.__add(a, b)
	if getmetatable(a) ~= Vec3 or getmetatable(b) ~= Vec3 then
		a, b = coerce(a, "vec3 + vec3"), coerce(b, "vec3 + vec3")
	end
	return seal(setmetatable({ x = a.x + b.x, y = a.y + b.y, z = a.z + b.z }, Vec3))
end

function Vec3.__sub(a, b)
	if getmetatable(a) ~= Vec3 or getmetatable(b) ~= Vec3 then
		a, b = coerce(a, "vec3 - vec3"), coerce(b, "vec3 - vec3")
	end
	return seal(setmetatable({ x = a.x - b.x, y = a.y - b.y, z = a.z - b.z }, Vec3))
end

function Vec3.__mul(a, b)
	if type(b) == "number" then
		return seal(setmetatable({ x = a.x * b, y = a.y * b, z = a.z * b }, Vec3))
	elseif type(a) == "number" then
		return seal(setmetatable({ x = a * b.x, y = a * b.y, z = a * b.z }, Vec3))
	end
	a, b = coerce(a, "vec3 * vec3"), coerce(b, "vec3 * vec3")
	return seal(setmetatable({ x = a.x * b.x, y = a.y * b.y, z = a.z * b.z }, Vec3))
end

function Vec3.__div(a, b)
	if type(b) == "number" then
		return seal(setmetatable({ x = a.x / b, y = a.y / b, z = a.z / b }, Vec3))
	end
	a, b = coerce(a, "vec3 / vec3"), coerce(b, "vec3 / vec3")
	return seal(setmetatable({ x = a.x / b.x, y = a.y / b.y, z = a.z / b.z }, Vec3))
end

function Vec3.__unm(a)
	return seal(setmetatable({ x = -a.x, y = -a.y, z = -a.z }, Vec3))
end

function Vec3.__eq(a, b)
	return getmetatable(a) == Vec3
		and getmetatable(b) == Vec3
		and a.x == b.x
		and a.y == b.y
		and a.z == b.z
end

function Vec3.__tostring(v)
	return `vec3({v.x}, {v.y}, {v.z})`
end

function V.length(v)
	return sqrt(v.x * v.x + v.y * v.y + v.z * v.z)
end

function V.length_squared(v)
	return v.x * v.x + v.y * v.y + v.z * v.z
end

-- The same direction, 1 long (zero stays zero).
function V.unit(v)
	local l = sqrt(v.x * v.x + v.y * v.y + v.z * v.z)
	if l > 0 and l < math.huge then
		return seal(setmetatable({ x = v.x / l, y = v.y / l, z = v.z / l }, Vec3))
	end
	return ZERO
end

function V.dot(a, b)
	b = coerce(b, "vec3:dot")
	return a.x * b.x + a.y * b.y + a.z * b.z
end

function V.cross(a, b)
	b = coerce(b, "vec3:cross")
	return seal(setmetatable({
		x = a.y * b.z - a.z * b.y,
		y = a.z * b.x - a.x * b.z,
		z = a.x * b.y - a.y * b.x,
	}, Vec3))
end

function V.distance(a, b)
	b = coerce(b, "vec3:distance")
	local x, y, z = a.x - b.x, a.y - b.y, a.z - b.z
	return sqrt(x * x + y * y + z * z)
end

function V.lerp(a, b, t: number)
	b = coerce(b, "vec3:lerp")
	return seal(setmetatable({
		x = a.x + (b.x - a.x) * t,
		y = a.y + (b.y - a.y) * t,
		z = a.z + (b.z - a.z) * t,
	}, Vec3))
end

function V.abs(v)
	return seal(setmetatable({ x = abs(v.x), y = abs(v.y), z = abs(v.z) }, Vec3))
end

function V.min(a, b)
	b = coerce(b, "vec3:min")
	return seal(setmetatable({ x = min(a.x, b.x), y = min(a.y, b.y), z = min(a.z, b.z) }, Vec3))
end

function V.max(a, b)
	b = coerce(b, "vec3:max")
	return seal(setmetatable({ x = max(a.x, b.x), y = max(a.y, b.y), z = max(a.z, b.z) }, Vec3))
end

-- On the ground plane (y = 0).
function V.flat(v)
	return seal(setmetatable({ x = v.x, y = 0, z = v.z }, Vec3))
end

function V.with_x(v, x: number)
	return seal(setmetatable({ x = x, y = v.y, z = v.z }, Vec3))
end

function V.with_y(v, y: number)
	return seal(setmetatable({ x = v.x, y = y, z = v.z }, Vec3))
end

function V.with_z(v, z: number)
	return seal(setmetatable({ x = v.x, y = v.y, z = z }, Vec3))
end

-- Turned about the up axis (counterclockwise seen from above, like `quat.axis_angle(vec3.up, a)`).
function V.rotate_y(v, angle: number)
	local s, c = sin(angle), cos(angle)
	return seal(setmetatable({ x = v.x * c + v.z * s, y = v.y, z = -v.x * s + v.z * c }, Vec3))
end

function V.unpack(v)
	return v.x, v.y, v.z
end

local vec3 = {
	zero = ZERO,
	one = new(1, 1, 1),
	up = new(0, 1, 0),
	down = new(0, -1, 0),
	right = new(1, 0, 0),
	left = new(-1, 0, 0),
	-- Cameras and lights look along -Z (AGENTS.md, "Units and coordinates").
	forward = new(0, 0, -1),
	back = new(0, 0, 1),
}

function vec3.lerp(a, b, t: number)
	return V.lerp(coerce(a, "vec3.lerp"), b, t)
end

-- A point on the ground circle: `angle` radians from +X towards +Z, `radius` (1) out.
function vec3.from_angle(angle: number, radius: number?)
	local r = radius or 1
	return seal(setmetatable({ x = cos(angle) * r, y = 0, z = sin(angle) * r }, Vec3))
end

-- Whether `v` is a vec3 (`typeof` says "table" for these).
function vec3.is(v: any): boolean
	return getmetatable(v) == Vec3
end

setmetatable(vec3, {
	__call = function(_, x: any, y: number?, z: number?)
		if x ~= nil and type(x) ~= "number" and y == nil then
			return coerce(x, "vec3()")
		end
		return seal(setmetatable({ x = x or 0, y = y or 0, z = z or 0 }, Vec3))
	end,
})

local function unitq(x: number, y: number, z: number, w: number)
	local l = sqrt(x * x + y * y + z * z + w * w)
	if l > 0 and l < math.huge then
		return seal(setmetatable({ x = x / l, y = y / l, z = z / l, w = w / l }, Quat))
	end
	return IDENTITY
end

local function quat_of(q: any, what: string)
	if getmetatable(q) == Quat then
		return q
	end
	error(`{what}: expected a quat, got {kind(q)}`, 3)
end

-- `a * b` turns by b, then by a; `q * v` turns the vector.
-- (The checks and the rotation are written out here rather than calling quat_of and Q.rotate:
-- this is the hot one, and each call would cost about as much as the arithmetic.)
function Quat.__mul(a, b)
	if getmetatable(a) ~= Quat then
		a = quat_of(a, "quat * quat")
	end
	local mb = getmetatable(b)
	if mb == Quat then
		return seal(setmetatable({
			x = a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
			y = a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
			z = a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
			w = a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
		}, Quat))
	elseif mb ~= Vec3 then
		b = coerce(b, "quat * vec3")
	end
	-- As Q.rotate.
	local qx, qy, qz, qw = a.x, a.y, a.z, a.w
	local vx, vy, vz = b.x, b.y, b.z
	local tx = 2 * (qy * vz - qz * vy)
	local ty = 2 * (qz * vx - qx * vz)
	local tz = 2 * (qx * vy - qy * vx)
	return seal(setmetatable({
		x = vx + qw * tx + (qy * tz - qz * ty),
		y = vy + qw * ty + (qz * tx - qx * tz),
		z = vz + qw * tz + (qx * ty - qy * tx),
	}, Vec3))
end

function Quat.__eq(a, b)
	return getmetatable(a) == Quat
		and getmetatable(b) == Quat
		and a.x == b.x
		and a.y == b.y
		and a.z == b.z
		and a.w == b.w
end

function Quat.__tostring(q)
	return `quat({q.x}, {q.y}, {q.z}, {q.w})`
end

function Q.rotate(q, v)
	if getmetatable(v) ~= Vec3 then
		v = coerce(v, "quat:rotate")
	end
	-- v + w t + q.xyz x t, with t = 2 (q.xyz x v)
	local qx, qy, qz, qw = q.x, q.y, q.z, q.w
	local tx = 2 * (qy * v.z - qz * v.y)
	local ty = 2 * (qz * v.x - qx * v.z)
	local tz = 2 * (qx * v.y - qy * v.x)
	return seal(setmetatable({
		x = v.x + qw * tx + (qy * tz - qz * ty),
		y = v.y + qw * ty + (qz * tx - qx * tz),
		z = v.z + qw * tz + (qx * ty - qy * tx),
	}, Vec3))
end

function Q.inverse(q)
	return seal(setmetatable({ x = -q.x, y = -q.y, z = -q.z, w = q.w }, Quat))
end

function Q.dot(a, b)
	b = quat_of(b, "quat:dot")
	return a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
end

-- The shortest way from a to b, t of the way (0 is a, 1 is b).
function Q.slerp(a, b, t: number)
	b = quat_of(b, "quat:slerp")
	local bx, by, bz, bw = b.x, b.y, b.z, b.w
	local d = a.x * bx + a.y * by + a.z * bz + a.w * bw
	if d < 0 then
		bx, by, bz, bw, d = -bx, -by, -bz, -bw, -d
	end
	if d > 1 - 1e-6 then
		local x, y, z, w = a.x + (bx - a.x) * t, a.y + (by - a.y) * t, a.z + (bz - a.z) * t, a.w + (bw - a.w) * t
		return unitq(x, y, z, w)
	end
	local theta = acos(d)
	local s = sin(theta)
	local sa, sb = sin(theta * (1 - t)) / s, sin(theta * t) / s
	return seal(setmetatable({
		x = a.x * sa + bx * sb,
		y = a.y * sa + by * sb,
		z = a.z * sa + bz * sb,
		w = a.w * sa + bw * sb,
	}, Quat))
end

-- Yaw (about up), pitch (about right), roll (about forward), in radians: the angles
-- `quat.euler` takes.
function Q.euler(q)
	local x, y, z, w = q.x, q.y, q.z, q.w
	local yaw = atan2(2 * (x * z + w * y), 1 - 2 * (x * x + y * y))
	local pitch = asin(clamp(2 * (w * x - y * z), -1, 1))
	local roll = atan2(2 * (x * y + w * z), 1 - 2 * (x * x + z * z))
	return yaw, pitch, roll
end

-- The angle between two rotations, in radians (0 to pi).
function Q.angle_to(a, b)
	b = quat_of(b, "quat:angle_to")
	return 2 * acos(min(abs(a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w), 1))
end

function Q.unpack(q)
	return q.x, q.y, q.z, q.w
end

local quat = { identity = IDENTITY }

local function axis_angle(x: number, y: number, z: number, angle: number)
	local l = sqrt(x * x + y * y + z * z)
	if l > 0 and l < math.huge then
		x, y, z = x / l, y / l, z / l
	else
		x, y, z = 0, 1, 0
	end
	local s = sin(angle * 0.5)
	return seal(setmetatable({ x = x * s, y = y * s, z = z * s, w = cos(angle * 0.5) }, Quat))
end

function quat.axis_angle(axis, angle: number)
	axis = coerce(axis, "quat.axis_angle")
	return axis_angle(axis.x, axis.y, axis.z, angle)
end

-- Yaw about up, then pitch about the turned right axis, then roll about the turned forward
-- axis (radians).
function quat.euler(yaw: number, pitch: number?, roll: number?)
	local sy, cy = sin(yaw * 0.5), cos(yaw * 0.5)
	local sp, cp = sin((pitch or 0) * 0.5), cos((pitch or 0) * 0.5)
	local sr, cr = sin((roll or 0) * 0.5), cos((roll or 0) * 0.5)
	-- (yaw about Y) * (pitch about X) * (roll about Z)
	return seal(setmetatable({
		x = cy * sp * cr + sy * cp * sr,
		y = sy * cp * cr - cy * sp * sr,
		z = cy * cp * sr - sy * sp * cr,
		w = cy * cp * cr + sy * sp * sr,
	}, Quat))
end

-- The rotation from a matrix with these columns (orthonormal): the right, up and back axes.
local function from_axes(m00, m01, m02, m10, m11, m12, m20, m21, m22)
	if m22 <= 0 then
		local dif10, omm22 = m11 - m00, 1 - m22
		if dif10 <= 0 then
			local four = omm22 - dif10
			local inv = 0.5 / sqrt(four)
			return seal(setmetatable({
				x = four * inv,
				y = (m01 + m10) * inv,
				z = (m02 + m20) * inv,
				w = (m12 - m21) * inv,
			}, Quat))
		end
		local four = omm22 + dif10
		local inv = 0.5 / sqrt(four)
		return seal(setmetatable({
			x = (m01 + m10) * inv,
			y = four * inv,
			z = (m12 + m21) * inv,
			w = (m20 - m02) * inv,
		}, Quat))
	end
	local sum10, opm22 = m11 + m00, 1 + m22
	if sum10 <= 0 then
		local four = opm22 - sum10
		local inv = 0.5 / sqrt(four)
		return seal(setmetatable({
			x = (m02 + m20) * inv,
			y = (m12 + m21) * inv,
			z = four * inv,
			w = (m01 - m10) * inv,
		}, Quat))
	end
	local four = opm22 + sum10
	local inv = 0.5 / sqrt(four)
	return seal(setmetatable({
		x = (m12 - m21) * inv,
		y = (m20 - m02) * inv,
		z = (m01 - m10) * inv,
		w = four * inv,
	}, Quat))
end

-- The rotation that turns forward (-Z) to `direction`, keeping its top as close to `up`
-- (vec3.up) as it can.
function quat.look(direction, up)
	local f = V.unit(coerce(direction, "quat.look"))
	if f == ZERO then
		f = vec3.forward
	end
	local u = if up == nil then vec3.up else coerce(up, "quat.look")
	local bx, by, bz = -f.x, -f.y, -f.z -- back
	local rx, ry, rz = u.y * bz - u.z * by, u.z * bx - u.x * bz, u.x * by - u.y * bx
	local rl = sqrt(rx * rx + ry * ry + rz * rz)
	if rl > 1e-12 then
		rx, ry, rz = rx / rl, ry / rl, rz / rl
	else
		rx, ry, rz = 1, 0, 0
	end
	local ux, uy, uz = by * rz - bz * ry, bz * rx - bx * rz, bx * ry - by * rx
	return from_axes(rx, ry, rz, ux, uy, uz, bx, by, bz)
end

-- The shortest rotation that turns direction `from` to direction `to`.
function quat.between(from, to)
	local a = V.unit(coerce(from, "quat.between"))
	local b = V.unit(coerce(to, "quat.between"))
	if a == ZERO then
		a = vec3.up
	end
	if b == ZERO then
		b = vec3.up
	end
	local d = a.x * b.x + a.y * b.y + a.z * b.z
	if d > 1 - 1e-12 then
		return IDENTITY
	elseif d < -1 + 1e-12 then
		-- Half a turn about any axis square to `from`.
		local axis = if abs(a.x) < 0.9 then V.cross(a, vec3.right) else V.cross(a, vec3.up)
		return axis_angle(axis.x, axis.y, axis.z, math.pi)
	end
	return unitq(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x, 1 + d)
end

-- Whether `q` is a quat (`typeof` says "table" for these).
function quat.is(q: any): boolean
	return getmetatable(q) == Quat
end

setmetatable(quat, {
	__call = function(_, x: number, y: number, z: number, w: number)
		return unitq(x, y, z, w)
	end,
})

-- Scripts share these: nobody may change what `+` means for the others.
for _, t in { Vec3, V, Quat, Q, getmetatable(vec3), getmetatable(quat) } do
	freeze(t)
end

return vec3, quat, Vec3, Quat

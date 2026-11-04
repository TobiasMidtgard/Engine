"""Round 24 step c (round 23 step 4's night23.py, on main with Weather following engine_world::MoonSource): where each
moon should be in each night picture. With MoonSource the weather's moonlight and disc aim at the space Moon (and the
weather draws no disc when space draws the body), so "space Moon" is where the one moon should be now; "clock moon" is
where round 23's weather moon stood (climate::moon), for reference only. Also prints weather.panel's moon fields and the
Moon's phase angle and lit fraction seen from the camera.

python -I night24.py <run dir>

For every picture h<hour>_<place>.png: the camera (the place's eye and target: showcase.goto's position,
the target from showcase.zones' centre plus the place's offset in places.rs), and three directions:
- the space Moon (space.bodies, the Moon body's world position from the camera; the Sun likewise),
- the sky shader's stand-in moon, opposite the Sun (sky.wgsl: shown only while the Sun is the main light),
- the weather's moon (the light's disc when the moon is the main light): climate::moon from weather.panel's
  clock (hour, day_of_year, moon_phase) at Climate::default (35 deg N, tilt 23.44), WeatherFrame yaw 0.
Each with its elevation, compass bearing (north +Z, east -X) and where it falls in the 1600x900 picture
(vertical FOV 55 deg), or how far outside.
"""
import glob, json, math, os, re, sys

d = sys.argv[1]
zones = {z["key"]: z["center"] for z in json.load(open(f"{d}/zones.json", encoding="cp1252"))["result"]}
# The target offsets of places.rs (near(key, eye, look): target = centre + look), or absolute targets.
TARGET = {"plaza": ("plaza", [0.0, 1.8, -1.0]), "lake": ("lake", [10.0, 0.0, -10.0]),
          "night": (None, [8.0, 0.5, -2.0]), "sunset": (None, [60.0, 6.0, -20.0]),
          "meadow": ("meadow", [-120.0, 0.0, 60.0]), "weather": ("weather", [-215.0, -20.0, -370.0])}
positions = {}
for line in open(f"{d}/timeline.txt", encoding="utf-8"):
    m = re.search(r'AT (\w+) at ([\d.]+) h: .*"position":\[([^\]]+)\]', line)
    if m:
        positions[(m.group(1), m.group(2))] = [float(x) for x in m.group(3).split(",")]


def sub(a, b): return [a[i] - b[i] for i in range(3)]
def dot(a, b): return sum(a[i] * b[i] for i in range(3))
def cross(a, b): return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
def norm(a):
    n = math.sqrt(dot(a, a))
    return [x / n for x in a]


def celestial(hour, decl, lat):
    h = (hour - 12.0) / 24.0 * 2 * math.pi
    up = math.sin(lat) * math.sin(decl) + math.cos(lat) * math.cos(decl) * math.cos(h)
    east = -math.cos(decl) * math.sin(h)
    north = math.cos(lat) * math.sin(decl) - math.sin(lat) * math.cos(decl) * math.cos(h)
    return norm([-east, up, north])


def weather_moon(hour, day, phase):
    sun_decl = -math.radians(23.44) * math.cos(2 * math.pi * (day + 10.0) / 365.25)
    decl = sun_decl * math.cos(2 * math.pi * phase)
    return celestial((hour - phase * 24.0) % 24.0, decl, math.radians(35.0))


def describe(v, fwd, right, up):
    el = math.degrees(math.asin(max(-1, min(1, v[1]))))
    bearing = math.degrees(math.atan2(-v[0], v[2])) % 360  # from north (+Z) toward east (-X)
    z = dot(v, fwd)
    t = math.tan(math.radians(27.5))
    if z <= 0:
        where = "behind the camera"
    else:
        x = dot(v, right) / z / (t * 16 / 9)
        y = dot(v, up) / z / t
        px, py = (x + 1) / 2 * 1600, (1 - y) / 2 * 900
        inside = 0 <= px < 1600 and 0 <= py < 900
        where = f"pixel ({px:.0f}, {py:.0f})" + ("" if inside else " - outside the picture")
    return f"elevation {el:5.1f}, bearing {bearing:5.1f}; {where}"


for path in sorted(glob.glob(f"{d}/bodies_h*.json")):
    tag = os.path.basename(path)[len("bodies_"):-len(".json")]
    m = re.match(r"h([\d_]+)_(\w+)$", tag)
    hour_s, place = m.group(1).replace("_", "."), m.group(2)
    bodies = json.load(open(path, encoding="cp1252"))["result"]
    panel = json.load(open(f"{d}/weather_{tag}.json", encoding="cp1252"))["result"]
    clock = panel["clock"]
    eye = positions.get((place, hour_s))
    if eye is None or place not in TARGET:
        print(f"== {tag}: no camera pose known")
        continue
    zk, off = TARGET[place]
    c = zones[zk] if zk else [0.0, 0.0, 0.0]
    target = [c[i] + off[i] for i in range(3)]
    fwd = norm(sub(target, eye))
    right = norm(cross(fwd, [0, 1, 0]))
    up = cross(right, fwd)
    by = {b["name"]: b for b in bodies["bodies"]}
    print(f"== {tag}: clock hour {clock['hour']:.2f}, day {clock['day_of_year']:.1f}, moon phase {clock['moon_phase']:.3f}; "
          f"sun elevation (weather) {panel['now']['sun_elevation_deg']:.1f}; camera at ({eye[0]:.0f}, {eye[1]:.0f}, {eye[2]:.0f}) "
          f"looking {describe(fwd, fwd, right, up).split(';')[0]}")
    for name in ("Moon", "Sun"):
        if name in by:
            v = norm(sub(by[name]["world_position"], eye))
            print(f"   space {name:5s}: {describe(v, fwd, right, up)}")
            if name == "Sun":
                print(f"   stand-in moon (opposite the space Sun): {describe([-x for x in v], fwd, right, up)}")
    wm = weather_moon(clock["hour"], clock["day_of_year"], clock["moon_phase"])
    print(f"   clock moon (round 23's weather moon, not expected now): {describe(wm, fwd, right, up)}")
    if "Moon" in by and "Sun" in by:
        vm = sub(by["Moon"]["world_position"], eye)
        # Phase angle at the Moon between the Sun and the camera.
        a, b = norm(sub(by["Sun"]["world_position"], by["Moon"]["world_position"])), norm([-x for x in vm])
        i = math.degrees(math.acos(max(-1, min(1, dot(a, b)))))
        el = math.degrees(math.acos(max(-1, min(1, dot(norm(vm), norm(sub(by["Sun"]["world_position"], eye)))))))
        r = by["Moon"].get("radius_m") or by["Moon"].get("radius")
        size = f", apparent diameter {math.degrees(2 * math.asin(min(1, r / math.sqrt(dot(vm, vm))))):.3f} deg" if r else ""
        print(f"   Moon from the camera: elongation from the Sun {el:.1f} deg, phase angle {i:.1f} deg, "
              f"lit fraction {(1 + math.cos(math.radians(i))) / 2:.3f}{size}")
        extra = {k: v for k, v in by["Moon"].items() if k not in ("world_position", "name")}
        print(f"   space.bodies Moon: {json.dumps(extra)[:400]}")
    def moonish(o, path=""):
        out = []
        if isinstance(o, dict):
            for k, v in o.items():
                p = f"{path}.{k}" if path else k
                if "moon" in k.lower():
                    out.append(f"{p} = {json.dumps(v)[:120]}")
                else:
                    out += moonish(v, p)
        return out
    print("   weather.panel moon fields: " + ("; ".join(moonish(panel)) or "none"))

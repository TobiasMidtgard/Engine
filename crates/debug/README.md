# engine_debug

Debug drawing for every module, game and AI agent: lines and wireframe shapes drawn over the
scene, precise anywhere in a large world, readable by day and by night.

```rust
use engine_debug::prelude::*;

app.add_plugins(DebugPlugin); // after or before DefaultPlugins, either works

fn show_ai(mut debug: ResMut<DebugDraw>, q: Query<(&GlobalTransform, &Npc)>) {
    for (t, npc) in &q {
        let eye = t.translation() + DVec3::Y * 1.7;
        debug.arrow(eye, npc.target, YELLOW);                 // this frame only
        debug.circle(t.translation(), Vec3::Y, npc.hearing_m, PURPLE);
        debug.axes_global(t, 0.5);                           // X red, Y green, Z blue
    }
    debug.sphere(DVec3::new(0.0, 1.0, 0.0), 0.5, RED)
        .duration(2.0)                                       // real seconds
        .on_top();                                           // ignore the depth buffer
}
```

## Shapes

All positions are `f64` world meters (`DVec3`), sizes meters, rotations `Quat`.

| Method | Shape |
|---|---|
| `line(a, b, c)`, `ray(origin, dir, c)` | segment; a ray's length is `dir`'s length |
| `arrow(from, to, c)`, `arrow_with_head(from, to, head, c)` | arrow with a 4-line head |
| `aabb(min, max, c)`, `oriented_box(center, half_extents, rotation, c)` | 12-edge boxes |
| `sphere(center, r, c)` | three great circles |
| `circle(center, normal, r, c)` | circle in the plane ⟂ `normal` |
| `cross(at, size, c)`, `point(at, c)` | 3-axis cross (`point` = 0.15 m cross) |
| `axes(&Transform, size)`, `axes_global(&GlobalTransform, size)`, `axes_at(pos, rot, size)` | coordinate frame |
| `capsule(a, b, r, c)`, `cylinder(a, b, r, c)` | around the segment `a`–`b` |
| `grid(center, normal, cell, cells, c)` | square grid on a plane |
| `frustum(view_proj, origin, max_distance, c)`, `camera_frustum(&Camera, &GlobalTransform, aspect, max_distance, c)` | camera-relative view-projection; infinite far planes are cut at `max_distance` |
| `polyline(points, c)`, `polygon(points, c)`, `path(points, closed, c)` | point lists (no allocation) |
| `shape(&DebugShape, c)`, `styled(&DebugShape, ShapeStyle)` | the data form used by the API |

Every method returns a `ShapeHandle`:

- `.duration(secs)`: keep the shape for `secs` **real** seconds. It keeps counting while the
  world is paused or in slow motion. `0` means this frame only (the default).
- `.forever()`: keep it until `DebugDraw::clear` or `debug.clear`.
- `.on_top()` / `.depth(DepthMode::OnTop)`: always visible. The default `DepthMode::Tested` is
  hidden behind geometry, and lines lying on a surface (ground grids, footprints) stay
  visible at any angle.

**Lifetimes.** A shape without a duration lives for one rendered frame. Shapes drawn in any
schedule up to `Extract` show in that frame's render (and in screenshots taken between
frames) and are removed at the start of the next frame (`First`). Shapes drawn after the
render (in `Last`, or between frames by `debug.draw` or test code) show in the next frame
instead. Give shapes drawn in `FixedUpdate` a duration of about one step, because the fixed
loop can run zero or several times per frame.

**Colors** are linear RGBA `[f32; 4]`: `RED`, `GREEN`, `BLUE`, `YELLOW`, `CYAN`, `MAGENTA`,
`ORANGE`, `PURPLE`, `PINK`, `LIME`, `WHITE`, `GRAY`, `BLACK`, plus `color::hex(0xff8800)`,
`color::with_alpha`, `color::mix` and `color::ramp(t)` (blue to red). They are written as
display values, not as physical light, so auto-exposure never washes them out or hides
them. Alpha below 1 blends.

## Built-in views (`DebugViews`, off by default)

| Field | Shows |
|---|---|
| `enabled` | master switch for **all** debug drawing (default `true`) |
| `wind {enabled, radius, spacing, height, scale, max_speed}` | `WindField` arrows on a grid around the focus/camera, colored by speed |
| `surface {enabled, radius, spacing}` | `SurfaceLayers` points: brown dry, blue wet, cyan puddles, white snow, black scorch |
| `focus` | `FocusPoints` markers (primary magenta) |
| `gravity` | gravity arrow at the primary focus |
| `events {projectiles, impulses, ground_contacts, noise, duration}` | `ProjectileTrace` lines, `WorldImpulse` spheres, `GroundContact` footprints, `NoiseEvent` circles |

`DebugViews` is a registered resource, so `resource.set DebugViews {...}` works too.

## API (AI agents, console, scripts)

```jsonc
// Draw (all or nothing; returns {shapes, lines, dropped_lines, total_lines})
debug.draw {"shapes": [
  {"line":   {"a": [0, 1, 0], "b": [5, 1, 0], "color": "red"}},
  {"sphere": {"center": [0, 1, 0], "radius": 0.5, "color": [0, 1, 1, 1], "depth": "on_top"}},
  {"arrow":  {"from": [0, 0, 0], "to": [0, 3, 0], "color": "#ff8800", "duration": -1}}
]}
debug.clear {}
debug.views {}                                     // read the toggles
debug.views {"wind": {"enabled": true}, "events": {"impulses": true}}
debug.views {"all": true}                          // every view on
```

Shape keys: `line, ray, arrow, aabb, box (obb), sphere, circle, cross, point, axes, capsule,
cylinder, grid, frustum, path (polyline)`, with the same fields as the Rust methods. Common
fields:

- `color`: `[r,g,b]` or `[r,g,b,a]` linear, a name, or `"#rrggbb[aa]"`.
- `duration`: real seconds. The default is **10** so an agent has time to take a
  `render.screenshot`. `0` means one frame and `-1` keeps the shape until `debug.clear`.
- `depth`: `"tested"` or `"on_top"`.

Errors name the shape index and include an example.

## How it renders

`DebugLinesFeature` (a `RenderFeature`, `Transparent` phase) converts the `f64` segments to
camera-relative `f32` each frame (`ViewData::origin`). Lines 1e8 m from the origin are as
precise as lines next to it. It writes them into one reused `Vec` and one `GrowBuffer`, then
draws two line-list pipelines:

- **Depth-tested.** Reverse-Z `GreaterEqual` against a depth nudged 5% toward the camera (a
  cheap early reject), no depth write. The fragment shader then compares the line's true
  depth with the scene depth copy (farthest of 3×3 pixels), so on-surface lines don't
  z-fight, with perspective and orthographic cameras alike.
- **On top.** Depth compare `Always`.

Metrics: `debug.lines` and `debug.dropped_lines` in `Diagnostics`.
`DebugDraw::max_lines` (262 144) caps memory if something draws in a loop.

## Limitations

- Lines are 1 pixel wide (wgpu line lists); there is no text or label drawing yet.
- Depth-tested lines show a 1-pixel halo through the silhouette edges of closer objects.
- `DebugDraw` is one `ResMut`, so systems that draw run one after another. Per-system
  buffers would let them run in parallel if this ever shows up in profiles.

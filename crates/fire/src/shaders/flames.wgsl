// Flames and embers. Each flame is an upright sprite turned to the camera that plays the flame
// flipbook (a little fire simulation recorded once, see flipbook.rs) at its own pace, colored
// by blackbody temperature, with soot at its tips. Before the flames, a heat-haze pass shimmers
// the scene behind and above them. Premultiplied output; soft where sprites meet the ground
// (scene depth copy). Requires lib_types + lib_frame.

struct FlameInstance {
    pos_height: vec4<f32>, // camera-relative base (flames) or center (embers), height (m)
    shape: vec4<f32>,      // half-width (m), intensity 0..1, seed, kind (0 flame, 1 ember)
    extra: vec4<f32>,      // flames: lean x, lean z (m per m of height), heat, 0; embers: velocity xyz, heat
};

@group(1) @binding(0) var<storage, read> instances: array<FlameInstance>;
@group(1) @binding(1) var scene_depth: texture_depth_2d;
@group(1) @binding(2) var flipbook: texture_3d<f32>;
@group(1) @binding(3) var scene_color: texture_2d<f32>;

// Flipbook frames, and how many play per second for a flame 1 m tall (taller flames flicker
// slower, as the square root of their height). Keep in step with flipbook.rs.
const BOOK_FRAMES: f32 = 96.0;
const BOOK_FPS: f32 = 60.0;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) params: vec4<f32>, // intensity, seed, kind, heat
    @location(2) size: vec2<f32>,                      // half-width, height (m)
    @location(3) rel: vec3<f32>,
    @location(4) @interpolate(flat) pixels_per_m: f32, // screen pixels per meter at the flame
};

// Linear sRGB color of a blackbody at `kelvin`, normalized to luminance 1.
fn blackbody(kelvin: f32) -> vec3<f32> {
    let t = clamp(kelvin, 800.0, 6500.0) / 100.0;
    let r = 1.0;
    let g = clamp(0.39008157876 * log(t) - 0.63184144378, 0.0, 1.0);
    var b = 0.0;
    if (t > 19.0) { b = clamp(0.54320678911 * log(t - 10.0) - 1.19625408914, 0.0, 1.0); }
    // Those fits are display-encoded; convert to linear.
    let c = pow(vec3<f32>(r, g, b), vec3<f32>(2.2));
    return c / max(luminance(c), 1e-4);
}

fn corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
        vec2(-1.0, 0.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    return c[i];
}

// A flame is a strip of FLAME_SEGMENTS quads up its height (6 vertices each), so it can bend.
const FLAME_SEGMENTS: u32 = 4u;

// Corner of vertex `i` of a flame strip: x in [-1, 1], y in [0, 1] up the flame.
fn strip_corner(i: u32) -> vec2<f32> {
    let c = corner(i % 6u);
    return vec2<f32>(c.x, (f32(i / 6u) + c.y) / f32(FLAME_SEGMENTS));
}

// How far downwind a flame's axis has drifted at height `y` (of its height), per unit of lean:
// the root stands up out of the fuel and the flame bends over above it (straight past y = 1).
fn bend(y: f32) -> f32 {
    return select(2.0 * y - 1.0, y * y, y < 1.0);
}

// Pulse of a flame's height: each one stretches and shrinks a little on its own rhythm.
fn pulse(seed: f32, t: f32) -> f32 {
    return 0.9 + 0.07 * sin(t * (3.0 + fract(seed) * 2.0) + seed)
        + 0.05 * sin(t * (7.0 + fract(seed * 7.3) * 4.0) + seed * 3.1);
}

// A flame sprite's corner `c` (x -1..1, y 0..1, `grow` stretches it upward).
fn flame_vertex(inst: FlameInstance, c: vec2<f32>, grow: f32, widen: f32) -> VsOut {
    let base = inst.pos_height.xyz;
    let h = inst.pos_height.w * pulse(inst.shape.z, frame.time.x);
    let w = inst.shape.x * widen;
    let cam_right = vec3<f32>(frame.view[0].x, frame.view[1].x, frame.view[2].x);
    // An axial billboard along the flame, turned to the camera. In wind the flame bends: its
    // root stands up and it leans over above (see `bend`), instead of shearing as a whole.
    let lean = vec3<f32>(inst.extra.x, 0.0, inst.extra.y);
    let a = normalize(vec3<f32>(0.0, 1.0, 0.0) + lean);
    let to_cam = normalize(-base - a * h * 0.5);
    var r = cross(a, to_cam);
    let rl = length(r);
    r = select(cam_right, r / max(rl, 1e-4), rl > 0.25);
    // Flames seen from above get a little depth instead of collapsing to a line.
    let lift = (1.0 - min(rl / 0.6, 1.0)) * h * 0.4;
    let y = c.y * grow;
    let up = vec3<f32>(0.0, y * h, 0.0) + lean * (bend(y) * h);
    let p = base + r * (c.x * w) + up + cross(r, a) * (y * lift);
    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = vec2<f32>(c.x, y);
    out.params = vec4<f32>(inst.shape.y, inst.shape.z, inst.shape.w, inst.extra.w);
    out.size = vec2<f32>(w, h);
    out.rel = p;
    let d = max(view_depth(base + a * h * 0.5), 0.05);
    out.pixels_per_m = frame.proj[1].y * 0.5 * frame.viewport.y / d;
    return out;
}

@vertex
fn vs_flame(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[ii];
    if (inst.shape.w < 0.5) {
        return flame_vertex(inst, strip_corner(vi), 1.0, 1.0);
    }
    // Embers are one quad: the rest of the strip is degenerate.
    let c = corner(vi % 6u);
    if (vi >= 6u) {
        var none: VsOut;
        none.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        return none;
    }
    // Ember: a short streak along its screen-space velocity (motion blur).
    let base = inst.pos_height.xyz;
    // Its glowing core, meters across, drawn at least ~1.5 pixels wide (thinner, it would
    // flicker in and out between pixels) and dimmed to match.
    let core = inst.shape.x;
    let pixels_per_m = frame.proj[1].y * 0.5 * frame.viewport.y / max(view_depth(base), 0.05);
    let w = max(core, 1.5 / pixels_per_m);
    let cam_up = vec3<f32>(frame.view[0].y, frame.view[1].y, frame.view[2].y);
    let vel = inst.extra.xyz;
    let speed = length(vel);
    var dir = cam_up;
    if (speed > 0.05) {
        let v = vel / speed;
        let vs = v - dot(v, normalize(-base)) * normalize(-base);
        if (length(vs) > 1e-3) { dir = normalize(vs); }
    }
    let side = normalize(cross(dir, normalize(-base)));
    // How far it flies while the shutter is open.
    let flown = speed * 0.025;
    let len = w + flown;
    let p = base + side * (c.x * w) + dir * ((c.y - 0.5) * 2.0 * len);
    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = c;
    // Its light spread over the streak: each point of it is lit only while the ember passes
    // (core / (core + flown) of the exposure), and a widened ember fills only part of it.
    let spread = core / w * core / (core + flown);
    out.params = vec4<f32>(spread, inst.shape.z, inst.shape.w, inst.extra.w);
    out.size = vec2<f32>(w, len);
    out.rel = p;
    out.pixels_per_m = 0.0;
    return out;
}

// Fade where the sprite meets opaque geometry (no hard cut lines on the ground).
fn soft_fade(frag: vec4<f32>, rel: vec3<f32>, distance: f32) -> f32 {
    let depth = textureLoad(scene_depth, vec2<i32>(frag.xy), 0);
    if (depth <= 0.0) { return 1.0; }
    // Reverse-Z infinite projection: view depth = near / depth.
    let scene = frame.exposure.z / depth;
    let mine = view_depth(rel);
    return saturate((scene - mine) / distance);
}

// The flipbook under a flame sprite: (flame, soot), animated. `uv` is the sprite's (x -1..1,
// y 0..1 at the flame's top).
fn flame_sample(uv: vec2<f32>, size: vec2<f32>, seed: f32, t: f32) -> vec2<f32> {
    // Every flame plays from its own point in the loop, a little faster or slower, some
    // mirrored, with a slow wobble so neighbors never line up.
    let rate = BOOK_FPS * inverseSqrt(max(size.y, 0.15)) * (0.85 + 0.3 * fract(seed * 1.73));
    let frame_w = fract(t * rate / BOOK_FRAMES + fract(seed * 0.618));
    let flip = select(1.0, -1.0, fract(seed * 0.37) > 0.5);
    let wob = value_noise3(vec3<f32>(uv.x * 1.3 + seed, uv.y * 1.7 - t * 0.9, t * 0.35 + seed)) - 0.5;
    let x = (uv.x * flip + wob * 0.18 * uv.y) * 0.5 + 0.5;
    // The flipbook's flames reach about 0.7 of its height (wisps above): that's the sprite's top.
    let y = uv.y * 0.7;
    let dims = vec2<f32>(textureDimensions(flipbook).xy);
    let half_texel = 0.5 / dims;
    let p = vec3<f32>(clamp(vec2(x, y), half_texel, 1.0 - half_texel), frame_w);
    return textureSampleLevel(flipbook, linear_repeat, p, 0.0).xy;
}

// Brightness of a flame's hottest gas, in exposed units: physically about 10 000 cd/m², but
// kept between a little over the scene's white (so flames show by day) and a few stops above
// it (so at night they're yellow-white at the core and orange through the body, not a white
// smear).
fn flame_peak(intensity: f32) -> f32 {
    let stops = log2(max(9000.0 * frame.exposure.x, 1e-6));
    let lo = -0.6;
    let hi = 1.0;
    // Smooth max with lo, then smooth min with hi.
    let raised = lo + log2(1.0 + exp2(stops - lo));
    let capped = hi - log2(1.0 + exp2(hi - raised));
    return exp2(capped) * (0.35 + 0.65 * saturate(intensity));
}

@fragment
fn fs_flame(in: VsOut) -> @location(0) vec4<f32> {
    let intensity = in.params.x;
    let seed = in.params.y;
    let kind = in.params.z;
    let t = frame.time.x;

    if (kind > 0.5) {
        // Ember streak.
        let x = in.uv.x;
        let y = in.uv.y * 2.0 - 1.0;
        let d = x * x + y * y * 0.6;
        let core = exp(-d * 6.0);
        if (core < 0.01) { discard; }
        let heat = in.params.w;
        let flick = 0.75 + 0.25 * sin(t * 23.0 + seed * 40.0);
        // Glowing coal (cd/m²) spread over its streak: bright against the night, faint
        // against a daylit sky.
        let v = 40000.0 * heat * flick * core * intensity * frame.exposure.x;
        // One right by the lens would be a blur out of focus: it fades out within a meter or so
        // instead of glaring across the frame.
        let near = smoothstep(0.4, 1.6, view_depth(in.rel));
        let col = blackbody(1100.0 + 700.0 * heat) * 0.8 * (1.0 - exp(-v / 0.8)) * near;
        return vec4<f32>(col, 0.0);
    }

    let s = flame_sample(in.uv, in.size, seed, t);
    // The bottom of the sprite fades in (the base of a flame is thin, bluish and dim), wisps
    // fade out at its top, and the whole flame dims as it dies down.
    let base = smoothstep(0.0, 0.1, in.uv.y) * (1.0 - smoothstep(0.82, 1.0, in.uv.y));
    let fade = soft_fade(in.clip, in.rel, 0.3) * smoothstep(0.0, 0.03, intensity);
    let heat = s.x * base;
    let soot = s.y * (1.0 - smoothstep(0.05, 0.3, heat));
    if (heat < 0.04 && soot < 0.02) { discard; }

    // Crisp tongues: the simulated field is soft, the edge of a flame isn't.
    // (The flipbook stores temperature compressed, so tongues and core both have precision.)
    let body = smoothstep(0.2, 0.42, heat);
    // Deep red where the gas is cooling, through orange to yellow in the hottest gas.
    let kelvin = mix(1150.0, 2150.0, pow(heat, 1.4));
    let glow = flame_peak(intensity) * pow(heat, 2.6) * body;
    let c = blackbody(kelvin) * glow;

    // Soot over the tips: dark smoke lit only by the sky and the fire under it.
    let soot_a = saturate(soot * 1.4) * 0.45 * saturate(intensity * 2.0);
    let soot_c = pre_expose(sky_ambient(vec3<f32>(0.0, 1.0, 0.0))) * 0.04
        + blackbody(1300.0) * flame_peak(intensity) * 0.015;
    // A flame is mostly emission, but bright enough to hide part of what's behind it, other
    // flames included (so dozens of overlapping sprites in a fire front don't add up to a
    // washed-out cream). Only the hot core: thin edges stay see-through (no dark rims by day).
    let alpha = saturate(body * 0.7 * smoothstep(0.35, 0.8, heat) + soot_a) * fade;
    let col = apply_fog(c + soot_c * soot_a, in.rel) * fade;
    return vec4<f32>(col, alpha);
}

// ---------------------------------------------------------------- heat haze

@vertex
fn vs_haze(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[ii];
    var out = flame_vertex(inst, strip_corner(vi), 2.2, 1.5);
    // Embers make no haze: a degenerate quad.
    if (inst.shape.w > 0.5) { out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0); }
    return out;
}

// The scene behind and above a flame, bent by the rising hot air.
@fragment
fn fs_haze(in: VsOut) -> @location(0) vec4<f32> {
    let intensity = in.params.x;
    let seed = in.params.y;
    let t = frame.time.x;
    // In the hot column: strongest just above the flame, fading out sideways and higher up.
    let y = in.uv.y;
    let across = 1.0 - smoothstep(0.35, 1.0, abs(in.uv.x));
    let along = smoothstep(0.15, 0.7, y) * (1.0 - smoothstep(1.3, 2.2, y));
    let fade = soft_fade(in.clip, in.rel, 0.5);
    let strength = across * along * fade * saturate(intensity * 1.5);
    if (strength < 0.01 || in.pixels_per_m <= 0.0) { discard; }
    // Small eddies rising fast, in meters (so they're the same size near and far).
    let q = vec3<f32>(in.uv.x * in.size.x * 9.0, y * in.size.y * 6.0 - t * 5.0, t * 1.3 + seed);
    let n = vec2<f32>(value_noise3(q), value_noise3(q + vec3(31.7, 11.3, 5.1))) - 0.5;
    // Bend by up to ~1.5 cm, but never more than a few pixels.
    let shift = n * 0.03 * in.size.y * strength * in.pixels_per_m;
    let px = in.clip.xy + clamp(shift, vec2(-6.0), vec2(6.0));
    // Only pull in what's behind the flame, never something in front of it.
    let at = clamp(vec2<i32>(px), vec2<i32>(0), vec2<i32>(frame.viewport.xy) - 1);
    let d = textureLoad(scene_depth, at, 0);
    let behind = d <= 0.0 || frame.exposure.z / d > view_depth(in.rel);
    if (!behind) { discard; }
    let bent = textureSampleLevel(scene_color, linear_clamp, px * frame.viewport.zw, 0.0).rgb;
    let m = saturate(strength);
    return vec4<f32>(bent * m, m);
}

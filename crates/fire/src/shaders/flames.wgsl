// Flames and embers: camera-facing sprites with a procedural, turbulent flame, colored by
// blackbody temperature. Premultiplied output (emission added, alpha darkens a little).
// Soft where they meet the ground (scene depth copy). Requires lib_types + lib_frame.

struct FlameInstance {
    pos_height: vec4<f32>, // camera-relative base (flames) or center (embers), height (m)
    shape: vec4<f32>,      // width (m), intensity 0..1, seed, kind (0 flame, 1 ember)
    extra: vec4<f32>,      // flames: lean x, lean z (m per m of height), heat, 0; embers: velocity xyz, heat
};

@group(1) @binding(0) var<storage, read> instances: array<FlameInstance>;
@group(1) @binding(1) var scene_depth: texture_depth_2d;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) params: vec4<f32>, // intensity, seed, kind, heat
    @location(2) size: vec2<f32>,                      // width, height (m)
    @location(3) rel: vec3<f32>,
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

@vertex
fn vs_flame(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[ii];
    let c = corner(vi);
    let base = inst.pos_height.xyz;
    var h = inst.pos_height.w;
    let w = inst.shape.x;
    let kind = inst.shape.w;
    var p: vec3<f32>;
    var out: VsOut;
    let cam_right = vec3<f32>(frame.view[0].x, frame.view[1].x, frame.view[2].x);
    let cam_up = vec3<f32>(frame.view[0].y, frame.view[1].y, frame.view[2].y);
    if (kind < 0.5) {
        // Flames pulse: each one stretches and shrinks a little on its own rhythm.
        let seed = inst.shape.z;
        let t = frame.time.x;
        h *= 0.82 + 0.12 * sin(t * (4.0 + fract(seed) * 3.0) + seed)
            + 0.08 * sin(t * (9.0 + fract(seed * 7.3) * 5.0) + seed * 3.1);
        // Flame: an axial billboard along the (wind-leaned) flame axis, turned to the camera.
        let axis = vec3<f32>(inst.extra.x, 1.0, inst.extra.y);
        let a = normalize(axis);
        let to_cam = normalize(-base - a * h * 0.5);
        var r = cross(a, to_cam);
        let rl = length(r);
        r = select(cam_right, r / max(rl, 1e-4), rl > 0.25);
        // Flames seen from above get a little depth instead of collapsing to a line.
        let lift = (1.0 - min(rl / 0.6, 1.0)) * h * 0.4;
        p = base + r * (c.x * w) + axis * (c.y * h) + cross(r, a) * (c.y * lift);
        out.size = vec2<f32>(w, h);
    } else {
        // Ember: a short streak along its screen-space velocity (motion blur).
        let vel = inst.extra.xyz;
        let speed = length(vel);
        var dir = cam_up;
        if (speed > 0.05) {
            let v = vel / speed;
            let vs = v - dot(v, normalize(-base)) * normalize(-base);
            if (length(vs) > 1e-3) { dir = normalize(vs); }
        }
        let side = normalize(cross(dir, normalize(-base)));
        let len = w + speed * 0.025;
        p = base + side * (c.x * w) + dir * ((c.y - 0.5) * 2.0 * len);
        out.size = vec2<f32>(w, len);
    }
    out.clip = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = c;
    out.params = vec4<f32>(inst.shape.y, inst.shape.z, kind, inst.extra.w);
    out.rel = p;
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
        let col = blackbody(900.0 + 600.0 * heat) * 40000.0 * heat * flick * core;
        return vec4<f32>(pre_expose(col), 0.0);
    }

    // Flame: turbulence in meters, rising, so big and small flames share one look.
    let w = in.size.x;
    let h = in.size.y;
    let y = in.uv.y;
    var x = in.uv.x;
    let s = seed * 17.31;
    var q = vec3<f32>(x * w, y * h, s);
    let rise = t * 2.6;
    let warp = fbm3(vec3<f32>(q.x * 0.9 + s, q.y * 0.7 - rise * 0.6, t * 0.5 + s), 3) - 0.5;
    x += warp * 0.9 * (0.15 + y);
    let n = fbm3(vec3<f32>(q.x * 2.1 + warp, q.y * 1.4 - rise, t * 0.8 + s * 1.7), 4);
    let fine = value_noise3(vec3<f32>(q.x * 6.0, q.y * 4.0 - rise * 2.2, s + t));
    // Body: wide at the bottom, narrowing into licking tongues.
    let half_w = mix(0.85, 0.12, pow(y, 0.75));
    let body = 1.0 - abs(x) / half_w;
    var heat = body * 1.2 + (n - 0.5) * 1.9 + (fine - 0.5) * 0.35 - y * 0.75;
    heat *= smoothstep(0.0, 0.06, y);
    heat = saturate(heat);
    if (heat <= 0.01) { discard; }
    // Hot yellow core low down, red-orange at the tips.
    let kelvin = mix(1000.0, 1650.0, pow(heat, 0.7)) - 250.0 * y;
    let lum = 3500.0 * intensity * pow(heat, 1.7) * (1.0 - 0.4 * y);
    var c = blackbody(kelvin) * lum;
    // A hint of blue at the base of hot flames.
    c += vec3<f32>(0.15, 0.3, 1.0) * 250.0 * intensity * heat * smoothstep(0.12, 0.0, y);
    let fade = soft_fade(in.clip, in.rel, 0.35) * smoothstep(0.0, 0.02, intensity);
    // Flames are mostly emission; a little opacity keeps them from vanishing in daylight.
    let alpha = saturate(heat * 0.18) * fade;
    let col = apply_fog(pre_expose(c), in.rel) * fade;
    return vec4<f32>(col, alpha);
}

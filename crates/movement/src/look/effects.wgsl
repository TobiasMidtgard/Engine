// Movement effects: blade trails, slash flashes and shock rings (ribbons); sparks, flares,
// motes and dust (particles); afterimage ghosts. Output is premultiplied: glowing things add
// light (alpha 0), dust and ghost bodies also cover what is behind them by their alpha.
// Requires lib_types + lib_frame.

struct FxParticle {
    pos_size: vec4<f32>, // camera-relative center (m), size (m)
    color: vec4<f32>,    // radiance (cd/m²) or dust albedo; w: dust opacity
    axis: vec4<f32>,     // sparks: unit velocity direction, streak length (m)
    params: vec4<f32>,   // kind (0 spark, 1 flare, 2 mote, 3 dust), seed, age 0..1, spare
};

@group(1) @binding(0) var fx_noise: texture_2d<f32>;   // R cloud, G billows, B fine, A ridges
@group(1) @binding(1) var scene_depth: texture_depth_2d;
@group(1) @binding(2) var<storage, read> particles: array<FxParticle>;

fn noise_at(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(fx_noise, linear_repeat, uv, 0.0);
}

// Fade where an effect meets opaque geometry, over `distance` meters (soft particles).
fn soft_fade(frag: vec4<f32>, rel: vec3<f32>, distance: f32) -> f32 {
    let depth = textureLoad(scene_depth, vec2<i32>(frag.xy), 0);
    if (depth <= 0.0) { return 1.0; }
    // Reverse-Z infinite projection: view depth = near / depth.
    let scene = frame.exposure.z / depth;
    return saturate((scene - view_depth(rel)) / distance);
}

// ---------------------------------------------------------------- ribbons

struct RibbonIn {
    @location(0) pos_mode: vec4<f32>,
    @location(1) uv_fade_seed: vec4<f32>,
    @location(2) body: vec4<f32>,
    @location(3) core: vec4<f32>,
};

struct RibbonOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fade_seed: vec2<f32>,
    @location(2) @interpolate(flat) mode: f32,
    @location(3) body: vec4<f32>,
    @location(4) core: vec4<f32>,
};

@vertex
fn vs_ribbon(in: RibbonIn) -> RibbonOut {
    var out: RibbonOut;
    out.clip = frame.view_proj * vec4<f32>(in.pos_mode.xyz, 1.0);
    out.uv = in.uv_fade_seed.xy;
    out.fade_seed = in.uv_fade_seed.zw;
    out.mode = in.pos_mode.w;
    out.body = in.body;
    out.core = in.core;
    return out;
}

@fragment
fn fs_ribbon(in: RibbonOut) -> @location(0) vec4<f32> {
    let fade = in.fade_seed.x;
    let seed = in.fade_seed.y;
    var light = vec3<f32>(0.0);
    var cover = 0.0;
    if (in.mode < 0.5) {
        // Blade trail. u: age along the trail (0 at the blade), v: across (0 hilt, 1 tip).
        // A crescent: the inner edge closes in on the tip's path with age, so the trail is
        // widest right behind the blade and tapers to a point along the arc the tip cut.
        // The taper follows age or, for a cut still young as a whole (fast, or made by someone
        // moving through slow motion), how far back along the cut the point is.
        let u = saturate(in.uv.x);
        let v = saturate(in.uv.y);
        let back = saturate(in.core.w);
        let w = max(u, back * 0.92);
        let inner = mix(0.3, 1.0, pow(w, 0.8));
        let x = saturate((v - inner) / max(1.0 - inner, 1e-3));
        let blade_side = smoothstep(0.0, 0.7, x);
        let tail = pow(1.0 - u, 1.2) * (1.0 - 0.55 * back * back);
        // Silk: fine fibers running along the cut, anchored to the moment each part was cut,
        // and a slow shimmer.
        let t = in.body.w;
        let fiber = noise_at(vec2<f32>(t * 0.35 + seed, v * 5.0 + seed * 3.0)).b;
        let shimmer = noise_at(vec2<f32>(t * 3.0 + seed * 5.0, v * 0.8)).r;
        let silk = mix(0.55, 1.2, fiber) * mix(0.75, 1.15, shimmer);
        // Brightest toward the cutting edge; a thin white-hot line right along it.
        let body = blade_side * (0.15 + 0.85 * pow(x, 1.6)) * tail * silk;
        let line = exp(-pow((1.0 - v) * 45.0, 2.0)) * pow(saturate(1.0 - w * 1.15), 1.5);
        light = (in.body.rgb * body + in.core.rgb * line) * fade;
        cover = 0.0;
    } else if (in.mode < 1.5) {
        // Slash flash. uv.x along the cut (0..reveal), uv.y across (-1..1).
        let a = abs(in.uv.y);
        let s = in.uv.x;
        let reveal = in.body.w;
        let core = exp(-a * a * 30.0);
        let glow = exp(-a * a * 3.0) * (1.0 - 0.5 * a);
        // While it draws across, the leading point burns hottest.
        let drawing = 1.0 - smoothstep(0.8, 1.0, reveal);
        let head = exp(-abs(reveal - s) * 16.0) * drawing;
        let n = noise_at(vec2<f32>(s * 2.5 + seed, a * 0.35 + seed)).r;
        light = (in.core.rgb * core * (1.0 + 2.5 * head) + in.body.rgb * glow * mix(0.6, 1.3, n)) * fade;
        cover = 0.3 * glow * fade;
    } else {
        // Shock ring. uv.x around, uv.y across (0 inside, 1 outside): sharp hot outer edge.
        let v = saturate(in.uv.y);
        let band = smoothstep(0.0, 0.85, v) * (1.0 - smoothstep(0.88, 1.0, v));
        let n = noise_at(vec2<f32>(in.uv.x * 5.0 + seed, v * 0.25)).g;
        light = (in.body.rgb * band + in.core.rgb * pow(band, 8.0)) * mix(0.5, 1.3, n) * fade;
    }
    return vec4<f32>(pre_expose(light), cover);
}

// ---------------------------------------------------------------- particles

struct ParticleOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) params: vec4<f32>,
    @location(3) rel: vec3<f32>,
    @location(4) light: vec3<f32>,
    @location(5) nuv: vec2<f32>,
    @location(6) basis: vec2<f32>,
};

fn corner(i: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    return c[i];
}

fn rotate2(c: vec2<f32>, a: f32) -> vec2<f32> {
    let s = sin(a);
    let k = cos(a);
    return vec2<f32>(c.x * k - c.y * s, c.x * s + c.y * k);
}

// Henyey-Greenstein phase function (per steradian).
fn phase_hg(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

@vertex
fn vs_particle(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> ParticleOut {
    let p = particles[ii];
    let c = corner(vi);
    let center = p.pos_size.xyz;
    let size = p.pos_size.w;
    let kind = p.params.x;
    let to_cam = normalize(-center);
    let cam_right = vec3<f32>(frame.view[0].x, frame.view[1].x, frame.view[2].x);
    let cam_up = vec3<f32>(frame.view[0].y, frame.view[1].y, frame.view[2].y);
    var out: ParticleOut;
    out.params = p.params;
    out.color = p.color;
    out.light = vec3<f32>(0.0);
    out.nuv = vec2<f32>(0.0);
    out.basis = c;
    var pos: vec3<f32>;
    if (kind < 0.5) {
        // Spark: a capsule from where it was a shutter-time ago to where it is now.
        let along = p.axis.xyz * p.axis.w;
        let mid = center - along * 0.5;
        let proj = along - dot(along, to_cam) * to_cam;
        let plen = length(proj);
        var dir = cam_up;
        if (plen > 1e-5) { dir = proj / plen; }
        let side = normalize(cross(dir, to_cam));
        pos = mid + side * (c.x * size) + dir * (c.y * (plen * 0.5 + size));
        // Length of the straight part in radii (for the capsule shape).
        out.params.w = plen * 0.5 / max(size, 1e-5);
    } else if (kind < 2.5) {
        // Flare or mote: camera-facing, turned by the seed.
        let rc = rotate2(c, p.params.y * TAU);
        pos = center + (cam_right * rc.x + cam_up * rc.y) * size;
    } else {
        // Dust puff: camera-facing and slowly turning. The sun reaching it (shadowed per
        // puff) is passed on; the fragment lights it like a soft, lumpy ball.
        let age = p.params.z;
        let rc = rotate2(c, p.params.y * TAU + age * (p.params.y - 0.5) * 1.6);
        pos = center + (cam_right * rc.x + cam_up * rc.y) * size;
        let shadow = shadow_factor(center, vec3<f32>(0.0, 1.0, 0.0));
        out.light = frame.sun_color.rgb * sun_transmittance(center) * shadow;
        out.basis = rc;
        out.nuv = rc * (0.42 + 0.2 * p.params.y) + vec2<f32>(p.params.y * 7.31, p.params.y * 3.17)
            + vec2<f32>(age * 0.07, -age * 0.05);
    }
    out.clip = frame.view_proj * vec4<f32>(pos, 1.0);
    out.uv = c;
    out.rel = pos;
    return out;
}

@fragment
fn fs_particle(in: ParticleOut) -> @location(0) vec4<f32> {
    let kind = in.params.x;
    if (kind < 0.5) {
        // Spark streak: a capsule, white-hot in the middle, fading toward the tail.
        let k = in.params.w;
        let y = in.uv.y * (k + 1.0);
        let d = vec2<f32>(in.uv.x, max(abs(y) - k, 0.0));
        let r2 = dot(d, d);
        let body = exp(-r2 * 3.5) * 0.5 + exp(-r2 * 14.0);
        let along = saturate((y + k + 1.0) / (2.0 * k + 2.0));
        let taper = mix(0.25, 1.0, along * along);
        // Motion blur spreads the same light along the streak.
        let light = in.color.rgb * body * taper / (1.0 + 0.12 * k);
        return vec4<f32>(pre_expose(light), 0.0);
    }
    let r = length(in.uv);
    if (kind < 1.5) {
        // Flare: a soft core and a thin four-point star.
        let q = in.uv;
        let glow = exp(-r * r * 7.0) + 0.3 * exp(-r * 3.5);
        let star = exp(-abs(q.y) * 40.0) * pow(saturate(1.0 - abs(q.x)), 2.0)
            + exp(-abs(q.x) * 40.0) * pow(saturate(1.0 - abs(q.y)), 2.0);
        let light = in.color.rgb * (glow + star * 0.8) * (1.0 - smoothstep(0.85, 1.0, r));
        return vec4<f32>(pre_expose(light), 0.0);
    }
    if (kind < 2.5) {
        let light = in.color.rgb * (exp(-r * r * 6.0) + 0.6 * exp(-r * r * 30.0));
        return vec4<f32>(pre_expose(light), 0.0);
    }
    // Dust: billowy noise inside a soft ball that thins out and breaks up as it ages.
    let r2 = dot(in.uv, in.uv);
    if (r2 >= 1.0) { discard; }
    let age = in.params.z;
    let n1 = noise_at(in.nuv);
    let n2 = noise_at(in.nuv * 2.3 + vec2<f32>(0.37, 0.71));
    let billow = n1.g * 0.6 + n2.r * 0.4;
    let ball = 1.0 - r2;
    let dens = saturate((ball * 1.25 - (1.0 - billow) * (0.55 + 0.4 * age)) * 2.6);
    let near = saturate((view_depth(in.rel) - 0.25) / 0.75);
    let alpha = dens * in.color.a * soft_fade(in.clip, in.rel, 0.35) * near;
    // Light it like a lumpy ball: the billows bend the normal, the sun wraps around it and
    // scatters forward through the thin edges, the sky fills in.
    let to_cam = normalize(-in.rel);
    let cam_right = vec3<f32>(frame.view[0].x, frame.view[1].x, frame.view[2].x);
    let cam_up = vec3<f32>(frame.view[0].y, frame.view[1].y, frame.view[2].y);
    let bump = (vec2<f32>(n1.g, n2.r) - vec2<f32>(0.5)) * 0.9;
    let n = normalize(cam_right * (in.basis.x + bump.x) + cam_up * (in.basis.y + bump.y)
        + to_cam * sqrt(max(ball, 0.0)) * 1.2);
    let l = frame.sun_dir.xyz;
    let wrap = saturate(dot(n, l) * 0.65 + 0.35);
    let forward = phase_hg(dot(-to_cam, l), 0.55) * (1.0 - 0.6 * dens);
    // Dense cores shade themselves; wisps at the edge let light through.
    let shade = mix(1.0, 0.7, dens * ball);
    let light = in.color.rgb * (in.light * (wrap * 0.8 * INV_PI + forward * 0.7) * shade
        + sky_ambient(n) * mix(1.0, 0.8, dens * ball));
    return vec4<f32>(pre_expose(light) * alpha, alpha);
}

// ---------------------------------------------------------------- afterimage ghosts

struct GhostIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) rim: vec4<f32>,    // rim radiance, body opacity
    @location(8) body: vec4<f32>,   // body emission, age 0..1
    @location(9) albedo: vec4<f32>, // lit tint, seed
};

struct GhostOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) rim: vec4<f32>,
    @location(3) @interpolate(flat) body: vec4<f32>,
    @location(4) @interpolate(flat) albedo: vec4<f32>,
};

@vertex
fn vs_ghost(in: GhostIn) -> GhostOut {
    let p = vec4<f32>(in.position, 1.0);
    let rel = vec3<f32>(dot(in.m0, p), dot(in.m1, p), dot(in.m2, p));
    let n = vec3<f32>(dot(in.m0.xyz, in.normal), dot(in.m1.xyz, in.normal), dot(in.m2.xyz, in.normal));
    var out: GhostOut;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.normal = n;
    out.rim = in.rim;
    out.body = in.body;
    out.albedo = in.albedo;
    return out;
}

@fragment
fn fs_ghost(in: GhostOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let v = normalize(-in.rel);
    let ndv = abs(dot(n, v));
    // A silhouette of light in the trail color: a translucent, softly banded body that
    // thickens toward the outline, and a bright rim.
    let fres = pow(1.0 - ndv, 2.0);
    let edge = pow(1.0 - ndv, 3.5);
    let w = tiling_pos(in.rel);
    let bands = 0.85 + 0.15 * sin(w.y * 30.0 + in.albedo.w * 40.0 + in.body.w * 5.0);
    // A shadow of the body in a deep tint of the trail color, edged with light.
    let opacity = saturate(in.rim.w * 1.5 * (0.6 + 0.4 * fres));
    let tint = in.albedo.rgb * 0.03 * sky_ambient(n);
    let light = tint * opacity + in.body.rgb * pow(fres, 1.5) * bands + in.rim.rgb * edge * 1.3;
    return vec4<f32>(pre_expose(light), opacity);
}

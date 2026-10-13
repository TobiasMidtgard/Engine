// Marks: decals projected onto the scene inside each mark's box (`marks.rs`).
//
// Each mark is drawn as the back faces of its box (so it still works with the camera inside).
// For every covered pixel the scene position comes back from the depth copy; pixels outside
// the box, or on surfaces facing away from the mark, are skipped. Two passes:
//
// - `fs_mark_shade` (multiply blend): scorch char, wet and chipped stone change how much light
//   the surface under them returns.
// - `fs_mark_cover` (premultiplied over): frost is a lit surface of its own, and a scorch's
//   embers glow.
//
// `MARK_BOX` (36 corner indices, outward faces counter-clockwise) is prepended by Rust.

struct MarkInstance {
    center: vec4<f32>,  // camera-relative center xyz, kind code
    right: vec4<f32>,   // unit right xyz, half width (m)
    forward: vec4<f32>, // unit forward xyz, half length (m)
    normal: vec4<f32>,  // unit normal xyz, half thickness of the box (m)
    life: vec4<f32>,    // age (s), life (s), strength, seed
    extra: vec4<f32>,   // ember glow (s), 0, 0, 0
};

@group(1) @binding(0) var<storage, read> marks: array<MarkInstance>;
@group(1) @binding(1) var scene_depth: texture_depth_2d;

const MARK_SCORCH: u32 = 0u;
const MARK_FROST: u32 = 1u;
const MARK_WET: u32 = 2u;
const MARK_CHIP: u32 = 3u;
// Marks fade out between these distances from the camera (m).
const MARK_FADE_NEAR: f32 = 60.0;
const MARK_FADE_FAR: f32 = 90.0;

struct MarkVs {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) index: u32,
};

@vertex
fn vs_mark(@builtin(vertex_index) vi: u32, @builtin(instance_index) index: u32) -> MarkVs {
    let m = marks[index];
    let k = MARK_BOX[vi];
    let c = vec3<f32>(f32(k & 1u), f32((k >> 1u) & 1u), f32((k >> 2u) & 1u)) * 2.0 - 1.0;
    let rel = m.center.xyz + m.right.xyz * (c.x * m.right.w) + m.forward.xyz * (c.y * m.forward.w)
        + m.normal.xyz * (c.z * m.normal.w);
    var out: MarkVs;
    out.position = frame.view_proj * vec4<f32>(rel, 1.0);
    out.index = index;
    return out;
}

// ---------------------------------------------------------------- the scene under a pixel

fn mark_scene_rel(px: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let p = clamp(px, vec2<i32>(0), size - 1);
    let depth = textureLoad(scene_depth, p, 0);
    let uv = (vec2<f32>(p) + 0.5) * frame.viewport.zw;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let w = frame.inv_view_proj * ndc;
    return vec4<f32>(w.xyz / w.w, depth);
}

struct MarkHit {
    rel: vec3<f32>,
    // Where in the box: x across, y along, z along the normal; each -1..1.
    local: vec3<f32>,
    // Surface normal of the scene, facing the camera.
    n: vec3<f32>,
    // How much of the mark shows here (facing, box edges, distance, age), 0..1.
    show: f32,
};

fn mark_hit(m: MarkInstance, frag: vec2<f32>) -> MarkHit {
    var hit: MarkHit;
    hit.show = 0.0;
    let px = vec2<i32>(frag);
    let here = mark_scene_rel(px);
    if (here.w <= 0.0) { return hit; } // sky
    let rel = here.xyz;
    let d = rel - m.center.xyz;
    let local = vec3<f32>(
        dot(d, m.right.xyz) / m.right.w,
        dot(d, m.forward.xyz) / m.forward.w,
        dot(d, m.normal.xyz) / m.normal.w,
    );
    if (any(abs(local) > vec3<f32>(1.0))) { return hit; }
    // The scene's normal from its neighbours, picking the side without a depth edge.
    let xa = mark_scene_rel(px + vec2<i32>(1, 0)).xyz - rel;
    let xb = rel - mark_scene_rel(px - vec2<i32>(1, 0)).xyz;
    let ya = mark_scene_rel(px + vec2<i32>(0, 1)).xyz - rel;
    let yb = rel - mark_scene_rel(px - vec2<i32>(0, 1)).xyz;
    let dx = select(xb, xa, dot(xa, xa) < dot(xb, xb));
    let dy = select(yb, ya, dot(ya, ya) < dot(yb, yb));
    var n = normalize(cross(dy, dx));
    if (dot(n, -rel) < 0.0) { n = -n; }
    let facing = smoothstep(0.35, 0.75, dot(n, m.normal.xyz));
    let edges = 1.0 - smoothstep(0.6, 1.0, abs(local.z));
    let dist = length(rel);
    let near = 1.0 - smoothstep(MARK_FADE_NEAR, MARK_FADE_FAR, dist);
    let age01 = saturate(m.life.x / max(m.life.y, 1e-3));
    let alive = 1.0 - smoothstep(0.66, 1.0, age01);
    hit.rel = rel;
    hit.local = local;
    hit.n = n;
    hit.show = facing * edges * near * alive * m.life.z;
    return hit;
}

fn mark_seed(m: MarkInstance) -> vec2<f32> {
    let s = m.life.w;
    return vec2<f32>(fract(s * 0.618034) * 97.0, fract(s * 0.414214) * 61.0);
}

// ---------------------------------------------------------------- scorch, wet, chips

fn mark_ragged_r(m: MarkInstance, p: vec2<f32>, roughness: f32) -> f32 {
    let n = fbm2(p * 2.4 + mark_seed(m), 4);
    return length(p) + (n - 0.5) * roughness;
}

fn mark_scorch_dark(m: MarkInstance, p: vec2<f32>) -> f32 {
    let r = mark_ragged_r(m, p, 0.6);
    let char = smoothstep(1.0, 0.4, r);
    // Soot thrown out in streaks.
    let a = atan2(p.y, p.x);
    let streak = smoothstep(0.55, 0.85, value_noise2(vec2<f32>(a * 6.0, mark_seed(m).x)));
    let streaks = streak * smoothstep(1.2, 0.5, length(p)) * 0.7;
    return max(char * (0.85 + 0.15 * fbm2(p * 9.0, 2)), streaks);
}

// Pale ash flecks in the middle of a scorch, 0..1.
fn mark_scorch_ash(m: MarkInstance, p: vec2<f32>) -> f32 {
    let flecks = smoothstep(0.62, 0.8, fbm2(p * 11.0 + mark_seed(m).yx, 3));
    return flecks * smoothstep(0.7, 0.2, length(p));
}

fn mark_wet(m: MarkInstance, p: vec2<f32>) -> f32 {
    // Dries from the edges: the wet patch shrinks with age.
    let age01 = saturate(m.life.x / max(m.life.y, 1e-3));
    let r = mark_ragged_r(m, p, 0.7) + age01 * 0.85;
    return smoothstep(1.0, 0.8, r);
}

fn mark_chip(m: MarkInstance, p: vec2<f32>) -> f32 {
    // Positive darkens, negative lightens (powdered stone around the hit).
    let r = length(p);
    let pock = smoothstep(0.32, 0.12, r);
    let a = atan2(p.y, p.x) + mark_seed(m).x;
    var cracks = 0.0;
    for (var i = 0; i < 5; i++) {
        let at = f32(i) * 1.2566 + 0.4 * sin(f32(i) * 7.3 + mark_seed(m).y);
        let da = abs(fract((a - at) / 6.2832 + 0.5) - 0.5) * 6.2832;
        let len = 0.55 + 0.35 * fract(f32(i) * 0.37 + mark_seed(m).x);
        cracks = max(cracks, smoothstep(0.06, 0.0, da * r) * smoothstep(len, 0.2, r));
    }
    let dust = smoothstep(0.35, 0.55, r) * smoothstep(1.0, 0.6, r);
    return max(pock * 0.75, cracks * 0.55) - dust * 0.12;
}

@fragment
fn fs_mark_shade(in: MarkVs) -> @location(0) vec4<f32> {
    let m = marks[in.index];
    let hit = mark_hit(m, in.position.xy);
    if (hit.show <= 0.002) { discard; }
    let kind = u32(m.center.w);
    let p = hit.local.xy;
    var ratio = vec3<f32>(1.0);
    switch kind {
        case MARK_SCORCH: {
            let dark = mix(1.0, 0.1, mark_scorch_dark(m, p));
            ratio = vec3<f32>(mix(dark, 0.75, mark_scorch_ash(m, p)));
        }
        case MARK_WET: {
            ratio = vec3<f32>(mix(1.0, 0.55, mark_wet(m, p)));
        }
        case MARK_CHIP: {
            ratio = vec3<f32>(1.0 - mark_chip(m, p));
        }
        default: { discard; }
    }
    return vec4<f32>(mix(vec3<f32>(1.0), ratio, hit.show), 1.0);
}

// ---------------------------------------------------------------- frost and embers

fn mark_ember_color(kelvin: f32) -> vec3<f32> {
    let t = clamp(kelvin, 800.0, 3000.0) / 100.0;
    let g = clamp(0.39008157876 * log(t) - 0.63184144378, 0.0, 1.0);
    let c = pow(vec3<f32>(1.0, g, 0.0), vec3<f32>(2.2));
    return c / max(luminance(c), 1e-4);
}

@fragment
fn fs_mark_cover(in: MarkVs) -> @location(0) vec4<f32> {
    let m = marks[in.index];
    let hit = mark_hit(m, in.position.xy);
    if (hit.show <= 0.002) { discard; }
    let kind = u32(m.center.w);
    let p = hit.local.xy;
    if (kind == MARK_SCORCH) {
        let glow_s = m.extra.x;
        if (glow_s <= 0.0) { discard; }
        let cool = saturate(1.0 - m.life.x / glow_s);
        let r = mark_ragged_r(m, p, 0.6);
        let bed = smoothstep(0.55, 0.1, r);
        // Thin glowing cracks and a few coals, not a red carpet.
        let n = fbm2(p * 6.0 + mark_seed(m), 4);
        let cracks = smoothstep(0.06, 0.0, abs(n - 0.5)) * 0.8;
        let coals = smoothstep(0.7, 0.85, fbm2(p * 13.0 + mark_seed(m).yx, 3));
        let heat = bed * max(cracks, coals) * cool * cool;
        if (heat <= 0.002) { discard; }
        // Embers at about 1300 K cooling to 900 K; a few thousand cd/m² at their hottest.
        let color = mark_ember_color(mix(900.0, 1350.0, heat)) * 5000.0 * heat * hit.show;
        return vec4<f32>(pre_expose(color), 0.0);
    }
    if (kind != MARK_FROST) { discard; }
    // Frost creeps out in a ragged patch and melts back from the edges as it ages.
    let age01 = saturate(m.life.x / max(m.life.y, 1e-3));
    let r = mark_ragged_r(m, p, 0.65) + age01 * 0.7;
    // Feathery crystals: ridges of noise in meters, so they keep their size on any mark. They
    // reach past a thin film of rime in the middle.
    let meters = p * vec2<f32>(m.right.w, m.forward.w);
    let ridge = 1.0 - abs(fbm2(meters * 7.0 + mark_seed(m), 5) * 2.0 - 1.0);
    let crystals = smoothstep(0.82, 0.97, ridge);
    let film = smoothstep(0.75, 0.25, r) * 0.45;
    let feathers = crystals * smoothstep(1.05, 0.55, r);
    let cover = max(film, feathers) * hit.show;
    if (cover <= 0.002) { discard; }
    var s = default_surface();
    s.albedo = vec3<f32>(0.82, 0.9, 0.98);
    s.roughness = mix(0.6, 0.22, crystals);
    s.normal = hit.n;
    s.subsurface = 0.25;
    let color = apply_fog(shade_surface(s, hit.rel), hit.rel);
    return vec4<f32>(color * cover, cover);
}

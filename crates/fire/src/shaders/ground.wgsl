// Painted ground: a flat patch of terrain whose material is blended per pixel from a stack
// of tiling layers (litter, dirt, gravel, mud, ash) by painted shapes with noisy, height-aware
// edges, so gravel stones poke out of the dirt and puddles fill the low spots. Requires
// lib_types + lib_frame.

const MAX_PAINTS: u32 = 12u;
const LAYERS: u32 = 5u;

struct Paint {
    shape: vec4<f32>,  // center x, z (m from the ground's center); half extents x, z (z < 0: disc of radius x)
    look: vec4<f32>,   // layer, blend width (m), raggedness (m), strength 0..1
};

struct GroundParams {
    origin: vec4<f32>,  // camera-relative center xyz, number of paints
    extent: vec4<f32>,  // half extents x, z; outer fade (m); base layer
    tiles: array<vec4<f32>, 2>, // meters per tile of each layer
    paints: array<Paint, 12>,
};

@group(1) @binding(0) var<uniform> ground: GroundParams;
@group(1) @binding(1) var albedo_layers: texture_2d_array<f32>;  // sRGB albedo, A = height
@group(1) @binding(2) var detail_layers: texture_2d_array<f32>;  // normal X/Y, roughness, AO
@group(1) @binding(3) var layer_sampler: sampler;
@group(1) @binding(4) var scorch_map: texture_2d<f32>;  // R = fire scorch over the rectangle
@group(1) @binding(5) var scorch_sampler: sampler;
@group(1) @binding(6) var heights: texture_2d<f32>;     // ground height above the center at each vertex, (GRID + 1)²

struct GroundOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) local: vec2<f32>,
    @location(2) up: vec3<f32>,
};

fn vertex_height(c: vec2<i32>) -> f32 {
    return textureLoad(heights, clamp(c, vec2<i32>(0), vec2<i32>(i32(GRID))), 0).r;
}

const GRID: u32 = 48u;

@vertex
fn vs_ground(@builtin(vertex_index) index: u32) -> GroundOut {
    let quad = index / 6u;
    let corner = array<vec2<u32>, 6>(
        vec2<u32>(0u, 0u), vec2<u32>(0u, 1u), vec2<u32>(1u, 0u),
        vec2<u32>(1u, 0u), vec2<u32>(0u, 1u), vec2<u32>(1u, 1u),
    )[index % 6u];
    let cell = vec2<u32>(quad % GRID, quad / GRID) + corner;
    let f = vec2<f32>(cell) / f32(GRID);
    let local = (f * 2.0 - 1.0) * ground.extent.xy;
    // Draped over the ground; the normal from the neighboring vertices.
    let c = vec2<i32>(cell);
    let lo = max(c - 1, vec2<i32>(0));
    let hi = min(c + 1, vec2<i32>(i32(GRID)));
    let step = 2.0 * ground.extent.xy / f32(GRID);
    let dx = (vertex_height(vec2<i32>(hi.x, c.y)) - vertex_height(vec2<i32>(lo.x, c.y)))
        / (f32(hi.x - lo.x) * step.x);
    let dz = (vertex_height(vec2<i32>(c.x, hi.y)) - vertex_height(vec2<i32>(c.x, lo.y)))
        / (f32(hi.y - lo.y) * step.y);
    let rel = ground.origin.xyz + vec3<f32>(local.x, vertex_height(c), local.y);
    var out: GroundOut;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.local = local;
    out.up = normalize(vec3<f32>(-dx, 1.0, -dz));
    return out;
}

fn layer_tile(i: u32) -> f32 {
    return ground.tiles[i / 4u][i % 4u];
}

// Coverage of one paint at `p` (local meters): 1 inside, fading over its blend width through
// noise so the edge is ragged.
fn paint_mask(paint: Paint, p: vec2<f32>, wp: vec2<f32>) -> f32 {
    let d = p - paint.shape.xy;
    var sd: f32;
    if (paint.shape.w < 0.0) {
        sd = length(d) - paint.shape.z;
    } else {
        let q = abs(d) - paint.shape.zw;
        sd = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
    }
    let ragged = paint.look.z * (fbm2(wp * 0.7 + paint.shape.xy, 3) * 2.0 - 1.0)
        + paint.look.z * 0.35 * (value_noise2(wp * 4.0) * 2.0 - 1.0);
    let blend = max(paint.look.y, 0.01);
    return (1.0 - smoothstep(-blend * 0.5, blend * 0.5, sd + ragged)) * paint.look.w;
}

struct LayerSample {
    albedo: vec3<f32>,
    height: f32,
    normal: vec2<f32>,
    roughness: f32,
    ao: f32,
};

fn sample_layer(i: u32, wp: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, variation: f32) -> LayerSample {
    let tile = layer_tile(i);
    let uv = wp / tile;
    let a = textureSampleGrad(albedo_layers, layer_sampler, uv, i, dx / tile, dy / tile);
    let d = textureSampleGrad(detail_layers, layer_sampler, uv, i, dx / tile, dy / tile);
    // A second, rotated and larger copy breaks up the tiling.
    let rot = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
    let uv2 = rot * uv * 0.43 + vec2<f32>(0.37, 0.71);
    let gx = rot * dx * (0.43 / tile);
    let gy = rot * dy * (0.43 / tile);
    let a2 = textureSampleGrad(albedo_layers, layer_sampler, uv2, i, gx, gy);
    let d2 = textureSampleGrad(detail_layers, layer_sampler, uv2, i, gx, gy);
    let k = smoothstep(0.3, 0.7, variation);
    var s: LayerSample;
    s.albedo = mix(a.rgb, a2.rgb, k);
    s.height = mix(a.a, a2.a, k);
    // Normal X/Y: x along +u, y toward -v. Turn the rotated copy's back into the tile's frame.
    let n1 = d.xy * 2.0 - 1.0;
    let raw2 = d2.xy * 2.0 - 1.0;
    let g = transpose(rot) * vec2<f32>(raw2.x, -raw2.y);
    s.normal = mix(n1, vec2<f32>(g.x, -g.y), k);
    s.roughness = mix(d.z, d2.z, k);
    s.ao = mix(d.w, d2.w, k);
    return s;
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    let wp = tiling_pos(in.rel).xz;
    let dx = dpdx(wp);
    let dy = dpdy(wp);
    let p = in.local;

    // Ragged outer edge: a noisy cut-out, so the patch ends in an irregular line rather
    // than a straight one.
    let half = ground.extent.xy;
    let edge = min(half.x - abs(p.x), half.y - abs(p.y));
    let fade = ground.extent.z;
    let n_edge = fbm2(wp * 0.35, 3) + 0.25 * (value_noise2(wp * 2.3) - 0.5);
    if (edge - fade * 0.5 + (n_edge - 0.5) * fade * 1.4 < 0.0) { discard; }

    // Layer weights from the paints, in order.
    var w: array<f32, 5>;
    for (var i = 0u; i < LAYERS; i++) { w[i] = 0.0; }
    w[u32(ground.extent.w)] = 1.0;
    let count = min(u32(ground.origin.w), MAX_PAINTS);
    for (var k = 0u; k < count; k++) {
        let paint = ground.paints[k];
        let m = paint_mask(paint, p, wp);
        if (m <= 0.0) { continue; }
        let layer = u32(paint.look.x);
        for (var i = 0u; i < LAYERS; i++) {
            w[i] *= 1.0 - m;
        }
        w[layer] += m;
    }

    // Sample the layers that show and blend them by height (stones stay on top of dust,
    // water settles in the hollows).
    let variation = fbm2(wp * 0.09, 3);
    var samples: array<LayerSample, 5>;
    var top = -10.0;
    for (var i = 0u; i < LAYERS; i++) {
        if (w[i] > 0.002) {
            samples[i] = sample_layer(i, wp, dx, dy, variation);
            top = max(top, samples[i].height + w[i]);
        }
    }
    let depth = 0.18;
    var total = 0.0;
    var albedo = vec3<f32>(0.0);
    var nrm = vec2<f32>(0.0);
    var rough = 0.0;
    var ao = 0.0;
    for (var i = 0u; i < LAYERS; i++) {
        if (w[i] > 0.002) {
            let b = max(samples[i].height + w[i] - top + depth, 0.0);
            total += b;
            albedo += samples[i].albedo * b;
            nrm += samples[i].normal * b;
            rough += samples[i].roughness * b;
            ao += samples[i].ao * b;
        }
    }
    let inv = 1.0 / max(total, 1e-4);
    albedo *= inv;
    nrm *= inv;
    rough *= inv;
    ao *= inv;
    // Large-scale tone: sun-bleached and damp patches.
    let tone = fbm2(wp * 0.05 + vec2<f32>(3.1, 7.7), 3);
    albedo *= 0.85 + 0.3 * tone;

    // Burnt: char where the fire has been, with a ragged edge, grey and white ash flecks
    // and unburnt bits; the litter's relief burns away.
    let jitter = vec2<f32>(fbm2(wp * 1.1, 3), fbm2(wp * 1.1 + vec2<f32>(13.0, 7.0), 3)) - 0.5;
    let suv = (p + jitter * 0.9 + half) / (2.0 * half);
    let scorch_raw = textureSampleLevel(scorch_map, scorch_sampler, suv, 0.0).r;
    let scorch = smoothstep(0.05, 0.6, scorch_raw * (0.8 + 0.4 * value_noise2(wp * 0.8)));
    var sheen = 1.0;
    if (scorch > 0.0) {
        // Ash: thin grey streaks where stalks lay (two crossing directions), in patches.
        let qa = vec2<f32>(wp.x * 0.8 + wp.y * 0.6, wp.y * 0.8 - wp.x * 0.6);
        let qb = vec2<f32>(wp.x * 0.26 - wp.y * 0.97, wp.y * 0.26 + wp.x * 0.97);
        let streak = max(value_noise2(qa * vec2<f32>(7.0, 60.0)), value_noise2(qb * vec2<f32>(6.0, 52.0) + 7.0));
        let patch_ash = smoothstep(0.35, 0.8, fbm2(wp * 0.45 + vec2<f32>(4.0, 1.0), 3));
        let ash = smoothstep(0.76, 0.9, streak) * (0.2 + 0.8 * patch_ash);
        var char_c = mix(vec3<f32>(0.016, 0.015, 0.013), vec3<f32>(0.045, 0.042, 0.038), fbm2(wp * 2.7, 3));
        char_c = mix(char_c, vec3<f32>(0.07, 0.067, 0.064), patch_ash * 0.55);
        char_c = mix(char_c, vec3<f32>(0.15, 0.147, 0.142), ash * 0.7);
        let survives = smoothstep(0.8, 0.95, value_noise2(wp * 3.3 + 9.0)) * (1.0 - scorch_raw) * 0.6;
        let k = scorch * (1.0 - survives);
        albedo = mix(albedo, char_c, k);
        rough = mix(rough, 1.0, k);
        ao = mix(ao, 1.0, k * 0.5);
        nrm *= 1.0 - 0.3 * k;
        // Soot is about the mattest thing there is: its fluffy surface traps light, so burnt
        // ground has almost no sheen even looking into a low sun (where a plain rough
        // surface's grazing reflection would light it up like bare dirt).
        sheen = 1.0 - 0.85 * k;
    }

    // Tangent space: +X along world X, image-up along world -Z, tilted with the ground.
    let tn = vec3<f32>(nrm.x, nrm.y, sqrt(saturate(1.0 - dot(nrm, nrm))));
    let up = normalize(in.up);
    let tx = normalize(vec3<f32>(1.0, 0.0, 0.0) - up * up.x);
    let n = normalize(tx * tn.x + up * tn.z - cross(tx, up) * tn.y);

    var s = default_surface();
    s.albedo = albedo;
    s.normal = n;
    s.roughness = clamp(rough, 0.03, 1.0);
    s.occlusion = ao;
    s.reflectance = 0.5 * sheen;
    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    let color = shade_surface(s, in.rel);
    return vec4<f32>(apply_fog(color, in.rel), 1.0);
}

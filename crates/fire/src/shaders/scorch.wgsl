// Scorch and coals: a screen-space decal over the fire field. For every pixel, rebuild its
// position from scene depth, look it up in the fire map (scorch, coal glow, flames, ground
// height) and darken it to char, with glowing coals and a bright flame base on top.
// Premultiplied: color = emission + scene × (1 - alpha). Requires lib_types + lib_frame.

struct ScorchParams {
    rect: vec4<f32>,   // camera-relative x, z of the map's min corner; size x, z (m)
    ground: vec4<f32>, // camera-relative y of the map's base height, texel size (m), time, painted grounds
    self_charred: array<vec4<f32>, 4>, // painted grounds: camera-relative min x, z, max x, z
};

@group(1) @binding(0) var<uniform> params: ScorchParams;
@group(1) @binding(1) var fire_map: texture_2d<f32>;
@group(1) @binding(2) var scene_depth: texture_depth_2d;

@vertex
fn vs_scorch(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn ember_color(kelvin: f32) -> vec3<f32> {
    let t = clamp(kelvin, 800.0, 3000.0) / 100.0;
    let g = clamp(0.39008157876 * log(t) - 0.63184144378, 0.0, 1.0);
    let c = pow(vec3<f32>(1.0, g, 0.0), vec3<f32>(2.2));
    return c / max(luminance(c), 1e-4);
}

// Exposed brightness of a glow `luminance` cd/m² bright, softly capped at `cap` (exposed units)
// so coals and flame roots stay orange at night instead of burning out to white.
fn glow_exposed(luminance: f32, cap: f32) -> f32 {
    let v = luminance * frame.exposure.x;
    return cap * (1.0 - exp(-v / cap));
}

// Burning stalks lying in the ash: in each cell of a grid (cell = 1) maybe one straight stalk
// of random direction and length, `chance` of them. Returns how strongly the nearest stalk
// glows at `p` (1 on its axis) and a random number for that stalk.
fn stalks(p: vec2<f32>, chance: f32) -> vec2<f32> {
    let i = floor(p);
    var best = 0.0;
    var id = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let c = i + vec2<f32>(f32(x), f32(y));
            let r = hash12(c + vec2<f32>(5.3, 71.9));
            if (r >= chance) { continue; }
            let o = c + vec2<f32>(hash12(c), hash12(c + vec2<f32>(37.1, 11.7)));
            let a = hash12(c + vec2<f32>(91.3, 2.9)) * 6.2832;
            let half = 0.3 + 0.45 * hash12(c + vec2<f32>(13.7, 47.1));
            let d = vec2<f32>(cos(a), sin(a));
            let along = clamp(dot(p - o, d), -half, half);
            let dist = length(p - o - d * along);
            // Thin, glowing unevenly along its length and fading at its ends.
            let width = 0.05 + 0.04 * hash12(c + vec2<f32>(3.1, 8.8));
            let k = (1.0 - smoothstep(width * 0.4, width, dist))
                * (1.0 - smoothstep(half * 0.6, half, abs(along)))
                * (0.45 + 0.55 * value_noise2(vec2<f32>(along * 9.0, r * 50.0)));
            if (k > best) {
                best = k;
                id = r / chance;
            }
        }
    }
    return vec2<f32>(best, id);
}

@fragment
fn fs_scorch(in: FullscreenOut) -> @location(0) vec4<f32> {
    let depth = textureLoad(scene_depth, vec2<i32>(in.position.xy), 0);
    if (depth <= 0.0) { discard; }
    let ndc = vec4<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0, depth, 1.0);
    let wp = frame.inv_view_proj * ndc;
    let rel = wp.xyz / wp.w;
    // Ragged edges instead of the cells' stair steps: look the map up a little off, by noise.
    let tp = tiling_pos(rel);
    let cell = params.ground.y;
    let jitter = vec2<f32>(fbm2(tp.xz * 1.3, 3), fbm2(tp.xz * 1.3 + vec2(17.0, 5.0), 3)) - 0.5;
    let uv = (rel.xz + jitter * cell * 1.6 - params.rect.xy) / params.rect.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { discard; }
    let m = textureSampleLevel(fire_map, linear_clamp, uv, 0.0);
    let scorch = m.r;
    let glow = m.g;
    let flame = m.b;
    if (scorch <= 0.002 && glow <= 0.002 && flame <= 0.002) { discard; }
    let ground = params.ground.x + m.a;
    let h = rel.y - ground;
    if (h < -1.5 || h > 3.5) { discard; }
    // Char reaches up stalks and trunks, strongest at the ground.
    let height_fade = 1.0 - smoothstep(0.6, 3.5, h);
    let mottle = fbm2(tp.xz * 0.9, 4);
    let ash = smoothstep(0.55, 0.85, value_noise2(tp.xz * 3.1));
    // Burnt ground: black char with grey ash flecks. Painted ground and its grass char
    // themselves (to their own char, with ash), so there only what stands taller darkens.
    var own = 0.0;
    for (var i = 0u; i < min(u32(params.ground.w), 4u); i++) {
        let r = params.self_charred[i];
        let inside = min(min(rel.x - r.x, r.z - rel.x), min(rel.z - r.y, r.w - rel.z));
        own = max(own, smoothstep(0.0, 1.0, inside));
    }
    own *= 1.0 - smoothstep(0.9, 1.4, h);
    var alpha = scorch * mix(0.78, 0.95, mottle) * height_fade * (1.0 - own);
    var emission = vec3<f32>(0.0);
    // Coals: burnt litter, mostly black, with stalks still glowing where they lie in the ash,
    // a few lumps (stubble bases) and loose specks, gathered where it is hottest and each
    // slowly breathing. Only on the ground: the stalks standing in it are burnt out.
    let t = params.ground.z;
    let drift = vec2<f32>(t * 0.05, -t * 0.03);
    let heat = smoothstep(0.3, 0.7, fbm2(tp.xz * 1.1 + drift, 3));
    let st = stalks(tp.xz * 7.0, 0.3 + 0.5 * heat);
    let lumps = smoothstep(0.76, 0.9, fbm2(tp.xz * 9.0, 3)) * (0.3 + 0.7 * heat);
    let specks = smoothstep(0.86, 0.95, value_noise2(tp.xz * 22.0 + drift * 4.0))
        * (0.3 + 0.7 * heat);
    let glowing = max(st.x * (0.55 + 0.45 * heat), lumps);
    // That detail is a few centimeters across: where a pixel covers more ground than that (far
    // off, or the ground seen edge-on) it fades to its average glow instead of aliasing into
    // long red streaks.
    let ground_m = view_depth(rel) / (frame.proj[1].y * 0.5 * frame.viewport.y)
        / max(abs(normalize(rel).y), 0.03);
    let detail = 1.0 - smoothstep(0.012, 0.05, ground_m);
    let pattern = mix(0.1, max(glowing, specks * 0.6), detail);
    let breathe = 0.7 + 0.3 * sin(t * 1.7 + st.y * 20.0);
    let coal = glow * pattern * breathe * (1.0 - smoothstep(0.1, 0.3, h));
    emission += ember_color(900.0 + 300.0 * coal) * glow_exposed(3500.0 * coal * coal, 0.5);
    // Flame base: the burning litter glows under the flames, patchy.
    let patchy = smoothstep(0.3, 0.7, fbm2(tp.xz * 1.7 + vec2<f32>(0.0, -t * 0.4), 3));
    let base = flame * (1.0 - smoothstep(0.0, 0.5, h)) * (0.35 + 0.65 * patchy);
    emission += ember_color(1250.0) * glow_exposed(6000.0 * base, 0.35);
    // Ash shows as lighter flecks on fully burnt ground.
    let ash_light = ash * scorch * (1.0 - glow) * 0.12;
    alpha = saturate(alpha - ash_light);
    return vec4<f32>(emission, alpha);
}

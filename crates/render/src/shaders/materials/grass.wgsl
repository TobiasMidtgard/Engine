// Short grass seen from above (a lawn or meadow floor for terrain): five overlapping layers of
// blades 2-5 cm long and 2-4 mm wide, leaning every way, rising toward their tips, greener and
// paler at the tips, darker at the base; dry straw-colored blades and clover leaves mixed in,
// soil showing in the gaps. 1 m tile, 4 cm of relief.

struct Blade {
    h: f32,
    id: u32,
    t: f32,   // 0 at the base .. 1 at the tip
};

// One layer of blades: each cell of a `freq` grid holds a blade from a random root, `len`
// cells long at most (≤ 2), leaning in a random direction.
fn blade_layer(uv: vec2<f32>, freq: i32, len: f32, base: f32, seed: u32) -> Blade {
    let p = uv * f32(freq);
    let n = vec2<i32>(floor(p));
    let cell_mm = gen.dims.x * 1000.0 / f32(freq);
    var best: Blade;
    best.h = -1.0;
    best.id = 0u;
    best.t = 0.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let g = n + vec2<i32>(i, j);
            let h = hash_u2(wrap_cell(g, vec2<i32>(freq)), seed);
            let root = vec2<f32>(g) + vec2<f32>(u01(pcg(h + 1u)), u01(pcg(h + 2u)));
            let a = u01(pcg(h + 3u)) * TAU;
            let l = len * (0.5 + 0.5 * u01(pcg(h + 4u)));
            let dir = vec2<f32>(cos(a), sin(a));
            let q = p - root;
            let t = dot(q, dir) / l;
            if (t < 0.0 || t > 1.0) {
                continue;
            }
            // A slight curve and a pointed tip.
            let bend = rnd(h, 5u) * 0.25 * t * t * l;
            let across = abs(dot(q, vec2<f32>(-dir.y, dir.x)) - bend);
            let width = (3.0 + 3.0 * u01(pcg(h + 6u))) / cell_mm * 0.5 * (1.0 - t * t);
            if (across > width) {
                continue;
            }
            // Height grows toward the tip; the blade's cross-section is folded (a V).
            let fold = 1.0 - across / max(width, 1e-5);
            let bh = base + t * (8.0 + 10.0 * u01(pcg(h + 7u))) + fold * 0.6;
            if (bh > best.h) {
                best.h = bh;
                best.id = h;
                best.t = t;
            }
        }
    }
    return best;
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    var b = blade_layer(uv, 44, 1.9, 6.0, 1501u);
    var layer = 0.0;
    let b1 = blade_layer(uv + vec2<f32>(0.37, 0.11), 48, 1.9, 8.0, 1502u);
    if (b1.h > b.h) { b = b1; layer = 1.0; }
    let b2 = blade_layer(uv + vec2<f32>(0.71, 0.53), 52, 1.9, 10.0, 1503u);
    if (b2.h > b.h) { b = b2; layer = 2.0; }
    let b3 = blade_layer(uv + vec2<f32>(0.19, 0.83), 40, 1.9, 12.0, 1504u);
    if (b3.h > b.h) { b = b3; layer = 3.0; }
    let b4 = blade_layer(uv + vec2<f32>(0.53, 0.29), 56, 1.9, 14.0, 1505u);
    if (b4.h > b.h) { b = b4; layer = 4.0; }
    let b5 = blade_layer(uv + vec2<f32>(0.83, 0.67), 60, 1.9, 16.0, 1506u);
    if (b5.h > b.h) { b = b5; layer = 5.0; }
    // Thatch and soil underneath: matted dead blades.
    let soil = 3.0 + fbm1(uv, 16, 4, 0.5, 1506u) * 1.5 + scratches(uv, 96, 0.9, 1.9, 0.08, 0.0, 3.14, 1507u) * 1.5;
    // Clover: small round leaflets in a few patches.
    let clover_patch = smoothstep(0.35, 0.6, fbm1(uv, 5, 3, 0.5, 1508u));
    let leaf = blobs(uv, vec2<i32>(110), 0.25, 0.4, 1509u);
    let clover = select(0.0, 1.0, leaf.h > 0.0 && u01(leaf.id) < clover_patch * 0.8);
    let clover_h = 14.0 + leaf.h * 8.0;
    var h = max(soil, b.h);
    var kind = select(0.0, 1.0, b.h > soil);
    if (clover > 0.5 && clover_h > h) {
        h = clover_h;
        kind = 2.0;
    }
    h = clamp(h, 0.5, 39.5);
    var l: Layers;
    l.a = vec4<f32>(h * 0.001, kind, u01(b.id), b.t);
    l.b = vec4<f32>(layer, leaf.d, 0.0, 0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let kind = l.a.y;
    let hb = pcg(u32(l.a.z * 16777216.0));
    let t = l.a.w;
    let layer = l.b.x;

    // Patchy lawn: lusher and yellower areas.
    let lush = fbm1(uv, 4, 4, 0.5, 1521u);
    var green = mix(hex(0x55782au), hex(0x6a8530u), smoothstep(-0.4, 0.4, lush));
    green = vary(green, 1.0 + 0.2 * rnd(hb, 1u), 1.0 + 0.15 * rnd(hb, 2u));
    // Darker at the base, paler toward the tip; lower layers in shade are darker.
    green *= mix(0.6, 1.15, t) * (0.78 + 0.05 * layer);
    green = mix(green, hex(0x8c9a45u), smoothstep(0.7, 1.0, t) * 0.35);
    let dry = u01(pcg(hb + 3u)) < 0.07 + 0.08 * smoothstep(0.0, 0.6, -lush);
    let straw = vary(hex(0xa38f5au), 1.0 + 0.15 * rnd(hb, 4u), 1.0) * mix(0.7, 1.05, t);
    var c = select(green, straw, dry);
    let soil = mix(hex(0x4a3c26u), hex(0x6d6238u), smoothstep(-0.3, 0.5, fbm1(uv, 40, 3, 0.5, 1523u)))
        * (1.0 + 0.3 * fbm1(uv, 200, 2, 0.5, 1522u));
    let clover = hex(0x3f6a2cu) * (1.0 - 0.3 * l.b.y);
    if (kind < 0.5) {
        c = soil;
    } else if (kind > 1.5) {
        c = clover;
    }
    c *= mix(0.5, 1.0, d.ao);

    var s: Pbr;
    s.albedo = c;
    s.roughness = select(select(0.55 + 0.1 * (1.0 - t), 0.75, dry), 0.95, kind < 0.5);
    s.roughness = select(s.roughness, 0.5, kind > 1.5);
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

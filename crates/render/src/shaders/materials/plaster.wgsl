// Painted plaster wall: hand-troweled lime plaster under warm off-white satin paint. Overlapping
// trowel strokes leave soft ridges, the paint has a slight orange peel, and the wall has hairline
// cracks, a few chips down to the gray-beige plaster, and faint dirt. 2 m tile, 4 mm of relief.

// Trowel strokes: each cell of an 8 x 8 grid (25 cm) holds two strokes, ellipses 20-50 cm long
// at random angles laid in random order. The topmost stroke at a point wins: its surface is a
// slightly tilted plane, and its rim is a soft ridge of plaster pushed out by the blade.
struct Stroke {
    h: f32,
    rim: f32,
    order: f32,
};

fn trowel(uv: vec2<f32>) -> Stroke {
    let freq = 8;
    let p = uv * f32(freq);
    let n = vec2<i32>(floor(p));
    let cell_mm = gen.dims.x * 1000.0 / f32(freq);
    var best: Stroke;
    best.order = -1.0;
    best.h = 0.0;
    best.rim = 0.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            for (var k = 0u; k < 2u; k++) {
                let g = n + vec2<i32>(i, j);
                let h = hash_u2(wrap_cell(g, vec2<i32>(freq)), 501u + k * 31u);
                let center = vec2<f32>(g) + vec2<f32>(u01(pcg(h + 1u)), u01(pcg(h + 2u)));
                let a = u01(pcg(h + 3u)) * PI;
                let len = 0.4 + 0.6 * u01(pcg(h + 4u));
                let wid = len * (0.35 + 0.2 * u01(pcg(h + 5u)));
                let q = p - center;
                let along = dot(q, vec2<f32>(cos(a), sin(a))) / len;
                let across = dot(q, vec2<f32>(-sin(a), cos(a))) / wid;
                let e = along * along + across * across;
                let order = u01(pcg(h + 6u));
                if (e < 1.0 && order > best.order) {
                    best.order = order;
                    let tilt = (along * rnd(h, 7u) + across * rnd(h, 8u)) * 0.25;
                    best.h = tilt + (order - 0.5) * 0.3;
                    best.rim = smoothstep(0.45, 1.0, e) * (1.0 - smoothstep(0.97, 1.0, e) * 0.5);
                }
            }
        }
    }
    return best;
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    let w = warp(uv, vec2<i32>(5), 3, 0.01, 502u);
    let st = trowel(w);
    let base = 2.0 + fbm1(uv, 4, 4, 0.5, 503u) * 0.4 + st.h + st.rim * st.rim * 0.18;
    // Paint orange peel and sand in the plaster.
    let peel = fbm1(uv, 160, 3, 0.55, 504u) * 0.035 + fbm1(uv, 48, 2, 0.5, 505u) * 0.05;
    // Hairline cracks (spider-web map cracking in places).
    let cr = voronoi(warp(uv, vec2<i32>(8), 3, 0.01, 506u), vec2<i32>(9), 1.0, 507u);
    let where_cracked = smoothstep(0.25, 0.6, fbm1(uv, 3, 3, 0.5, 508u));
    let crack = (1.0 - smoothstep(0.0, 0.012, cr.edge)) * where_cracked;
    // Chips: patches where paint and the top coat broke off, with a crisp step.
    let chip_n = fbm1(uv, 8, 5, 0.55, 509u) + 0.5 * fbm1(uv, 2, 2, 0.5, 510u);
    let chip = smoothstep(0.5, 0.53, chip_n);
    let chip_depth = 0.6 + 0.3 * fbm1(uv, 64, 3, 0.5, 511u);

    let h = base + peel - crack * 0.25 - chip * chip_depth;
    var l: Layers;
    l.a = vec4<f32>(max(h, 0.05) * 0.001, chip, crack, st.rim);
    l.b = vec4<f32>(st.order, 0.0, 0.0, 0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let chip = l.a.y;
    let crack = l.a.z;

    // Warm off-white satin paint, slightly uneven (roller laps, touch-ups).
    var paint = hex(0xd9d1c3u);
    paint *= 1.0 + 0.035 * fbm1(uv, 4, 4, 0.5, 521u) + 0.015 * fbm1(uv, 64, 2, 0.5, 522u);
    // Dirt: darker toward hollows and in large faint patches.
    paint *= 1.0 - 0.1 * smoothstep(-0.2, 0.7, fbm1(uv, 2, 5, 0.55, 523u));
    paint *= mix(0.9, 1.0, d.ao);
    // Exposed plaster: gray-beige lime with grains.
    var plaster = hex(0xa69d8du) * (1.0 + 0.1 * fbm1(uv, 300, 2, 0.5, 524u));
    plaster *= 1.0 + 0.08 * fbm1(uv, 20, 3, 0.5, 525u);
    // The paint edge around a chip is a bit lifted and dirty.
    let rim = saturate(-d.curv_fine * 2.0) * (1.0 - chip);
    var c = mix(paint, plaster, chip);
    c *= 1.0 - 0.15 * rim;
    c = mix(c, hex(0x4a4640u), crack * 0.6);

    var s: Pbr;
    s.albedo = c;
    s.roughness = mix(0.55 + 0.04 * fbm1(uv, 48, 3, 0.5, 526u), 0.92, chip);
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

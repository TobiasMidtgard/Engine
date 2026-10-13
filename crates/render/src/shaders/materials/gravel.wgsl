// Loose angular gravel: crushed stones 1.5-3 cm across in mixed grays and buff, lying on and
// against each other in three overlapping layers, with fine grit and dust between them.
// 1 m tile, 3.5 cm of relief.

struct Stone {
    h: f32,
    id: u32,
    top: f32,
};

// One layer of stones: each Voronoi cell is a stone, faceted (height rises with the distance
// from the cell border, so ridges run along the cell's skeleton), tilted and capped.
fn stone_layer(uv: vec2<f32>, freq: i32, base: f32, seed: u32) -> Stone {
    let w = warp(uv, vec2<i32>(freq / 4), 2, 0.12 / f32(freq), seed + 1u);
    let c = voronoi(w, vec2<i32>(freq), 1.0, seed);
    let cell_mm = gen.dims.x * 1000.0 / f32(freq);
    let gap = 0.04 + 0.04 * u01(next_hash(c.id));
    let e = (c.edge - gap) * cell_mm;
    // Facets: steep near the rim, flatter on top.
    let profile = min(e * (0.9 + 0.5 * u01(pcg(c.id + 3u))), 5.0 + 6.0 * u01(pcg(c.id + 4u)) + e * 0.25);
    let tilt = dot(-c.to_center * cell_mm, vec2<f32>(rnd(c.id, 5u), rnd(c.id, 6u))) * 0.25;
    var s: Stone;
    s.h = select(-50.0, base + profile + tilt + rnd(c.id, 7u) * 3.0, e > 0.0);
    s.id = c.id;
    s.top = saturate(e / 6.0);
    return s;
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    var best = stone_layer(uv, 28, 7.0, 1201u);
    let b = stone_layer(uv, 36, 10.0, 1211u);
    if (b.h > best.h) { best = b; }
    let c = stone_layer(uv, 44, 12.0, 1221u);
    if (c.h > best.h) { best = c; }
    // Rough stone surfaces.
    let rough = fbm1(uv, 192, 3, 0.55, 1231u) * 0.5;
    // Grit and dust in between.
    let grit = 6.0 + fbm1(uv, 24, 3, 0.5, 1232u) * 2.0 + fbm1(uv, 320, 2, 0.6, 1233u) * 0.8;
    let stone_h = best.h + rough * best.top;
    let h = clamp(max(stone_h, grit), 0.5, 34.5);
    var l: Layers;
    l.a = vec4<f32>(h * 0.001, select(0.0, 1.0, stone_h > grit), u01(best.id), best.top);
    l.b = vec4<f32>(0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let is_stone = l.a.y;
    let id = l.a.z;
    let hs = pcg(u32(id * 16777216.0));

    // Mixed crushed stone: gray granite, blue-gray limestone, buff sandstone, dark basalt.
    var c: vec3<f32>;
    if (id < 0.4) {
        c = hex(0x86837du);
    } else if (id < 0.65) {
        c = hex(0x72747au);
    } else if (id < 0.85) {
        c = hex(0x8f8473u);
    } else {
        c = hex(0x55524fu);
    }
    c = vary(c, 1.0 + 0.16 * rnd(hs, 1u), 0.85 + 0.2 * rnd(hs, 2u));
    // Speckled stone texture, and a few stones veined with quartz.
    let speck = fbm1(uv, 384, 2, 0.5, 1241u);
    c *= 1.0 + 0.2 * speck;
    c = mix(c, hex(0xc9c4b8u), smoothstep(0.8, 0.95, ridged(uv, vec2<i32>(40), 2, 0.5, 1244u)) * select(0.0, 0.6, u01(pcg(hs + 5u)) < 0.2));
    // Fresh broken edges are lighter.
    c = mix(c, c * 1.3, saturate(-d.curv_fine * 1.5));
    // Grit: dusty brown-gray.
    let grit = hex(0x5d554bu) * (1.0 + 0.3 * fbm1(uv, 400, 2, 0.5, 1242u));
    c = mix(grit, c, is_stone);
    c *= mix(0.4, 1.0, d.ao * d.ao);
    // Dust over everything in patches.
    c = mix(c, hex(0x9a9184u), smoothstep(0.1, 0.7, fbm1(uv, 4, 4, 0.5, 1243u)) * 0.25);

    var s: Pbr;
    s.albedo = c;
    s.roughness = mix(0.92, 0.72 + 0.12 * u01(pcg(hs + 3u)), is_stone);
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

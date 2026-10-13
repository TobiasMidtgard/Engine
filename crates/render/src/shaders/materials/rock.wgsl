// Rough rock / cliff face: weathered gray sandstone-granite broken into angular blocks along
// joints, with fractured planar faces, ledges, faint bedding, cracks, lichen crusts, moss and dirt
// in the crevices and dark water streaks. 3 m tile, 0.3 m of relief; one slope-blur pass softens
// the sharpest fractures the way weathering does.

// One layer of fractured blocks: every Voronoi cell is a tilted plane, so cells meet in steps
// and creases like broken rock. Returns height (m) and the cell id and crack closeness.
struct Facets {
    h: f32,
    id: u32,
    crack: f32,
};

fn facets(uv: vec2<f32>, freq: i32, slope: f32, lift: f32, seed: u32) -> Facets {
    let c = voronoi(uv, vec2<i32>(freq), 0.9, seed);
    let cell_m = gen.dims.x / f32(freq);
    // Planes lean mostly the same way (bedding) with random variation.
    let g = vec2<f32>(0.35 + 0.5 * rnd(c.id, 1u), -0.4 + 0.6 * rnd(c.id, 2u)) * slope;
    let off = -c.to_center * cell_m;
    var f: Facets;
    f.h = dot(off, g) + lift * rnd(c.id, 3u);
    f.id = c.id;
    f.crack = 1.0 - smoothstep(0.0, 0.04, c.edge);
    return f;
}

// Stepped ledges: `x` quantized into steps of `step` with short, steep risers.
fn terrace(x: f32, step: f32) -> f32 {
    let k = x / step;
    return (floor(k) + smoothstep(0.75, 1.0, fract(k))) * step;
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    let w = warp(uv, vec2<i32>(3), 4, 0.06, 301u);
    // Large undulating mass of the cliff, broken into ledges.
    let mass = fbm1(w, 2, 5, 0.5, 302u);
    let ledges = terrace(mass * 0.09, 0.025) + mass * 0.02;
    // Big blocks and smaller spalls on top of them.
    let big = facets(w, 5, 0.3, 0.035, 303u);
    let small = facets(warp(uv, vec2<i32>(8), 3, 0.015, 304u), 17, 0.35, 0.01, 305u);
    // Bedding: faint horizontal steps.
    let bed = fract(w.y * 9.0 + fbm1(w, 4, 3, 0.5, 306u) * 0.6);
    let bedding = smoothstep(0.0, 0.15, bed) * 0.01;
    // Crisp fracture texture and fine grain.
    let rough = (ridged(w, vec2<i32>(24), 5, 0.55, 307u) - 0.4) * 0.02 + fbm1(uv, 160, 3, 0.55, 308u) * 0.003;
    var h = 0.15 + ledges + big.h + small.h * 0.8 + bedding + rough;
    // Some joints between the big blocks open into deep cracks; most stay tight.
    let open = smoothstep(-0.05, 0.25, fbm1(uv, 6, 3, 0.5, 309u));
    let crack = big.crack * open;
    h -= crack * 0.05;
    h = clamp(h, 0.005, 0.295);

    var l: Layers;
    l.a = vec4<f32>(h, u01(big.id), u01(small.id), crack);
    l.b = vec4<f32>(small.crack * 0.3, 0.0, 0.0, 0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    var l = layers_at(uv);
    // Weathering: soften steep fractures over 1.5 cm, keep a little of the raw crispness.
    let blurred = slope_blur(uv, 0.015, 6);
    l.a.x = mix(l.a.x, blurred, 0.65);
    return l;
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let h = l.a.x / gen.dims.y;
    let hb = pcg(u32(l.a.y * 16777216.0));
    let hs = pcg(u32(l.a.z * 16777216.0));

    // Gray rock with warm and cool blocks, banded slightly by bedding.
    var rock = mix(hex(0x77726au), hex(0x857a6cu), u01(hb));
    rock = vary(rock, 1.0 + 0.12 * rnd(hb, 5u) + 0.07 * rnd(hs, 6u), 1.0 + 0.3 * rnd(hb, 7u));
    rock *= 1.0 + 0.18 * fbm1(uv, 10, 5, 0.55, 321u);
    let grain = fbm1(uv, 400, 2, 0.5, 322u);
    rock *= 1.0 + 0.12 * grain;
    // Iron staining (rusty orange-brown) in broad patches.
    let iron = smoothstep(0.2, 0.7, fbm1(uv, 4, 4, 0.5, 323u));
    rock = mix(rock, hex(0x8a6a4cu), iron * 0.5);
    // Exposed edges are lighter and fresher, crevices and cracks darker.
    rock *= 1.0 + 0.35 * saturate(-d.curv_fine * 1.2) + 0.25 * saturate(-d.curv_broad * 0.8);
    rock *= 1.0 - 0.4 * saturate(d.curv_broad * 1.0);
    let crack = l.a.w;
    rock = mix(rock, hex(0x2f2c28u), crack * 0.6);

    // Lichen crusts on exposed faces: pale gray-green and the odd orange spot.
    let exposed = saturate(1.0 - d.curv_broad * 2.0) * smoothstep(0.35, 0.6, h);
    let lichen = smoothstep(0.25, 0.45, fbm1(uv, 14, 5, 0.55, 324u)) * exposed;
    let spot = blobs(uv, vec2<i32>(40), 0.1, 0.3, 325u);
    let orange = select(0.0, smoothstep(0.0, 0.02, spot.h), u01(spot.id) < 0.04) * exposed;
    rock = mix(rock, hex(0x9ea08au) * (1.0 + 0.25 * grain), lichen * 0.7);
    rock = mix(rock, hex(0x9c7040u), orange * 0.6);
    // Moss and dirt collect in hollows and on ledges facing up (+y in the image is up).
    let ledge = saturate(d.normal.y * 3.0) * saturate(d.curv_broad * 2.0 + 0.2);
    let moss = smoothstep(0.1, 0.5, fbm1(uv, 8, 4, 0.5, 326u) + ledge) * saturate(d.curv_broad * 3.0 + ledge);
    rock = mix(rock, hex(0x3b4421u) * (1.0 + 0.4 * grain), saturate(moss) * 0.7);
    let dirt = saturate(d.curv_broad * 2.5) * (1.0 - moss);
    rock = mix(rock, hex(0x3a3128u), dirt * 0.5);
    // Water streaks run down from the ledges.
    let streak = smoothstep(0.2, 0.7, fbm(uv, vec2<i32>(18, 2), 4, 0.5, 327u)) * smoothstep(0.3, 0.7, fbm1(uv, 3, 3, 0.5, 328u) * 0.5 + 0.5);
    rock *= 1.0 - 0.3 * streak;

    var s: Pbr;
    s.albedo = rock;
    s.roughness = 0.88 + 0.06 * grain - 0.12 * streak - 0.08 * saturate(-d.curv_fine);
    s.metallic = 0.0;
    s.ao = 1.0 - 0.35 * crack;
    s.normal = d.normal;
    return s;
}

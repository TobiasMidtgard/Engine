// Cast concrete: cloudy gray cement skin over a few exposed aggregate stones, bug holes (air
// pockets) 1-6 mm wide, faint formwork lines, hairline cracks, form-tie holes, water stains
// and efflorescence. 2 m tile, 6 mm of relief.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Cement skin: very gentle waves and a fine sandy grain.
    let skin = 3.2 + fbm1(uv, 6, 4, 0.5, 401u) * 0.35 + fbm1(uv, 200, 3, 0.55, 402u) * 0.12;
    // Formwork: faint board edges every 25 cm (8 per tile) and grain printed from the boards.
    let board = abs(fract(uv.y * 8.0) - 0.5) * 2.0;
    let board_edge = smoothstep(0.985, 1.0, board) * 0.25;
    let board_grain = fbm(uv, vec2<i32>(4, 64), 3, 0.5, 403u) * 0.08;
    // Exposed aggregate where the skin wore off.
    let agg = voronoi(uv, vec2<i32>(110), 1.0, 404u);
    let worn = smoothstep(0.15, 0.55, fbm1(uv, 5, 4, 0.55, 405u));
    let stone = select(0.0, smoothstep(0.0, 0.25, agg.edge) * 0.5, u01(agg.id) < 0.45) * worn;
    // Bug holes: round air pockets of many sizes, most of them small.
    let holes_a = blobs(uv, vec2<i32>(90), 0.08, 0.2, 406u);
    let holes_b = blobs(uv, vec2<i32>(30), 0.05, 0.12, 407u);
    let hole_a = select(0.0, smoothstep(0.0, 0.7, 1.0 - holes_a.d), u01(holes_a.id) < 0.18);
    let hole_b = select(0.0, smoothstep(0.0, 0.5, 1.0 - holes_b.d), u01(holes_b.id) < 0.2);
    let hole = max(hole_a * 1.6, hole_b * 2.6);
    // Hairline cracks.
    let crack_line = ridged(warp(uv, vec2<i32>(6), 3, 0.02, 408u), vec2<i32>(3), 4, 0.5, 409u);
    let crack = smoothstep(0.86, 0.97, crack_line) * smoothstep(0.0, 0.4, fbm1(uv, 4, 3, 0.5, 410u));
    // Form-tie holes: a 2 x 2 grid of 35 mm cones, plugged with grout set back a few mm.
    let tie_p = wrap_delta(uv * 2.0 - vec2<f32>(0.5, 0.5)) / 2.0;
    let tie_r = length(tie_p) * gen.dims.x * 1000.0;
    let tie = 1.0 - smoothstep(16.0, 18.0, tie_r);
    let tie_rim = smoothstep(16.0, 18.0, tie_r) * (1.0 - smoothstep(18.0, 24.0, tie_r));

    var h = skin - board_edge + board_grain + stone - hole - crack * 0.6;
    h = mix(h, 0.6 + tie_r * 0.08, tie);
    h += tie_rim * 0.2;

    var l: Layers;
    l.a = vec4<f32>(max(h, 0.05) * 0.001, stone, hole, crack);
    l.b = vec4<f32>(tie, u01(agg.id), worn, board);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let stone = l.a.y;
    let hole = l.a.z;
    let crack = l.a.w;
    let tie = l.b.x;

    // Cement: cool mid-gray, cloudy at several scales, with sand specks.
    var c = hex(0x8e8c87u);
    // Cloudy: lighter and darker patches of cement paste at several scales, slightly warm or cool.
    let cloud = fbm1(uv, 3, 5, 0.6, 421u);
    c *= 1.0 + 0.2 * cloud + 0.08 * fbm1(uv, 24, 3, 0.5, 422u);
    c = mix(c, c * vec3<f32>(1.03, 1.0, 0.95), smoothstep(-0.3, 0.5, fbm1(uv, 2, 3, 0.5, 427u)));
    // Damp, darker patches.
    c *= 1.0 - 0.18 * smoothstep(0.2, 0.7, fbm1(warp(uv, vec2<i32>(3), 3, 0.05, 428u), 4, 5, 0.55, 429u));
    let sand = fbm1(uv, 512, 2, 0.5, 423u);
    c *= 1.0 + 0.1 * sand;
    // Formwork boards print slightly different tones.
    let board_row = floor(uv.y * 8.0);
    c *= 1.0 + 0.04 * rnd(u32(board_row) + 77u, 1u);
    // Aggregate: warm and cool pebbles.
    let agg = vary(mix(hex(0x6f6a62u), hex(0x9a9286u), l.b.y), 1.0, 1.2);
    c = mix(c, agg, saturate(stone * 2.0));
    // Holes and cracks hold dark dust.
    c = mix(c, c * 0.7, saturate(hole * 0.8));
    c = mix(c, hex(0x3d3b37u), crack * 0.7);
    // Tie plugs are a darker grout.
    c = mix(c, hex(0x6c6a66u), tie);
    // Rain stains run down from the top; white efflorescence in patches.
    let stain = smoothstep(0.1, 0.8, fbm(uv, vec2<i32>(10, 1), 4, 0.5, 424u)) * smoothstep(-0.2, 0.6, fbm1(uv, 2, 3, 0.5, 425u));
    c *= 1.0 - 0.28 * stain;
    let bloom = smoothstep(0.35, 0.8, fbm1(uv, 5, 5, 0.6, 426u));
    c = mix(c, hex(0xc4c2bcu), bloom * 0.3);
    c *= mix(0.85, 1.0, d.ao);

    var s: Pbr;
    s.albedo = c;
    s.roughness = 0.86 + 0.05 * sand - 0.1 * stain + 0.05 * bloom - 0.08 * stone;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

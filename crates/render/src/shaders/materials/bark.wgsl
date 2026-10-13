// Furrowed tree bark (old oak or pine-like): long vertical ridges that split and merge, broken
// across into plates, separated by deep V-shaped furrows that show reddish inner bark. Fibrous
// ridge tops, gray weathered surfaces, lichen and moss in patches. 1 m tile (along the trunk is
// +v), 5 cm of relief.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Furrows run along the zero lines of noise stretched along the trunk, so they wander, split
    // and rejoin like real bark. Ridges between them are 4-12 cm wide.
    let w = warp(uv, vec2<i32>(4, 2), 4, 0.03, 701u);
    let n = fbm(w, vec2<i32>(7, 1), 4, 0.5, 702u) + fbm(w, vec2<i32>(14, 2), 2, 0.5, 703u) * 0.25;
    // Distance from the furrow in mm (about), and ridge id from the sign and band of the noise.
    let across = abs(n) * 160.0;
    let band = floor(n * 3.0);
    let rid = hash_u2(vec2<i32>(i32(band), 0), 704u);
    // Plates: ridges broken across by shallower cracks every 5-20 cm.
    let cr = fbm(w + vec2<f32>(0.0, f32(rid & 255u) / 256.0), vec2<i32>(3, 10), 3, 0.5, 705u);
    let cross = (1.0 - smoothstep(0.0, 0.06, abs(cr))) * smoothstep(-0.2, 0.3, fbm1(uv, 6, 2, 0.5, 706u));
    // V furrows with steep walls, broad flat ridge tops.
    let furrow = smoothstep(0.0, 26.0, across);
    let top = 1.0 - (1.0 - furrow) * (1.0 - furrow) * (1.0 - furrow);
    // Fibrous texture along the trunk and scaly flakes on the ridges.
    let fibers = fbm(uv, vec2<i32>(48, 6), 4, 0.55, 707u) * 1.6 + fbm(uv, vec2<i32>(192, 16), 2, 0.5, 708u) * 0.6;
    let flakes = voronoi(uv, vec2<i32>(36, 20), 1.0, 709u);
    let flake = smoothstep(0.0, 0.25, flakes.edge) * 1.0 * u01(flakes.id);
    var h = 6.0 + 30.0 * top + 3.0 * rnd(rid, 1u) * top + fibers * (0.4 + 0.6 * top) + flake * top;
    h -= cross * 7.0 * top;
    h += fbm1(uv, 4, 3, 0.5, 710u) * 3.0;
    h = clamp(h, 0.5, 49.5);

    var l: Layers;
    l.a = vec4<f32>(h * 0.001, top, cross, u01(rid));
    l.b = vec4<f32>(fibers, flake, 0.0, 0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let top = l.a.y;
    let cross = l.a.z;
    let hc = pcg(u32(l.a.w * 16777216.0));
    let fibers = l.b.x;

    // Weathered gray-brown outer bark, reddish-brown inner bark in the furrows.
    var outer = vary(hex(0x6a5e52u), 1.0 + 0.12 * rnd(hc, 1u), 1.0 + 0.2 * rnd(hc, 2u));
    outer *= 1.0 + 0.12 * fibers + 0.1 * fbm1(uv, 16, 3, 0.5, 721u);
    outer = mix(outer, hex(0x8a8379u), saturate(-d.curv_fine * 1.5) * 0.5);
    let inner = hex(0x4a3324u) * (1.0 + 0.2 * fibers);
    var c = mix(inner, outer, smoothstep(0.15, 0.6, top));
    c = mix(c, inner * 0.8, cross * 0.6);
    c *= mix(0.45, 1.0, d.ao);
    // Lichen (pale gray-green crusts) on the ridges, moss deeper on one side.
    let lichen = smoothstep(0.3, 0.55, fbm1(uv, 8, 5, 0.55, 722u)) * top;
    c = mix(c, hex(0x9aa38au) * (1.0 + 0.2 * fbm1(uv, 160, 2, 0.5, 723u)), lichen * 0.75);
    let moss = smoothstep(0.2, 0.6, fbm(uv, vec2<i32>(3, 2), 4, 0.5, 724u)) * (1.0 - top * 0.5);
    c = mix(c, hex(0x4a5a22u) * (1.0 + 0.3 * fibers), moss * 0.6);

    var s: Pbr;
    s.albedo = c;
    s.roughness = 0.9 + 0.05 * fibers - 0.1 * moss;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

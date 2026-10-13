// Polished white marble tiles (Carrara-like): 60 cm tiles, 2 x 2 per tile, cut from different
// parts of the block so each shows its own soft gray clouding and branching veins. Mirror
// polish (roughness about 0.06), 2 mm light gray grout joints, very slight lippage and the odd
// chipped corner. 1.2 m tile, 2 mm of relief.

// Marble veins at a point of the block: (x) bold soft veins with a darker core, (y) a network of
// fine hairline veins, (z) diffuse gray clouding. All follow one strongly warped flow, as the
// minerals did when the rock was folded.
fn veins(p: vec2<f32>, seed: u32) -> vec3<f32> {
    let q = warp(p, vec2<i32>(2), 6, 0.3, seed);
    let n1 = fbm1(q, 2, 6, 0.55, seed + 1u);
    let bold = pow(saturate(1.0 - abs(n1) * 3.0), 6.0);
    let core = pow(saturate(1.0 - abs(n1) * 3.0), 40.0);
    let q2 = warp(q, vec2<i32>(4), 4, 0.08, seed + 2u);
    let n2 = fbm1(q2, 4, 5, 0.55, seed + 3u);
    let fine = pow(saturate(1.0 - abs(n2) * 4.0), 24.0) * smoothstep(-0.3, 0.3, fbm1(p, 3, 3, 0.5, seed + 4u));
    let cloud = smoothstep(-0.2, 0.7, fbm1(q, 3, 6, 0.6, seed + 5u));
    return vec3<f32>(bold * 0.6 + core * 0.4, fine, cloud);
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    let t = brick_grid(uv, 2, 2, 0.0, 1401u);
    let inside = to_mm(-sd_round_box(t.local, t.half - vec2<f32>(mm(1.0)), mm(0.6)));
    // Tiles sit flat to within a fraction of a millimeter; the grout is 1 mm down.
    let lip = rnd(t.id, 1u) * 0.08 + (t.local.x / t.half.x) * rnd(t.id, 2u) * 0.05;
    let bevel = smoothstep(0.0, 0.8, inside);
    var h = mix(0.6, 1.5 + lip, bevel);
    // A chipped corner on some tiles.
    let corner = length(abs(t.local) - t.half) / mm(1.0);
    let chipped = select(0.0, 1.0 - smoothstep(4.0, 6.0, corner + fbm1(uv, 64, 2, 0.5, 1402u) * 2.0), u01(next_hash(t.id)) < 0.3);
    h -= chipped * 0.5 * bevel;
    // Faint polish waviness.
    h += fbm1(uv, 8, 3, 0.5, 1403u) * 0.02;

    var l: Layers;
    l.a = vec4<f32>(h * 0.001, bevel, chipped, u01(t.id));
    l.b = vec4<f32>(t.local / t.half, 0.0, 0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let bevel = l.a.y;
    let chipped = l.a.z;
    let ht = pcg(u32(l.a.w * 16777216.0));
    // Each tile samples the block at its own offset and rotation (veins don't continue across
    // joints). The offset is whole tiles so the pattern still repeats.
    let a = rnd(ht, 3u) * 3.14159;
    let rot = mat2x2<f32>(cos(a), sin(a), -sin(a), cos(a));
    let p = rot * (uv - vec2<f32>(0.5)) + vec2<f32>(rnd(ht, 4u), rnd(ht, 5u)) * 3.0;
    let v = veins(p, 1410u);

    var c = hex(0xeceae6u);
    c = mix(c, hex(0xcfcecbu), v.z * 0.55);
    c = mix(c, hex(0x9a9ca0u), v.y * 0.55);
    c = mix(c, hex(0x6c6e73u), v.x * 0.8);
    c *= 1.0 + 0.02 * fbm1(uv, 256, 2, 0.5, 1430u);
    // Grout and chips.
    let grout = hex(0xb9b5adu) * (1.0 + 0.1 * fbm1(uv, 300, 2, 0.5, 1431u));
    c = mix(grout, c, bevel);
    c = mix(c, c * 0.92, chipped);

    var s: Pbr;
    s.albedo = c;
    s.roughness = mix(0.85, 0.06 + 0.03 * fbm1(uv, 16, 3, 0.5, 1432u) + 0.1 * v.x, bevel);
    s.roughness = mix(s.roughness, 0.6, chipped);
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

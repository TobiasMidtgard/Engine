// Rusty steel plate left out in the weather: about half of it is rust, from a thin brown film
// to thick, blistered, dark crusts with flaking rims and pits under them; between the patches
// the steel is dull and mottled with oxide, darker mill scale and bright scratches, and rust
// stains run down from the patches. Bare steel is metallic (F0 about 0.55); rust and scale are
// not. 1 m tile, 3 mm of relief.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Scratches and gouges, mostly in one direction.
    let fine = scratches(uv, 64, 0.35, 1.8, 0.02, 0.15, 0.35, 801u);
    let deep = scratches(uv, 16, 0.2, 1.8, 0.012, -0.4, 0.8, 802u);
    let scratch = max(fine * 0.6, deep);
    // Rust coverage: broad patches with a ragged, eaten front; it also follows the gouges.
    let w = warp(uv, vec2<i32>(4), 4, 0.04, 803u);
    let patches = fbm1(w, 3, 6, 0.55, 804u);
    let front = fbm1(uv, 40, 4, 0.6, 805u) * 0.16;
    let along = scratches(uv, 16, 0.2, 1.8, 0.06, -0.4, 0.8, 802u) * 0.25;
    let rust_n = patches + front + along;
    let rust = smoothstep(-0.12, -0.04, rust_n);
    let heavy = smoothstep(0.08, 0.3, rust_n);
    // Thick crust stands up to 1.2 mm proud: blisters, granular texture, flaking rims where the
    // crust lifts off at its edge; pits eat into the steel under it.
    let blister = blobs(uv, vec2<i32>(70), 0.15, 0.45, 806u);
    let granular = fbm1(uv, 160, 4, 0.6, 807u);
    let rim = smoothstep(0.03, 0.08, rust_n) * (1.0 - smoothstep(0.08, 0.16, rust_n));
    let crust = rust * (0.12 + 0.08 * granular) + heavy * (0.35 + 0.3 * granular + blister.h * 1.4) + rim * 0.25;
    let pits = blobs(uv, vec2<i32>(160), 0.08, 0.25, 808u);
    let pit = select(0.0, pits.h * 2.5, u01(pits.id) < 0.4) * rust * (1.0 - heavy);
    // Steel: brushed in one direction, faintly wavy from rolling.
    let brush = fbm(uv, vec2<i32>(2, 512), 2, 0.5, 809u) * 0.01;
    var h = 1.2 + fbm1(uv, 3, 3, 0.5, 810u) * 0.08 + brush - scratch * 0.1 + crust - pit;
    h = clamp(h, 0.05, 2.95);

    var l: Layers;
    l.a = vec4<f32>(h * 0.001, rust, heavy, scratch);
    l.b = vec4<f32>(pit, rust_n, granular, 0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let rust = l.a.y;
    let heavy = l.a.z;
    let scratch = l.a.w;
    let rust_n = l.b.y;
    let granular = l.b.z;

    // Steel: dull and mottled with a thin oxide film, brighter along fresh scratches.
    let brush = fbm(uv, vec2<i32>(4, 256), 3, 0.5, 821u);
    let dull = smoothstep(-0.3, 0.5, fbm1(uv, 6, 5, 0.55, 828u));
    var steel = mix(hex(0xc4c5c6u), hex(0xafb0b2u), dull) * (1.0 + 0.04 * brush);
    // Mill scale: flaky dark blue-gray patches with sharp, ragged borders.
    let scale_n = fbm1(warp(uv, vec2<i32>(6), 3, 0.02, 827u), 5, 6, 0.6, 822u);
    let scale = smoothstep(0.12, 0.17, scale_n) * (1.0 - scratch);
    // Rust: brown film, dark crusts, orange where it is fresh and flaking.
    var rc = mix(hex(0x6b3a22u), hex(0x3e2416u), heavy);
    rc = mix(rc, hex(0x8a4c26u), smoothstep(0.2, 0.7, granular) * 0.6 * (1.0 - heavy * 0.5));
    rc = mix(rc, hex(0x2a1810u), saturate(d.curv_fine * 2.0) * 0.6);
    rc *= 1.0 + 0.15 * fbm1(uv, 96, 3, 0.5, 823u);
    // Stains run down (+v) from the rust onto the steel.
    let streak = smoothstep(0.0, 0.5, fbm(uv, vec2<i32>(24, 2), 4, 0.5, 825u)) * smoothstep(-0.25, 0.1, rust_n);
    let stain = streak * (1.0 - rust) * 0.7;

    var c = steel;
    var metal = 1.0;
    c = mix(c, hex(0x3d4045u), scale * 0.8);
    metal = mix(metal, 0.3, scale * 0.8);
    c = mix(c, steel * 1.1, scratch);
    metal = max(metal, scratch);
    c = mix(c, hex(0x5e3a26u), stain);
    metal = mix(metal, 0.0, stain);
    c = mix(c, rc, rust);
    metal = mix(metal, 0.0, rust);

    var s: Pbr;
    s.albedo = c;
    var rough = 0.42 + 0.12 * dull + 0.05 * brush - 0.15 * scratch;
    rough = mix(rough, 0.6, scale * 0.8);
    rough = mix(rough, 0.72, stain);
    rough = mix(rough, 0.8 + 0.12 * heavy + 0.05 * granular, rust);
    s.roughness = rough;
    s.metallic = metal;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

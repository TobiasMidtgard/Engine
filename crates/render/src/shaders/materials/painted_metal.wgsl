// Painted steel panels: 50 cm plates with recessed seams and rows of domed rivets, in
// semi-gloss industrial green. Paint chips off the raised edges and rivet heads down to red
// oxide primer and bright steel, scratches cut through it, rust weeps from the rivets and seams,
// and grime collects in the recesses. 1 m tile, 6 mm of relief.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // 2 x 2 panels per tile; 3 mm seams with rolled edges.
    let t = brick_grid(uv, 2, 2, 0.0, 901u);
    let inside = to_mm(-sd_round_box(t.local, t.half - vec2<f32>(mm(1.5)), mm(6.0)));
    let edge = smoothstep(0.0, 4.0, inside);
    var h = 1.0 + 3.0 * sqrt(edge) + rnd(t.id, 1u) * 0.15;
    // Gentle oil-canning of each sheet.
    h += fbm1(uv, 4, 3, 0.5, 902u) * 0.25 * edge;
    // Rivets 25 mm from the edges, every 56 mm, 9 mm heads.
    let rows = vec2<f32>(to_mm(t.half.x), to_mm(t.half.y)) - vec2<f32>(25.0);
    let pmm = vec2<f32>(to_mm(t.local.x), to_mm(t.local.y));
    let along = vec2<f32>(pmm.x - round(pmm.x / 56.25) * 56.25, pmm.y - round(pmm.y / 56.25) * 56.25);
    let dx = abs(abs(pmm.x) - rows.x);
    let dy = abs(abs(pmm.y) - rows.y);
    let on_v = length(vec2<f32>(dx, along.y));
    let on_h = length(vec2<f32>(dy, along.x));
    let rd = select(min(on_v, on_h), 99.0, abs(pmm.x) > rows.x + 5.0 || abs(pmm.y) > rows.y + 5.0);
    let rivet = sqrt(saturate(1.0 - (rd / 4.5) * (rd / 4.5)));
    h = max(h, 4.0 + rivet * 1.6 - select(0.0, 0.3, rd > 4.5));
    // Dents.
    let dent = blobs(uv, vec2<i32>(8), 0.1, 0.25, 903u);
    let dent_shape = (1.0 - dent.d * dent.d) * (1.0 - dent.d * dent.d);
    h -= select(0.0, dent_shape * 0.5, u01(dent.id) < 0.3 && dent.h > 0.0) * edge;
    // Scratches through the paint (a 0.1 mm step) and the chips' paint edges.
    let scratch = max(scratches(uv, 20, 0.35, 1.9, 0.012, 0.3, 1.2, 904u), scratches(uv, 64, 0.3, 1.9, 0.02, -0.2, 2.0, 905u) * 0.8);
    var l: Layers;
    l.a = vec4<f32>(h * 0.001, edge, rivet, u01(t.id));
    l.b = vec4<f32>(scratch, rd, inside, 0.0);
    return l;
}

// How much the surface bulges at `uv` over a ring of `r` meters (> 0 on edges and domes).
fn convexity(uv: vec2<f32>, r: f32) -> f32 {
    let h0 = height_at(uv);
    var sum = 0.0;
    for (var k = 0; k < 8; k++) {
        let a = f32(k) * (TAU / 8.0);
        sum += height_at(uv + vec2<f32>(cos(a), sin(a)) * (r / gen.dims.x));
    }
    return (h0 - sum / 8.0) / r;
}

// Edge wear: paint chips off where the sheet bulges (rolled seam edges, rivet heads) and where
// scratches cut it, first to the primer, then to bare steel. The paint and primer are 0.1 mm
// each, so a chip leaves a crisp step in the height.
fn preset_refine(uv: vec2<f32>) -> Layers {
    var l = layers_at(uv);
    // Rings at least a texel and a half wide, so the wear looks the same at any map size.
    let fine_r = max(0.0015, 1.5 * texel_m());
    let convex = saturate(convexity(uv, fine_r) * 1.5) + saturate(convexity(uv, max(0.006, 4.0 * texel_m())) * 1.2);
    let wear_n = fbm1(uv, 48, 5, 0.6, 921u) * 0.5 + 0.5;
    // Some areas are knocked about much more than others.
    let abuse = smoothstep(-0.4, 0.5, fbm1(uv, 3, 3, 0.5, 926u));
    let e = convex * (0.6 + 0.8 * abuse) + wear_n * 0.8 + l.a.z * 0.3 + abuse * 0.15 - 0.4;
    let scratch = l.b.x;
    // Knocks in the open field of a panel too, more where it is abused.
    let knock = fbm1(uv, 20, 5, 0.6, 927u) + abuse * 0.25;
    let chip = max(max(smoothstep(0.34, 0.38, e), smoothstep(0.36, 0.39, knock)), smoothstep(0.25, 0.4, scratch));
    let bare = max(max(smoothstep(0.44, 0.48, e), smoothstep(0.41, 0.44, knock)), smoothstep(0.5, 0.65, scratch));
    l.a.x -= (chip + bare) * 0.0001;
    l.b.z = chip;
    l.b.w = bare;
    return l;
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let edge = l.a.y;
    let rivet = l.a.z;
    let hp = pcg(u32(l.a.w * 16777216.0));
    let scratch = l.b.x;
    let rd = l.b.y;

    let wear_n = fbm1(uv, 48, 5, 0.6, 921u) * 0.5 + 0.5;
    let chip = l.b.z;
    let to_metal = l.b.w;

    // Paint: industrial green, each panel a shade off, faded and dusty in patches.
    var paint = vary(hex(0x55695au), 1.0 + 0.05 * rnd(hp, 1u), 0.8 + 0.05 * rnd(hp, 2u));
    let fade = smoothstep(-0.2, 0.7, fbm1(uv, 3, 4, 0.5, 922u));
    paint = mix(paint, vary(paint, 1.25, 0.7), fade * 0.5);
    let primer = hex(0x7a3527u) * (1.0 + 0.1 * wear_n);
    let steel = hex(0xc0c1c2u) * (1.0 + 0.08 * fbm(uv, vec2<i32>(4, 128), 2, 0.5, 923u));
    var c = mix(paint, primer, chip);
    c = mix(c, steel, to_metal);
    var metal = to_metal;

    // Rust weeps from rivets and seams, running down (+v).
    let below_rivet = smoothstep(14.0, 4.0, rd) + (1.0 - edge);
    let weep = saturate(below_rivet) * smoothstep(0.0, 0.5, fbm(uv, vec2<i32>(32, 3), 4, 0.5, 924u));
    let rust_c = mix(hex(0x6e3519u), hex(0x9a5527u), wear_n);
    c = mix(c, rust_c, weep * 0.65);
    metal *= 1.0 - smoothstep(0.3, 0.7, weep);
    // Grime: in the seams, around rivets, and settled in broad patches and streaks.
    let settled = smoothstep(0.0, 0.7, fbm1(uv, 4, 5, 0.55, 928u)) * 0.35
        + smoothstep(0.2, 0.8, fbm(uv, vec2<i32>(16, 2), 4, 0.5, 929u)) * 0.25;
    let grime = saturate(1.0 - d.ao * 1.1 + 0.15) + (1.0 - edge) * 0.6 + settled;
    // Dirt sits in a thin film on bare steel, which keeps most of its brightness.
    c = mix(c, hex(0x2e2a24u), saturate(grime) * 0.6 * (1.0 - 0.7 * metal));
    // Only thick grime hides the bare steel's shine; a dusting leaves it metallic.
    metal *= 1.0 - smoothstep(0.35, 0.8, grime);
    c *= 1.0 - 0.1 * (1.0 - metal) * smoothstep(0.0, 0.8, fbm1(uv, 8, 4, 0.5, 925u));

    var s: Pbr;
    s.albedo = c;
    var rough = 0.42 + 0.12 * fade + 0.05 * wear_n;
    rough = mix(rough, 0.72, chip);
    rough = mix(rough, 0.3, to_metal);
    rough = mix(rough, 0.85, max(weep * 0.65, saturate(grime) * 0.6));
    s.roughness = rough;
    s.metallic = metal;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

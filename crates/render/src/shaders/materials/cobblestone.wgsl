// Cobblestone paving: rounded granite cobbles about 12-16 cm across, laid in loose rows and worn
// smooth on top, set 2-4 cm proud of sandy, dirty joints with grit and moss. 1.2 m tile.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Rows of stones: a Voronoi pattern squashed a little across the rows, wandering.
    let w = warp(uv, vec2<i32>(4), 3, mm(18.0), 101u);
    let c = voronoi(w, vec2<i32>(9, 8), 0.75, 102u);
    let id = c.id;
    // Distance inside the stone (mm) after the joint (6-12 mm wide, varying along the joint).
    let gap = 0.035 + 0.02 * (fbm1(uv, 24, 2, 0.5, 103u) * 0.5 + 0.5);
    let e = (c.edge - gap) * 1200.0 / 9.0;
    // Rounded, irregular outline: the polygon is clipped by a circle around the stone's
    // center (corners become joints), with noise on the distance.
    let radius = (0.5 + 0.12 * u01(pcg(id + 9u))) * 1200.0 / 9.0;
    let inside = min(e, radius - c.f1 * 1200.0 / 9.0) + fbm1(uv, 48, 3, 0.5, 104u) * 4.0;

    // Rounded dome: a quarter circle over the outer 20-30 mm, a smooth crown in the middle.
    let rim = 20.0 + 10.0 * u01(next_hash(id));
    let t = saturate(inside / rim);
    let dome = sqrt(1.0 - (1.0 - t) * (1.0 - t));
    let crown = (1.0 - (c.f1 / 0.65) * (c.f1 / 0.65)) * 6.0;
    let tilt = dot(-c.to_center, vec2<f32>(rnd(id, 1u), rnd(id, 2u))) * 6.0;
    // Worn granite: broad dents, crystal grain, a few chips on the rim.
    let wear = fbm1(uv, 32, 4, 0.5, 105u) * 1.4 + fbm1(uv, 256, 2, 0.5, 106u) * 0.35;
    let chips = max(0.0, fbm1(uv, 80, 3, 0.5, 107u) - 0.25) * 9.0 * (1.0 - t);
    let stone = 15.0 + 2.5 * rnd(id, 3u) + 20.0 * dome + crown + tilt + wear - chips;
    let edge = select(-40.0, stone, inside > 0.0);

    // Joints: compacted sand and grit 12-18 mm high, with pebbles.
    let fill = 13.0 + 3.0 * fbm1(uv, 12, 3, 0.5, 108u) + 1.5 * fbm1(uv, 160, 3, 0.6, 109u);
    let pebble = blobs(uv, vec2<i32>(70), 0.12, 0.3, 110u);
    let joint = fill + select(0.0, pebble.h * 14.0, u01(pebble.id) < 0.5);
    let h = smax(edge, joint, 2.5);
    let in_stone = smoothstep(-0.5, 1.5, edge - joint);

    var l: Layers;
    l.a = vec4<f32>(h * 0.001, in_stone, t, u01(id));
    l.b = vec4<f32>(c.to_center, select(0.0, 1.0, u01(pebble.id) < 0.5 && pebble.h > 0.0), chips);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn stone_color(id: f32, h: u32) -> vec3<f32> {
    var c: vec3<f32>;
    if (id < 0.35) {
        c = hex(0x6e6b66u);       // gray granite
    } else if (id < 0.55) {
        c = hex(0x7a7067u);       // warm gray
    } else if (id < 0.75) {
        c = hex(0x5a5d62u);       // cool blue-gray
    } else if (id < 0.9) {
        c = hex(0x766760u);       // pinkish granite
    } else {
        c = hex(0x4d4b48u);       // dark basalt
    }
    return vary(c, 1.0 + 0.14 * rnd(h, 11u), 0.85 + 0.12 * rnd(h, 12u));
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let id = l.a.w;
    let h = pcg(u32(id * 16777216.0));
    let in_stone = l.a.y;
    let top = l.a.z;

    // Granite: base tone, mottling, black mica and white feldspar specks.
    var stone = stone_color(id, h);
    stone *= 1.0 + 0.15 * fbm1(uv, 40, 3, 0.5, 121u);
    let speck = fbm1(uv, 512, 2, 0.5, 122u);
    stone = mix(stone, hex(0x2a2a2bu), smoothstep(0.35, 0.6, speck) * 0.55);
    stone = mix(stone, hex(0xb3aea6u), smoothstep(0.4, 0.65, -speck) * 0.35);
    // Polished tops are a touch darker and cleaner; rims and chips are lighter and dusty.
    stone *= mix(1.08, 0.92, smoothstep(0.6, 1.0, top));
    stone = mix(stone, stone * 1.25, saturate(-d.curv_fine * 1.5) * (1.0 - top));
    // Dirt creeping up the sides from the joints.
    let dirt_up = (1.0 - smoothstep(0.0, 0.5, top)) * (0.6 + 0.4 * fbm1(uv, 30, 3, 0.5, 123u));
    stone = mix(stone, hex(0x4a4036u), dirt_up * 0.55);

    // Joints: sandy grit, dark damp soil, moss in some of them.
    var joint = mix(hex(0x6b6153u), hex(0x3b3329u), smoothstep(-0.3, 0.5, fbm1(uv, 20, 4, 0.5, 124u)));
    joint *= 1.0 + 0.3 * fbm1(uv, 300, 2, 0.5, 125u);
    let pebble = l.b.z;
    joint = mix(joint, vary(hex(0x7d776du), 1.0 + 0.3 * rnd(h, 13u), 1.0), pebble * 0.8);
    let moss = smoothstep(0.1, 0.55, fbm1(uv, 6, 4, 0.55, 126u)) * (1.0 - pebble);
    joint = mix(joint, hex(0x3c4a1eu) * (1.0 + 0.4 * fbm1(uv, 200, 2, 0.5, 127u)), moss * 0.85);

    var albedo = mix(joint, stone, in_stone);
    // Large-scale grime and damp patches over everything.
    albedo *= 1.0 - 0.22 * smoothstep(-0.3, 0.6, fbm1(uv, 3, 4, 0.5, 128u));

    var s: Pbr;
    s.albedo = albedo;
    let stone_rough = mix(0.82, 0.5, smoothstep(0.55, 1.0, top)) + 0.06 * speck;
    let joint_rough = mix(0.95, 0.78, moss);
    s.roughness = mix(joint_rough, stone_rough, in_stone);
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

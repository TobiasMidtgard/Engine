// Red brick wall: 215 x 65 mm bricks in running bond with 10 mm raked mortar joints, 6 bricks by
// 18 courses per 1.35 m tile. Hand-laid wobble, per-brick size, tilt and color (orange-red to
// over-fired purple-brown clinkers), sand-faced texture with pits, chipped arrises that show
// lighter fresh clay, sandy mortar recessed about 9 mm, soot and salt bloom.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Hand-laid: the courses wander by a millimeter or two.
    let w = uv + vec2<f32>(fbm1(uv, 3, 3, 0.5, 11u) * mm(2.0), fbm1(uv, 5, 3, 0.5, 12u) * mm(1.2));
    let t = brick_grid(w, 6, 18, 0.5, 7u);
    let id = t.id;
    // Each brick is a little off in size and position inside its cell.
    let half = t.half - vec2<f32>(mm(5.0 + 1.2 * rnd(id, 1u)), mm(5.0 + 0.8 * rnd(id, 2u)));
    let local = t.local - vec2<f32>(mm(1.2 * rnd(id, 3u)), mm(0.7 * rnd(id, 4u)));
    let inside = to_mm(-sd_round_box(local, half, mm(3.0)));

    // Chipped arrises: bites along the edges, deeper near the corners and on some bricks.
    let corner = to_mm(length(max(abs(local) - half + vec2<f32>(mm(25.0)), vec2<f32>(0.0))));
    let damage = 0.5 + 0.9 * u01(next_hash(id));
    let bite = fbm1(uv, 40, 5, 0.55, 21u) * 0.5 + 0.5;
    let chip = max(0.0, (bite - 0.42) * 22.0) * damage + corner * 0.12 * damage * bite;

    // Brick face: a few mm proud of the mortar, slightly tilted, wavy and sand-faced, with pits.
    let face_wave = fbm1(uv, 24, 3, 0.5, 31u) * 0.35 + fbm1(uv, 96, 3, 0.5, 32u) * 0.12;
    let tilt = (local.x / half.x) * rnd(id, 5u) * 0.6 + (local.y / half.y) * rnd(id, 6u) * 0.4;
    let grain = fbm1(uv, 384, 2, 0.6, 33u) * 0.07;
    let pit_cell = blobs(uv, vec2<i32>(180, 180), 0.12, 0.35, 34u);
    let pits = select(0.0, pit_cell.h * 3.0, u01(pit_cell.id) < 0.2 + 0.2 * fbm1(uv, 6, 2, 0.5, 36u));
    let face = 14.0 + 0.6 * rnd(id, 7u) + face_wave + tilt + grain - pits;
    // Broken areas drop 2-5 mm below the face with a rough fracture surface.
    let fracture = face - 2.5 - 2.5 * (fbm1(uv, 64, 4, 0.55, 35u) * 0.5 + 0.5);
    let broken = 1.0 - smoothstep(chip - 0.8, chip + 0.5, inside);
    var brick = mix(face, fracture, broken);
    // Rounded arris, then the near-vertical side down into the joint.
    brick = min(brick, face - 1.4 + smoothstep(0.0, 2.0, inside) * 1.4);
    brick = min(brick, 4.0 + inside * 5.0);

    // Raked mortar: concave across the joint, sandy, with the odd lump squeezed out.
    let joint = saturate(-inside / 5.0);
    let sand = fbm1(uv, 256, 3, 0.6, 41u) * 0.35 + fbm1(uv, 64, 3, 0.5, 42u) * 0.3;
    let lumps = max(0.0, fbm1(uv, 30, 3, 0.5, 43u) - 0.35) * 5.0;
    let mortar = 5.0 - 1.0 * sqrt(joint) + sand + lumps;
    let h = smax(brick, mortar, 0.6);

    let in_brick = smoothstep(-0.3, 0.3, brick - mortar);
    var l: Layers;
    l.a = vec4<f32>(h * 0.001, in_brick, broken * in_brick, u01(id));
    l.b = vec4<f32>(local / half, pits, inside);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn brick_color(id: f32, h: u32) -> vec3<f32> {
    var c: vec3<f32>;
    if (id < 0.5) {
        c = hex(0x9a4a33u);       // common red
    } else if (id < 0.72) {
        c = hex(0xae5a3cu);       // orange-red, lightly fired
    } else if (id < 0.88) {
        c = hex(0x6e3428u);       // dark red-brown
    } else if (id < 0.95) {
        c = hex(0x4e2d2bu);       // over-fired purple-brown
    } else {
        c = hex(0xb47a55u);       // pale, under-fired
    }
    return vary(c, 1.0 + 0.16 * rnd(h, 11u), 0.82 + 0.12 * rnd(h, 12u));
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let id = l.a.w;
    let h = pcg(u32(id * 16777216.0));
    let in_brick = l.a.y;
    let broken = l.a.z;
    let local = l.b.xy;

    // Brick: mottled, with darker kiln flashing toward one end and iron speckles.
    var brick = brick_color(id, h);
    let mottle = fbm1(uv, 20, 4, 0.55, 51u);
    brick *= 1.0 + 0.18 * mottle;
    let flash_side = select(-1.0, 1.0, rnd(h, 13u) > 0.0);
    let flash = smoothstep(0.1, 1.0, local.x * flash_side) * (0.5 + 0.5 * fbm1(uv, 16, 3, 0.5, 52u));
    brick = mix(brick, brick * vec3<f32>(0.55, 0.5, 0.55), flash * 0.45 * u01(next_hash(h)));
    let speck = smoothstep(0.55, 0.75, fbm1(uv, 300, 2, 0.5, 53u));
    brick = mix(brick, hex(0x3a2420u), speck * 0.6);
    // Sand-faced: grains catch a little light on the bumps.
    brick *= 1.0 - 0.25 * saturate(d.curv_fine * 2.0) + 0.08 * saturate(-d.curv_fine * 3.0);
    // Fresh broken clay is lighter and more orange.
    brick = mix(brick, vary(hex(0xc0704au), 1.0 + 0.1 * rnd(h, 14u), 1.0), broken * 0.75);
    // Pits hold dust.
    brick = mix(brick, brick * 0.7, saturate(l.b.z * 3.0));

    // Mortar: sandy gray-beige with light and dark grains, darker deep in the joint.
    var mortar = hex(0xa59e90u) * (1.0 + 0.12 * fbm1(uv, 12, 4, 0.5, 61u));
    let grains = fbm1(uv, 512, 2, 0.5, 62u);
    mortar *= 1.0 + 0.22 * grains;
    mortar *= mix(0.65, 1.0, d.ao);

    var albedo = mix(mortar, brick, in_brick);
    // Weathering: soot, grime running in large patches, and white salt bloom near the joints.
    let grime = smoothstep(-0.2, 0.7, fbm1(uv, 3, 5, 0.55, 71u));
    albedo *= 1.0 - 0.28 * grime;
    let salt = smoothstep(0.35, 0.75, fbm1(uv, 6, 5, 0.6, 72u)) * (1.0 - smoothstep(0.0, 9.0, l.b.w));
    albedo = mix(albedo, hex(0xc8c3b6u), salt * 0.35 * in_brick + salt * 0.25 * (1.0 - in_brick));

    var s: Pbr;
    s.albedo = albedo;
    let glaze = select(0.0, 0.12, id >= 0.88 && id < 0.95);
    s.roughness = mix(0.94, mix(0.82 - glaze, 0.92, broken), in_brick) + 0.05 * mottle + 0.05 * salt;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

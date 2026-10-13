// Oiled oak floor planks, 150 mm wide, running along u, with staggered butt joints. Flat-sawn
// grain (cathedral arches where the saw cut through the growth cones), straight rift grain on
// some boards, open pores, knots that bend the rings around them, eased edges, 1-2 mm dirty
// gaps, slight cupping, wear and the odd dent. 1.2 m tile, 6 mm of relief.

struct Plank {
    x: f32,     // along the board from its start (mm)
    y: f32,     // across the board from its center (mm)
    len: f32,   // board length (mm)
    id: u32,
    edge: f32,  // distance to the nearest board edge (mm)
};

const PLANK_ROWS: i32 = 8;

fn plank_at(uv: vec2<f32>) -> Plank {
    let tile_mm = gen.dims.x * 1000.0;
    let row = i32(floor(uv.y * f32(PLANK_ROWS)));
    let row_w = wrap_cell(vec2<i32>(row, 0), vec2<i32>(PLANK_ROWS, 1)).x;
    let rh = hash_u2(vec2<i32>(row_w, 0), 601u);
    // One or two butt joints per row and tile, staggered from row to row.
    let j0 = u01(rh);
    let two = u01(pcg(rh + 1u)) < 0.45;
    let len0 = select(1.0, 0.38 + 0.24 * u01(pcg(rh + 2u)), two);
    let a = fract(uv.x - j0);
    let seg = select(0u, 1u, a >= len0);
    var p: Plank;
    let start = select(0.0, len0, seg == 1u);
    p.len = select(len0, 1.0 - len0, seg == 1u) * tile_mm;
    p.x = (a - start) * tile_mm;
    p.y = (fract(uv.y * f32(PLANK_ROWS)) - 0.5) * tile_mm / f32(PLANK_ROWS);
    p.id = pcg(rh ^ (seg * 0x9e3779b9u + 17u));
    let half_w = tile_mm / f32(PLANK_ROWS) * 0.5;
    p.edge = min(half_w - abs(p.y), min(p.x, p.len - p.x));
    return p;
}

// Growth-ring distance (mm) at a point of a board: the saw cut a cone of rings around the pith.
fn ring_radius(p: Plank, uv: vec2<f32>) -> f32 {
    let id = p.id;
    let y0 = rnd(id, 1u) * 160.0;
    let z0 = 12.0 + 120.0 * u01(pcg(id + 2u));
    let taper = 0.01 + 0.03 * u01(pcg(id + 3u));
    // Grain wanders slowly along the board and a little across it.
    let wander = fbm(uv + vec2<f32>(0.0, f32(id & 1023u) / 1024.0), vec2<i32>(6, 3), 4, 0.5, 602u) * 9.0;
    let z = z0 + taper * (p.x - p.len * 0.5);
    var r = sqrt((p.y + wander - y0) * (p.y + wander - y0) + z * z);
    r += fbm(uv, vec2<i32>(3, 24), 3, 0.5, 603u + (id & 7u)) * 1.2;
    return r;
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    let p = plank_at(uv);
    let id = p.id;
    var r = ring_radius(p, uv);
    // A knot on some boards: the rings swirl around it.
    let has_knot = u01(pcg(id + 9u)) < 0.4;
    let k = vec2<f32>(p.len * (0.15 + 0.7 * u01(pcg(id + 10u))), rnd(id, 11u) * 45.0);
    let kr = 5.0 + 9.0 * u01(pcg(id + 12u));
    let kd = length((vec2<f32>(p.x, p.y) - k) * vec2<f32>(0.6, 1.0));
    let knot_pull = select(0.0, exp(-kd * kd / (kr * kr * 9.0)), has_knot);
    r -= knot_pull * kr * 3.0;
    let knot = select(0.0, 1.0 - smoothstep(kr * 0.85, kr, kd), has_knot);
    // Rings vary in width from year to year.
    let spacing = 2.4 + 2.2 * u01(pcg(id + 4u));
    let year = r / spacing;
    let ring = fract(year + gnoise(vec2<f32>(year * 0.4, f32(id & 255u) + 0.5), vec2<i32>(4096, 256), 611u) * 0.35);
    let late = smoothstep(0.55, 0.9, ring) * (1.0 - smoothstep(0.93, 1.0, ring));

    // Board surface: tiny height and cupping differences, brushed grain (soft earlywood lower).
    let half_w = gen.dims.x * 1000.0 / f32(PLANK_ROWS) * 0.5;
    let cup = (p.y / half_w) * (p.y / half_w) * 0.12 * u01(pcg(id + 5u));
    let pores = fbm(uv + vec2<f32>(0.0, f32(id & 255u) / 256.0), vec2<i32>(12, 640), 2, 0.5, 604u);
    var h = 4.6 + 0.12 * rnd(id, 6u) + cup + late * 0.05 - smoothstep(0.35, 0.8, pores) * 0.06;
    h -= knot * 0.08 - select(0.0, (1.0 - smoothstep(0.0, kr * 0.7, kd)) * 0.04, has_knot);
    // Dents and scuffs.
    let dent = blobs(uv, vec2<i32>(20), 0.05, 0.15, 605u);
    h -= select(0.0, dent.h * 4.0, u01(dent.id) < 0.25);
    // Eased edges (1.5 mm radius) down into 1.5 mm gaps.
    let gap = 0.7 + 0.25 * u01(pcg(id + 7u));
    let e = p.edge - gap;
    let ease = 1.5;
    let rounded = sqrt(saturate(1.0 - (1.0 - saturate(e / ease)) * (1.0 - saturate(e / ease))));
    h = select(0.4, mix(h - ease * 0.8, h, rounded), e > 0.0);

    var l: Layers;
    l.a = vec4<f32>(h * 0.001, late, knot, u01(id));
    l.b = vec4<f32>(pores, select(0.0, 1.0, e <= 0.0), ring, p.x / max(p.len, 1.0));
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let late = l.a.y;
    let knot = l.a.z;
    let h = pcg(u32(l.a.w * 16777216.0));
    let pores = l.b.x;
    let gap = l.b.y;

    // Oiled oak: honey earlywood, darker latewood; boards vary from pale to amber and brown.
    let tone = u01(pcg(h + 20u));
    var early = mix(hex(0xa3825fu), hex(0x7f6045u), tone);
    early = vary(early, 1.0 + 0.1 * rnd(h, 21u), 0.8 + 0.1 * rnd(h, 22u));
    let latec = early * vec3<f32>(0.55, 0.5, 0.46);
    var c = mix(early, latec, late * 0.9);
    // Flecks and streaks along the board.
    c *= 1.0 + 0.1 * fbm(uv, vec2<i32>(4, 48), 4, 0.5, 621u);
    c *= 1.0 - 0.3 * smoothstep(0.3, 0.8, pores);
    // Fine open pores: short dark dashes along the grain.
    let fine = fbm(uv + vec2<f32>(0.0, l.a.w), vec2<i32>(48, 768), 2, 0.5, 623u);
    c *= 1.0 - 0.18 * smoothstep(0.2, 0.7, fine);
    // Knots: dark, with their own tight rings.
    c = mix(c, hex(0x4b2f1cu) * (0.8 + 0.4 * fract(l.b.z * 3.0)), knot * 0.9);
    // Worn lighter where people walk, dirt in the gaps and along the edges.
    let wear = smoothstep(0.1, 0.7, fbm1(uv, 3, 4, 0.5, 622u));
    c *= 1.0 + 0.08 * wear;
    c = mix(c, hex(0x261b12u), gap * 0.85);
    c *= mix(0.75, 1.0, d.ao);
    c *= 1.0 - 0.2 * saturate(d.curv_fine * 1.5);

    var s: Pbr;
    s.albedo = c;
    s.roughness = 0.5 - 0.06 * late + 0.12 * wear + 0.1 * smoothstep(0.3, 0.8, pores) + 0.3 * gap + 0.1 * knot;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

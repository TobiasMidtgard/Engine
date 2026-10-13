// Forest soil and mud: dark humus in clods and crumbs, half-buried pebbles, fallen twigs and
// decaying leaves, with rain water standing in the low hollows (flat, glossy, dark) and wet,
// darker soil around it. 1.5 m tile, 6 cm of relief; the refine pass finds the water's edge.

const WATER_LEVEL: f32 = 0.016;

// Pebbles: in a `fraction` of the cells, one flattened, lumpy ellipse of random size, rotation
// and aspect, rounded like a worn stone. Heights in cells; `d` is 0 at the center, 1 at the rim.
struct Pebble {
    h: f32,
    id: u32,
    d: f32,
};

fn pebbles(uv: vec2<f32>, freq: i32, fraction: f32, rmin: f32, rmax: f32, seed: u32) -> Pebble {
    let period = vec2<i32>(freq);
    let p = uv * f32(freq);
    let n = vec2<i32>(floor(p));
    let f = p - floor(p);
    var best: Pebble;
    best.h = 0.0;
    best.id = 0u;
    best.d = 2.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = vec2<i32>(i, j);
            let h0 = hash_u2(wrap_cell(n + g, period), seed);
            if (u01(h0) < fraction) {
                let center = vec2<f32>(g) + vec2<f32>(0.5) + 0.3 * vec2<f32>(rnd(h0, 1u), rnd(h0, 2u)) - f;
                let r = mix(rmin, rmax, u01(pcg(h0 ^ 0x9e3779b9u)));
                let a = u01(pcg(h0 + 7u)) * TAU;
                let aspect = mix(0.5, 1.0, u01(pcg(h0 + 11u)));
                let q = vec2<f32>(dot(center, vec2<f32>(cos(a), sin(a))), dot(center, vec2<f32>(-sin(a), cos(a))) / aspect);
                let ang = atan2(q.y, q.x);
                let lump = 1.0 + 0.1 * sin(3.0 * ang + TAU * rnd(h0, 3u)) + 0.05 * sin(5.0 * ang + TAU * rnd(h0, 4u));
                let d = length(q) / (r * lump);
                if (d < 1.0) {
                    let h = pow(1.0 - d * d, 0.6) * r * (0.5 + 0.5 * aspect);
                    if (h > best.h) {
                        best.h = h;
                        best.id = pcg(h0 + 5u);
                        best.d = d;
                    }
                }
            }
        }
    }
    return best;
}

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Ground: broad hummocks and ruts, crumbly clods.
    let w = warp(uv, vec2<i32>(3), 4, 0.04, 1101u);
    let ground = 22.0 + fbm1(w, 2, 5, 0.5, 1102u) * 14.0 + fbm1(w, 7, 3, 0.5, 1103u) * 4.0;
    let clods = voronoi(uv, vec2<i32>(70), 1.0, 1104u);
    let crumb = smoothstep(0.0, 0.3, clods.edge) * 1.6 * u01(clods.id) + fbm1(uv, 256, 2, 0.5, 1105u) * 0.5;
    var h = ground + crumb;
    var kind = 0.0; // 0 soil, 1 pebble, 2 twig, 3 leaf
    var id = clods.id;

    // Pebbles, half buried: a few larger stones and a scatter of small grit.
    let peb = pebbles(warp(uv, vec2<i32>(16), 2, mm(3.0), 1106u), 24, 0.22, 0.12, 0.4, 1107u);
    let grit = pebbles(uv, 96, 0.12, 0.15, 0.4, 1116u);
    let pebble = max(peb.h, grit.h * 0.5);
    let ph = ground - 1.0 + pebble * 16.0 + fbm1(uv, 128, 2, 0.5, 1108u) * 0.5;
    var peb_d = 2.0;
    if (pebble > 0.0 && ph > h) {
        h = ph;
        kind = 1.0;
        id = select(grit.id, peb.id, peb.h >= grit.h * 0.5);
        peb_d = select(grit.d, peb.d, peb.h >= grit.h * 0.5);
    }
    // Twigs: thin round sticks lying on top.
    let tw = scratches(warp(uv, vec2<i32>(24), 2, 0.004, 1113u), 14, 0.3, 1.6, 0.03, 0.0, 3.14, 1109u);
    // Fallen conifer needles: short thin strokes everywhere, in drifts.
    let drift = smoothstep(-0.2, 0.5, fbm1(uv, 5, 3, 0.5, 1114u));
    let needles = scratches(uv, 90, 0.6 * drift, 1.6, 0.06, 0.0, 3.14, 1115u);
    let needle_h = ground + 1.5 + needles * 1.2;
    if (needles > 0.3 && needle_h > h) {
        h = needle_h;
        kind = 4.0;
    }
    let twig_h = ground + 3.0 + sqrt(tw) * 6.0;
    if (tw > 0.05 && twig_h > h) {
        h = twig_h;
        kind = 2.0;
    }
    // Leaves: pointed, lying flat with a raised midrib and curled rims, some half rotten (holes).
    let lc = voronoi(uv, vec2<i32>(22), 1.0, 1110u);
    let la = rnd(lc.id, 1u) * 3.14;
    let lp = vec2<f32>(dot(lc.to_center, vec2<f32>(cos(la), sin(la))), dot(lc.to_center, vec2<f32>(-sin(la), cos(la))));
    let len = 0.3 + 0.2 * u01(pcg(lc.id + 2u));
    let x = lp.x / len;
    let half_w = len * 0.45 * pow(saturate(1.0 - x * x), 0.75);
    let ld = select(2.0, abs(lp.y) / max(half_w, 1e-4), abs(x) < 1.0);
    let rotten = fbm1(uv, 96, 3, 0.5, 1112u) > 0.35 - 0.5 * u01(pcg(lc.id + 3u));
    let leaf = select(0.0, 1.0 - smoothstep(0.85, 1.0, ld), u01(lc.id) < 0.55 && !rotten);
    let midrib = (1.0 - smoothstep(0.0, 0.08, ld)) * 0.25;
    let leaf_h = ground + 1.0 + ld * ld * 0.7 + midrib + fbm1(uv, 64, 2, 0.5, 1111u) * 0.3;
    if (leaf > 0.5 && leaf_h > h) {
        h = leaf_h;
        kind = 3.0;
        id = lc.id;
    }
    h = clamp(h, 1.0, 59.0);
    var l: Layers;
    l.a = vec4<f32>(h * 0.001, kind, u01(id), 0.0);
    // The smooth ground (m) decides where water stands, so puddle edges follow the terrain.
    l.b = vec4<f32>(0.0, ground * 0.001, ld, peb_d);
    return l;
}

// Water fills everything below the water level; wetness spreads a few centimeters beyond.
fn preset_refine(uv: vec2<f32>) -> Layers {
    var l = layers_at(uv);
    let ground = l.b.y;
    // Water covers everything up to the water level where the ground dips below it.
    let water = smoothstep(WATER_LEVEL + 0.001, WATER_LEVEL - 0.001, ground) * step(l.a.x, WATER_LEVEL + 0.0015);
    // Wet soil around the puddles: look for water on two rings.
    var near = 0.0;
    for (var k = 0; k < 12; k++) {
        let a = f32(k) * (TAU / 12.0);
        let dir = vec2<f32>(cos(a), sin(a)) / gen.dims.x;
        near = max(near, select(0.0, 1.0, layers_at(uv + dir * 0.025).b.y < WATER_LEVEL));
        near = max(near, select(0.0, 0.5, layers_at(uv + dir * 0.06).b.y < WATER_LEVEL));
    }
    l.a.x = mix(l.a.x, max(l.a.x, WATER_LEVEL + 0.0005), water);
    l.a.w = water;
    l.b.x = max(near, water);
    return l;
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let kind = l.a.y;
    let hk = pcg(u32(l.a.z * 16777216.0));
    let water = l.a.w;
    let wet = l.b.x;

    // Humus: dark brown, crumbs a bit lighter and drier on top.
    var c = mix(hex(0x33291fu), hex(0x463a2cu), smoothstep(-0.4, 0.6, fbm1(uv, 10, 4, 0.5, 1121u)));
    c *= 1.0 + 0.25 * fbm1(uv, 200, 2, 0.5, 1122u);
    c = mix(c, c * 1.3, saturate(-d.curv_fine * 2.0));
    var rough = 0.92;
    if (kind > 0.5 && kind < 1.5) {
        // Gray, brown and a few dark or ochre stones, speckled.
        let t = u01(hk);
        c = select(select(hex(0x4c4844u), hex(0x564a3eu), t > 0.45), hex(0x34322fu), t > 0.8);
        c = select(c, hex(0x6a5a44u), t > 0.93);
        c = vary(c, 1.0 + 0.15 * rnd(hk, 1u), 0.9);
        c *= 1.0 + 0.2 * fbm1(uv, 300, 2, 0.5, 1123u);
        // Soil stuck to the stone, thickest where it meets the ground.
        let coat = 0.2 + 0.6 * smoothstep(0.45, 0.95, l.b.w) + 0.3 * smoothstep(0.1, 0.5, fbm1(uv, 64, 3, 0.5, 1126u));
        c = mix(c, hex(0x33291fu), saturate(coat));
        rough = mix(0.62, 0.9, saturate(coat));
    } else if (kind > 1.5 && kind < 2.5) {
        c = hex(0x4e3a28u) * (1.0 + 0.3 * fbm(uv, vec2<i32>(64, 8), 2, 0.5, 1124u));
        rough = 0.85;
    } else if (kind > 3.5) {
        c = hex(0x6a4c30u) * (0.8 + 0.4 * u01(pcg(u32(uv.x * 9973.0) ^ u32(uv.y * 7919.0))));
        rough = 0.7;
    } else if (kind > 2.5) {
        // Decaying leaves: brown, ochre, a few still orange.
        let t = u01(hk);
        c = select(select(hex(0x4a3a2au), hex(0x5e4a34u), t > 0.5), hex(0x6e5034u), t > 0.85);
        c *= 1.0 + 0.2 * fbm1(uv, 128, 3, 0.5, 1125u);
        // Veins and darker, rotting rims.
        c *= 1.0 - 0.25 * smoothstep(0.6, 1.0, l.b.z) - 0.15 * (1.0 - smoothstep(0.0, 0.1, l.b.z));
        rough = 0.75;
    }
    c *= mix(0.6, 1.0, d.ao);
    // Wet ground: darker and glossier; standing water: dark, mirror-smooth, flat.
    c *= mix(1.0, 0.55, wet);
    rough = mix(rough, 0.35, wet * 0.8);
    let mud = hex(0x2a2018u);
    c = mix(c, mud * 0.6, water);
    rough = mix(rough, 0.03, water);

    var s: Pbr;
    s.albedo = c;
    s.roughness = rough;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = normalize(mix(d.normal, vec3<f32>(0.0, 0.0, 1.0), water));
    return s;
}

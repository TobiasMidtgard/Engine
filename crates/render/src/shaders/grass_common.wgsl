// Grass blades: what the cull pass (grass_cull.wgsl) and the passes that draw its records
// (grass.wgsl for the scene and the depth prepass, grass_shadow.wgsl for the sun's cascades)
// share. Requires lib_types.
//
// The cull pass grows every blade once a frame from its tile's grid coordinates and its index
// (so the same blade grows in the same spot every frame and nothing per blade is kept between
// frames) and writes what a vertex needs of it in one `GrassRecord`; `grass_vertex` places a
// blade's vertices along its curve. A blade is a cubic Bezier from the root (on the field's
// height map) rising straight, arching over toward its tip and drooping under its own weight;
// a ripple runs up it in the wind. It is shaded as if rounded across its width, and widened in
// view when seen edge-on so a field stays full instead of breaking into slivers.
//
// A field can grow leaves instead of blades (`lush.w`, the leaf amount): each instance is then
// a leaf on a stalk (`tip.w` of its length), narrow at the stalk and ovate past it, folded a
// little along its midrib; a tight clump of them is a rosette (wood sorrel, clover, wild
// strawberry). Leaves can also lie on the ground (`lod.z`, how flat): fallen leaves, curling up
// a little at the tips.

struct GrassField {
    origin: vec4<f32>,  // camera-relative position of the field's -x -z corner (xyz)
    axis_x: vec4<f32>,  // field x axis (unit, world), w: size along x (m)
    axis_y: vec4<f32>,  // field up, w: size along z (m)
    axis_z: vec4<f32>,  // field z axis, w: seed head length (fraction of the stem, 0 round heads)
    grid: vec4<u32>,    // map points along x and z, unused, seed
    shape: vec4<f32>,   // blade height (m), height variation, root width (m), bend
    lush: vec4<f32>,    // fresh albedo (linear), w: leaf amount (0 blades .. 1 leaves)
    dry: vec4<f32>,     // dry albedo (linear), wind response
    lod: vec4<f32>,     // full-density distance (m), draw distance (m), lying flat (0..1), shadow distance (m)
    clump: vec4<f32>,   // clump size (m, 0 = none), pull to center, fan out, height spread
    look: vec4<f32>,    // clump color spread, droop, gloss, snow depth by the camera (m)
    tip: vec4<f32>,     // tip tint (rgb, x the blade's color), w: a leaf's stalk (fraction of its length)
    wear: vec4<f32>,    // worn-ground map: first point (field-local m, xz), points per m, 1 = present
    gust: vec4<f32>,    // steady wind (m/s, xyz), w: the wind's run (m) less speed x time
    bloom: vec4<f32>,   // flower heads' albedo (linear), w: share of blades flowering
    kind: vec4<f32>,    // flower head width (m; a flowering stem's record width is its head's), this kind's blades as a share of the field's most, pinnae pairs (or -(leaflets + stalk) for a compound leaf), leaf rise (0..1)
};
// A field growing several kinds of grass has one entry for each, one after the other: the
// same placement, distances and wind, each kind's look (`grid.z` of the first: how many). A
// blade's record names its kind's entry.


@group(1) @binding(1) var<storage, read> grass_fields: array<GrassField>;

struct Blade {
    rel: vec3<f32>,     // camera-relative position of this vertex
    normal: vec3<f32>,  // front-side normal (world, unit)
    along: f32,         // 0 at the root, 1 at the tip
    across: f32,        // -1 .. 1 from edge to edge
    albedo: vec3<f32>,
    gloss: f32,         // 0 matte .. 1 glossy along the blade's middle
    shade: f32,         // how much light reaches this point under the meadow (0..1)
    glow: f32,          // how much sun shines through here from behind (0..1)
    visible: bool,
};


// What a vertex needs of its blade (a `BladeSeed` with the tip directions worked out). The
// compute pass (grass_cull.wgsl) packs it into a `GrassRecord` once per blade a frame.
struct BladeShape {
    field: u32,
    segments: u32,
    root: vec3<f32>,
    face2: vec2<f32>,
    height: f32,
    width: f32,
    tip: vec3<f32>,      // tip direction now
    tip_prev: vec3<f32>, // and at the previous frame (motion vectors)
    ripple: f32,
    rise: f32,
    snowed: f32,
    albedo: vec3<f32>,
    clump: vec2<f32>,
    flower: f32,         // 1: a flowering stem with a head at its top
};

// One blade packed in 64 bytes: the root in full (camera-relative, so precise near the
// camera), the rest in halves.
struct GrassRecord {
    root: vec4<u32>,  // root xyz (f32 bits), field
    tips: vec4<u32>,  // tip xy, tip z + prev x, prev yz, facing
    look: vec4<u32>,  // height + width, albedo rg, albedo b + snowed, clump (unorm)
    more: vec4<u32>,  // ripple + rise (unorm), segments, flowering (1), unused
};

fn grass_pack(s: BladeShape) -> GrassRecord {
    var r: GrassRecord;
    r.root = vec4<u32>(bitcast<vec3<u32>>(s.root), s.field);
    r.tips = vec4<u32>(
        pack2x16float(s.tip.xy),
        pack2x16float(vec2<f32>(s.tip.z, s.tip_prev.x)),
        pack2x16float(s.tip_prev.yz),
        pack2x16float(s.face2),
    );
    r.look = vec4<u32>(
        pack2x16float(vec2<f32>(s.height, s.width)),
        pack2x16float(s.albedo.rg),
        pack2x16float(vec2<f32>(s.albedo.b, s.snowed)),
        pack2x16unorm(s.clump),
    );
    r.more = vec4<u32>(pack2x16unorm(vec2<f32>(s.ripple, s.rise)), s.segments, u32(s.flower > 0.5), 0u);
    return r;
}

fn grass_unpack(r: GrassRecord) -> BladeShape {
    var s: BladeShape;
    s.root = bitcast<vec3<f32>>(r.root.xyz);
    s.field = r.root.w;
    let t0 = unpack2x16float(r.tips.x);
    let t1 = unpack2x16float(r.tips.y);
    let t2 = unpack2x16float(r.tips.z);
    s.tip = vec3<f32>(t0, t1.x);
    s.tip_prev = vec3<f32>(t1.y, t2);
    s.face2 = unpack2x16float(r.tips.w);
    let hw = unpack2x16float(r.look.x);
    s.height = hw.x;
    s.width = hw.y;
    let rg = unpack2x16float(r.look.y);
    let bs = unpack2x16float(r.look.z);
    s.albedo = vec3<f32>(rg, bs.x);
    s.snowed = bs.y;
    s.clump = unpack2x16unorm(r.look.w);
    let rr = unpack2x16unorm(r.more.x);
    s.ripple = rr.x;
    s.rise = rr.y;
    s.segments = r.more.y;
    s.flower = f32(r.more.z);
    return s;
}


// Vertex `vertex` of a blade at time `t`, its tip along `tip_dir` at that time. Two vertices a
// row, then the tip.
fn grass_vertex(s: BladeShape, vertex: u32, t: f32, tip_dir: vec3<f32>) -> Blade {
    var out: Blade;
    let f = grass_fields[s.field];
    let up = f.axis_y.xyz;
    let facing = f.axis_x.xyz * s.face2.x + f.axis_z.xyz * s.face2.y;
    let side = cross(up, facing);
    let root = s.root;
    let height = s.height;
    let d = length(root);
    // A herb's flowering stem rises straight to its head, not spread like its leaves.
    let leaf = select(f.lush.w, 0.0, s.flower > 0.5);
    let lying = f.lod.z;
    // Cubic Bezier: rising straight from the root, arching over toward the tip, which droops
    // under its weight the further the blade leans.
    let over = tip_dir - up * dot(tip_dir, up);
    let leaning = length(over);
    var p0 = root;
    var p1 = root + up * (height * 0.38);
    var p2 = root + up * (height * 0.74) + over * (height * 0.5);
    // (A seed head nods a little, not over: its stem is stiffer than a blade.)
    let droop = f.look.y * select(1.0, 0.35, s.flower > 0.5 && f.axis_z.w > 0.0);
    var p3 = root + tip_dir * (height * (1.0 - 0.12 * leaning)) - up * (height * droop * leaning * 0.8);
    if (leaf > 0.0 && lying < 1.0) {
        // A leaf on a stalk: the stalk rises, the blade spreads out flat from its top (leaning
        // with the wind and away from benders) and droops a little at the tip.
        let spread = normalize(facing + over * 0.8);
        let upright = f.kind.w;
        let rise = height * mix(0.25 + 0.25 * s.rise, 0.5 + 0.25 * s.rise, upright);
        let reach = height * (0.75 - 0.2 * upright);
        p1 = mix(p1, root + up * rise, leaf);
        p2 = mix(p2, root + up * (rise * 1.15) + spread * (reach * 0.45), leaf);
        p3 = mix(p3, root + up * (rise * (1.0 - 0.6 * f.look.y)) + spread * reach, leaf);
    }
    if (lying > 0.0) {
        // Lying on the ground along its own direction, just above it, some curling up at the
        // tip and a few cupped up along their length.
        let base = root + up * 0.004;
        // Stiff things (twigs: `bend` near 1) lie straight; leaves curl up.
        let curl = height * (0.04 + 0.3 * s.rise * s.rise) * (1.0 - min(f.shape.w, 1.0));
        p0 = mix(p0, base, lying);
        p1 = mix(p1, base + facing * (height * 0.33) + up * (curl * 0.35), lying);
        p2 = mix(p2, base + facing * (height * 0.66) + up * (curl * 0.6), lying);
        p3 = mix(p3, base + facing * (height * 0.97) + up * curl, lying);
    }

    // Vertex: two per row, then the tip; rows closer together toward the tip, where it bends.
    let segments = max(s.segments, 1u);
    var sgn = f32(vertex & 1u) * 2.0 - 1.0;
    let flower = s.flower > 0.5;
    let row = f32(vertex / 2u);
    var u = row / f32(segments);
    // A flowering stem's rows run evenly up its stalk to just under the head, the last one
    // across the head (its widest part), so the head stays a short round dot, not a spike.
    let last = f32(segments - 1u);
    let flower_u = select(0.9 * row / max(last - 1.0, 1.0), 0.95, row >= last && segments > 1u);
    u = select(1.0 - pow(1.0 - u, 1.35), flower_u, flower);
    // A grass's seed head (`bloom_length`): a spike or plume the top part of the stem long.
    // The root row, one up the stem (given three rows or more, so the stem bends smoothly
    // into its head), then every other row inside the head, so even two segments show it.
    let head_length = f.axis_z.w;
    let spike = flower && head_length > 0.0;
    if (spike) {
        let first = select(1.0, 2.0, last >= 2.0);
        let a = (row - first + 1.0) / (last - first + 2.0);
        u = 1.0 - head_length * (1.0 - a);
        u = select(u, 0.6 * (1.0 - head_length), row > 0.5 && row < first - 0.5);
        u = select(u, 0.0, row < 0.5);
    }
    if (vertex >= 2u * segments) {
        u = 1.0;
        sgn = 0.0;
    }
    let q = 1.0 - u;
    var pos = q * q * q * p0 + 3.0 * q * q * u * p1 + 3.0 * q * u * u * p2 + u * u * u * p3;
    let tangent = normalize(3.0 * q * q * (p1 - p0) + 6.0 * q * u * (p2 - p1) + 3.0 * u * u * (p3 - p2) + up * 1e-4);
    // A ripple runs up the blade, strongest at the tip (barely at all in still air).
    let wind = f.gust.xyz;
    let speed = length(wind);
    let run = f.gust.w + speed * t;
    let wind_dir = select(facing, wind / max(speed, 1e-3), speed > 0.01);
    let ripple = sin(t * 2.6 + run * 0.2 + s.ripple * TAU - u * 2.2) * u * u;
    pos += wind_dir * (ripple * height * (0.003 + 0.06 * min(speed / 8.0, 1.5)) * f.dry.w);
    // A leaf: a thin stalk, then an ovate blade, widest a little before its middle.
    let stalk = f.tip.w;
    let x = saturate((u - stalk) / max(1.0 - stalk, 0.05));
    // A compound leaf's blade is drawn whole, its leaflets cut out of it (`grass_cut`).
    let whole = select(max(pow(sin(PI * pow(x, 0.8)), 0.7), 0.1), 1.0, f.kind.z < 0.0);
    let leaf_shape = select(select(0.1, 0.04, f.kind.z < 0.0), whole, u > stalk);
    // A blade tapers to its tip; a stick lying on the ground (no leaf) only a little.
    let taper = mix(pow(q, 0.6), 1.0 - 0.55 * u, lying * (1.0 - leaf));
    var half_width = 0.5 * s.width * mix(taper, leaf_shape, leaf);
    // A flowering stem: thin, then its head (a disc seen edge-on is still a few petals wide).
    var head = smoothstep(0.91, 0.94, u);
    if (spike) {
        // Spindle-shaped, widest a third of the way up, tapering to the tip.
        let a = saturate((u - (1.0 - head_length)) / head_length);
        head = smoothstep(0.0, 0.12, a);
        half_width = 0.5 * s.width * max(pow(sin(PI * pow(a, 0.75)), 0.7), 0.06);
    } else if (flower) {
        half_width = mix(0.05 * s.width, 0.5 * s.width, head);
    }
    // Seen edge-on a blade is widened across the view, so the field keeps its cover (from a
    // few meters out: close up it would read as a broad ribbon). Leaves aren't: they lie open.
    let v = normalize(-pos);
    let face_n = normalize(cross(side, tangent));
    let edge = 1.0 - abs(dot(face_n, v));
    var across_view = cross(tangent, v);
    across_view = select(side, normalize(across_view), dot(across_view, across_view) > 1e-8);
    across_view *= sign(dot(across_view, side) + 1e-6);
    let widen = 1.2 * smoothstep(1.5, 6.0, d) * (1.0 - leaf);
    out.rel = pos + side * (sgn * half_width) + across_view * (sgn * half_width * edge * edge * widen);
    // Rounded across the blade (a leaf only folds a little along its midrib); far away, shaded
    // like its clump (one rounded mound), on the leaf's own side.
    var n = normalize(face_n + side * (sgn * mix(0.45, 0.2, leaf)));
    var clump_n = normalize(up + (f.axis_x.xyz * (s.clump.x - 0.5) + f.axis_z.xyz * (s.clump.y - 0.5)) * 0.5);
    clump_n = select(clump_n, -clump_n, leaf > 0.5 && dot(n, clump_n) < 0.0);
    n = normalize(mix(n, clump_n, smoothstep(0.15, 1.0, d / f.lod.y) * 0.7));
    out.normal = n;
    out.along = u;
    out.across = sgn;
    // Lower parts of a meadow sit in the shade of the blades around them; a rosette's leaves
    // less so, and fallen leaves only a little. Sun shines through toward the tips, but not
    // through a leaf lying on the ground.
    out.shade = mix(mix(0.3, 1.0, sqrt(u)), mix(0.55, 1.0, u), leaf);
    out.shade = mix(out.shade, 0.85 + 0.15 * u, lying);
    // Wind sheen, as in a striped lawn: a blade laid away from the eye shows its lit upper
    // face, one laid toward it its shaded underside and the dark gaps at its foot. So where a
    // gust front lays the grass over, the meadow pales (looking downwind) or darkens (upwind)
    // in a band that rolls across it (the honami of a wheat field, which Py et al. 2006
    // tracked by exactly this change of brightness), and the turned-up faces catch the sky's
    // sheen far across the field.
    let to_blade = root - up * dot(root, up);
    let away = dot(over, to_blade / max(length(to_blade), 1e-3));
    let blade_kind = (1.0 - lying) * (1.0 - leaf);
    let laid = smoothstep(0.15, 0.8, away) * blade_kind;
    let toward = smoothstep(0.15, 0.8, -away) * blade_kind;
    out.shade = mix(out.shade, 0.85 + 0.15 * u, laid * 0.85) * (1.0 - 0.3 * toward);
    out.glow = u * u * (1.0 - lying);

    // Color: tips tinted, snow caught on the blades (most on their arched upper parts).
    var albedo = mix(s.albedo, s.albedo * f.tip.rgb, u * u);
    if (flower) {
        albedo = mix(albedo, f.bloom.rgb, head);
    }
    albedo = mix(albedo, vec3<f32>(0.82, 0.85, 0.88), s.snowed * (0.25 + 0.45 * u));
    // Darker low down, as the shade (`out.shade`) is darker there.
    let low = mix(mix(mix(0.7, 1.0, u), mix(0.8, 1.0, u), leaf), 0.95 + 0.05 * u, lying);
    out.albedo = albedo * mix(low, 0.95 + 0.1 * u, laid * 0.85) * (1.0 - 0.2 * toward);
    // Glossy up close; far off the sheen would only sparkle.
    out.gloss = f.look.z * (1.0 - smoothstep(8.0, 35.0, d) * (1.0 - 0.7 * laid));
    out.visible = true;
    return out;
}


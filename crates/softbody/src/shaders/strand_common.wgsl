// Strand expansion shared by the hair color and shadow passes. Requires lib_types.
//
// Every drawn strand is generated in the vertex shader from a simulated guide strand: the
// guide's particles (camera-relative) are interpolated with Catmull-Rom, and each strand gets
// its own offset around the guide (spread at the root, gathered toward the tip by clumping),
// a curl, a length and a brightness, all from a hash of its index. One instance = one strand;
// vertex 2k and 2k+1 are the two sides of segment point k.

struct StrandGuide {
    first: u32,
    count: u32,
    groom: u32,
    seed: u32,
    normal: vec4<f32>,   // root normal (world), w = guide length (m)
    tangent: vec4<f32>,  // root tangent (world)
};

struct Groom {
    color: vec4<f32>,    // linear rgb albedo, root thickness (m)
    shape: vec4<f32>,    // spread (m), clumping, curl radius (m), curl turns per meter
    counts: vec4<u32>,   // first guide, strands per guide, first strand, render segments
    extra: vec4<f32>,    // length variation, root brightness (self-shadowing), unused, unused
};

@group(1) @binding(0) var<storage, read> strand_points: array<vec4<f32>>;
@group(1) @binding(1) var<storage, read> strand_guides: array<StrandGuide>;
@group(1) @binding(2) var<storage, read> grooms: array<Groom>;

struct StrandPoint {
    pos: vec3<f32>,
    tangent: vec3<f32>,
    t: f32,
    width: f32,
    shade: f32,
    // Random per drawn strand, 0..1.
    seed: f32,
    groom: u32,
};

// Normalize, or `fallback` for a (near-)zero vector. Interpolated directions can cancel out,
// and one NaN pixel turns into a white bloom spot.
fn safe_normalize(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    return select(fallback, v * inverseSqrt(l2), l2 > 1e-20);
}

fn hash_u(x: u32) -> f32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h) / 4294967295.0;
}

fn guide_eval(g: StrandGuide, f: f32) -> mat2x3<f32> {
    let last = i32(g.count) - 1;
    let i = clamp(i32(floor(f)), 0, max(last - 1, 0));
    let u = clamp(f - f32(i), 0.0, 1.0);
    let base = i32(g.first);
    let p0 = strand_points[u32(base + max(i - 1, 0))].xyz;
    let p1 = strand_points[u32(base + i)].xyz;
    let p2 = strand_points[u32(base + min(i + 1, last))].xyz;
    let p3 = strand_points[u32(base + min(i + 2, last))].xyz;
    let a = 2.0 * p1;
    let b = p2 - p0;
    let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let pos = 0.5 * (a + b * u + c * u * u + d * u * u * u);
    let tan = 0.5 * (b + 2.0 * c * u + 3.0 * d * u * u);
    return mat2x3<f32>(pos, tan);
}

fn find_groom(instance: u32) -> u32 {
    var k = 0u;
    for (var i = 1u; i < arrayLength(&grooms); i++) {
        if (instance >= grooms[i].counts.z) { k = i; }
    }
    return k;
}

fn strand_point(instance: u32, vertex: u32) -> StrandPoint {
    let gi = find_groom(instance);
    let groom = grooms[gi];
    let local = instance - groom.counts.z;
    let spg = max(groom.counts.y, 1u);
    let guide = strand_guides[groom.counts.x + local / spg];
    let child = local % spg;
    let seed = guide.seed * 7919u + child * 104729u;
    let h1 = hash_u(seed);
    let h2 = hash_u(seed + 1u);
    let h3 = hash_u(seed + 2u);
    let h4 = hash_u(seed + 3u);
    let segs = max(groom.counts.w, 1u);
    let k = min(vertex / 2u, segs);
    let t = f32(k) / f32(segs);
    let len_factor = 1.0 - groom.extra.x * h3;
    let f = t * len_factor * f32(guide.count - 1u);
    let e = guide_eval(guide, f);
    let tan = safe_normalize(e[1], guide.normal.xyz);
    // Offset around the guide: a disc at the root, transported along the strand.
    let n0 = guide.normal.xyz;
    let t0 = guide.tangent.xyz;
    let b0 = cross(n0, t0);
    let r = groom.shape.x * sqrt(h1) * select(1.0, 0.0, child == 0u);
    let theta = 6.2831853 * h2;
    let off0 = (t0 * cos(theta) + b0 * sin(theta)) * r;
    var off = off0 - tan * dot(off0, tan);
    off *= mix(1.0, 1.0 - groom.shape.y, pow(t, 0.8));
    // Curl around the strand.
    let curl = groom.shape.z;
    if (curl > 0.0) {
        var u = off - tan * dot(off, tan);
        if (dot(u, u) < 1e-12) { u = cross(tan, n0); }
        u = normalize(u + vec3(1e-7));
        let v = cross(tan, u);
        let phi = 6.2831853 * (groom.shape.w * t * guide.normal.w * len_factor + h4);
        off += (u * cos(phi) + v * sin(phi)) * curl * smoothstep(0.0, 0.2, t);
    }
    var p: StrandPoint;
    p.pos = e[0] + off;
    p.tangent = tan;
    p.t = t;
    p.width = groom.color.w * mix(1.0, 0.2, t);
    p.shade = 0.8 + 0.4 * h4;
    p.seed = hash_u(seed + 4u);
    p.groom = gi;
    return p;
}

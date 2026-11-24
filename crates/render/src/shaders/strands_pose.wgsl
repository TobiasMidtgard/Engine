// Strands, first pass (strands.rs): every drawn point posed by its guides, and its fibre's
// projected area added to its instance's density volume, from which the shading reads how
// much hair lies between a point and a light (as Unreal's grooms voxelize their strands for
// self-shadowing, and AMD's TressFX its fur). A groom far away draws few strands, each many
// times wider: too few to fill the volume evenly, so more of its strands are posed here than
// are drawn (`more.z`), only to add their area. Requires lib_types + strands_common.

@group(0) @binding(0) var<uniform> params: StrandParams;
@group(0) @binding(1) var<storage, read> instances: array<StrandInst>;
@group(0) @binding(2) var<storage, read> rest: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> info: array<StrandInfo>;
@group(0) @binding(4) var<storage, read> guide_offsets: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> posed: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> voxels: array<atomic<u32>>;

const NONE: u32 = 0xffffffffu;
// A followed guide's number (strand_feature.rs, `follow_bits`).
const FOLLOW_GUIDE_BITS: u32 = 20u;
const FOLLOW_GUIDE_MASK: u32 = 0xfffffu;

// A drawn point is kept this far outside the shapes hair is kept out of, m.
const KEPT_OUT: f32 = 0.0018;

// The way out of a tapered capsule (spheres of radius ra at a and rb at b and the cone between
// them) and the signed distance to it, after the hair solver this renderer's guides come from
// (softbody, tress.rs; I. Quilez's round cone).
fn capsule(p: vec3<f32>, a: vec3<f32>, ra: f32, b: vec3<f32>, rb: f32) -> vec4<f32> {
    let ba = b - a;
    let l2 = dot(ba, ba);
    let rr = ra - rb;
    let a2 = l2 - rr * rr;
    if (a2 <= 1e-8) {
        // One sphere holds the other.
        let c = select(b, a, ra >= rb);
        let d = p - c;
        let len = length(d);
        return vec4<f32>(select(vec3<f32>(0.0, 1.0, 0.0), d / len, len > 1e-6), len - max(ra, rb));
    }
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = dot(pa, ba);
    let z = y - l2;
    let x = pa * l2 - ba * y;
    let x2 = dot(x, x);
    let k = sign(rr) * rr * rr * x2;
    let along = ba * sqrt(il2);
    if (sign(z) * a2 * z * z * l2 > k) {
        let d = p - b;
        let len = length(d);
        return vec4<f32>(select(along, d / len, len > 1e-6), len - rb);
    }
    if (sign(y) * a2 * y * y * l2 < k) {
        let len = length(pa);
        return vec4<f32>(select(-along, pa / len, len > 1e-6), len - ra);
    }
    var across = vec3<f32>(1.0, 0.0, 0.0);
    if (x2 > 1e-10) {
        across = x / sqrt(x2);
    } else if (abs(along.z) < 0.9) {
        across = normalize(cross(along, vec3<f32>(0.0, 0.0, 1.0)));
    }
    let n = normalize(across * sqrt(a2 * il2) + along * (rr * sqrt(il2)));
    return vec4<f32>(n, (sqrt(x2 * a2 * il2) + y * rr) * il2 - ra);
}

@compute @workgroup_size(256)
fn cs_pose(@builtin(global_invocation_id) id: vec3<u32>) {
    let inst = instances[id.y];
    let per = inst.first.z;
    let i = id.x;
    if (i >= inst.more.z * per) {
        return;
    }
    let s = i / per;
    let j = i - s * per;
    let at = inst.first.x + i;
    var p = rest[at];
    if (inst.out.y != NONE) {
        let f = info[inst.first.y + s].follow;
        let w01 = unpack2x16unorm(f.w);
        let w = vec3<f32>(w01, max(1.0 - w01.x - w01.y, 0.0));
        var off = vec3<f32>(0.0);
        for (var g = 0u; g < 3u; g++) {
            if (f[g] != NONE && w[g] > 0.0) {
                // The guide's motion as far along it as this point is along its strand,
                // length for length (the high bits: how far along the guide the strand
                // reaches): a short hair moves as the start of its long guide does.
                let reach = f32(f[g] >> FOLLOW_GUIDE_BITS) / 4094.0;
                let along = f32(j) * reach;
                let k = min(u32(along), per - 2u);
                let base = inst.out.y + (f[g] & FOLLOW_GUIDE_MASK) * per + k;
                off += w[g] * mix(guide_offsets[base].xyz, guide_offsets[base + 1u].xyz,
                    along - f32(k));
            }
        }
        var q = p.xyz + off;
        // Kept out of the shapes the guides are kept out of (they lie ahead of the
        // instance's guide offsets, their number last): between guides that part around a
        // head or a neck the blend alone cuts through it. A point that lies in a shape at
        // rest (the shapes are rounder than the scalp) may lie as deep.
        if (j > 0u) {
            let shapes = u32(guide_offsets[inst.out.y - 1u].x);
            let first = inst.out.y - 1u - 2u * shapes;
            for (var c = 0u; c < shapes; c++) {
                let a = guide_offsets[first + 2u * c];
                let b = guide_offsets[first + 2u * c + 1u];
                let now = capsule(q, a.xyz, a.w, b.xyz, b.w);
                if (now.w < KEPT_OUT) {
                    let keep = min(KEPT_OUT, capsule(p.xyz, a.xyz, a.w, b.xyz, b.w).w);
                    if (now.w < keep) {
                        q += now.xyz * (keep - now.w);
                    }
                }
            }
        }
        p = vec4<f32>(q, p.w);
    }
    // (Only a drawn strand has slots.)
    if (s < inst.first.w) {
        posed[inst.out.x + i] = p;
    }

    // The fibre from this point to the next, spread over the cells it passes (a long hair's
    // segments are longer than a cell).
    if (j + 1u >= per) {
        return;
    }
    let next = rest[at + 1u];
    let len = distance(next.xyz, rest[at].xyz);
    // The groom's whole cover (the drawn strands' widths add up to it), shared among the
    // strands that fill the volume.
    let drawn = f32(inst.first.w - 1u) + bitcast<f32>(inst.out.z);
    let area = 2.0 * p.w * inst.tint.w * drawn / f32(inst.more.z) * len;
    let steps = u32(clamp(ceil(len / inst.grid.w), 1.0, 8.0));
    let d = i32(params.misc.w);
    // The next point's pose: its guides move it as they move this one, near enough.
    let ahead = next.xyz - rest[at].xyz;
    let share = u32(area / f32(steps) * STRAND_AREA_UNIT + 0.5);
    for (var k = 0u; k < steps; k++) {
        let q = p.xyz + ahead * ((f32(k) + 0.5) / f32(steps));
        let c = vec3<i32>(floor((q - inst.grid.xyz) / inst.grid.w));
        if (all(c >= vec3<i32>(0)) && all(c < vec3<i32>(d))) {
            let cell = u32(c.x + d * (c.y + d * c.z));
            atomicAdd(&voxels[inst.out.w + cell], share);
        }
    }
}

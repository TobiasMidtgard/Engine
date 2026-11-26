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
// Measured bodies' samples: the signed distance (low 16 bits, BODY_UNIT m each) and the box's
// weight there (the next 8).
@group(0) @binding(7) var<storage, read> bodies: array<u32>;

const NONE: u32 = 0xffffffffu;
// A followed guide's number (strand_feature.rs, `follow_bits`).
const FOLLOW_GUIDE_BITS: u32 = 20u;
const FOLLOW_GUIDE_MASK: u32 = 0xfffffu;

// A drawn point is kept this far outside the shapes hair is kept out of, m.
const KEPT_OUT: f32 = 0.0018;
// How far a soaked hair gathers to its lock's guide (1: onto it), and the share of the way it
// keeps for the rope's thickness.
const WET_GATHERS: f32 = 0.9;
const WET_ROPE: f32 = 0.12;

// Meters to a unit of a measured body's distances (engine_world, `HairBody::UNIT`), and the
// entries a box takes ahead of an instance's guide offsets (strand_feature.rs).
const BODY_UNIT: f32 = 1e-5;
const BOX_ENTRIES: u32 = 6u;
const MAX_BOXES: u32 = 12u;
// (softbody, tress.rs, `PARTS_WITHIN`.)
const PARTS_WITHIN: f32 = 0.012;
// No body near.
const FAR: f32 = 1e9;

fn body_cell(i: u32) -> vec2<f32> {
    let c = bodies[i];
    return vec2<f32>(f32(i32(c << 16u) >> 16u), f32((c >> 16u) & 0xffu));
}

// The way out of the measured body whose `boxes` boxes start at entry `first`, and the signed
// distance to it, at `q` (the instance's model space). Each box holds the distance to the
// skin its own bone carries and is sampled where its matrix carries `q` (softbody, tress.rs,
// `felt`). (0, FAR) outside every box.
fn body(first: u32, boxes: u32, q: vec3<f32>) -> vec4<f32> {
    var ds: array<f32, MAX_BOXES>;
    var gs: array<vec3<f32>, MAX_BOXES>;
    var ws: array<f32, MAX_BOXES>;
    var n = 0u;
    var nearest = FAR;
    for (var k = 0u; k < min(boxes, MAX_BOXES); k++) {
        let o = first + BOX_ENTRIES * k;
        let r0 = guide_offsets[o];
        let r1 = guide_offsets[o + 1u];
        let r2 = guide_offsets[o + 2u];
        let lattice = guide_offsets[o + 3u];
        let local = vec3<f32>(dot(r0.xyz, q) + r0.w, dot(r1.xyz, q) + r1.w, dot(r2.xyz, q) + r2.w);
        let c = (local - lattice.xyz) * lattice.w;
        let size = vec3<u32>(guide_offsets[o + 4u].xyz);
        if (any(c < vec3<f32>(0.0)) || any(c >= vec3<f32>(size) - 1.0)) {
            continue;
        }
        let at = guide_offsets[o + 5u];
        let cell = vec3<u32>(c);
        let f = c - vec3<f32>(cell);
        let row = size.x * size.y;
        let i = u32(at.x) + (u32(at.y) << 16u) + cell.x + size.x * cell.y + row * cell.z;
        let c000 = body_cell(i);
        let c100 = body_cell(i + 1u);
        let c010 = body_cell(i + size.x);
        let c110 = body_cell(i + size.x + 1u);
        let c001 = body_cell(i + row);
        let c101 = body_cell(i + row + 1u);
        let c011 = body_cell(i + row + size.x);
        let c111 = body_cell(i + row + size.x + 1u);
        let x00 = mix(c000, c100, f.x);
        let x10 = mix(c010, c110, f.x);
        let x01 = mix(c001, c101, f.x);
        let x11 = mix(c011, c111, f.x);
        let y0 = mix(x00, x10, f.y);
        let y1 = mix(x01, x11, f.y);
        let here = mix(y0, y1, f.z);
        let grad = vec3<f32>(
            mix(mix(c100.x - c000.x, c110.x - c010.x, f.y), mix(c101.x - c001.x, c111.x - c011.x, f.y), f.z),
            mix(x10.x - x00.x, x11.x - x01.x, f.z),
            y1.x - y0.x);
        // (The gradient back to the instance's space: by the matrix's rows.)
        gs[n] = (r0.xyz * grad.x + r1.xyz * grad.y + r2.xyz * grad.z) * (BODY_UNIT * lattice.w);
        ds[n] = here.x * BODY_UNIT;
        ws[n] = max(here.y / 255.0, 1e-3);
        nearest = min(nearest, abs(ds[n]));
        n += 1u;
    }
    if (n == 0u) {
        return vec4<f32>(0.0, 0.0, 0.0, FAR);
    }
    // The nearest box's skin is the body; boxes whose skin is about as near (a joint's, which
    // two bones share) count with it by their weights.
    var sum = 0.0;
    var way = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < n; k++) {
        let u = ws[k] * max(1.0 - (abs(ds[k]) - nearest) / PARTS_WITHIN, 0.0);
        sum += ds[k] * u;
        way += gs[k] * u;
        total += u;
    }
    let len = length(way);
    return vec4<f32>(select(vec3<f32>(0.0, 1.0, 0.0), way / len, len > 1e-9), sum / total);
}

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
        let head = guide_offsets[inst.out.y - 1u];
        // The guides' state follows their offsets where any is wet or cut: the guides at
        // rest, and in the last entries of each one's first points how wet it is, the share
        // of its length a cut left, and how far gone the cut-off piece is.
        let stated = u32(head.y);
        // The guide the strand follows most: the lock it belongs to.
        var lock = 0u;
        if (w.y > w.x && f.y != NONE) { lock = 1u; }
        if (w.z > max(w.x, w.y) && f.z != NONE) { lock = 2u; }
        var wet = 0.0;
        var cut_at = FAR;
        var shed = 1.0;
        if (stated > 0u && f[lock] != NONE) {
            let state = inst.out.y + stated + (f[lock] & FOLLOW_GUIDE_MASK) * per;
            wet = guide_offsets[state].w;
            let cut = guide_offsets[state + 1u].w;
            if (cut < 1.0) {
                // (In the strand's own points: its guide's share, length for length.)
                cut_at = cut * f32(per - 1u) / max(f32(f[lock] >> FOLLOW_GUIDE_BITS) / 4094.0, 1e-3);
                shed = guide_offsets[state + 2u].w;
            }
        }
        // Cut: the first point past the cut is the hair's new end, at the cut itself; the
        // points after it are the piece that came off, drawn (but for a gap of one point,
        // which parts it from the hair) where the lock's guide falls until it is gone.
        let fj = f32(j);
        let end = fj >= cut_at && fj < cut_at + 1.0;
        let off_cut = fj >= cut_at + 1.0;
        let at_j = select(fj, cut_at, end);
        var off = vec3<f32>(0.0);
        var lock_off = vec3<f32>(0.0);
        var lock_rest = p.xyz;
        for (var g = 0u; g < 3u; g++) {
            if (f[g] != NONE && w[g] > 0.0) {
                // The guide's motion as far along it as this point is along its strand,
                // length for length (the high bits: how far along the guide the strand
                // reaches): a short hair moves as the start of its long guide does.
                let reach = f32(f[g] >> FOLLOW_GUIDE_BITS) / 4094.0;
                let first_point = inst.out.y + (f[g] & FOLLOW_GUIDE_MASK) * per;
                var along = at_j * reach;
                // (A guide cut shorter than this hair moves it as its own end does: past it
                // is a piece falling. Its last whole point is that end and its next the
                // piece's first: between them there is nothing to move by, a hair moved by
                // both would be drawn out from the one to the other.)
                if (stated > 0u) {
                    let its_cut = guide_offsets[first_point + stated + 1u].w;
                    if (its_cut < 1.0) {
                        let last = floor(its_cut * f32(per - 1u));
                        if (!off_cut) {
                            along = min(along, last);
                        } else if (g == lock) {
                            along = max(along, last + 1.0);
                        }
                    }
                }
                let k = min(u32(along), per - 2u);
                let base = first_point + k;
                let its = mix(guide_offsets[base].xyz, guide_offsets[base + 1u].xyz,
                    along - f32(k));
                off += w[g] * its;
                if (g == lock) {
                    lock_off = its;
                    if (stated > 0u) {
                        lock_rest = mix(guide_offsets[base + stated].xyz,
                            guide_offsets[base + stated + 1u].xyz, along - f32(k));
                    }
                }
            }
        }
        if (end) {
            // (The hair's own shape up to the cut.)
            let k = min(u32(cut_at), per - 2u);
            let here = inst.first.x + s * per + k;
            p = vec4<f32>(mix(rest[here].xyz, rest[here + 1u].xyz, cut_at - f32(k)), p.w);
        }
        // Wet hair gathers into ropes: each hair to its lock's guide, the more the farther
        // from the root, and moves as that guide alone does. So does a piece cut off.
        let gather = select(wet * WET_GATHERS * smoothstep(0.0, 0.3, fj / f32(per - 1u)), 0.0, stated == 0u);
        let own = select(gather, 1.0, off_cut);
        off = mix(off, lock_off, own);
        p = vec4<f32>(mix(p.xyz, lock_rest, gather * (1.0 - WET_ROPE * (0.4 + 1.2 * fract(f32(s) * 0.754877)))), p.w);
        // The width carries how wet the point is to the shading (`strand_wet`), in the part
        // of it below a tenth of a micrometer. Below zero: not drawn, nor the segments at it.
        // (A piece cut off thins away before it is gone.)
        let thin = select(1.0, 1.0 - smoothstep(0.6, 1.0, shed), off_cut);
        var width = (floor(p.w * thin * STRAND_WET_STEPS) + 0.01 + 0.97 * wet) / STRAND_WET_STEPS;
        if (off_cut && (fj < cut_at + 2.0 || shed >= 1.0)) {
            width = -1.0;
        }
        p.w = width;
        var q = p.xyz + off;
        // Kept out of the shapes the guides are kept out of (they lie ahead of the
        // instance's guide offsets, their number last): between guides that part around a
        // head or a neck the blend alone cuts through it. A point that lies in a shape at
        // rest (the shapes are rounder than the scalp) may lie as deep.
        if (j > 0u) {
            let shapes = u32(head.x);
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
            // And out of the measured body (its boxes lie ahead of the shapes, their number
            // last): skin and clothes as they are drawn, so every hair lies on them, each
            // strand at its own height in the layer of hair lying there, the first point
            // after the root at half of it.
            let boxes = u32(guide_offsets[first - 1u].x);
            if (boxes > 0u) {
                let layer = 0.6 + 1.4 * fract(f32(s) * 0.618034);
                let keep = KEPT_OUT * layer * min(f32(j) * 0.5, 1.0);
                let first_box = first - 1u - BOX_ENTRIES * boxes;
                for (var again = 0u; again < 2u; again++) {
                    let now = body(first_box, boxes, q);
                    if (now.w >= keep) {
                        break;
                    }
                    q += now.xyz * (keep - now.w);
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
    let area = 2.0 * max(p.w, 0.0) * inst.tint.w * drawn / f32(inst.more.z) * len;
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

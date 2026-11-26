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
// (softbody, tress.rs: `READS_FROM`, `REFINED`, `CARRIES_ALL`, `SAME`.)
const READS_FROM: f32 = 0.25;
const REFINED: u32 = 2u;
const CARRIES_ALL: f32 = 0.98;
const SAME: f32 = 0.0005;
// How far a hair's point is moved, m, before the body's boxes are asked how they see it again.
const VIEWED_ANEW: f32 = 0.004;
// No body near.
const FAR: f32 = 1e9;
// How far a guide has come from a hair's lock, m, where it starts to count less for the hair
// and where it no longer counts.
const PARTED: vec2<f32> = vec2<f32>(0.025, 0.07);
// How far off the body a lock's guide is, m, where the lock's hairs start to be laid on the
// body less, and where they are left alone.
const LAID: vec2<f32> = vec2<f32>(0.008, 0.02);
// How deep under the plane the body is at its lock's guide a hair's point is, m, where it
// starts to be brought onto it less, and where it is left (it is elsewhere).
const UNDER: vec2<f32> = vec2<f32>(0.015, 0.035);
// How deep in the body a hair's point is, m, where it starts to be pushed out less, and where
// it is left.
const DEEP: vec2<f32> = vec2<f32>(0.008, 0.02);
// The hairs of a piece a blade cut off come apart as they fall: how far each drifts from the
// piece (m), how far it turns (radians), how much of the fall the last of them is behind, and
// the share of the piece's time (see `shed`) in which they come apart and catch up.
const SHED_DRIFTS: f32 = 0.07;
const SHED_TURNS: f32 = 1.1;
const SHED_LAGS: f32 = 0.6;
const SHED_APART: vec2<f32> = vec2<f32>(0.2, 0.3);

fn body_cell(i: u32) -> vec2<f32> {
    let c = bodies[i];
    return vec2<f32>(f32(i32(c << 16u) >> 16u), f32((c >> 16u) & 0xffu));
}

// Box `o`'s first sample's index and place in cells for `at` on the body as grown (w: 0
// outside the box).
fn body_place(o: u32, at: vec3<f32>) -> vec4<f32> {
    let lattice = guide_offsets[o + 3u];
    let c = (at - lattice.xyz) * lattice.w;
    let size = guide_offsets[o + 4u].xyz;
    return vec4<f32>(c, f32(all(c >= vec3<f32>(0.0)) && all(c < size - 1.0)));
}

// How much of the skin nearest `at` on the body as grown box `o`'s bone carries (0..1; -1
// outside the box).
fn body_share(o: u32, at: vec3<f32>) -> f32 {
    let c = body_place(o, at);
    if (c.w == 0.0) {
        return -1.0;
    }
    let size = vec3<u32>(guide_offsets[o + 4u].xyz);
    let start = guide_offsets[o + 5u];
    let cell = vec3<u32>(c.xyz);
    let f = c.xyz - vec3<f32>(cell);
    let row = size.x * size.y;
    let i = u32(start.x) + (u32(start.y) << 16u) + cell.x + size.x * cell.y + row * cell.z;
    let x00 = mix(body_cell(i).y, body_cell(i + 1u).y, f.x);
    let x10 = mix(body_cell(i + size.x).y, body_cell(i + size.x + 1u).y, f.x);
    let x01 = mix(body_cell(i + row).y, body_cell(i + row + 1u).y, f.x);
    let x11 = mix(body_cell(i + row + size.x).y, body_cell(i + row + size.x + 1u).y, f.x);
    return mix(mix(x00, x10, f.y), mix(x01, x11, f.y), f.z) / 255.0;
}

// The body as grown at `at`, from box `o`: the signed distance and its gradient there
// ((0, FAR) outside the box).
fn body_sample(o: u32, at: vec3<f32>) -> vec4<f32> {
    let c = body_place(o, at);
    if (c.w == 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, FAR);
    }
    let size = vec3<u32>(guide_offsets[o + 4u].xyz);
    let start = guide_offsets[o + 5u];
    let cell = vec3<u32>(c.xyz);
    let f = c.xyz - vec3<f32>(cell);
    let row = size.x * size.y;
    let i = u32(start.x) + (u32(start.y) << 16u) + cell.x + size.x * cell.y + row * cell.z;
    let c000 = body_cell(i).x;
    let c100 = body_cell(i + 1u).x;
    let c010 = body_cell(i + size.x).x;
    let c110 = body_cell(i + size.x + 1u).x;
    let c001 = body_cell(i + row).x;
    let c101 = body_cell(i + row + 1u).x;
    let c011 = body_cell(i + row + size.x).x;
    let c111 = body_cell(i + row + size.x + 1u).x;
    let x00 = mix(c000, c100, f.x);
    let x10 = mix(c010, c110, f.x);
    let x01 = mix(c001, c101, f.x);
    let x11 = mix(c011, c111, f.x);
    let y0 = mix(x00, x10, f.y);
    let y1 = mix(x01, x11, f.y);
    return vec4<f32>(
        mix(mix(c100 - c000, c110 - c010, f.y), mix(c101 - c001, c111 - c011, f.y), f.z),
        mix(x10 - x00, x11 - x01, f.z),
        y1 - y0,
        mix(y0, y1, f.z) * BODY_UNIT);
}

// Where box `o`'s bone alone would have brought `q` (the instance's model space) from on the
// body as grown.
fn body_was(o: u32, q: vec3<f32>) -> vec3<f32> {
    let r0 = guide_offsets[o];
    let r1 = guide_offsets[o + 1u];
    let r2 = guide_offsets[o + 2u];
    return vec3<f32>(dot(r0.xyz, q) + r0.w, dot(r1.xyz, q) + r1.w, dot(r2.xyz, q) + r2.w);
}

fn inverse3(m: mat3x3<f32>) -> mat3x3<f32> {
    let r0 = cross(m[1], m[2]);
    let r1 = cross(m[2], m[0]);
    let r2 = cross(m[0], m[1]);
    let det = dot(m[0], r0);
    return transpose(mat3x3<f32>(r0, r1, r2)) * (1.0 / select(det, 1.0, abs(det) < 1e-9));
}

// How a measured body's boxes see a point: for the (at most two) boxes whose bones carry most
// of the skin there, where the point at `taken` was on the body as the hair was grown on it,
// and what brings a small move of the point back there (softbody, tress.rs, `views`).
struct BodyViews {
    n: u32,
    taken: vec3<f32>,
    part: array<u32, 2>,
    was: array<vec3<f32>, 2>,
    back: array<mat3x3<f32>, 2>,
}

// The views of `q` (the instance's model space) of the measured body whose `boxes` boxes
// start at entry `first`. Every box holds the whole body as the hair was grown on it and how
// much its bone carries of the skin nearest each sample; its matrix carries `q` back as its
// bone alone would. Where bones share the skin, it has moved as their blend by their shares,
// as it is skinned: that blend undone gives where `q` was.
fn body_views(first: u32, boxes: u32, q: vec3<f32>) -> BodyViews {
    var v: BodyViews;
    v.n = 0u;
    v.taken = q;
    let count = min(boxes, MAX_BOXES);
    var most = vec2<f32>(0.0);
    var which = vec2<u32>(NONE);
    for (var k = 0u; k < count; k++) {
        let o = first + BOX_ENTRIES * k;
        let share = body_share(o, body_was(o, q));
        if (share > most.x) {
            most = vec2<f32>(share, most.x);
            which = vec2<u32>(o, which.x);
        } else if (share > most.y) {
            most.y = share;
            which.y = o;
        }
    }
    var start = vec3<f32>(FAR);
    for (var e = 0u; e < 2u; e++) {
        let o = which[e];
        if (o == NONE || most[e] <= 0.0 || most[e] < READS_FROM * most.x) {
            break;
        }
        var was = body_was(o, q);
        // (Bones that have moved alike see the point alike.)
        if (distance(was, start) < SAME) {
            break;
        }
        start = was;
        var back = transpose(mat3x3<f32>(guide_offsets[o].xyz, guide_offsets[o + 1u].xyz, guide_offsets[o + 2u].xyz));
        if (most[e] < CARRIES_ALL) {
            // (No blend to undo where every box that reaches there has moved as this one.)
            var alike = true;
            for (var j = 0u; j < count; j++) {
                let oj = first + BOX_ENTRIES * j;
                if (body_place(oj, was).w > 0.0 && distance(body_was(oj, q), was) >= SAME) {
                    alike = false;
                    break;
                }
            }
            for (var it = 0u; it < REFINED && !alike; it++) {
                var sum = mat3x3<f32>(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
                var origin = vec3<f32>(0.0);
                var total = 0.0;
                for (var j = 0u; j < count; j++) {
                    let oj = first + BOX_ENTRIES * j;
                    let share = body_share(oj, was);
                    if (share > 0.0) {
                        // (The bone's own motion: its matrix back the other way.)
                        let r0 = guide_offsets[oj];
                        let r1 = guide_offsets[oj + 1u];
                        let r2 = guide_offsets[oj + 2u];
                        let moved = mat3x3<f32>(r0.xyz, r1.xyz, r2.xyz);
                        sum += moved * share;
                        origin -= (moved * vec3<f32>(r0.w, r1.w, r2.w)) * share;
                        total += share;
                    }
                }
                if (total <= 0.0) {
                    break;
                }
                back = inverse3(sum * (1.0 / total));
                let next = back * (q - origin / total);
                let moved = distance(next, was);
                was = next;
                if (moved < SAME) {
                    break;
                }
            }
        }
        // (Two that find it at the same place read the same there.)
        if (e == 1u && distance(was, v.was[0]) < SAME) {
            break;
        }
        v.part[v.n] = o;
        v.was[v.n] = was;
        v.back[v.n] = back;
        v.n += 1u;
    }
    return v;
}

// The way out of the measured body and the signed distance to it at `q`, near where its
// boxes' `views` were taken (they move with it). Of the boxes that count, the one that finds
// `q` deepest is right. (0, FAR) outside every box.
fn body(first: u32, boxes: u32, views: BodyViews, q: vec3<f32>) -> vec4<f32> {
    var found = vec4<f32>(0.0, 0.0, 0.0, FAR);
    for (var e = 0u; e < views.n; e++) {
        let back = views.back[e];
        let at = views.was[e] + back * (q - views.taken);
        var here = body_sample(views.part[e], at);
        for (var k = 0u; k < min(boxes, MAX_BOXES) && here.w >= FAR; k++) {
            here = body_sample(first + BOX_ENTRIES * k, at);
        }
        if (here.w < found.w) {
            found = vec4<f32>(here.xyz * back, here.w);
        }
    }
    let len = length(found.xyz);
    return vec4<f32>(select(vec3<f32>(0.0, 1.0, 0.0), found.xyz / len, len > 1e-9), found.w);
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
        } else if (stated > 0u) {
            // Hair that follows no guide (combed to a tie, wound in a knot): as wet as the
            // guides are on average.
            wet = head.z;
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
        // Of a piece cut off: its lock's guide where it was cut (the hair's new end), and in
        // the middle of the piece.
        var end_off = vec3<f32>(0.0);
        var mid_off = vec3<f32>(0.0);
        let mid_j = 0.5 * (cut_at + 1.0 + f32(per - 1u));
        var moved: array<vec3<f32>, 3>;
        var share = vec3<f32>(0.0);
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
                moved[g] = its;
                share[g] = w[g];
                if (g == lock) {
                    lock_off = its;
                    if (off_cut) {
                        let last = min(u32(cut_at * reach), per - 1u);
                        end_off = guide_offsets[first_point + last].xyz;
                        let at_mid = max(mid_j * reach, f32(last) + 1.0);
                        let km = min(u32(at_mid), per - 2u);
                        mid_off = mix(guide_offsets[first_point + km].xyz,
                            guide_offsets[first_point + km + 1u].xyz, at_mid - f32(km));
                    }
                    if (stated > 0u) {
                        lock_rest = mix(guide_offsets[base + stated].xyz,
                            guide_offsets[base + stated + 1u].xyz, along - f32(k));
                    }
                }
            }
        }
        // Locks part: a guide gone another way than the hair's own lock (round the other side
        // of a neck or a shoulder, over a collar the lock hangs under) moves the hair less the
        // further apart the two have come, and the lock moves it instead. Blended, the hair
        // is drawn through the body between them.
        for (var g = 0u; g < 3u; g++) {
            var by = moved[g];
            if (g != lock && share[lock] > 0.0) {
                by = mix(by, lock_off, smoothstep(PARTED.x, PARTED.y, distance(by, lock_off)));
            }
            off += share[g] * by;
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
        // How much a hair cut off lies on the body.
        var laid = 0.0;
        if (off_cut && shed < 1.0) {
            // A piece cut off is so many single hairs: each falls a little behind the others,
            // drifts its own way from them and turns as it falls. Where the piece comes to lie
            // on the body they lie spread out on it. (Moved as one with their lock's guide,
            // the cut hair of a head fell and lay there as one braid.)
            let r = fract(f32(s) * vec4<f32>(0.754877, 0.569840, 0.438579, 0.618034)
                + vec4<f32>(0.31, 0.17, 0.73, 0.0));
            let apart = smoothstep(0.0, SHED_APART.x, shed);
            let lag = SHED_LAGS * r.z * (1.0 - smoothstep(0.02, SHED_APART.y, shed));
            let k = min(u32(mid_j), per - 1u);
            var mid = rest[inst.first.x + s * per + k].xyz + mix(mid_off, end_off, lag);
            q = p.xyz + mix(off, end_off, lag);
            let way = 6.2832 * r.x;
            var drift = vec3<f32>(cos(way), 0.0, sin(way)) * (SHED_DRIFTS * sqrt(r.w) * apart);
            var axis = normalize(vec3<f32>(cos(way * 7.0), 2.0 * r.y - 1.0, sin(way * 7.0)));
            let shapes = u32(head.x);
            let first = inst.out.y - 1u - 2u * shapes;
            let boxes = u32(guide_offsets[first - 1u].x);
            if (boxes > 0u) {
                let first_box = first - 1u - BOX_ENTRIES * boxes;
                let under = body(first_box, boxes, body_views(first_box, boxes, mid), mid);
                let lies = 1.0 - smoothstep(0.015, 0.05, under.w);
                drift -= under.xyz * (dot(drift, under.xyz) * lies);
                axis = normalize(mix(axis, under.xyz, lies) + vec3<f32>(1e-4, 0.0, 0.0));
                laid = lies;
            }
            let turn = (2.0 * r.y - 1.0) * SHED_TURNS * apart * (1.0 - 0.5 * laid);
            let v = q - mid;
            q = mid + drift + v * cos(turn) + cross(axis, v) * sin(turn)
                + axis * (dot(axis, v) * (1.0 - cos(turn)));
        }
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
                // The lock as one first: where its guide lies on the body, its hairs are laid
                // on the plane the body is there. (Each pushed out its own way from where the
                // blend left it, centimeters deep under the guide, a hair comes out in zigzags:
                // deep inside, the way out turns from point to point.)
                var views = body_views(first_box, boxes, q);
                if (stated > 0u && share[lock] > 0.0) {
                    let guide = lock_rest + lock_off;
                    let under = body(first_box, boxes, views, guide);
                    // (Only the hairs that are with the guide there: one that has its own
                    // way, round the other side of a neck, is not under this plane at all,
                    // and brought onto it would stand out from the head in a fan.)
                    let depth = keep - dot(q - guide, under.xyz) - under.w;
                    if (depth > 0.0) {
                        q += under.xyz * (depth
                            * (1.0 - smoothstep(LAID.x, LAID.y, under.w))
                            * (1.0 - smoothstep(UNDER.x, UNDER.y, depth))
                            * (1.0 - smoothstep(UNDER.y, 2.0 * UNDER.y, distance(q, guide))));
                    }
                    if (distance(q, views.taken) > VIEWED_ANEW) {
                        views = body_views(first_box, boxes, q);
                    }
                }
                // Then each hair out of what it is still in, less the deeper it is (deep, it
                // is hidden, and the way out is anyone's guess).
                for (var again = 0u; again < 2u; again++) {
                    let now = body(first_box, boxes, views, q);
                    if (now.w >= keep) {
                        // (A hair cut off that lies on the body lies on it all along: turned
                        // on a shoulder as a straight stick, its ends stood off it.)
                        q -= now.xyz * ((now.w - keep) * laid
                            * (1.0 - smoothstep(0.03, 0.06, now.w)));
                        break;
                    }
                    q += now.xyz * ((keep - now.w) * (1.0 - smoothstep(DEEP.x, DEEP.y, -now.w)));
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

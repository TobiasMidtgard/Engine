// Strands, before the scene pass (strands.rs): shade every point once, bin the segments into
// 8x8-pixel tiles, and rasterize each tile in compute with real coverage. Requires lib_types +
// lib_frame + lib_atmosphere + strands_common.
//
// As the shipped software hair rasterizers of 2023-2025 do (Frostbite's in Dragon Age: The
// Veilguard, after Tafuri's "Strand-based hair rendering in Frostbite", SIGGRAPH 2019; the
// open-source WebGPU port "frostbitten-hair-webgpu", Matuszczyk 2024): light is computed per
// strand point, not per pixel, so a pixel covered by forty hairs costs forty interpolations,
// not forty BSDF evaluations; each pixel keeps the four nearest fibres in front of the opaque
// scene and adds the rest as one far layer, then blends them front to back. A hair a tenth of
// a pixel wide covers a tenth of its pixels' area, every frame, instead of all of a tenth of
// them on random frames (the screen-door hair it replaces): thin hair is soft and still, not
// dotted and crawling.

@group(1) @binding(0) var<uniform> params: StrandParams;
@group(1) @binding(1) var<storage, read> instances: array<StrandInst>;
@group(1) @binding(2) var<storage, read> info: array<StrandInfo>;
@group(1) @binding(3) var<storage, read> posed: array<vec4<f32>>;
@group(1) @binding(4) var<storage, read> voxels: array<u32>;
@group(1) @binding(5) var<storage, read_write> history: array<vec4<f32>>;
@group(1) @binding(6) var<storage, read_write> lit: array<StrandLit>;
// Per tile: segment counts, then (from `params.tiles`) first references, fill cursors, block
// sums, the active tiles; then the active tile count and the references wanted.
@group(1) @binding(7) var<storage, read_write> tiles: array<atomic<u32>>;
@group(1) @binding(8) var<storage, read_write> refs: array<u32>;
// The raster's dispatch (x, y, z), then the composite's draw (vertices, instances, 0, 0). A
// group of its own: the raster's dispatch reads it as indirect arguments.
@group(2) @binding(0) var<storage, read_write> args: array<u32, 8>;
@group(1) @binding(10) var hair_color: texture_storage_2d<rgba16float, write>;
@group(1) @binding(11) var hair_motion: texture_storage_2d<rgba16float, write>;

const NONE: u32 = 0xffffffffu;
// Tiles a segment may cover; longer ones (a hair across the lens) are left out.
const MAX_SEGMENT_TILES: u32 = 256u;
// Active tiles per row of the raster's dispatch.
const RASTER_ROW: u32 = 32768u;
// Nearest fibres each pixel keeps apart; the rest blend as one.
const LAYERS: u32 = 4u;

fn active_count_index() -> u32 {
    return params.tiles.w + params.misc.x;
}

// ---------------------------------------------------------------- shading

// Optical depth of the hair between model-space point `m` and the edge of the density volume
// along model direction `dir` (unit), up to `steps` cells.
fn strand_depth(inst: StrandInst, m: vec3<f32>, dir: vec3<f32>, steps: u32) -> f32 {
    let d = i32(params.misc.w);
    var q = (m - inst.grid.xyz) / inst.grid.w + dir;
    var sum = 0u;
    for (var k = 0u; k < steps; k++) {
        let c = vec3<i32>(floor(q));
        if (any(c < vec3<i32>(0)) || any(c >= vec3<i32>(d))) {
            break;
        }
        sum += voxels[inst.out.w + u32(c.x + d * (c.y + d * c.z))];
        q += dir;
    }
    // Projected fibre area per cell face: the share of light the fibres in a cell stop.
    return f32(sum) / (STRAND_AREA_UNIT * inst.grid.w * inst.grid.w);
}

const HAIR_F0: f32 = 0.04654; // cuticle, index 1.55

fn hair_fresnel(cos_theta: f32) -> f32 {
    return HAIR_F0 + (1.0 - HAIR_F0) * pow(1.0 - saturate(cos_theta), 5.0);
}

fn hair_gauss(b: f32, x: f32) -> f32 {
    return exp(-0.5 * x * x / (b * b)) / (2.5066283 * b);
}

// Marschner's fibre model in Karis's fit ("Physically Based Hair Shading in Unreal", 2016):
// light from `l` scattered toward `v` by a fibre along `t` of color `base`. The R highlight off
// the cuticle, TT through the fibre when backlit, TRT off its back wall (colored, with glints).
// `glint.x` scales TRT, `glint.y` the tilt of the cuticle scales.
fn hair_bsdf(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, glint: vec2<f32>) -> vec3<f32> {
    let sin_v = clamp(dot(t, v), -1.0, 1.0);
    let sin_l = clamp(dot(t, l), -1.0, 1.0);
    let cos_theta_d = max(cos(0.5 * abs(asin(sin_v) - asin(sin_l))), 0.05);
    let lp = l - sin_l * t;
    let vp = v - sin_v * t;
    let cos_phi = dot(lp, vp) * inverseSqrt(dot(lp, lp) * dot(vp, vp) + 1e-4);
    let cos_half_phi = sqrt(saturate(0.5 + 0.5 * cos_phi));
    let n_prime = 1.19 / cos_theta_d + 0.36 * cos_theta_d;
    let shift = 0.035 * glint.y;
    let b = rough * rough;
    let s = sin_l + sin_v;
    let vol = dot(v, l);
    var out = vec3<f32>(0.0);
    let mr = hair_gauss(b * 1.4142 * cos_theta_d, s + shift * 2.0);
    out += vec3<f32>(mr * 0.4 * cos_half_phi * hair_fresnel(sqrt(saturate(0.5 + 0.5 * vol))));
    let mtt = hair_gauss(b * 0.5, s - shift);
    let a = 1.0 / n_prime;
    let h = cos_half_phi * (1.0 + a * (0.6 - 0.8 * cos_phi));
    let ftt = hair_fresnel(cos_theta_d * sqrt(saturate(1.0 - h * h)));
    let ttt = pow(base, vec3<f32>(0.5 * sqrt(saturate(1.0 - h * h * a * a)) / cos_theta_d));
    out += mtt * exp(-3.65 * cos_phi - 3.98) * sq(1.0 - ftt) * ttt;
    let mtrt = hair_gauss(b * 2.0, s - shift * 4.0);
    let ftrt = hair_fresnel(cos_theta_d * 0.5);
    let ttrt = pow(base, vec3<f32>(0.8 / cos_theta_d));
    out += mtrt * exp(17.0 * cos_phi - 16.78) * sq(1.0 - ftrt) * ftrt * ttrt * glint.x;
    return out;
}

// Light scattered between many fibres (Karis's approximation): soft, wrapped, more saturated
// the deeper it went.
fn hair_multiple(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, shadow: f32, scatter: f32) -> vec3<f32> {
    let kajiya = 1.0 - abs(dot(t, l));
    let nf = normalize(v - t * dot(v, t) + vec3<f32>(0.0, 1e-5, 0.0));
    let nl = saturate((dot(nf, l) + 1.0) * 0.25);
    let diffuse = INV_PI * mix(nl * 2.0, kajiya, 0.33) * scatter;
    let luma = max(luminance(base), 1e-4);
    let tint = pow(base / luma, vec3<f32>(0.3 * (1.0 - shadow)));
    return base * diffuse * tint;
}

// One light through `depth` of hair, with the shadow maps' `shadow`. Dual scattering (Zinke et
// al. 2008), roughly: the hair in the way stops light singly scattered toward the eye, but light
// scattered between fibres goes around them, deeper the fairer they are, and takes their color.
fn hair_light(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, glint: vec2<f32>, shadow: f32, depth: f32, scatter: f32) -> vec3<f32> {
    let fair = saturate(dot(base, vec3<f32>(0.3, 0.5, 0.2)) * 2.0);
    let tau = depth * mix(1.0, 0.45, fair);
    // The shadow maps hold the hair too: the darker of the two counts, not both.
    let single = min(shadow, exp(-depth));
    let tint = pow(base / max(max(base.r, base.g), max(base.b, 1e-4)), vec3<f32>(0.12 * tau));
    let through = min(shadow, exp(-0.3 * tau)) * tint;
    return hair_bsdf(t, v, l, base, rough, glint) * single
        + hair_multiple(t, v, l, base, single, scatter) * through;
}

// How much of a light at `dist` along `l` the people's capsules (capsules.rs) let through to
// `rel`: the head behind a lock, the shoulder under it. The point lights cast no shadow maps,
// and the density volume holds only the hair. As `capsule_occlusion` does for the sun, soft
// with the distance along the ray; the capsule the point lies on is skipped.
fn strand_capsule_light(rel: vec3<f32>, l: vec3<f32>, dist: f32) -> f32 {
    let people = min(u32(frame.distant_shadows.y), 32u);
    let base = frame.counts.x;
    var vis = 1.0;
    for (var p = 0u; p < people; p++) {
        let head = point_lights[base + p];
        let d = rel - head.position_range.xyz;
        let reach = head.position_range.w;
        let along = -dot(d, l);
        if (along < -reach || dot(d, d) - along * along > reach * reach) { continue; }
        let first = u32(head.color_radius.x);
        let count = min(u32(head.color_radius.y), 32u);
        for (var i = 0u; i < count; i++) {
            let c = point_lights[first + i];
            let a = c.position_range.xyz;
            let r = c.position_range.w;
            let ab = c.color_radius.xyz - a;
            let len2 = max(dot(ab, ab), 1e-8);
            let near = a + ab * saturate(dot(rel - a, ab) / len2);
            if (length(near - rel) < r * 1.15) { continue; }
            let w = rel - a;
            let ab_l = dot(ab, l);
            let denom = len2 - ab_l * ab_l;
            let s = select(0.0, saturate((dot(w, ab) - dot(w, l) * ab_l) / denom), denom > 1e-8);
            let q = a + ab * s;
            let t = dot(q - rel, l);
            if (t > 0.0 && t < dist) {
                let miss = length(q - (rel + l * t));
                let pen = 0.01 + t * 0.08;
                vis = min(vis, smoothstep(r - pen, r + pen, miss));
            }
        }
    }
    return vis;
}

fn strand_shade(inst: StrandInst, m: vec3<f32>, rel: vec3<f32>, t: vec3<f32>, base: vec3<f32>, random: f32, rough_in: f32, along: f32) -> vec3<f32> {
    let fur = inst.more.y == 1u;
    let v = normalize(-rel);
    let nc = normalize(v - t * dot(v, t) + vec3<f32>(0.0, 1e-5, 0.0));
    let glint = vec2<f32>(0.6 + 0.8 * fract(random * 91.7), 0.4 + 1.2 * fract(random * 53.1));
    let rough = rough_in * (0.85 + 0.3 * fract(random * 37.3));
    // No fibre runs quite straight: each leans its own way, and its highlight with it.
    let b = normalize(cross(t, nc));
    // ...and wavers a few degrees along its length, so a lock's highlight breaks into streaks
    // instead of one smooth band (the sheen of a wig).
    let lean = vec2<f32>(fract(random * 23.7), fract(random * 71.3)) - 0.5;
    let w1 = sin(along * 37.0 + random * 61.0);
    let w2 = sin(along * 23.0 + random * 43.0);
    let ts = normalize(t + nc * (0.1 * lean.x + 0.08 * w1) + b * (0.06 * lean.y + 0.05 * w2));
    // A thin layer (brows, lashes) has next to no hair behind it to scatter light back.
    let scatter = mix(0.8, 1.15, saturate(dot(base, vec3<f32>(0.3, 0.5, 0.2)) * 2.5))
        * select(1.0, 1.1, fur) * select(1.0, 0.4, inst.more.w == 1u);
    // How deep in the hair the point sits, from above and from the side facing the eye: its
    // sky light and the gaps between locks.
    let up = normalize(strand_to_model_dir(inst, vec3<f32>(0.0, 1.0, 0.0)));
    let out = normalize(strand_to_model_dir(inst, nc));
    let tau_sky = 0.5 * (strand_depth(inst, m, up, 16u) + strand_depth(inst, m, out, 16u));
    let occ = mix(exp(-tau_sky), 1.0, 0.1);
    // The gaps between locks and the roots stay darker under every light.
    let lit = mix(occ, 1.0, 0.6);

    var color = vec3<f32>(0.0);
    let caps = capsule_occlusion(rel, nc);
    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel);
    if (dot(sun, sun) > 0.0) {
        let shadow = min(shadow_factor(rel, nc), caps.x);
        let depth = strand_depth(inst, m, normalize(strand_to_model_dir(inst, l)), 48u);
        color += hair_light(ts, v, l, base, rough, glint, shadow, depth, scatter) * sun * lit;
    }
    let count = frame.counts.x;
    for (var i = 0u; i < count; i++) {
        let light = point_lights[i];
        let to_light = light.position_range.xyz - rel;
        let dist2 = dot(to_light, to_light);
        let range = light.position_range.w;
        if (dist2 > range * range) { continue; }
        let pl = to_light * inverseSqrt(max(dist2, 1e-8));
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(light.color_radius.w));
        let depth = strand_depth(inst, m, normalize(strand_to_model_dir(inst, pl)), 24u);
        let body = strand_capsule_light(rel, pl, sqrt(dist2));
        color += hair_light(ts, v, pl, base, rough, glint, body, depth, scatter) * light.color_radius.rgb * falloff * lit;
    }
    let sky_vis = sky_visibility(rel, nc);
    let fill = frame.ambient.rgb;
    let sky = (sky_ambient_covered(nc, sky_vis) - fill) * sky_vis + fill;
    let ao = occ * caps.y;
    color += base * sky * ao * (0.4 + 0.5 * scatter);
    // The sky's sheen along the fibres.
    let env = sample_env_rough(reflect(-v, nc), 0.35) * sky_vis + fill;
    color += env * 0.04 * ao;
    return apply_fog(pre_expose(color), rel);
}

// Shade every drawn point (x: point in the instance, y: instance).
@compute @workgroup_size(256)
fn cs_shade(@builtin(global_invocation_id) id: vec3<u32>) {
    let inst = instances[id.y];
    let per = inst.first.z;
    let i = id.x;
    if (i >= inst.first.w * per) {
        return;
    }
    let s = i / per;
    let j = i - s * per;
    let slot = inst.out.x + i;
    let p = posed[slot];
    let m = p.xyz;
    let rel = strand_model_point(inst, m);
    let mp = posed[slot - select(0u, 1u, j > 0u)].xyz;
    let mn = posed[slot + select(0u, 1u, j + 1u < per)].xyz;
    let t = normalize(strand_model_dir(inst, mn - mp) + vec3<f32>(1e-7, 0.0, 0.0));
    var flags = select(0u, STRAND_LAST, j + 1u == per) | select(0u, STRAND_ROOT, j == 0u);

    let prev = history[slot];
    let fresh = params.misc.y != 0u || bitcast<u32>(prev.w) + 1u != params.misc.z;
    history[slot] = vec4<f32>(rel, bitcast<f32>(params.misc.z));

    let clip = frame.view_proj * vec4<f32>(rel, 1.0);
    var out: StrandLit;
    if (clip.w <= 1e-3) {
        out.pos = vec4<f32>(0.0, 0.0, 0.0, -1.0);
        out.shade = vec4<u32>(0u, 0u, 0u, flags);
        lit[slot] = out;
        return;
    }
    let ndc = clip.xyz / clip.w;
    let pixel = (ndc.xy * vec2<f32>(0.5, -0.5) + 0.5) * frame.viewport.xy;
    let half_px = p.w * inst.tint.w * frame.proj[1][1] * frame.viewport.y * 0.5 / clip.w;

    let c = info[inst.first.y + s].color;
    let root = vec4<f32>(unpack2x16float(c.x), unpack2x16float(c.y));
    let tip = vec4<f32>(unpack2x16float(c.z), unpack2x16float(c.w));
    let along = f32(j) / f32(max(per - 1u, 1u));
    let base = max(mix(root.rgb, tip.rgb, along) * inst.tint.rgb, vec3<f32>(1e-4));
    let color = strand_shade(inst, m, rel, t, base, root.w, tip.w, along);
    let opacity = select(params.screen.z, params.screen.w, inst.more.y == 1u);

    let now = motion_clip(rel);
    let before = prev_motion_clip(select(prev.xyz, static_prev_rel(rel), fresh));
    var motion = vec2<f32>(0.0);
    if (now.w > 1e-6 && before.w > 1e-6) {
        motion = (now.xy / now.w - before.xy / before.w) * vec2<f32>(0.5, -0.5);
    }
    out.pos = vec4<f32>(pixel, ndc.z, half_px);
    out.shade = vec4<u32>(
        pack2x16float(color.rg),
        pack2x16float(vec2<f32>(color.b, opacity)),
        pack2x16float(motion),
        flags,
    );
    lit[slot] = out;
}

// ---------------------------------------------------------------- binning

struct Segment {
    a: vec4<f32>,
    b: vec4<f32>,
    // Reach beyond the line, px: the half width and the pixel filter.
    reach: f32,
    lo: vec2<u32>,
    hi: vec2<u32>,
    ok: bool,
};

// The tiles segment `slot`..`slot + 1` may touch.
fn strand_segment(id: vec3<u32>) -> Segment {
    var seg: Segment;
    seg.ok = false;
    let inst = instances[id.y];
    let per = inst.first.z;
    if (id.x >= inst.first.w * per) {
        return seg;
    }
    let slot = inst.out.x + id.x;
    let a = lit[slot];
    if ((a.shade.w & STRAND_LAST) != 0u || a.pos.w < 0.0) {
        return seg;
    }
    let b = lit[slot + 1u];
    if (b.pos.w < 0.0) {
        return seg;
    }
    seg.a = a.pos;
    seg.b = b.pos;
    seg.reach = max(a.pos.w, b.pos.w) + 0.75;
    let size = frame.viewport.xy;
    let lo = min(a.pos.xy, b.pos.xy) - seg.reach;
    let hi = max(a.pos.xy, b.pos.xy) + seg.reach;
    if (any(hi < vec2<f32>(0.0)) || any(lo >= size)) {
        return seg;
    }
    let grid = vec2<f32>(params.size.xy);
    seg.lo = vec2<u32>(clamp(floor(lo / f32(STRAND_TILE)), vec2<f32>(0.0), grid - 1.0));
    seg.hi = vec2<u32>(clamp(floor(hi / f32(STRAND_TILE)), vec2<f32>(0.0), grid - 1.0));
    let n = (seg.hi - seg.lo + 1u);
    seg.ok = n.x * n.y <= MAX_SEGMENT_TILES;
    return seg;
}

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = saturate(dot(p - a, ab) / max(dot(ab, ab), 1e-8));
    return length(p - (a + ab * t));
}

// Whether the segment comes near tile `tile` (its center within the reach and half a diagonal).
fn segment_touches(seg: Segment, tile: vec2<u32>) -> bool {
    let center = (vec2<f32>(tile) + 0.5) * f32(STRAND_TILE);
    return segment_distance(center, seg.a.xy, seg.b.xy) <= seg.reach + 0.7072 * f32(STRAND_TILE);
}

@compute @workgroup_size(256)
fn cs_count(@builtin(global_invocation_id) id: vec3<u32>) {
    let seg = strand_segment(id);
    if (!seg.ok) {
        return;
    }
    for (var y = seg.lo.y; y <= seg.hi.y; y++) {
        for (var x = seg.lo.x; x <= seg.hi.x; x++) {
            if (segment_touches(seg, vec2<u32>(x, y))) {
                atomicAdd(&tiles[x + y * params.size.x], 1u);
            }
        }
    }
}

@compute @workgroup_size(256)
fn cs_fill(@builtin(global_invocation_id) id: vec3<u32>) {
    let seg = strand_segment(id);
    if (!seg.ok) {
        return;
    }
    let slot = instances[id.y].out.x + id.x;
    for (var y = seg.lo.y; y <= seg.hi.y; y++) {
        for (var x = seg.lo.x; x <= seg.hi.x; x++) {
            if (segment_touches(seg, vec2<u32>(x, y))) {
                let t = x + y * params.size.x;
                let at = atomicLoad(&tiles[params.tiles.x + t]) + atomicAdd(&tiles[params.tiles.y + t], 1u);
                if (at < params.size.z) {
                    refs[at] = slot;
                }
            }
        }
    }
}

// Prefix sums of the tile counts: blocks of 1024 tiles, then the blocks, then the tiles again
// (with the list of tiles that have any segment).
var<workgroup> scan_sums: array<u32, 256>;

@compute @workgroup_size(256)
fn cs_scan_tiles(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let first = wg.x * 1024u + lid * 4u;
    let n = params.misc.x;
    var counts = vec4<u32>(0u);
    for (var k = 0u; k < 4u; k++) {
        if (first + k < n) {
            counts[k] = atomicLoad(&tiles[first + k]);
        }
    }
    let own = counts.x + counts.y + counts.z + counts.w;
    scan_sums[lid] = own;
    workgroupBarrier();
    for (var step = 1u; step < 256u; step *= 2u) {
        var add = 0u;
        if (lid >= step) {
            add = scan_sums[lid - step];
        }
        workgroupBarrier();
        scan_sums[lid] += add;
        workgroupBarrier();
    }
    var at = scan_sums[lid] - own;
    for (var k = 0u; k < 4u; k++) {
        if (first + k < n) {
            atomicStore(&tiles[params.tiles.x + first + k], at);
        }
        at += counts[k];
    }
    if (lid == 255u) {
        atomicStore(&tiles[params.tiles.z + wg.x], scan_sums[255]);
    }
}

@compute @workgroup_size(1)
fn cs_scan_blocks() {
    let blocks = (params.misc.x + 1023u) / 1024u;
    var sum = 0u;
    for (var b = 0u; b < blocks; b++) {
        let v = atomicLoad(&tiles[params.tiles.z + b]);
        atomicStore(&tiles[params.tiles.z + b], sum);
        sum += v;
    }
    atomicStore(&tiles[active_count_index() + 1u], sum);
}

@compute @workgroup_size(256)
fn cs_scan_finish(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = id.x;
    if (t >= params.misc.x) {
        return;
    }
    let start = atomicLoad(&tiles[params.tiles.x + t]) + atomicLoad(&tiles[params.tiles.z + t / 1024u]);
    atomicStore(&tiles[params.tiles.x + t], start);
    if (atomicLoad(&tiles[t]) > 0u && start < params.size.z) {
        let k = atomicAdd(&tiles[active_count_index()], 1u);
        atomicStore(&tiles[params.tiles.w + k], t);
    }
}

@compute @workgroup_size(1)
fn cs_args() {
    let n = atomicLoad(&tiles[active_count_index()]);
    args[0] = min(n, RASTER_ROW);
    args[1] = (n + RASTER_ROW - 1u) / RASTER_ROW;
    args[2] = 1u;
    args[3] = 6u;
    args[4] = n;
    args[5] = 0u;
    args[6] = 0u;
    args[7] = 0u;
}

// ---------------------------------------------------------------- raster

var<workgroup> wg_tile: u32;
var<workgroup> wg_start: u32;
var<workgroup> wg_count: u32;
var<workgroup> seg_a: array<vec4<f32>, 64>;
var<workgroup> seg_b: array<vec4<f32>, 64>;
var<workgroup> seg_ca: array<vec4<f32>, 64>;
var<workgroup> seg_cb: array<vec4<f32>, 64>;
var<workgroup> seg_motion: array<vec4<f32>, 64>;
var<workgroup> seg_root: array<u32, 64>;

fn unpack_color(s: vec4<u32>) -> vec4<f32> {
    return vec4<f32>(unpack2x16float(s.x), unpack2x16float(s.y));
}

// One workgroup per tile with segments, a thread per pixel.
@compute @workgroup_size(8, 8)
fn cs_raster(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) lid: u32, @builtin(local_invocation_id) local: vec3<u32>) {
    if (lid == 0u) {
        let k = wg.x + wg.y * RASTER_ROW;
        var count = 0u;
        var start = 0u;
        var tile = 0u;
        if (k < atomicLoad(&tiles[active_count_index()])) {
            tile = atomicLoad(&tiles[params.tiles.w + k]);
            start = atomicLoad(&tiles[params.tiles.x + tile]);
            count = atomicLoad(&tiles[tile]);
            count = min(count, params.size.z - min(start, params.size.z));
        }
        wg_tile = tile;
        wg_start = start;
        wg_count = count;
    }
    let count = workgroupUniformLoad(&wg_count);
    if (count == 0u) {
        return;
    }
    let tile = wg_tile;
    let start = wg_start;
    let pixel = vec2<u32>(tile % params.size.x, tile / params.size.x) * STRAND_TILE + local.xy;
    let size = vec2<u32>(frame.viewport.xy);
    let inside = all(pixel < size);
    let pc = vec2<f32>(pixel) + 0.5;
    var scene_z = 0.0;
    if (inside) {
        scene_z = textureLoad(prepass_depth, vec2<i32>(pixel), 0);
    }

    // The nearest fibres (unsorted; z 0 empty) and everything behind them, summed.
    var lz = array<f32, LAYERS>(-1.0, -1.0, -1.0, -1.0);
    var lc = array<vec4<f32>, LAYERS>();
    var lm = array<vec2<f32>, LAYERS>();
    var tail = vec4<f32>(0.0);

    for (var base = 0u; base < count; base += 64u) {
        if (base + lid < count) {
            let r = refs[start + base + lid];
            let a = lit[r];
            let b = lit[r + 1u];
            seg_a[lid] = a.pos;
            seg_b[lid] = b.pos;
            seg_ca[lid] = unpack_color(a.shade);
            seg_cb[lid] = unpack_color(b.shade);
            seg_motion[lid] = vec4<f32>(unpack2x16float(a.shade.z), unpack2x16float(b.shade.z));
            // Root segment: covers its root's cap; tip segment: its tip's.
            seg_root[lid] = (a.shade.w & STRAND_ROOT) | select(0u, STRAND_LAST, (b.shade.w & STRAND_LAST) != 0u);
        }
        workgroupBarrier();
        let n = min(64u, count - base);
        if (inside) {
            for (var i = 0u; i < n; i++) {
                let a = seg_a[i];
                let b = seg_b[i];
                let ab = b.xy - a.xy;
                let len2 = dot(ab, ab);
                var t = 0.0;
                if (len2 > 1e-8) {
                    t = dot(pc - a.xy, ab) / len2;
                }
                // Each segment covers the pixels beside it, not those past its ends (the
                // neighbouring segment's): each joint once, not twice. The strand's own ends get
                // round caps.
                if ((t < 0.0 && (seg_root[i] & STRAND_ROOT) == 0u) || (t >= 1.0 && (seg_root[i] & STRAND_LAST) == 0u)) {
                    continue;
                }
                t = saturate(t);
                let d = length(pc - (a.xy + ab * t));
                let w = mix(a.w, b.w, t);
                // A box-filtered line w either side of its axis, d from the pixel's center.
                let cov = max(0.0, min(d + w, 0.5) - max(d - w, -0.5));
                if (cov <= 0.0) {
                    continue;
                }
                let z = mix(a.z, b.z, t);
                if (z < scene_z) {
                    continue;
                }
                let c = mix(seg_ca[i], seg_cb[i], t);
                let alpha = min(cov, 1.0) * c.a;
                if (alpha < 0.002) {
                    continue;
                }
                let mo = mix(seg_motion[i].xy, seg_motion[i].zw, t);
                var far = 0u;
                for (var k = 1u; k < LAYERS; k++) {
                    if (lz[k] < lz[far]) {
                        far = k;
                    }
                }
                if (z > lz[far]) {
                    tail += vec4<f32>(lc[far].rgb * lc[far].a, lc[far].a);
                    lz[far] = z;
                    lc[far] = vec4<f32>(c.rgb, alpha);
                    lm[far] = mo;
                } else {
                    tail += vec4<f32>(c.rgb * alpha, alpha);
                }
            }
        }
        workgroupBarrier();
    }
    if (!inside) {
        return;
    }
    // Front to back: the layers nearest first, then the rest as one layer of their mean color
    // and the coverage of that many independent fibres.
    var transmit = 1.0;
    var color = vec3<f32>(0.0);
    var motion = vec2<f32>(0.0);
    var nearest = -1.0;
    for (var k = 0u; k < LAYERS; k++) {
        var best = 0u;
        for (var q = 1u; q < LAYERS; q++) {
            if (lz[q] > lz[best]) {
                best = q;
            }
        }
        if (lz[best] < 0.0) {
            break;
        }
        if (nearest < 0.0) {
            nearest = lz[best];
            motion = lm[best];
        }
        color += transmit * lc[best].a * lc[best].rgb;
        transmit *= 1.0 - lc[best].a;
        lz[best] = -2.0;
    }
    if (tail.a > 0.0) {
        let a = 1.0 - exp(-tail.a);
        color += transmit * a * tail.rgb / tail.a;
        transmit *= 1.0 - a;
    }
    let alpha = 1.0 - transmit;
    textureStore(hair_color, vec2<i32>(pixel), vec4<f32>(color, alpha));
    textureStore(hair_motion, vec2<i32>(pixel), vec4<f32>(motion, max(nearest, 0.0), alpha));
}

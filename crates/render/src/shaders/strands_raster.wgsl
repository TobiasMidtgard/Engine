// Strands, before the scene pass (strands.rs): place every point on screen, light once the
// points a pixel can show, bin the segments into 8x8-pixel tiles, and rasterize each tile in
// compute with real coverage. Requires lib_types + lib_frame + lib_atmosphere + strands_common.
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
//
// Where the time went, and what keeps it down (RTX 5080, 16 Nov 2026: 21 to 30 ms for herds
// of furred animals far away): the raster costs a tile's segments times its 64 pixels, so the
// frame draws no more strands than a groom's pixels can show (strand_feature.rs
// `strands_wanted`), lists no segment in a tile where the scene hides it (`cs_tile_depth`),
// lights no point that is listed nowhere, and tells each pixel thread with one bit which
// segments can reach it (`segment_rows`).

@group(1) @binding(0) var<uniform> params: StrandParams;
@group(1) @binding(1) var<storage, read> instances: array<StrandInst>;
@group(1) @binding(2) var<storage, read> info: array<StrandInfo>;
@group(1) @binding(3) var<storage, read> posed: array<vec4<f32>>;
@group(1) @binding(4) var<storage, read> voxels: array<u32>;
@group(1) @binding(5) var<storage, read_write> history: array<vec4<f32>>;
@group(1) @binding(6) var<storage, read_write> lit: array<StrandLit>;
// Per tile: segment counts, then (from `params.tiles`) first references, fill cursors, block
// sums, the active tiles; then the active tile count and the references wanted; then per tile
// the farthest opaque depth in it (`cs_tile_depth`, f32 bits).
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

fn tile_far_index() -> u32 {
    return active_count_index() + 2u;
}

// ---------------------------------------------------------------- what is hidden

// The farthest depth the prepass left in each tile (reverse-Z: the least). A segment farther
// than that everywhere in a tile has no pixel to show in it: every fragment of it there fails
// the raster's depth test. Such a segment is not listed for the tile, and a point both of
// whose segments are listed nowhere is not lit: the animal behind a hill or a wall, and the
// far side of a coat near enough for its body to fill whole tiles (a coat only a few tiles
// tall keeps its far side: every tile it touches shows some ground behind it too).
@compute @workgroup_size(8, 8)
fn cs_tile_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= params.size.xy)) {
        return;
    }
    let size = vec2<u32>(frame.viewport.xy);
    let corner = id.xy * STRAND_TILE;
    var far = 1.0;
    for (var y = 0u; y < STRAND_TILE; y++) {
        for (var x = 0u; x < STRAND_TILE; x++) {
            let p = corner + vec2<u32>(x, y);
            if (all(p < size)) {
                far = min(far, textureLoad(prepass_depth, vec2<i32>(p), 0));
            }
        }
    }
    atomicStore(&tiles[tile_far_index() + id.x + id.y * params.size.x], bitcast<u32>(far));
}

// ---------------------------------------------------------------- shading

// Optical depth of the hair between model-space point `m` and the edge of the density volume
// along model direction `dir` (unit), up to `steps` cells. `jitter` (cells, each axis within
// ±0.5) shifts where one hair samples the grid: hairs side by side read different cells, so
// the grid's cells don't show as square patches of shade across a lock.
fn strand_depth(inst: StrandInst, m: vec3<f32>, dir: vec3<f32>, steps: u32, jitter: vec3<f32>) -> f32 {
    let d = i32(params.misc.w);
    let at = (m - inst.grid.xyz) / inst.grid.w;
    let own = vec3<i32>(floor(at));
    // (Along the way the shift only ever starts the march further on, and the point's own cell
    // never counts: a cell is a centimetre across on long hair and holds the hair under the
    // point as well as over it. Shifted back into it, half the hairs lying on top of a lock
    // took the lock under them for hair in their light, and sunlit hair was hatched with dark
    // dashes wherever its surface sat low in a cell. A coat of fur is thinner than its cells
    // are wide, and its own cell is most of the hair there is in its light: fur keeps it.)
    let fur = inst.more.y == 1u;
    let back = select(dot(jitter, dir), 0.0, fur);
    var q = at + jitter + dir * (1.0 + abs(back) - back);
    var sum = 0u;
    for (var k = 0u; k < steps; k++) {
        let c = vec3<i32>(floor(q));
        if (any(c < vec3<i32>(0)) || any(c >= vec3<i32>(d))) {
            break;
        }
        if (fur || any(c != own)) {
            sum += voxels[inst.out.w + u32(c.x + d * (c.y + d * c.z))];
        }
        q += dir;
    }
    // Projected fibre area per cell face: the share of light the fibres in a cell stop.
    return f32(sum) / (STRAND_AREA_UNIT * inst.grid.w * inst.grid.w);
}

const HAIR_F0: f32 = 0.04654; // cuticle, index 1.55
// How much brighter head hair's R highlight is than fur's: the near-white sheen that breaks
// along the locks of a combed head.
const HUMAN_SHINE: f32 = 1.5;

fn hair_fresnel(cos_theta: f32) -> f32 {
    return HAIR_F0 + (1.0 - HAIR_F0) * pow(1.0 - saturate(cos_theta), 5.0);
}

fn hair_gauss(b: f32, x: f32) -> f32 {
    return exp(-0.5 * x * x / (b * b)) / (2.5066283 * b);
}

// Marschner's fibre model in Karis's fit ("Physically Based Hair Shading in Unreal", 2016):
// light from `l` scattered toward `v` by a fibre along `t` of color `base`. The R highlight off
// the cuticle, TT through the fibre when backlit, TRT off its back wall (colored, with glints).
// `glint.x` scales TRT, `glint.y` the tilt of the cuticle scales, `glint.z` R (the cuticle's
// shine: combed, conditioned head hair shines brighter than an animal's coat).
fn hair_bsdf(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, glint: vec3<f32>) -> vec3<f32> {
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
    out += vec3<f32>(mr * 0.4 * glint.z * cos_half_phi * hair_fresnel(sqrt(saturate(0.5 + 0.5 * vol))));
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
fn hair_light(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, glint: vec3<f32>, shadow: f32, depth: f32, scatter: f32) -> vec3<f32> {
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

// Soaked hair: how much its color deepens (an exponent on it) and darkens, how much brighter
// and narrower its highlight is, how much less light scatters among its fibres, and how much
// more of the sky it mirrors. (The darkening is in the scattered light: the color itself is
// multiplied many times over among the fibres, and darkened by a third it turned brown hair
// black, with only a grey shine left on it.)
const WET_DEEPENS: f32 = 0.12;
const WET_DARKENS: f32 = 0.0;
const WET_SHINES: f32 = 0.4;
const WET_SMOOTHS: f32 = 0.35;
const WET_DULLS: f32 = 0.4;
const WET_MIRRORS: f32 = 0.5;

fn strand_shade(inst: StrandInst, m: vec3<f32>, rel: vec3<f32>, t: vec3<f32>, base_in: vec3<f32>, random: f32, rough_in: f32, along: f32, bend: f32, wet: f32) -> vec3<f32> {
    let fur = inst.more.y == 1u;
    let v = normalize(-rel);
    let nc = normalize(v - t * dot(v, t) + vec3<f32>(0.0, 1e-5, 0.0));
    // (Combed head hair shines; coarser beard and brow hair, rougher, much less.)
    // Wet hair: water fills the gaps between the fibres of a rope and smooths their scales.
    // Light that enters is absorbed over a longer way and less of it scatters back out (the
    // hair darkens by almost half and its color deepens a little), and the film mirrors the
    // light in a narrow, bright highlight.
    let base = pow(base_in, vec3<f32>(1.0 + WET_DEEPENS * wet)) * (1.0 - WET_DARKENS * wet);
    let shine = mix(HUMAN_SHINE, 0.8, smoothstep(0.35, 0.42, rough_in)) * (1.0 + WET_SHINES * wet);
    let glint = vec3<f32>(0.6 + 0.8 * fract(random * 91.7), 0.4 + 1.2 * fract(random * 53.1), select(shine, 1.0 + WET_SHINES * wet, fur));
    // The light is worked out at the points only, and the segments blend it: a highlight
    // narrower than the turn of the hair from one point to the next (`bend`, rad) caught one
    // point of a hair and missed the next, and over the crown, where the hair curves round the
    // head, it glittered in dashes. So the lobes widen to the turn they stand for (their
    // width squared, Karis's beta, summed in quadrature with half the turn).
    let rough0 = rough_in * (0.85 + 0.3 * fract(random * 37.3)) * (1.0 - WET_SMOOTHS * wet);
    let rough = sqrt(sqrt(sq(rough0 * rough0) + sq(0.5 * bend)));
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
        * select(1.0, 1.1, fur) * select(1.0, 0.4, inst.more.w == 1u) * (1.0 - WET_DULLS * wet);
    // How deep in the hair the point sits, toward the sky above the side facing the eye and
    // straight out of that side: its sky light and the gaps between locks. (Not straight up:
    // hanging hair has the whole length of its own sheet above it, and measured that way all of
    // it below the crown counted as buried: the back of a head in shade was one black mass.)
    let up = normalize(strand_to_model_dir(inst, normalize(vec3<f32>(0.0, 1.0, 0.0) + 0.99 * nc)));
    let out = normalize(strand_to_model_dir(inst, nc));
    let jitter = vec3<f32>(fract(random * 13.7), fract(random * 29.3), fract(random * 47.9)) - 0.5;
    let tau_sky = 0.5 * (strand_depth(inst, m, up, 16u, jitter) + strand_depth(inst, m, out, 16u, jitter));
    let occ = mix(exp(-tau_sky), 1.0, select(0.06, 0.1, fur));
    // The gaps between locks and the roots stay darker under every light: deep in a head of
    // hair much darker (it is what parts one lock from the next), in fur less.
    let lit = mix(occ, 1.0, select(0.45, 0.6, fur));

    var color = vec3<f32>(0.0);
    let caps = capsule_occlusion(rel, nc);
    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel);
    if (dot(sun, sun) > 0.0) {
        let shadow = min(shadow_factor(rel, nc), caps.x);
        let depth = strand_depth(inst, m, normalize(strand_to_model_dir(inst, l)), 48u, jitter);
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
        let depth = strand_depth(inst, m, normalize(strand_to_model_dir(inst, pl)), 24u, jitter);
        let body = strand_capsule_light(rel, pl, sqrt(dist2));
        color += hair_light(ts, v, pl, base, rough, glint, body, depth, scatter) * light.color_radius.rgb * falloff * lit;
    }
    let sky_vis = sky_visibility(rel, nc);
    let fill = frame.ambient.rgb;
    let sky = (sky_ambient_covered(nc, sky_vis) - fill) * sky_vis + fill;
    let ao = occ * caps.y;
    // Sky light reaching into the hair has passed through fibres on the way, taking their color
    // (blonde goes amber in its depths, not grey-green).
    let sky_tint = pow(base / max(max(base.r, base.g), max(base.b, 1e-4)), vec3<f32>(min(0.6 * tau_sky, 3.0)));
    color += base * sky * ao * sky_tint * (0.4 + 0.5 * scatter);
    // The sky's sheen along the fibres.
    let env = sample_env_rough(reflect(-v, nc), 0.35) * sky_vis + fill;
    color += env * 0.04 * ao * (1.0 + WET_MIRRORS * wet);
    return apply_fog(pre_expose(color), rel);
}

// Place every drawn point on screen (x: point in the instance, y: instance): where it is, how
// wide, how it moved. Its light comes after (`cs_shade`), and only where a pixel can show it.
@compute @workgroup_size(256)
fn cs_project(@builtin(global_invocation_id) id: vec3<u32>) {
    let inst = instances[id.y];
    let per = inst.first.z;
    let i = id.x;
    if (i >= inst.first.w * per) {
        return;
    }
    let j = i % per;
    let slot = inst.out.x + i;
    let p = posed[slot];
    let rel = strand_model_point(inst, p.xyz);
    let flags = select(0u, STRAND_LAST, j + 1u == per) | select(0u, STRAND_ROOT, j == 0u);

    let prev = history[slot];
    // (Its slot held this point last frame: its instance kept its slots, and the point was drawn.)
    let fresh = id.y >= params.misc.y || bitcast<u32>(prev.w) + 1u != params.misc.z;
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
    let half_px = p.w * strand_width(inst, i / per) * frame.proj[1][1] * frame.viewport.y * 0.5 / clip.w;

    let now = motion_clip(rel);
    let before = prev_motion_clip(select(prev.xyz, static_prev_rel(rel), fresh));
    var motion = vec2<f32>(0.0);
    if (now.w > 1e-6 && before.w > 1e-6) {
        motion = (now.xy / now.w - before.xy / before.w) * vec2<f32>(0.5, -0.5);
    }
    out.pos = vec4<f32>(pixel, ndc.z, half_px);
    out.shade = vec4<u32>(0u, 0u, pack2x16float(motion), flags);
    lit[slot] = out;
}

// Light the points a pixel can show: those with a segment some tile lists (`segment_shows`;
// the binning lists a segment by the same test, so every point the raster reads has its
// light). The rest keep no color: nothing reads it.
//
// (This pass writes the points' `shade` and reads only their `pos`, its neighbors' too, so
// no thread reads what another writes.)
@compute @workgroup_size(256)
fn cs_shade(@builtin(global_invocation_id) id: vec3<u32>) {
    let inst = instances[id.y];
    let per = inst.first.z;
    let i = id.x;
    if (i >= inst.first.w * per) {
        return;
    }
    if (!shade_skip(SKIP_STRANDS_OLD_WAY)) {
        var shows = segment_shows(strand_segment(id));
        if (!shows && i > 0u) {
            shows = segment_shows(strand_segment(vec3<u32>(i - 1u, id.y, 0u)));
        }
        if (!shows) {
            return;
        }
    }
    let s = i / per;
    let j = i - s * per;
    let slot = inst.out.x + i;
    let m = posed[slot].xyz;
    let rel = strand_model_point(inst, m);
    let mp = posed[slot - select(0u, 1u, j > 0u)].xyz;
    let mn = posed[slot + select(0u, 1u, j + 1u < per)].xyz;
    let t = normalize(strand_model_dir(inst, mn - mp) + vec3<f32>(1e-7, 0.0, 0.0));
    // How far the hair turns at this point (rad; none at its ends).
    var bend = 0.0;
    if (j > 0u && j + 1u < per) {
        let d0 = m - mp;
        let d1 = mn - m;
        bend = acos(clamp(dot(d0, d1) * inverseSqrt(max(dot(d0, d0) * dot(d1, d1), 1e-20)), -1.0, 1.0));
    }

    let c = info[inst.first.y + s].color;
    let root = vec4<f32>(unpack2x16float(c.x), unpack2x16float(c.y));
    let tip = vec4<f32>(unpack2x16float(c.z), unpack2x16float(c.w));
    let along = f32(j) / f32(max(per - 1u, 1u));
    let base = max(mix(root.rgb, tip.rgb, along) * inst.tint.rgb, vec3<f32>(1e-4));
    var color = pre_expose(base * frame.ambient.rgb);
    if (!shade_skip(SKIP_STRAND_LIGHT)) {
        color = strand_shade(inst, m, rel, t, base, root.w, tip.w, along, bend, strand_wet(inst, posed[slot].w));
    }
    let opacity = select(params.screen.z, params.screen.w, inst.more.y == 1u);
    lit[slot].shade.x = pack2x16float(color.rg);
    lit[slot].shade.y = pack2x16float(vec2<f32>(color.b, opacity));
}

// ---------------------------------------------------------------- binning

struct Segment {
    a: vec4<f32>,
    b: vec4<f32>,
    // Reach beyond the line, px: the half width and the pixel filter.
    reach: f32,
    // The nearest any of its fragments is (reverse-Z: the greatest depth), with room for the
    // raster's rounding.
    near: f32,
    lo: vec2<u32>,
    hi: vec2<u32>,
    ok: bool,
};

// The tiles segment `slot`..`slot + 1` may touch.
// The middle of the segment from `a` to `b` on the curve through the strand's points (a
// Catmull-Rom spline on screen; `back` is `a` less the point before it and `onward` the point
// after `b` less `b`, zero at the strand's ends). Each segment is drawn as two halves through
// it: straight from point to point, a wavy hair seen end-on, at the edge of the head, showed
// as a polygon of sharp corners (a coiled wire), its points a centimetre apart.
fn curve_mid(a: vec2<f32>, b: vec2<f32>, back: vec2<f32>, onward: vec2<f32>) -> vec2<f32> {
    let ab = b - a;
    let ta = select(ab, (ab + back) * 0.5, dot(back, back) > 1e-8);
    let tb = select(ab, (ab + onward) * 0.5, dot(onward, onward) > 1e-8);
    // (The Hermite curve at its middle; its bulge kept within a quarter of the segment, so a
    // hairpin seen on screen doesn't swing out a loop.)
    let bulge = (ta - tb) * 0.125;
    let most = 0.25 * length(ab);
    return (a + b) * 0.5 + bulge * min(1.0, most / max(length(bulge), 1e-6));
}

fn strand_segment(id: vec3<u32>) -> Segment {
    var seg: Segment;
    seg.ok = false;
    let inst = instances[id.y];
    let per = inst.first.z;
    if (id.x >= inst.first.w * per) {
        return seg;
    }
    let slot = inst.out.x + id.x;
    // (Positions only: `cs_shade` calls this while it writes the points' other half.)
    let a = lit[slot].pos;
    if (id.x % per + 1u == per || a.w < 0.0) {
        return seg;
    }
    let b = lit[slot + 1u].pos;
    if (b.w < 0.0) {
        return seg;
    }
    seg.a = a;
    seg.b = b;
    // (Room for the bulge of the curve through the points, `curve_mid`.)
    var back = vec2<f32>(0.0);
    if (id.x % per != 0u) {
        let p = lit[slot - 1u].pos;
        back = select(vec2<f32>(0.0), a.xy - p.xy, p.w >= 0.0);
    }
    var onward = vec2<f32>(0.0);
    if (id.x % per + 2u < per) {
        let n = lit[slot + 2u].pos;
        onward = select(vec2<f32>(0.0), n.xy - b.xy, n.w >= 0.0);
    }
    let bulge = length(curve_mid(a.xy, b.xy, back, onward) - (a.xy + b.xy) * 0.5);
    seg.reach = max(a.w, b.w) + 0.75 + bulge;
    seg.near = max(a.z, b.z) * 1.000001;
    let size = frame.viewport.xy;
    let lo = min(a.xy, b.xy) - seg.reach;
    let hi = max(a.xy, b.xy) + seg.reach;
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

// Whether tile `tile` lists the segment: it comes near, and it is not behind everything
// opaque in the tile (`cs_tile_depth`).
fn segment_in_tile(seg: Segment, tile: vec2<u32>) -> bool {
    if (!segment_touches(seg, tile)) {
        return false;
    }
    if (shade_skip(SKIP_STRANDS_OLD_WAY)) {
        return true;
    }
    let far = bitcast<f32>(atomicLoad(&tiles[tile_far_index() + tile.x + tile.y * params.size.x]));
    return seg.near >= far;
}

// Whether any tile lists the segment.
fn segment_shows(seg: Segment) -> bool {
    if (!seg.ok) {
        return false;
    }
    for (var y = seg.lo.y; y <= seg.hi.y; y++) {
        for (var x = seg.lo.x; x <= seg.hi.x; x++) {
            if (segment_in_tile(seg, vec2<u32>(x, y))) {
                return true;
            }
        }
    }
    return false;
}

@compute @workgroup_size(256)
fn cs_count(@builtin(global_invocation_id) id: vec3<u32>) {
    let seg = strand_segment(id);
    if (!seg.ok) {
        return;
    }
    for (var y = seg.lo.y; y <= seg.hi.y; y++) {
        for (var x = seg.lo.x; x <= seg.hi.x; x++) {
            if (segment_in_tile(seg, vec2<u32>(x, y))) {
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
            if (segment_in_tile(seg, vec2<u32>(x, y))) {
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
var<workgroup> seg_rows: array<vec2<u32>, 64>;
// The screen tangents at each end, between this segment's direction and its neighbour's:
// the joints' cut lines (`joint_tangent`).
var<workgroup> seg_cut: array<vec4<f32>, 64>;
// The middle of each segment's curve (`curve_mid`).
var<workgroup> seg_mid: array<vec2<f32>, 64>;

// The pixels of the 8x8 tile at `corner` whose centers the segment can cover, a bit a pixel
// and a byte a row (rows 0 to 3 in x, 4 to 7 in y). Every pixel `cs_raster` finds coverage
// for is among them (coverage is above zero only within the half width and half a pixel of
// the axis; the 0.01 is room for rounding), and it passes over the rest without working out
// their distance. That is most of the work it had: each of a tile's 64 threads walks the
// tile's whole list, and a segment of fur far away covers two or three of the 64 pixels.
fn segment_rows(a: vec4<f32>, b: vec4<f32>, corner: vec2<f32>, bulge: f32) -> vec2<u32> {
    let reach = max(a.w, b.w) + 0.51 + bulge;
    let ab = b.xy - a.xy;
    var rows = vec2<u32>(0u);
    for (var y = 0u; y < STRAND_TILE; y++) {
        let cy = corner.y + f32(y) + 0.5;
        // The part of the segment within reach of this row's height.
        var t0 = 0.0;
        var t1 = 1.0;
        if (abs(ab.y) > 1e-6) {
            let u = (cy - reach - a.y) / ab.y;
            let v = (cy + reach - a.y) / ab.y;
            t0 = max(min(u, v), 0.0);
            t1 = min(max(u, v), 1.0);
        } else if (abs(a.y - cy) > reach) {
            continue;
        }
        if (t0 > t1) {
            continue;
        }
        // Its columns: the pixel centers within reach of that part.
        let x0 = a.x + ab.x * t0;
        let x1 = a.x + ab.x * t1;
        let first = u32(clamp(ceil(min(x0, x1) - reach - corner.x - 0.5), 0.0, 8.0));
        let last = i32(clamp(floor(max(x0, x1) + reach - corner.x - 0.5), -1.0, 7.0));
        if (i32(first) > last) {
            continue;
        }
        let bits = ((2u << u32(last)) - (1u << first)) << ((y & 3u) * 8u);
        if (y < 4u) {
            rows.x |= bits;
        } else {
            rows.y |= bits;
        }
    }
    return rows;
}

// The cut line at a joint: across the strand along the mean of the directions into and out of
// it (`into` and `out`, on screen), so the pixels about a bend go to one segment or the other
// by which side of it they are, and the outside of the bend is filled out to a miter. (Cut
// square to each segment, the outside of every bend fell to neither: the strand showed a notch
// at every point, and where its segments were shorter than it is wide, a staircase of blocks.)
fn joint_tangent(back: vec2<f32>, onward: vec2<f32>) -> vec2<f32> {
    let o = onward / max(length(onward), 1e-6);
    let i = back / max(length(back), 1e-6);
    let m = i + o;
    // (A hairpin, or a neighbour that isn't on screen: square to this segment.)
    if (dot(m, m) < 0.25 || dot(back, back) < 1e-8) {
        return o;
    }
    return normalize(m);
}

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
    let corner = vec2<u32>(tile % params.size.x, tile / params.size.x) * STRAND_TILE;
    let pixel = corner + local.xy;
    let all_rows = shade_skip(SKIP_STRANDS_OLD_WAY);
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
            let ab = b.pos.xy - a.pos.xy;
            var back = vec2<f32>(0.0);
            if ((a.shade.w & STRAND_ROOT) == 0u) {
                let p = lit[r - 1u].pos;
                back = select(vec2<f32>(0.0), a.pos.xy - p.xy, p.w >= 0.0);
            }
            var onward = vec2<f32>(0.0);
            if ((b.shade.w & STRAND_LAST) == 0u) {
                let n = lit[r + 2u].pos;
                onward = select(vec2<f32>(0.0), n.xy - b.pos.xy, n.w >= 0.0);
            }
            seg_cut[lid] = vec4<f32>(joint_tangent(back, ab), joint_tangent(ab, select(ab, onward, dot(onward, onward) > 1e-8)));
            let mid = curve_mid(a.pos.xy, b.pos.xy, back, onward);
            seg_mid[lid] = mid;
            var rows = vec2<u32>(0xffffffffu);
            if (!all_rows) {
                rows = segment_rows(a.pos, b.pos, vec2<f32>(corner), length(mid - (a.pos.xy + b.pos.xy) * 0.5));
            }
            seg_rows[lid] = rows;
        }
        workgroupBarrier();
        let n = min(64u, count - base);
        if (inside) {
            for (var i = 0u; i < n; i++) {
                // Out of the segment's reach: nothing to work out.
                let rows = seg_rows[i];
                if (((select(rows.x, rows.y, local.y >= 4u) >> ((local.y & 3u) * 8u + local.x)) & 1u) == 0u) {
                    continue;
                }
                let a = seg_a[i];
                let b = seg_b[i];
                let m = seg_mid[i];
                // Each segment covers the pixels between the cut lines at its joints, the
                // neighbouring segments those beyond (`joint_tangent`): each joint once, not
                // twice, mitered round a bend. The strand's own ends get round caps.
                let root = (seg_root[i] & STRAND_ROOT) != 0u;
                let tip = (seg_root[i] & STRAND_LAST) != 0u;
                let cut = seg_cut[i];
                if ((!root && dot(pc - a.xy, cut.xy) < 0.0) || (!tip && dot(pc - b.xy, cut.zw) >= 0.0)) {
                    continue;
                }
                // Two halves, a to the curve's middle and on to b (`curve_mid`), joined round.
                // Past a joint (the outside of a bend), the distance from the end half's line;
                // past the strand's ends, from the end (a cap).
                let am = m - a.xy;
                let mb = b.xy - m;
                let u1 = dot(pc - a.xy, am) / max(dot(am, am), 1e-8);
                let u2 = dot(pc - m, mb) / max(dot(mb, mb), 1e-8);
                let s1 = saturate(u1);
                let s2 = saturate(u2);
                let e1 = length(pc - (a.xy + am * s1));
                let e2 = length(pc - (m + mb * s2));
                let d1 = length(pc - (a.xy + am * select(min(u1, 1.0), s1, root)));
                let d2 = length(pc - (m + mb * select(max(u2, 0.0), s2, tip)));
                let first = d1 <= d2;
                let d = select(d2, d1, first);
                let t = select(0.5 + 0.5 * s2, 0.5 * s1, first);
                let w = mix(a.w, b.w, t);
                // (A miter no longer than twice the strand's width: the far reach of a sharp
                // bend's point is left out.)
                if (min(e1, e2) > 2.0 * w + 1.0) {
                    continue;
                }
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

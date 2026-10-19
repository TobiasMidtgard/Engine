// Depth of field (depth_of_field.rs). Requires lib_types only (no frame group).
//
// A thin-lens circle of confusion (CoC) from the depth buffer and a gather-based bokeh with
// separate near and far fields, after Jimenez 2014 ("Next Generation Post Processing in Call of
// Duty: Advanced Warfare") and the scatter-as-gather weighting Frostbite and Unreal use. Runs on
// the pre-exposed HDR scene (before TAA, which then anti-aliases the result), so bright
// highlights keep their energy and open into discs.
//
// focus      1x1: the focus distance in diopters (1/m): the manual one, or autofocus on the
//            screen center (sky ignored), blended toward it over time
// setup      half res: color and signed CoC (half-res pixels) of each 2x2 block, weighted
//            toward its nearest pixel so a block on a silhouette doesn't mix the two depths
// tiles      8x8 half-res texels (16 px): the largest near-field CoC, the largest CoC
// dilate     tiles: the largest near-field CoC that can reach each tile from around it
// prefilter  half res: a small depth-aware blur sized to the gather's sample spacing, so a
//            single bright texel fills its bokeh disc instead of printing the sample pattern
// gather     half res: rings of samples over the pixel's gather radius. Far field (behind the
//            focus plane, and in focus): the normalized average of samples whose CoC reaches
//            the pixel, a sample behind the pixel spreading no farther than the pixel's own CoC
//            (what is in front hides what is behind, so no halos around in-focus objects). Near
//            field (in front of the focus plane): each sample scatters over its own CoC with
//            weight 1 / its area, giving a coverage alpha, so out-of-focus foreground bleeds
//            softly over whatever is behind it, sharp or not
// postfilter half res: a 3x3 median of both fields, which takes out what is left of the sample
//            pattern
// composite  full res: the sharp image, the far field (depth-aware upsample) where the CoC is
//            over about a pixel, and the near field over both

struct DofParams {
    // x: CoC scale (full-res pixels of radius per diopter of defocus, before the focus term
    // 1 / (1 - f / focus distance)), y: focal length (m), z: near plane (m), w: largest radius
    // (full-res pixels)
    lens: vec4<f32>,
    // full-res width and height, half-res width and height (pixels)
    size: vec4<f32>,
    // x: autofocus (1/0), y: blend toward this frame's focus (0..1, 1 = snap), z: manual focus
    // (diopters), w: autofocus window half size (pixels)
    focus: vec4<f32>,
    // x: how many tiles away the near field can reach, y: closest focus (diopters), z, w unused
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> dof: DofParams;
@group(0) @binding(1) var dof_linear: sampler;
// What each pass reads (see depth_of_field.rs); unused ones hold any other texture.
@group(0) @binding(2) var dof_input_a: texture_2d<f32>;
@group(0) @binding(3) var dof_input_b: texture_2d<f32>;
@group(0) @binding(4) var dof_input_c: texture_2d<f32>;
@group(0) @binding(5) var dof_scene_depth: texture_depth_2d; // final scene depth (reverse-Z, 0 = sky)
@group(0) @binding(6) var dof_focus_state: texture_2d<f32>;  // r: focus (diopters)

// Half-res texels per tile side (16 full-res pixels).
const DOF_TILE: i32 = 8;
// Most rings of gather samples (ring k holds 8k samples: 1 + 4K(K+1) in all).
const DOF_MAX_RINGS: i32 = 7;
// Autofocus samples per side of its square.
const DOF_AF_GRID: i32 = 9;
// Brightest a sample counts in the average that stands in for what a blurred foreground hides
// (pre-exposed: a few times white, below lights and their reflections).
const DOF_FILL_LUMA: f32 = 4.0;

@vertex
fn vs_dof(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn dof_full_size() -> vec2<i32> {
    return vec2<i32>(dof.size.xy);
}

fn dof_half_size() -> vec2<i32> {
    return vec2<i32>(dof.size.zw);
}

fn dof_focus() -> f32 {
    return textureLoad(dof_focus_state, vec2<i32>(0, 0), 0).r;
}

// Signed CoC radius in full-res pixels of reverse-Z depth `depth` with the lens focused at
// `focus` diopters: negative in front of the focus plane, positive behind it. Thin lens:
// c = f^2 (1/s - 1/z) / (N (1 - f/s)), on a sensor `lens.x` scales to pixels.
fn dof_coc(depth: f32, focus: f32) -> f32 {
    let inv_z = max(depth, 0.0) / dof.lens.z;
    let r = dof.lens.x * (focus - inv_z) / max(1.0 - dof.lens.y * focus, 0.05);
    return clamp(r, -dof.lens.w, dof.lens.w);
}

fn dof_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn dof_sanitize(c: vec3<f32>) -> vec3<f32> {
    if (any(c != c)) {
        return vec3<f32>(0.0);
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(65000.0));
}

// ---------------------------------------------------------------- focus

// The distance (diopters) of what is at the screen center: the weighted median of a 9x9 grid of
// depths over a square around the center, center-weighted, sky ignored (so a few sky pixels
// through a gap don't pull the focus to infinity), then averaged over the samples within a
// fifth of it so the focus moves smoothly as the subject does. Mostly sky: infinity.
fn dof_autofocus() -> f32 {
    var d: array<f32, 81>;
    var w: array<f32, 81>;
    let size = dof_full_size();
    let center = vec2<f32>(size) * 0.5;
    var n = 0;
    var total = 0.0;
    var valid = 0.0;
    for (var y = 0; y < DOF_AF_GRID; y++) {
        for (var x = 0; x < DOF_AF_GRID; x++) {
            let o = vec2<f32>(f32(x), f32(y)) / f32(DOF_AF_GRID - 1) * 2.0 - 1.0;
            let g = exp(-2.0 * dot(o, o));
            total += g;
            let px = clamp(vec2<i32>(center + o * dof.focus.w), vec2<i32>(0), size - 1);
            let z = textureLoad(dof_scene_depth, px, 0);
            if (z > 0.0) {
                d[n] = z / dof.lens.z;
                w[n] = g;
                valid += g;
                n += 1;
            }
        }
    }
    if (valid < 0.25 * total) {
        return 0.0;
    }
    var median = 1e9;
    for (var i = 0; i < n; i++) {
        var below = 0.0;
        for (var j = 0; j < n; j++) {
            if (d[j] <= d[i]) {
                below += w[j];
            }
        }
        if (below >= 0.5 * valid) {
            median = min(median, d[i]);
        }
    }
    let sigma = 0.2 * median + 1e-3;
    var sum = 0.0;
    var weight = 0.0;
    for (var i = 0; i < n; i++) {
        let t = (d[i] - median) / sigma;
        let k = w[i] * exp(-t * t);
        sum += k * d[i];
        weight += k;
    }
    return sum / max(weight, 1e-6);
}

@fragment
fn fs_focus(in: FullscreenOut) -> @location(0) vec4<f32> {
    var goal = dof.focus.z;
    if (dof.focus.x > 0.5) {
        goal = min(dof_autofocus(), dof.misc.y);
    }
    let focus = mix(dof_focus(), goal, dof.focus.y);
    return vec4<f32>(focus, 0.0, 0.0, 1.0);
}

// ---------------------------------------------------------------- setup

@fragment
fn fs_setup(in: FullscreenOut) -> @location(0) vec4<f32> {
    let h = vec2<i32>(floor(in.position.xy));
    let size = dof_full_size();
    let focus = dof_focus();
    var colors: array<vec3<f32>, 4>;
    var cocs: array<f32, 4>;
    var nearest = 0;
    var nearest_z = -1.0;
    for (var i = 0; i < 4; i++) {
        let p = min(2 * h + vec2<i32>(i % 2, i / 2), size - 1);
        colors[i] = dof_sanitize(textureLoad(dof_input_a, p, 0).rgb);
        let z = textureLoad(dof_scene_depth, p, 0);
        cocs[i] = dof_coc(z, focus);
        if (z > nearest_z) {
            nearest_z = z;
            nearest = i;
        }
    }
    let coc = cocs[nearest];
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0; i < 4; i++) {
        let w = 1.0 / (1.0 + abs(cocs[i] - coc));
        sum += colors[i] * w;
        weight += w;
    }
    return vec4<f32>(sum / weight, 0.5 * coc);
}

// ---------------------------------------------------------------- tiles

@fragment
fn fs_tiles(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = vec2<i32>(floor(in.position.xy));
    let size = dof_half_size();
    var near = 0.0;
    var largest = 0.0;
    for (var y = 0; y < DOF_TILE; y++) {
        for (var x = 0; x < DOF_TILE; x++) {
            let q = min(t * DOF_TILE + vec2<i32>(x, y), size - 1);
            let coc = textureLoad(dof_input_a, q, 0).a;
            near = max(near, -coc);
            largest = max(largest, abs(coc));
        }
    }
    return vec4<f32>(near, largest, 0.0, 1.0);
}

@fragment
fn fs_dilate(in: FullscreenOut) -> @location(0) vec4<f32> {
    let t = vec2<i32>(floor(in.position.xy));
    let tiles = vec2<i32>(textureDimensions(dof_input_a));
    let reach = i32(dof.misc.x);
    var near = 0.0;
    for (var y = -reach; y <= reach; y++) {
        for (var x = -reach; x <= reach; x++) {
            let q = t + vec2<i32>(x, y);
            if (any(q < vec2<i32>(0)) || any(q >= tiles)) {
                continue;
            }
            let r = textureLoad(dof_input_a, q, 0).x;
            // The gap between the two tiles (half-res texels).
            let gap = max(abs(vec2<f32>(f32(x), f32(y))) - 1.0, vec2<f32>(0.0)) * f32(DOF_TILE);
            if (r >= length(gap)) {
                near = max(near, r);
            }
        }
    }
    return vec4<f32>(near, textureLoad(dof_input_a, t, 0).y, 0.0, 1.0);
}

// ---------------------------------------------------------------- gather

// Gather radius (half-res texels): the pixel's own CoC, or the largest near-field CoC that can
// reach it, whichever is larger.
fn dof_gather_radius(coc: f32, near: f32) -> f32 {
    return min(max(abs(coc), near), 0.5 * dof.lens.w);
}

// The half-res texel under `at` (texel units).
fn dof_texel(at: vec2<f32>) -> vec4<f32> {
    return textureLoad(dof_input_a, clamp(vec2<i32>(floor(at)), vec2<i32>(0), dof_half_size() - 1), 0);
}

// Color and CoC at `at` (texel units): bilinear where the four texels around it are at about
// the same CoC, so a small bright light reads smoothly however the samples fall on it (point
// samples print the sample pattern into its disc), and the nearest texel where they aren't: a
// bilinear sample on a silhouette would mix the two depths' colors under an in-between CoC and
// print a halo.
fn dof_sample(at: vec2<f32>) -> vec4<f32> {
    let uv = at / vec2<f32>(dof_half_size());
    let cocs = textureGather(3, dof_input_a, dof_linear, uv);
    let lo = min(min(cocs.x, cocs.y), min(cocs.z, cocs.w));
    let hi = max(max(cocs.x, cocs.y), max(cocs.z, cocs.w));
    if (hi - lo <= max(0.5, 0.1 * max(abs(lo), abs(hi)))) {
        return textureSampleLevel(dof_input_a, dof_linear, uv, 0.0);
    }
    return dof_texel(at);
}

fn dof_rings(radius: f32) -> i32 {
    return clamp(i32(ceil(radius / 3.0)), 1, DOF_MAX_RINGS);
}

@fragment
fn fs_prefilter(in: FullscreenOut) -> @location(0) vec4<f32> {
    let h = vec2<i32>(floor(in.position.xy));
    let center = textureLoad(dof_input_a, h, 0);
    let near = textureLoad(dof_input_b, h / DOF_TILE, 0).x;
    let radius = dof_gather_radius(center.a, near);
    let spacing = radius / f32(dof_rings(radius));
    let rho = 0.5 * min(abs(center.a), spacing);
    if (rho < 0.5) {
        return center;
    }
    let tolerance = max(1.0, 0.5 * abs(center.a));
    var sum = center.rgb;
    var weight = 1.0;
    for (var i = 0; i < 8; i++) {
        let a = f32(i) * (TAU / 8.0);
        let s = dof_sample(vec2<f32>(h) + 0.5 + vec2<f32>(cos(a), sin(a)) * rho);
        let w = saturate(1.0 - abs(s.a - center.a) / tolerance);
        sum += s.rgb * w;
        weight += w;
    }
    return vec4<f32>(sum / weight, center.a);
}

struct DofLayers {
    // Far field (rgb) and the pixel's CoC (half-res texels), for the depth-aware upsample.
    @location(0) far: vec4<f32>,
    // Near field, premultiplied: rgb * alpha, alpha.
    @location(1) near: vec4<f32>,
};

struct DofSums {
    far: vec4<f32>,
    // The near field in two layers by CoC, which in front of the focus plane is by distance:
    // the front one (CoC over about half the gather radius) over the one behind it.
    front: vec4<f32>,
    back: vec4<f32>,
    // The samples not in the near field, and those in its back layer, those brighter than
    // DOF_FILL_LUMA weighted down to it so a few very bright ones don't outweigh the rest
    // (rgb * weight, weight).
    fill: vec4<f32>,
    back_fill: vec4<f32>,
    // How many samples are not in the near field, how many are (what is behind those is
    // hidden), and how many are in its back layer.
    count: vec3<f32>,
};

// Add a sample (color, signed CoC and distance from the pixel, in half-res texels) to the sums
// of the pixel whose CoC is `center_coc` and gather radius `radius`.
fn dof_accumulate(sums: ptr<function, DofSums>, rgb: vec3<f32>, coc: f32, dist: f32, center_coc: f32, radius: f32) {
    // Near field: more than about a pixel in front of the focus plane. Spreads over its own CoC,
    // whatever lies behind it, with 1 / its area (each sample stands for an equal share of the
    // gather disc, so the sum is the fraction of the pixel's light it brings: its coverage).
    // Layered by CoC so a blurred post in front of a less blurred rock spreads over the rock
    // instead of being outweighed by it (the smaller CoC has the larger weight per sample).
    let near_part = smoothstep(0.5, 1.0, -coc);
    let rn = max(-coc, 0.5);
    let wn = near_part * saturate(rn - dist + 0.5) / (rn * rn);
    let front = smoothstep(0.35, 0.65, rn / radius);
    (*sums).front += vec4<f32>(rgb, 1.0) * (wn * front);
    (*sums).back += vec4<f32>(rgb, 1.0) * (wn * (1.0 - front));
    // Far field: spreads over its own CoC, but a sample behind the pixel no farther than the
    // pixel's own CoC: the pixel's surface hides what is behind it beyond its own blur. A pixel
    // in the near field is itself see-through at its blurred edge, so there the far field is
    // what lies behind it, unoccluded (it shows only where the near field's alpha lets it).
    var rf = max(coc, 0.0);
    if (coc > center_coc) {
        let hidden = 1.0 - smoothstep(0.5, 1.0, -center_coc);
        rf = mix(rf, min(rf, max(center_coc, 0.0)), hidden);
    }
    rf = max(rf, 0.5);
    let wf = (1.0 - near_part) * saturate(rf - dist + 0.5) / (rf * rf);
    (*sums).far += vec4<f32>(rgb * wf, wf);
    let k = 1.0 / max(1.0, dof_luma(rgb) / DOF_FILL_LUMA);
    (*sums).fill += vec4<f32>(rgb, 1.0) * ((1.0 - near_part) * k);
    (*sums).back_fill += vec4<f32>(rgb, 1.0) * (near_part * (1.0 - front) * k);
    (*sums).count += vec3<f32>(1.0 - near_part, near_part, near_part * (1.0 - front));
}

@fragment
fn fs_gather(in: FullscreenOut) -> DofLayers {
    let h = vec2<i32>(floor(in.position.xy));
    let center = textureLoad(dof_input_a, h, 0);
    let near = textureLoad(dof_input_b, h / DOF_TILE, 0).x;
    let radius = dof_gather_radius(center.a, near);
    var out: DofLayers;
    if (radius < 0.5) {
        out.far = center;
        out.near = vec4<f32>(0.0);
        return out;
    }
    let rings = dof_rings(radius);
    let at = vec2<f32>(h) + 0.5;
    var sums: DofSums;
    sums.far = vec4<f32>(0.0);
    sums.front = vec4<f32>(0.0);
    sums.back = vec4<f32>(0.0);
    sums.fill = vec4<f32>(0.0);
    sums.back_fill = vec4<f32>(0.0);
    sums.count = vec3<f32>(0.0);
    dof_accumulate(&sums, center.rgb, center.a, 0.0, center.a, radius);
    for (var k = 1; k <= rings; k++) {
        let r = radius * f32(k) / f32(rings);
        let count = 8 * k;
        let offset = 0.5 * f32(k % 2);
        for (var j = 0; j < count; j++) {
            let a = (f32(j) + offset) * (TAU / f32(count));
            let s = dof_sample(at + vec2<f32>(cos(a), sin(a)) * r);
            dof_accumulate(&sums, s.rgb, s.a, r, center.a, radius);
        }
    }
    // Each sample stands for pi R^2 / (4 K^2) of the disc; its weight is 1 / (pi r^2).
    let share = radius * radius / f32(4 * rings * rings);
    let front = saturate(sums.front.w * share);
    var back = saturate(sums.back.w * share);
    var back_rgb = sums.back.rgb / max(sums.back.w, 1e-12) * back;
    // A pixel in the front layer hides what of the back layer is behind it: there, the back
    // layer covers at least the share of what is around that is in it (so the front layer's
    // blurred edge shows the back layer through it, not what is behind both).
    let center_front = smoothstep(0.5, 1.0, -center.a)
        * smoothstep(0.35, 0.65, max(-center.a, 0.5) / radius);
    let around = sums.count.x + sums.count.z;
    if (center_front > 0.0 && around > 1e-3 && sums.back_fill.w > 1e-6) {
        let hidden = center_front * max(sums.count.z / around - back, 0.0);
        back_rgb += sums.back_fill.rgb / sums.back_fill.w * hidden;
        back += hidden;
    }
    out.near = vec4<f32>(
        sums.front.rgb / max(sums.front.w, 1e-12) * front + back_rgb * (1.0 - front),
        front + back * (1.0 - front),
    );
    // Where the near field covers a sample, the far field behind it is unknown: count it with an
    // average sample's weight, as the darker of the weighted average of the far field reaching
    // the pixel and the average of all the far samples around, with bright ones played down.
    // Renormalizing over the visible samples alone (the weighted average) would brighten a
    // highlight's disc next to a blurred foreground edge the more of its surroundings the edge
    // hides; a plain average would spread a ghost of every highlight nearby over the whole
    // gather radius. With no far sample reaching the pixel (it is deep in the near field, whose
    // alpha hides it), that average is a small weight away.
    var far = center.rgb;
    if (sums.count.x > 1e-3) {
        let plain = sums.fill.rgb / max(sums.fill.w, 1e-12);
        var behind = plain;
        if (sums.far.w > 1e-10) {
            let weighted = sums.far.rgb / sums.far.w;
            behind = select(plain, weighted, dof_luma(weighted) < dof_luma(plain));
        }
        let missing = sums.count.y * sums.far.w / sums.count.x;
        let fill = sums.count.x * 1e-3 / max(radius * radius, 1.0);
        far = (sums.far.rgb + behind * missing + plain * fill) / (sums.far.w + missing + fill);
    }
    out.far = vec4<f32>(far, center.a);
    return out;
}

// ---------------------------------------------------------------- postfilter

fn dof_med3(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> vec4<f32> {
    return max(min(a, b), min(max(a, b), c));
}

// Median of a 3x3 window (row by row), per channel: the median of the rows' largest minimum,
// middle median and smallest maximum.
fn dof_median9(v: array<vec4<f32>, 9>) -> vec4<f32> {
    var lo = vec4<f32>(-65504.0);
    var hi = vec4<f32>(65504.0);
    var mid: array<vec4<f32>, 3>;
    for (var r = 0; r < 3; r++) {
        let a = v[3 * r];
        let b = v[3 * r + 1];
        let c = v[3 * r + 2];
        lo = max(lo, min(min(a, b), c));
        hi = min(hi, max(max(a, b), c));
        mid[r] = dof_med3(a, b, c);
    }
    return dof_med3(lo, dof_med3(mid[0], mid[1], mid[2]), hi);
}

// A 3x3 median over both fields (half res), after Jimenez: it takes out what is left of the
// sample pattern (rings in the disc of a highlight half hidden behind a blurred foreground,
// where the prefilter can't spread it) and keeps the edges of discs and silhouettes. In the far
// field, texels at a different CoC don't vote (they count as the center), so in-focus and
// blurred colors don't trade places along a silhouette.
@fragment
fn fs_postfilter(in: FullscreenOut) -> DofLayers {
    let h = vec2<i32>(floor(in.position.xy));
    let size = dof_half_size();
    let center = textureLoad(dof_input_a, h, 0);
    var out: DofLayers;
    out.far = center;
    out.near = textureLoad(dof_input_b, h, 0);
    let tolerance = max(1.0, 0.5 * abs(center.a));
    var far: array<vec4<f32>, 9>;
    var near: array<vec4<f32>, 9>;
    let blurred = abs(center.a) >= 0.5;
    for (var i = 0; i < 9; i++) {
        let q = clamp(h + vec2<i32>(i % 3 - 1, i / 3 - 1), vec2<i32>(0), size - 1);
        let f = textureLoad(dof_input_a, q, 0);
        far[i] = select(center, f, abs(f.a - center.a) <= tolerance);
        near[i] = textureLoad(dof_input_b, q, 0);
    }
    if (blurred) {
        out.far = vec4<f32>(dof_median9(far).rgb, center.a);
    }
    out.near = dof_median9(near);
    return out;
}

// ---------------------------------------------------------------- composite

// The far field at full-res pixel `p` whose CoC is `coc`: bilinear over the four half-res
// texels around it, each weighted by how close its CoC is, so the blurred background next to an
// in-focus silhouette doesn't take the silhouette's color (and the reverse).
fn dof_far_upsample(p: vec2<i32>, coc: f32) -> vec3<f32> {
    let hp = (vec2<f32>(p) + 0.5) * 0.5 - 0.5;
    let base = floor(hp);
    let f = hp - base;
    let size = dof_half_size();
    let tolerance = max(1.0, 0.25 * abs(coc));
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    var plain = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i % 2, i / 2);
        let q = clamp(vec2<i32>(base) + o, vec2<i32>(0), size - 1);
        let t = textureLoad(dof_input_b, q, 0);
        let bx = select(1.0 - f.x, f.x, o.x == 1);
        let by = select(1.0 - f.y, f.y, o.y == 1);
        let d = (2.0 * t.a - coc) / tolerance;
        let w = bx * by * exp(-d * d);
        sum += t.rgb * w;
        weight += w;
        plain += t.rgb * (bx * by);
    }
    return (sum + plain * 1e-4) / (weight + 1e-4);
}

@fragment
fn fs_composite(in: FullscreenOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.position.xy));
    let size = dof_full_size();
    let color = textureLoad(dof_input_a, p, 0);
    let focus = dof_focus();
    let coc = dof_coc(textureLoad(dof_scene_depth, p, 0), focus);
    // How much of the blurred far field to show: the CoC (over a pixel) through a 3x3 tent, so
    // the blend doesn't flip at silhouettes as TAA's jitter moves them by a fraction of a pixel.
    // A pixel in the near field shows none of its own sharp color: the near field brings it,
    // blurred, over what lies behind. Which blurred far field: the pixel's own if it is blurred,
    // else that of the most blurred pixel around, for the same reason (an in-focus pixel takes
    // the anti-aliased color of its silhouette from the sharp image).
    var blend = 0.0;
    var blurred = coc;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(p + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            let w = f32((2 - abs(x)) * (2 - abs(y))) / 16.0;
            let c = dof_coc(textureLoad(dof_scene_depth, q, 0), focus);
            blend += w * smoothstep(1.0, 2.0, abs(c));
            if (abs(coc) < 1.0 && abs(c) > abs(blurred)) {
                blurred = c;
            }
        }
    }
    blend = max(blend, smoothstep(1.0, 2.0, -coc));
    let near_uv = (vec2<f32>(p) + 0.5) * 0.5 / vec2<f32>(dof_half_size());
    let near = textureSampleLevel(dof_input_c, dof_linear, near_uv, 0.0);
    if (blend <= 0.0 && near.a <= 0.0) {
        return color;
    }
    var base = color.rgb;
    if (blend > 0.0) {
        base = mix(color.rgb, dof_far_upsample(p, blurred), blend);
    }
    return vec4<f32>(base * (1.0 - near.a) + near.rgb, color.a);
}

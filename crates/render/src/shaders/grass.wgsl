// Grass in the scene and the depth prepass. Requires lib_types + lib_frame + grass_common.

struct GrassOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) albedo: vec3<f32>,
    @location(3) along: f32,
    @location(4) across: f32,
    @location(5) gloss: f32,
    @location(6) shade: f32,
    @location(7) glow: f32,
    @location(8) @interpolate(flat) pinnae: f32,
};

// The blades the cull pass (grass_cull.wgsl) kept this frame, grouped by segments. A group's
// draw starts at vertex 16 x its first record, so vertex index / 16 + instance is the record
// and vertex index % 16 the vertex along the blade (grass_shadow.wgsl reads them the same way).
@group(1) @binding(4) var<storage, read> grass_records: array<GrassRecord>;

fn grass_record(instance: u32, vertex: u32) -> BladeShape {
    return grass_unpack(grass_records[(vertex >> 4u) + instance]);
}

@vertex
fn vs_main(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> GrassOut {
    var out: GrassOut;
    let s = grass_record(instance, vertex);
    let b = grass_vertex(s, vertex & 15u, frame.time.x, s.tip);
    out.clip = frame.view_proj * vec4<f32>(b.rel, 1.0);
    out.rel = b.rel;
    out.normal = b.normal;
    out.albedo = b.albedo;
    out.along = b.along;
    out.across = b.across;
    out.gloss = b.gloss;
    out.shade = b.shade;
    out.glow = b.glow;
    out.pinnae = grass_blade_cut(grass_fields[s.field], s);
    return out;
}

fn grass_surface(in: GrassOut, front: bool) -> Surface {
    var s = default_surface();
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    s.normal = n;
    // Lower parts of a meadow sit in the shade of the blades around them (`grass_blade`). The
    // midrib is a little paler and glossier than the edges.
    let mid = 1.0 - abs(in.across);
    s.albedo = in.albedo * (0.94 + 0.1 * mid);
    s.occlusion = in.shade;
    s.roughness = mix(0.7, 0.36, in.gloss * mid);
    s.reflectance = 0.35;
    s.subsurface = 0.5;
    // Wet: blades are waxy and shed water, so rain leaves a film and beads that gloss them
    // (smoother, a little more reflective) and darken them only slightly, unlike soil, which
    // soaks it up (Lagarde, "Water drop 2b", 2013: non-porous surfaces keep their albedo).
    // A fern's frond: the rachis paler and yellower, each leaflet's midvein a pale line with
    // the blade either side of it darker toward its edges (where it is thinnest and curls).
    if (in.pinnae > 299.5) {
        // A raceme (`grass_raceme`): the open flowers its head's colour, paler at their middle
        // (the white style and stamens) and darker at the petals' foot; the pods dark red, the
        // buds dark purple, the axis and stalks reddish.
        let r = grass_raceme(in.along, in.across, in.pinnae);
        // Each flower, pod and bud its own: a little lighter or darker, bluer or redder.
        let own = mix(vec3<f32>(0.86, 0.9, 1.08), vec3<f32>(1.1, 1.0, 0.9), r.w);
        if (r.y > 3.5) {
            // The stem below the head keeps its own colour.
        } else if (r.y < 0.5) {
            s.albedo *= vec3<f32>(0.5, 0.36, 0.28);
        } else if (r.y < 1.5) {
            // Young pods: purplish green, reddening as they ripen.
            s.albedo = mix(s.albedo * grass_raceme_tone(0.0), vec3<f32>(0.1, 0.05, 0.035), 0.7) * own;
        } else if (r.y < 2.5) {
            // Petals: darker at their foot and toward their rims, the style and stamens a small
            // pale eye.
            s.albedo *= mix(0.72, 1.05, smoothstep(0.1, 0.55, r.z)) * mix(1.0, 0.85, smoothstep(0.75, 1.0, r.z));
            s.albedo *= own;
            s.albedo = mix(s.albedo, vec3<f32>(0.6, 0.5, 0.55), (1.0 - smoothstep(0.04, 0.1, r.z)) * 0.45);
            s.subsurface = 0.8;
        } else {
            s.albedo *= grass_raceme_tone(1.0) * own;
        }
    } else if (in.pinnae > 0.0) {
        let a = abs(in.across);
        let t = fract(in.along * in.pinnae - a * 0.55 + 0.25);
        let edge = abs(t - 0.5) * 2.0 / max(0.7 * (1.0 - 0.75 * a * a), 0.05);
        let vein = 1.0 - smoothstep(0.0, 0.14, edge);
        let rachis = 1.0 - smoothstep(0.06, 0.12, a);
        let tone = mix(1.0, 0.8, smoothstep(0.45, 1.0, edge)) + 0.12 * vein;
        s.albedo *= tone;
        s.albedo = mix(s.albedo, s.albedo * vec3<f32>(1.35, 1.3, 0.85), rachis);
        s.roughness = mix(s.roughness, 0.42, vein);
    }
    // A compound leaf's leaflets (wood sorrel, anemone, a woodruff whorl): each paler along
    // its midvein and darker toward its margin, a sorrel's leaflets folded along it.
    if (in.pinnae < 0.0) {
        let e = grass_leaflet_edge(in.along, in.across, in.pinnae);
        let vein = 1.0 - smoothstep(0.0, 0.12, e);
        s.albedo *= mix(1.04, 0.82, smoothstep(0.5, 1.0, e)) + 0.1 * vein;
        s.roughness = mix(s.roughness, 0.4, vein);
    }
    let wet = saturate(surface_weather(in.rel).x);
    s.albedo *= 1.0 - 0.12 * wet;
    s.roughness = mix(s.roughness, 0.16, wet * 0.85);
    s.reflectance = mix(s.reflectance, 0.5, wet);
    return s;
}

@fragment
fn fs_main(in: GrassOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // GRASS_CUT main
    let s = grass_surface(in, front);
    // Debug views (RenderSettings.debug_view), as for meshes.
    switch frame.counts.z {
        case 1u: { return vec4<f32>(s.normal * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(vec3(sky_visibility(in.rel, s.normal)), 1.0); }
        case 4u: { return vec4<f32>(vec3(screen_ao(in.rel)), 1.0); }
        case 5u: {
            let r = screen_reflection(in.rel);
            return vec4<f32>(pre_expose(r.rgb) * r.a, 1.0);
        }
        case 6u: { return vec4<f32>(pre_expose(screen_gi(in.rel)), 1.0); }
        default: {}
    }
    let color = shade_surface(s, in.rel) + grass_translucency(s, in.rel, in.glow);
    return vec4<f32>(apply_fog(color, in.rel), 1.0);
}

// Sunlight through the blades. A blade is thin, so lit from behind it glows a deeper, yellower
// green, strongest looking toward the sun and toward the tips (the lower parts sit in the
// shade of the meadow).
fn grass_translucency(s: Surface, rel: vec3<f32>, glow: f32) -> vec3<f32> {
    let l = frame.sun_dir.xyz;
    let v = normalize(-rel);
    let through = saturate(-dot(s.normal, l));
    let toward_sun = pow(saturate(dot(v, -l)), 3.0);
    let amount = (through * 0.35 + toward_sun * 0.9) * glow;
    if (amount <= 0.0) { return vec3<f32>(0.0); }
    let lit_side = select(s.normal, -s.normal, dot(s.normal, l) < 0.0);
    let sun = frame.sun_color.rgb * sun_transmittance(rel) * cloud_shadow(rel) * shadow_factor(rel, lit_side);
    let tint = s.albedo * saturate3(s.albedo * 4.0);
    return pre_expose(sun * tint * amount * INV_PI);
}

struct GrassPrepassOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) motion_now: vec4<f32>,
    @location(2) motion_prev: vec4<f32>,
    @location(3) cut: vec2<f32>,
    @location(4) @interpolate(flat) pinnae: f32,
};

@vertex
fn vs_prepass(@builtin(instance_index) instance: u32, @builtin(vertex_index) vertex: u32) -> GrassPrepassOut {
    var out: GrassPrepassOut;
    let s = grass_record(instance, vertex);
    let b = grass_vertex(s, vertex & 15u, frame.time.x, s.tip);
    out.clip = frame.view_proj * vec4<f32>(b.rel, 1.0);
    out.normal = b.normal;
    // Where the blade was last frame: same root, last frame's wind.
    let p = grass_vertex(s, vertex & 15u, frame.time.x - frame.time.z, s.tip_prev);
    out.motion_now = motion_clip(b.rel);
    out.motion_prev = prev_motion_clip(static_prev_rel(p.rel));
    out.cut = vec2<f32>(b.along, b.across);
    out.pinnae = grass_blade_cut(grass_fields[s.field], s);
    return out;
}

@fragment
fn fs_prepass(in: GrassPrepassOut, @builtin(front_facing) front: bool) -> PrepassOutput {
    // GRASS_CUT prepass
    var n = normalize(in.normal);
    if (!front) { n = -n; }
    return prepass_output(n, 0.65, in.motion_now, in.motion_prev);
}

// How a blade is cut (`grass_cut`): as its kind's leaves, but a flowering stem is a plain stem
// and its head, and a shrub's berry (a flowering stem among twigs) is cut round.
const BERRY_CUT: f32 = 50.0;

fn grass_flower_cut(cut: f32, flower: f32) -> f32 {
    if (flower < 0.5) {
        return cut;
    }
    return select(0.0, BERRY_CUT, cut > 99.5);
}

// ... and a raceme near the camera cut into its flowers, pods and buds (`grass_raceme_code`).
fn grass_blade_cut(f: GrassField, s: BladeShape) -> f32 {
    let raceme = grass_raceme_code(f, s);
    return select(grass_flower_cut(f.kind.z, s.flower), raceme, raceme > 0.0);
}

// Where (`along`, `across`) lies on a raceme (`code` from `grass_raceme_code`): x 1 inside it
// (0: cut away), y what it is (0 the axis, 1 a seed pod, 2 an open flower, 3 a bud, 4 the stem
// below the head), z how far out from a flower's middle to its petals' edge (0..1), w that
// flower's, pod's or bud's own random number (0..1). The head's flowers sit
// alternately left and right, one every 1/florets of its length; each slot is about as long as
// half the head is wide (`GrassStyle::florets`), so distances are in slots along and
// half-widths across. A raceme opens from its foot upward, so seed pods lengthen below the open
// flowers while buds still nod above them, as on fireweed through midsummer.
fn grass_raceme(along: f32, across: f32, code: f32) -> vec4<f32> {
    let n = floor(code - 300.0);
    let head = code - 300.0 - n;
    let h = (along - (1.0 - head)) / head;
    let a = abs(across);
    // Below the head (y 4): the stem, which the blade flares out from over the last 2% of its
    // length into the head's foot (`grass_vertex`), so there only the stem's width is kept.
    if (h < 0.0) {
        let flare = saturate(1.0 + h * head / 0.02);
        return vec4<f32>(select(0.0, 1.0, a * mix(0.12, 0.85, flare) <= 0.12), 4.0, 0.0, 0.5);
    }
    let x = h * n;
    // The flowering axis runs up the middle to the buds.
    var hit = vec4<f32>(select(0.0, 1.0, a < 0.08 && h < 0.92), 0.0, 1.0, 0.5);
    let k0 = floor(x);
    for (var j = -2; j <= 1; j++) {
        let k = k0 + f32(j);
        if (k < 0.0 || k >= n) {
            continue;
        }
        // Left at even slots, right at odd ones; each with its own slight twist.
        let side = select(-1.0, 1.0, (k - 2.0 * floor(k * 0.5)) < 0.5);
        let hk = (k + 0.5) / n;
        let twist = fract(sin(k * 12.9898 + n) * 43758.5453);
        if (hk < 0.2) {
            // A seed pod: thin, 2.4 slots long, angled up and out from its slot's foot.
            let dir = normalize(vec2<f32>(2.2, 0.8 + 0.25 * twist));
            let d = vec2<f32>(x - (k + 0.15), across * side);
            let s = clamp(dot(d, dir), 0.0, 2.4);
            let off = length(d - dir * s);
            if (off < 0.13 * (1.0 - 0.35 * s / 2.4)) {
                return vec4<f32>(1.0, 1.0, 0.0, twist);
            }
        } else if (hk < 0.8) {
            // An open flower: four broad petals round its middle, facing out, its slot's half
            // of the head; the lowest just past their best, a little smaller.
            let c = vec2<f32>(k + 0.5, side * (0.3 + 0.1 * twist));
            let d = vec2<f32>(x - c.x, across - c.y);
            let r = length(d);
            let petals = 0.7 + 0.3 * abs(cos(2.0 * atan2(d.y, d.x) + twist * 3.0));
            let size = 0.82 * mix(0.8, 1.0, smoothstep(0.2, 0.32, hk));
            if (r < size * petals) {
                return vec4<f32>(1.0, 2.0, r / (size * petals), twist);
            }
            // Its short stalk from the axis.
            if (abs(d.y) < c.y * side * 1.0 && abs(d.x + d.y * side * 0.4) < 0.07) {
                hit = vec4<f32>(1.0, 0.0, 1.0, 0.5);
            }
        } else {
            // A bud: a nodding spindle close to the axis, smaller toward the tip.
            let w = 0.5 * (1.0 - 0.6 * smoothstep(0.8, 1.0, hk));
            let d = vec2<f32>((x - (k + 0.5)) / 0.75, (across - side * 0.22 * w) / w);
            if (dot(d, d) < 1.0) {
                return vec4<f32>(1.0, 3.0, 0.0, twist);
            }
        }
    }
    return hit;
}

// A fern's frond cut into pinnae (the cut pipelines only: a discard anywhere in a shader costs
// every fragment its early depth test, so whole blades never pay for it). `pinnae` pairs of
// leaflets down the frond, each sweeping toward the tip and narrowing to its own point,
// either side of the rachis, which always stays.
fn grass_cut(along: f32, across: f32, pinnae: f32) -> bool {
    if (pinnae > 299.5) {
        return grass_raceme(along, across, pinnae).x < 0.5;
    }
    if (pinnae == BERRY_CUT) {
        // A berry on its thin stalk: the head (widest at 0.95 up the stem) cut to a round
        // dot, not the diamond its two rows make.
        let du = (along - 0.95) / 0.03;
        return along > 0.9 && du * du + across * across > 1.0;
    }
    if (pinnae > 199.5) {
        return grass_twig_cut(along, across, pinnae - 200.0, true);
    }
    if (pinnae > 99.5) {
        return grass_twig_cut(along, across, pinnae - 100.0, false);
    }
    if (pinnae < 0.0) {
        return grass_leaflet_edge(along, across, pinnae) > 1.0;
    }
    if (pinnae == 0.0) {
        return false;
    }
    let a = abs(across);
    if (a < 0.1) {
        return false;
    }
    // Each pinna is a lobe one 1/pinnae of the frond long at the rachis, slanted forward.
    let t = fract(along * pinnae - a * 0.55 + 0.25);
    // ...narrowing toward its tip at the frond's edge, with a gap to the next, its margin
    // finely toothed.
    let teeth = 0.06 * abs(sin(a * 48.0));
    let fill = 0.7 * (1.0 - 0.75 * a * a) - teeth;
    return abs(t - 0.5) * 2.0 > fill;
}

// A dwarf shrub's twig (bilberry, lingonberry) or a bramble's cane (`palmate`, each leaf three
// leaflets): `n` leaves set alternately up a thin stem, one
// every 1/n of its length, on the left then the right, each a small oval on a short petiole
// angled up and out to the blade's edge, its margin finely toothed. One leaf's slot along the
// twig is about as long as half the blade is wide (`GrassStyle::alternate_leaves`), so the ovals
// keep their shape; up the tapering tip they get smaller, as a shoot's young leaves do.
fn grass_twig_cut(along: f32, across: f32, n: f32, palmate: bool) -> bool {
    let a = abs(across);
    // The stem stays (a bramble's cane is thin for its broad leaves).
    if (a < select(0.07, 0.035, palmate)) {
        return false;
    }
    // Leaves on the right sit at even slots, on the left at odd ones: two slots apart on a side.
    let parity = select(1.0, 0.0, across > 0.0);
    let x = along * n;
    let first = floor((x - parity) * 0.5) * 2.0 + parity;
    // Each leaf: its stalk leaves the stem a little above its slot's start; the oval leans
    // out at about 50 degrees from the stem, 1.2 slots long, half as wide.
    let axis = vec2<f32>(0.62, 0.78);
    for (var k = 0; k < 2; k++) {
        let base = first - 2.0 * f32(k) + 0.1;
        let d = vec2<f32>(x - base, a - 0.05);
        let s = dot(d, axis);
        let t = d.x * axis.y - d.y * axis.x;
        if (palmate) {
            // A bramble's leaf: a stalk 0.3 slots long, then three leaflets from its top, the
            // end one a slot long, the side ones splayed 40 degrees and a little shorter.
            if (s > -0.05 && s < 0.32 && abs(t) < 0.045) {
                return false;
            }
            let q = vec2<f32>(s - 0.3, t);
            if (grass_ovate(q, 1.0)) {
                return false;
            }
            let c = 0.766;
            let sn = 0.643;
            if (grass_ovate(vec2<f32>(q.x * c + q.y * sn, q.y * c - q.x * sn), 0.75)
                || grass_ovate(vec2<f32>(q.x * c - q.y * sn, q.y * c + q.x * sn), 0.75)) {
                return false;
            }
            continue;
        }
        // Ovate: widest a little below the middle, coming to a blunt point.
        let f = s / 1.2;
        if (f > 0.08 && f < 1.0) {
            if (grass_ovate(vec2<f32>(s, t), 1.2)) {
                return false;
            }
        } else if (f > -0.05 && f <= 0.08 && abs(t) < 0.05) {
            // The leaf's short stalk.
            return false;
        }
    }
    return true;
}

// Whether `q` (along a leaf's midrib from its base, across it) lies in an ovate leaf `len`
// long: widest a little below the middle, coming to a blunt point, its margin finely toothed.
fn grass_ovate(q: vec2<f32>, len: f32) -> bool {
    let f = q.x / len;
    if (f <= 0.04 || f >= 1.0) {
        return false;
    }
    let half = len * (0.25 * pow(sin(PI * pow(f, 0.85)), 0.8) - 0.017 * abs(sin(f * 40.0)));
    return abs(q.y) < half;
}

// A compound leaf of leaflets radiating from its stalk's top, `code` = -(leaflets + stalk)
// (`GrassStyle::leaflets`, `stalk`): where (`along`, `across`) lies in it, 0 on a leaflet's
// midvein, 1 on its margin, over 1 outside (cut away). The leaf's blade (past the stalk) is
// drawn as a square about as wide as it is long; the leaflets meet in its middle.
// Three are a wood sorrel's: obcordate, widest at the tip with a notch in it, folding along
// the midvein. More are lance-shaped: an anemone's five toothed segments, a woodruff's whorl
// of eight narrow leaves.
fn grass_leaflet_edge(along: f32, across: f32, code: f32) -> f32 {
    let n = floor(-code);
    let stalk = fract(-code);
    let x = (along - stalk) / max(1.0 - stalk, 0.05);
    if (x < 0.0) {
        // The stalk itself stays.
        return 0.0;
    }
    let q = vec2<f32>(x - 0.5, across * 0.5);
    let r = length(q);
    if (r < 0.035) {
        return 0.0;
    }
    let sector = TAU / n;
    let theta = atan2(q.y, q.x);
    let delta = theta - round(theta / sector) * sector;
    // Along and across the nearest leaflet, as fractions of its length.
    let s = r * cos(delta) / 0.5;
    let t = abs(r * sin(delta)) / 0.5;
    if (s <= 0.0 || s >= 1.0) {
        return 2.0;
    }
    if (n < 3.5) {
        let half = 0.6 * pow(s, 0.7) * sqrt(max(1.0 - pow(s, 6.0), 0.0));
        let notch = 1.0 - 0.2 * exp(-(t * t) / 0.012);
        return max(t / max(half, 1e-3), s / notch);
    }
    // An anemone's segments are broader than a whorl's narrow leaves.
    var half = (select(1.3, 0.95, n > 6.5) / n) * pow(sin(PI * pow(s, 0.7)), 0.8);
    if (n < 6.5) {
        half -= 0.035 * abs(sin(s * 38.0));
    }
    return t / max(half, 1e-3);
}

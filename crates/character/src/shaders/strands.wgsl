// Hair strands (scalp hair, brows, lashes, beard). Requires lib_types + lib_frame + common.wgsl
// + strand_data.wgsl.
//
// Every hair is a polyline of `points_per` points; instance i draws hair i as a ribbon that
// faces the camera. Real hairs are far thinner than a pixel, so a ribbon is drawn at least
// about a pixel wide and keeps only the share of its pixels the true width would cover
// (random per pixel, hair and frame); temporal anti-aliasing averages that into soft,
// correctly thin hair.
//
// Shading is Marschner's fiber model in Karis's fit ("Physically Based Hair Shading in
// Unreal", 2016): a white highlight reflected off the cuticle (R, shifted toward the root by the
// tilted scales), a colored one from light that crossed the fiber twice (TRT, shifted toward the
// tip, with glints), light passing straight through when backlit (TT), and an approximation of
// the light scattered between many fibers, which gives hair its depth and saturated color.
// Each point's occlusion (how deep inside the hair it sits) darkens its sky light, and each
// light is dimmed by the hair and body between it and the point (its optical depth along six
// directions, blended by the light's): a hairdo lit from one side falls into shade toward the
// other, and light that crossed more hair picks up more of its color.

@vertex
fn vs_strand(@builtin(vertex_index) vertex: u32, @builtin(instance_index) hair: u32) -> StrandOut {
    let rp = ribbon_point(hair, vertex);
    let dist = max(length(rp.rel), 1e-3);
    // One pixel at this distance, m.
    let pixel = dist * 2.0 / (frame.proj[1][1] * frame.viewport.y);
    let half_width = max(rp.radius, pixel * 0.55);
    let to_cam = -rp.rel / dist;
    let side = normalize(cross(rp.tangent, to_cam) + vec3<f32>(1e-6, 0.0, 0.0));
    let rel = rp.rel + side * (half_width * ribbon_side(vertex % 6u));
    var out: StrandOut;
    out.rel = rel;
    out.tangent = rp.tangent;
    out.attrs = strand_attrs[rp.index];
    out.coverage = saturate(rp.radius / half_width);
    out.hair = hair;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.clip_now = motion_clip(rel);
    out.clip_prev = prev_motion_clip(prev_model_point(rp.prev_model) + side * (half_width * ribbon_side(vertex % 6u)));
    out.rest = rp.rest;
    out.axis_x = rp.axes[0];
    out.axis_y = rp.axes[1];
    out.axis_z = rp.axes[2];
    return out;
}

// Optical depth of the hair and body between the point and a light in direction `l` (world).
fn strand_depth_toward(in: StrandOut, l: vec3<f32>) -> f32 {
    let dir = vec3<f32>(dot(l, in.axis_x), dot(l, in.axis_y), dot(l, in.axis_z));
    let jitter = hash13(vec3<f32>(in.clip.xy, frame.time.w * 0.37));
    return hair_shadow_depth(in.rest, dir, jitter, 1.0);
}

fn strand_keep(in: StrandOut) -> bool {
    let h = hash13(vec3<f32>(in.clip.xy, frame.time.w * 0.618 + f32(in.hair % 4096u) * 0.37));
    return h < in.coverage;
}

fn strand_base_color(kind: u32, random: f32) -> vec3<f32> {
    var pig = character.hair;
    if (kind == 1u || kind == 2u) {
        pig = character.brows;
    } else if (kind == 3u) {
        pig = character.beard;
    }
    var c = hair_color(pig.x, pig.y, pig.z, random);
    // Lashes are much darker than the brows and hair (even fair people's lashes are brown).
    if (kind == 2u) {
        c = hair_color(max(pig.x + 0.45, 0.85), pig.y * 0.3, 0.0, random) * 0.6;
    }
    return c;
}

const HAIR_F0: f32 = 0.04654; // cuticle, index 1.55

fn hair_fresnel(cos_theta: f32) -> f32 {
    return HAIR_F0 + (1.0 - HAIR_F0) * pow(1.0 - saturate(cos_theta), 5.0);
}

fn hair_gauss(b: f32, x: f32) -> f32 {
    return exp(-0.5 * x * x / (b * b)) / (2.5066283 * b);
}

// Light from `l` scattered toward `v` by a fiber along `t` of color `base` (per unit of
// incoming light; includes the fiber's projected width). `rough` is the cuticle's longitudinal
// roughness, `glint` scales the TRT lobe per fiber.
fn hair_bsdf(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, glint: f32) -> vec3<f32> {
    let sin_v = clamp(dot(t, v), -1.0, 1.0);
    let sin_l = clamp(dot(t, l), -1.0, 1.0);
    let cos_theta_d = max(cos(0.5 * abs(asin(sin_v) - asin(sin_l))), 0.05);
    let lp = l - sin_l * t;
    let vp = v - sin_v * t;
    let cos_phi = dot(lp, vp) * inverseSqrt(dot(lp, lp) * dot(vp, vp) + 1e-4);
    let cos_half_phi = sqrt(saturate(0.5 + 0.5 * cos_phi));
    let n_prime = 1.19 / cos_theta_d + 0.36 * cos_theta_d;
    let shift = 0.035;
    let b = rough * rough;
    let s = sin_l + sin_v;
    let vol = dot(v, l);
    var out = vec3<f32>(0.0);
    // R: off the cuticle.
    let mr = hair_gauss(b * 1.4142 * cos_theta_d, s + shift * 2.0);
    out += vec3<f32>(mr * 0.4 * cos_half_phi * hair_fresnel(sqrt(saturate(0.5 + 0.5 * vol))));
    // TT: through the fiber.
    let mtt = hair_gauss(b * 0.5, s - shift);
    let a = 1.0 / n_prime;
    let h = cos_half_phi * (1.0 + a * (0.6 - 0.8 * cos_phi));
    let ftt = hair_fresnel(cos_theta_d * sqrt(saturate(1.0 - h * h)));
    let ttt = pow(base, vec3<f32>(0.5 * sqrt(saturate(1.0 - h * h * a * a)) / cos_theta_d));
    out += mtt * exp(-3.65 * cos_phi - 3.98) * sq(1.0 - ftt) * ttt;
    // TRT: in, off the back wall, out.
    let mtrt = hair_gauss(b * 2.0, s - shift * 4.0);
    let ftrt = hair_fresnel(cos_theta_d * 0.5);
    let ttrt = pow(base, vec3<f32>(0.8 / cos_theta_d));
    out += mtrt * exp(17.0 * cos_phi - 16.78) * sq(1.0 - ftrt) * ftrt * ttrt * glint;
    return out;
}

// Light scattered between many fibers (Karis's approximation): soft, wrapped, and more
// saturated where the hair shadows itself.
fn hair_multiple(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, shadow: f32, scatter: f32) -> vec3<f32> {
    let kajiya = 1.0 - abs(dot(t, l));
    let nf = normalize(v - t * dot(v, t) + vec3<f32>(0.0, 1e-5, 0.0));
    let nl = saturate((dot(nf, l) + 1.0) * 0.25);
    let diffuse = INV_PI * mix(nl * 2.0, kajiya, 0.33) * scatter;
    // Light that wandered through many fibers picks up more of their color.
    let luma = max(dot(base, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-4);
    let tint = pow(base / luma, vec3<f32>(0.3 * (1.0 - shadow)));
    return base * diffuse * tint;
}

// `shadow` is the shadow map's, `depth` the optical depth of the hair in the way. Light scattered
// between fibers gets through deeper (it goes around them), and takes on their color.
fn hair_light(t: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, glint: f32, shadow: f32, depth: f32, scatter: f32) -> vec3<f32> {
    // Fair fibers let more light through than dark ones.
    let fair = saturate(dot(base, vec3<f32>(0.3, 0.5, 0.2)) * 2.0);
    let tau = depth * mix(1.0, 0.45, fair);
    // The shadow map holds the hair too, so the darker of the two counts, not both.
    let single = min(shadow, exp(-tau));
    // Light that went around many fibers is deeper in their color (its brightest channel kept).
    let tint = pow(base / max(max(base.r, base.g), max(base.b, 1e-4)), vec3<f32>(0.12 * tau));
    let through = min(shadow, exp(-0.3 * tau)) * tint;
    return hair_bsdf(t, v, l, base, rough, glint) * single
        + hair_multiple(t, v, l, base, single, scatter) * through;
}

@fragment
fn fs_strand(in: StrandOut) -> @location(0) vec4<f32> {
    if (!strand_keep(in)) {
        discard;
    }
    let kind = strand_params.info.y;
    let t = normalize(in.tangent);
    let v = normalize(-in.rel);
    // A normal facing the camera across the fiber.
    let nc = normalize(v - t * dot(v, t) + vec3<f32>(0.0, 1e-5, 0.0));
    let random = in.attrs.y;
    let base = strand_base_color(kind, random);
    let occ = in.attrs.x;
    // Glints: fibers differ in how they catch the light.
    let glint = 0.6 + 0.8 * fract(random * 91.7);
    // Scalp hair is smoother than brows and beard; fair hair scatters more light between
    // fibers than dark.
    let rough = select(0.42, 0.3, kind == 0u);
    let scatter = mix(0.8, 1.3, saturate(dot(base, vec3<f32>(0.3, 0.5, 0.2)) * 2.5));
    // The hair in the way of each light shades it (below); occlusion keeps the gaps between
    // locks and the roots darker.
    let lit = mix(occ, 1.0, 0.6);
    var color = vec3<f32>(0.0);

    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(in.rel) * cloud_shadow(in.rel);
    if (dot(sun, sun) > 0.0) {
        let shadow = shadow_factor(in.rel, nc);
        let depth = strand_depth_toward(in, l);
        color += hair_light(t, v, l, base, rough, glint, shadow, depth, scatter) * sun * lit;
    }
    let count = frame.counts.x;
    for (var i = 0u; i < count; i++) {
        let light = point_lights[i];
        let to_light = light.position_range.xyz - in.rel;
        let dist2 = dot(to_light, to_light);
        let range = light.position_range.w;
        if (dist2 > range * range) { continue; }
        let pl = to_light * inverseSqrt(max(dist2, 1e-8));
        let falloff = sq(saturate(1.0 - sq(dist2 / (range * range)))) / max(dist2, sq(light.color_radius.w));
        let depth = strand_depth_toward(in, pl);
        color += hair_light(t, v, pl, base, rough, glint, 1.0, depth, scatter) * light.color_radius.rgb * falloff * lit;
    }
    let sky_vis = sky_visibility(in.rel, nc);
    let fill = frame.ambient.rgb;
    let sky = (sky_ambient_covered(nc, sky_vis) - fill) * sky_vis + fill;
    let ao = min(occ, screen_ao(in.rel));
    color += base * sky * ao * (0.4 + 0.5 * scatter);
    // The sky's sheen along the fibers.
    let env = sample_env_rough(reflect(-v, nc), 0.35) * sky_vis + fill;
    color += env * 0.04 * ao;
    return vec4<f32>(apply_fog(pre_expose(color), in.rel), 1.0);
}

@fragment
fn fs_strand_prepass(in: StrandOut) -> PrepassOutput {
    if (!strand_keep(in)) {
        discard;
    }
    let v = normalize(-in.rel);
    let t = normalize(in.tangent);
    let nc = normalize(v - t * dot(v, t) + vec3<f32>(0.0, 1e-5, 0.0));
    return prepass_output(nc, 0.6, in.clip_now, in.clip_prev);
}

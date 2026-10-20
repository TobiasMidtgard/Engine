// Hair strands (scalp hair, brows, lashes, beard). Requires lib_types + lib_frame + common.wgsl
// + strand_data.wgsl.
//
// Every hair is a polyline of `points_per` points; instance i draws hair i as a ribbon that
// faces the camera. Real hairs are far thinner than a pixel, so a ribbon is drawn at least
// about a pixel wide and keeps only the share of its pixels the true width would cover
// (random per pixel, hair and frame); temporal anti-aliasing averages that into soft,
// correctly thin hair.
//
// Shading follows the fiber model: diffuse light around the fiber (Kajiya-Kay), a white
// highlight from the surface shifted toward the root, a colored second highlight from light
// that went through the fiber and back (shifted toward the tip, with glints), and light
// passing through when the hair is backlit. Each point's occlusion (how deep inside the hair
// it sits) darkens all of it.

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
    return out;
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

fn kk_spec(t: vec3<f32>, h: vec3<f32>, exponent: f32) -> f32 {
    let th = dot(t, h);
    return pow(sqrt(max(1.0 - th * th, 0.0)), exponent);
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
    let shift_r = select(-0.08, -0.04, kind != 0u);
    let t_r = normalize(t + nc * shift_r);
    let t_trt = normalize(t + nc * 0.14);
    var color = vec3<f32>(0.0);

    let l = frame.sun_dir.xyz;
    let sun = frame.sun_color.rgb * sun_transmittance(in.rel) * cloud_shadow(in.rel);
    if (dot(sun, sun) > 0.0) {
        let shadow = shadow_factor(in.rel, nc);
        let h = normalize(l + v);
        let tl = dot(t, l);
        let diffuse = base * INV_PI * sqrt(max(1.0 - tl * tl, 0.0)) * (0.5 + 0.5 * saturate(dot(nc, l) + 0.5));
        let r = vec3<f32>(0.08) * kk_spec(t_r, h, 180.0);
        let trt = base * 1.6 * kk_spec(t_trt, h, 30.0) * 0.22 * glint;
        let tt = base * pow(saturate(dot(-l, v)), 8.0) * 0.6;
        color += (diffuse + r + trt + tt) * sun * shadow * mix(occ, 1.0, 0.25);
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
        let h = normalize(pl + v);
        let tl = dot(t, pl);
        let diffuse = base * INV_PI * sqrt(max(1.0 - tl * tl, 0.0)) * (0.5 + 0.5 * saturate(dot(nc, pl) + 0.5));
        let r = vec3<f32>(0.08) * kk_spec(t_r, h, 180.0);
        let trt = base * 1.6 * kk_spec(t_trt, h, 30.0) * 0.22 * glint;
        let tt = base * pow(saturate(dot(-pl, v)), 8.0) * 0.6;
        color += (diffuse + r + trt + tt) * light.color_radius.rgb * falloff * mix(occ, 1.0, 0.25);
    }
    let sky_vis = sky_visibility(in.rel, nc);
    let fill = frame.ambient.rgb;
    let sky = (sky_ambient_covered(nc, sky_vis) - fill) * sky_vis + fill;
    let ao = min(occ, screen_ao(in.rel));
    color += base * sky * ao;
    let env = sample_env_rough(reflect(-v, nc), 0.45) * sky_vis + fill;
    color += env * 0.03 * ao;
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

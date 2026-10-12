// Smoke: a raymarch through one volume, drawn on the far side of its box. Lit by the sun (with
// self-shadowing, a few orders of scattering and the scene's shadow map), the sky above, the
// ground below and nearby fires; fogged; blended over the scene as premultiplied color.

struct SmokeParams {
    min_rel: vec4<f32>,   // box min (camera-relative), voxel size
    size: vec4<f32>,      // box size (m), blend previous -> current
    dims: vec4<f32>,      // voxels, firelight scale
    look: vec4<f32>,      // detail, max steps, tunnel count, unused
    tunnels: array<vec4<f32>, 32>, // per tunnel: from + radius, to + strength (camera-relative)
};

@group(1) @binding(0) var<uniform> smoke: SmokeParams;
@group(1) @binding(1) var density_prev: texture_3d<f32>;
@group(1) @binding(2) var density_cur: texture_3d<f32>;
@group(1) @binding(3) var light_tex: texture_3d<f32>;
@group(1) @binding(4) var glow_tex: texture_3d<f32>;
@group(1) @binding(5) var scene_depth: texture_depth_2d;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
};

// Corner of the unit box for vertex `i` of 36 (6 faces x 2 triangles).
fn box_corner(i: u32) -> vec3<f32> {
    var quad = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    );
    let face = i / 6u;
    let c = quad[i % 6u];
    let side = f32(face % 2u);
    let axis = face / 2u;
    if (axis == 0u) { return vec3(side, c.x, c.y); }
    if (axis == 1u) { return vec3(c.x, side, c.y); }
    return vec3(c.x, c.y, side);
}

@vertex
fn vs_smoke(@builtin(vertex_index) i: u32) -> VsOut {
    let rel = smoke.min_rel.xyz + box_corner(i) * smoke.size.xyz;
    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    return out;
}

fn hg(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

// Smoke particles scatter mostly forward (bright toward the sun), with a little back-scatter.
fn smoke_phase(cos_t: f32, g: f32) -> f32 {
    return mix(hg(cos_t, g), hg(cos_t, -0.2), 0.2);
}

// One tap of the sun's shadow map (the march averages many of them).
fn sun_shadow(rel: vec3<f32>) -> f32 {
    if (frame.sun_color.w < 0.5) { return 1.0; }
    let depth = view_depth(rel);
    if (depth > frame.shadow_splits[3]) { return 1.0; }
    var cascade = 0u;
    for (var i = 0u; i < 3u; i++) {
        if (depth > frame.shadow_splits[i]) { cascade = i + 1u; }
    }
    let lp = frame.shadow_cascades[cascade] * vec4<f32>(rel, 1.0);
    let uv = lp.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let z = lp.z - frame.shadow_params.x * 2.0;
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || z > 1.0) { return 1.0; }
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, cascade, z);
}

// Interleaved gradient noise: a per-pixel offset that hides the step pattern.
fn step_jitter(px: vec2<f32>, frame_index: f32) -> f32 {
    let p = px + 5.588238 * (frame_index % 64.0);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// Closest distance between the view ray (t0..t1) and a segment.
fn ray_segment_distance(dir: vec3<f32>, t0: f32, t1: f32, a: vec3<f32>, b: vec3<f32>) -> f32 {
    let v = b - a;
    let w = -a;
    let bb = dot(dir, v);
    let c = dot(v, v);
    let d = dot(dir, w);
    let e = dot(v, w);
    let denom = max(c - bb * bb, 1e-6);
    let s = clamp((bb * e - c * d) / denom, t0, t1);
    let u = clamp((dot(dir * s - a, v)) / max(c, 1e-6), 0.0, 1.0);
    return length(dir * s - (a + v * u));
}

@fragment
fn fs_smoke(in: VsOut) -> @location(0) vec4<f32> {
    let face_dist = length(in.rel);
    let dir = in.rel / max(face_dist, 1e-4);
    let bmin = smoke.min_rel.xyz;
    let bmax = bmin + smoke.size.xyz;
    let safe = select(dir, vec3(1e-6), abs(dir) < vec3(1e-6));
    let ta = bmin / safe;
    let tb = bmax / safe;
    let tn = min(ta, tb);
    let tf = max(ta, tb);
    let t_near = max(max(tn.x, tn.y), max(tn.z, 0.0));
    let t_far = min(min(tf.x, tf.y), tf.z);
    // Only the far side of the box draws, so each pixel marches once (also from inside).
    if (face_dist < t_far * 0.999 - 0.01) { discard; }

    let t0 = t_near;
    var t1 = t_far;
    let d = textureLoad(scene_depth, vec2<i32>(in.clip.xy), 0);
    if (d > 0.0) {
        let forward = -(frame.view * vec4<f32>(dir, 0.0)).z;
        t1 = min(t1, frame.exposure.z / d / max(forward, 1e-4));
    }
    if (t1 <= t0) { discard; }

    let cell = smoke.min_rel.w;
    let len = t1 - t0;
    let steps = i32(clamp(ceil(len / (cell * 0.6)), 4.0, smoke.look.y));
    let ds = len / f32(steps);

    // Bullet tunnels near this ray.
    var mask = 0u;
    let tunnel_count = min(u32(smoke.look.z), 16u);
    for (var i = 0u; i < tunnel_count; i++) {
        let a = smoke.tunnels[i * 2u];
        let b = smoke.tunnels[i * 2u + 1u];
        if (ray_segment_distance(dir, t0, t1, a.xyz, b.xyz) < a.w * 1.2) {
            mask |= 1u << i;
        }
    }

    let sun_light = frame.sun_camera.rgb;
    let sun_on = dot(sun_light, sun_light) > 0.0;
    let cos_t = dot(dir, frame.sun_dir.xyz);
    // Sky light from above and light bounced off the ground below.
    let amb_up = sky_ambient(vec3<f32>(0.0, 1.0, 0.0));
    let amb_down = sky_ambient(vec3<f32>(0.0, -1.0, 0.0));
    // Billows finer than a pixel only shimmer: fade the detail out with distance (the size
    // of a pixel at the volume, against the detail noise's ~1 m scale).
    let pixel_m = 2.0 / (frame.proj[1][1] * frame.viewport.y) * max(t0, 1.0);
    let detail = smoke.look.x * saturate(1.5 - pixel_m / 0.05);
    // Fine billows drift with the wind and rise a little.
    let drift = (frame.wind.xyz * 0.8 + vec3<f32>(0.0, 0.35, 0.0)) * (frame.time.x % 1000.0);

    var transmittance = 1.0;
    var color = vec3(0.0);
    var t_weighted = 0.0;
    var t = t0 + ds * step_jitter(in.clip.xy, frame.time.w);
    for (var i = 0; i < steps; i++) {
        if (t > t1) { break; }
        let p = dir * t;
        var q = p;
        if (detail > 0.0) {
            // Warp the lookup so the voxels' smooth shapes break up into billows.
            let wp = (tiling_pos(p) - drift) * 0.3;
            let warp = vec3<f32>(
                value_noise3(wp),
                value_noise3(wp + vec3<f32>(31.7, 0.0, 0.0)),
                value_noise3(wp + vec3<f32>(0.0, 47.3, 0.0)),
            ) - 0.5;
            q = p + warp * (cell * 2.0 * detail);
        }
        let local = q - bmin;
        let uvw = local / smoke.size.xyz;
        var s = mix(
            textureSampleLevel(density_prev, linear_clamp, uvw, 0.0),
            textureSampleLevel(density_cur, linear_clamp, uvw, 0.0),
            smoke.size.w,
        );
        if (s.x > 1e-4) {
            // Fade toward the open sides and top so the box never shows.
            let edge = min(
                min(min(local.x, smoke.size.x - local.x), min(local.z, smoke.size.z - local.z)),
                smoke.size.y - local.y,
            );
            var k = saturate(edge / (cell * 2.5));
            if (detail > 0.0) {
                // Erode: where the smoke is thin it breaks into wisps; dense cores keep their
                // shape. `d` is how dense it is (σ = 2/m is halfway).
                let n = fbm3((tiling_pos(p) - drift) * 0.8, 3);
                let d = s.x / (s.x + 2.0);
                let e = saturate(n * 1.5 - 0.2) * min(detail, 1.0) * 0.8;
                k *= saturate((d - e) / max(1.0 - e, 1e-3)) / max(d, 1e-4);
            }
            var m = mask;
            loop {
                if (m == 0u) { break; }
                let j = firstTrailingBit(m);
                m &= m - 1u;
                let a = smoke.tunnels[j * 2u];
                let b = smoke.tunnels[j * 2u + 1u];
                let ab = b.xyz - a.xyz;
                let h = saturate(dot(p - a.xyz, ab) / max(dot(ab, ab), 1e-6));
                let dist = length(p - a.xyz - ab * h);
                k *= 1.0 - b.w * (1.0 - smoothstep(a.w * 0.55, a.w, dist));
            }
            s *= k;
        }
        let sigma = s.x;
        if (sigma > 1e-4) {
            let albedo = saturate3(s.yzw / sigma);
            let light = textureSampleLevel(light_tex, linear_clamp, uvw, 0.0);
            var inscatter = vec3(0.0);
            if (sun_on) {
                // Scattering orders: each later one bounced inside the smoke, so it is
                // dimmer, less shadowed by it and spread wider.
                // Light scattered once (mostly forward, so smoke glows toward the sun) and a
                // few times more. Many bounces inside bright smoke even out into a soft glow,
                // which is why a thick white cloud looks white from every side; dark smoke
                // absorbs before it gets there.
                var sun = vec3(0.0);
                var w = vec3(1.0);
                var reach = 1.0;
                var g = 0.65;
                for (var o = 0; o < 2; o++) {
                    sun += w * smoke_phase(cos_t, g) * exp(-light.x * reach);
                    w *= albedo * 0.7;
                    reach *= 0.25;
                    g *= 0.4;
                }
                sun += albedo * albedo * (2.2 / (4.0 * PI)) * exp(-light.x * 0.12);
                inscatter += sun * sun_light * sun_shadow(p);
            }
            // Sky and ground light, also partly scattered on through the smoke.
            inscatter += 0.5 * amb_up * (0.6 * exp(-light.y) + 0.4 * exp(-light.y * 0.2));
            inscatter += 0.5 * amb_down * (0.6 * exp(-light.z * 0.6) + 0.4 * exp(-light.z * 0.15));
            let glow = textureSampleLevel(glow_tex, linear_clamp, uvw, 0.0).rgb * smoke.dims.w;
            inscatter += glow * (0.25 * INV_PI);
            let step_t = exp(-sigma * ds);
            let absorbed = transmittance * (1.0 - step_t);
            color += albedo * inscatter * absorbed;
            t_weighted += absorbed * t;
            transmittance *= step_t;
            if (transmittance < 0.004) { break; }
        }
        t += ds;
    }
    let alpha = 1.0 - transmittance;
    if (alpha < 0.002) { discard; }
    // Fog between the camera and the smoke (at its weighted depth) veils it like anything else.
    let rel_mid = dir * (t_weighted / alpha);
    let haze = apply_fog(vec3(0.0), rel_mid);
    let lit = apply_fog(pre_expose(color), rel_mid) - haze * transmittance;
    return vec4<f32>(max(lit, vec3(0.0)), alpha);
}

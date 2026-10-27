// Smoke: a raymarch through one volume, drawn on the far side of its box. Lit by the sun (with
// self-shadowing, a few orders of scattering and the scene's shadow map), the sky above, the
// ground below and nearby fires; fogged; blended over the scene as premultiplied color.

struct SmokeParams {
    min_rel: vec4<f32>,   // box min (camera-relative), voxel size
    size: vec4<f32>,      // box size (m), blend previous -> current
    dims: vec4<f32>,      // voxels, firelight scale
    look: vec4<f32>,      // detail, max steps, tunnel count, fog (1: soft, wide ground fog)
    drift: vec4<f32>,     // billows' drift with the wind (xyz, m), rise (w, m)
    wind: vec4<f32>,      // smoothed wind (xyz, m/s)
    tunnels: array<vec4<f32>, 32>, // per tunnel: from + radius, to + strength (camera-relative)
};

@group(1) @binding(0) var<uniform> smoke: SmokeParams;
@group(1) @binding(1) var density_prev: texture_3d<f32>;
@group(1) @binding(2) var density_cur: texture_3d<f32>;
@group(1) @binding(3) var light_tex: texture_3d<f32>;
@group(1) @binding(4) var glow_tex: texture_3d<f32>;
@group(1) @binding(5) var scene_depth: texture_depth_2d;
// Tiling billowy noise: r Perlin-Worley (2 cells per tile), g/b/a inverted Worley fbm (4, 8,
// 16 cells per tile).
@group(1) @binding(6) var detail_tex: texture_3d<f32>;

// Meters per tile of the detail texture: the warp's and the billows'.
const WARP_TILE: f32 = 16.0;
const BILLOW_TILE: f32 = 8.0;
// How far wind shear stretches the billows along the wind.
const STREAK: f32 = 2.5;

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

// Single-scattering albedo for a smoke whose thick cloud looks `color`: a deep cloud of
// particles that each scatter a fraction ω of the light reflects (1 - s) / (1 + s) of it, with
// s = sqrt(1 - ω) (two-stream). White smoke (0.92) is made of particles that hardly absorb.
fn particle_albedo(color: vec3<f32>) -> vec3<f32> {
    let s = (1.0 - color) / (1.0 + color);
    return 1.0 - s * s;
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

// The sun's shadow in smoke, softened: two taps either side of the point, across the sun's
// rays and turned by `spin` (which changes from step to step, so the march averages a disc).
// A tree's shadow falling through a plume reads as a soft darker shaft, not as a sharp copy of
// its crown hanging in the smoke.
fn soft_sun_shadow(rel: vec3<f32>, spin: f32, radius: f32) -> f32 {
    let l = frame.sun_dir.xyz;
    let a = normalize(cross(l, select(vec3(0.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0), abs(l.y) > 0.9)));
    let b = cross(l, a);
    let o = (a * cos(spin * TAU) + b * sin(spin * TAU)) * radius;
    return 0.5 * (sun_shadow(rel + o) + sun_shadow(rel - o));
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
    let steps = i32(clamp(ceil(len / (cell * 0.4)), 4.0, smoke.look.y));
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
    // Sky light from above and around, and light bounced off the ground below.
    let amb_up = sky_ambient(vec3<f32>(0.0, 1.0, 0.0));
    let amb_down = sky_ambient(vec3<f32>(0.0, -1.0, 0.0));
    let flat_dir = vec3<f32>(dir.x, 0.0, dir.z) + vec3<f32>(1e-4, 0.0, 0.0);
    let amb_side = 0.5 * (sky_ambient(normalize(flat_dir)) + sky_ambient(normalize(-flat_dir)));
    // Fog is smoke's soft, wide cousin: its noise is a few times larger, it stays solid where
    // it's thin (a fog bank's top is a soft surface, not wisps) and it doesn't rise.
    let fog = smoke.look.w;
    // Volumes finer than half a meter hold small fires' thin wisps: smaller billows, and
    // erosion that leaves thinner smoke standing (it would eat a campfire's wisp whole).
    let fine = mix(clamp(cell / 0.5, 0.4, 1.0), 1.0, fog);
    let grain = mix(1.0, 5.0, fog) * fine;
    // Billows finer than a pixel only shimmer: fade the detail out with distance (the size of
    // a pixel where each step is, against the detail noise's ~1 m scale). Judged per step, not
    // where the ray enters the box: a big box's far side is many times farther, and billows
    // eroded there broke the thin edge of a plume into a spray of single-pixel flecks.
    let pixel_per_m = 2.0 / (frame.proj[1][1] * frame.viewport.y);
    // Fine billows drift with the wind and rise a little.
    let drift = smoke.drift.xyz + vec3<f32>(0.0, smoke.drift.w * (1.0 - fog), 0.0);
    let half_sigma = mix(2.0, 0.06, fog) * fine * fine;
    // Wind shear draws thin, drifting smoke out into streaks: its billows are looked up
    // stretched along the wind (anchored to the box, so they don't swim as the camera moves).
    // Fog is too slow and low to streak.
    let wind_flat = vec3<f32>(smoke.wind.x, 0.0, smoke.wind.z);
    let wind_dir = wind_flat / max(length(wind_flat), 1e-3);
    let streak = (1.0 - 1.0 / STREAK) * smoothstep(0.5, 2.0, length(wind_flat)) * (1.0 - fog);

    var transmittance = 1.0;
    var color = vec3(0.0);
    var t_weighted = 0.0;
    let spin = step_jitter(in.clip.xy, frame.time.w);
    var t = t0 + ds * spin;
    // How far the sun's shadow is spread in the smoke (m): about a cell, at least 40 cm.
    let shadow_blur = max(cell * 0.6, 0.4);
    for (var i = 0; i < steps; i++) {
        if (t > t1) { break; }
        let p = dir * t;
        var q = p;
        var lumps = 0.5;
        let detail = smoke.look.x * saturate(1.5 - pixel_per_m * max(t, 1.0) / (0.1 * grain));
        if (detail > 0.0) {
            // Warp the lookup so the voxels' smooth shapes break up into billows.
            let wn = textureSampleLevel(detail_tex, linear_repeat, (tiling_pos(p) - drift) / (WARP_TILE * grain), 0.0);
            lumps = wn.r;
            q = p + (wn.gba - 0.5) * (cell * 2.4 * detail);
        }
        let local = q - bmin;
        let uvw = local / smoke.size.xyz;
        // Light reaching into the creases between billows (finer than the light texture).
        var crease = 1.0;
        var s = mix(
            textureSampleLevel(density_prev, linear_clamp, uvw, 0.0),
            textureSampleLevel(density_cur, linear_clamp, uvw, 0.0),
            smoke.size.w,
        );
        if (s.x > 1e-4) {
            // Fade toward the open sides and top so the box never shows: over a few cells, along
            // a ragged line (by the billow noise), so a plume drifting out of the box dissolves
            // instead of ending in a straight wall.
            let edge = min(
                min(min(local.x, smoke.size.x - local.x), min(local.z, smoke.size.z - local.z)),
                smoke.size.y - local.y,
            );
            var k = saturate((edge - cell * 3.0 * (lumps - 0.5)) / (cell * 5.0));
            if (detail > 0.0) {
                // Erode: where the smoke is thin it breaks into round billows and wisps; dense
                // cores keep their shape. `d` is how dense it is (σ = 2/m is halfway, less in
                // fine volumes; for fog 0.06/m); `gaps` is high between billows; big lumps
                // (`lumps`) erode less.
                let d = s.x / (s.x + half_sigma);
                // Thin smoke breaks up at the coarse octaves only, into streaks: the fine
                // octaves' peaks left a spray of single specks where a plume had thinned out.
                // Thick smoke keeps its round billows. The streaks are a second lookup,
                // stretched along the wind, blended in by thinness: blending the lookup's
                // position instead jumped it by meters wherever the thinness changed, far
                // from the box's corner (orange speckle all over a thin haze).
                let thin = 1.0 - smoothstep(0.05, 0.3, d);
                let billow_uv = (tiling_pos(p) - drift * 1.3) / (BILLOW_TILE * grain);
                let bn = textureSampleLevel(detail_tex, linear_repeat, billow_uv, 0.0);
                var torn = bn.g * 0.8 + bn.b * 0.2;
                if (thin > 0.0 && streak > 0.0) {
                    let along = wind_dir * (dot(local, wind_dir) * streak) / (BILLOW_TILE * grain);
                    let st = textureSampleLevel(detail_tex, linear_repeat, billow_uv - along, 0.0);
                    torn = st.g * 0.8 + st.b * 0.2;
                }
                let gaps = 1.0 - mix(bn.g * 0.5 + bn.b * 0.3 + bn.a * 0.2, torn, thin);
                // Thin smoke is eroded half as much: eaten down to the noise's peaks it was left
                // as loose round puffs hanging in the air, where a thinning plume is a veil.
                let e = saturate(gaps * 1.6 - 0.35) * mix(1.25, 0.7, lumps) * min(detail, 1.0) * mix(0.9, 0.55, fog) * sqrt(fine) * (1.0 - 0.5 * thin);
                k *= saturate((d - e) / max(1.0 - e, 1e-3)) / max(d, 1e-4);
                crease = mix(1.0, mix(0.5, 1.15, 1.0 - gaps), min(detail, 1.0));
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
            // What a thick cloud of it looks like, and what its particles scatter.
            let tint = saturate3(s.yzw / sigma);
            let albedo = particle_albedo(tint);
            let light = textureSampleLevel(light_tex, linear_clamp, uvw, 0.0);
            var inscatter = vec3(0.0);
            if (sun_on) {
                // Light scattered once (mostly forward, so smoke glows toward the sun) and a
                // few times more. Each later order bounced inside the smoke, so it is weaker,
                // reaches deeper and spreads wider: deep inside a thick white cloud the light
                // still glows through, while dark smoke absorbs it before it gets there.
                var sun = vec3(0.0);
                var w = tint;
                var reach = 1.0;
                var g = 0.65;
                let shade = soft_sun_shadow(p, spin + f32(i) * 0.618034, shadow_blur);
                for (var o = 0; o < 4; o++) {
                    // Light that has bounced around in the smoke also came from its sunlit
                    // parts beside a shadow, so the shadow takes less of it.
                    let shaded = select(mix(1.0, shade, 0.45), shade, o == 0);
                    sun += w * smoke_phase(cos_t, g) * exp(-light.x * reach) * shaded;
                    w *= albedo * 0.8;
                    reach *= 0.3;
                    g *= 0.45;
                }
                inscatter += sun * sun_light;
            }
            // Sky and ground light, also partly scattered on through the smoke.
            let ms = 0.6 * albedo * albedo;
            var sky = 0.35 * amb_up * (exp(-light.y) + ms * exp(-light.y * 0.15));
            sky += 0.4 * amb_side * (exp(-light.w) + ms * exp(-light.w * 0.15));
            sky += 0.25 * amb_down * (exp(-light.z * 0.6) + ms * exp(-light.z * 0.12));
            let glow = textureSampleLevel(glow_tex, linear_clamp, uvw, 0.0).rgb * smoke.dims.w;
            inscatter = (inscatter + tint * sky) * crease + tint * glow * (0.25 * INV_PI);
            let step_t = exp(-sigma * ds);
            let absorbed = transmittance * (1.0 - step_t);
            color += inscatter * absorbed;
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
    let veil = surface_fog(rel_mid);
    let lit = pre_expose(color) * veil.transmittance + veil.inscatter * alpha;
    return vec4<f32>(max(lit, vec3(0.0)), alpha);
}

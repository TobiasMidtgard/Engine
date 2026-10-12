// Soft-body surfaces: cloth, soft volumes and rope tubes. Requires lib_types + lib_frame.
//
// Vertices arrive camera-relative and already deformed (the CPU interpolates the simulation
// between fixed steps). The fragment stage adds what the simulation can't afford at
// vertex resolution: printed patterns, fine wrinkles where the solver reports compression,
// fabric sheen, charring and glowing burn edges, and wetness.

struct SoftMaterial {
    color: vec4<f32>,     // rgb, alpha
    color2: vec4<f32>,    // rgb, wrinkle strength
    color3: vec4<f32>,    // rgb, wetness
    emissive: vec4<f32>,  // rgb (cd/m^2), sheen
    params: vec4<f32>,    // roughness, metallic, subsurface, unused
    scale: vec4<f32>,     // meters per uv unit (u, v), unused, unused
    pattern: vec4<u32>,   // id, parameter, unused, unused
};

@group(1) @binding(0) var<storage, read> materials: array<SoftMaterial>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tension: vec2<f32>,  // strain along u and v (negative = compressed)
    @location(4) burn: vec2<f32>,     // char, glow
    @location(5) material: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tension: vec2<f32>,
    @location(4) burn: vec2<f32>,
    @location(5) @interpolate(flat) material: u32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.rel = in.position;
    out.normal = in.normal;
    out.uv = in.uv;
    out.tension = in.tension;
    out.burn = in.burn;
    out.material = in.material;
    out.clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    return out;
}

// Anti-aliased step: 0 below `edge`, 1 above, blended over one pixel.
fn aa_step(edge: f32, x: f32) -> f32 {
    let w = max(fwidth(x), 1e-5);
    return saturate((x - edge) / w + 0.5);
}

fn pick3(i: u32, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    if (i == 0u) { return a; }
    if (i == 1u) { return b; }
    return c;
}

fn bands(x: f32, n: u32, m: SoftMaterial) -> vec3<f32> {
    let k = u32(floor(clamp(x, 0.0, 0.9999) * f32(n)));
    let colors = select(2u, 3u, n % 3u == 0u);
    // Blend across band edges for anti-aliasing.
    let f = fract(clamp(x, 0.0, 0.9999) * f32(n));
    let w = fwidth(x * f32(n));
    let c0 = pick3(k % colors, m.color.rgb, m.color2.rgb, m.color3.rgb);
    let c1 = pick3((k + 1u) % colors, m.color.rgb, m.color2.rgb, m.color3.rgb);
    return mix(c0, c1, saturate((f - 1.0) / max(w, 1e-5) + 0.5) * select(0.0, 1.0, k + 1u < n));
}

// Printed pattern color at `uv` (0..1 over the cloth).
fn pattern_color(m: SoftMaterial, uv: vec2<f32>) -> vec3<f32> {
    let id = m.pattern.x;
    let n = max(m.pattern.y, 1u);
    let aspect = m.scale.x / max(m.scale.y, 1e-4);
    if (id == 1u) { return bands(uv.y, n, m); }
    if (id == 2u) { return bands(uv.x, n, m); }
    if (id == 3u) {
        // Nordic cross, offset toward the hoist, outlined.
        let p = vec2<f32>(uv.x * aspect, uv.y);
        let cx = 0.36 * aspect;
        let d = min(abs(p.x - cx), abs(p.y - 0.5));
        let outline = 1.0 - aa_step(0.13, d);
        let inner = 1.0 - aa_step(0.075, d);
        return mix(mix(m.color.rgb, m.color3.rgb, outline), m.color2.rgb, inner);
    }
    if (id == 4u) {
        let p = vec2<f32>((uv.x - 0.5) * aspect, uv.y - 0.5);
        return mix(m.color.rgb, m.color2.rgb, 1.0 - aa_step(0.3, length(p)));
    }
    if (id == 5u) {
        let c = (u32(floor(uv.x * f32(n))) + u32(floor(uv.y * f32(n)))) % 2u;
        return select(m.color.rgb, m.color2.rgb, c == 1u);
    }
    if (id == 6u) {
        let d = min(min(uv.x, 1.0 - uv.x) * m.scale.x, min(uv.y, 1.0 - uv.y) * m.scale.y);
        let trim = 1.0 - aa_step(0.06, d);
        let piping = (1.0 - aa_step(0.085, d)) * aa_step(0.07, d);
        return mix(mix(m.color.rgb, m.color2.rgb, trim), m.color3.rgb, piping);
    }
    if (id == 7u) {
        return mix(m.color.rgb, m.color2.rgb, 1.0 - aa_step(0.12, abs(uv.x - (1.0 - uv.y))));
    }
    if (id == 8u) {
        // Twisted rope: three plies spiraling along the rope (u around, v along in meters).
        let s = fract(uv.x * 3.0 + uv.y * 8.0);
        let groove = smoothstep(0.0, 0.18, s) * smoothstep(1.0, 0.82, s);
        return m.color.rgb * mix(0.55, 1.0, groove);
    }
    return m.color.rgb;
}

// Cotangent frame from screen derivatives (no tangents needed). Returns T, B.
fn cotangent_frame(n: vec3<f32>, rel: vec3<f32>, uv: vec2<f32>) -> mat2x3<f32> {
    let dp1 = dpdx(rel);
    let dp2 = dpdy(rel);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-24));
    return mat2x3<f32>(t * inv, b * inv);
}

// Normalize, or `fallback` for a (near-)zero vector. Interpolated directions can cancel out,
// and one NaN pixel turns into a white bloom spot.
fn safe_normalize(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    return select(fallback, v * inverseSqrt(l2), l2 > 1e-20);
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let m = materials[in.material];
    var n = safe_normalize(in.normal, safe_normalize(-in.rel, vec3(0.0, 1.0, 0.0)));
    if (!front) { n = -n; }

    // Fine wrinkles where the cloth is compressed: ridges run across the squeeze direction.
    // Built from uv in meters so the wavelength (~3.5 cm) doesn't depend on the cloth size.
    let tb = cotangent_frame(n, in.rel, in.uv);
    let um = in.uv * m.scale.xy;
    let squeeze = max(-in.tension, vec2(0.0));
    let amount = m.color2.w * clamp(squeeze * 6.0, vec2(0.0), vec2(1.0));
    let warp = value_noise2(um * 4.0) * 6.0;
    let ku = 6.2831 / 0.035;
    let du = cos(um.x * ku + warp) * amount.x;
    let dv = cos(um.y * ku + warp * 1.3) * amount.y;
    n = normalize(n - (tb[0] * du + tb[1] * dv) * 0.35);

    var albedo = pattern_color(m, in.uv);
    var roughness = m.params.x;
    // Wet fabric darkens and turns a little glossy.
    let wet = m.color3.w;
    albedo *= mix(1.0, 0.55, wet);
    roughness = mix(roughness, 0.35, wet * 0.7);
    // Charring.
    let char = saturate(in.burn.x);
    albedo = mix(albedo, vec3<f32>(0.025, 0.02, 0.018), smoothstep(0.1, 0.9, char));

    var s = default_surface();
    s.albedo = albedo;
    s.roughness = roughness;
    s.metallic = m.params.y;
    s.subsurface = m.params.z;
    s.normal = n;
    let flicker = 0.75 + 0.25 * sin(frame.time.x * 23.0 + in.uv.x * 40.0) * sin(frame.time.x * 17.0 + in.uv.y * 31.0);
    s.emissive = m.emissive.rgb + vec3<f32>(1.0, 0.36, 0.06) * (in.burn.y * in.burn.y * 40000.0 * flicker);

    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    var color = shade_surface(s, in.rel);

    // Fabric sheen: a soft rim brightening at grazing angles (velvet, satin).
    let v = safe_normalize(-in.rel, n);
    let sheen = m.emissive.w;
    if (sheen > 0.0) {
        let ndv = saturate(dot(n, v));
        let rim = pow(1.0 - ndv, 4.0);
        let l = frame.sun_dir.xyz;
        let sun = frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, n);
        let wrap = saturate(dot(n, l) * 0.5 + 0.5);
        let sky = sky_ambient(n) * sky_visibility(in.rel, n) * PI;
        color += pre_expose((sun * wrap + sky) * albedo * rim * sheen * 0.25 * INV_PI);
    }

    var alpha = m.color.a;
    if (alpha < 0.999) {
        // Translucent bodies look denser at grazing angles (Fresnel).
        let f = pow(1.0 - saturate(dot(n, v)), 3.0);
        alpha = mix(alpha, 1.0, f * 0.8);
    }
    color = apply_fog(color, in.rel);
    return vec4<f32>(color, alpha);
}

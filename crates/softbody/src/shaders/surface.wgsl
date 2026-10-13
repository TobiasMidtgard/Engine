// Soft-body surfaces: cloth, soft volumes and rope tubes. Requires lib_types + lib_frame +
// surface_common.
//
// Vertices arrive camera-relative and already deformed (the CPU interpolates the simulation
// between fixed steps). The fragment stage adds what the simulation can't afford at
// vertex resolution: printed patterns, fine wrinkles where the solver reports compression,
// fabric sheen, charring and glowing burn edges, and wetness.

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tension: vec2<f32>,  // strain along u and v (negative = compressed)
    @location(4) burn: vec2<f32>,     // burn progress, burning
    @location(5) material: u32,
    @location(6) wet: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tension: vec2<f32>,
    @location(4) burn: vec2<f32>,
    @location(5) @interpolate(flat) material: u32,
    @location(6) wet: f32,
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
    out.wet = in.wet;
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

// Seigaiha ("blue sea waves"): rows of fans of concentric arcs, each row overlapping the one
// above it. `p` in fan radii: fans are 2 apart along a row, rows 0.5 apart, every other row
// shifted by 1. Returns how much line covers `p` (0..1).
fn seigaiha(p: vec2<f32>) -> f32 {
    let j0 = i32(floor(p.y * 2.0)) - 2;
    var r = 0.0;
    // The lowest row whose fan holds `p` is drawn over the others.
    for (var j = j0; j <= j0 + 4; j++) {
        let off = f32(j & 1);
        let c = vec2(round((p.x - off) * 0.5) * 2.0 + off, f32(j) * 0.5);
        let d = length(p - c);
        if (d < 1.0) {
            r = d;
            break;
        }
    }
    // Four arcs to a fan, and a solid dot at its heart.
    let rings = r * 4.0;
    let w = max(fwidth(rings), 1e-4);
    let line = 1.0 - smoothstep(0.16 - w, 0.16 + w, abs(rings - round(rings)));
    return max(select(line, 0.0, rings < 0.5), 1.0 - smoothstep(0.42 - w, 0.42 + w, rings));
}

// Plum blossoms scattered over cells of `p` (one cell per blossom spacing), some cells bare:
// five round petals around a heart. Returns (petal, heart) cover.
fn blossoms(p: vec2<f32>) -> vec2<f32> {
    var petal = 0.0;
    var heart = 0.0;
    let base = floor(p);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let cell = base + vec2(f32(x), f32(y));
            let h = hash12(cell * 1.37 + 0.5);
            if (h < 0.35) { continue; }
            let c = cell + 0.5 + (vec2(hash12(cell + 7.1), hash12(cell + 3.3)) - 0.5) * 0.6;
            let size = mix(0.16, 0.3, hash12(cell + 11.7));
            let spin = hash12(cell + 5.9) * 6.2832;
            let q = (p - c) / size;
            let w = max(fwidth(q.x), 1e-4);
            var d = length(q) - 0.45;
            for (var k = 0; k < 5; k++) {
                let a = spin + f32(k) * 1.2566;
                d = min(d, length(q - vec2(cos(a), sin(a)) * 0.55) - 0.42);
            }
            // Each blossom covers the hearts of those behind it.
            petal = max(petal, 1.0 - smoothstep(-w, w, d));
            heart = max(heart * (1.0 - smoothstep(-w, w, d)), 1.0 - smoothstep(0.2 - w, 0.2 + w, length(q)));
        }
    }
    return vec2(petal, heart);
}

// Kanoko shibori (tie-dye dots): rows of small squarish rings, every other row shifted, each
// a little uneven where the cloth was bound. `p` in dot spacings. Returns ring cover.
fn shibori(p: vec2<f32>) -> f32 {
    let row = floor(p.y);
    let q = vec2(p.x + 0.5 * (row - 2.0 * floor(row * 0.5)), p.y);
    let cell = floor(q);
    let f = fract(q) - 0.5 + (vec2(hash12(cell + 1.3), hash12(cell + 9.1)) - 0.5) * 0.08;
    // Rounded square: the binding thread pinched the cloth into a little tent.
    let d = pow(pow(abs(f.x), 3.0) + pow(abs(f.y), 3.0), 1.0 / 3.0);
    let w = max(fwidth(p.x), 1e-4) * 1.5;
    let size = mix(0.2, 0.25, hash12(cell + 4.4));
    let ring = smoothstep(size * 0.55 - w, size * 0.55 + w, d) * (1.0 - smoothstep(size - w, size + w * 3.0, d));
    return ring;
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
    // Dyed patterns sized in millimeters (`n`) over the cloth in meters.
    let pm = uv * m.scale.xy / (f32(n) * 0.001);
    if (id == 9u) {
        return mix(m.color.rgb, m.color2.rgb, seigaiha(pm * 2.0));
    }
    if (id == 10u) {
        let b = blossoms(pm);
        return mix(mix(m.color.rgb, m.color2.rgb, b.x), m.color3.rgb, b.y);
    }
    if (id == 11u) {
        return mix(m.color.rgb, m.color2.rgb, shibori(pm));
    }
    return m.color.rgb;
}

// Woven fabric at fabric-space position `pm` (meters): returns the height gradient along
// u and v (x, y), an albedo factor (z) and a roughness offset (w). Three scales, each faded
// out before it gets smaller than a couple of pixels so nothing shimmers:
// - plain-weave threads (over-under, ~1.3 mm pitch) close up,
// - yarn irregularity (slubs: thicker, lighter runs along warp and weft) at mid range,
// - a folded hem with a stitch line along the cloth's edges.
fn weave(pm: vec2<f32>, uv: vec2<f32>, size: vec2<f32>) -> vec4<f32> {
    var grad = vec2(0.0);
    var tone = 1.0;
    var rough = 0.0;
    let pitch = 0.0013;
    let g = pm / pitch;
    let px = max(length(fwidth(g)), 1e-4);
    let threads = 1.0 - smoothstep(0.3, 0.6, px);
    if (threads > 0.0) {
        let cell = floor(g);
        let f = fract(g) - 0.5;
        let warp_on_top = ((i32(cell.x) + i32(cell.y)) & 1) == 0;
        // The top thread is a rounded ridge; the gap between threads dips.
        let ridge = select(vec2(0.0, -sin(f.y * PI)), vec2(-sin(f.x * PI), 0.0), warp_on_top);
        // It also arches over the crossing thread along its length.
        let arch = select(vec2(-sin(f.x * PI) * 0.4, 0.0), vec2(0.0, -sin(f.y * PI) * 0.4), warp_on_top);
        grad += (ridge + arch) * 0.6 * threads;
        tone *= 1.0 - 0.12 * threads * (1.0 - abs(cos(select(f.y, f.x, warp_on_top) * PI)));
    }
    // Slubs: yarn thickness varies along each thread.
    let yarn = 1.0 - smoothstep(0.03, 0.12, length(fwidth(pm)) * 60.0);
    let slub = (value_noise2(vec2(pm.x * 900.0, pm.y * 25.0)) + value_noise2(vec2(pm.x * 25.0, pm.y * 900.0)) - 1.0) * yarn;
    let mottle = fbm2(pm * 18.0, 3) - 0.5;
    tone *= 1.0 + 0.08 * slub + 0.1 * mottle;
    rough += 0.06 * mottle;
    // Hem: a 2.5 cm fold along every edge, slightly raised and darker, with stitches.
    let edge = min(min(uv.x, 1.0 - uv.x) * size.x, min(uv.y, 1.0 - uv.y) * size.y);
    let hem = 1.0 - smoothstep(0.022, 0.028, edge);
    if (hem > 0.0) {
        let fold = smoothstep(0.0, 0.006, edge) * (1.0 - smoothstep(0.02, 0.028, edge));
        tone *= mix(1.0, 0.86, hem) * mix(1.0, 1.04, fold);
        let along = select(pm.y, pm.x, min(uv.y, 1.0 - uv.y) * size.y < min(uv.x, 1.0 - uv.x) * size.x);
        let stitch = smoothstep(0.0015, 0.0, abs(edge - 0.018)) * step(0.35, fract(along / 0.004));
        let stitch_px = 1.0 - smoothstep(0.5, 1.5, length(fwidth(pm)) / 0.002);
        tone *= 1.0 - 0.25 * stitch * stitch_px;
    }
    return vec4(grad, tone, rough);
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
    // A see-through solid shows only its outside: its far side would read as the inner
    // walls of a hollow glass box. The color from inside is scattered light (below).
    if (!front && m.color.a < 0.999 && m.params.z > 0.5) { discard; }
    let dmg = damage(m, in.uv, in.burn);
    if (dmg.cut > 0.0) { discard; }
    var n = safe_normalize(in.normal, safe_normalize(-in.rel, vec3(0.0, 1.0, 0.0)));
    if (!front) { n = -n; }
    let n_geo = n;

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
    if (m.params.w > 0.0) {
        let fabric = weave(um, in.uv, m.scale.xy);
        n = normalize(n - (tb[0] * fabric.x + tb[1] * fabric.y) * m.params.w);
        albedo *= fabric.z;
        roughness = saturate(roughness + fabric.w);
    }
    // Wet fabric darkens and deepens in color (water fills the gaps between the fibers, so
    // less light scatters back out), turns glossy, and its weave and fuzz lie flat.
    let wet = max(m.color3.w, in.wet);
    albedo = mix(albedo, pow(albedo, vec3(1.35)) * 0.7, wet);
    roughness = mix(roughness, 0.3, wet * 0.75);
    n = normalize(mix(n, n_geo, wet * 0.5));
    // Scorch, then char toward a burn front or a bullet hole.
    albedo = mix(albedo, albedo * vec3<f32>(0.42, 0.3, 0.2), dmg.scorch * 0.8);
    albedo = mix(albedo, vec3<f32>(0.025, 0.02, 0.018), dmg.char);
    roughness = mix(roughness, 1.0, max(dmg.char, dmg.scorch * 0.5));

    // Translucent, scattering bodies (jelly): the surface mostly reflects; the color comes
    // from light scattered inside (added below).
    let jelly = m.color.a < 0.999 && m.params.z > 0.5;
    var s = default_surface();
    s.albedo = select(albedo, albedo * 0.12, jelly);
    s.roughness = roughness;
    s.metallic = m.params.y;
    s.subsurface = m.params.z;
    if (m.params.w > 0.0) {
        s.reflectance = 0.35;
    }
    s.normal = n;
    let flicker = 0.75 + 0.25 * sin(frame.time.x * 23.0 + in.uv.x * 40.0) * sin(frame.time.x * 17.0 + in.uv.y * 31.0);
    let ember = dmg.glow * dmg.glow * (0.6 + 0.8 * value_noise2(in.uv * m.scale.xy * 40.0 + frame.time.x * 3.0));
    s.emissive = m.emissive.rgb + vec3<f32>(1.0, 0.36, 0.06) * (ember * 60000.0 * flicker);

    if (frame.counts.z == 1u) {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    var color = shade_surface(s, in.rel);

    // Fabric sheen: a soft rim brightening at grazing angles (velvet, satin).
    let v = safe_normalize(-in.rel, n);
    // Fabric sheen: fibers standing out of the cloth scatter light forward, so fabric
    // brightens toward grazing angles instead of showing a sharp highlight ("Charlie" sheen,
    // Estevez & Kulla 2017, with Neubelt's visibility). Velvet pile is crushed in patches,
    // which shifts its sheen around.
    let sheen = m.emissive.w * (1.0 - 0.8 * wet);
    if (sheen > 0.0) {
        let ndv = max(dot(n, v), 1e-3);
        let l = frame.sun_dir.xyz;
        let ndl = saturate(dot(n, l));
        let h = safe_normalize(l + v, n);
        let ndh = saturate(dot(n, h));
        let a = mix(0.5, 0.3, saturate(sheen));
        let inv = 1.0 / a;
        let d = (2.0 + inv) * pow(max(1.0 - ndh * ndh, 1e-4), inv * 0.5) * 0.5 * INV_PI;
        let vis = 1.0 / (4.0 * (ndl + ndv - ndl * ndv) + 1e-3);
        let crush = select(1.0, 0.55 + 0.9 * fbm2(um * 6.0, 3), sheen > 0.6);
        let tint = mix(albedo, vec3(1.0), 0.25) * sheen * crush;
        let sun = frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, n);
        let sky = sky_ambient(n) * sky_visibility(in.rel, n) * PI;
        let rim = pow(1.0 - ndv, 3.0);
        color += pre_expose(tint * (sun * d * vis * ndl + sky * rim * 0.35 * INV_PI));
    }

    var alpha = m.color.a;
    if (jelly) {
        let ndv = saturate(dot(n, v));
        // Light crosses more jelly through the middle than at the rim: deeper, darker color
        // there (Beer-Lambert), clear and bright toward the edges.
        let tint = pow(max(albedo, vec3(1e-3)), vec3(0.7 + 1.9 * ndv));
        let l = frame.sun_dir.xyz;
        let sun = frame.sun_color.rgb * sun_transmittance(in.rel) * shadow_factor(in.rel, n);
        // Glows when the sun is behind it; wraps softly around the lit side.
        let through = pow(saturate(dot(v, -l) * 0.5 + 0.5), 3.0);
        let wrap = saturate(dot(n, l) * 0.5 + 0.5);
        let sky = sky_ambient(n) * sky_visibility(in.rel, n) * PI;
        let inner = (sun * (0.3 * wrap + 1.1 * through) + sky * 0.7) * tint * INV_PI;
        color += pre_expose(inner);
        // Thick middle hides more of what's behind; grazing angles reflect (Fresnel).
        let f = pow(1.0 - ndv, 4.0);
        alpha = max(mix(alpha * 0.75, min(alpha + 0.12, 1.0), ndv), f);
    } else if (alpha < 0.999) {
        // Translucent bodies look denser at grazing angles (Fresnel).
        let f = pow(1.0 - saturate(dot(n, v)), 3.0);
        alpha = mix(alpha, 1.0, f * 0.8);
    }
    color = apply_fog(color, in.rel);
    return vec4<f32>(color, alpha);
}

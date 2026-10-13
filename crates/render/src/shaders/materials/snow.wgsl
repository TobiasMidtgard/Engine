// Wind-packed snow: soft drifts, wind-carved sastrugi ridges with sharp windward edges, small
// pits and lumps, and a fine granular crust. Bright (albedo about 0.8-0.9), a little blue in
// the hollows, with tiny ice facets that sparkle (low roughness, tilted normals). 3 m tile,
// 6 cm of relief.

fn preset_layers(uv: vec2<f32>) -> Layers {
    let w = warp(uv, vec2<i32>(2), 4, 0.06, 1301u);
    let drifts = fbm1(w, 2, 5, 0.5, 1302u) * 14.0;
    // Sastrugi: elongated ridges along the wind (u), steep on one side.
    let sw = warp(uv, vec2<i32>(4), 3, 0.03, 1303u);
    let ridge_n = fbm(sw, vec2<i32>(3, 14), 4, 0.5, 1304u);
    let ridge = smoothstep(0.0, 0.5, ridge_n) * 8.0 - smoothstep(-0.2, 0.3, fbm(sw + vec2<f32>(0.0, 0.004), vec2<i32>(3, 14), 4, 0.5, 1304u)) * 2.0;
    // Lumps and pits.
    let lumps = fbm1(uv, 24, 4, 0.55, 1305u) * 2.0;
    let pit = blobs(uv, vec2<i32>(40), 0.1, 0.3, 1306u);
    let pits = select(0.0, pit.h * pit.h * 12.0, u01(pit.id) < 0.12);
    // Granular crust.
    let crust = fbm1(uv, 512, 2, 0.6, 1307u) * 0.35 + fbm1(uv, 128, 2, 0.5, 1308u) * 0.4;
    let h = clamp(28.0 + drifts + ridge + lumps - pits + crust, 1.0, 59.0);
    var l: Layers;
    l.a = vec4<f32>(h * 0.001, ridge, pits, 0.0);
    l.b = vec4<f32>(0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    // Fresh snow is very bright; hollows look bluer, older patches a little gray.
    var c = hex(0xeceef2u);
    c = mix(c, hex(0xd9e2eeu), saturate(d.curv_broad * 2.0) * 0.6);
    c *= 1.0 - 0.05 * smoothstep(0.0, 0.8, fbm1(uv, 3, 4, 0.5, 1321u));
    c *= 1.0 + 0.03 * fbm1(uv, 400, 2, 0.5, 1322u);
    // Sparkles: a few ice facets per square centimeter catch the sun.
    let g = vec2<i32>(floor(uv * f32(texels())));
    let hs = hash_u2(g, 1323u);
    let sparkle = select(0.0, 1.0, u01(hs) < 0.012);
    var n = d.normal;
    if (sparkle > 0.0) {
        n = normalize(n + vec3<f32>(rnd(hs, 1u), rnd(hs, 2u), 0.0) * 0.6);
    }
    var s: Pbr;
    s.albedo = c;
    s.roughness = mix(0.62 + 0.08 * fbm1(uv, 64, 3, 0.5, 1324u), 0.12, sparkle);
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = n;
    return s;
}

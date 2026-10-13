// Wind-rippled sand: ripples about 9 cm apart with gentle windward and steep lee slopes, crests
// that wander, fork and end; darker heavy minerals settle in the troughs, crests are paler and
// coarser; broad undulation underneath and a fine grain. 2 m tile, 3 cm of relief.

fn preset_layers(uv: vec2<f32>) -> Layers {
    // Ripple phase: 22 per tile across v, bent by warping; noise on the phase makes forks
    // (Y junctions) where the count of crests changes.
    let w = warp(uv, vec2<i32>(2), 4, 0.05, 1001u);
    let bend = fbm1(w, 3, 4, 0.5, 1002u) * 1.6 + fbm1(uv, 9, 3, 0.5, 1003u) * 0.35;
    let phase = w.y * 22.0 + bend;
    let f = fract(phase);
    // Asymmetric profile: slow rise (stoss) over 75% of the wavelength, steep fall (lee).
    let rise = smoothstep(0.0, 0.75, f);
    let fall = 1.0 - smoothstep(0.75, 1.0, f);
    var ripple = select(fall, rise, f < 0.75);
    ripple = ripple * ripple * (3.0 - 2.0 * ripple);
    // Ripple height varies along the crest and dies out in places.
    let amp = 6.0 * smoothstep(-0.6, 0.3, fbm1(uv, 4, 3, 0.5, 1004u)) + 1.5;
    let dunes = fbm1(uv, 2, 4, 0.5, 1005u) * 7.0;
    let grain = fbm1(uv, 384, 2, 0.6, 1006u) * 0.25 + fbm1(uv, 96, 2, 0.5, 1007u) * 0.3;
    let h = 13.0 + dunes + amp * ripple + grain;

    var l: Layers;
    l.a = vec4<f32>(h * 0.001, ripple, f, amp);
    l.b = vec4<f32>(0.0);
    return l;
}

fn preset_refine(uv: vec2<f32>) -> Layers {
    return layers_at(uv);
}

fn preset_surface(uv: vec2<f32>, l: Layers, d: Derived) -> Pbr {
    let ripple = l.a.y;
    // Warm quartz sand, paler on the crests, darker minerals in the troughs.
    var c = hex(0xcbab7cu);
    c *= 1.0 + 0.06 * fbm1(uv, 3, 4, 0.5, 1021u);
    let trough = 1.0 - smoothstep(0.0, 0.35, ripple);
    c = mix(c, hex(0x9a8262u), trough * 0.55 * smoothstep(-0.4, 0.4, fbm1(uv, 12, 3, 0.5, 1022u)));
    c = mix(c, c * 1.08, smoothstep(0.7, 1.0, ripple));
    // Individual grains: dark and light specks.
    let speck = fbm1(uv, 768, 1, 0.5, 1023u);
    c *= 1.0 + 0.22 * speck;
    c = mix(c, hex(0x4a4038u), smoothstep(0.55, 0.8, fbm1(uv, 512, 1, 0.5, 1024u)) * 0.35);

    var s: Pbr;
    s.albedo = c;
    s.roughness = 0.88 + 0.05 * speck;
    s.metallic = 0.0;
    s.ao = 1.0;
    s.normal = d.normal;
    return s;
}

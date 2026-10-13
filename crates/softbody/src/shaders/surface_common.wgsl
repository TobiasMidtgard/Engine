// Shared by the soft-body surface color and shadow passes: materials, bullet holes and the
// burn front. Requires lib_types.

struct SoftMaterial {
    color: vec4<f32>,     // rgb, alpha
    color2: vec4<f32>,    // rgb, wrinkle strength
    color3: vec4<f32>,    // rgb, wetness
    emissive: vec4<f32>,  // rgb (cd/m^2), sheen
    params: vec4<f32>,    // roughness, metallic, subsurface, weave strength
    scale: vec4<f32>,     // meters per uv unit (u, v), unused, unused
    pattern: vec4<u32>,   // id, parameter, first hole, hole count
};

@group(1) @binding(0) var<storage, read> materials: array<SoftMaterial>;
// Bullet holes: uv, radius (m), seed.
@group(1) @binding(1) var<storage, read> holes: array<vec4<f32>>;

struct Damage {
    // > 0: no fabric here (discard).
    cut: f32,
    // 0 clean .. 1 charred black.
    char: f32,
    // Glowing ember edge, 0..1.
    glow: f32,
    // Powder burn and frayed threads around bullet holes, 0..1.
    scorch: f32,
};

// Holes and the burn front at surface coordinate `uv`. `burn` is (progress, burning):
// progress is interpolated from the particles, so noise in fabric space turns the
// triangle-sized burn state into a ragged, curling front with a glowing rim.
fn damage(m: SoftMaterial, uv: vec2<f32>, burn: vec2<f32>) -> Damage {
    var d = Damage(0.0, 0.0, 0.0, 0.0);
    let pm = uv * m.scale.xy; // fabric space, meters
    let first = m.pattern.z;
    let count = m.pattern.w;
    for (var i = 0u; i < count; i++) {
        let h = holes[first + i];
        let off = (uv - h.xy) * m.scale.xy;
        let dist = length(off);
        if (dist > h.z * 5.0) { continue; }
        // Ragged outline: a few lobes plus fine fiber noise.
        let a = atan2(off.y, off.x);
        let lobes = value_noise2(vec2(a * 1.6 + h.w * 40.0, h.w * 13.0)) - 0.5;
        let fiber = value_noise2(pm * 900.0 + h.w * 7.0) - 0.5;
        let r = h.z * (1.0 + 0.55 * lobes) + h.z * 0.35 * fiber;
        d.cut = max(d.cut, r - dist);
        // Powder burn fading out over a few radii, threads pulled at the rim.
        let ring = 1.0 - smoothstep(r, r * 3.2, dist);
        d.scorch = max(d.scorch, ring * ring * (0.8 + 0.4 * fiber));
        d.char = max(d.char, (1.0 - smoothstep(r, r * 1.5, dist)) * 0.7);
    }
    if (burn.x > 0.0) {
        let n = fbm2(pm * 7.0, 4) - 0.5;
        let fine = value_noise2(pm * 60.0) - 0.5;
        let f = burn.x + n * 0.45 + fine * 0.08;
        d.cut = max(d.cut, f - 0.95);
        d.char = max(d.char, smoothstep(0.3, 0.85, f));
        // The rim just before the edge glows while the cloth burns.
        d.glow = smoothstep(0.62, 0.93, f) * burn.y;
        // Scorched brown band ahead of the char.
        d.scorch = max(d.scorch, smoothstep(0.03, 0.4, f));
    }
    return d;
}

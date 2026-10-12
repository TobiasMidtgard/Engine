// Scorch and coals: a screen-space decal over the fire field. For every pixel, rebuild its
// position from scene depth, look it up in the fire map (scorch, coal glow, flames, ground
// height) and darken it to char, with glowing coals and a bright flame base on top.
// Premultiplied: color = emission + scene × (1 - alpha). Requires lib_types + lib_frame.

struct ScorchParams {
    rect: vec4<f32>,   // camera-relative x, z of the map's min corner; size x, z (m)
    ground: vec4<f32>, // camera-relative y of the map's base height, texel size (m), time, 0
};

@group(1) @binding(0) var<uniform> params: ScorchParams;
@group(1) @binding(1) var fire_map: texture_2d<f32>;
@group(1) @binding(2) var scene_depth: texture_depth_2d;

@vertex
fn vs_scorch(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn ember_color(kelvin: f32) -> vec3<f32> {
    let t = clamp(kelvin, 800.0, 3000.0) / 100.0;
    let g = clamp(0.39008157876 * log(t) - 0.63184144378, 0.0, 1.0);
    let c = pow(vec3<f32>(1.0, g, 0.0), vec3<f32>(2.2));
    return c / max(luminance(c), 1e-4);
}

@fragment
fn fs_scorch(in: FullscreenOut) -> @location(0) vec4<f32> {
    let depth = textureLoad(scene_depth, vec2<i32>(in.position.xy), 0);
    if (depth <= 0.0) { discard; }
    let ndc = vec4<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0, depth, 1.0);
    let wp = frame.inv_view_proj * ndc;
    let rel = wp.xyz / wp.w;
    let uv = (rel.xz - params.rect.xy) / params.rect.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { discard; }
    let m = textureSampleLevel(fire_map, linear_clamp, uv, 0.0);
    let scorch = m.r;
    let glow = m.g;
    let flame = m.b;
    if (scorch <= 0.002 && glow <= 0.002 && flame <= 0.002) { discard; }
    let ground = params.ground.x + m.a;
    let h = rel.y - ground;
    if (h < -1.5 || h > 3.5) { discard; }
    // Char reaches up stalks and trunks, strongest at the ground.
    let height_fade = 1.0 - smoothstep(0.6, 3.5, h);
    let tp = tiling_pos(rel);
    let mottle = fbm2(tp.xz * 0.9, 4);
    let ash = smoothstep(0.55, 0.85, value_noise2(tp.xz * 3.1));
    // Burnt ground: black char with grey ash flecks.
    var alpha = scorch * mix(0.78, 0.95, mottle) * height_fade;
    var emission = vec3<f32>(0.0);
    // Coals: a crackle pattern of glowing cracks that slowly breathes.
    let t = params.ground.z;
    let crack = fbm2(tp.xz * 2.3 + vec2<f32>(t * 0.05, -t * 0.03), 4);
    let cracks = smoothstep(0.5, 0.75, crack) * (0.7 + 0.3 * sin(t * 1.7 + crack * 20.0));
    let coal = glow * cracks * (1.0 - smoothstep(0.0, 0.6, h));
    emission += ember_color(900.0 + 300.0 * coal) * 3500.0 * coal * coal;
    // Flame base: a bright band where flames touch the fuel.
    let base = flame * (1.0 - smoothstep(0.0, 0.8, h)) * (0.6 + 0.4 * mottle);
    emission += ember_color(1300.0) * 6000.0 * base;
    // Ash shows as lighter flecks on fully burnt ground.
    let ash_light = ash * scorch * (1.0 - glow) * 0.12;
    alpha = saturate(alpha - ash_light);
    return vec4<f32>(pre_expose(emission), alpha);
}

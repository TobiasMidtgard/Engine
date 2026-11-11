// Lamp tiles: which lamps can reach each 16-pixel tile of the screen, worked out once after the
// prepass, for the lamp loop in shading to go through only those (`lamp_mask` in lib_frame.wgsl;
// `RenderSettings::trial_cuts` bit `TILED_LAMPS`). Requires lib_types + lib_frame. Group 0 is
// the screen light pass's frame group: the prepass's depth bound, the tiles a stand-in.
//
// Why: every pixel's shader went through every lamp in view (61 at the plaza, 153 at the
// overview), two to three times a pixel where triangles are small, to find the few in reach.
//
// A tile keeps the lamps whose sphere of reach crosses its four sides and begins nearer than
// the farthest surface the prepass drew in it. Whatever else is drawn in the tile later (glass,
// smoke, water) lies in front of that surface, so it is covered too. A tile with sky in it
// reaches to infinity. One fragment a tile writes lamps 0-127 to layer 0, 128-255 to layer 1.

struct TilesOut {
    @location(0) lo: vec4<u32>,
    @location(1) hi: vec4<u32>,
};

@vertex
fn vs_tiles(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

// The plane through `a`, `b` and `c`, facing `inside`: (normal, offset).
fn tile_side(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, inside: vec3<f32>) -> vec4<f32> {
    var n = normalize(cross(b - a, c - a));
    if (dot(n, inside - a) < 0.0) { n = -n; }
    return vec4<f32>(n, -dot(n, a));
}

@fragment
fn fs_tiles(in: FullscreenOut) -> TilesOut {
    let tile = vec2<i32>(floor(in.position.xy));
    let size = vec2<i32>(textureDimensions(prepass_depth));
    let lo = tile * LAMP_TILE_PX;
    let hi = min(lo + vec2<i32>(LAMP_TILE_PX), size);

    // The farthest surface in the tile: the smallest depth (reverse Z), 0 where nothing is.
    // Always 16 by 16 loads, clamped to the image (an edge tile reads its last pixels again):
    // loop ends that differ between the pixels of a block never ended on lavapipe at 160x120.
    var far_z = 1.0;
    let last = size - vec2<i32>(1);
    for (var y = 0; y < LAMP_TILE_PX; y++) {
        for (var x = 0; x < LAMP_TILE_PX; x++) {
            let at = clamp(lo + vec2<i32>(x, y), vec2<i32>(0), last);
            far_z = min(far_z, textureLoad(prepass_depth, at, 0));
        }
    }
    var far = 3.0e38;
    if (far_z > 0.0) {
        far = scene_linear_depth(vec2<f32>(0.5), far_z);
    }

    // The tile's four sides, a pixel wider all round for the jitter, each through its edge on
    // the near plane and a point farther out (so an orthographic view works too).
    let a = (vec2<f32>(lo) - 1.0) * frame.viewport.zw;
    let b = (vec2<f32>(hi) + 1.0) * frame.viewport.zw;
    let n00 = view_position(vec2<f32>(a.x, a.y), 1.0);
    let n10 = view_position(vec2<f32>(b.x, a.y), 1.0);
    let n11 = view_position(vec2<f32>(b.x, b.y), 1.0);
    let n01 = view_position(vec2<f32>(a.x, b.y), 1.0);
    let f00 = view_position(vec2<f32>(a.x, a.y), 0.5);
    let f11 = view_position(vec2<f32>(b.x, b.y), 0.5);
    let inside = view_position(0.5 * (a + b), 0.75);
    let top = tile_side(n00, n10, f00, inside);
    let left = tile_side(n00, n01, f00, inside);
    let bottom = tile_side(n11, n01, f11, inside);
    let right = tile_side(n11, n10, f11, inside);

    var bits: array<u32, 8>;
    let count = min(u32(frame.counts.x), LAMP_TILE_MAX);
    for (var i = 0u; i < count; i++) {
        let light = point_lights[i];
        let c = (frame.view * vec4<f32>(light.position_range.xyz, 1.0)).xyz;
        let r = light.position_range.w;
        if (-c.z - r > far) { continue; }
        if (dot(top.xyz, c) + top.w < -r || dot(left.xyz, c) + left.w < -r ||
            dot(bottom.xyz, c) + bottom.w < -r || dot(right.xyz, c) + right.w < -r) {
            continue;
        }
        bits[i >> 5u] |= 1u << (i & 31u);
    }
    var out: TilesOut;
    out.lo = vec4<u32>(bits[0], bits[1], bits[2], bits[3]);
    out.hi = vec4<u32>(bits[4], bits[5], bits[6], bits[7]);
    return out;
}

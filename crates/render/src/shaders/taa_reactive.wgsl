// Reactive mask for temporal anti-aliasing: where the transparent phase drew something thin (a
// falling snowflake, a rain streak, a spark), so the resolve keeps most of this frame there
// instead of clipping it away as an outlier. Requires lib_types only.
//
// The transparent phase's change to a pixel is the difference between the scene before it
// (`reactive_before`, the copy made for refraction) and after (`reactive_after`), in a compressed
// space so bright and dark scenes weigh alike. A pixel is reactive when its change stands out from
// the ring of pixels three away on both sides along some direction, and few of that ring changed
// nearly as much: a flake, or a streak up to about five pixels wide crossing the ring twice. Wide
// transparent things (water, glass, smoke, a large near flake) change their neighbors as much, so
// TAA keeps anti-aliasing them; so do their edges (inside on one side), their corners, and the
// corners of a pattern seen through them (a quarter of the ring or more).

@group(0) @binding(0) var reactive_after: texture_2d<f32>;
@group(0) @binding(1) var reactive_before: texture_2d<f32>;

@vertex
fn vs_reactive(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn reactive_compress(c: vec3<f32>) -> vec3<f32> {
    // NaN and the half-float range: clamp, as the resolve does.
    let s = select(clamp(c, vec3<f32>(0.0), vec3<f32>(65000.0)), vec3<f32>(0.0), c != c);
    return s / (1.0 + luminance(s));
}

fn reactive_change(px: vec2<i32>, size: vec2<i32>) -> f32 {
    let q = clamp(px, vec2<i32>(0), size - 1);
    let d = abs(
        reactive_compress(textureLoad(reactive_after, q, 0).rgb)
            - reactive_compress(textureLoad(reactive_before, q, 0).rgb),
    );
    return max(d.r, max(d.g, d.b));
}

// The 24 pixels three away (the border of the 7x7 square), half of them: the other half is the
// negation of each.
const REACTIVE_RING = array<vec2<i32>, 12>(
    vec2<i32>(3, -2), vec2<i32>(3, -1), vec2<i32>(3, 0), vec2<i32>(3, 1), vec2<i32>(3, 2),
    vec2<i32>(3, 3), vec2<i32>(-3, 3), vec2<i32>(-2, 3), vec2<i32>(-1, 3), vec2<i32>(0, 3),
    vec2<i32>(1, 3), vec2<i32>(2, 3),
);

@fragment
fn fs_reactive(in: FullscreenOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(reactive_after));
    let px = vec2<i32>(floor(in.position.xy));
    let center = reactive_change(px, size);
    if (center < 0.003) {
        return vec4<f32>(0.0);
    }
    // The least, over the directions, of the larger change on either side (low only if the pixel
    // is thin across some direction), and how many of the ring changed nearly as much.
    var around = 1.0;
    var alike = 0.0;
    for (var i = 0; i < 12; i++) {
        let o = REACTIVE_RING[i];
        let a = reactive_change(px + o, size);
        let b = reactive_change(px - o, size);
        around = min(around, max(a, b));
        alike += f32(a > 0.5 * center) + f32(b > 0.5 * center);
    }
    // Relative, so faint rain counts as much as bright snow; barely changed pixels fade out.
    let thin = saturate(1.5 * (1.0 - around / center) - 0.25);
    let few = saturate((8.0 - alike) / 2.0);
    let r = thin * few * saturate((center - 0.003) * 200.0);
    return vec4<f32>(r, 0.0, 0.0, 1.0);
}

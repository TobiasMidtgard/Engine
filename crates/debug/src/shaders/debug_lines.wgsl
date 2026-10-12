// Debug lines. Requires lib_types + lib_frame (uses `frame.view_proj`).
//
// Positions are camera-relative (the CPU subtracts the f64 camera position). Colors are
// written as display values, NOT pre-exposed: a debug line looks the same at noon, at night
// and in a cave, and is never washed out or crushed by auto-exposure.
//
// Depth-tested lines use two tests:
// 1. The hardware test (reverse-Z GreaterEqual) against a depth nudged DEPTH_NUDGE toward the
//    camera: a cheap early reject of lines clearly behind the scene.
// 2. `fs_line_tested` compares the true line depth with the FARTHEST scene depth in the 3x3
//    pixels around the fragment (scene depth copy). A line lying on a surface is visible at
//    any viewing angle (the line and the surface are sampled half a pixel apart, which at
//    grazing angles is a large depth difference), while a line behind geometry stays hidden
//    except for a one-pixel halo at silhouettes.
//
// The true depth comes from the un-nudged clip z and w, interpolated perspective-correctly
// (both are affine in the position, so z / w is exact per fragment). It is NOT recovered from
// the nudged `position.z`: the nudge is clamped at the near plane, which for orthographic
// cameras covers the first ~5% of the depth range and would hide nearby lines.

// Relative depth nudge for the hardware test (reverse-Z: larger depth = closer).
const DEPTH_NUDGE: f32 = 1.05;
// Relative tolerance of the precise per-fragment test.
const DEPTH_TOLERANCE: f32 = 1.0005;

// Scene depth (reverse-Z) after the opaque and sky phases. Only used by `fs_line_tested`.
@group(1) @binding(0) var scene_depth: texture_depth_2d;

struct LineIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Un-nudged clip-space z and w; z / w is the line's true depth.
    @location(1) depth_zw: vec2<f32>,
};

@vertex
fn vs_line(in: LineIn) -> LineOut {
    var clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    var out: LineOut;
    out.depth_zw = clip.zw;
    // Scaling clip z scales NDC depth (z / w). Points in front of the near plane stay inside
    // it (z <= w), so the nudge never clips a line.
    if (clip.z <= clip.w) {
        clip.z = min(clip.z * DEPTH_NUDGE, clip.w);
    }
    out.clip = clip;
    out.color = in.color;
    return out;
}

fn line_color(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(max(c.rgb, vec3<f32>(0.0)), clamp(c.a, 0.0, 1.0));
}

// Always on top: no depth test at all (the pipeline compares with Always).
@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return line_color(in.color);
}

@fragment
fn fs_line_tested(in: LineOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let p = vec2<i32>(in.clip.xy);
    var farthest = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(p + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            farthest = min(farthest, textureLoad(scene_depth, q, 0));
        }
    }
    let depth = in.depth_zw.x / in.depth_zw.y;
    if (depth * DEPTH_TOLERANCE < farthest) {
        discard;
    }
    return line_color(in.color);
}

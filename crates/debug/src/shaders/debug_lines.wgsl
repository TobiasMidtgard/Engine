// Debug lines. Requires lib_types + lib_frame (uses `frame.view_proj`).
//
// Positions are camera-relative (the CPU subtracts the f64 camera position). Lines are drawn
// onto the final tonemapped image, so colors are exact display colors: a debug line looks the
// same at noon, at night and in a cave, and never affects auto-exposure.
//
// Depth-tested lines (`fs_line_tested`, reverse-Z: larger depth = closer) use the scene depth
// copy twice:
// 1. Hidden when the line is behind the scene at its own pixel by more than DEPTH_NUDGE.
// 2. Hidden when the line is behind the FARTHEST scene depth in the 3x3 pixels around the
//    fragment. A line lying on a surface stays visible at any viewing angle (the line and the
//    surface are sampled half a pixel apart, which at grazing angles is a large depth
//    difference), while a line behind geometry stays hidden except for a one-pixel halo at
//    silhouettes.
//
// The true depth comes from the clip z and w passed separately and interpolated
// perspective-correctly (both are affine in the position, so z / w is exact per fragment).

// Relative depth nudge for the center-pixel test (reverse-Z: larger depth = closer).
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
    // Clip-space z and w; z / w is the line's depth.
    @location(1) depth_zw: vec2<f32>,
};

@vertex
fn vs_line(in: LineIn) -> LineOut {
    let clip = frame.view_proj * vec4<f32>(in.position, 1.0);
    var out: LineOut;
    out.depth_zw = clip.zw;
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
    // Window pixels to scene pixels (a camera `Viewport` puts the scene inside the window).
    let p = vec2<i32>(in.clip.xy - frame.window.xy);
    let depth = in.depth_zw.x / in.depth_zw.y;
    if (depth * DEPTH_NUDGE < textureLoad(scene_depth, clamp(p, vec2<i32>(0), size - 1), 0)) {
        discard;
    }
    var farthest = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(p + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            farthest = min(farthest, textureLoad(scene_depth, q, 0));
        }
    }
    if (depth * DEPTH_TOLERANCE < farthest) {
        discard;
    }
    return line_color(in.color);
}

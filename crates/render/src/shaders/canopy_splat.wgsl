// Leaf crowns splatted into the canopy map (canopy.rs): each crown an ellipsoid of leaves seen
// from above, added into RGBA16F texels of 1 m. Per texel:
//   r = leaf area over the texel per square meter (tau, the column's leaf area index),
//   g = tau * mean leaf height, b = tau * mean squared leaf height,
// heights in CANOPY_HEIGHT_UNIT meters above the map's reference height. The mips then hold the
// mean leaf area index over larger squares and the mean and spread of leaf height there, as
// variance shadow maps keep depth moments (Donnelly and Lauritzen 2006).

struct Crown {
    // Center relative to the map's center (x, z; m), horizontal radius (m), and the leaf area
    // per square meter under its center (tau0).
    center_radius_tau: vec4<f32>,
    // Center height and vertical half-axis, in height units; unused; unused.
    height: vec4<f32>,
}

struct Splat {
    // Half the map's width (m), unused x3.
    half_extent: vec4<f32>,
}

@group(0) @binding(0) var<uniform> splat: Splat;
@group(0) @binding(1) var<storage, read> crowns: array<Crown>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Where in the crown's disc (-1..1, beyond it the footprint's margin).
    @location(0) at: vec2<f32>,
    @location(1) @interpolate(flat) crown: u32,
}

@vertex
fn vs_splat(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> VsOut {
    let c = crowns[i];
    let corner = vec2<f32>(f32(v & 1u), f32((v >> 1u) & 1u)) * 2.0 - 1.0;
    let xz = c.center_radius_tau.xy + corner * c.center_radius_tau.z;
    let half = splat.half_extent.x;
    var out: VsOut;
    // Texel rows run along +z from the top of the image (v = 0 at z = -half).
    out.pos = vec4<f32>(xz.x / half, -xz.y / half, 0.0, 1.0);
    out.at = corner;
    out.crown = i;
    return out;
}

@fragment
fn fs_splat(in: VsOut) -> @location(0) vec4<f32> {
    let d2 = dot(in.at, in.at);
    if (d2 >= 1.0) { discard; }
    let c = crowns[in.crown];
    // A vertical chord through an ellipsoid: 2 * half-axis * sqrt(1 - d²) long, its leaves
    // spread evenly along it (variance: (half chord)² / 3).
    let chord = sqrt(1.0 - d2);
    let tau = c.center_radius_tau.w * chord;
    let y = c.height.x;
    let half_chord = c.height.y * chord;
    return vec4<f32>(tau, tau * y, tau * (y * y + half_chord * half_chord / 3.0), 0.0);
}

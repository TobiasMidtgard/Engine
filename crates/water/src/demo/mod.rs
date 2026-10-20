//! The water test area: a tarn on a hilltop, spilling through a notch in its rim, down a
//! waterfall and a stream into a pond at the foot of the hill, with a crag across it whose
//! spring cascades down a crevice into the tarn.
//!
//! Everything in it tests one thing water does, and keeps doing it so a screenshot at any
//! moment shows it:
//!
//! | Test | What to look for |
//! |---|---|
//! | Wind waves, light | Gerstner waves sized by the wind, refraction of the lake bed, sky and sun reflections, caustics on the bottom, depth color |
//! | Flow | Water leaving the lake through the outlet, foaming over the falls, running down the stream and filling the pond, which spills out of its own notch |
//! | Floating | Rowboat moored at the jetty, buoys on ropes, logs riding the stream over the falls |
//! | Density | Cork, pine, ice and concrete blocks dropped every 14 s: they float high, middling, low, and sink |
//! | Interaction | A motorboat circling with a wake, a swimmer, a wader leaving rings in the shallows |
//! | Impacts | Gunfire into the lake every 4 s, a depth charge every 11 s (dome, rings, foam, boats thrown) |
//! | Weather | Rain over part of the lake and the pond: rings on the water, puddles on the grass |
//! | Cascade | A spring high on the crag across the tarn falling down its crevice in tiers into a cove: free falls, rapids, a curtain sliding down a face |
//!
//! [`spawn_demo`] builds it around a point on the ground and [`demo_system`] (added by
//! [`crate::WaterPlugin`]) drives it.

pub mod crag;
pub mod materials;
pub mod props;
pub mod scenery;
pub mod valley;

pub use valley::{Valley, ValleyStream, spawn_valley, valley_views};

use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use engine_core::math::noise;
use engine_core::prelude::*;
use engine_core::tasks::{ComputeTaskPool, par_chunks_mut};
use engine_render::{Mesh, Vertex};
use engine_world::{ContactKind, GroundContact, ProjectileTrace, WorldImpulse, WorldShift};

use crate::body::{RainShower, WaterBed, WaterBody, WaterOptics};
use crate::buoyancy::{Buoyancy, Floating};
use crate::flow::{FlowBed, FlowFill, FlowRegion, FlowSource};
use crate::interact::WaterInteractor;
use crate::state::SharedWater;
use materials::Surface;
use props::Part;
use scenery::{Jetty, merged, spawn_buoy, spawn_jetty, spawn_motorboat, spawn_prop, spawn_rowboat};

/// Still level of the tarn above the ground, meters.
pub const LAKE_LEVEL: f32 = 5.5;
/// Radius of the tarn's water body, meters (the shore itself wanders a few meters inside).
pub const LAKE_RADIUS: f32 = 32.0;
/// Depth in the middle, meters.
pub const LAKE_DEPTH: f32 = 5.0;
/// The bed rises over this many meters to the shore.
const SHORE_WIDTH: f32 = 12.0;
/// Top of the rim around the tarn.
const RIM: f32 = LAKE_LEVEL + 1.4;
/// Direction the outlet, falls, stream and pond lie in (local x, z).
const OUTLET: Vec2 = Vec2::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);
/// Along the outlet: the lip of the waterfall and the pond's center.
const FALLS_AT: f32 = 38.5;
const POND_AT: f32 = 66.0;
const POND_LEVEL: f32 = 0.55;
/// The stream's flow grid: corner (local x, z), cells, cell size.
const FLOW_CORNER: Vec2 = Vec2::new(12.0, 12.0);
const FLOW_CELLS: u32 = 104;
const FLOW_CELL: f32 = 0.5;
/// Jetty direction from the lake center, radians.
const JETTY_ANGLE: f32 = 1.31;
/// Height of the wader's hips above its soles: where the entity rides.
const WADER_HIP: f64 = 0.9;

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Shoreline radius of the tarn at angle `theta` (it wanders between ~26 and ~30 m).
pub fn shore_radius(theta: f32) -> f32 {
    28.0 + 1.5 * (3.0 * theta + 0.7).sin() + 0.8 * (7.0 * theta + 2.1).sin()
}

/// Bed of the outlet channel at distance `s` along the outlet direction.
fn channel_bed(s: f32) -> f32 {
    if s < FALLS_AT {
        LAKE_LEVEL - 0.15 - 0.55 * smooth(30.0, FALLS_AT, s)
    } else if s < FALLS_AT + 1.1 {
        mix(LAKE_LEVEL - 0.7, 2.2, smooth(FALLS_AT, FALLS_AT + 1.1, s))
    } else {
        // Down the slope, with a step of rapids halfway, into the pond.
        let t = ((s - FALLS_AT - 1.1) / (POND_AT - 7.0 - FALLS_AT - 1.1)).clamp(0.0, 1.0);
        mix(2.2, 0.4, t) + 0.3 * (1.0 - smooth(48.0, 48.6, s)) - 0.3 * (1.0 - t)
    }
}

/// Width of the outlet channel's floor (half), meters.
fn channel_half_width(s: f32) -> f32 {
    1.1 + 0.6 * smooth(30.0, 50.0, s)
}

/// Shoreline radius of the pond at angle `phi` around its center.
fn pond_radius(phi: f32) -> f32 {
    8.5 + 1.2 * (2.0 * phi + 1.0).sin() + 0.6 * (5.0 * phi + 2.0).sin()
}

/// The pond's spill ditch leaves at this distance along the outlet and ends in a culvert.
const DITCH_FROM: f32 = POND_AT + 7.5;
const DITCH_TO: f32 = POND_AT + 14.0;

/// Ground height of the test area (meters above the ground plane) at local `x`, `z` meters
/// from its center. The hill mesh, the stream's bed and everything placed in it use this.
pub fn tarn_height(x: f32, z: f32) -> f32 {
    let r = (x * x + z * z).sqrt();
    let theta = z.atan2(x);
    let rs = shore_radius(theta);
    let n = noise::fbm2(x as f64 * 0.07, z as f64 * 0.07, 0x7A21, 4, 2.0, 0.5);
    let fine = noise::fbm2(x as f64 * 0.4, z as f64 * 0.4, 0x7A22, 2, 2.0, 0.5);

    // The lake bed: a bowl rising to just above the water at the shore.
    let lake = LAKE_LEVEL + 0.25 - (LAKE_DEPTH + 0.25) * smooth(0.0, SHORE_WIDTH, rs - r)
        + n * 0.35 * smooth(0.0, 6.0, rs - r);
    // The hill: a rim around the lake, then a slope down to the plain.
    let base = LAKE_LEVEL + 0.25 + (RIM - LAKE_LEVEL - 0.25) * smooth(rs, rs + 4.0, r);
    let hill = base * (1.0 - smooth(rs + 4.0, 60.0, r)).powf(1.3)
        + n * 1.4 * smooth(rs + 2.0, rs + 8.0, r) * (1.0 - smooth(46.0, 60.0, r));
    let mut h = if r < rs { lake } else { hill };

    let along = x * OUTLET.x + z * OUTLET.y;
    let across = (x * OUTLET.y - z * OUTLET.x).abs();
    // Below the falls the stream has built a ridge of its own gravel out to the pond.
    if along > FALLS_AT {
        let ridge = (channel_bed(along) + 0.9)
            * (1.0 - smooth(3.0, 11.0, across + n * 2.0))
            * (1.0 - smooth(POND_AT - 6.0, POND_AT - 2.0, along));
        h = h.max(ridge);
    }

    // The pond at the foot, in a low earth bank with a wandering shore.
    let to_pond = Vec2::new(x, z) - OUTLET * POND_AT;
    let rp = pond_radius(to_pond.y.atan2(to_pond.x));
    let dp = to_pond.length();
    if dp < rp {
        // The basin meets the bank's crest (0.96) at the shore, so there is no step there.
        let basin = 0.06 + 0.9 * smooth(0.3 * rp, rp, dp);
        h = mix(h.max(0.96), basin, 1.0 - smooth(rp - 1.0, rp, dp));
    } else {
        h = h.max(0.96 * (1.0 - smooth(rp, rp + 4.0 + n * 1.5, dp)));
    }

    // The outlet: a notch in the rim, then a ravine with V-shaped walls down to the pond.
    if along > rs - 4.0 && along < POND_AT + 4.0 {
        let w = channel_half_width(along);
        // Rounded where the floor meets the walls (a soft max), so the bank has no crease.
        let x = across - w;
        let walls = channel_bed(along) + 0.5 * (x + (x * x + 0.6).sqrt()) * (0.85 + 0.25 * n);
        // Faded in at the notch and out into the pond, and rounded where the walls meet the
        // hillside: sharp folds catch hard shadow edges.
        let span =
            smooth(rs - 4.0, rs - 1.0, along) * (1.0 - smooth(POND_AT - 3.0, POND_AT + 4.0, along));
        let rounded = smooth_min(h, walls, 1.5).max(h.min(channel_bed(along)));
        h = mix(h, rounded, span);
    }
    // The pond's spill ditch to the culvert.
    if along > POND_AT && along < DITCH_TO + 2.0 {
        let bed = POND_LEVEL - 0.35 * smooth(DITCH_FROM, DITCH_TO, along);
        let bank = bed + (across - 0.7).max(0.0) * 0.6;
        let ditch = smooth(POND_AT + 3.0, POND_AT + 5.0, along)
            * (1.0 - smooth(DITCH_TO, DITCH_TO + 1.5, along));
        let embank = (bed + 0.45)
            * (1.0 - smooth(1.5, 4.0, across))
            * ditch
            * smooth(rp - 0.5, rp + 1.0, dp);
        h = h.max(embank);
        h = mix(h, h.min(bank), ditch);
        // The ditch ends at the culvert's headwall.
        let end = smooth(DITCH_TO - 0.2, DITCH_TO + 0.3, along) * (1.0 - smooth(1.5, 4.0, across));
        h = h.max((bed + 0.45) * end);
    }
    crag::shape(x, z, h + fine * 0.08)
}

/// `a.min(b)`, rounded over a band `k` wide where the two are close.
fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

fn srgb(hex: u32) -> Vec3 {
    let c = |v: u32| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3::new(c(hex >> 16 & 0xff), c(hex >> 8 & 0xff), c(hex & 0xff))
}

/// Ground color: grass, rock on steep ground, sand and pebbles at the shore, silt under water.
fn ground_color(x: f32, z: f32, h: f32, normal: Vec3) -> Vec3 {
    let r = (x * x + z * z).sqrt();
    let rs = shore_radius(z.atan2(x));
    let n = noise::fbm2(x as f64 * 0.21, z as f64 * 0.21, 0xC0105, 3, 2.0, 0.5);
    let grass = srgb(0x55743a).lerp(srgb(0x6f7d3d), smooth(-0.2, 0.6, n));
    let rock = srgb(0x7d776f).lerp(srgb(0x5f5a53), smooth(-0.3, 0.5, -n));
    // The crag is a cooler, darker stone. Each of its beds has a tone of its own (a few
    // buff and rusty ones among the grey), darkest at the partings, and water seeping out
    // of the partings streaks the faces below them.
    let steep = 1.0 - normal.y;
    let presence = crag::presence(x, z);
    let rock = if presence > 0.0 {
        let t = crag::bed_coord(x, z, h);
        let bed = t.floor();
        let tone = engine_core::math::hash01(bed as i64, 0, 0, 0xBED5);
        let mut stone = srgb(0x878580).lerp(srgb(0x63635f), tone);
        if engine_core::math::hash01(bed as i64, 1, 0, 0xBED5) > 0.7 {
            stone = stone.lerp(srgb(0x7a6c5a), 0.3);
        }
        stone = stone.lerp(srgb(0x3e3f42), smooth(0.25, 0.0, t.fract()) * 0.6);
        let (u, a) = crag::frame(x, z);
        let along = a + u * 0.15;
        let seep = noise::fbm2(along as f64 * 0.9, h as f64 * 0.05, 0x5EE9, 3, 2.0, 0.5);
        let streak = smooth(0.15, 0.45, seep) * smooth(0.3, 0.7, steep);
        stone *= 1.0 - 0.4 * streak;
        // Joints: near-vertical cracks cutting the beds into blocks, offset from bed to bed.
        let joint = 1.0
            - noise::perlin2(
                along as f64 * 0.32 + bed as f64 * 1.7,
                h as f64 * 0.04,
                0x1017,
            )
            .abs();
        stone *= 1.0 - 0.55 * smooth(0.94, 0.99, joint) * smooth(0.2, 0.5, steep);
        rock.lerp(stone * (0.92 + 0.16 * n), presence)
    } else {
        rock
    };
    let mut c = grass.lerp(srgb(0x6b573b), smooth(0.15, 0.4, steep + n * 0.05) * 0.5);
    c = c.lerp(rock, smooth(0.3, 0.55, steep + n * 0.12));
    // Around the tarn.
    if r < rs + 3.5 {
        let above = h - LAKE_LEVEL;
        let sand = srgb(0xb3a07a).lerp(srgb(0x8c7a5a), smooth(0.3, -0.05, above));
        let shore =
            (1.0 - smooth(0.4, 0.9, above + n * 0.2)) * (1.0 - smooth(rs + 1.5, rs + 3.5, r));
        c = c.lerp(sand, shore);
        if above < -0.05 {
            let depth = -above;
            let bed = srgb(0xa08c66).lerp(srgb(0x4f4a38), smooth(0.6, 4.0, depth));
            c = bed.lerp(srgb(0x6a645a), smooth(0.2, 0.6, n) * 0.5);
        }
    }
    // Pebbles in the stream bed, mud at the pond's edge and silt under it.
    let along = x * OUTLET.x + z * OUTLET.y;
    let across = (x * OUTLET.y - z * OUTLET.x).abs();
    if along > 30.0 && along < POND_AT {
        let bed = channel_bed(along);
        let wet = (1.0 - smooth(0.15, 0.6, h - bed)) * (1.0 - smooth(2.0, 3.5, across));
        c = c.lerp(
            srgb(0x6b655b).lerp(srgb(0x8a8273), smooth(-0.2, 0.4, n)),
            wet,
        );
    }
    // The crevice down the crag: dark wet rock, with a little moss where the spray reaches.
    let wet = crag::wetness(x, z, h);
    if wet > 0.0 {
        let moss = srgb(0x3f4a2c).lerp(srgb(0x2f3326), smooth(-0.2, 0.4, n));
        let damp = (c * 0.55).lerp(moss, smooth(0.2, 0.6, n) * 0.6);
        c = c.lerp(damp, wet);
    }
    let to_pond = Vec2::new(x, z) - OUTLET * POND_AT;
    let rp = pond_radius(to_pond.y.atan2(to_pond.x));
    let near_pond = 1.0 - smooth(rp, rp + 1.5, to_pond.length());
    c = c.lerp(
        srgb(0x6e5d44),
        near_pond * (1.0 - smooth(POND_LEVEL + 0.05, POND_LEVEL + 0.3, h)),
    );
    c = c.lerp(
        srgb(0x4d4a36),
        near_pond * (1.0 - smooth(POND_LEVEL - 0.25, POND_LEVEL - 0.05, h)),
    );
    c
}

/// The hill with the tarn, falls, stream and pond: a vertex-colored height mesh. Triangles
/// that would lie on the ground plane are left out, and so is the crag's crevice
/// ([`crag::PATCH`], which [`crag_mesh`] fills in finer).
pub fn tarn_mesh(spacing: f32) -> Mesh {
    height_mesh(
        Vec2::splat(-66.0),
        Vec2::splat(66.0),
        spacing,
        Some(crag::PATCH),
        false,
    )
}

/// The crag's crevice and the rock round it ([`crag::PATCH`]), at half the spacing of the
/// hill's mesh around it (`hill_spacing`) so its ledges and drops come out sharp. Its border
/// follows the hill's mesh exactly, so there are no cracks between them.
///
/// Two meshes: the turf and soil on its ledges and round it (for [`Surface::Ground`]) and its
/// bare rock (for [`Surface::Cliff`]), sharing vertices along their ragged border.
pub fn crag_mesh(hill_spacing: f32) -> (Mesh, Mesh) {
    let (lo, hi) = crag::PATCH;
    let mesh = height_mesh(lo, hi, hill_spacing * 0.5, None, true);
    let (mut turf, mut bare) = (Vec::new(), Vec::new());
    for t in mesh.indices.as_chunks::<3>().0 {
        let v = |k: usize| &mesh.vertices[t[k] as usize];
        let mid = (0..3).map(|k| Vec3::from(v(k).position)).sum::<Vec3>() / 3.0;
        let up = (0..3).map(|k| v(k).normal[1]).sum::<f32>() / 3.0;
        if crag::is_cliff(mid.x, mid.z, mid.y, up) {
            bare.extend_from_slice(t);
        } else {
            turf.extend_from_slice(t);
        }
    }
    // The rock texture carries the stone's own color; the vertex color keeps the crag's
    // (beds, streaks, damp) relative to it, so the two meshes match where they meet.
    // Hollows hold shade and grime, edges and noses are weathered pale: from the ground's
    // curvature over a meter.
    let ratio = materials::mean_color(Surface::Ground) / materials::mean_color(Surface::Cliff);
    let mut rock = mesh.vertices.clone();
    for v in &mut rock {
        let [x, y, z] = v.position;
        let d = 0.5;
        let around = tarn_height(x + d, z)
            + tarn_height(x - d, z)
            + tarn_height(x, z + d)
            + tarn_height(x, z - d);
        let bend = (around - 4.0 * y) / (d * d);
        let shade = 1.0 - 0.45 * smooth(0.0, 1.6, bend) + 0.25 * smooth(0.0, -1.6, bend);
        // Bare rock takes the stone's color however gentle the triangle's corners are (not
        // the turf's, which the turf mesh shows where the ground levels off).
        let n = Vec3::from(v.normal);
        let bare = Vec3::new(n.x, n.y.min(0.35), n.z).normalize_or(Vec3::X);
        // Moss and grass hold on the ledges.
        let ledge = smooth(0.62, 0.9, n.y);
        let moss = srgb(0x4b5a30).lerp(
            srgb(0x5d6b34),
            smooth(-0.3, 0.3, (x * 0.37).sin() * (z * 0.29).cos()),
        );
        let c = ground_color(x, z, y, bare).lerp(moss, ledge * 0.75) * ratio * shade;
        v.color = c.extend(v.color[3]).to_array();
    }
    (Mesh::new(mesh.vertices, turf), Mesh::new(rock, bare))
}

/// A vertex-colored mesh of the area's ground over `lo`..`hi` (local x, z) every `spacing`
/// meters, leaving out the quads inside `hole` and those on the ground plane. With
/// `half_of_neighbor`, it sits in a hole in a mesh twice as coarse: the points along its
/// border between that mesh's vertices are put on its edges.
fn height_mesh(
    lo: Vec2,
    hi: Vec2,
    spacing: f32,
    hole: Option<(Vec2, Vec2)>,
    half_of_neighbor: bool,
) -> Mesh {
    let nx = ((hi.x - lo.x) / spacing).round() as u32 + 1;
    let nz = ((hi.y - lo.y) / spacing).round() as u32 + 1;
    let pos = |i: u32, j: u32| lo + Vec2::new(i as f32, j as f32) * spacing;
    let mut heights = Vec::with_capacity((nx * nz) as usize);
    for j in 0..nz {
        for i in 0..nx {
            let p = pos(i, j);
            heights.push(tarn_height(p.x, p.y));
        }
    }
    if half_of_neighbor {
        for j in 0..nz {
            for i in 0..nx {
                let (edge_x, edge_z) = (i == 0 || i == nx - 1, j == 0 || j == nz - 1);
                let k = (j * nx + i) as usize;
                if edge_z && i % 2 == 1 {
                    heights[k] = 0.5 * (heights[k - 1] + heights[k + 1]);
                } else if edge_x && j % 2 == 1 {
                    heights[k] = 0.5 * (heights[k - nx as usize] + heights[k + nx as usize]);
                }
            }
        }
    }
    let slope = |i: u32, j: u32, w: i64| {
        let (i, j) = (i as i64, j as i64);
        // Past the mesh's edge, from the ground itself (a patch's normals match the mesh
        // round it).
        let e = |di: i64, dj: i64| {
            let (ii, jj) = (i + di, j + dj);
            if (0..nx as i64).contains(&ii) && (0..nz as i64).contains(&jj) {
                heights[(jj * nx as i64 + ii) as usize]
            } else {
                let p = lo + Vec2::new(ii as f32, jj as f32) * spacing;
                tarn_height(p.x, p.y)
            }
        };
        Vec3::new(
            e(-w, 0) - e(w, 0),
            2.0 * w as f32 * spacing,
            e(0, -w) - e(0, w),
        )
        .normalize()
    };
    let mut vertices = Vec::with_capacity(heights.len());
    for j in 0..nz {
        for i in 0..nx {
            let p = pos(i, j);
            let h = heights[(j * nx + i) as usize];
            // Along its border a finer mesh takes its neighbor's normals (over its spacing), so
            // the two shade the same where they meet, and blends to its own sharper normals
            // over a few meters (a step there reads as a seam).
            let normal = if half_of_neighbor {
                let edge = i.min(j).min(nx - 1 - i).min(nz - 1 - j) as f32 * spacing;
                let fine = smooth(0.5, 4.0, edge);
                slope(i, j, 2).lerp(slope(i, j, 1), fine).normalize()
            } else {
                slope(i, j, 1)
            };
            // Color by the slope over a wider area (about 3 m), so it doesn't flicker from
            // vertex to vertex.
            let broad = slope(i, j, (1.5 / spacing).round().max(1.0) as i64);
            let color = ground_color(p.x, p.y, h, broad);
            // Where the hill meets the plain it sinks smoothly under the ground plane.
            let y = h - 0.04 * (1.0 - smooth(0.0, 0.12, h));
            vertices
                .push(Vertex::new(Vec3::new(p.x, y, p.y), normal, p).with_color(color.extend(1.0)));
        }
    }
    let inside = |v: u32| {
        let p = pos(v % nx, v / nx);
        hole.is_some_and(|(a, b)| {
            p.x >= a.x - 1e-3 && p.y >= a.y - 1e-3 && p.x <= b.x + 1e-3 && p.y <= b.y + 1e-3
        })
    };
    let mut indices = Vec::new();
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = j * nx + i;
            let (b, c, d) = (a + 1, a + nx, a + nx + 1);
            let h = |v: u32| heights[v as usize];
            if [a, b, c, d].iter().all(|v| h(*v) <= 0.015)
                || [a, b, c, d].iter().all(|v| inside(*v))
            {
                continue;
            }
            // Split each quad along the diagonal that follows the ground (valleys and ridges
            // that cross the grid at an angle stay straight instead of zigzagging).
            if (h(a) - h(d)).abs() < (h(b) - h(c)).abs() {
                indices.extend_from_slice(&[a, c, d, a, d, b]);
            } else {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }
    Mesh::new(vertices, indices)
}

/// What a demo entity does.
#[derive(Component, Debug, Clone, Copy)]
pub enum DemoRole {
    /// Drives around a circle on the lake.
    Motorboat {
        center: DVec3,
        radius: f32,
        thrust: f32,
    },
    /// Swims around a circle at the surface.
    Swimmer {
        center: DVec3,
        radius: f32,
        speed: f32,
    },
    /// Wades back and forth in the shallows.
    Wader { from: DVec3, to: DVec3, speed: f32 },
    /// Dropped into the lake from `home` every cycle.
    DropBlock { home: DVec3 },
    /// Rides the stream; starts again at `start` when it reaches the pond or gets stuck.
    Log { start: DVec3 },
}

impl DemoRole {
    /// Move the places this role keeps into the frame after a world shift.
    pub fn shift(&mut self, s: &WorldShift) {
        match self {
            Self::Motorboat { center, .. } | Self::Swimmer { center, .. } => {
                *center = s.point(*center)
            }
            Self::Wader { from, to, .. } => {
                *from = s.point(*from);
                *to = s.point(*to);
            }
            Self::DropBlock { home } => *home = s.point(*home),
            Self::Log { start } => *start = s.point(*start),
        }
    }
}

/// The demo's clock and timers.
#[derive(Resource, Debug)]
pub struct DemoState {
    pub origin: DVec3,
    pub time: f32,
    next_drop: f32,
    next_volley: f32,
    shots_left: u32,
    next_shot: f32,
    next_charge: f32,
    next_step: f32,
    step_left: bool,
    rng: Rng,
}

/// A view of the test area for tours and galleries: eye and target relative to its center.
#[derive(Debug, Clone, Copy)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    /// Simulated seconds to wait on arriving before the shot.
    pub warmup_secs: f32,
    /// Everything in the shot comes from CPU simulation (no particles), so a tour may run the
    /// warm-up without drawing and draw only the frames that settle the picture.
    pub quick_warmup: bool,
}

/// The test area's close-ups, one per thing it tests that the overview can't show.
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "water_shore",
        name: "Tarn shore: waves, refraction and the moored rowboat",
        eye: [-1.5, 7.3, 30.5],
        target: [-2.0, 5.0, 0.0],
        hour: 16.5,
        warmup_secs: 3.0,
        quick_warmup: false,
    },
    DemoView {
        key: "water_falls",
        name: "Water spilling from the tarn over the falls",
        eye: [34.5, 3.6, 33.0],
        target: [27.0, 4.2, 27.4],
        hour: 16.5,
        warmup_secs: 3.0,
        quick_warmup: false,
    },
    DemoView {
        key: "water_wake",
        name: "Motorboat wake, swimmer and floating blocks",
        eye: [-3.0, 11.0, 26.0],
        target: [-5.0, 5.5, -3.0],
        hour: 16.5,
        warmup_secs: 3.0,
        quick_warmup: false,
    },
    DemoView {
        key: "water_under",
        name: "Under the tarn: caustics and light shafts",
        eye: [-2.0, 3.6, 7.0],
        target: [5.0, 4.6, 17.5],
        hour: 16.5,
        warmup_secs: 3.0,
        quick_warmup: true,
    },
    DemoView {
        key: "water_cascade",
        name: "A spring cascading down the crag's crevice into the tarn",
        eye: [-8.2, 9.0, -15.3],
        target: [-25.2, 13.0, -25.2],
        hour: 10.0,
        warmup_secs: 3.0,
        quick_warmup: false,
    },
    DemoView {
        key: "water_pond",
        name: "The stream filling the pond below the falls",
        eye: [56.0, 2.0, 38.0],
        target: [46.7, 0.3, 46.7],
        hour: 16.5,
        warmup_secs: 3.0,
        quick_warmup: false,
    },
];

/// Build the water test area with its center at `origin` (on the ground).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    spawn_demo_with(commands, origin, DemoOptions::default());
}

/// How [`spawn_demo_with`] builds the area.
#[derive(Debug, Clone, Copy)]
pub struct DemoOptions {
    /// Build the hill as the area's own mesh. Turn off when the world's terrain already has
    /// it (from [`area_height`]), so footsteps, physics and foliage see the real ground.
    pub ground_mesh: bool,
}

impl Default for DemoOptions {
    fn default() -> Self {
        Self { ground_mesh: true }
    }
}

/// Half the side of the square the area's hill covers, meters from its center. Outside it the
/// ground is the plain at the center's height.
pub const AREA_HALF_SIZE: f32 = 66.0;

/// Ground height of the area at `x`, `z` (meters from its center) above the center's height:
/// the hill, the tarn's bed, the ravine and the pond; 0 on the plain around it. A world's
/// terrain can add this to its own ground at the zone (and pass `ground_mesh: false`).
pub fn area_height(x: f32, z: f32) -> f32 {
    if x.abs() > AREA_HALF_SIZE || z.abs() > AREA_HALF_SIZE {
        return 0.0;
    }
    // The hill fades out over the last few meters of the square.
    let edge = 1.0 - smooth(AREA_HALF_SIZE - 6.0, AREA_HALF_SIZE, x.abs().max(z.abs()));
    let h = tarn_height(x, z);
    (h * smooth(0.0, 0.12, h)).max(0.0) * edge
}

/// What the area's ground is at `x`, `z` (meters from its center), for footstep sounds,
/// grip and where plants may grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaSurface {
    Grass,
    Dirt,
    Rock,
    Sand,
    Gravel,
    Mud,
    /// Under the tarn, the stream or the pond.
    Underwater,
}

/// The ground type at `x`, `z` (meters from the area's center).
pub fn area_surface(x: f32, z: f32) -> AreaSurface {
    let h = tarn_height(x, z);
    let e = 0.5;
    let normal = Vec3::new(
        tarn_height(x - e, z) - tarn_height(x + e, z),
        2.0 * e,
        tarn_height(x, z - e) - tarn_height(x, z + e),
    )
    .normalize();
    let r = (x * x + z * z).sqrt();
    let rs = shore_radius(z.atan2(x));
    let along = x * OUTLET.x + z * OUTLET.y;
    let across = (x * OUTLET.y - z * OUTLET.x).abs();
    let to_pond = Vec2::new(x, z) - OUTLET * POND_AT;
    let near_pond = to_pond.length() < pond_radius(to_pond.y.atan2(to_pond.x)) + 1.5;
    if (r < rs && h < LAKE_LEVEL) || (near_pond && h < POND_LEVEL) {
        return AreaSurface::Underwater;
    }
    if along > 30.0 && along < POND_AT && across < 3.0 && h - channel_bed(along) < 0.4 {
        return if h - channel_bed(along) < 0.08 {
            AreaSurface::Underwater
        } else {
            AreaSurface::Gravel
        };
    }
    if near_pond && h < POND_LEVEL + 0.3 {
        return AreaSurface::Mud;
    }
    if r < rs + 3.5 && h < LAKE_LEVEL + 0.7 {
        return AreaSurface::Sand;
    }
    let steep = 1.0 - normal.y;
    if steep > 0.42 {
        AreaSurface::Rock
    } else if steep > 0.22 {
        AreaSurface::Dirt
    } else {
        AreaSurface::Grass
    }
}

/// The test area's rock shapes (seed, size built at): small ones first, then
/// [`SMALL_ROCKS`].. large ones. Each boulder reuses one, scaled and turned.
const ROCKS: [(u64, f32); 8] = [
    (7, 1.0),
    (8, 1.0),
    (9, 1.0),
    (10, 1.0),
    (11, 1.0),
    (12, 2.0),
    (13, 2.0),
    (14, 2.0),
];
const SMALL_ROCKS: usize = 5;

/// Build [`ROCKS`] (a few seconds of work on one core), on the compute pool when there is one.
fn rock_shapes() -> Vec<Vec<Part>> {
    let mut built: Vec<Option<Vec<Part>>> = ROCKS.iter().map(|_| None).collect();
    let build = |i: usize, slot: &mut [Option<Vec<Part>>]| {
        let (seed, size) = ROCKS[i];
        slot[0] = Some(props::boulder(seed, size));
    };
    if ComputeTaskPool::try_get().is_some() {
        par_chunks_mut(&mut built, 1, build);
    } else {
        for (i, slot) in built.chunks_mut(1).enumerate() {
            build(i, slot);
        }
    }
    built.into_iter().flatten().collect()
}

/// [`spawn_demo`] with options.
pub fn spawn_demo_with(commands: &mut Commands, origin: DVec3, options: DemoOptions) {
    let at = |x: f32, y: f32, z: f32| origin + DVec3::new(x as f64, y as f64, z as f64);
    let polar = |angle: f32, r: f32| Vec2::new(angle.cos(), angle.sin()) * r;

    // The hill, tarn bed, falls, stream and pond.
    if options.ground_mesh {
        spawn_prop(
            commands,
            "Tarn hill",
            Transform::from_translation(origin),
            vec![Part::new(tarn_mesh(0.5), Surface::Ground)],
        );
        spawn_prop(
            commands,
            "Crag crevice",
            Transform::from_translation(origin),
            {
                let (turf, bare) = crag_mesh(0.5);
                vec![
                    Part::new(turf, Surface::Ground),
                    Part::new(bare, Surface::Cliff),
                ]
            },
        );
    }

    // The tarn.
    commands.spawn((
        Name::new("Tarn"),
        Transform::from_translation(at(0.0, LAKE_LEVEL, 0.0)),
        WaterBody::lake(LAKE_RADIUS)
            .with_bed(WaterBed::Bowl {
                depth_m: LAKE_DEPTH,
                shore_m: SHORE_WIDTH,
            })
            .with_optics(WaterOptics::clear_lake()),
    ));

    // The outlet stream, falls and pond: fed by the tarn through the notch in its rim.
    let mut bed = Vec::with_capacity((FLOW_CELLS * FLOW_CELLS) as usize);
    for j in 0..FLOW_CELLS {
        for i in 0..FLOW_CELLS {
            let p = FLOW_CORNER + (Vec2::new(i as f32, j as f32) + 0.5) * FLOW_CELL;
            bed.push(tarn_height(p.x, p.y));
        }
    }
    let pond = OUTLET * POND_AT;
    commands.spawn((
        Name::new("Stream and pond"),
        Transform::from_translation(at(FLOW_CORNER.x, 0.0, FLOW_CORNER.y)),
        FlowRegion {
            cells: [FLOW_CELLS, FLOW_CELLS],
            cell_m: FLOW_CELL,
            bed: FlowBed::Heights(bed),
            open_edges: true,
            friction: 0.35,
            loss_m_s: 0.0,
            sources: vec![FlowSource {
                at: (OUTLET * (DITCH_TO - 0.4) - FLOW_CORNER).to_array(),
                radius_m: 0.8,
                rate_m3_s: -0.6,
            }],
            fills: vec![FlowFill {
                at: (pond - FLOW_CORNER).to_array(),
                level: POND_LEVEL + 0.02,
                ..Default::default()
            }],
            prewarm_s: 40.0,
            ..Default::default()
        },
    ));

    // The crag's spring, cascading down its crevice into the cove.
    let (place, region) = crag::cascade(origin, tarn_height);
    commands.spawn((Name::new("Crag cascade"), place, region));

    // The culvert the pond's spill ditch drains into: its mouth faces back up the ditch and
    // the pipe runs on into the bank.
    let culvert = OUTLET * (DITCH_TO - 0.1);
    let invert = tarn_height(culvert.x, culvert.y) - 0.05;
    spawn_prop(
        commands,
        "Culvert",
        Transform::from_translation(at(culvert.x, invert + 0.38, culvert.y))
            .with_rotation(Quat::from_rotation_y(PI * 0.75)),
        props::culvert(),
    );

    // Boulders: by the falls and down the ravine, on the rim, and in the shallows.
    let rocks = rock_shapes();
    let mut rng = Rng::new(0xB01D);
    let side_of = Vec2::new(OUTLET.y, -OUTLET.x);
    let mut boulders = 0;
    while boulders < 30 {
        let k = boulders;
        let (p, size) = if k < 12 {
            let s = FALLS_AT - 4.0 + rng.range(0.0, 18.0);
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let across = side * (channel_half_width(s) + rng.range(0.5, 3.5));
            (OUTLET * s + side_of * across, rng.range(0.7, 2.2))
        } else {
            let a = rng.range(0.0, TAU);
            let near = |b: f32| ((a - b + PI).rem_euclid(TAU) - PI).abs() < 0.22;
            if near(JETTY_ANGLE) || near(PI * 0.25) {
                continue;
            }
            if k < 22 {
                (
                    polar(a, shore_radius(a) + rng.range(0.5, 5.0)),
                    rng.range(0.6, 1.9),
                )
            } else {
                (
                    polar(a, shore_radius(a) - rng.range(0.8, 3.0)),
                    rng.range(0.5, 1.3),
                )
            }
        };
        boulders += 1;
        // One of the shapes for its size class, stretched a little so no two look alike.
        let (shape, built) = if size < 1.5 {
            (k % SMALL_ROCKS, 1.0)
        } else {
            (SMALL_ROCKS + k % (ROCKS.len() - SMALL_ROCKS), 2.0)
        };
        let stretch = Vec3::new(
            rng.range(0.85, 1.15),
            rng.range(0.8, 1.1),
            rng.range(0.85, 1.15),
        ) * (size / built);
        let parts = rocks[shape]
            .iter()
            .map(|p| Part {
                mesh: p.mesh.clone(),
                surface: p.surface,
                double_sided: p.double_sided,
            })
            .collect();
        // Half buried: the flat bed goes into the ground.
        spawn_prop(
            commands,
            "Boulder",
            Transform::from_translation(at(p.x, tarn_height(p.x, p.y) - size * 0.05, p.y))
                .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU))),
            merged([(parts, Mat4::from_scale(stretch))]),
        );
    }

    // The jetty: decking on piles, from the bank out over the deep water.
    let jdir = polar(JETTY_ANGLE, 1.0);
    let jside = Vec2::new(-jdir.y, jdir.x);
    let shore = shore_radius(JETTY_ANGLE);
    let deck = LAKE_LEVEL + 0.5;
    let jetty_len = 13.0;
    let bed = move |x: f64, z: f64| {
        origin.y + tarn_height((x - origin.x) as f32, (z - origin.z) as f32) as f64
    };
    let land = jdir * (shore + 1.0);
    spawn_jetty(
        commands,
        &Jetty {
            shore: at(land.x, deck, land.y),
            dir: -jdir.as_dvec2(),
            length: jetty_len,
            water_level: origin.y + LAKE_LEVEL as f64,
            bed: &bed,
            seed: 0x1E77,
        },
    );
    let jetty_end = jdir * (shore + 1.0 - jetty_len);

    // A rowboat moored at the end of the jetty.
    let row_at = jetty_end + jside * 2.6;
    spawn_rowboat(
        commands,
        at(row_at.x, LAKE_LEVEL, row_at.y),
        -JETTY_ANGLE + PI * 0.5,
        at(jetty_end.x, deck, jetty_end.y),
    );

    // A motorboat doing laps.
    let lap_center = Vec2::new(-5.0, -3.0);
    let lap_radius = 12.0;
    let start = lap_center + Vec2::new(lap_radius, 0.0);
    let motorboat = spawn_motorboat(commands, at(start.x, LAKE_LEVEL, start.y), -PI * 0.5);
    commands.entity(motorboat).insert(DemoRole::Motorboat {
        center: at(lap_center.x, LAKE_LEVEL, lap_center.y),
        radius: lap_radius,
        thrust: 5200.0,
    });

    // Buoys marking a swimming area, on ropes to the bottom.
    for k in 0..5 {
        let p = polar(0.55 + k as f32 * 0.2, 14.0);
        spawn_buoy(
            commands,
            at(p.x, LAKE_LEVEL, p.y),
            at(p.x, tarn_height(p.x, p.y), p.y),
            k,
        );
    }

    // Density blocks: cork, pine, ice and concrete, dropped side by side off the jetty.
    let blocks = [
        ("Cork block", 240.0, Surface::Cork),
        ("Pine block", 520.0, Surface::Pine),
        ("Ice block", 917.0, Surface::Ice),
        ("Concrete block", 2400.0, Surface::BlockConcrete),
    ];
    for (k, (name, density, surface)) in blocks.into_iter().enumerate() {
        let p = jetty_end + jdir * 1.5 - jside * 2.4 + jdir * (k as f32 * 1.1 - 1.6);
        let home = at(p.x, LAKE_LEVEL + 2.5, p.y);
        let e = spawn_prop(
            commands,
            name,
            Transform::from_translation(home),
            vec![Part::new(props::block(0.5), surface)],
        );
        commands.entity(e).insert((
            Buoyancy::new(Vec3::splat(0.25), density),
            Floating::default(),
            DemoRole::DropBlock { home },
        ));
    }

    // A swimmer doing lengths of the buoyed area, and a wader in the shallows.
    let e = spawn_prop(
        commands,
        "Swimmer",
        Transform::from_translation(at(10.0, LAKE_LEVEL, 12.0)),
        // The water line runs along the swimmer's back, 0.15 m above the model's middle.
        merged([(props::swimmer(), Mat4::from_translation(Vec3::Y * -0.09))]),
    );
    commands.entity(e).insert((
        WaterInteractor::new(0.35).with_wake(1.2),
        DemoRole::Swimmer {
            center: at(8.0, LAKE_LEVEL, 10.0),
            radius: 5.5,
            speed: 1.1,
        },
    ));

    let wade_angle = 0.25f32;
    let (a, b) = (wade_angle - 0.12, wade_angle + 0.12);
    let wade = |angle: f32| {
        let p = polar(angle, shore_radius(angle) - 2.6);
        at(p.x, tarn_height(p.x, p.y), p.y)
    };
    let e = spawn_prop(
        commands,
        "Wader",
        Transform::from_translation(wade(a) + DVec3::Y * WADER_HIP),
        // The entity rides at the hips (where it meets the water); the soles are below.
        merged([(
            props::wader(),
            Mat4::from_translation(Vec3::Y * -WADER_HIP as f32),
        )]),
    );
    commands.entity(e).insert((
        WaterInteractor::new(0.25).with_wake(0.8),
        DemoRole::Wader {
            from: wade(a),
            to: wade(b),
            speed: 0.9,
        },
    ));

    // A second wader fords the stream above the falls: its legs part the current, and the
    // wake they make drifts downstream (the stream is as interactive as the tarn).
    let ford = OUTLET * 34.0;
    let across = Vec2::new(-OUTLET.y, OUTLET.x);
    let ford_at = |k: f32| {
        let p = ford + across * k;
        at(p.x, tarn_height(p.x, p.y), p.y)
    };
    let e = spawn_prop(
        commands,
        "Stream wader",
        Transform::from_translation(ford_at(-1.0) + DVec3::Y * WADER_HIP),
        merged([(
            props::wader(),
            Mat4::from_translation(Vec3::Y * -WADER_HIP as f32),
        )]),
    );
    commands.entity(e).insert(DemoRole::Wader {
        from: ford_at(-1.0),
        to: ford_at(1.0),
        speed: 0.5,
    });
    // The stream only reaches its shins: they push the water, not its hips.
    commands.spawn((
        Name::new("Stream wader legs"),
        Transform::from_translation(DVec3::Y * (0.1 - WADER_HIP)),
        WaterInteractor::new(0.2).with_wake(1.5),
        ChildOf(e),
    ));

    // Logs that ride the stream over the falls into the pond.
    for k in 0..3 {
        let s = 31.0 + k as f32 * 2.5;
        let p = OUTLET * s + Vec2::new(OUTLET.y, -OUTLET.x) * (k as f32 - 1.0) * 0.6;
        let start = at(p.x, channel_bed(s) + 0.5, p.y);
        let e = spawn_prop(
            commands,
            "Log",
            Transform::from_translation(start)
                .with_rotation(Quat::from_rotation_y(-PI * 0.25 + 0.3 * k as f32)),
            props::log(1.7, 0.17, k as u64),
        );
        commands.entity(e).insert((
            Buoyancy {
                fill: PI / 4.0,
                ..Buoyancy::new(Vec3::new(0.85, 0.17, 0.17), 560.0).with_probes(3, 1)
            },
            Floating::default(),
            WaterInteractor::new(0.25).with_wake(0.4),
            DemoRole::Log { start },
        ));
    }

    // Rain over the far half of the tarn and over the pond and the grass around it.
    let p = polar(-0.5, 16.0);
    commands.spawn((
        Name::new("Shower over the tarn"),
        Transform::from_translation(at(p.x, LAKE_LEVEL, p.y)),
        RainShower {
            radius_m: 11.0,
            mm_h: 8.0,
        },
    ));
    commands.spawn((
        Name::new("Shower over the pond"),
        Transform::from_translation(at(pond.x, 0.0, pond.y)),
        RainShower {
            radius_m: 16.0,
            mm_h: 14.0,
        },
    ));

    commands.insert_resource(DemoState {
        origin,
        time: 0.0,
        next_drop: 1.0,
        next_volley: 2.5,
        shots_left: 0,
        next_shot: 0.0,
        next_charge: 6.0,
        next_step: 0.0,
        step_left: false,
        rng: Rng::new(0x1A4E),
    });
}

/// Signed angle (radians) to turn about +y from `from` to `to` (horizontal parts).
fn yaw_between(from: Vec3, to: Vec3) -> f32 {
    let cross = from.z * to.x - from.x * to.z;
    let dot = from.x * to.x + from.z * to.z;
    cross.atan2(dot)
}

/// Drive the test area: boats, swimmers, drops, gunfire, blasts and footsteps.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn demo_system(
    time: Res<Time>,
    state: Option<ResMut<DemoState>>,
    shared: Res<SharedWater>,
    mut actors: Query<(
        Entity,
        &mut Transform,
        &DemoRole,
        Option<&mut Floating>,
        Option<&Buoyancy>,
    )>,
    mut traces: MessageWriter<ProjectileTrace>,
    mut blasts: MessageWriter<WorldImpulse>,
    mut contacts: MessageWriter<GroundContact>,
) {
    let Some(mut demo) = state else {
        return;
    };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    demo.time += dt;
    let t = demo.time;
    let origin = demo.origin;
    let water = shared.0.read();
    let surface = |p: DVec3| water.sample(p).map(|w| w.surface_height);

    let drop_now = t >= demo.next_drop;
    if drop_now {
        demo.next_drop = t + 14.0;
    }
    let mut step = None;
    if t >= demo.next_step {
        demo.next_step = t + 0.42;
        demo.step_left = !demo.step_left;
        step = Some(demo.step_left);
    }

    for (entity, mut tf, role, floating, buoyancy) in &mut actors {
        match *role {
            DemoRole::Motorboat {
                center,
                radius,
                thrust,
            } => {
                let (Some(mut fl), Some(b)) = (floating, buoyancy) else {
                    continue;
                };
                let rel = (tf.translation - center).as_vec3() * Vec3::new(1.0, 0.0, 1.0);
                let r = rel.length().max(0.1);
                let tangent = Vec3::new(rel.z, 0.0, -rel.x) / r;
                let pull = ((r - radius) * 0.25).clamp(-0.8, 0.8);
                let desired = (tangent - rel / r * pull).normalize();
                let fwd = tf.rotation * Vec3::X;
                let mass = b.density * b.volume();
                let he = b.half_extents;
                let inertia = mass / 3.0 * (he.x * he.x + he.z * he.z);
                let err = yaw_between(fwd, desired);
                let in_water = fl.submerged > 0.05;
                if in_water {
                    let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
                    fl.force += flat * thrust;
                    // The keel: water resists sliding sideways.
                    let v = fl.velocity;
                    let side = Vec3::new(v.x, 0.0, v.z) - flat * flat.dot(Vec3::new(v.x, 0.0, v.z));
                    fl.force -= side * mass * 1.8;
                    // Firm steering: the hull's turning drag needs a good rudder to hold a
                    // 12 m lap at speed (softer, it ran wide onto the beach).
                    fl.torque.y += inertia * (10.0 * err - 3.0 * fl.angular_velocity.y);
                }
            }
            DemoRole::Swimmer {
                center,
                radius,
                speed,
            } => {
                let a = t * speed / radius;
                let p =
                    center + DVec3::new((a.cos() * radius) as f64, 0.0, (a.sin() * radius) as f64);
                let y = surface(p).unwrap_or(center.y) - 0.06 + (t * 4.2).sin() as f64 * 0.025;
                tf.translation = DVec3::new(p.x, y, p.z);
                // Facing along the circle (+x is the head).
                let heading = Vec3::new(-a.sin(), 0.0, a.cos());
                tf.rotation = Quat::from_rotation_y(yaw_between(Vec3::X, heading))
                    * Quat::from_rotation_x((t * 2.1).sin() * 0.25);
            }
            DemoRole::Wader { from, to, speed } => {
                let span = (to - from).length().max(0.1);
                let phase = (t * speed / span as f32) % 2.0;
                let k = if phase < 1.0 { phase } else { 2.0 - phase };
                let k = smooth(0.0, 1.0, k) as f64;
                let feet = from + (to - from) * k;
                let local = feet - origin;
                let ground = tarn_height(local.x as f32, local.z as f32) as f64;
                let feet = DVec3::new(feet.x, ground, feet.z);
                tf.translation = feet + DVec3::Y * WADER_HIP;
                let dir = ((to - from) * if phase < 1.0 { 1.0 } else { -1.0 }).as_vec3();
                tf.rotation = Quat::from_rotation_y(yaw_between(Vec3::X, dir));
                if let Some(left) = step {
                    let side = (tf.rotation * Vec3::Z).as_dvec3() * if left { 0.12 } else { -0.12 };
                    contacts.write(GroundContact {
                        position: feet + side,
                        normal: Vec3::Y,
                        size: [0.11, 0.28],
                        yaw: yaw_between(Vec3::X, dir),
                        load_kg: 75.0,
                        speed,
                        kind: ContactKind::Foot,
                        surface: None,
                        entity: Some(entity),
                    });
                }
            }
            DemoRole::DropBlock { home } => {
                if drop_now {
                    tf.translation = home;
                    tf.rotation = Quat::from_euler(
                        EulerRot::YXZ,
                        demo.rng.range(0.0, TAU),
                        demo.rng.range(-0.3, 0.3),
                        demo.rng.range(-0.3, 0.3),
                    );
                    if let Some(mut fl) = floating {
                        *fl = Floating::default();
                    }
                }
            }
            DemoRole::Log { start } => {
                let local = tf.translation - origin;
                let along = (local.x * OUTLET.x as f64 + local.z * OUTLET.y as f64) as f32;
                let slow = floating
                    .as_ref()
                    .is_some_and(|f| f.velocity.length() < 0.05);
                // Start again from the outlet once it reaches the pond's far side, leaves the
                // stream, or has rested a while.
                let restart = along > POND_AT + 6.0
                    || local.y < -2.0
                    || (slow && demo.rng.f32() < dt / 12.0)
                    || (along > POND_AT - 4.0 && demo.rng.f32() < dt / 10.0);
                if restart {
                    tf.translation = start;
                    if let Some(mut fl) = floating {
                        *fl = Floating::default();
                    }
                }
            }
        }
    }

    // Gunfire: a burst into the lake from the rim.
    if t >= demo.next_volley {
        demo.next_volley = t + 4.0;
        demo.shots_left = 7;
        demo.next_shot = t;
    }
    if demo.shots_left > 0 && t >= demo.next_shot {
        demo.shots_left -= 1;
        demo.next_shot = t + 0.09;
        let gun = Vec2::new(1.75f32.cos(), 1.75f32.sin()) * 34.0;
        let from = origin + DVec3::new(gun.x as f64, (RIM + 1.4) as f64, gun.y as f64);
        let aim = Vec2::new(0.9f32.cos(), 0.9f32.sin()) * 9.0
            + Vec2::new(demo.rng.range(-3.0, 3.0), demo.rng.range(-2.0, 2.0))
            + Vec2::new(-1.0, 0.5) * (7 - demo.shots_left) as f32 * 0.8;
        let to = origin + DVec3::new(aim.x as f64, (LAKE_LEVEL - 1.5) as f64, aim.y as f64);
        traces.write(ProjectileTrace {
            from,
            to,
            radius: 0.05,
            speed: 420.0,
            shooter: None,
        });
    }

    // A depth charge off the far side of the swimming area.
    if t >= demo.next_charge {
        demo.next_charge = t + 11.0;
        blasts.write(WorldImpulse {
            center: origin + DVec3::new(-9.0, (LAKE_LEVEL - 2.2) as f64, -6.0),
            radius: 7.0,
            strength: 28.0,
            energy_j: 0.0,
            heat_c: 0.0,
            source: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tarn_holds_water_and_spills_through_its_outlet() {
        // Around the rim the ground is above the lake, except in the outlet channel.
        for k in 0..72 {
            let a = k as f32 / 72.0 * TAU;
            let rs = shore_radius(a);
            let p = Vec2::new(a.cos(), a.sin()) * (rs + 2.5);
            let h = tarn_height(p.x, p.y);
            let along = p.dot(OUTLET);
            let across = (p.x * OUTLET.y - p.y * OUTLET.x).abs();
            if along > 0.0 && across < 1.0 {
                assert!(h < LAKE_LEVEL, "outlet at {a}: {h}");
            } else if across > 4.0 {
                assert!(h > LAKE_LEVEL + 0.2, "rim at {a}: {h}");
            }
        }
        // The middle is deep, and the bed stays above the ground plane everywhere.
        assert!(tarn_height(0.0, 0.0) < LAKE_LEVEL - LAKE_DEPTH + 0.6);
        assert!(tarn_height(0.0, 0.0) > 0.1);
        let pond = OUTLET * POND_AT;
        assert!(tarn_height(pond.x, pond.y) > 0.02);
        assert!(tarn_height(pond.x, pond.y) < POND_LEVEL - 0.3);
    }

    #[test]
    fn the_exported_ground_matches_the_area() {
        // Flat plain outside, the hill and its features inside.
        assert_eq!(area_height(80.0, 0.0), 0.0);
        assert!(area_height(0.0, -45.0) > 2.0);
        assert!((area_height(0.0, 0.0) - tarn_height(0.0, 0.0)).abs() < 1e-3);
        assert_eq!(area_surface(0.0, 0.0), AreaSurface::Underwater);
        let pond = OUTLET * POND_AT;
        assert_eq!(area_surface(pond.x, pond.y), AreaSurface::Underwater);
        let rs = shore_radius(-1.0);
        let shore = Vec2::new((-1.0f32).cos(), (-1.0f32).sin()) * (rs + 0.8);
        assert_eq!(area_surface(shore.x, shore.y), AreaSurface::Sand);
        assert_eq!(area_surface(40.0, -40.0), AreaSurface::Grass);
        // The crag's face across the tarn is bare rock.
        let face = crag::point(0.6, 6.0);
        assert_eq!(area_surface(face.x, face.y), AreaSurface::Rock);
    }

    #[test]
    fn the_channel_runs_downhill() {
        let mut last = f32::INFINITY;
        let mut s = 30.0;
        while s < 58.0 {
            let b = channel_bed(s);
            assert!(b <= last + 1e-4, "{s}: {b} after {last}");
            last = b;
            s += 0.25;
        }
    }

    #[test]
    fn the_mesh_skips_the_flat_ground() {
        let m = tarn_mesh(2.0);
        let full = 67u32 * 67 * 2;
        assert!(m.triangle_count() < full as usize * 9 / 10);
        assert!(m.triangle_count() > 1000);
    }
}

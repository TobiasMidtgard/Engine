//! The water test area: a tarn on a hilltop, spilling through a notch in its rim, down a
//! waterfall and a stream into a pond at the foot of the hill.
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
//!
//! [`spawn_demo`] builds it around a point on the ground and [`demo_system`] (added by
//! [`crate::WaterPlugin`]) drives it.

use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use engine_assets::Assets;
use engine_core::math::noise;
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRenderer, Shape, StandardMaterial, Vertex};
use engine_world::{ContactKind, GroundContact, ProjectileTrace, WorldImpulse};

use crate::body::{RainShower, WaterBed, WaterBody, WaterOptics};
use crate::buoyancy::{Buoyancy, Floating, Tether};
use crate::flow::{FlowBed, FlowFill, FlowRegion, FlowSource};
use crate::interact::WaterInteractor;
use crate::state::SharedWater;

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
        h = mix(
            h,
            0.06 + 0.9 * smooth(0.3 * rp, rp, dp),
            1.0 - smooth(rp - 1.0, rp, dp),
        );
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
    h + fine * 0.08
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
    let steep = 1.0 - normal.y;
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

/// A boulder about `size` meters across: a lumpy, flattened sphere, vertex-colored gray.
fn rock_mesh(seed: u64, size: f32) -> Mesh {
    let mut m = Shape::Sphere {
        radius: size * 0.5,
        segments: 14,
    }
    .mesh();
    let s = seed as f64 * 17.3;
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let d = p.normalize_or_zero();
        let bump = noise::fbm2(
            d.x as f64 * 1.7 + s,
            (d.z as f64 + d.y as f64 * 0.7) * 1.7,
            seed,
            3,
            2.0,
            0.5,
        );
        let mut q = p * (1.0 + 0.3 * bump);
        q.y *= 0.65;
        if q.y < -size * 0.15 {
            q.y = -size * 0.15 + (q.y + size * 0.15) * 0.3;
        }
        v.position = q.to_array();
        let g = 0.85 + 0.3 * bump;
        v.color = (srgb(0x86807a) * g).extend(1.0).to_array();
    }
    m.compute_normals();
    m
}

/// The hill with the tarn, falls, stream and pond: a vertex-colored height mesh. Triangles
/// that would lie on the ground plane are left out.
pub fn tarn_mesh(spacing: f32) -> Mesh {
    let (lo, hi) = (-66.0f32, 66.0f32);
    let n = ((hi - lo) / spacing).ceil() as u32 + 1;
    let mut heights = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        for i in 0..n {
            heights.push(tarn_height(
                lo + i as f32 * spacing,
                lo + j as f32 * spacing,
            ));
        }
    }
    let at = |i: u32, j: u32| heights[(j.min(n - 1) * n + i.min(n - 1)) as usize];
    let mut vertices = Vec::with_capacity(heights.len());
    for j in 0..n {
        for i in 0..n {
            let (x, z) = (lo + i as f32 * spacing, lo + j as f32 * spacing);
            let h = at(i, j);
            let dx = at(i + 1, j) - at(i.saturating_sub(1), j);
            let dz = at(i, j + 1) - at(i, j.saturating_sub(1));
            let normal = Vec3::new(-dx, 2.0 * spacing, -dz).normalize();
            // Color by the slope over a wider area, so it doesn't flicker from vertex to vertex.
            let wx = at(i + 3, j) - at(i.saturating_sub(3), j);
            let wz = at(i, j + 3) - at(i, j.saturating_sub(3));
            let broad = Vec3::new(-wx, 6.0 * spacing, -wz).normalize();
            let color = ground_color(x, z, h, broad);
            // Where the hill meets the plain it sinks smoothly under the ground plane.
            let y = h - 0.04 * (1.0 - smooth(0.0, 0.12, h));
            vertices.push(
                Vertex::new(Vec3::new(x, y, z), normal, Vec2::new(x, z) * 0.25)
                    .with_color(color.extend(1.0)),
            );
        }
    }
    let mut indices = Vec::new();
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = j * n + i;
            let (b, c, d) = (a + 1, a + n, a + n + 1);
            let h = |v: u32| heights[v as usize];
            if [a, b, c, d].iter().all(|v| h(*v) <= 0.015) {
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

/// A boat hull along +x: `length`, `beam` and `depth` in meters, centered on the origin,
/// open on top (draw it double-sided).
pub fn hull_mesh(length: f32, beam: f32, depth: f32) -> Mesh {
    const SECTIONS: u32 = 18;
    const AROUND: u32 = 12;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for s in 0..=SECTIONS {
        let t = s as f32 / SECTIONS as f32;
        let x = (t - 0.5) * length;
        // Full beam a bit aft of the middle, a transom at the stern, a point at the bow.
        let width = if t < 0.4 {
            0.8 + 0.2 * (t / 0.4 * PI * 0.5).sin()
        } else {
            ((t - 0.4) / 0.6 * PI * 0.5).cos().max(0.0).powf(0.8)
        } * beam
            * 0.5;
        let keel = -depth * 0.5 + depth * 0.55 * smooth(0.65, 1.0, t).powf(1.5);
        let top = depth * 0.5 + depth * 0.25 * smooth(0.6, 1.0, t);
        for k in 0..=AROUND {
            let phi = (k as f32 / AROUND as f32 - 0.5) * PI;
            let z = width * phi.sin();
            let y = keel + (top - keel) * (1.0 - phi.cos().max(0.0).powf(0.35));
            vertices.push(Vertex::new(
                Vec3::new(x, y, z),
                Vec3::Y,
                Vec2::new(t, k as f32 / AROUND as f32),
            ));
        }
    }
    let row = AROUND + 1;
    for s in 0..SECTIONS {
        for k in 0..AROUND {
            let a = s * row + k;
            let (b, c, d) = (a + 1, a + row, a + row + 1);
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    // Transom.
    let center = vertices.len() as u32;
    let stern_top = vertices[0].position[1];
    vertices.push(Vertex::new(
        Vec3::new(-length * 0.5, stern_top * 0.2, 0.0),
        -Vec3::X,
        Vec2::ZERO,
    ));
    for k in 0..AROUND {
        indices.extend_from_slice(&[center, k + 1, k]);
    }
    let mut mesh = Mesh::new(vertices, indices);
    mesh.compute_normals();
    mesh
}

/// `shape`'s mesh moved by `transform` and tinted `color` (linear).
fn part(shape: Shape, transform: Transform, color: Vec3) -> Mesh {
    let mut m = shape.mesh().with_vertex_color(color.extend(1.0));
    m.transform(&Mat4::from_scale_rotation_translation(
        transform.scale,
        transform.rotation,
        transform.translation.as_vec3(),
    ));
    m
}

fn tinted(mut mesh: Mesh, color: Vec3) -> Mesh {
    mesh = mesh.with_vertex_color(color.extend(1.0));
    mesh
}

/// Attach a mesh to `entity` once the world has a mesh store (it may not, in tests).
fn add_mesh(commands: &mut Commands, entity: Entity, mesh: Mesh, material: StandardMaterial) {
    commands.queue(move |world: &mut World| {
        let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
            return;
        };
        let handle = meshes.add(mesh);
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.insert(MeshRenderer::new(handle, material));
        }
    });
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

/// Build the water test area with its center at `origin` (on the ground).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let at = |x: f32, y: f32, z: f32| origin + DVec3::new(x as f64, y as f64, z as f64);
    let polar = |angle: f32, r: f32| Vec2::new(angle.cos(), angle.sin()) * r;
    let mat = |hex: u32, rough: f32| StandardMaterial::hex(hex).with_roughness(rough);

    // The hill, tarn bed, falls, stream and pond.
    let mut ground = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.92);
    ground.reflectance = 0.35;
    let hill = commands
        .spawn((Name::new("Tarn hill"), Transform::from_translation(origin)))
        .id();
    add_mesh(commands, hill, tarn_mesh(0.5), ground);

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
            }],
            prewarm_s: 40.0,
            ..Default::default()
        },
    ));

    // The culvert the pond's spill ditch drains into.
    let culvert = OUTLET * (DITCH_TO - 0.2);
    let culvert_y = tarn_height(culvert.x, culvert.y);
    commands.spawn((
        Name::new("Culvert"),
        Transform::from_translation(at(culvert.x, culvert_y + 0.25, culvert.y))
            .with_rotation(Quat::from_rotation_y(-PI * 0.25) * Quat::from_rotation_z(PI * 0.5)),
        MeshRenderer::new(
            Shape::Torus {
                radius: 0.38,
                tube: 0.09,
                segments: 20,
            },
            mat(0x9a968e, 0.8),
        ),
    ));
    commands.spawn((
        Transform::from_translation(at(culvert.x, culvert_y + 0.25, culvert.y))
            .with_rotation(Quat::from_rotation_y(-PI * 0.25) * Quat::from_rotation_z(PI * 0.5)),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 0.36,
                height: 0.12,
                segments: 20,
            },
            mat(0x0e0d0c, 1.0),
        ),
    ));
    let wall = OUTLET * (DITCH_TO + 0.15);
    commands.spawn((
        Transform::from_translation(at(wall.x, culvert_y + 0.2, wall.y))
            .with_rotation(Quat::from_rotation_y(-PI * 0.25)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.3, 1.0, 3.2],
            },
            mat(0x8e8a82, 0.85),
        ),
    ));

    // Boulders: by the falls and down the ravine, on the rim, and in the shallows.
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
        let e = commands
            .spawn((
                Name::new("Boulder"),
                Transform::from_translation(at(p.x, tarn_height(p.x, p.y) + size * 0.08, p.y))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU))),
            ))
            .id();
        add_mesh(
            commands,
            e,
            rock_mesh(k as u64 + 7, size),
            mat(0xffffff, 0.85),
        );
    }

    // The jetty: planks on posts, from the shore out over the deep water.
    let jdir = polar(JETTY_ANGLE, 1.0);
    let jside = Vec2::new(-jdir.y, jdir.x);
    let shore = shore_radius(JETTY_ANGLE);
    let deck = LAKE_LEVEL + 0.45;
    let wood = mat(0x7a5634, 0.85);
    let jetty_len = 12.0;
    let yaw = -JETTY_ANGLE;
    for k in 0..24 {
        let r = shore + 1.0 - k as f32 * 0.55;
        let p = jdir * r;
        commands.spawn((
            Transform::from_translation(at(p.x, deck, p.y))
                .with_rotation(Quat::from_rotation_y(yaw)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.48, 0.07, 2.0],
                },
                if k % 3 == 1 { mat(0x6e4c2e, 0.9) } else { wood },
            ),
        ));
    }
    for k in 0..5 {
        let r = shore + 0.5 - k as f32 * (jetty_len / 4.0);
        for side in [-0.95f32, 0.95] {
            let p = jdir * r + jside * side;
            let bottom = tarn_height(p.x, p.y);
            let height = deck - bottom + 0.2;
            commands.spawn((
                Transform::from_translation(at(p.x, bottom + height * 0.5 - 0.1, p.y)),
                MeshRenderer::new(
                    Shape::Cylinder {
                        radius: 0.11,
                        height,
                        segments: 10,
                    },
                    mat(0x4a3a2a, 0.9),
                ),
            ));
        }
    }
    let jetty_end = jdir * (shore + 0.5 - jetty_len);

    // A rowboat moored at the end of the jetty.
    let row_at = jetty_end + jside * 2.6;
    let rowboat = {
        let mut m = tinted(hull_mesh(3.6, 1.35, 0.6), Vec3::ONE);
        for x in [-0.7f32, 0.5] {
            m.merge(&part(
                Shape::Cuboid {
                    size: [0.28, 0.05, 1.15],
                },
                Transform::from_xyz(x as f64, 0.12, 0.0),
                srgb(0xc9b28a),
            ));
        }
        m
    };
    let e = commands
        .spawn((
            Name::new("Rowboat"),
            Transform::from_translation(at(row_at.x, LAKE_LEVEL + 0.1, row_at.y))
                .with_rotation(Quat::from_rotation_y(-JETTY_ANGLE + PI * 0.5)),
            Buoyancy::new(Vec3::new(1.7, 0.3, 0.6), 420.0)
                .with_fill(0.45)
                .with_probes(3, 2),
            Floating::default(),
            Tether {
                anchor: at(jetty_end.x, deck, jetty_end.y),
                length_m: 2.8,
                stiffness: 900.0,
            },
            WaterInteractor::new(0.6).with_wake(0.6),
        ))
        .id();
    let mut hull = mat(0x8c5a32, 0.7);
    hull.double_sided = true;
    add_mesh(commands, e, rowboat, hull);

    // A motorboat doing laps.
    let lap_center = Vec2::new(-5.0, -3.0);
    let lap_radius = 12.0;
    let start = lap_center + Vec2::new(lap_radius, 0.0);
    let motorboat = {
        let mut m = tinted(hull_mesh(5.2, 2.0, 0.9), srgb(0xf2f2ee));
        m.merge(&part(
            Shape::Cuboid {
                size: [2.4, 0.06, 1.75],
            },
            Transform::from_xyz(0.9, 0.42, 0.0),
            srgb(0xa0703e),
        ));
        m.merge(&part(
            Shape::Cuboid {
                size: [0.1, 0.45, 1.5],
            },
            Transform::from_xyz(-0.15, 0.62, 0.0).with_rotation(Quat::from_rotation_z(-0.5)),
            srgb(0x1c2a33),
        ));
        m.merge(&part(
            Shape::Cuboid {
                size: [0.45, 0.6, 0.35],
            },
            Transform::from_xyz(-2.65, 0.0, 0.0),
            srgb(0x222222),
        ));
        m
    };
    let e = commands
        .spawn((
            Name::new("Motorboat"),
            Transform::from_translation(at(start.x, LAKE_LEVEL + 0.1, start.y))
                .with_rotation(Quat::from_rotation_y(-PI * 0.5)),
            Buoyancy {
                drag: 0.12,
                angular_drag: 0.8,
                ..Buoyancy::new(Vec3::new(2.4, 0.45, 0.9), 380.0)
                    .with_fill(0.5)
                    .with_probes(3, 2)
            },
            Floating::default(),
            WaterInteractor::new(0.9).with_wake(2.5),
            DemoRole::Motorboat {
                center: at(lap_center.x, LAKE_LEVEL, lap_center.y),
                radius: lap_radius,
                thrust: 5200.0,
            },
        ))
        .id();
    let mut hull = mat(0xffffff, 0.35);
    hull.double_sided = true;
    add_mesh(commands, e, motorboat, hull);

    // Buoys marking a swimming area, on ropes to the bottom.
    for k in 0..5 {
        let p = polar(0.55 + k as f32 * 0.2, 14.0);
        let buoy = {
            let mut m = part(
                Shape::Sphere {
                    radius: 0.32,
                    segments: 14,
                },
                Transform::IDENTITY,
                srgb(if k % 2 == 0 { 0xd8361e } else { 0xf0f0ea }),
            );
            m.merge(&part(
                Shape::Cone {
                    radius: 0.12,
                    height: 0.5,
                    segments: 10,
                },
                Transform::from_xyz(0.0, 0.45, 0.0),
                srgb(0xd8361e),
            ));
            m
        };
        let e = commands
            .spawn((
                Name::new("Buoy"),
                Transform::from_translation(at(p.x, LAKE_LEVEL + 0.3, p.y)),
                Buoyancy::sphere(0.32, 220.0),
                Floating::default(),
                Tether {
                    anchor: at(p.x, tarn_height(p.x, p.y), p.y),
                    length_m: 0.6,
                    stiffness: 300.0,
                },
            ))
            .id();
        add_mesh(commands, e, buoy, mat(0xffffff, 0.45));
    }

    // Density blocks: cork, pine, ice and concrete, dropped side by side off the jetty.
    let blocks = [
        ("Cork block", 240.0, 0xc49a6c, 0.8),
        ("Pine block", 520.0, 0xd9b77e, 0.7),
        ("Ice block", 917.0, 0xd6eef5, 0.08),
        ("Concrete block", 2400.0, 0x8f8b85, 0.9),
    ];
    for (k, (name, density, color, rough)) in blocks.into_iter().enumerate() {
        let p = jetty_end + jdir * 1.5 - jside * 2.4 + jdir * (k as f32 * 1.1 - 1.6);
        let home = at(p.x, LAKE_LEVEL + 2.5, p.y);
        let mut m = mat(color, rough);
        if density == 917.0 {
            m = m.with_alpha(0.75);
        }
        commands.spawn((
            Name::new(name),
            Transform::from_translation(home),
            MeshRenderer::new(Shape::Cuboid { size: [0.5; 3] }, m),
            Buoyancy::new(Vec3::splat(0.25), density),
            Floating::default(),
            DemoRole::DropBlock { home },
        ));
    }

    // A swimmer doing lengths of the buoyed area, and a wader in the shallows.
    let swimmer = {
        let mut m = part(
            Shape::Capsule {
                radius: 0.17,
                height: 1.25,
                segments: 12,
            },
            Transform::from_rotation(Quat::from_rotation_z(PI * 0.5)),
            srgb(0x1f4fa8),
        );
        m.merge(&part(
            Shape::Sphere {
                radius: 0.12,
                segments: 12,
            },
            Transform::from_xyz(0.78, 0.04, 0.0),
            srgb(0xc68e6a),
        ));
        m
    };
    let e = commands
        .spawn((
            Name::new("Swimmer"),
            Transform::from_translation(at(10.0, LAKE_LEVEL, 12.0)),
            WaterInteractor::new(0.35).with_wake(1.2),
            DemoRole::Swimmer {
                center: at(8.0, LAKE_LEVEL, 10.0),
                radius: 5.5,
                speed: 1.1,
            },
        ))
        .id();
    add_mesh(commands, e, swimmer, mat(0xffffff, 0.5));

    let wade_angle = 0.25f32;
    let (a, b) = (wade_angle - 0.12, wade_angle + 0.12);
    let wade = |angle: f32| {
        let p = polar(angle, shore_radius(angle) - 2.6);
        at(p.x, tarn_height(p.x, p.y), p.y)
    };
    let wader = {
        let mut m = part(
            Shape::Capsule {
                radius: 0.2,
                height: 1.0,
                segments: 12,
            },
            Transform::from_xyz(0.0, 0.25, 0.0),
            srgb(0xe2b31c),
        );
        m.merge(&part(
            Shape::Cylinder {
                radius: 0.17,
                height: 0.85,
                segments: 12,
            },
            Transform::from_xyz(0.0, -0.45, 0.0),
            srgb(0x2f3a2a),
        ));
        m.merge(&part(
            Shape::Sphere {
                radius: 0.12,
                segments: 12,
            },
            Transform::from_xyz(0.0, 0.88, 0.0),
            srgb(0xc68e6a),
        ));
        m
    };
    let e = commands
        .spawn((
            Name::new("Wader"),
            Transform::from_translation(wade(a) + DVec3::Y * 0.9),
            WaterInteractor::new(0.25).with_wake(0.8),
            DemoRole::Wader {
                from: wade(a),
                to: wade(b),
                speed: 0.9,
            },
        ))
        .id();
    add_mesh(commands, e, wader, mat(0xffffff, 0.6));

    // Logs that ride the stream over the falls into the pond.
    for k in 0..3 {
        let s = 31.0 + k as f32 * 2.5;
        let p = OUTLET * s + Vec2::new(OUTLET.y, -OUTLET.x) * (k as f32 - 1.0) * 0.6;
        let start = at(p.x, channel_bed(s) + 0.5, p.y);
        let log = part(
            Shape::Cylinder {
                radius: 0.17,
                height: 1.7,
                segments: 10,
            },
            Transform::from_rotation(Quat::from_rotation_z(PI * 0.5)),
            srgb(0x6b4a2e),
        );
        let e = commands
            .spawn((
                Name::new("Log"),
                Transform::from_translation(start)
                    .with_rotation(Quat::from_rotation_y(-PI * 0.25 + 0.3 * k as f32)),
                Buoyancy {
                    fill: PI / 4.0,
                    ..Buoyancy::new(Vec3::new(0.85, 0.17, 0.17), 560.0).with_probes(3, 1)
                },
                Floating::default(),
                WaterInteractor::new(0.25).with_wake(0.4),
                DemoRole::Log { start },
            ))
            .id();
        add_mesh(commands, e, log, mat(0xffffff, 0.85));
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
                let pull = ((r - radius) * 0.12).clamp(-0.7, 0.7);
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
                    fl.torque.y += inertia * (2.5 * err - 2.2 * fl.angular_velocity.y);
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
                tf.translation = feet + DVec3::Y * 0.9;
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

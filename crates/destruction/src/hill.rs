//! The Valheim-style dig-and-build hill, built by [`spawn_voxel_hill`] around an origin (for the
//! showcase's voxel hill zone). One loop of [`HILL_CYCLE_SECS`]:
//!
//! - a pickaxe drives a tunnel into the hill's rocky side, then opens a chamber too wide for the
//!   rock and soil above it, which caves in;
//! - a hoe raises and levels a terrace of soil on the other flank and smooths its edges;
//! - beside the hill a timber shelter goes up piece by piece in build-mode support colors (blue
//!   on the ground through red at the limit), then loses two posts and half of it comes down.
//!
//! Then the hill and the hut are rebuilt and it starts again.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::{DamageEvent, DamageKind, EditOp, EditShape, SurfaceMaterials, TerrainEdit};

use crate::DemoView;
use crate::debris::Debris;
use crate::material::DestructibleMaterial;
use crate::structure::{ShowSupport, StructuralPiece};
use crate::voxel::VoxelVolume;

/// Seconds per loop.
pub const HILL_CYCLE_SECS: f32 = 28.0;

/// Hill size (width × height × depth, m) and voxel size.
pub const HILL_SIZE: [f32; 3] = [34.0, 8.0, 30.0];
const HILL_VOXEL: f32 = 0.3;
/// Where the hut stands, relative to the origin (flat ground beside the hill's −Z foot).
const HUT: [f64; 3] = [15.0, 0.0, -10.0];

const TUNNEL_DIGS: u32 = 15;
const CHAMBER_DIGS: u32 = 15;
const RAISES: u32 = 6;
/// The hoe's level-ground strokes; each pass averages the soil with its neighbors once, so a
/// few in a row turn the raised mounds' edges into a soft bank.
const SMOOTH_PASSES: u32 = 6;

/// The dig-and-build hill's moments.
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "voxel_tunnel",
        name: "Digging a tunnel into the hill",
        eye: [-2.5, 2.2, -16.0],
        target: [-4.0, 1.4, -6.0],
        hour: 10.5,
        warmup_secs: 5.1,
    },
    DemoView {
        key: "voxel_cave_in",
        name: "The tunnel's hall caving in",
        eye: [-1.0, 7.0, -17.0],
        target: [-4.5, 2.5, -2.0],
        hour: 10.5,
        warmup_secs: 10.6,
    },
    DemoView {
        key: "voxel_terrace",
        name: "Raising and leveling a terrace with the hoe",
        eye: [18.0, 8.0, -6.0],
        target: [8.5, 3.5, 2.0],
        hour: 10.5,
        warmup_secs: 17.6,
    },
    DemoView {
        key: "voxel_hut",
        name: "A timber hut in support colors losing its posts",
        eye: [9.5, 3.5, -16.5],
        target: [15.0, 1.8, -10.0],
        hour: 10.5,
        warmup_secs: 19.5,
    },
];

/// The running hill script.
#[derive(Component, Debug, Clone, Default)]
pub struct HillDemo {
    pub origin: DVec3,
    /// Seconds into the current loop.
    pub time: f32,
    pub cycle: u32,
    next: usize,
    pub hill: Option<Entity>,
    /// Hut pieces built so far this loop.
    pub hut: Vec<Entity>,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Build(usize),
    Tunnel(u32),
    Chamber(u32),
    Raise(u32),
    Smooth,
    KnockPost(usize),
    Reset,
}

fn timeline() -> Vec<(f32, Step)> {
    let mut t = Vec::new();
    for k in 0..hut_plan().len() {
        t.push((0.4 + k as f32 * 0.45, Step::Build(k)));
    }
    for k in 0..TUNNEL_DIGS {
        t.push((1.0 + k as f32 * 0.45, Step::Tunnel(k)));
    }
    for k in 0..CHAMBER_DIGS {
        t.push((8.0 + k as f32 * 0.35, Step::Chamber(k)));
    }
    for k in 0..RAISES {
        t.push((14.0 + k as f32 * 0.5, Step::Raise(k)));
    }
    for k in 0..SMOOTH_PASSES {
        t.push((17.2 + k as f32 * 0.12, Step::Smooth));
    }
    t.push((18.5, Step::KnockPost(0)));
    t.push((19.6, Step::KnockPost(1)));
    t.push((HILL_CYCLE_SECS - 0.01, Step::Reset));
    t.sort_by(|a, b| a.0.total_cmp(&b.0));
    t
}

/// One piece of the hut: name, center (relative to the hut), rotation, size.
struct PieceSpec {
    name: &'static str,
    at: [f64; 3],
    rotation: Quat,
    size: Vec3,
}

/// The hut (an open timber shelter, so its support runs only through the frame), in build
/// order: posts, top beams, king posts, the ridge and the roof.
fn hut_plan() -> Vec<PieceSpec> {
    let mut v = Vec::new();
    let post = Vec3::new(0.22, 2.6, 0.22);
    for (x, z) in [(-1.9, -1.9), (1.9, -1.9), (1.9, 1.9), (-1.9, 1.9)] {
        v.push(PieceSpec {
            name: "Hut post",
            at: [x, 1.3, z],
            rotation: Quat::IDENTITY,
            size: post,
        });
    }
    for z in [-1.9, 1.9] {
        v.push(PieceSpec {
            name: "Hut beam",
            at: [0.0, 2.7, z],
            rotation: Quat::IDENTITY,
            size: Vec3::new(4.2, 0.2, 0.2),
        });
    }
    for x in [-1.9, 1.9] {
        v.push(PieceSpec {
            name: "Hut beam",
            at: [x, 2.7, 0.0],
            rotation: Quat::IDENTITY,
            size: Vec3::new(0.2, 0.2, 4.2),
        });
    }
    // A ridge post on each gable and the ridge beam.
    for z in [-1.9, 1.9] {
        v.push(PieceSpec {
            name: "Hut king post",
            at: [0.0, 3.35, z],
            rotation: Quat::IDENTITY,
            size: Vec3::new(0.18, 1.1, 0.18),
        });
    }
    v.push(PieceSpec {
        name: "Hut ridge",
        at: [0.0, 3.95, 0.0],
        rotation: Quat::IDENTITY,
        size: Vec3::new(0.2, 0.2, 4.4),
    });
    // Two roof panels from the ridge down past the side beams.
    for s in [-1.0f64, 1.0] {
        v.push(PieceSpec {
            name: "Hut roof",
            at: [s * 1.05, 3.35, 0.0],
            rotation: Quat::from_rotation_z(-s as f32 * 0.58),
            size: Vec3::new(2.6, 0.12, 4.6),
        });
    }
    v
}

fn hill_volume() -> VoxelVolume {
    let [w, h, d] = HILL_SIZE;
    VoxelVolume::hill(w, h, d, HILL_VOXEL).with_seed(23)
}

/// Build the dig-and-build hill around `origin` (ground level). It is about 34 × 30 m with the
/// hut beside its −Z/+X foot; everything fits in a 26 m radius.
pub fn spawn_voxel_hill(commands: &mut Commands, origin: DVec3) {
    let hill = commands
        .spawn((
            Name::new("Voxel hill"),
            Transform::from_translation(origin),
            hill_volume(),
        ))
        .id();
    commands.spawn((
        Name::new("Hill demo"),
        HillDemo {
            origin,
            hill: Some(hill),
            ..Default::default()
        },
    ));
}

/// Advance every [`HillDemo`] (FixedUpdate, `SimSet::Gameplay`).
pub fn hill_script_system(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<HillDemo>>()
        .iter(world)
        .collect();
    let steps = timeline();
    for e in demos {
        let Some(mut demo) = world.get::<HillDemo>(e).cloned() else {
            continue;
        };
        demo.time += dt;
        while demo.next < steps.len() && steps[demo.next].0 <= demo.time {
            let step = steps[demo.next].1;
            demo.next += 1;
            run_step(world, &mut demo, step);
        }
        if demo.time >= HILL_CYCLE_SECS {
            demo.time -= HILL_CYCLE_SECS;
            demo.next = 0;
            demo.cycle += 1;
        }
        if let Some(mut d) = world.get_mut::<HillDemo>(e) {
            *d = demo;
        }
    }
}

/// Start every [`HillDemo`] over: the hill and the hut are rebuilt. Returns how many restarted.
pub fn restart_hills(world: &mut World) -> usize {
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<HillDemo>>()
        .iter(world)
        .collect();
    for &e in &demos {
        let Some(mut demo) = world.get::<HillDemo>(e).cloned() else {
            continue;
        };
        run_step(world, &mut demo, Step::Reset);
        demo.time = 0.0;
        demo.next = 0;
        demo.cycle += 1;
        if let Some(mut d) = world.get_mut::<HillDemo>(e) {
            *d = demo;
        }
    }
    demos.len()
}

fn edit(world: &mut World, center: DVec3, shape: EditShape, op: EditOp, surface: Option<&str>) {
    let surface = surface.and_then(|name| {
        world
            .get_resource::<SurfaceMaterials>()
            .and_then(|s| s.id(name))
    });
    world.write_message(TerrainEdit {
        center,
        shape,
        op,
        surface,
        source: None,
    });
}

fn run_step(world: &mut World, demo: &mut HillDemo, step: Step) {
    let o = demo.origin;
    match step {
        Step::Build(k) => {
            let plan = hut_plan();
            let Some(spec) = plan.get(k) else {
                return;
            };
            let at = o + DVec3::from_array(HUT) + DVec3::from_array(spec.at);
            let t = Transform::from_translation(at).with_rotation(spec.rotation);
            let mut piece = StructuralPiece::new(DestructibleMaterial::Wood, spec.size);
            piece.break_energy_j = 6000.0;
            let e = world
                .spawn((
                    Name::new(spec.name),
                    t,
                    GlobalTransform::from(t),
                    MeshRenderer::new(
                        Shape::Cuboid {
                            size: spec.size.to_array(),
                        },
                        StandardMaterial::hex(0x7a5634).with_roughness(0.85),
                    ),
                    piece,
                    ShowSupport {
                        on: true,
                        children: false,
                    },
                ))
                .id();
            demo.hut.push(e);
        }
        Step::Tunnel(k) => {
            // Into the rocky −Z side, a pick's reach at a time.
            let z = -10.5 + k as f64 * 0.6;
            edit(
                world,
                o + DVec3::new(-4.0, 1.25, z),
                EditShape::Sphere { radius: 1.15 },
                EditOp::Remove,
                None,
            );
        }
        Step::Chamber(k) => {
            // Widen the tunnel's end into a hall too wide for its roof (about 9 × 7 m).
            let (col, row) = ((k % 5) as f64, (k / 5) as f64);
            edit(
                world,
                o + DVec3::new(-7.4 + col * 1.4, 1.8, -3.4 + row * 1.8),
                EditShape::Sphere { radius: 1.8 },
                EditOp::Remove,
                None,
            );
        }
        Step::Raise(k) => {
            // Raise a terrace of soil on the +X flank in hoe-sized mounds, which the smoothing
            // passes then level into one bank.
            let x = 7.0 + (k % 3) as f64 * 1.8;
            let z = 1.0 + (k / 3) as f64 * 2.2;
            edit(
                world,
                o + DVec3::new(x, 2.3, z),
                EditShape::Sphere { radius: 1.5 },
                EditOp::Add,
                Some("dirt"),
            );
        }
        Step::Smooth => {
            edit(
                world,
                o + DVec3::new(8.8, 3.0, 2.1),
                EditShape::Sphere { radius: 4.2 },
                EditOp::Smooth { strength: 1.0 },
                None,
            );
        }
        Step::KnockPost(k) => {
            // Both posts on the −X side: that side's beam, the ridge and the roof panel over it
            // lose their path to the ground and fall; the +X half stands.
            let Some(&post) = demo.hut.get([0, 3][k.min(1)]) else {
                return;
            };
            let Some(t) = world.get::<Transform>(post).copied() else {
                return;
            };
            world.write_message(DamageEvent {
                target: post,
                amount: 400.0,
                kind: DamageKind::Blunt,
                point: t.translation + DVec3::new(0.0, -0.5, -0.1),
                impulse: Vec3::new(0.0, 0.0, 60.0),
                source: None,
            });
        }
        Step::Reset => {
            if let Some(h) = demo.hill
                && let Some(mut v) = world.get_mut::<VoxelVolume>(h)
            {
                v.set_changed();
            }
            for e in demo.hut.drain(..) {
                if world.get::<Debris>(e).is_none() && world.get_entity(e).is_ok() {
                    world.despawn(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_builds_from_the_ground_up() {
        let plan = hut_plan();
        assert!(plan.len() >= 11);
        // Posts first, so every later piece has something to rest on.
        assert!(plan[..4].iter().all(|p| p.name == "Hut post"));
        let steps = timeline();
        assert!(steps.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(steps.last().unwrap().0 < HILL_CYCLE_SECS);
    }
}

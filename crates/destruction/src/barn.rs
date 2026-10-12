//! The burning storehouse: a timber shed in the fortress courtyard that catches fire from a
//! woodpile inside, chars, sags and comes down, then is rebuilt for the next loop.
//!
//! The shed is ordinary [`StructuralPiece`]s with an `engine_world::Combustible` each; the
//! fire module (when the game installs one) does the burning, [`crate::burn`] chars the wood
//! and weakens each piece as its fuel goes, and the support solver drops what can no longer
//! stand. Without a fire module the shed just stands. Its boards are old and dry (a third of
//! solid timber's fuel, little moisture), so it comes down within a loop.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::Combustible;

use crate::burn::{Charring, WOOD_IGNITION_C};
use crate::debris::Debris;
use crate::dress::PropModel;
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::props::PropKind;
use crate::structure::StructuralPiece;

/// Seconds per loop: built, lit, burnt down, left smoldering, rebuilt.
pub const CYCLE_SECS: f32 = 120.0;
/// When the woodpile is lit.
const IGNITE_AT: f32 = 2.0;
/// When what is left starts sinking away.
const CLEAR_AT: f32 = CYCLE_SECS - 8.0;
/// Fraction of solid timber's fuel in the shed's old, thin boards.
const FUEL_FRACTION: f32 = 0.35;
/// Moisture of the shed's dry boards.
const MOISTURE: f32 = 0.06;

/// Where the shed stands in the fortress courtyard (relative to the demo's origin).
pub const BARN: [f64; 3] = [31.0, 0.0, 20.0];

/// A piece of the storehouse (kept out of the breach range's debris clean-up).
#[derive(Component, Debug, Clone, Copy)]
pub struct BarnPiece;

/// The storehouse's loop.
#[derive(Component, Debug, Clone, Default)]
pub struct BarnFire {
    pub origin: DVec3,
    pub yaw: f32,
    /// Seconds into the loop.
    pub time: f32,
    pub cycle: u32,
    /// Every piece (standing or fallen).
    pub pieces: Vec<Entity>,
    /// The woodpile that is lit.
    pub kindling: Vec<Entity>,
    lit: bool,
    cleared: bool,
}

/// Spawn the storehouse at `at` (ground level), its open front facing `yaw` + −Z.
pub fn spawn_barn(commands: &mut Commands, at: DVec3, yaw: f32) -> Entity {
    let (pieces, kindling) = build(commands, at, yaw);
    commands
        .spawn((
            Name::new("Burning storehouse"),
            BarnFire {
                origin: at,
                yaw,
                pieces,
                kindling,
                ..BarnFire::default()
            },
        ))
        .id()
}

fn build(commands: &mut Commands, at: DVec3, yaw: f32) -> (Vec<Entity>, Vec<Entity>) {
    let frame = Quat::from_rotation_y(yaw);
    let mut pieces = Vec::new();
    let mut seed = 0u64;
    let mut add = |commands: &mut Commands,
                   name: &str,
                   local: Vec3,
                   rotation: Quat,
                   size: Vec3,
                   look: PropKind| {
        seed += 1;
        let t = Transform::from_translation(at + (frame * local).as_dvec3())
            .with_rotation(frame * rotation);
        let mut piece = StructuralPiece::new(DestructibleMaterial::Wood, size);
        piece.break_energy_j = 6000.0;
        let e = commands
            .spawn((
                Name::new(name.to_string()),
                t,
                GlobalTransform::from(t),
                piece,
                PropModel::new(look, seed),
                BarnPiece,
            ))
            .id();
        pieces.push(e);
        e
    };
    let timber = |size: Vec3| PropKind::Timber {
        size: size.to_array(),
    };
    let (hx, hz, wall_h) = (2.2f32, 1.7f32, 2.5f32);
    // Posts at the corners and the middle of the long sides.
    for x in [-hx, 0.0, hx] {
        for z in [-hz, hz] {
            let size = Vec3::new(0.16, wall_h, 0.16);
            add(
                commands,
                "Post",
                Vec3::new(x, wall_h * 0.5, z),
                Quat::IDENTITY,
                size,
                timber(size),
            );
        }
    }
    // Top plates.
    let plate_y = wall_h + 0.08;
    for z in [-hz, hz] {
        let size = Vec3::new(2.0 * hx + 0.16, 0.16, 0.16);
        add(
            commands,
            "Plate",
            Vec3::new(0.0, plate_y, z),
            Quat::IDENTITY,
            size,
            timber(size),
        );
    }
    for x in [-hx, hx] {
        let size = Vec3::new(0.16, 0.16, 2.0 * hz - 0.16);
        add(
            commands,
            "Plate",
            Vec3::new(x, plate_y, 0.0),
            Quat::IDENTITY,
            size,
            timber(size),
        );
    }
    // Board walls just outside the posts, boards facing out: the back, both ends, and the
    // left half of the front (the right half is the open doorway).
    let t = 0.06;
    let out = 0.08 + t * 0.5;
    let board_h = wall_h - 0.1;
    let mut wall = |commands: &mut Commands, local: Vec3, yaw: f32, width: f32| {
        let size = Vec3::new(width, board_h, t);
        add(
            commands,
            "Board wall",
            local,
            Quat::from_rotation_y(yaw),
            size,
            PropKind::BoardWall {
                size: size.to_array(),
            },
        );
    };
    for x in [-hx * 0.5, hx * 0.5] {
        wall(
            commands,
            Vec3::new(x, board_h * 0.5, hz + out),
            0.0,
            hx - 0.16,
        );
    }
    wall(
        commands,
        Vec3::new(-hx * 0.5, board_h * 0.5, -hz - out),
        PI,
        hx - 0.16,
    );
    for (x, yaw) in [(hx + out, FRAC_PI_2), (-hx - out, -FRAC_PI_2)] {
        wall(
            commands,
            Vec3::new(x, board_h * 0.5, 0.0),
            yaw,
            2.0 * hz - 0.16,
        );
    }
    // Roof: two shingled panels on the plates (each panel's X runs down its slope).
    let pitch = 0.55f32;
    let run = hz + 0.3;
    let slope = run / pitch.cos();
    for s in [-1.0f32, 1.0] {
        let rot = Quat::from_rotation_x(s * pitch) * Quat::from_rotation_y(-FRAC_PI_2);
        let size = Vec3::new(slope, 0.1, 2.0 * hx + 0.5);
        add(
            commands,
            "Roof",
            Vec3::new(
                0.0,
                // The underside rests on the plate (slightly into it, so they touch).
                plate_y + 0.08 + (hz - run * 0.5) * pitch.tan() + 0.05 / pitch.cos() - 0.02,
                s * run * 0.5,
            ),
            rot,
            size,
            PropKind::ShinglePanel {
                size: size.to_array(),
                eave_at_plus_x: s > 0.0,
            },
        );
    }
    // A woodpile in the back corner: what gets lit.
    let mut kindling = Vec::new();
    for (x, y, z, along_x) in [
        (-1.3, 0.07, 1.05, true),
        (-1.3, 0.07, 0.75, true),
        (-1.3, 0.21, 0.9, false),
        (-1.0, 0.21, 0.9, false),
        (-1.15, 0.35, 0.9, true),
    ] {
        let size = if along_x {
            Vec3::new(0.9, 0.14, 0.14)
        } else {
            Vec3::new(0.14, 0.14, 0.7)
        };
        let e = add(
            commands,
            "Firewood",
            Vec3::new(x, y, z),
            Quat::IDENTITY,
            size,
            timber(size),
        );
        kindling.push(e);
    }
    (pieces, kindling)
}

/// Dry the shed's new pieces out (before `burn::make_wood_combustible_system` gives them
/// solid timber's fuel): old thin boards, a third of the fuel, little moisture (Update).
pub fn barn_fuel_system(
    mut commands: Commands,
    table: Res<DestructionMaterials>,
    pieces: Query<(Entity, &StructuralPiece), (Added<BarnPiece>, Without<Combustible>)>,
) {
    let props = table.get(DestructibleMaterial::Wood);
    for (e, piece) in &pieces {
        let h = piece.half_extents;
        let fuel = 8.0 * h.x * h.y * h.z * props.fuel_kg_m3 * FUEL_FRACTION;
        commands.entity(e).insert((
            Combustible {
                fuel_kg: fuel,
                moisture: MOISTURE,
                ignition_c: WOOD_IGNITION_C,
                ..Combustible::default()
            },
            Charring {
                fuel0_kg: fuel,
                ..Charring::default()
            },
        ));
    }
}

/// Advance every [`BarnFire`] (FixedUpdate, `SimSet::Gameplay`).
pub fn barn_script_system(
    mut commands: Commands,
    time: Res<Time>,
    mut barns: Query<&mut BarnFire>,
    mut fires: Query<&mut Combustible>,
    mut debris: Query<&mut Debris, With<BarnPiece>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for mut barn in &mut barns {
        barn.time += dt;
        if !barn.lit && barn.time >= IGNITE_AT {
            barn.lit = true;
            // A torch in the woodpile: hot enough to light it (the fire module does the rest).
            for e in &barn.kindling {
                if let Ok(mut c) = fires.get_mut(*e) {
                    c.temperature_c = c.temperature_c.max(900.0);
                }
            }
        }
        if !barn.cleared && barn.time >= CLEAR_AT {
            barn.cleared = true;
            for e in &barn.pieces {
                if let Ok(mut d) = debris.get_mut(*e) {
                    d.age = d.age.max(d.lifetime);
                }
            }
        }
        if barn.time >= CYCLE_SECS {
            rebuild(&mut commands, &mut barn);
        }
    }
}

fn rebuild(commands: &mut Commands, barn: &mut BarnFire) {
    for e in barn.pieces.drain(..) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }
    let (pieces, kindling) = build(commands, barn.origin, barn.yaw);
    barn.pieces = pieces;
    barn.kindling = kindling;
    barn.time = 0.0;
    barn.cycle += 1;
    barn.lit = false;
    barn.cleared = false;
}

/// Start every storehouse's loop over: rebuilt whole, lit again shortly. Returns how many.
pub fn restart_barns(world: &mut World) -> usize {
    let barns: Vec<Entity> = world
        .query_filtered::<Entity, With<BarnFire>>()
        .iter(world)
        .collect();
    for &e in &barns {
        let Some(mut barn) = world.get::<BarnFire>(e).cloned() else {
            continue;
        };
        let mut queue = bevy_ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, world);
            rebuild(&mut commands, &mut barn);
        }
        queue.apply(world);
        if let Some(mut b) = world.get_mut::<BarnFire>(e) {
            *b = barn;
        }
    }
    barns.len()
}

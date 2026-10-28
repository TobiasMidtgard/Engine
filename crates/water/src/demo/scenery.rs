//! Scenery shared by the test area and the valley lake: a timber jetty on piles, boats and
//! buoys, drawn with the detailed [`props`](super::props) and their textured
//! [`materials`](super::materials).

use engine_assets::Assets;
use engine_core::math::{DVec2, Mat4};
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRenderer, StandardMaterial};
use engine_world::physics::{Fit, FitCollider};

use super::props::{self, Part};
use crate::boat::Boat;
use crate::buoyancy::{Buoyancy, Floating, Tether};
use crate::interact::WaterInteractor;

/// Draw `parts` on `entity`: one child per part at the entity's origin, each with its
/// surface's textured material. The textures are generated the first time (about a second);
/// worlds without a mesh store (tests) get no meshes.
pub fn add_parts(commands: &mut Commands, entity: Entity, parts: Vec<Part>) {
    commands.queue(move |world: &mut World| {
        if !world.contains_resource::<Assets<Mesh>>() {
            return;
        }
        let materials: Vec<StandardMaterial> = {
            let library = super::art::materials(world);
            parts
                .iter()
                .map(|p| {
                    let mut m = library.get(p.surface);
                    m.double_sided |= p.double_sided;
                    m
                })
                .collect()
        };
        if world.get_entity(entity).is_err() {
            return;
        }
        for (part, material) in parts.into_iter().zip(materials) {
            let mesh = world.resource_mut::<Assets<Mesh>>().add(part.mesh);
            world.spawn((
                Name::new("Part"),
                Transform::default(),
                ChildOf(entity),
                MeshRenderer::new(mesh, material),
            ));
        }
    });
}

/// Spawn a prop: an entity at `transform` drawn with `parts`.
pub fn spawn_prop(
    commands: &mut Commands,
    name: &str,
    transform: Transform,
    parts: Vec<Part>,
) -> Entity {
    let e = commands.spawn((Name::new(name.to_owned()), transform)).id();
    add_parts(commands, e, parts);
    e
}

/// Copies of props placed by `transform`s (relative to their entity), merged into one part
/// per surface so a whole structure costs a handful of draws.
pub fn merged(placed: impl IntoIterator<Item = (Vec<Part>, Mat4)>) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    for (parts, m) in placed {
        for mut part in parts {
            part.mesh.transform(&m);
            match out
                .iter_mut()
                .find(|o| o.surface == part.surface && o.double_sided == part.double_sided)
            {
                Some(o) => o.mesh.merge(&part.mesh),
                None => out.push(part),
            }
        }
    }
    out
}

/// A timber jetty on piles, from a point on the bank straight out over the water.
pub struct Jetty<'a> {
    /// Where the deck meets the bank (x, z) and the deck's top (y).
    pub shore: DVec3,
    /// Direction out over the water (x, z), any length.
    pub dir: DVec2,
    pub length: f32,
    pub water_level: f64,
    /// Ground height at (x, z), for the piles.
    pub bed: &'a dyn Fn(f64, f64) -> f64,
    pub seed: u64,
}

impl Jetty<'_> {
    /// Unit direction out over the water.
    pub fn out(&self) -> DVec2 {
        self.dir.normalize_or(DVec2::X)
    }

    /// Turn that points a prop's +x out along the jetty.
    pub fn rotation(&self) -> Quat {
        let d = self.out();
        Quat::from_rotation_y((-d.y).atan2(d.x) as f32)
    }

    /// The deck-top point `along` meters out and `across` meters along the jetty's own +z
    /// (to the right, looking out).
    pub fn point(&self, along: f64, across: f64) -> DVec3 {
        let d = self.out();
        let p = DVec2::new(self.shore.x, self.shore.z) + d * along + DVec2::new(-d.y, d.x) * across;
        DVec3::new(p.x, self.shore.y, p.y)
    }

    /// The deck-top point at the far end, on the middle line.
    pub fn end(&self) -> DVec3 {
        self.point(self.length as f64, 0.0)
    }
}

/// Deck slot width of [`props::deck_section`], meters.
const SLOT: f32 = 0.55;
/// Distance between pile pairs along the jetty, meters.
const BAY: f32 = 2.2;
/// Piles stand just outside the 2 m boards, either side of the middle line, meters.
const POST_ACROSS: f32 = 1.12;

/// Build `jetty`: deck slots, pile pairs to the bottom, cleats and a swim ladder at the end.
pub fn spawn_jetty(commands: &mut Commands, jetty: &Jetty) -> Entity {
    let origin = jetty.shore;
    let rot = jetty.rotation();
    // Everything is placed in the jetty's own frame: +x out, +z to the right, y up from
    // the deck top.
    let local =
        |along: f32, y: f32, across: f32| Mat4::from_translation(Vec3::new(along, y, across));
    let mut placed = Vec::new();
    let slots = (jetty.length / SLOT).ceil() as u32;
    for k in 0..slots {
        let along = (k as f32 + 0.5) * SLOT;
        placed.push((
            props::deck_section(jetty.seed.wrapping_add(k as u64)),
            local(along, -0.035, 0.0),
        ));
    }
    let bays = (jetty.length / BAY).floor() as u32;
    for k in 0..=bays {
        let along = (k as f32 * BAY).min(jetty.length - 0.15).max(0.15);
        for (s, across) in [(0u64, -POST_ACROSS), (1, POST_ACROSS)] {
            let p = jetty.point(along as f64, across as f64);
            let bottom = (jetty.bed)(p.x, p.z) - 0.4;
            let top = origin.y + 0.25;
            let height = (top - bottom) as f32;
            if height < 0.5 {
                continue;
            }
            let waterline = (jetty.water_level - bottom) as f32;
            placed.push((
                props::post(height, waterline, jetty.seed ^ (k as u64 * 2 + s)),
                local(along, (bottom - origin.y) as f32, across),
            ));
        }
    }
    for across in [-0.82f32, 0.82] {
        placed.push((props::cleat(), local(jetty.length - 0.6, 0.0, across)));
    }
    let ladder = (origin.y - jetty.water_level) as f32 + 0.9;
    placed.push((props::ladder(ladder), local(jetty.length, 0.0, 0.45)));
    let e = commands
        .spawn((
            Name::new("Jetty"),
            Transform::from_translation(origin).with_rotation(rot),
            // The deck stands over the wet band at the shore: keep it out of the floor.
            crate::shore::ShoreCover {
                min: [-0.3, -POST_ACROSS],
                max: [jetty.length, POST_ACROSS],
            },
            // Solid as drawn (deck, piles, cleats, ladder): physics fits the triangles once
            // the merged parts' meshes are in.
            FitCollider::new(Fit::Mesh),
        ))
        .id();
    add_parts(commands, e, merged(placed));
    e
}

/// A rowboat floating at `at` (on the water), bow along `yaw`, tied to `anchor`. A physics
/// body once a physics backend is installed ([`Boat`]).
pub fn spawn_rowboat(commands: &mut Commands, at: DVec3, yaw: f32, anchor: DVec3) -> Entity {
    let (parts, hull, inside) = super::art::rowboat();
    let e = commands
        .spawn((
            Name::new("Rowboat"),
            Transform::from_translation(at + DVec3::Y * 0.1)
                .with_rotation(Quat::from_rotation_y(yaw)),
            Buoyancy::new(Vec3::new(1.7, 0.3, 0.6), 420.0)
                .with_fill(0.45)
                .with_probes(3, 2),
            Floating::default(),
            Tether {
                anchor,
                length_m: 2.8,
                stiffness: 900.0,
            },
            // Painter on the stem head.
            Boat::new(hull, 140.0, 0.16).with_interior(inside).moored(
                Vec3::new(1.72, 0.4, 0.0),
                anchor,
                2.8,
            ),
            WaterInteractor::new(0.6).with_wake(0.6),
        ))
        .id();
    add_parts(commands, e, parts);
    e
}

/// A motorboat floating at `at`, bow along `yaw`; give it a role or a mooring
/// ([`moor_motorboat`]) to keep it busy. A physics body once a physics backend is installed.
pub fn spawn_motorboat(commands: &mut Commands, at: DVec3, yaw: f32) -> Entity {
    let (parts, hull, inside) = super::art::motorboat();
    let e = commands
        .spawn((
            Name::new("Motorboat"),
            Transform::from_translation(at + DVec3::Y * 0.1)
                .with_rotation(Quat::from_rotation_y(yaw)),
            Buoyancy {
                drag: 0.12,
                angular_drag: 0.8,
                ..Buoyancy::new(Vec3::new(2.4, 0.45, 0.9), 380.0)
                    .with_fill(0.5)
                    .with_probes(3, 2)
            },
            Floating::default(),
            Boat::new(hull, 650.0, 0.3).with_interior(inside),
            WaterInteractor::new(0.9).with_wake(2.5),
        ))
        .id();
    add_parts(commands, e, parts);
    e
}

/// Tie a motorboat's bow to `anchor` with `length_m` of rope (`stiffness` N/m for the water
/// module's own float, when no physics backend runs).
pub fn moor_motorboat(
    commands: &mut Commands,
    boat: Entity,
    anchor: DVec3,
    length_m: f32,
    stiffness: f32,
) {
    commands.entity(boat).insert(Tether {
        anchor,
        length_m,
        stiffness,
    });
    commands.queue(move |world: &mut World| {
        if let Some(mut b) = world.get_mut::<Boat>(boat) {
            b.mooring = Some((Vec3::new(2.5, 0.42, 0.0), anchor, length_m));
        }
    });
}

/// A mooring buoy floating at `at`, roped to `anchor` on the bottom.
pub fn spawn_buoy(commands: &mut Commands, at: DVec3, anchor: DVec3, seed: u64) -> Entity {
    let e = commands
        .spawn((
            Name::new("Buoy"),
            Transform::from_translation(at + DVec3::Y * 0.3),
            Buoyancy::sphere(0.32, 220.0),
            Floating::default(),
            Tether {
                anchor,
                length_m: 0.6,
                stiffness: 300.0,
            },
        ))
        .id();
    add_parts(commands, e, props::buoy(seed));
    e
}

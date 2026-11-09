//! Smoke and wakes: chimneys smoke over lived-in houses, workshop stacks while they work,
//! steamers from their funnels, and every hull draws a foam wake while it moves.
//!
//! Effects are `engine_vfx` particle effects defined here as data; each plume or wake is an
//! entity placed at a building's chimney or attached to a ship.

use engine::prelude::*;
use engine_vfx::{
    Curve, EffectDesc, EffectLibrary, EmitShape, EmitterDesc, Facing, Gradient, ParticleEffect,
    Sprite,
};

use crate::build::MovingParts;
use crate::defs::{Kind, Tier};
use crate::grid;
use crate::sim::{Building, House, Producer};
use crate::vessels::Craft;

fn smoke_layer(
    name: &str,
    albedo: f32,
    peak: f32,
    rate: f32,
    life: [f32; 2],
    size: [f32; 2],
    grow: f32,
) -> EmitterDesc {
    let a = albedo;
    EmitterDesc {
        name: name.into(),
        rate,
        shape: EmitShape::Cone {
            angle_deg: 8.0,
            radius: 0.25,
        },
        speed: [0.8, 1.6],
        acceleration: [0.0, 0.9, 0.0],
        drag: 1.2,
        wind: 1.0,
        turbulence: 0.5,
        turbulence_scale: 2.5,
        lifetime: life,
        size,
        spin_deg: [-20.0, 20.0],
        rotation_deg: [0.0, 360.0],
        sprite: Sprite::Smoke,
        lit: 1.0,
        color: Gradient::new(&[
            (0.0, [a, a * 0.97, a * 0.94, 0.0]),
            (0.08, [a, a * 0.97, a * 0.94, peak]),
            (1.0, [a * 1.3, a * 1.28, a * 1.26, 0.0]),
        ]),
        size_over_life: Curve::new(&[(0.0, 0.5), (1.0, grow)]),
        soft: 0.6,
        erode: 0.3,
        ..Default::default()
    }
}

/// The game's effects, added to the effect library at start.
pub fn effects() -> Vec<EffectDesc> {
    let fx = |name: &str, description: &str, prewarm: f32, emitters: Vec<EmitterDesc>| EffectDesc {
        name: name.into(),
        description: description.into(),
        prewarm,
        emitters,
        ..Default::default()
    };
    vec![
        fx(
            "isle.chimney",
            "Wood smoke from a cottage or townhouse chimney: a thin pale wisp leaning with the wind.",
            12.0,
            vec![smoke_layer(
                "smoke",
                0.72,
                0.62,
                3.5,
                [8.0, 12.0],
                [1.5, 2.1],
                5.0,
            )],
        ),
        fx(
            "isle.stack",
            "Coal smoke from a workshop stack: a thicker grey plume.",
            15.0,
            vec![smoke_layer(
                "smoke",
                0.34,
                0.8,
                5.0,
                [10.0, 15.0],
                [2.8, 3.6],
                5.5,
            )],
        ),
        fx(
            "isle.funnel",
            "A steamer's funnel: dark coal smoke streaming back.",
            8.0,
            vec![EmitterDesc {
                inherit_velocity: 0.0,
                ..smoke_layer("smoke", 0.14, 0.9, 9.0, [9.0, 13.0], [3.6, 4.6], 5.0)
            }],
        ),
        fx(
            "isle.wake",
            "Foam behind a moving hull: flat white patches that spread and fade.",
            0.0,
            vec![EmitterDesc {
                name: "foam".into(),
                rate_per_meter: 0.9,
                shape: EmitShape::Disc { radius: 1.2 },
                speed: [0.2, 0.6],
                lifetime: [9.0, 14.0],
                size: [3.0, 4.5],
                rotation_deg: [0.0, 360.0],
                spin_deg: [-6.0, 6.0],
                sprite: Sprite::Foam,
                facing: Facing::Horizontal,
                lit: 1.0,
                color: Gradient::new(&[
                    (0.0, [0.95, 0.97, 0.98, 0.0]),
                    (0.04, [0.97, 0.98, 0.99, 0.95]),
                    (1.0, [0.9, 0.94, 0.96, 0.0]),
                ]),
                size_over_life: Curve::new(&[(0.0, 0.7), (1.0, 3.6)]),
                erode: 0.6,
                ..Default::default()
            }],
        ),
    ]
}

pub fn register(world: &mut World) {
    let Some(mut lib) = world.get_resource_mut::<EffectLibrary>() else {
        return;
    };
    for e in effects() {
        if let Err(err) = lib.insert(e) {
            engine::core::log::warn!("archipelago effect: {err}");
        }
    }
}

/// A smoke plume belonging to a building, at one of its chimneys.
#[derive(Component, Clone, Copy, Debug)]
pub struct Plume {
    pub building: Entity,
    pub slot: u8,
}

/// An effect riding on a ship or aircraft at `offset` in its own space.
#[derive(Component, Clone, Copy, Debug)]
pub struct Riding {
    pub on: Entity,
    pub offset: Vec3,
}

/// Chimney tops of a building, in its own space, and which effect each one plays.
pub fn chimneys(kind: Kind, tier: Tier, seed: u64) -> Vec<(Vec3, &'static str)> {
    let v = crate::looks::variant(seed);
    match kind {
        Kind::House => match tier {
            Tier::Fisherfolk => {
                let x = if v == 2 { -2.24 } else { 2.24 };
                vec![(Vec3::new(x, 7.6, 1.2), "isle.chimney")]
            }
            Tier::Labourers => vec![(Vec3::new(3.7, 10.9, 0.8), "isle.chimney")],
        },
        Kind::Bakery => vec![(Vec3::new(-4.1, 9.2, 0.0), "isle.stack")],
        Kind::Brewery => vec![(Vec3::new(4.6, 16.2, -3.6), "isle.stack")],
        Kind::Brickworks => vec![(Vec3::new(5.5, 19.5, -1.5), "isle.stack")],
        _ => vec![],
    }
}

/// Spawn the plumes of a building just placed.
pub fn add_plumes(world: &mut World, e: Entity) {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    let n = chimneys(b.kind, Tier::Fisherfolk, b.seed).len();
    for slot in 0..n {
        let p = world
            .spawn((
                Name::new("chimney smoke"),
                Transform::default(),
                ParticleEffect::new(chimneys(b.kind, Tier::Fisherfolk, b.seed)[slot].1),
                Plume {
                    building: e,
                    slot: slot as u8,
                },
            ))
            .id();
        world
            .entity_mut(e)
            .entry::<MovingParts>()
            .or_default()
            .get_mut()
            .0
            .push(p);
    }
}

/// Keep plumes on their chimneys (houses change shape when they move up a tier) and smoking
/// only while someone lives there or the workshop works.
pub fn tend_plumes(world: &mut World, mut timer: Local<f32>) {
    *timer -= world.resource::<Time>().delta_secs().max(1.0 / 120.0);
    if *timer > 0.0 {
        return;
    }
    *timer = 0.5;
    let mut q = world.query::<(Entity, &Plume)>();
    let plumes: Vec<(Entity, Plume)> = q.iter(world).map(|(e, p)| (e, *p)).collect();
    for (e, plume) in plumes {
        let Some(b) = world.get::<Building>(plume.building).cloned() else {
            continue;
        };
        let (tier, rate) = if let Some(h) = world.get::<House>(plume.building) {
            (h.tier, if h.residents > 0.5 { 1.0 } else { 0.0 })
        } else {
            let eff = world
                .get::<Producer>(plume.building)
                .map_or(0.0, |p| p.efficiency);
            (
                Tier::Fisherfolk,
                if eff > 0.05 { 0.4 + 0.6 * eff } else { 0.0 },
            )
        };
        let spots = chimneys(b.kind, tier, b.seed);
        let Some(&(local, name)) = spots.get(plume.slot as usize) else {
            continue;
        };
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let rot = Quat::from_rotation_y(grid::yaw(b.rot));
        let at = DVec3::new(c.x, b.base_y, c.y) + (rot * local).as_dvec3();
        if let Some(mut t) = world.get_mut::<Transform>(e) {
            t.translation = at;
        }
        if let Some(mut fx) = world.get_mut::<ParticleEffect>(e) {
            if fx.effect != name {
                fx.effect = name.into();
            }
            // Stop and start rather than run at rate 0 (an emitter at rate 0 counts as done).
            let on = rate > 0.0;
            if fx.playing != on {
                fx.playing = on;
            }
            if on && (fx.rate - rate).abs() > 0.01 {
                fx.rate = rate;
            }
        }
    }
}

/// Effects a ship or aircraft carries, in its own space (+X forward).
pub fn riders(craft: Craft) -> Vec<(Vec3, &'static str)> {
    let stern = -craft.half_length() + 1.5;
    match craft {
        Craft::Steamer => vec![
            (Vec3::new(-4.5, 13.4, 0.0), "isle.funnel"),
            (Vec3::new(stern, 0.15, 0.0), "isle.wake"),
        ],
        Craft::PaddleSteamer => vec![
            (Vec3::new(5.0, 14.2, 0.0), "isle.funnel"),
            (Vec3::new(stern, 0.15, 0.0), "isle.wake"),
        ],
        Craft::Airship | Craft::Biplane | Craft::Seaplane => vec![],
        _ => vec![(Vec3::new(stern, 0.15, 0.0), "isle.wake")],
    }
}

/// Give a vessel its funnel smoke and wake.
pub fn add_riders(world: &mut World, vessel: Entity, craft: Craft) {
    for (offset, name) in riders(craft) {
        let scale = if name == "isle.wake" {
            (craft.half_length() / 20.0).clamp(0.4, 1.4)
        } else {
            1.0
        };
        world.spawn((
            Name::new(format!("{name} on {}", craft.name())),
            Transform::default(),
            ParticleEffect::new(name).with_scale(scale),
            Riding { on: vessel, offset },
        ));
    }
}

/// Move riding effects with their vessels.
pub fn follow_riders(
    mut riders: Query<(&Riding, &mut Transform)>,
    bodies: Query<&Transform, Without<Riding>>,
) {
    for (r, mut t) in &mut riders {
        if let Ok(parent) = bodies.get(r.on) {
            t.translation = parent.translation + (parent.rotation * r.offset).as_dvec3();
            t.rotation = parent.rotation;
        }
    }
}

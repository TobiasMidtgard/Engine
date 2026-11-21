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
use crate::globe::{Biome, Frame};
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
                0.85,
                0.3,
                3.0,
                [6.0, 9.0],
                [0.8, 1.1],
                4.2,
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
        show(SHOW_S),
    ]
}

/// One colour of firework star: a burst of sparks at `height` every `every` seconds from
/// `delay`, falling and fading in `col`.
fn stars(
    name: &str,
    height: f32,
    delay: f32,
    every: f32,
    col: [f32; 3],
    lasts: f32,
) -> EmitterDesc {
    let [r, g, b] = col;
    EmitterDesc {
        name: name.into(),
        bursts: vec![engine_vfx::Burst {
            time: 0.0,
            count: 150,
            repeat: every,
            cycles: (lasts / every).max(1.0) as u32,
        }],
        delay,
        duration: Some(lasts),
        shape: EmitShape::Sphere {
            radius: 0.6,
            surface: true,
        },
        offset: [0.0, height, 0.0],
        speed: [22.0, 30.0],
        gravity: 0.35,
        drag: 1.6,
        lifetime: [1.6, 2.4],
        size: [0.6, 0.85],
        sprite: Sprite::Spark,
        facing: Facing::Velocity,
        stretch: 0.05,
        color: Gradient::new(&[
            (0.0, [1.0, 1.0, 0.95, 1.0]),
            (0.15, [r, g, b, 1.0]),
            (0.7, [r * 0.9, g * 0.8, b * 0.8, 0.8]),
            (1.0, [r * 0.6, g * 0.4, b * 0.3, 0.0]),
        ]),
        emissive: 40_000.0,
        additive: 1.0,
        soft: 0.0,
        max_particles: 2000,
        ..Default::default()
    }
}

/// Rising shell trails: thin gold streaks shot up to where the stars burst.
fn shells(lasts: f32) -> EmitterDesc {
    EmitterDesc {
        name: "shells".into(),
        rate: 1.6,
        duration: Some(lasts),
        shape: EmitShape::Cone {
            angle_deg: 9.0,
            radius: 3.0,
        },
        speed: [32.0, 38.0],
        gravity: 1.0,
        lifetime: [1.0, 1.3],
        size: [0.25, 0.3],
        sprite: Sprite::Spark,
        facing: Facing::Velocity,
        stretch: 0.06,
        color: Gradient::new(&[(0.0, [1.0, 0.8, 0.45, 1.0]), (1.0, [1.0, 0.5, 0.2, 0.0])]),
        emissive: 20_000.0,
        additive: 1.0,
        ..Default::default()
    }
}

/// A building on fire: flames over the roof, a column of black smoke seen across the island,
/// embers riding the draft.
fn blaze() -> EffectDesc {
    EffectDesc {
        name: "isle.blaze".into(),
        description: "A house or workshop on fire: tall flames out of the roof, thick dark \
                      smoke rising in a column, sparks and embers in the draft."
            .into(),
        prewarm: 4.0,
        emitters: vec![
            EmitterDesc {
                name: "blaze".into(),
                rate: 40.0,
                offset: [0.0, 3.5, 0.0],
                shape: EmitShape::Cone {
                    angle_deg: 6.0,
                    radius: 3.2,
                },
                speed: [1.0, 2.4],
                acceleration: [0.0, 3.0, 0.0],
                drag: 1.2,
                turbulence: 1.5,
                turbulence_scale: 2.0,
                wind: 0.4,
                lifetime: [1.0, 1.7],
                size: [3.0, 4.6],
                size_over_life: Curve::new(&[(0.0, 0.6), (0.3, 1.0), (1.0, 0.3)]),
                rotation_deg: [-10.0, 10.0],
                sprite: Sprite::Flame,
                color: Gradient::new(&[
                    (0.0, [1.0, 0.72, 0.32, 0.0]),
                    (0.1, [1.0, 0.66, 0.26, 0.95]),
                    (0.5, [1.0, 0.5, 0.15, 0.6]),
                    (1.0, [0.9, 0.3, 0.08, 0.0]),
                ]),
                emissive: 12_000.0,
                additive: 0.8,
                soft: 0.3,
                ..Default::default()
            },
            EmitterDesc {
                name: "billows".into(),
                rate: 6.0,
                offset: [0.0, 5.0, 0.0],
                shape: EmitShape::Cone {
                    angle_deg: 10.0,
                    radius: 2.5,
                },
                speed: [1.5, 3.0],
                acceleration: [0.0, 1.5, 0.0],
                drag: 0.8,
                wind: 0.6,
                lifetime: [1.4, 2.2],
                size: [3.0, 4.5],
                size_over_life: Curve::new(&[(0.0, 0.5), (1.0, 1.6)]),
                rotation_deg: [0.0, 360.0],
                spin_deg: [-30.0, 30.0],
                sprite: Sprite::Billow,
                color: Gradient::new(&[
                    (0.0, [1.0, 0.55, 0.2, 0.0]),
                    (0.1, [1.0, 0.5, 0.18, 0.9]),
                    (1.0, [0.18, 0.16, 0.15, 0.0]),
                ]),
                emissive: 4_000.0,
                additive: 0.15,
                soft: 0.5,
                ..Default::default()
            },
            EmitterDesc {
                offset: [0.0, 7.0, 0.0],
                ..smoke_layer("column", 0.16, 0.75, 7.0, [12.0, 18.0], [4.0, 6.0], 5.0)
            },
            EmitterDesc {
                name: "embers".into(),
                rate: 14.0,
                offset: [0.0, 4.0, 0.0],
                shape: EmitShape::Cone {
                    angle_deg: 25.0,
                    radius: 3.0,
                },
                speed: [2.0, 5.0],
                acceleration: [0.0, 1.5, 0.0],
                drag: 0.6,
                turbulence: 3.0,
                turbulence_scale: 1.5,
                wind: 0.8,
                lifetime: [1.5, 3.0],
                size: [0.12, 0.2],
                sprite: Sprite::Glow,
                color: Gradient::new(&[(0.0, [1.0, 0.8, 0.4, 1.0]), (1.0, [1.0, 0.3, 0.05, 0.0])]),
                emissive: 20_000.0,
                additive: 1.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// A burnt-out shell: embers glowing in the rubble and a thin grey smoke.
fn embers() -> EffectDesc {
    EffectDesc {
        name: "isle.embers".into(),
        description: "A burnt-out ruin: a few embers glowing in the ash and a thin grey smoke."
            .into(),
        prewarm: 8.0,
        emitters: vec![
            EmitterDesc {
                offset: [0.0, 1.0, 0.0],
                ..smoke_layer("ash smoke", 0.45, 0.35, 1.5, [8.0, 12.0], [2.0, 3.0], 3.5)
            },
            EmitterDesc {
                name: "glow".into(),
                rate: 3.0,
                offset: [0.0, 0.3, 0.0],
                shape: EmitShape::Disc { radius: 3.0 },
                speed: [0.0, 0.2],
                lifetime: [1.5, 3.0],
                size: [0.3, 0.6],
                sprite: Sprite::Glow,
                color: Gradient::new(&[
                    (0.0, [1.0, 0.35, 0.08, 0.0]),
                    (0.3, [1.0, 0.35, 0.08, 0.8]),
                    (1.0, [0.8, 0.15, 0.02, 0.0]),
                ]),
                emissive: 6_000.0,
                additive: 1.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// Torches in a crowd of rioters: a ring of small flames at head height.
fn torches() -> EffectDesc {
    EffectDesc {
        name: "isle.torches".into(),
        description: "Torches held up in a crowd: small flames bobbing at head height and a \
                      little smoke."
            .into(),
        prewarm: 2.0,
        emitters: vec![
            EmitterDesc {
                name: "torches".into(),
                rate: 30.0,
                offset: [0.0, 2.4, 0.0],
                shape: EmitShape::Disc { radius: 5.0 },
                speed: [0.2, 0.5],
                acceleration: [0.0, 2.0, 0.0],
                drag: 1.5,
                turbulence: 2.0,
                turbulence_scale: 0.4,
                lifetime: [0.3, 0.5],
                size: [0.3, 0.45],
                sprite: Sprite::Flame,
                color: Gradient::new(&[
                    (0.0, [1.0, 0.7, 0.3, 0.0]),
                    (0.15, [1.0, 0.62, 0.24, 0.9]),
                    (1.0, [0.9, 0.3, 0.08, 0.0]),
                ]),
                emissive: 9_000.0,
                additive: 0.9,
                ..Default::default()
            },
            EmitterDesc {
                offset: [0.0, 3.0, 0.0],
                ..smoke_layer("torch smoke", 0.3, 0.25, 4.0, [3.0, 5.0], [0.5, 0.9], 3.0)
            },
        ],
        ..Default::default()
    }
}

/// How long the Fireworks item burns.
pub const SHOW_S: f32 = 90.0;

/// The show a Town Hall puts on (the Fireworks item), `lasts` seconds long.
fn show(lasts: f32) -> EffectDesc {
    EffectDesc {
        name: "isle.fireworks".into(),
        description: "A town's firework show: shells climb from the square and burst in gold, \
                      red, green and blue stars over the roofs."
            .into(),
        emitters: vec![
            shells(lasts),
            stars("gold", 40.0, 0.0, 2.3, [1.0, 0.78, 0.3], lasts),
            stars("red", 48.0, 0.3, 2.9, [1.0, 0.22, 0.18], lasts),
            stars("green", 34.0, 0.6, 3.4, [0.35, 1.0, 0.4], lasts),
            stars("blue", 54.0, 0.9, 3.8, [0.45, 0.6, 1.0], lasts),
        ],
        ..Default::default()
    }
}

/// Start a firework show over building `e` (it burns as long as the item acts).
pub fn fireworks(world: &mut World, e: Entity) {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    let c = grid::footprint_center(b.kind, b.origin, b.rot);
    let at = DVec3::new(c.x, b.base_y, c.y);
    let Some(stand) = world.get_resource::<Frame>().map(|f| f.stand(at, 0.0)) else {
        return;
    };
    let mut fx = ParticleEffect::new("isle.fireworks");
    fx.despawn_when_done = true;
    world.spawn((Name::new("fireworks"), stand, fx));
}

pub fn register(world: &mut World) {
    let Some(mut lib) = world.get_resource_mut::<EffectLibrary>() else {
        return;
    };
    for e in effects().into_iter().chain([blaze(), embers(), torches()]) {
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
pub fn chimneys(kind: Kind, tier: Tier, seed: u64, biome: Biome) -> Vec<(Vec3, &'static str)> {
    let tier = crate::looks::house_tier(tier, seed);
    let v = crate::looks::variant(seed);
    match kind {
        // Far houses: no hearth smoke in the heat; a stone chimney in the north.
        Kind::House if matches!(biome, Biome::Tropical | Biome::Arid) => vec![],
        Kind::Glassworks => vec![(Vec3::new(4.0, 11.5, -2.5), "isle.stack")],
        Kind::Saltery => vec![(Vec3::new(-3.2, 7.5, -5.8), "isle.chimney")],
        Kind::Confectioner => vec![(Vec3::new(-3.0, 9.6, -3.3), "isle.chimney")],
        Kind::Furrier => vec![(Vec3::new(3.0, 10.8, -3.8), "isle.chimney")],
        Kind::LampMaker => vec![(Vec3::new(-2.8, 8.4, -3.3), "isle.chimney")],
        Kind::School => vec![(Vec3::new(4.2, 10.2, -3.0), "isle.chimney")],
        Kind::Tavern => vec![(Vec3::new(4.45, 11.9, -1.4), "isle.chimney")],
        Kind::House if biome == Biome::Arctic => match tier {
            Tier::Fisherfolk => {
                let x = if v == 2 { -1.98 } else { 1.98 };
                vec![(Vec3::new(x, 6.2, -0.2), "isle.chimney")]
            }
            Tier::Labourers => vec![(Vec3::new(2.4, 9.9, -0.4), "isle.chimney")],
            Tier::Burghers => burgher_smoke(biome),
            t => crate::patrician::house_smoke(t, biome),
        },
        Kind::Distillery => vec![(Vec3::new(6.6, 13.7, -7.0), "isle.stack")],
        Kind::OilWorks => vec![(Vec3::new(6.9, 12.2, -6.9), "isle.stack")],
        Kind::TrapperLodge => vec![(Vec3::new(-4.82, 5.9, -1.0), "isle.chimney")],
        Kind::House => match tier {
            Tier::Fisherfolk => {
                let x = if v == 2 { -2.24 } else { 2.24 };
                vec![(Vec3::new(x, 7.6, 1.2), "isle.chimney")]
            }
            Tier::Labourers => vec![(Vec3::new(3.7, 10.9, 0.8), "isle.chimney")],
            Tier::Burghers => burgher_smoke(biome),
            t => crate::patrician::house_smoke(t, biome),
        },
        Kind::Ironworks => vec![
            (Vec3::new(-8.2, 16.2, -5.8), "isle.stack"),
            (Vec3::new(2.0, 18.2, -6.2), "isle.stack"),
            (Vec3::new(5.6, 14.8, -2.4), "isle.stack"),
        ],
        Kind::GoldMine => vec![(Vec3::new(-6.0, 16.2, -6.0), "isle.stack")],
        Kind::OreMine => vec![(Vec3::new(-6.0, 9.8, -5.6), "isle.chimney")],
        Kind::Roastery => vec![(Vec3::new(0.0, 11.2, 4.2), "isle.chimney")],
        Kind::Chocolatier => vec![(Vec3::new(-4.6, 13.8, -3.2), "isle.chimney")],
        Kind::Bakery => vec![(Vec3::new(-4.1, 9.2, 0.0), "isle.stack")],
        Kind::Brewery => vec![(Vec3::new(4.6, 16.2, -3.6), "isle.stack")],
        Kind::Brickworks => vec![(Vec3::new(5.5, 19.5, -1.5), "isle.stack")],
        _ => vec![],
    }
}

fn burgher_smoke(biome: Biome) -> Vec<(Vec3, &'static str)> {
    crate::burgher::house_chimney(biome)
        .map(|at| (at, "isle.chimney"))
        .into_iter()
        .collect()
}

/// Spawn the plumes of a building just placed.
pub fn add_plumes(world: &mut World, e: Entity) {
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    let biome = crate::island::islands()[b.island].biome();
    let n = chimneys(b.kind, Tier::Fisherfolk, b.seed, biome).len();
    for slot in 0..n {
        let p = world
            .spawn((
                Name::new("chimney smoke"),
                Transform::default(),
                ParticleEffect::new(chimneys(b.kind, Tier::Fisherfolk, b.seed, biome)[slot].1),
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
        let biome = crate::island::islands()[b.island].biome();
        let spots = chimneys(b.kind, tier, b.seed, biome);
        let Some(&(local, name)) = spots.get(plume.slot as usize) else {
            continue;
        };
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let rot = Quat::from_rotation_y(grid::yaw(b.rot));
        let at = DVec3::new(c.x, b.base_y, c.y) + (rot * local).as_dvec3();
        let stand = world.resource::<Frame>().stand(at, 0.0);
        if let Some(mut t) = world.get_mut::<Transform>(e)
            && (t.translation - stand.translation).length_squared() > 1e-4
        {
            *t = stand;
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
        Craft::Gunboat => vec![
            (Vec3::new(-3.5, 7.4, 0.0), "isle.funnel"),
            (Vec3::new(stern, 0.15, 0.0), "isle.wake"),
        ],
        Craft::Liner => vec![
            (Vec3::new(8.0, 24.0, 0.0), "isle.funnel"),
            (Vec3::new(-3.0, 24.0, 0.0), "isle.funnel"),
            (Vec3::new(-14.0, 24.0, 0.0), "isle.funnel"),
            (Vec3::new(stern, 0.15, 0.0), "isle.wake"),
        ],
        Craft::Dreadnought => vec![
            (Vec3::new(10.0, 31.4, 0.0), "isle.funnel"),
            (Vec3::new(-22.0, 31.6, 0.0), "isle.funnel"),
            (Vec3::new(stern, 0.15, 0.0), "isle.wake"),
        ],
        Craft::Leviathan => [38.0f32, 23.0, -26.0, -40.0, -54.0]
            .into_iter()
            .map(|x| {
                (
                    Vec3::new(x, 31.5 + (x / 90.0).powi(2) * 4.0, 0.0),
                    "isle.funnel",
                )
            })
            .chain([(Vec3::new(stern, 0.15, 0.0), "isle.wake")])
            .collect(),
        Craft::Airship
        | Craft::Biplane
        | Craft::Seaplane
        | Craft::FlyingBoat
        | Craft::SkyCarrier => vec![],
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

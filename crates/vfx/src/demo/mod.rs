//! The Spell court: the showcase's particle and magic test area.
//!
//! [`spawn_demo`] builds it around a point. Everything moves on its own (the systems below
//! are added by [`crate::VfxPlugin`]) and follows world time, so slow motion slows the
//! spells too. Laid out on a 16 m stone disc, with +X east and +Z toward the default
//! viewpoint:
//!
//! | Where | Test case |
//! |---|---|
//! | north | Arcane portal: rune circles, spiraling motes, orbiting orbs with trails |
//! | middle lane | Fireballs and frost bolts flying into a target wall, with impacts and shockwaves |
//! | west | Lightning arcing between two obelisks, striking with sparks |
//! | center | Healing aura on a dais |
//! | east | Fountain (water jets landing in a basin) and an anvil throwing sparks |
//! | south | Pond with stones splashing in, footsteps on mud, snow, sand and water, impacts on six materials |
//! | rim | Torches and fireflies |

pub mod art;

use engine_core::math::{DVec3, Quat, Rng, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, PointLight, Shape, StandardMaterial};
use engine_world::{ContactKind, GroundContact, SurfaceMaterials, WorldImpulse};

use crate::emitter::ParticleEffect;
use crate::events::spawn_effect_at;
use crate::trail::Beam;

/// A viewpoint on the court: eye and target relative to the point passed to [`spawn_demo`],
/// the hour of day it looks best at, and how long to let the effects run before a screenshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup: f32,
}

/// Suggested viewpoints (camera bookmarks and screenshot-tour stops).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "magic_spells",
        name: "Spells at dusk: portal, lightning, fireballs and frost",
        eye: [10.0, 4.8, 16.0],
        target: [-2.0, 1.4, -4.0],
        hour: 18.3,
        warmup: 0.0,
    },
    DemoView {
        key: "magic_splashes",
        name: "Stones splashing into a pond",
        eye: [1.5, 1.9, 5.2],
        target: [1.5, 0.8, 11.5],
        hour: 15.0,
        warmup: 0.0,
    },
    DemoView {
        key: "magic_footsteps",
        name: "Footsteps in mud, snow, sand and water",
        eye: [-5.5, 1.5, 9.4],
        target: [-5.5, 0.4, 14.0],
        hour: 16.5,
        warmup: 0.0,
    },
    DemoView {
        key: "magic_impacts",
        name: "Impacts on rock, wood, metal, dirt, snow and sand",
        eye: [9.35, 1.5, 9.8],
        target: [9.35, 0.4, 14.6],
        hour: 17.3,
        warmup: 0.0,
    },
];

/// Fires spell projectiles from this entity at `target` in turn.
#[derive(Component, Debug, Clone)]
pub struct DemoCaster {
    pub target: DVec3,
    /// (projectile effect, impact effect) pairs, cast in turn.
    pub spells: Vec<(String, String)>,
    pub period: f32,
    pub speed: f32,
    pub timer: f32,
    pub next: usize,
}

/// A spell in flight: flies straight to `to`, then bursts.
#[derive(Component, Debug, Clone)]
pub struct DemoProjectile {
    pub from: DVec3,
    pub to: DVec3,
    pub speed: f32,
    pub impact: String,
    pub traveled: f64,
}

/// Plays a one-shot effect here every `period` seconds.
#[derive(Component, Debug, Clone)]
pub struct DemoRepeater {
    pub effect: String,
    pub period: f32,
    pub timer: f32,
    /// Up axis of the effect.
    pub up: Vec3,
    pub scale: f32,
}

/// Circles around `center`, bobbing up and down.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoOrbit {
    pub center: DVec3,
    pub radius: f64,
    /// Radians per second.
    pub speed: f64,
    pub height: f64,
    pub bob: f64,
    pub phase: f64,
}

/// A lightning arc that strikes for `on` seconds every `period` seconds.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoLightning {
    pub period: f32,
    pub on: f32,
    pub timer: f32,
}

/// Walks back and forth between `a` and `b`, writing a footstep (`GroundContact`) every
/// `stride` meters on whichever patch it is on.
#[derive(Component, Debug, Clone)]
pub struct DemoWalker {
    pub a: DVec3,
    pub b: DVec3,
    pub speed: f64,
    pub stride: f64,
    /// Surface name per patch, by distance from `a` (meters where the patch ends).
    pub patches: Vec<(f64, String)>,
    pub along: f64,
    pub forward: bool,
    pub since_step: f64,
    pub left: bool,
}

/// Drops a stone into the pond every `period` seconds.
#[derive(Component, Debug, Clone)]
pub struct DemoDropper {
    pub radius: f64,
    pub height: f64,
    pub period: f32,
    pub timer: f32,
    pub seed: u64,
}

/// A falling stone: splashes when it reaches `surface_y`.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoFalling {
    pub velocity: f64,
    pub surface_y: f64,
}

fn mesh(commands: &mut Commands, shape: Shape, material: StandardMaterial, at: Transform) {
    commands.spawn((at, MeshRenderer::new(shape, material)));
}

fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

fn cylinder(radius: f32, height: f32) -> Shape {
    Shape::Cylinder {
        radius,
        height,
        segments: 48,
    }
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

/// Build the Spell court around `origin` (a point on the ground).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let o = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    let stone = StandardMaterial::hex(0x6f6a66).with_roughness(0.8);
    let dark = StandardMaterial::hex(0x34302e).with_roughness(0.85);
    let floor = StandardMaterial::hex(0x56514c).with_roughness(0.8);
    let pale = StandardMaterial::hex(0xb9b2a6).with_roughness(0.7);
    let metal = StandardMaterial::hex(0x7d828a)
        .with_metallic(1.0)
        .with_roughness(0.35);
    let mut water = StandardMaterial::hex(0x1d4a55)
        .with_roughness(0.04)
        .with_alpha(0.88);
    water.reflectance = 0.5;

    // The court: a dark stone disc with a pale inlaid ring.
    mesh(commands, cylinder(21.0, 0.06), floor, at(o(0.0, 0.03, 0.0)));
    mesh(
        commands,
        Shape::Torus {
            radius: 20.4,
            tube: 0.12,
            segments: 96,
        },
        pale,
        at(o(0.0, 0.04, 0.0)),
    );

    // North: the arcane portal between two standing stones, with orbs circling it.
    let portal = o(0.0, 0.0, -11.0);
    commands.spawn((at(portal), ParticleEffect::new("spell.arcane_portal")));
    for x in [-4.6, 4.6] {
        mesh(
            commands,
            cuboid(0.9, 4.2, 0.7),
            stone,
            Transform::from_translation(portal + DVec3::new(x, 2.1, -0.8))
                .with_rotation(Quat::from_rotation_y(x.signum() as f32 * 0.25)),
        );
    }
    for i in 0..3 {
        commands.spawn((
            at(portal + DVec3::new(0.0, 2.5, 0.0)),
            ParticleEffect::new("magic.orb"),
            DemoOrbit {
                center: portal,
                radius: 4.0 + i as f64 * 0.5,
                speed: 0.9 - i as f64 * 0.15,
                height: 2.0 + i as f64 * 0.45,
                bob: 0.35,
                phase: i as f64 * std::f64::consts::TAU / 3.0,
            },
        ));
    }

    // Middle lane: a caster plinth in the west fires at a target wall in the east.
    let lane_z = -3.5;
    let caster = o(-12.5, 1.6, lane_z);
    let wall = o(9.0, 0.0, lane_z);
    art::spawn_prop(
        commands,
        "plinth.caster",
        at(caster - DVec3::new(0.0, 1.6, 0.0)),
        || art::props::plinth(0.6, 0.6),
    );
    mesh(
        commands,
        cuboid(0.8, 2.8, 3.0),
        stone,
        at(wall + DVec3::new(0.4, 1.4, 0.0)),
    );
    commands.spawn((
        at(caster),
        ParticleEffect::new("spell.charge").with_scale(0.6),
        DemoCaster {
            target: wall + DVec3::new(0.0, 1.5, 0.0),
            spells: vec![
                ("spell.fireball".into(), "spell.fireball_impact".into()),
                ("spell.frost_bolt".into(), "spell.frost_impact".into()),
            ],
            period: 1.9,
            speed: 15.0,
            timer: 1.0,
            next: 0,
        },
    ));

    // West: lightning between two obelisks.
    let a = o(-13.0, 0.0, 3.0);
    let b = o(-7.0, 0.0, 9.0);
    for p in [a, b] {
        mesh(
            commands,
            cuboid(0.7, 4.4, 0.7),
            dark,
            at(p + DVec3::new(0.0, 2.2, 0.0)),
        );
        mesh(
            commands,
            Shape::Sphere {
                radius: 0.32,
                segments: 24,
            },
            metal,
            at(p + DVec3::new(0.0, 4.6, 0.0)),
        );
        commands.spawn((
            at(p + DVec3::new(0.0, 4.6, 0.0)),
            ParticleEffect::new("spell.charge").with_scale(0.5),
        ));
    }
    commands.spawn((
        at(a + DVec3::new(0.0, 4.6, 0.0)),
        Beam {
            target: b + DVec3::new(0.0, 4.6, 0.0),
            ..Default::default()
        },
        PointLight {
            color: [0.6, 0.7, 1.0],
            intensity_lm: 0.0,
            range: 14.0,
            radius: 0.2,
        },
        DemoLightning {
            period: 2.0,
            on: 1.3,
            timer: 0.0,
        },
    ));

    // Center: a healing aura on a dais.
    let dais = o(-2.5, 0.0, 4.0);
    mesh(
        commands,
        cylinder(1.8, 0.16),
        pale,
        at(dais + DVec3::new(0.0, 0.08, 0.0)),
    );
    commands.spawn((
        at(dais + DVec3::new(0.0, 0.17, 0.0)),
        ParticleEffect::new("spell.heal_aura"),
    ));

    // East: a fountain and an anvil.
    let fountain = o(7.0, 0.0, 5.5);
    mesh(
        commands,
        cylinder(2.4, 0.45),
        stone,
        at(fountain + DVec3::new(0.0, 0.225, 0.0)),
    );
    mesh(
        commands,
        Shape::Torus {
            radius: 2.3,
            tube: 0.16,
            segments: 64,
        },
        pale,
        at(fountain + DVec3::new(0.0, 0.5, 0.0)),
    );
    mesh(
        commands,
        cylinder(2.2, 0.04),
        water,
        at(fountain + DVec3::new(0.0, 0.5, 0.0)),
    );
    mesh(
        commands,
        cylinder(0.18, 0.6),
        pale,
        at(fountain + DVec3::new(0.0, 0.8, 0.0)),
    );
    mesh(
        commands,
        cylinder(0.45, 0.08),
        pale,
        at(fountain + DVec3::new(0.0, 1.1, 0.0)),
    );
    commands.spawn((
        at(fountain + DVec3::new(0.0, 1.12, 0.0)),
        ParticleEffect::new("water.fountain"),
    ));
    let anvil = o(12.0, 0.0, 0.5);
    mesh(
        commands,
        cuboid(0.5, 0.7, 0.5),
        dark,
        at(anvil + DVec3::new(0.0, 0.35, 0.0)),
    );
    mesh(
        commands,
        cuboid(1.1, 0.3, 0.5),
        metal,
        at(anvil + DVec3::new(0.0, 0.85, 0.0)),
    );
    commands.spawn((
        at(anvil + DVec3::new(0.0, 1.0, 0.0)),
        DemoRepeater {
            effect: "impact.metal".into(),
            period: 1.1,
            timer: 0.3,
            up: Vec3::Y,
            scale: 1.0,
        },
    ));

    // South: a pond that stones fall into.
    let pond = o(1.5, 0.0, 11.5);
    mesh(
        commands,
        cylinder(3.2, 0.02),
        water,
        at(pond + DVec3::new(0.0, 0.07, 0.0)),
    );
    mesh(
        commands,
        Shape::Torus {
            radius: 3.35,
            tube: 0.22,
            segments: 64,
        },
        stone,
        at(pond + DVec3::new(0.0, 0.08, 0.0)),
    );
    commands.spawn((
        at(pond),
        DemoDropper {
            radius: 2.2,
            height: 4.0,
            period: 0.6,
            timer: 0.3,
            seed: 7,
        },
    ));
    // A stone already on its way down, so the pond splashes as soon as the court loads.
    commands.spawn((
        at(pond + DVec3::new(0.5, 1.2, -0.4)),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.16,
                segments: 16,
            },
            StandardMaterial::hex(0x55504a).with_roughness(0.8),
        ),
        DemoFalling {
            velocity: 0.0,
            surface_y: pond.y + 0.08,
        },
    ));

    // South-west: footsteps across mud, snow, sand and shallow water.
    let start = o(-9.5, 0.0, 14.0);
    let patches = [
        ("mud", 0x3d2b1f, 0.6),
        ("snow", 0xf2f4f7, 0.55),
        ("sand", 0xc9b28a, 0.9),
        ("water", 0x2a5562, 0.05),
    ];
    for (i, (_, hex, rough)) in patches.iter().enumerate() {
        let mut m = StandardMaterial::hex(*hex).with_roughness(*rough);
        if i == 3 {
            m = m.with_alpha(0.85);
        }
        mesh(
            commands,
            cuboid(2.0, 0.02, 1.6),
            m,
            at(start + DVec3::new(1.0 + i as f64 * 2.0, 0.07, 0.0)),
        );
    }
    commands.spawn((
        at(start + DVec3::new(0.0, 0.9, 0.0)),
        MeshRenderer::new(
            Shape::Capsule {
                radius: 0.28,
                height: 1.3,
                segments: 16,
            },
            stone,
        ),
        DemoWalker {
            a: start + DVec3::new(0.2, 0.0, 0.0),
            b: start + DVec3::new(7.8, 0.0, 0.0),
            speed: 1.6,
            stride: 0.7,
            patches: patches
                .iter()
                .enumerate()
                .map(|(i, (name, _, _))| (1.8 + i as f64 * 2.0, name.to_string()))
                .collect(),
            along: 0.0,
            forward: true,
            since_step: 0.0,
            left: false,
        },
    ));

    // South-east: shot impacts on six materials, one after another.
    let row = o(5.6, 0.0, 14.6);
    let plates = [
        ("impact.rock", 0x8a8580),
        ("impact.wood", 0x8a6239),
        ("impact.metal", 0xc8ccd4),
        ("impact.dirt", 0x5c4632),
        ("impact.snow", 0xf2f4f7),
        ("impact.sand", 0xc9b28a),
    ];
    for (i, (effect, hex)) in plates.iter().enumerate() {
        let p = row + DVec3::new(i as f64 * 1.5, 0.0, 0.0);
        let mut m = StandardMaterial::hex(*hex).with_roughness(0.8);
        if effect.ends_with("metal") {
            m = m.with_metallic(1.0).with_roughness(0.35);
        }
        mesh(
            commands,
            cuboid(1.2, 0.1, 1.2),
            m,
            at(p + DVec3::new(0.0, 0.05, 0.0)),
        );
        commands.spawn((
            at(p + DVec3::new(0.0, 0.1, 0.0)),
            DemoRepeater {
                effect: effect.to_string(),
                period: 1.5,
                timer: i as f32 * 0.25,
                up: Vec3::Y,
                scale: 1.5,
            },
        ));
    }

    // The rim: torches, and fireflies in the bushes beyond.
    for i in 0..8 {
        let angle = (i as f64 + 0.5) * std::f64::consts::TAU / 8.0;
        let p = o(angle.cos() * 19.6, 0.0, angle.sin() * 19.6);
        mesh(
            commands,
            cylinder(0.07, 1.9),
            metal,
            at(p + DVec3::new(0.0, 0.95, 0.0)),
        );
        mesh(
            commands,
            cylinder(0.16, 0.18),
            dark,
            at(p + DVec3::new(0.0, 1.95, 0.0)),
        );
        commands.spawn((
            at(p + DVec3::new(0.0, 2.05, 0.0)),
            ParticleEffect::new("fire.torch"),
        ));
    }
    let bush = StandardMaterial::hex(0x2f4a24)
        .with_roughness(0.8)
        .with_subsurface(0.4);
    for (x, z) in [(-22.5, -7.5), (22.0, -10.0), (-20.5, 15.5), (22.5, 13.0)] {
        for k in 0..3 {
            let off = DVec3::new(k as f64 * 0.9 - 0.9, 0.0, (k % 2) as f64 * 0.6);
            mesh(
                commands,
                Shape::Sphere {
                    radius: 0.9 - k as f32 * 0.12,
                    segments: 20,
                },
                bush,
                at(o(x, 0.55, z) + off),
            );
        }
        commands.spawn((at(o(x, 1.0, z)), ParticleEffect::new("ambient.fireflies")));
    }
}

/// Casts spells at the target in turn.
pub fn run_casters(
    mut commands: Commands,
    time: Res<Time>,
    mut casters: Query<(&mut DemoCaster, &GlobalTransform)>,
) {
    let dt = time.delta_secs();
    for (mut caster, gt) in &mut casters {
        if caster.spells.is_empty() {
            continue;
        }
        caster.timer -= dt;
        if caster.timer > 0.0 {
            continue;
        }
        caster.timer += caster.period.max(0.2);
        let (projectile, impact) = caster.spells[caster.next % caster.spells.len()].clone();
        caster.next += 1;
        let from = gt.translation();
        commands.spawn((
            at(from),
            ParticleEffect::new(projectile),
            DemoProjectile {
                from,
                to: caster.target,
                speed: caster.speed,
                impact,
                traveled: 0.0,
            },
        ));
    }
}

/// Moves spells in flight; on arrival they burst, push the air and disappear.
pub fn run_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    mut projectiles: Query<(
        Entity,
        &mut DemoProjectile,
        &mut Transform,
        Option<&LocalTimeScale>,
    )>,
    mut impulses: MessageWriter<WorldImpulse>,
) {
    for (entity, mut p, mut t, local) in &mut projectiles {
        let dt = time.delta_for(local) as f64;
        let length = p.from.distance(p.to);
        p.traveled += p.speed as f64 * dt;
        let k = (p.traveled / length.max(1e-6)).min(1.0);
        // A slight arc, highest mid-flight.
        let lift = (k * std::f64::consts::PI).sin() * length * 0.04;
        t.translation = p.from.lerp(p.to, k) + DVec3::new(0.0, lift, 0.0);
        if k >= 1.0 {
            let back = (p.from - p.to).normalize_or_zero().as_vec3();
            spawn_effect_at(&mut commands, &p.impact, p.to, back, 1.0);
            impulses.write(WorldImpulse {
                center: p.to,
                radius: 3.0,
                strength: 6.0,
                energy_j: 0.0,
                heat_c: 0.0,
                source: Some(entity),
            });
            commands.entity(entity).despawn();
        }
    }
}

/// Plays repeating one-shot effects.
pub fn run_repeaters(
    mut commands: Commands,
    time: Res<Time>,
    mut repeaters: Query<(&mut DemoRepeater, &GlobalTransform)>,
) {
    let dt = time.delta_secs();
    for (mut r, gt) in &mut repeaters {
        r.timer -= dt;
        if r.timer > 0.0 {
            continue;
        }
        r.timer += r.period.max(0.1);
        spawn_effect_at(&mut commands, &r.effect, gt.translation(), r.up, r.scale);
    }
}

/// Moves orbiting things.
pub fn run_orbits(time: Res<Time>, mut orbits: Query<(&DemoOrbit, &mut Transform)>) {
    let t = time.elapsed_secs_f64();
    for (o, mut tr) in &mut orbits {
        let a = o.phase + t * o.speed;
        tr.translation = o.center
            + DVec3::new(
                a.cos() * o.radius,
                o.height + (t * 1.7 + o.phase).sin() * o.bob,
                a.sin() * o.radius,
            );
    }
}

/// Switches lightning arcs on and off; each strike sparks at both ends.
pub fn run_lightning(
    mut commands: Commands,
    time: Res<Time>,
    mut arcs: Query<(
        &mut DemoLightning,
        &mut Beam,
        &mut PointLight,
        &GlobalTransform,
    )>,
) {
    let dt = time.delta_secs();
    for (mut l, mut beam, mut light, gt) in &mut arcs {
        let was = l.timer < l.on;
        l.timer = (l.timer + dt) % l.period.max(0.1);
        let on = l.timer < l.on;
        beam.visible = on;
        let flicker = 0.6 + 0.4 * ((time.elapsed_secs() * 37.0).sin() * 0.5 + 0.5);
        light.intensity_lm = if on { 30_000.0 * flicker } else { 0.0 };
        if on && !was {
            let a = gt.translation();
            spawn_effect_at(&mut commands, "spell.lightning_hit", a, Vec3::Y, 0.6);
            spawn_effect_at(
                &mut commands,
                "spell.lightning_hit",
                beam.target,
                Vec3::Y,
                0.6,
            );
        }
    }
}

/// Walks the footstep tester and writes its footsteps.
pub fn run_walkers(
    time: Res<Time>,
    materials: Option<Res<SurfaceMaterials>>,
    mut walkers: Query<(Entity, &mut DemoWalker, &mut Transform)>,
    mut contacts: MessageWriter<GroundContact>,
) {
    let dt = time.delta_secs() as f64;
    for (entity, mut w, mut t) in &mut walkers {
        let length = w.a.distance(w.b);
        let step = w.speed * dt;
        if w.forward {
            w.along += step;
            if w.along >= length {
                w.along = length;
                w.forward = false;
            }
        } else {
            w.along -= step;
            if w.along <= 0.0 {
                w.along = 0.0;
                w.forward = true;
            }
        }
        let dir = (w.b - w.a).normalize_or_zero();
        let pos = w.a + dir * w.along;
        t.translation = pos + DVec3::new(0.0, 0.9, 0.0);
        w.since_step += step;
        if w.since_step < w.stride {
            continue;
        }
        w.since_step -= w.stride;
        w.left = !w.left;
        let heading = if w.forward { dir } else { -dir };
        let side = DVec3::new(-heading.z, 0.0, heading.x) * if w.left { 0.15 } else { -0.15 };
        let surface = w
            .patches
            .iter()
            .find(|(end, _)| w.along <= *end)
            .or(w.patches.last())
            .and_then(|(_, name)| materials.as_ref().and_then(|m| m.id(name)));
        contacts.write(GroundContact {
            position: pos + side + DVec3::new(0.0, 0.08, 0.0),
            normal: Vec3::Y,
            // A heavy stone golem, so its footprints kick up enough to see from afar.
            size: [0.18, 0.4],
            yaw: heading.x.atan2(heading.z) as f32,
            load_kg: 250.0,
            speed: w.speed as f32,
            kind: ContactKind::Foot,
            surface,
            entity: Some(entity),
        });
    }
}

/// Drops stones into ponds and splashes them.
pub fn run_droppers(
    mut commands: Commands,
    time: Res<Time>,
    mut droppers: Query<(&mut DemoDropper, &GlobalTransform)>,
    mut stones: Query<(Entity, &mut DemoFalling, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (mut d, gt) in &mut droppers {
        d.timer -= dt;
        if d.timer > 0.0 {
            continue;
        }
        d.timer += d.period.max(0.2);
        d.seed = d.seed.wrapping_add(1);
        let mut rng = Rng::new(d.seed);
        let angle = rng.range(0.0, std::f32::consts::TAU) as f64;
        let r = d.radius * (rng.f32() as f64).sqrt();
        let surface = gt.translation();
        let p = surface + DVec3::new(angle.cos() * r, d.height, angle.sin() * r);
        commands.spawn((
            at(p),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.16,
                    segments: 16,
                },
                StandardMaterial::hex(0x55504a).with_roughness(0.8),
            ),
            DemoFalling {
                velocity: 0.0,
                surface_y: surface.y + 0.08,
            },
        ));
    }
    let dt = dt as f64;
    for (entity, mut f, mut t) in &mut stones {
        f.velocity -= 9.81 * dt;
        t.translation.y += f.velocity * dt;
        if t.translation.y <= f.surface_y {
            let p = DVec3::new(t.translation.x, f.surface_y, t.translation.z);
            spawn_effect_at(&mut commands, "water.splash", p, Vec3::Y, 0.8);
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::library::EffectLibrary;

    #[test]
    fn the_court_plays_every_test_case() {
        use crate::emitter::ParticleEffect;
        use engine_core::math::DVec3;
        use engine_core::prelude::*;
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, crate::VfxPlugin));
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.05 });
        let origin = DVec3::new(-130.0, 0.0, 40.0);
        app.add_systems(Startup, move |mut commands: Commands| {
            super::spawn_demo(&mut commands, origin)
        });
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..120 {
            app.update();
            let mut q = app.world.query::<&ParticleEffect>();
            seen.extend(q.iter(&app.world).map(|e| e.effect.clone()));
        }
        for name in [
            "spell.fireball",
            "spell.fireball_impact",
            "spell.frost_bolt",
            "spell.frost_impact",
            "spell.lightning_hit",
            "water.splash",
            "step.mud",
            "step.snow",
            "step.sand",
            "step.water",
            "impulse.dust",
            "impact.rock",
            "impact.metal",
        ] {
            assert!(seen.contains(name), "{name} never played: {seen:?}");
        }
    }

    #[test]
    fn every_effect_the_demo_uses_exists() {
        let lib = EffectLibrary::with_builtins();
        let src = include_str!("mod.rs");
        let mut checked = 0;
        for part in src.split('"').skip(1).step_by(2) {
            let is_effect = ["spell.", "magic.", "water.", "impact.", "fire.", "ambient."]
                .iter()
                .any(|p| part.starts_with(p))
                && !part.contains(' ')
                && !part.ends_with('.');
            if is_effect {
                assert!(lib.contains(part), "the demo uses a missing effect: {part}");
                checked += 1;
            }
        }
        assert!(checked >= 12, "{checked}");
    }
}

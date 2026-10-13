//! The Spell court: the showcase's particle and magic test area.
//!
//! [`spawn_demo`] builds it around a point. Everything moves on its own (the systems below
//! are added by [`crate::VfxPlugin`]) and follows world time, so slow motion slows the
//! spells too. Laid out on a paved disc 42 m across, with +X east and +Z toward the default
//! viewpoint. The props (stone, iron, wood, water and plants) are procedural, see [`art`]:
//!
//! | Where | Test case |
//! |---|---|
//! | north | Arcane portal: rune circles, spiraling motes, orbiting orbs with trails |
//! | middle lane | Fireballs and frost bolts flying into a target wall, with impacts and shockwaves |
//! | west | Lightning arcing between two obelisks, striking with sparks |
//! | north-west | A mortar firing starburst fireworks: stars that trail glitter and crackle |
//! | center | Healing aura on a dais |
//! | east | Fountain (water jets landing in a basin) and an anvil throwing sparks |
//! | south | Pond with stones splashing in, footsteps on mud, snow, sand and water, impacts on six materials |
//! | rim | Torches and fireflies |

pub mod art;

use engine_assets::Assets;
use engine_core::math::{DVec3, Quat, Rng, Vec3};
use engine_core::prelude::*;
use engine_render::{
    Image, MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, StandardMaterial,
};
use engine_world::{ContactKind, GroundContact, SurfaceMaterials, WorldImpulse};

use self::art::props;

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
        key: "magic_portal",
        name: "A standing portal swirling the valley behind it",
        eye: [3.2, 2.4, -7.5],
        target: [0.0, 4.2, -17.4],
        hour: 18.4,
        warmup: 2.0,
    },
    DemoView {
        key: "magic_starburst",
        name: "Starburst fireworks over the court",
        eye: [6.0, 1.7, 12.0],
        target: [-11.5, 21.0, -11.5],
        hour: 18.9,
        warmup: 3.0,
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
        key: "magic_marks",
        name: "Scorch and frost marks on the target wall",
        eye: [5.4, 1.9, -0.9],
        target: [9.0, 1.5, -3.4],
        hour: 18.1,
        warmup: 7.4,
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
    /// Spells land this far to either side of `target` in turn, so their marks sit side by side.
    pub spread: DVec3,
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
    /// Each shot lands somewhere within this many meters of here, across `up`.
    pub spread: f32,
    pub shots: u32,
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

/// Which part of the golem a limb pivot moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimbKind {
    Arm,
    Leg,
}

/// A golem limb pivot (child of a [`DemoWalker`]): legs step in time with the walker's
/// footsteps, arms swing against them.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoLimb {
    pub walker: Entity,
    /// +1 for the limb on the walker's local +X side, -1 for -X.
    pub side: f32,
    pub kind: LimbKind,
    /// Rest position of the pivot in the walker's space, meters.
    pub pivot: Vec3,
}

fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

/// Spawn prop `key` as children of `parent` (so it moves with it). Like [`art::spawn_prop`],
/// it does nothing without a renderer.
fn attach_prop(commands: &mut Commands, parent: Entity, key: &str, build: fn() -> art::Prop) {
    let key = key.to_string();
    commands.queue(move |world: &mut World| {
        if !world.contains_resource::<Assets<Mesh>>()
            || !world.contains_resource::<Assets<StandardMaterial>>()
            || !world.contains_resource::<Assets<Image>>()
            || world.get_entity(parent).is_err()
        {
            return;
        }
        for (mesh, material) in art::register(world, &key, build) {
            world.spawn((
                Transform::default(),
                MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
                ChildOf(parent),
            ));
        }
    });
}

/// One of three rough stones for dropping into the pond.
fn stone_variant(seed: u64) -> (&'static str, fn() -> art::Prop) {
    match seed % 3 {
        0 => ("court.stone.0", || props::golem::dropped_stone(0)),
        1 => ("court.stone.1", || props::golem::dropped_stone(1)),
        _ => ("court.stone.2", || props::golem::dropped_stone(2)),
    }
}

/// Where everything stands, in court space (meters from the center, on the ground).
mod layout {
    /// Paved radius of the court.
    pub const RADIUS: f64 = 21.0;
    /// The portal's rune circle and orbiting orbs.
    pub const PORTAL: (f64, f64) = (0.0, -11.0);
    /// The gate behind it, clear of the orbs (they circle 4–5 m out).
    pub const GATE: (f64, f64) = (0.0, -17.4);
    pub const LANE_Z: f64 = -3.5;
    pub const CASTER_X: f64 = -12.5;
    pub const WALL_X: f64 = 9.0;
    pub const OBELISKS: [(f64, f64); 2] = [(-13.0, 3.0), (-7.0, 9.0)];
    pub const DAIS: (f64, f64) = (-2.5, 4.0);
    pub const FOUNTAIN: (f64, f64) = (7.0, 5.5);
    pub const ANVIL: (f64, f64) = (12.0, 0.5);
    /// The firework mortar, between two braziers.
    pub const LAUNCHER: (f64, f64) = (-11.5, -11.5);
    pub const POND: (f64, f64) = (1.5, 11.5);
    /// Start of the footstep strip (it runs 8 m toward +X).
    pub const STRIP: (f64, f64) = (-9.5, 14.0);
    /// Center of the first impact plate (the others follow 1.5 m apart toward +X).
    pub const PLATES: (f64, f64) = (5.6, 14.6);
    pub const TORCH_RADIUS: f64 = 19.6;
    pub const SHRUBS: [(f64, f64); 4] = [(-22.5, -7.5), (22.0, -10.0), (-20.5, 15.5), (22.5, 13.0)];
}

/// The parts of the court floor left unpaved because a prop stands there.
pub fn clearings() -> Vec<props::court::Clearing> {
    use layout::*;
    use props::court::Clearing;
    let v = |(x, z): (f64, f64)| engine_core::math::Vec2::new(x as f32, z as f32);
    let rect = |c: (f64, f64), hx: f64, hz: f64| Clearing::Rect {
        min: v((c.0 - hx, c.1 - hz)),
        max: v((c.0 + hx, c.1 + hz)),
    };
    let circle = |c: (f64, f64), r: f32| Clearing::Circle {
        center: v(c),
        radius: r,
    };
    let mut out = vec![
        circle(PORTAL, 3.62),
        rect(GATE, 5.0, 1.8),
        rect((CASTER_X, LANE_Z), 0.55, 0.55),
        rect((WALL_X + 0.4, LANE_Z), 0.45, 2.4),
        circle(DAIS, 2.2),
        circle(LAUNCHER, 0.85),
        circle(FOUNTAIN, 2.6),
        circle(POND, 3.75),
        rect((STRIP.0 + 4.0, STRIP.1), 4.15, 0.95),
        rect((PLATES.0 + 3.75, PLATES.1), 4.46, 0.71),
    ];
    out.extend(OBELISKS.iter().map(|o| rect(*o, 0.8, 0.8)));
    out
}

/// Build the Spell court around `origin` (a point on the ground).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    use layout::*;
    let o = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    let ground = |(x, z): (f64, f64)| o(x, 0.0, z);
    let yaw = |a: f32| Quat::from_rotation_y(a);

    // The court: flagstone courses with a rune ring, a curb and a medallion.
    art::spawn_prop(commands, "court.floor", at(origin), || {
        props::court::floor(RADIUS as f32, &clearings())
    });

    // North: the arcane portal on a summoning floor before the gate, with orbs circling it.
    let portal = ground(PORTAL);
    art::spawn_prop(
        commands,
        "court.summoning",
        at(portal),
        props::portal::summoning_circle,
    );
    art::spawn_prop(
        commands,
        "court.gate",
        at(ground(GATE)),
        props::portal::gate,
    );
    commands.spawn((at(portal), ParticleEffect::new("spell.arcane_portal")));
    // A standing portal in the gate's arch, facing the court (+Z).
    let arch = ground(GATE) + DVec3::new(0.0, props::portal::SPRING as f64, 0.0);
    commands.spawn((
        at(arch).with_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
        ParticleEffect::new("spell.portal_gate"),
    ));
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

    // Middle lane: a caster's pedestal in the west fires at a target wall in the east.
    let caster = o(CASTER_X, 1.6, LANE_Z);
    let wall = o(WALL_X, 0.0, LANE_Z);
    let pedestal = o(CASTER_X, 0.0, LANE_Z);
    art::spawn_prop(
        commands,
        "court.pedestal",
        at(pedestal),
        props::lane::caster_pedestal,
    );
    // The focus crystal bobs gently over the pedestal, around the launch point.
    let focus = commands
        .spawn((
            at(caster),
            DemoOrbit {
                center: pedestal,
                radius: 0.0,
                speed: 0.0,
                height: 1.6,
                bob: 0.04,
                phase: 0.0,
            },
        ))
        .id();
    attach_prop(commands, focus, "court.focus", props::lane::focus_crystal);
    art::spawn_prop(commands, "court.wall", at(wall), props::lane::target_wall);
    commands.spawn((
        at(caster),
        ParticleEffect::new("spell.charge").with_scale(0.6),
        DemoCaster {
            target: wall + DVec3::new(0.0, 1.5, 0.0),
            spells: vec![
                ("spell.fireball".into(), "spell.fireball_impact".into()),
                ("spell.frost_bolt".into(), "spell.frost_impact".into()),
            ],
            // Scorches to the left, frost to the right.
            spread: DVec3::new(0.0, 0.0, 0.75),
            period: 1.9,
            speed: 15.0,
            timer: 1.0,
            next: 0,
        },
    ));

    // West: lightning between two obelisks, arcing between their crystals.
    let crystal = props::storm::CRYSTAL_Y as f64;
    let a = ground(OBELISKS[0]);
    let b = ground(OBELISKS[1]);
    art::spawn_prop(
        commands,
        "court.obelisk.0",
        at(a).with_rotation(yaw(0.785)),
        || props::storm::obelisk(0),
    );
    art::spawn_prop(
        commands,
        "court.obelisk.1",
        at(b).with_rotation(yaw(0.785)),
        || props::storm::obelisk(1),
    );
    for p in [a, b] {
        commands.spawn((
            at(p + DVec3::new(0.0, crystal, 0.0)),
            ParticleEffect::new("spell.charge").with_scale(0.5),
        ));
    }
    commands.spawn((
        at(a + DVec3::new(0.0, crystal, 0.0)),
        Beam {
            target: b + DVec3::new(0.0, crystal, 0.0),
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

    // Center: a healing aura on a stepped dais.
    let dais = ground(DAIS);
    art::spawn_prop(commands, "court.dais", at(dais), props::water::dais);
    commands.spawn((
        // Just above the inlaid flower in the middle of the dais.
        at(dais + DVec3::new(0.0, props::water::DAIS_TOP as f64 + 0.012, 0.0)),
        ParticleEffect::new("spell.heal_aura"),
    ));

    // East: a fountain and an anvil.
    let fountain = ground(FOUNTAIN);
    art::spawn_prop(
        commands,
        "court.fountain",
        at(fountain),
        props::water::fountain,
    );
    commands.spawn((
        at(fountain + DVec3::new(0.0, props::water::NOZZLE as f64, 0.0)),
        ParticleEffect::new("water.fountain"),
    ));
    let anvil = ground(ANVIL);
    art::spawn_prop(
        commands,
        "court.anvil",
        at(anvil).with_rotation(yaw(0.5)),
        props::forge::anvil,
    );
    commands.spawn((
        at(anvil + DVec3::new(0.0, props::forge::FACE as f64, 0.0)),
        DemoRepeater {
            effect: "impact.metal".into(),
            period: 1.1,
            timer: 0.3,
            up: Vec3::Y,
            scale: 1.0,
            spread: 0.0,
            shots: 0,
        },
    ));

    // North-west: a mortar firing starburst fireworks over the court.
    let launcher = ground(LAUNCHER);
    art::spawn_prop(
        commands,
        "court.mortar",
        at(launcher),
        props::launcher::mortar,
    );
    commands.spawn((
        at(launcher + DVec3::new(0.0, props::launcher::MUZZLE as f64, 0.0)),
        DemoRepeater {
            effect: "spell.starburst".into(),
            period: 3.2,
            timer: 1.0,
            up: Vec3::Y,
            scale: 1.0,
            spread: 0.0,
            shots: 0,
        },
    ));

    // South: a pond that stones fall into.
    let pond = ground(POND);
    let water = props::water::POND_WATER as f64;
    art::spawn_prop(commands, "court.pond", at(pond), props::water::pond);
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
    let first = commands
        .spawn((
            at(pond + DVec3::new(0.5, 1.2, -0.4)),
            DemoFalling {
                velocity: 0.0,
                surface_y: pond.y + water + 0.01,
            },
        ))
        .id();
    let (key, build) = stone_variant(0);
    attach_prop(commands, first, key, build);

    // South-west: footsteps across mud, snow, sand and shallow water, walked by a stone golem.
    let start = ground(STRIP);
    art::spawn_prop(
        commands,
        "court.patches",
        at(start),
        props::beds::footstep_patches,
    );
    let patches = ["mud", "snow", "sand", "water"];
    let walker = commands
        .spawn((
            at(start + DVec3::new(0.0, 0.9, 0.0)),
            DemoWalker {
                a: start + DVec3::new(0.2, 0.0, 0.0),
                b: start + DVec3::new(7.8, 0.0, 0.0),
                speed: 1.6,
                stride: 0.7,
                patches: patches
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (1.8 + i as f64 * 2.0, name.to_string()))
                    .collect(),
                along: 0.0,
                forward: true,
                since_step: 0.0,
                left: false,
            },
        ))
        .id();
    attach_prop(
        commands,
        walker,
        "court.golem.body",
        props::golem::golem_body,
    );
    for side in [-1.0f32, 1.0] {
        for (kind, pivot, key, build) in [
            (
                LimbKind::Arm,
                props::golem::SHOULDER,
                "court.golem.arm",
                props::golem::golem_arm as fn() -> art::Prop,
            ),
            (
                LimbKind::Leg,
                props::golem::HIP,
                "court.golem.leg",
                props::golem::golem_leg,
            ),
        ] {
            let pivot = pivot * Vec3::new(side, 1.0, 1.0);
            let limb = commands
                .spawn((
                    at(pivot.as_dvec3()),
                    ChildOf(walker),
                    DemoLimb {
                        walker,
                        side,
                        kind,
                        pivot,
                    },
                ))
                .id();
            attach_prop(commands, limb, key, build);
        }
    }

    // South-east: shot impacts on six materials, one after another.
    let row = ground(PLATES);
    art::spawn_prop(
        commands,
        "court.plates",
        at(row),
        props::beds::impact_plates,
    );
    let plates = [
        "impact.rock",
        "impact.wood",
        "impact.metal",
        "impact.dirt",
        "impact.snow",
        "impact.sand",
    ];
    for (i, effect) in plates.iter().enumerate() {
        let p = row + DVec3::new(i as f64 * 1.5, 0.0, 0.0);
        commands.spawn((
            at(p + DVec3::new(0.0, 0.1, 0.0)),
            DemoRepeater {
                effect: effect.to_string(),
                period: 1.5,
                timer: i as f32 * 0.25,
                up: Vec3::Y,
                scale: 1.5,
                // Shots scatter over the plate, so rock gathers a cluster of chips.
                spread: 0.35,
                shots: 0,
            },
        ));
    }

    // The rim: braziers, and fireflies in the shrubs beyond.
    for i in 0..8 {
        let angle = (i as f64 + 0.5) * std::f64::consts::TAU / 8.0;
        let p = o(angle.cos() * TORCH_RADIUS, 0.0, angle.sin() * TORCH_RADIUS);
        art::spawn_prop(
            commands,
            "court.brazier",
            at(p).with_rotation(yaw(-angle as f32)),
            props::rim::brazier,
        );
        commands.spawn((
            at(p + DVec3::new(0.0, props::rim::COALS as f64, 0.0)),
            ParticleEffect::new("fire.torch"),
        ));
    }
    for (i, (x, z)) in SHRUBS.iter().enumerate() {
        let spot = o(*x, 0.0, *z);
        let turn = yaw((*z).atan2(*x) as f32 + std::f32::consts::FRAC_PI_2);
        if i % 2 == 0 {
            art::spawn_prop(
                commands,
                "court.shrubs.0",
                at(spot).with_rotation(turn),
                || props::rim::shrubs(0),
            );
        } else {
            art::spawn_prop(
                commands,
                "court.shrubs.1",
                at(spot).with_rotation(turn),
                || props::rim::shrubs(1),
            );
        }
        commands.spawn((at(o(*x, 1.0, *z)), ParticleEffect::new("ambient.fireflies")));
    }
}

/// The demo's own world positions, for following a [`engine_world::WorldShift`].
#[derive(SystemParam)]
pub struct ShiftQueries<'w, 's> {
    casters: Query<'w, 's, &'static mut DemoCaster>,
    projectiles: Query<'w, 's, &'static mut DemoProjectile>,
    orbits: Query<'w, 's, &'static mut DemoOrbit>,
    walkers: Query<'w, 's, &'static mut DemoWalker>,
    falling: Query<'w, 's, &'static mut DemoFalling>,
}

/// Move the demo's waypoints into a new reference frame (the demo itself assumes +Y is up).
pub fn shift(q: &mut ShiftQueries, s: &engine_world::WorldShift) {
    for mut c in &mut q.casters {
        c.target = s.point(c.target);
    }
    for mut p in &mut q.projectiles {
        p.from = s.point(p.from);
        p.to = s.point(p.to);
    }
    for mut o in &mut q.orbits {
        o.center = s.point(o.center);
    }
    for mut w in &mut q.walkers {
        w.a = s.point(w.a);
        w.b = s.point(w.b);
    }
    for mut f in &mut q.falling {
        f.surface_y += s.translation.y;
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
        let side = if caster.next.is_multiple_of(2) {
            -1.0
        } else {
            1.0
        };
        let to = caster.target + caster.spread * side;
        caster.next += 1;
        let from = gt.translation();
        commands.spawn((
            at(from),
            ParticleEffect::new(projectile),
            DemoProjectile {
                from,
                to,
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
        // Scatter shots evenly over a disc across `up` (a golden-angle spiral).
        let k = r.shots as f32;
        r.shots = r.shots.wrapping_add(1);
        let (a, b) = r.up.normalize_or(Vec3::Y).any_orthonormal_pair();
        let radius = r.spread * (k * 0.618_034).fract().sqrt();
        let (sin, cos) = (k * 2.399_963).sin_cos();
        let at = gt.translation() + ((a * cos + b * sin) * radius).as_dvec3();
        spawn_effect_at(&mut commands, &r.effect, at, r.up, r.scale);
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
        light.intensity_lm = if on { 5_000.0 * flicker } else { 0.0 };
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

/// Walks the footstep tester and writes its footsteps; poses the golem's limbs in step.
///
/// Each foot lands half a stride ahead of the body and is pushed back under it until the
/// other foot lands, so a footstep is written where (and when) a foot comes down. The body
/// dips while the legs are spread and rises as they pass.
pub fn run_walkers(
    time: Res<Time>,
    materials: Option<Res<SurfaceMaterials>>,
    mut walkers: Query<(Entity, &mut DemoWalker, &mut Transform)>,
    mut limbs: Query<(&DemoLimb, &mut Transform), Without<DemoWalker>>,
    mut contacts: MessageWriter<GroundContact>,
) {
    let dt = time.delta_secs() as f64;
    let leg = props::golem::LEG as f64;
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
        let heading = if w.forward { dir } else { -dir };
        w.since_step += step;
        if w.since_step >= w.stride {
            w.since_step -= w.stride;
            w.left = !w.left;
            let side = DVec3::new(-heading.z, 0.0, heading.x) * if w.left { 0.15 } else { -0.15 };
            // The foot comes down half a stride ahead, on whatever patch is there.
            let lead = if w.forward { 0.5 } else { -0.5 } * w.stride;
            let foot = (w.along + lead).clamp(-0.1, length + 0.1);
            let surface = w
                .patches
                .iter()
                .find(|(end, _)| foot <= *end)
                .or(w.patches.last())
                .and_then(|(_, name)| materials.as_ref().and_then(|m| m.id(name)));
            contacts.write(GroundContact {
                position: w.a + dir * foot + side + DVec3::new(0.0, 0.08, 0.0),
                normal: Vec3::Y,
                // A heavy stone golem, so its footprints kick up enough to see from afar.
                size: [0.18, 0.4],
                yaw: GroundContact::yaw_of(heading.as_vec3()),
                load_kg: 250.0,
                speed: w.speed as f32,
                kind: ContactKind::Foot,
                surface,
                entity: Some(entity),
            });
        }
        // Body: over the planted foot, dipping as the legs spread; turning to face its way.
        let p = (w.since_step / w.stride.max(1e-3)).clamp(0.0, 1.0);
        let spread = ((0.5 - p) * w.stride / leg).clamp(-0.95, 0.95).asin();
        t.translation = pos + DVec3::new(0.0, 0.065 + leg * spread.cos() + 0.2, 0.0);
        let facing = Quat::from_rotation_y((-heading.x).atan2(-heading.z) as f32);
        let turn = 1.0 - (-8.0 * dt as f32).exp();
        t.rotation = t.rotation.slerp(facing, turn);
    }
    for (limb, mut lt) in &mut limbs {
        let Ok((_, w, _)) = walkers.get(limb.walker) else {
            continue;
        };
        let p = (w.since_step / w.stride.max(1e-3)).clamp(0.0, 1.0);
        // The planted foot is the one that last came down (`left` means local +X).
        let planted = w.left == (limb.side > 0.0);
        let reach = if planted { 0.5 - p } else { p - 0.5 } * w.stride;
        let swing = (reach / leg).clamp(-0.95, 0.95).asin() as f32;
        match limb.kind {
            LimbKind::Leg => {
                let lift = if planted {
                    0.0
                } else {
                    (p * std::f64::consts::PI).sin() as f32 * 0.09
                };
                lt.translation = (limb.pivot + Vec3::Y * lift).as_dvec3();
                lt.rotation = Quat::from_rotation_x(swing);
            }
            LimbKind::Arm => {
                lt.translation = limb.pivot.as_dvec3();
                lt.rotation =
                    Quat::from_rotation_x(-swing * 0.7) * Quat::from_rotation_z(limb.side * 0.1);
            }
        }
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
        let turned = Quat::from_euler(
            engine_core::math::EulerRot::YXZ,
            rng.range(0.0, 6.3),
            rng.range(0.0, 6.3),
            0.0,
        );
        let stone = commands
            .spawn((
                at(p).with_rotation(turned),
                DemoFalling {
                    velocity: 0.0,
                    surface_y: surface.y + 0.08,
                },
            ))
            .id();
        let (key, build) = stone_variant(d.seed);
        attach_prop(&mut commands, stone, key, build);
    }
    let dt = dt as f64;
    for (entity, mut f, mut t) in &mut stones {
        f.velocity -= 9.81 * dt;
        t.translation.y += f.velocity * dt;
        // Tumble as it falls.
        let axis = Vec3::new(1.0, 0.4, (entity.to_bits() % 5) as f32 * 0.3 - 0.6).normalize();
        t.rotation = Quat::from_axis_angle(axis, 3.0 * dt as f32) * t.rotation;
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

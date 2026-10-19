//! Yuzen nagashi: lengths of freshly dyed kimono silk rinsed in a stream, as Kyoto's dyers
//! rinsed them in the Kamo river. Each is tied by one end to a bamboo pole laid across the
//! stream on crossed stakes and streams out with the current, soaked and dark, rippling in
//! its eddies.
//!
//! The frame waits for the water: once the stream under it is known ([`tie_silks`]), it
//! settles so the pole rides a hand above the surface and ties the silks on.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::smoothstep;
use engine_render::Mesh;
use engine_world::{WaterQuery, WaterSample};

use super::kit::{Mat, ModelBuilder};
use super::meshkit::*;
use super::props::rnd;
use super::*;

/// Silk strips, m: a kimono bolt's width, and long enough to stream well out.
pub const STRIP_WIDTH: f32 = 0.38;
pub const STRIP_LENGTH: f32 = 7.0;
/// The pole's axis above the water, m.
const POLE_ABOVE: f32 = 0.24;
const POLE_RADIUS: f32 = 0.034;
const STAKE_RADIUS: f32 = 0.024;
/// The stakes lean this far from upright, either way along the stream (radians).
const STAKE_LEAN: f32 = 0.38;
/// Strips across the stream, m apart.
const STRIP_PITCH: f32 = 0.7;

/// A rinsing frame: tied once the water under it is known.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct RinseFrame {
    pub tied: bool,
}

/// Lay a rinsing frame across a stream at `at` (on the stream's middle line; the height is
/// found from the water), with the silks streaming toward `downstream`. `span` is the pole's
/// length: a little wider than the stream.
pub fn spawn_rinse(commands: &mut Commands, at: DVec3, downstream: Vec3, span: f32) -> Entity {
    commands.queue(super::kit::ensure_materials);
    let down = downstream.with_y(0.0).normalize_or(Vec3::Z);
    let frame = commands
        .spawn((
            Name::new("Silk rinse (yuzen nagashi)"),
            Transform::from_translation(at).with_rotation(Quat::from_rotation_arc(Vec3::Z, down)),
            RinseFrame { tied: false },
        ))
        .id();
    frame_model(commands, span).attach(commands, frame, Transform::default());
    frame
}

/// Settle each untied frame on the water under it and tie its silks on.
pub fn tie_silks(
    mut commands: Commands,
    mut frames: Query<(Entity, &mut Transform, &mut RinseFrame)>,
    water: Res<engine_world::Water>,
) {
    for (entity, mut t, mut frame) in &mut frames {
        if frame.tied {
            continue;
        }
        let Some(w) = water.sample(t.translation) else {
            continue;
        };
        t.translation.y = w.surface_height + f64::from(POLE_ABOVE);
        frame.tied = true;
        for (k, silk) in silks().into_iter().enumerate() {
            let x = (k as f32 - 2.0) * STRIP_PITCH;
            commands.spawn((
                Name::new(format!("Rinsing silk {k}")),
                // Tied under the pole and laid out downstream, sloping into the water.
                Transform::from_xyz(f64::from(x), f64::from(-POLE_RADIUS - 0.004), 0.0)
                    .with_rotation(Quat::from_rotation_x(-FRAC_PI_2 + 0.12)),
                strip(silk),
                ChildOf(entity),
            ));
        }
    }
}

/// One strip of silk crepe: light, soft, close to floating once wet.
fn strip(material: SoftMaterial) -> Cloth {
    Cloth {
        size: [STRIP_WIDTH, STRIP_LENGTH],
        resolution: [5, 61],
        anchor: [0.5, 0.0],
        pins: ClothPins::TopEdge { every: 1 },
        mass_per_m2: 0.07,
        shear_compliance: 2e-2,
        bend_compliance: 5e-2,
        pierceable: false,
        flammable: false,
        material,
        sim: SimSettings {
            thickness: 0.006,
            friction: 0.3,
            damping: 0.06,
            wind_scale: 0.4,
            // Wet silk is about as heavy as the water it displaces: it hangs in the stream,
            // which holds it out and rolls it about.
            buoyancy: 1.1,
            soak: 0.1,
            water_drag: 6.0,
            prewarm_secs: 4.0,
            ..SimSettings::default()
        },
        ..Cloth::default()
    }
}

/// Five silks in classic yuzen patterns.
fn silks() -> [SoftMaterial; 5] {
    let silk = |m: SoftMaterial| m.with_roughness(0.48).with_sheen(0.7).with_wrinkles(0.5);
    [
        // Seigaiha waves in white on indigo.
        silk(SoftMaterial::hex(0x1c2c5a).with_pattern(Pattern::Waves(240), 0xeae4d4, 0xeae4d4)),
        // White plum blossoms with gold hearts on vermilion.
        silk(SoftMaterial::hex(0xbf3426).with_pattern(Pattern::Blossoms(120), 0xf3ecdf, 0xe0ac36)),
        // Kanoko tie-dye dots on murasaki purple.
        silk(SoftMaterial::hex(0x4a2a5c).with_pattern(Pattern::Shibori(40), 0xeee4ee, 0xeee4ee)),
        // Fine white waves on celadon.
        silk(SoftMaterial::hex(0x6fa592).with_pattern(Pattern::Waves(190), 0xf1efe4, 0xf1efe4)),
        // Crimson blossoms on cherry-pink.
        silk(SoftMaterial::hex(0xe9bcbc).with_pattern(Pattern::Blossoms(90), 0xa8233a, 0xf0cf55)),
    ]
}

/// The bamboo pole on two crossed-stake trestles near the banks, with rope lashings and the
/// cords that tie each silk on. Local frame: the pole's axis along X at the origin, the stream
/// flowing toward +Z.
fn frame_model(commands: &mut Commands, span: f32) -> super::kit::Model {
    let mut mb = ModelBuilder::new(format!("rinse_frame_{}", (span * 100.0).round() as u32));
    let pole = culm(span, POLE_RADIUS, 0.36, 1);
    mb.add(
        Mat::Bamboo,
        place(
            pole,
            Vec3::new(-span * 0.5, 0.0, 0.0),
            Quat::from_rotation_z(-FRAC_PI_2),
        ),
    );
    // The crossing sits below the pole so the stakes' fork cradles it.
    let cross_y = -(POLE_RADIUS + STAKE_RADIUS) / STAKE_LEAN.sin();
    let waterline = -POLE_ABOVE;
    for (side, x) in [(0u32, -span * 0.5 + 0.32), (1, span * 0.5 - 0.32)] {
        let c = Vec3::new(x, cross_y, 0.0);
        for (k, lean) in [STAKE_LEAN, -STAKE_LEAN].into_iter().enumerate() {
            let dir = Quat::from_rotation_x(lean) * Vec3::Y;
            let foot = c - dir * 2.4;
            let stake = culm(2.4 + 0.42, STAKE_RADIUS, 0.3, 10 + side * 2 + k as u32);
            let stake = place(stake, foot, Quat::from_rotation_arc(Vec3::Y, dir));
            // Dark and slick below the waterline, where algae grows.
            mb.add(Mat::Bamboo, wet_below(stake, waterline));
        }
        // A square lashing round the crossing: turns about each stake, frapped between.
        for k in 0..4 {
            let lean = if k % 2 == 0 { STAKE_LEAN } else { -STAKE_LEAN };
            let along = Quat::from_rotation_x(lean);
            let offset = along * Vec3::Y * (0.03 * (k / 2) as f32 - 0.015);
            mb.add_small(
                Mat::Rope,
                place(
                    torus(STAKE_RADIUS * 2.2, 0.0055, 16, 6),
                    c + offset,
                    along * Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_x(PI * 0.1),
                ),
            );
        }
        // And one turn binding the pole into the fork.
        mb.add_small(
            Mat::Rope,
            place(
                torus(POLE_RADIUS + 0.006, 0.006, 18, 6),
                Vec3::new(x + 0.05, 0.0, 0.0),
                Quat::from_rotation_z(FRAC_PI_2),
            ),
        );
    }
    // Two cords round the pole at each silk's corners.
    for k in 0..5 {
        let x = (k as f32 - 2.0) * STRIP_PITCH;
        for dx in [-0.5, 0.5] {
            mb.add_small(
                Mat::Rope,
                place(
                    torus(POLE_RADIUS + 0.004, 0.0045, 18, 6),
                    Vec3::new(x + dx * (STRIP_WIDTH - 0.05), 0.0, 0.0),
                    Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_y(0.4),
                ),
            );
        }
    }
    mb.build(commands)
}

/// A bamboo culm along +Y from 0 to `length`: tapering slightly, with a ridged node every
/// `internode` (give or take) and the skin darker at each node.
fn culm(length: f32, radius: f32, internode: f32, seed: u32) -> Mesh {
    let r = |y: f32| radius * (1.0 - 0.1 * y / length);
    let mut nodes = Vec::new();
    let mut y = internode * (0.3 + 0.5 * rnd(seed, 0));
    while y < length - 0.05 {
        nodes.push(y);
        y += internode * (0.85 + 0.3 * rnd(seed, nodes.len() as u32));
    }
    let mut profile = vec![
        Vec2::ZERO,
        Vec2::new(r(0.0) * 0.92, 0.0),
        Vec2::new(r(0.0), 0.006),
    ];
    for &n in &nodes {
        let rn = r(n);
        profile.extend([
            Vec2::new(rn, n - 0.014),
            Vec2::new(rn * 1.045, n - 0.003),
            Vec2::new(rn * 1.07, n),
            Vec2::new(rn * 1.03, n + 0.004),
            Vec2::new(rn, n + 0.012),
        ]);
    }
    let top = r(length);
    profile.extend([
        Vec2::new(top, length - 0.006),
        Vec2::new(top * 0.92, length),
        Vec2::new(0.0, length),
    ]);
    let mut mesh = lathe(&profile, 14, LatheUv::meters(radius));
    for v in &mut mesh.vertices {
        let y = v.position[1];
        let d = nodes.iter().map(|n| (y - n).abs()).fold(f32::MAX, f32::min);
        let f = 1.0 - 0.32 * (1.0 - smoothstep(0.004, 0.02, d));
        for c in &mut v.color[..3] {
            *c *= f;
        }
    }
    mesh
}

/// Darken (and green) a mesh below local height `y`: wet wood with a skin of algae.
fn wet_below(mut mesh: Mesh, y: f32) -> Mesh {
    for v in &mut mesh.vertices {
        let k = 1.0 - smoothstep(y - 0.25, y + 0.04, v.position[1]);
        v.color[0] *= 1.0 - 0.62 * k;
        v.color[1] *= 1.0 - 0.5 * k;
        v.color[2] *= 1.0 - 0.66 * k;
    }
    mesh
}

/// A stream for tests and previews: water at `level` flowing at `current` within `half_width`
/// of the line x = 0.
pub struct TestStream {
    pub level: f64,
    pub half_width: f64,
    pub current: Vec3,
}

impl WaterQuery for TestStream {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        (p.x.abs() < self.half_width).then_some(WaterSample {
            surface_height: self.level,
            depth: 0.8,
            velocity: self.current,
            normal: Vec3::Y,
        })
    }
}

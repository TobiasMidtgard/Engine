//! The grapple's rope and hook: a braided steel cable from the pilot's left fist to a
//! three-clawed hook. The hook flies out with the cable snaking and sagging behind it, bites
//! with its claws spread, and the cable snaps taut as the reel pulls.
//!
//! The rope is a chain of cable segments re-laid every frame between the fist (as drawn,
//! between fixed steps) and the hook. Nothing here moves the character; see
//! [`crate::grapple`].

use std::f32::consts::{PI, TAU};

use engine_core::math::{DVec3, Mat4, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_core::transform::TransformInterpolation;
use engine_render::{MeshRenderer, StandardMaterial};
use engine_world::physics::CharacterController;

use super::fighter::Style;
use super::rig::{KNUCKLE_L, Xf};
use super::{Library, MeshBuilder, NoGhost};
use crate::demo::Puppet;
use crate::grapple;
use crate::mover::Mover;

/// Cable segments from the fist to the hook.
pub const SEGMENTS: usize = 24;
/// Cable radius, m.
const CABLE_RADIUS: f32 = 0.0075;

const CABLE_MESH: &str = "grapple/cable";
const HOOK_MESH: &str = "grapple/hook";
const CLAWS_CLOSED: &str = "grapple/claws_closed";
const CLAWS_OPEN: &str = "grapple/claws_open";
const GLOW_MESH: &str = "grapple/glow";
const CABLE_MATERIAL: &str = "grapple/cable";
const STEEL_MATERIAL: &str = "grapple/steel";

fn glow_material(style: Style) -> String {
    format!("grapple/glow/{}", style.key())
}

/// Registers the cable, the hook (claws folded for flight, spread once bitten) and their
/// materials.
pub(crate) fn generate(lib: &mut Library) {
    // One meter of cable along +Y (stretched per segment), with a slow twist in the shading
    // so the braid reads.
    let mut cable = MeshBuilder::tube(&[Vec3::ZERO, Vec3::Y], CABLE_RADIUS, 8);
    cable.shade(|p, n| 0.85 + 0.15 * (n.x.atan2(n.z) * 3.0 + p.y * 40.0).sin());
    lib.mesh(CABLE_MESH, cable);

    // The hook points along +Y: the cable ties on at the origin, the tip is 0.17 m out.
    let shaft = MeshBuilder::lathe(
        &[
            Vec2::new(0.0, -0.012),
            Vec2::new(0.012, -0.01),
            Vec2::new(0.016, 0.0),
            Vec2::new(0.024, 0.02),
            Vec2::new(0.024, 0.085),
            Vec2::new(0.019, 0.11),
            Vec2::new(0.008, 0.15),
            Vec2::new(0.0, 0.172),
        ],
        16,
    );
    lib.mesh(HOOK_MESH, shaft);
    lib.mesh(CLAWS_CLOSED, claws(false));
    lib.mesh(CLAWS_OPEN, claws(true));
    let ring = MeshBuilder::lathe(
        &[
            Vec2::new(0.0245, 0.038),
            Vec2::new(0.0262, 0.042),
            Vec2::new(0.0262, 0.052),
            Vec2::new(0.0245, 0.056),
        ],
        20,
    );
    lib.mesh(GLOW_MESH, ring);

    lib.material(
        CABLE_MATERIAL,
        StandardMaterial::hex(0x23272b)
            .with_metallic(0.85)
            .with_roughness(0.42),
    );
    lib.material(
        STEEL_MATERIAL,
        StandardMaterial::hex(0x8d949a)
            .with_metallic(1.0)
            .with_roughness(0.28),
    );
    for style in Style::ALL {
        let [r, g, b] = style.glow();
        lib.material(
            &glow_material(style),
            StandardMaterial::hex(0x202020)
                .with_roughness(0.3)
                .with_emissive([r, g, b], 45.0),
        );
    }
}

/// Three claws around the shaft: folded flat along it for flight, or swung out and back like
/// an anchor's flukes once the hook bites.
fn claws(open: bool) -> MeshBuilder {
    let path: Vec<Vec3> = (0..=8)
        .map(|i| {
            let t = i as f32 / 8.0;
            if open {
                // Out from the collar, sweeping back toward the cable.
                let a = t * 2.1;
                Vec3::new(0.022 + 0.07 * a.sin(), 0.075 - 0.07 * (1.0 - a.cos()), 0.0)
            } else {
                Vec3::new(0.025 + 0.006 * (t * PI).sin(), 0.06 + 0.07 * t, 0.0)
            }
        })
        .collect();
    let one = MeshBuilder::tube(&path, 0.0055, 6);
    let mut all = MeshBuilder::new();
    for k in 0..3 {
        all.append(&one, Mat4::from_rotation_y(k as f32 * TAU / 3.0));
    }
    all
}

/// The rope and hook entities of one character (on the character).
#[derive(Component, Debug, Clone)]
pub struct GrappleRope {
    segments: [Entity; SEGMENTS],
    hook: Entity,
    claws: Entity,
    collar: Entity,
    open: bool,
    shown: bool,
}

/// A piece of a character's rope (despawned with the character).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct RopePart {
    owner: Entity,
}

fn hidden(mesh: &str, material: &str) -> MeshRenderer {
    let mut r = MeshRenderer::new(super::mesh(mesh), super::material(material));
    r.visible = false;
    r
}

fn spawn_rope(commands: &mut Commands, owner: Entity, style: Style) -> GrappleRope {
    let mut part = |name: &'static str, r: MeshRenderer| {
        commands
            .spawn((
                Name::new(name),
                Transform::default(),
                r,
                RopePart { owner },
                NoGhost,
            ))
            .id()
    };
    let segments =
        std::array::from_fn(|_| part("Grapple cable", hidden(CABLE_MESH, CABLE_MATERIAL)));
    let hook = part("Grapple hook", hidden(HOOK_MESH, STEEL_MATERIAL));
    // The claws and the glowing collar ride on the hook.
    let claws = commands
        .spawn((
            Name::new("Grapple claws"),
            Transform::default(),
            hidden(CLAWS_CLOSED, STEEL_MATERIAL),
            ChildOf(hook),
            NoGhost,
        ))
        .id();
    let collar = commands
        .spawn((
            Name::new("Grapple collar"),
            Transform::default(),
            hidden(GLOW_MESH, &glow_material(style)).without_shadows(),
            ChildOf(hook),
            NoGhost,
        ))
        .id();
    GrappleRope {
        segments,
        hook,
        claws,
        collar,
        open: false,
        shown: false,
    }
}

/// Points along the rope from `from` (the fist) to `to` (the hook): straight once taut,
/// snaking and sagging behind a flying hook (`slack` 1) and settling as it snaps taut.
pub fn rope_points(
    from: DVec3,
    to: DVec3,
    up: Vec3,
    slack: f32,
    secs: f32,
) -> [DVec3; SEGMENTS + 1] {
    let span = (to - from).as_vec3();
    let len = span.length();
    let dir = span / len.max(1e-4);
    let side = dir.cross(up).normalize_or(Vec3::X);
    let sag_dir = (side.cross(dir)).normalize_or(-up);
    let sag_dir = if sag_dir.dot(up) > 0.0 {
        -sag_dir
    } else {
        sag_dir
    };
    let reach = (len / 8.0).min(1.0);
    std::array::from_fn(|i| {
        let s = i as f32 / SEGMENTS as f32;
        let belly = (s * PI).sin();
        let wave = (TAU * 2.5 * s - secs * 32.0).sin() * 0.11 * reach;
        let sag = 0.32 * reach;
        let off = (side * wave + sag_dir * sag) * belly * slack;
        from + (span * s + off).as_dvec3()
    })
}

/// Where a body is drawn this frame (between its last two fixed steps).
fn visual(t: &Transform, interp: Option<&TransformInterpolation>, alpha: f32) -> Transform {
    interp
        .and_then(|i| i.previous)
        .map_or(*t, |p| p.lerp(t, alpha))
}

/// Lays every rope from its pilot's fist to the hook (`Update`, after the puppets are posed).
#[allow(clippy::type_complexity)]
pub(crate) fn draw_ropes(
    mut commands: Commands,
    time: Res<Time>,
    fixed: Res<FixedTime>,
    mut owners: Query<(
        Entity,
        &Mover,
        &Transform,
        Option<&TransformInterpolation>,
        &CharacterController,
        &Puppet,
        Option<&mut GrappleRope>,
    )>,
    mut parts: Query<(&mut Transform, &mut MeshRenderer), Without<Mover>>,
) {
    let alpha = fixed.alpha();
    let secs = time.elapsed_secs();
    for (owner, m, t, interp, cc, puppet, rope) in &mut owners {
        let Some(mut rope) = rope else {
            if m.grapple.is_some() {
                let rope = spawn_rope(&mut commands, owner, puppet.style());
                commands.entity(owner).insert(rope);
            }
            continue;
        };
        let shown = m.grapple.is_some();
        if rope.shown != shown {
            rope.shown = shown;
            let all = rope
                .segments
                .iter()
                .chain([&rope.hook, &rope.claws, &rope.collar]);
            for &e in all {
                if let Ok((_, mut r)) = parts.get_mut(e) {
                    r.visible = shown;
                }
            }
        }
        let Some(g) = m.grapple else {
            continue;
        };
        let body = visual(t, interp, alpha);
        let hand: Xf = puppet.hand_l();
        let fist = hand.pos + hand.rot * (KNUCKLE_L * 0.7);
        let from = body.translation + (body.rotation * fist).as_dvec3();
        let shoulders = grapple::shoulders(t.translation, m.up, cc.height);
        let hook = grapple::hook_position(&g, shoulders);
        let slack = if g.attached {
            (-g.secs * 16.0).exp()
        } else {
            1.0
        };
        let points = rope_points(from, hook, m.up, slack, secs);
        for (i, &e) in rope.segments.iter().enumerate() {
            let (a, b) = (points[i], points[i + 1]);
            let d = (b - a).as_vec3();
            let len = d.length().max(1e-4);
            if let Ok((mut pt, _)) = parts.get_mut(e) {
                pt.translation = a;
                pt.rotation = Quat::from_rotation_arc(Vec3::Y, d / len);
                pt.scale = Vec3::new(1.0, len, 1.0);
            }
        }
        // The hook: along its flight, or tip into the surface once it bites.
        let flight = (hook - from).as_vec3().normalize_or(Vec3::NEG_Z);
        let into = if g.attached && g.normal != Vec3::ZERO {
            -g.normal
        } else {
            flight
        };
        if let Ok((mut ht, _)) = parts.get_mut(rope.hook) {
            ht.translation = hook - (into * 0.12).as_dvec3();
            ht.rotation = Quat::from_rotation_arc(Vec3::Y, into);
        }
        if rope.open != g.attached {
            rope.open = g.attached;
            if let Ok((_, mut r)) = parts.get_mut(rope.claws) {
                r.mesh = super::mesh(if g.attached { CLAWS_OPEN } else { CLAWS_CLOSED });
            }
        }
    }
}

/// Despawns the ropes of characters that are gone (`Update`).
pub(crate) fn drop_orphan_ropes(
    mut commands: Commands,
    parts: Query<(Entity, &RopePart)>,
    owners: Query<(), With<Mover>>,
) {
    for (e, part) in &parts {
        if !owners.contains(part.owner) {
            commands.entity(e).despawn();
        }
    }
}

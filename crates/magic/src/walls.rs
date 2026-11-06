//! Walls that stand (round-2-plan.md, "Walls that stand up"). Every piece along a wall spell
//! raises a body of its element: jagged ice for frost, a bed of coals under a flame curtain
//! for fire, copper posts with arcs leaping between them for storm, a hanging veil for the
//! misty and dark weaves. Bodies pop up the way ice-wall artists ask for (80.lv "Ice spikes VFX
//! production tips", realtimevfx "WIP ice wall"): fast, past their height, then snap back,
//! with no slow rise; they crack and sink when the wall ends, and change with the wall when a
//! spell meets it (frost to steam).

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{AlphaMode, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};

use crate::looks::{LookCmd, Role, WallFamily, family_of};
use crate::runtime::{LookPiece, MagicQueue, roll};

/// Seconds a body takes to stand: up past its height in the first two thirds, back after.
pub const RISE_SECS: f32 = 0.18;
/// How far past its height a body springs (1.12 = 12 %).
pub const OVERSHOOT: f32 = 1.12;
/// Seconds a body takes to crack and sink.
pub const SINK_SECS: f32 = 0.7;

/// What a wall piece is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stuff {
    /// Jagged translucent ice.
    Ice,
    /// A bed of glowing coals (the flame rides on the piece's look).
    Coals,
    /// A copper post; arcs leap to the next one.
    Post,
    /// A hanging sheet of mist, blood, blight or shadow.
    Veil,
}

pub fn stuff(look: &str) -> Stuff {
    match family_of(look) {
        WallFamily::Frost => Stuff::Ice,
        WallFamily::Fire => Stuff::Coals,
        WallFamily::Storm => Stuff::Post,
        WallFamily::Other => Stuff::Veil,
    }
}

/// A veil's colour and opacity, and how much it glows.
fn veil(look: &str, color: [f32; 3]) -> ([f32; 4], f32) {
    match look {
        "steam" => ([0.88, 0.9, 0.93, 0.32], 0.0),
        "blood" | "nightfeed" => ([0.32, 0.01, 0.02, 0.5], 0.35),
        "blight" | "plague" | "wither" | "frostrot" => ([0.3, 0.42, 0.06, 0.45], 0.6),
        "shadow" | "doom" | "night_terror" | "ghost_step" | "dread" => {
            ([0.015, 0.01, 0.025, 0.85], 0.0)
        }
        _ => ([color[0], color[1], color[2], 0.38], 0.9),
    }
}

/// The body a piece of a wall `width` m wide stands up, its height, and whether its mesh
/// sits on its base (else it's centered).
pub fn body(look: &str, color: [f32; 3], width: f32, seed: u64) -> (MeshRenderer, f32, bool) {
    let jitter = roll(seed, 1);
    match stuff(look) {
        Stuff::Ice => {
            let h = 2.0 + 0.7 * jitter;
            (
                MeshRenderer::new(
                    MeshRef::Shape(Shape::Rock {
                        size: [width * 1.15, h, 0.95],
                        seed: (seed % 100_000) as u32,
                        detail: 3,
                    }),
                    // Glacier ice: pale blue, nearly solid, lit from inside so it reads at
                    // dusk as well as at noon (a thin glassy slab vanished at golden hour).
                    MaterialRef::Inline(StandardMaterial {
                        base_color: [0.6, 0.85, 1.0, 0.9],
                        emissive: [0.3, 0.62, 1.0],
                        emissive_strength: 1.4,
                        perceptual_roughness: 0.12,
                        reflectance: 0.7,
                        alpha_mode: AlphaMode::Blend,
                        ..Default::default()
                    }),
                ),
                h,
                true,
            )
        }
        Stuff::Coals => (
            MeshRenderer::new(
                MeshRef::Shape(Shape::Rock {
                    size: [width * 1.05, 0.28, 0.7],
                    seed: (seed % 100_000) as u32,
                    detail: 2,
                }),
                MaterialRef::Inline(StandardMaterial {
                    base_color: [0.08, 0.03, 0.02, 1.0],
                    emissive: [1.0, 0.32, 0.06],
                    emissive_strength: 5.0,
                    perceptual_roughness: 0.9,
                    ..Default::default()
                }),
            ),
            0.28,
            true,
        ),
        Stuff::Post => {
            let h = 2.3 + 0.3 * jitter;
            (
                MeshRenderer::new(
                    MeshRef::Shape(Shape::Cylinder {
                        radius: 0.07,
                        height: h,
                        segments: 10,
                    }),
                    MaterialRef::Inline(StandardMaterial {
                        base_color: [0.72, 0.38, 0.2, 1.0],
                        metallic: 1.0,
                        perceptual_roughness: 0.35,
                        emissive: [0.55, 0.6, 1.0],
                        emissive_strength: 0.6,
                        ..Default::default()
                    }),
                ),
                h,
                false,
            )
        }
        Stuff::Veil => {
            let h = 2.6;
            let (rgba, glow) = veil(look, color);
            (
                MeshRenderer::new(
                    MeshRef::Shape(Shape::Cuboid {
                        size: [width * 1.04, h, 0.05],
                    }),
                    MaterialRef::Inline(StandardMaterial {
                        base_color: rgba,
                        emissive: [rgba[0], rgba[1], rgba[2]],
                        emissive_strength: glow,
                        perceptual_roughness: 0.95,
                        alpha_mode: AlphaMode::Blend,
                        ..Default::default()
                    }),
                )
                .without_shadows(),
                h,
                false,
            )
        }
    }
}

/// Height scale over a body's life: up past its height fast, then back.
pub fn rise(age: f32) -> f32 {
    let up = RISE_SECS * 0.65;
    if age < up {
        OVERSHOOT * (age / up).max(0.0)
    } else if age < RISE_SECS {
        OVERSHOOT + (1.0 - OVERSHOOT) * (age - up) / (RISE_SECS - up)
    } else {
        1.0
    }
}

/// A wall piece's body (a child of the piece).
#[derive(Component, Debug, Clone)]
pub struct WallBody {
    pub piece: Entity,
    pub look: String,
    pub color: [f32; 3],
    pub stuff: Stuff,
    pub height: f32,
    /// The mesh sits on its base (else centered: lifted by half its height).
    pub based: bool,
    pub rest: Quat,
    pub age: f32,
    /// Seconds since it began to crack and sink.
    pub sink: Option<f32>,
    pub next_arc: f32,
    pub seed: u64,
}

/// What a new body needs, queued for the flush.
#[derive(Debug, Clone)]
pub struct NewBody {
    pub piece: Entity,
    pub look: String,
    pub color: [f32; 3],
    pub along: Vec3,
    pub width: f32,
    pub seed: u64,
}

/// Spawn a body for a piece (from the flush).
pub fn spawn_body(commands: &mut Commands, b: NewBody) {
    let (mesh, height, based) = body(&b.look, b.color, b.width, b.seed);
    // Stand across the wall's line, leaning a little each its own way.
    let lean = (roll(b.seed, 2) - 0.5) * 0.16;
    let twist = (roll(b.seed, 3) - 0.5) * 0.25;
    let rest = Quat::from_rotation_arc(Vec3::X, b.along.normalize_or(Vec3::X))
        * Quat::from_rotation_y(twist)
        * Quat::from_rotation_x(lean);
    commands.spawn((
        Name::new("Spell wall body"),
        Transform::from_rotation(rest).with_scale(Vec3::new(1.0, 0.001, 1.0)),
        mesh,
        WallBody {
            piece: b.piece,
            stuff: stuff(&b.look),
            look: b.look,
            color: b.color,
            height,
            based,
            rest,
            age: 0.0,
            sink: None,
            next_arc: roll(b.seed, 4) * 0.3,
            seed: b.seed,
        },
        ChildOf(b.piece),
    ));
}

/// Stand bodies up, change them with their wall, arc between posts, sink them when it ends.
pub fn rise_walls(
    time: Res<Time>,
    mut bodies: Query<(Entity, &mut WallBody, &mut Transform)>,
    pieces: Query<(&LookPiece, &GlobalTransform)>,
    mut queue: ResMut<MagicQueue>,
) {
    let dt = time.delta_secs();
    for (e, mut b, mut tf) in &mut bodies {
        b.age += dt;
        let piece = pieces.get(b.piece).ok();
        let at = piece.map(|(_, g)| g.translation());
        let changed = piece.is_some_and(|(p, _)| p.look != b.look && p.fading.is_none());
        let gone = piece.is_none_or(|(p, _)| p.fading.is_some()) || changed;
        if gone && b.sink.is_none() {
            b.sink = Some(0.0);
            // Ice cracks: shards from every other slab (a burst from every piece of every
            // wall was a firework). Coals, posts and veils just sink.
            if let Some(at) = at
                && b.stuff == Stuff::Ice
                && b.seed % 2 == 0
            {
                queue.looks.push(LookCmd::Burst {
                    at: at + DVec3::Y * (b.height * 0.5) as f64,
                    up: Vec3::Y,
                    look: b.look.clone(),
                    role: Role::Impact,
                    scale: 0.35,
                    color: b.color,
                });
            }
            if changed && let Some((p, _)) = piece {
                queue.wall_bodies.push(NewBody {
                    piece: b.piece,
                    look: p.look.clone(),
                    color: p.color,
                    along: p.along,
                    width: p.width,
                    seed: b.seed.wrapping_add(7),
                });
            }
        }
        let mut s = rise(b.age);
        if let Some(t) = b.sink.as_mut() {
            *t += dt;
            let k = (*t / SINK_SECS).min(1.0);
            s *= 1.0 - k * k;
            if *t >= SINK_SECS {
                queue.despawn.push(e);
            }
        }
        let s = s.max(0.001);
        tf.rotation = b.rest;
        tf.scale = Vec3::new(1.0, s, 1.0);
        tf.translation = if b.based {
            DVec3::ZERO
        } else {
            DVec3::Y * (b.height * 0.5 * s) as f64
        };
        // Arcs leap from post to post while the wall stands.
        if b.stuff == Stuff::Post && b.sink.is_none() && b.age > RISE_SECS {
            b.next_arc -= dt;
            if b.next_arc <= 0.0 {
                let n = (b.age * 97.0) as u64;
                b.next_arc = 0.12 + roll(b.seed, n) * 0.3;
                if let (Some(at), Some((p, _))) = (at, piece)
                    && let Some(prev) = p.prev
                    && let Ok((_, g)) = pieces.get(prev)
                {
                    let top = |p: DVec3| p + DVec3::Y * (b.height * 0.9) as f64;
                    queue.looks.push(LookCmd::Beam {
                        from: top(at),
                        to: top(g.translation()),
                        look: b.look.clone(),
                        color: b.color,
                        width: 0.05,
                        secs: 0.12,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_spring_past_their_height_then_settle() {
        assert_eq!(rise(0.0), 0.0);
        let peak = (0..100)
            .map(|i| rise(i as f32 * RISE_SECS / 100.0))
            .fold(0.0, f32::max);
        assert!((peak - OVERSHOOT).abs() < 0.02, "{peak}");
        assert_eq!(rise(RISE_SECS), 1.0);
        assert_eq!(rise(5.0), 1.0);
    }

    #[test]
    fn each_element_stands_its_own_wall() {
        assert_eq!(stuff("frost"), Stuff::Ice);
        assert_eq!(stuff("fire"), Stuff::Coals);
        assert_eq!(stuff("storm"), Stuff::Post);
        assert_eq!(stuff("steam"), Stuff::Veil);
        assert_eq!(stuff("shadow"), Stuff::Veil);
        let (_, h, based) = body("frost", [0.5, 0.8, 1.0], 1.5, 3);
        assert!((2.0..=2.7).contains(&h) && based);
        // Shadow hangs dark and nearly opaque; steam is a thin white mist.
        assert!(veil("shadow", [0.0; 3]).0[3] > 0.8);
        assert!(veil("steam", [0.0; 3]).0[3] < 0.4);
    }
}

//! Afterimages: glowing, fading copies of a character left behind while it dashes, wall-runs,
//! slides or swings. They are spaced in **real** time, so they show motion the way the
//! player saw it: a blink-fast dash leaves a few far-apart ghosts, the same dash seen in
//! slow motion leaves a dense trail. That also makes still screenshots show movement.
//!
//! A ghost copies the whole character: every visible `MeshRenderer` in its hierarchy (the
//! root's own too) except entities marked [`NoGhost`], at its world transform at that
//! instant, so armor, weapons and the pose all show. A character without any mesh gets a
//! capsule. The effects feature draws ghosts as a translucent shadow of the body in a deep
//! tint of the character's trail color, edged with light, fading in as the character moves
//! off it and out as it ages (see [`crate::look::effects`]).
//!
//! Ghosts are pooled: a faded [`Afterimage`] entity is reused for the next one, and at most
//! [`MAX_AFTERIMAGES`] exist at once (the oldest is recycled beyond that).

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Mat3, Mat4, Vec3};
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRef, MeshRenderer, ResolvedMesh, Shape};
use engine_world::LocomotionMode;
use engine_world::physics::CharacterController;

use crate::combat::{AttackPhase, CombatState};
use crate::look::NoGhost;
use crate::mover::Mover;

/// Leave afterimages behind this character.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Afterimages {
    pub enabled: bool,
    /// Linear RGB of the glow.
    pub color: [f32; 3],
    /// Emission of a fresh afterimage's body, cd/m².
    pub glow: f32,
    /// Opacity of a fresh afterimage, 0..1.
    pub alpha: f32,
    /// Real seconds between afterimages.
    pub interval_secs: f32,
    /// Meters between afterimages (keeps slow-motion trails readable).
    pub spacing_m: f32,
    /// Real seconds an afterimage takes to fade away.
    pub lifetime_secs: f32,
    /// Also leave afterimages whenever moving faster than this, m/s (world).
    pub min_speed: f32,
    /// Emission of the glowing rim along a fresh afterimage's silhouette, cd/m² (at the
    /// effects' reference exposure; daylight needs thousands).
    pub rim_glow: f32,
    #[serde(skip)]
    since_last: f32,
    #[serde(skip)]
    last_at: Option<DVec3>,
}

impl Default for Afterimages {
    fn default() -> Self {
        Self {
            enabled: true,
            color: [0.35, 0.8, 1.0],
            glow: 60.0,
            alpha: 0.3,
            interval_secs: 0.03,
            spacing_m: 0.55,
            lifetime_secs: 0.5,
            min_speed: 14.0,
            rim_glow: 14_000.0,
            since_last: f32::MAX,
            last_at: None,
        }
    }
}

impl Afterimages {
    /// Afterimages in a color (linear RGB).
    pub fn colored(color: [f32; 3]) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }

    /// The same afterimages, lasting `secs` real seconds.
    pub fn lasting(mut self, secs: f32) -> Self {
        self.lifetime_secs = secs;
        self
    }
}

/// Afterimages alive at once over all characters; beyond this the oldest is reused.
pub const MAX_AFTERIMAGES: usize = 160;

/// One copied body part of an [`Afterimage`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostPart {
    /// The part it copies.
    pub source: Entity,
    /// Its mesh, if already resolved (otherwise looked up from `source` when drawn).
    pub mesh: Option<Handle<Mesh>>,
    /// The part's transform relative to the afterimage's `anchor` (meters, no f64).
    pub transform: Mat4,
}

/// One afterimage (spawned and recycled by the movement module).
#[derive(Component, Debug, Clone)]
pub struct Afterimage {
    /// Real seconds since it appeared.
    pub age: f32,
    pub lifetime: f32,
    /// The character it copies.
    pub source: Option<Entity>,
    /// World position its parts are relative to (the character's position).
    pub anchor: DVec3,
    /// Every visible part of the character, frozen.
    pub parts: Vec<GhostPart>,
    style: Afterimages,
    seed: f32,
    active: bool,
}

/// What the effects feature needs to draw a ghost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GhostStyle {
    pub age01: f32,
    pub color: [f32; 3],
    pub rim_nits: f32,
    pub glow: f32,
    pub alpha: f32,
    pub seed: f32,
}

impl Afterimage {
    /// Still showing (faded afterimages wait in the pool).
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn draw_style(&self) -> Option<GhostStyle> {
        if !self.active || self.parts.is_empty() {
            return None;
        }
        Some(GhostStyle {
            age01: (self.age / self.lifetime.max(1e-3)).clamp(0.0, 1.0),
            color: self.style.color,
            rim_nits: self.style.rim_glow,
            glow: self.style.glow,
            alpha: self.style.alpha.clamp(0.0, 1.0),
            seed: self.seed,
        })
    }
}

fn should_trail(m: &Mover, cs: Option<&CombatState>, speed: f32, min_speed: f32) -> bool {
    matches!(
        m.mode,
        LocomotionMode::Dash | LocomotionMode::WallRun | LocomotionMode::Slide
    ) || speed >= min_speed
        || cs
            .and_then(|c| c.attack)
            .is_some_and(|a| a.phase == AttackPhase::Strike)
}

/// `world` relative to `anchor`, as an f32 matrix.
fn relative(world: &GlobalTransform, anchor: DVec3) -> Mat4 {
    let a = world.affine();
    let m: Mat3 = a.matrix3.as_mat3();
    let t = (a.translation - anchor).as_vec3();
    Mat4::from_cols(
        m.x_axis.extend(0.0),
        m.y_axis.extend(0.0),
        m.z_axis.extend(0.0),
        t.extend(1.0),
    )
}

/// Spawn afterimages (PostUpdate, after transforms settle, so ghosts copy this frame's pose).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn spawn_afterimages(
    mut commands: Commands,
    time: Res<Time>,
    mut chars: Query<(
        Entity,
        &GlobalTransform,
        &Mover,
        &CharacterController,
        &mut Afterimages,
        Option<&CombatState>,
        Option<&engine_world::locomotion::LocomotionState>,
    )>,
    parts: Query<(
        &GlobalTransform,
        Option<&MeshRenderer>,
        Option<&ResolvedMesh>,
        Option<&Children>,
        Has<NoGhost>,
    )>,
    mut ghosts: Query<(Entity, &mut Afterimage)>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut stack: Local<Vec<Entity>>,
    mut copied: Local<Vec<GhostPart>>,
    mut capsules: Local<Vec<([f32; 2], Handle<Mesh>)>>,
    mut seq: Local<u32>,
) {
    let real = time.real_delta_secs();
    if real <= 0.0 {
        return;
    }
    let mut spawned = 0;
    for (root, gt, m, cc, mut style, cs, loco) in &mut chars {
        if !style.enabled {
            continue;
        }
        style.since_last += real;
        let speed = loco.map_or(m.velocity.length(), |l| l.velocity.length());
        if !should_trail(m, cs, speed, style.min_speed) {
            continue;
        }
        let at = gt.translation();
        let near = style
            .last_at
            .is_some_and(|p| p.distance(at) < style.spacing_m as f64);
        if style.since_last < style.interval_secs || near {
            continue;
        }
        style.since_last = 0.0;
        style.last_at = Some(at);

        // Copy every visible part of the hierarchy.
        copied.clear();
        stack.clear();
        stack.push(root);
        while let Some(e) = stack.pop() {
            let Ok((part_gt, renderer, resolved, children, no_ghost)) = parts.get(e) else {
                continue;
            };
            if let Some(r) = renderer
                && r.visible
                && !no_ghost
            {
                let mesh = resolved.map(|r| r.mesh).or(match r.mesh {
                    MeshRef::Handle(h) => Some(h),
                    _ => None,
                });
                copied.push(GhostPart {
                    source: e,
                    mesh,
                    transform: relative(part_gt, at),
                });
            }
            if let Some(children) = children {
                stack.extend(children.iter());
            }
        }
        if copied.is_empty() {
            // A bare character: a capsule leaning like the body.
            let (radius, height) = (cc.radius * 0.85, cc.height * 0.95);
            let mesh = meshes.as_deref_mut().map(|meshes| {
                match capsules.iter().find(|(k, _)| *k == [radius, height]) {
                    Some((_, h)) => *h,
                    None => {
                        let shape = Shape::Capsule {
                            radius,
                            height,
                            segments: 16,
                        };
                        let h = meshes.get_or_insert_keyed(&shape.key(), || shape.mesh());
                        capsules.push(([radius, height], h));
                        h
                    }
                }
            });
            copied.push(GhostPart {
                source: root,
                mesh,
                transform: Mat4::from_scale_rotation_translation(
                    gt.scale(),
                    gt.rotation() * m.body_tilt(),
                    Vec3::ZERO,
                ),
            });
        }

        *seq = seq.wrapping_add(1);
        let seed = (*seq as f32 * 0.618_034).fract();
        let fill = |g: &mut Afterimage| {
            g.age = 0.0;
            g.lifetime = style.lifetime_secs.max(0.05);
            g.source = Some(root);
            g.anchor = at;
            g.parts.clear();
            g.parts.extend_from_slice(&copied);
            g.style = *style;
            g.seed = seed;
            g.active = true;
        };
        let free = ghosts.iter().find(|(_, g)| !g.active).map(|(e, _)| e);
        let reuse = free.or_else(|| {
            (ghosts.iter().count() + spawned >= MAX_AFTERIMAGES)
                .then(|| {
                    ghosts
                        .iter()
                        .max_by(|a, b| {
                            (a.1.age / a.1.lifetime).total_cmp(&(b.1.age / b.1.lifetime))
                        })
                        .map(|(e, _)| e)
                })
                .flatten()
        });
        match reuse.and_then(|e| ghosts.get_mut(e).ok()) {
            Some((_, mut g)) => fill(&mut g),
            None => {
                let mut g = Afterimage {
                    age: 0.0,
                    lifetime: 0.0,
                    source: None,
                    anchor: DVec3::ZERO,
                    parts: Vec::with_capacity(copied.len()),
                    style: *style,
                    seed,
                    active: false,
                };
                fill(&mut g);
                commands.spawn((Name::new("Afterimage"), g));
                spawned += 1;
            }
        }
    }
}

/// Age afterimages and return faded ones to the pool (PostUpdate).
pub(crate) fn fade_afterimages(
    time: Res<Time>,
    mut diagnostics: Option<ResMut<Diagnostics>>,
    mut q: Query<&mut Afterimage>,
) {
    let real = time.real_delta_secs();
    let mut count = 0;
    for mut a in &mut q {
        if !a.active {
            continue;
        }
        a.age += real;
        if a.age >= a.lifetime {
            a.active = false;
            a.parts.clear();
            continue;
        }
        count += 1;
    }
    if let Some(d) = diagnostics.as_deref_mut() {
        d.set("movement.afterimages", count as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MovementPlugin, MovementProfile};
    use engine_core::math::Quat;
    use engine_core::time::TimeSource;
    use engine_render::StandardMaterial;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin)
            .add_plugins(MovementPlugin);
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app
    }

    fn mesh(shape: Shape) -> MeshRenderer {
        MeshRenderer::new(shape, StandardMaterial::default())
    }

    fn ghosts_of(app: &mut App, who: Entity) -> Vec<Afterimage> {
        let mut q = app.world.query::<&Afterimage>();
        q.iter(&app.world)
            .filter(|g| g.is_active() && g.source == Some(who))
            .cloned()
            .collect()
    }

    #[test]
    fn ghosts_copy_every_visible_part_of_a_body() {
        let mut app = app();
        let root = app
            .world
            .spawn((
                Transform::from_xyz(5.0, 1.0, -3.0).with_rotation(Quat::from_rotation_y(0.7)),
                MovementProfile::pilot(),
                Afterimages {
                    min_speed: 0.0,
                    ..Default::default()
                },
            ))
            .id();
        let body = app
            .world
            .spawn((
                Transform::from_xyz(0.0, -0.2, 0.0),
                mesh(Shape::Capsule {
                    radius: 0.3,
                    height: 1.3,
                    segments: 8,
                }),
                ChildOf(root),
            ))
            .id();
        let head = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 0.9, 0.0),
                mesh(Shape::Sphere {
                    radius: 0.17,
                    segments: 8,
                }),
                ChildOf(body),
            ))
            .id();
        // The sword's base has no mesh; its blade is a grandchild.
        let sword = app
            .world
            .spawn((
                Transform::from_xyz(0.4, 0.1, 0.0).with_rotation(Quat::from_rotation_x(-0.5)),
                ChildOf(root),
            ))
            .id();
        let blade = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 0.0, -0.55),
                mesh(Shape::Cuboid {
                    size: [0.04, 0.09, 1.1],
                }),
                ChildOf(sword),
            ))
            .id();
        // Left out: a helper marked NoGhost and a hidden mesh.
        app.world.spawn((
            Transform::default(),
            mesh(Shape::Cuboid { size: [1.0; 3] }),
            NoGhost,
            ChildOf(root),
        ));
        let mut hidden = mesh(Shape::Cuboid { size: [1.0; 3] });
        hidden.visible = false;
        app.world
            .spawn((Transform::default(), hidden, ChildOf(root)));

        app.update();
        let ghosts = ghosts_of(&mut app, root);
        assert_eq!(ghosts.len(), 1, "one ghost after the first frame");
        let g = &ghosts[0];
        let mut sources: Vec<Entity> = g.parts.iter().map(|p| p.source).collect();
        sources.sort();
        let mut want = vec![body, head, blade];
        want.sort();
        assert_eq!(sources, want);
        // Each part sits where it was in the world, relative to the anchor.
        for p in &g.parts {
            let world = app.world.get::<GlobalTransform>(p.source).unwrap();
            let expected = relative(world, g.anchor);
            assert!(
                p.transform.abs_diff_eq(expected, 1e-4),
                "{:?} vs {:?}",
                p.transform,
                expected
            );
        }
        let part = |e: Entity| g.parts.iter().find(|p| p.source == e).unwrap().transform;
        let neck = part(head).w_axis.truncate() - part(body).w_axis.truncate();
        assert!((neck.length() - 0.9).abs() < 1e-4, "{neck}");
        // The blade keeps the sword's tilt and the body's turn.
        let blade_dir = part(blade).transform_vector3(Vec3::NEG_Z);
        let expected = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.5) * Vec3::NEG_Z;
        assert!(blade_dir.abs_diff_eq(expected, 1e-4), "{blade_dir}");
    }

    #[test]
    fn a_bare_character_leaves_a_capsule_and_ghosts_are_pooled() {
        let mut app = app();
        let root = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 50.0, 0.0),
                MovementProfile::pilot(),
                Afterimages {
                    min_speed: 0.0,
                    spacing_m: 0.0,
                    interval_secs: 0.0,
                    ..Default::default()
                },
            ))
            .id();
        app.update();
        let ghosts = ghosts_of(&mut app, root);
        assert_eq!(ghosts.len(), 1);
        assert_eq!(ghosts[0].parts.len(), 1);
        assert_eq!(ghosts[0].parts[0].source, root);
        // One ghost per frame, each fading in 0.5 s: the pool stops growing.
        for _ in 0..90 {
            app.update();
        }
        let total = app.world.query::<&Afterimage>().iter(&app.world).count();
        let active = ghosts_of(&mut app, root).len();
        assert!(active <= 31, "{active} active");
        assert!(total <= 33, "{total} pooled");
    }
}

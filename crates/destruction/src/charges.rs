//! Breaching charges (Rainbow Six Siege style): an explosive charge blows a hole through soft
//! walls and barricades; a thermite charge burns a clean opening around its edge, cutting rebar
//! too, so it gets through reinforced walls, and through any reinforcement panels set behind
//! the wall it is on ([`crate::reinforce`]).

use engine_core::math::{DVec3, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRef, MeshRenderer, Shape};
use engine_world::WorldImpulse;

use crate::assets::DestructionAssets;
use crate::debris::{DebrisLook, DebrisSpawn, PendingDebris};
use crate::hits::{Hit, HitQueue};
use crate::material::DestructibleMaterial;
use crate::reinforce::{ReinforcedBy, Reinforcement, to_panel};
use crate::wall::{DestructibleWall, WallState};

/// What a [`BreachCharge`] does when it goes off.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeKind {
    /// Blast of `energy_j` joules against the wall face (4e5 opens drywall and planks; it only
    /// strips concrete off a reinforced wall's rebar mesh).
    Explosive { energy_j: f32 },
    /// Burns a `size` [w, h] meters opening around its edge over `burn_secs`, through anything.
    Thermite { size: [f32; 2], burn_secs: f32 },
}

impl Default for ChargeKind {
    fn default() -> Self {
        ChargeKind::Explosive { energy_j: 4.0e5 }
    }
}

/// A charge stuck to a [`DestructibleWall`]. It keeps itself on the wall face at `at`
/// (wall-local [x, y] meters), counts down `fuse_secs` once `armed` and goes off.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct BreachCharge {
    pub kind: ChargeKind,
    /// The wall it is stuck to.
    #[serde(skip)]
    pub wall: Option<Entity>,
    /// Center on the wall, wall-local meters.
    pub at: Vec2,
    /// On the back (-Z) face instead of the front.
    pub back: bool,
    /// Seconds from arming to going off.
    pub fuse_secs: f32,
    /// Counting down.
    pub armed: bool,
    /// Seconds since it was armed.
    pub timer: f32,
    /// Thermite burn progress 0..1.
    pub progress: f32,
    /// Velocity (wall-local, m/s; +Z is out of the front) a thermite-cut panel leaves with, so
    /// it can be lifted clear of what is under it (a climber on the ladder under a hatch); zero
    /// lets the cut push it straight out.
    pub tip: Vec3,
}

impl Default for BreachCharge {
    fn default() -> Self {
        Self {
            kind: ChargeKind::default(),
            wall: None,
            at: Vec2::new(0.0, 1.1),
            back: false,
            fuse_secs: 2.0,
            armed: true,
            timer: 0.0,
            progress: 0.0,
            tip: Vec3::ZERO,
        }
    }
}

impl BreachCharge {
    /// An explosive charge on `wall` at wall-local `at`.
    pub fn explosive(wall: Entity, at: Vec2, energy_j: f32) -> Self {
        Self {
            kind: ChargeKind::Explosive { energy_j },
            wall: Some(wall),
            at,
            ..Self::default()
        }
    }

    /// A thermite charge on `wall` cutting a `size` opening centered at `at`.
    pub fn thermite(wall: Entity, at: Vec2, size: [f32; 2]) -> Self {
        Self {
            kind: ChargeKind::Thermite {
                size,
                burn_secs: 4.0,
            },
            wall: Some(wall),
            at,
            ..Self::default()
        }
    }

    /// The cut panel leaves at `tip` (wall-local, m/s); see [`Self::tip`].
    pub fn with_tip(mut self, tip: Vec3) -> Self {
        self.tip = tip;
        self
    }

    /// Size of the charge itself, meters.
    pub fn body_size(&self) -> Vec3 {
        match self.kind {
            ChargeKind::Explosive { .. } => Vec3::new(0.32, 0.5, 0.06),
            ChargeKind::Thermite { size, .. } => Vec3::new(size[0] * 0.45, size[1] * 0.35, 0.08),
        }
    }
}

/// Point at `s` (0..1) along a rectangle's edge, clockwise from the top-left corner.
pub fn perimeter_point(min: Vec2, max: Vec2, s: f32) -> Vec2 {
    let w = max.x - min.x;
    let h = max.y - min.y;
    let total = 2.0 * (w + h);
    let mut d = s.clamp(0.0, 1.0) * total;
    if d <= w {
        return Vec2::new(min.x + d, max.y);
    }
    d -= w;
    if d <= h {
        return Vec2::new(max.x, max.y - d);
    }
    d -= h;
    if d <= w {
        return Vec2::new(max.x - d, min.y);
    }
    d -= w;
    Vec2::new(min.x, min.y + d)
}

/// Keep charges on their walls, count down, and set them off.
#[allow(clippy::too_many_arguments)]
pub fn charges_system(
    mut commands: Commands,
    time: Res<Time>,
    mut charges: Query<(Entity, &mut BreachCharge, &mut Transform), Without<DestructibleWall>>,
    walls: Query<
        (
            &Transform,
            &GlobalTransform,
            Option<&ChildOf>,
            &WallState,
            Option<&ReinforcedBy>,
        ),
        With<DestructibleWall>,
    >,
    panels: Query<&Reinforcement>,
    mut queue: ResMut<HitQueue>,
    mut pending: ResMut<PendingDebris>,
    mut impulses: MessageWriter<WorldImpulse>,
    mut rng: Local<Option<Rng>>,
) {
    let dt = time.delta_secs();
    let rng = rng.get_or_insert_with(|| Rng::new(0x07E4_317E));
    for (e, mut charge, mut t) in &mut charges {
        let Some((wt, wgt, parent, state, by)) = charge.wall.and_then(|w| walls.get(w).ok()) else {
            // The wall is gone: so is the charge.
            commands.entity(e).despawn();
            continue;
        };
        let wall_affine = if parent.is_none() {
            GlobalTransform::from(*wt)
        } else {
            *wgt
        };
        let thickness = state.grid.thickness;
        let side = if charge.back { -1.0 } else { 1.0 };
        let depth = charge.body_size().z * 0.5;
        let local = Vec3::new(charge.at.x, charge.at.y, side * (thickness * 0.5 + depth));
        let flip = if charge.back {
            Quat::from_rotation_y(std::f32::consts::PI)
        } else {
            Quat::IDENTITY
        };
        t.translation = wall_affine.transform_point(local.as_dvec3());
        t.rotation = wall_affine.rotation() * flip;
        let normal = wall_affine.rotation() * Vec3::Z * side;
        if !charge.armed {
            continue;
        }
        charge.timer += dt;
        if charge.timer < charge.fuse_secs {
            continue;
        }
        match charge.kind {
            ChargeKind::Explosive { energy_j } => {
                let scale = (energy_j / 1.0e5).cbrt();
                impulses.write(WorldImpulse {
                    center: t.translation + (normal * 0.04).as_dvec3(),
                    radius: (3.0 * scale).clamp(1.0, 30.0),
                    strength: (30.0 * scale).clamp(5.0, 200.0),
                    energy_j,
                    heat_c: 500.0,
                    source: Some(e),
                });
                commands.entity(e).despawn();
            }
            ChargeKind::Thermite { size, burn_secs } => {
                // It burns through the wall and any reinforcement panels set behind it.
                let wall = charge.wall.unwrap_or(e);
                let front = !charge.back;
                let behind: Vec<(Entity, f32)> = match by {
                    Some(by) if front => {
                        by.0.iter()
                            .filter_map(|&p| panels.get(p).ok().map(|r| (p, r.offset)))
                            .collect()
                    }
                    _ => Vec::new(),
                };
                let tip = charge.tip;
                let mut cut = |min: Vec2, max: Vec2, melt: bool| {
                    queue.hits.push(Hit::Cut {
                        wall,
                        min,
                        max,
                        front,
                        melt,
                        tip,
                    });
                    for &(panel, offset) in &behind {
                        let (min, max) = to_panel(min, max, offset);
                        queue.hits.push(Hit::Cut {
                            wall: panel,
                            min,
                            max,
                            front: true,
                            melt,
                            tip,
                        });
                    }
                };
                let half = Vec2::from_array(size) * 0.5;
                let (min, max) = (charge.at - half, charge.at + half);
                let before = charge.progress;
                charge.progress = (charge.progress + dt / burn_secs.max(0.1)).min(1.0);
                // Melt the edge between the last point and this one.
                let width = 0.1f32.max(state.grid.cell.max_element() * 1.2);
                let steps = 3;
                for k in 0..steps {
                    let s = before + (charge.progress - before) * (k + 1) as f32 / steps as f32;
                    let p = perimeter_point(min, max, s);
                    cut(
                        p - Vec2::splat(width * 0.5),
                        p + Vec2::splat(width * 0.5),
                        true,
                    );
                }
                // Sparks spray off the burning point.
                let p = perimeter_point(min, max, charge.progress);
                let at = wall_affine
                    .transform_point(Vec3::new(p.x, p.y, side * thickness * 0.5).as_dvec3());
                for _ in 0..3 {
                    let v = normal * rng.range(1.0, 4.0) + rng.unit_vector() * 2.5 + Vec3::Y * 1.5;
                    pending.items.push(spark(at, v, rng));
                }
                if charge.progress >= 1.0 {
                    // Finish: the panel inside drops out.
                    cut(
                        min + Vec2::splat(width * 0.4),
                        max - Vec2::splat(width * 0.4),
                        false,
                    );
                    commands.entity(e).despawn();
                }
            }
        }
    }
}

fn spark(at: DVec3, velocity: Vec3, rng: &mut Rng) -> DebrisSpawn {
    DebrisSpawn {
        position: at,
        rotation: Quat::IDENTITY,
        velocity,
        angular: Vec3::ZERO,
        look: DebrisLook::Spark {
            size: rng.range(0.012, 0.03),
        },
        half_extents: Vec3::splat(0.01),
        hull: Vec::new(),
        mass_kg: 0.001,
        material: DestructibleMaterial::Metal,
        shatter_speed: 0.0,
        lifetime: Some(rng.range(0.4, 1.1)),
        reuse: None,
        scale: None,
        resting: false,
        from_rock: false,
    }
}

/// Marks the drawn parts of a charge.
#[derive(Component, Debug, Clone, Copy)]
pub struct ChargeVisual;

/// Give new charges their look: a dark box, plus a glowing strip for thermite while it burns.
pub fn charge_visuals_system(
    mut commands: Commands,
    assets: Option<Res<DestructionAssets>>,
    charges: Query<(Entity, &BreachCharge), Without<ChargeVisual>>,
    mut lit: Query<(&ChildOf, &mut MeshRenderer), With<ChargeVisual>>,
    all: Query<&BreachCharge>,
    time: Res<Time>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (e, charge) in &charges {
        let size = charge.body_size();
        commands.entity(e).insert(ChargeVisual).with_children(|c| {
            c.spawn((
                Transform::default(),
                MeshRenderer::new(
                    MeshRef::Shape(Shape::Cuboid {
                        size: size.to_array(),
                    }),
                    MaterialRef::Handle(assets.charge_body),
                ),
            ));
            // A status light / thermite strip that glows.
            let strip = match charge.kind {
                ChargeKind::Explosive { .. } => Vec3::new(0.05, 0.05, 0.02),
                ChargeKind::Thermite { .. } => Vec3::new(size.x * 0.9, 0.05, 0.02),
            };
            c.spawn((
                Transform::from_translation(DVec3::new(
                    0.0,
                    (size.y * 0.3) as f64,
                    (size.z * 0.5) as f64,
                )),
                MeshRenderer::new(
                    MeshRef::Shape(Shape::Cuboid {
                        size: strip.to_array(),
                    }),
                    MaterialRef::Handle(assets.thermite_glow),
                )
                .without_shadows(),
                ChargeVisual,
            ));
        });
    }
    // Blink the light while the fuse runs.
    let blink = (time.elapsed_secs() * 4.0).fract() < 0.5;
    for (parent, mut r) in &mut lit {
        if let Ok(charge) = all.get(parent.parent()) {
            let burning = matches!(charge.kind, ChargeKind::Thermite { .. })
                && charge.timer >= charge.fuse_secs;
            let on = burning || (charge.armed && blink);
            if r.visible != on {
                r.visible = on;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perimeter_walks_the_whole_edge() {
        let (min, max) = (Vec2::new(-1.0, 0.0), Vec2::new(1.0, 1.0));
        assert_eq!(perimeter_point(min, max, 0.0), Vec2::new(-1.0, 1.0));
        assert_eq!(perimeter_point(min, max, 1.0 / 3.0), Vec2::new(1.0, 1.0));
        assert_eq!(perimeter_point(min, max, 0.5), Vec2::new(1.0, 0.0));
        let end = perimeter_point(min, max, 1.0);
        assert!(end.distance(Vec2::new(-1.0, 1.0)) < 1e-5);
    }
}

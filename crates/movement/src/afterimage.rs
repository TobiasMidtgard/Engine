//! Afterimages: glowing, fading copies of a character left behind while it dashes, wall-runs,
//! slides or swings. They are spaced in **real** time, so they show motion the way the
//! player saw it: a blink-fast dash leaves a few far-apart ghosts, the same dash seen in
//! slow motion leaves a dense trail. That also makes still screenshots show movement.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_render::{AlphaMode, MeshRenderer, Shape, StandardMaterial};
use engine_world::LocomotionMode;
use engine_world::physics::CharacterController;

use crate::combat::{AttackPhase, CombatState};
use crate::mover::Mover;

/// Leave afterimages behind this character.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Afterimages {
    pub enabled: bool,
    /// Linear RGB of the glow.
    pub color: [f32; 3],
    /// Emission strength of a fresh afterimage (cd/m²-like).
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

/// One afterimage (spawned by the movement module, despawned when faded).
#[derive(Component, Debug, Clone, Copy)]
pub struct Afterimage {
    /// Real seconds since it appeared.
    pub age: f32,
    pub lifetime: f32,
    level: u8,
    style: Afterimages,
}

/// Fade steps: each is one shared material, so hundreds of afterimages cost a few materials.
const LEVELS: u8 = 5;

fn material(style: &Afterimages, level: u8) -> StandardMaterial {
    // Quantized so identical styles share GPU materials.
    let k = 1.0 - level as f32 / LEVELS as f32;
    let q = |x: f32| (x * 64.0).round() / 64.0;
    let mut m = StandardMaterial::color(q(style.color[0]), q(style.color[1]), q(style.color[2]))
        .with_emissive(
            [q(style.color[0]), q(style.color[1]), q(style.color[2])],
            (style.glow * k).round(),
        )
        .with_roughness(0.4);
    m.base_color[3] = q(style.alpha * k);
    m.alpha_mode = AlphaMode::Blend;
    m
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

/// Spawn afterimages (Update: presentation, after fixed steps).
#[allow(clippy::type_complexity)]
pub(crate) fn spawn_afterimages(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(
        &GlobalTransform,
        &Mover,
        &CharacterController,
        &mut Afterimages,
        Option<&CombatState>,
        Option<&engine_world::locomotion::LocomotionState>,
    )>,
) {
    let real = time.real_delta_secs();
    if real <= 0.0 {
        return;
    }
    for (gt, m, cc, mut style, cs, loco) in &mut q {
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
        let rotation = gt.rotation() * m.body_tilt();
        let height = cc.height;
        commands.spawn((
            Transform::from_translation(gt.translation())
                .with_rotation(rotation)
                .with_scale(gt.scale()),
            MeshRenderer::new(
                Shape::Capsule {
                    radius: cc.radius * 0.85,
                    height: height * 0.95,
                    segments: 12,
                },
                material(&style, 0),
            )
            .without_shadows(),
            Afterimage {
                age: 0.0,
                lifetime: style.lifetime_secs.max(0.05),
                level: 0,
                style: *style,
            },
        ));
    }
}

/// Fade and remove afterimages.
pub(crate) fn fade_afterimages(
    mut commands: Commands,
    time: Res<Time>,
    mut diagnostics: Option<ResMut<Diagnostics>>,
    mut q: Query<(Entity, &mut Afterimage, &mut MeshRenderer)>,
) {
    let real = time.real_delta_secs();
    let mut count = 0;
    for (e, mut a, mut r) in &mut q {
        a.age += real;
        if a.age >= a.lifetime {
            commands.entity(e).despawn();
            continue;
        }
        count += 1;
        let level = ((a.age / a.lifetime) * LEVELS as f32) as u8;
        if level != a.level {
            a.level = level;
            r.material = material(&a.style, level).into();
        }
    }
    if let Some(d) = diagnostics.as_deref_mut() {
        d.set("movement.afterimages", count as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_levels_share_materials() {
        let s = Afterimages::default();
        assert_eq!(material(&s, 2), material(&s, 2));
        assert!(material(&s, 0).base_color[3] > material(&s, 3).base_color[3]);
        assert_eq!(material(&s, 0).alpha_mode, AlphaMode::Blend);
    }
}

//! Coloured rings on the ground for each sound as it plays.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};

use super::bodies::gait::GROUND;
use crate::AudioCue;

/// Coloured rings on the ground for the sounds the audio module plays: on and around footstep
/// trails, or everywhere.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundRings {
    pub enabled: bool,
    /// Rings for every sound in the world, not just near trails.
    pub everywhere: bool,
    /// Trail origins (set by [`spawn_demo`](super::spawn_demo)); rings show within 40 m of one.
    pub near: Vec<DVec3>,
}

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct SoundRing {
    age: f32,
    life: f32,
    size: f32,
    color: usize,
    level: usize,
}

const RING_LEVELS: usize = 4;

pub(super) fn ring_color(name: &str) -> [f32; 3] {
    let key = name.rsplit('.').next().unwrap_or(name);
    match key {
        "grass" => [0.35, 1.0, 0.3],
        "mud" => [0.85, 0.5, 0.2],
        "snow" => [0.85, 0.92, 1.0],
        "ice" => [0.3, 0.9, 1.0],
        "gravel" => [0.75, 0.72, 0.68],
        "wood" => [1.0, 0.62, 0.2],
        "metal" => [0.55, 0.65, 1.0],
        "concrete" => [0.9, 0.9, 0.85],
        "sand" => [1.0, 0.85, 0.4],
        "leaves" => [1.0, 0.4, 0.1],
        "water" => [0.2, 0.55, 1.0],
        "explosion" => [1.0, 0.25, 0.1],
        _ => [1.0, 1.0, 1.0],
    }
}

const RING_NAMES: &[&str] = &[
    "grass",
    "mud",
    "snow",
    "ice",
    "gravel",
    "wood",
    "metal",
    "concrete",
    "sand",
    "leaves",
    "water",
    "explosion",
    "other",
];

fn ring_material(color: usize, level: usize) -> StandardMaterial {
    let c = ring_color(RING_NAMES[color.min(RING_NAMES.len() - 1)]);
    let k = (RING_LEVELS - level) as f32 / RING_LEVELS as f32;
    StandardMaterial::color(c[0] * 0.2, c[1] * 0.2, c[2] * 0.2).with_emissive(c, 6.0 * k * k)
}

fn ring_shape() -> Shape {
    Shape::Torus {
        radius: 1.0,
        tube: 0.03,
        segments: 48,
    }
}

pub(super) fn spawn_rings(
    mut commands: Commands,
    rings: Res<SoundRings>,
    mut cues: MessageReader<AudioCue>,
) {
    if !rings.enabled {
        cues.clear();
        return;
    }
    for cue in cues.read() {
        let Some(p) = cue.position else { continue };
        if !rings.everywhere && !rings.near.iter().any(|o| o.distance(p) < 40.0) {
            continue;
        }
        let key = cue.name.rsplit('.').next().unwrap_or(&cue.name);
        let color = RING_NAMES
            .iter()
            .position(|n| *n == key)
            .unwrap_or(RING_NAMES.len() - 1);
        let size = 0.35 + 1.6 * cue.loudness.clamp(0.0, 1.5);
        commands.spawn((
            Name::new("Sound ring"),
            SoundRing {
                age: 0.0,
                life: 0.45 + 0.4 * cue.loudness.min(1.0),
                size,
                color,
                level: 0,
            },
            Transform::from_translation(DVec3::new(p.x, ring_height(&rings.near, p) + 0.03, p.z))
                .with_uniform_scale(0.05),
            MeshRenderer::new(ring_shape(), ring_material(color, 0)),
        ));
    }
}

/// How high a ring sits: on a trail's surface where the sound lands on the path, else where
/// the sound is. Bodies from other modules put their footsteps on the ground under the path,
/// which would hide the ring beneath it.
fn ring_height(trails: &[DVec3], p: DVec3) -> f64 {
    trails
        .iter()
        .filter(|o| super::surface_under(p - **o).is_some())
        .map(|o| o.y + GROUND)
        .filter(|top| p.y > top - 0.5)
        .fold(p.y, f64::max)
}

pub(super) fn grow_rings(
    mut commands: Commands,
    time: Res<Time>,
    mut rings: Query<(Entity, &mut SoundRing, &mut Transform, &mut MeshRenderer)>,
) {
    let dt = time.delta_secs();
    for (e, mut r, mut t, mut mesh) in &mut rings {
        r.age += dt;
        let k = r.age / r.life;
        if k >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        let grow = 1.0 - (1.0 - k).powi(3);
        let s = r.size * (0.08 + 0.92 * grow);
        t.scale = Vec3::new(s, 1.0, s);
        let level = ((k * RING_LEVELS as f32) as usize).min(RING_LEVELS - 1);
        if level != r.level {
            r.level = level;
            mesh.material = ring_material(r.color, level).into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_sit_on_the_path_not_under_it() {
        let origin = DVec3::new(3.0, 1.0, 150.0);
        let (on_path, _) = super::super::trail_point(5.0);
        // A footstep on the ground a few centimetres under the path comes up onto it.
        let p = origin + on_path + DVec3::new(0.0, 0.01, 0.0);
        assert_eq!(ring_height(&[origin], p), origin.y + GROUND);
        // Above it, or off the path, a ring stays where the sound is.
        let above = p + DVec3::new(0.0, 1.0, 0.0);
        assert_eq!(ring_height(&[origin], above), above.y);
        let off = origin + DVec3::new(0.0, 0.0, 40.0);
        assert_eq!(ring_height(&[origin], off), off.y);
        // Well below the path (under a bridge) it stays too.
        let below = p - DVec3::new(0.0, 2.0, 0.0);
        assert_eq!(ring_height(&[origin], below), below.y);
    }
}

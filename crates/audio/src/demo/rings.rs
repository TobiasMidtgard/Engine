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
    style: RingStyle,
    loudness: f32,
    color: usize,
    /// The opacity step it shows (see [`FADE_STEPS`]).
    step: usize,
}

/// How a ring shows a sound: how wide it spreads (radius, meters), how long it lasts
/// (seconds) and how opaque it is at its strongest (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RingStyle {
    pub size: f32,
    pub life: f32,
    pub peak: f32,
    /// 0 for a quiet sound .. 1 for a loud one.
    pub loud: f32,
}

/// The ring for a sound of `loudness` (the cue's, about 0.17 for a dog's paw, 0.4 for a
/// hiker's step, 0.66 for the runner's and 0.8 for a horse's hoof). Quiet sounds make small,
/// brief, faint rings round the walker's feet; loud ones spread wide, last and stay clearly
/// visible.
pub(super) fn ring_style(loudness: f32) -> RingStyle {
    let l = loudness.clamp(0.0, 1.5);
    let loud = smooth(0.1, 0.75, l);
    RingStyle {
        size: 0.2 + 1.7 * l,
        life: 0.3 + 0.55 * l.min(1.0),
        peak: 0.6 + 0.35 * loud,
        loud,
    }
}

/// Opacity of a ring `k` (0..1) of the way through its life, as a share of its peak: in at
/// once, then thinning to nothing as it spreads, so a ring never ends as a hard hoop. A loud
/// sound's ring (`loud` 1) holds its strength for the first half of its life; a quiet one's
/// starts thinning straight away.
fn fade(k: f32, loud: f32) -> f32 {
    let hold = -0.25 + 0.7 * loud.clamp(0.0, 1.0);
    smooth(0.0, 0.06, k) * (1.0 - smooth(hold, 1.0, k))
}

/// Opacity steps a ring fades through: rings share one material per colour and step.
const FADE_STEPS: usize = 16;

fn fade_step(style: &RingStyle, k: f32) -> usize {
    (style.peak * fade(k, style.loud) * FADE_STEPS as f32).round() as usize
}

/// A step that lands inside a ring spreading from a step this many seconds ago joins it,
/// unless it is clearly louder: a trotting dog's quick patter of four paws would otherwise
/// stack ring on ring round it.
const JOIN_SECS: f32 = 0.2;
/// ... within this share of that ring's widest radius.
const JOIN_REACH: f32 = 1.0;

/// Whether a sound of `loudness` at `p` joins one of `rings` (centre, age, loudness, style).
fn joins(p: DVec3, loudness: f32, rings: &[(DVec3, f32, f32, RingStyle)]) -> bool {
    rings.iter().any(|(c, age, l, style)| {
        *age < JOIN_SECS
            && loudness <= l * 1.25
            && (p - *c).length() < (style.size * JOIN_REACH) as f64
            && (p.y - c.y).abs() < 0.5
    })
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

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

/// A ring's material at opacity `step` of [`FADE_STEPS`]: a pale tint of its ground's colour
/// (so an orange ring still stands out on orange leaves), translucent, with a soft glow of its
/// own that fades with it, so it reads in shade too.
fn ring_material(color: usize, step: usize) -> StandardMaterial {
    let c = ring_color(RING_NAMES[color.min(RING_NAMES.len() - 1)]);
    let a = step.min(FADE_STEPS) as f32 / FADE_STEPS as f32;
    let pale = |v: f32| 0.5 + 0.4 * v;
    let mut m = StandardMaterial::color(pale(c[0]), pale(c[1]), pale(c[2]))
        .with_roughness(0.6)
        .with_emissive(c, 900.0 * a)
        .with_alpha(a);
    m.receives_wetness = false;
    m
}

/// A thin band, about 2 cm across on the ground whatever the ring's size: rings near the
/// camera (a film riding behind a walker) would otherwise read as a hoop lying on the path,
/// and a small ring's band would shrink to a hairline.
const BAND_M: f32 = 0.019;

/// The ring's torus (radius 1, scaled to `size`): its tube, in a few steps so rings share
/// meshes, is the band's width at that size.
fn ring_tube(size: f32) -> f32 {
    let tube = (BAND_M / size.max(0.01)).clamp(0.012, 0.04);
    (tube * 500.0).round() / 500.0
}

fn ring_shape(tube: f32) -> Shape {
    Shape::Torus {
        radius: 1.0,
        tube,
        segments: 96,
    }
}

pub(super) fn spawn_rings(
    mut commands: Commands,
    rings: Res<SoundRings>,
    mut cues: MessageReader<AudioCue>,
    live: Query<(&SoundRing, &Transform)>,
) {
    if !rings.enabled {
        cues.clear();
        return;
    }
    let mut spreading: Vec<(DVec3, f32, f32, RingStyle)> = live
        .iter()
        .filter(|(r, _)| r.age < JOIN_SECS)
        .map(|(r, t)| (t.translation, r.age, r.loudness, r.style))
        .collect();
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
        let at = DVec3::new(p.x, ring_height(&rings.near, p) + 0.03, p.z);
        if joins(at, cue.loudness, &spreading) {
            continue;
        }
        let style = ring_style(cue.loudness);
        spreading.push((at, 0.0, cue.loudness, style));
        commands.spawn((
            Name::new("Sound ring"),
            SoundRing {
                age: 0.0,
                style,
                loudness: cue.loudness,
                color,
                step: 0,
            },
            Transform::from_translation(at).with_uniform_scale(0.05),
            MeshRenderer::new(ring_shape(ring_tube(style.size)), ring_material(color, 0))
                .without_shadows(),
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
        let k = r.age / r.style.life;
        let step = fade_step(&r.style, k);
        if k >= 1.0 || (step == 0 && k > 0.5) {
            commands.entity(e).despawn();
            continue;
        }
        let grow = 1.0 - (1.0 - k).powi(3);
        let s = r.style.size * (0.08 + 0.92 * grow);
        // The band stays about 2 cm high, flattened from its tube.
        t.scale = Vec3::new(s, 0.8 * 0.012 / ring_tube(r.style.size), s);
        if step != r.step {
            r.step = step;
            mesh.material = ring_material(r.color, step).into();
            mesh.visible = step > 0;
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

    #[test]
    fn quiet_steps_ring_small_and_brief_loud_ones_wide_and_clear() {
        // Cue loudness of a dog's paw, a hiker's step, the runner's and a horse's hoof.
        let [dog, hiker, runner, horse] = [0.17, 0.4, 0.66, 0.79].map(ring_style);
        for (a, b) in [(dog, hiker), (hiker, runner), (runner, horse)] {
            assert!(
                a.size < b.size && a.life < b.life && a.peak <= b.peak,
                "{a:?} {b:?}"
            );
        }
        // Round a 0.56 m dog: a ring under half a meter, gone in about a third of a second
        // and lighter; the horse's spreads past a meter and a half, nearly opaque.
        assert!(
            dog.size < 0.5 && dog.life < 0.4 && dog.peak < 0.65,
            "{dog:?}"
        );
        assert!(horse.size > 1.4 && horse.peak > 0.8, "{horse:?}");
        assert!(runner.peak > 0.75, "{runner:?}");
    }

    #[test]
    fn rings_fade_out_to_nothing_as_they_spread() {
        for l in [0.17, 0.4, 0.79, 1.5] {
            let style = ring_style(l);
            let steps: Vec<usize> = (0..=40)
                .map(|i| fade_step(&style, i as f32 / 40.0))
                .collect();
            let top = steps
                .iter()
                .position(|s| s == steps.iter().max().unwrap())
                .unwrap();
            // Up at once, then never brighter again, and nothing left at the end.
            assert!(top <= 4, "{l}: {steps:?}");
            assert!(
                steps[top..].windows(2).all(|w| w[1] <= w[0]),
                "{l}: {steps:?}"
            );
            assert_eq!(*steps.last().unwrap(), 0, "{l}: {steps:?}");
            assert!(steps[36] * 4 <= steps[top], "{l}: {steps:?}");
            assert_eq!(ring_material(1, steps[40]).base_color[3], 0.0);
        }
        // Halfway through its life a horse's ring still shows at three quarters of its
        // strength or more; a dog's has thinned to under half.
        let (dog, horse) = (ring_style(0.17), ring_style(0.79));
        let peak = |s: &RingStyle| (0..10).map(|i| fade_step(s, i as f32 / 100.0)).max();
        assert!(
            fade_step(&horse, 0.5) * 4 >= peak(&horse).unwrap() * 3,
            "{horse:?}"
        );
        assert!(fade_step(&dog, 0.5) * 2 < peak(&dog).unwrap(), "{dog:?}");
    }

    #[test]
    fn a_quick_patter_shares_a_ring() {
        let paw = 0.17;
        let style = ring_style(paw);
        let first = DVec3::new(0.0, 1.0, 0.0);
        let ring = [(first, 0.0, paw, style)];
        // The next paw, a few tenths of a meter on and a moment later, joins that ring.
        let next = first + DVec3::new(0.3, 0.0, 0.0);
        assert!(joins(next, paw, &ring));
        // Once it has spread a while, or farther off, or for a much louder sound (a horse's
        // hoof beside the dog), a new ring starts.
        assert!(!joins(next, paw, &[(first, JOIN_SECS + 0.01, paw, style)]));
        assert!(!joins(first + DVec3::new(1.5, 0.0, 0.0), paw, &ring));
        assert!(!joins(next, 0.79, &ring));
        // A horse's hooves, a quarter second apart, each get their own.
        let hoof = ring_style(0.79);
        assert!(!joins(next, 0.79, &[(first, 0.26, 0.79, hoof)]));
    }

    #[test]
    fn small_and_large_rings_have_the_same_band() {
        // A dog's ring and a horse's lie on the ground as bands of about the same width.
        for l in [0.17, 0.4, 0.79] {
            let size = ring_style(l).size;
            let band = ring_tube(size) * size;
            assert!((band - BAND_M).abs() < BAND_M * 0.3, "{l}: {band}");
        }
    }
}

//! Ribbons: trails behind moving things and crackling lightning beams.
//!
//! A [`Trail`] records where its entity has been and draws a camera-facing ribbon through those
//! points that fades over `lifetime`. A [`Beam`] draws a jagged, flickering bolt from its entity
//! to a target point (lightning spells, tesla coils, arcs between electrodes). Both are built on
//! the CPU each frame (a few thousand vertices) and drawn by the particle render feature.
//!
//! When an entity with a trail is despawned (a spell bolt hits), its trail stays and fades out
//! instead of vanishing.

use std::collections::{HashMap, VecDeque};

use engine_core::math::{DVec3, Rng, Vec3};
use engine_core::prelude::*;

use crate::effect::{Curve, Gradient};
use crate::gpu::RibbonVertex;

/// What a ribbon looks like across its width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RibbonStyle {
    /// Soft edges (sword swings, wakes).
    Smooth,
    /// A bright core with flowing noise (magic trails).
    #[default]
    Energy,
    /// A white-hot thin core with a wide glow (lightning).
    Lightning,
    /// Wispy and noisy (smoke trails behind rockets).
    Smoke,
}

impl RibbonStyle {
    /// GPU code.
    pub fn code(self) -> f32 {
        self as u32 as f32
    }
}

/// A ribbon trail behind a moving entity.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trail {
    /// Width at the head, meters.
    pub width: f32,
    /// Seconds each point of the trail lasts.
    pub lifetime: f32,
    /// Meters the entity moves before a new point is added.
    pub min_spacing: f32,
    /// Color from head (0) to tail (1): linear RGB and coverage.
    pub color: Gradient,
    /// Width multiplier from head (0) to tail (1).
    pub width_over_life: Curve,
    /// Glow: luminance for color 1.0, cd/m² (0 = lit like smoke by nothing; use for glowing
    /// trails).
    pub emissive: f32,
    /// 0 alpha blended .. 1 additive.
    pub additive: f32,
    pub style: RibbonStyle,
    /// Stop adding points (the existing trail still fades).
    pub paused: bool,
    /// Most points kept.
    pub max_points: u32,
}

impl Default for Trail {
    fn default() -> Self {
        Self {
            width: 0.3,
            lifetime: 0.6,
            min_spacing: 0.15,
            color: Gradient::new(&[(0.0, [1.0, 1.0, 1.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 0.0])]),
            width_over_life: Curve::new(&[(0.0, 1.0), (1.0, 0.2)]),
            emissive: 20_000.0,
            additive: 1.0,
            style: RibbonStyle::Energy,
            paused: false,
            max_points: 256,
        }
    }
}

/// A flickering bolt from this entity to `target`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beam {
    /// World position the beam reaches, meters.
    pub target: DVec3,
    /// Width of the glow, meters.
    pub width: f32,
    /// Linear RGB.
    pub color: [f32; 3],
    /// Luminance of the core, cd/m².
    pub emissive: f32,
    /// Sideways jitter as a fraction of the beam's length (0 = straight).
    pub jaggedness: f32,
    /// New bolt shapes per second.
    pub flicker_hz: f32,
    /// Side branches that fork off the main bolt.
    pub branches: u32,
    pub style: RibbonStyle,
    /// Hide the beam without removing it.
    pub visible: bool,
}

impl Default for Beam {
    fn default() -> Self {
        Self {
            target: DVec3::ZERO,
            width: 0.25,
            color: [0.6, 0.75, 1.0],
            emissive: 80_000.0,
            jaggedness: 0.12,
            flicker_hz: 14.0,
            branches: 3,
            style: RibbonStyle::Lightning,
            visible: true,
        }
    }
}

/// One point of a ribbon, ready to turn into vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RibbonPoint {
    pub pos: DVec3,
    /// Full width, meters.
    pub width: f32,
    /// Linear RGB radiance (cd/m²) and coverage.
    pub color: [f32; 4],
    /// 0 head .. 1 tail.
    pub along: f32,
}

#[derive(Debug, Clone)]
struct TrailPoint {
    pos: DVec3,
    age: f32,
}

#[derive(Debug, Clone)]
struct TrailRun {
    trail: Trail,
    points: VecDeque<TrailPoint>,
    /// Last known time scale of the entity (orphans keep it).
    local: Option<LocalTimeScale>,
}

#[derive(Debug, Clone, Default)]
struct BeamRun {
    paths: Vec<Vec<DVec3>>,
    timer: f32,
    seed: u64,
}

/// Ribbon state for every trail and beam (managed by the plugin).
#[derive(Resource, Default)]
pub struct Ribbons {
    trails: HashMap<Entity, TrailRun>,
    orphans: Vec<TrailRun>,
    beams: HashMap<Entity, (Beam, BeamRun)>,
    next_seed: u64,
}

impl Ribbons {
    /// Live trails (including fading orphans) and beams.
    pub fn counts(&self) -> (usize, usize) {
        (self.trails.len() + self.orphans.len(), self.beams.len())
    }

    /// Move every trail and beam point into a new reference frame.
    pub fn shift(&mut self, s: &engine_world::WorldShift) {
        for run in self.trails.values_mut().chain(self.orphans.iter_mut()) {
            for p in &mut run.points {
                p.pos = s.point(p.pos);
            }
        }
        for (beam, run) in self.beams.values_mut() {
            beam.target = s.point(beam.target);
            for p in run.paths.iter_mut().flatten() {
                *p = s.point(*p);
            }
        }
    }

    /// Every ribbon as a list of points, for drawing.
    pub fn for_each_ribbon(&self, mut f: impl FnMut(&[RibbonPoint], RibbonStyle, f32)) {
        let mut points = Vec::new();
        for run in self.trails.values().chain(self.orphans.iter()) {
            if run.points.len() < 2 {
                continue;
            }
            points.clear();
            let t = &run.trail;
            let life = t.lifetime.max(1e-3);
            for p in &run.points {
                let a = (p.age / life).clamp(0.0, 1.0);
                let c = t.color.sample(a);
                let k = if t.emissive > 0.0 { t.emissive } else { 1.0 };
                points.push(RibbonPoint {
                    pos: p.pos,
                    width: t.width * t.width_over_life.sample(a),
                    color: [c[0] * k, c[1] * k, c[2] * k, c[3]],
                    along: a,
                });
            }
            f(&points, t.style, t.additive);
        }
        for (beam, run) in self.beams.values() {
            if !beam.visible {
                continue;
            }
            for (i, path) in run.paths.iter().enumerate() {
                if path.len() < 2 {
                    continue;
                }
                points.clear();
                // Branches are thinner and dimmer.
                let (w, k) = if i == 0 { (1.0, 1.0) } else { (0.55, 0.6) };
                let n = path.len() - 1;
                for (j, p) in path.iter().enumerate() {
                    let along = j as f32 / n as f32;
                    // Branches taper to nothing.
                    let taper = if i == 0 { 1.0 } else { 1.0 - along };
                    let e = beam.emissive * k;
                    points.push(RibbonPoint {
                        pos: *p,
                        width: beam.width * w * taper.max(0.05),
                        color: [beam.color[0] * e, beam.color[1] * e, beam.color[2] * e, 1.0],
                        along,
                    });
                }
                f(&points, beam.style, 1.0);
            }
        }
    }
}

/// Record trail points and age them; orphan trails of despawned entities.
pub fn update_trails(
    time: Res<Time>,
    mut ribbons: ResMut<Ribbons>,
    q: Query<(Entity, &Trail, &GlobalTransform, Option<&LocalTimeScale>)>,
) {
    let ribbons = &mut *ribbons;
    let mut seen = Vec::new();
    for (entity, trail, gt, local) in &q {
        seen.push(entity);
        let pos = gt.translation();
        let run = ribbons.trails.entry(entity).or_insert_with(|| TrailRun {
            trail: trail.clone(),
            points: VecDeque::new(),
            local: None,
        });
        if run.trail != *trail {
            run.trail = trail.clone();
        }
        run.local = local.copied();
        let dt = time.delta_for(local);
        age_points(&mut run.points, dt, run.trail.lifetime);
        if !trail.paused {
            push_point(run, pos);
        }
    }
    // Trails whose entity is gone (or lost its Trail) fade out on their own.
    let gone: Vec<Entity> = ribbons
        .trails
        .keys()
        .filter(|e| !seen.contains(e))
        .copied()
        .collect();
    for e in gone {
        if let Some(run) = ribbons.trails.remove(&e) {
            ribbons.orphans.push(run);
        }
    }
    for run in &mut ribbons.orphans {
        let dt = time.delta_for(run.local.as_ref());
        age_points(&mut run.points, dt, run.trail.lifetime);
    }
    ribbons.orphans.retain(|r| !r.points.is_empty());
}

fn age_points(points: &mut VecDeque<TrailPoint>, dt: f32, lifetime: f32) {
    for p in points.iter_mut() {
        p.age += dt;
    }
    while points.back().is_some_and(|p| p.age >= lifetime) {
        points.pop_back();
    }
}

fn push_point(run: &mut TrailRun, pos: DVec3) {
    let spacing = run.trail.min_spacing.max(0.0) as f64;
    // The head always follows the entity; a new point is left behind once it has moved
    // `min_spacing` from the previous one.
    if run.points.len() >= 2 && pos.distance(run.points[1].pos) < spacing {
        let head = &mut run.points[0];
        head.pos = pos;
        head.age = 0.0;
    } else {
        run.points.push_front(TrailPoint { pos, age: 0.0 });
    }
    let max = run.trail.max_points.max(2) as usize;
    run.points.truncate(max);
}

/// Re-shape beams at their flicker rate.
pub fn update_beams(
    time: Res<Time>,
    mut ribbons: ResMut<Ribbons>,
    q: Query<(Entity, &Beam, &GlobalTransform, Option<&LocalTimeScale>)>,
) {
    let ribbons = &mut *ribbons;
    let mut seen = Vec::new();
    for (entity, beam, gt, local) in &q {
        seen.push(entity);
        let from = gt.translation();
        let entry = ribbons.beams.entry(entity).or_insert_with(|| {
            ribbons.next_seed = ribbons.next_seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            (
                beam.clone(),
                BeamRun {
                    seed: ribbons.next_seed ^ entity.to_bits(),
                    ..Default::default()
                },
            )
        });
        let (stored, run) = entry;
        let moved = stored.target != beam.target;
        *stored = beam.clone();
        run.timer -= time.delta_for(local);
        if run.timer <= 0.0 || moved || run.paths.is_empty() {
            run.timer = 1.0 / beam.flicker_hz.max(0.5);
            run.seed = run.seed.wrapping_add(1);
            let mut rng = Rng::new(run.seed);
            run.paths.clear();
            lightning_paths(from, beam.target, beam, &mut rng, &mut run.paths);
        } else if let Some(main) = run.paths.first() {
            // Keep the ends attached while the shape holds.
            if main.first().is_some_and(|p| p.distance(from) > 1e-3) {
                let delta = from - main[0];
                let n = main.len().max(2) - 1;
                for path in run.paths.iter_mut() {
                    for (j, p) in path.iter_mut().enumerate() {
                        let k = 1.0 - (j as f64 / n as f64).min(1.0);
                        *p += delta * k;
                    }
                }
            }
        }
    }
    ribbons.beams.retain(|e, _| seen.contains(e));
}

/// A jagged bolt from `from` to `to` by midpoint displacement, plus branches.
pub fn lightning_paths(
    from: DVec3,
    to: DVec3,
    beam: &Beam,
    rng: &mut Rng,
    out: &mut Vec<Vec<DVec3>>,
) {
    let length = from.distance(to);
    if length < 1e-3 {
        return;
    }
    let mut main = vec![from, to];
    let mut offset = length * beam.jaggedness.max(0.0) as f64;
    let dir = (to - from) / length;
    let levels = ((length / 0.4).log2().ceil() as i32).clamp(1, 7);
    for _ in 0..levels {
        let mut next = Vec::with_capacity(main.len() * 2);
        for pair in main.windows(2) {
            next.push(pair[0]);
            let mid = (pair[0] + pair[1]) * 0.5;
            let side = random_perpendicular(dir, rng);
            next.push(mid + side * offset * (rng.f32() as f64 - 0.5) * 2.0);
        }
        next.push(*main.last().unwrap());
        main = next;
        offset *= 0.5;
    }
    let n = main.len();
    out.push(main);
    for _ in 0..beam.branches {
        let main = &out[0];
        let start = 1 + (rng.f32() * (n as f32 * 0.7)) as usize;
        let start = start.min(n - 2);
        let origin = main[start];
        let along = main[(start + 1).min(n - 1)] - origin;
        let side = random_perpendicular(dir, rng);
        let branch_dir = (along.normalize_or_zero() + side * 0.9).normalize_or_zero();
        let branch_len = length * (0.12 + 0.2 * rng.f32() as f64);
        let mut path = vec![origin];
        let steps = 6;
        let mut p = origin;
        for _ in 0..steps {
            let jitter = random_perpendicular(branch_dir, rng)
                * branch_len
                * 0.15
                * (rng.f32() as f64 - 0.5);
            p += branch_dir * (branch_len / steps as f64) + jitter;
            path.push(p);
        }
        out.push(path);
    }
}

fn random_perpendicular(dir: DVec3, rng: &mut Rng) -> DVec3 {
    let r = rng.unit_vector().as_dvec3();
    let p = r - dir * r.dot(dir);
    p.try_normalize()
        .unwrap_or_else(|| dir.any_orthonormal_vector())
}

/// Turn ribbon points into camera-facing triangles (camera at `origin`). Six vertices per
/// segment, appended to `out`.
pub fn build_ribbon(
    points: &[RibbonPoint],
    origin: DVec3,
    style: RibbonStyle,
    additive: f32,
    out: &mut Vec<RibbonVertex>,
) {
    if points.len() < 2 {
        return;
    }
    let rel: Vec<Vec3> = points.iter().map(|p| (p.pos - origin).as_vec3()).collect();
    let n = rel.len();
    let mut dist = 0.0f32;
    let mut prev: Option<(RibbonVertex, RibbonVertex)> = None;
    for i in 0..n {
        let a = rel[i.saturating_sub(1)];
        let b = rel[(i + 1).min(n - 1)];
        let tangent = (b - a).normalize_or_zero();
        let side = tangent.cross(rel[i]).normalize_or_zero();
        if i > 0 {
            dist += rel[i].distance(rel[i - 1]);
        }
        let p = &points[i];
        let half = side * p.width * 0.5;
        let make = |pos: Vec3, across: f32| RibbonVertex {
            pos: pos.to_array(),
            dist,
            color: p.color,
            across,
            style: style.code(),
            additive,
            along: p.along,
        };
        let l = make(rel[i] - half, -1.0);
        let r = make(rel[i] + half, 1.0);
        if let Some((pl, pr)) = prev {
            out.extend_from_slice(&[pl, pr, r, pl, r, l]);
        }
        prev = Some((l, r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Ribbons>()
            .add_systems(PostUpdate, (update_trails, update_beams));
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        app
    }

    #[test]
    fn trails_record_points_fade_and_survive_despawn() {
        let mut app = app();
        let e = app
            .world
            .spawn((
                Transform::default(),
                Trail {
                    lifetime: 0.35,
                    min_spacing: 0.5,
                    ..Default::default()
                },
            ))
            .id();
        for i in 0..5 {
            app.world.get_mut::<Transform>(e).unwrap().translation = DVec3::new(i as f64, 0.0, 0.0);
            app.world
                .get_mut::<GlobalTransform>(e)
                .unwrap()
                .0
                .translation = DVec3::new(i as f64, 0.0, 0.0);
            app.update();
        }
        let ribbons = app.world.resource::<Ribbons>();
        let run = &ribbons.trails[&e];
        // Points older than the lifetime are gone; the head is the current position.
        assert!(
            run.points.len() >= 2 && run.points.len() <= 5,
            "{}",
            run.points.len()
        );
        assert_eq!(run.points[0].pos, DVec3::new(4.0, 0.0, 0.0));
        app.world.despawn(e);
        app.update();
        assert_eq!(app.world.resource::<Ribbons>().counts().0, 1, "orphan kept");
        app.update_n(6);
        assert_eq!(app.world.resource::<Ribbons>().counts().0, 0, "faded out");
    }

    #[test]
    fn lightning_connects_both_ends() {
        let mut rng = Rng::new(3);
        let mut paths = Vec::new();
        let from = DVec3::new(1e7, 5.0, 0.0);
        let to = from + DVec3::new(10.0, 0.0, 0.0);
        lightning_paths(from, to, &Beam::default(), &mut rng, &mut paths);
        assert_eq!(paths.len(), 1 + Beam::default().branches as usize);
        let main = &paths[0];
        assert_eq!(main[0], from);
        assert_eq!(*main.last().unwrap(), to);
        assert!(main.len() > 8);
        // Jagged but not wild.
        let max_off = main
            .iter()
            .map(|p| (p.y - 5.0).abs().max(p.z.abs()))
            .fold(0.0, f64::max);
        assert!(max_off > 0.01 && max_off < 4.0, "{max_off}");
    }

    #[test]
    fn ribbons_face_the_camera() {
        let pts: Vec<RibbonPoint> = (0..3)
            .map(|i| RibbonPoint {
                pos: DVec3::new(i as f64, 0.0, -10.0),
                width: 1.0,
                color: [1.0; 4],
                along: i as f32 / 2.0,
            })
            .collect();
        let mut out = Vec::new();
        build_ribbon(&pts, DVec3::ZERO, RibbonStyle::Energy, 1.0, &mut out);
        assert_eq!(out.len(), 12);
        // Camera looks down -Z at a ribbon along X: the ribbon spreads along Y.
        let ys: Vec<f32> = out.iter().map(|v| v.pos[1]).collect();
        assert!(ys.iter().all(|y| (y.abs() - 0.5).abs() < 1e-4), "{ys:?}");
        assert_eq!(out.last().unwrap().dist, 2.0);
    }
}

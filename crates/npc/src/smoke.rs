//! Smoke canisters. A squad's medic carries them: when the enemy watches a downed mate's spot,
//! it throws one to land between them, and the cloud hides the spot (the medic waits for that
//! as any skilled fighter waits for a watched spot to go out of view, and shots into it are
//! mostly guesses).
//!
//! The canister's gas goes out as a [`SmokeEmission`] when a smoke module is installed (it
//! registers the "smoke" [`Obscurance`] source and simulates the cloud). Without one, the NPC
//! module keeps a plain cloud of its own (a ball that swells, hangs and thins out) under the
//! "npc_smoke" source, so sight dims all the same, and draws it as a few soft grey puffs.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_render::{AlphaMode, MeshRenderer, Shape, StandardMaterial};
use engine_world::{Obscurance, ObscuranceQuery, SmokeEmission};

/// How long a thrown canister flies before it pops (s).
pub const FLIGHT_SECS: f64 = 0.9;
/// How long a cloud lasts (s): it swells over the first two seconds and thins out over the last
/// five.
pub const CLOUD_SECS: f64 = 22.0;
/// The cloud's radius at its fullest (m).
pub const CLOUD_RADIUS: f64 = 3.2;
/// How thick it is at its fullest (1/m): through its middle, about nothing gets through.
pub const CLOUD_EXTINCTION: f32 = 1.4;

/// Canisters in flight and the clouds they made (world space).
#[derive(Resource, Debug, Clone, Default)]
pub struct SmokeClouds {
    pub clouds: Vec<Cloud>,
    /// The next cloud's id.
    next: u32,
}

/// One canister: where it lands and when it was thrown (world clock, s).
#[derive(Debug, Clone, Copy)]
pub struct Cloud {
    pub id: u32,
    pub at: DVec3,
    pub thrown: f64,
    /// Handed to a smoke module (which simulates the cloud) instead of kept here.
    pub emitted: bool,
}

impl Cloud {
    /// Its radius and thickness `now` (none while in flight or once gone).
    fn now(&self, now: f64) -> Option<(f64, f32)> {
        let age = now - self.thrown - FLIGHT_SECS;
        if !(0.0..CLOUD_SECS).contains(&age) {
            return None;
        }
        let swell = (age / 2.0).min(1.0);
        let thin = ((CLOUD_SECS - age) / 5.0).min(1.0) as f32;
        Some((CLOUD_RADIUS * (0.4 + 0.6 * swell), CLOUD_EXTINCTION * thin))
    }

    /// Whether it is (or soon will be) thick at `p`.
    pub fn covers(&self, p: DVec3, now: f64) -> bool {
        let age = now - self.thrown - FLIGHT_SECS;
        age < CLOUD_SECS - 6.0 && p.distance(self.at) < CLOUD_RADIUS
    }
}

impl SmokeClouds {
    /// Throw a canister to land at `at`.
    pub fn throw(&mut self, at: DVec3, now: f64) {
        self.clouds.push(Cloud {
            id: self.next,
            at,
            thrown: now,
            emitted: false,
        });
        self.next = self.next.wrapping_add(1);
    }

    /// Whether a cloud (or one on its way) hides `p`.
    pub fn covers(&self, p: DVec3, now: f64) -> bool {
        self.clouds.iter().any(|c| c.covers(p, now))
    }
}

/// The NPC module's own clouds, for sight (balls of even thickness).
struct Balls(Vec<(DVec3, f64, f32)>);

impl ObscuranceQuery for Balls {
    fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        let d = to - from;
        let len = d.length();
        if len == 0.0 {
            return 1.0;
        }
        let dir = d / len;
        let mut depth = 0.0;
        for &(c, r, k) in &self.0 {
            let oc = from - c;
            let b = oc.dot(dir);
            let disc = b * b - (oc.length_squared() - r * r);
            if disc <= 0.0 {
                continue;
            }
            let (t0, t1) = (-b - disc.sqrt(), -b + disc.sqrt());
            depth += k as f64 * (t1.min(len) - t0.max(0.0)).max(0.0);
        }
        (-depth).exp() as f32
    }

    fn extinction(&self, p: DVec3) -> f32 {
        self.0
            .iter()
            .filter(|(c, r, _)| p.distance(*c) < *r)
            .map(|(_, _, k)| k)
            .sum()
    }
}

/// Pops the canisters that land, and keeps the clouds' sight up to date.
pub(crate) fn smoke_clouds(
    mut clouds: ResMut<SmokeClouds>,
    mut obscurance: ResMut<Obscurance>,
    mut emit: MessageWriter<SmokeEmission>,
    time: Res<Time>,
) {
    if clouds.clouds.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let module = obscurance.names().any(|n| n == "smoke");
    let mut balls = Vec::new();
    for c in &mut clouds.clouds {
        let age = now - c.thrown - FLIGHT_SECS;
        if module && (0.0..4.0).contains(&age) {
            // A smoke module's cloud from the canister's gas, the first few seconds.
            c.emitted = true;
            emit.write(SmokeEmission {
                position: c.at + DVec3::Y * 0.2,
                velocity: Vec3::Y * 0.6,
                radius: 0.6,
                mass_kg: 0.25 * time.delta_secs(),
                temperature_c: 60.0,
                darkness: 0.1,
            });
        }
        if !c.emitted
            && let Some((r, k)) = c.now(now)
        {
            balls.push((c.at + DVec3::Y * 0.8, r, k));
        }
    }
    clouds
        .clouds
        .retain(|c| now - c.thrown - FLIGHT_SECS < CLOUD_SECS);
    if balls.is_empty() {
        obscurance.remove("npc_smoke");
    } else {
        obscurance.set("npc_smoke", Arc::new(Balls(balls)));
    }
}

/// One shell of one puff of a cloud the NPC module keeps itself. A puff is a few faint grey
/// balls one inside the other, so it is thick in the middle and thins out to its edge.
#[derive(Component, Debug, Clone, Copy)]
pub struct CloudPuff {
    cloud: u32,
    n: usize,
    shell: usize,
}

/// Where a cloud's puffs sit (offsets and sizes, in its radius): a low, lumpy heap.
const PUFFS: [(Vec3, f32); 8] = [
    (Vec3::new(0.0, -0.05, 0.0), 0.7),
    (Vec3::new(0.5, -0.25, 0.2), 0.5),
    (Vec3::new(-0.45, -0.2, -0.25), 0.55),
    (Vec3::new(0.15, 0.3, -0.3), 0.45),
    (Vec3::new(-0.25, -0.3, 0.45), 0.5),
    (Vec3::new(0.3, -0.3, -0.5), 0.42),
    (Vec3::new(-0.55, -0.35, 0.2), 0.4),
    (Vec3::new(-0.1, 0.25, 0.3), 0.42),
];

/// A puff's shells, outermost first (in its size).
const SHELLS: [f32; 3] = [1.0, 0.8, 0.58];

/// Draws the clouds the NPC module keeps itself (a smoke module draws its own).
pub(crate) fn show_clouds(
    mut commands: Commands,
    clouds: Res<SmokeClouds>,
    mut puffs: Query<(Entity, &CloudPuff, &mut Transform)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    let size = |c: &Cloud| c.now(now).filter(|_| !c.emitted);
    let mut have = Vec::new();
    for (e, p, mut t) in &mut puffs {
        let cloud = clouds.clouds.iter().find(|c| c.id == p.cloud);
        match cloud.and_then(|c| size(c).map(|s| (c, s))) {
            Some((c, (r, k))) => {
                let (off, scale) = PUFFS[p.n];
                let r = r as f32 * (0.5 + 0.5 * k / CLOUD_EXTINCTION);
                t.translation = c.at + DVec3::Y * 0.8 + (off * r).as_dvec3();
                t.scale = Vec3::new(1.0, 0.85, 1.0) * (r * scale * SHELLS[p.shell]);
                have.push(p.cloud);
            }
            None => commands.entity(e).despawn(),
        }
    }
    for c in clouds.clouds.iter().filter(|c| size(c).is_some()) {
        if have.contains(&c.id) {
            continue;
        }
        let haze = StandardMaterial {
            base_color: [0.7, 0.7, 0.68, 0.17],
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 1.0,
            ..Default::default()
        };
        for n in 0..PUFFS.len() {
            for shell in 0..SHELLS.len() {
                let ball = Shape::Sphere {
                    radius: 1.0,
                    segments: 16,
                };
                commands.spawn((
                    CloudPuff {
                        cloud: c.id,
                        n,
                        shell,
                    },
                    Transform::from_translation(c.at).with_scale(Vec3::ZERO),
                    MeshRenderer::new(ball, haze).without_shadows(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clouds' sight agrees with their thickness, as every obscurance source must.
    #[test]
    fn the_clouds_obscure_as_thick_as_they_are() {
        let balls = Balls(vec![
            (DVec3::new(0.0, 0.8, 0.0), CLOUD_RADIUS, CLOUD_EXTINCTION),
            (DVec3::new(2.5, 0.8, 1.0), 1.5, 0.4),
        ]);
        for (from, to) in [
            (DVec3::new(-20.0, 1.5, 0.0), DVec3::new(20.0, 0.9, 0.3)),
            (DVec3::new(-5.0, 0.9, -1.0), DVec3::new(0.0, 0.9, 0.0)),
            (DVec3::new(0.5, 1.0, 0.2), DVec3::new(0.6, 1.0, 0.1)),
            (DVec3::new(0.0, 10.0, 0.0), DVec3::new(30.0, 10.0, 0.0)),
            (DVec3::new(1.0, 0.5, -6.0), DVec3::new(3.0, 1.2, 6.0)),
        ] {
            engine_world::check_obscurance(&balls, from, to).unwrap();
        }
        // Through the middle of a full cloud, next to nothing gets through.
        let t = balls.transmittance(DVec3::new(-10.0, 0.8, 0.0), DVec3::new(10.0, 0.8, -0.1));
        assert!(t < 0.01, "{t}");
    }

    /// A canister flies, pops, swells, hangs and thins out.
    #[test]
    fn a_cloud_swells_hangs_and_thins_out() {
        let c = Cloud {
            id: 0,
            at: DVec3::ZERO,
            thrown: 10.0,
            emitted: false,
        };
        assert!(c.now(10.0 + FLIGHT_SECS * 0.5).is_none());
        let (r0, _) = c.now(10.0 + FLIGHT_SECS + 0.1).unwrap();
        let (r1, k1) = c.now(10.0 + FLIGHT_SECS + 5.0).unwrap();
        let (_, k2) = c.now(10.0 + FLIGHT_SECS + CLOUD_SECS - 1.0).unwrap();
        assert!(r0 < r1 && r1 == CLOUD_RADIUS);
        assert!(k1 == CLOUD_EXTINCTION && k2 < 0.3 * k1);
        assert!(c.now(10.0 + FLIGHT_SECS + CLOUD_SECS + 0.1).is_none());
        assert!(c.covers(DVec3::new(1.0, 0.0, 0.0), 10.0));
        assert!(!c.covers(DVec3::new(1.0, 0.0, 0.0), 10.0 + CLOUD_SECS));
    }
}

//! Lightning: strikes from storm cells, with a jagged branching bolt, a flash that lights the
//! ground and the clouds, thunder that AI can hear, and fires where it hits something dry.

use engine_core::math::{DVec2, DVec3, Rng, Vec3};
use engine_core::prelude::*;
use engine_render::PointLight;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{
    FocusPoints, Ground, IgniteRequest, NoiseEvent, SurfaceLayers, WeatherState, WorldImpulse,
};

use crate::sim::WeatherSim;

/// How often and how hard lightning strikes.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lightning {
    /// Flashes per minute near the player at full storm activity.
    pub flashes_per_min: f32,
    /// Share of flashes that reach the ground (the rest light up the cloud from inside).
    pub ground_share: f32,
    /// Flashes happen up to this far from the player, meters.
    pub radius_m: f32,
    /// Ground strikes can start fires where the ground isn't soaked.
    pub ignites: bool,
    /// Peak light output of a flash, lumens.
    pub flash_lumens: f32,
}

impl Default for Lightning {
    fn default() -> Self {
        Self {
            flashes_per_min: 7.0,
            ground_share: 0.4,
            radius_m: 6000.0,
            ignites: true,
            flash_lumens: 4.0e9,
        }
    }
}

/// Sent for every flash. `ground` is where it hit (None for a flash inside the cloud).
#[derive(Message, Debug, Clone, Copy)]
pub struct LightningStrike {
    /// Where the bolt leaves the cloud.
    pub top: DVec3,
    pub ground: Option<DVec3>,
    /// Peak brightness relative to an average flash.
    pub intensity: f32,
}

/// One segment of a bolt, world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoltSegment {
    pub a: DVec3,
    pub b: DVec3,
    /// Glow width, meters.
    pub width: f32,
    /// 1 for the main channel, less for branches.
    pub brightness: f32,
}

/// A flash in progress.
#[derive(Debug, Clone)]
pub struct Bolt {
    pub segments: Vec<BoltSegment>,
    /// Center of the flash's light on the ground and things around it.
    pub light_at: DVec3,
    /// Where the flash lights the cloud from inside (the channel spreads out there).
    pub cloud_at: DVec3,
    pub age_s: f32,
    pub life_s: f32,
    pub intensity: f32,
    /// Seconds after the flash of each return stroke.
    pub strokes: [f32; 4],
    pub light: Option<Entity>,
    /// Hits the ground (draws a visible channel) or only lights the cloud.
    pub ground: bool,
}

impl Bolt {
    /// Brightness now, 0..~1.5: a few return strokes that each fade in tens of milliseconds.
    pub fn brightness(&self) -> f32 {
        let mut b = 0.0f32;
        for (i, s) in self.strokes.iter().enumerate() {
            let t = self.age_s - s;
            if t >= 0.0 {
                let peak = if i == 0 { 1.0 } else { 0.75 };
                b = b.max(peak * (-t / 0.045).exp());
            }
        }
        // A faint continuing current between strokes.
        if self.age_s < self.life_s {
            b = b.max(0.08 * (1.0 - self.age_s / self.life_s));
        }
        b * self.intensity
    }
}

/// Flashes in progress, for the renderer.
#[derive(Resource, Debug, Clone, Default)]
pub struct ActiveBolts {
    pub bolts: Vec<Bolt>,
}

impl ActiveBolts {
    /// The brightest flash right now: (where it lights the cloud, brightness).
    pub fn brightest(&self) -> Option<(DVec3, f32)> {
        self.bolts
            .iter()
            .map(|b| (b.cloud_at, b.brightness()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// Requests from the API for a strike at a point.
#[derive(Resource, Debug, Clone, Default)]
pub struct StrikeRequests(pub Vec<DVec3>);

/// Build a jagged bolt from `top` down to `bottom`, with branches.
pub fn build_bolt(top: DVec3, bottom: DVec3, rng: &mut Rng) -> Vec<BoltSegment> {
    let mut out = Vec::new();
    let length = (top - bottom).length() as f32;
    channel(top, bottom, length * 0.12, 6, 1.0, 0.6, rng, &mut out, 0);
    out
}

#[allow(clippy::too_many_arguments)]
fn channel(
    a: DVec3,
    b: DVec3,
    jitter: f32,
    depth: u32,
    brightness: f32,
    width: f32,
    rng: &mut Rng,
    out: &mut Vec<BoltSegment>,
    level: u32,
) {
    // Midpoint displacement: a polyline of 2^depth segments.
    let mut points = vec![a, b];
    let mut j = jitter as f64;
    for _ in 0..depth {
        let mut next = Vec::with_capacity(points.len() * 2);
        for w in points.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            let d = w[1] - w[0];
            let side = DVec3::new(
                rng.range(-1.0, 1.0) as f64,
                0.0,
                rng.range(-1.0, 1.0) as f64,
            );
            let side = side - d.normalize_or_zero() * side.dot(d.normalize_or_zero());
            next.push(w[0]);
            next.push(mid + side * j);
        }
        next.push(*points.last().unwrap());
        points = next;
        j *= 0.5;
    }
    for (i, w) in points.windows(2).enumerate() {
        out.push(BoltSegment {
            a: w[0],
            b: w[1],
            width,
            brightness,
        });
        // Branches fork off downward and fade.
        if level < 2 && i > 2 && rng.chance(0.09) {
            let start = w[1];
            let remaining = (start.y - b.y).max(10.0);
            let dir = DVec3::new(
                rng.range(-0.8, 0.8) as f64,
                -1.0,
                rng.range(-0.8, 0.8) as f64,
            )
            .normalize();
            let len = remaining * rng.range(0.15, 0.45) as f64;
            channel(
                start,
                start + dir * len,
                len as f32 * 0.15,
                4,
                brightness * 0.45,
                width * 0.55,
                rng,
                out,
                level + 1,
            );
        }
    }
}

/// FixedUpdate: decide on new flashes, age the old ones, light them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lightning_system(
    mut commands: Commands,
    time: Res<Time>,
    params: Res<Lightning>,
    state: Res<WeatherState>,
    mut sim: ResMut<WeatherSim>,
    focus: Res<FocusPoints>,
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surface: Res<SurfaceLayers>,
    mut requests: ResMut<StrikeRequests>,
    mut bolts: ResMut<ActiveBolts>,
    mut lights: Query<&mut PointLight>,
    mut strikes: MessageWriter<LightningStrike>,
    mut ignite: MessageWriter<IgniteRequest>,
    mut noise: MessageWriter<NoiseEvent>,
    mut impulses: MessageWriter<WorldImpulse>,
) {
    let dt = time.delta_secs();
    let focus = focus.primary();

    // Age and light the flashes in progress.
    bolts.bolts.retain_mut(|bolt| {
        bolt.age_s += dt;
        let alive = bolt.age_s < bolt.life_s;
        if let Some(e) = bolt.light {
            if alive {
                if let Ok(mut light) = lights.get_mut(e) {
                    light.intensity_lm = params.flash_lumens * bolt.brightness();
                }
            } else {
                commands.entity(e).despawn();
            }
        }
        alive
    });

    // New flashes: Poisson process scaled by storm activity near the player.
    let mut targets: Vec<(DVec3, bool)> = requests.0.drain(..).map(|p| (p, true)).collect();
    let activity = state.storm.max(sim.local_regime.storm * 0.5);
    if activity > 0.01 && dt > 0.0 {
        let rate = params.flashes_per_min / 60.0 * activity;
        let chance = 1.0 - (-rate * dt).exp();
        if sim.rng.f32() < chance {
            // Find a stormy spot near the player (storm cells are where it strikes).
            for _ in 0..8 {
                let r = params.radius_m * sim.rng.f32().sqrt();
                let a = sim.rng.f32() * std::f32::consts::TAU;
                let p = DVec2::new(focus.x, focus.z)
                    + DVec2::new(a.cos() as f64, a.sin() as f64) * r as f64;
                if sim.rng.f32() < sim.cell_at(p).storm.max(0.05) {
                    let to_ground = sim.rng.f32() < params.ground_share;
                    targets.push((DVec3::new(p.x, 0.0, p.y), to_ground));
                    break;
                }
            }
        }
    }

    for (at, to_ground) in targets {
        let base = state.cloud_base_m.max(300.0) as f64;
        let ground_y = ground.height(at).unwrap_or(0.0);
        let top = DVec3::new(at.x, ground_y + base, at.z);
        // Lightning finds the tallest thing under it.
        let hit = physics
            .as_deref()
            .and_then(|q| {
                q.raycast(
                    top - DVec3::Y * 1.0,
                    Vec3::NEG_Y,
                    base as f32 + 50.0,
                    &QueryFilter::default(),
                )
            })
            .map(|h| h.point)
            .unwrap_or(DVec3::new(at.x, ground_y, at.z));
        let fork_seed = sim.time_s.to_bits();
        let mut rng = sim.rng.fork(fork_seed);
        let intensity = rng.range(0.6, 1.4);
        // The channel runs on inside the cloud a few hundred meters above the base.
        let inside = top + DVec3::Y * rng.range(200.0, 900.0) as f64;
        let (segments, light_at, ground_hit) = if to_ground {
            (
                build_bolt(top, hit, &mut rng),
                hit.lerp(top, 0.25),
                Some(hit),
            )
        } else {
            (Vec::new(), inside, None)
        };
        let strokes = [
            0.0,
            rng.range(0.04, 0.09),
            rng.range(0.1, 0.2),
            if rng.chance(0.5) {
                rng.range(0.22, 0.4)
            } else {
                0.0
            },
        ];
        let light = commands
            .spawn((
                Transform::from_translation(light_at),
                PointLight {
                    color: [0.75, 0.82, 1.0],
                    intensity_lm: 0.0,
                    range: 9000.0,
                    radius: 30.0,
                },
            ))
            .id();
        bolts.bolts.push(Bolt {
            segments,
            light_at,
            cloud_at: inside,
            age_s: 0.0,
            life_s: 0.55,
            intensity,
            strokes,
            light: Some(light),
            ground: ground_hit.is_some(),
        });
        strikes.write(LightningStrike {
            top,
            ground: ground_hit,
            intensity,
        });
        noise.write(NoiseEvent {
            position: light_at,
            radius: 9000.0,
            source: None,
        });
        if let Some(h) = ground_hit {
            impulses.write(WorldImpulse {
                center: h,
                radius: 14.0,
                strength: 18.0,
                energy_j: 5.0e5,
                heat_c: 0.0,
                source: None,
            });
            let soaked = surface.sample(h).wetness > 0.6 || state.precipitation_mm_h > 15.0;
            if params.ignites && !soaked {
                ignite.write(IgniteRequest {
                    position: h,
                    radius: 1.5,
                    temperature_c: 2500.0,
                    source: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolt_reaches_the_ground_and_branches() {
        let mut rng = Rng::new(3);
        let top = DVec3::new(0.0, 1200.0, 0.0);
        let bottom = DVec3::ZERO;
        let segs = build_bolt(top, bottom, &mut rng);
        assert!(segs.len() > 64);
        assert!(segs.iter().any(|s| s.brightness < 1.0), "has branches");
        let main: Vec<_> = segs.iter().filter(|s| s.brightness == 1.0).collect();
        assert!((main[0].a - top).length() < 1e-6);
        assert!((main.last().unwrap().b - bottom).length() < 1e-6);
    }

    #[test]
    fn flash_flickers_and_fades() {
        let mut bolt = Bolt {
            segments: Vec::new(),
            light_at: DVec3::ZERO,
            cloud_at: DVec3::ZERO,
            age_s: 0.0,
            life_s: 0.5,
            intensity: 1.0,
            strokes: [0.0, 0.06, 0.15, 0.0],
            light: None,
            ground: true,
        };
        let first = bolt.brightness();
        bolt.age_s = 0.04;
        let dim = bolt.brightness();
        bolt.age_s = 0.065;
        let restrike = bolt.brightness();
        assert!(
            first > 0.9 && dim < 0.5 && restrike > 0.6,
            "{first} {dim} {restrike}"
        );
        bolt.age_s = 0.49;
        assert!(bolt.brightness() < 0.1);
    }
}

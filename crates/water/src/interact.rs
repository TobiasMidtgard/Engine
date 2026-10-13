//! What disturbs the water: swimmers, boats, footsteps, bullets, explosions and rain.
//!
//! Everything arrives through `engine_world` (positions, `GroundContact`, `ProjectileTrace`,
//! `WorldImpulse`, `WeatherState`), so water reacts to any module's characters, weapons and
//! weather without knowing them.

use engine_core::math::{DVec2, DVec3, Rng, Vec3};
use engine_core::prelude::*;
use engine_world::{
    GroundContact, NoiseEvent, ProjectileTrace, WaterSplash, WeatherState, WorldImpulse,
};

use crate::body::{RainShower, WaterSettings};
use crate::ripples::Disturbance;
use crate::state::{SharedWater, WaterState};

/// Faster than this between two steps, m/s, an interactor was moved, not carried by its own
/// motion: spawned in place, teleported, or shifted with the world before its transform caught
/// up. It throws no spray and digs no wake for that step.
const TELEPORT_SPEED: f32 = 150.0;

/// Something that pushes the water as it moves through it: a swimmer, a wading animal, a
/// boat's hull, a dragging rope. Leaves wakes and rings, and splashes on entry.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct WaterInteractor {
    /// Radius of the body in the water, meters.
    pub radius: f32,
    /// Wake strength multiplier (1 = a body this size; 2 = a churning propeller).
    pub wake: f32,
    /// Last position seen (managed by the water module).
    #[serde(skip)]
    pub last: Option<DVec3>,
    /// Was touching the water last step (managed).
    #[serde(skip)]
    pub wet: bool,
    /// Spray owed to `WaterSplash` messages while moving fast, kg (managed).
    #[serde(skip)]
    pub spray: f32,
    /// Seconds the trough it dug under itself lasts after its last push (managed).
    #[serde(skip)]
    pub trough_s: f32,
}

impl Default for WaterInteractor {
    fn default() -> Self {
        Self {
            radius: 0.4,
            wake: 1.0,
            last: None,
            wet: false,
            spray: 0.0,
            trough_s: 0.0,
        }
    }
}

impl WaterInteractor {
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            ..Self::default()
        }
    }

    pub fn with_wake(mut self, wake: f32) -> Self {
        self.wake = wake;
        self
    }
}

/// Splash rings for an object of `radius` hitting the water at `speed` (m/s).
pub fn splash(pos: DVec3, radius: f32, speed: f32) -> Disturbance {
    Disturbance {
        pos: DVec2::new(pos.x, pos.z),
        radius: radius.max(0.15),
        depth: (0.02 + 0.012 * speed).min(0.35) * radius.clamp(0.2, 2.0),
        foam: (0.25 + speed * 0.08).min(1.0),
    }
}

/// Is `p` at the water surface (within `band` meters) and where is the surface?
fn surface_near(state: &WaterState, p: DVec3, band: f32) -> Option<f64> {
    let s = state.sample(p)?;
    ((s.surface_height - p.y).abs() <= band as f64).then_some(s.surface_height)
}

/// Turn interactors, world events and rain into ripple disturbances.
#[allow(clippy::too_many_arguments)]
pub fn disturbance_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    shared: Res<SharedWater>,
    weather: Option<Res<WeatherState>>,
    mut interactors: Query<(
        Entity,
        &GlobalTransform,
        &mut WaterInteractor,
        Option<&LocalTimeScale>,
    )>,
    showers: Query<(&GlobalTransform, &RainShower)>,
    mut contacts: MessageReader<GroundContact>,
    mut traces: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut noise: MessageWriter<NoiseEvent>,
    mut splash_out: MessageWriter<WaterSplash>,
    mut rng: Local<Option<Rng>>,
) {
    let dt = time.delta_secs();
    let rng = rng.get_or_insert_with(|| Rng::new(0x05EA_51DE));
    let mut out: Vec<Disturbance> = Vec::new();
    let mut noises: Vec<NoiseEvent> = Vec::new();
    let mut splashes: Vec<WaterSplash> = Vec::new();
    let mut hulls: Vec<(DVec2, f32)> = Vec::new();
    let had_hulls;
    {
        let state = shared.0.read();
        had_hulls = !state.hulls.is_empty();

        // Swimmers, boats, wading animals.
        for (entity, gt, mut it, local) in &mut interactors {
            let p = gt.translation();
            let step = time.delta_for(local).max(1e-5);
            let last = it.last.replace(p);
            let Some(water) = state.sample(p) else {
                it.wet = false;
                continue;
            };
            let r = it.radius.max(0.05);
            let submerged =
                ((water.surface_height - (p.y - r as f64)) as f32 / (2.0 * r)).clamp(0.0, 1.0);
            let touching = submerged > 0.0 && submerged < 1.0
                || (submerged >= 1.0 && (water.surface_height - p.y) < r as f64 * 1.5);
            let v = last
                .map(|l| ((p - l) / step as f64).as_vec3())
                .filter(|v| v.length() < TELEPORT_SPEED)
                .unwrap_or(Vec3::ZERO);
            if touching {
                let speed = Vec3::new(v.x, 0.0, v.z).length();
                // Entering the water fast: a splash.
                if !it.wet && v.y < -1.5 {
                    out.push(splash(p, r, -v.y));
                    splashes.push(WaterSplash {
                        position: DVec3::new(p.x, water.surface_height, p.z),
                        velocity: v - water.velocity,
                        radius: r,
                        mass_kg: 1000.0 * r * r * -v.y * 0.05,
                        source: Some(entity),
                    });
                    if settings.noise_events {
                        noises.push(NoiseEvent {
                            position: p,
                            radius: 10.0 + 6.0 * -v.y,
                            source: Some(entity),
                        });
                    }
                }
                // Moving: push the surface where the body is (the wave equation turns the moving
                // push into a V-shaped wake), plus a bow wave just ahead.
                let push = 0.5
                    * it.wake
                    * r
                    * speed.min(8.0)
                    * step
                    * (1.0 - (2.0 * submerged - 1.0).abs()).max(0.35);
                // Fast hulls throw bow spray: sent out in puffs as `WaterSplash` so particles
                // and audio can show and play it.
                let fast = speed - 2.5;
                if fast > 0.0 && it.wake >= 1.0 {
                    it.spray += 1000.0 * r * r * fast * fast * 0.004 * it.wake * step;
                    if it.spray > 2.0 {
                        let dir = Vec3::new(v.x, 0.0, v.z) / speed;
                        let side = Vec3::new(-dir.z, 0.0, dir.x)
                            * if rng.f32() < 0.5 { -1.0 } else { 1.0 };
                        let at = p + (dir * r * 1.6 + side * r * 0.4).as_dvec3();
                        // Bow spray flies out sideways, faster than the hull goes; thrown gently
                        // up, readers would take it for a waterfall's mist.
                        splashes.push(WaterSplash {
                            position: DVec3::new(at.x, water.surface_height, at.z),
                            velocity: dir * speed * 0.5
                                + side * (speed * 0.6 + 1.5)
                                + Vec3::Y * (1.0 + speed * 0.3),
                            radius: r * 0.5,
                            mass_kg: std::mem::take(&mut it.spray),
                            source: Some(entity),
                        });
                    }
                } else {
                    it.spray = 0.0;
                }
                let bob = (v.y.abs() * step * 0.6 * r).min(0.05);
                it.trough_s = (it.trough_s - step).max(0.0);
                if it.trough_s > 0.0 {
                    hulls.push((DVec2::new(p.x, p.z), r));
                }
                if push + bob > 1e-5 {
                    if it.trough_s <= 0.0 {
                        hulls.push((DVec2::new(p.x, p.z), r));
                    }
                    it.trough_s = 1.0;
                    out.push(Disturbance {
                        pos: DVec2::new(p.x, p.z),
                        radius: r * 0.9,
                        depth: push + bob,
                        foam: (speed * 0.05 * it.wake).min(0.4) * dt * 30.0,
                    });
                }
            }
            it.wet = touching;
        }

        // Footsteps, hooves and wheels in shallow water.
        for c in contacts.read() {
            if let Some(h) = surface_near(&state, c.position, 1.5)
                && h > c.position.y
            {
                let size = (c.size[0] + c.size[1]) * 0.5;
                let depth = (0.01 + c.load_kg * 2e-4 + c.speed * 0.01).min(0.12);
                out.push(Disturbance {
                    pos: DVec2::new(c.position.x, c.position.z),
                    radius: size.max(0.15) * 1.2,
                    depth,
                    foam: (0.15 + c.speed * 0.05).min(0.8),
                });
            }
        }

        // Bullets and arrows crossing the surface.
        for t in traces.read() {
            let (Some(a), Some(b)) = (state.sample(t.from), state.sample(t.to)) else {
                continue;
            };
            let above_a = t.from.y > a.surface_height;
            let above_b = t.to.y > b.surface_height;
            if above_a == above_b {
                continue;
            }
            // Find the crossing.
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                let p = t.from + (t.to - t.from) * mid;
                let above = state
                    .sample(p)
                    .map(|s| p.y > s.surface_height)
                    .unwrap_or(true);
                if above == above_a {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let hit = t.from + (t.to - t.from) * hi;
            splashes.push(WaterSplash {
                position: hit,
                velocity: (t.to - t.from).normalize_or_zero().as_vec3() * t.speed,
                radius: 0.05,
                mass_kg: 0.4,
                source: t.shooter,
            });
            out.push(Disturbance {
                pos: DVec2::new(hit.x, hit.z),
                radius: 0.18,
                depth: (0.03 + t.speed * 1e-4).min(0.12),
                foam: 0.9,
            });
            if settings.noise_events {
                noises.push(NoiseEvent {
                    position: hit,
                    radius: 15.0,
                    source: t.shooter,
                });
            }
        }

        // Explosions near or under the surface.
        for i in impulses.read() {
            let Some(s) = state.sample(i.center) else {
                continue;
            };
            let gap = (i.center.y - s.surface_height) as f32;
            if gap.abs() > i.radius {
                continue;
            }
            let k = 1.0 - gap.abs() / i.radius.max(0.01);
            splashes.push(WaterSplash {
                position: DVec3::new(i.center.x, s.surface_height, i.center.z),
                velocity: Vec3::Y * i.strength * k,
                radius: i.radius * 0.5,
                mass_kg: (i.strength * i.radius * i.radius * k * 2.0).min(50_000.0),
                source: i.source,
            });
            out.push(Disturbance {
                pos: DVec2::new(i.center.x, i.center.z),
                radius: (i.radius * 0.35).max(0.5),
                // Underwater blasts lift a dome; surface blasts punch a crater.
                depth: gap.signum() * (i.strength * 0.02 * k).min(1.5),
                foam: 1.0,
            });
            if settings.noise_events {
                noises.push(NoiseEvent {
                    position: i.center,
                    radius: i.radius * 8.0,
                    source: i.source,
                });
            }
        }

        // Raindrops: a sample of real drops big enough for the grid (the shader draws the rest).
        let field = &state.ripples;
        if field.has_water {
            let mut rain_at = |center: DVec2, radius: f64, mm_h: f32| {
                let area = if radius.is_finite() {
                    std::f64::consts::PI * radius * radius
                } else {
                    field.size() * field.size()
                };
                let expected = settings.rain_drops * mm_h * area as f32 * dt;
                let mut count = expected.floor() as u32;
                if rng.f32() < expected.fract() {
                    count += 1;
                }
                for _ in 0..count.min(400) {
                    let pos = if radius.is_finite() {
                        let d = rng.in_unit_disk();
                        center + DVec2::new(d.x as f64, d.y as f64) * radius
                    } else {
                        field.origin + DVec2::new(rng.f32() as f64, rng.f32() as f64) * field.size()
                    };
                    out.push(Disturbance {
                        pos,
                        radius: 0.14,
                        depth: 0.004 + rng.f32() * 0.01,
                        foam: 0.0,
                    });
                }
            };
            if let Some(w) = &weather
                && w.is_raining()
            {
                rain_at(DVec2::ZERO, f64::INFINITY, w.precipitation_mm_h);
            }
            for (gt, s) in &showers {
                let c = gt.translation();
                rain_at(DVec2::new(c.x, c.z), s.radius_m as f64, s.mm_h);
            }
        }
    }
    if !out.is_empty() || !hulls.is_empty() || had_hulls {
        let mut state = shared.0.write();
        state.hulls = hulls;
        for d in out {
            state.ripples.disturb(d);
        }
    }
    for n in noises {
        noise.write(n);
    }
    for s in splashes {
        splash_out.write(s);
    }
}

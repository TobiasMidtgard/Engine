//! What disturbs the water: swimmers, boats, footsteps, cloth and ropes, bullets, explosions
//! and rain.
//!
//! Everything arrives through `engine_world` (positions, `GroundContact`, `WaterDisturbance`,
//! `ProjectileTrace`, `WorldImpulse`, `WeatherState`), so water reacts to any module's
//! characters, cloth, weapons and weather without knowing them.

use std::collections::HashMap;

use engine_core::math::{DVec2, DVec3, Rng, Vec2, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_world::{
    FocusPoints, GroundContact, NoiseEvent, ProjectileTrace, SimulationRange, WaterDisturbance,
    WaterSplash, WeatherState, WorldImpulse,
};

use crate::body::{RainShower, WaterSettings};
use crate::ripples::Disturbance;
use crate::state::{SharedWater, WaterState};

/// Faster than this between two steps, m/s, an interactor was moved, not carried by its own
/// motion: spawned in place, teleported, or shifted with the world before its transform caught
/// up. It throws no spray and digs no wake for that step.
const TELEPORT_SPEED: f32 = 150.0;
/// A position unchanged this long, seconds, is standing still (not just between frames).
const STILL_AFTER_S: f32 = 0.25;
/// Strouhal number of a body in the water: it sheds a pair of eddies every 1/0.2 = 5 body
/// widths it travels (a Kármán vortex street).
const STROUHAL: f32 = 0.2;
/// How far a long hull holds the water down under it per unit of its wake strength, meters
/// (a motorboat's wake of 2.5 about the draft of its 650 kg with crew, 25 cm; a rowboat's
/// 0.6, 6 cm).
const HULL_HEAD: f32 = 0.1;
/// How hard a shed eddy turns: its fastest water goes about 0.4 times the body's speed.
const EDDY: f32 = 1.3;
/// Going into the water faster than this, m/s, a `WaterDisturbance` throws spray.
const SPLASH_SPEED: f32 = 2.0;
/// Spray owed by one source is sent as a `WaterSplash` once it reaches this much water, kg
/// (a few puffs a second for a flapping sheet, not one per patch per step).
const SPRAY_PUFF_KG: f32 = 0.15;
/// Bow spray stops growing past this hull speed through the water, m/s (see the bow spray).
const SPRAY_TOP_MS: f32 = 12.0;
/// The most water one puff of bow spray carries, kg: a puff goes out every step or two, so
/// more makes a standing cloud rather than a sheet off the bow.
const SPRAY_PUFF_MAX_KG: f32 = 8.0;

/// What [`disturbance_system`] reads of each interactor: its place, its time scale and, for a
/// physics body, its velocity.
type Interactors = (
    Entity,
    &'static GlobalTransform,
    &'static mut WaterInteractor,
    Option<&'static LocalTimeScale>,
    Option<&'static engine_world::physics::Velocity>,
);

/// How fast `it`, now at `p`, moves: a physics body's own velocity, else its position's change
/// over the time since that last changed. Fixed steps run several to a frame and the transform
/// moves once a frame, so dividing a frame's move by one step made a boat look several times as
/// fast as it went at low frame rates (and throw a wall of spray); the steps between moves keep
/// the last velocity seen.
fn moved(it: &mut WaterInteractor, p: DVec3, step: f32, body: Option<Vec3>) -> Vec3 {
    it.since += step;
    let Some(last) = it.last else {
        it.last = Some(p);
        it.since = 0.0;
        it.moving = Vec3::ZERO;
        return body.unwrap_or(Vec3::ZERO);
    };
    if p != last {
        let v = ((p - last) / it.since as f64).as_vec3();
        it.moving = if v.length() < TELEPORT_SPEED {
            v
        } else {
            Vec3::ZERO
        };
        it.last = Some(p);
        it.since = 0.0;
    } else if it.since > STILL_AFTER_S {
        it.moving = Vec3::ZERO;
    }
    body.unwrap_or(it.moving)
}

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
    /// Half its length along the way it moves, meters, for a long hull (0: as round as
    /// `radius`). Under way it pushes the water at its bow and stern.
    pub length: f32,
    /// Last position seen (managed by the water module).
    #[serde(skip)]
    pub last: Option<DVec3>,
    /// Seconds since that position was seen, and the velocity it was last seen moving at
    /// (managed): a transform moves once a frame however many fixed steps the frame takes.
    #[serde(skip)]
    pub since: f32,
    #[serde(skip)]
    pub moving: Vec3,
    /// Was touching the water last step (managed).
    #[serde(skip)]
    pub wet: bool,
    /// Spray owed to `WaterSplash` messages while moving fast, kg (managed).
    #[serde(skip)]
    pub spray: f32,
    /// Seconds the trough it dug under itself lasts after its last push (managed).
    #[serde(skip)]
    pub trough_s: f32,
    /// Eddies shed so far, counting up as it moves (managed): each whole number sheds one,
    /// off alternate sides.
    #[serde(skip)]
    pub shed: f32,
    /// Out of the `SimulationRange` of every focus: it stirs nothing until one comes near
    /// (managed).
    #[serde(skip)]
    pub resting: bool,
}

impl Default for WaterInteractor {
    fn default() -> Self {
        Self {
            radius: 0.4,
            wake: 1.0,
            length: 0.0,
            last: None,
            since: 0.0,
            moving: Vec3::ZERO,
            wet: false,
            spray: 0.0,
            trough_s: 0.0,
            shed: 0.0,
            resting: false,
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

    /// A long hull, `half_length` from its middle to its bow (see [`Self::length`]).
    pub fn with_length(mut self, half_length: f32) -> Self {
        self.length = half_length;
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
        stir: None,
        spin: 0.0,
        press: 0.0,
        keel: Vec2::ZERO,
    }
}

/// Is `p` at the water surface (within `band` meters) and where is the surface?
fn surface_near(state: &WaterState, p: DVec3, band: f32) -> Option<f64> {
    let s = state.sample(p)?;
    ((s.surface_height - p.y).abs() <= band as f64).then_some(s.surface_height)
}

/// Turn interactors, world events and rain into ripple disturbances. Interactors out of the
/// `SimulationRange` of every focus rest.
#[allow(clippy::too_many_arguments)]
pub fn disturbance_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    shared: Res<SharedWater>,
    focus: Res<FocusPoints>,
    range: Res<SimulationRange>,
    weather: Option<Res<WeatherState>>,
    mut interactors: Query<Interactors>,
    showers: Query<(&GlobalTransform, &RainShower)>,
    mut contacts: MessageReader<GroundContact>,
    mut pushes: MessageReader<WaterDisturbance>,
    mut traces: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut noise: MessageWriter<NoiseEvent>,
    mut splash_out: MessageWriter<WaterSplash>,
    mut rng: Local<Option<Rng>>,
    mut spray_owed: Local<HashMap<Option<Entity>, f32>>,
) {
    let dt = time.delta_secs();
    let rng = rng.get_or_insert_with(|| Rng::new(0x05EA_51DE));
    let mut out: Vec<Disturbance> = Vec::new();
    let mut noises: Vec<NoiseEvent> = Vec::new();
    let mut splashes: Vec<WaterSplash> = Vec::new();
    let mut hulls: Vec<crate::state::Hull> = Vec::new();
    let had_hulls;
    {
        let state = shared.0.read();
        had_hulls = !state.hulls.is_empty();

        // Swimmers, boats, wading animals.
        for (entity, gt, mut it, local, body) in &mut interactors {
            let p = gt.translation();
            let mut resting = it.resting;
            if crate::rests(&range, &focus, p, it.radius as f64, &mut resting) {
                if !it.resting {
                    it.resting = true;
                    it.last = None;
                    it.wet = false;
                }
                continue;
            }
            it.resting = false;
            let step = time.delta_for(local).max(1e-5);
            let v = moved(&mut it, p, step, body.map(|b| b.linear));
            let Some(water) = state.sample(p) else {
                it.wet = false;
                continue;
            };
            let r = it.radius.max(0.05);
            let submerged =
                ((water.surface_height - (p.y - r as f64)) as f32 / (2.0 * r)).clamp(0.0, 1.0);
            let touching = submerged > 0.0 && submerged < 1.0
                || (submerged >= 1.0 && (water.surface_height - p.y) < r as f64 * 1.5);
            // What makes the wake is how it moves through the water: a wader standing in a
            // stream parts the current as one walking through still water does.
            let through = v - water.velocity;
            if touching {
                let speed = Vec3::new(through.x, 0.0, through.z).length();
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
                // Past [`SPRAY_TOP_MS`] the sheet thrown off the bow no longer grows: it breaks
                // into finer drops, and a hull moving that fast is a glitch more often than not.
                // Only a hull planing along the surface: a log tumbling down a fall or a
                // swimmer isn't one.
                let fast = speed.min(SPRAY_TOP_MS) - 2.5;
                if fast > 0.0 && it.wake >= 1.0 && it.length > r && v.y.abs() < 2.0 {
                    it.spray += 1000.0 * r * r * fast * fast * 0.004 * it.wake * step;
                    if it.spray > 2.0 {
                        let dir = Vec3::new(through.x, 0.0, through.z) / speed;
                        let side = Vec3::new(-dir.z, 0.0, dir.x)
                            * if rng.f32() < 0.5 { -1.0 } else { 1.0 };
                        let at = p + (dir * r * 1.6 + side * r * 0.4).as_dvec3();
                        // Bow spray flies out sideways, faster than the hull goes; thrown gently
                        // up, readers would take it for a waterfall's mist.
                        let speed = speed.min(SPRAY_TOP_MS);
                        splashes.push(WaterSplash {
                            position: DVec3::new(at.x, water.surface_height, at.z),
                            velocity: dir * speed * 0.5
                                + side * (speed * 0.6 + 1.5)
                                + Vec3::Y * (1.0 + speed * 0.3),
                            radius: r * 0.5,
                            mass_kg: std::mem::take(&mut it.spray).min(SPRAY_PUFF_MAX_KG),
                            source: Some(entity),
                        });
                    }
                } else {
                    it.spray = 0.0;
                }
                let bob = (v.y.abs() * step * 0.6 * r).min(0.05);
                // Its own dip, left out of what it floats on: a long hull's along its length.
                let half = if it.length > r && speed > 0.3 {
                    (Vec2::new(through.x, through.z) / speed * (it.length - r)).as_dvec2()
                } else {
                    DVec2::ZERO
                };
                let hull = crate::state::Hull {
                    center: DVec2::new(p.x, p.z),
                    half,
                    radius: r,
                };
                it.trough_s = (it.trough_s - step).max(0.0);
                if it.trough_s > 0.0 {
                    hulls.push(hull);
                }
                if push + bob > 1e-5 {
                    if it.trough_s <= 0.0 {
                        hulls.push(hull);
                    }
                    it.trough_s = 1.0;
                    // The body drags the water it moves through along with it: the wash
                    // behind it runs on and curls into eddies at its edges.
                    let flat = Vec2::new(through.x, through.z);
                    // A long hull also holds the water down along its length as it goes: a
                    // pressure patch the shape of its waterline, which the dispersive surface
                    // turns into a Kelvin wake whose waves are as long as the speed makes them
                    // (Havelock 1908; the grid takes the plain push). The patch's sharp edges
                    // and pointed bow make the short divergent waves that draw the wake's V; a
                    // smooth patch makes only the long ones, too gentle to see (Darmon,
                    // Benzaquen and Raphaël 2014).
                    let long = it.length > r && speed > 0.3;
                    let head = if long {
                        (HULL_HEAD * it.wake).min(0.4) * smoothstep(0.3, 1.0, speed)
                    } else {
                        0.0
                    };
                    let keel = if long {
                        flat / speed * it.length
                    } else {
                        Vec2::ZERO
                    };
                    out.push(Disturbance {
                        pos: DVec2::new(p.x, p.z),
                        radius: r * 0.9,
                        depth: push + bob,
                        foam: (speed * 0.07 * it.wake).min(0.5) * dt * 30.0,
                        stir: (speed > 0.1).then_some(flat * 0.6),
                        spin: 0.0,
                        press: head * step,
                        keel,
                    });
                    // Eddies peel off alternate sides behind it, turning the way the water
                    // dragged past that side turns: its wake winds into a vortex street.
                    if speed > 0.3 {
                        let before = it.shed;
                        it.shed += speed * step * 2.0 * STROUHAL / (2.0 * r);
                        if it.shed.floor() > before.floor() {
                            let dir = flat / speed;
                            let left = Vec2::new(-dir.y, dir.x);
                            let side = if it.shed.floor() as i64 % 2 == 0 {
                                1.0
                            } else {
                                -1.0
                            };
                            let at = DVec2::new(p.x, p.z)
                                - (dir * r * 1.2 - left * side * r * 0.7).as_dvec2();
                            out.push(Disturbance {
                                pos: at,
                                radius: r * 0.9,
                                spin: side * (EDDY * speed / r).min(12.0),
                                ..Default::default()
                            });
                        }
                    }
                    // A churning propeller also throws a jet of foamy water back past the
                    // hull: the shear between the two rolls up into a trail of swirls.
                    if it.wake > 1.0 && speed > 0.5 {
                        let dir = flat / speed;
                        let churn = (it.wake - 1.0).min(2.0);
                        // Turbulent: the jet wavers from side to side, shedding eddies.
                        let side = Vec2::new(-dir.y, dir.x) * (rng.f32() - 0.5) * r * 0.8;
                        let wobble = Vec2::from_angle((rng.f32() - 0.5) * 1.2);
                        let behind = DVec2::new(p.x, p.z) - (dir * r * 1.6 - side).as_dvec2();
                        out.push(Disturbance {
                            pos: behind,
                            radius: r * 0.6,
                            depth: 0.0,
                            foam: (0.2 * churn * speed.min(6.0)).min(1.0) * dt * 30.0,
                            stir: Some(wobble.rotate(flat) * (0.6 - 0.45 * churn)),
                            spin: 0.0,
                            press: 0.0,
                            keel: Vec2::ZERO,
                        });
                    }
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
                    stir: None,
                    spin: 0.0,
                    press: 0.0,
                    keel: Vec2::ZERO,
                });
            }
        }

        // Cloth, ropes, hair, nets and paddles pushing the surface (`WaterDisturbance`): rings
        // where they slap in or lift out, a wake and drag where they sweep along it, spray
        // when they go in fast.
        for d in pushes.read() {
            let r = d.radius.max(0.05);
            let Some(surface) = surface_near(&state, d.position, r + 0.5) else {
                continue;
            };
            let v = d.velocity;
            let flat = Vec2::new(v.x, v.z);
            let sweep = flat.length().min(8.0);
            // How solid the push is: a light sheet slapping in dents the surface a little, a
            // paddle or a net full of fish moves it like a body would (its mass against a 1 cm
            // layer of water under it).
            let layer = 1000.0 * std::f32::consts::PI * r * r * 0.01;
            let solid = (d.mass_kg.max(0.0) / layer).sqrt().clamp(0.4, 1.0);
            let plunge = (-v.y).clamp(0.0, 10.0);
            let lift = v.y.clamp(0.0, 10.0);
            let depth = (0.5 * sweep + 0.8 * plunge - 0.3 * lift) * r * dt.max(1e-4) * solid;
            out.push(Disturbance {
                pos: DVec2::new(d.position.x, d.position.z),
                radius: r,
                depth: depth.clamp(-0.05, 0.08),
                // Slapping in churns up a little white; dragging through the water only a
                // thin film over time (a sheet held in a stream all day stays a sheet).
                foam: ((plunge - 0.8).max(0.0) * 0.25 + sweep * 0.06 * dt).min(0.6) * solid,
                stir: (sweep > 0.1).then_some(flat.clamp_length_max(4.0) * 0.5 * solid),
                spin: 0.0,
                press: 0.0,
                keel: Vec2::ZERO,
            });
            if plunge > SPLASH_SPEED {
                let owed = spray_owed.entry(d.source).or_insert(0.0);
                *owed += d.mass_kg.max(0.0) * (plunge - SPLASH_SPEED) * 0.5;
                if *owed > SPRAY_PUFF_KG {
                    let at = DVec3::new(d.position.x, surface, d.position.z);
                    splashes.push(WaterSplash {
                        position: at,
                        velocity: v,
                        radius: r,
                        mass_kg: std::mem::take(owed),
                        source: d.source,
                    });
                    if settings.noise_events {
                        noises.push(NoiseEvent {
                            position: at,
                            radius: 6.0 + 3.0 * plunge,
                            source: d.source,
                        });
                    }
                }
            }
        }
        // (Sources come and go: forget them now and then rather than keep every one.)
        if spray_owed.len() > 256 {
            spray_owed.clear();
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
                stir: None,
                spin: 0.0,
                press: 0.0,
                keel: Vec2::ZERO,
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
                stir: None,
                spin: 0.0,
                press: 0.0,
                keel: Vec2::ZERO,
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
                        stir: None,
                        spin: 0.0,
                        press: 0.0,
                        keel: Vec2::ZERO,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hull_moved_once_a_frame_goes_its_real_speed() {
        // 6 m/s, three fixed steps of 1/60 s to a frame, the transform moving once a frame.
        let mut it = WaterInteractor::new(0.9);
        let mut p = DVec3::ZERO;
        let mut seen = Vec::new();
        for _frame in 0..10 {
            p.x += 6.0 * 3.0 / 60.0;
            for _ in 0..3 {
                seen.push(moved(&mut it, p, 1.0 / 60.0, None).x);
            }
        }
        for v in &seen[3..] {
            assert!((v - 6.0).abs() < 0.05, "{seen:?}");
        }
        // Stopped: still after a moment.
        for _ in 0..30 {
            moved(&mut it, p, 1.0 / 60.0, None);
        }
        assert_eq!(moved(&mut it, p, 1.0 / 60.0, None), Vec3::ZERO);
        // A physics body says how fast it goes.
        assert_eq!(moved(&mut it, p, 1.0 / 60.0, Some(Vec3::X)), Vec3::X);
    }
}

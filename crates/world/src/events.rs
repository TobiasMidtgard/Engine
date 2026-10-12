//! World events: how one system tells every other system that something physical happened.
//!
//! Gameplay, physics and AI write these; simulations react to them without knowing who sent
//! them. A gunshot writes one [`ProjectileTrace`]: smoke parts around the bullet path, audio
//! plays a whiz, AI hears it, water splashes where it crosses the surface.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;

use crate::physical::SurfaceId;

/// A projectile (bullet, arrow, spell bolt) traveled from `from` to `to` this step.
#[derive(Message, Debug, Clone, Copy)]
pub struct ProjectileTrace {
    pub from: DVec3,
    pub to: DVec3,
    /// Radius of influence on smoke/foliage, meters.
    pub radius: f32,
    /// Speed, m/s.
    pub speed: f32,
    pub shooter: Option<Entity>,
}

/// A pressure impulse: explosions, shockwaves, heavy landings, magic blasts.
#[derive(Message, Debug, Clone, Copy)]
pub struct WorldImpulse {
    pub center: DVec3,
    pub radius: f32,
    /// Peak air velocity at the center, m/s (drives wind, smoke, foliage, particles).
    pub strength: f32,
    /// Energy for destruction, joules (0 = none).
    pub energy_j: f32,
    /// Heat added at the center, °C (0 = none; > 300 can ignite flammables).
    pub heat_c: f32,
    pub source: Option<Entity>,
}

/// Something touched the ground with weight: feet, paws, wheels, bodies.
/// Terrain deforms (mud, snow, sand), grass bends, audio plays a footstep, VFX kicks dust.
#[derive(Message, Debug, Clone, Copy)]
pub struct GroundContact {
    pub position: DVec3,
    /// Contact normal (usually ground up).
    pub normal: Vec3,
    /// Footprint/contact size (width, length), meters.
    pub size: [f32; 2],
    /// Heading of the contact's long axis about +Y, radians: the toe, or the direction of
    /// travel, points along `Quat::from_rotation_y(yaw) * Vec3::NEG_Z`, so yaw 0 faces −Z like
    /// `Transform::forward()`. Write it with [`GroundContact::yaw_of`]; read it with
    /// [`GroundContact::forward`].
    pub yaw: f32,
    /// Weight on this contact, kg.
    pub load_kg: f32,
    /// Horizontal speed, m/s (skids, splashes).
    pub speed: f32,
    pub kind: ContactKind,
    /// Surface if known; otherwise receivers query the ground.
    pub surface: Option<SurfaceId>,
    pub entity: Option<Entity>,
}

impl GroundContact {
    /// The [`yaw`](Self::yaw) of a contact whose toe or travel points along `forward` (any
    /// length; only its horizontal part counts).
    pub fn yaw_of(forward: Vec3) -> f32 {
        (-forward.x).atan2(-forward.z)
    }

    /// Unit horizontal direction of the toe or travel.
    pub fn forward(&self) -> Vec3 {
        Quat::from_rotation_y(self.yaw) * Vec3::NEG_Z
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContactKind {
    #[default]
    Foot,
    Paw,
    Hoof,
    Wheel,
    Track,
    Body,
}

/// Request to ignite at a point (spells, lightning, sparks, explosions).
#[derive(Message, Debug, Clone, Copy)]
pub struct IgniteRequest {
    pub position: DVec3,
    pub radius: f32,
    /// Flame temperature, °C.
    pub temperature_c: f32,
    pub source: Option<Entity>,
}

/// Damage dealt to an entity. Health, destruction and AI all read this.
#[derive(Message, Debug, Clone)]
pub struct DamageEvent {
    pub target: Entity,
    pub amount: f32,
    pub kind: DamageKind,
    pub point: DVec3,
    /// Push applied at the point, N·s.
    pub impulse: Vec3,
    pub source: Option<Entity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DamageKind {
    #[default]
    Blunt,
    Slash,
    Pierce,
    Ballistic,
    Explosive,
    Fire,
    Cold,
    Magic,
    Fall,
}

/// A sound that AI can hear (separate from what the audio engine plays).
#[derive(Message, Debug, Clone, Copy)]
pub struct NoiseEvent {
    pub position: DVec3,
    /// Audible radius, meters.
    pub radius: f32,
    pub source: Option<Entity>,
}

/// Something broke the water surface: an object falling in, a bullet, a footstep or wheel in
/// shallow water, a boat's bow wave, a breaching whale. The water module writes it; particles
/// (spray), audio (splash sound) and AI read it.
#[derive(Message, Debug, Clone, Copy)]
pub struct WaterSplash {
    /// Where on the surface, world meters.
    pub position: DVec3,
    /// Velocity of the object relative to the water, m/s (negative y = going in).
    pub velocity: Vec3,
    /// Size of the disturbance, meters.
    pub radius: f32,
    /// Water thrown up, kg (spray amount, sound loudness).
    pub mass_kg: f32,
    pub source: Option<Entity>,
}

/// Smoke or gas released into the air (fires, explosions, exhausts, smoke grenades).
/// The smoke module turns these into volumetric smoke; particles may add soot.
#[derive(Message, Debug, Clone, Copy)]
pub struct SmokeEmission {
    pub position: DVec3,
    /// Initial velocity of the released gas, m/s.
    pub velocity: Vec3,
    /// Source radius, meters.
    pub radius: f32,
    /// Mass released this step, kg (a campfire ≈ 0.002 kg/s, a smoke grenade ≈ 0.05 kg/s).
    pub mass_kg: f32,
    /// Gas temperature, °C (hot smoke rises).
    pub temperature_c: f32,
    /// 0 = white steam .. 1 = black soot.
    pub darkness: f32,
}

/// Lightning struck. The weather module writes it when a storm decides on a strike; rendering
/// and particles draw the bolt and flash, fire may ignite where a cloud-to-ground strike lands
/// (it still sends its own [`IgniteRequest`] if it wants one), and audio rolls thunder along the
/// channel, each part arriving `distance / 343 m/s` late.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct LightningStrike {
    /// Where the channel leaves the cloud, world meters.
    pub from: DVec3,
    /// Where it ends: the ground strike point (cloud-to-ground) or inside the cloud.
    pub to: DVec3,
    /// Peak current, kA (a typical cloud-to-ground stroke is 10 to 50, rarely over 200).
    pub peak_current_ka: f32,
    /// Reaches the ground (true) or stays in or between clouds (false).
    pub cloud_to_ground: bool,
    /// Picks the channel's zigzag: everyone calling [`LightningStrike::path`] gets the same bolt.
    pub seed: u64,
}

/// Highest `detail` [`LightningStrike::path`] accepts (4097 points).
pub const LIGHTNING_MAX_DETAIL: u32 = 12;

impl LightningStrike {
    /// How far each midpoint may move sideways, as a fraction of its segment's length.
    const ROUGHNESS: f64 = 0.12;

    /// The channel as `2^detail + 1` points from `from` to `to` (`detail` is capped at
    /// [`LIGHTNING_MAX_DETAIL`]). The same strike always gives the same points, and a coarser
    /// path is every other point of a finer one, so the bolt the renderer draws at detail 10
    /// and the thunder audio rolls along at detail 5 line up. Points only move sideways, so
    /// point `i` sits `i / 2^detail` of the way along the line from `from` to `to`.
    pub fn path(&self, detail: u32) -> Vec<DVec3> {
        let detail = detail.min(LIGHTNING_MAX_DETAIL);
        let n = 1usize << detail;
        let mut points = vec![self.from; n + 1];
        points[n] = self.to;
        let Some(axis) = (self.to - self.from).try_normalize() else {
            return points;
        };
        let (side, other_side) = axis.any_orthonormal_pair();
        // Key each point by where it sits on the finest path, so detail levels agree.
        let finest = LIGHTNING_MAX_DETAIL - detail;
        let mut step = n;
        while step > 1 {
            let half = step / 2;
            for i in (half..n).step_by(step) {
                let (a, b) = (points[i - half], points[i + half]);
                let key = ((i as u64) << finest).wrapping_mul(2) ^ self.seed.rotate_left(17);
                let x = unit_noise(key);
                let y = unit_noise(key ^ 0x5851_f42d_4c95_7f2d);
                let reach = a.distance(b) * Self::ROUGHNESS;
                points[i] = (a + b) * 0.5 + (side * x + other_side * y) * reach;
            }
            step = half;
        }
        points
    }
}

/// A value in -1..1 from a key (splitmix64).
fn unit_noise(key: u64) -> f64 {
    let mut z = key.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// An attack is starting (telegraph). Sent by players and NPCs before the hit lands, so
/// defenders can block or dodge, cameras and time dilation can react (cinematic slow
/// motion), and audio/VFX can cue the swing.
#[derive(Message, Debug, Clone, Copy)]
pub struct AttackEvent {
    pub attacker: Entity,
    pub target: Option<Entity>,
    /// Seconds (world time) until the hit lands.
    pub windup_secs: f32,
    /// Reach of the attack, meters.
    pub reach: f32,
    pub kind: AttackKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AttackKind {
    #[default]
    Melee,
    Ranged,
    Spell,
    /// A gap-closing dash attack.
    Dash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_yaw_follows_the_transform_convention() {
        let contact = |yaw| GroundContact {
            position: DVec3::ZERO,
            normal: Vec3::Y,
            size: [0.1, 0.28],
            yaw,
            load_kg: 70.0,
            speed: 1.4,
            kind: ContactKind::Foot,
            surface: None,
            entity: None,
        };
        assert_eq!(GroundContact::yaw_of(Vec3::NEG_Z), 0.0);
        for dir in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::new(0.6, -0.3, -0.8)] {
            let yaw = GroundContact::yaw_of(dir);
            let flat = Vec3::new(dir.x, 0.0, dir.z).normalize();
            assert!(contact(yaw).forward().distance(flat) < 1e-6, "{dir}");
            // The same heading a transform turned by that yaw looks along.
            let look = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            assert!(look.distance(flat) < 1e-6, "{dir}");
        }
    }

    fn strike(seed: u64) -> LightningStrike {
        LightningStrike {
            from: DVec3::new(120.0, 2400.0, -80.0),
            to: DVec3::new(-300.0, 3.0, 450.0),
            peak_current_ka: 30.0,
            cloud_to_ground: true,
            seed,
        }
    }

    #[test]
    fn a_strike_always_draws_the_same_bolt() {
        let s = strike(7);
        assert_eq!(s.path(9), s.path(9));
        assert_ne!(s.path(9), strike(8).path(9));
        let path = s.path(9);
        assert_eq!(path.len(), 513);
        assert_eq!(path[0], s.from);
        assert_eq!(path[512], s.to);
        assert_eq!(s.path(40).len(), (1 << LIGHTNING_MAX_DETAIL) + 1);
    }

    #[test]
    fn coarse_paths_are_every_other_point_of_fine_ones() {
        let s = strike(42);
        let fine = s.path(10);
        let coarse = s.path(5);
        for (i, p) in coarse.iter().enumerate() {
            assert_eq!(*p, fine[i << 5], "point {i}");
        }
    }

    #[test]
    fn the_bolt_zigzags_sideways_but_keeps_going() {
        let s = strike(3);
        let path = s.path(10);
        let line = s.to - s.from;
        let length = line.length();
        let axis = line / length;
        let n = path.len() - 1;
        let mut traveled = 0.0;
        for (i, p) in path.iter().enumerate() {
            let along = (*p - s.from).dot(axis);
            assert!(
                (along - length * i as f64 / n as f64).abs() < 1e-6,
                "point {i}"
            );
            let sideways = (*p - s.from - axis * along).length();
            assert!(
                sideways < 0.4 * length,
                "point {i} is {sideways} m off the line"
            );
            if i > 0 {
                traveled += p.distance(path[i - 1]);
            }
        }
        // Real channels are tortuous: noticeably longer than the straight line.
        assert!(traveled > 1.2 * length, "{traveled} vs {length}");
        let widest = path
            .iter()
            .map(|p| (*p - s.from - axis * (*p - s.from).dot(axis)).length())
            .fold(0.0, f64::max);
        assert!(widest > 0.02 * length, "{widest}");
    }

    #[test]
    fn a_zero_length_strike_is_a_point() {
        let s = LightningStrike {
            to: DVec3::new(120.0, 2400.0, -80.0),
            ..strike(1)
        };
        assert!(s.path(4).iter().all(|p| *p == s.from));
    }
}

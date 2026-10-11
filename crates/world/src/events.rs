//! World events: how one system tells every other system that something physical happened.
//!
//! Gameplay, physics and AI write these; simulations react to them without knowing who sent
//! them. A gunshot writes one [`ProjectileTrace`]: smoke parts around the bullet path, audio
//! plays a whiz, AI hears it, water splashes where it crosses the surface.

use engine_core::math::{DVec3, Vec3};
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
    /// Yaw of the contact's long axis, radians (footprint orientation).
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

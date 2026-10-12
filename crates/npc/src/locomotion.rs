//! The fallback mover: turns `MoveIntent` into motion for NPCs and animals that no movement
//! module drives. Once a movement module has claimed `LocomotionBackend`, it leaves every body
//! with a `CharacterController` to that module (a game opts an NPC into the movement module's
//! wall-running and sliding by giving it that module's character components) and keeps
//! moving the rest. Switch it off entirely with `NpcSettings::builtin_locomotion`.
//!
//! It accelerates toward the wished velocity, dashes, follows the ground and walkable tops of crates and
//! platforms, slides along walls, swims in deep water, turns toward where it looks, writes
//! `LocomotionState` and `AnimationParams`, and sends a `GroundContact` per footstep so
//! terrain, grass and audio react to NPCs and animals the same way they react to the player.

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;
use engine_world::locomotion::{LocomotionBackend, LocomotionMode, LocomotionState, MoveIntent};
use engine_world::physics::CharacterController;
use engine_world::{ContactKind, Ground, GroundContact, Water};

use crate::agent::{NpcClock, NpcCombat, NpcMotor, NpcSettings, NpcVitals};
use crate::skills::NpcSkills;
use crate::world_model::TacticalWorld;

/// What touches the ground when this body walks (footprints and footstep sounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FootKind {
    #[default]
    Foot,
    Paw,
    Hoof,
    /// No footsteps (birds in the air, floating things).
    None,
}

/// Footstep settings for the fallback mover.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Footfall {
    pub kind: FootKind,
    /// Body mass, kg (split over the feet on the ground).
    pub mass_kg: f32,
    /// Footprint size (width, length), meters.
    pub size: [f32; 2],
    /// Distance between footfalls at walking speed, meters.
    pub stride: f32,
}

impl Default for Footfall {
    fn default() -> Self {
        Self {
            kind: FootKind::Foot,
            mass_kg: 80.0,
            size: [0.11, 0.28],
            stride: 0.75,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Dash {
    from: DVec3,
    to: DVec3,
    t: f32,
    secs: f32,
}

/// The fallback mover's per-body state.
#[derive(Component, Debug, Clone, Default)]
pub struct MoverState {
    dash: Option<Dash>,
    vertical: f32,
    stride: f32,
    left: bool,
    /// Gait phase in radians (stand-in bodies swing their legs with it).
    pub phase: f32,
}

/// Bodies that move themselves (flying birds) and must be left alone by the fallback mover.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SelfPropelled;

type MoverItem<'a> = (
    Entity,
    &'a MoveIntent,
    &'a NpcMotor,
    &'a mut Transform,
    &'a mut LocomotionState,
    &'a mut MoverState,
    Option<&'a mut AnimationParams>,
    Option<&'a NpcClock>,
    Option<&'a LocalTimeScale>,
    Option<&'a NpcCombat>,
    Option<&'a NpcVitals>,
    Option<&'a NpcSkills>,
    Option<&'a Footfall>,
    Has<CharacterController>,
);

#[allow(clippy::too_many_arguments)]
pub(crate) fn fallback_move(
    settings: Res<NpcSettings>,
    backend: Option<Res<LocomotionBackend>>,
    time: Res<Time>,
    world: Res<TacticalWorld>,
    ground: Res<Ground>,
    water: Res<Water>,
    mut q: Query<MoverItem, Without<SelfPropelled>>,
    mut contacts: MessageWriter<GroundContact>,
) {
    if !settings.builtin_locomotion {
        return;
    }
    let backend = backend.is_some();
    for (
        e,
        intent,
        motor,
        mut t,
        mut state,
        mut mover,
        anim,
        clock,
        local,
        combat,
        vitals,
        skills,
        foot,
        controlled,
    ) in &mut q
    {
        if backend && controlled {
            continue;
        }
        let dt = clock.map(|c| c.dt).unwrap_or_else(|| time.delta_for(local));
        if dt <= 0.0 {
            continue;
        }
        if vitals.is_some_and(|v| v.downed) {
            mover.dash = None;
            state.velocity = Vec3::ZERO;
            state.mode = LocomotionMode::Idle;
            state.blocking = false;
            state.attacking = false;
            continue;
        }
        let pos = t.translation;
        let agility = skills.map(|s| s.agility).unwrap_or(1.0);
        // ---------------------------------------------------------------- dash
        if mover.dash.is_none()
            && let Some(to) = intent.dash_to
        {
            let to = DVec3::new(to.x, pos.y, to.z);
            mover.dash = Some(Dash {
                from: pos,
                to,
                t: 0.0,
                secs: motor.dash_secs.max(0.05),
            });
        }
        let mut new_pos;
        let dashing;
        if let Some(mut d) = mover.dash {
            d.t = (d.t + dt / d.secs).min(1.0);
            let ease = 1.0 - (1.0 - d.t).powi(2);
            new_pos = d.from.lerp(d.to, ease as f64);
            new_pos.y = pos.y;
            dashing = true;
            mover.dash = if d.t >= 1.0 { None } else { Some(d) };
        } else {
            dashing = false;
            let mut wish = intent.direction.clamp_length_max(1.0);
            wish.y = 0.0;
            let mut speed = motor.run_speed * agility;
            if intent.sprint && wish.length() > 0.5 {
                speed = motor.sprint_speed * agility;
            }
            if intent.crouch {
                speed *= 0.45;
            }
            if intent.block || intent.attack.is_some() {
                speed *= 0.5;
            }
            let swimming = water.submersion(pos + DVec3::Y * (motor.height as f64 * 0.6)) > 0.0;
            if swimming {
                speed *= 0.35;
            }
            let desired = wish * speed;
            let mut v = Vec3::new(state.velocity.x, 0.0, state.velocity.z);
            let dv = desired - v;
            let max = motor.acceleration * agility * dt;
            v += dv.clamp_length_max(max);
            new_pos = pos + (v * dt).as_dvec3();
        }
        // ---------------------------------------------------------------- collide and stand
        new_pos = world.resolve_overlap(new_pos, motor.radius, motor.height, motor.step_height);
        let surface = world.surface_height(new_pos, pos.y, motor.step_height, &ground);
        let mut grounded = true;
        if surface >= pos.y - 0.06 {
            new_pos.y = surface;
            mover.vertical = 0.0;
        } else {
            mover.vertical -= 9.81 * dt;
            new_pos.y = (pos.y + (mover.vertical * dt) as f64).max(surface);
            grounded = new_pos.y <= surface + 1e-3;
            if grounded {
                mover.vertical = 0.0;
            }
        }
        // Float in deep water.
        if let Some(w) = water.sample(new_pos)
            && w.surface_height - new_pos.y > motor.height as f64 * 0.75
        {
            new_pos.y = w.surface_height - motor.height as f64 * 0.75;
            grounded = false;
        }
        let moved = new_pos - pos;
        let vel = (moved / dt as f64).as_vec3();
        t.translation = new_pos;
        // ---------------------------------------------------------------- turn
        let face = intent.look.or_else(|| {
            let flat = Vec3::new(vel.x, 0.0, vel.z);
            (flat.length() > 0.3).then(|| flat.normalize())
        });
        if let Some(f) = face {
            let f = Vec3::new(f.x, 0.0, f.z);
            if f.length_squared() > 1e-6 {
                let target_yaw = (-f.x).atan2(-f.z);
                let (yaw, _, _) = t.rotation.to_euler(EulerRot::YXZ);
                let mut diff = target_yaw - yaw;
                while diff > PI {
                    diff -= TAU;
                }
                while diff < -PI {
                    diff += TAU;
                }
                let step = motor.turn_rate * agility * dt * if dashing { 2.0 } else { 1.0 };
                let new_yaw = yaw + diff.clamp(-step, step);
                t.rotation = Quat::from_rotation_y(new_yaw);
            }
        }
        // ---------------------------------------------------------------- report
        let horiz = Vec3::new(vel.x, 0.0, vel.z).length();
        let now = clock.map(|c| c.now).unwrap_or(0.0);
        state.velocity = vel;
        state.grounded = grounded;
        state.blocking = intent.block;
        state.attacking = intent.attack.is_some();
        state.vulnerable = combat.is_some_and(|c| c.is_vulnerable(now));
        state.stamina = 1.0;
        state.mode = if dashing {
            LocomotionMode::Dash
        } else if !grounded && water.submersion(new_pos + DVec3::Y * 1.0) > 0.0 {
            LocomotionMode::Swim
        } else if !grounded {
            LocomotionMode::Air
        } else if intent.crouch {
            LocomotionMode::Crouch
        } else if horiz < 0.2 {
            LocomotionMode::Idle
        } else if horiz < motor.walk_speed * 1.4 {
            LocomotionMode::Walk
        } else if horiz < motor.run_speed * 1.1 {
            LocomotionMode::Run
        } else {
            LocomotionMode::Sprint
        };
        let facing = t.forward();
        if let Some(mut a) = anim {
            a.set("speed", horiz);
            a.set("crouch", if intent.crouch { 1.0 } else { 0.0 });
            let dir = if horiz > 0.1 {
                let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
                let v = Vec3::new(vel.x, 0.0, vel.z) / horiz;
                f.cross(v).y.atan2(f.dot(v))
            } else {
                0.0
            };
            a.set("direction", dir);
        }
        // ---------------------------------------------------------------- footsteps
        let foot = foot.copied().unwrap_or_default();
        mover.phase = (mover.phase + horiz * dt / foot.stride.max(0.1) * PI) % TAU;
        if foot.kind == FootKind::None || !grounded {
            continue;
        }
        mover.stride += horiz * dt;
        let stride = foot.stride * (1.0 + horiz / 6.0);
        if mover.stride >= stride {
            mover.stride = 0.0;
            mover.left = !mover.left;
            let right = Vec3::new(-facing.z, 0.0, facing.x).normalize_or_zero();
            let side = if mover.left { -0.12 } else { 0.12 };
            contacts.write(GroundContact {
                position: new_pos + (right * side).as_dvec3(),
                normal: Vec3::Y,
                size: foot.size,
                yaw: (-facing.x).atan2(-facing.z),
                load_kg: foot.mass_kg
                    * if horiz > motor.run_speed * 0.8 {
                        1.6
                    } else {
                        1.0
                    },
                speed: horiz,
                kind: match foot.kind {
                    FootKind::Foot | FootKind::None => ContactKind::Foot,
                    FootKind::Paw => ContactKind::Paw,
                    FootKind::Hoof => ContactKind::Hoof,
                },
                surface: None,
                entity: Some(e),
            });
        }
    }
}

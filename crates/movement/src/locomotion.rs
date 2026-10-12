//! The locomotion step: turns each character's [`MoveIntent`] into motion every fixed step.
//!
//! Runs in `FixedUpdate` / `SimSet::Gameplay`, before physics. It reads what the physics
//! backend reported last step ([`CharacterState`]: ground, walls, ceiling), probes the world
//! with [`PhysicsQueries`] (walls to run on, ledges to climb), picks a mode (run, sprint,
//! slide, air, wall-run, mantle, dash) and writes the velocity the character controller should
//! move with ([`CharacterMove`]). Gravity is applied here, as the physics contract asks.
//!
//! Every character runs on its own clock: `dt = time.delta_for(LocalTimeScale)`, so slow
//! motion, fast elites and a player who ignores slow motion all work without special cases.

use std::f32::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::transform::look_rotation;
use engine_world::animation::{AnimationParams, AnimationTrigger, SkeletonPose};
use engine_world::locomotion::{LocomotionMode, LocomotionState, MoveIntent};
use engine_world::physics::{
    CharacterController, CharacterMove, CharacterState, Hit, PhysicsQueries, QueryFilter,
};
use engine_world::{ContactKind, Gravity, GroundContact, NoiseEvent};

use crate::mover::{Dash, DashRequest, Mantle, Mover, WallContact, flat, move_towards};
use crate::profile::MovementProfile;

/// Walls are surfaces whose normal is within this of horizontal (|n·up| below it).
const WALL_MAX_UP: f32 = 0.35;
/// Ground velocity pushes into the ground this hard so contact never flickers, m/s.
const GROUND_STICK: f32 = 0.5;
/// Velocity into a wall while wall-running, m/s.
const WALL_STICK: f32 = 1.5;
/// Safety limit on a mantle, s.
const MANTLE_MAX_SECS: f32 = 1.2;

/// Messages written by the step.
#[derive(SystemParam)]
pub(crate) struct Outputs<'w> {
    triggers: MessageWriter<'w, AnimationTrigger>,
    contacts: MessageWriter<'w, GroundContact>,
    noise: MessageWriter<'w, NoiseEvent>,
}

impl Outputs<'_> {
    fn trigger(&mut self, entity: Entity, name: &str) {
        self.triggers.write(AnimationTrigger {
            entity,
            name: name.to_string(),
        });
    }

    fn noise(&mut self, entity: Entity, position: DVec3, radius: f32) {
        self.noise.write(NoiseEvent {
            position,
            radius,
            source: Some(entity),
        });
    }
}

/// Scene probes for one character (walls, ledges, ground below).
struct Probe<'a> {
    queries: &'a PhysicsQueries,
    filter: &'a QueryFilter,
    /// Character entities: never wall-run on or climb another character.
    movers: &'a Query<'a, 'a, (), With<Mover>>,
}

impl Probe<'_> {
    fn ray(&self, origin: DVec3, dir: Vec3, max: f32) -> Option<Hit> {
        if max <= 0.0 {
            return None;
        }
        self.queries
            .raycast(origin, dir, max, self.filter)
            .filter(|h| !self.movers.contains(h.entity))
    }

    /// A runnable wall along `dir` within `max` meters.
    fn wall(&self, origin: DVec3, dir: Vec3, max: f32, up: Vec3) -> Option<WallContact> {
        let hit = self.ray(origin, dir, max)?;
        (hit.normal.dot(up).abs() < WALL_MAX_UP && hit.normal.dot(dir) < -0.2).then(|| {
            WallContact {
                normal: flat(hit.normal, up).normalize_or_zero(),
                point: hit.point,
                entity: Some(hit.entity),
            }
        })
    }
}

/// Everything about one character the step needs, gathered once.
struct Body {
    entity: Entity,
    pos: DVec3,
    up: Vec3,
    /// Gravity magnitude, m/s².
    g: f32,
    radius: f32,
    height: f32,
}

impl Body {
    fn feet(&self) -> DVec3 {
        self.pos - (self.up * self.height * 0.5).as_dvec3()
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn step_movers(
    time: Res<Time>,
    gravity: Res<Gravity>,
    queries: Res<PhysicsQueries>,
    mut out: Outputs,
    mut filter: Local<QueryFilter>,
    movers: Query<(), With<Mover>>,
    mut q: Query<(
        Entity,
        &mut Transform,
        &mut Mover,
        &MovementProfile,
        &MoveIntent,
        &mut CharacterMove,
        &CharacterState,
        &mut CharacterController,
        &mut LocomotionState,
        &mut AnimationParams,
        Option<&LocalTimeScale>,
        Has<SkeletonPose>,
    )>,
) {
    let world_dt = time.delta_secs();
    for (
        entity,
        mut transform,
        mut m,
        p,
        intent,
        mut mv,
        state,
        mut cc,
        mut loco,
        mut params,
        local,
        animated,
    ) in &mut q
    {
        let dt = time.delta_for(local);
        if dt <= 0.0 {
            mv.velocity = Vec3::ZERO;
            continue;
        }
        let pos = transform.translation;
        let up = gravity.up_at(pos);
        let body = Body {
            entity,
            pos,
            up,
            g: gravity.at(pos).length(),
            radius: cc.radius,
            height: cc.height,
        };
        m.up = up;
        if !m.initialized {
            m.initialized = true;
            m.stand_height = cc.height;
            m.dash_charges = p.dash_charges as f32;
            m.air_jumps_left = p.air_jumps;
            if let Some(f) = flat(transform.forward(), up).try_normalize() {
                m.facing = f;
            }
        }
        filter.exclude.clear();
        filter.exclude.push(entity);
        let probe = Probe {
            queries: &queries,
            filter: &filter,
            movers: &movers,
        };

        let v = step_one(&mut m, p, intent, state, &body, &probe, &mut out, dt);

        // Facing: combat target, then where the intent looks, then the motion.
        let face = m
            .influence
            .face
            .or(intent.look)
            .map(|l| flat(l, up))
            .filter(|l| l.length() > 0.1)
            .or_else(|| match m.mode {
                LocomotionMode::Dash => m.dash.map(|d| flat(d.dir, up)),
                LocomotionMode::Mantle => m.mantle.map(|mt| mt.forward),
                _ => {
                    let h = flat(v, up);
                    (h.length() > 0.5).then_some(h)
                }
            });
        if let Some(f) = face.and_then(|f| f.try_normalize()) {
            let snap = matches!(m.mode, LocomotionMode::Dash) || m.influence.face.is_some();
            m.facing = if snap {
                f
            } else {
                turn_towards(m.facing, f, p.turn_rate * dt)
            };
        }
        if let Some(r) = look_rotation(m.facing, up) {
            transform.rotation = r;
        }

        resize_capsule(
            &mut m,
            p,
            intent.crouch,
            &mut cc,
            &mut transform,
            &probe,
            &body,
        );
        feedback(&mut m, p, &body, v, dt, animated, &mut out);

        m.velocity = v;
        mv.velocity = v;
        let rate = if world_dt > 0.0 { dt / world_dt } else { 1.0 };
        loco.mode = m.mode;
        loco.velocity = v * rate;
        loco.grounded = m.grounded;
        loco.stamina = (m.dash_charges / p.dash_charges.max(1) as f32).clamp(0.0, 1.0);
        write_params(&m, &mut params, up);
    }
}

/// One character, one step. Returns the velocity to move with (own time, m/s).
#[allow(clippy::too_many_arguments)]
fn step_one(
    m: &mut Mover,
    p: &MovementProfile,
    intent: &MoveIntent,
    state: &CharacterState,
    b: &Body,
    probe: &Probe,
    out: &mut Outputs,
    dt: f32,
) -> Vec3 {
    let up = b.up;

    // ---- what physics reported last step
    let max_slope_cos = 0.64; // ≈ 50°; the controller decides walkability, this filters walls
    let leaving = m.velocity.dot(state.ground_normal) > 1.0;
    let grounded = state.grounded && state.ground_normal.dot(up) >= max_slope_cos && !leaving;
    let landed = grounded && !m.grounded && m.air_secs > 0.05;
    if landed {
        land(m, p, b, out);
    }
    m.grounded = grounded;
    m.ground_normal = if grounded { state.ground_normal } else { up };

    let mut v = m.velocity + m.knockback;
    m.knockback = Vec3::ZERO;
    if grounded {
        let n = state.ground_normal;
        let into = v.dot(n);
        if into < 0.0 {
            v -= n * into;
        }
    }
    if let Some(n) = state.wall_normal
        && n.dot(up).abs() < 0.7
    {
        let into = v.dot(n);
        if into < 0.0 {
            v -= n * into;
        }
    }
    if state.ceiling {
        let u = v.dot(up);
        if u > 0.0 {
            v -= up * u;
        }
    }

    // ---- timers and edge-triggered intents
    m.mode_secs += dt;
    let jump_pressed = intent.jump && !m.prev_jump;
    m.prev_jump = intent.jump;
    m.jump_buffer = if jump_pressed {
        p.jump_buffer_secs
    } else {
        (m.jump_buffer - dt).max(0.0)
    };
    let crouch_pressed = intent.crouch && !m.prev_crouch;
    m.prev_crouch = intent.crouch;
    let dash_pressed = intent.dash_to.is_some() && !m.prev_dash;
    m.prev_dash = intent.dash_to.is_some();
    m.slide_cooldown = (m.slide_cooldown - dt).max(0.0);
    m.evade_secs = (m.evade_secs - dt).max(0.0);
    m.dash_charges =
        (m.dash_charges + dt / p.dash_recharge_secs.max(0.01)).min(p.dash_charges as f32);
    if grounded {
        m.coyote = p.coyote_secs;
        m.air_secs = 0.0;
        m.fall_speed = 0.0;
        m.air_jumps_left = p.air_jumps;
        m.last_wall = None;
    } else {
        m.coyote = (m.coyote - dt).max(0.0);
        m.air_secs += dt;
        m.fall_speed = m.fall_speed.max(-v.dot(up));
    }

    if m.disabled {
        m.dash = None;
        m.mantle = None;
        m.wall = None;
        let vh = flat(v, up);
        let vv = v.dot(up);
        return if grounded {
            m.set_mode(LocomotionMode::Idle);
            move_towards(vh, Vec3::ZERO, p.ground_brake * dt) - up * GROUND_STICK
        } else {
            m.set_mode(LocomotionMode::Air);
            vh + up * (vv - b.g * dt).max(-p.max_fall_speed)
        };
    }

    let mut wish = flat(intent.direction, up);
    if wish.length() > 1.0 {
        wish = wish.normalize();
    }
    wish *= m.influence.move_scale;

    // ---- dash (from intent or from combat)
    let request = m.pending_dash.take().or_else(|| {
        dash_pressed
            .then_some(intent.dash_to)
            .flatten()
            .map(|to| DashRequest {
                to,
                strike: false,
                max_distance: None,
            })
    });
    if let Some(req) = request
        && m.mantle.is_none()
        && m.dash_charges >= 1.0
    {
        start_dash(m, p, req, b, v, out);
    }
    if let Some(mut d) = m.dash {
        let blocked = m.mode_secs > dt
            && state
                .wall_normal
                .is_some_and(|n| n.dot(d.dir) < -0.7 && n.dot(up).abs() < 0.7);
        if d.secs_left > 0.0 && !blocked {
            let step = d.secs_left.min(dt);
            d.secs_left -= dt;
            m.dash = Some(d);
            m.set_mode(LocomotionMode::Dash);
            return d.dir * d.speed * (step / dt);
        }
        // Dash over: keep its momentum (or plant for a strike).
        m.dash = None;
        let exit = flat(d.dir, up).normalize_or_zero() * d.exit_speed;
        v = if grounded {
            exit - up * GROUND_STICK
        } else {
            exit
        };
        m.set_mode(if grounded {
            LocomotionMode::Run
        } else {
            LocomotionMode::Air
        });
        return v;
    }

    // ---- mantle in progress
    if let Some(mut mt) = m.mantle {
        let feet_above = (b.feet() - mt.top).dot(up.as_dvec3()) as f32;
        if mt.rising && feet_above >= 0.06 {
            mt.rising = false;
        }
        let past_edge = (b.pos - mt.top).dot(mt.forward.as_dvec3()) as f32;
        let done = (!mt.rising && past_edge >= b.radius * 0.5) || m.mode_secs > MANTLE_MAX_SECS;
        if !done {
            m.mantle = Some(mt);
            m.set_mode(LocomotionMode::Mantle);
            return if mt.rising {
                up * p.mantle_speed + mt.forward * 0.5
            } else {
                mt.forward * mt.exit_speed.max(3.5)
            };
        }
        m.mantle = None;
        m.set_mode(LocomotionMode::Run);
        return mt.forward * mt.exit_speed - up * GROUND_STICK;
    }

    if grounded {
        if m.mode == LocomotionMode::WallRun {
            m.wall = None;
        }
        // ---- jump (also out of a slide: the slide-hop keeps all momentum)
        if m.jump_buffer > 0.0 {
            m.jump_buffer = 0.0;
            m.coyote = 0.0;
            m.set_mode(LocomotionMode::Air);
            out.trigger(b.entity, "jump");
            let launch = MovementProfile::launch_speed(p.jump_height, b.g);
            return flat(v, up) + up * launch;
        }
        // ---- slide
        let speed = flat(v, up).length();
        if m.mode == LocomotionMode::Slide {
            if intent.crouch && speed >= p.slide_end_speed {
                return slide(m, p, b, v, wish, state.ground_normal, dt);
            }
            m.set_mode(if intent.crouch {
                LocomotionMode::Crouch
            } else {
                LocomotionMode::Run
            });
        } else if (crouch_pressed || (landed && intent.crouch)) && speed >= p.slide_min_speed {
            let boost = if m.slide_cooldown <= 0.0 {
                m.slide_cooldown = p.slide_boost_cooldown;
                p.slide_boost
            } else {
                0.0
            };
            let dir = flat(v, up).normalize_or_zero();
            v = dir * (speed + boost).min(p.slide_max_speed.max(speed));
            m.set_mode(LocomotionMode::Slide);
            out.trigger(b.entity, "slide");
            out.noise(b.entity, b.feet(), 6.0);
            return on_ground(v, up, state.ground_normal);
        }
        return ground_move(m, p, intent, b, v, wish, state.ground_normal, dt);
    }

    // ---- airborne
    if m.mode == LocomotionMode::WallRun
        && let Some(wall) = m.wall
    {
        return wall_run(
            m,
            p,
            b,
            probe,
            v,
            wish,
            wall,
            jump_pressed,
            crouch_pressed,
            dt,
            out,
        );
    }
    m.wall = None;

    if m.jump_buffer > 0.0 && m.coyote > 0.0 {
        // Coyote jump: just ran off a ledge.
        m.jump_buffer = 0.0;
        m.coyote = 0.0;
        m.set_mode(LocomotionMode::Air);
        out.trigger(b.entity, "jump");
        return flat(v, up) + up * MovementProfile::launch_speed(p.jump_height, b.g);
    }
    if jump_pressed && m.air_jumps_left > 0 {
        m.air_jumps_left -= 1;
        m.jump_buffer = 0.0;
        let mut vh = flat(v, up);
        if wish.length() > 0.1 {
            let speed = vh.length().max(p.run_speed * wish.length());
            let toward = wish.normalize() * speed;
            vh = vh.lerp(toward, p.air_jump_redirect.clamp(0.0, 1.0));
        }
        let launch = MovementProfile::launch_speed(p.air_jump_height, b.g);
        m.set_mode(LocomotionMode::Air);
        out.trigger(b.entity, "double_jump");
        out.noise(b.entity, b.pos, 5.0);
        return vh + up * launch.max(v.dot(up));
    }
    if p.mantle
        && let Some(mt) = find_ledge(p, b, probe, v, wish)
    {
        m.mantle = Some(mt);
        m.set_mode(LocomotionMode::Mantle);
        out.trigger(b.entity, "mantle");
        return up * p.mantle_speed;
    }
    if p.wallrun
        && let Some(wall) = find_wall_to_run(m, p, b, probe, state, v, wish)
    {
        m.wall = Some(wall);
        m.set_mode(LocomotionMode::WallRun);
        m.air_jumps_left = p.air_jumps;
        out.trigger(b.entity, "wallrun");
        let vh = flat(v, up);
        let along = vh - wall.normal * vh.dot(wall.normal);
        let lift = v.dot(up).clamp(0.0, 3.0).max(p.wallrun_lift);
        return along + up * lift - wall.normal * WALL_STICK;
    }

    // Air control (accelerates only up to `air_speed` along the stick: momentum is never lost).
    let mut vh = flat(v, up);
    if let Some(dir) = wish.try_normalize() {
        let current = vh.dot(dir);
        let add = (p.air_speed * wish.length() - current).clamp(0.0, p.air_accel * dt);
        vh += dir * add;
    }
    let vv = (v.dot(up) - b.g * dt).max(-p.max_fall_speed);
    m.set_mode(LocomotionMode::Air);
    vh + up * vv
}

fn start_dash(
    m: &mut Mover,
    p: &MovementProfile,
    req: DashRequest,
    b: &Body,
    v: Vec3,
    out: &mut Outputs,
) {
    let mut d = (req.to - b.pos).as_vec3();
    if m.grounded {
        d = flat(d, b.up);
    }
    let len = d.length();
    if len < 1e-3 {
        return;
    }
    let stop = if req.strike {
        p.dash_stop_distance
    } else {
        0.0
    };
    let dist = (len - stop).min(req.max_distance.unwrap_or(p.dash_distance));
    if dist < 0.2 {
        return;
    }
    let speed = p.dash_speed();
    let exit_speed = if req.strike {
        0.0
    } else {
        p.dash_exit_speed.max(flat(v, b.up).length())
    };
    m.dash = Some(Dash {
        dir: d / len,
        speed,
        secs_left: dist / speed,
        strike: req.strike,
        exit_speed,
    });
    m.dash_charges = (m.dash_charges - 1.0).max(0.0);
    m.evade_secs = if req.strike { 0.0 } else { p.dash_iframe_secs };
    m.wall = None;
    m.mantle = None;
    m.set_mode(LocomotionMode::Dash);
    m.mode_secs = 0.0;
    out.trigger(b.entity, "dash");
    out.noise(b.entity, b.pos, 12.0);
}

fn land(m: &mut Mover, p: &MovementProfile, b: &Body, out: &mut Outputs) {
    let impact = m.fall_speed;
    if m.air_secs > 0.2 {
        out.trigger(b.entity, "land");
    }
    let feet = b.feet();
    let right = m.facing.cross(b.up).as_dvec3() * 0.12;
    let yaw = m.facing.x.atan2(m.facing.z);
    for side in [-1.0, 1.0] {
        out.contacts.write(GroundContact {
            position: feet + right * side,
            normal: b.up,
            size: [0.11, 0.28],
            yaw,
            // A hard landing presses in deeper: weight × (1 + v / 4 m/s).
            load_kg: p.mass_kg * 0.5 * (1.0 + impact / 4.0),
            speed: m.velocity.length(),
            kind: ContactKind::Foot,
            surface: None,
            entity: Some(b.entity),
        });
    }
    out.noise(b.entity, feet, 4.0 + impact * 1.5);
}

fn on_ground(v: Vec3, up: Vec3, n: Vec3) -> Vec3 {
    // Keep the horizontal velocity and add the vertical part that follows the slope.
    let vh = flat(v, up);
    let ndu = n.dot(up).max(0.2);
    vh - up * (vh.dot(n) / ndu) - n * GROUND_STICK
}

#[allow(clippy::too_many_arguments)]
fn ground_move(
    m: &mut Mover,
    p: &MovementProfile,
    intent: &MoveIntent,
    b: &Body,
    v: Vec3,
    wish: Vec3,
    n: Vec3,
    dt: f32,
) -> Vec3 {
    let up = b.up;
    let crouching = intent.crouch || m.low_ceiling;
    let base = if crouching {
        p.crouch_speed
    } else if intent.sprint {
        p.sprint_speed
    } else {
        p.run_speed
    };
    let target = wish * base;
    let mut vh = flat(v, up);
    if let Some(lunge) = m.influence.lunge {
        vh = flat(lunge, up);
    } else if vh.length() > base + 0.1 && target.length() > 0.01 && vh.dot(target) > 0.0 {
        // Faster than wanted (out of a slide, dash or hop): steer, but bleed speed slowly.
        let speed = (vh.length() - p.overspeed_brake * dt).max(base);
        let dir = turn_towards(vh.normalize(), target.normalize(), 6.0 * dt);
        vh = dir * speed;
    } else {
        let accel = if target.length() < 0.01 {
            p.ground_brake
        } else {
            p.ground_accel
        };
        vh = move_towards(vh, target, accel * dt);
    }
    let speed = vh.length();
    m.set_mode(if crouching {
        LocomotionMode::Crouch
    } else if speed < 0.3 {
        LocomotionMode::Idle
    } else if speed > p.run_speed + 0.3 {
        LocomotionMode::Sprint
    } else if speed > p.run_speed * 0.55 {
        LocomotionMode::Run
    } else {
        LocomotionMode::Walk
    });
    on_ground(vh, up, n)
}

fn slide(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    v: Vec3,
    wish: Vec3,
    n: Vec3,
    dt: f32,
) -> Vec3 {
    let up = b.up;
    // Downhill pull: gravity along the ground plane, flattened.
    let g = -up * b.g;
    let along_plane = g - n * g.dot(n);
    let mut vh = flat(v, up) + flat(along_plane, up) * dt;
    let speed = (vh.length() - p.slide_friction * dt).clamp(0.0, p.slide_max_speed);
    let mut dir = vh.normalize_or_zero();
    if let Some(w) = wish.try_normalize()
        && dir != Vec3::ZERO
    {
        dir = turn_towards(dir, w, p.slide_steer * dt);
    }
    vh = dir * speed;
    m.set_mode(LocomotionMode::Slide);
    on_ground(vh, up, n)
}

#[allow(clippy::too_many_arguments)]
fn wall_run(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    wall: WallContact,
    jump_pressed: bool,
    crouch_pressed: bool,
    dt: f32,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let reach = b.radius + p.wall_reach + 0.25;
    let detach = |m: &mut Mover, v: Vec3, push: f32| {
        m.last_wall = m.wall.take();
        m.set_mode(LocomotionMode::Air);
        v + wall.normal * push
    };
    let Some(wall) = probe.wall(b.pos, -wall.normal, reach, up) else {
        // Ran off the end of the wall: keep flying.
        return detach(m, v, 0.0);
    };
    m.wall = Some(wall);
    let vh = flat(v, up);
    let tangent = vh - wall.normal * vh.dot(wall.normal);
    let tdir = tangent.try_normalize().unwrap_or_else(|| {
        let t = wall.normal.cross(up);
        if t.dot(m.facing) >= 0.0 { t } else { -t }
    });

    if jump_pressed {
        let mut out_v = tdir * tangent.length().max(p.run_speed) + wall.normal * p.wall_jump_push;
        if let Some(w) = wish.try_normalize()
            && w.dot(wall.normal) > -0.2
        {
            let speed = out_v.length();
            out_v = (out_v.normalize() + w).normalize_or_zero() * speed;
        }
        m.last_wall = m.wall.take();
        m.jump_buffer = 0.0;
        m.set_mode(LocomotionMode::Air);
        out.trigger(b.entity, "wall_jump");
        out.noise(b.entity, b.pos, 6.0);
        return out_v + up * MovementProfile::launch_speed(p.wall_jump_height, b.g);
    }
    if crouch_pressed {
        return detach(m, v, 2.0);
    }
    if m.mode_secs >= p.wallrun_max_secs || tangent.length() < p.wallrun_min_speed * 0.5 {
        return detach(m, v, 1.5);
    }
    // Wall-run into a ledge: climb it.
    if p.mantle
        && let Some(mt) = find_ledge(p, b, probe, v, tdir)
    {
        m.wall = None;
        m.mantle = Some(mt);
        m.set_mode(LocomotionMode::Mantle);
        out.trigger(b.entity, "mantle");
        return up * p.mantle_speed;
    }
    let along = tangent.length();
    let along = if along < p.wallrun_speed {
        (along + p.wallrun_accel * dt).min(p.wallrun_speed)
    } else {
        (along - p.overspeed_brake * dt).max(p.wallrun_speed)
    };
    let half = p.wallrun_max_secs * 0.45;
    let k = smooth01((m.mode_secs - half) / (p.wallrun_max_secs - half).max(1e-3));
    let gscale = p.wallrun_gravity + (1.0 - p.wallrun_gravity) * k;
    let vv = (v.dot(up) - b.g * gscale * dt).max(-6.0);
    m.set_mode(LocomotionMode::WallRun);
    tdir * along + up * vv - wall.normal * WALL_STICK
}

fn find_wall_to_run(
    m: &Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    state: &CharacterState,
    v: Vec3,
    wish: Vec3,
) -> Option<WallContact> {
    let up = b.up;
    let vh = flat(v, up);
    let speed = vh.length();
    if speed < p.wallrun_min_speed || v.dot(up) < -10.0 {
        return None;
    }
    // High enough above the ground?
    if probe.ray(b.feet(), -up, p.wallrun_min_height).is_some() {
        return None;
    }
    let fwd = vh / speed;
    let right = fwd.cross(up);
    let reach = b.radius + p.wall_reach;
    let mut best: Option<(f64, WallContact)> = None;
    if let Some(n) = state.wall_normal
        && n.dot(up).abs() < WALL_MAX_UP
    {
        let n = flat(n, up).normalize_or_zero();
        best = Some((
            0.0,
            WallContact {
                normal: n,
                point: b.pos - (n * b.radius).as_dvec3(),
                entity: None,
            },
        ));
    }
    for dir in [
        right,
        -right,
        (fwd + right).normalize(),
        (fwd - right).normalize(),
    ] {
        if let Some(w) = probe.wall(b.pos, dir, reach, up) {
            let d = w.point.distance(b.pos);
            if best
                .as_ref()
                .is_none_or(|(bd, bw)| bw.entity.is_none() || d < *bd)
            {
                best = Some((d, w));
            }
        }
    }
    let (_, wall) = best?;
    if let Some(last) = m.last_wall
        && last.normal.dot(wall.normal) > 0.85
        && (last.entity.is_none() || wall.entity.is_none() || last.entity == wall.entity)
    {
        return None; // the wall we just jumped off
    }
    let along = (vh - wall.normal * vh.dot(wall.normal)).length();
    if along < p.wallrun_min_speed * 0.8 || wish.dot(wall.normal) > 0.5 {
        return None;
    }
    Some(wall)
}

/// A ledge in front of the character that it can climb onto.
fn find_ledge(p: &MovementProfile, b: &Body, probe: &Probe, v: Vec3, wish: Vec3) -> Option<Mantle> {
    let up = b.up;
    let vh = flat(v, up);
    let dir = wish
        .try_normalize()
        .filter(|_| wish.length() > 0.3)
        .or_else(|| vh.try_normalize())?;
    if vh.dot(dir) < -0.5 {
        return None;
    }
    let upd = up.as_dvec3();
    let feet = b.feet();
    let low = feet + upd * (p.mantle_min_height as f64 + 0.05);
    let hit = probe.ray(low, dir, b.radius + 0.5)?;
    if hit.normal.dot(up).abs() >= WALL_MAX_UP || hit.normal.dot(dir) > -0.5 {
        return None;
    }
    let inward = flat(-hit.normal, up).normalize_or_zero();
    let over = hit.point + (inward * 0.3).as_dvec3();
    let top_y = feet + upd * (p.mantle_max_height as f64 + 0.15);
    let start = over + upd * (top_y - over).dot(upd);
    let down = probe.ray(start, -up, p.mantle_max_height - p.mantle_min_height + 0.15)?;
    if down.normal.dot(up) < 0.7 || down.distance < 0.01 {
        return None;
    }
    let ledge = (down.point - feet).dot(upd) as f32;
    if ledge < p.mantle_min_height || ledge > p.mantle_max_height {
        return None;
    }
    // Room to rise (head) and to stand on the ledge.
    if probe
        .ray(b.pos, up, b.height * 0.5 + ledge + 0.05)
        .is_some()
    {
        return None;
    }
    let stand = feet + upd * (ledge as f64 + b.radius as f64 + 0.3);
    if probe.ray(stand, inward, b.radius + 0.9).is_some() {
        return None;
    }
    let edge = hit.point + upd * (down.point - hit.point).dot(upd);
    Some(Mantle {
        top: edge,
        forward: inward,
        rising: true,
        exit_speed: vh.length().max(p.run_speed * 0.8),
    })
}

fn resize_capsule(
    m: &mut Mover,
    p: &MovementProfile,
    crouch: bool,
    cc: &mut CharacterController,
    transform: &mut Transform,
    probe: &Probe,
    b: &Body,
) {
    if p.crouch_height <= 0.0 || p.crouch_height >= m.stand_height {
        return;
    }
    let want = m.mode == LocomotionMode::Slide || (crouch && m.grounded);
    let upd = b.up.as_dvec3();
    if want && !m.crouched {
        let dh = m.stand_height - p.crouch_height;
        cc.height = p.crouch_height;
        if m.grounded {
            transform.translation -= upd * (dh as f64 * 0.5);
        }
        m.crouched = true;
    } else if !want && m.crouched {
        let dh = m.stand_height - cc.height;
        // Stand only with head room.
        if probe
            .ray(b.pos, b.up, cc.height * 0.5 + dh + 0.02)
            .is_none()
        {
            cc.height = m.stand_height;
            if m.grounded {
                transform.translation += upd * (dh as f64 * 0.5);
            }
            m.crouched = false;
            m.low_ceiling = false;
        } else {
            m.low_ceiling = true;
            if m.grounded {
                m.set_mode(LocomotionMode::Crouch);
            }
        }
    }
}

/// Footsteps, slide scrapes and wall-run steps (when no animation module emits them).
fn feedback(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    v: Vec3,
    dt: f32,
    animated: bool,
    out: &mut Outputs,
) {
    let up = b.up;
    let speed = flat(v, up).length();
    let (kind, every, size, on_wall) = match m.mode {
        LocomotionMode::Slide => (ContactKind::Body, 0.4, [0.4, 0.9], false),
        LocomotionMode::WallRun => (ContactKind::Foot, 0.9 + 0.1 * speed, [0.11, 0.28], true),
        LocomotionMode::Walk
        | LocomotionMode::Run
        | LocomotionMode::Sprint
        | LocomotionMode::Crouch
            if m.grounded =>
        {
            (ContactKind::Foot, 0.7 + 0.11 * speed, [0.11, 0.28], false)
        }
        _ => {
            m.stride = 0.0;
            return;
        }
    };
    if kind == ContactKind::Foot && (animated || !p.stride_footsteps) {
        return;
    }
    m.stride += speed * dt;
    if m.stride < every {
        return;
    }
    m.stride -= every;
    m.left_foot = !m.left_foot;
    let side = if m.left_foot { -1.0 } else { 1.0 };
    let right = m.facing.cross(up);
    let (position, normal) = match (on_wall, &m.wall) {
        (true, Some(w)) => (
            w.point + (up * (0.4 * side)).as_dvec3() - (b.up * b.height * 0.3).as_dvec3(),
            w.normal,
        ),
        _ => (b.feet() + (right * 0.12 * side).as_dvec3(), up),
    };
    out.contacts.write(GroundContact {
        position,
        normal,
        size,
        yaw: m.facing.x.atan2(m.facing.z),
        load_kg: p.mass_kg,
        speed,
        kind,
        surface: None,
        entity: Some(b.entity),
    });
    let quiet = if m.mode == LocomotionMode::Crouch {
        0.35
    } else {
        1.0
    };
    out.noise(b.entity, position, (2.0 + speed * 1.2) * quiet);
}

fn write_params(m: &Mover, params: &mut AnimationParams, up: Vec3) {
    let vh = flat(m.velocity, up);
    let speed = vh.length();
    let direction = if speed > 0.1 {
        let local = Quat::from_rotation_arc(m.facing, Vec3::NEG_Z);
        let d = local * vh;
        d.x.atan2(-d.z)
    } else {
        0.0
    };
    params.set("speed", speed);
    params.set("direction", direction);
    params.set("lean", m.wall_side());
    params.set(
        "crouch",
        if matches!(m.mode, LocomotionMode::Slide | LocomotionMode::Crouch) {
            1.0
        } else {
            0.0
        },
    );
    params.set(
        "effort",
        match m.mode {
            LocomotionMode::Dash | LocomotionMode::Mantle => 1.0,
            LocomotionMode::Sprint | LocomotionMode::WallRun => 0.8,
            LocomotionMode::Run => 0.5,
            _ => 0.2,
        },
    );
    params.set("airborne", if m.grounded { 0.0 } else { 1.0 });
    params.set("vertical_speed", m.velocity.dot(up));
    params.set(
        "wallrun",
        if m.mode == LocomotionMode::WallRun {
            1.0
        } else {
            0.0
        },
    );
}

/// Rotate unit vector `from` toward `to` by at most `max_angle` radians.
pub(crate) fn turn_towards(from: Vec3, to: Vec3, max_angle: f32) -> Vec3 {
    let (Some(a), Some(b)) = (from.try_normalize(), to.try_normalize()) else {
        return to.try_normalize().unwrap_or(from);
    };
    let angle = a.dot(b).clamp(-1.0, 1.0).acos();
    if angle <= max_angle || angle < 1e-4 {
        return b;
    }
    if angle > PI - 1e-3 {
        // Opposite: any perpendicular axis works; turn around "up".
        return Quat::from_rotation_y(max_angle) * a;
    }
    let axis = a.cross(b).normalize();
    Quat::from_axis_angle(axis, max_angle) * a
}

fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_is_rate_limited() {
        let v = turn_towards(Vec3::X, Vec3::Z, 0.1);
        assert!((v.angle_between(Vec3::X) - 0.1).abs() < 1e-4);
        assert_eq!(turn_towards(Vec3::X, Vec3::Z, 3.0), Vec3::Z);
        let back = turn_towards(Vec3::X, -Vec3::X, 0.5);
        assert!((back.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ground_velocity_follows_slopes() {
        // 30° slope rising toward +X.
        let n = Vec3::new(-0.5, 0.866, 0.0).normalize();
        let v = on_ground(Vec3::new(5.0, 0.0, 0.0), Vec3::Y, n);
        // Uphill: the velocity climbs, parallel to the slope apart from the small stick.
        assert!(v.y > 2.0, "{v:?}");
        assert!((v + n * GROUND_STICK).dot(n).abs() < 1e-4);
    }
}

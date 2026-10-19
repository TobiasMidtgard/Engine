//! The locomotion step: turns each character's [`MoveIntent`] into motion every fixed step.
//!
//! Runs in `FixedUpdate` / `SimSet::Gameplay`, before physics. It reads what the physics
//! backend reported last step ([`CharacterState`]: ground, walls, ceiling), probes the world
//! with [`PhysicsQueries`] (walls to run on, ledges to climb), picks a mode (run, sprint,
//! slide, air, wall-run, mantle, ledge hang, ladder, zip line, dash) and writes the velocity the character controller should
//! move with ([`CharacterMove`]). Gravity is applied here, as the physics contract asks.
//!
//! Every character runs on its own clock: `dt = time.delta_for(LocalTimeScale)`, so slow
//! motion, fast elites and a player who ignores slow motion all work without special cases.

use std::f32::consts::PI;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::transform::look_rotation;
use engine_world::animation::{AnimationParams, AnimationTrigger, SkeletonPose};
use engine_world::locomotion::{ClimbLine, Climbable, LocomotionMode, LocomotionState, MoveIntent};
use engine_world::physics::{
    CharacterController, CharacterMove, CharacterState, Collider, Hit, PhysicsQueries, QueryFilter,
    Velocity,
};
use engine_world::{
    ContactKind, Gravity, Ground, GroundContact, NoiseEvent, SurfaceId, SurfaceMaterials,
};

use crate::grapple;
use crate::ladder;
use crate::mover::{
    Corner, Dash, DashRequest, Hang, Mantle, Mover, WallContact, flat, move_towards,
};
use crate::platform::{self, Deck, Pin};
use crate::profile::MovementProfile;
use crate::vault;
use crate::zipline::{self, Cable, Zipline};

/// Walls are surfaces whose normal is within this of horizontal (|n·up| below it).
const WALL_MAX_UP: f32 = 0.35;
/// Ground velocity pushes into the ground this hard so contact never flickers, m/s.
const GROUND_STICK: f32 = 0.5;
/// Velocity into a wall while wall-running, m/s.
const WALL_STICK: f32 = 1.5;
/// Safety limit on a mantle, s.
const MANTLE_MAX_SECS: f32 = 1.2;
/// Hanging, the ledge's top is this far above the top of the capsule (arms stretched), m.
const HANG_REACH: f32 = 0.3;
/// Gap between the capsule and the wall while hanging, m.
const HANG_GAP: f32 = 0.04;
/// Fastest pull toward the hanging position after a catch, m/s.
const HANG_PULL: f32 = 5.0;
/// Holding toward the wall this long while hanging climbs up, s.
const HANG_CLIMB_HOLD: f32 = 0.3;

/// Messages written by the step.
#[derive(SystemParam)]
pub(crate) struct Outputs<'w> {
    triggers: MessageWriter<'w, AnimationTrigger>,
    contacts: MessageWriter<'w, GroundContact>,
    noise: MessageWriter<'w, NoiseEvent>,
}

impl Outputs<'_> {
    pub(crate) fn trigger(&mut self, entity: Entity, name: &str) {
        self.triggers.write(AnimationTrigger {
            entity,
            name: name.to_string(),
        });
    }

    pub(crate) fn contact(&mut self, contact: GroundContact) {
        self.contacts.write(contact);
    }

    pub(crate) fn noise(&mut self, entity: Entity, position: DVec3, radius: f32) {
        self.noise.write(NoiseEvent {
            position,
            radius,
            source: Some(entity),
        });
    }
}

/// Everything but characters that a character can stand on, hold on to or hook: where it is
/// and how fast it moves, if it says.
pub(crate) type Bodies<'w, 's> =
    Query<'w, 's, (&'static Transform, Option<&'static Velocity>), Without<Mover>>;

/// Zip lines.
pub(crate) type Lines<'w, 's> = Query<'w, 's, (Entity, &'static Zipline)>;

/// Ladders: where they are (their own `Transform` when they have no parent, so a ladder moved
/// this step is where it is now) and what they are made of.
pub(crate) type Ladders<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Climbable,
        &'static Transform,
        &'static GlobalTransform,
        Option<&'static Collider>,
        Has<ChildOf>,
    ),
    Without<Mover>,
>;

/// Scene probes for one character (walls, ledges, ground below).
pub(crate) struct Probe<'a> {
    queries: &'a PhysicsQueries,
    filter: &'a QueryFilter,
    /// Character entities: never wall-run on or climb another character.
    movers: &'a Query<'a, 'a, (), With<Mover>>,
    bodies: &'a Bodies<'a, 'a>,
    lines: &'a Lines<'a, 'a>,
    ladders: &'a Ladders<'a, 'a>,
    /// The surface rungs ring as when a ladder has no collider to say.
    metal: Option<SurfaceId>,
    /// The character's seconds per world second (bodies move in world time).
    rate: f32,
}

impl Probe<'_> {
    /// How fast the point `at` of `entity` moves, if the body says (m/s in the character's
    /// own seconds).
    pub(crate) fn velocity_at(&self, entity: Entity, at: DVec3) -> Option<Vec3> {
        let (t, v) = self.bodies.get(entity).ok()?;
        let v = v?;
        Some((v.linear + v.angular.cross((at - t.translation).as_vec3())) / self.rate)
    }

    pub(crate) fn ray(&self, origin: DVec3, dir: Vec3, max: f32) -> Option<Hit> {
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

    /// The cable of the zip line `line`, sagging against `up`.
    pub(crate) fn cable(&self, line: Entity, up: Vec3) -> Option<Cable> {
        let (_, z) = self.lines.get(line).ok()?;
        let (t, _) = self.bodies.get(line).ok()?;
        Some(Cable::new(t, z, up))
    }

    /// Every zip line's cable.
    pub(crate) fn cables(&self, up: Vec3) -> impl Iterator<Item = (Entity, Cable)> + '_ {
        self.lines.iter().filter_map(move |(e, z)| {
            let (t, _) = self.bodies.get(e).ok()?;
            Some((e, Cable::new(t, z, up)))
        })
    }

    /// The climb line of the ladder `ladder`, in world space.
    pub(crate) fn climb_line(&self, ladder: Entity) -> Option<ClimbLine> {
        let (_, c, t, g, _, parented) = self.ladders.get(ladder).ok()?;
        Some(climb_line(c, t, g, parented))
    }

    /// Every ladder's climb line.
    pub(crate) fn climb_lines(&self) -> impl Iterator<Item = (Entity, ClimbLine)> + '_ {
        self.ladders
            .iter()
            .map(|(e, c, t, g, _, parented)| (e, climb_line(c, t, g, parented)))
    }

    /// What the ladder `ladder`'s rungs are made of: its collider's surface, else metal.
    pub(crate) fn ladder_surface(&self, ladder: Entity) -> Option<SurfaceId> {
        let (.., collider, _) = self.ladders.get(ladder).ok()?;
        collider
            .map(|c| c.surface)
            .filter(|&s| s != SurfaceId::DEFAULT)
            .or(self.metal)
    }

    /// Pins `point` and `dir` to the ladder `ladder` (one without a parent; a child's own
    /// `Transform` isn't where it is).
    pub(crate) fn ladder_pin(&self, ladder: Entity, point: DVec3, dir: Vec3) -> Option<Pin> {
        let (.., parented) = self.ladders.get(ladder).ok()?;
        if parented {
            return None;
        }
        self.pin(ladder, point, dir)
    }

    /// Pins `point` and `dir` to the body `entity`, so they move with it.
    pub(crate) fn pin(&self, entity: Entity, point: DVec3, dir: Vec3) -> Option<Pin> {
        let (t, _) = self.bodies.get(entity).ok()?;
        Some(Pin::new(entity, t, point, dir))
    }
}

/// A ladder's climb line in world space. A ladder without a parent goes by its `Transform`
/// (up to date within the fixed steps of a frame), a child by its `GlobalTransform`.
fn climb_line(c: &Climbable, t: &Transform, g: &GlobalTransform, parented: bool) -> ClimbLine {
    if parented {
        c.in_world(g)
    } else {
        c.in_world(&GlobalTransform::from(*t))
    }
}

/// Everything about one character the step needs, gathered once.
#[derive(Clone, Copy)]
pub(crate) struct Body {
    pub(crate) entity: Entity,
    pub(crate) pos: DVec3,
    pub(crate) up: Vec3,
    /// Gravity magnitude, m/s².
    pub(crate) g: f32,
    pub(crate) radius: f32,
    pub(crate) height: f32,
}

impl Body {
    pub(crate) fn feet(&self) -> DVec3 {
        self.pos - (self.up * self.height * 0.5).as_dvec3()
    }
}

/// Characters that ask to move but have no [`Mover`] yet.
type Unadopted = (With<MoveIntent>, With<CharacterController>, Without<Mover>);

/// The movement module is the installed [`LocomotionBackend`](engine_world::LocomotionBackend),
/// so it moves every character that asks to move, not only the ones spawned with a
/// [`MovementProfile`]. A body with a `MoveIntent` and a `CharacterController` but no profile
/// (an NPC from another module, say) gets [`MovementProfile::human`] and keeps its own capsule.
/// Bodies without a `CharacterController` (animals, birds, props) are left to whoever spawned
/// them.
pub(crate) fn adopt_characters(mut commands: Commands, q: Query<Entity, Unadopted>) {
    for e in &q {
        commands.entity(e).insert(MovementProfile::human());
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn step_movers(
    time: Res<Time>,
    gravity: Res<Gravity>,
    ground: Res<Ground>,
    queries: Res<PhysicsQueries>,
    mut out: Outputs,
    mut filter: Local<QueryFilter>,
    movers: Query<(), With<Mover>>,
    decks: Bodies,
    lines: Lines,
    ladders: Ladders,
    materials: Res<SurfaceMaterials>,
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
    let metal = materials.id("metal");
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
        let rate = if world_dt > 0.0 { dt / world_dt } else { 1.0 };
        let probe = Probe {
            queries: &queries,
            filter: &filter,
            movers: &movers,
            bodies: &decks,
            lines: &lines,
            ladders: &ladders,
            metal,
            rate,
        };

        follow_pins(&mut m, p, &decks, entity, dt, &mut out);
        // The deck: the ground under the feet (physics carries the character on it), else the
        // wall run along or the ledge held (movement carries the character with those).
        let deck = state
            .ground_entity
            .filter(|_| state.grounded)
            .map(|g| (g, true))
            .or_else(|| held(&m).map(|e| (e, false)))
            .and_then(|(g, carried)| decks.get(g).ok().map(|(t, vel)| (g, t, vel, carried)))
            .map(|(g, t, vel, carried)| Deck {
                entity: g,
                transform: t,
                // Per world second (decks run on world time); `rate` is this character's
                // seconds per world second.
                velocity: vel
                    .map(|v| (v.linear + v.angular.cross((pos - t.translation).as_vec3())) / rate),
                carried,
            });
        platform::ride(&mut m, deck, pos, dt);
        let (state, lift) = on_ground_surface(state, &ground, &body);

        let mut v = step_one(&mut m, p, intent, &state, &body, &probe, &mut out, dt);
        if m.grounded && m.settle > GROUND_STICK && v.dot(up) <= 0.0 {
            v -= up * (m.settle - GROUND_STICK);
        }

        // Facing: the wall hung from, the ladder, the way over a vaulted obstacle, the
        // grapple's hook, the zip line, the combat target, where the intent looks, the motion.
        let hook = m
            .grapple
            .and_then(|g| flat((g.anchor - pos).as_vec3(), up).try_normalize());
        let zip = m.zip.and_then(|z| flat(z.heading, up).try_normalize());
        let ladder = m.ladder.map(|l| l.facing);
        let vaulting = m.vault.map(|vt| vt.forward);
        let face = m
            .hang
            .map(|h| -h.normal)
            .or(ladder)
            .or(vaulting)
            .or(hook)
            .or(zip)
            .or_else(|| {
                m.influence
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
                    })
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
        // The deck's motion: physics carries a character standing on it, movement one on a
        // wall or a ledge.
        let deck = m.platform.map_or(Vec3::ZERO, |d| d.velocity);
        let carry = m
            .platform
            .filter(|d| !d.carried)
            .map_or(Vec3::ZERO, |d| d.velocity);
        // Back up onto the ground surface if the step sank into it.
        mv.velocity = v + carry + up * (lift / dt);
        loco.mode = m.mode;
        // The world's frame: the deck's motion plus the character's own on it.
        loco.velocity = (v + deck) * rate;
        loco.grounded = m.grounded;
        loco.stamina = (m.dash_charges / p.dash_charges.max(1) as f32).clamp(0.0, 1.0);
        write_params(&m, &mut params, up);
    }
}

/// The body the character holds on to off the ground: the wall it runs along, the ledge it
/// hangs from or climbs, the obstacle it vaults, the ladder it climbs.
fn held(m: &Mover) -> Option<Entity> {
    let on = |pin: Option<Pin>| pin.map(|p| p.entity);
    on(m.hang.and_then(|h| h.on))
        .or_else(|| on(m.mantle.and_then(|mt| mt.on)))
        .or_else(|| on(m.vault.and_then(|vt| vt.on)))
        .or_else(|| m.ladder.and_then(|l| l.entity))
        .or_else(|| {
            m.wall
                .filter(|_| m.mode == LocomotionMode::WallRun)
                .and_then(|w| w.entity)
        })
}

/// Moves what is pinned to bodies along with them: the ledge hung from or climbed, the top
/// of the obstacle vaulted and the grapple's hook. A hook whose body is gone lets go.
fn follow_pins(
    m: &mut Mover,
    p: &MovementProfile,
    bodies: &Bodies,
    entity: Entity,
    dt: f32,
    out: &mut Outputs,
) {
    let up = m.up;
    let at = |pin: Option<Pin>| {
        let pin = pin?;
        bodies.get(pin.entity).ok().map(|(t, _)| pin.place(t))
    };
    if let Some(h) = &mut m.hang
        && let Some((top, normal)) = at(h.on)
    {
        h.top = top;
        h.normal = flat(normal, up).try_normalize().unwrap_or(h.normal);
    }
    if let Some(mt) = &mut m.mantle
        && let Some((top, forward)) = at(mt.on)
    {
        mt.top = top;
        mt.forward = flat(forward, up).try_normalize().unwrap_or(mt.forward);
    }
    if let Some(vt) = &mut m.vault
        && let Some((plant, forward)) = at(vt.on)
    {
        vt.plant = plant;
        vt.forward = flat(forward, up).try_normalize().unwrap_or(vt.forward);
    }
    let Some(g) = &mut m.grapple else {
        return;
    };
    g.anchor_velocity = Vec3::ZERO;
    if g.on.is_none() {
        return;
    }
    match at(g.on) {
        Some((anchor, normal)) => {
            g.anchor_velocity = ((anchor - g.anchor) / dt as f64).as_vec3();
            g.anchor = anchor;
            g.normal = normal;
        }
        None => grapple::release(m, p, entity, out),
    }
}

/// How far above the [`Ground`] surface the feet still count as standing on it, m.
const GROUND_SNAP: f64 = 0.05;

/// Terrain without colliders: physics reports no ground, but the feet are on (or sank into)
/// the [`Ground`] surface, so the character stands on it. Returns the state to step with and
/// how far to lift the character back up, m. Physics backends that collide with the terrain
/// report the ground themselves and this never applies.
fn on_ground_surface(state: &CharacterState, ground: &Ground, b: &Body) -> (CharacterState, f32) {
    if state.grounded || b.up.y < 0.9 {
        return (*state, 0.0);
    }
    let feet = b.pos.y - (b.height * 0.5) as f64;
    let Some(g) = ground.sample(b.pos) else {
        return (*state, 0.0);
    };
    if feet > g.height + GROUND_SNAP {
        return (*state, 0.0);
    }
    let on = CharacterState {
        grounded: true,
        ground_normal: g.normal,
        ground_entity: None,
        ground_surface: Some(g.surface),
        ..*state
    };
    (on, (g.height - feet).max(0.0) as f32)
}

/// Physics backends may report the ground before the feet touch it (within their snap
/// distance). On landing, keep coming down at the fall's speed until the backend stops the
/// character, instead of hovering and sinking at [`GROUND_STICK`].
fn settle(m: &mut Mover, state: &CharacterState, up: Vec3, landed: bool) {
    if landed {
        m.settle = m.fall_speed.max(0.0);
    } else if -state.velocity.dot(up) < m.settle * 0.5 {
        // Stopped by the ground (or never moved down): settled.
        m.settle = 0.0;
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

    let knocked = m.knockback.length() > ladder::KNOCKED_OFF;
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
    m.grab_cooldown = (m.grab_cooldown - dt).max(0.0);
    m.grapple_cooldown = (m.grapple_cooldown - dt).max(0.0);
    let shot = m.pending_grapple.take();
    m.evade_secs = (m.evade_secs - dt).max(0.0);
    m.dash_charges =
        (m.dash_charges + dt / p.dash_recharge_secs.max(0.01)).min(p.dash_charges as f32);
    if grounded {
        settle(m, state, up, landed);
        m.coyote = p.coyote_secs;
        m.air_secs = 0.0;
        m.fall_speed = 0.0;
        m.air_jumps_left = p.air_jumps;
        m.last_wall = None;
    } else {
        m.settle = 0.0;
        m.coyote = (m.coyote - dt).max(0.0);
        m.air_secs += dt;
        m.fall_speed = m.fall_speed.max(-v.dot(up));
    }

    if m.disabled {
        m.dash = None;
        m.mantle = None;
        m.hang = None;
        m.ladder = None;
        m.vault = None;
        m.grapple = None;
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

    // Dashing, climbing, vaulting, hanging, on a ladder or riding a zip line lets go of the
    // grapple.
    if m.grapple.is_some()
        && (m.dash.is_some()
            || m.mantle.is_some()
            || m.vault.is_some()
            || m.hang.is_some()
            || m.ladder.is_some()
            || m.zip.is_some())
    {
        grapple::release(m, p, b.entity, out);
    }

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
        && m.vault.is_none()
        && m.hang.is_none()
        && m.dash_charges >= 1.0
    {
        if m.zip.is_some() {
            zipline::let_go(m, b, out, "zip_release");
        }
        if m.ladder.is_some() {
            ladder::let_go(m, b, out, "ladder_exit", true);
        }
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
            // Rising, keep close to the wall, without pressing into its face once against it
            // (a character pressing into a wall that slides past can lose its slide along it).
            // Its top edge still needs the press to roll the capsule over it.
            let against = state
                .wall_normal
                .is_some_and(|n| n.dot(mt.forward) < -0.7 && n.dot(up).abs() < 0.1);
            let press = if against { 0.0 } else { 0.5 };
            return if mt.rising {
                up * p.mantle_speed + mt.forward * press
            } else {
                mt.forward * mt.exit_speed.max(3.5)
            };
        }
        m.mantle = None;
        m.set_mode(LocomotionMode::Run);
        return mt.forward * mt.exit_speed - up * GROUND_STICK;
    }

    // ---- vault in progress
    if let Some(v) = vault::over(m, p, b, knocked, dt, out) {
        return v;
    }

    // ---- hanging from a ledge
    if let Some(h) = m.hang {
        if !grounded {
            return hang(
                m,
                p,
                b,
                probe,
                h,
                v,
                wish,
                jump_pressed,
                crouch_pressed,
                dt,
                out,
            );
        }
        m.hang = None;
    }

    // ---- ladder: climbing one, or grabbing one in reach
    if m.dash.is_none()
        && m.mantle.is_none()
        && m.vault.is_none()
        && m.hang.is_none()
        && m.zip.is_none()
        && let Some(v) = ladder::step(
            m,
            p,
            b,
            probe,
            v,
            wish,
            ladder::Held {
                jump_pressed,
                crouch: intent.crouch,
                sprint: intent.sprint,
                knocked,
            },
            grounded,
            dt,
            out,
        )
    {
        return v;
    }

    // ---- zip line: riding one, or catching one in reach
    if m.dash.is_none()
        && m.mantle.is_none()
        && m.vault.is_none()
        && m.hang.is_none()
        && let Some(v) = zipline::step(
            m,
            p,
            b,
            probe,
            v,
            wish,
            jump_pressed,
            crouch_pressed,
            grounded,
            dt,
            out,
        )
    {
        return v;
    }

    // ---- grapple hook
    if let Some(reel) = grapple::step(
        m,
        p,
        b,
        probe,
        shot,
        v,
        wish,
        jump_pressed,
        crouch_pressed,
        grounded,
        dt,
        out,
    ) {
        // Reeled in to a ledge: climb onto it (the rope lifts the body as high as a hang
        // reaches).
        let pull = reel.velocity;
        // Toward the stick, else into the wall the hook bit (its ledge is above the hook, even
        // when the body swings in beside it), else the way the character faces.
        let into = m
            .grapple
            .map(|g| flat(-g.normal, up))
            .filter(|d| d.length() > 0.5)
            .map(Vec3::normalize);
        let toward = if wish.length() > 0.3 {
            wish
        } else {
            into.unwrap_or(m.facing)
        };
        let reach = MovementProfile {
            mantle_max_height: p.mantle_max_height.max(p.ledge_grab_max_height),
            ..*p
        };
        if p.mantle
            && let Some(mut mt) = find_ledge(&reach, b, probe, pull, toward)
        {
            grapple::release(m, p, b.entity, out);
            mt.exit_speed = mt.exit_speed.min(p.sprint_speed);
            m.mantle = Some(mt);
            m.set_mode(LocomotionMode::Mantle);
            out.trigger(b.entity, "mantle");
            return up * p.mantle_speed;
        }
        if !reel.arrived {
            m.set_mode(LocomotionMode::Air);
            return pull;
        }
        // All the way in: let go and fly on.
        grapple::release(m, p, b.entity, out);
    }

    // ---- vault: taking off for an obstacle in reach
    let held = vault::Held {
        jump: m.jump_buffer > 0.0,
        sprint: intent.sprint,
    };
    let vault_ahead = match vault::take_off(m, p, b, probe, v, wish, held, grounded, out) {
        vault::TakeOff::Go(v) => return v,
        vault::TakeOff::Ahead => true,
        vault::TakeOff::No => false,
    };

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
    // (An obstacle about to be vaulted isn't climbed onto or caught.)
    if p.mantle
        && !vault_ahead
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
    // Too slow along the wall to run on it, or straight at it: catch its top.
    if p.ledge_grab
        && !vault_ahead
        && m.grab_cooldown <= 0.0
        && let Some(h) = find_hang(p, b, probe, v, wish)
    {
        m.hang = Some(h);
        m.set_mode(LocomotionMode::Climb);
        m.air_jumps_left = p.air_jumps;
        out.trigger(b.entity, "ledge_grab");
        out.noise(b.entity, b.pos, 3.0);
        return hang_pull(b, &h, h.top, dt);
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
    m.hang = None;
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
    let yaw = GroundContact::yaw_of(m.facing);
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
    // Wall-run into a ledge ahead, or steer into the wall near its top: climb over it (and
    // step onto it at a jog rather than at the full speed of the run along it).
    let over = || {
        (wish.dot(-wall.normal) > 0.75)
            .then(|| find_ledge(p, b, probe, v, -wall.normal))
            .flatten()
            .map(|mt| Mantle {
                exit_speed: mt.exit_speed.min(p.run_speed * 0.6),
                ..mt
            })
    };
    if p.mantle
        && let Some(mt) = find_ledge(p, b, probe, v, tdir).or_else(over)
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
    if v.dot(up) < -10.0 {
        return None;
    }
    // High enough above the ground?
    if probe.ray(b.feet(), -up, p.wallrun_min_height).is_some() {
        return None;
    }
    // Speed is judged against the wall once it is found (a wall can move), so look for walls
    // around the way the character moves, or faces when it barely moves.
    let vh = flat(v, up);
    let fwd = vh
        .try_normalize()
        .filter(|_| vh.length() > 1.0)
        .unwrap_or(m.facing);
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
    // Fast enough along the wall, relative to it: a train car's side is run along at the
    // speed past it, so running beside a car as fast as it goes is no wall-run, and a jump at
    // a car sliding past is.
    let moving = wall
        .entity
        .and_then(|e| probe.velocity_at(e, wall.point))
        .unwrap_or(Vec3::ZERO);
    let rel = flat(v - moving, up);
    let along = (rel - wall.normal * rel.dot(wall.normal)).length();
    if rel.length() < p.wallrun_min_speed
        || along < p.wallrun_min_speed * 0.8
        || wish.dot(wall.normal) > 0.5
    {
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
    if !room_to_climb(b, probe, ledge, inward) {
        return None;
    }
    let edge = hit.point + upd * (down.point - hit.point).dot(upd);
    Some(Mantle {
        top: edge,
        forward: inward,
        rising: true,
        exit_speed: vh.length().max(p.run_speed * 0.8),
        on: probe.pin(down.entity, edge, inward),
    })
}

/// Room to rise `ledge` meters (head) and then to stand on the ledge, going `inward`.
fn room_to_climb(b: &Body, probe: &Probe, ledge: f32, inward: Vec3) -> bool {
    let up = b.up;
    if probe
        .ray(b.pos, up, b.height * 0.5 + ledge + 0.05)
        .is_some()
    {
        return false;
    }
    let stand = b.feet() + (up * (ledge + b.radius + 0.3)).as_dvec3();
    probe.ray(stand, inward, b.radius + 0.9).is_none()
}

/// The top of a wall whose face is at `face` (normal `normal`, out of the wall): where the
/// face meets the top, if the top is walkable, clear above, and between `low` and `high`
/// meters above the feet.
fn ledge_top(
    b: &Body,
    probe: &Probe,
    face: DVec3,
    normal: Vec3,
    low: f32,
    high: f32,
) -> Option<DVec3> {
    let upd = b.up.as_dvec3();
    let feet = b.feet();
    let over = face - (normal * 0.25).as_dvec3();
    let from = over + upd * ((feet - over).dot(upd) + high as f64 + 0.15);
    let down = probe.ray(from, -b.up, high - low + 0.15)?;
    if down.normal.dot(b.up) < 0.7 || down.distance < 0.01 {
        return None;
    }
    let height = (down.point - feet).dot(upd) as f32;
    (low..=high)
        .contains(&height)
        .then(|| face + upd * (down.point - face).dot(upd))
}

/// The wall in front of the chest along `dir`, if it is a wall (not a slope or a character):
/// where it was hit, its horizontal normal and its body.
fn chest_wall(b: &Body, probe: &Probe, dir: Vec3) -> Option<(DVec3, Vec3, Entity)> {
    let chest = b.pos + (b.up * (b.height * 0.25)).as_dvec3();
    let hit = probe.ray(chest, dir, b.radius + 0.45)?;
    if hit.normal.dot(b.up).abs() >= WALL_MAX_UP || hit.normal.dot(dir) > -0.5 {
        return None;
    }
    Some((
        hit.point,
        flat(hit.normal, b.up).normalize_or_zero(),
        hit.entity,
    ))
}

/// A ledge too high to climb onto directly, in reach of the hands: near the top of a jump or
/// falling, moving or pushing toward the wall.
fn find_hang(p: &MovementProfile, b: &Body, probe: &Probe, v: Vec3, wish: Vec3) -> Option<Hang> {
    let up = b.up;
    if v.dot(up) > 1.5 {
        return None;
    }
    let vh = flat(v, up);
    let dir = wish
        .try_normalize()
        .filter(|_| wish.length() > 0.3)
        .or_else(|| vh.try_normalize().filter(|_| vh.length() > 0.5))?;
    let (face, normal, wall) = chest_wall(b, probe, dir)?;
    let low = p.mantle_max_height.min(p.ledge_grab_max_height);
    let top = ledge_top(b, probe, face, normal, low, p.ledge_grab_max_height)?;
    Some(Hang::new(top, normal, probe.pin(wall, top, normal)))
}

/// Velocity toward the hanging position under `top` (sideways is left free).
fn hang_pull(b: &Body, h: &Hang, top: DVec3, dt: f32) -> Vec3 {
    let hold = top - (b.up * (b.height * 0.5 + HANG_REACH)).as_dvec3()
        + (h.normal * (b.radius + HANG_GAP)).as_dvec3();
    let side = b.up.cross(h.normal);
    let gap = (hold - b.pos).as_vec3();
    let gap = gap - side * gap.dot(side);
    (gap / dt.max(1e-4)).clamp_length_max(HANG_PULL)
}

/// The ledge going on around a corner where the face hung from ends, shimmying `dir`: onto a
/// wall across the way (an inside corner) or onto the end of the wall, turned toward `dir`
/// (an outside corner), its top at the same height (`ledge` above the feet).
fn find_corner(
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    h: &Hang,
    dir: Vec3,
    ledge: f32,
) -> Option<Corner> {
    let up = b.up;
    let n0 = h.normal;
    let chest = b.pos + (up * (b.height * 0.25)).as_dvec3();
    let reach = b.radius + HANG_GAP;
    let is_wall = |hit: &Hit| hit.normal.dot(up).abs() < WALL_MAX_UP;
    let inside = probe
        .ray(chest, dir, b.radius + 0.2)
        .filter(|hit| is_wall(hit) && hit.normal.dot(-dir) > 0.8);
    let hit = inside.or_else(|| {
        // Look back from past the end, behind the face, at the wall's end.
        let from = chest + (dir * (b.radius + 0.45) - n0 * (reach + 0.2)).as_dvec3();
        probe
            .ray(from, -dir, b.radius + 0.6)
            .filter(|hit| hit.distance > 0.0 && is_wall(hit) && hit.normal.dot(dir) > 0.8)
    })?;
    let normal = flat(hit.normal, up).normalize_or_zero();
    let top = ledge_top(b, probe, hit.point, normal, ledge - 0.3, ledge + 0.3)?;
    // The corner's edge at the top: the next face's ledge moved onto the old face's plane.
    let edge = top + (n0 * (h.top - top).dot(n0.as_dvec3()) as f32).as_dvec3();
    let hold = edge - (up * (b.height * 0.5 + HANG_REACH)).as_dvec3();
    let (along, clear) = if inside.is_some() {
        (n0, reach + 0.1)
    } else {
        (-n0, 0.12)
    };
    let from = b.pos;
    let via = hold + ((n0 + normal) * reach).as_dvec3();
    let to = hold + (normal * reach + along * clear).as_dvec3();
    let length = (via.distance(from) + to.distance(via)) as f32;
    Some(Corner {
        from,
        via,
        to,
        from_top: h.top,
        from_normal: n0,
        top: edge + (along * clear).as_dvec3(),
        normal,
        along,
        came: dir,
        wall: Some(hit.entity),
        t: 0.0,
        secs: (length / p.shimmy_speed.max(0.1)).max(0.25),
    })
}

/// One step around a corner of the ledge: along the curve, turning to face the next face.
fn round_corner(
    m: &mut Mover,
    b: &Body,
    probe: &Probe,
    mut h: Hang,
    mut c: Corner,
    dt: f32,
) -> Vec3 {
    c.t = (c.t + dt / c.secs).min(1.0);
    let turn = smooth01(c.t);
    h.normal = c
        .from_normal
        .lerp(c.normal, turn)
        .try_normalize()
        .unwrap_or(c.normal);
    h.top = c.from_top.lerp(c.top, turn as f64);
    h.on = None;
    if c.t >= 1.0 {
        h.top = c.top;
        h.normal = c.normal;
        h.on = c.wall.and_then(|w| probe.pin(w, c.top, c.normal));
        h.corner = None;
        h.carry = Some((c.came, c.along));
    } else {
        h.corner = Some(c);
    }
    m.hang = Some(h);
    m.set_mode(LocomotionMode::Climb);
    ((c.at(c.t) - b.pos).as_vec3() / dt.max(1e-4)).clamp_length_max(HANG_PULL * 1.5)
}

/// One step hanging from a ledge: shimmy along it (and around its corners), climb up, kick
/// off or let go.
#[allow(clippy::too_many_arguments)]
fn hang(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    h: Hang,
    v: Vec3,
    wish: Vec3,
    jump_pressed: bool,
    crouch_pressed: bool,
    dt: f32,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let feet = b.feet();
    let let_go = |m: &mut Mover, cooldown: f32| {
        m.hang = None;
        m.grab_cooldown = cooldown;
        m.set_mode(LocomotionMode::Air);
    };
    if let Some(c) = h.corner {
        if crouch_pressed {
            let_go(m, 0.5);
            out.trigger(b.entity, "let_go");
            return h.normal * 1.0;
        }
        return round_corner(m, b, probe, h, c, dt);
    }
    // Just around a corner, the stick held the old way along the ledge goes on the new way.
    let carry = h.carry.filter(|(came, _)| wish.dot(*came) > 0.3);
    let wish = carry.map_or(wish, |(_, along)| along * wish.length());
    // Still there? (Shimmied off its end, or it broke.)
    let held = (h.top - feet).dot(up.as_dvec3()) as f32;
    let Some((face, normal, wall)) =
        chest_wall(b, probe, -h.normal).filter(|(_, n, _)| n.dot(h.normal) > 0.8)
    else {
        let_go(m, 0.3);
        return flat(v, up);
    };
    let Some(top) = ledge_top(b, probe, face, normal, held - 0.35, held + 0.35) else {
        let_go(m, 0.3);
        return flat(v, up);
    };
    let h = Hang {
        carry,
        ..Hang::new(top, normal, probe.pin(wall, top, normal))
    };
    m.hang = Some(h);
    let right = up.cross(normal);
    let toward = wish.dot(-normal);
    let side = wish.dot(right);

    if crouch_pressed {
        let_go(m, 0.5);
        out.trigger(b.entity, "let_go");
        return normal * 1.0;
    }
    if jump_pressed && toward < -0.3 {
        // Kick off the wall, steered by the stick.
        let_go(m, 0.4);
        m.jump_buffer = 0.0;
        out.trigger(b.entity, "wall_jump");
        out.noise(b.entity, b.pos, 6.0);
        let away = normal * p.wall_jump_push + right * (side * p.run_speed * 0.6);
        return away + up * MovementProfile::launch_speed(p.wall_jump_height, b.g);
    }
    let ledge = (top - feet).dot(up.as_dvec3()) as f32;
    let climb = (jump_pressed || (toward > 0.5 && m.mode_secs >= HANG_CLIMB_HOLD))
        && room_to_climb(b, probe, ledge, -normal);
    if climb {
        m.hang = None;
        m.jump_buffer = 0.0;
        m.mantle = Some(Mantle {
            top,
            forward: -normal,
            rising: true,
            exit_speed: p.run_speed * 0.5,
            on: h.on.map(Pin::reversed),
        });
        m.set_mode(LocomotionMode::Mantle);
        out.trigger(b.entity, "mantle");
        return up * p.mantle_speed;
    }

    // Shimmy, as long as the ledge goes on that way.
    let mut along = Vec3::ZERO;
    if side.abs() > 0.2 {
        let dir = right * side.signum();
        let ahead = Body {
            pos: b.pos + (dir * (b.radius + 0.15)).as_dvec3(),
            ..*b
        };
        let continues = chest_wall(&ahead, probe, -normal)
            .filter(|(_, n, _)| n.dot(normal) > 0.8)
            .and_then(|(f, n, _)| ledge_top(&ahead, probe, f, n, ledge - 0.3, ledge + 0.3))
            .is_some();
        // Corners only on ledges that stand still.
        let still = m
            .platform
            .is_none_or(|d| d.velocity.length_squared() < 1e-4);
        if continues {
            along = right * (side * p.shimmy_speed);
        } else if let Some(c) = find_corner(p, b, probe, &h, dir, ledge).filter(|_| still) {
            out.trigger(b.entity, "ledge_corner");
            return round_corner(
                m,
                b,
                probe,
                Hang {
                    corner: Some(c),
                    ..h
                },
                c,
                dt,
            );
        }
    }
    m.set_mode(LocomotionMode::Climb);
    hang_pull(b, &h, top, dt) + along
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
        yaw: GroundContact::yaw_of(m.facing),
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
        // 0 ahead, positive to the left (counter-clockwise from above), ±π behind.
        (-d.x).atan2(-d.z)
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
    let reeling = m.grapple.is_some_and(|g| g.attached);
    params.set(
        "effort",
        match m.mode {
            _ if reeling => 0.9,
            _ if m.zip.is_some() => 0.6,
            LocomotionMode::Dash | LocomotionMode::Mantle => 1.0,
            LocomotionMode::Climb => 0.7,
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
    params.set("grapple", if reeling { 1.0 } else { 0.0 });
    params.set("zipline", if m.zip.is_some() { 1.0 } else { 0.0 });
    params.set("ladder", if m.ladder.is_some() { 1.0 } else { 0.0 });
    params.set("climb_speed", m.ladder.map_or(0.0, |l| l.speed));
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
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn the_direction_parameter_turns_counter_clockwise_like_yaw() {
        // Facing −Z: ahead is 0, to the left (−X) is +π/2, to the right −π/2, behind ±π.
        for (velocity, want) in [
            (Vec3::NEG_Z, 0.0),
            (Vec3::NEG_X, FRAC_PI_2),
            (Vec3::X, -FRAC_PI_2),
            (Vec3::Z, PI),
        ] {
            let m = Mover {
                facing: Vec3::NEG_Z,
                velocity: velocity * 4.0,
                ..Default::default()
            };
            let mut params = AnimationParams::default();
            write_params(&m, &mut params, Vec3::Y);
            let got = params.get("direction");
            assert!(
                (got - want).abs() < 1e-4 || (got.abs() - PI).abs() < 1e-4 && want == PI,
                "{velocity}: {got} for {want}"
            );
        }
    }

    #[test]
    fn a_landing_keeps_coming_down_until_the_ground_stops_it() {
        let mut m = Mover {
            fall_speed: 6.0,
            ..Default::default()
        };
        let falling = CharacterState {
            grounded: true,
            velocity: Vec3::new(0.0, -6.0, 0.0),
            ..Default::default()
        };
        settle(&mut m, &falling, Vec3::Y, true);
        assert_eq!(m.settle, 6.0);
        // Still coming down at that speed (the ground was reported early): keep it.
        settle(&mut m, &falling, Vec3::Y, false);
        assert_eq!(m.settle, 6.0);
        // Stopped by the ground: settled.
        let stopped = CharacterState {
            velocity: Vec3::new(0.0, -0.4, 0.0),
            ..falling
        };
        settle(&mut m, &stopped, Vec3::Y, false);
        assert_eq!(m.settle, 0.0);
    }

    #[test]
    fn feet_on_or_in_the_ground_surface_stand_on_it() {
        let ground = Ground::default(); // flat at y = 0
        let body = |y: f64| Body {
            entity: Entity::PLACEHOLDER,
            pos: DVec3::new(3.0, y, 4.0),
            up: Vec3::Y,
            g: 9.81,
            radius: 0.35,
            height: 1.8,
        };
        let air = CharacterState::default();
        // Sank 0.3 m in: standing, and lifted back out.
        let (s, lift) = on_ground_surface(&air, &ground, &body(0.6));
        assert!(s.grounded && s.ground_normal == Vec3::Y && s.ground_entity.is_none());
        assert!((lift - 0.3).abs() < 1e-5);
        // Just above: standing, nothing to lift.
        let (s, lift) = on_ground_surface(&air, &ground, &body(0.93));
        assert!(s.grounded && lift == 0.0);
        // In the air: unchanged.
        let (s, _) = on_ground_surface(&air, &ground, &body(1.5));
        assert!(!s.grounded);
        // Physics found ground (a collider): its report wins.
        let on_box = CharacterState {
            grounded: true,
            ground_normal: Vec3::new(0.0, 0.8, 0.6),
            ..Default::default()
        };
        assert_eq!(on_ground_surface(&on_box, &ground, &body(0.6)).0, on_box);
    }

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

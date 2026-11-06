//! Space magic (round-9-plan.md): portals you see through and pass through, a blink that lands
//! where you aim, and a recall that rewinds you along your own path.
//!
//! A gate is a rectangle in its entity's XY plane, seen and entered from its +Z side, centred on
//! the entity (the renderer's [`engine_render::Portal`] convention, which draws the view through
//! it). Two gates that lead to each other make a two-way door. Whatever crosses a gate's plane
//! from the front, inside its rectangle, comes out of the other one's front, carried by
//! [`carry`] (as [`engine_render::Portal::carry`]): its position, its heading and its velocity
//! turn with the pair, so a bolt fired into one flies on out of the other.
//!
//! * **Portal** (`portal`): the first cast opens an end facing the caster, the second its other
//!   end facing away from the caster, and links them; a third starts a new pair and the old one
//!   closes. An end alone is a membrane nothing passes. Bodies ([`pass_gates`]), bolts and lobs
//!   (`runtime::fly_missiles`) and beams (`runtime::tick_beams`) go through; each traveller waits
//!   [`REENTRY_SECS`] before it can go back in.
//! * **Blink** (`blink`): slides the caster to the aim point within its reach in
//!   [`BLINK_SECS`], short of walls, onto a ledge it aims at ([`blink_target`]).
//! * **Recall** (`recall`): every caster keeps a [`PathLog`]; a recall slides it back along the
//!   last seconds of it in [`REWIND_SECS`], untouchable meanwhile, its health back to what it was
//!   if that was more.

use std::collections::{HashMap, VecDeque};

use engine_assets::{Assets, Handle};
use engine_core::math::{DAffine3, DQuat, DVec3, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, MaterialRef, Mesh, MeshRef, MeshRenderer, NoMeshLod, NoVirtualGeometry, PointLight,
    Portal, Shape, StandardMaterial, Vertex,
};
use engine_world::physics::{
    CharacterController, PhysicsQueries, QueryFilter, RigidBody, Velocity,
};

use crate::caster::{MagicCaster, Vitality};
use crate::compile::Spell;
use crate::looks::{LookCmd, Role};
use crate::runtime::{CastInfo, MagicQueue, ground_below};
use crate::wards::Paint;

/// Seconds an end takes to open (it grows from its centre), and to close.
pub const OPEN_SECS: f32 = 0.25;
pub const CLOSE_SECS: f32 = 0.35;
/// Seconds before something that went through may go through again (Symmetra's teleporter).
pub const REENTRY_SECS: f32 = 1.0;
/// The fastest anything comes out, m/s.
pub const MAX_SPEED: f32 = 50.0;
/// Emission of an end's frame and of its membrane, nits (wards.rs: daylight needs thousands).
pub const FRAME_GLOW: f32 = 16_000.0;
pub const MEMBRANE_GLOW: f32 = 2_400.0;
/// The light an open end casts round it, lm.
pub const GATE_LIGHT_LM: f32 = 2_200.0;
/// A plain portal's two rim colours (each end its own; colour never means in or out).
pub const VIOLET: [f32; 3] = [0.62, 0.3, 1.0];
pub const TEAL: [f32; 3] = [0.1, 0.95, 0.82];
/// Seconds a blink slides.
pub const BLINK_SECS: f32 = 0.15;
/// The highest ledge a blink steps onto, above the feet, m.
pub const LEDGE_M: f32 = 2.6;
/// Seconds a recall's rewind takes.
pub const REWIND_SECS: f32 = 0.45;
/// Seconds between the points of a caster's path.
pub const PATH_STEP: f32 = 0.05;
/// The longest a path is kept, s.
pub const PATH_KEEP: f32 = 6.0;
/// Emission of an afterimage, nits.
pub const GHOST_GLOW: f32 = 9_000.0;
/// A plain blink's and a plain recall's colour (a woven one takes its element's): bright, so
/// they read as light (the blink's own violet drew as dark smoke in the first render).
pub const BLINK_COLOR: [f32; 3] = [0.45, 0.85, 1.0];
pub const RECALL_COLOR: [f32; 3] = [1.0, 0.78, 0.35];

// ------------------------------------------------------------------------------- geometry

/// Where a gate stands and how big it is (its entity's pose, without scale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateFrame {
    pub at: DVec3,
    pub rot: DQuat,
    /// Width and height, m (the opening's scale already applied).
    pub size: Vec2,
}

impl GateFrame {
    pub fn of(g: &GlobalTransform, size: Vec2) -> Self {
        let scale = g.scale();
        Self {
            at: g.translation(),
            rot: g.rotation().as_dquat(),
            size: size * Vec2::new(scale.x.abs(), scale.y.abs()),
        }
    }

    /// A root entity's frame from its `Transform` (a gate has no parent).
    pub fn of_transform(t: &Transform, size: Vec2) -> Self {
        Self {
            at: t.translation,
            rot: t.rotation.as_dquat(),
            size: size * Vec2::new(t.scale.x.abs(), t.scale.y.abs()),
        }
    }

    /// A point in the gate's own space (x across, y up, +z out of its front).
    pub fn local(&self, p: DVec3) -> DVec3 {
        self.rot.inverse() * (p - self.at)
    }

    /// The way out of its front.
    pub fn normal(&self) -> Vec3 {
        (self.rot * DVec3::Z).as_vec3()
    }

    /// Whether a point of the gate's plane (in its own space) lies inside the opening, with
    /// `margin` m taken off each side (a body's radius, so it fits through).
    pub fn inside(&self, l: DVec3, margin: f32) -> bool {
        let half = (self.size * 0.5 - Vec2::splat(margin)).max(Vec2::ZERO);
        l.x.abs() <= half.x as f64 && l.y.abs() <= half.y as f64
    }

    /// Where the step from `a` to `b` passes through the opening from the front: the share of
    /// the way (0..1), or None if it doesn't (from behind, beside it, or not reaching it).
    pub fn crossing(&self, a: DVec3, b: DVec3, margin: f32) -> Option<f64> {
        let (la, lb) = (self.local(a), self.local(b));
        if la.z < 0.0 || lb.z >= 0.0 {
            return None;
        }
        let t = la.z / (la.z - lb.z);
        let p = la + (lb - la) * t;
        self.inside(p, margin).then_some(t)
    }

    fn rigid(&self) -> DAffine3 {
        DAffine3::from_rotation_translation(self.rot, self.at)
    }
}

/// The map from in front of `entry` to beyond `exit` (as `engine_render::Portal::carry`): turn
/// half round about the entry's up, then stand at the exit.
pub fn carry(entry: &GateFrame, exit: &GateFrame) -> DAffine3 {
    exit.rigid() * DAffine3::from_rotation_y(std::f64::consts::PI) * entry.rigid().inverse()
}

/// A position carried through.
pub fn carry_point(m: &DAffine3, p: DVec3) -> DVec3 {
    m.transform_point3(p)
}

/// A direction or velocity carried through.
pub fn carry_vector(m: &DAffine3, v: Vec3) -> Vec3 {
    m.transform_vector3(v.as_dvec3()).as_vec3()
}

/// A rotation carried through (a body's facing).
pub fn carry_rotation(entry: &GateFrame, exit: &GateFrame, r: DQuat) -> DQuat {
    exit.rot * DQuat::from_rotation_y(std::f64::consts::PI) * entry.rot.inverse() * r
}

/// A velocity no faster than [`MAX_SPEED`].
pub fn capped(v: Vec3) -> Vec3 {
    v.clamp_length_max(MAX_SPEED)
}

// ------------------------------------------------------------------------------- gates

/// One end of a portal.
#[derive(Component, Debug, Clone)]
pub struct Gate {
    pub caster: Option<Entity>,
    /// The other end, once the pair is linked.
    pub exit: Option<Entity>,
    /// 0 for the first end of its pair, 1 for the second.
    pub end: u8,
    /// Width and height when fully open, m.
    pub size: Vec2,
    pub age: f32,
    /// Seconds it stays open.
    pub life: f32,
    pub color: [f32; 3],
    /// The spell's look (bursts as things pass).
    pub look: String,
    /// Seconds since it began to close.
    pub closing: Option<f32>,
    /// Brightness kick from the last thing through, fading.
    pub pulse: f32,
    /// Its frame, its back membrane, its both-sided membrane (while alone) and its light.
    pub parts: [Option<Entity>; 4],
}

impl Gate {
    /// Fully open and not closing: things may pass.
    pub fn open(&self) -> bool {
        self.age >= OPEN_SECS && self.closing.is_none()
    }

    /// How far it has grown (it overshoots a little as it opens, and shrinks as it closes).
    pub fn scale(&self) -> f32 {
        let s = (self.age / OPEN_SECS).clamp(0.0, 1.0);
        // Ease out with a little overshoot ("back" easing).
        let (c, u) = (1.7, s - 1.0);
        let open = 1.0 + (c + 1.0) * u * u * u + c * u * u;
        let shut = self
            .closing
            .map_or(1.0, |t| (1.0 - t / CLOSE_SECS).clamp(0.0, 1.0).powf(0.7));
        (open * shut).max(0.02)
    }
}

/// A gate's open view of itself for things on their way through: where it is, and where its
/// other end is if the pair is linked and both are open.
#[derive(Debug, Clone, Copy)]
pub struct Door {
    pub entity: Entity,
    pub frame: GateFrame,
    pub exit: Option<(Entity, GateFrame)>,
    pub color: [f32; 3],
    pub exit_color: [f32; 3],
}

impl Door {
    /// The map through it, if it leads anywhere.
    pub fn carry(&self) -> Option<DAffine3> {
        self.exit.map(|(_, x)| carry(&self.frame, &x))
    }
}

/// The open gates, as doors.
pub fn doors<'a>(
    gates: impl Iterator<Item = (Entity, &'a Gate, &'a Transform)> + Clone,
) -> Vec<Door> {
    let open: Vec<(Entity, &Gate, GateFrame)> = gates
        .filter(|(_, g, _)| g.open())
        .map(|(e, g, t)| {
            (
                e,
                g,
                GateFrame {
                    at: t.translation,
                    rot: t.rotation.as_dquat(),
                    size: g.size,
                },
            )
        })
        .collect();
    open.iter()
        .map(|(e, g, f)| {
            let exit = g
                .exit
                .and_then(|x| open.iter().find(|(o, _, _)| *o == x))
                .map(|(o, og, of)| (*o, *of, og.color));
            Door {
                entity: *e,
                frame: *f,
                exit: exit.map(|(o, of, _)| (o, of)),
                color: g.color,
                exit_color: exit.map_or(g.color, |x| x.2),
            }
        })
        .collect()
}

/// The first door the step from `a` to `b` goes into from the front (with `margin` m to spare
/// at its sides), as the door's index and the share of the step, skipping `skip`.
pub fn first_crossing(
    doors: &[Door],
    a: DVec3,
    b: DVec3,
    margin: f32,
    skip: &[Entity],
) -> Option<(usize, f64)> {
    doors
        .iter()
        .enumerate()
        .filter(|(_, d)| !skip.contains(&d.entity))
        .filter_map(|(i, d)| d.frame.crossing(a, b, margin).map(|t| (i, t)))
        .min_by(|x, y| x.1.total_cmp(&y.1))
}

/// The bursts of something going through `door`: a ring in each opening, round its middle,
/// sparks along its facing.
pub fn passage_looks(queue: &mut MagicQueue, door: &Door, scale: f32) {
    let mut ends = vec![(door.frame.at, door.frame.normal(), door.color)];
    if let Some((_, x)) = door.exit {
        ends.push((x.at, x.normal(), door.exit_color));
    }
    for (at, up, color) in ends {
        queue.looks.push(LookCmd::Burst {
            at,
            up,
            look: "portal".into(),
            role: Role::Impact,
            scale,
            color,
        });
    }
    pulse_through(queue, door);
}

/// Both ends of `door` flare (a beam passing through).
pub fn pulse_through(queue: &mut MagicQueue, door: &Door) {
    queue.gate_pulses.push(door.entity);
    if let Some((x, _)) = door.exit {
        queue.gate_pulses.push(x);
    }
}

/// A portal end to open.
#[derive(Debug, Clone)]
pub struct NewGate {
    pub cast: CastInfo,
    /// The ground under it.
    pub at: DVec3,
    /// From it toward the caster, flat.
    pub to_caster: Vec3,
}

/// The rim colours of a portal's two ends: violet and teal, or a woven element's colour and a
/// paler one.
pub fn gate_colors(spell: &Spell) -> [[f32; 3]; 2] {
    match spell.weave.elements.first() {
        Some(e) => {
            let c = e.color();
            let pale = [
                c[0] + (1.0 - c[0]) * 0.5,
                c[1] + (1.0 - c[1]) * 0.5,
                c[2] + (1.0 - c[2]) * 0.5,
            ];
            [c, pale]
        }
        None => [VIOLET, TEAL],
    }
}

/// A spot no further than `reach` from `from` toward `to` (flat).
pub fn within(from: DVec3, to: DVec3, reach: f32) -> DVec3 {
    let d = to - from;
    let flat = DVec3::new(d.x, 0.0, d.z);
    let len = flat.length();
    if len <= reach as f64 || len < 1e-6 {
        return to;
    }
    from + flat * (reach as f64 / len) + DVec3::Y * d.y
}

/// The meshes of gates by size (cm): frame and membrane.
#[derive(Resource, Default)]
pub struct GateMeshes {
    frames: HashMap<(u32, u32), Handle<Mesh>>,
    membranes: HashMap<(u32, u32), Handle<Mesh>>,
}

impl GateMeshes {
    fn frame(&mut self, size: Vec2, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        let key = ((size.x * 100.0) as u32, (size.y * 100.0) as u32);
        *self
            .frames
            .entry(key)
            .or_insert_with(|| meshes.add(frame_mesh(size.x, size.y)))
    }

    fn membrane(&mut self, size: Vec2, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        let key = ((size.x * 100.0) as u32, (size.y * 100.0) as u32);
        *self
            .membranes
            .entry(key)
            .or_insert_with(|| meshes.add(membrane_mesh(size.x, size.y)))
    }
}

/// The band between two rectangles round the origin, half-sizes `a` and `b`, alpha `aa` on the
/// first and `ab` on the second, at depth `z` facing +z.
fn ring(m: &mut Mesh, a: Vec2, b: Vec2, aa: f32, ab: f32, z: f32) {
    let corners = |h: Vec2| {
        [
            Vec3::new(-h.x, -h.y, z),
            Vec3::new(h.x, -h.y, z),
            Vec3::new(h.x, h.y, z),
            Vec3::new(-h.x, h.y, z),
        ]
    };
    let (ca, cb) = (corners(a), corners(b));
    let v = |p: Vec3, al: f32| {
        Vertex::new(p, Vec3::Z, Default::default()).with_color(Vec4::new(1.0, 1.0, 1.0, al))
    };
    for k in 0..4 {
        let k2 = (k + 1) % 4;
        let base = m.vertices.len() as u32;
        m.vertices
            .extend([v(ca[k], aa), v(ca[k2], aa), v(cb[k2], ab), v(cb[k], ab)]);
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn quad(m: &mut Mesh, c: [Vec3; 4], a: [f32; 4], n: Vec3) {
    let base = m.vertices.len() as u32;
    for k in 0..4 {
        m.vertices.push(
            Vertex::new(c[k], n, Default::default()).with_color(Vec4::new(1.0, 1.0, 1.0, a[k])),
        );
    }
    m.indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// The frame of light round an opening `w` by `h`, centred, facing +z (drawn from both sides):
/// a soft halo outside, a bright band, a glow fading inward over the view, rune ticks along the
/// band and a pool of light on the floor at its foot.
pub fn frame_mesh(w: f32, h: f32) -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let half = Vec2::new(w, h) * 0.5;
    let z = 0.006;
    const BAND: f32 = 0.06;
    const HALO: f32 = 0.16;
    // Narrow: wider, the glow over the view washed it out to a pale haze (first render).
    const GLOW: f32 = 0.11;
    // Halo outside, the bright band on the edge, the glow fading inward over the view.
    ring(
        &mut m,
        half + Vec2::splat(HALO),
        half + Vec2::splat(0.02),
        0.0,
        0.55,
        z,
    );
    ring(
        &mut m,
        half + Vec2::splat(0.02),
        half - Vec2::splat(BAND),
        1.0,
        0.85,
        z,
    );
    ring(
        &mut m,
        half - Vec2::splat(BAND),
        half - Vec2::splat(BAND + GLOW),
        0.3,
        0.0,
        z,
    );
    // Rune ticks: short bright bars across the band, every 0.21 m along each side.
    let tick = |m: &mut Mesh, at: Vec2, along: Vec2, out: Vec2| {
        let (a, o) = (along * 0.016, out);
        let p0 = at - o * 0.05;
        let p1 = at + o * (0.05 + HALO * 0.7);
        quad(
            m,
            [
                (p0 - a).extend(z + 0.001),
                (p0 + a).extend(z + 0.001),
                (p1 + a).extend(z + 0.001),
                (p1 - a).extend(z + 0.001),
            ],
            [1.0, 1.0, 0.0, 0.0],
            Vec3::Z,
        );
    };
    for side in 0..4 {
        let (from, to, out) = match side {
            0 => (
                Vec2::new(-half.x, -half.y),
                Vec2::new(-half.x, half.y),
                Vec2::NEG_X,
            ),
            1 => (
                Vec2::new(half.x, -half.y),
                Vec2::new(half.x, half.y),
                Vec2::X,
            ),
            2 => (
                Vec2::new(-half.x, half.y),
                Vec2::new(half.x, half.y),
                Vec2::Y,
            ),
            _ => continue,
        };
        let len = from.distance(to);
        let n = (len / 0.21).floor().max(1.0) as u32;
        let along = (to - from) / len;
        for k in 1..n {
            let at = from + along * (len * k as f32 / n as f32);
            tick(&mut m, at, along, out);
        }
    }
    // A pool of light on the floor round its foot, brightest at the threshold.
    let segs = 48;
    let (rx, rz) = (half.x + 0.5, 0.55);
    let y = -half.y + 0.012;
    for k in 0..segs {
        let a0 = k as f32 / segs as f32 * std::f32::consts::TAU;
        let a1 = (k + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let inner = |a: f32| Vec3::new(a.cos() * half.x * 0.9, y, a.sin() * 0.12);
        let outer = |a: f32| Vec3::new(a.cos() * rx, y, a.sin() * rz);
        quad(
            &mut m,
            [inner(a0), inner(a1), outer(a1), outer(a0)],
            [0.55, 0.55, 0.0, 0.0],
            Vec3::Y,
        );
    }
    m
}

/// The membrane over an opening `w` by `h`, facing -z (what an end shows from behind; while
/// alone it is drawn from both sides): brighter at the rim, dim in the middle.
pub fn membrane_mesh(w: f32, h: f32) -> Mesh {
    let mut m = Mesh::new(Vec::new(), Vec::new());
    let half = Vec2::new(w, h) * 0.5;
    let z = -0.01;
    ring(&mut m, half, half - Vec2::splat(0.3), 0.85, 0.35, z);
    let c = half - Vec2::splat(0.3);
    quad(
        &mut m,
        [
            Vec3::new(-c.x, -c.y, z),
            Vec3::new(c.x, -c.y, z),
            Vec3::new(c.x, c.y, z),
            Vec3::new(-c.x, c.y, z),
        ],
        [0.35; 4],
        Vec3::Z,
    );
    // Face -z: turn every triangle over.
    for tri in m.indices.chunks_mut(3) {
        tri.swap(1, 2);
    }
    for v in m.vertices.iter_mut() {
        v.normal = [0.0, 0.0, -1.0];
    }
    m
}

/// The see-through, glowing material of an end's frame, membrane or an afterimage: black, so
/// all its light is emission, its vertices' alpha drawing the pattern.
pub fn glow_material(
    color: [f32; 3],
    strength: f32,
    alpha: f32,
    double_sided: bool,
) -> StandardMaterial {
    StandardMaterial {
        base_color: [0.0, 0.0, 0.0, alpha],
        emissive: color,
        emissive_strength: crate::wards::glow_step(strength),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided,
        metallic: 0.0,
        perceptual_roughness: 1.0,
        ..Default::default()
    }
}

fn glow_part(mesh: Handle<Mesh>, material: StandardMaterial, visible: bool) -> MeshRenderer {
    MeshRenderer {
        mesh: MeshRef::Handle(mesh),
        material: MaterialRef::Inline(material),
        cast_shadows: false,
        visible,
    }
}

/// A fading glowing figure where a body was (a blink's start, a recall's path).
#[derive(Component, Debug, Clone, Copy)]
pub struct Afterimage {
    pub age: f32,
    /// Seconds it holds before it fades.
    pub hold: f32,
    pub fade: f32,
    pub color: [f32; 3],
}

fn spawn_ghost(
    commands: &mut Commands,
    feet: DVec3,
    rot: Quat,
    color: [f32; 3],
    hold: f32,
    fade: f32,
) {
    commands.spawn((
        Name::new("Afterimage"),
        Transform::from_translation(feet + DVec3::Y * 0.95).with_rotation(rot),
        MeshRenderer {
            mesh: MeshRef::Shape(Shape::Capsule {
                radius: 0.3,
                height: 1.2,
                segments: 12,
            }),
            material: MaterialRef::Inline(glow_material(color, GHOST_GLOW, 0.45, false)),
            cast_shadows: false,
            visible: true,
        },
        Afterimage {
            age: 0.0,
            hold,
            fade,
            color,
        },
        NoVirtualGeometry,
        NoMeshLod,
    ));
}

/// Keep the portals: open the ends cast this step (linking pairs, closing an old pair when a
/// third end opens), grow, pulse and light them, close them when their time is up, and fade
/// the afterimages.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn tend_gates(
    time: Res<Time>,
    mut commands: Commands,
    mut queue: ResMut<MagicQueue>,
    mut gm: ResMut<GateMeshes>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut gates: Query<(Entity, &mut Gate, &mut Transform)>,
    mut parts: Query<(&mut MeshRenderer, Option<&mut PointLight>), Without<Afterimage>>,
    mut lights: Query<&mut PointLight, Without<MeshRenderer>>,
    mut ghosts: Query<(Entity, &mut Afterimage, &mut MeshRenderer)>,
    (mut materials, mut painted): (
        Option<ResMut<Assets<StandardMaterial>>>,
        Local<HashMap<Entity, Handle<StandardMaterial>>>,
    ),
) {
    let dt = time.delta_secs();
    let new = std::mem::take(&mut queue.gates);
    let pulses = std::mem::take(&mut queue.gate_pulses);
    if gates.is_empty() && new.is_empty() && ghosts.is_empty() && painted.is_empty() {
        return;
    }
    if let Some(mats) = materials.as_deref_mut() {
        painted.retain(|&e, h| {
            let live = parts.contains(e) || ghosts.contains(e);
            if !live {
                mats.remove(*h);
            }
            live
        });
    }
    // ---- open the new ends
    // This step's first ends by caster (two casts in one step still pair up).
    let mut fresh: HashMap<Entity, (Entity, Vec2)> = HashMap::new();
    for ng in new {
        let spell = &ng.cast.spell;
        let size = Vec2::from(spell.payload.portal_m);
        let colors = gate_colors(spell);
        let owner = ng.cast.caster;
        // The caster's end waiting for its other end, if any.
        let waiting = owner.and_then(|c| {
            fresh.remove(&c).or_else(|| {
                gates
                    .iter()
                    .find(|(_, g, _)| {
                        g.caster == Some(c) && g.exit.is_none() && g.end == 0 && g.closing.is_none()
                    })
                    .map(|(e, g, _)| (e, g.size))
            })
        });
        if waiting.is_none()
            && let Some(c) = owner
        {
            // A new pair: the caster's old one closes.
            let old: Vec<Entity> = gates
                .iter()
                .filter(|(_, g, _)| g.caster == Some(c) && g.closing.is_none())
                .map(|(e, _, _)| e)
                .collect();
            for e in old {
                if let Ok((_, mut g, _)) = gates.get_mut(e) {
                    g.closing = Some(0.0);
                }
            }
        }
        let end = u8::from(waiting.is_some());
        // The first end faces the caster; the second faces away, so walking into the near end
        // carries you on out of the far one.
        let facing = if end == 0 {
            ng.to_caster
        } else {
            -ng.to_caster
        };
        let facing = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::Z);
        let rot = Quat::from_rotation_y(facing.x.atan2(facing.z));
        let color = colors[end as usize];
        let center = ng.at + DVec3::Y * (size.y * 0.5) as f64;
        let ec = commands.spawn((
            Name::new(if end == 0 {
                "Portal (first end)"
            } else {
                "Portal (second end)"
            }),
            Transform::from_translation(center)
                .with_rotation(rot)
                .with_scale(Vec3::new(0.02, 0.02, 1.0)),
            NoVirtualGeometry,
            NoMeshLod,
        ));
        let me = ec.id();
        let mut gate = Gate {
            caster: owner,
            exit: waiting.map(|w| w.0),
            end,
            size,
            age: 0.0,
            life: spell.payload.portal_secs,
            color,
            look: spell.weave.key.clone(),
            closing: None,
            pulse: 0.0,
            parts: [None; 4],
        };
        if let Some(meshes) = meshes.as_deref_mut() {
            let frame = gm.frame(size, meshes);
            let membrane = gm.membrane(size, meshes);
            let mut child = |r: MeshRenderer| {
                commands
                    .spawn((
                        Name::new("Portal part"),
                        Transform::IDENTITY,
                        r,
                        NoVirtualGeometry,
                        NoMeshLod,
                        ChildOf(me),
                    ))
                    .id()
            };
            gate.parts[0] = Some(child(glow_part(
                frame,
                glow_material(color, 0.0, 1.0, true),
                true,
            )));
            gate.parts[1] = Some(child(glow_part(
                membrane,
                glow_material(color, 0.0, 0.8, false),
                waiting.is_some(),
            )));
            gate.parts[2] = Some(child(glow_part(
                membrane,
                glow_material(color, 0.0, 0.9, true),
                waiting.is_none(),
            )));
        }
        gate.parts[3] = Some(
            commands
                .spawn((
                    Name::new("Portal light"),
                    Transform::from_xyz(0.0, 0.0, 0.3),
                    PointLight {
                        color,
                        intensity_lm: 0.0,
                        range: 7.0,
                        radius: 0.4,
                    },
                    ChildOf(me),
                ))
                .id(),
        );
        if let Some((w, _)) = waiting {
            commands.entity(me).insert(Portal::new(w, size));
            commands.entity(w).insert(Portal::new(me, size));
            if let Ok((_, mut g, _)) = gates.get_mut(w) {
                g.exit = Some(me);
                // Its membrane only from behind now: from the front it shows the other place.
                if let (Some(back), Some(both)) = (g.parts[1], g.parts[2]) {
                    if let Ok((mut r, _)) = parts.get_mut(back) {
                        r.visible = true;
                    }
                    if let Ok((mut r, _)) = parts.get_mut(both) {
                        r.visible = false;
                    }
                }
            }
        } else if let Some(c) = owner {
            fresh.insert(c, (me, size));
        }
        commands.entity(me).insert(gate);
        queue.looks.push(LookCmd::Burst {
            at: center,
            up: facing,
            look: "portal".into(),
            role: Role::Impact,
            scale: 1.3,
            color,
        });
    }

    // ---- grow, pulse, light, close
    let mut closing_now: Vec<Entity> = Vec::new();
    let mut paint = Paint::new(materials.as_deref_mut(), &mut painted);
    for (e, mut g, mut t) in &mut gates {
        g.age += dt;
        if pulses.contains(&e) {
            g.pulse = 1.0;
        }
        g.pulse = (g.pulse - dt / 0.35).max(0.0);
        if g.closing.is_none() && g.age >= g.life {
            g.closing = Some(0.0);
        }
        if let Some(c) = g.closing.as_mut() {
            if *c == 0.0 {
                closing_now.push(e);
            }
            *c += dt;
        }
        let s = g.scale();
        let want = Vec3::new(s, s, 1.0);
        if t.scale != want {
            t.scale = want;
        }
        // It comes in bright and settles; a passage flares it.
        let birth = 1.0 + 1.5 * (1.0 - (g.age / 0.6).clamp(0.0, 1.0));
        let shut = g
            .closing
            .map_or(1.0, |c| (1.0 - c / CLOSE_SECS).clamp(0.0, 1.0));
        let k = birth * shut * (1.0 + 1.2 * g.pulse);
        let alone = g.exit.is_none();
        let glows = [FRAME_GLOW * k, MEMBRANE_GLOW * k, MEMBRANE_GLOW * 1.6 * k];
        for (i, glow) in glows.into_iter().enumerate() {
            if let Some(p) = g.parts[i]
                && let Ok((mut r, _)) = parts.get_mut(p)
            {
                paint.glow(p, &mut r, g.color, glow);
            }
        }
        if let Some(p) = g.parts[3]
            && let Ok(mut l) = lights.get_mut(p)
        {
            let lm =
                GATE_LIGHT_LM * s.min(1.0) * shut * (1.0 + g.pulse) * if alone { 0.6 } else { 1.0 };
            if (l.intensity_lm - lm).abs() > 1.0 {
                l.intensity_lm = lm;
            }
        }
        if g.closing.is_some_and(|c| c >= CLOSE_SECS) {
            commands.entity(e).despawn();
        }
    }
    // An end that closes takes its other end with it, with a fizzle.
    for e in closing_now {
        let Ok((_, g, t)) = gates.get(e) else {
            continue;
        };
        let (exit, color, at) = (g.exit, g.color, t.translation);
        queue.looks.push(LookCmd::Burst {
            at,
            up: Vec3::Y,
            look: "portal".into(),
            role: Role::Fizzle,
            scale: 1.0,
            color,
        });
        if let Some(x) = exit
            && let Ok((_, mut other, _)) = gates.get_mut(x)
            && other.closing.is_none()
        {
            other.closing = Some(0.0);
        }
    }

    // ---- afterimages
    for (e, mut a, mut r) in &mut ghosts {
        a.age += dt;
        let k = 1.0 - ((a.age - a.hold) / a.fade.max(1e-3)).clamp(0.0, 1.0);
        if k <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        paint.glow(e, &mut r, a.color, GHOST_GLOW * k);
    }
}

// ------------------------------------------------------------------------------- passage

/// Carry bodies through open doors: anything with health, a body or a controller that crosses
/// a linked end from the front comes out of the other end's front, turned with it, its velocity
/// too; an end alone stops it. Runs before physics (gameplay), so physics takes the move as a
/// teleport.
#[allow(clippy::type_complexity)]
pub fn pass_gates(
    time: Res<Time>,
    gates: Query<(Entity, &Gate, &Transform)>,
    mut bodies: Query<
        (
            Entity,
            &mut Transform,
            Option<&mut Velocity>,
            Option<&RigidBody>,
            Option<&mut TransformInterpolation>,
        ),
        (
            Or<(With<Vitality>, With<RigidBody>, With<CharacterController>)>,
            Without<Gate>,
            Without<ChildOf>,
            Without<Sliding>,
            Without<Rewinding>,
        ),
    >,
    mut queue: ResMut<MagicQueue>,
    mut last: Local<HashMap<Entity, (DVec3, f32)>>,
) {
    let dt = time.delta_secs();
    let doors = doors(gates.iter());
    if doors.is_empty() {
        last.clear();
        return;
    }
    let near = |p: DVec3| {
        doors
            .iter()
            .any(|d| d.frame.at.distance(p) < (d.frame.size.max_element() + 12.0) as f64)
    };
    let mut seen: Vec<Entity> = Vec::new();
    for (e, mut t, vel, rb, smooth) in &mut bodies {
        let pos = t.translation;
        if !near(pos) {
            continue;
        }
        seen.push(e);
        let Some(&(prev, cool)) = last.get(&e) else {
            last.insert(e, (pos, 0.0));
            continue;
        };
        if cool > 0.0 {
            last.insert(e, (pos, (cool - dt).max(0.0)));
            continue;
        }
        // Its feet and its middle: a crate's centre is low, a person's feet are on the floor.
        let crossed = [0.0, 0.9].iter().find_map(|lift| {
            let up = DVec3::Y * *lift;
            first_crossing(&doors, prev + up, pos + up, 0.1, &[])
        });
        let Some((i, _)) = crossed else {
            last.insert(e, (pos, 0.0));
            continue;
        };
        let door = doors[i];
        match door.carry() {
            Some(m) => {
                let (_, exit) = door.exit.unwrap();
                let out = carry_point(&m, pos) + (exit.normal() * 0.05).as_dvec3();
                let rot = carry_rotation(&door.frame, &exit, t.rotation.as_dquat());
                t.translation = out;
                t.rotation = rot.as_quat();
                // Drawn where it is now, not swept across the hall from where it went in.
                if let Some(mut s) = smooth {
                    s.reset();
                }
                if let Some(mut v) = vel {
                    let linear = capped(carry_vector(&m, v.linear));
                    let angular = carry_vector(&m, v.angular);
                    if v.linear != linear || v.angular != angular {
                        v.linear = linear;
                        v.angular = angular;
                    }
                }
                passage_looks(&mut queue, &door, if rb.is_some() { 1.0 } else { 1.2 });
                last.insert(e, (out, REENTRY_SECS));
            }
            None => {
                // A membrane: it bounces off, softly.
                t.translation = prev;
                if let Some(mut v) = vel {
                    let n = door.frame.normal();
                    let into = v.linear.dot(n);
                    if into < 0.0 {
                        v.linear -= n * into * 1.3;
                    }
                }
                queue.gate_pulses.push(door.entity);
                last.insert(e, (prev, 0.0));
            }
        }
    }
    last.retain(|e, _| seen.contains(e));
}

// ------------------------------------------------------------------------------- blink, recall

/// Where a blink from `feet` aimed at `aim` lands: toward the aim (flat), no further than
/// `reach`, short of a wall in the way, onto a ledge it aims at if that is within [`LEDGE_M`]
/// of the feet, on the ground.
pub fn blink_target(
    queries: &PhysicsQueries,
    feet: DVec3,
    aim: DVec3,
    forward: Vec3,
    reach: f32,
    exclude: &QueryFilter,
) -> DVec3 {
    let d = aim - feet;
    let flat = DVec3::new(d.x, 0.0, d.z);
    let len = flat.length();
    let (dir, dist) = if len < 0.3 {
        let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
        (f.as_dvec3(), reach as f64)
    } else {
        (flat / len, len.min(reach as f64))
    };
    // How far up it aims, when the aim is within reach (a ledge it points at).
    let rise = if len <= reach as f64 + 0.5 {
        d.y.clamp(-8.0, LEDGE_M as f64)
    } else {
        0.0
    };
    let eye = feet + DVec3::Y;
    let goal = feet + dir * dist + DVec3::Y * (1.0 + rise.max(0.0));
    let span = goal - eye;
    let n = span.length().max(1e-6);
    let way = span / n;
    let land = match queries.raycast(eye, way.as_vec3(), (n + 0.45) as f32, exclude) {
        None => goal,
        Some(h) => {
            // A face in the way: onto its top if it is a ledge it aims at, else short of it.
            let past = eye + way * (h.distance as f64 + 0.45);
            let probe = DVec3::new(past.x, feet.y + LEDGE_M as f64 + 0.3, past.z);
            let top = queries.raycast(probe, Vec3::NEG_Y, LEDGE_M + 0.3, exclude);
            match top {
                Some(top)
                    if rise > 0.2
                        && top.normal.y > 0.7
                        && top.point.y > feet.y + 0.25
                        && top.distance > 0.05 =>
                {
                    top.point + DVec3::Y * 0.05
                }
                _ => eye + way * (h.distance as f64 - 0.6).max(0.0),
            }
        }
    };
    ground_below(queries, land, exclude)
}

/// A move of a caster through space, queued by a cast (runtime::release).
#[derive(Debug, Clone)]
pub enum Travel {
    Blink {
        body: Entity,
        to: DVec3,
        look: String,
        color: [f32; 3],
    },
    Recall {
        body: Entity,
        secs: f32,
        look: String,
        color: [f32; 3],
    },
}

/// A blink under way.
#[derive(Component, Debug, Clone, Copy)]
pub struct Sliding {
    pub from: DVec3,
    pub to: DVec3,
    pub t: f32,
    pub secs: f32,
}

/// Where a caster has been: (seconds on its clock, feet, facing, health), oldest first.
#[derive(Component, Debug, Clone, Default)]
pub struct PathLog {
    pub clock: f32,
    pub next: f32,
    pub points: VecDeque<(f32, DVec3, Quat, f32)>,
}

/// A recall under way: sliding back along `path` (now first), untouchable.
#[derive(Component, Debug, Clone)]
pub struct Rewinding {
    pub path: Vec<(DVec3, Quat)>,
    pub t: f32,
    pub secs: f32,
    /// Its health then, given back at the end if it is more than now.
    pub health: Option<f32>,
}

/// Where along a polyline a share `s` (0..1) of its length lies.
fn along(path: &[(DVec3, Quat)], s: f32) -> (DVec3, Quat) {
    if path.len() < 2 {
        return path
            .first()
            .copied()
            .unwrap_or((DVec3::ZERO, Quat::IDENTITY));
    }
    let lens: Vec<f64> = path.windows(2).map(|w| w[0].0.distance(w[1].0)).collect();
    let total: f64 = lens.iter().sum();
    if total < 1e-6 {
        return *path.last().unwrap();
    }
    let mut goal = total * s.clamp(0.0, 1.0) as f64;
    for (i, l) in lens.iter().enumerate() {
        if goal <= *l || i == lens.len() - 1 {
            let k = if *l > 1e-9 { (goal / l).min(1.0) } else { 1.0 };
            let (a, b) = (path[i], path[i + 1]);
            return (a.0.lerp(b.0, k), a.1.slerp(b.1, k as f32));
        }
        goal -= l;
    }
    *path.last().unwrap()
}

/// Move casters through space: start the blinks and recalls cast this step, slide blinkers to
/// where they aimed, rewind recallers along their paths, and keep every caster's path.
#[allow(clippy::type_complexity)]
pub fn travel(
    time: Res<Time>,
    mut commands: Commands,
    mut queue: ResMut<MagicQueue>,
    mut casters: Query<
        (
            Entity,
            &mut Transform,
            Option<&mut Vitality>,
            Option<&mut PathLog>,
            Option<&mut Sliding>,
            Option<&mut Rewinding>,
            Option<&LocalTimeScale>,
        ),
        With<MagicCaster>,
    >,
) {
    for tr in std::mem::take(&mut queue.travels) {
        match tr {
            Travel::Blink {
                body,
                to,
                look,
                color,
            } => {
                let Ok((_, t, ..)) = casters.get(body) else {
                    continue;
                };
                let from = t.translation;
                commands.entity(body).try_insert(Sliding {
                    from,
                    to,
                    t: 0.0,
                    secs: BLINK_SECS,
                });
                // A streak of light from where it was to where it lands, a burst at each end
                // and its afterimage left behind.
                queue.looks.push(LookCmd::Beam {
                    from: from + DVec3::Y,
                    to: to + DVec3::Y,
                    look: look.clone(),
                    color,
                    width: 0.2,
                    secs: 0.35,
                });
                for at in [from, to] {
                    queue.looks.push(LookCmd::Burst {
                        at: at + DVec3::Y,
                        up: Vec3::Y,
                        look: look.clone(),
                        role: Role::Impact,
                        scale: 0.9,
                        color,
                    });
                }
                spawn_ghost(&mut commands, from, t.rotation, color, 0.1, 0.45);
            }
            Travel::Recall {
                body,
                secs,
                look,
                color,
            } => {
                let Ok((_, t, v, log, ..)) = casters.get(body) else {
                    continue;
                };
                let now = (t.translation, t.rotation);
                let Some(log) = log else {
                    continue;
                };
                let since = log.clock - secs;
                let mut path: Vec<(DVec3, Quat)> = vec![now];
                path.extend(
                    log.points
                        .iter()
                        .rev()
                        .filter(|p| p.0 >= since)
                        .map(|p| (p.1, p.2)),
                );
                let then = log.points.iter().find(|p| p.0 >= since);
                if path.len() < 2 || path.iter().all(|p| p.0.distance(now.0) < 0.05) {
                    queue.looks.push(LookCmd::Burst {
                        at: now.0 + DVec3::Y,
                        up: Vec3::Y,
                        look,
                        role: Role::Fizzle,
                        scale: 0.6,
                        color,
                    });
                    continue;
                }
                let health = then
                    .map(|p| p.3)
                    .filter(|h| v.is_some_and(|v| *h > v.health));
                // A line of glowing marks along the way back, each fading once it is passed,
                // and a thread of light through them.
                let total: f64 = path.windows(2).map(|w| w[0].0.distance(w[1].0)).sum();
                let marks = ((total / 1.1).ceil() as usize).clamp(2, 9);
                for k in 1..=marks {
                    let s = k as f32 / marks as f32;
                    let (p, r) = along(&path, s);
                    spawn_ghost(&mut commands, p, r, color, REWIND_SECS * s + 0.05, 0.4);
                }
                for w in path.windows(2).step_by(2) {
                    queue.looks.push(LookCmd::Beam {
                        from: w[0].0 + DVec3::Y,
                        to: w[1].0 + DVec3::Y,
                        look: look.clone(),
                        color,
                        width: 0.07,
                        secs: REWIND_SECS + 0.3,
                    });
                }
                queue.looks.push(LookCmd::Burst {
                    at: now.0 + DVec3::Y,
                    up: Vec3::Y,
                    look,
                    role: Role::Impact,
                    scale: 1.0,
                    color,
                });
                commands.entity(body).try_insert(Rewinding {
                    path,
                    t: 0.0,
                    secs: REWIND_SECS,
                    health,
                });
                commands.entity(body).try_remove::<Sliding>();
            }
        }
    }
    let dt_all = time.delta_secs();
    for (e, mut t, vit, log, slide, rewind, local) in &mut casters {
        let dt = time.delta_for(local);
        if let Some(mut r) = rewind {
            r.t += dt_all;
            let s = (r.t / r.secs).clamp(0.0, 1.0);
            // Quick to start, easing into where it was.
            let k = 1.0 - (1.0 - s) * (1.0 - s);
            let (p, q) = along(&r.path, k);
            t.translation = p;
            t.rotation = q;
            if s >= 1.0 {
                if let (Some(h), Some(mut v)) = (r.health, vit) {
                    v.health = v.health.max(h).min(v.max);
                }
                if let Some(mut log) = log {
                    log.points.clear();
                }
                commands.entity(e).try_remove::<Rewinding>();
            }
            continue;
        }
        if let Some(mut sl) = slide {
            sl.t += dt_all;
            let s = (sl.t / sl.secs).clamp(0.0, 1.0);
            let k = s * s * (3.0 - 2.0 * s);
            let hop = (std::f32::consts::PI * s).sin() as f64 * 0.25;
            t.translation = sl.from.lerp(sl.to, k as f64) + DVec3::Y * hop;
            if s >= 1.0 {
                t.translation = sl.to;
                commands.entity(e).try_remove::<Sliding>();
            }
        }
        let health = vit.map_or(0.0, |v| v.health);
        match log {
            Some(mut log) => {
                log.clock += dt;
                if log.clock >= log.next {
                    log.next = log.clock + PATH_STEP;
                    let (clock, at, rot) = (log.clock, t.translation, t.rotation);
                    log.points.push_back((clock, at, rot, health));
                    while log.points.front().is_some_and(|p| p.0 < clock - PATH_KEEP) {
                        log.points.pop_front();
                    }
                }
            }
            None => {
                commands.entity(e).try_insert(PathLog::default());
            }
        }
    }
}

/// Close every portal and afterimage at once (a hall program restarting).
pub fn clear(world: &mut World) {
    let gone: Vec<Entity> = world
        .query_filtered::<Entity, Or<(With<Gate>, With<Afterimage>)>>()
        .iter(world)
        .collect();
    for e in gone {
        world.despawn(e);
    }
    let movers: Vec<Entity> = world
        .query_filtered::<Entity, Or<(With<Sliding>, With<Rewinding>, With<PathLog>)>>()
        .iter(world)
        .collect();
    for e in movers {
        if let Ok(mut ec) = world.get_entity_mut(e) {
            ec.remove::<(Sliding, Rewinding, PathLog)>();
        }
    }
}

/// `magic.portals`: the portal ends open now, and who is blinking or rewinding.
pub fn register(app: &mut App) {
    app.register_api(
        "magic.portals",
        "Portals now: each end (where, the way its front faces, size, its other end if linked, \
         open or opening or closing, seconds left, caster), and the casters mid-blink or \
         mid-recall.",
        "{}",
        |world, _args| {
            let mut q = world.query::<(Entity, &Gate, &Transform)>();
            let ends: Vec<serde_json::Value> = q
                .iter(world)
                .map(|(e, g, t)| {
                    let n = t.rotation * Vec3::Z;
                    serde_json::json!({
                        "id": e.to_bits(),
                        "end": g.end,
                        "at": [t.translation.x, t.translation.y, t.translation.z],
                        "facing": [n.x, n.y, n.z],
                        "size_m": [g.size.x, g.size.y],
                        "exit": g.exit.map(|x| x.to_bits()),
                        "state": if g.closing.is_some() {
                            "closing"
                        } else if g.open() {
                            "open"
                        } else {
                            "opening"
                        },
                        "secs_left": (g.life - g.age).max(0.0),
                        "caster": g.caster.map(|c| c.to_bits()),
                    })
                })
                .collect();
            let mut q = world.query::<(Entity, Has<Sliding>, Has<Rewinding>)>();
            let moving: Vec<serde_json::Value> = q
                .iter(world)
                .filter(|(_, s, r)| *s || *r)
                .map(|(e, s, _)| {
                    serde_json::json!({
                        "id": e.to_bits(),
                        "doing": if s { "blink" } else { "recall" },
                    })
                })
                .collect();
            Ok(serde_json::json!({ "ends": ends, "moving": moving }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(at: DVec3, yaw_deg: f64) -> GateFrame {
        GateFrame {
            at,
            rot: DQuat::from_rotation_y(yaw_deg.to_radians()),
            size: Vec2::new(1.6, 2.6),
        }
    }

    #[test]
    fn only_the_front_of_the_opening_lets_through() {
        let g = gate(DVec3::new(0.0, 1.3, 0.0), 0.0);
        // Walking in from the front, through the middle.
        let t = g
            .crossing(DVec3::new(0.0, 1.0, 0.5), DVec3::new(0.0, 1.0, -0.5), 0.0)
            .unwrap();
        assert!((t - 0.5).abs() < 1e-9);
        // From behind, beside it, above it, or stopping short: no.
        assert!(
            g.crossing(DVec3::new(0.0, 1.0, -0.5), DVec3::new(0.0, 1.0, 0.5), 0.0)
                .is_none()
        );
        assert!(
            g.crossing(DVec3::new(1.0, 1.0, 0.5), DVec3::new(1.0, 1.0, -0.5), 0.0)
                .is_none()
        );
        assert!(
            g.crossing(DVec3::new(0.0, 2.8, 0.5), DVec3::new(0.0, 2.8, -0.5), 0.0)
                .is_none()
        );
        assert!(
            g.crossing(DVec3::new(0.0, 1.0, 0.5), DVec3::new(0.0, 1.0, 0.1), 0.0)
                .is_none()
        );
        // A body too wide for where it crosses doesn't fit.
        assert!(
            g.crossing(DVec3::new(0.6, 1.0, 0.5), DVec3::new(0.6, 1.0, -0.5), 0.3)
                .is_none()
        );
    }

    #[test]
    fn what_goes_in_one_comes_out_of_the_others_front_turned_with_it() {
        // Entry faces +z at the origin; exit 20 m east, facing east (+x).
        let entry = gate(DVec3::new(0.0, 1.3, 0.0), 0.0);
        let exit = gate(DVec3::new(20.0, 1.3, 0.0), 90.0);
        let m = carry(&entry, &exit);
        // A bolt 0.1 m in front of the entry, flying into it (-z), 0.3 m right of its middle.
        let p = carry_point(&m, DVec3::new(0.3, 1.5, 0.1));
        let v = carry_vector(&m, Vec3::new(0.0, 0.0, -26.0));
        // It comes out 0.1 m behind the exit's plane and leaves it through its front (+x).
        let l = exit.local(p);
        assert!((l.z + 0.1).abs() < 1e-9, "{l}");
        assert!((l.y - 0.2).abs() < 1e-9, "{l}");
        assert!((l.x + 0.3).abs() < 1e-9, "mirrored across: {l}");
        assert!((v - Vec3::new(26.0, 0.0, 0.0)).length() < 1e-4, "{v}");
        // A body facing into the entry faces out of the exit.
        let r = carry_rotation(&entry, &exit, DQuat::IDENTITY);
        let fwd = (r * DVec3::NEG_Z).normalize();
        assert!((fwd - DVec3::X).length() < 1e-9, "{fwd}");
        // And the way back is the inverse.
        let back = carry(&exit, &entry);
        let q = carry_point(&back, p);
        assert!((q - DVec3::new(0.3, 1.5, 0.1)).length() < 1e-9, "{q}");
    }

    #[test]
    fn an_end_grows_overshoots_a_little_and_shrinks_away() {
        let mut g = Gate {
            caster: None,
            exit: None,
            end: 0,
            size: Vec2::new(1.4, 2.4),
            age: 0.0,
            life: 20.0,
            color: VIOLET,
            look: "portal".into(),
            closing: None,
            pulse: 0.0,
            parts: [None; 4],
        };
        assert!(g.scale() < 0.05 && !g.open());
        let peak = (1..25)
            .map(|k| {
                g.age = k as f32 * OPEN_SECS / 24.0;
                g.scale()
            })
            .fold(0.0f32, f32::max);
        assert!((1.02..1.15).contains(&peak), "{peak}");
        g.age = 1.0;
        assert!((g.scale() - 1.0).abs() < 1e-5 && g.open());
        g.closing = Some(CLOSE_SECS * 0.5);
        assert!(g.scale() < 0.8 && !g.open());
        g.closing = Some(CLOSE_SECS);
        assert!(g.scale() <= 0.02);
    }

    #[test]
    fn a_path_is_walked_by_its_length() {
        let path = [
            (DVec3::ZERO, Quat::IDENTITY),
            (DVec3::new(1.0, 0.0, 0.0), Quat::IDENTITY),
            (DVec3::new(1.0, 0.0, 3.0), Quat::IDENTITY),
        ];
        assert!(along(&path, 0.0).0.distance(DVec3::ZERO) < 1e-9);
        assert!(along(&path, 0.25).0.distance(DVec3::new(1.0, 0.0, 0.0)) < 1e-9);
        assert!(along(&path, 0.5).0.distance(DVec3::new(1.0, 0.0, 1.0)) < 1e-9);
        assert!(along(&path, 1.0).0.distance(DVec3::new(1.0, 0.0, 3.0)) < 1e-9);
    }

    #[test]
    fn frame_and_membrane_meshes_are_whole() {
        for m in [frame_mesh(1.4, 2.4), membrane_mesh(1.4, 2.4)] {
            assert!(m.indices.len() % 3 == 0 && !m.indices.is_empty());
            assert!(m.indices.iter().all(|i| (*i as usize) < m.vertices.len()));
        }
        // The membrane faces -z.
        let m = membrane_mesh(1.4, 2.4);
        let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
        let (a, b, c) = (p(m.indices[0]), p(m.indices[1]), p(m.indices[2]));
        assert!((b - a).cross(c - a).z < 0.0);
    }
}

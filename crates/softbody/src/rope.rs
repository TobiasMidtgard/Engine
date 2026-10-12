//! Ropes, cables and rope bridges.
//!
//! A [`Rope`] is a chain of particles with (nearly) rigid segments and a little bending
//! stiffness, tied at either end to a point in the world, to its own entity or to another
//! entity. Ends tied to dynamic rigid bodies pull them (a tow cable, a grappling line, a
//! wrecking ball's chain), through `ApplyImpulse` from the physics contract. Ropes can be cut
//! with `softbody.cut` or by a `ProjectileTrace` passing through them.
//!
//! A [`RopeBridge`] is two deck ropes carrying planks, two hand ropes and the suspenders
//! between them. The planks are kinematic physics bodies the bridge moves every step, so
//! characters stand and walk on them through the physics module, and their weight (from
//! `CharacterState::ground_entity`, or a [`SoftLoad`]) bends the bridge. Cut a deck rope and
//! the bridge swings down from its other end.

use engine_core::prelude::*;

use crate::body::{FrameSource, SoftBodySim, SoftKind, Tube};
use crate::settings::{Pattern, SimSettings, SoftMaterial};
use crate::solver::{Goal, Sim};

/// Where a rope end is tied.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RopeEnd {
    /// Hangs free.
    Free,
    /// At `offset` (meters, local space) from the rope entity's own origin; follows the entity.
    Here { offset: [f32; 3] },
    /// At a fixed world position.
    World { at: [f64; 3] },
    /// At `offset` in another entity's local space. Dynamic rigid bodies get pulled.
    Entity { entity: Entity, offset: [f32; 3] },
}

impl Default for RopeEnd {
    fn default() -> Self {
        RopeEnd::Here { offset: [0.0; 3] }
    }
}

/// A rope or cable. Add it to an entity with a `Transform`; ends are tied as `end_a` and
/// `end_b` say.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rope {
    pub end_a: RopeEnd,
    pub end_b: RopeEnd,
    /// Length, meters. With both ends tied, any length beyond the gap is slack.
    pub length: f32,
    /// Particles along the rope (2 or more). About one per 10–25 cm looks smooth.
    pub segments: u32,
    /// Drawn and collision radius, meters.
    pub radius: f32,
    /// Linear density, kg/m (climbing rope 0.07, steel cable 0.5, chain 2).
    pub mass_per_m: f32,
    /// Stretch compliance, m/N (0 = doesn't stretch).
    pub stretch_compliance: f32,
    /// Bending compliance, m/N (rope 1e-2, cable 1e-4).
    pub bend_compliance: f32,
    /// Bullets and `softbody.cut` can cut it.
    pub cuttable: bool,
    /// Beads threaded on it (bead curtains, necklaces); `radius` is then the thread.
    pub beads: Option<Beads>,
    pub sim: SimSettings,
    pub material: SoftMaterial,
}

/// Beads threaded on a rope, drawn at even spacing along it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beads {
    /// Center-to-center spacing, meters.
    pub spacing: f32,
    /// Bead radius, meters.
    pub radius: f32,
    /// Length along the rope relative to the width (1 round, 1.5 a barrel, 0.6 a disc).
    pub elongation: f32,
    /// Facets around a cut-glass bead; 0 = smooth.
    pub facets: u32,
}

impl Default for Beads {
    fn default() -> Self {
        Self {
            spacing: 0.05,
            radius: 0.014,
            elongation: 1.0,
            facets: 0,
        }
    }
}

impl Default for Rope {
    fn default() -> Self {
        Self {
            end_a: RopeEnd::default(),
            end_b: RopeEnd::Free,
            length: 3.0,
            segments: 20,
            radius: 0.015,
            mass_per_m: 0.08,
            stretch_compliance: 0.0,
            bend_compliance: 1e-2,
            cuttable: true,
            beads: None,
            sim: SimSettings {
                substeps: 16,
                damping: 0.1,
                thickness: 0.015,
                ..SimSettings::default()
            },
            material: SoftMaterial::hex(0x8a7350)
                .with_pattern(Pattern::Rope, 0x8a7350, 0x8a7350)
                .with_sheen(0.2)
                .with_wrinkles(0.0),
        }
    }
}

/// A walkable rope bridge spanning from the entity's origin along its local +X axis.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RopeBridge {
    /// Span, meters: the far end is at local `(length, 0, 0)`.
    pub length: f32,
    /// Deck width, meters.
    pub width: f32,
    pub planks: u32,
    /// How far the middle of the deck hangs below its ends, meters.
    pub sag: f32,
    /// Height of the hand ropes above the deck ends, meters.
    pub rail_height: f32,
    pub rope_radius: f32,
    /// Plank thickness, meters.
    pub plank_thickness: f32,
    /// Gap between planks along the span, meters.
    pub plank_gap: f32,
    pub plank_mass_kg: f32,
    /// Plank color (sRGB hex) and roughness, for the default plain plank.
    pub plank_color: u32,
    pub plank_roughness: f32,
    /// Asset key of a custom plank mesh, modeled as a unit cube (−0.5..0.5) and stretched to
    /// each plank's size; `None` draws a plain box.
    pub plank_mesh: Option<String>,
    /// Asset key of a custom plank material; `None` uses `plank_color` and `plank_roughness`.
    pub plank_material: Option<String>,
    pub sim: SimSettings,
    pub material: SoftMaterial,
}

impl Default for RopeBridge {
    fn default() -> Self {
        Self {
            length: 20.0,
            width: 1.4,
            planks: 24,
            sag: 1.2,
            rail_height: 1.0,
            rope_radius: 0.025,
            plank_thickness: 0.06,
            plank_gap: 0.12,
            plank_mass_kg: 6.0,
            plank_color: 0x7a5636,
            plank_roughness: 0.85,
            plank_mesh: None,
            plank_material: None,
            sim: SimSettings {
                substeps: 16,
                damping: 0.25,
                thickness: 0.03,
                wind_scale: 0.5,
                prewarm_secs: 2.0,
                ..SimSettings::default()
            },
            material: SoftMaterial::hex(0x9a8158)
                .with_pattern(Pattern::Rope, 0x9a8158, 0x9a8158)
                .with_sheen(0.2)
                .with_wrinkles(0.0),
        }
    }
}

/// Marks a plank entity driven by a [`RopeBridge`].
#[derive(Component, Debug, Clone, Copy)]
pub struct BridgePlank {
    pub bridge: Entity,
    pub index: u32,
}

/// Puts weight on rope bridges (and anything else that carries loads) without a physics
/// character controller: a scripted walker, a crate placed by gameplay. The load acts at the
/// entity's position, which should be where it touches the deck.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftLoad {
    pub mass_kg: f32,
}

impl Default for SoftLoad {
    fn default() -> Self {
        Self { mass_kg: 80.0 }
    }
}

/// Segments of ropes that can be cut.
#[derive(Debug, Clone, Default)]
pub struct Lines {
    /// Particle pairs; the distance constraint for segment `s` is tagged `s`.
    pub segments: Vec<[u32; 2]>,
    pub alive: Vec<bool>,
    /// Bend constraints (tagged `segments.len() + k`) span these two segments.
    pub bend_support: Vec<[u32; 2]>,
    /// Drawn ropes: segment indices in order, and radius.
    pub chains: Vec<(Vec<u32>, f32)>,
    pub cuttable: bool,
    /// Beads drawn on every chain.
    pub beads: Option<Beads>,
}

impl Lines {
    /// Add a chain through `particles` (connected in order). Returns the chain index.
    pub fn add_chain(
        &mut self,
        sim: &mut Sim,
        particles: &[u32],
        radius: f32,
        stretch_compliance: f32,
        bend_compliance: Option<f32>,
    ) -> usize {
        let mut segs = Vec::with_capacity(particles.len().saturating_sub(1));
        for w in particles.windows(2) {
            let s = self.segments.len() as u32;
            self.segments.push([w[0], w[1]]);
            self.alive.push(true);
            sim.add_distance(w[0], w[1], stretch_compliance, s);
            segs.push(s);
        }
        self.chains.push((segs, radius));
        if let Some(bend) = bend_compliance {
            for k in 2..particles.len() {
                // Tag is fixed up after all chains are known (see `finish`).
                self.bend_support.push([
                    self.chains.last().unwrap().0[k - 2],
                    self.chains.last().unwrap().0[k - 1],
                ]);
                sim.add_distance(particles[k - 2], particles[k], bend, u32::MAX - 1);
            }
        }
        self.chains.len() - 1
    }

    /// Give bend constraints their final tags once every segment exists.
    pub fn finish(&self, sim: &mut Sim) {
        let base = self.segments.len() as u32;
        let mut k = 0u32;
        for d in &mut sim.distances {
            if d.tag == u32::MAX - 1 {
                d.tag = base + k;
                k += 1;
            }
        }
    }

    /// Drawn tubes: each chain split wherever it was cut.
    pub fn tubes(&self) -> Vec<Tube> {
        let mut out = Vec::new();
        for (segs, radius) in &self.chains {
            let mut run: Vec<u32> = Vec::new();
            for &s in segs {
                let [a, b] = self.segments[s as usize];
                if self.alive[s as usize] {
                    if run.is_empty() {
                        run.push(a);
                    }
                    run.push(b);
                } else if run.len() >= 2 {
                    out.push(Tube {
                        particles: std::mem::take(&mut run),
                        radius: *radius,
                        beads: self.beads,
                    });
                } else {
                    run.clear();
                }
            }
            if run.len() >= 2 {
                out.push(Tube {
                    particles: run,
                    radius: *radius,
                    beads: self.beads,
                });
            }
        }
        out
    }

    /// Cut segments (and the bends across them). Returns true if anything was cut.
    pub fn cut(&mut self, sim: &mut Sim, segments: &[u32]) -> bool {
        let mut changed = false;
        for &s in segments {
            if let Some(a) = self.alive.get_mut(s as usize)
                && *a
            {
                *a = false;
                changed = true;
            }
        }
        if changed {
            let n = self.segments.len() as u32;
            sim.distances.retain(|d| {
                if d.tag < n {
                    self.alive[d.tag as usize]
                } else if let Some(s) = self.bend_support.get((d.tag - n) as usize) {
                    self.alive[s[0] as usize] && self.alive[s[1] as usize]
                } else {
                    true
                }
            });
            sim.tethers.clear();
        }
        changed
    }

    /// Segments within `radius` of the segment `from → to` (body-local), considering each
    /// rope's own radius.
    pub fn segments_near(&self, sim: &Sim, from: Vec3, to: Vec3, radius: f32) -> Vec<u32> {
        let mut out = Vec::new();
        for (segs, r) in &self.chains {
            for &s in segs {
                if !self.alive[s as usize] {
                    continue;
                }
                let [a, b] = self.segments[s as usize];
                let d = segment_distance(from, to, sim.pos[a as usize], sim.pos[b as usize]);
                if d < radius + r {
                    out.push(s);
                }
            }
        }
        out
    }
}

/// Closest distance between segments `p0 p1` and `q0 q1`.
pub fn segment_distance(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> f32 {
    let d1 = p1 - p0;
    let d2 = q1 - q0;
    let r = p0 - q0;
    let a = d1.length_squared();
    let e = d2.length_squared();
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-12 && e <= 1e-12 {
        return r.length();
    }
    if a <= 1e-12 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-12 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-12 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    ((p0 + d1 * s) - (q0 + d2 * t)).length()
}

/// Runtime state of a rope bridge.
#[derive(Debug, Clone, Default)]
pub struct BridgeState {
    /// Plank entity and its left/right deck particles.
    pub planks: Vec<(Entity, u32, u32)>,
    /// Plank half extents (along the span, thickness, across), meters.
    pub plank_half: Vec3,
    /// World pose per plank after the last step.
    pub plank_poses: Vec<(DVec3, Quat)>,
    pub lines: Lines,
}

impl BridgeState {
    /// Pose of plank `k` from its deck particles (body-local center, rotation).
    pub fn plank_pose(&self, sim: &Sim, k: usize, rope_radius: f32) -> (Vec3, Quat) {
        let (_, l, r) = self.planks[k];
        let (pl, pr) = (sim.pos[l as usize], sim.pos[r as usize]);
        let across = (pr - pl).normalize_or(Vec3::Z);
        let prev = if k > 0 { k - 1 } else { k };
        let next = (k + 1).min(self.planks.len() - 1);
        let mid = |i: usize| {
            let (_, a, b) = self.planks[i];
            (sim.pos[a as usize] + sim.pos[b as usize]) * 0.5
        };
        let mut along = mid(next) - mid(prev);
        if along.length_squared() < 1e-8 {
            along = Vec3::X;
        }
        let along = (along - across * along.dot(across)).normalize_or(Vec3::X);
        let up = across.cross(along).normalize_or(Vec3::Y);
        let rot = Quat::from_mat3(&Mat3::from_cols(along, up, across)).normalize();
        let center = (pl + pr) * 0.5 + up * (self.plank_half.y + rope_radius);
        (center, rot)
    }

    /// Which plank (if any) a world point stands on: the point must be over the plank's
    /// footprint and within 0.5 m above its top. Returns the plank index and how far across
    /// (0 = left rope, 1 = right rope).
    pub fn plank_under(&self, p: DVec3) -> Option<(usize, f32)> {
        let h = self.plank_half;
        self.plank_poses.iter().enumerate().find_map(|(k, (c, r))| {
            let local = r.conjugate() * (p - *c).as_vec3();
            let over = local.x.abs() <= h.x * 1.6 && local.z.abs() <= h.z * 1.05;
            let near = local.y > -h.y - 0.1 && local.y < h.y + 0.5;
            (over && near).then(|| (k, (local.z / (2.0 * h.z) + 0.5).clamp(0.0, 1.0)))
        })
    }

    /// Height of the deck surface under a world point, if it is over the bridge.
    pub fn deck_height(&self, p: DVec3) -> Option<f64> {
        let h = self.plank_half;
        let mut best: Option<(f32, f64)> = None;
        for (c, r) in &self.plank_poses {
            let local = r.conjugate() * (p - *c).as_vec3();
            if local.z.abs() > h.z * 1.05 || local.x.abs() > h.x * 2.5 {
                continue;
            }
            let top = *c + (*r * Vec3::new(local.x, h.y, local.z)).as_dvec3();
            let d = local.x.abs();
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, top.y));
            }
        }
        best.map(|(_, y)| y)
    }
}

/// Initial layout between two points: a parabola sagging enough to use up `length`.
fn hang(a: Vec3, b: Vec3, length: f32, n: usize, down: Vec3) -> Vec<Vec3> {
    let span = (b - a).length();
    let slack = (length - span).max(0.0);
    let sag = (3.0 * span * slack / 8.0).sqrt();
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            a.lerp(b, t) + down * (sag * 4.0 * t * (1.0 - t))
        })
        .collect()
}

/// Build a rope on `entity` at `transform`. `resolve` gives the world transform of other
/// entities a rope end is tied to.
pub fn build_rope(
    spec: &Rope,
    entity: Entity,
    transform: &GlobalTransform,
    resolve: impl Fn(Entity) -> Option<GlobalTransform>,
) -> Result<SoftBodySim, String> {
    let n = spec.segments.max(2) as usize;
    let origin = transform.translation();
    let mut ends = Vec::new();
    for end in [spec.end_a, spec.end_b] {
        let info = match end {
            RopeEnd::Free => None,
            RopeEnd::Here { offset } => {
                let p = transform.transform_point(Vec3::from(offset).as_dvec3());
                Some((
                    (p - origin).as_vec3(),
                    FrameSource::Entity(entity),
                    transform.rotation(),
                    Vec3::ZERO,
                    false,
                ))
            }
            RopeEnd::World { at } => Some((
                (DVec3::from_array(at) - origin).as_vec3(),
                FrameSource::World,
                Quat::IDENTITY,
                Vec3::ZERO,
                false,
            )),
            RopeEnd::Entity {
                entity: other,
                offset,
            } => {
                let Some(t) = resolve(other) else {
                    return Err(format!(
                        "Rope end is tied to entity {other:?}, which has no transform."
                    ));
                };
                let p = t.transform_point(Vec3::from(offset).as_dvec3());
                Some((
                    (p - origin).as_vec3(),
                    FrameSource::Entity(other),
                    t.rotation(),
                    (t.translation() - origin).as_vec3(),
                    true,
                ))
            }
        };
        ends.push(info);
    }
    let down = Vec3::NEG_Y;
    let length = spec.length.max(0.01);
    let points = match (&ends[0], &ends[1]) {
        (Some(a), Some(b)) => hang(a.0, b.0, length, n, down),
        (Some(a), None) | (None, Some(a)) => {
            let mut pts: Vec<Vec3> = (0..n)
                .map(|i| a.0 + down * (length * i as f32 / (n - 1) as f32))
                .collect();
            if ends[0].is_none() {
                pts.reverse();
            }
            pts
        }
        (None, None) => (0..n)
            .map(|i| Vec3::X * (length * i as f32 / (n - 1) as f32))
            .collect(),
    };
    let mut sim = Sim::new(origin);
    let mass = spec.mass_per_m.max(1e-4) * length / n as f32;
    let ids: Vec<u32> = points.iter().map(|p| sim.add_particle(*p, mass)).collect();
    let mut lines = Lines {
        cuttable: spec.cuttable,
        ..Lines::default()
    };
    lines.add_chain(
        &mut sim,
        &ids,
        spec.radius,
        spec.stretch_compliance,
        Some(spec.bend_compliance),
    );
    lines.finish(&mut sim);

    let mut frames = Vec::new();
    for (k, end) in ends.iter().enumerate() {
        let Some((pos, src, rot, frame_pos, dynamic)) = end else {
            continue;
        };
        let p = if k == 0 { ids[0] } else { ids[n - 1] };
        sim.pos[p as usize] = *pos;
        frames.push(*src);
        let f = sim.add_frame(*frame_pos, *rot);
        let local = rot.conjugate() * (*pos - *frame_pos);
        if *dynamic {
            // Soft but very stiff, so the rope pulls the body back (two-way).
            sim.goals.push(Goal {
                p,
                frame: f,
                local,
                compliance: 1e-7,
            });
        } else {
            sim.pin(p, f, local);
        }
    }
    let drawn = spec
        .beads
        .map_or(spec.radius, |b| b.radius.max(spec.radius));
    sim.params.radius = drawn.max(spec.sim.thickness);
    sim.params.drag = 0.4;
    let mut body = SoftBodySim::new(SoftKind::Rope, sim, spec.sim, spec.material);
    body.frames = frames;
    lines.beads = spec.beads;
    body.tubes = lines.tubes();
    body.bridge = Some(BridgeState {
        lines,
        ..BridgeState::default()
    });
    Ok(body)
}

/// Build a rope bridge on `entity`. `spawn_plank(index, half_extents)` creates a plank entity.
pub fn build_bridge(
    spec: &RopeBridge,
    entity: Entity,
    transform: &GlobalTransform,
    mut spawn_plank: impl FnMut(u32, Vec3) -> Entity,
) -> Result<SoftBodySim, String> {
    let planks = spec.planks.clamp(2, 400) as usize;
    let origin = transform.translation();
    let rot = transform.rotation();
    let to_local = |p: Vec3| rot * p;
    let length = spec.length.max(1.0);
    let half_w = spec.width.max(0.2) * 0.5;
    let spacing = length / (planks + 1) as f32;
    let depth = (spacing - spec.plank_gap).max(0.05);
    let plank_half = Vec3::new(depth * 0.5, spec.plank_thickness * 0.5, half_w + 0.05);
    let mut sim = Sim::new(origin);
    let rope_mass = 0.15 * spacing;
    let sag = |x: f32, s: f32| -s * 4.0 * (x / length) * (1.0 - x / length);

    // Deck: anchors at both ends plus a left/right particle per plank.
    let mut left = Vec::new();
    let mut right = Vec::new();
    for k in 0..planks + 2 {
        let x = k as f32 * spacing;
        let y = sag(x, spec.sag);
        let anchor = k == 0 || k == planks + 1;
        let m = if anchor {
            rope_mass
        } else {
            rope_mass + spec.plank_mass_kg * 0.5
        };
        left.push(sim.add_particle(to_local(Vec3::new(x, y, -half_w)), m));
        right.push(sim.add_particle(to_local(Vec3::new(x, y, half_w)), m));
    }
    // Hand ropes above the deck particles.
    let rail_sag = spec.sag * 0.55;
    let mut rail_l = Vec::new();
    let mut rail_r = Vec::new();
    for k in 0..planks + 2 {
        let x = k as f32 * spacing;
        let y = spec.rail_height + sag(x, rail_sag);
        rail_l.push(sim.add_particle(to_local(Vec3::new(x, y, -half_w - 0.05)), rope_mass));
        rail_r.push(sim.add_particle(to_local(Vec3::new(x, y, half_w + 0.05)), rope_mass));
    }
    let mut lines = Lines {
        cuttable: true,
        ..Lines::default()
    };
    let r = spec.rope_radius;
    lines.add_chain(&mut sim, &left, r, 0.0, Some(5e-3));
    lines.add_chain(&mut sim, &right, r, 0.0, Some(5e-3));
    lines.add_chain(&mut sim, &rail_l, r, 0.0, Some(1e-2));
    lines.add_chain(&mut sim, &rail_r, r, 0.0, Some(1e-2));
    // Suspenders from the hand ropes to the deck, every other plank.
    for k in (1..=planks).step_by(2) {
        lines.add_chain(&mut sim, &[rail_l[k], left[k]], r * 0.6, 0.0, None);
        lines.add_chain(&mut sim, &[rail_r[k], right[k]], r * 0.6, 0.0, None);
    }
    // Soft diagonals between neighboring planks resist shearing. They span one segment of
    // each deck rope and go when either is cut.
    let segs_per_rope = (planks + 1) as u32;
    for k in 0..=planks {
        for (a, b) in [(left[k], right[k + 1]), (right[k], left[k + 1])] {
            lines
                .bend_support
                .push([k as u32, segs_per_rope + k as u32]);
            sim.add_distance(a, b, 2e-4, u32::MAX - 1);
        }
    }
    lines.finish(&mut sim);
    // Planks hold the two deck ropes apart rigidly.
    let mut plank_list = Vec::with_capacity(planks);
    for k in 1..=planks {
        sim.add_distance(left[k], right[k], 0.0, u32::MAX);
        let e = spawn_plank((k - 1) as u32, plank_half);
        plank_list.push((e, left[k], right[k]));
    }
    // Pin the four corners and the hand rope ends to the bridge entity.
    let frame = sim.add_frame(Vec3::ZERO, rot);
    for p in [
        left[0],
        right[0],
        left[planks + 1],
        right[planks + 1],
        rail_l[0],
        rail_r[0],
        rail_l[planks + 1],
        rail_r[planks + 1],
    ] {
        sim.pin_here(p, frame);
    }
    sim.params.radius = spec.sim.thickness;
    sim.params.drag = 0.2;
    let mut body = SoftBodySim::new(SoftKind::Bridge, sim, spec.sim, spec.material);
    body.frames = vec![FrameSource::Entity(entity)];
    body.tubes = lines.tubes();
    let mut state = BridgeState {
        planks: plank_list,
        plank_half,
        plank_poses: Vec::new(),
        lines,
    };
    state.plank_poses = (0..planks)
        .map(|k| {
            let (c, q) = state.plank_pose(&body.sim, k, r);
            (origin + c.as_dvec3(), q)
        })
        .collect();
    body.ignore = state.planks.iter().map(|p| p.0).collect();
    body.bridge = Some(state);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> SoftBodySim {
        let spec = RopeBridge {
            length: 12.0,
            planks: 14,
            ..RopeBridge::default()
        };
        let mut next = 1000u32;
        build_bridge(
            &spec,
            Entity::PLACEHOLDER,
            &GlobalTransform::from(Transform::from_xyz(0.0, 5.0, 0.0)),
            |_, _| {
                next += 1;
                Entity::from_raw_u32(next).unwrap()
            },
        )
        .unwrap()
    }

    fn settle(body: &mut SoftBodySim, steps: usize) {
        body.sim.params.damping = 1.0;
        for _ in 0..steps {
            body.sim.step(1.0 / 60.0, &[]);
        }
    }

    fn deck_mid_y(body: &SoftBodySim) -> f32 {
        let b = body.bridge.as_ref().unwrap();
        let k = b.planks.len() / 2;
        b.plank_pose(&body.sim, k, 0.025).0.y
    }

    #[test]
    fn segment_distance_is_correct() {
        let d = segment_distance(
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, -1.0),
            Vec3::new(0.0, 1.0, 1.0),
        );
        assert!((d - 1.0).abs() < 1e-5);
        let d = segment_distance(
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
        );
        assert!((d - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bridge_sags_under_a_load_and_drops_when_cut() {
        let mut body = bridge();
        settle(&mut body, 300);
        let rest = deck_mid_y(&body);
        assert!(rest < -0.8 && rest > -2.5, "deck middle at {rest}");
        // An 80 kg walker in the middle.
        for _ in 0..300 {
            let b = body.bridge.as_ref().unwrap();
            let (_, l, r) = b.planks[b.planks.len() / 2];
            body.sim.loads.push((l, Vec3::new(0.0, -9.81 * 40.0, 0.0)));
            body.sim.loads.push((r, Vec3::new(0.0, -9.81 * 40.0, 0.0)));
            body.sim.step(1.0 / 60.0, &[]);
        }
        let loaded = deck_mid_y(&body);
        assert!(loaded < rest - 0.05, "loaded {loaded} vs rest {rest}");
        // Cut both deck ropes and both hand ropes at the far end.
        let b = body.bridge.as_mut().unwrap();
        let last: Vec<u32> = b
            .lines
            .chains
            .iter()
            .take(4)
            .map(|(segs, _)| *segs.last().unwrap())
            .collect();
        assert!(b.lines.cut(&mut body.sim, &last));
        body.tubes = b.lines.tubes();
        settle(&mut body, 240);
        let dropped = deck_mid_y(&body);
        assert!(dropped < loaded - 1.0, "dropped {dropped} vs {loaded}");
        assert!(body.sim.pos.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn rope_cut_splits_the_tube() {
        let spec = Rope {
            end_b: RopeEnd::World {
                at: [4.0, 0.0, 0.0],
            },
            length: 5.0,
            ..Rope::default()
        };
        let mut body = build_rope(
            &spec,
            Entity::PLACEHOLDER,
            &GlobalTransform::default(),
            |_| None,
        )
        .unwrap();
        assert_eq!(body.tubes.len(), 1);
        let b = body.bridge.as_mut().unwrap();
        let hit = b.lines.segments_near(
            &body.sim,
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(2.0, -3.0, 0.0),
            0.01,
        );
        assert_eq!(hit.len(), 1, "{hit:?}");
        assert!(b.lines.cut(&mut body.sim, &hit));
        body.tubes = b.lines.tubes();
        assert_eq!(body.tubes.len(), 2);
    }
}

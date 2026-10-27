//! Cloth: flags, capes, curtains, sails, banners and clothing.
//!
//! A [`Cloth`] is a grid of particles joined by stretch, shear and bend constraints, with
//! aerodynamic triangles so wind fills it like fabric instead of pushing it like a rope.
//! Long-range tethers keep it from stretching when its anchor moves fast. It can be pinned to
//! its own entity (a flag pole, a curtain rail, a mast) or to the bones of an animated
//! character (a cape, a skirt).
//!
//! Cloth tears where it stretches past [`Cloth::tear_ratio`], gets holes where a
//! `ProjectileTrace` passes through it, and burns away from where heat or an
//! `IgniteRequest` reaches it; rain makes it heavier and darker. The solver reports
//! per-vertex tension, which the renderer turns into wrinkles where the fabric bunches up.

use std::collections::HashMap;
use std::f32::consts::PI;

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;

use crate::body::{FrameSource, SoftBodySim, SoftKind, Surface};
use crate::settings::{SimSettings, SoftMaterial};
use crate::solver::{Aero, Goal, Sim, Tether};

/// What holds a cloth up.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum ClothPins {
    /// Nothing: it falls (a dropped towel, a tarp).
    None,
    /// The top edge, every `every`-th particle (curtain rings: 2–4; a rail: 1).
    TopEdge { every: u32 },
    /// The left edge: a flag on a pole.
    #[default]
    LeftEdge,
    /// The two top corners (a banner on two hooks, laundry).
    TopCorners,
    /// The top `rows` rows, each particle tied to whichever of `bones` is nearest when built,
    /// on the `SkeletonPose` of the nearest ancestor that has one (capes, skirts, sashes).
    Bones { rows: u32, bones: Vec<String> },
    /// Specific grid points `[column, row]`.
    Points(Vec<[u32; 2]>),
    /// Whole edges: a gaff sail laced to its mast, gaff and boom (left, top, bottom), a
    /// trampoline on all four.
    Edges {
        left: bool,
        top: bool,
        right: bool,
        bottom: bool,
    },
}

/// Simulated cloth. Add it to an entity (with a `Transform`) and the plugin builds the
/// simulation in place.
///
/// The cloth lies in the entity's local XY plane: `u` runs along +X, `v` runs down along −Y,
/// the front faces +Z, and the point `anchor` (in 0..1 of the size) sits at the entity's
/// origin. Pins follow the entity, so a flag on a moving ship stays on its mast. With a
/// [`panel`](Self::panel) the cloth is that four-sided shape instead.
///
/// On an entity carried by a dynamic rigid body (the entity itself or an ancestor), the
/// pins hold the cloth with very stiff springs instead, and the wind on the cloth pushes the
/// body through them: a sail drives its boat.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cloth {
    /// Width and height, meters.
    pub size: [f32; 2],
    /// Particles along the width and the height (2 or more each).
    pub resolution: [u32; 2],
    /// The point of the cloth at the entity origin: [0, 0] top-left (flags),
    /// [0.5, 0] top-center (curtains, capes).
    pub anchor: [f32; 2],
    /// Corners of a four-sided panel in the entity's local XY plane, meters: top-left,
    /// top-right, bottom-right, bottom-left (a gaff or lug sail, a kite, an awning). `u` runs
    /// from the left edge to the right one, `v` from the top edge down. `None` = the `size`
    /// rectangle placed by `anchor`.
    pub panel: Option<[[f32; 2]; 4]>,
    /// Belly sewn into the cloth, as a fraction of its width (sails 0.08–0.12): deepest two
    /// fifths of the way across, fading to every edge, bulging toward +Z. Wind from the front
    /// pops it through to the other side. 0 = flat.
    pub camber: f32,
    pub pins: ClothPins,
    /// Width the cloth is gathered to along its top, as a fraction of `size[0]` (curtains on
    /// rings ≈ 0.7). Below 1 it starts in folds between the pins. 1 = flat.
    pub gather: f32,
    /// Areal density, kg/m² (silk 0.05, cotton 0.15, velvet 0.3, canvas 0.4).
    pub mass_per_m2: f32,
    /// Stretch compliance, m/N (0 = doesn't stretch at all).
    pub stretch_compliance: f32,
    /// Shear compliance, m/N.
    pub shear_compliance: f32,
    /// Bending compliance, m/N (silk 1e-2 folds easily, canvas 1e-4 stays flat).
    pub bend_compliance: f32,
    /// Tears where stretched past this ratio (1.6 = 60 % longer than at rest). `None` never tears.
    pub tear_ratio: Option<f32>,
    /// Bullets and arrows (`ProjectileTrace`) punch holes.
    pub pierceable: bool,
    /// Burns away when heated above ~250 °C or hit by an `IgniteRequest`.
    pub flammable: bool,
    /// Pressure drag across the cloth (≈ 1.2 for fabric). 0 disables aerodynamics.
    pub aero_drag: f32,
    /// Skin friction along the cloth (makes flags stream out).
    pub aero_friction: f32,
    /// Lift across the wind when the cloth meets it at an angle (sails ≈ 1). 0 = none.
    pub aero_lift: f32,
    pub sim: SimSettings,
    pub material: SoftMaterial,
}

impl Default for Cloth {
    fn default() -> Self {
        Self {
            size: [2.0, 1.4],
            resolution: [20, 14],
            anchor: [0.0, 0.0],
            panel: None,
            camber: 0.0,
            pins: ClothPins::LeftEdge,
            gather: 1.0,
            mass_per_m2: 0.15,
            stretch_compliance: 1e-6,
            shear_compliance: 2e-2,
            bend_compliance: 2e-3,
            tear_ratio: None,
            pierceable: true,
            flammable: true,
            aero_drag: 1.2,
            aero_friction: 0.04,
            aero_lift: 0.0,
            sim: SimSettings::default(),
            material: SoftMaterial::default(),
        }
    }
}

impl Cloth {
    /// A flag of `size` on a pole along its left edge. Fine (8 cm cells), supple bunting that
    /// lifts across the wind: ripples run down it in waves, as a flag flies, instead of the
    /// whole flag swinging out stiff like a board.
    pub fn flag(size: [f32; 2], material: SoftMaterial) -> Self {
        let res_u = ((size[0] / 0.08).round() as u32).clamp(6, 48);
        let res_v = ((size[1] / 0.08).round() as u32).clamp(4, 32);
        Self {
            size,
            resolution: [res_u, res_v],
            mass_per_m2: 0.2,
            bend_compliance: 2e-2,
            aero_lift: 0.8,
            material,
            sim: SimSettings {
                prewarm_secs: 2.0,
                ..SimSettings::default()
            },
            ..Self::default()
        }
    }

    /// A curtain hanging from rings along its top edge, gathered into folds.
    pub fn curtain(size: [f32; 2], material: SoftMaterial) -> Self {
        let res_u = ((size[0] / 0.1).round() as u32).clamp(8, 64);
        let res_v = ((size[1] / 0.1).round() as u32).clamp(8, 64);
        Self {
            size,
            resolution: [res_u, res_v],
            anchor: [0.5, 0.0],
            pins: ClothPins::TopEdge { every: 3 },
            gather: 0.7,
            mass_per_m2: 0.3,
            bend_compliance: 5e-3,
            material,
            sim: SimSettings {
                wind_scale: 0.3,
                damping: 0.2,
                prewarm_secs: 2.0,
                self_collide: true,
                ..SimSettings::default()
            },
            ..Self::default()
        }
    }

    /// A cape tied to a character's shoulders and upper chest.
    pub fn cape(size: [f32; 2], material: SoftMaterial, bones: &[&str]) -> Self {
        Self {
            size,
            resolution: [14, 20],
            anchor: [0.5, 0.0],
            pins: ClothPins::Bones {
                rows: 1,
                bones: bones.iter().map(|b| b.to_string()).collect(),
            },
            mass_per_m2: 0.35,
            bend_compliance: 4e-3,
            pierceable: true,
            material,
            sim: SimSettings {
                substeps: 16,
                follow: 0.25,
                thickness: 0.02,
                friction: 0.3,
                damping: 0.15,
                prewarm_secs: 1.0,
                self_collide: true,
                ..SimSettings::default()
            },
            ..Self::default()
        }
    }

    /// A fore-and-aft sail laced along its luff (left edge), head (top) and foot (bottom),
    /// with the leech free. `corners` are throat, peak, clew and tack in the entity's local XY
    /// plane (see [`panel`](Self::panel)): a gaff sail, or a lug sail with its yard. Put it on
    /// a child of the boat's rigid body and the wind in it drives the boat.
    pub fn sail(corners: [[f32; 2]; 4], material: SoftMaterial) -> Self {
        let [throat, peak, clew, tack] = corners.map(Vec2::from_array);
        let across = 0.5 * (throat.distance(peak) + tack.distance(clew));
        let down = 0.5 * (throat.distance(tack) + peak.distance(clew));
        Self {
            size: [across, down],
            resolution: [
                ((across / 0.11).round() as u32).clamp(8, 48),
                ((down / 0.11).round() as u32).clamp(8, 48),
            ],
            panel: Some(corners),
            camber: 0.1,
            pins: ClothPins::Edges {
                left: true,
                top: true,
                right: false,
                bottom: true,
            },
            mass_per_m2: 0.28,
            shear_compliance: 4e-3,
            bend_compliance: 6e-4,
            tear_ratio: Some(1.8),
            aero_drag: 1.3,
            aero_friction: 0.03,
            aero_lift: 1.1,
            material,
            sim: SimSettings {
                substeps: 12,
                damping: 0.1,
                prewarm_secs: 2.0,
                ..SimSettings::default()
            },
            ..Self::default()
        }
    }
}

/// Tearing, burning and wetness state of a built cloth.
#[derive(Debug, Clone, Default)]
pub struct ClothState {
    pub resolution: [u32; 2],
    /// Every triangle the cloth started with.
    pub triangles: Vec<[u32; 3]>,
    /// Rest-shape inverse edge matrices per triangle (pattern space, meters).
    pub rest_inv: Vec<Mat2>,
    pub alive: Vec<bool>,
    /// Burn progress per triangle: 0 intact, 0..1 burning, 1 burnt away.
    pub burn: Vec<f32>,
    pub tri_edges: Vec<[u32; 3]>,
    pub edge_tris: Vec<[u32; 2]>,
    /// Stretch-constraint rest length per edge.
    pub edge_rest: Vec<f32>,
    /// For bend constraints (tag ≥ edge count): the two edges they span.
    pub bend_support: Vec<[u32; 2]>,
    /// Pattern-space coordinates per particle (meters).
    pub pattern: Vec<Vec2>,
    pub tear_ratio: Option<f32>,
    pub pierceable: bool,
    pub flammable: bool,
    /// 0 dry .. 1 soaked.
    pub wetness: f32,
    /// Per particle: burn progress of the triangles around it, 0 untouched .. 1 burnt away.
    /// The renderer draws a ragged burn front, char and glowing edges from it.
    pub char: Vec<f32>,
    /// Per particle: still burning (the front glows) 0..1.
    pub glow: Vec<f32>,
    /// Bullet holes too small to remove triangles for, drawn by the renderer: surface uv,
    /// radius in meters and a random seed. The oldest go first past [`MAX_HOLES`].
    pub holes: Vec<[f32; 4]>,
    pub burning: u32,
    /// Triangles lost to tearing, piercing and fire.
    pub lost: u32,
    pub cell: f32,
}

/// Most drawn bullet holes one cloth keeps.
pub const MAX_HOLES: usize = 128;

impl ClothState {
    fn edge_alive(&self, e: u32) -> bool {
        self.edge_tris[e as usize]
            .iter()
            .any(|&t| t != u32::MAX && self.alive[t as usize])
    }

    /// Remove triangles, then the constraints and drawn triangles that depended on them.
    /// Returns true if anything changed.
    pub fn kill(
        &mut self,
        tris: impl IntoIterator<Item = usize>,
        sim: &mut Sim,
        surface: &mut Surface,
    ) -> bool {
        let mut changed = false;
        for t in tris {
            if self.alive[t] {
                self.alive[t] = false;
                self.lost += 1;
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        let edges = self.edge_tris.len() as u32;
        let edge_ok: Vec<bool> = (0..edges).map(|e| self.edge_alive(e)).collect();
        sim.distances.retain(|d| {
            if d.tag < edges {
                edge_ok[d.tag as usize]
            } else if let Some(s) = self.bend_support.get((d.tag - edges) as usize) {
                edge_ok[s[0] as usize] && edge_ok[s[1] as usize]
            } else {
                true
            }
        });
        // Tethers assume the cloth is in one piece.
        sim.tethers.clear();
        sim.triangles_changed();
        sim.aero_triangles.clear();
        surface.triangles.clear();
        surface.rest_inv.clear();
        for (t, tri) in self.triangles.iter().enumerate() {
            if self.alive[t] {
                sim.aero_triangles.push(*tri);
                surface.triangles.push(*tri);
                surface.rest_inv.push(self.rest_inv[t]);
            }
        }
        true
    }

    /// Kill triangles stretched past the tear ratio. Returns true if it tore.
    pub fn tear_overstretched(&mut self, sim: &mut Sim, surface: &mut Surface) -> bool {
        let Some(ratio) = self.tear_ratio else {
            return false;
        };
        let edges = self.edge_tris.len() as u32;
        let mut doomed = Vec::new();
        for d in &sim.distances {
            if d.tag >= edges {
                continue;
            }
            let len = (sim.pos[d.a as usize] - sim.pos[d.b as usize]).length();
            if len > d.rest * ratio {
                for t in self.edge_tris[d.tag as usize] {
                    if t != u32::MAX {
                        doomed.push(t as usize);
                    }
                }
            }
        }
        !doomed.is_empty() && self.kill(doomed, sim, surface)
    }

    /// A projectile from `from` to `to` (body-local). Punches a hole where it crosses the
    /// cloth and pushes the fabric around it. Returns the hit points.
    pub fn pierce(
        &mut self,
        sim: &mut Sim,
        surface: &mut Surface,
        from: Vec3,
        to: Vec3,
        radius: f32,
        speed: f32,
    ) -> Vec<Vec3> {
        let mut hits = Vec::new();
        if !self.pierceable {
            return hits;
        }
        let dir = to - from;
        for (t, tri) in self.triangles.iter().enumerate() {
            if !self.alive[t] {
                continue;
            }
            let [a, b, c] = tri.map(|i| sim.pos[i as usize]);
            if let Some(s) = segment_triangle(from, dir, a, b, c) {
                let h = from + dir * s;
                // Through a shared vertex or edge, neighbors report the same crossing.
                if hits.iter().all(|o: &Vec3| o.distance(h) > self.cell * 0.25) {
                    hits.push(h);
                    let [wa, wb, wc] = barycentric(h, a, b, c);
                    let uv = tri.map(|i| surface.uv.get(i as usize).copied().unwrap_or_default());
                    let at = uv[0] * wa + uv[1] * wb + uv[2] * wc;
                    let seed = hash01(self.holes.len() as u64, (h.x * 1e4) as i64 as u64);
                    // Fabric frays a little wider than the bullet.
                    self.holes
                        .push([at.x, at.y, radius.max(0.004) * 1.25, seed]);
                }
            }
        }
        if hits.is_empty() {
            return hits;
        }
        if self.holes.len() > MAX_HOLES {
            let extra = self.holes.len() - MAX_HOLES;
            self.holes.drain(..extra);
        }
        // Only projectiles about as wide as a cell take whole triangles out; smaller holes are
        // drawn (see `holes`) and the cloth stays in one piece.
        if radius >= self.cell * 0.5 {
            let mut doomed = Vec::new();
            for (t, tri) in self.triangles.iter().enumerate() {
                if !self.alive[t] {
                    continue;
                }
                let centroid = tri.iter().map(|&i| sim.pos[i as usize]).sum::<Vec3>() / 3.0;
                if hits.iter().any(|h| h.distance(centroid) < radius) {
                    doomed.push(t);
                }
            }
            self.kill(doomed, sim, surface);
        }
        let hole = radius.max(self.cell * 0.75);
        let push = dir.normalize_or_zero() * (speed * 0.004).min(4.0);
        let reach = hole * 3.0;
        sim.add_velocity(|p| {
            let d = hits
                .iter()
                .map(|h| h.distance(p))
                .fold(f32::INFINITY, f32::min);
            push * (1.0 - d / reach).max(0.0)
        });
        hits
    }

    /// Set triangles within `radius` of `at` (body-local) on fire. Returns how many caught.
    pub fn ignite(&mut self, sim: &Sim, at: Vec3, radius: f32) -> u32 {
        if !self.flammable || self.wetness > 0.5 {
            return 0;
        }
        let mut n = 0;
        for (t, tri) in self.triangles.iter().enumerate() {
            if !self.alive[t] || self.burn[t] > 0.0 {
                continue;
            }
            let centroid = tri.iter().map(|&i| sim.pos[i as usize]).sum::<Vec3>() / 3.0;
            if centroid.distance(at) < radius {
                self.burn[t] = 1e-3;
                n += 1;
            }
        }
        self.burning += n;
        n
    }

    /// Advance fire: burning triangles char, spread to neighbors (faster upward) and fall
    /// away. Returns body-local points where smoke rises this step.
    pub fn update_burn(
        &mut self,
        dt: f32,
        step: u64,
        sim: &mut Sim,
        surface: &mut Surface,
        up: Vec3,
    ) -> Vec<Vec3> {
        let mut smoke = Vec::new();
        if self.burning == 0 {
            return smoke;
        }
        const BURN_RATE: f32 = 0.45; // progress per second
        const SPREAD_RATE: f32 = 2.2; // ignitions per second per edge
        let damp = 1.0 - self.wetness.clamp(0.0, 1.0);
        let mut ignite = Vec::new();
        let mut gone = Vec::new();
        let mut burning = 0;
        for t in 0..self.triangles.len() {
            let b = self.burn[t];
            if !self.alive[t] || b <= 0.0 {
                continue;
            }
            burning += 1;
            self.burn[t] = (b + dt * BURN_RATE * damp).min(1.0);
            if self.burn[t] >= 1.0 {
                gone.push(t);
                continue;
            }
            if b > 0.15 {
                let c0 = self.centroid(sim, t);
                for e in self.tri_edges[t] {
                    for n in self.edge_tris[e as usize] {
                        if n == u32::MAX || n as usize == t {
                            continue;
                        }
                        let n = n as usize;
                        if !self.alive[n] || self.burn[n] > 0.0 {
                            continue;
                        }
                        // Fire climbs: neighbors above catch three times faster.
                        let rise = (self.centroid(sim, n) - c0).normalize_or_zero().dot(up);
                        let chance = dt * SPREAD_RATE * damp * (1.0 + 2.0 * rise.max(0.0));
                        if hash01(step, t as u64 * 3 + n as u64) < chance {
                            ignite.push(n);
                        }
                    }
                }
                if hash01(step ^ 0x5eed, t as u64) < dt * 1.5 {
                    smoke.push(c0);
                }
            }
        }
        for n in ignite {
            self.burn[n] = 1e-3;
        }
        self.burning = burning;
        self.kill(gone, sim, surface);
        // Per-particle char and glow for drawing.
        self.char.iter_mut().for_each(|c| *c = 0.0);
        self.glow.iter_mut().for_each(|g| *g = 0.0);
        for (t, tri) in self.triangles.iter().enumerate() {
            let b = self.burn[t];
            if b <= 0.0 {
                continue;
            }
            let glow = if self.alive[t] {
                smoothstep(0.02, 0.25, b)
            } else {
                0.0
            };
            for &i in tri {
                let i = i as usize;
                self.char[i] = self.char[i].max(b);
                self.glow[i] = self.glow[i].max(glow);
            }
        }
        smoke
    }

    fn centroid(&self, sim: &Sim, t: usize) -> Vec3 {
        self.triangles[t]
            .iter()
            .map(|&i| sim.pos[i as usize])
            .sum::<Vec3>()
            / 3.0
    }

    /// Fraction of the cloth still there.
    pub fn intact_fraction(&self) -> f32 {
        let alive = self.alive.iter().filter(|a| **a).count();
        alive as f32 / self.alive.len().max(1) as f32
    }
}

/// Barycentric weights of `p` (on the triangle's plane) in triangle `a b c`.
fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
    let (d20, d21) = (v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-12 {
        return [1.0, 0.0, 0.0];
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    [1.0 - v - w, v, w]
}

/// Where a segment `from + s·dir` (s in 0..1) crosses triangle `a b c`, if it does.
pub fn segment_triangle(from: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    // Möller–Trumbore.
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = from - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (0.0..=1.0).contains(&t).then_some(t)
}

/// Depth of a sewn-in belly at (u, v), 0..1: deepest two fifths of the way across and halfway
/// down (where a sail carries its draft), 0 along every edge.
fn belly(u: f32, v: f32) -> f32 {
    (PI * u.clamp(0.0, 1.0).powf(0.756)).sin() * (PI * v.clamp(0.0, 1.0)).sin()
}

fn hash01(a: u64, b: u64) -> f32 {
    (hash_u64(a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b) >> 40) as f32 / (1u64 << 24) as f32
}

/// The skeleton a bone-pinned cloth hangs from: the owner entity, its transform and pose.
pub type SkeletonRef<'a> = (Entity, &'a GlobalTransform, &'a SkeletonPose);

/// Stiffness of the springs that hold pinned cloth to a body that moves under physics
/// (m/N): stiff enough to read as pinned, soft enough to pass the cloth's pull on.
const CARRIED_PIN_COMPLIANCE: f32 = 1e-7;

/// Build the simulation for `spec` on entity `entity` placed at `transform`. `carried`: a
/// dynamic rigid body carries the entity, so pins become stiff springs that pull on it.
pub fn build(
    spec: &Cloth,
    entity: Entity,
    transform: &GlobalTransform,
    skeleton: Option<SkeletonRef>,
    carried: bool,
) -> Result<SoftBodySim, String> {
    let nu = spec.resolution[0].max(2) as usize;
    let nv = spec.resolution[1].max(2) as usize;
    if nu * nv > 40_000 {
        return Err(format!(
            "Cloth resolution {nu}×{nv} is too fine; use at most 40 000 particles."
        ));
    }
    let origin = transform.translation();
    let rot = transform.rotation();
    let scale = transform.scale();
    let panel = spec.panel.map(|c| c.map(Vec2::from_array));
    let (w, h, area) = match panel {
        Some([tl, tr, br, bl]) => {
            let area = 0.5 * ((br - tl).perp_dot(bl - tr)).abs();
            if area < 1e-4 {
                return Err("Cloth panel corners enclose no area.".into());
            }
            (
                0.5 * (tl.distance(tr) + bl.distance(br)),
                0.5 * (tl.distance(bl) + tr.distance(br)),
                area,
            )
        }
        None => {
            let [w, h] = [spec.size[0].max(0.01), spec.size[1].max(0.01)];
            (w, h, w * h)
        }
    };
    let mut sim = Sim::new(origin);
    let mass = spec.mass_per_m2.max(1e-4) * area / (nu * nv) as f32;
    let mut pattern = Vec::with_capacity(nu * nv);
    let mut uv = Vec::with_capacity(nu * nv);
    for j in 0..nv {
        for i in 0..nu {
            let u = i as f32 / (nu - 1) as f32;
            let v = j as f32 / (nv - 1) as f32;
            let (flat, width) = match panel {
                Some([tl, tr, br, bl]) => {
                    let (top, bottom) = (tl.lerp(tr, u), bl.lerp(br, u));
                    let width = tl.lerp(bl, v).distance(tr.lerp(br, v));
                    (top.lerp(bottom, v), width)
                }
                None => (
                    Vec2::new((u - spec.anchor[0]) * w, -(v - spec.anchor[1]) * h),
                    w,
                ),
            };
            let local = flat.extend(spec.camber * width * belly(u, v));
            sim.add_particle(rot * (local * scale), mass);
            pattern.push(if panel.is_some() {
                Vec2::new(flat.x, -flat.y)
            } else {
                Vec2::new(u * w, v * h)
            });
            uv.push(Vec2::new(u, v));
        }
    }
    let id = |i: usize, j: usize| (j * nu + i) as u32;

    // Triangles with alternating diagonals (no directional bias), front facing +Z.
    let mut triangles = Vec::with_capacity((nu - 1) * (nv - 1) * 2);
    for j in 0..nv - 1 {
        for i in 0..nu - 1 {
            let (a, b, c, d) = (id(i, j), id(i, j + 1), id(i + 1, j), id(i + 1, j + 1));
            if (i + j) % 2 == 0 {
                triangles.push([a, b, c]);
                triangles.push([c, b, d]);
            } else {
                triangles.push([a, b, d]);
                triangles.push([a, d, c]);
            }
        }
    }
    // Edges, each shared by up to two triangles.
    let mut edge_ids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut edge_tris: Vec<[u32; 2]> = Vec::new();
    let mut tri_edges = Vec::with_capacity(triangles.len());
    for (t, tri) in triangles.iter().enumerate() {
        let mut te = [0u32; 3];
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            let e = *edge_ids.entry(key).or_insert_with(|| {
                edge_tris.push([u32::MAX; 2]);
                (edge_tris.len() - 1) as u32
            });
            let slot = &mut edge_tris[e as usize];
            if slot[0] == u32::MAX {
                slot[0] = t as u32;
            } else {
                slot[1] = t as u32;
            }
            te[k] = e;
        }
        tri_edges.push(te);
    }
    let mut edge_list: Vec<((u32, u32), u32)> = edge_ids.iter().map(|(k, v)| (*k, *v)).collect();
    edge_list.sort_by_key(|(_, e)| *e);
    let mut edge_rest = vec![0.0; edge_tris.len()];
    for ((a, b), e) in &edge_list {
        let diagonal = a % nu as u32 != b % nu as u32 && a / nu as u32 != b / nu as u32;
        let compliance = if diagonal {
            spec.shear_compliance
        } else {
            spec.stretch_compliance
        };
        sim.add_distance(*a, *b, compliance, *e);
        edge_rest[*e as usize] = sim.distances.last().map(|d| d.rest).unwrap_or(0.0);
    }
    // Bending: skip-one springs along rows and columns, each spanning two edges.
    let edges = edge_tris.len() as u32;
    let mut bend_support = Vec::new();
    let edge_of = |a: u32, b: u32| edge_ids.get(&(a.min(b), a.max(b))).copied();
    for j in 0..nv {
        for i in 0..nu {
            for (di, dj) in [(2usize, 0usize), (0, 2)] {
                if i + di >= nu || j + dj >= nv {
                    continue;
                }
                let (a, m, b) = (id(i, j), id(i + di / 2, j + dj / 2), id(i + di, j + dj));
                if let (Some(e1), Some(e2)) = (edge_of(a, m), edge_of(m, b)) {
                    sim.add_distance(
                        a,
                        b,
                        spec.bend_compliance,
                        edges + bend_support.len() as u32,
                    );
                    bend_support.push([e1, e2]);
                }
            }
        }
    }

    // Gathered cloth starts in folds: squeeze it along u and let it bow out alternately in
    // front of and behind the pins, so the arc length between pins stays the same.
    let gather = spec.gather.clamp(0.2, 1.0);
    if gather < 0.999 && panel.is_none() {
        let pin_spacing = match spec.pins {
            ClothPins::TopEdge { every } => every.max(1) as f32 * w / (nu - 1) as f32,
            _ => 0.3,
        };
        let squeezed = pin_spacing * gather;
        let amplitude = 2.0 * squeezed / PI * (1.0 / gather - 1.0).sqrt();
        for j in 0..nv {
            for i in 0..nu {
                let u = i as f32 / (nu - 1) as f32;
                let v = j as f32 / (nv - 1) as f32;
                let x = u * w * gather;
                let local = Vec3::new(
                    x - spec.anchor[0] * w * gather,
                    -(v - spec.anchor[1]) * h,
                    amplitude * (PI * x / squeezed).sin(),
                );
                sim.place(id(i, j), rot * (local * scale));
            }
        }
    }

    // Pins.
    let mut frames = Vec::new();
    let self_frame = sim.add_frame(Vec3::ZERO, rot);
    frames.push(FrameSource::Entity(entity));
    let mut follow = None;
    let mut time_owner = None;
    let mut pin_points: Vec<(usize, usize)> = Vec::new();
    match &spec.pins {
        ClothPins::None => {}
        ClothPins::TopEdge { every } => {
            let every = (*every).max(1) as usize;
            for i in (0..nu).step_by(every) {
                pin_points.push((i, 0));
            }
            if !(nu - 1).is_multiple_of(every) {
                pin_points.push((nu - 1, 0));
            }
        }
        ClothPins::LeftEdge => pin_points.extend((0..nv).map(|j| (0, j))),
        ClothPins::TopCorners => pin_points.extend([(0, 0), (nu - 1, 0)]),
        ClothPins::Points(points) => {
            for p in points {
                if (p[0] as usize) < nu && (p[1] as usize) < nv {
                    pin_points.push((p[0] as usize, p[1] as usize));
                }
            }
        }
        ClothPins::Edges {
            left,
            top,
            right,
            bottom,
        } => {
            for j in 0..nv {
                for i in 0..nu {
                    if (*left && i == 0)
                        || (*right && i == nu - 1)
                        || (*top && j == 0)
                        || (*bottom && j == nv - 1)
                    {
                        pin_points.push((i, j));
                    }
                }
            }
        }
        ClothPins::Bones { rows, bones } => {
            let Some((owner, owner_t, pose)) = skeleton else {
                return Err(
                    "Cloth pinned to bones needs a parent (or ancestor) with a SkeletonPose."
                        .into(),
                );
            };
            let mut bone_frames: Vec<(usize, u16, DVec3)> = Vec::new();
            for name in bones {
                let Some(b) = pose.index_of(name) else {
                    return Err(format!(
                        "Bone \"{name}\" is not in the skeleton. Bones: {}.",
                        pose.names.join(", ")
                    ));
                };
                let Some((p, r)) = pose.bone_world(owner_t, b) else {
                    continue;
                };
                frames.push(FrameSource::Bone { owner, bone: b });
                let f = sim.add_frame((p - origin).as_vec3(), r);
                bone_frames.push((b, f, p));
            }
            if bone_frames.is_empty() {
                return Err("Cloth pinned to bones lists no bones.".into());
            }
            for j in 0..(*rows as usize).clamp(1, nv) {
                for i in 0..nu {
                    let p = origin + sim.pos[id(i, j) as usize].as_dvec3();
                    let nearest = bone_frames
                        .iter()
                        .min_by(|a, b| a.2.distance(p).total_cmp(&b.2.distance(p)))
                        .map(|b| b.1)
                        .unwrap_or(self_frame);
                    sim.pin_here(id(i, j), nearest);
                }
            }
            // Follow the middle listed bone (usually the chest).
            follow = Some(bone_frames[bone_frames.len() / 2].1);
            time_owner = Some(owner);
        }
    }
    let mut held = vec![false; nu * nv];
    for (i, j) in &pin_points {
        let p = id(*i, *j);
        held[p as usize] = true;
        if carried {
            let local = rot.conjugate() * sim.pos[p as usize];
            sim.goals.push(Goal {
                p,
                frame: self_frame,
                local,
                compliance: CARRIED_PIN_COMPLIANCE,
            });
        } else {
            sim.pin_here(p, self_frame);
        }
    }

    // Long-range tethers to the nearest pin, in pattern space.
    let pinned: Vec<usize> = (0..nu * nv)
        .filter(|&p| held[p] || sim.is_pinned(p as u32))
        .collect();
    if !pinned.is_empty() {
        for p in 0..nu * nv {
            if held[p] || sim.is_pinned(p as u32) {
                continue;
            }
            if let Some(&a) = pinned.iter().min_by(|&&a, &&b| {
                pattern[a]
                    .distance(pattern[p])
                    .total_cmp(&pattern[b].distance(pattern[p]))
            }) {
                sim.tethers.push(Tether {
                    p: p as u32,
                    anchor: a as u32,
                    max: pattern[a].distance(pattern[p]) * 1.05 * scale.max_element(),
                });
            }
        }
    }

    // Rest-shape matrices for tension (pattern space).
    let rest_inv: Vec<Mat2> = triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| pattern[i as usize]);
            let m = Mat2::from_cols(b - a, c - a);
            if m.determinant().abs() > 1e-12 {
                m.inverse()
            } else {
                Mat2::IDENTITY
            }
        })
        .collect();

    sim.aero_triangles = triangles.clone();
    sim.params.radius = spec.sim.thickness;
    if spec.sim.self_collide {
        sim.params.self_collide = (spec.sim.thickness * 1.5).max(0.006);
    }
    sim.params.aero = (spec.aero_drag > 0.0).then_some(Aero {
        drag: spec.aero_drag,
        friction: spec.aero_friction,
        lift: spec.aero_lift,
    });
    if let Some(f) = follow
        && spec.sim.follow > 0.0
    {
        sim.params.follow = Some((f, spec.sim.follow));
    }
    let surface = Surface {
        triangles: triangles.clone(),
        uv,
        embed: None,
        rest_inv: rest_inv.clone(),
        uv_scale: Vec2::new(w, h),
    };
    let cell = (w / (nu - 1) as f32).max(h / (nv - 1) as f32);
    let n_particles = nu * nv;
    let state = ClothState {
        resolution: [nu as u32, nv as u32],
        alive: vec![true; triangles.len()],
        burn: vec![0.0; triangles.len()],
        triangles,
        rest_inv,
        tri_edges,
        edge_tris,
        edge_rest,
        bend_support,
        pattern,
        tear_ratio: spec.tear_ratio,
        pierceable: spec.pierceable,
        flammable: spec.flammable,
        wetness: 0.0,
        char: vec![0.0; n_particles],
        glow: vec![0.0; n_particles],
        holes: Vec::new(),
        burning: 0,
        lost: 0,
        cell,
    };
    let mut body = SoftBodySim::new(SoftKind::Cloth, sim, spec.sim, spec.material);
    body.frames = frames;
    body.surface = Some(surface);
    body.cloth = Some(state);
    body.time_owner = time_owner;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag() -> SoftBodySim {
        let spec = Cloth {
            size: [1.2, 0.8],
            resolution: [13, 9],
            ..Cloth::default()
        };
        build(
            &spec,
            Entity::PLACEHOLDER,
            &GlobalTransform::from(Transform::from_xyz(5.0, 3.0, 0.0)),
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn builds_a_grid_with_pins_and_tethers() {
        let body = flag();
        assert_eq!(body.sim.len(), 13 * 9);
        assert_eq!(body.sim.pins.len(), 9, "left edge pinned");
        assert_eq!(body.sim.tethers.len(), 13 * 9 - 9);
        let surface = body.surface.as_ref().unwrap();
        assert_eq!(surface.triangles.len(), 12 * 8 * 2);
        // Front faces +Z.
        let [a, b, c] = surface.triangles[0].map(|i| body.sim.pos[i as usize]);
        assert!((b - a).cross(c - a).z > 0.0);
    }

    /// A gaff sail's throat, peak, clew and tack.
    const GAFF: [[f32; 2]; 4] = [[0.1, 3.4], [1.9, 4.7], [2.85, 1.13], [0.1, 1.12]];

    fn sail(carried: bool) -> SoftBodySim {
        let spec = Cloth::sail(GAFF, SoftMaterial::hex(0xf2ead8));
        let t = GlobalTransform::default();
        build(&spec, Entity::PLACEHOLDER, &t, None, carried).unwrap()
    }

    #[test]
    fn a_sail_panel_takes_its_shape_and_holds_its_belly() {
        let body = sail(false);
        let spec = Cloth::sail(GAFF, SoftMaterial::default());
        let [nu, nv] = body.cloth.as_ref().unwrap().resolution.map(|n| n as usize);
        let at = |i: usize, j: usize| body.sim.pos[j * nu + i];
        let corners = [(0, 0), (nu - 1, 0), (nu - 1, nv - 1), (0, nv - 1)];
        for ((i, j), c) in corners.into_iter().zip(GAFF) {
            let want = Vec2::from_array(c).extend(0.0);
            assert!(at(i, j).distance(want) < 1e-4, "{} vs {c:?}", at(i, j));
        }
        // About a tenth of the chord deep, two fifths of the way back, toward +Z.
        let mid = at(nu * 2 / 5, nv / 2);
        let chord = at(0, nv / 2).distance(at(nu - 1, nv / 2));
        assert!(
            (mid.z / chord - 0.1).abs() < 0.02,
            "belly {} in {chord}",
            mid.z
        );
        // Luff, head and foot are pinned; the leech is free.
        assert_eq!(body.sim.pins.len(), 2 * nu + nv - 2);
        assert!(!body.sim.is_pinned((nv / 2 * nu + nu - 1) as u32));
        // Rows and columns are stretch edges even though the panel isn't a rectangle.
        let stretch = body
            .sim
            .distances
            .iter()
            .filter(|d| d.compliance == spec.stretch_compliance)
            .count();
        assert_eq!(stretch, (nu - 1) * nv + nu * (nv - 1));
    }

    #[test]
    fn a_carried_sail_hangs_on_stiff_goals_instead_of_pins() {
        let body = sail(true);
        let [nu, nv] = body.cloth.as_ref().unwrap().resolution.map(|n| n as usize);
        assert!(body.sim.pins.is_empty());
        assert_eq!(body.sim.goals.len(), 2 * nu + nv - 2);
        assert_eq!(body.sim.tethers.len(), nu * nv - (2 * nu + nv - 2));
    }

    /// A limp sheet folded in half and dropped onto the floor: how far each folded-over particle
    /// ends up above the one it was folded onto.
    /// A towel folded in half and settled on the floor.
    fn settled_towel(self_collide: bool) -> SoftBodySim {
        let spec = Cloth {
            size: [1.0, 0.6],
            resolution: [21, 13],
            pins: ClothPins::None,
            // Limp, so the crease holds and only self-collision keeps the layers apart.
            bend_compliance: 10.0,
            sim: SimSettings {
                self_collide,
                thickness: 0.01,
                ..SimSettings::default()
            },
            ..Cloth::default()
        };
        // Lying flat, 0.3 m above the floor; the fold runs along x = 0.5.
        let t = GlobalTransform::from(
            Transform::from_xyz(0.0, 0.3, 0.0).with_rotation(Quat::from_rotation_x(-PI / 2.0)),
        );
        let mut body = build(&spec, Entity::PLACEHOLDER, &t, None, false).unwrap();
        let sim = &mut body.sim;
        sim.params.aero = None;
        sim.ground = vec![[0.0, 1.0, 0.0, 0.3]];
        for j in 0..13 {
            let crease = sim.pos[j * 21 + 10].x;
            for i in 11..21 {
                let k = j * 21 + i;
                let p = sim.pos[k];
                sim.place(k as u32, Vec3::new(2.0 * crease - p.x, p.y + 0.03, p.z));
            }
        }
        for _ in 0..120 {
            sim.step(1.0 / 60.0, &[]);
        }
        body
    }

    fn folded_towel(self_collide: bool) -> Vec<f32> {
        let body = settled_towel(self_collide);
        let sim = &body.sim;
        let mut gaps = Vec::new();
        for j in 1..12 {
            for i in 12..20 {
                let (top, under) = (j * 21 + i, j * 21 + 20 - i);
                gaps.push(sim.pos[top].y - sim.pos[under].y);
            }
        }
        gaps
    }

    #[test]
    fn a_folded_towel_stays_two_layers() {
        let apart =
            |gaps: &[f32]| gaps.iter().filter(|g| **g > 0.008).count() as f32 / gaps.len() as f32;
        let stacked = apart(&folded_towel(true));
        let merged = apart(&folded_towel(false));
        assert!(stacked > 0.95, "only {stacked} of the fold stays on top");
        assert!(
            merged < 0.5,
            "without self-collision {merged} stayed on top anyway"
        );
    }

    #[test]
    fn a_resting_towel_torn_open_keeps_colliding_with_itself() {
        let mut body = settled_towel(true);
        let SoftBodySim {
            sim,
            surface,
            cloth,
            ..
        } = &mut body;
        let (surface, cloth) = (surface.as_mut().unwrap(), cloth.as_mut().unwrap());
        let before = surface.triangles.len();
        // Through both layers near the towel's last triangles: the pairs kept from the
        // resting towel name triangles that are gone or renumbered.
        cloth.pierce(
            sim,
            surface,
            Vec3::new(0.45, 1.0, 0.25),
            Vec3::new(0.45, -1.0, 0.25),
            0.15,
            400.0,
        );
        assert!(surface.triangles.len() < before);
        for _ in 0..60 {
            sim.step(1.0 / 60.0, &[]);
        }
        assert!(sim.pos.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn a_towel_balled_up_lets_go_of_itself() {
        let mut body = settled_towel(true);
        let sim = &mut body.sim;
        let c = sim.pos[0];
        for i in 0..sim.pos.len() {
            let h = (i as u32).wrapping_mul(2_654_435_761);
            let d = Vec3::new(
                (h % 97) as f32 / 97.0,
                (h / 97 % 89) as f32 / 89.0,
                (h / 8633 % 83) as f32 / 83.0,
            );
            sim.place(i as u32, c + d * 0.06);
        }
        sim.step(1.0 / 60.0, &[]);
        // Hundreds of pairs a particle: it stops colliding with itself instead of paying for
        // them, and falls open.
        assert_eq!(sim.self_pairs(), 0);
        for _ in 0..60 {
            sim.step(1.0 / 60.0, &[]);
        }
        assert!(sim.pos.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn hangs_without_stretching() {
        let mut body = flag();
        body.sim.params.aero = None;
        for _ in 0..240 {
            body.sim.step(1.0 / 60.0, &[]);
        }
        let worst = body
            .sim
            .distances
            .iter()
            .take(100)
            .map(|d| (body.sim.pos[d.a as usize] - body.sim.pos[d.b as usize]).length() / d.rest)
            .fold(0.0f32, f32::max);
        assert!(worst < 1.08, "stretched {worst}");
        // The free corner droops below the top of the pole.
        assert!(body.sim.pos[12].y < -0.02, "{}", body.sim.pos[12]);
    }

    #[test]
    fn a_bullet_leaves_a_hole() {
        let mut body = flag();
        let SoftBodySim {
            sim,
            surface,
            cloth,
            ..
        } = &mut body;
        let (surface, cloth) = (surface.as_mut().unwrap(), cloth.as_mut().unwrap());
        let before = surface.triangles.len();
        let hits = cloth.pierce(
            sim,
            surface,
            Vec3::new(0.6, -0.4, 1.0),
            Vec3::new(0.6, -0.4, -1.0),
            0.02,
            400.0,
        );
        assert_eq!(hits.len(), 1);
        // A bullet leaves a drawn hole at the hit's uv; the cloth stays in one piece.
        assert_eq!(surface.triangles.len(), before);
        assert_eq!(cloth.holes.len(), 1);
        let [u, v, r, _] = cloth.holes[0];
        assert!((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) && r > 0.02);
        // A cannonball takes the triangles with it.
        cloth.pierce(
            sim,
            surface,
            Vec3::new(0.3, -0.3, 1.0),
            Vec3::new(0.3, -0.3, -1.0),
            0.3,
            400.0,
        );
        assert!(surface.triangles.len() < before);
        let missed = cloth.pierce(
            sim,
            surface,
            Vec3::new(5.0, 0.0, 1.0),
            Vec3::new(5.0, 0.0, -1.0),
            0.02,
            400.0,
        );
        assert!(missed.is_empty());
    }

    #[test]
    fn fire_spreads_and_burns_the_cloth_away() {
        let mut body = flag();
        let SoftBodySim {
            sim,
            surface,
            cloth,
            ..
        } = &mut body;
        let (surface, cloth) = (surface.as_mut().unwrap(), cloth.as_mut().unwrap());
        // Light the bottom middle.
        assert!(cloth.ignite(sim, Vec3::new(0.6, -0.8, 0.0), 0.15) > 0);
        let mut smoke = 0;
        for step in 0..60 * 12 {
            smoke += cloth
                .update_burn(1.0 / 60.0, step, sim, surface, Vec3::Y)
                .len();
        }
        assert!(cloth.intact_fraction() < 0.3, "{}", cloth.intact_fraction());
        assert!(smoke > 0);
        // Wet cloth doesn't catch.
        let mut wet = flag();
        let c = wet.cloth.as_mut().unwrap();
        c.wetness = 0.9;
        assert_eq!(c.ignite(&wet.sim, Vec3::new(0.6, -0.8, 0.0), 0.15), 0);
    }

    #[test]
    fn overstretched_cloth_tears() {
        let mut body = flag();
        body.cloth.as_mut().unwrap().tear_ratio = Some(1.3);
        // Yank the far edge away.
        for j in 0..9 {
            let i = j * 13 + 12;
            body.sim.pos[i].x += 2.0;
        }
        let SoftBodySim {
            sim,
            surface,
            cloth,
            ..
        } = &mut body;
        assert!(
            cloth
                .as_mut()
                .unwrap()
                .tear_overstretched(sim, surface.as_mut().unwrap())
        );
    }
}

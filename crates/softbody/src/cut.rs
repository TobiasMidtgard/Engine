//! Hair cut by blades.
//!
//! Every [`CuttingEdge`] in the world is followed from fixed step to fixed step. Where the
//! quad its edge swept in a step crosses a lock of simulated hair ([`crate::guides`]), the lock
//! is cut there or pushed aside, by whether the force on a fibre reaches what the edge needs
//! to shear it:
//!
//! - **What the edge needs.** A fibre shears when the pressure under the edge passes keratin's
//!   strength, about 150 MPa, over the strip the edge bears on: its width, twice the edge's
//!   radius, across the fibre's 70 micrometers. A razor's edge is a tenth of a micrometer
//!   round and needs 2 mN; a sword kept for use, a few micrometers and a twentieth of a
//!   newton; a blunt training blade, a tenth of a millimeter and 2 N. `sharpness` runs through
//!   those three decades. Drawing the edge along itself as it presses (a slicing cut) lowers the
//!   force by the root of one plus the square of the slide over the push (Atkins' slice-push
//!   ratio), and wet keratin is weaker by a third.
//! - **What the fibre gives.** A hanging hair gives way: nothing holds it but its own inertia
//!   and the stretch the blow puts in it. A fibre struck across at speed `v` stretches by
//!   `(v / (c √2))^(4/3)` (Smith's theory of the transverse impact on a yarn; `c` the speed of
//!   sound along the fibre, 1750 m/s for hair's 4 GPa), and the kink that runs out from the edge
//!   turns that tension across it: a force that grows as `v^(5/3)`, 4 mN at 10 m/s. In a lock
//!   the fibres behind back the ones at the edge, about fifteen deep; each lock a little more
//!   or less, so at the limit a blade takes some locks and leaves others. Hair that lies on
//!   the body can't give way at all: there the hand's press is on it.
//!
//! So a razor cuts in a slow draw (from 1.3 m/s), a honed sword in any swing (3 m/s), a sword
//! kept for use in a committed one (10 m/s) or against the body, a worn edge only at the full
//! speed of its point, and a blunt blade never: it sweeps the hair along until the hair slips
//! off it. The piece that comes off falls
//! ([`crate::tress::TressCut`]).

use engine_core::glam::DVec3;
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_world::CuttingEdge;

use crate::SoftBodySim;
use crate::guides::GuideHair;

/// Keratin's strength in shear, Pa, and a hair's thickness, m.
const STRENGTH: f32 = 1.5e8;
const FIBRE: f32 = 7.0e-5;
/// The radius of a blunt edge (`sharpness` 0), m, and the decades down to a razor's (1).
const BLUNT: f32 = 1.0e-4;
const DECADES: f32 = 3.0;
/// Wet hair shears at this much less.
const WET_WEAKER: f32 = 0.3;
/// The force a blow across a free fibre puts on the edge, N per (m/s)^(5/3).
const BLOW: f32 = 9.6e-5;
/// Fibres of a lock behind the one at the edge, backing it.
pub const BACKED: f32 = 15.0;
/// The force on a fibre the blade presses on the body, N, and how near the body that is, m.
const PRESSED: f32 = 0.2;
const ON_BODY: f32 = 0.006;
/// How squarely the edge must lead the blade's motion to cut (the cosine): slower than that
/// across it, the flat or the back meets the hair.
const LEADS: f32 = 0.3;
/// A blade that doesn't cut keeps the hair this far ahead of it, m.
const AHEAD: f32 = 0.003;
/// The share of the blade's speed a piece takes with it as it comes off.
const FLICKED: f32 = 0.12;

/// The force (N) an edge of `sharpness` (0 blunt .. 1 a razor) needs on one fibre to shear it,
/// sliding along itself `slice` times as fast as it pushes, the hair `wet` (0..1).
pub fn force_needed(sharpness: f32, slice: f32, wet: f32) -> f32 {
    let radius = BLUNT * 10f32.powf(-DECADES * sharpness.clamp(0.0, 1.0));
    STRENGTH * 2.0 * radius * FIBRE * (1.0 - WET_WEAKER * wet.clamp(0.0, 1.0))
        / (1.0 + slice * slice).sqrt()
}

/// The force (N) on the fibre at the edge when a blade crosses a lock at `speed` (m/s across
/// the edge), `backing` fibres deep behind it; `pressed`: the hair lies on the body and the
/// blade comes down on it.
pub fn force_given(speed: f32, backing: f32, pressed: bool) -> f32 {
    BLOW * speed.max(0.0).powf(5.0 / 3.0) * backing.max(1.0) + if pressed { PRESSED } else { 0.0 }
}

/// Where a [`CuttingEdge`] was at the last fixed step (world).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct EdgeSwept {
    pub heel: DVec3,
    pub tip: DVec3,
}

/// Where the segment `a`..`b` crosses the triangle `p0, p1, p2`: how far along the segment,
/// and the triangle's two weights (of `p1` and `p2`).
fn crosses(a: Vec3, b: Vec3, p0: Vec3, p1: Vec3, p2: Vec3) -> Option<(f32, f32, f32)> {
    let (e1, e2, d) = (p1 - p0, p2 - p0, b - a);
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = a - p0;
    let u = s.dot(h) * inv;
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    let t = e2.dot(q) * inv;
    ((0.0..=1.0).contains(&t) && u >= 0.0 && v >= 0.0 && u + v <= 1.0).then_some((t, u, v))
}

/// What a blade did to a lock in a step.
struct Met {
    strand: usize,
    /// The particle the crossed segment runs into (strand-relative), and how far along it.
    at: usize,
    part: f32,
    /// The blade there: a point of its edge now, and its velocity (body space, m/s).
    edge: Vec3,
    velocity: Vec3,
    /// The way it moves across its edge.
    across: Vec3,
    cut: bool,
}

/// Follow every [`CuttingEdge`] and cut or push the simulated hair its edge swept through in
/// the last step (after the hair's own step is in, before the next).
pub fn cut_hair(
    mut commands: Commands,
    fixed: Res<FixedTime>,
    edges: Query<(Entity, &CuttingEdge, &GlobalTransform, Option<&EdgeSwept>)>,
    mut hair: Query<&mut SoftBodySim, With<GuideHair>>,
) {
    let dt = fixed.world_step_secs() as f32;
    for (entity, edge, t, was) in &edges {
        let world = |p: Vec3| t.translation() + (t.rotation() * (p * t.scale())).as_dvec3();
        let now = EdgeSwept {
            heel: world(edge.heel),
            tip: world(edge.tip),
        };
        if was != Some(&now) {
            commands.entity(entity).insert(now);
        }
        let Some(was) = was else {
            continue;
        };
        let travel = was.heel.distance(now.heel).max(was.tip.distance(now.tip)) as f32;
        // (Still, or put somewhere else: nothing swept.)
        if dt <= 0.0 || !(1e-5..=3.0).contains(&travel) {
            continue;
        }
        let facing = (t.rotation() * edge.facing).normalize_or_zero();
        let mid = (was.heel + was.tip + now.heel + now.tip) * 0.25;
        let reach = [was.heel, was.tip, now.heel, now.tip]
            .iter()
            .map(|c| c.distance(mid))
            .fold(0.0, f64::max);
        for mut body in &mut hair {
            if body.center.distance(mid) > f64::from(body.radius) + reach + 0.05 {
                continue;
            }
            let met = sweep(&body, edge, facing, (was, &now), dt);
            if met.is_empty() {
                continue;
            }
            let body = &mut *body;
            let Some(tress) = body.sim.tress.as_mut() else {
                continue;
            };
            for m in met {
                let s = tress.strands[m.strand];
                let first = s.first as usize;
                if m.cut {
                    let whole = tress
                        .cuts
                        .get(m.strand)
                        .filter(|c| c.at != 0)
                        .map_or(s.count as usize, |c| c.at as usize);
                    if tress.cut(m.strand, m.at, m.part) {
                        for i in first + m.at..first + whole {
                            body.sim.vel[i] += m.velocity * FLICKED;
                        }
                    }
                    continue;
                }
                // Swept along: the two points at the blade stay ahead of it and move with it.
                tress.wake(m.strand);
                for i in [m.at - 1, m.at] {
                    if i == 0 {
                        continue;
                    }
                    let (p, v) = (&mut body.sim.pos[first + i], &mut body.sim.vel[first + i]);
                    let ahead = (*p - m.edge).dot(m.across);
                    if ahead < AHEAD {
                        *p += m.across * (AHEAD - ahead);
                        *v += m.across * (m.velocity - *v).dot(m.across).max(0.0);
                    }
                }
            }
        }
    }
}

/// The locks of `body` the edge crossed going from `was` to `now` in `dt`, each where it was
/// crossed nearest its root.
fn sweep(
    body: &SoftBodySim,
    edge: &CuttingEdge,
    facing: Vec3,
    (was, now): (&EdgeSwept, &EdgeSwept),
    dt: f32,
) -> Vec<Met> {
    let Some(tress) = body.sim.tress.as_ref() else {
        return Vec::new();
    };
    let (h0, t0, h1, t1) = (
        body.local(was.heel),
        body.local(was.tip),
        body.local(now.heel),
        body.local(now.tip),
    );
    let along = (t1 - h1).normalize_or_zero();
    let (pos, vel) = (&body.sim.pos, &body.sim.vel);
    let mut met = Vec::new();
    for (k, s) in tress.strands.iter().enumerate() {
        let first = s.first as usize;
        let whole = tress
            .cuts
            .get(k)
            .filter(|c| c.at != 0)
            .map_or(s.count as usize, |c| c.at as usize);
        if first + whole > pos.len() {
            continue;
        }
        for i in 1..whole {
            let (a, b) = (pos[first + i - 1], pos[first + i]);
            // How far along the lock's segment, and along the edge from its heel.
            let Some((part, u)) = crosses(a, b, h0, t0, t1)
                .map(|(t, u, v)| (t, u + v))
                .or_else(|| crosses(a, b, h0, t1, h1).map(|(t, u, _)| (t, u)))
            else {
                continue;
            };
            let velocity = ((h1 + (t1 - h1) * u) - (h0 + (t0 - h0) * u)) / dt;
            let rel = velocity - vel[first + i - 1].lerp(vel[first + i], part);
            let slide = rel.dot(along);
            let across = rel - along * slide;
            let speed = across.length();
            if speed < 1e-4 {
                break;
            }
            let across = across / speed;
            let leads = across.dot(facing);
            let edge_first = leads > LEADS || (edge.double && leads < -LEADS);
            let wet = body
                .wet
                .get(first..first + whole)
                .map_or(0.0, |w| w.iter().sum::<f32>() / whole as f32);
            let at = a.lerp(b, part);
            let pressed = tress
                .surface_at(&body.sim.frames, at)
                .is_some_and(|(out, d, _)| d < ON_BODY && rel.dot(out) < -0.05);
            let backing = BACKED * (0.6 + 0.8 * s.seed.clamp(0.0, 1.0));
            let cut = edge_first
                && force_given(speed, backing, pressed)
                    >= force_needed(edge.sharpness, slide.abs() / speed, wet);
            met.push(Met {
                strand: k,
                at: i,
                part,
                edge: h1 + (t1 - h1) * u,
                velocity,
                across,
                cut,
            });
            break;
        }
    }
    met
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slowest blow across a free lock (average backing) that cuts it, m/s.
    fn slowest(sharpness: f32) -> f32 {
        (1..2000)
            .map(|v| v as f32 * 0.1)
            .find(|v| force_given(*v, BACKED, false) >= force_needed(sharpness, 0.0, 0.0))
            .unwrap_or(f32::INFINITY)
    }

    #[test]
    fn a_razor_cuts_in_a_slow_draw_and_a_blunt_blade_never() {
        // A razor: a hand's slow draw. A honed sword: any swing. A sword kept for use: only
        // near the full speed of its point. A training blade: at no speed a blade moves at.
        assert!(slowest(1.0) < 2.0, "{}", slowest(1.0));
        assert!((2.0..8.0).contains(&slowest(0.8)), "{}", slowest(0.8));
        assert!((8.0..30.0).contains(&slowest(0.5)), "{}", slowest(0.5));
        assert!(slowest(0.0) > 60.0, "{}", slowest(0.0));
        // Sharper never needs more.
        let mut last = f32::INFINITY;
        for s in 0..=10 {
            let needed = force_needed(s as f32 / 10.0, 0.0, 0.0);
            assert!(needed < last);
            last = needed;
        }
    }

    #[test]
    fn slicing_wet_hair_and_hair_on_the_body_cut_more_easily() {
        let push = force_needed(0.5, 0.0, 0.0);
        assert!(force_needed(0.5, 2.0, 0.0) < push * 0.5);
        assert!(force_needed(0.5, 0.0, 1.0) < push * 0.8);
        // Pressed on the body, a sword kept for use cuts at a walking pace; a blunt blade
        // still doesn't.
        assert!(force_given(1.0, BACKED, true) >= force_needed(0.5, 0.0, 0.0));
        assert!(force_given(1.0, BACKED, true) < force_needed(0.0, 0.0, 0.0));
    }

    #[test]
    fn a_segment_crosses_a_triangle_where_it_does() {
        let (p0, p1, p2) = (Vec3::ZERO, Vec3::X, Vec3::Z);
        let hit = crosses(
            Vec3::new(0.25, 1.0, 0.25),
            Vec3::new(0.25, -1.0, 0.25),
            p0,
            p1,
            p2,
        );
        let (t, u, v) = hit.unwrap();
        assert!((t - 0.5).abs() < 1e-5 && (u - 0.25).abs() < 1e-5 && (v - 0.25).abs() < 1e-5);
        // Beside it, short of it, and along it: no.
        assert!(
            crosses(
                Vec3::new(0.8, 1.0, 0.8),
                Vec3::new(0.8, -1.0, 0.8),
                p0,
                p1,
                p2
            )
            .is_none()
        );
        assert!(
            crosses(
                Vec3::new(0.2, 1.0, 0.2),
                Vec3::new(0.2, 0.5, 0.2),
                p0,
                p1,
                p2
            )
            .is_none()
        );
        assert!(
            crosses(
                Vec3::new(0.1, 0.0, 0.1),
                Vec3::new(0.3, 0.0, 0.2),
                p0,
                p1,
                p2
            )
            .is_none()
        );
    }
}

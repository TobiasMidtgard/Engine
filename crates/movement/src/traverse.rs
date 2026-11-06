//! Traversal markup: what a probe can't tell from a box (Dying Light 2's tagged parkour
//! helpers, Assassin's Creed 3's authored tree types, Spider-Man's special-cased awnings).
//!
//! Ledges, walls and vault tops are found by probing the colliders, as before. Thin or
//! meaningful pieces carry a [`TraversalElement`]: the top of a lamp post is a perch, a cable
//! between two roofs can be walked, a crag's face has holds, an ice fall takes picks anywhere,
//! an awning bounces. The movement step reads them through
//! [`Probe::elements`](crate::locomotion::Probe::elements) for its targeted jumps
//! ([`crate::target`]) and the states that move on them ([`crate::perch`],
//! [`crate::balance`], [`crate::bar`], [`crate::wall`]).
//!
//! An element's points are in its entity's frame (its `Transform`), so markup moves with
//! the piece it marks.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

/// What an element is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ElementKind {
    /// A small top to land and crouch on: lamp-post heads, bollards, chimney caps, branch
    /// forks. `a` is the top's center.
    #[default]
    Perch,
    /// An upright pole to grab, climb and swing round: lamp-post shafts, drainpipes, trunks.
    /// `a` bottom, `b` top; `width` its diameter.
    PoleV,
    /// A bar to hang and swing from: scaffold pipes, sign brackets, low branches. `a` to `b`.
    PoleH,
    /// A wide beam to run along (≥ 0.25 m): planks, roof ridges, timber beams.
    Beam,
    /// A railing or a narrow beam to balance along (0.05–0.25 m).
    Rail,
    /// A cable or rope to walk (Tomb Raider's tightropes): it sags under the walker.
    Tightrope,
    /// A washing line: hang from it and go hand over hand (it sags).
    Clothesline,
    /// A tree branch: walked like a beam, bending under the walker; its tip end is `b`.
    Branch,
    /// A rock face with holds (cracks, crimps, jugs): `a` and `b` are opposite corners of the
    /// patch on the face, `normal` points out of it; holds are generated from `seed`.
    Rock,
    /// An ice face: picks bite anywhere on the patch (`a`, `b`, `normal` as for rock).
    Ice,
    /// Fabric over a shop front: lands bounce off it (`a`, `b` opposite corners, `normal` up
    /// out of the cloth).
    Awning,
    /// Somewhere soft to land from any height (a hay cart): `a` its top's center, `width` its
    /// size.
    Haystack,
}

impl ElementKind {
    /// Lines walked along with balance.
    pub fn is_walkable_line(self) -> bool {
        matches!(
            self,
            ElementKind::Beam | ElementKind::Rail | ElementKind::Tightrope | ElementKind::Branch
        )
    }

    /// Faces climbed hold by hold.
    pub fn is_face(self) -> bool {
        matches!(self, ElementKind::Rock | ElementKind::Ice)
    }
}

/// Traversal markup on a level piece. See the module docs.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct TraversalElement {
    pub kind: ElementKind,
    /// First point (the entity's frame), m.
    pub a: Vec3,
    /// Second point (same as `a` for a point element), m.
    pub b: Vec3,
    /// Width of a line or a perch's top, a pole's diameter, a haystack's size, m.
    pub width: f32,
    /// Out of a face or an awning, up out of a perch (the entity's frame).
    pub normal: Vec3,
    /// How far a cable sags at mid-span with nobody on it, m (and how much more a walker
    /// presses it down: see [`crate::balance`]).
    pub sag: f32,
    /// Seeds generated holds (rock).
    pub seed: u32,
}

impl Default for TraversalElement {
    fn default() -> Self {
        Self {
            kind: ElementKind::Perch,
            a: Vec3::ZERO,
            b: Vec3::ZERO,
            width: 0.3,
            normal: Vec3::Y,
            sag: 0.0,
            seed: 0,
        }
    }
}

impl TraversalElement {
    pub fn perch(top: Vec3, width: f32) -> Self {
        Self {
            kind: ElementKind::Perch,
            a: top,
            b: top,
            width,
            ..Default::default()
        }
    }

    pub fn line(kind: ElementKind, a: Vec3, b: Vec3, width: f32) -> Self {
        Self {
            kind,
            a,
            b,
            width,
            ..Default::default()
        }
    }

    pub fn face(kind: ElementKind, a: Vec3, b: Vec3, normal: Vec3, seed: u32) -> Self {
        Self {
            kind,
            a,
            b,
            width: 0.0,
            normal,
            seed,
            sag: 0.0,
        }
    }

    pub fn with_sag(self, sag: f32) -> Self {
        Self { sag, ..self }
    }

    /// In world space, for an entity at `t`.
    pub fn in_world(&self, entity: Entity, t: &Transform) -> Element {
        let at = |p: Vec3| t.translation + (t.rotation * (p * t.scale)).as_dvec3();
        Element {
            entity,
            kind: self.kind,
            a: at(self.a),
            b: at(self.b),
            width: self.width,
            normal: (t.rotation * self.normal).normalize_or(Vec3::Y),
            sag: self.sag,
            seed: self.seed,
        }
    }
}

/// An element in world space (what the movement step works with).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Element {
    pub entity: Entity,
    pub kind: ElementKind,
    pub a: DVec3,
    pub b: DVec3,
    pub width: f32,
    pub normal: Vec3,
    pub sag: f32,
    pub seed: u32,
}

impl Element {
    /// From `a` to `b`.
    pub fn span(&self) -> Vec3 {
        (self.b - self.a).as_vec3()
    }

    pub fn length(&self) -> f32 {
        self.span().length()
    }

    /// Unit direction from `a` to `b` (`fallback` for a point).
    pub fn dir(&self, fallback: Vec3) -> Vec3 {
        self.span().try_normalize().unwrap_or(fallback)
    }

    /// The point `s` meters from `a` toward `b` on the straight line (no sag).
    pub fn at(&self, s: f32) -> DVec3 {
        self.a + (self.dir(Vec3::X) * s).as_dvec3()
    }

    /// How far along the line the point nearest `p` is, m (clamped to the line).
    pub fn project(&self, p: DVec3) -> f32 {
        let len = self.length();
        if len < 1e-4 {
            return 0.0;
        }
        ((p - self.a).as_vec3().dot(self.dir(Vec3::X))).clamp(0.0, len)
    }

    /// How far a cable sags `s` meters along with nobody on it (a parabola through both
    /// ends), m.
    pub fn rest_sag(&self, s: f32) -> f32 {
        let len = self.length().max(1e-3);
        let u = (s / len).clamp(0.0, 1.0);
        4.0 * self.sag * u * (1.0 - u)
    }

    /// The top of the line `s` meters along (lines are marked along their walking top),
    /// pressed down by `sag` (rest plus load), against `up`.
    pub fn top_at(&self, s: f32, sag: f32, up: Vec3) -> DVec3 {
        self.at(s) - (up * sag).as_dvec3()
    }

    /// The face's frame for a rock or ice patch: (origin at `a`, across unit, up-the-face unit,
    /// width, height). Across runs from `a` toward `b`'s side; up is `normal × across`'s
    /// opposite so that it points up the face.
    pub fn face_frame(&self, up: Vec3) -> (DVec3, Vec3, Vec3, f32, f32) {
        let n = self.normal;
        // Up the face: world up with the normal taken out (on an overhang, still up).
        let fu = (up - n * up.dot(n)).normalize_or(up);
        let across = fu.cross(n).normalize_or(Vec3::X);
        let d = self.span();
        let (w, h) = (d.dot(across), d.dot(fu));
        // Origin at the lower-left corner whichever corners `a` and `b` are.
        let origin = self.a + (across * w.min(0.0)).as_dvec3() + (fu * h.min(0.0)).as_dvec3();
        (origin, across, fu, w.abs(), h.abs())
    }
}

/// Every traversal element and where it is.
pub(crate) type Elements<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static TraversalElement,
        &'static Transform,
        &'static GlobalTransform,
        Has<ChildOf>,
    ),
    Without<crate::mover::Mover>,
>;

/// What the states on elements read of the intent this step.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Input {
    /// The stick (horizontal, world, 0..1).
    pub wish: Vec3,
    /// Where the camera (or the brain) looks.
    pub look: Option<Vec3>,
    pub jump_pressed: bool,
    pub crouch: bool,
    pub crouch_pressed: bool,
    pub sprint: bool,
    /// Hit hard: torn off whatever is held.
    pub knocked: bool,
}

/// The velocity that moves with `moving` and closes the gap from `pos` to `want` at up to
/// `max` m/s (a body held by an element follows it, catching up after a bump).
pub(crate) fn follow(pos: DVec3, want: DVec3, moving: Vec3, max: f32, dt: f32) -> Vec3 {
    let free = pos + (moving * dt).as_dvec3();
    let fix = (want - free).as_vec3().clamp_length_max(max * dt);
    moving + fix / dt.max(1e-4)
}

/// Lets go of the element held (by state) and remembers it, so it isn't caught again at
/// once.
pub(crate) fn let_go(m: &mut crate::mover::Mover, entity: Option<Entity>, cooldown: f32) {
    m.leave_elements();
    m.elem_last = entity;
    m.elem_cooldown = cooldown;
    m.set_mode(engine_world::LocomotionMode::Air);
}

/// What the animation needs of the traversal state the mover is in (character space: `inv`
/// turns world directions into it, `feet` is its origin).
pub(crate) fn anim(
    m: &crate::mover::Mover,
    inv: &engine_core::math::Quat,
    feet: DVec3,
    height: f32,
) -> Option<crate::look::rig::TravAnim> {
    use crate::look::rig::TravAnim;
    let local = |p: DVec3| *inv * (p - feet).as_vec3();
    let _ = height;
    if let Some(td) = m.takedown {
        return Some(TravAnim::Takedown { t: td.t() });
    }
    if let Some(h) = m.perch {
        return Some(TravAnim::Perch { squat: h.squat() });
    }
    if let Some(h) = m.pole {
        let at = local(h.axis);
        return Some(TravAnim::Pole {
            at: Vec3::new(at.x, 0.0, at.z),
            speed: h.speed,
            radius: h.radius,
        });
    }
    if let Some(h) = m.bar {
        return Some(TravAnim::Bar {
            grip: local(h.at),
            swing: h.angle,
            rate: h.rate,
            along: h.along,
        });
    }
    if let Some(w) = m.line {
        return Some(TravAnim::Balance {
            lean: w.lean * w.heading,
            speed: w.speed.abs(),
            narrow: w.narrow,
        });
    }
    if let Some(f) = m.face {
        let (hands, feet_at) = f.contacts();
        // Where the eyes go: the hold the moving limb is heading for, else between the hands.
        let goal = f
            .moving
            .map_or((hands[0] + hands[1]) * 0.5, |mv| match mv.limb {
                0..=3 => mv.to[0],
                _ => (mv.to[0] + mv.to[1]) * 0.5,
            });
        return Some(TravAnim::Face {
            hands: hands.map(local),
            feet: feet_at.map(local),
            moving: f.moving.map(|mv| (mv.limb.min(3), mv.t)),
            ice: f.ice,
            goal: local(goal),
        });
    }
    if let Some(r) = m.roll {
        return Some(TravAnim::Roll { t: r.t() });
    }
    if let Some(speed) = m.crawl {
        return Some(TravAnim::Crawl { speed });
    }
    if m.recover.0 > 0.0 && m.recover.1 > 0.0 {
        // Down on a knee from a hard landing, coming up as it wears off.
        let k = m.recover.0 / m.recover.1;
        return Some(TravAnim::Perch {
            squat: (0.4 + 0.6 * k) * k.sqrt(),
        });
    }
    None
}

/// Nearest point to `p` on the segment `a`..`b`, and how far along it, 0..1.
pub fn nearest_on_segment(a: DVec3, b: DVec3, p: DVec3) -> (DVec3, f32) {
    let d = b - a;
    let len2 = d.length_squared();
    if len2 < 1e-9 {
        return (a, 0.0);
    }
    let t = ((p - a).dot(d) / len2).clamp(0.0, 1.0);
    (a + d * t, t as f32)
}

/// A hold on a rock face: where (on the face), which way it pulls best, what kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub at: DVec3,
    pub kind: HoldKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    /// A big hold the whole hand wraps: rest here.
    Jug,
    /// A thin edge for the fingertips.
    Crimp,
    /// A crack the hand jams in.
    Crack,
    /// Small, for the feet.
    Foot,
}

/// Holds on a rock face patch (`kind` [`ElementKind::Rock`]): a jittered grid 0.3–0.55 m apart,
/// with crack lines running up it, the same every time for the same seed.
pub fn rock_holds(e: &Element, up: Vec3) -> Vec<Hold> {
    let (origin, across, fu, w, h) = e.face_frame(up);
    let mut rng = e.seed.wrapping_mul(2654435761).wrapping_add(0x9E37_79B9);
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng as f32) / (u32::MAX as f32)
    };
    let step = 0.42;
    let cols = (w / step).floor().max(1.0) as i32;
    let rows = (h / step).floor().max(1.0) as i32;
    let mut out = Vec::with_capacity((cols * rows) as usize);
    // Two cracks running up the face: every row has a hold on them.
    let crack_x = [w * (0.25 + 0.1 * next()), w * (0.65 + 0.1 * next())];
    // Columns that lost their hold in the row below (never two gaps running up a column).
    let mut gap = vec![false; cols as usize + 1];
    for r in 0..=rows {
        for c in 0..=cols {
            let x = c as f32 * step + (next() - 0.5) * 0.22;
            let y = r as f32 * step + (next() - 0.5) * 0.18;
            if x < 0.05 || x > w - 0.05 || y < 0.1 || y > h - 0.05 {
                continue;
            }
            // Thin out: the face isn't a ladder.
            let thin = next() < 0.18 && !gap[c as usize];
            gap[c as usize] = thin;
            if thin {
                continue;
            }
            let kind = match next() {
                k if k < 0.3 => HoldKind::Jug,
                k if k < 0.7 => HoldKind::Crimp,
                _ => HoldKind::Foot,
            };
            out.push(Hold {
                at: origin + (across * x + fu * y).as_dvec3(),
                kind,
            });
        }
        for &cx in &crack_x {
            let y = r as f32 * step + step * 0.5;
            if y > 0.1 && y < h - 0.05 {
                let x = cx + (y * 1.7).sin() * 0.12;
                out.push(Hold {
                    at: origin + (across * x + fu * y).as_dvec3(),
                    kind: HoldKind::Crack,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(kind: ElementKind, a: DVec3, b: DVec3) -> Element {
        Element {
            entity: Entity::PLACEHOLDER,
            kind,
            a,
            b,
            width: 0.1,
            normal: Vec3::Z,
            sag: 0.3,
            seed: 7,
        }
    }

    #[test]
    fn a_cable_sags_most_at_mid_span() {
        let e = elem(
            ElementKind::Tightrope,
            DVec3::ZERO,
            DVec3::new(10.0, 0.0, 0.0),
        );
        assert!(e.rest_sag(0.0).abs() < 1e-5);
        assert!((e.rest_sag(5.0) - 0.3).abs() < 1e-5);
        assert!(e.rest_sag(2.5) < 0.3 && e.rest_sag(2.5) > 0.1);
    }

    #[test]
    fn rock_holds_cover_the_face_within_reach_of_each_other() {
        let e = elem(
            ElementKind::Rock,
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(4.0, 8.0, 0.0),
        );
        let holds = rock_holds(&e, Vec3::Y);
        assert!(holds.len() > 80, "{}", holds.len());
        for h in &holds {
            assert!(h.at.z.abs() < 1e-6);
            assert!((0.0..=4.0).contains(&h.at.x) && (0.0..=8.0).contains(&h.at.y));
        }
        // Every hold has a neighbour within an arm's reach above it (but the top row).
        for h in holds.iter().filter(|h| h.at.y < 7.0) {
            let above = holds
                .iter()
                .any(|o| o.at.y > h.at.y + 0.15 && (o.at - h.at).length() < 0.9);
            assert!(above, "{h:?}");
        }
        assert_eq!(holds, rock_holds(&e, Vec3::Y), "same seed, same holds");
    }

    #[test]
    fn the_face_frame_puts_up_the_face_up() {
        let e = elem(
            ElementKind::Ice,
            DVec3::new(4.0, 6.0, 0.0),
            DVec3::new(0.0, 0.0, 0.0),
        );
        let (o, across, fu, w, h) = e.face_frame(Vec3::Y);
        assert!(fu.dot(Vec3::Y) > 0.99);
        assert!((w - 4.0).abs() < 1e-4 && (h - 6.0).abs() < 1e-4);
        assert!(o.y.abs() < 1e-6, "{o:?} {across:?}");
    }
}

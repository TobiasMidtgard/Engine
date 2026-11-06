//! Soft things to fall on: an awning bounces a fall back up and out over the street (Assassin's
//! Creed's cloth canopies), hay takes any fall (the leap of faith; see the landing in
//! [`crate::locomotion`]).

use engine_core::math::Vec3;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::traverse::{Element, ElementKind};

/// How much of the speed into the cloth comes back out of it.
pub const RESTITUTION: f32 = 0.65;
/// Least speed a bounce sends the body up off the cloth, m/s.
const MIN_UP: f32 = 4.0;

/// Where `e` (an awning) is under the feet: the feet's height over the cloth, if over it.
fn over(e: &Element, b: &Body) -> Option<f32> {
    let (origin, across, fu, w, h) = e.face_frame(b.up);
    let rel = (b.feet() - origin).as_vec3();
    let (x, y) = (rel.dot(across), rel.dot(fu));
    let above = rel.dot(e.normal);
    ((-0.1..w + 0.1).contains(&x) && (-0.1..h + 0.1).contains(&y)).then_some(above)
}

/// Bounces off the awning `e`: the speed into the cloth comes back out (at least
/// [`MIN_UP`]), the speed along it keeps going, down its slope and out over its edge.
pub(crate) fn bounce(m: &mut Mover, b: &Body, e: &Element, v: Vec3, out: &mut Outputs) -> Vec3 {
    let n = e.normal;
    let into = v.dot(n).min(0.0);
    let along = v - n * v.dot(n);
    // Down the slope: the fall's speed partly turns along the cloth.
    let downhill = flat(-n, b.up).normalize_or_zero();
    let mut out_v = along + n * (-into * RESTITUTION) + downhill * (-into * 0.25);
    let up = out_v.dot(b.up);
    if up < MIN_UP {
        out_v += b.up * (MIN_UP - up);
    }
    m.elem_last = Some(e.entity);
    m.elem_cooldown = 0.4;
    m.fall_speed = 0.0;
    m.air_secs = 0.0;
    out.trigger(b.entity, "awning_bounce");
    out.impact(b.entity, ImpactKind::Bounce, b.feet(), n, -into);
    out_v
}

/// Falling onto an awning bounces off it.
pub(crate) fn capture(
    m: &mut Mover,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    out: &mut Outputs,
) -> Option<Vec3> {
    if v.dot(b.up) > -1.5 {
        return None;
    }
    let e = probe.elements_near(b.feet(), 0.5).find(|e| {
        e.kind == ElementKind::Awning
            && Some(e.entity) != m.elem_last
            && over(e, b).is_some_and(|above| (-0.3..0.25).contains(&above))
    })?;
    Some(bounce(m, b, &e, v, out))
}

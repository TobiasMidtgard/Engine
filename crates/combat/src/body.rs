//! Zone capsules: a body's parts as about twenty capsules built from its
//! [`SkeletonPose`](engine_world::animation::SkeletonPose), in the body's model space (origin
//! at the feet, facing −Z). A point finds the part it is on ([`BodyCapsules::zone_at`]), a
//! ray the part it passes nearest ([`BodyCapsules::pick`]). Knees are the region within
//! [`BodyZone::KNEE_RADIUS_M`] of the `lower_leg` bone's origin, as the contract says.
//!
//! No physics query: a pick is a few dozen segment distances (about 2 µs).

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::combat::BodyZone;

/// One capsule of a body part, model space, meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneCapsule {
    pub zone: BodyZone,
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}

impl ZoneCapsule {
    /// Distance from `p` to the capsule's surface (negative inside).
    pub fn distance(&self, p: Vec3) -> f32 {
        let (q, _) = closest_on_segment(self.a, self.b, p);
        q.distance(p) - self.radius
    }

    /// How well `p` sits in this capsule, for picking between overlapping parts: inside, the
    /// share of the radius it is off the axis minus one (−1 on the axis .. 0 on the surface);
    /// outside, meters off the surface. Lower is better.
    pub fn rank(&self, p: Vec3) -> f32 {
        let (q, _) = closest_on_segment(self.a, self.b, p);
        let d = q.distance(p);
        if d < self.radius {
            d / self.radius.max(1e-6) - 1.0
        } else {
            d - self.radius
        }
    }
}

/// A body's zone capsules, and where its knees are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BodyCapsules {
    pub capsules: Vec<ZoneCapsule>,
    /// Model-space knee joints (the `lower_leg` origins), left then right.
    pub knees: [Option<Vec3>; 2],
    /// Body size against a 1.75 m person (radii scale with it).
    pub scale: f32,
}

/// Bone names each zone's capsules run between, and their radius for a 1.75 m person.
const SEGMENTS: &[(BodyZone, &str, &str, f32)] = &[
    (BodyZone::Neck, "neck", "head", 0.06),
    (BodyZone::Chest, "chest", "neck", 0.15),
    (BodyZone::Chest, "upper_arm_l", "upper_arm_r", 0.1),
    (BodyZone::Belly, "hips", "chest", 0.15),
    (BodyZone::UpperArmL, "upper_arm_l", "lower_arm_l", 0.055),
    (BodyZone::UpperArmR, "upper_arm_r", "lower_arm_r", 0.055),
    (BodyZone::ForearmL, "lower_arm_l", "hand_l", 0.045),
    (BodyZone::ForearmR, "lower_arm_r", "hand_r", 0.045),
    (BodyZone::ThighL, "upper_leg_l", "lower_leg_l", 0.08),
    (BodyZone::ThighR, "upper_leg_r", "lower_leg_r", 0.08),
    (BodyZone::ShinL, "lower_leg_l", "foot_l", 0.055),
    (BodyZone::ShinR, "lower_leg_r", "foot_r", 0.055),
    (BodyZone::ShinL, "foot_l", "toe_l", 0.045),
    (BodyZone::ShinR, "foot_r", "toe_r", 0.045),
];

impl BodyCapsules {
    /// The capsules of a posed humanoid. Bones it lacks leave their parts out; a pose with no
    /// humanoid bones gives the stand-in of [`BodyCapsules::standing`].
    pub fn from_pose(pose: &SkeletonPose) -> Self {
        let at = |name: &str| pose.index_of(name).map(|i| pose.bones[i].translation);
        let Some(head) = at("head") else {
            return Self::standing(1.75);
        };
        let height = (head.y + 0.13).max(0.5);
        let scale = height / 1.75;
        let mut capsules = Vec::with_capacity(20);
        for &(zone, from, to, r) in SEGMENTS {
            if let (Some(a), Some(mut b)) = (at(from), at(to)) {
                // The chest stops below the neck's base rather than swallowing it.
                if zone == BodyZone::Chest && to == "neck" {
                    b -= (b - a).normalize_or_zero() * r * scale * 0.8;
                }
                capsules.push(ZoneCapsule {
                    zone,
                    a,
                    b,
                    radius: r * scale,
                });
            }
        }
        // The skull carries on past the head bone the way the neck points; the hands past the
        // wrists the way the forearms point.
        if let Some(neck) = at("neck") {
            let up = (head - neck).normalize_or(Vec3::Y);
            capsules.push(ZoneCapsule {
                zone: BodyZone::Head,
                a: head + up * 0.04 * scale,
                b: head + up * 0.15 * scale,
                radius: 0.1 * scale,
            });
        }
        for (zone, elbow, wrist) in [
            (BodyZone::HandL, "lower_arm_l", "hand_l"),
            (BodyZone::HandR, "lower_arm_r", "hand_r"),
        ] {
            if let (Some(e), Some(w)) = (at(elbow), at(wrist)) {
                let along = (w - e).normalize_or(Vec3::NEG_Y);
                capsules.push(ZoneCapsule {
                    zone,
                    a: w + along * 0.03 * scale,
                    b: w + along * 0.1 * scale,
                    radius: 0.045 * scale,
                });
            }
        }
        Self {
            capsules,
            knees: [at("lower_leg_l"), at("lower_leg_r")],
            scale,
        }
    }

    /// A body standing straight, arms at its sides (for bodies without a skeleton: the
    /// stand-in fighters, tests).
    pub fn standing(height: f32) -> Self {
        let s = height / 1.75;
        let v = |x: f32, y: f32| Vec3::new(x, y, 0.0) * s;
        let c = |zone, a: Vec3, b: Vec3, r: f32| ZoneCapsule {
            zone,
            a,
            b,
            radius: r * s,
        };
        let capsules = vec![
            c(BodyZone::Head, v(0.0, 1.62), v(0.0, 1.73), 0.1),
            c(BodyZone::Neck, v(0.0, 1.47), v(0.0, 1.58), 0.06),
            c(BodyZone::Chest, v(0.0, 1.2), v(0.0, 1.35), 0.15),
            c(BodyZone::Chest, v(-0.18, 1.4), v(0.18, 1.4), 0.1),
            c(BodyZone::Belly, v(0.0, 0.92), v(0.0, 1.18), 0.15),
            c(BodyZone::UpperArmL, v(-0.2, 1.4), v(-0.24, 1.12), 0.055),
            c(BodyZone::UpperArmR, v(0.2, 1.4), v(0.24, 1.12), 0.055),
            c(BodyZone::ForearmL, v(-0.24, 1.12), v(-0.26, 0.86), 0.045),
            c(BodyZone::ForearmR, v(0.24, 1.12), v(0.26, 0.86), 0.045),
            c(BodyZone::HandL, v(-0.26, 0.83), v(-0.27, 0.76), 0.045),
            c(BodyZone::HandR, v(0.26, 0.83), v(0.27, 0.76), 0.045),
            c(BodyZone::ThighL, v(-0.1, 0.92), v(-0.1, 0.5), 0.08),
            c(BodyZone::ThighR, v(0.1, 0.92), v(0.1, 0.5), 0.08),
            c(BodyZone::ShinL, v(-0.1, 0.5), v(-0.1, 0.08), 0.055),
            c(BodyZone::ShinR, v(0.1, 0.5), v(0.1, 0.08), 0.055),
        ];
        Self {
            capsules,
            knees: [Some(v(-0.1, 0.5)), Some(v(0.1, 0.5))],
            scale: s,
        }
    }

    /// The part a model-space point is on (or nearest), with knees taken from the leg by
    /// their radius. `None` only for a body with no capsules.
    pub fn zone_at(&self, p: Vec3) -> Option<BodyZone> {
        let best = self
            .capsules
            .iter()
            .min_by(|a, b| a.rank(p).total_cmp(&b.rank(p)))?;
        Some(self.knee_or(best.zone, p))
    }

    /// A thigh or shin point within the knee radius of its knee joint is the knee.
    fn knee_or(&self, zone: BodyZone, p: Vec3) -> BodyZone {
        let side = match zone {
            BodyZone::ThighL | BodyZone::ShinL => 0,
            BodyZone::ThighR | BodyZone::ShinR => 1,
            _ => return zone,
        };
        match self.knees[side] {
            Some(k) if k.distance(p) <= BodyZone::KNEE_RADIUS_M * self.scale => {
                [BodyZone::KneeL, BodyZone::KneeR][side]
            }
            _ => zone,
        }
    }

    /// The part a model-space ray (`origin`, unit `dir`) passes through or nearest, within
    /// `snap` meters of its surface: the zone and the point on the part (on the capsule's
    /// surface toward the ray). Of parts the ray goes through, the nearest along it wins.
    pub fn pick(&self, origin: Vec3, dir: Vec3, snap: f32) -> Option<(BodyZone, Vec3)> {
        let mut best: Option<(f32, f32, BodyZone, Vec3)> = None; // (miss, along, zone, point)
        for c in &self.capsules {
            let (t, on_ray, on_seg) = ray_segment(origin, dir, c.a, c.b);
            if t < 0.0 {
                continue;
            }
            let miss = (on_ray.distance(on_seg) - c.radius).max(0.0);
            if miss > snap {
                continue;
            }
            // Inside: rank by distance along the ray to the surface; outside: by the miss.
            let along = if miss == 0.0 {
                t - (c.radius * c.radius - on_ray.distance_squared(on_seg))
                    .max(0.0)
                    .sqrt()
            } else {
                t
            };
            let point = on_seg + (on_ray - on_seg).normalize_or(-dir) * c.radius;
            let better = match best {
                None => true,
                Some((m, a, ..)) => {
                    if miss == 0.0 && m == 0.0 {
                        along < a
                    } else {
                        miss < m
                    }
                }
            };
            if better {
                best = Some((miss, along, c.zone, point));
            }
        }
        best.map(|(_, _, zone, point)| (self.knee_or(zone, point), point))
    }

    /// The middle of a zone (its capsules' midpoint; a knee's joint).
    pub fn center(&self, zone: BodyZone) -> Option<Vec3> {
        match zone {
            BodyZone::KneeL => return self.knees[0],
            BodyZone::KneeR => return self.knees[1],
            _ => {}
        }
        let mut sum = Vec3::ZERO;
        let mut n = 0.0;
        for c in self.capsules.iter().filter(|c| c.zone == zone) {
            sum += (c.a + c.b) * 0.5;
            n += 1.0;
        }
        (n > 0.0).then(|| sum / n)
    }

    /// The capsules of one zone (a knee: a short piece of each leg capsule round the joint).
    pub fn of_zone(&self, zone: BodyZone) -> impl Iterator<Item = ZoneCapsule> + '_ {
        let knee = match zone {
            BodyZone::KneeL => Some((0, BodyZone::ThighL, BodyZone::ShinL)),
            BodyZone::KneeR => Some((1, BodyZone::ThighR, BodyZone::ShinR)),
            _ => None,
        };
        let r = BodyZone::KNEE_RADIUS_M * self.scale;
        self.capsules.iter().filter_map(move |c| match knee {
            None => (c.zone == zone).then_some(*c),
            Some((side, thigh, shin)) => {
                let k = self.knees[side]?;
                (c.zone == thigh || c.zone == shin).then(|| {
                    let far = if c.a.distance(k) < c.b.distance(k) {
                        c.b
                    } else {
                        c.a
                    };
                    ZoneCapsule {
                        zone,
                        a: k,
                        b: k + (far - k).normalize_or_zero() * r,
                        radius: c.radius * 1.1,
                    }
                })
            }
        })
    }
}

/// The point on segment `a`-`b` nearest `p`, and its parameter 0..1.
pub fn closest_on_segment(a: Vec3, b: Vec3, p: Vec3) -> (Vec3, f32) {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-12 {
        return (a, 0.0);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (a + ab * t, t)
}

/// Closest approach of a ray (`o`, unit `d`) and a segment: the ray parameter, the point on the
/// ray and the point on the segment.
pub fn ray_segment(o: Vec3, d: Vec3, a: Vec3, b: Vec3) -> (f32, Vec3, Vec3) {
    let u = b - a;
    let w = o - a;
    let uu = u.dot(u);
    let ud = u.dot(d);
    let uw = u.dot(w);
    let dw = d.dot(w);
    let denom = uu - ud * ud; // d is unit
    let mut s = if uu < 1e-12 {
        0.0
    } else if denom.abs() < 1e-9 {
        (uw / uu).clamp(0.0, 1.0)
    } else {
        ((uw - ud * dw) / denom).clamp(0.0, 1.0)
    };
    let mut t = (a + u * s - o).dot(d);
    if uu >= 1e-12 {
        // Re-project onto the segment for the clamped ray point.
        s = ((o + d * t - a).dot(u) / uu).clamp(0.0, 1.0);
        t = (a + u * s - o).dot(d);
    }
    (t, o + d * t, a + u * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_zone_is_found_on_a_standing_body() {
        let b = BodyCapsules::standing(1.75);
        for z in BodyZone::ALL {
            let c = b.center(z).unwrap_or_else(|| panic!("{z:?} has no middle"));
            assert_eq!(b.zone_at(c), Some(z), "{z:?} at {c}");
        }
    }

    #[test]
    fn a_point_by_the_knee_is_the_knee_and_further_up_the_thigh() {
        let b = BodyCapsules::standing(1.75);
        assert_eq!(
            b.zone_at(Vec3::new(0.1, 0.55, -0.07)),
            Some(BodyZone::KneeR)
        );
        assert_eq!(
            b.zone_at(Vec3::new(0.1, 0.75, -0.07)),
            Some(BodyZone::ThighR)
        );
        assert_eq!(
            b.zone_at(Vec3::new(-0.1, 0.25, -0.05)),
            Some(BodyZone::ShinL)
        );
    }

    #[test]
    fn a_ray_picks_the_part_in_front_and_snaps_near_misses() {
        let b = BodyCapsules::standing(1.75);
        // From the front (the body faces -Z), straight at the throat.
        let (z, p) = b.pick(Vec3::new(0.0, 1.52, -3.0), Vec3::Z, 0.25).unwrap();
        assert_eq!(z, BodyZone::Neck);
        assert!(p.z < 0.0, "the point is on the front: {p}");
        // Just beside the right knee, missing the leg by 5 cm.
        let (z, _) = b.pick(Vec3::new(0.24, 0.5, -3.0), Vec3::Z, 0.25).unwrap();
        assert_eq!(z, BodyZone::KneeR);
        // Well off the body: nothing.
        assert!(b.pick(Vec3::new(1.5, 1.0, -3.0), Vec3::Z, 0.25).is_none());
        // From above the shoulder down onto the head: the head, not the chest behind it.
        let (z, _) = b
            .pick(
                Vec3::new(0.0, 3.0, -1.0),
                Vec3::new(0.0, -1.3, 1.0).normalize(),
                0.1,
            )
            .unwrap();
        assert_eq!(z, BodyZone::Head);
    }

    #[test]
    fn a_pose_builds_capsules_on_its_bones() {
        let fit: Vec<(&str, Vec3)> = vec![
            ("root", Vec3::ZERO),
            ("hips", Vec3::new(0.0, 0.95, 0.0)),
            ("spine", Vec3::new(0.0, 1.1, 0.0)),
            ("chest", Vec3::new(0.0, 1.25, 0.0)),
            ("neck", Vec3::new(0.0, 1.5, 0.0)),
            ("head", Vec3::new(0.0, 1.6, 0.0)),
            ("upper_arm_l", Vec3::new(-0.2, 1.42, 0.0)),
            ("lower_arm_l", Vec3::new(-0.25, 1.12, 0.0)),
            ("hand_l", Vec3::new(-0.27, 0.86, 0.0)),
            ("upper_arm_r", Vec3::new(0.2, 1.42, 0.0)),
            ("lower_arm_r", Vec3::new(0.25, 1.12, 0.0)),
            ("hand_r", Vec3::new(0.27, 0.86, 0.0)),
            ("upper_leg_l", Vec3::new(-0.1, 0.92, 0.0)),
            ("lower_leg_l", Vec3::new(-0.1, 0.5, 0.0)),
            ("foot_l", Vec3::new(-0.1, 0.08, 0.0)),
            ("toe_l", Vec3::new(-0.1, 0.02, -0.14)),
            ("upper_leg_r", Vec3::new(0.1, 0.92, 0.0)),
            ("lower_leg_r", Vec3::new(0.1, 0.5, 0.0)),
            ("foot_r", Vec3::new(0.1, 0.08, 0.0)),
            ("toe_r", Vec3::new(0.1, 0.02, -0.14)),
        ];
        let pose = SkeletonPose {
            names: fit.iter().map(|(n, _)| n.to_string()).collect(),
            parents: fit.iter().map(|_| None).collect(),
            bones: fit
                .iter()
                .map(|(_, p)| engine_world::animation::BonePose {
                    translation: *p,
                    ..Default::default()
                })
                .collect(),
        };
        let b = BodyCapsules::from_pose(&pose);
        assert!((b.scale - 1.0).abs() < 0.02);
        for z in BodyZone::ALL {
            let c = b.center(z).unwrap();
            assert_eq!(b.zone_at(c), Some(z), "{z:?}");
            assert!(b.of_zone(z).count() > 0, "{z:?} has capsules to draw");
        }
    }
}

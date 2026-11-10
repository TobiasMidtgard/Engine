//! Hair and fur drawn by the shared strand renderer (`engine_render::strands`, Rendering's R2).
//!
//! Every hair body softbody simulates and draws itself (not people's hair from `HairGuides`,
//! which the character module draws) gets one [`StrandGroom`] built from its guides' rest
//! shapes, and an entity carrying a [`StrandInstance`] and the guides' posed points
//! ([`HairGuidesPosed`]) every frame:
//!
//! - **Model space** is the groom's anchor frame (the bone nearest the skeleton's root that
//!   carries hair: the head for scalp hair, the hips for a coat), so the instance follows the
//!   body rigidly and the guides carry only what moves relative to it (a swinging leg, the
//!   tail, sway).
//! - **Strands** are grown around their guide as Softbody's own strand shader grew them (a
//!   disc at the root, clumped toward the tip, curled, of varied length; see the retired
//!   `strand_common.wgsl` in git history), and each follows the three guides around its root,
//!   with the weights that rebuild the root from theirs (barycentric). The renderer moves
//!   strands by translation only (posed minus rest); a strand blended from guides that
//!   share its root's neighborhood lands on the posed skin even where a leg turns 90°
//!   against the hips, and only its few millimetres of curl and clumping turn with the anchor
//!   instead of the bone.
//! - **Length** variation ends a strand early by tapering its radius to zero there, rather
//!   than squeezing its points into a shorter span, so every point of a strand sits beside the
//!   same point of its guides and takes exactly their motion.
//!
//! The groom is built on a worker (`off_frame`): the dog's coat is 177 000 strands.

use engine_assets::{Assets, Handle};
use engine_core::off_frame::OffFrameCommands;
use engine_core::prelude::*;
use engine_render::{StrandFiber, StrandFollow, StrandGroom, StrandInstance};
use engine_world::HairGuidesPosed;

use crate::body::SoftBodySim;
use crate::hair::HairStyle;

/// Hair this long or longer is drawn as human hair, shorter as fur.
const FUR_BELOW: f32 = 0.15;
/// Guides with at least this many segments get extra drawn points between their particles
/// (Catmull-Rom): long hair bends smoothly instead of in 3.5 cm kinks.
const SMOOTH_FROM_SEGMENTS: u32 = 8;
/// Neighbor guides whose root normal turns more than this from a strand's own (cosine) are
/// across a thin part (an ear, the tail) and aren't blended.
const SAME_SIDE: f32 = 0.25;
/// Barycentric weights below this mean the root is well outside its guides' triangle:
/// inverse-distance weights instead.
const MIN_WEIGHT: f32 = -0.25;
/// Wet hair darkens to this at soaked (the renderer has no wetness of its own).
const WET_DARKEN: f32 = 0.55;

/// On a hair body: the entity drawing its strands, and what its groom was built from.
#[derive(Component, Debug, Clone)]
pub struct DrawnStrands {
    pub entity: Entity,
    pub groom: Handle<StrandGroom>,
    key: GroomKey,
}

/// On the entity drawing a hair body's strands: that body.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrandsOf(pub Entity);

/// On a hair body whose groom is being built.
#[derive(Component, Debug, Clone)]
pub struct Grooming(GroomKey);

/// What a groom depends on: a rebuilt body (new guides or style) gets a new groom.
#[derive(Debug, Clone, PartialEq)]
struct GroomKey {
    particles: usize,
    guides: usize,
    anchor: u16,
    sub: u32,
    style: HairStyle,
    color: [f32; 3],
    /// Grown after the body's settling steps. Hair is grown at once, on its styled shape, and
    /// again once settled (styled shapes sag and swing in the settling steps), so the
    /// renderer's bounds and self-shadow volume, made from the rest shape, fit it.
    settled: bool,
}

impl GroomKey {
    /// The same particles and guides: a groom of the other still poses from this body.
    fn same_layout(&self, o: &Self) -> bool {
        (self.particles, self.guides, self.anchor, self.sub)
            == (o.particles, o.guides, o.anchor, o.sub)
    }

    fn of(body: &SoftBodySim) -> Option<Self> {
        let s = body.strands.as_ref()?;
        let first = s.guides.first()?;
        Some(Self {
            particles: body.sim.len(),
            guides: s.guides.len(),
            anchor: s.anchor,
            sub: if first.count > SMOOTH_FROM_SEGMENTS {
                2
            } else {
                1
            },
            style: s.style,
            color: s.color,
            settled: body.pending_prewarm == 0,
        })
    }
}

/// A body's guides at rest, in the anchor frame's space: what a groom is built from.
#[derive(Debug, Clone)]
pub struct GroomInput {
    pub style: HairStyle,
    /// Linear RGB.
    pub color: [f32; 3],
    /// Particles per guide.
    pub count: usize,
    /// Drawn points per guide segment.
    pub sub: u32,
    /// `count` points per guide, root first.
    pub points: Vec<Vec3>,
    /// Per guide: root normal and tangent.
    pub frames: Vec<(Vec3, Vec3)>,
}

impl GroomInput {
    /// The body's guides as they are now, in its anchor frame (`None` for a body without
    /// strands, or whose guides differ in length).
    pub fn of(body: &SoftBodySim) -> Option<Self> {
        let key = GroomKey::of(body)?;
        let s = body.strands.as_ref()?;
        let count = s.guides[0].count as usize;
        if count < 2 || s.guides.iter().any(|g| g.count as usize != count) {
            return None;
        }
        let (at, turn) = body.sim.frames.get(s.anchor as usize)?.pose(1.0);
        let back = turn.inverse();
        let mut points = Vec::with_capacity(s.guides.len() * count);
        let mut frames = Vec::with_capacity(s.guides.len());
        for g in &s.guides {
            let range = g.first as usize..g.first as usize + count;
            points.extend(body.sim.pos.get(range)?.iter().map(|p| back * (*p - at)));
            let rot = body
                .sim
                .frames
                .get(g.frame as usize)
                .map_or(turn, |f| f.pose(1.0).1);
            frames.push((back * rot * g.normal, back * rot * g.tangent));
        }
        Some(Self {
            style: key.style,
            color: key.color,
            count,
            sub: key.sub,
            points,
            frames,
        })
    }

    /// Drawn points per strand.
    pub fn points_per(&self) -> usize {
        (self.count - 1) * self.sub as usize + 1
    }
}

/// Catmull-Rom through `src`, `sub` points per segment (a copy for `sub` 1), into `out`.
fn smooth(src: &[Vec3], sub: u32, out: &mut Vec<Vec3>) {
    if sub <= 1 || src.len() < 2 {
        out.extend_from_slice(src);
        return;
    }
    let last = src.len() - 1;
    for i in 0..last {
        let p0 = src[i.saturating_sub(1)];
        let (p1, p2) = (src[i], src[i + 1]);
        let p3 = src[(i + 2).min(last)];
        for s in 0..sub {
            let u = s as f32 / sub as f32;
            let a = 2.0 * p1;
            let b = p2 - p0;
            let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
            let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
            out.push(0.5 * (a + b * u + c * u * u + d * u * u * u));
        }
    }
    out.push(src[last]);
}

/// Random 0..1 from an integer (the same hash the old strand shader used, so strands keep
/// their place, length and shade).
fn hash_u(x: u32) -> f32 {
    let mut h = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277_803_737);
    h = (h >> 22) ^ h;
    h as f32 / 4_294_967_295.0
}

/// Guide roots bucketed in a grid, for the guides around a strand's root.
pub(crate) struct RootGrid {
    cell: f32,
    map: std::collections::HashMap<IVec3, Vec<u32>>,
}

impl RootGrid {
    pub(crate) fn new(roots: impl Iterator<Item = Vec3>, cell: f32) -> Self {
        let mut map: std::collections::HashMap<IVec3, Vec<u32>> = Default::default();
        for (i, r) in roots.enumerate() {
            map.entry((r / cell).floor().as_ivec3())
                .or_default()
                .push(i as u32);
        }
        Self { cell, map }
    }

    /// The three nearest guides to `p` (on its side: `keep`), nearest first.
    pub(crate) fn nearest3(
        &self,
        p: Vec3,
        root: impl Fn(u32) -> Vec3,
        keep: impl Fn(u32) -> bool,
    ) -> [(u32, f32); 3] {
        let mut best = [(u32::MAX, f32::MAX); 3];
        let c = (p / self.cell).floor().as_ivec3();
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(list) = self.map.get(&(c + IVec3::new(dx, dy, dz))) else {
                        continue;
                    };
                    for &g in list {
                        let d = root(g).distance_squared(p);
                        if d >= best[2].1 || !keep(g) {
                            continue;
                        }
                        best[2] = (g, d);
                        if best[2].1 < best[1].1 {
                            best.swap(1, 2);
                        }
                        if best[1].1 < best[0].1 {
                            best.swap(0, 1);
                        }
                    }
                }
            }
        }
        best
    }
}

/// Weights of `a`, `b`, `c` rebuilding `p` (projected onto their plane), when it lies near
/// enough to their triangle.
fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<[f32; 3]> {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
    let (d20, d21) = (v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    // Nearly collinear: no plane to project on.
    if den <= 0.02 * d00 * d11 || den <= 0.0 {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    let u = 1.0 - v - w;
    (u.min(v).min(w) >= MIN_WEIGHT).then_some([u, v, w])
}

/// The guides a strand rooted at `p` follows and how much of each.
pub(crate) fn follow(near: [(u32, f32); 3], p: Vec3, root: impl Fn(u32) -> Vec3) -> StrandFollow {
    let found = near.iter().filter(|(g, _)| *g != u32::MAX).count();
    let mut f = StrandFollow::default();
    if found == 0 {
        return f;
    }
    if found == 3 {
        let [a, b, c] = near.map(|(g, _)| root(g));
        if let Some(w) = barycentric(p, a, b, c) {
            f.guides = near.map(|(g, _)| g);
            f.weights = w;
            return f;
        }
    }
    // Inverse distance (squared) over the ones found.
    let mut sum = 0.0;
    for (k, (g, d2)) in near.iter().take(found).enumerate() {
        let w = 1.0 / (d2 + 1e-8);
        f.guides[k] = *g;
        f.weights[k] = w;
        sum += w;
    }
    for w in &mut f.weights {
        *w /= sum;
    }
    f
}

/// Build the groom: `strands_per_guide` strands around every guide.
pub fn build_groom(input: &GroomInput) -> StrandGroom {
    let style = &input.style;
    let per = input.points_per();
    let guides = input.frames.len();
    // Guides at rest, smoothed to the drawn points.
    let mut rest = Vec::with_capacity(guides * per);
    for g in 0..guides {
        smooth(
            &input.points[g * input.count..(g + 1) * input.count],
            input.sub,
            &mut rest,
        );
    }
    let root = |g: u32| rest[g as usize * per];
    let grid = RootGrid::new(
        (0..guides as u32).map(root),
        (style.spread * 1.5).max(0.006),
    );
    let fur = style.length < FUR_BELOW;
    let spg = style.strands_per_guide.max(1) as usize;
    let strands = guides * spg;
    let mut out = StrandGroom {
        fiber: if fur {
            StrandFiber::Fur
        } else {
            StrandFiber::Human
        },
        points_per: per as u32,
        points: Vec::with_capacity(strands * per),
        roots: Vec::with_capacity(strands),
        tips: if fur {
            Vec::with_capacity(strands)
        } else {
            Vec::new()
        },
        guides: rest.iter().map(|p| p.to_array()).collect(),
        follow: Vec::with_capacity(strands),
        thin: false,
    };
    let [r, g, b] = input.color;
    let color = Vec3::new(r, g, b);
    let mut tangents = vec![Vec3::ZERO; per];
    for k in 0..guides {
        let pts = &rest[k * per..(k + 1) * per];
        for (j, t) in tangents.iter_mut().enumerate() {
            let a = pts[j.saturating_sub(1)];
            let b = pts[(j + 1).min(per - 1)];
            *t = (b - a).normalize_or(input.frames[k].0);
        }
        let length: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let (n0, t0) = input.frames[k];
        let t0 = (t0 - n0 * t0.dot(n0)).normalize_or(n0.any_orthonormal_vector());
        let b0 = n0.cross(t0);
        for child in 0..spg {
            let seed = (k as u32)
                .wrapping_mul(7919)
                .wrapping_add((child as u32).wrapping_mul(104_729));
            let [h1, h2, h3, h4] = [0, 1, 2, 3].map(|i| hash_u(seed.wrapping_add(i)));
            let len_factor = (1.0 - style.length_variation * h3).max(0.05);
            let r = if child == 0 {
                0.0
            } else {
                style.spread * h1.sqrt()
            };
            let theta = std::f32::consts::TAU * h2;
            let off0 = (t0 * theta.cos() + b0 * theta.sin()) * r;
            for j in 0..per {
                let t = j as f32 / (per - 1) as f32;
                let tan = tangents[j];
                let mut off = off0 - tan * off0.dot(tan);
                off *= 1.0 - style.clumping * t.powf(0.8);
                if style.curl > 0.0 {
                    let mut u = off - tan * off.dot(tan);
                    if u.length_squared() < 1e-12 {
                        u = tan.cross(n0);
                    }
                    let u = u.normalize_or(tan.any_orthonormal_vector());
                    let v = tan.cross(u);
                    let phi = std::f32::consts::TAU
                        * (style.curl_frequency * t * length * len_factor + h4);
                    off += (u * phi.cos() + v * phi.sin()) * style.curl * smoothstep(0.0, 0.2, t);
                }
                let p = pts[j] + off;
                // Thinning to the tip, and ending at `len_factor` of the guide.
                let radius = if t <= len_factor + 1e-4 {
                    style.thickness * (1.0 - 0.8 * t / len_factor)
                } else {
                    0.0
                };
                out.points.push([p.x, p.y, p.z, radius]);
            }
            let shade = 0.8 + 0.4 * h4;
            let rand = hash_u(seed.wrapping_add(4));
            let c = color * shade;
            out.roots.push([c.x, c.y, c.z, rand]);
            if fur {
                // Fur tips are lighter where the pigment thins (agouti banding, sun bleaching).
                let tip = (c * Vec3::new(1.12, 1.09, 1.03)).min(Vec3::splat(0.95));
                out.tips.push([tip.x, tip.y, tip.z, 0.4]);
            }
            let start = pts[0] + off0;
            let near = grid.nearest3(start, root, |g| {
                input.frames[g as usize].0.dot(n0) > SAME_SIDE
            });
            out.follow.push(follow(near, start, root));
        }
    }
    out
}

/// Pose every drawn groom: its entity at the anchor frame, its guides' points in that frame,
/// between the last two steps as the body is drawn; build grooms for new hair bodies on a
/// worker; drop drawing entities whose body is gone or was rebuilt.
#[allow(clippy::type_complexity)]
pub fn draw_strands(
    mut commands: Commands,
    time: Option<Res<FixedTime>>,
    bodies: Query<(
        Entity,
        &SoftBodySim,
        Option<&DrawnStrands>,
        Option<&Grooming>,
        Option<&Name>,
    )>,
    mut drawn: Query<(
        Entity,
        &StrandsOf,
        &mut Transform,
        &mut HairGuidesPosed,
        &mut StrandInstance,
    )>,
    mut grooms: Option<ResMut<Assets<StrandGroom>>>,
) {
    let alpha = time.map_or(1.0, |t| t.alpha());
    for (entity, body, shown, grooming, name) in &bodies {
        if body.drawn_elsewhere || body.sim.is_empty() {
            continue;
        }
        let Some(key) = GroomKey::of(body) else {
            continue;
        };
        // Settled, rebuilt (new guides) or restyled: a new groom, the old one drawn until it
        // lands.
        if shown.is_some_and(|d| d.key == key) || grooming.is_some_and(|g| g.0 == key) {
            continue;
        }
        let Some(input) = GroomInput::of(body) else {
            continue;
        };
        let Some(&(at, turn)) = body
            .sim
            .frames
            .get(key.anchor as usize)
            .map(|f| f.pose(1.0))
            .as_ref()
        else {
            continue;
        };
        let place =
            Transform::from_translation(body.sim.origin + at.as_dvec3()).with_rotation(turn);
        let label = name.map_or_else(|| "hair".to_string(), |n| n.as_str().to_string());
        commands.entity(entity).insert(Grooming(key.clone()));
        commands.off_frame(
            format!("softbody groom {label}"),
            move || {
                let t = std::time::Instant::now();
                let groom = build_groom(&input);
                (groom, t.elapsed().as_secs_f32() * 1000.0)
            },
            move |world: &mut World, (groom, ms): (StrandGroom, f32)| {
                // The body may have gone or been rebuilt meanwhile.
                let fits = world
                    .get::<SoftBodySim>(entity)
                    .and_then(GroomKey::of)
                    .is_some_and(|k| {
                        k.same_layout(&key) && (k.style, k.color) == (key.style, key.color)
                    });
                if !fits {
                    if let Ok(mut e) = world.get_entity_mut(entity) {
                        e.remove::<Grooming>();
                    }
                    return;
                }
                tracing::info!(
                    "softbody: {label} grown, {} strands of {} points, in {ms:.0} ms{}",
                    groom.strand_count(),
                    groom.points_per,
                    if key.settled {
                        ""
                    } else {
                        " (before settling)"
                    }
                );
                let old = world.get::<DrawnStrands>(entity).cloned();
                let mut assets = world.get_resource_or_insert_with(Assets::<StrandGroom>::default);
                if let Some(old) = &old {
                    assets.remove(old.groom);
                }
                let handle = assets.add(groom);
                if let Some(old) = old {
                    world.despawn(old.entity);
                }
                let shown = world
                    .spawn((
                        Name::new(format!("{label} strands")),
                        place,
                        GlobalTransform::from(place),
                        StrandsOf(entity),
                        StrandInstance::new(handle),
                        HairGuidesPosed::default(),
                    ))
                    .id();
                world
                    .entity_mut(entity)
                    .remove::<Grooming>()
                    .insert(DrawnStrands {
                        entity: shown,
                        groom: handle,
                        key,
                    });
            },
        );
    }
    let mut buf: Vec<Vec3> = Vec::new();
    for (e, of, mut t, mut posed, mut instance) in &mut drawn {
        let Ok((_, body, Some(d), _, _)) = bodies.get(of.0) else {
            // Its body is gone.
            commands.entity(e).despawn();
            if let Some(g) = grooms.as_mut() {
                g.remove(instance.groom);
            }
            continue;
        };
        if d.entity != e {
            commands.entity(e).despawn();
            continue;
        }
        let Some(s) = body.strands.as_ref() else {
            continue;
        };
        // Rebuilt with other guides: its new groom is on the way.
        if !GroomKey::of(body).is_some_and(|k| k.same_layout(&d.key)) {
            continue;
        }
        let a = body.draw_alpha(alpha);
        let Some((at, turn)) = body
            .sim
            .frames
            .get(d.key.anchor as usize)
            .map(|f| f.pose(a))
        else {
            continue;
        };
        let place =
            Transform::from_translation(body.sim.origin + at.as_dvec3()).with_rotation(turn);
        *t = place;
        let back = turn.inverse();
        let count = s.guides[0].count as usize;
        let points = &mut posed.points;
        points.clear();
        for g in &s.guides {
            buf.clear();
            buf.extend(
                (g.first as usize..g.first as usize + count)
                    .map(|i| back * (body.sim.interpolated(i, a) - at)),
            );
            smooth(&buf, d.key.sub, points);
        }
        // Wet hair is darker.
        let wet = body.wet.iter().sum::<f32>() / body.wet.len().max(1) as f32;
        let tint = 1.0 - (1.0 - WET_DARKEN) * wet.clamp(0.0, 1.0);
        if instance.tint[0] != tint {
            instance.tint = [tint; 3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(guides: &[(Vec3, Vec3)]) -> GroomInput {
        let count = 4;
        let mut points = Vec::new();
        for (root, n) in guides {
            for j in 0..count {
                points.push(*root + *n * 0.01 * j as f32);
            }
        }
        GroomInput {
            style: HairStyle {
                strands_per_guide: 12,
                ..HairStyle::fur()
            },
            color: [0.3, 0.2, 0.1],
            count,
            sub: 1,
            points,
            frames: guides
                .iter()
                .map(|(_, n)| (*n, n.any_orthonormal_vector()))
                .collect(),
        }
    }

    #[test]
    fn a_groom_is_valid_and_its_strands_start_where_their_guides_rebuild_them() {
        // A patch of skin with guides 1 cm apart.
        let mut guides = Vec::new();
        for x in 0..6 {
            for z in 0..6 {
                guides.push((Vec3::new(x as f32 * 0.01, 0.0, z as f32 * 0.01), Vec3::Y));
            }
        }
        let inp = input(&guides);
        let groom = build_groom(&inp);
        assert!(groom.is_valid());
        assert_eq!(groom.strand_count(), guides.len() * 12);
        assert_eq!(groom.fiber, StrandFiber::Fur);
        let per = groom.points_per as usize;
        let mut rebuilt = 0;
        for (s, f) in groom.follow.iter().enumerate() {
            let sum: f32 = f.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "weights sum to one");
            let root = Vec3::from_slice(&groom.points[s * per][..3]);
            let mut blend = Vec3::ZERO;
            for (g, w) in f.guides.iter().zip(f.weights) {
                if *g != u32::MAX {
                    blend += Vec3::from(groom.guides[*g as usize * per]) * w;
                }
            }
            if blend.distance(root) < 1e-4 {
                rebuilt += 1;
            }
        }
        // All but strands past the patch's edge.
        assert!(rebuilt * 10 > groom.strand_count() * 7, "{rebuilt}");
    }

    #[test]
    fn a_turned_limb_carries_its_strands_roots_on_the_skin() {
        // A patch of guides 1 cm apart; turn it 90° about z (a leg swinging against the hips):
        // by the renderer's rule (rest plus weighted guide motion) the roots of strands inside
        // the patch stay on the turned patch, where following their own guide alone would
        // leave them up to the spread (2 cm) off it.
        let guides: Vec<_> = (0..12)
            .flat_map(|x| {
                (0..12).map(move |z| (Vec3::new(x as f32 * 0.01, 0.0, z as f32 * 0.01), Vec3::Y))
            })
            .collect();
        let groom = build_groom(&input(&guides));
        let per = groom.points_per as usize;
        let turn = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let posed: Vec<Vec3> = groom.guides.iter().map(|g| turn * Vec3::from(*g)).collect();
        let mut worst: f32 = 0.0;
        let mut worst_translation_only: f32 = 0.0;
        for (s, f) in groom.follow.iter().enumerate() {
            let rest = Vec3::from_slice(&groom.points[s * per][..3]);
            let inside = |v: f32| (0.015..=0.095).contains(&v);
            if !inside(rest.x) || !inside(rest.z) {
                continue;
            }
            let mut p = rest;
            for (g, w) in f.guides.iter().zip(f.weights) {
                if *g != u32::MAX {
                    let i = *g as usize * per;
                    p += (posed[i] - Vec3::from(groom.guides[i])) * w;
                }
            }
            worst = worst.max(p.distance(turn * rest));
            // Following its own guide alone (the guide it was grown around).
            let own = (s / 12) * per;
            let q = rest + posed[own] - Vec3::from(groom.guides[own]);
            worst_translation_only = worst_translation_only.max(q.distance(turn * rest));
        }
        assert!(worst < 0.001, "roots off the turned skin by {worst} m");
        assert!(
            worst_translation_only > 10.0 * worst,
            "{worst_translation_only} vs {worst}"
        );
    }

    #[test]
    fn smoothing_keeps_the_guide_points() {
        let src = [Vec3::ZERO, Vec3::X, Vec3::new(2.0, 1.0, 0.0)];
        let mut out = Vec::new();
        smooth(&src, 2, &mut out);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], src[0]);
        assert!((out[2] - src[1]).length() < 1e-6);
        assert_eq!(out[4], src[2]);
    }
}

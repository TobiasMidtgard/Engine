//! The trail walkers' bodies: smooth procedural meshes (lofted cross-sections, sculpted heads)
//! hung on bones, posed every frame from each walker's gait.
//!
//! The pose is a pure function of how far the walker has come (see [`gait`]), so slow motion
//! slows the limbs exactly as much as the walking, the runner who ignores slow motion keeps
//! his stride, and each planted foot sits on the footprint `walk_trail` reports for it.
//!
//! Each part follows one bone. Near a joint (a hip, a knee, a shoulder, the root of a neck) its
//! vertices blend toward the bone on the other side, turning about the joint, and that part's
//! vertices blend back toward it, so the two meet halfway and limbs bend smoothly instead of
//! passing through each other.

use std::rc::Rc;
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Mesh, MeshRenderer, StandardMaterial, Vertex};
use engine_world::animation::{BodyRequest, SkeletonPose};

use super::{TrailWalker, yaw};
use rig::Xf;

pub(super) mod gait;
mod horse;
mod human;
mod quadruped;
mod rig;
mod sdf;
mod shape;
mod textures;

/// How much a point (in its part's bone frame) follows a neighbouring bone instead, 0..1.
pub(crate) type Weight = Rc<dyn Fn(Vec3) -> f32>;

/// A neighbouring bone a part blends toward near the joint they share.
#[derive(Clone)]
pub(crate) struct Blend {
    bone: usize,
    /// The joint is the neighbour's origin (it hangs from this part), not this part's own.
    child: bool,
    weight: Weight,
}

/// One mesh of a body, hung on a bone.
pub(crate) struct Part {
    pub bone: usize,
    /// Asset key the mesh is cached under, so a second trail reuses it.
    pub key: String,
    pub mesh: Box<dyn Fn() -> Mesh>,
    pub material: StandardMaterial,
    /// Up to two neighbouring bones this part blends toward.
    pub skin: Vec<Blend>,
}

/// Collects a body's parts under `audio/trail/<who>/`.
pub(crate) struct Parts {
    who: &'static str,
    pub list: Vec<Part>,
    /// Blends for the next part added (see [`Parts::parent`]).
    next: Vec<Blend>,
}

impl Parts {
    pub fn new(who: &'static str) -> Self {
        Self {
            who,
            list: Vec::new(),
            next: Vec::new(),
        }
    }

    /// The next part added (both meshes of a [`Parts::furry`] one) blends toward the bone it
    /// hangs from by `weight` near their joint (this part's origin).
    pub fn parent(&mut self, bone: usize, weight: impl Fn(Vec3) -> f32 + 'static) -> &mut Self {
        self.next.push(Blend {
            bone,
            child: false,
            weight: Rc::new(weight),
        });
        self
    }

    /// The next part added blends toward a bone hanging from it by `weight` near their joint
    /// (that bone's origin).
    pub fn child(&mut self, bone: usize, weight: impl Fn(Vec3) -> f32 + 'static) -> &mut Self {
        self.next.push(Blend {
            bone,
            child: true,
            weight: Rc::new(weight),
        });
        self
    }

    pub fn add(
        &mut self,
        bone: usize,
        name: &str,
        mesh: impl Fn() -> Mesh + 'static,
        m: StandardMaterial,
    ) {
        let skin = std::mem::take(&mut self.next);
        self.push(bone, name, Box::new(mesh), m, skin);
    }

    fn push(
        &mut self,
        bone: usize,
        name: &str,
        mesh: Box<dyn Fn() -> Mesh>,
        material: StandardMaterial,
        skin: Vec<Blend>,
    ) {
        debug_assert!(skin.len() <= 2, "{name}: at most two blends");
        self.list.push(Part {
            bone,
            key: format!("audio/trail/{}/{name}", self.who),
            mesh,
            material,
            skin,
        });
    }

    /// A part, and fur shells over it (built in the same space, then both moved by `shift`).
    #[allow(clippy::too_many_arguments)]
    pub fn furry(
        &mut self,
        bone: usize,
        name: &str,
        mesh: impl Fn() -> Mesh + 'static,
        m: StandardMaterial,
        shell: StandardMaterial,
        layers: usize,
        fur: impl Fn(Vec3, Vec3) -> (f32, Vec3) + 'static,
        shift: Vec3,
    ) {
        let skin = std::mem::take(&mut self.next);
        let base = Rc::new(mesh);
        let b = base.clone();
        let place = move |m: Mesh| shape::placed(m, Quat::IDENTITY, shift);
        self.push(bone, name, Box::new(move || place(b())), m, skin.clone());
        self.push(
            bone,
            &format!("{name}_fur"),
            Box::new(move || place(shape::shells(&base(), layers, quadruped::CUT, &fur))),
            shell,
            skin,
        );
    }
}

/// A piece of a trail walker's body: which bone of its walker's rig it follows.
#[derive(Component, Clone, Copy, Debug)]
pub struct BodyPart(pub u8);

/// A part whose vertices near its joints blend toward the neighbouring bones: its own copy of
/// the mesh, rewritten when a joint bends.
#[derive(Component)]
pub(super) struct Skin {
    /// The neighbouring bones, and whether each hangs from this part.
    bones: Vec<(u8, bool)>,
    mesh: Handle<Mesh>,
    rest: Arc<Vec<Vertex>>,
    /// (vertex, weight toward each neighbour) for every vertex that moves.
    weights: Arc<Vec<(u32, [f32; 2])>>,
    last: Vec<Xf>,
}

/// How to pose a walker's body.
#[derive(Component, Clone, Copy, Debug)]
pub(super) enum Rig {
    Human(human::Build),
    Dog,
    Horse,
}

impl Rig {
    /// Every bone this frame, in the walker's root space.
    fn pose(&self, w: &TrailWalker) -> [Xf; MAX_BONES] {
        let mut out = [Xf::IDENTITY; MAX_BONES];
        match self {
            Rig::Human(b) => human::pose(w, b, &mut out),
            Rig::Dog => quadruped::pose(w, &quadruped::DOG, &mut out),
            Rig::Horse => quadruped::pose(w, &quadruped::HORSE, &mut out),
        }
        out
    }

    /// The rest pose the meshes were built in.
    fn rest(&self) -> [Xf; MAX_BONES] {
        let mut out = [Xf::IDENTITY; MAX_BONES];
        match self {
            Rig::Human(b) => human::rest(b, &mut out),
            Rig::Dog => quadruped::rest(&quadruped::DOG, &mut out),
            Rig::Horse => quadruped::rest(&quadruped::HORSE, &mut out),
        }
        out
    }
}

const MAX_BONES: usize = 32;

/// Spawn a walker and its body parts (meshes are built once and cached by key; without a
/// renderer the parts are spawned without meshes and still animate).
fn spawn(
    commands: &mut Commands,
    name: &'static str,
    walker: TrailWalker,
    scale: Option<LocalTimeScale>,
    body: Option<BodyRequest>,
    rig: Rig,
    parts: impl FnOnce(&textures::Textures) -> Vec<Part> + Send + 'static,
) {
    commands.queue(move |world: &mut World| {
        let tex = textures::Textures::get(world);
        let parts = parts(&tex);
        let mut built: Vec<(Option<Handle<Mesh>>, Option<Skin>)> = Vec::with_capacity(parts.len());
        match world.get_resource_mut::<Assets<Mesh>>() {
            Some(mut meshes) => {
                for p in &parts {
                    let h = meshes.get_or_insert_keyed(&p.key, || (p.mesh)());
                    let skin = (!p.skin.is_empty()).then_some(()).and_then(|()| {
                        let rest = meshes.get(h)?.clone();
                        let weights: Vec<(u32, [f32; 2])> = rest
                            .vertices
                            .iter()
                            .enumerate()
                            .filter_map(|(i, v)| {
                                let mut w = [0.0; 2];
                                for (j, b) in p.skin.iter().take(2).enumerate() {
                                    w[j] = (b.weight)(Vec3::from(v.position)).clamp(0.0, 1.0);
                                }
                                (w[0] > 1e-3 || w[1] > 1e-3).then_some((i as u32, w))
                            })
                            .collect();
                        if weights.is_empty() {
                            return None;
                        }
                        let rest_vertices = Arc::new(rest.vertices.clone());
                        Some(Skin {
                            bones: p
                                .skin
                                .iter()
                                .take(2)
                                .map(|b| (b.bone as u8, b.child))
                                .collect(),
                            mesh: meshes.add(rest),
                            rest: rest_vertices,
                            weights: Arc::new(weights),
                            last: Vec::new(),
                        })
                    });
                    built.push((Some(skin.as_ref().map_or(h, |s| s.mesh)), skin));
                }
            }
            None => built.extend(parts.iter().map(|_| (None, None))),
        }
        let (pos, dir) = gait::root(&walker);
        let mut root = world.spawn((
            Name::new(name),
            Transform::from_translation(pos).with_rotation(yaw(dir)),
            walker,
            rig,
        ));
        if let Some(s) = scale {
            root.insert(s);
        }
        if let Some(b) = body {
            root.insert(b);
        }
        let root = root.id();
        for (p, (h, skin)) in parts.iter().zip(built) {
            let mut e = world.spawn((Transform::default(), BodyPart(p.bone as u8), ChildOf(root)));
            if let Some(h) = h {
                e.insert(MeshRenderer::new(h, p.material));
            }
            if let Some(s) = skin {
                e.insert(s);
            }
        }
    });
}

/// A hiker (`runner` false: red shell jacket, daypack, boots) or a runner (fitted top,
/// tights, running shoes). `garment` is the jacket or top colour (sRGB hex).
///
/// Both ask the animation module for a body (a scanned person, animated from how the walker
/// moves); the body built here stands in until one arrives, and stays without one.
pub(super) fn person(
    commands: &mut Commands,
    walker: TrailWalker,
    garment: u32,
    runner: bool,
    scale: Option<LocalTimeScale>,
) {
    let build = if runner { human::RUNNER } else { human::HIKER };
    let body = if runner {
        BodyRequest::human(2).with_model("anim/characters/athlete.glb")
    } else {
        BodyRequest::human(1).with_model("anim/characters/walker.glb")
    };
    spawn(
        commands,
        if runner {
            "Trail runner"
        } else {
            "Trail hiker"
        },
        walker,
        scale,
        Some(body),
        Rig::Human(build),
        move |tex| human::parts(&build, garment, tex),
    );
}

/// A brown retriever, standing in until the animation module has a dog.
pub(super) fn dog(commands: &mut Commands, walker: TrailWalker) {
    spawn(
        commands,
        "Trail dog",
        walker,
        None,
        Some(BodyRequest::new("dog").with_height(0.56).with_seed(4)),
        Rig::Dog,
        quadruped::dog_parts,
    );
}

/// A bay horse, standing in until the animation module's horse arrives.
pub(super) fn horse(commands: &mut Commands, walker: TrailWalker) {
    spawn(
        commands,
        "Trail horse",
        walker,
        None,
        Some(BodyRequest::new("horse").with_height(1.52).with_seed(3)),
        Rig::Horse,
        horse::parts,
    );
}

/// Pose every walker's body for this frame, and bend the meshes at their joints.
#[allow(clippy::type_complexity)]
pub(super) fn animate(
    walkers: Query<(&TrailWalker, &Rig, &Children, Has<SkeletonPose>)>,
    mut parts: Query<
        (
            &BodyPart,
            &mut Transform,
            Option<&mut Skin>,
            Option<&mut MeshRenderer>,
        ),
        Without<TrailWalker>,
    >,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    for (w, rig, children, dressed) in &walkers {
        // The animation module has given this walker a body: hide the stand-in (and show it
        // again if the body goes).
        for &child in &children[..] {
            if let Ok((_, _, _, Some(mut r))) = parts.get_mut(child)
                && r.visible == dressed
            {
                r.visible = !dressed;
            }
        }
        if dressed {
            continue;
        }
        let pose = rig.pose(w);
        let rest = rig.rest();
        for &child in &children[..] {
            let Ok((part, mut t, skin, _)) = parts.get_mut(child) else {
                continue;
            };
            let bone = part.0 as usize;
            let x = pose[bone];
            t.translation = x.t.as_dvec3();
            t.rotation = x.r;
            let (Some(mut skin), Some(meshes)) = (skin, meshes.as_deref_mut()) else {
                continue;
            };
            // Where each neighbour would carry this part (in the part's own frame), and the
            // joint it turns about.
            let joints: Vec<(Xf, Vec3)> = skin
                .bones
                .iter()
                .map(|&(o, child)| {
                    let o = o as usize;
                    let m = x
                        .inverse()
                        .mul(pose[o])
                        .mul(rest[o].inverse())
                        .mul(rest[bone]);
                    let c = if child {
                        rest[bone].inverse().point(rest[o].t)
                    } else {
                        Vec3::ZERO
                    };
                    (m, c)
                })
                .collect();
            let same =
                skin.last.len() == joints.len()
                    && skin.last.iter().zip(&joints).all(|(l, (m, _))| {
                        l.r.abs_diff_eq(m.r, 1e-6) && l.t.abs_diff_eq(m.t, 1e-6)
                    });
            if same {
                continue;
            }
            skin.last = joints.iter().map(|j| j.0).collect();
            if let Some(mesh) = meshes.get_mut(skin.mesh) {
                bend(&mut mesh.vertices, &skin.rest, &skin.weights, &joints);
            }
        }
    }
}

/// Turn each weighted vertex part of the way toward where each neighbour would carry it: a
/// rotation about their joint, so the bend keeps its volume.
fn bend(out: &mut [Vertex], rest: &[Vertex], weights: &[(u32, [f32; 2])], joints: &[(Xf, Vec3)]) {
    for &(i, w) in weights {
        let i = i as usize;
        let (Some(v), Some(src)) = (out.get_mut(i), rest.get(i)) else {
            continue;
        };
        let p0 = Vec3::from(src.position);
        let n0 = Vec3::from(src.normal);
        let (mut p, mut n) = (p0, n0);
        for (&(m, c), &w) in joints.iter().zip(&w) {
            if w <= 0.0 {
                continue;
            }
            let q = Quat::IDENTITY.lerp(m.r, w);
            p += c + q * (p0 - c) + (m.point(c) - c) * w - p0;
            n += q * n0 - n0;
        }
        v.position = p.to_array();
        v.normal = n.normalize_or(n0).to_array();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::DVec3;
    use engine_world::ContactKind;

    /// Every mesh of every walker builds with finite vertices and in-range indices.
    #[test]
    fn every_part_builds_a_sound_mesh() {
        let tex = textures::Textures::default();
        let all = [
            human::parts(&human::HIKER, 0xb3362c, &tex),
            human::parts(&human::RUNNER, 0x2c5fb3, &tex),
            quadruped::dog_parts(&tex),
            horse::parts(&tex),
        ];
        for parts in all {
            for p in parts {
                let m = (p.mesh)();
                assert!(!m.vertices.is_empty(), "{} is empty", p.key);
                for v in &m.vertices {
                    let ok = v.position.iter().chain(&v.normal).all(|x| x.is_finite());
                    assert!(ok, "{} has a non-finite vertex", p.key);
                }
                let n = m.vertices.len() as u32;
                assert!(
                    m.indices.iter().all(|&i| i < n),
                    "{} indexes past its vertices",
                    p.key
                );
                assert!(p.skin.len() <= 2, "{} blends toward too many bones", p.key);
            }
        }
    }

    /// The rest pose puts every joint where the walking rig does: each bone's origin sits at
    /// the same place on the bone it hangs from, so blending across a joint only turns
    /// vertices about it.
    #[test]
    fn the_rest_pose_matches_the_rig() {
        use human::{CHEST, FOOT, FOREARM, HAND, PELVIS, SHIN, THIGH, UPPER_ARM};
        use quadruped::{EAR, FRONT, NECK, PASTERN, REAR, TAIL, leg};
        let mut people = vec![(PELVIS, CHEST), (CHEST, human::HEAD)];
        for i in 0..2 {
            people.extend([
                (PELVIS, THIGH[i]),
                (THIGH[i], SHIN[i]),
                (SHIN[i], FOOT[i]),
                (CHEST, UPPER_ARM[i]),
                (UPPER_ARM[i], FOREARM[i]),
                (FOREARM[i], HAND[i]),
            ]);
        }
        let mut animals = vec![
            (FRONT, NECK),
            (NECK, quadruped::HEAD),
            (quadruped::HEAD, EAR[0]),
            (quadruped::HEAD, EAR[1]),
            (REAR, TAIL[0]),
            (TAIL[0], TAIL[1]),
            (TAIL[1], TAIL[2]),
        ];
        for (f, pastern) in PASTERN.into_iter().enumerate() {
            let b = leg(f);
            let body = if f % 2 == 0 { FRONT } else { REAR };
            animals.extend([(body, b), (b, b + 1), (b + 1, b + 2), (b + 2, pastern)]);
        }
        let mut dog = animals.clone();
        dog.extend((0..4).map(|f| (leg(f) + 2, leg(f) + 3)));
        let cases = [
            (
                Rig::Human(human::HIKER),
                ContactKind::Foot,
                1.4,
                people.clone(),
            ),
            (Rig::Human(human::RUNNER), ContactKind::Foot, 4.0, people),
            (Rig::Dog, ContactKind::Paw, 1.4, dog),
            (Rig::Horse, ContactKind::Hoof, 1.4, animals),
        ];
        for (rig, kind, speed, pairs) in cases {
            let mut w = TrailWalker::new(DVec3::new(3.0, 0.0, 1.0), 33.0, kind, 70.0, speed);
            let rest = rig.rest();
            for _ in 0..40 {
                w.along += 0.037;
                w.to_next_step -= 0.037;
                while w.to_next_step <= 0.0 {
                    w.to_next_step += w.step_len;
                    w.next_foot = (w.next_foot + 1) % w.feet;
                }
                let pose = rig.pose(&w);
                for &(p, c) in &pairs {
                    let posed = pose[p].inverse().point(pose[c].t);
                    let rested = rest[p].inverse().point(rest[c].t);
                    assert!(
                        posed.distance(rested) < 2e-3,
                        "{rig:?}: bone {c} on {p} at {posed}, rest {rested}"
                    );
                }
            }
        }
    }
}

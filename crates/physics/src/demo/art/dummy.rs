//! A Hybrid III-style crash test dummy: the ten ragdoll parts that tumble down the staircase
//! and a standing figure for the stand-in characters on the bridge and the turntable.
//!
//! Yellow satin vinyl skin, black rubber joint sleeves, a segmented black neck, a waist
//! bellows, steel hubs and bolts at the shoulders, elbows, hips and knees, mitten hands, black
//! shoes, and black-and-yellow quadrant calibration targets on the sides of the head, the
//! chest and the hips (H-points) and on the knees. A molded face (brow, nose bridge, cheeks,
//! jaw, chin) without eyes, and a rib contour on the chest jacket.
//!
//! Every fleshy part is an implicit shape (smoothly blended ellipsoids, tapered tubes and
//! capsules plus molded details) that a wrap of rings is shot onto from a spine: the surface
//! follows the shape exactly, is shaded with the shape's true normals (no faceting) and gets
//! its triangles where it curves. Crevices are darkened slightly in the vertex colors
//! (baked ambient occlusion).
//!
//! Joints follow the real dummy: each part ends in a shape centered on its pivot (a ball for
//! the neck, shoulders, waist and hips, a cylinder around the hinge axis for elbows and
//! knees) and the neighbor's sleeve covers it, so bending never opens a hole.
//!
//! ## Meshes and materials
//!
//! Every part is a [`DummyMesh`]: `vinyl` and `steel`.
//!
//! * `vinyl` carries all colors in its vertex colors (yellow skin, the shirt-colored chest
//!   jacket, black rubber, black shoes, the target pattern). Draw it with a near-white satin
//!   vinyl material ([`vinyl_material`], or the `vinyl_skin` texture set: the colors are
//!   tuned for its ~0.78 albedo).
//! * `steel`: hubs, bolts, the bellows clamp ([`steel_material`]).
//!
//! UVs are metric (1 UV unit = 1 m), as everywhere in [`super::kit`].
//!
//! ## Frames
//!
//! The dummy faces +Z with Y up; +X is its left. [`RagdollLayout`] gives bodies, colliders
//! and joints with the soles at the origin. [`ragdoll_parts`] returns each part in its own
//! body frame (origin at the body's center, rest pose); [`standing_dummy`] returns one figure
//! centered on a character capsule.
//!
//! Triangles: just under 25 000 per dummy (both meshes, all parts).

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4, smoothstep};
use engine_render::{Mesh, StandardMaterial};

use super::kit::{MeshBuilder, srgb};

// =============================================================================================
// Public API
// =============================================================================================

/// A capsule collider along its body's local Y (as `Collider::capsule(radius, half_height)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapsuleSpec {
    /// Body center, meters, in the layout's frame (soles at the origin).
    pub center: Vec3,
    pub radius: f32,
    /// Half the length of the straight part (total height = 2 × (half_height + radius)).
    pub half_height: f32,
}

/// Where a ragdoll's bodies, colliders and joints are, in meters: soles at the origin,
/// standing upright, facing +Z, +X is the dummy's left. Paired parts are given for the left
/// side; the right side mirrors them in X. Every body starts unrotated.
///
/// [`RagdollLayout::PROVING_GROUND`] is the ragdoll `demo::ragdoll` spawns today;
/// [`RagdollLayout::HYBRID_III_50TH`] (the default) is the same ragdoll with 50th-percentile
/// male proportions (1.79 m), a forearm collider that covers the hand and a shin collider
/// that reaches the heel. The meshes adapt to either.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RagdollLayout {
    /// Pelvis body center and its cuboid's half extents.
    pub pelvis: Vec3,
    pub pelvis_half: Vec3,
    /// Chest capsule.
    pub chest: CapsuleSpec,
    /// Head body center and its sphere's radius.
    pub head: Vec3,
    pub head_radius: f32,
    pub upper_arm: CapsuleSpec,
    /// Forearm capsule (the hand is part of this body).
    pub forearm: CapsuleSpec,
    pub thigh: CapsuleSpec,
    /// Shin capsule (the foot is part of this body).
    pub shin: CapsuleSpec,
    /// Joint pivots: pelvis–chest, chest–head, chest–upper arm, upper arm–forearm (hinge
    /// about X), pelvis–thigh, thigh–shin (hinge about X).
    pub waist: Vec3,
    pub neck: Vec3,
    pub shoulder: Vec3,
    pub elbow: Vec3,
    pub hip: Vec3,
    pub knee: Vec3,
    /// Where the hand starts (no joint: landmarks for the sculpt).
    pub wrist: Vec3,
    /// Where the shoe starts.
    pub ankle: Vec3,
}

impl RagdollLayout {
    /// The ragdoll in `demo::ragdoll` as it is today (1.92 m tall: long torso, head high).
    pub const PROVING_GROUND: Self = Self {
        pelvis: Vec3::new(0.0, 1.0, 0.0),
        pelvis_half: Vec3::new(0.16, 0.1, 0.1),
        chest: CapsuleSpec {
            center: Vec3::new(0.0, 1.38, 0.0),
            radius: 0.15,
            half_height: 0.12,
        },
        head: Vec3::new(0.0, 1.8, 0.0),
        head_radius: 0.12,
        upper_arm: CapsuleSpec {
            center: Vec3::new(0.3, 1.38, 0.0),
            radius: 0.05,
            half_height: 0.1,
        },
        forearm: CapsuleSpec {
            center: Vec3::new(0.3, 1.08, 0.0),
            radius: 0.045,
            half_height: 0.1,
        },
        thigh: CapsuleSpec {
            center: Vec3::new(0.1, 0.7, 0.0),
            radius: 0.07,
            half_height: 0.14,
        },
        shin: CapsuleSpec {
            center: Vec3::new(0.1, 0.28, 0.0),
            radius: 0.06,
            half_height: 0.15,
        },
        waist: Vec3::new(0.0, 1.1, 0.0),
        neck: Vec3::new(0.0, 1.66, 0.0),
        shoulder: Vec3::new(0.3, 1.55, 0.0),
        elbow: Vec3::new(0.3, 1.23, 0.0),
        hip: Vec3::new(0.1, 0.92, 0.0),
        knee: Vec3::new(0.1, 0.5, 0.0),
        wrist: Vec3::new(0.3, 0.975, 0.0),
        ankle: Vec3::new(0.1, 0.075, 0.0),
    };

    /// A 50th-percentile male (Hybrid III 50th: 1.79 m to the crown): the proving-ground
    /// ragdoll with the upper body lowered 0.1 – 0.16 m (neck pivot at the base of the neck,
    /// chest capsule ending there), the shoulders brought in to ±0.2 m, the forearm collider
    /// lengthened to cover the curled hand and the shin collider lengthened to reach the
    /// heel. Legs, pelvis and waist are unchanged.
    pub const HYBRID_III_50TH: Self = Self {
        pelvis: Vec3::new(0.0, 1.0, 0.0),
        pelvis_half: Vec3::new(0.16, 0.1, 0.1),
        chest: CapsuleSpec {
            center: Vec3::new(0.0, 1.3, 0.0),
            radius: 0.14,
            half_height: 0.06,
        },
        head: Vec3::new(0.0, 1.675, 0.0),
        head_radius: 0.12,
        upper_arm: CapsuleSpec {
            center: Vec3::new(0.2, 1.285, 0.0),
            radius: 0.05,
            half_height: 0.105,
        },
        forearm: CapsuleSpec {
            center: Vec3::new(0.2, 0.925, 0.0),
            radius: 0.045,
            half_height: 0.155,
        },
        thigh: CapsuleSpec {
            center: Vec3::new(0.1, 0.7, 0.0),
            radius: 0.07,
            half_height: 0.14,
        },
        shin: CapsuleSpec {
            center: Vec3::new(0.1, 0.245, 0.0),
            radius: 0.06,
            half_height: 0.185,
        },
        waist: Vec3::new(0.0, 1.1, 0.0),
        neck: Vec3::new(0.0, 1.5, 0.0),
        shoulder: Vec3::new(0.2, 1.45, 0.0),
        elbow: Vec3::new(0.2, 1.13, 0.0),
        hip: Vec3::new(0.1, 0.92, 0.0),
        knee: Vec3::new(0.1, 0.5, 0.0),
        wrist: Vec3::new(0.2, 0.865, 0.0),
        ankle: Vec3::new(0.1, 0.075, 0.0),
    };
}

impl Default for RagdollLayout {
    fn default() -> Self {
        Self::HYBRID_III_50TH
    }
}

/// One part's meshes: `vinyl` (vertex-colored; [`vinyl_material`]) and `steel` (hubs and
/// bolts; [`steel_material`]). `steel` may be empty.
#[derive(Debug, Clone, Default)]
pub struct DummyMesh {
    pub vinyl: Mesh,
    pub steel: Mesh,
}

impl DummyMesh {
    /// Triangles in both meshes.
    pub fn triangle_count(&self) -> usize {
        self.vinyl.triangle_count() + self.steel.triangle_count()
    }

    fn merge(&mut self, other: &DummyMesh) {
        self.vinyl.merge(&other.vinyl);
        self.steel.merge(&other.steel);
    }

    fn transformed(&self, m: Mat4) -> DummyMesh {
        let mut out = self.clone();
        out.vinyl.transform(&m);
        out.steel.transform(&m);
        out
    }

    fn mirrored(&self) -> DummyMesh {
        DummyMesh {
            vinyl: mirror_x(&self.vinyl),
            steel: mirror_x(&self.steel),
        }
    }
}

/// The ten ragdoll parts, each in its own body frame: origin at the body's center from the
/// [`RagdollLayout`], rest pose (standing, facing +Z). Paired parts are `[right, left]`, i.e.
/// `[side -1, side +1]` in X, the order `demo::ragdoll` spawns them in.
#[derive(Debug, Clone, Default)]
pub struct RagdollMeshes {
    pub pelvis: DummyMesh,
    pub chest: DummyMesh,
    /// The head with the segmented neck (which pivots with it about the neck joint).
    pub head: DummyMesh,
    pub upper_arm: [DummyMesh; 2],
    /// Forearm with wrist sleeve and mitten hand.
    pub forearm: [DummyMesh; 2],
    pub thigh: [DummyMesh; 2],
    /// Shin with the shoe.
    pub shin: [DummyMesh; 2],
}

impl RagdollMeshes {
    /// Every part, for counting or iterating: pelvis, chest, head, then right/left upper
    /// arms, forearms, thighs and shins.
    pub fn all(&self) -> [&DummyMesh; 11] {
        [
            &self.pelvis,
            &self.chest,
            &self.head,
            &self.upper_arm[0],
            &self.upper_arm[1],
            &self.forearm[0],
            &self.forearm[1],
            &self.thigh[0],
            &self.thigh[1],
            &self.shin[0],
            &self.shin[1],
        ]
    }

    /// Triangles of the whole dummy (both meshes, all parts).
    pub fn triangle_count(&self) -> usize {
        self.all().iter().map(|m| m.triangle_count()).sum()
    }
}

/// Build the ragdoll's part meshes for `layout`. `shirt` (linear RGBA, e.g.
/// `kit::srgb(0x3a6ea5)`) colors the chest jacket, so dummies stay distinguishable; head,
/// limbs and pelvis keep the crash-test yellow. Takes a few tens of milliseconds (release).
pub fn ragdoll_parts(layout: &RagdollLayout, shirt: Vec4) -> RagdollMeshes {
    let s = sculpt(layout, shirt);
    let at = |p: &DummyMesh, center: Vec3| p.transformed(Mat4::from_translation(-center));
    let pair = |p: &DummyMesh, center: Vec3| {
        let left = at(p, center);
        [left.mirrored(), left]
    };
    RagdollMeshes {
        pelvis: at(&s.pelvis, layout.pelvis),
        chest: at(&s.chest, layout.chest.center),
        head: at(&s.head, layout.head),
        upper_arm: pair(&s.upper_arm, layout.upper_arm.center),
        forearm: pair(&s.forearm, layout.forearm.center),
        thigh: pair(&s.thigh, layout.thigh.center),
        shin: pair(&s.shin, layout.shin.center),
    }
}

/// One dummy `height` meters tall (crown to soles) standing with its arms a little away from
/// its sides, centered on a character capsule of the same height: the soles are at
/// `y = -height / 2`, the crown at `+height / 2`, facing +Z. With `height = 1.8` it fills the
/// default `CharacterController` (radius 0.35, height 1.8): the hands stay within 0.34 m of
/// the axis, the toes 0.23 m in front of it. `shirt` colors the chest jacket (linear RGBA).
pub fn standing_dummy(height: f32, shirt: Vec4) -> DummyMesh {
    let l = RagdollLayout::HYBRID_III_50TH;
    let s = sculpt(&l, shirt);
    let about = |pivot: Vec3, r: Quat| {
        Mat4::from_translation(pivot) * Mat4::from_quat(r) * Mat4::from_translation(-pivot)
    };
    let arm = about(
        l.shoulder,
        Quat::from_rotation_z(0.17) * Quat::from_rotation_x(-0.05),
    );
    let fore = arm * about(l.elbow, Quat::from_rotation_x(-0.22));
    let leg = about(l.hip, Quat::from_rotation_z(0.025));
    let mut left = s.upper_arm.transformed(arm);
    left.merge(&s.forearm.transformed(fore));
    left.merge(&s.thigh.transformed(leg));
    left.merge(&s.shin.transformed(leg));
    let mut out = s.head.clone();
    out.merge(&s.chest);
    out.merge(&s.pelvis);
    out.merge(&left);
    out.merge(&left.mirrored());
    let top = out
        .vinyl
        .vertices
        .iter()
        .map(|v| v.position[1])
        .fold(0.0f32, f32::max);
    let k = height / top;
    out.transformed(
        Mat4::from_translation(Vec3::Y * (-0.5 * height)) * Mat4::from_scale(Vec3::splat(k)),
    )
}

/// The vinyl material the dummy's vertex colors are tuned for: near-white satin (albedo like
/// the `vinyl_skin` texture), roughness 0.45, a little subsurface wrap.
pub fn vinyl_material() -> StandardMaterial {
    StandardMaterial::hex(0xe4e4e2)
        .with_roughness(0.45)
        .with_subsurface(0.1)
}

/// Machined steel for hubs and bolts.
pub fn steel_material() -> StandardMaterial {
    StandardMaterial::hex(0x8a8d92)
        .with_metallic(1.0)
        .with_roughness(0.4)
}

/// Crash-test yellow (linear vertex color).
pub fn yellow() -> Vec4 {
    srgb(0xffcf26)
}

/// Black rubber (linear vertex color).
pub fn rubber() -> Vec4 {
    srgb(0x242426)
}

fn shoe_black() -> Vec4 {
    srgb(0x1a1a1c)
}

fn sole_gray() -> Vec4 {
    srgb(0x3c3b3a)
}

fn lace_gray() -> Vec4 {
    srgb(0x48484a)
}

// =============================================================================================
// Sculpt: every part in the layout's frame (soles at the origin), paired parts on the left
// =============================================================================================

struct Sculpt {
    head: DummyMesh,
    chest: DummyMesh,
    pelvis: DummyMesh,
    upper_arm: DummyMesh,
    forearm: DummyMesh,
    thigh: DummyMesh,
    shin: DummyMesh,
}

fn sculpt(l: &RagdollLayout, shirt: Vec4) -> Sculpt {
    Sculpt {
        head: head(l),
        chest: chest(l, shirt),
        pelvis: pelvis(l),
        upper_arm: upper_arm(l),
        forearm: forearm(l),
        thigh: thigh(l),
        shin: shin(l),
    }
}

/// Two builders (vinyl, steel) for one part.
struct Part {
    vinyl: MeshBuilder,
    steel: MeshBuilder,
}

impl Part {
    fn new() -> Self {
        Self {
            vinyl: MeshBuilder::new(),
            steel: MeshBuilder::new(),
        }
    }

    fn finish(self) -> DummyMesh {
        debug_assert!(
            [&self.vinyl, &self.steel].iter().all(|b| b
                .mesh
                .vertices
                .iter()
                .all(|v| v.position.iter().all(|c| c.is_finite()))),
            "dummy part with non-finite vertices"
        );
        DummyMesh {
            vinyl: self.vinyl.build(),
            steel: self.steel.build(),
        }
    }
}

const fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

// ---------------------------------------------------------------------------------------------
// Head and neck
// ---------------------------------------------------------------------------------------------

/// The skull and face around the head's center (unscaled: sized for a 0.12 m head sphere).
fn head_field(q: Vec3) -> f32 {
    // Cranium and face.
    let mut d = ellipsoid(q, v3(0.0, 0.02, -0.012), v3(0.08, 0.098, 0.1));
    d = smin(
        d,
        ellipsoid(q, v3(0.0, -0.026, 0.016), v3(0.06, 0.078, 0.072)),
        0.03,
    );
    for sx in [-1.0, 1.0] {
        // Jaw line to the chin, cheekbones.
        d = smin(
            d,
            capsule(
                q,
                v3(sx * 0.052, -0.056, -0.012),
                v3(sx * 0.02, -0.094, 0.058),
                0.016,
            ),
            0.02,
        );
        d = smin(
            d,
            ellipsoid(q, v3(sx * 0.045, -0.008, 0.058), v3(0.022, 0.014, 0.022)),
            0.02,
        );
    }
    // Chin.
    d = smin(
        d,
        ellipsoid(q, v3(0.0, -0.094, 0.072), v3(0.028, 0.019, 0.019)),
        0.014,
    );
    // Brow ridge.
    d = smin(
        d,
        capsule(q, v3(-0.036, 0.028, 0.082), v3(0.036, 0.028, 0.082), 0.014),
        0.022,
    );
    // Shallow eye sockets.
    for sx in [-1.0, 1.0] {
        d = smax(
            d,
            -ellipsoid(q, v3(sx * 0.033, 0.004, 0.1), v3(0.02, 0.013, 0.016)),
            0.012,
        );
    }
    // Nose bridge and nose, nostril wings.
    d = smin(
        d,
        round_cone(
            q,
            v3(0.0, 0.014, 0.093),
            v3(0.0, -0.03, 0.109),
            0.0075,
            0.0115,
        ),
        0.012,
    );
    for sx in [-1.0, 1.0] {
        d = smin(
            d,
            ellipsoid(
                q,
                v3(sx * 0.0115, -0.034, 0.099),
                v3(0.0095, 0.0085, 0.0095),
            ),
            0.006,
        );
    }
    // Closed lips, barely molded.
    d = smin(
        d,
        ellipsoid(q, v3(0.0, -0.055, 0.081), v3(0.02, 0.006, 0.009)),
        0.006,
    );
    smin(
        d,
        ellipsoid(q, v3(0.0, -0.067, 0.079), v3(0.017, 0.006, 0.009)),
        0.006,
    )
}

fn head(l: &RagdollLayout) -> DummyMesh {
    let c = l.head;
    let s = l.head_radius / 0.12;
    let f = |p: Vec3| head_field((p - c) / s) * s;
    let mut part = Part::new();
    let core = c + v3(0.0, -0.01, 0.004) * s;
    let face = |th: f32| 1.0 + 1.4 * (-(ang(th, 0.0) / 0.55).powi(2)).exp();
    let band = |sg: f32| 1.0 + 0.9 * (-((sg - 0.42) / 0.2).powi(2)).exp();
    Wrap::new(
        &[core - Vec3::Y * 0.006 * s, core + Vec3::Y * 0.006 * s],
        Vec3::Z,
    )
    .hint(0.1 * s, 0.1 * s)
    .size(38, 30)
    .reach(0.2 * s)
    .cols_weight(&face)
    .rows_weight(&band)
    .skin(&mut part.vinyl, &f, yellow(), 1.0);
    for sx in [-1.0, 1.0] {
        target(
            &mut part.vinyl,
            &f,
            c + v3(sx * 0.09, -0.004, -0.014) * s,
            Vec3::Y,
            0.027 * s,
        );
    }

    // The segmented rubber neck: pucks between aluminum discs, pivoting at its base.
    let base = l.neck;
    let top = c + v3(0.0, -0.062, -0.02) * s;
    let axis = (top - base).normalize();
    let len = top.distance(base);
    let r = 0.041 * s;
    let mut profile = vec![Vec2::new(0.0, -r)];
    for k in 1..=4 {
        let a = -FRAC_PI_2 + FRAC_PI_2 * k as f32 / 4.0;
        profile.push(Vec2::new(r * a.cos(), r * a.sin()));
    }
    let pucks = 4;
    let pitch = (len - 0.002) / pucks as f32;
    for i in 0..pucks {
        let y0 = pitch * i as f32;
        let first = if i == 0 { 1.0 } else { 0.9 };
        profile.push(Vec2::new(r * first, y0 + pitch * 0.18));
        profile.push(Vec2::new(r, y0 + pitch * 0.5));
        profile.push(Vec2::new(r * 0.93, y0 + pitch * 0.8));
        profile.push(Vec2::new(r * 0.8, y0 + pitch));
    }
    profile.push(Vec2::new(r * 0.8, len + 0.03 * s));
    profile.push(Vec2::new(0.0, len + 0.03 * s));
    part.vinyl.colored(rubber(), |b| {
        b.placed(frame_y(base, axis), |b| b.lathe(&profile, 16, 55.0));
    });
    // Pivot pin of the nodding joint, just under the skull.
    let mut under = 0.0;
    while under < len && f(base + axis * under) > 0.0 {
        under += 0.001;
    }
    for sx in [-1.0, 1.0] {
        let at = base + axis * (under - 0.011 * s) + Vec3::X * (sx * 0.84 * r);
        bolt(&mut part.steel, at, Vec3::X * sx, 0.0065 * s, true);
    }
    part.finish()
}

// ---------------------------------------------------------------------------------------------
// Chest jacket
// ---------------------------------------------------------------------------------------------

/// Molded ribcage on the jacket: broad rib ridges sweeping down from the sternum toward
/// the front, and the costal arch, below which the belly steps back. Outward offset, meters.
fn ribs(p: Vec3, lo: f32, hi: f32) -> f32 {
    let a = p.x.atan2(p.z).abs();
    if hi <= lo || a > 2.3 {
        return 0.0;
    }
    let side = (a / FRAC_PI_2).min(1.4);
    let fade = smoothstep(0.12, 0.35, a) * (1.0 - smoothstep(1.7, 2.2, a));
    let mut g = 0.0;
    for i in 0..3 {
        let yi = lo + (hi - lo) * (0.25 + 0.75 * i as f32 / 2.0) + 0.03 * side;
        g += (-((p.y - yi) / 0.015).powi(2)).exp();
    }
    // Costal arch: an inverted V from the sternum's tip down to the sides.
    let arch = hi - 0.01 - 0.075 * (a / 1.1).min(1.0) * (1.0 - smoothstep(1.3, 1.9, a) * 0.6);
    let belly = smoothstep(arch + 0.012, arch - 0.012, p.y) * (1.0 - smoothstep(1.4, 2.0, a));
    0.0025 * fade * g * (1.0 - belly) - 0.004 * belly
}

fn torso_field(l: &RagdollLayout) -> impl Fn(Vec3) -> f32 + use<> {
    let (yw, ys, yn) = (l.waist.y, l.shoulder.y, l.neck.y);
    let y0 = yw + 0.045;
    let y1 = ys + 0.005;
    let ax = (l.shoulder.x - 0.012).min(0.2);
    let tube = Tube::new(
        v3(0.0, y0, 0.0),
        v3(0.0, y1, 0.0),
        Vec3::Z,
        &[
            (0.0, [0.135, 0.135, 0.097, 0.098]),
            (0.3, [0.145, 0.145, 0.106, 0.104]),
            (0.6, [0.154, 0.154, 0.118, 0.112]),
            (0.82, [0.158, 0.158, 0.123, 0.115]),
            (1.0, [0.148, 0.148, 0.108, 0.11]),
        ],
    )
    .power(2.3)
    .round(0.022);
    let sh = [
        v3(-(ax - 0.04), ys - 0.004, -0.012),
        v3(ax - 0.04, ys - 0.004, -0.012),
    ];
    let neck_base = v3(0.0, yn - 0.042, -0.03);
    let (rib_lo, rib_hi) = (y0 + 0.035, ys - 0.13);
    let pec = [-1.0f32, 1.0].map(|sx| Quat::from_rotation_z(sx * 0.32).inverse());
    move |p: Vec3| {
        let mut d = tube.eval(p);
        d = smin(d, capsule(p, sh[0], sh[1], 0.045), 0.05);
        for (i, sx) in [-1.0, 1.0].into_iter().enumerate() {
            // Trapezius, pectoral plates, shoulder blades.
            d = smin(
                d,
                round_cone(
                    p,
                    neck_base,
                    v3(sx * (ax - 0.045), ys + 0.01, -0.018),
                    0.04,
                    0.04,
                ),
                0.04,
            );
            let c = v3(sx * 0.07, ys - 0.062, 0.1);
            d = smin(
                d,
                ellipsoid(c + pec[i] * (p - c), c, v3(0.08, 0.04, 0.022)),
                0.03,
            );
            d = smin(
                d,
                ellipsoid(
                    p,
                    v3(sx * 0.075, ys - 0.075, -0.085),
                    v3(0.06, 0.075, 0.035),
                ),
                0.03,
            );
        }
        // Spine and sternum grooves.
        d = smax(
            d,
            -capsule(
                p,
                v3(0.0, y0 + 0.04, -0.128),
                v3(0.0, ys - 0.01, -0.128),
                0.011,
            ),
            0.01,
        );
        d = smax(
            d,
            -capsule(
                p,
                v3(0.0, ys - 0.17, 0.134),
                v3(0.0, ys - 0.035, 0.134),
                0.009,
            ),
            0.012,
        );
        d - ribs(p, rib_lo, rib_hi)
    }
}

fn chest(l: &RagdollLayout, shirt: Vec4) -> DummyMesh {
    let f = torso_field(l);
    let mut part = Part::new();
    let (yw, ys) = (l.waist.y, l.shoulder.y);
    let y0 = yw + 0.045;
    // Fewer rows on the bottom (inside the bellows), more over the ribs.
    let rows = |sg: f32| {
        0.3 + 0.7 * smoothstep(0.08, 0.2, sg) + 0.9 * smoothstep(0.2, 0.3, sg)
            - 0.9 * smoothstep(0.55, 0.65, sg)
    };
    Wrap::new(&[v3(0.0, y0 + 0.03, 0.0), v3(0.0, ys - 0.03, 0.0)], Vec3::Z)
        .hint(0.115, 0.15)
        .size(38, 36)
        .reach(0.4)
        .rows_weight(&rows)
        .skin(&mut part.vinyl, &f, shirt, 1.0);
    for sx in [-1.0, 1.0] {
        target(
            &mut part.vinyl,
            &f,
            v3(sx * 0.2, ys - 0.115, 0.045),
            Vec3::Y,
            0.024,
        );
    }

    // Neck collar (the neck bracket's rubber ring), tilted forward like the base of a neck.
    let tilt = Quat::from_rotation_x(0.4);
    let collar_at = l.neck + v3(0.0, -0.006, 0.002);
    let ring = [
        Vec2::new(0.045, -0.014),
        Vec2::new(0.054, -0.013),
        Vec2::new(0.0585, -0.006),
        Vec2::new(0.0585, 0.003),
        Vec2::new(0.055, 0.008),
        Vec2::new(0.048, 0.009),
        Vec2::new(0.043, 0.005),
        Vec2::new(0.043, -0.01),
        Vec2::new(0.045, -0.014),
    ];
    let m = Mat4::from_rotation_translation(tilt, collar_at);
    part.vinyl.colored(rubber(), |b| {
        b.placed(m, |b| b.lathe(&ring, 24, 50.0));
    });
    for k in 0..4 {
        let a = PI * 0.25 + FRAC_PI_2 * k as f32;
        let p = m.transform_point3(v3(0.0505 * a.cos(), 0.0085, 0.0505 * a.sin()));
        bolt(&mut part.steel, p, tilt * Vec3::Y, 0.0034, false);
    }

    // Shoulders far from the torso (the proving-ground layout) get a black clavicle yoke.
    let ax = (l.shoulder.x - 0.012).min(0.2);
    if l.shoulder.x - ax > 0.03 {
        for sx in [-1.0, 1.0] {
            let s = l.shoulder * v3(sx, 1.0, 1.0);
            let from = v3(sx * (ax - 0.03), s.y + 0.004, -0.008);
            part.vinyl.colored(rubber(), |b| {
                b.placed(frame_y(from, (s - from).normalize()), |b| {
                    let len = s.distance(from);
                    b.lathe(
                        &[
                            Vec2::new(0.0, 0.0),
                            Vec2::new(0.032, 0.0),
                            Vec2::new(0.03, len * 0.5),
                            Vec2::new(0.034, len),
                            Vec2::new(0.0, len),
                        ],
                        14,
                        60.0,
                    )
                });
                let ball: Vec<Vec2> = (0..=8)
                    .map(|i| {
                        let a = PI * (i as f32 / 8.0 - 0.5);
                        Vec2::new(0.042 * a.cos().max(0.0), 0.042 * a.sin())
                    })
                    .collect();
                b.placed(Mat4::from_translation(s), |b| b.lathe(&ball, 14, 60.0));
            });
        }
    }
    part.finish()
}

// ---------------------------------------------------------------------------------------------
// Pelvis and waist bellows
// ---------------------------------------------------------------------------------------------

fn pelvis_field(l: &RagdollLayout) -> impl Fn(Vec3) -> f32 + use<> {
    let yh = l.hip.y;
    let yw = l.waist.y;
    let hem = yh - 0.075;
    // A boxy block of flesh like a pair of shorts, from the hem to the waist.
    let block = Tube::new(
        v3(0.0, hem, -0.004),
        v3(0.0, yw, -0.004),
        Vec3::Z,
        &[
            (0.0, [0.176, 0.176, 0.104, 0.11]),
            (0.35, [0.182, 0.182, 0.104, 0.112]),
            (0.7, [0.165, 0.165, 0.1, 0.104]),
            (1.0, [0.136, 0.136, 0.092, 0.094]),
        ],
    )
    .power(2.6)
    .round(0.014);
    move |p: Vec3| {
        let mut d = block.eval(p);
        for sx in [-1.0, 1.0] {
            // Buttocks.
            d = smin(
                d,
                ellipsoid(p, v3(sx * 0.062, yh + 0.004, -0.058), v3(0.07, 0.08, 0.066)),
                0.03,
            );
        }
        // Lower belly.
        d = smin(
            d,
            ellipsoid(p, v3(0.0, yh + 0.1, 0.03), v3(0.125, 0.07, 0.078)),
            0.04,
        );
        // Flat hem: the thighs leave it at a steep angle, a clean seam.
        d = smax(d, hem - p.y, 0.012);
        smax(d, p.y - yw, 0.03)
    }
}

fn pelvis(l: &RagdollLayout) -> DummyMesh {
    let f = pelvis_field(l);
    let mut part = Part::new();
    let yh = l.hip.y;
    let hem = yh - 0.075;
    // Rings stacked up the block: the hem and the waist ledge each sit on one row, and the
    // flat bottom (with the crotch arch) and top are shot straight along the spine.
    let rows = |sg: f32| 0.55 + 0.9 * smoothstep(0.2, 0.33, sg) - 0.9 * smoothstep(0.67, 0.8, sg);
    Wrap::new(
        &[
            v3(0.0, hem + 0.014, -0.004),
            v3(0.0, l.waist.y - 0.035, -0.004),
        ],
        Vec3::Z,
    )
    .hint(0.1, 0.17)
    .size(34, 28)
    .flat(true, true, 0.05)
    .reach(0.3)
    .rows_weight(&rows)
    .skin(&mut part.vinyl, &f, yellow(), 1.0);

    // H-point hubs: black discs on the hips with a target and a center bolt.
    for sx in [-1.0, 1.0] {
        let side = Vec3::X * sx;
        let on = settle(&f, v3(sx * 0.2, yh, -0.004));
        let center = on - side * 0.002;
        let thick = 0.012;
        part.vinyl.colored(rubber(), |b| {
            b.placed(frame_y(center, side), |b| {
                cap(b, 0.036, thick * 0.5, 0.004, 20)
            })
        });
        let face = center + side * (thick * 0.5);
        target(&mut part.vinyl, &plane(face, side), face, Vec3::Y, 0.029);
        bolt(&mut part.steel, face, side, 0.0065, true);
    }

    // Waist bellows: a corrugated rubber ball around the waist pivot, so the chest can tip
    // over it without opening a gap. Elliptic: wider than deep.
    let r = 0.112;
    let mut profile = vec![Vec2::new(0.098, -0.05), Vec2::new(0.099, -0.012)];
    let ridges = 5;
    let (ya, yb) = (-0.008, 0.083);
    let pitch = (yb - ya) / ridges as f32;
    let ball = |y: f32| (r * r - y * y).max(0.0).sqrt();
    for i in 0..ridges {
        for t in [0.0, 0.3, 0.5, 0.7] {
            let y = ya + pitch * (i as f32 + t);
            let groove = 1.0 - (t * PI).sin().powf(0.5);
            profile.push(Vec2::new(ball(y).min(0.106) * (1.0 - 0.065 * groove), y));
        }
    }
    for k in 0..=4 {
        let a = (yb / r).asin() + (FRAC_PI_2 - (yb / r).asin()) * k as f32 / 4.0;
        profile.push(Vec2::new(r * a.cos() * 0.935, r * a.sin()));
    }
    profile.last_mut().unwrap().x = 0.0;
    let squash = Mat4::from_translation(l.waist) * Mat4::from_scale(v3(1.08, 1.0, 0.86));
    part.vinyl.colored(rubber(), |b| {
        b.placed(squash, |b| b.lathe(&profile, 20, 60.0));
    });
    // Steel clamp band where the bellows leaves the pelvis.
    let band = [
        Vec2::new(0.097, -0.017),
        Vec2::new(0.1035, -0.0155),
        Vec2::new(0.1045, -0.006),
        Vec2::new(0.0975, -0.0035),
    ];
    part.steel.placed(squash, |b| b.lathe(&band, 20, 50.0));
    part.finish()
}

// ---------------------------------------------------------------------------------------------
// Arms
// ---------------------------------------------------------------------------------------------

fn upper_arm(l: &RagdollLayout) -> DummyMesh {
    let (s, e) = (l.shoulder, l.elbow);
    let tube = Tube::new(
        s,
        e,
        Vec3::Z,
        &[
            (0.0, [0.05, 0.05, 0.05, 0.05]),
            (0.18, [0.054, 0.046, 0.05, 0.05]),
            (0.5, [0.046, 0.043, 0.048, 0.049]),
            (0.82, [0.039, 0.038, 0.04, 0.042]),
            (1.0, [0.031, 0.031, 0.035, 0.035]),
        ],
    );
    let f = |p: Vec3| smin(tube.eval(p), sphere(p, s, 0.056), 0.03);
    let mut part = Part::new();
    Wrap::new(&[s, e], Vec3::Z)
        .hint(0.05, 0.05)
        .size(20, 22)
        .open(false, true)
        .reach(0.12)
        .skin(&mut part.vinyl, &f, yellow(), 1.0);
    // Shoulder bolt on the deltoid.
    let on = settle(&f, s + Vec3::X * 0.07);
    bolt(
        &mut part.steel,
        on - Vec3::X * 0.001,
        normal(&f, on),
        0.0085,
        true,
    );
    hinge(&mut part, e, 0.042, 0.045, 0.022, false);
    part.finish()
}

/// The left hand's frame at the wrist: (out = back of the hand, down, front = thumb side).
fn hand_frame(l: &RagdollLayout) -> (Vec3, Vec3, Vec3) {
    let down = (l.wrist - l.elbow).normalize();
    let front = (Vec3::Z - down * Vec3::Z.dot(down)).normalize();
    (front.cross(down), down, front)
}

/// Curled mitten fingers: knuckles, then three phalanges bending toward the palm (−out),
/// in the hand's (out, down) plane.
const FINGERS: [Vec2; 4] = [
    Vec2::new(0.0, 0.088),
    Vec2::new(-0.0087, 0.1291),
    Vec2::new(-0.0254, 0.149),
    Vec2::new(-0.0442, 0.1558),
];

fn hand_field(l: &RagdollLayout) -> impl Fn(Vec3) -> f32 + use<> {
    let (o, dn, fr) = hand_frame(l);
    let w = l.wrist;
    move |p: Vec3| {
        let q = p - w;
        let local = Vec3::new(q.dot(o), q.dot(dn), q.dot(fr));
        let at = |x: f32, y: f32| Vec3::new(x, y, 0.0);
        // Palm: narrow at the wrist, wide at the knuckles.
        let mut d = slab(local, at(0.0, -0.006), at(0.0, 0.03), 0.029, 0.0135);
        d = smin(
            d,
            slab(local, at(0.0, 0.045), at(0.0, 0.088), 0.038, 0.0145),
            0.03,
        );
        // Thumb ball.
        d = smin(
            d,
            ellipsoid(local, v3(-0.011, 0.034, 0.022), v3(0.014, 0.03, 0.02)),
            0.012,
        );
        let widths = [0.037, 0.035, 0.032];
        let thick = [0.0108, 0.0098, 0.0088];
        for i in 0..3 {
            let a = FINGERS[i];
            let b = FINGERS[i + 1];
            d = smin(
                d,
                slab(local, at(a.x, a.y), at(b.x, b.y), widths[i], thick[i]),
                0.012,
            );
        }
        d
    }
}

const THUMB: [Vec3; 4] = [
    v3(-0.004, 0.028, 0.024),
    v3(-0.011, 0.058, 0.044),
    v3(-0.021, 0.081, 0.044),
    v3(-0.029, 0.092, 0.037),
];

fn forearm(l: &RagdollLayout) -> DummyMesh {
    let (e, w) = (l.elbow, l.wrist);
    let tube = Tube::new(
        e,
        w,
        Vec3::Z,
        &[
            (0.0, [0.033, 0.033, 0.036, 0.036]),
            (0.1, [0.037, 0.036, 0.039, 0.04]),
            (0.3, [0.045, 0.039, 0.041, 0.04]),
            (0.7, [0.031, 0.029, 0.033, 0.032]),
            (1.0, [0.0225, 0.0215, 0.03, 0.028]),
        ],
    );
    let f = |p: Vec3| tube.eval(p);
    let mut part = Part::new();
    Wrap::new(&[e, w], Vec3::Z)
        .hint(0.04, 0.04)
        .size(18, 18)
        .open(true, true)
        .reach(0.1)
        .skin(&mut part.vinyl, &f, yellow(), 1.0);

    // Wrist sleeve.
    let (o, dn, fr) = hand_frame(l);
    let band = [
        Vec2::new(0.025, -0.013),
        Vec2::new(0.0305, -0.0105),
        Vec2::new(0.0322, 0.0),
        Vec2::new(0.0305, 0.0105),
        Vec2::new(0.025, 0.013),
    ];
    let basis = Mat4::from_cols(
        (o * 0.8).extend(0.0),
        (-dn).extend(0.0),
        (fr * 1.06).extend(0.0),
        w.extend(1.0),
    );
    part.vinyl.colored(rubber(), |b| {
        b.placed(basis, |b| b.lathe(&band, 18, 60.0));
    });

    // Mitten hand and thumb.
    let hf = hand_field(l);
    let to_feet = |x: f32, y: f32, z: f32| w + o * x + dn * y + fr * z;
    let mut spine = vec![to_feet(0.0, -0.004, 0.0), to_feet(0.0, 0.045, 0.0)];
    for (i, k) in FINGERS.iter().enumerate() {
        let k = if i == 3 { FINGERS[2].lerp(*k, 0.6) } else { *k };
        spine.push(to_feet(k.x, k.y, 0.0));
    }
    Wrap::new(&spine, o)
        .hint(0.016, 0.037)
        .size(20, 24)
        .open(true, false)
        .reach(0.06)
        .skin(&mut part.vinyl, &hf, yellow(), 1.0);
    let thumb_pts = THUMB.map(|t| to_feet(t.x, t.y, t.z));
    let thumb = |p: Vec3| {
        let mut d = round_cone(p, thumb_pts[0], thumb_pts[1], 0.0145, 0.0125);
        d = smin(
            d,
            round_cone(p, thumb_pts[1], thumb_pts[2], 0.0125, 0.0112),
            0.006,
        );
        smin(
            d,
            round_cone(p, thumb_pts[2], thumb_pts[3], 0.0112, 0.0102),
            0.006,
        )
    };
    Wrap::new(&thumb_pts, o)
        .hint(0.012, 0.012)
        .size(12, 11)
        .open(true, false)
        .reach(0.03)
        .skin(&mut part.vinyl, &thumb, yellow(), 0.6);
    part.finish()
}

/// A black hinge sleeve around X at `at` (elbow, knee) with steel hub caps; the outer face
/// (+X) carries a target instead of a cap when `target_out` is set. The limbs it joins stay
/// inside its full-radius band (|x| < `half_len` − 0.2 `radius`), so the seams are clean.
fn hinge(part: &mut Part, at: Vec3, radius: f32, half_len: f32, hub: f32, target_out: bool) {
    part.vinyl.colored(rubber(), |b| {
        b.placed(frame_y(at, Vec3::X), |b| {
            disc(b, radius, half_len, radius * 0.2, 20)
        })
    });
    for side in [-1.0f32, 1.0] {
        let n = Vec3::X * side;
        let face = at + n * half_len;
        if side > 0.0 && target_out {
            target(
                &mut part.vinyl,
                &plane(face, n),
                face,
                Vec3::Y,
                radius * 0.66,
            );
            bolt(&mut part.steel, face, n, hub * 0.25, false);
        } else {
            let r = if side > 0.0 { hub } else { hub * 0.85 };
            part.steel.placed(frame_y(face + n * 0.002, n), |b| {
                cap(b, r, 0.0025, 0.0012, 20)
            });
            bolt(&mut part.steel, face + n * 0.0045, n, r * 0.36, false);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Legs
// ---------------------------------------------------------------------------------------------

fn thigh(l: &RagdollLayout) -> DummyMesh {
    let (hp, k) = (l.hip, l.knee);
    let tube = Tube::new(
        hp,
        k,
        Vec3::Z,
        &[
            (0.0, [0.068, 0.068, 0.068, 0.068]),
            (0.14, [0.07, 0.072, 0.07, 0.076]),
            (0.32, [0.074, 0.072, 0.078, 0.077]),
            (0.5, [0.071, 0.068, 0.074, 0.071]),
            (0.75, [0.06, 0.056, 0.062, 0.058]),
            (0.9, [0.047, 0.045, 0.054, 0.05]),
            (1.0, [0.042, 0.042, 0.046, 0.046]),
        ],
    );
    let f = |p: Vec3| {
        let mut d = smin(tube.eval(p), sphere(p, hp, 0.068), 0.02);
        d = smin(
            d,
            ellipsoid(p, k + v3(-0.02, 0.075, 0.03), v3(0.032, 0.05, 0.03)),
            0.02,
        );
        // Knee block, inside the hinge sleeve.
        smin(d, cylinder_x(p, k, 0.046, 0.04), 0.012)
    };
    let mut part = Part::new();
    Wrap::new(&[hp, k - Vec3::Y * 0.02], Vec3::Z)
        .hint(0.08, 0.08)
        .size(22, 22)
        .reach(0.16)
        .skin(&mut part.vinyl, &f, yellow(), 1.0);
    hinge(&mut part, k, 0.052, 0.058, 0.03, true);
    // Knee cap: a molded vinyl pad on the front of the sleeve, its rim sunk into it.
    let pad: Vec<Vec2> = std::iter::once(Vec2::new(0.031, 0.025))
        .chain((0..=5).map(|i| {
            let a = FRAC_PI_2 * i as f32 / 5.0;
            Vec2::new(
                0.031 * a.cos().max(0.0).powf(0.45),
                0.052 + 0.0115 * a.sin(),
            )
        }))
        .collect();
    let m = Mat4::from_translation(k + Vec3::Y * 0.004)
        * Mat4::from_quat(Quat::from_rotation_arc(Vec3::Y, Vec3::Z))
        * Mat4::from_scale(v3(1.0, 1.0, 1.2));
    part.vinyl
        .colored(yellow(), |b| b.placed(m, |b| b.lathe(&pad, 16, 50.0)));
    part.finish()
}

fn shoe_field(l: &RagdollLayout) -> impl Fn(Vec3) -> f32 + use<> {
    let (fx, fz) = (l.ankle.x, l.ankle.z);
    move |p: Vec3| {
        let mut d = ellipsoid(p, v3(fx, 0.052, fz - 0.025), v3(0.041, 0.048, 0.048));
        d = smin(
            d,
            ellipsoid(p, v3(fx + 0.002, 0.05, fz + 0.055), v3(0.046, 0.044, 0.088)),
            0.03,
        );
        d = smin(
            d,
            ellipsoid(p, v3(fx + 0.003, 0.037, fz + 0.142), v3(0.05, 0.03, 0.078)),
            0.03,
        );
        d = smin(
            d,
            capsule(
                p,
                v3(fx, 0.055, fz - 0.012),
                v3(fx, 0.072, fz - 0.012),
                0.043,
            ),
            0.025,
        );
        smax(d, 0.017 - p.y, 0.005)
    }
}

fn shin(l: &RagdollLayout) -> DummyMesh {
    let (k, a) = (l.knee, l.ankle);
    let tube = Tube::new(
        k,
        a,
        Vec3::Z,
        &[
            (0.0, [0.04, 0.04, 0.043, 0.043]),
            (0.1, [0.045, 0.044, 0.046, 0.052]),
            (0.33, [0.052, 0.05, 0.043, 0.066]),
            (0.62, [0.042, 0.04, 0.036, 0.048]),
            (0.88, [0.033, 0.032, 0.032, 0.036]),
            (1.0, [0.031, 0.03, 0.031, 0.033]),
        ],
    );
    let f = |p: Vec3| smin(tube.eval(p), cylinder_x(p, k, 0.044, 0.04), 0.01);
    let mut part = Part::new();
    Wrap::new(&[k, a], Vec3::Z)
        .hint(0.05, 0.05)
        .size(20, 22)
        .open(false, true)
        .reach(0.14)
        .skin(&mut part.vinyl, &f, yellow(), 1.0);

    // Shoe: upper, sole, padded collar, laces.
    let sf = shoe_field(l);
    let (fx, fz) = (a.x, a.z);
    Wrap::new(
        &[
            v3(fx, 0.048, fz - 0.035),
            v3(fx, 0.047, fz + 0.04),
            v3(fx, 0.04, fz + 0.15),
        ],
        Vec3::Y,
    )
    .hint(0.045, 0.045)
    .size(20, 20)
    .reach(0.15)
    .skin(&mut part.vinyl, &sf, shoe_black(), 0.8);
    let center = Vec2::new(fx, fz + 0.07);
    let mut outline = Vec::new();
    for i in 0..40 {
        let ang = TAU * i as f32 / 40.0;
        let dir = Vec2::new(ang.cos(), ang.sin());
        let mut t = 0.0;
        while t < 0.3 && sf(v3(center.x + dir.x * t, 0.021, center.y + dir.y * t)) < 0.0 {
            t += 0.0005;
        }
        let p = center + dir * (t + 0.0045);
        // Kit outline (x, y) → world (x, −z) after the −90° turn about X.
        outline.push(Vec2::new(p.x - center.x, -(p.y - center.y)));
    }
    let sole = Mat4::from_rotation_translation(
        Quat::from_rotation_x(-FRAC_PI_2),
        v3(center.x, 0.0105, center.y),
    );
    part.vinyl.colored(sole_gray(), |b| {
        b.placed(sole, |b| b.extrude(&outline, 0.021, 0.0035, 35.0))
    });
    let cuff = [
        Vec2::new(0.0305, 0.104),
        Vec2::new(0.042, 0.101),
        Vec2::new(0.0465, 0.11),
        Vec2::new(0.041, 0.119),
        Vec2::new(0.031, 0.117),
    ];
    part.vinyl.colored(shoe_black(), |b| {
        b.placed(Mat4::from_translation(v3(fx, 0.0, fz - 0.012)), |b| {
            b.lathe(&cuff, 18, 60.0)
        })
    });
    part.vinyl.colored(lace_gray(), |b| {
        for i in 0..4 {
            let z = fz + 0.028 + 0.019 * i as f32;
            let ends = [-1.0, 1.0].map(|s: f32| {
                let p = settle(&sf, v3(fx + s * 0.017, 0.12, z));
                p + normal(&sf, p) * 0.0018
            });
            let mid = settle(&sf, v3(fx, 0.12, z));
            let mid = mid + normal(&sf, mid) * 0.0024;
            b.tube(&[ends[0], mid, ends[1]], 0.0026, 6, false, true);
        }
    });
    part.finish()
}

// =============================================================================================
// Hardware: bolts, hub discs, targets
// =============================================================================================

/// A transform taking +Y to `axis`, origin at `at`.
fn frame_y(at: Vec3, axis: Vec3) -> Mat4 {
    Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Y, axis.normalize()), at)
}

/// A hex bolt head (on a washer when `washer` is set and it is not tiny), seated on a
/// surface at `at` facing `axis`; `r` is the hex's corner radius.
fn bolt(b: &mut MeshBuilder, at: Vec3, axis: Vec3, r: f32, washer: bool) {
    let axis = axis.normalize();
    let washer = if !washer || r < 0.005 { 0.0 } else { r * 0.28 };
    if washer > 0.0 {
        b.placed(frame_y(at, axis), |b| {
            b.lathe(
                &[
                    Vec2::new(r * 1.5, -washer),
                    Vec2::new(r * 1.5, washer * 0.5),
                    Vec2::new(r * 1.3, washer),
                    Vec2::new(r * 0.5, washer),
                ],
                10,
                40.0,
            )
        });
    }
    let hex: Vec<Vec2> = (0..6)
        .map(|i| {
            let a = TAU * i as f32 / 6.0;
            Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    let h = r * 0.7;
    let m = Mat4::from_rotation_translation(
        Quat::from_rotation_arc(Vec3::Z, axis),
        at + axis * (washer + h * 0.5),
    );
    b.placed(m, |b| b.extrude(&hex, h, r * 0.12, 20.0));
}

/// A cap along +Y seated on a surface (no bottom face): `r` radius, `half` half thickness,
/// top edge chamfered by `bevel`.
fn cap(b: &mut MeshBuilder, r: f32, half: f32, bevel: f32, sides: u32) {
    let e = bevel.min(half * 0.95).min(r * 0.5);
    b.lathe(
        &[
            Vec2::new(r, -half),
            Vec2::new(r, half - e),
            Vec2::new(r - e, half),
            Vec2::new(0.0, half),
        ],
        sides,
        40.0,
    );
}

/// A disc along +Y centered at the origin: `r` radius, `half` half thickness, edges rounded
/// by `bevel` (two steps, smooth).
fn disc(b: &mut MeshBuilder, r: f32, half: f32, bevel: f32, sides: u32) {
    let e = bevel.min(half * 0.95).min(r * 0.5);
    let k = 0.3 * e;
    b.lathe(
        &[
            Vec2::new(0.0, -half),
            Vec2::new(r - e, -half),
            Vec2::new(r - k, -half + k),
            Vec2::new(r, -half + e),
            Vec2::new(r, half - e),
            Vec2::new(r - k, half - k),
            Vec2::new(r - e, half),
            Vec2::new(0.0, half),
        ],
        sides,
        40.0,
    );
}

/// A black-and-yellow quadrant calibration target printed on the surface of `f` near
/// `guess` (a decal lifted 0.7 mm off it), quadrant edges along `up`.
fn target(b: &mut MeshBuilder, f: &dyn Fn(Vec3) -> f32, guess: Vec3, up: Vec3, radius: f32) {
    let p0 = settle(f, guess);
    let n0 = normal(f, p0);
    let tu = (up - n0 * up.dot(n0)).normalize();
    let ts = n0.cross(tu);
    let at = |a: f32, rho: f32| -> (Vec3, Vec3, Vec2) {
        let off = ts * a.cos() + tu * a.sin();
        let s = settle(f, p0 + off * rho);
        let n = normal(f, s);
        (s + n * 0.0007, n, Vec2::new(a.cos(), a.sin()) * rho)
    };
    let steps = 6;
    let old = b.color;
    for k in 0..4 {
        b.color = if k % 2 == 0 { rubber() } else { yellow() };
        let a0 = FRAC_PI_2 * k as f32;
        let radii = [0.0, 0.45 * radius, 0.82 * radius];
        b.grid(radii.len(), steps + 1, |r, c| {
            at(a0 + FRAC_PI_2 * c as f32 / steps as f32, radii[r])
        });
    }
    b.color = rubber();
    let ring = [0.82 * radius, radius];
    b.grid(2, 4 * steps + 1, |r, c| {
        at(TAU * c as f32 / (4 * steps) as f32, ring[r])
    });
    b.color = old;
}

/// A plane through `at` facing `n`, as a field.
fn plane(at: Vec3, n: Vec3) -> impl Fn(Vec3) -> f32 {
    move |p: Vec3| (p - at).dot(n)
}

// =============================================================================================
// Implicit shapes
// =============================================================================================

/// Polynomial smooth minimum (union with a fillet about `k` wide).
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// Smooth maximum (intersection, or subtraction with a negated shape).
fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

fn sphere(p: Vec3, c: Vec3, r: f32) -> f32 {
    p.distance(c) - r
}

/// Approximate distance to an axis-aligned ellipsoid (exact on the surface).
fn ellipsoid(p: Vec3, c: Vec3, r: Vec3) -> f32 {
    let q = p - c;
    let k1 = (q / (r * r)).length();
    if k1 < 1e-9 {
        return -r.min_element();
    }
    let k0 = (q / r).length();
    k0 * (k0 - 1.0) / k1
}

/// A cylinder along X centered at `c`: radius `r`, half length `h`, flat ends.
fn cylinder_x(p: Vec3, c: Vec3, r: f32, h: f32) -> f32 {
    let q = p - c;
    let d = Vec2::new(Vec2::new(q.y, q.z).length() - r, q.x.abs() - h);
    d.max(Vec2::ZERO).length() + d.max_element().min(0.0)
}

fn capsule(p: Vec3, a: Vec3, b: Vec3, r: f32) -> f32 {
    let (pa, ba) = (p - a, b - a);
    let h = (pa.dot(ba) / ba.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (pa - ba * h).length() - r
}

/// A capsule whose radius goes from `r1` at `a` to `r2` at `b` (exact distance).
fn round_cone(p: Vec3, a: Vec3, b: Vec3, r1: f32, r2: f32) -> f32 {
    let ba = b - a;
    let l2 = ba.dot(ba);
    let rr = r1 - r2;
    let a2 = l2 - rr * rr;
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = pa.dot(ba);
    let z = y - l2;
    let x2 = (pa * l2 - ba * y).length_squared();
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = rr.signum() * rr * rr * x2;
    if z.signum() * a2 * z2 > k {
        return (x2 + z2).sqrt() * il2 - r2;
    }
    if y.signum() * a2 * y2 < k {
        return (x2 + y2).sqrt() * il2 - r1;
    }
    ((x2 * a2 * il2).sqrt() + y * rr) * il2 - r1
}

/// A rounded slab in hand-local space: the segment `a`–`b` (in the local XY plane) swept
/// `half_w` across local Z, inflated by `r`.
fn slab(p: Vec3, a: Vec3, b: Vec3, half_w: f32, r: f32) -> f32 {
    let flat = Vec3::new(p.x, p.y, 0.0);
    let rho = capsule(flat, a, b, 0.0);
    let across = (p.z.abs() - (half_w - r)).max(0.0);
    (rho * rho + across * across).sqrt() - r
}

/// A tube from `a` to `b` whose cross-section is an (asymmetric) superellipse with radii
/// keyed along its length: `[out, in, front, back]`, out = front × axis.
struct Tube {
    a: Vec3,
    axis: Vec3,
    len: f32,
    out: Vec3,
    front: Vec3,
    keys: Vec<(f32, [f32; 4])>,
    power: f32,
    round: f32,
}

impl Tube {
    fn new(a: Vec3, b: Vec3, front: Vec3, keys: &[(f32, [f32; 4])]) -> Self {
        let len = a.distance(b);
        let axis = (b - a) / len;
        let front = (front - axis * front.dot(axis)).normalize();
        Self {
            a,
            axis,
            len,
            out: front.cross(axis),
            front,
            keys: keys.to_vec(),
            power: 2.0,
            round: 0.0,
        }
    }

    /// Superellipse exponent of the cross-section (2 = ellipse, larger = boxier).
    fn power(mut self, n: f32) -> Self {
        self.power = n;
        self
    }

    /// Round the flat ends' edges by `r`.
    fn round(mut self, r: f32) -> Self {
        self.round = r;
        self
    }

    fn eval(&self, p: Vec3) -> f32 {
        let q = p - self.a;
        let h = q.dot(self.axis);
        let r = keyed(&self.keys, (h / self.len).clamp(0.0, 1.0));
        let x = q.dot(self.out);
        let z = q.dot(self.front);
        let rx = (if x >= 0.0 { r[0] } else { r[1] }) - self.round;
        let rz = (if z >= 0.0 { r[2] } else { r[3] }) - self.round;
        let planar = (x * x + z * z).sqrt();
        let (u, w) = ((x / rx).abs(), (z / rz).abs());
        let rho = if self.power == 2.0 {
            (u * u + w * w).sqrt()
        } else {
            (u.powf(self.power) + w.powf(self.power)).powf(1.0 / self.power)
        };
        let radial = if rho > 1e-6 {
            planar * (1.0 - 1.0 / rho)
        } else {
            -rx.min(rz)
        };
        let e = (-h).max(h - self.len) + self.round;
        let d = if e > 0.0 && radial > 0.0 {
            (e * e + radial * radial).sqrt()
        } else {
            e.max(radial)
        };
        d - self.round
    }
}

/// Cubic Hermite interpolation through keyed values (Catmull-Rom slopes).
fn keyed(keys: &[(f32, [f32; 4])], t: f32) -> [f32; 4] {
    let n = keys.len();
    if t <= keys[0].0 {
        return keys[0].1;
    }
    if t >= keys[n - 1].0 {
        return keys[n - 1].1;
    }
    let i = keys.iter().position(|k| k.0 > t).unwrap_or(n - 1) - 1;
    let slope = |j: usize, c: usize| {
        let (a, b) = (j.saturating_sub(1), (j + 1).min(n - 1));
        (keys[b].1[c] - keys[a].1[c]) / (keys[b].0 - keys[a].0)
    };
    let (t0, t1) = (keys[i].0, keys[i + 1].0);
    let h = t1 - t0;
    let s = (t - t0) / h;
    let (s2, s3) = (s * s, s * s * s);
    std::array::from_fn(|c| {
        (2.0 * s3 - 3.0 * s2 + 1.0) * keys[i].1[c]
            + (s3 - 2.0 * s2 + s) * h * slope(i, c)
            + (-2.0 * s3 + 3.0 * s2) * keys[i + 1].1[c]
            + (s3 - s2) * h * slope(i + 1, c)
    })
}

fn gradient(f: &dyn Fn(Vec3) -> f32, p: Vec3) -> Vec3 {
    const E: f32 = 1.5e-4;
    Vec3::new(
        f(p + Vec3::X * E) - f(p - Vec3::X * E),
        f(p + Vec3::Y * E) - f(p - Vec3::Y * E),
        f(p + Vec3::Z * E) - f(p - Vec3::Z * E),
    ) / (2.0 * E)
}

fn normal(f: &dyn Fn(Vec3) -> f32, p: Vec3) -> Vec3 {
    gradient(f, p).normalize_or(Vec3::Y)
}

/// Move `p` onto the surface of `f` (Newton steps along the gradient).
fn settle(f: &dyn Fn(Vec3) -> f32, mut p: Vec3) -> Vec3 {
    for _ in 0..16 {
        let v = f(p);
        if v.abs() < 1e-7 {
            break;
        }
        let g = gradient(f, p);
        let g2 = g.length_squared();
        if g2 < 1e-12 {
            break;
        }
        p -= g * (v / g2).clamp(-0.02, 0.02);
    }
    p
}

/// Distance along the ray from `from` (inside the shape) in `dir` to where it leaves the shape
/// for the last time within `reach`: the outermost crossing.
fn shoot(f: &dyn Fn(Vec3) -> f32, from: Vec3, dir: Vec3, reach: f32) -> f32 {
    let mut t = reach;
    let mut v = f(from + dir * t);
    if v <= 0.0 {
        return reach;
    }
    loop {
        let tn = t - (v * 0.7).clamp(0.0006, 0.015);
        if tn <= 0.0 {
            return 0.0;
        }
        let vn = f(from + dir * tn);
        if vn <= 0.0 {
            let (mut lo, mut hi) = (tn, t);
            for _ in 0..14 {
                let mid = 0.5 * (lo + hi);
                if f(from + dir * mid) <= 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return 0.5 * (lo + hi);
        }
        t = tn;
        v = vn;
    }
}

/// Ambient occlusion from the shape itself (how much of it is near above the surface).
fn occlusion(f: &dyn Fn(Vec3) -> f32, p: Vec3, n: Vec3) -> f32 {
    let mut occ = 0.0;
    let mut w = 1.0;
    for i in 1..=5 {
        let h = 0.005 * i as f32;
        occ += (h - f(p + n * h)).max(0.0) * w;
        w *= 0.7;
    }
    (1.0 - 13.0 * occ).clamp(0.45, 1.0)
}

/// Angular distance between two angles, radians.
fn ang(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

// =============================================================================================
// Wrap: rings shot from a spine onto an implicit shape
// =============================================================================================

/// A smooth curve through control points, with arc length and rotation-minimizing frames.
struct Spine {
    pts: Vec<Vec3>,
    tan: Vec<Vec3>,
    nrm: Vec<Vec3>,
    cum: Vec<f32>,
    length: f32,
}

impl Spine {
    fn new(ctrl: &[Vec3], front: Vec3) -> Self {
        let n = ctrl.len();
        let get = |i: isize| -> Vec3 {
            if i < 0 {
                2.0 * ctrl[0] - ctrl[1]
            } else if i as usize >= n {
                2.0 * ctrl[n - 1] - ctrl[n - 2]
            } else {
                ctrl[i as usize]
            }
        };
        let sub = 40;
        let mut pts = Vec::new();
        for i in 0..n as isize - 1 {
            let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
            for k in 0..sub {
                let s = k as f32 / sub as f32;
                let (s2, s3) = (s * s, s * s * s);
                pts.push(
                    0.5 * (2.0 * p1
                        + (p2 - p0) * s
                        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * s2
                        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * s3),
                );
            }
        }
        pts.push(ctrl[n - 1]);
        let m = pts.len();
        let tan: Vec<Vec3> = (0..m)
            .map(|i| (pts[(i + 1).min(m - 1)] - pts[i.saturating_sub(1)]).normalize_or(Vec3::Y))
            .collect();
        let mut nrm = Vec::with_capacity(m);
        let mut nv =
            (front - tan[0] * front.dot(tan[0])).normalize_or(tan[0].any_orthonormal_vector());
        for i in 0..m {
            if i > 0 {
                nv = Quat::from_rotation_arc(tan[i - 1], tan[i]) * nv;
                nv = (nv - tan[i] * nv.dot(tan[i])).normalize_or(nv);
            }
            nrm.push(nv);
        }
        let mut cum = vec![0.0];
        for i in 1..m {
            cum.push(cum[i - 1] + pts[i].distance(pts[i - 1]));
        }
        let length = cum[m - 1];
        Self {
            pts,
            tan,
            nrm,
            cum,
            length,
        }
    }

    /// Position, tangent and normal at arc fraction `s` (0..1).
    fn at(&self, s: f32) -> (Vec3, Vec3, Vec3) {
        let target = s.clamp(0.0, 1.0) * self.length;
        let i = self
            .cum
            .partition_point(|&c| c < target)
            .clamp(1, self.pts.len() - 1);
        let seg = (self.cum[i] - self.cum[i - 1]).max(1e-9);
        let t = ((target - self.cum[i - 1]) / seg).clamp(0.0, 1.0);
        let tan = self.tan[i - 1].lerp(self.tan[i], t).normalize();
        let nrm = self.nrm[i - 1].lerp(self.nrm[i], t);
        let nrm = (nrm - tan * nrm.dot(tan)).normalize();
        (self.pts[i - 1].lerp(self.pts[i], t), tan, nrm)
    }
}

/// `n + 1` values from `a` to `b`, spaced inversely to `weight(fraction)`.
fn distribute(n: usize, a: f32, b: f32, weight: &dyn Fn(f32) -> f32) -> Vec<f32> {
    const K: usize = 600;
    let mut cdf = vec![0.0f32; K + 1];
    for k in 1..=K {
        let x = (k as f32 - 0.5) / K as f32;
        cdf[k] = cdf[k - 1] + weight(x).max(1e-3);
    }
    let total = cdf[K];
    (0..=n)
        .map(|i| {
            let goal = total * i as f32 / n.max(1) as f32;
            let k = cdf.partition_point(|&c| c < goal).clamp(1, K);
            let f = ((goal - cdf[k - 1]) / (cdf[k] - cdf[k - 1]).max(1e-9)).clamp(0.0, 1.0);
            a + (b - a) * (k as f32 - 1.0 + f) / K as f32
        })
        .collect()
}

/// A tube of rings around a spine (closed by caps at either end, or open), whose vertices
/// are shot out from the spine onto an implicit shape.
struct Wrap<'a> {
    spine: Vec<Vec3>,
    front: Vec3,
    hint: Vec2,
    around: usize,
    rows: usize,
    open: [bool; 2],
    flat: [bool; 2],
    depth: f32,
    reach: f32,
    cols_w: Option<&'a dyn Fn(f32) -> f32>,
    rows_w: Option<&'a dyn Fn(f32) -> f32>,
}

impl<'a> Wrap<'a> {
    /// Around the spine through `spine` (at least two points); column 0 points `front`.
    fn new(spine: &[Vec3], front: Vec3) -> Self {
        Self {
            spine: spine.to_vec(),
            front,
            hint: Vec2::splat(0.05),
            around: 24,
            rows: 24,
            open: [false; 2],
            flat: [false; 2],
            depth: 0.05,
            reach: 0.2,
            cols_w: None,
            rows_w: None,
        }
    }

    /// Typical cross-section semi-axes (along `front`, across): columns are spaced evenly
    /// around an ellipse of this shape, and the end caps are sized from it.
    fn hint(mut self, along_front: f32, across: f32) -> Self {
        self.hint = Vec2::new(along_front, across);
        self
    }

    fn size(mut self, around: usize, rows: usize) -> Self {
        self.around = around;
        self.rows = rows;
        self
    }

    /// Leave the start and/or end open (hidden inside another piece).
    fn open(mut self, start: bool, end: bool) -> Self {
        self.open = [start, end];
        self
    }

    /// Flat caps: the start and/or end cap is shot along the spine (from `depth` inside)
    /// instead of around the pole, so a flat end face keeps its rim on one row.
    fn flat(mut self, start: bool, end: bool, depth: f32) -> Self {
        self.flat = [start, end];
        self.depth = depth;
        self
    }

    /// How far from the spine the surface can be.
    fn reach(mut self, r: f32) -> Self {
        self.reach = r;
        self
    }

    /// Column density by angle from `front` (radians).
    fn cols_weight(mut self, w: &'a dyn Fn(f32) -> f32) -> Self {
        self.cols_w = Some(w);
        self
    }

    /// Row density by position from the start pole (0) to the end pole (1).
    fn rows_weight(mut self, w: &'a dyn Fn(f32) -> f32) -> Self {
        self.rows_w = Some(w);
        self
    }

    /// Shoot the wrap onto `f` and add it to `b` in `color`, darkened in crevices by
    /// `ao` (0 = none, 1 = full).
    fn skin(&self, b: &mut MeshBuilder, f: &dyn Fn(Vec3) -> f32, color: Vec4, ao: f32) {
        let spine = Spine::new(&self.spine, self.front);
        let (h, n) = (self.hint, self.around);
        // Columns evenly spaced around the hint ellipse (times the column weight).
        let point = |phi: f32| Vec2::new(h.x * phi.cos(), h.y * phi.sin());
        let theta_of = |phi: f32| {
            let q = point(phi);
            q.y.atan2(q.x).rem_euclid(TAU)
        };
        const K: usize = 720;
        let mut cdf = vec![0.0f32; K + 1];
        let mut arc = vec![0.0f32; K + 1];
        for k in 1..=K {
            let (p0, p1) = (TAU * (k - 1) as f32 / K as f32, TAU * k as f32 / K as f32);
            let ds = point(p1).distance(point(p0));
            let w = self.cols_w.map_or(1.0, |w| w(theta_of(0.5 * (p0 + p1))));
            cdf[k] = cdf[k - 1] + ds * w;
            arc[k] = arc[k - 1] + ds;
        }
        let cols: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                if i == 0 {
                    return (0.0, 0.0);
                }
                if i == n {
                    return (TAU, arc[K]);
                }
                let goal = cdf[K] * i as f32 / n as f32;
                let k = cdf.partition_point(|&c| c < goal).clamp(1, K);
                let t = ((goal - cdf[k - 1]) / (cdf[k] - cdf[k - 1]).max(1e-9)).clamp(0.0, 1.0);
                let phi = TAU * (k as f32 - 1.0 + t) / K as f32;
                (theta_of(phi), arc[k - 1] + (arc[k] - arc[k - 1]) * t)
            })
            .collect();
        // Rows from pole to pole: start cap, along the spine, end cap.
        let rc = 0.5 * (h.x + h.y);
        let (l0, lm) = (FRAC_PI_2 * rc, spine.length);
        let a0 = if self.open[0] { l0 } else { 0.0 };
        let a1 = if self.open[1] { l0 + lm } else { l0 + lm + l0 };
        let one: &dyn Fn(f32) -> f32 = &|_| 1.0;
        let arcs = distribute(self.rows - 1, a0, a1, self.rows_w.unwrap_or(one));
        let base = b.mesh.vertices.len() as u32;
        let width = cols.len();
        let mut along = vec![0.0f32; width];
        let mut prev: Vec<Vec3> = Vec::new();
        let old = b.color;
        for &a in &arcs {
            let cap = if a < l0 {
                Some(0)
            } else if a > l0 + lm {
                Some(1)
            } else {
                None
            };
            let (core, tan, nrm, phi) = if a < l0 {
                let (p, t, nr) = spine.at(0.0);
                (p, t, nr, -FRAC_PI_2 + a / l0 * FRAC_PI_2)
            } else if a <= l0 + lm {
                let (p, t, nr) = spine.at((a - l0) / lm.max(1e-9));
                (p, t, nr, 0.0)
            } else {
                let (p, t, nr) = spine.at(1.0);
                (p, t, nr, (a - l0 - lm) / l0 * FRAC_PI_2)
            };
            let bin = tan.cross(nrm);
            let mut row = Vec::with_capacity(width);
            for (ci, &(th, u)) in cols.iter().enumerate() {
                let radial = nrm * th.cos() + bin * th.sin();
                let p = match cap {
                    Some(e) if self.flat[e] => {
                        // Rows at shrinking fractions of the rim, denser toward it.
                        let s = if e == 0 {
                            a / l0
                        } else {
                            (l0 + lm + l0 - a) / l0
                        };
                        let s = (1.0 - (1.0 - s).powf(1.5)).min(0.985);
                        let rim = core + radial * shoot(f, core, radial, self.reach);
                        let dir = if e == 0 { -tan } else { tan };
                        let from = core.lerp(rim, s) - dir * self.depth;
                        from + dir * shoot(f, from, dir, self.depth * 2.0)
                    }
                    _ => {
                        let dir = (radial * phi.cos() + tan * phi.sin()).normalize();
                        core + dir * shoot(f, core, dir, self.reach)
                    }
                };
                let nn = normal(f, p);
                let shade = 1.0 + (occlusion(f, p, nn) - 1.0) * ao;
                if let Some(q) = prev.get(ci) {
                    along[ci] += p.distance(*q);
                }
                b.color = (color.truncate() * shade).extend(color.w);
                b.vertex(p, nn, Vec2::new(u, along[ci]));
                row.push(p);
            }
            prev = row;
        }
        b.color = old;
        let at = |r: usize, c: usize| base + (r * width + c) as u32;
        for r in 0..arcs.len() - 1 {
            for c in 0..width - 1 {
                b.quad(at(r, c), at(r, c + 1), at(r + 1, c + 1), at(r + 1, c));
            }
        }
    }
}

/// Mirror a mesh across the YZ plane (left part → right part), keeping triangles front-facing.
fn mirror_x(m: &Mesh) -> Mesh {
    let mut out = m.clone();
    for v in &mut out.vertices {
        v.position[0] = -v.position[0];
        v.normal[0] = -v.normal[0];
    }
    for t in out.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    out
}

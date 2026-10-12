//! Sculpted bodies for the demo characters: a display mannequin and a dog.
//!
//! Each body is an implicit sculpt (see [`super::sculpt`]) laid out along its skeleton's bind
//! pose: smooth unions of muscles and bones as ellipsoids and round cones, carved for sockets,
//! grooves and creases. [`build`] meshes it into one seamless surface and gives every vertex
//! up to four bone weights (the sculpt says which bone each vertex belongs to; diffusing
//! those labels over the surface makes smooth joints without bleeding across gaps such as
//! between the thighs). [`super::skin`] deforms it every frame.

use engine_core::prelude::*;
use engine_world::animation::BonePose;

use super::sculpt::{Loft, MeshSettings, Sculpt, SculptMesh, vertex_neighbors};

/// Rotation turning −Y (a bone's length axis) toward `to - from`.
pub fn aim(from: Vec3, to: Vec3) -> Quat {
    let d = to - from;
    if d.length_squared() < 1e-10 {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(Vec3::NEG_Y, d.normalize())
    }
}

/// `q` mirrored across the YZ plane (the other side of the body).
pub fn mirror(q: Quat, side: f32) -> Quat {
    if side < 0.0 {
        Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
    } else {
        q
    }
}

// ---------------------------------------------------------------------------------------
// Skeleton dimensions (model space, meters; Y up, facing −Z, the right side is +X)

/// The mannequin's skeleton: bind joint positions and limb lengths.
pub struct Human;

impl Human {
    pub const PELVIS: Vec3 = Vec3::new(0.0, 0.985, 0.0);
    pub const SPINE: Vec3 = Vec3::new(0.0, 1.125, 0.006);
    pub const CHEST: Vec3 = Vec3::new(0.0, 1.305, 0.0);
    pub const NECK: Vec3 = Vec3::new(0.0, 1.5, 0.014);
    pub const HEAD: Vec3 = Vec3::new(0.0, 1.615, 0.004);
    /// Right shoulder joint (the left mirrors x).
    pub const SHOULDER: Vec3 = Vec3::new(0.18, 1.45, 0.012);
    pub const UPPER_ARM: f32 = 0.295;
    pub const FOREARM: f32 = 0.255;
    /// Right hip joint.
    pub const HIP: Vec3 = Vec3::new(0.088, 0.93, 0.0);
    pub const THIGH: f32 = 0.44;
    pub const SHIN: f32 = 0.415;
    /// Ankle joint above the sole, and the heel and the ball of the foot from the ankle.
    pub const ANKLE_HEIGHT: f32 = 0.075;
    pub const HEEL_BACK: f32 = 0.05;
    pub const BALL_FORWARD: f32 = 0.135;
    /// How far the bind pose spreads the arms from vertical, radians.
    pub const BIND_ARM_SPREAD: f32 = 0.6;
    /// Bind pose spread of the legs from vertical, radians.
    pub const BIND_LEG_SPREAD: f32 = 0.035;

    pub const PELVIS_I: usize = 0;
    pub const SPINE_I: usize = 1;
    pub const CHEST_I: usize = 2;
    pub const NECK_I: usize = 3;
    pub const HEAD_I: usize = 4;
    /// Upper arm, forearm, hand of the left (−X) and right side.
    pub const ARM_L: usize = 5;
    pub const ARM_R: usize = 8;
    /// Thigh, shin, foot.
    pub const LEG_L: usize = 11;
    pub const LEG_R: usize = 14;
    pub const BONES: usize = 17;
}

/// The dog's skeleton.
pub struct Dog;

impl Dog {
    pub const HIPS: Vec3 = Vec3::new(0.0, 0.56, 0.24);
    pub const SPINE: Vec3 = Vec3::new(0.0, 0.575, 0.0);
    pub const CHEST: Vec3 = Vec3::new(0.0, 0.56, -0.22);
    pub const NECK: Vec3 = Vec3::new(0.0, 0.635, -0.355);
    pub const HEAD: Vec3 = Vec3::new(0.0, 0.775, -0.465);
    pub const TAIL: [Vec3; 4] = [
        Vec3::new(0.0, 0.585, 0.355),
        Vec3::new(0.0, 0.56, 0.5),
        Vec3::new(0.0, 0.5, 0.625),
        Vec3::new(0.0, 0.43, 0.725),
    ];
    /// Right front leg: shoulder, elbow, wrist, paw (the left mirrors x).
    pub const FRONT: [Vec3; 4] = [
        Vec3::new(0.085, 0.465, -0.3),
        Vec3::new(0.08, 0.315, -0.215),
        Vec3::new(0.075, 0.085, -0.255),
        Vec3::new(0.075, 0.022, -0.278),
    ];
    /// Right hind leg: hip, stifle, hock, paw.
    pub const HIND: [Vec3; 4] = [
        Vec3::new(0.08, 0.5, 0.265),
        Vec3::new(0.09, 0.33, 0.185),
        Vec3::new(0.08, 0.125, 0.33),
        Vec3::new(0.08, 0.022, 0.302),
    ];
    /// Paw contact point above the ground.
    pub const PAW_HEIGHT: f32 = 0.022;

    pub const HIPS_I: usize = 0;
    pub const SPINE_I: usize = 1;
    pub const CHEST_I: usize = 2;
    pub const NECK_I: usize = 3;
    pub const HEAD_I: usize = 4;
    pub const TAIL_I: usize = 5;
    /// First bone (upper leg) of each leg; then the lower leg, the pastern and the paw.
    pub const LEG_FL: usize = 8;
    pub const LEG_FR: usize = 12;
    pub const LEG_HL: usize = 16;
    pub const LEG_HR: usize = 20;
    pub const BONES: usize = 24;
}

/// `p` on the given side of the body (`side` = −1 left, +1 right; `p` is on the right).
pub fn side_v(p: Vec3, side: f32) -> Vec3 {
    Vec3::new(p.x * side, p.y, p.z)
}

fn pose(t: Vec3, r: Quat) -> BonePose {
    BonePose {
        translation: t,
        rotation: r,
        scale: Vec3::ONE,
    }
}

/// The mannequin's bind pose (arms spread, legs straight), model space.
pub fn human_bind() -> Vec<BonePose> {
    let mut b = vec![BonePose::default(); Human::BONES];
    b[Human::PELVIS_I] = pose(Human::PELVIS, Quat::IDENTITY);
    b[Human::SPINE_I] = pose(Human::SPINE, Quat::IDENTITY);
    b[Human::CHEST_I] = pose(Human::CHEST, Quat::IDENTITY);
    b[Human::NECK_I] = pose(Human::NECK, Quat::IDENTITY);
    b[Human::HEAD_I] = pose(Human::HEAD, Quat::IDENTITY);
    for (side, arm, leg) in [
        (-1.0f32, Human::ARM_L, Human::LEG_L),
        (1.0, Human::ARM_R, Human::LEG_R),
    ] {
        let (s, e, w) = human_bind_arm(side);
        b[arm] = pose(s, aim(s, e));
        b[arm + 1] = pose(e, aim(e, w));
        b[arm + 2] = pose(w, aim(e, w));
        let (h, k, a) = human_bind_leg(side);
        b[leg] = pose(h, aim(h, k));
        b[leg + 1] = pose(k, aim(k, a));
        b[leg + 2] = pose(a, Quat::IDENTITY);
    }
    b
}

/// Shoulder, elbow and wrist of one arm in the bind pose.
pub fn human_bind_arm(side: f32) -> (Vec3, Vec3, Vec3) {
    let s = side_v(Human::SHOULDER, side);
    let a = Human::BIND_ARM_SPREAD;
    let up = Vec3::new(side * a.sin(), -a.cos(), -0.03).normalize();
    let e = s + up * Human::UPPER_ARM;
    let fore = (up + Vec3::new(0.0, 0.0, -0.2)).normalize();
    (s, e, e + fore * Human::FOREARM)
}

/// Hip, knee and ankle of one leg in the bind pose.
pub fn human_bind_leg(side: f32) -> (Vec3, Vec3, Vec3) {
    let h = side_v(Human::HIP, side);
    let a = Human::BIND_LEG_SPREAD;
    let dir = Vec3::new(side * a.sin(), -a.cos(), 0.0);
    let k = h + dir * Human::THIGH;
    (h, k, k + dir * Human::SHIN)
}

/// The dog's bind pose (standing square), model space.
pub fn dog_bind() -> Vec<BonePose> {
    let mut b = vec![BonePose::default(); Dog::BONES];
    b[Dog::HIPS_I] = pose(Dog::HIPS, Quat::IDENTITY);
    b[Dog::SPINE_I] = pose(Dog::SPINE, Quat::IDENTITY);
    b[Dog::CHEST_I] = pose(Dog::CHEST, Quat::IDENTITY);
    b[Dog::NECK_I] = pose(Dog::NECK, Quat::IDENTITY);
    b[Dog::HEAD_I] = pose(Dog::HEAD, Quat::IDENTITY);
    for k in 0..3 {
        b[Dog::TAIL_I + k] = pose(Dog::TAIL[k], aim(Dog::TAIL[k], Dog::TAIL[k + 1]));
    }
    for (first, side, joints) in [
        (Dog::LEG_FL, -1.0, Dog::FRONT),
        (Dog::LEG_FR, 1.0, Dog::FRONT),
        (Dog::LEG_HL, -1.0, Dog::HIND),
        (Dog::LEG_HR, 1.0, Dog::HIND),
    ] {
        let j = joints.map(|p| side_v(p, side));
        b[first] = pose(j[0], aim(j[0], j[1]));
        b[first + 1] = pose(j[1], aim(j[1], j[2]));
        b[first + 2] = pose(j[2], aim(j[2], j[3]));
        b[first + 3] = pose(j[3], Quat::IDENTITY);
    }
    b
}

// ---------------------------------------------------------------------------------------
// The mannequin

/// A premium display mannequin, 1.84 m, idealized athletic proportions: one lofted torso
/// (rib cage, waist, pelvis, seat) with soft pectorals, shoulder blades, lower-back columns
/// and glutes over it, one lofted piece per limb with deltoids, calves and kneecaps, a
/// stylized head (brow, nose, lips, jaw, simplified ears) and relaxed hands and feet.
pub fn human_sculpt() -> Sculpt {
    let mut s = Sculpt::new();
    human_torso(&mut s);
    human_head(&mut s);
    // Each limb blends into the torso but never into the other limb.
    s.group(0.03, |s| {
        for side in [-1.0f32, 1.0] {
            s.group(0.0, |s| human_arm(s, side));
        }
    });
    s.group(0.045, |s| {
        for side in [-1.0f32, 1.0] {
            s.group(0.0, |s| human_leg(s, side));
        }
    });
    // Flat soles.
    let sole = human_bind()[Human::LEG_L + 2].translation.y - Human::ANKLE_HEIGHT;
    s.clip(0.004, Vec3::NEG_Y, -sole);
    s
}

fn human_torso(s: &mut Sculpt) {
    let v = Vec3::new;
    let id = Quat::IDENTITY;
    let (pelvis, spine, chest) = (0u8, 1u8, 2u8);
    let rx = Quat::from_rotation_x;
    let ry = Quat::from_rotation_y;
    let rz = Quat::from_rotation_z;
    // Height, half-width, front and back of the core, bottom (crotch) to the base of the neck.
    let torso = Loft::new(Vec3::ZERO, Vec3::Y, Vec3::NEG_Z, Vec3::X)
        .bone(pelvis)
        .round(2.2)
        .key(0.84, 0.105, 0.055, 0.07)
        .key(0.9, 0.163, 0.08, 0.105)
        .key(0.95, 0.166, 0.09, 0.1)
        .key(1.0, 0.156, 0.093, 0.085)
        .bone(spine)
        .key(1.06, 0.143, 0.095, 0.072)
        .key(1.12, 0.141, 0.098, 0.074)
        .key(1.19, 0.147, 0.102, 0.083)
        .bone(chest)
        .round(2.35)
        .key(1.26, 0.157, 0.108, 0.095)
        .key(1.33, 0.164, 0.114, 0.102)
        .key(1.4, 0.166, 0.11, 0.104)
        .round(2.2)
        .key(1.45, 0.156, 0.097, 0.1)
        .key(1.49, 0.128, 0.075, 0.088)
        .round(2.0)
        .key(1.52, 0.085, 0.056, 0.072)
        .key(1.545, 0.062, 0.048, 0.062);
    s.loft(0.0, torso);
    for side in [-1.0f32, 1.0] {
        let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
        // Trapezius slope from the neck to the shoulder, over the top of the shoulder.
        s.cone(
            chest,
            0.05,
            m(0.03, 1.552, 0.03),
            m(0.15, 1.494, 0.018),
            0.037,
            0.03,
        );
        s.ellipsoid(
            chest,
            0.04,
            m(0.168, 1.474, 0.008),
            v(0.042, 0.03, 0.05),
            id,
        );
    }
    // Muscle definition laid on the form.
    for side in [-1.0f32, 1.0] {
        let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
        let q = |r: Quat| mirror(r, side);
        // Pectorals: a plate thickest low and to the side, its lower edge rising toward the
        // armpit.
        s.bump_surface(
            m(0.08, 1.37, -0.12),
            v(0.085, 0.06, 0.06),
            q(ry(-0.35) * rz(0.3)),
            0.009,
        );
        s.bump_surface(
            m(0.095, 1.345, -0.12),
            v(0.06, 0.03, 0.05),
            q(ry(-0.4) * rz(0.35)),
            0.004,
        );
        // Lats: the V from the armpit to the waist.
        s.bump_surface(
            m(0.165, 1.31, 0.03),
            v(0.05, 0.12, 0.07),
            q(rz(-0.1)),
            0.006,
        );
        // Shoulder blades.
        s.bump_surface(
            m(0.085, 1.39, 0.11),
            v(0.06, 0.075, 0.05),
            q(ry(0.4) * rz(0.1)),
            0.005,
        );
        // Columns of the lower back.
        s.bump_surface(m(0.035, 1.12, 0.08), v(0.03, 0.13, 0.05), id, 0.006);
        // Collarbones, from the notch to the shoulder.
        s.bump_surface(
            m(0.085, 1.497, -0.07),
            v(0.075, 0.011, 0.03),
            q(ry(-0.29)),
            0.0035,
        );
        // Glutes.
        s.bump_surface(
            m(0.068, 0.895, 0.11),
            v(0.075, 0.085, 0.07),
            q(rx(-0.1)),
            0.014,
        );
    }
    // Abdomen, the notch between the collarbones, the navel, the spine and the cleft of the
    // seat.
    s.bump_surface(v(0.0, 1.16, -0.11), v(0.075, 0.13, 0.05), id, 0.004);
    s.bump_surface(v(0.0, 1.37, -0.12), v(0.016, 0.07, 0.04), id, -0.002);
    s.bump_surface(v(0.0, 1.5, -0.07), v(0.026, 0.02, 0.04), id, -0.002);
    s.bump_surface(v(0.0, 1.075, -0.11), v(0.0075, 0.0095, 0.03), id, -0.005);
    s.bump_surface(v(0.0, 1.25, 0.11), v(0.011, 0.24, 0.05), id, -0.004);
    s.bump_surface(v(0.0, 0.88, 0.13), v(0.01, 0.07, 0.06), id, -0.012);

    // Neck, leaning forward a little, with the muscles from behind the ears to the
    // collarbones.
    let neck = Loft::new(
        v(0.0, 1.46, 0.022),
        v(0.0, 0.22, -0.027),
        Vec3::NEG_Z,
        Vec3::X,
    )
    .bone(chest)
    .key(0.0, 0.066, 0.058, 0.062)
    .bone(3)
    .key(0.06, 0.06, 0.055, 0.058)
    .key(0.15, 0.057, 0.053, 0.055)
    .bone(4)
    .key(0.22, 0.055, 0.052, 0.052);
    s.loft(0.045, neck);
    for side in [-1.0f32, 1.0] {
        let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
        s.cone(
            3,
            0.03,
            m(0.05, 1.655, 0.012),
            m(0.018, 1.5, -0.046),
            0.012,
            0.01,
        );
    }
}

fn human_head(s: &mut Sculpt) {
    let v = Vec3::new;
    let id = Quat::IDENTITY;
    let head = Human::HEAD_I as u8;
    let rx = Quat::from_rotation_x;
    let rz = Quat::from_rotation_z;
    s.group(0.03, |s| {
        // Cranium and the face mass; the jaw to the chin.
        s.ellipsoid(head, 0.0, v(0.0, 1.735, 0.012), v(0.076, 0.098, 0.1), id);
        s.ellipsoid(
            head,
            0.045,
            v(0.0, 1.68, -0.028),
            v(0.064, 0.075, 0.072),
            id,
        );
        s.ellipsoid(head, 0.03, v(0.0, 1.645, -0.07), v(0.032, 0.032, 0.029), id);
        s.ellipsoid(head, 0.03, v(0.0, 1.61, -0.066), v(0.024, 0.02, 0.02), id);
        for side in [-1.0f32, 1.0] {
            let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
            let q = |r: Quat| mirror(r, side);
            s.cone(
                head,
                0.04,
                m(0.057, 1.664, 0.004),
                m(0.022, 1.61, -0.06),
                0.017,
                0.015,
            );
            // Ears: a flat shell.
            s.ellipsoid(
                head,
                0.01,
                m(0.077, 1.69, 0.012),
                v(0.0095, 0.03, 0.019),
                q(rz(0.15) * rx(0.15)),
            );
        }
        // One continuous brow.
        s.ellipsoid(
            head,
            0.035,
            v(0.0, 1.742, -0.074),
            v(0.06, 0.016, 0.022),
            id,
        );
        // Nose: bridge and tip.
        s.ellipsoid(
            head,
            0.016,
            v(0.0, 1.692, -0.1),
            v(0.0105, 0.03, 0.013),
            rx(0.38),
        );
        s.sphere(head, 0.011, v(0.0, 1.669, -0.11), 0.0092);
        // Features laid on the face.
        for side in [-1.0f32, 1.0] {
            let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
            // Eye sockets, closed almond lids in them.
            s.bump_surface(m(0.032, 1.711, -0.1), v(0.025, 0.018, 0.03), id, -0.005);
            s.bump_surface(m(0.032, 1.7125, -0.1), v(0.017, 0.011, 0.03), id, 0.0042);
            // Cheekbones, the wings of the nose, the bowl of the ear.
            s.bump_surface(m(0.05, 1.69, -0.08), v(0.026, 0.016, 0.03), id, 0.004);
            s.bump_surface(m(0.0125, 1.667, -0.1), v(0.009, 0.0085, 0.012), id, 0.0035);
            s.bump_surface(
                m(0.09, 1.688, 0.013),
                v(0.007, 0.017, 0.011),
                q_ear(side),
                -0.0035,
            );
        }
        // Lips: a fuller lower lip, the parting line, the groove above and the dip below;
        // the chin.
        s.bump_surface(v(0.0, 1.641, -0.11), v(0.021, 0.008, 0.02), id, 0.0025);
        s.bump_surface(v(0.0, 1.627, -0.11), v(0.018, 0.008, 0.02), id, 0.003);
        s.bump_surface(v(0.0, 1.6345, -0.11), v(0.021, 0.0024, 0.02), id, -0.0007);
        s.bump_surface(v(0.0, 1.652, -0.11), v(0.0045, 0.008, 0.02), id, -0.0007);
        s.bump_surface(v(0.0, 1.603, -0.1), v(0.017, 0.012, 0.02), id, 0.003);
    });
}

fn q_ear(side: f32) -> Quat {
    mirror(
        Quat::from_rotation_z(0.15) * Quat::from_rotation_x(0.15),
        side,
    )
}

fn human_arm(s: &mut Sculpt, side: f32) {
    let v = Vec3::new;
    let id = Quat::IDENTITY;
    let bind = human_bind();
    let first = if side < 0.0 {
        Human::ARM_L
    } else {
        Human::ARM_R
    };
    let (upper, fore, hand) = (first as u8, first as u8 + 1, first as u8 + 2);
    let (sh, el, wr) = (
        bind[first].translation,
        bind[first + 1].translation,
        bind[first + 2].translation,
    );
    let ru = bind[first].rotation;
    let rf = bind[first + 1].rotation;
    let fd = (wr - el).normalize();
    // Local offsets in the bone frames, +x lateral.
    let lu = |x: f32, y: f32, z: f32| sh + ru * v(x * side, y, z);
    let lf = |x: f32, y: f32, z: f32| el + rf * v(x * side, y, z);
    let qu = |r: Quat| ru * mirror(r, side);
    let qf = |r: Quat| rf * mirror(r, side);
    // Shoulder to wrist in one piece: half-width, front, back.
    let arm = Loft::new(sh, el - sh, Vec3::NEG_Z, v(side, 0.0, 0.0))
        .bone(upper)
        .key(-0.07, 0.025, 0.025, 0.025)
        .key(-0.03, 0.046, 0.048, 0.048)
        .key(0.05, 0.047, 0.047, 0.048)
        .key(0.13, 0.043, 0.047, 0.046)
        .key(0.21, 0.038, 0.04, 0.04)
        .key(0.265, 0.037, 0.035, 0.036)
        .bone(fore)
        .key_through(el, 0.039, 0.034, 0.036)
        .key_through(el + fd * 0.06, 0.041, 0.038, 0.037)
        .key_through(el + fd * 0.13, 0.033, 0.032, 0.031)
        .key_through(el + fd * 0.2, 0.024, 0.027, 0.026)
        .key_through(wr, 0.019, 0.028, 0.027)
        .key_through(wr + fd * 0.015, 0.018, 0.025, 0.024);
    s.loft(0.0, arm);
    // Deltoid cap, the point of the elbow, the muscle on the thumb side of the forearm.
    s.ellipsoid(
        upper,
        0.045,
        lu(0.012, -0.045, 0.0),
        v(0.05, 0.085, 0.056),
        qu(id),
    );
    s.bump_surface(
        el + ru * v(0.0, 0.0, 0.05),
        v(0.02, 0.022, 0.03),
        qu(id),
        0.004,
    );
    s.bump_surface(lf(0.03, -0.06, -0.03), v(0.03, 0.07, 0.035), qf(id), 0.005);
    let _ = fore;
    s.group(0.012, |s| human_hand(s, hand, wr, rf, side));
}

/// A relaxed hand in the forearm's frame (`rot`) at the wrist: +x the back of the hand, −y
/// along the fingers, −z the thumb.
fn human_hand(s: &mut Sculpt, bone: u8, wrist: Vec3, rot: Quat, side: f32) {
    let v = Vec3::new;
    let at = |p: Vec3| wrist + rot * v(p.x * side, p.y, p.z);
    let dir = |x: f32, y: f32, z: f32| rot * v(x * side, y, z);
    let id = Quat::IDENTITY;
    let q = |r: Quat| rot * mirror(r, side);
    // Palm: x toward the thumb, thicker on the palm side than the back, rounding off into the
    // webs between the fingers.
    let palm = Loft::new(
        at(v(0.0, 0.006, 0.0)),
        dir(0.0, -1.0, 0.0),
        dir(-1.0, 0.0, 0.0),
        dir(0.0, 0.0, -1.0),
    )
    .bone(bone)
    .round(2.6)
    .key(0.0, 0.026, 0.014, 0.013)
    .key(0.03, 0.034, 0.016, 0.012)
    .key(0.06, 0.04, 0.016, 0.0115)
    .key(0.084, 0.041, 0.0125, 0.0105)
    .key(0.093, 0.037, 0.0085, 0.0075)
    .key(0.099, 0.026, 0.004, 0.0035)
    .key(0.101, 0.012, 0.001, 0.001);
    s.loft(0.0, palm);
    // Balls of the thumb and of the little finger.
    s.ellipsoid(
        bone,
        0.012,
        at(v(-0.009, -0.036, -0.022)),
        v(0.014, 0.028, 0.016),
        q(id),
    );
    s.ellipsoid(
        bone,
        0.012,
        at(v(-0.007, -0.052, 0.028)),
        v(0.01, 0.034, 0.01),
        q(id),
    );
    // Fingers, slightly apart: base on the knuckle line, (proximal, middle, distal) lengths,
    // radius at the base, flexion of each joint toward the palm (radians) and fan angle.
    type Finger = (f32, f32, [f32; 3], f32, [f32; 3], f32);
    let fingers: [Finger; 4] = [
        (
            -0.0265,
            -0.089,
            [0.042, 0.025, 0.021],
            0.0084,
            [0.2, 0.3, 0.18],
            -0.1,
        ),
        (
            -0.0088,
            -0.092,
            [0.046, 0.029, 0.022],
            0.0087,
            [0.25, 0.36, 0.2],
            -0.035,
        ),
        (
            0.0088,
            -0.09,
            [0.043, 0.027, 0.021],
            0.0082,
            [0.3, 0.42, 0.22],
            0.035,
        ),
        (
            0.0255,
            -0.084,
            [0.034, 0.02, 0.019],
            0.0072,
            [0.36, 0.48, 0.24],
            0.11,
        ),
    ];
    s.group(0.012, |s| {
        for (z, y, lengths, radius, flex, fan) in fingers {
            let d0 = v(0.0, -fan.cos(), fan.sin());
            let palm_dir = v(-1.0, 0.0, 0.0);
            let mut p = v(0.0, y + 0.014, z);
            let mut angle = 0.0;
            let mut r = radius;
            for k in 0..3 {
                angle += flex[k];
                let d = d0 * angle.cos() + palm_dir * angle.sin();
                let len = lengths[k] + if k == 0 { 0.014 } else { 0.0 };
                let next = p + d * len;
                let r_next = r * if k == 2 { 0.8 } else { 0.9 };
                let end = if k == 2 { next - d * r_next } else { next };
                s.cone(
                    bone,
                    if k == 0 { 0.0 } else { 0.004 },
                    at(p),
                    at(end),
                    r,
                    r_next,
                );
                p = next;
                r = r_next;
            }
        }
        // Thumb: metacarpal inside the ball of the thumb, two phalanges along the index.
        let thumb = [
            (v(-0.3, -0.78, -0.5), 0.04, 0.0118),
            (v(-0.32, -0.9, -0.14), 0.032, 0.0106),
            (v(-0.25, -0.96, 0.04), 0.026, 0.0096),
        ];
        let mut p = v(-0.004, -0.022, -0.02);
        for (k, (d, len, r)) in thumb.into_iter().enumerate() {
            let d = d.normalize();
            let next = p + d * len;
            let r_next = if k == 2 { r * 0.8 } else { thumb[k + 1].2 };
            let end = if k == 2 { next - d * r_next } else { next };
            s.cone(
                bone,
                if k == 0 { 0.0 } else { 0.005 },
                at(p),
                at(end),
                r,
                r_next,
            );
            p = next;
        }
    });
    // Knuckles on the back of the hand.
    for (z, y, _, radius, _, _) in fingers {
        s.bump_surface(
            at(v(0.03, y + 0.004, z)),
            Vec3::splat(radius * 1.1),
            id,
            0.0016,
        );
    }
}

fn human_leg(s: &mut Sculpt, side: f32) {
    let v = Vec3::new;
    let bind = human_bind();
    let first = if side < 0.0 {
        Human::LEG_L
    } else {
        Human::LEG_R
    };
    let (thigh, shin, foot) = (first as u8, first as u8 + 1, first as u8 + 2);
    let (hip, knee, ankle) = (
        bind[first].translation,
        bind[first + 1].translation,
        bind[first + 2].translation,
    );
    let m = |p: Vec3, x: f32, y: f32, z: f32| p + v(x * side, y, z);
    // Hip to ankle in one piece (the bind leg is straight): along the leg, lateral offset,
    // half-width, front, back. The top rounds off inside the pelvis; the outer thigh carries
    // on from the hip.
    let keys: [(f32, f32, f32, f32, f32); 16] = [
        (-0.08, -0.02, 0.04, 0.04, 0.045),
        (-0.03, -0.012, 0.072, 0.072, 0.078),
        (0.04, 0.0, 0.083, 0.082, 0.085),
        (0.1, 0.005, 0.083, 0.083, 0.082),
        (0.16, 0.005, 0.078, 0.081, 0.076),
        (0.24, 0.002, 0.07, 0.075, 0.068),
        (0.34, -0.002, 0.058, 0.064, 0.058),
        (0.4, -0.003, 0.05, 0.054, 0.052),
        (0.44, 0.0, 0.047, 0.048, 0.048),
        (0.49, 0.0, 0.048, 0.044, 0.054),
        (0.56, -0.003, 0.053, 0.04, 0.068),
        (0.64, -0.002, 0.045, 0.036, 0.054),
        (0.72, 0.0, 0.036, 0.031, 0.038),
        (0.79, 0.0, 0.029, 0.027, 0.029),
        (0.845, 0.0, 0.031, 0.028, 0.03),
        (0.875, 0.0, 0.03, 0.028, 0.03),
    ];
    let mut leg = Loft::new(hip, ankle - hip, Vec3::NEG_Z, v(side, 0.0, 0.0)).bone(thigh);
    for (t, x, a, f, g) in keys {
        if t >= Human::THIGH {
            leg = leg.bone(shin);
        }
        leg = leg.key_at(t, x, 0.0, a, f, g);
    }
    s.loft(0.0, leg);
    // The teardrop above the knee, the kneecap, the two heads of the calf, the ankle bones.
    let id = Quat::IDENTITY;
    s.bump_surface(
        m(knee, -0.035, 0.065, -0.04),
        v(0.03, 0.05, 0.035),
        id,
        0.005,
    );
    s.bump_surface(m(knee, 0.0, 0.008, -0.06), v(0.024, 0.03, 0.03), id, 0.005);
    s.bump_surface(
        m(knee, -0.03, -0.13, 0.07),
        v(0.035, 0.085, 0.045),
        id,
        0.006,
    );
    s.bump_surface(
        m(knee, 0.03, -0.115, 0.065),
        v(0.03, 0.075, 0.04),
        id,
        0.005,
    );
    s.bump_surface(
        m(ankle, -0.04, 0.004, 0.0),
        v(0.014, 0.014, 0.02),
        id,
        0.004,
    );
    s.bump_surface(
        m(ankle, 0.04, -0.008, 0.006),
        v(0.014, 0.014, 0.02),
        id,
        0.004,
    );
    let _ = (thigh, shin);
    human_foot(s, foot, ankle, side);
}

/// Heel, arch, instep, ball and five toes on a flat sole; toes point −Z.
fn human_foot(s: &mut Sculpt, bone: u8, ankle: Vec3, side: f32) {
    let v = Vec3::new;
    let sole = ankle.y - Human::ANKLE_HEIGHT;
    let heel = Vec3::new(ankle.x, sole, ankle.z + 0.07);
    // Along the foot: x lateral, the center `yc` above the sole, half-width, top, bottom.
    let keys: [(f32, f32, f32, f32, f32, f32); 8] = [
        (0.0, 0.0, 0.035, 0.026, 0.025, 0.035),
        (0.03, 0.0, 0.045, 0.031, 0.05, 0.045),
        (0.07, 0.0, 0.05, 0.034, 0.045, 0.05),
        (0.12, 0.003, 0.038, 0.038, 0.03, 0.038),
        (0.17, 0.002, 0.027, 0.043, 0.02, 0.027),
        (0.205, -0.001, 0.02, 0.046, 0.015, 0.02),
        (0.235, -0.003, 0.015, 0.043, 0.01, 0.015),
        (0.25, -0.005, 0.012, 0.035, 0.007, 0.012),
    ];
    let mut loft = Loft::new(heel, Vec3::NEG_Z, Vec3::Y, v(side, 0.0, 0.0)).bone(bone);
    for (t, x, yc, a, top, bottom) in keys {
        loft = loft.key_at(t, x, -yc, a, top, bottom);
    }
    s.loft(0.02, loft);
    // Toes, big (medial) to little: x, tip distance from the heel, radius.
    let toes: [(f32, f32, f32); 5] = [
        (-0.026, 0.278, 0.0125),
        (-0.007, 0.27, 0.0095),
        (0.0075, 0.262, 0.009),
        (0.02, 0.252, 0.0085),
        (0.031, 0.24, 0.008),
    ];
    s.group(0.008, |s| {
        for (x, tip, r) in toes {
            s.group(0.001, |s| {
                let from = heel + v(x * 0.9 * side, r + 0.003, -0.215);
                let to = heel + v(x * side, r * 0.92, -(tip - r));
                s.cone(bone, 0.0, from, to, r * 1.05, r * 0.95);
            });
        }
    });
    // The arch under the inside of the foot.
    s.carve_ellipsoid(
        0.015,
        v(ankle.x - side * 0.032, sole - 0.006, ankle.z - 0.045),
        v(0.02, 0.016, 0.05),
        Quat::IDENTITY,
    );
}

// ---------------------------------------------------------------------------------------
// The dog

/// A medium retriever: one lofted body (deep keel-shaped chest, tucked loin, level croup), a
/// thick arched neck, a head with stop, muzzle, flews, brow and drop ears, angulated lofted legs
/// with shoulders and muscular second thighs on round paws, and a tapering tail. Eyes and nose
/// are separate glossy parts ([`DOG_EYES`], [`dog_nose_sculpt`]).
pub fn dog_sculpt() -> Sculpt {
    let bind = dog_bind();
    let mut s = Sculpt::new();
    let v = Vec3::new;
    let id = Quat::IDENTITY;
    let rx = Quat::from_rotation_x;
    let rz = Quat::from_rotation_z;
    let (hips, spine, chest, neck, head) = (0u8, 1u8, 2u8, 3u8, 4u8);

    // Body, forechest to rump: along z, the section's center height, half-width, depth below
    // (keel) and height above (back).
    let keys: [(f32, f32, f32, f32, f32, u8); 11] = [
        (-0.39, 0.47, 0.03, 0.03, 0.03, chest),
        (-0.36, 0.47, 0.065, 0.07, 0.075, chest),
        (-0.3, 0.475, 0.1, 0.125, 0.12, chest),
        (-0.21, 0.48, 0.113, 0.15, 0.135, chest),
        (-0.11, 0.49, 0.112, 0.145, 0.115, spine),
        (-0.02, 0.505, 0.1, 0.115, 0.097, spine),
        (0.08, 0.525, 0.082, 0.082, 0.083, spine),
        (0.17, 0.53, 0.088, 0.09, 0.075, hips),
        (0.26, 0.53, 0.098, 0.1, 0.073, hips),
        (0.34, 0.52, 0.08, 0.085, 0.06, hips),
        (0.385, 0.515, 0.03, 0.035, 0.025, hips),
    ];
    let mut body = Loft::new(Vec3::ZERO, Vec3::Z, Vec3::NEG_Y, Vec3::X).round(2.15);
    for (z, y, a, below, above, bone) in keys {
        body = body.bone(bone).key_at(z, 0.0, y, a, below, above);
    }
    s.loft(0.0, body);
    // Neck: deep at the shoulders, arched along the crest, into the back of the skull.
    let neck_from = v(0.0, 0.5, -0.29);
    let neck_to = v(0.0, 0.79, -0.47);
    let throat = (neck_to - neck_from).cross(Vec3::X).normalize();
    let neck_loft = Loft::new(neck_from, neck_to - neck_from, throat, Vec3::X)
        .bone(chest)
        .key(0.0, 0.085, 0.105, 0.09)
        .bone(neck)
        .key(0.1, 0.072, 0.08, 0.075)
        .key(0.2, 0.062, 0.066, 0.062)
        .bone(head)
        .key(0.28, 0.055, 0.058, 0.056)
        .key(0.33, 0.035, 0.035, 0.035);
    s.loft(0.06, neck_loft);

    // Head: skull, stop and muzzle, lower jaw, flews; brow, cheeks, drop ears.
    s.group(0.04, |s| {
        s.ellipsoid(head, 0.0, v(0.0, 0.815, -0.495), v(0.058, 0.055, 0.073), id);
        s.cone(
            head,
            0.035,
            v(0.0, 0.797, -0.555),
            v(0.0, 0.778, -0.668),
            0.039,
            0.029,
        );
        s.ellipsoid(
            head,
            0.025,
            v(0.0, 0.776, -0.612),
            v(0.034, 0.03, 0.058),
            id,
        );
        s.cone(
            head,
            0.02,
            v(0.0, 0.748, -0.53),
            v(0.0, 0.751, -0.642),
            0.026,
            0.018,
        );
        for side in [-1.0f32, 1.0] {
            let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
            let q = |r: Quat| mirror(r, side);
            s.ellipsoid(
                head,
                0.025,
                m(0.022, 0.759, -0.618),
                v(0.017, 0.022, 0.05),
                q(rx(0.08)),
            );
            s.ellipsoid(
                head,
                0.014,
                m(0.068, 0.79, -0.478),
                v(0.011, 0.06, 0.042),
                q(rz(0.22)),
            );
        }
        for side in [-1.0f32, 1.0] {
            let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
            s.carve_ellipsoid(0.008, m(0.034, 0.812, -0.567), v(0.014, 0.012, 0.012), id);
            s.bump_surface(m(0.024, 0.832, -0.556), v(0.02, 0.014, 0.02), id, 0.004);
            s.bump_surface(m(0.045, 0.785, -0.52), v(0.03, 0.03, 0.04), id, 0.006);
        }
    });

    // Legs.
    for (first, side, front) in [
        (Dog::LEG_FL, -1.0f32, true),
        (Dog::LEG_FR, 1.0, true),
        (Dog::LEG_HL, -1.0, false),
        (Dog::LEG_HR, 1.0, false),
    ] {
        let j: [Vec3; 4] = std::array::from_fn(|k| bind[first + k].translation);
        let b = |k: usize| (first + k) as u8;
        let m = |x: f32, y: f32, z: f32| v(x * side, y, z);
        let lateral = v(side, 0.0, 0.0);
        s.group(0.03, |s| {
            if front {
                // Shoulder (blade and upper arm) over the chest, then the leg from the elbow.
                s.ellipsoid(
                    b(0),
                    0.0,
                    m(0.072, 0.47, -0.27),
                    v(0.045, 0.13, 0.075),
                    mirror(rx(0.45), side),
                );
                let leg = Loft::new(j[1], j[2] - j[1], Vec3::NEG_Z, lateral)
                    .bone(b(0))
                    .key_through(j[1] + v(0.0, 0.1, 0.02), 0.012, 0.012, 0.012)
                    .key_through(j[1] + v(0.0, 0.07, 0.015), 0.036, 0.04, 0.045)
                    .bone(b(1))
                    .key_through(j[1], 0.037, 0.04, 0.046)
                    .key_through(j[1].lerp(j[2], 0.35), 0.033, 0.035, 0.034)
                    .key_through(j[1].lerp(j[2], 0.75), 0.025, 0.026, 0.025)
                    .bone(b(2))
                    .key_through(j[2], 0.024, 0.025, 0.024)
                    .key_through(j[2].lerp(j[3], 0.7), 0.022, 0.023, 0.022)
                    .key_through(j[3] + v(0.0, 0.006, -0.004), 0.012, 0.012, 0.012);
                s.loft(0.012, leg);
            } else {
                // Thigh: broad and flat from the hip to the stifle, the second thigh behind
                // the shank, the hock, the long rear pastern.
                let thigh = Loft::new(j[0], j[1] - j[0], Vec3::NEG_Z, lateral)
                    .bone(b(0))
                    .key_at(-0.08, -0.03, 0.0, 0.012, 0.015, 0.015)
                    .key_at(-0.05, -0.02, 0.0, 0.035, 0.05, 0.06)
                    .key(0.0, 0.055, 0.09, 0.1)
                    .key(0.08, 0.055, 0.082, 0.085)
                    .key(0.15, 0.042, 0.055, 0.055)
                    .key(0.19, 0.034, 0.038, 0.04)
                    .key(0.215, 0.02, 0.02, 0.02);
                s.loft(0.0, thigh);
                let shank = Loft::new(j[1], j[2] - j[1], Vec3::NEG_Z, lateral)
                    .bone(b(1))
                    .key(-0.02, 0.025, 0.025, 0.03)
                    .key(0.01, 0.036, 0.038, 0.05)
                    .key(0.08, 0.033, 0.031, 0.05)
                    .key(0.16, 0.025, 0.023, 0.029)
                    .key(0.225, 0.023, 0.022, 0.025)
                    .key(0.245, 0.012, 0.012, 0.012);
                s.loft(0.02, shank);
                s.sphere(b(1), 0.012, j[2] + v(0.0, 0.004, 0.016), 0.017);
                let pastern = Loft::new(j[2], j[3] - j[2], Vec3::NEG_Z, lateral)
                    .bone(b(2))
                    .key(-0.015, 0.015, 0.015, 0.015)
                    .key(0.01, 0.022, 0.023, 0.024)
                    .key(0.08, 0.021, 0.021, 0.022)
                    .key(0.1, 0.012, 0.012, 0.012);
                s.loft(0.012, pastern);
            }
            // Round paw with four toes.
            let paw = j[3];
            s.ellipsoid(
                b(3),
                0.015,
                paw + v(0.0, 0.006, -0.012),
                v(0.026, 0.022, 0.032),
                id,
            );
            for x in [-0.017f32, -0.006, 0.006, 0.017] {
                s.sphere(
                    b(3),
                    0.006,
                    paw + v(x, 0.002 - x.abs() * 0.1, -0.034 + x.abs() * 0.3),
                    0.0115,
                );
            }
        });
    }

    // Tail.
    let tail: [Vec3; 4] = Dog::TAIL;
    let radii = [0.03, 0.023, 0.016, 0.009];
    s.cone(
        5,
        0.03,
        tail[0] + v(0.0, 0.0, -0.04),
        tail[0],
        0.038,
        radii[0],
    );
    for k in 0..3 {
        s.cone(
            (Dog::TAIL_I + k) as u8,
            0.015,
            tail[k],
            tail[k + 1],
            radii[k],
            radii[k + 1],
        );
    }
    // Paws stand on a flat sole.
    s.clip(0.006, Vec3::NEG_Y, 0.0);
    s
}

/// The dog's nose (rhinarium with nostrils), around the origin; it sits at
/// [`DOG_NOSE_AT`] in the bind pose.
pub fn dog_nose_sculpt() -> Sculpt {
    let v = Vec3::new;
    let mut s = Sculpt::new();
    let o = DOG_NOSE_AT;
    s.ellipsoid(
        4,
        0.0,
        o,
        v(0.021, 0.015, 0.016),
        Quat::from_rotation_x(0.25),
    );
    s.ellipsoid(
        4,
        0.008,
        o + v(0.0, -0.008, 0.004),
        v(0.012, 0.01, 0.012),
        Quat::IDENTITY,
    );
    for side in [-1.0f32, 1.0] {
        s.carve_ellipsoid(
            0.003,
            o + v(0.008 * side, -0.001, -0.014),
            v(0.004, 0.0028, 0.005),
            Quat::from_rotation_y(0.4 * side),
        );
    }
    s.carve_cone(
        0.003,
        o + v(0.0, -0.004, -0.016),
        o + v(0.0, -0.018, -0.01),
        0.0016,
        0.0016,
    );
    s
}

/// Where the nose sits in the dog's bind pose.
pub const DOG_NOSE_AT: Vec3 = Vec3::new(0.0, 0.784, -0.684);
/// Eye centers and radius in the dog's bind pose.
pub const DOG_EYES: [Vec3; 2] = [
    Vec3::new(-0.033, 0.813, -0.565),
    Vec3::new(0.033, 0.813, -0.565),
];
pub const DOG_EYE_RADIUS: f32 = 0.0125;

// ---------------------------------------------------------------------------------------
// Templates

/// Which body to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyPart {
    Mannequin,
    Dog,
    DogNose,
    DogEyes,
}

/// A rest-pose mesh with skin weights and the bind pose it was built in.
#[derive(Debug, Clone, Default)]
pub struct SkinTemplate {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Baked ambient occlusion as a vertex color (linear RGBA).
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Up to four bones per vertex and their weights (summing to 1, largest first).
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Model-space bind pose.
    pub bind: Vec<BonePose>,
}

impl SkinTemplate {
    /// The template as a render mesh (rest pose).
    pub fn mesh(&self) -> engine_render::Mesh {
        let vertices = self
            .positions
            .iter()
            .zip(&self.normals)
            .zip(&self.colors)
            .map(|((p, n), c)| {
                engine_render::Vertex::new(*p, *n, Vec2::ZERO).with_color(Vec4::from(*c))
            })
            .collect();
        engine_render::Mesh::new(vertices, self.indices.clone())
    }
}

/// How finely bodies are meshed: 1 = full quality, larger = coarser (tests).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyQuality(pub f32);

impl Default for BodyQuality {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Mesh `part`, weight it and package it (runs for a second or two at full quality).
pub fn build(part: BodyPart, quality: BodyQuality) -> SkinTemplate {
    let q = quality.0.max(0.1);
    match part {
        BodyPart::Mannequin => {
            let mesh = human_sculpt().mesh(&MeshSettings {
                cell: 0.0034 * q,
                target_triangles: (90_000.0 / (q * q)) as usize,
                max_edge: 0.022 * q,
                tolerance: 0.0002 * q,
                relax: 3,
                occlusion: 0.035,
            });
            template(mesh, human_bind(), 0.65, smoothing(36, q))
        }
        BodyPart::Dog => {
            let mesh = dog_sculpt().mesh(&MeshSettings {
                cell: 0.004 * q,
                target_triangles: (60_000.0 / (q * q)) as usize,
                max_edge: 0.025 * q,
                tolerance: 0.00015 * q,
                relax: 3,
                occlusion: 0.03,
            });
            template(mesh, dog_bind(), 0.75, smoothing(30, q))
        }
        BodyPart::DogNose => {
            let mesh = dog_nose_sculpt().mesh(&MeshSettings {
                cell: 0.0012 * q,
                target_triangles: 3000,
                max_edge: 0.006,
                tolerance: 0.00003,
                relax: 2,
                occlusion: 0.008,
            });
            template(mesh, dog_bind(), 0.6, 0)
        }
        BodyPart::DogEyes => {
            let mut s = Sculpt::new();
            for c in DOG_EYES {
                s.sphere(Dog::HEAD_I as u8, 0.0, c, DOG_EYE_RADIUS);
            }
            let mesh = s.mesh(&MeshSettings {
                cell: 0.0015 * q,
                target_triangles: 1600,
                max_edge: 0.004,
                tolerance: 0.0,
                relax: 2,
                occlusion: 0.0,
            });
            template(mesh, dog_bind(), 1.0, 0)
        }
    }
}

/// Smoothing passes for the same blend width (meters) at a coarser quality.
fn smoothing(full: usize, quality: f32) -> usize {
    (full as f32 / (quality * quality)).ceil() as usize
}

/// Package a meshed sculpt: bake occlusion into vertex colors (`occlusion_floor` = darkest)
/// and diffuse the per-vertex bone labels over the surface `smoothing` times into weights.
fn template(
    mesh: SculptMesh,
    bind: Vec<BonePose>,
    occlusion_floor: f32,
    smoothing: usize,
) -> SkinTemplate {
    let n = mesh.positions.len();
    let bones = bind.len();
    let adjacency = vertex_neighbors(n, &mesh.triangles);
    // Dense weights per vertex, starting from the sculpt's labels.
    let mut w = vec![0.0f32; n * bones];
    for (i, b) in mesh.bones.iter().enumerate() {
        w[i * bones + (*b as usize).min(bones - 1)] = 1.0;
    }
    let mut next = w.clone();
    for _ in 0..smoothing {
        for i in 0..n {
            let ring = &adjacency[i];
            let row = &mut next[i * bones..(i + 1) * bones];
            if ring.is_empty() {
                row.copy_from_slice(&w[i * bones..(i + 1) * bones]);
                continue;
            }
            let p = mesh.positions[i];
            let mut total = 0.0;
            row.fill(0.0);
            for &j in ring {
                let c = 1.0 / (p.distance(mesh.positions[j as usize]) + 1e-4);
                total += c;
                for (r, x) in row
                    .iter_mut()
                    .zip(&w[j as usize * bones..(j as usize + 1) * bones])
                {
                    *r += c * x;
                }
            }
            for (r, x) in row.iter_mut().zip(&w[i * bones..(i + 1) * bones]) {
                *r = 0.5 * x + 0.5 * *r / total;
            }
        }
        std::mem::swap(&mut w, &mut next);
    }
    let mut joints = Vec::with_capacity(n);
    let mut weights = Vec::with_capacity(n);
    for i in 0..n {
        let row = &w[i * bones..(i + 1) * bones];
        let mut order: Vec<usize> = (0..bones).collect();
        order.sort_by(|a, b| row[*b].total_cmp(&row[*a]));
        let mut j = [0u16; 4];
        let mut wt = [0.0f32; 4];
        for k in 0..4.min(bones) {
            j[k] = order[k] as u16;
            // Drop tiny weights so joints don't drag far-away skin.
            wt[k] = if row[order[k]] > 0.01 {
                row[order[k]]
            } else {
                0.0
            };
        }
        let sum: f32 = wt.iter().sum();
        if sum > 0.0 {
            wt.iter_mut().for_each(|x| *x /= sum);
        } else {
            wt = [1.0, 0.0, 0.0, 0.0];
        }
        joints.push(j);
        weights.push(wt);
    }
    let colors = mesh
        .occlusion
        .iter()
        .map(|o| {
            let a = occlusion_floor + (1.0 - occlusion_floor) * o;
            [a, a, a, 1.0]
        })
        .collect();
    SkinTemplate {
        positions: mesh.positions,
        normals: mesh.normals,
        colors,
        indices: mesh.triangles.into_iter().flatten().collect(),
        joints,
        weights,
        bind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Culling (bounding spheres and boxes) must never change the field: a part skipped at
    /// one point and blended at the next would leave a crease in the surface.
    #[test]
    fn culling_never_changes_the_fields() {
        for (name, sculpt) in [("mannequin", human_sculpt()), ("dog", dog_sculpt())] {
            let full = sculpt.without_culling();
            let (lo, hi) = sculpt.bounds();
            let mut seed = 0x2545_f491_u32;
            let mut rand = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32
            };
            let mut near = 0;
            for _ in 0..200_000 {
                let p = lo + (hi - lo) * Vec3::new(rand(), rand(), rand());
                let (d, b) = sculpt.evaluate(p);
                if d.abs() > 0.03 {
                    continue;
                }
                near += 1;
                let (e, c) = full.evaluate(p);
                assert!((d - e).abs() < 1e-6, "{name}: {d} vs {e} at {p}");
                assert_eq!(b, c, "{name}: bone at {p}");
            }
            assert!(near > 1000, "{name}: {near} samples near the surface");
        }
    }
}

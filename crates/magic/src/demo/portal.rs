//! `spells_portal` (round-9-plan.md, "The hall stop"): the warden of the second lane opens a
//! portal on the hall floor in front of the students, then its other end far down the hall,
//! and fires bolts through it into a dummy beyond the far end; a storm beam from the first lane
//! bends through the pair into another; a push from the third lane shoves a crate into the near
//! end and it slides out of the far one; on the fourth plinth a mage blinks off the dais, then
//! again, and recalls back along its path.
//!
//! The dummies stand where each lane's line through the near end's middle comes out of the far
//! end ([`exit_spot`]), so what goes in visibly lands.

use engine_core::math::{DQuat, DVec3, Vec2, Vec3};
use engine_core::prelude::*;

use super::art::*;
use super::{AimAt, Cast, Lane, d, lane};
use crate::design::{CastType, Modifier, SpellDesign};
use crate::space::{GateFrame, carry, carry_vector};

/// The near end (x, z from the hall's centre): on the floor in front of the students.
pub const NEAR: [f64; 2] = [-6.0, 2.0];
/// The far end, down the hall to the right.
pub const FAR: [f64; 2] = [6.0, -3.0];
/// The warden who opens them (and fires bolts through them).
pub const WARDEN: usize = 1;
/// The lane that blinks and recalls.
pub const BLINKER: usize = 3;
/// Where the blinker blinks to, in turn (x, z): off the dais into the open floor between the
/// two ends, in the camera's view.
pub const BLINKS: [[f64; 2]; 2] = [[1.0, 5.0], [-1.5, 0.5]];
/// An end's size, m (a plain portal's).
pub const SIZE: [f32; 2] = [1.4, 2.4];
/// How far each lane's dummy stands beyond the far end, m, and the crate in front of the near
/// end.
pub const DUMMY_M: [(usize, f64); 3] = [(WARDEN, 5.5), (0, 4.6), (2, 5.0)];
pub const CRATE_M: f64 = 1.6;
/// The ends' life: they close before the next period opens a new pair.
pub const DURATION: f32 = 0.75;

/// A lane's hand (on its plinth, facing down the hall).
fn hand(lane: usize) -> DVec3 {
    DVec3::new(LANES[lane], PLINTH_TOP, MAGE_Z) + Vec3::new(0.25, 1.35, -0.45).as_dvec3()
}

fn flat(v: DVec3) -> DVec3 {
    DVec3::new(v.x, 0.0, v.z).normalize_or(DVec3::NEG_Z)
}

/// The two ends' frames (from the hall's centre), as the warden's casts open them: the near
/// one facing the warden, the far one facing away from the warden.
pub fn frames() -> (GateFrame, GateFrame) {
    let warden = DVec3::new(LANES[WARDEN], PLINTH_TOP, MAGE_Z);
    let size = Vec2::from(SIZE);
    let near = DVec3::new(NEAR[0], FLOOR + SIZE[1] as f64 * 0.5, NEAR[1]);
    let far = DVec3::new(FAR[0], FLOOR + SIZE[1] as f64 * 0.5, FAR[1]);
    let frame = |at: DVec3, n: DVec3| GateFrame {
        at,
        rot: DQuat::from_rotation_y(n.x.atan2(n.z)),
        size,
    };
    (
        frame(near, flat(warden - near)),
        frame(far, flat(far - warden)),
    )
}

/// Where `lane`'s line through the near end's middle comes out of the far end, `m` beyond it,
/// on the floor (x, z from the hall's centre).
pub fn exit_spot(lane: usize, m: f64) -> [f64; 2] {
    let (near, far) = frames();
    let dir = (near.at - hand(lane)).as_vec3().normalize();
    let out = flat(carry_vector(&carry(&near, &far), dir).as_dvec3());
    let p = far.at + out * m;
    [p.x, p.z]
}

/// Where the crate waits: on the third lane's line into the near end, `CRATE_M` short of it.
pub fn crate_spot() -> [f64; 2] {
    let (near, _) = frames();
    let back = flat(hand(2) - near.at);
    let p = near.at + back * CRATE_M;
    [p.x, p.z]
}

/// The point to aim at: the near end's middle.
fn into_near() -> AimAt {
    let (near, _) = frames();
    AimAt::At(near.at.x, near.at.y, near.at.z)
}

fn portal() -> SpellDesign {
    d(&["portal"], CastType::Area)
        .with(Modifier::Duration { factor: DURATION })
        .named("Portal")
}

/// The program's lanes.
pub(super) fn lanes() -> Vec<Lane> {
    let cast = |at: f32, design: SpellDesign, aim: AimAt| Cast {
        at,
        design: Some(design),
        aim,
    };
    let casts = |casts: Vec<Cast>| Lane {
        casts,
        ..lane(SpellDesign::default())
    };
    let fire = || d(&["fire"], CastType::Bolt).named("Fire bolt");
    let blink = || d(&["blink"], CastType::OnSelf).named("Blink");
    let recall = || d(&["recall"], CastType::OnSelf).named("Recall");
    let mut warden = vec![
        cast(0.1, portal(), AimAt::Floor(NEAR[0], NEAR[1])),
        cast(0.8, portal(), AimAt::Floor(FAR[0], FAR[1])),
    ];
    for at in [2.0, 2.8, 3.6, 4.4, 10.6, 11.4, 12.2] {
        warden.push(cast(at, fire(), into_near()));
    }
    let blinker = |t: f32| {
        vec![
            cast(t, blink(), AimAt::Floor(BLINKS[0][0], BLINKS[0][1])),
            cast(t + 0.9, blink(), AimAt::Floor(BLINKS[1][0], BLINKS[1][1])),
            cast(t + 2.5, recall(), AimAt::Focus),
        ]
    };
    let mut blinks = blinker(2.4);
    blinks.extend(blinker(8.6));
    vec![
        // A storm beam bent through the pair.
        casts(vec![cast(
            5.6,
            d(&["storm"], CastType::Beam).named("Storm beam"),
            into_near(),
        )]),
        casts(warden),
        // A push that shoves the crate in.
        casts(vec![cast(
            8.2,
            d(&["push"], CastType::Bolt).named("Push"),
            AimAt::Focus,
        )]),
        casts(blinks),
        casts(Vec::new()),
        casts(Vec::new()),
    ]
}

/// The program's props: a dummy beyond the far end for each lane that sends something through,
/// and the crate.
pub fn spawn_props(commands: &mut Commands, origin: DVec3) {
    for (k, (lane, m)) in DUMMY_M.iter().enumerate() {
        let [x, z] = exit_spot(*lane, *m);
        let e = spawn_dummy(
            commands,
            origin + DVec3::new(x, FLOOR, z),
            6 + k,
            false,
            false,
        );
        commands.entity(e).insert(ProgramProp);
    }
    let [x, z] = crate_spot();
    spawn_crate(
        commands,
        origin + DVec3::new(x, FLOOR + 0.38, z),
        2,
        true,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dummies_stand_apart_beyond_the_far_end_and_the_crate_before_the_near_one() {
        let (near, far) = frames();
        // The near end faces the warden, the far end faces away from it.
        let warden = DVec3::new(LANES[WARDEN], PLINTH_TOP, MAGE_Z);
        assert!(near.local(warden).z > 0.0);
        assert!(far.local(warden).z < 0.0);
        let spots: Vec<[f64; 2]> = DUMMY_M.iter().map(|(l, m)| exit_spot(*l, *m)).collect();
        for (i, a) in spots.iter().enumerate() {
            let p = DVec3::new(a[0], far.at.y, a[1]);
            assert!(far.local(p).z > 3.0, "beyond the far end: {a:?}");
            assert!(
                a[0].abs() < 20.0 && (-26.0..16.0).contains(&a[1]),
                "in the hall"
            );
            for b in &spots[i + 1..] {
                let gap = DVec3::new(a[0] - b[0], 0.0, a[1] - b[1]).length();
                assert!(gap > 1.5, "{a:?} {b:?}");
            }
        }
        // The crate waits in front of the near end, on the third lane's line into its middle.
        let c = crate_spot();
        let l = near.local(DVec3::new(c[0], near.at.y, c[1]));
        assert!(l.z > 1.0 && (l.length() - CRATE_M).abs() < 1e-6, "{l}");
    }
}

//! The body a person's hair is held out of (`hair_body`) against the skin it was measured
//! from: at rest, with the head turned and with the arms lowered, on the skin, over it and
//! under it. Needs the fetched human pack (`cargo run -p fetch-assets`); passes with a note
//! without it.

use std::path::Path;

use engine_character::{
    Appearance, AppearanceExt, BuildOptions, HumanPack, build, hair_body, hair_motion, mesh,
};
use engine_core::prelude::*;

fn pack() -> Option<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(engine_character::pack::PACK_PATH);
    let bytes = std::fs::read(path).ok();
    if bytes.is_none() {
        eprintln!("skipped: the human pack isn't fetched (cargo run -p fetch-assets)");
    }
    bytes
}

/// The `share` quantile of `values`.
fn at_share(values: &mut [f32], share: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    values[((values.len() - 1) as f32 * share) as usize]
}

#[test]
fn the_hair_body_is_the_dressed_skin_at_rest_under_a_turned_head_and_with_the_arms_down() {
    let Some(bytes) = pack() else {
        return;
    };
    let pack = HumanPack::parse(&bytes).expect("the human pack");
    let regions = mesh::Regions::new(&pack);
    // Someone dressed whose hair moves.
    let (look, meshes, pose, guides) = Appearance::presets()
        .into_iter()
        .find_map(|(_, look)| {
            let meshes = build::build(&pack, &regions, &look, BuildOptions::default());
            let pose = meshes.rig.as_ref()?.rest_pose();
            let guides = hair_motion::hair_guides(&meshes, &pose, 1)?;
            Some((look, meshes, pose, guides))
        })
        .expect("a preset with hair that moves");
    let body = hair_body::hair_body(&meshes, &look, &pose, &guides).expect("a body");
    assert!(body.parts.len() >= 3 && body.parts.len() <= hair_body::MOST_PARTS);
    let drawn = hair_body::drawn_surface(&meshes, &look, &pose);

    let (neck, head, chest) = (
        pose.index_of("neck").unwrap(),
        pose.index_of("head").unwrap(),
        pose.index_of("chest").unwrap(),
    );
    let under = |mut b: usize, of: usize| loop {
        if b == of {
            return true;
        }
        match pose.parents[b] {
            Some(p) => b = usize::from(p),
            None => return false,
        }
    };
    let top = guides
        .guides
        .iter()
        .map(|g| g.points[0].y)
        .fold(f32::MAX, f32::min);
    // The head and neck as a film turns them (neck 35%, head the rest), the skin carried by
    // its bones' weights.
    // (And the arms as someone standing holds them, 24 degrees down from how the hair was
    // grown: the skin of the shoulders is shared between the arms and the trunk.)
    let arm = |side: &str| {
        let joint = pose.index_of(&format!("upper_arm_{side}")).unwrap();
        let limb: Vec<bool> = (0..pose.bones.len())
            .map(|b| {
                under(b, joint) || (pose.names[b].contains("arm") && pose.names[b].ends_with(side))
            })
            .collect();
        (pose.bones[joint].translation, limb)
    };
    let (arm_l, arm_r) = (arm("l"), arm("r"));
    for (name, yaw, pitch, roll, arms) in [
        ("rest", 0.0, 0.0, 0.0, 0.0),
        ("turned", 0.78f32, 0.14, 0.24, 0.0),
        ("arms down", 0.0, 0.0, 0.0, 0.42f32),
        ("turned, arms down", 0.78, 0.14, 0.24, 0.42),
    ] {
        let lowered = |(joint, _): &(Vec3, Vec<bool>)| {
            // (Toward the body: the way the joint's side of the trunk is on.)
            let turn = Quat::from_rotation_z(-arms * joint.x.signum());
            Mat4::from_translation(*joint) * Mat4::from_quat(turn) * Mat4::from_translation(-*joint)
        };
        let (t_arm_l, t_arm_r) = (lowered(&arm_l), lowered(&arm_r));
        let whole = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
        let (qn, qh) = (
            Quat::IDENTITY.slerp(whole, 0.35),
            Quat::IDENTITY.slerp(whole, 0.65),
        );
        let (pn, ph) = (pose.bones[neck].translation, pose.bones[head].translation);
        let t_neck = Mat4::from_translation(pn) * Mat4::from_quat(qn) * Mat4::from_translation(-pn);
        let t_head = Mat4::from_translation(t_neck.transform_point3(ph))
            * Mat4::from_quat(qh * qn)
            * Mat4::from_translation(-ph);
        let moved = |b: u16| {
            if arm_l.1[usize::from(b)] {
                t_arm_l
            } else if arm_r.1[usize::from(b)] {
                t_arm_r
            } else if under(usize::from(b), head) {
                t_head
            } else if under(usize::from(b), neck) {
                t_neck
            } else {
                Mat4::IDENTITY
            }
        };
        let posed = |d: &hair_body::DrawnPoint| {
            let (mut at, mut out, mut total) = (Vec3::ZERO, Vec3::ZERO, 0.0);
            for k in 0..4 {
                let w = f32::from(d.weights[k]);
                if w > 0.0 {
                    let t = moved(d.bones[k]);
                    at += t.transform_point3(d.at) * w;
                    out += t.transform_vector3(d.out) * w;
                    total += w;
                }
            }
            (total > 0.0).then(|| (at / total, out.normalize_or(Vec3::Y)))
        };
        // On the skin the hair lies on (head, neck, shoulders): the distance is zero there,
        // within the lattice's 8 mm samples.
        // (With the arms down, above the armpits: there the arm's skin is skinned into the
        // side's.)
        let from = if arms > 0.0 {
            arm_l.0.y - 0.03
        } else {
            top - 0.4
        };
        let mut on: Vec<f32> = drawn
            .iter()
            .filter(|d| d.at.y > from)
            .filter_map(posed)
            .filter_map(|(p, _)| hair_body::at(&body, moved, p))
            .map(|(d, _)| d * 1000.0)
            .collect();
        assert!(on.len() > 1000, "{name}: {} points", on.len());
        let (low, high) = (at_share(&mut on, 0.01), at_share(&mut on, 0.99));
        eprintln!("{name}: on the skin {low:.1} to {high:.1} mm (1% to 99%)");
        assert!(
            low > -4.0 && high < 4.0,
            "{name}: on the skin {low} to {high} mm"
        );
        // Over the shoulders and the neck, where the hair hangs, it is outside (with the arms
        // down a part that carried its own skin alone swung a piece of shoulder up there).
        let mut over: Vec<f32> = drawn
            .iter()
            .filter(|d| d.at.y > arm_l.0.y - 0.03 && d.at.y < arm_l.0.y + 0.12)
            .filter_map(posed)
            .filter_map(|(p, n)| hair_body::at(&body, moved, p + n * 0.02))
            .map(|(d, _)| d * 1000.0)
            .collect();
        assert!(over.len() > 300, "{name}: {} shoulder points", over.len());
        let (few, half) = (at_share(&mut over, 0.02), at_share(&mut over, 0.5));
        eprintln!("{name}: 20 mm over the shoulders reads {few:.1} (2%) and {half:.1} (half) mm");
        assert!(
            few > 10.0 && half > 15.0,
            "{name}: 20 mm over the shoulders reads {few} (2%) and {half} (half) mm"
        );
        // Under the chest's skin and clothes it is inside, and over them outside (a pocket
        // behind the sweater once read as outside, and locks lay in it).
        for (h, name_of) in [(-0.02f32, "under"), (0.02, "over")] {
            let mut off: Vec<f32> = drawn
                .iter()
                .filter(|d| {
                    // (Not the chest's sides under lowered arms: over them is the arm.)
                    d.at.y > top - 0.4
                        && (arms == 0.0 || d.at.x.abs() < 0.1)
                        && (0..4).any(|k| usize::from(d.bones[k]) == chest && d.weights[k] >= 200)
                })
                .filter_map(posed)
                .filter_map(|(p, n)| hair_body::at(&body, moved, p + n * h))
                .map(|(d, _)| d * 1000.0 * h.signum())
                .collect();
            assert!(off.len() > 200, "{name}: {} chest points", off.len());
            let (few, half) = (at_share(&mut off, 0.05), at_share(&mut off, 0.5));
            assert!(
                few > 8.0 && half > 15.0,
                "{name}: 20 mm {name_of} the chest reads {few} (5%) and {half} (half) mm"
            );
        }
    }
}

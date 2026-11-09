//! Skins the base mesh on the CPU in a hand shape and writes the posed points, for checking
//! finger poses without the GPU (plot them with any point plotter).
//!
//! `cargo run -p engine_character --release --example pose_dump -- <pack> <out.txt> [shape] [weight]`
//! writes `x y z part` per body vertex near the hands (part 0 skin, 1..5 thumb..pinky), in
//! the humanoid's model space (facing −Z, +X right).

use engine_character::HumanPack;
use engine_character::hands;
use engine_character::rig::{BoneKind, Rig};
use engine_world::character::{HandControls, HandPose};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("pack");
    let mut pack = HumanPack::parse(&bytes).expect("parse");
    assert!(engine_character::skin::upgrade(
        &mut pack,
        &engine_character::skin::baked()
    ));
    let shape = args.get(3).map(String::as_str).unwrap_or("Relaxed");
    let weight: f32 = args.get(4).and_then(|w| w.parse().ok()).unwrap_or(1.0);
    let shape = hands::SHAPES
        .into_iter()
        .find(|s| hands::shape_name(*s).eq_ignore_ascii_case(shape))
        .expect("shape");
    let rig = Rig::new(&pack, &pack.positions).expect("rig");
    let pose = rig.rest_pose();
    // A pose without finger bones (as the animation module's): the hands pose them.
    let map: Vec<Option<usize>> = rig
        .map(&pose)
        .into_iter()
        .zip(&rig.kind)
        .map(|(m, k)| {
            if matches!(k, BoneKind::Finger(..)) {
                None
            } else {
                m
            }
        })
        .collect();
    let hp = HandPose {
        shape,
        weight,
        ..Default::default()
    };
    let controls = HandControls {
        grip: [0.0; 2],
        hands: [hp, hp],
    };
    let mut palette = Vec::new();
    rig.palette_with(&pose, &map, Some(&controls), &mut palette);
    let body = pack.group("body").unwrap();
    let mut seen = vec![false; pack.positions.len()];
    let mut out = String::new();
    for q in pack.quads.iter().filter(|q| q.group == body) {
        for v in q.v {
            let v = v as usize;
            if std::mem::replace(&mut seen[v], true) {
                continue;
            }
            let w = pack.skin[v];
            let mut p = engine_core::math::Vec3::ZERO;
            let mut part = 0;
            let mut best = 0;
            for k in 0..4 {
                let b = w.bones[k] as usize;
                let wt = w.weights[k] as f32 / 255.0;
                p += palette[b].transform_point3(pack.positions[v]) * wt;
                if w.weights[k] > best {
                    best = w.weights[k];
                    part = match rig.kind[b] {
                        BoneKind::Finger(f, _, _) => f + 1,
                        _ => 0,
                    };
                }
            }
            // Only the hands and forearms.
            if pack.positions[v].y > 0.38 || pack.positions[v].x.abs() < 0.28 {
                continue;
            }
            out.push_str(&format!("{} {} {} {}\n", p.x, p.y, p.z, part));
        }
    }
    std::fs::write(&args[2], out).expect("write");
}

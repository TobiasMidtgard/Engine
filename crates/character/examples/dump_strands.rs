//! Debug: `cargo run -p engine_character --example dump_strands -- <preset> <out.json>` writes
//! every strand set's points and the skin's scalp mask, for plotting.

use engine_character::{Appearance, BuildOptions, HumanPack, build, mesh};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let preset = args.get(1).map(String::as_str).unwrap_or("Astrid");
    let out = args.get(2).map(String::as_str).unwrap_or("strands.json");
    let bytes =
        std::fs::read(format!("assets/{}", engine_character::pack::PACK_PATH)).expect("pack");
    let pack = HumanPack::parse(&bytes).expect("parse");
    let regions = mesh::Regions::new(&pack);
    println!(
        "rims: {} {}",
        regions.lid_margins[0][0].len(),
        regions.lid_margins[0][1].len()
    );
    let a = Appearance::preset(preset).expect("preset");
    let m = build::build(&pack, &regions, &a, BuildOptions::default());
    if let Some(lm) = m.landmarks {
        println!("head {:?} eyes {:?}", lm.head, lm.eyes);
    }
    let sets: Vec<serde_json::Value> = m
        .strands
        .iter()
        .map(|s| {
            serde_json::json!({
                "kind": format!("{:?}", s.kind),
                "per": s.points_per,
                "points": s.points.iter().map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>(),
            })
        })
        .collect();
    let skin: Vec<[f32; 4]> = m
        .skin
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            [
                p.x,
                p.y,
                p.z,
                m.skin.masks[i * mesh::MASKS + mesh::Mask::Scalp as usize],
            ]
        })
        .collect();
    let collider = engine_character::strands::SkinCollider::new(&m.skin);
    for set in &m.strands {
        let mut below = 0;
        let mut hist = [0usize; 6];
        for p in &set.points {
            let q = engine_core::math::Vec3::new(p[0], p[1], p[2]);
            if let Some((i, _)) = collider.nearest(q) {
                let h = (q - m.skin.positions[i]).dot(m.skin.normals[i]);
                if h < 0.0003 {
                    below += 1;
                }
                let b = match h {
                    h if h < 0.0 => 0,
                    h if h < 0.0005 => 1,
                    h if h < 0.001 => 2,
                    h if h < 0.003 => 3,
                    h if h < 0.01 => 4,
                    _ => 5,
                };
                hist[b] += 1;
            }
        }
        println!(
            "{:?}: {} points, {} below 0.3 mm, hist <0 <.5 <1 <3 <10 >10 mm: {:?}",
            set.kind,
            set.points.len(),
            below,
            hist
        );
    }
    std::fs::write(
        out,
        serde_json::json!({"sets": sets, "skin": skin}).to_string(),
    )
    .expect("write");
}

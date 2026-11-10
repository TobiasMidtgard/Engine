//! Bytes a built person keeps on the CPU, by part.
//! `cargo run -p engine_character --release --example mem_dump -- assets/fetched/character/human_hm08_v2.bin`

use engine_character::{AppearanceExt, CharacterBuild, HumanPack, build, mesh, skin};
use engine_world::character::Appearance;

fn v<T>(x: &[T]) -> usize {
    std::mem::size_of_val(x)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("pack");
    let mut pack = HumanPack::parse(&bytes).expect("parse");
    skin::upgrade(&mut pack, &skin::baked());
    let regions = mesh::Regions::new(&pack);
    for name in ["Astrid", "Kofi"] {
        let a = Appearance::preset(name).unwrap();
        let mut m = build::build(&pack, &regions, &a, CharacterBuild::default().options());
        if args.get(2).is_some_and(|a| a == "release") {
            m.release_cpu();
        }
        let s = &m.skin;
        let skin = v(&s.positions)
            + v(&s.normals)
            + v(&s.rest)
            + v(&s.masks)
            + v(&s.curvature)
            + v(&s.indices)
            + v(&s.bones)
            + v(&s.weights)
            + v(&s.base)
            + v(&s.base_quads)
            + v(&s.extra)
            + v(&s.extra_parts)
            + v(&s.coarse_indices);
        let strands: usize = m
            .strands
            .iter()
            .map(|x| v(&x.points) + v(&x.attrs) + v(&x.guides) + v(&x.follow))
            .sum();
        let packed: usize = m
            .packed_strands
            .iter()
            .map(|x| v(&x.points) + v(&x.attrs))
            .sum();
        let roots: usize = m.strand_roots.iter().map(|x| v(x)).sum::<usize>()
            + m.guide_roots.iter().map(|x| v(x)).sum::<usize>();
        let face = v(&m.face.offsets) + v(&m.face.entries);
        let eyes = v(&m.eyes.indices);
        let mb = |b: usize| b as f64 / 1e6;
        println!(
            "{name}: skin {:.1} MB ({} verts), strands {:.1}, packed {:.1}, roots {:.1}, face {:.1}, eyes idx {:.1}",
            mb(skin),
            s.positions.len(),
            mb(strands),
            mb(packed),
            mb(roots),
            mb(face),
            mb(eyes)
        );
    }
}

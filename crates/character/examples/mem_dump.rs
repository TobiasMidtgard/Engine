//! Bytes a built person keeps on the CPU, by part.
//! `cargo run -p engine_character --release --example mem_dump -- assets/fetched/character/human_hm08_v2.bin`

use engine_character::{AppearanceExt, CharacterBuild, HumanPack, build, face, mesh, skin};
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
        if let Some(g) = &m.hair_shadow {
            println!(
                "  hair grid {:?} cells, {:.1} MB",
                g.dims,
                mb(g.density.len())
            );
        }
        // Mask ranges per channel (they go to the GPU as unorm bytes).
        let (mut lo, mut hi) = ([f32::MAX; 12], [f32::MIN; 12]);
        for c in s.masks.chunks(12) {
            for k in 0..12 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        println!("  masks lo {lo:?}\n  masks hi {hi:?}");
        // Expression entries by size of their offsets.
        let edges_mm = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 1e9];
        let mut hist = [0usize; 8];
        let (mut small, mut dn_max) = (0usize, 0f32);
        for e in &m.face.entries {
            let (_, dp, dn) = face::unpack(e);
            let mm = dp.length() * 1000.0;
            hist[edges_mm.iter().position(|&x| mm < x).unwrap()] += 1;
            if mm < 0.05 && dn.length() < 4e-3 {
                small += 1;
            }
            dn_max = dn_max.max(dn.abs().max_element());
        }
        let moved = m.face.offsets.windows(2).filter(|w| w[1] > w[0]).count();
        println!(
            "  shapes: {} entries on {} vertices, by |dp| mm <{edges_mm:?}: {hist:?}; dp<0.05mm & dn<4e-3: {small}; max |dn| {dn_max}",
            m.face.entries.len(),
            moved
        );
    }
}

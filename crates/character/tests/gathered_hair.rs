//! The gathered hairstyles as grown on a built person: a half-up style's knot stands on the
//! head over hair that hangs and moves, and a ponytail's tail hangs from its tie and moves
//! while the hair combed into the tie holds its shape. Needs the fetched human pack
//! (`cargo run -p fetch-assets`); passes with a note without it.

use std::path::Path;

use engine_character::strands::{StrandKind, StrandSet};
use engine_character::{Appearance, BuildOptions, HumanPack, build, mesh};
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

/// The scalp hair of the default person with `style`.
fn scalp(pack: &HumanPack, style: &str) -> StrandSet {
    let mut look = Appearance::default();
    look.hair.style = style.into();
    look.hair.curl = 0.3;
    let regions = mesh::Regions::new(pack);
    build::build(pack, &regions, &look, BuildOptions::default())
        .strands
        .into_iter()
        .find(|s| s.kind == StrandKind::Scalp)
        .expect("scalp hair")
}

/// One hair's points.
fn hair(set: &StrandSet, h: usize) -> impl Iterator<Item = Vec3> + '_ {
    let per = set.points_per as usize;
    set.points[h * per..(h + 1) * per]
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
}

/// Hairs that move with guides, and hairs that hold their shape.
fn moving_and_held(set: &StrandSet) -> (Vec<usize>, Vec<usize>) {
    assert_eq!(set.follow.len(), set.strand_count(), "a follow per hair");
    (0..set.strand_count()).partition(|h| set.follow[*h].weights[0] > 0.0)
}

#[test]
fn a_half_up_style_has_a_knot_on_the_head_over_hair_that_hangs_and_moves() {
    let Some(bytes) = pack() else {
        return;
    };
    let pack = HumanPack::parse(&bytes).expect("the human pack");
    let long = scalp(&pack, "long");
    let set = scalp(&pack, "halfup");
    let top = |s: &StrandSet| s.points.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    let roots_top = (0..long.strand_count())
        .map(|h| long.points[h * long.points_per as usize][1])
        .fold(f32::MIN, f32::max);
    // The knot stands on the head: centimetres above the highest root, and well above where
    // long hair lies.
    let (lies, stands) = (top(&long) - roots_top, top(&set) - roots_top);
    assert!(lies < 0.02, "long hair lies {lies} m over the highest root");
    assert!(
        stands > 0.025 && stands > lies + 0.012,
        "the knot stands {stands} m over the highest root, long hair {lies}"
    );
    let (moving, held) = moving_and_held(&set);
    assert!(set.guide_count() >= 60, "{} guides", set.guide_count());
    assert!(set.tied.iter().all(|t| *t == 0), "loose hair is not tied");
    assert!(moving.len() > 4000, "{} hairs hang", moving.len());
    assert!(
        held.len() > 2 * moving.len(),
        "{} hairs are gathered",
        held.len()
    );
    // What moves is rooted at the back of the head and hangs well below it; what is gathered
    // ends in the knot, and none of it hangs.
    let lowest = |h: usize| hair(&set, h).map(|p| p.y).fold(f32::MAX, f32::min);
    let hangs = moving
        .iter()
        .filter(|h| lowest(**h) < roots_top - 0.3)
        .count();
    assert!(
        hangs * 2 > moving.len(),
        "{hangs} of {} hang long",
        moving.len()
    );
    let hanging_held = held
        .iter()
        .filter(|h| lowest(**h) < roots_top - 0.24)
        .count();
    assert_eq!(hanging_held, 0, "gathered hair hangs");
    // (Front to back, the head's middle is about where the gathered hair's roots are.)
    let middle = held
        .iter()
        .map(|h| hair(&set, *h).next().unwrap().z)
        .sum::<f32>()
        / held.len() as f32;
    let front_roots = moving
        .iter()
        .filter(|h| hair(&set, **h).next().unwrap().z > middle)
        .count();
    assert_eq!(front_roots, 0, "hair of the front hangs loose");
}

#[test]
fn a_ponytail_hangs_from_its_tie_and_the_hair_combed_into_it_holds() {
    let Some(bytes) = pack() else {
        return;
    };
    let pack = HumanPack::parse(&bytes).expect("the human pack");
    let set = scalp(&pack, "ponytail");
    let per = set.points_per as usize;
    let (moving, held) = moving_and_held(&set);
    assert!(moving.len() > 5000 && held.len() > 10_000);
    // The tail's guides leave one tie, within a few centimetres of each other, and end far
    // below it.
    let guides = set.guide_count();
    assert!((24..=96).contains(&guides), "{guides} guides");
    assert_eq!(
        set.tied,
        vec![1; guides],
        "the tail's guides are tied together"
    );
    let root = |g: usize| set.guides[g * per];
    let tip = |g: usize| set.guides[g * per + per - 1];
    let tie = (0..guides).map(root).sum::<Vec3>() / guides as f32;
    for g in 0..guides {
        assert!(root(g).distance(tie) < 0.03, "guide {g} leaves the tie");
        assert!(tip(g).y < tie.y - 0.2, "guide {g} hangs");
    }
    // Every hair of the tail starts at the tie; every held hair ends near it (combed into it
    // or wound round it) or short of it (the hairline's short hairs), and none hangs.
    for &h in &moving {
        let start = hair(&set, h).next().unwrap();
        assert!(start.distance(tie) < 0.04, "a tail hair starts at the tie");
    }
    let low = held
        .iter()
        .filter(|h| hair(&set, **h).any(|p| p.y < tie.y - 0.26))
        .count();
    assert_eq!(low, 0, "held hair hangs");
    let into_tie = held
        .iter()
        .filter(|h| hair(&set, **h).last().unwrap().distance(tie) < 0.05)
        .count();
    assert!(
        into_tie * 10 > held.len() * 8,
        "{into_tie} of {} reach the tie",
        held.len()
    );
    // Along the hairline, baby hairs a centimetre or two long lie on the skin.
    let length = |h: usize| {
        let pts: Vec<Vec3> = hair(&set, h).collect();
        pts.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>()
    };
    let baby = held.iter().filter(|h| length(**h) < 0.026).count();
    assert!(baby > 200, "{baby} baby hairs");
}

//! The Spell court's props. Each function returns a [`Prop`] in its own space (origin on the
//! ground at the prop's center, +Y up, -Z its front unless noted); `spawn_demo` places them.
//!
//! - [`court`]: the paved floor with its rune ring and medallion.
//! - [`portal`]: the arcane gate in the north.
//! - [`lane`]: the caster's pedestal and the target wall.
//! - [`storm`]: the storm obelisks.
//! - [`water`]: the healing dais, the fountain and the pond.
//! - [`forge`]: the anvil on its stump.
//! - [`beds`]: footstep patches and impact plates.
//! - [`rim`]: braziers and shrubs.
//! - [`launcher`]: the firework mortar.
//! - [`stand`]: the rocket motor test stand outside the north-east rim.
//! - [`golem`]: the walking stone golem and the dropped stones.

pub mod beds;
pub mod court;
pub mod forge;
pub mod golem;
pub mod lane;
pub mod launcher;
pub mod portal;
pub mod rim;
pub mod stand;
pub mod storm;
pub mod water;

use engine_core::math::{Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::meshes::{cavity, lathe, mottle, uv_box};
use super::{Prop, Surface};

/// A round stone plinth, `radius` wide and `height` tall, with a molded rim.
pub fn plinth(radius: f32, height: f32) -> Prop {
    let r = radius;
    let h = height;
    let profile = [
        Vec2::new(0.0, 0.0),
        Vec2::new(r, 0.0),
        Vec2::new(r, h * 0.15),
        Vec2::new(r * 0.9, h * 0.25),
        Vec2::new(r * 0.85, h * 0.8),
        Vec2::new(r * 0.95, h * 0.9),
        Vec2::new(r * 0.95, h),
        Vec2::new(0.0, h),
    ];
    Prop::new().with(Surface::CarvedStone, lathe(&profile, 48))
}

/// Finish a piece of carved stone: box-projected UVs (shifted per piece so neighbors differ),
/// dirt in creases and lighter worn edges, gentle mottling.
pub fn dress(m: &mut Mesh, seed: u64, wear: f32) {
    let mut rng = Rng::new(seed);
    uv_box(m, Vec2::new(rng.range(0.0, 17.0), rng.range(0.0, 17.0)));
    cavity(m, 0.45 * wear, 0.18 * wear, 5.0);
    mottle(m, 1.3, 0.07, seed);
}

/// A random shade for one stone among many: brightness and a slight warm or cool cast.
pub fn shade(rng: &mut Rng, spread: f32) -> Vec3 {
    let b = 1.0 + rng.range(-spread, spread * 0.6);
    let warm = rng.range(-0.05, 0.05);
    Vec3::new(1.0 + warm, 1.0, 1.0 - warm * 1.2) * b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every prop the court spawns, with how many copies it places, for the budget check.
    fn court_props() -> Vec<(&'static str, Prop, usize)> {
        vec![
            ("floor", court::floor(21.0, &crate::demo::clearings()), 1),
            ("portal", portal::gate(), 1),
            ("summoning", portal::summoning_circle(), 1),
            ("caster", lane::caster_pedestal(), 1),
            ("crystal", lane::focus_crystal(), 1),
            ("wall", lane::target_wall(), 1),
            ("obelisk", storm::obelisk(1), 2),
            ("dais", water::dais(), 1),
            ("fountain", water::fountain(), 1),
            ("pond", water::pond(), 1),
            ("anvil", forge::anvil(), 1),
            ("patches", beds::footstep_patches(), 1),
            ("plates", beds::impact_plates(), 1),
            ("brazier", rim::brazier(), 8),
            ("shrubs", rim::shrubs(0), 4),
            ("golem", golem::golem_body(), 1),
            ("golem arm", golem::golem_arm(), 2),
            ("golem leg", golem::golem_leg(), 2),
            ("stone", golem::dropped_stone(0), 6),
            ("stand", stand::engine_stand(), 1),
        ]
    }

    #[test]
    fn props_are_well_formed_and_within_budget() {
        let mut total = 0;
        for (name, prop, copies) in court_props() {
            assert!(!prop.parts.is_empty(), "{name} is empty");
            for part in &prop.parts {
                let n = part.mesh.vertices.len() as u32;
                assert!(
                    part.mesh.indices.iter().all(|&i| i < n),
                    "{name}: index out of range"
                );
                assert!(
                    part.mesh.vertices.iter().all(|v| v
                        .position
                        .iter()
                        .chain(&v.normal)
                        .all(|x| x.is_finite())),
                    "{name}: NaN"
                );
            }
            let tris = prop.triangle_count();
            println!("{name:>10}: {tris:>8} triangles × {copies}");
            total += tris * copies;
        }
        println!("     total: {total:>8} triangles");
        assert!(total < 1_500_000, "the court has {total} triangles");
    }
}

//! The year in the forest: deciduous trees and shrubs leaf out in spring in fresh yellow-green,
//! turn their species' autumn color, drop their leaves and stand bare through winter; bracken
//! rusts and dies back to the ground, summer flowers go to seed and vanish, and meadow grass
//! turns to straw.
//!
//! Plants show [`FoliageSeason::day_of_year`]: midsummer unless set (`foliage.season`), or the
//! weather's calendar (`WeatherState::day_of_year`) with `follow_weather`. Neighbours don't
//! turn in step: each of a species' three leaf tints runs a week apart and every plant adds a
//! few days of its own, so a wood colors and thins patchily.
//!
//! - Leaf color: each species' leaf materials ([`apply_season_colors`]).
//! - Leaf fall and leaf-out: trees and shrubs switch between their full crown and thinned
//!   copies of it ([`crate::meshgen::tree::LEAF_STEPS`]); bare, they show their branches.
//! - Ferns and flowers: each plant disappears once its share of the year's growth drops below
//!   its own threshold ([`Leafing::shows`]).
//! - Meadow grass: [`grass_dryness`] browns the GPU grass toward straw.
//! - The forest floor: freshly fallen leaves color the leaf litter (and the leaf clumps on it)
//!   in autumn, and it lies dark and sodden through winter ([`litter_season`]).

use engine_assets::Assets;
use engine_core::math::{hash01, smoothstep};
use engine_core::prelude::*;
use engine_render::StandardMaterial;
use engine_world::WeatherState;

use crate::assets::{FoliageVisuals, TINTS};
use crate::plant::Genes;
use crate::species::Seasons;

/// Days in the plants' year.
pub const YEAR_DAYS: f32 = 365.0;
/// Early July: every deciduous plant in full summer leaf.
pub const MIDSUMMER: f32 = 190.0;
/// How many days earlier (−) or later (+) each leaf tint keeps the calendar.
pub const TINT_SHIFT_DAYS: [f32; TINTS] = [-7.0, 0.0, 7.0];

/// Which day of the year the plants show.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FoliageSeason {
    /// Day of the year, 0..365 (northern calendar: 100 ≈ 10 April, 290 ≈ 17 October).
    pub day_of_year: f32,
    /// Follow the weather's calendar (`WeatherState::day_of_year`) instead of a fixed day.
    pub follow_weather: bool,
}

impl Default for FoliageSeason {
    fn default() -> Self {
        Self {
            day_of_year: MIDSUMMER,
            follow_weather: false,
        }
    }
}

/// A deciduous plant's leaves on one day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leafing {
    /// Share of a full crown in leaf, 0 (bare) .. 1.
    pub amount: f32,
    /// How far the leaves have turned, 0 (summer green) .. 1 (full autumn color).
    pub turn: f32,
    /// Fresh spring green: 1 as the buds open, gone by early summer.
    pub fresh: f32,
}

impl Leafing {
    /// In full summer leaf (evergreens all year).
    pub const SUMMER: Leafing = Leafing {
        amount: 1.0,
        turn: 0.0,
        fresh: 0.0,
    };

    /// Whether a small plant (fern, flower) still stands: each one gives up at its own point
    /// of the season (`genes`), so a patch thins out rather than vanishing at once.
    pub fn shows(&self, genes: &Genes) -> bool {
        self.amount > 0.15 + 0.6 * personal(genes, 1)
    }
}

impl Seasons {
    /// The plant's leaves on `day`, for a plant that keeps the calendar `shift_days` late.
    pub fn leafing(&self, day: f32, shift_days: f32) -> Leafing {
        let d = (day - shift_days).rem_euclid(YEAR_DAYS);
        let out = smoothstep(self.leaf_out_day, self.leaf_out_day + 18.0, d);
        let span = (self.bare_day - self.turn_day).max(1.0);
        let fall = smoothstep(self.turn_day + 0.3 * span, self.bare_day, d);
        Leafing {
            amount: out * (1.0 - fall),
            turn: smoothstep(self.turn_day, self.turn_day + 0.65 * span, d),
            fresh: out * (1.0 - smoothstep(self.leaf_out_day + 8.0, self.leaf_out_day + 45.0, d)),
        }
    }

    /// Leaf color on a day: `summer` (a tint of the species' green) turning toward the
    /// autumn color, withering as the last leaves hang on, and lighter, yellower green
    /// while the leaves are new. Tints turn slightly different shades.
    pub fn color(&self, summer: [f32; 3], l: Leafing, tint: usize) -> [f32; 3] {
        let shade = [[1.06, 0.97, 0.9], [1.0, 1.0, 1.0], [0.93, 0.98, 1.05]][tint % TINTS];
        let autumn: [f32; 3] = std::array::from_fn(|i| self.autumn_color[i] * shade[i]);
        // The last leaves to hang on are dry and brown.
        let withered = [autumn[0] * 0.62, autumn[1] * 0.48, autumn[2] * 0.4];
        let dry = l.turn * (1.0 - l.amount) * (1.0 - l.amount);
        let fresh = [summer[0] * 1.45, summer[1] * 1.3, summer[2] * 0.85];
        std::array::from_fn(|i| {
            let c = summer[i] + (autumn[i] - summer[i]) * l.turn;
            let c = c + (withered[i] - c) * dry;
            c + (fresh[i] - c) * 0.75 * l.fresh
        })
    }
}

/// What deciduous plants are doing on a day, in a few words (for tools and the UI).
pub fn phase_name(day: f32) -> &'static str {
    match day.rem_euclid(YEAR_DAYS) as u32 {
        105..140 => "leafing out (spring)",
        140..255 => "in full leaf (summer)",
        255..290 => "turning (autumn)",
        290..325 => "dropping their leaves (late autumn)",
        _ => "bare (winter)",
    }
}

/// A plant's own lateness, days: its tint's shift plus a few days of its own.
pub fn shift_days(genes: &Genes) -> f32 {
    let tint = (((genes.hue + 1.0) * 0.5 * TINTS as f32) as usize).min(TINTS - 1);
    TINT_SHIFT_DAYS[tint] + 8.0 * (personal(genes, 0) - 0.5)
}

/// A stable 0..1 number per plant, from genes that don't change as it grows.
fn personal(genes: &Genes, salt: i64) -> f32 {
    let bits = |v: f32| (v * 1.0e4) as i64;
    hash01(
        bits(genes.vigor) ^ (bits(genes.drought) << 7),
        bits(genes.shade) ^ (bits(genes.lean_dir) << 5),
        salt,
        0x5ea5,
    )
}

/// How dry meadow grass is from the season alone, 0 (lush) .. 1 (straw): green from late
/// spring through August, browning through autumn, straw all winter.
pub fn grass_dryness(day: f32) -> f32 {
    let d = day.rem_euclid(YEAR_DAYS);
    let greening = 1.0 - smoothstep(75.0, 150.0, d);
    let browning = smoothstep(235.0, 330.0, d);
    0.8 * greening.max(browning)
}

/// The leaf litter on a day: above 0 (up to 1) freshly fallen autumn leaves, from the first
/// leaf fall in early October until they brown in late November; below 0 (down to −0.6) the
/// dark, sodden litter of winter, drying out by late April.
pub fn litter_season(day: f32) -> f32 {
    let d = day.rem_euclid(YEAR_DAYS);
    let fresh = smoothstep(262.0, 295.0, d) * (1.0 - smoothstep(318.0, 350.0, d));
    let sodden = smoothstep(330.0, 362.0, d).max(1.0 - smoothstep(60.0, 118.0, d));
    fresh - 0.6 * sodden
}

/// Tint the leaf clumps lying on the forest floor with the season ([`litter_season`]): bright
/// and warm while freshly fallen, dark through winter.
pub fn apply_litter_color(
    season: Res<FoliageSeason>,
    assets: Option<Res<crate::scenery::SceneryAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(assets) = assets else {
        return;
    };
    if !season.is_changed() && !assets.is_changed() {
        return;
    }
    let s = litter_season(season.day_of_year);
    let tint = if s > 0.0 {
        [1.0 + 0.55 * s, 1.0 + 0.08 * s, 1.0 - 0.5 * s]
    } else {
        [1.0 + 0.32 * s, 1.0 + 0.34 * s, 1.0 + 0.36 * s]
    };
    if let Some(m) = materials.get_mut(assets.leaves)
        && m.base_color[..3] != tint
    {
        m.base_color = [tint[0], tint[1], tint[2], m.base_color[3]];
    }
}

/// Copy the weather's calendar into [`FoliageSeason`] when it follows the weather. Small clock
/// steps are skipped, so plants re-dress a few times a day rather than every frame.
pub fn follow_weather_calendar(
    mut season: ResMut<FoliageSeason>,
    weather: Option<Res<WeatherState>>,
) {
    let Some(weather) = weather.filter(|_| season.follow_weather) else {
        return;
    };
    let day = weather.day_of_year.rem_euclid(YEAR_DAYS);
    if (day - season.day_of_year).abs() > 0.1 {
        season.day_of_year = day;
    }
}

/// The share of their translucency fully turned leaves lose.
const DRY_TURNED: f32 = 0.7;

/// Recolor every deciduous species' leaves for the day.
pub fn apply_season_colors(
    season: Res<FoliageSeason>,
    visuals: Res<FoliageVisuals>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !season.is_changed() && !visuals.is_changed() {
        return;
    }
    for vis in visuals.species.iter().flatten() {
        let Some(seasons) = vis.built_from.seasons else {
            continue;
        };
        for (tint, &shift) in TINT_SHIFT_DAYS.iter().enumerate() {
            let l = seasons.leafing(season.day_of_year, shift);
            let [r, g, b] = seasons.color(vis.leaf_colors[tint], l, tint);
            // Turned leaves are drying: less light passes through them (a rust frond lit from
            // behind with a summer leaf's translucency shone signal red).
            let through = vis.leaf_subsurface * (1.0 - DRY_TURNED * l.turn);
            let hero = vis.leaves_hero.map(|h| (h[tint], through));
            // The far crowns' mass atlas is many leaves deep: half as much gets through.
            let far = (vis.leaves_far[tint] != vis.leaves[tint])
                .then_some((vis.leaves_far[tint], through * 0.5));
            for (handle, subsurface) in [
                (vis.leaves[tint], through),
                (vis.leaves_still[tint], through),
            ]
            .into_iter()
            .chain(far)
            .chain(hero)
            {
                if let Some(m) = materials.get_mut(handle)
                    && (m.base_color[..3] != [r, g, b] || m.subsurface != subsurface)
                {
                    m.base_color = [r, g, b, m.base_color[3]];
                    m.subsurface = subsurface;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::PlantLibrary;

    fn seasons(name: &str) -> Seasons {
        let lib = PlantLibrary::default();
        lib.get(lib.id(name).unwrap()).seasons.unwrap()
    }

    #[test]
    fn an_oak_is_bare_in_winter_full_in_summer_and_turns_in_autumn() {
        let oak = seasons("oak");
        let at = |day: f32| oak.leafing(day, 0.0);
        assert_eq!(at(20.0).amount, 0.0, "January");
        assert_eq!(at(MIDSUMMER), Leafing::SUMMER, "July");
        let spring = at(oak.leaf_out_day + 10.0);
        assert!(spring.amount > 0.2 && spring.amount < 1.0 && spring.fresh > 0.5);
        let autumn = at(oak.turn_day + 25.0);
        assert!(autumn.turn > 0.5 && autumn.amount > 0.3 && autumn.amount < 1.0);
        assert_eq!(at(350.0).amount, 0.0, "December");
    }

    #[test]
    fn birches_turn_before_oaks() {
        let day = 280.0;
        let birch = seasons("birch").leafing(day, 0.0);
        let oak = seasons("oak").leafing(day, 0.0);
        assert!(birch.turn > oak.turn + 0.3, "{birch:?} vs {oak:?}");
    }

    #[test]
    fn autumn_leaves_take_the_autumn_color() {
        let birch = seasons("birch");
        let green = [0.1, 0.2, 0.03];
        let l = birch.leafing(birch.turn_day + 30.0, 0.0);
        let c = birch.color(green, l, 1);
        assert!(c[0] > c[1] * 0.9 && c[0] > green[0] * 3.0, "{c:?}");
        assert_eq!(birch.color(green, Leafing::SUMMER, 1), green);
    }

    #[test]
    fn leaf_litter_is_fresh_in_autumn_and_sodden_in_winter() {
        assert_eq!(litter_season(MIDSUMMER), 0.0);
        assert!(litter_season(300.0) > 0.8, "late October");
        assert!(litter_season(20.0) < -0.5, "January");
        assert_eq!(litter_season(150.0), 0.0, "June");
    }

    #[test]
    fn grass_is_straw_in_winter_and_green_in_summer() {
        assert!(grass_dryness(15.0) > 0.7);
        assert_eq!(grass_dryness(MIDSUMMER), 0.0);
        assert!(grass_dryness(300.0) > 0.4);
    }
}

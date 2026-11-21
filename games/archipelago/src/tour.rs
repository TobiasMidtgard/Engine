//! `--tour`: run the start for a while without drawing, then fly to each view, wait for the
//! ground and the trees, and save `screenshots/archipelago/<key>.png`.

use engine::prelude::*;
use engine::render::{RenderDisabled, RenderPaused, capture_now};

use crate::camera::StrategyCam;
use crate::fleet::Vessel;
use crate::island::islands;
use crate::sim::Building;
use crate::vessels::Craft;

/// Where a view looks from.
enum Aim {
    /// The home town's middle.
    Town,
    Island(usize),
    /// Where an island's settlement meets the sea.
    Settle(usize),
    /// The middle of a far land's first town ([`crate::town::far_site`]); the view's yaw is
    /// relative to the way inland from its quay.
    FarTown(usize),
    /// The first building of this kind on an island.
    Kind(crate::defs::Kind, usize),
    /// The first building of this kind on an island, seen from its front: yaw 0 looks
    /// straight at the front, a positive yaw walks round to its left.
    Front(crate::defs::Kind, usize),
    /// The first house of this tier on an island, seen from its front like [`Aim::Front`].
    Home(crate::defs::Tier, usize),
    /// The first vessel of this craft.
    Craft(Craft),
    /// The pedlar's barque.
    Pedlar,
    /// A vessel by name.
    Ship(&'static str),
    /// The fleet review line (`ARCH_FLEET_REVIEW=1`), seen from the beam: yaw 0 looks at the
    /// hulls' sides.
    Review,
    At(f64, f64),
}

struct View {
    key: &'static str,
    aim: Aim,
    /// Offset from the aim, meters (x, z).
    shift: (f64, f64),
    /// Camera yaw; for a craft, relative to its heading (0 broadside to port, π/2 off the bow).
    yaw: f32,
    dist: f32,
    pitch_deg: Option<f32>,
    /// Hide the HUD for this one.
    clean: bool,
    /// Draw the fog of war (off for views of lands not yet explored).
    fog: bool,
    /// Select the building the view aims at, so its card shows.
    card: bool,
    /// Set the clock to this hour for the view (and back after).
    hour: Option<f32>,
    /// Light the Fireworks over the building the view aims at.
    show: bool,
    /// Start this incident at the nearest house to the aim (a fire: the nearest building
    /// that is not a fire watch), as bad as `.1` (0..1).
    trouble: Option<(crate::incidents::Trouble, f32)>,
    /// Burn the aimed building out first.
    ruin: bool,
    /// Game seconds to run (not drawn) after starting the incident: crews on their way.
    wait_s: f32,
    /// A tool in the hand with the cursor in the middle of the screen. A building moves the
    /// view to the nearest spot by the aim where it fits (beside a road, on land).
    hold: Option<crate::tools::Tool>,
    /// For the road tool: where the drag began, in cells from the cursor's.
    drag_from: Option<(i32, i32)>,
    /// Set the scene up round the cell in the middle of the view (blueprints, a stamp), and
    /// perhaps move the view to another cell.
    prep: Option<fn(&mut World, IVec2) -> Option<IVec2>>,
}

const fn v(key: &'static str, aim: Aim, yaw: f32, dist: f32) -> View {
    View {
        key,
        aim,
        shift: (0.0, 0.0),
        yaw,
        dist,
        pitch_deg: None,
        clean: false,
        fog: true,
        card: false,
        hour: None,
        show: false,
        trouble: None,
        ruin: false,
        wait_s: 0.0,
        hold: None,
        drag_from: None,
        prep: None,
    }
}

fn views() -> Vec<View> {
    use crate::defs::{Kind as K, Tier as T};
    use crate::tools::Tool;
    vec![
        View {
            pitch_deg: Some(52.0),
            ..v("archipelago", Aim::At(-150.0, 250.0), 0.35, 5200.0)
        },
        // The planet: from over the home isles out to the whole globe, and the other lands.
        View {
            clean: true,
            fog: false,
            ..v("planet_isles", Aim::At(0.0, 0.0), 0.35, 9_000.0)
        },
        View {
            clean: true,
            fog: false,
            ..v("planet_high", Aim::At(0.0, 0.0), 0.35, 34_000.0)
        },
        View {
            clean: true,
            fog: false,
            ..v("planet_globe", Aim::At(0.0, 0.0), 0.2, 120_000.0)
        },
        // The fog of war over what has not been explored.
        View {
            clean: true,
            pitch_deg: Some(50.0),
            ..v("fog_isles", Aim::At(0.0, 0.0), 0.35, 7_000.0)
        },
        View {
            clean: true,
            ..v("fog_globe", Aim::At(0.0, 0.0), 0.2, 120_000.0)
        },
        View {
            clean: true,
            pitch_deg: Some(14.0),
            ..v("fog_bank", Aim::Island(0), 4.4, 900.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(38.0),
            ..v("sarrowmere", Aim::Island(5), 0.8, 5_200.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(22.0),
            ..v("sarrowmere_coast", Aim::Settle(5), 2.4, 420.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(38.0),
            ..v("amberland", Aim::Island(9), 5.2, 5_600.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(17.0),
            ..v("amberland_coast", Aim::Island(9), 0.9, 1_500.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(38.0),
            ..v("frostholm", Aim::Island(12), 2.2, 4_800.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(14.0),
            ..v("frostholm_coast", Aim::Island(12), 2.2, 1_800.0)
        },
        // The first towns of the three far regions (the camera looks inland over the quay).
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(27.0),
            shift: (-88.0, 19.0),
            ..v("sarrowmere_town", Aim::Settle(5), 2.4, 380.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(27.0),
            ..v("amber_town", Aim::FarTown(9), 0.45, 300.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(27.0),
            ..v("frost_town", Aim::FarTown(12), 0.45, 380.0)
        },
        // Their streets, fields and works.
        View {
            clean: true,
            pitch_deg: Some(21.0),
            ..v("stilt_street", Aim::Kind(K::Market, 5), 4.1, 95.0)
        },
        View {
            clean: true,
            pitch_deg: Some(26.0),
            ..v("cane_fields", Aim::Kind(K::CanePlantation, 5), 2.9, 150.0)
        },
        View {
            clean: true,
            pitch_deg: Some(24.0),
            ..v("distillery", Aim::Kind(K::Distillery, 5), -0.3, 55.0)
        },
        View {
            pitch_deg: Some(30.0),
            ..v("sarrowmere_hud", Aim::Kind(K::Market, 5), 2.0, 260.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(27.0),
            shift: (60.0, -13.0),
            ..v("palm_key", Aim::Settle(6), 4.31, 300.0)
        },
        View {
            clean: true,
            pitch_deg: Some(21.0),
            ..v("adobe_street", Aim::Kind(K::Market, 9), 4.47, 95.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(35.0),
            ..v("amber_quay", Aim::Kind(K::Harbour, 9), 4.7, 200.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(30.0),
            ..v("sarrow_quay", Aim::Kind(K::Harbour, 5), 2.0, 150.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(24.0),
            ..v("sarrow_wharf", Aim::Front(K::Harbour, 5), 0.55, 66.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(22.0),
            ..v("sarrow_fishery", Aim::Front(K::Fishery, 5), 0.6, 34.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(30.0),
            ..v("sarrow_market", Aim::Front(K::Market, 5), 0.5, 52.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(20.0),
            ..v("sarrow_chapel", Aim::Front(K::Chapel, 5), 0.6, 60.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(24.0),
            ..v("amber_wharf", Aim::Front(K::Harbour, 9), 0.55, 66.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(22.0),
            ..v("amber_fishery", Aim::Front(K::Fishery, 9), 0.6, 34.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(30.0),
            ..v("amber_market", Aim::Front(K::Market, 9), 0.5, 52.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(20.0),
            ..v("amber_chapel", Aim::Front(K::Chapel, 9), 0.6, 60.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(24.0),
            ..v("frost_wharf", Aim::Front(K::Harbour, 12), 0.55, 66.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(22.0),
            ..v("frost_fishery", Aim::Front(K::Fishery, 12), 0.6, 34.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(30.0),
            ..v("frost_market", Aim::Front(K::Market, 12), 0.5, 52.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(20.0),
            ..v("frost_chapel", Aim::Front(K::Chapel, 12), 3.7, 60.0)
        },
        View {
            clean: true,
            pitch_deg: Some(26.0),
            ..v("date_grove", Aim::Kind(K::DateGrove, 9), 3.9, 120.0)
        },
        View {
            clean: true,
            pitch_deg: Some(28.0),
            ..v("salt_pans", Aim::Kind(K::SaltWorks, 10), 1.33, 80.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(27.0),
            shift: (-60.0, 23.0),
            ..v("saltholm", Aim::Settle(10), 1.33, 320.0)
        },
        View {
            clean: true,
            pitch_deg: Some(21.0),
            ..v("cabin_street", Aim::Kind(K::Market, 12), 1.34, 95.0)
        },
        View {
            clean: true,
            pitch_deg: Some(20.0),
            ..v("trapper", Aim::Kind(K::TrapperLodge, 12), 0.6, 42.0)
        },
        View {
            clean: true,
            pitch_deg: Some(22.0),
            ..v("oil_works", Aim::Kind(K::OilWorks, 12), 0.9, 70.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(27.0),
            shift: (60.0, -24.0),
            ..v("whalers_rock", Aim::Settle(13), 4.48, 300.0)
        },
        v("town", Aim::Town, 0.6, 330.0),
        // A rival game's houses at home (`--rivals 3 --play-shots gullwick_town,...`):
        // Northmoor, Frostholm and Sarrowmere.
        View {
            fog: false,
            ..v("gullwick_town", Aim::Kind(K::Market, 3), 2.2, 300.0)
        },
        View {
            fog: false,
            ..v("ostrava_town", Aim::Kind(K::Market, 12), 1.3, 300.0)
        },
        View {
            fog: false,
            ..v("harrowgate_town", Aim::Kind(K::Market, 5), 4.1, 300.0)
        },
        View {
            clean: true,
            pitch_deg: Some(24.0),
            ..v("town_street", Aim::Town, 2.1, 110.0)
        },
        View {
            clean: true,
            pitch_deg: Some(20.0),
            ..v("gardens", Aim::Kind(K::House, 0), 0.75, 40.0)
        },
        // Round 6: the paved middle, the Burghers and what they live on.
        View {
            clean: true,
            pitch_deg: Some(38.0),
            ..v("paved_centre", Aim::Town, 0.9, 170.0)
        },
        View {
            clean: true,
            pitch_deg: Some(24.0),
            ..v("burghers", Aim::Home(T::Burghers, 0), -0.45, 40.0)
        },
        View {
            card: true,
            pitch_deg: Some(24.0),
            ..v("burgher_card", Aim::Home(T::Burghers, 0), -0.5, 60.0)
        },
        View {
            clean: true,
            pitch_deg: Some(42.0),
            ..v("school", Aim::Front(K::School, 0), 0.4, 44.0)
        },
        View {
            clean: true,
            pitch_deg: Some(42.0),
            ..v("tavern", Aim::Front(K::Tavern, 0), -0.4, 42.0)
        },
        View {
            clean: true,
            pitch_deg: Some(22.0),
            ..v("glassworks", Aim::Front(K::Glassworks, 0), 0.6, 34.0)
        },
        View {
            clean: true,
            pitch_deg: Some(22.0),
            ..v("saltery", Aim::Front(K::Saltery, 0), 0.6, 30.0)
        },
        View {
            clean: true,
            pitch_deg: Some(22.0),
            ..v("furrier", Aim::Front(K::Furrier, 0), -0.6, 28.0)
        },
        View {
            clean: true,
            pitch_deg: Some(22.0),
            ..v("lamp_maker", Aim::Front(K::LampMaker, 0), 0.6, 26.0)
        },
        View {
            clean: true,
            pitch_deg: Some(22.0),
            ..v("confectioner", Aim::Front(K::Confectioner, 1), 0.6, 28.0)
        },
        View {
            clean: true,
            pitch_deg: Some(24.0),
            ..v("dirt_lane", Aim::Kind(K::SheepFarm, 0), 4.0, 70.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(20.0),
            ..v("villas", Aim::Home(T::Burghers, 5), 0.5, 46.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(20.0),
            ..v("mansions", Aim::Home(T::Burghers, 9), 0.5, 46.0)
        },
        View {
            clean: true,
            fog: false,
            pitch_deg: Some(20.0),
            ..v("merchants", Aim::Home(T::Burghers, 12), 0.5, 46.0)
        },
        // Round 7: the three buildings that hold items, their reach, and the pedlar.
        View {
            clean: true,
            pitch_deg: Some(30.0),
            ..v("town_hall", Aim::Front(K::TownHall, 0), 0.45, 62.0)
        },
        View {
            card: true,
            pitch_deg: Some(52.0),
            ..v("town_hall_reach", Aim::Front(K::TownHall, 0), 0.3, 240.0)
        },
        View {
            clean: true,
            pitch_deg: Some(30.0),
            ..v("trading_union", Aim::Front(K::TradingUnion, 0), -0.5, 44.0)
        },
        // Round 8 build: the Ship chandler, and the Town Hall's fireworks at night.
        View {
            clean: true,
            pitch_deg: Some(46.0),
            ..v("chandler", Aim::Front(K::Chandler, 0), 0.5, 52.0)
        },
        View {
            clean: true,
            pitch_deg: Some(9.0),
            fog: false,
            hour: Some(20.6),
            show: true,
            ..v("fireworks", Aim::Front(K::TownHall, 0), 0.45, 190.0)
        },
        View {
            card: true,
            pitch_deg: Some(52.0),
            ..v("union_reach", Aim::Front(K::TradingUnion, 0), 0.3, 260.0)
        },
        View {
            clean: true,
            pitch_deg: Some(32.0),
            ..v("bakers_union", Aim::Front(K::TradingUnion, 1), 0.5, 64.0)
        },
        View {
            clean: true,
            pitch_deg: Some(28.0),
            ..v("harbour_office", Aim::Front(K::HarbourOffice, 0), 0.5, 56.0)
        },
        View {
            card: true,
            pitch_deg: Some(55.0),
            ..v("office_reach", Aim::Front(K::HarbourOffice, 0), 0.2, 400.0)
        },
        View {
            clean: true,
            pitch_deg: Some(16.0),
            ..v("pedlar", Aim::Pedlar, 0.5, 120.0)
        },
        v("harbour", Aim::Kind(K::Harbour, 0), 3.4, 190.0),
        // A new game (`--play`): the barque off the empty coast, the first harbour and the
        // shipyard with a hull on its slip, and the first island settled from home.
        View {
            pitch_deg: Some(22.0),
            ..v("new_game", Aim::Ship(crate::settle::FIRST_SHIP), 2.4, 260.0)
        },
        View {
            pitch_deg: Some(40.0),
            ..v("first_harbour", Aim::Front(K::Harbour, 0), 0.6, 150.0)
        },
        View {
            pitch_deg: Some(34.0),
            ..v("shipyard", Aim::Front(K::Shipyard, 0), 0.7, 120.0)
        },
        View {
            pitch_deg: Some(24.0),
            clean: true,
            ..v("slipway", Aim::Front(K::Shipyard, 0), 0.35, 70.0)
        },
        View {
            pitch_deg: Some(45.0),
            ..v("new_town", Aim::Town, 0.8, 420.0)
        },
        View {
            pitch_deg: Some(38.0),
            ..v("outpost", Aim::Front(K::Harbour, 4), 0.6, 200.0)
        },
        v("market", Aim::Kind(K::Market, 0), 1.2, 90.0),
        View {
            pitch_deg: Some(30.0),
            ..v("farms", Aim::Kind(K::SheepFarm, 0), 4.0, 260.0)
        },
        v("brannsey", Aim::Island(1), 0.9, 1400.0),
        View {
            pitch_deg: Some(26.0),
            ..v("mill", Aim::Kind(K::Mill, 1), 5.2, 120.0)
        },
        View {
            pitch_deg: Some(18.0),
            clean: true,
            fog: false,
            ..v("airship", Aim::Craft(Craft::Airship), 0.7, 260.0)
        },
        View {
            pitch_deg: Some(22.0),
            clean: true,
            fog: false,
            ..v("steamer", Aim::Craft(Craft::Steamer), 2.3, 150.0)
        },
        v("northmoor", Aim::Island(3), 4.4, 1300.0),
        View {
            pitch_deg: Some(16.0),
            clean: true,
            fog: false,
            ..v("clipper", Aim::Craft(Craft::Clipper), 2.2, 190.0)
        },
        View {
            pitch_deg: Some(20.0),
            clean: true,
            fog: false,
            ..v("paddle", Aim::Craft(Craft::PaddleSteamer), 0.45, 110.0)
        },
        View {
            pitch_deg: Some(12.0),
            clean: true,
            fog: false,
            ..v("seaplane", Aim::Craft(Craft::Seaplane), 2.1, 45.0)
        },
        // Needs ARCH_FLEET_REVIEW=1.
        View {
            pitch_deg: Some(7.0),
            clean: true,
            fog: false,
            ..v("fleet", Aim::Review, 2.95, 300.0)
        },
        View {
            pitch_deg: Some(10.0),
            clean: true,
            fog: false,
            ..v("fleet_new", Aim::Craft(Craft::Frigate), 0.1, 360.0)
        },
        View {
            pitch_deg: Some(16.0),
            clean: true,
            fog: false,
            ..v("frigate", Aim::Craft(Craft::Frigate), 2.3, 150.0)
        },
        View {
            pitch_deg: Some(18.0),
            clean: true,
            fog: false,
            ..v("liner", Aim::Craft(Craft::Liner), 2.2, 190.0)
        },
        View {
            pitch_deg: Some(18.0),
            clean: true,
            fog: false,
            ..v("gunboat", Aim::Craft(Craft::Gunboat), 0.6, 80.0)
        },
        View {
            pitch_deg: Some(16.0),
            clean: true,
            fog: false,
            ..v("brig", Aim::Craft(Craft::Brig), 2.3, 110.0)
        },
        View {
            pitch_deg: Some(16.0),
            clean: true,
            fog: false,
            ..v("cutter", Aim::Craft(Craft::Cutter), 2.2, 70.0)
        },
        View {
            pitch_deg: Some(14.0),
            clean: true,
            fog: false,
            ..v("flying_boat", Aim::Craft(Craft::FlyingBoat), 2.2, 80.0)
        },
        // The three that answer incidents, then incidents (these leave fires and ruins
        // behind, so they come last).
        View {
            pitch_deg: Some(26.0),
            ..v("fire_watch", Aim::Front(K::FireWatch, 0), 0.5, 50.0)
        },
        View {
            pitch_deg: Some(26.0),
            ..v("watch_house", Aim::Front(K::WatchHouse, 0), 0.5, 46.0)
        },
        View {
            pitch_deg: Some(26.0),
            ..v("infirmary", Aim::Front(K::Infirmary, 0), 0.5, 56.0)
        },
        View {
            pitch_deg: Some(30.0),
            clean: true,
            trouble: Some((crate::incidents::Trouble::Fire, 0.9)),
            ..v("fire", Aim::Home(T::Labourers, 0), 0.7, 48.0)
        },
        View {
            pitch_deg: Some(34.0),
            trouble: Some((crate::incidents::Trouble::Fire, 0.6)),
            wait_s: 9.0,
            ..v("fire_pump", Aim::Front(K::FireWatch, 0), 0.3, 85.0)
        },
        View {
            pitch_deg: Some(26.0),
            clean: true,
            trouble: Some((crate::incidents::Trouble::Riot, 0.9)),
            ..v("riot", Aim::Home(T::Fisherfolk, 0), 0.6, 30.0)
        },
        View {
            pitch_deg: Some(34.0),
            trouble: Some((crate::incidents::Trouble::Riot, 0.6)),
            wait_s: 10.0,
            ..v("constables", Aim::Front(K::WatchHouse, 0), 0.3, 80.0)
        },
        View {
            pitch_deg: Some(40.0),
            clean: true,
            trouble: Some((crate::incidents::Trouble::Sickness, 0.9)),
            ..v("sickness", Aim::Home(T::Burghers, 0), 3.8, 46.0)
        },
        View {
            pitch_deg: Some(34.0),
            trouble: Some((crate::incidents::Trouble::Sickness, 0.6)),
            wait_s: 10.0,
            ..v("doctor", Aim::Front(K::Infirmary, 0), 0.3, 80.0)
        },
        View {
            pitch_deg: Some(30.0),
            clean: true,
            ruin: true,
            ..v("ruin", Aim::Front(K::Weaver, 0), 0.6, 36.0)
        },
        // Building: what the hand shows while placing (the cursor in the middle).
        View {
            hold: Some(Tool::Build {
                kind: K::House,
                rot: 0,
            }),
            // Out at the edge of the town, where the streets meet open land.
            shift: (-200.0, -40.0),
            ..v("build_house", Aim::Town, 0.6, 170.0)
        },
        View {
            hold: Some(Tool::Build {
                kind: K::Market,
                rot: 0,
            }),
            ..v("build_market", Aim::Town, 0.6, 300.0)
        },
        View {
            hold: Some(Tool::Build {
                kind: K::SheepFarm,
                rot: 0,
            }),
            ..v("build_sheep", Aim::Kind(K::SheepFarm, 0), 4.0, 220.0)
        },
        View {
            fog: false,
            hold: Some(Tool::Build {
                kind: K::Harbour,
                rot: 0,
            }),
            ..v("build_harbour", Aim::Island(4), 0.9, 650.0)
        },
        View {
            hold: Some(Tool::Road),
            drag_from: Some((-26, 22)),
            shift: (-120.0, -60.0),
            ..v("build_road", Aim::Town, 0.6, 220.0)
        },
        // Blueprints: a row of houses, a market and their road planned, a house in the hand.
        View {
            hold: Some(Tool::Build {
                kind: K::House,
                rot: 0,
            }),
            shift: (-200.0, -40.0),
            prep: Some(prep_blueprints),
            ..v("build_blueprint", Aim::Town, 0.6, 200.0)
        },
        // A stamp of the town's middle in the hand, over open ground beside it.
        View {
            hold: Some(Tool::Stamp { index: 0, rot: 1 }),
            prep: Some(prep_stamp),
            ..v("build_stamp", Aim::Town, 0.6, 300.0)
        },
    ]
}

/// The blueprints view: blueprint mode on and a row of plans beside the house in the hand.
fn prep_blueprints(world: &mut World, c: IVec2) -> Option<IVec2> {
    world.resource_mut::<crate::blueprint::Blueprints>().on = true;
    let n = crate::blueprint::tour_lay(world, c + IVec2::new(-9, 5));
    engine::core::log::info!("tour: {n} blueprints laid");
    None
}

/// The stamp view: the town round the market nearest the aim kept as a stamp, and the view
/// moved to open ground near it where most of it fits (turned as the view holds it).
fn prep_stamp(world: &mut World, c: IVec2) -> Option<IVec2> {
    let m = world
        .query::<&crate::sim::Building>()
        .iter(world)
        .filter(|b| b.kind == crate::defs::Kind::Market)
        .min_by_key(|b| (b.origin - c).as_i64vec2().length_squared())?
        .origin;
    let i = crate::stamps::tour_copy(world, m, 18)?;
    let s = world.resource::<crate::stamps::Book>().stamps[i].clone();
    let n = s.buildings.len().max(1);
    let fit_at = |w: &World, at: IVec2| {
        let (bs, _) = crate::stamps::members(&s, 1, at);
        bs.iter()
            .filter(|(k, o, r)| crate::stamps::fits(w, *k, *o, *r).is_ok())
            .count()
    };
    // Out on open ground, clear of the town (it should not read as part of it).
    let spot = (16..90i32)
        .flat_map(|r| {
            (0..24).map(move |k| {
                let a = k as f64 / 24.0 * std::f64::consts::TAU;
                m + IVec2::new(
                    (a.cos() * r as f64 * 2.0) as i32,
                    (a.sin() * r as f64 * 2.0) as i32,
                )
            })
        })
        .find(|at| fit_at(world, *at) == n);
    engine::core::log::info!(
        "tour: stamp {:?} ({} buildings, {} roads) at {spot:?}",
        s.name,
        s.buildings.len(),
        s.roads.len()
    );
    spot
}

/// Start a view's incident near `at`: at the nearest house (riots, sickness) or the nearest
/// building that is not one of the three that answer incidents (fire).
fn stage_incident(app: &mut App, view: &View, at: DVec3) -> Option<DVec2> {
    use crate::incidents::{self, Incident, Trouble};
    if view.ruin
        && let Some(b) = aimed_building(&mut app.world, &view.aim)
    {
        let mut q = app.world.query::<(Entity, &Building)>();
        if let Some(e) = q.iter(&app.world).find(|(_, x)| x.id == b.id).map(|x| x.0) {
            incidents::burn_now(&mut app.world, e);
        }
    }
    let (t, level) = view.trouble?;
    let here = DVec2::new(at.x, at.z);
    let target = {
        let mut q = app.world.query_filtered::<
            (Entity, &Building, Option<&crate::sim::House>),
            (Without<Incident>, Without<incidents::Ruin>),
        >();
        q.iter(&app.world)
            .filter(|(_, b, h)| {
                b.island == 0
                    && (h.is_some() || t == Trouble::Fire)
                    && !matches!(
                        b.kind,
                        crate::defs::Kind::FireWatch
                            | crate::defs::Kind::WatchHouse
                            | crate::defs::Kind::Infirmary
                            | crate::defs::Kind::Road
                            | crate::defs::Kind::PavedRoad
                    )
            })
            .min_by_key(|(_, b, _)| {
                let c = crate::grid::footprint_center(b.kind, b.origin, b.rot);
                ((c - here).length() * 10.0) as i64
            })
            .map(|(e, b, _)| (e, b.clone()))
    };
    let (e, b) = target?;
    incidents::start(&mut app.world, e, &b, t, false);
    if let Some(mut i) = app.world.get_mut::<Incident>(e) {
        i.level = level;
    }
    if view.wait_s > 0.0 {
        app.world.insert_resource(RenderPaused);
        let speed = app.world.resource::<crate::sim::Economy>().speed;
        app.world.resource_mut::<crate::sim::Economy>().speed = 2.0;
        let frames = (view.wait_s * 30.0 / 2.0).ceil() as usize;
        app.update_n(frames);
        app.world.resource_mut::<crate::sim::Economy>().speed = speed;
        app.world.remove_resource::<RenderPaused>();
        if let Some(mut i) = app.world.get_mut::<Incident>(e) {
            i.level = i.level.max(level);
        }
    }
    Some(crate::grid::footprint_center(b.kind, b.origin, b.rot))
}

fn aim_point(world: &mut World, aim: &Aim) -> Option<DVec3> {
    Some(match aim {
        Aim::Town => world.get_resource::<crate::town::TownCentre>()?.0,
        Aim::Island(i) => {
            let c = islands().get(*i)?.center;
            DVec3::new(c.x, 0.0, c.y)
        }
        Aim::Settle(i) => {
            let c = crate::island::settle_coast(islands().get(*i)?)?;
            DVec3::new(c.x, 0.0, c.y)
        }
        Aim::FarTown(i) => {
            let (_, c) = crate::town::far_site(*i)?;
            DVec3::new(c.x, 0.0, c.y)
        }
        Aim::Kind(..) | Aim::Front(..) | Aim::Home(..) => {
            let b = aimed_building(world, aim)?;
            let c = crate::grid::footprint_center(b.kind, b.origin, b.rot);
            DVec3::new(c.x, b.base_y, c.y)
        }
        Aim::Craft(c) => {
            let mut q = world.query::<&Vessel>();
            q.iter(world).find(|v| v.craft == *c).map(|v| v.at)?
        }
        Aim::Review => {
            let mut q = world.query::<&Vessel>();
            let at: Vec<DVec3> = q
                .iter(world)
                .filter(|v| v.name.starts_with("Fleet review") && !v.craft.flies())
                .map(|v| v.at)
                .collect();
            if at.is_empty() {
                return None;
            }
            at.iter().copied().sum::<DVec3>() / at.len() as f64
        }
        Aim::Pedlar => {
            let mut q = world.query::<&Vessel>();
            q.iter(world)
                .find(|v| matches!(v.roam, crate::fleet::Roam::Pedlar { .. }))
                .map(|v| v.at)?
        }
        Aim::Ship(name) => {
            let mut q = world.query::<&Vessel>();
            q.iter(world).find(|v| v.name == *name).map(|v| v.at)?
        }
        Aim::At(x, z) => DVec3::new(*x, 0.0, *z),
    })
}

/// The building a view aims at: the first of its kind (or the first house of its tier) on
/// the island.
fn aimed_building(world: &mut World, aim: &Aim) -> Option<Building> {
    match aim {
        Aim::Kind(k, isle) | Aim::Front(k, isle) => {
            let mut q = world.query::<&Building>();
            q.iter(world)
                .filter(|b| b.kind == *k && b.island == *isle)
                .min_by_key(|b| b.id)
                .cloned()
        }
        Aim::Home(tier, isle) => {
            let mut q = world.query::<(&Building, &crate::sim::House)>();
            q.iter(world)
                .filter(|(b, h)| h.tier == *tier && b.island == *isle)
                .min_by_key(|(b, _)| b.id)
                .map(|(b, _)| b.clone())
        }
        _ => None,
    }
}

/// The heading of the craft a view follows (0 for other views).
fn craft_heading(world: &mut World, aim: &Aim) -> f32 {
    if let Aim::FarTown(i) = aim {
        // The yaw that looks from the quay to the town's middle.
        return crate::town::far_site(*i).map_or(0.0, |(quay, mid)| {
            let d = (mid - quay).normalize_or_zero();
            (-d.x).atan2(-d.y) as f32
        });
    }
    if matches!(aim, Aim::Front(..) | Aim::Home(..)) {
        // The yaw that looks at the building's front from before it.
        return aimed_building(world, aim).map_or(0.0, |b| {
            let fd = crate::grid::front_dir(b.rot).as_vec2();
            fd.x.atan2(fd.y)
        });
    }
    let mut q = world.query::<&Vessel>();
    match aim {
        Aim::Craft(c) => q
            .iter(world)
            .find(|v| v.craft == *c)
            .map_or(0.0, |v| v.heading),
        Aim::Review => q
            .iter(world)
            .find(|v| v.name.starts_with("Fleet review"))
            .map_or(0.0, |v| v.heading),
        Aim::Pedlar => q
            .iter(world)
            .find(|v| matches!(v.roam, crate::fleet::Roam::Pedlar { .. }))
            .map_or(0.0, |v| v.heading),
        Aim::Ship(name) => q
            .iter(world)
            .find(|v| v.name == *name)
            .map_or(0.0, |v| v.heading),
        _ => 0.0,
    }
}

/// Frames until the terrain's patches and the foliage are in, at most `cap_s` wall seconds.
fn settle(app: &mut App, cap_s: f32) {
    let started = std::time::Instant::now();
    loop {
        app.update();
        let w = &app.world;
        let (building, missing) = w
            .get_resource::<engine::core::diagnostics::Diagnostics>()
            .map_or((0.0, 0.0), |d| {
                (
                    d.get("terrain.patches_building").unwrap_or(0.0),
                    d.get("terrain.patches_missing").unwrap_or(0.0),
                )
            });
        let foliage = w
            .get_resource::<engine_foliage::FoliageLoading>()
            .is_some_and(|f| !f.is_ready());
        let elapsed = started.elapsed().as_secs_f32();
        if building + missing == 0.0 && !foliage && elapsed > 0.5 {
            break;
        }
        if elapsed > cap_s {
            engine::core::log::warn!(
                "tour: gave up waiting ({building} patches building, {missing} missing, foliage {foliage})"
            );
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
}

/// Run the tour instead of the game loop.
pub fn run(mut app: App, only: Option<&str>) {
    let out = "screenshots/archipelago";
    let _ = std::fs::create_dir_all(out);
    app.world.resource_mut::<EngineConfig>().headless = true;
    // The same pictures each time: no incidents but the ones a view starts.
    if let Some(mut i) = app.world.get_resource_mut::<crate::incidents::Incidents>() {
        i.rolls = false;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(engine::core::time::TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    let started = std::time::Instant::now();
    // Run the start for a minute of game time without drawing: carts out, ships under way.
    app.world.insert_resource(RenderPaused);
    app.update_n(2);
    app.world.resource_mut::<crate::sim::Economy>().speed = 4.0;
    app.update_n(450);
    app.world.resource_mut::<crate::sim::Economy>().speed = 1.0;
    app.world.remove_resource::<RenderPaused>();
    engine::core::log::info!(
        "tour: warmed up in {:.1} s",
        started.elapsed().as_secs_f32()
    );
    if let Ok(state) =
        engine::core::registry::call_api(&mut app.world, "isle.state", serde_json::json!({}))
    {
        let _ = std::fs::write(
            format!("{out}/state.json"),
            serde_json::to_string_pretty(&state).unwrap_or_default(),
        );
    }
    if let Ok(list) =
        engine::core::registry::call_api(&mut app.world, "isle.list", serde_json::json!({}))
    {
        let _ = std::fs::write(
            format!("{out}/buildings.json"),
            serde_json::to_string_pretty(&list).unwrap_or_default(),
        );
    }
    if let Ok(items) =
        engine::core::registry::call_api(&mut app.world, "isle.items", serde_json::json!({}))
    {
        let _ = std::fs::write(
            format!("{out}/items.json"),
            serde_json::to_string_pretty(&items).unwrap_or_default(),
        );
    }
    shoot(&mut app, only, out);
}

/// Take the tour's pictures (all, or the views named in `only`) into `out`.
pub fn shoot(app: &mut App, only: Option<&str>, out: &str) {
    let keys: Option<Vec<&str>> = only.map(|s| s.split(',').collect());
    for view in views() {
        if keys.as_ref().is_some_and(|k| !k.contains(&view.key)) {
            continue;
        }
        let Some(mut p) = aim_point(&mut app.world, &view.aim) else {
            engine::core::log::warn!("tour: nothing to aim {} at", view.key);
            continue;
        };
        if let Some(crate::tools::Tool::Build { kind, rot }) = view.hold {
            // Look for room by the shifted aim, then aim so the view centres on it.
            let near = DVec2::new(p.x + view.shift.0, p.z + view.shift.1);
            match fitting_spot(&app.world, kind, rot, near) {
                Some(c) => {
                    let y = crate::island::height(c.x, c.y).max(0.0);
                    p = DVec3::new(c.x - view.shift.0, y, c.y - view.shift.1);
                }
                None => engine::core::log::warn!("tour: no room for a {kind:?} by {}", view.key),
            }
        }
        // A road drag: both ends on open ground near the shifted aim and the drag's start.
        let mut drag_from = view.drag_from.map(|(x, z)| IVec2::new(x, z));
        if matches!(
            view.hold,
            Some(crate::tools::Tool::Road | crate::tools::Tool::Paving)
        ) {
            let near = DVec2::new(p.x + view.shift.0, p.z + view.shift.1);
            let b = open_cell(&app.world, crate::grid::cell_of(near.x, near.y));
            // The drag's start: the open cell nearest the wanted one with a way through.
            let a = drag_from.zip(b).and_then(|(d, b)| {
                let g = app.world.resource::<crate::grid::Grid>();
                (0..12i32)
                    .flat_map(|r| {
                        (-r..=r)
                            .flat_map(move |x| (-r..=r).map(move |z| IVec2::new(x, z)))
                            .filter(move |o| o.x.abs().max(o.y.abs()) == r)
                    })
                    .map(|o| b + d + o)
                    .filter(|c| g.is_free(*c) && crate::grid::cell_roadable(*c))
                    .take(40)
                    .find(|a| crate::placing::road_path(g, *a, b).is_some())
            });
            drag_from = None;
            if let (Some(a), Some(b)) = (a, b) {
                let c = crate::grid::cell_center(b);
                p = DVec3::new(c.x - view.shift.0, p.y, c.y - view.shift.1);
                drag_from = Some(a);
            } else {
                engine::core::log::warn!("tour: no road drag fits by {}", view.key);
            }
        }
        if let Some(prep) = view.prep {
            let near = DVec2::new(p.x + view.shift.0, p.z + view.shift.1);
            if let Some(c) = prep(&mut app.world, crate::grid::cell_of(near.x, near.y)) {
                let at = crate::grid::cell_center(c);
                let y = crate::island::height(at.x, at.y).max(0.0);
                p = DVec3::new(at.x - view.shift.0, y, at.y - view.shift.1);
            }
        }
        let heading = craft_heading(&mut app.world, &view.aim);
        {
            let mut q = app.world.query::<&mut StrategyCam>();
            if let Some(mut cam) = q.iter_mut(&mut app.world).next() {
                cam.goal_focus = DVec3::new(p.x + view.shift.0, p.y, p.z + view.shift.1);
                cam.goal_yaw = view.yaw + heading;
                cam.goal_dist = view.dist;
                cam.pitch_override = view.pitch_deg.map(f32::to_radians);
                cam.free_height = matches!(
                    view.aim,
                    Aim::Craft(_) | Aim::Pedlar | Aim::Review | Aim::Ship(_)
                );
                cam.snap();
            }
        }
        set_hud(&mut app.world, !view.clean);
        let chosen = if view.card {
            aimed_building(&mut app.world, &view.aim).and_then(|b| {
                let mut q = app.world.query::<(Entity, &Building)>();
                q.iter(&app.world).find(|(_, x)| x.id == b.id).map(|x| x.0)
            })
        } else {
            None
        };
        crate::tools::select(&mut app.world, chosen);
        if let Some(mut fog) = app.world.get_resource_mut::<crate::fog::FogOfWar>() {
            fog.shown = view.fog;
        }
        let hour_was = view.hour.and_then(|h| {
            let was = engine::core::registry::call_api(
                &mut app.world,
                "weather.time",
                serde_json::json!({}),
            )
            .ok()?;
            let _ = engine::core::registry::call_api(
                &mut app.world,
                "weather.time",
                serde_json::json!({ "hour": h }),
            );
            was["hour"].as_f64()
        });
        let lit_at = if view.show {
            if let Some(e) = aimed_building(&mut app.world, &view.aim).and_then(|b| {
                let mut q = app.world.query::<(Entity, &Building)>();
                q.iter(&app.world).find(|(_, x)| x.id == b.id).map(|x| x.0)
            }) {
                crate::effects::fireworks(&mut app.world, e);
            }
            Some(app.world.resource::<Time>().elapsed_secs())
        } else {
            None
        };
        // A close view of an incident looks at the building it started at.
        if let Some(c) = stage_incident(app, &view, p)
            && view.wait_s == 0.0
        {
            let mut q = app.world.query::<&mut StrategyCam>();
            if let Some(mut cam) = q.iter_mut(&mut app.world).next() {
                cam.goal_focus = DVec3::new(c.x + view.shift.0, p.y, c.y + view.shift.1);
                cam.snap();
            }
        }
        if let Some(tool) = view.hold {
            let (w, h) = {
                let win = app.world.resource::<engine::core::window::Window>();
                (win.width as f32, win.height as f32)
            };
            app.world.resource_mut::<engine::input::Mouse>().position =
                Some(Vec2::new(w * 0.5, h * 0.5));
            crate::tools::stage(&mut app.world, tool, drag_from);
        }
        let t0 = std::time::Instant::now();
        // (A software renderer at 1440p needs longer: ARCH_TOUR_SETTLE_S.)
        let cap = std::env::var("ARCH_TOUR_SETTLE_S")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(240.0);
        settle(app, cap);
        // Exposure and the environment, following a moving craft.
        for _ in 0..8 {
            if matches!(
                view.aim,
                Aim::Craft(_) | Aim::Pedlar | Aim::Review | Aim::Ship(_)
            ) && let Some(p) = aim_point(&mut app.world, &view.aim)
            {
                let mut q = app.world.query::<&mut StrategyCam>();
                if let Some(mut cam) = q.iter_mut(&mut app.world).next() {
                    cam.goal_focus = p;
                    cam.free_height = true;
                    cam.snap();
                }
            }
            app.update();
        }
        // Give the rockets time to climb and burst.
        if let Some(t) = lit_at {
            while app.world.resource::<Time>().elapsed_secs() - t < 1.4 {
                app.update();
            }
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        if let Some(d) = app
            .world
            .get_resource::<engine::core::diagnostics::Diagnostics>()
        {
            let g = |k: &str| d.get(k).unwrap_or(-1.0);
            engine::core::log::info!(
                "tour: vfx effects {} emitters {} resting {} pool {} drawn {} skipped {}",
                g("vfx.effects"),
                g("vfx.emitters"),
                g("vfx.resting"),
                g("vfx.pool_used"),
                g("vfx.drawn_emitters"),
                g("vfx.skipped_emitters")
            );
        }
        let path = format!("{out}/{}.png", view.key);
        match capture_now(&mut app.world, &path) {
            Ok(_) => engine::core::log::info!("tour: {path} ({:.1} s)", t0.elapsed().as_secs_f32()),
            Err(e) => engine::core::log::warn!("tour: {path}: {e}"),
        }
        if view.hold.is_some() {
            app.world.resource_mut::<engine::input::Mouse>().position = None;
            crate::tools::stage(&mut app.world, crate::tools::Tool::Select, None);
        }
        if view.prep.is_some() {
            let mut bp = app.world.resource_mut::<crate::blueprint::Blueprints>();
            bp.on = false;
            bp.prints.clear();
            bp.roads.clear();
        }
        if let Some(h) = hour_was {
            let _ = engine::core::registry::call_api(
                &mut app.world,
                "weather.time",
                serde_json::json!({ "hour": h }),
            );
        }
    }
}

/// The middle of the nearest footprint to `near` where `kind` turned `rot` fits: on land
/// beside a road, or (a quay) anywhere it may stand.
fn fitting_spot(world: &World, kind: crate::defs::Kind, rot: u8, near: DVec2) -> Option<DVec2> {
    use crate::grid;
    let g = world.resource::<crate::grid::Grid>();
    let c0 = grid::cell_of(near.x, near.y);
    let (w, d) = kind.footprint(rot);
    let coast = kind.def().site == crate::defs::Site::Coast;
    let mut best: Option<(i32, IVec2)> = None;
    for r in 0..90i32 {
        if best.is_some_and(|b| b.0 < r * r) {
            break;
        }
        for z in -r..=r {
            for x in -r..=r {
                if x.abs().max(z.abs()) != r {
                    continue;
                }
                let o = c0 + IVec2::new(x, z) - IVec2::new(w / 2, d / 2);
                let d2 = x * x + z * z;
                if best.is_some_and(|b| b.0 <= d2) || grid::check(g, kind, o, rot).is_err() {
                    continue;
                }
                if !coast && !grid::ring(kind, o, rot).iter().any(|c| g.is_road(*c)) {
                    continue;
                }
                best = Some((d2, o));
            }
        }
    }
    best.map(|(_, o)| grid::footprint_center(kind, o, rot))
}

/// The open cell (a road may go there) nearest `c`.
fn open_cell(world: &World, c: IVec2) -> Option<IVec2> {
    let g = world.resource::<crate::grid::Grid>();
    (0..40i32).find_map(|r| {
        (-r..=r)
            .flat_map(|x| (-r..=r).map(move |z| IVec2::new(x, z)))
            .filter(|d| d.x.abs().max(d.y.abs()) == r)
            .map(|d| c + d)
            .find(|c| g.is_free(*c) && crate::grid::cell_roadable(*c))
    })
}

/// Show or hide the game's bars.
fn set_hud(world: &mut World, on: bool) {
    let mut q = world.query::<(&engine_ui::node::Screen, &mut engine_ui::node::Node)>();
    for (s, mut node) in q.iter_mut(world) {
        if s.key.starts_with("isle_") {
            node.style.opacity = Some(if on { 1.0 } else { 0.0 });
        }
    }
}

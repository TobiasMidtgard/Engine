//! Far woods after a jump: the longest frame and how long until every cell is in, with the
//! cells built in the frame (`inline`, as tours did before round 22), on the task pool all at
//! once (`tour`) or on the task pool a few at a time (play's defaults).
//!
//! `cargo run -p engine_foliage --release --example far_woods_load [-- inline|tour|play]`

use std::time::Instant;

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_foliage::species::PlantLibrary;
use engine_foliage::{FarWoods, FoliageLoading};
use engine_render::{Camera, Image, Mesh, StandardMaterial};
use engine_world::{ForestCover, ForestQuery, ForestSample};

struct Everywhere;
impl ForestQuery for Everywhere {
    fn sample(&self, p: DVec3) -> Option<ForestSample> {
        // A terrain's answer costs tens of microseconds (noise and climate belts): stand in
        // for that, or the build looks free.
        let mut h = p.x.to_bits() ^ p.z.to_bits();
        for _ in 0..4000 {
            h = h.rotate_left(5).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
        std::hint::black_box(h);
        Some(ForestSample {
            density: 0.6,
            conifer: 0.5,
        })
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "tour".into());
    engine_core::tasks::init_task_pools();
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins(engine_world::WorldPlugin);
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<Image>>()
        .add_plugins(engine_foliage::FoliagePlugin);
    app.world.insert_resource(FoliageLoading::in_frame());
    let all = PlantLibrary::default();
    let keep = ["pine", "spruce", "oak", "birch"].map(|n| all.get(all.id(n).unwrap()).clone());
    app.world.insert_resource(PlantLibrary {
        species: keep.to_vec(),
    });
    let (builds, per_frame) = match mode.as_str() {
        "inline" => (0, u32::MAX),
        "play" => (
            FarWoods::default().background_builds,
            FarWoods::default().cells_per_frame,
        ),
        _ => (4096, 256),
    };
    app.world.insert_resource(FarWoods {
        background_builds: builds,
        cells_per_frame: per_frame,
        hold_loading: true,
        ..Default::default()
    });
    app.world
        .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
    let cam = app
        .world
        .spawn((Camera::default(), Transform::from_translation(DVec3::ZERO)))
        .id();
    // Species visuals and the first woods.
    let t0 = Instant::now();
    while !app.world.resource::<FoliageLoading>().is_ready() || t0.elapsed().as_secs() < 2 {
        app.update();
        assert!(t0.elapsed().as_secs() < 600, "never ready");
    }
    // The jump (a tour stop, a fast travel).
    app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(6000.0, 0.0, 0.0);
    let start = Instant::now();
    let (mut longest, mut over, mut frames) = (0.0f64, 0, 0);
    loop {
        let f = Instant::now();
        app.update();
        let ms = f.elapsed().as_secs_f64() * 1000.0;
        longest = longest.max(ms);
        over += usize::from(ms > 33.0);
        frames += 1;
        if frames > 2 && app.world.resource::<FoliageLoading>().is_ready() {
            break;
        }
        assert!(
            start.elapsed().as_secs() < 600,
            "never ready after the jump"
        );
    }
    let mut cells = app.world.query::<&engine_foliage::far_woods::FarWoodCell>();
    println!(
        "{mode}: {} cells; longest frame {longest:.1} ms, {over} frames over 33 ms, {frames} frames, all in after {:.2} s",
        cells.iter(&app.world).count(),
        start.elapsed().as_secs_f64()
    );
}

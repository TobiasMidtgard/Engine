//! Features that prepare shared (`RenderFeature::prepares_shared`) run side by side: four that
//! each sleep 2 ms overlap in time, what they leave in their `PrepareOut` reaches the world, and a
//! feature that prepares the old way still prepares. (Overlap rather than a time limit: the
//! test may share the cores with other test processes.) Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    GpuShared, PrepareOut, RenderAppExt, RenderDisabled, RenderFeature, RenderSettings, ViewData,
};

const SLEEP: Duration = Duration::from_millis(2);
const SLEEPERS: usize = 4;

#[derive(Resource, Default)]
struct Seen(u32);

/// When each sleeper's shared prepares ran: (sleeper, start, end).
type Runs = Arc<Mutex<Vec<(usize, Instant, Instant)>>>;

/// Sleeps in its shared prepare and counts, through a deferred change, how often it ran.
struct Sleeper(usize, Runs);

impl RenderFeature for Sleeper {
    fn name(&self) -> &str {
        ["sleeper 0", "sleeper 1", "sleeper 2", "sleeper 3"][self.0]
    }

    fn prepares_shared(&self) -> bool {
        true
    }

    fn prepare_serial(
        &mut self,
        world: &mut World,
        _gpu: &GpuShared,
        _view: &ViewData,
        _settings: &RenderSettings,
    ) {
        world.init_resource::<Seen>();
    }

    fn prepare_shared(
        &mut self,
        world: &World,
        _gpu: &GpuShared,
        _view: &ViewData,
        _settings: &RenderSettings,
        out: &mut PrepareOut,
    ) {
        assert!(
            world.contains_resource::<Seen>(),
            "prepare_serial ran first"
        );
        let start = Instant::now();
        std::thread::sleep(SLEEP);
        self.1.lock().unwrap().push((self.0, start, Instant::now()));
        out.set(format!("test.{}", self.name()), 1.0);
        out.defer(|world| world.resource_mut::<Seen>().0 += 1);
    }
}

/// Prepares the old way.
struct Plain(Arc<AtomicU32>);

impl RenderFeature for Plain {
    fn name(&self) -> &str {
        "plain"
    }

    fn prepare(
        &mut self,
        _world: &mut World,
        _gpu: &GpuShared,
        _view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn shared_prepares_run_side_by_side() {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 64;
        w.height = 36;
    }
    app.add_plugins(RenderPlugin);
    let runs = Runs::default();
    for i in 0..SLEEPERS {
        app.add_render_feature(Sleeper(i, runs.clone()));
    }
    let plain = Arc::new(AtomicU32::new(0));
    app.add_render_feature(Plain(plain.clone()));
    app.world.spawn((
        Camera::perspective(60.0),
        Transform::from_xyz(0.0, 2.0, 6.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU ({}), skipping", d.0);
        return;
    }
    let frames = 8;
    let mut walls = Vec::new();
    let mut sums = Vec::new();
    for _ in 0..frames {
        app.update();
        let d = app.world.resource::<Diagnostics>();
        walls.push(d.metrics["render.cpu.prepare side by side"]);
        sums.push(d.metrics["render.prepare_shared_ms"]);
        for i in 0..SLEEPERS {
            let ms = d.metrics[&format!("render.cpu.prepare sleeper {i}")];
            assert!(ms >= 1.9, "sleeper {i} took {ms:.2} ms");
            assert_eq!(d.metrics[&format!("test.sleeper {i}")], 1.0);
        }
    }
    let n = (frames + 1) as u32;
    assert_eq!(app.world.resource::<Seen>().0, SLEEPERS as u32 * n);
    assert_eq!(
        plain.load(Ordering::Relaxed),
        n,
        "the plain feature prepared every frame"
    );

    // The best frame, for the log: a busy test machine can stall any one.
    let (wall, sum) = walls
        .iter()
        .zip(&sums)
        .map(|(&w, &s)| (w, s))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap();
    eprintln!("side by side: {wall:.2} ms for {sum:.2} ms of prepares");
    assert!(sum >= 8.0, "the prepares took {sum:.2} ms");
    // Two different sleepers ran at the same time.
    let runs = runs.lock().unwrap();
    let overlapped = runs
        .iter()
        .any(|a| runs.iter().any(|b| a.0 != b.0 && a.1 < b.2 && b.1 < a.2));
    assert!(overlapped, "no two shared prepares ran at once");
}

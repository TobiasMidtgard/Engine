//! The water follows world shifts (a floating-origin move, a new planet's frame): straight
//! after the shift the `Water` query answers at a point's new position what it answered at its
//! old one, and the test area carries on as if nothing happened.

use engine_core::math::DQuat;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::demo;
use engine_water::{SharedWater, WaterPlugin};
use engine_world::{Water, WaterSample, WorldPlugin, WorldShift, WorldShiftSet};

const ORIGIN: DVec3 = DVec3::new(170.0, 0.0, -140.0);
/// The shift happens on this frame (the stream runs and boats move by then).
const SHIFT_FRAME: u32 = 90;

#[derive(Resource)]
struct Probe {
    shift: WorldShift,
    frame: u32,
    points: Vec<DVec3>,
    before: Vec<Option<WaterSample>>,
    after: Vec<Option<WaterSample>>,
}

/// Points every 2 m over the test area (lake, stream, falls, pond and dry hill).
fn points() -> Vec<DVec3> {
    let half = demo::AREA_HALF_SIZE as f64;
    let mut out = Vec::new();
    let mut z = -half;
    while z <= half {
        let mut x = -half;
        while x <= half {
            out.push(ORIGIN + DVec3::new(x, 0.0, z));
            x += 2.0;
        }
        z += 2.0;
    }
    out
}

fn sample_before(
    mut probe: ResMut<Probe>,
    water: Res<Water>,
    mut shifts: MessageWriter<WorldShift>,
) {
    probe.frame += 1;
    if probe.frame != SHIFT_FRAME {
        return;
    }
    probe.before = probe.points.iter().map(|p| water.0.sample(*p)).collect();
    shifts.write(probe.shift);
}

fn sample_after(mut probe: ResMut<Probe>, water: Res<Water>) {
    if probe.frame != SHIFT_FRAME {
        return;
    }
    let s = probe.shift;
    probe.after = probe
        .points
        .iter()
        .map(|p| water.0.sample(s.point(*p)))
        .collect();
}

fn run(shift: WorldShift, frames: u32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Probe {
        shift,
        frame: 0,
        points: points(),
        before: Vec::new(),
        after: Vec::new(),
    });
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, ORIGIN);
    });
    app.add_systems(
        PostUpdate,
        (
            sample_before.in_set(WorldShiftSet::Write),
            sample_after
                .after(WorldShiftSet::Apply)
                .before(UpdateSet::Propagate),
        ),
    );
    app.update_n(frames as usize);
    app
}

/// Largest differences between before and after: (surface height m, depth m, velocity m/s,
/// normal), and how many probes were wet.
fn compare(app: &App) -> ([f64; 4], usize) {
    let probe = app.world.resource::<Probe>();
    let s = probe.shift;
    assert_eq!(
        probe.before.len(),
        probe.points.len(),
        "the shift never ran"
    );
    let mut worst = [0.0f64; 4];
    let mut wet = 0;
    for ((p, b), a) in probe.points.iter().zip(&probe.before).zip(&probe.after) {
        let (b, a) = match (b, a) {
            (Some(b), Some(a)) => (b, a),
            (None, None) => continue,
            // Exactly on a shoreline, rounding decides.
            (Some(w), None) | (None, Some(w)) if w.depth < 1e-3 => continue,
            _ => panic!("water at {p:?}: before {b:?}, after {a:?}"),
        };
        wet += 1;
        let expected_height = s.point(DVec3::new(p.x, b.surface_height, p.z)).y;
        let diffs = [
            (a.surface_height - expected_height).abs(),
            (a.depth - b.depth).abs() as f64,
            (a.velocity - s.direction(b.velocity)).length() as f64,
            (a.normal - s.direction(b.normal)).length() as f64,
        ];
        for (w, d) in worst.iter_mut().zip(diffs) {
            *w = w.max(d);
        }
    }
    (worst, wet)
}

#[test]
fn a_floating_origin_move_keeps_every_drop_in_place() {
    let shift = WorldShift::translate(DVec3::new(-12_345.5, 0.0, 67_890.25));
    let app = run(shift, SHIFT_FRAME);
    let (worst, wet) = compare(&app);
    assert!(wet > 500, "only {wet} wet probes");
    assert!(worst[0] < 1e-3, "surface off by {:.5} m", worst[0]);
    assert!(worst[1] < 1e-3, "depth off by {:.5} m", worst[1]);
    assert!(worst[2] < 1e-3, "current off by {:.5} m/s", worst[2]);
    assert!(worst[3] < 1e-3, "normal off by {:.5}", worst[3]);
}

#[test]
fn a_turned_frame_turns_the_water_with_it() {
    let shift = WorldShift {
        rotation: DQuat::from_rotation_y(0.7),
        translation: DVec3::new(250.0, 0.0, -80.0),
        ..WorldShift::IDENTITY
    };
    let app = run(shift, SHIFT_FRAME);
    let (worst, wet) = compare(&app);
    assert!(wet > 500, "only {wet} wet probes");
    // Lakes and streams turn with their entities exactly; only ripples are resampled.
    assert!(worst[0] < 5e-3, "surface off by {:.4} m", worst[0]);
    assert!(worst[1] < 1e-3, "depth off by {:.5} m", worst[1]);
    assert!(worst[2] < 5e-3, "current off by {:.4} m/s", worst[2]);
    assert!(worst[3] < 5e-3, "normal off by {:.4}", worst[3]);
}

#[test]
fn the_test_area_carries_on_after_a_shift() {
    let t = DVec3::new(5_000.0, 0.0, -3_000.0);
    let mut app = run(WorldShift::translate(t), SHIFT_FRAME + 90);
    // No wakes from boats that "moved" 6 km in a frame, no runaway ripples.
    let shared = app.world.resource::<SharedWater>().clone();
    let state = shared.0.read();
    let peak = state.ripples.h.iter().fold(0.0f32, |m, h| m.max(h.abs()));
    assert!(peak.is_finite() && peak < 0.5, "ripples up to {peak} m");
    // The stream still runs into the pond where it was.
    let flow: f64 = state.flows.iter().map(|f| f.volume()).sum();
    assert!(flow > 1.0, "only {flow:.2} m³ flowing");
    drop(state);
    let water = app.world.resource::<Water>().clone();
    let mut q = app.world.query::<(&Name, &Transform)>();
    let boat = q
        .iter(&app.world)
        .find(|(n, _)| n.as_str() == "Motorboat")
        .map(|(_, t)| t.translation)
        .expect("motorboat");
    let on = water.0.sample(boat);
    assert!(
        on.is_some_and(|w| (boat.y - w.surface_height).abs() < 1.0),
        "motorboat at {boat:?} is off the water ({on:?})"
    );
    assert!((boat - (ORIGIN + t)).length() < 80.0, "boat at {boat:?}");
}

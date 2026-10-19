//! A sea wets its beach but not the low plain far behind it, and a sandbar raised out of the
//! water (a terrain edit) gets wet once the terrain says it changed.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_water::{WaterBed, WaterBody, WaterPlugin};
use engine_world::{
    Ground, GroundQuery, GroundSample, SurfaceId, SurfaceLayers, TerrainChanged, WorldPlugin,
};

/// Sea bed 5 m down, a beach rising 1 in 10 through the still level at x = 0, and a plain
/// 0.25 m above the water from x = 2.5 m on (low enough for the swash band, were it not so far
/// from the water). With `bar`, a sandbar out in the shallows rises to 0.1 m round (-10, 0).
struct Coast {
    bar: bool,
}

impl Coast {
    fn height(&self, x: f64, z: f64) -> f64 {
        let mut h = (x * 0.1).clamp(-5.0, 0.25);
        if self.bar {
            let d = ((x + 10.0).powi(2) + z * z).sqrt();
            h = h.max(0.1 - 0.5 * d);
        }
        h
    }
}

impl GroundQuery for Coast {
    fn sample(&self, p: DVec3) -> Option<GroundSample> {
        let e = 0.25;
        let (hx, hz) = (
            self.height(p.x + e, p.z) - self.height(p.x - e, p.z),
            self.height(p.x, p.z + e) - self.height(p.x, p.z - e),
        );
        Some(GroundSample {
            height: self.height(p.x, p.z),
            normal: Vec3::new(-hx as f32, 2.0 * e as f32, -hz as f32).normalize(),
            surface: SurfaceId(3),
        })
    }
}

fn wet(app: &App, x: f64, z: f64) -> f32 {
    app.world
        .resource::<SurfaceLayers>()
        .sample(DVec3::new(x, 0.0, z))
        .wetness
}

#[test]
fn the_sea_wets_its_beach_not_the_plain_behind() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((WorldPlugin, WaterPlugin));
    app.insert_resource(Ground(Arc::new(Coast { bar: false })));
    app.add_systems(Startup, |mut commands: Commands| {
        commands.spawn((
            Name::new("Sea"),
            Transform::default(),
            WaterBody::ocean().with_bed(WaterBed::Ground),
        ));
    });
    app.update_n(8 * 30);

    assert!(wet(&app, 0.5, 0.5) > 0.9, "beach {}", wet(&app, 0.5, 0.5));
    assert!(
        wet(&app, 10.5, 0.5) > 0.5,
        "top of the beach {}",
        wet(&app, 10.5, 0.5)
    );
    assert_eq!(wet(&app, 60.5, 0.5), 0.0, "the plain far from the water");
    assert_eq!(wet(&app, -10.5, 0.5), 0.0, "sea bed");

    // A sandbar rises out of the shallows.
    app.insert_resource(Ground(Arc::new(Coast { bar: true })));
    app.world.write_message(TerrainChanged {
        center: DVec3::new(-10.0, 0.0, 0.0),
        radius: 1.5,
    });
    app.update_n(3 * 30);
    assert!(
        wet(&app, -10.5, 0.5) > 0.9,
        "sandbar {}",
        wet(&app, -10.5, 0.5)
    );
}

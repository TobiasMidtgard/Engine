//! A soft body landing tells the world what it landed on: the surface of the ground it
//! already asked about, so receivers (prints, dust) need not ask the ground again.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_softbody::{SoftBodyPlugin, SoftMaterial, SoftVolume};
use engine_world::{ContactKind, GroundContact, SurfaceId};

#[derive(Resource, Default)]
struct Landings(Vec<GroundContact>);

fn listen(mut contacts: MessageReader<GroundContact>, mut seen: ResMut<Landings>) {
    seen.0.extend(
        contacts
            .read()
            .filter(|c| c.kind == ContactKind::Body)
            .cloned(),
    );
}

#[test]
fn a_dropped_jelly_lands_on_the_grounds_surface() {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.init_resource::<Landings>();
    app.add_systems(Update, listen);
    // Two metres up over the default flat ground (grass, surface 3).
    app.world.spawn((
        Transform::from_xyz(0.0, 2.0, 0.0),
        SoftVolume::jelly(0.6, SoftMaterial::hex(0x10c040)),
    ));
    app.update_n(90);
    let landings = &app.world.resource::<Landings>().0;
    assert!(!landings.is_empty(), "it landed");
    assert!(
        landings.iter().all(|c| c.surface == Some(SurfaceId(3))),
        "each landing names the ground's surface: {:?}",
        landings.iter().map(|c| c.surface).collect::<Vec<_>>()
    );
}

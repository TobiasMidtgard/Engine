//! Following the world when it is re-anchored: a floating-origin move, or a new reference frame
//! (`engine_world::WorldShift`). engine_world moves the shared state fire writes to (`HeatField`,
//! `SurfaceLayers`, `WindField`), the entities (objects, fuel patches, the fire's lights) and
//! pending `IgniteRequest`s; this moves what fire keeps in world space of its own, in
//! `WorldShiftSet::Apply`, so the same ground keeps burning in the new frame.
//!
//! * The fire field moves its origin (exact, at any distance) or, when the world turns, is
//!   resampled into the new frame (see `FireField::shift`).
//! * Embers move with `WorldShift::point` and pick up the old frame's motion with
//!   `WorldShift::velocity_at`; hot landings waiting for the step move too.
//! * Placed fuel patches, light targets and queued API commands move with the world.
//! * Nothing else needs it: objects, the demo and the renderer read positions from transforms or
//!   the field every time.

use engine_core::prelude::*;
use engine_world::{SimulationFocus, WorldShift};

use crate::embers::Embers;
use crate::field::FireField;
use crate::sim::{FireCommand, FireCommands, FireLights, FirePatches};

/// All shifts written this frame, combined (they apply in order).
fn combined(shifts: &mut MessageReader<WorldShift>) -> Option<WorldShift> {
    shifts.read().copied().reduce(|acc, next| acc.then(&next))
}

/// Move the fire field, embers, placed patches, light targets and queued commands.
pub(crate) fn shift_fire(
    mut shifts: MessageReader<WorldShift>,
    mut field: ResMut<FireField>,
    mut embers: ResMut<Embers>,
    mut patches: ResMut<FirePatches>,
    mut lights: ResMut<FireLights>,
    mut queued: ResMut<FireCommands>,
    foci: Query<(&GlobalTransform, &SimulationFocus)>,
) {
    let Some(s) = combined(&mut shifts) else {
        return;
    };
    // `GlobalTransform` still holds the old frame until transforms propagate after the shift.
    let focus = foci
        .iter()
        .max_by_key(|(_, f)| f.priority)
        .map(|(g, _)| g.translation());
    field.shift(&s, focus);
    for e in &mut embers.list {
        e.position = s.point(e.position);
        e.velocity = s.velocity_at(e.position, e.velocity);
    }
    for (p, _) in &mut embers.landings {
        *p = s.point(*p);
    }
    for p in &mut patches.list {
        p.center = s.point(p.center);
        let x = s.direction(Vec3::new(p.axis_x.x, 0.0, p.axis_x.y));
        p.axis_x = Vec2::new(x.x, x.z).normalize_or(p.axis_x);
    }
    for t in &mut lights.targets {
        t.position = s.point(t.position);
    }
    for c in &mut queued.0 {
        match c {
            FireCommand::Ignite { at, .. } | FireCommand::Extinguish { at, .. } => {
                *at = s.point(*at);
            }
            FireCommand::Clear => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use engine_core::math::{DQuat, DVec3};
    use engine_core::prelude::*;
    use engine_world::{WorldShift, WorldShiftSet};

    use crate::field::CellState;
    use crate::{
        Ember, Embers, FireCommand, FireCommands, FireField, FirePlugin, FuelKind, FuelPatch,
        PatchShape,
    };

    /// Shifts to write in the next `WorldShiftSet::Write`, and the fire as it was right then
    /// (exactly what `Apply` gets to move).
    #[derive(Resource, Default)]
    struct Plan {
        shifts: Vec<WorldShift>,
        before: Option<(FireField, Embers)>,
    }

    fn write_planned(world: &mut World) {
        let shifts = std::mem::take(&mut world.resource_mut::<Plan>().shifts);
        if shifts.is_empty() {
            return;
        }
        let before = (
            world.resource::<FireField>().clone(),
            world.resource::<Embers>().clone(),
        );
        world.resource_mut::<Plan>().before = Some(before);
        for s in shifts {
            world.write_message(s);
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(FirePlugin);
        app.init_resource::<Plan>();
        app.add_systems(PostUpdate, write_planned.in_set(WorldShiftSet::Write));
        app
    }

    const FIRE: DVec3 = DVec3::new(3.0, 0.0, -2.0);

    /// A grass fire a few seconds along, with an ember in the air.
    fn burning() -> App {
        let mut app = app();
        app.world.resource_mut::<engine_world::WeatherState>().wind = Vec3::new(2.0, 0.0, 1.0);
        app.world
            .resource_mut::<FireCommands>()
            .0
            .push(FireCommand::Ignite {
                at: FIRE,
                radius: 2.5,
                temperature_c: 900.0,
            });
        app.update_n(90);
        app.world.resource_mut::<Embers>().list.push(Ember {
            position: DVec3::new(4.0, 6.0, 1.0),
            velocity: Vec3::new(1.0, 0.5, -2.0),
            heat: 1.0,
            age: 0.0,
            lift: 0.0,
            size: 0.01,
            life: 30.0,
            ignites: false,
        });
        assert!(app.world.resource::<FireField>().stats.burning_cells > 20);
        app
    }

    /// Shift in the next frame; the fire and embers just before it.
    fn shift(app: &mut App, s: WorldShift) -> (FireField, Embers) {
        app.world.resource_mut::<Plan>().shifts.push(s);
        app.update();
        app.world
            .resource_mut::<Plan>()
            .before
            .take()
            .expect("the shift went out")
    }

    /// Every cell that is burning or has burned, by world position (old frame).
    fn fire_cells(field: &FireField) -> Vec<(DVec3, CellState, f32, f32)> {
        field
            .cells_near(FIRE, 100.0)
            .filter(|(_, c)| !matches!(c.state(), CellState::Bare | CellState::Intact))
            .map(|(p, c)| (p, c.state(), c.burn, c.scorch))
            .collect()
    }

    fn burning_cells(field: &FireField) -> usize {
        field
            .tiles
            .iter()
            .flat_map(|t| &t.cells)
            .filter(|c| c.is_burning())
            .count()
    }

    #[test]
    fn a_translated_fire_is_exactly_where_it_was() {
        let mut app = burning();
        let s = WorldShift::translate(DVec3::new(1000.3, 2.0, -500.7));
        let (before, embers) = shift(&mut app, s);
        let after = app.world.resource::<FireField>();
        let cells = fire_cells(&before);
        assert!(cells.iter().any(|c| c.1 == CellState::Burning));
        for &(p, state, burn, scorch) in &cells {
            let c = after
                .cell_at(s.point(p))
                .expect("the cell moved with the fire");
            assert_eq!(
                (c.state(), c.burn, c.scorch),
                (state, burn, scorch),
                "at {p}"
            );
            // On the same ground, which moved up with the world.
            let (key, local) = FireField::split(after.cell_coord(s.point(p)));
            let world = after.cell_world(after.tile(key).unwrap(), local);
            assert!(
                (world - s.point(p)).length() < 1e-6,
                "{world} vs {}",
                s.point(p)
            );
        }
        assert_eq!(burning_cells(after), burning_cells(&before));
        // Nothing is left where the fire was.
        assert!(after.cell_at(FIRE).is_none());
        // The API sees it at the new place.
        let state = format!("{:?}", before.cell_at(FIRE).unwrap().state());
        let sample = engine_core::registry::call_api(
            &mut app.world,
            "fire.sample",
            serde_json::json!({"at": s.point(FIRE).to_array()}),
        )
        .unwrap();
        assert_eq!(sample["state"], state.as_str(), "{sample}");
        // The ember moved with it.
        let (old, new) = (embers.list[0], app.world.resource::<Embers>().list[0]);
        assert!((new.position - s.point(old.position)).length() < 1e-9);
        assert!((new.velocity - old.velocity).length() < 1e-6);
    }

    #[test]
    fn a_translated_fire_burns_on_as_if_nothing_happened() {
        let s = WorldShift::translate(DVec3::new(1000.3, 0.0, -500.7));
        let mut shifted = burning();
        shift(&mut shifted, s);
        let mut control = burning();
        control.update();
        shifted.update_n(60);
        control.update_n(60);
        let a = shifted.world.resource::<FireField>();
        let b = control.world.resource::<FireField>();
        assert!(
            (a.stats.burning_m2 - b.stats.burning_m2).abs() <= 0.1 * b.stats.burning_m2,
            "{:?} vs {:?}",
            a.stats,
            b.stats
        );
        // Burning around the moved place, and nothing near the old one.
        assert!(
            a.cells_near(s.point(FIRE), 20.0)
                .any(|(_, c)| c.is_burning())
        );
        assert!(a.cells_near(FIRE, 100.0).next().is_none());
    }

    #[test]
    fn a_quarter_turn_keeps_the_fire_where_it_was() {
        let mut app = burning();
        let s = WorldShift {
            rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            translation: DVec3::new(37.3, 1.5, -12.9),
            ..WorldShift::IDENTITY
        };
        let (before, embers) = shift(&mut app, s);
        let after = app.world.resource::<FireField>();
        let cell = after.cell_m as f64;
        let cells = fire_cells(&before);
        assert!(cells.iter().any(|c| c.1 == CellState::Burning));
        for &(p, state, _, _) in &cells {
            // The same state within a cell or two of where the cell went.
            let q = s.point(p);
            let found = (-2..=2).any(|dz| {
                (-2..=2).any(|dx| {
                    let at = q + DVec3::new(dx as f64, 0.0, dz as f64) * cell;
                    after.cell_at(at).is_some_and(|c| c.state() == state)
                })
            });
            assert!(found, "{state:?} at {p} went missing");
        }
        assert_eq!(burning_cells(after), burning_cells(&before));
        // The ember turned with the world.
        let (old, new) = (embers.list[0], app.world.resource::<Embers>().list[0]);
        assert!((new.position - s.point(old.position)).length() < 1e-9);
        let v = s.direction(old.velocity);
        assert!(
            (new.velocity - v).length() < 1e-5,
            "{} vs {v}",
            new.velocity
        );
    }

    #[test]
    fn fuel_patches_moved_by_a_shift_are_not_refueled() {
        let mut app = app();
        app.world.spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            FuelPatch::grass(PatchShape::Disc { radius: 8.0 }),
        ));
        app.update();
        // Make the tiles under it, then soak part of it.
        let wet = DVec3::new(-3.0, 0.0, 1.0);
        app.world.resource_mut::<FireCommands>().0.extend([
            FireCommand::Ignite {
                at: wet,
                radius: 3.0,
                temperature_c: 100.0,
            },
            FireCommand::Extinguish {
                at: wet,
                radius: 2.0,
                water_kg_m2: 2.0,
            },
        ]);
        app.update_n(3);
        let soaked = app
            .world
            .resource::<FireField>()
            .cell_at(wet)
            .unwrap()
            .moisture;
        assert!(soaked > 1.0, "{soaked}");
        let (mut wet, mut center) = (wet, DVec3::ZERO);
        for s in [
            WorldShift::translate(DVec3::new(1000.3, 2.0, -500.7)),
            WorldShift {
                rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
                ..WorldShift::IDENTITY
            },
        ] {
            shift(&mut app, s);
            app.update_n(3);
            (wet, center) = (s.point(wet), s.point(center));
            let field = app.world.resource::<FireField>();
            let moisture = field.cell_at(wet).unwrap().moisture;
            assert!(moisture > soaked * 0.9, "refueled: {soaked} -> {moisture}");
            let patch = app.world.resource::<crate::sim::FirePatches>().list[0];
            assert!(patch.center.distance(center) < 1e-6);
            assert!(patch.contains(wet));
        }
    }

    #[test]
    fn a_turned_fire_gets_fuel_where_the_turn_left_none_and_burns_on() {
        let s = WorldShift {
            rotation: DQuat::from_rotation_y(0.6),
            translation: DVec3::new(-250.25, 0.0, 80.1),
            ..WorldShift::IDENTITY
        };
        let mut turned = burning();
        let (before, _) = shift(&mut turned, s);
        let fuelless = |f: &FireField| {
            f.tiles
                .iter()
                .flat_map(|t| &t.cells)
                .filter(|c| c.kind == FuelKind::None)
                .count()
        };
        // The default ground is grass everywhere; the turned tiles stick out of the old ones.
        assert_eq!(fuelless(&before), 0);
        assert!(fuelless(turned.world.resource::<FireField>()) > 0);
        let mut control = burning();
        control.update();
        turned.update_n(60);
        control.update_n(60);
        let a = turned.world.resource::<FireField>();
        let b = control.world.resource::<FireField>();
        assert_eq!(fuelless(a), 0);
        assert!(
            (a.stats.burning_m2 - b.stats.burning_m2).abs() <= 0.2 * b.stats.burning_m2,
            "{:?} vs {:?}",
            a.stats,
            b.stats
        );
        assert!(
            a.cells_near(s.point(FIRE), 20.0)
                .any(|(_, c)| c.is_burning())
        );
    }
}

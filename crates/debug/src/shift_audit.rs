//! World shift audit: finds what doesn't follow a re-anchor.
//!
//! With `ENGINE_SHIFT_AUDIT=<frame>:<x>,<y>,<z>` (meters, e.g. `600:0,2000,0`) the engine writes
//! one [`WorldShift`] of that translation at that frame; with `<frame>:rehome:<meters>` (e.g.
//! `600:rehome:40000`) one that turns the world as the space module does when the home spot
//! moves that far north along the ground (about the planet's center, 6371 km below the
//! origin), so the old valley tilts and sinks. Every entity with a [`GlobalTransform`] should
//! end up where the shift maps it. With `<frame>:trip:<body>` (e.g. `600:trip:Moon`) it makes
//! the trip a player makes instead: `space.travel` to that body, and home again 60 frames
//! later, after which everything should be back where it was. With `<frame>:walk:<meters>`
//! (e.g. `600:walk:45000`) it moves the focus that far north and audits the shift the space
//! module makes for it, the re-home a long flight makes.
//!
//! A plain or `rehome` test shift doesn't tell the space module, so its planet, its ground
//! and what stands on that ground stay put: a walker snapped to the ground shows as "stayed"
//! there without being wrong. Confirm a culprit with `walk` or `trip`. Some time later the audit logs
//! the ones that didn't, grouped by their module's components:
//!
//! * "stayed": still near its old coordinates, so something re-places it from a world point
//!   that wasn't shifted (a demo path, a pivot, a home, a cached target);
//! * "went elsewhere": far from both, usually a world point shifted twice, or one that was
//!   moved but not turned;
//! * "floating" / "buried" (walks and trips): now more than 20 m higher or lower above the
//!   ground than before (things hanging in the sky after a trip);
//! * "appeared": entities spawned after the shift, by kind, a duplicate when a streamer still
//!   thinks the camera is where it was.
//!
//! The space module's own bodies and far woods follow its frame, which this shift doesn't
//! tell it about, so they show up too; ignore them.
//!
//! Fix one by registering the world points with `app.shift_components::<C>()` or
//! `shift_resource::<R>()` (engine_world, AGENTS.md "Units and coordinates").

use std::collections::HashMap;

use engine_core::math::{DQuat, DVec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_world::{Ground, SimulationFocus, WorldShift, WorldShiftSet};

/// Frames between the shift and each report.
const REPORT_AFTER: [u32; 2] = [60, 600];
/// Kinds listed per report.
const ROWS: usize = 25;
/// The planet's radius for `rehome`, meters (Earth's).
const PLANET_RADIUS_M: f64 = 6_371_000.0;
/// A report row: how many, where one of them is, and how far that one is from where it
/// should be (for "floating" and "buried", its change in height above the ground).
type Row = (usize, DVec3, DVec3);

/// Frames a `trip` stays away.
const TRIP_FRAMES: u32 = 60;

#[derive(Debug, Clone, PartialEq)]
enum Plan {
    Shift(WorldShift),
    /// To this body and home again.
    Trip(String),
    /// Move the focus (camera, player) this far north, meters, and audit the shift the space
    /// module makes for it (a re-home past `rehome_distance_m`, 40 km by default).
    Walk(f64),
}

#[derive(Resource)]
struct ShiftAudit {
    frame: u32,
    plan: Plan,
    /// Each entity's world position just before the shift.
    before: HashMap<Entity, DVec3>,
    /// Entities per kind just before the shift.
    kinds_before: HashMap<String, usize>,
    /// Height above the ground just before the shift, meters, where there is ground.
    heights_before: HashMap<Entity, f64>,
    frames: u32,
    /// The frame the world shifted and how (everything shifted that frame), once it has.
    applied: Option<(u32, WorldShift)>,
}

/// Parse `"<frame>:<x>,<y>,<z>"`, `"<frame>:rehome:<meters>"` or `"<frame>:trip:<body>"`.
fn parse(spec: &str) -> Option<(u32, Plan)> {
    let (frame, v) = spec.split_once(':')?;
    let frame = frame.trim().parse().ok()?;
    if let Some(d) = v.trim().strip_prefix("walk:") {
        return Some((frame, Plan::Walk(d.trim().parse().ok()?)));
    }
    if let Some(body) = v.trim().strip_prefix("trip:") {
        return Some((frame, Plan::Trip(body.trim().to_string())));
    }
    if let Some(d) = v.trim().strip_prefix("rehome:") {
        // The spot `d` north (+Z) comes to the origin: a turn about +X through the center.
        let angle = d.trim().parse::<f64>().ok()? / PLANET_RADIUS_M;
        let rotation = DQuat::from_rotation_x(-angle);
        let center = DVec3::new(0.0, -PLANET_RADIUS_M, 0.0);
        let shift = WorldShift {
            rotation,
            translation: center - rotation * center,
            ..WorldShift::IDENTITY
        };
        return Some((frame, Plan::Shift(shift)));
    }
    let mut parts = v.split(',').map(|p| p.trim().parse::<f64>());
    let (x, y, z) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    Some((
        frame,
        Plan::Shift(WorldShift::translate(DVec3::new(x, y, z))),
    ))
}

/// Install the audit when `ENGINE_SHIFT_AUDIT` asks for it.
pub(crate) fn install(app: &mut App) {
    let Ok(spec) = std::env::var("ENGINE_SHIFT_AUDIT") else {
        return;
    };
    let Some((frame, plan)) = parse(&spec) else {
        engine_core::log::warn!(
            "ENGINE_SHIFT_AUDIT={spec:?} not understood; use <frame>:<x>,<y>,<z> (e.g. \
             600:0,2000,0), <frame>:rehome:<meters> (e.g. 600:rehome:40000), \
             <frame>:walk:<meters> (e.g. 600:walk:45000) or <frame>:trip:<body> (e.g. \
             600:trip:Moon)"
        );
        return;
    };
    app.insert_resource(ShiftAudit {
        frame,
        plan,
        before: HashMap::new(),
        kinds_before: HashMap::new(),
        heights_before: HashMap::new(),
        frames: 0,
        applied: None,
    })
    .add_systems(PostUpdate, write_shift.in_set(WorldShiftSet::Write))
    .add_systems(PostUpdate, catch_shift.in_set(WorldShiftSet::Apply))
    .add_systems(Last, report);
}

/// The module an entity belongs to and its components there, e.g.
/// `engine_anim: Animator, DemoPath`. Core, render and world components are left out.
fn kind(world: &World, entity: Entity) -> String {
    let Ok(components) = world.inspect_entity(entity) else {
        return String::new();
    };
    let mut by_crate: HashMap<String, Vec<String>> = HashMap::new();
    for (_, info) in components {
        let name = info.name().to_string();
        let path = name.split('<').next().unwrap_or(&name);
        let krate = path.split("::").next().unwrap_or("");
        if matches!(
            krate,
            "engine_core" | "engine_render" | "engine_world" | "bevy_ecs"
        ) {
            continue;
        }
        let short = path.rsplit("::").next().unwrap_or(path).to_string();
        by_crate.entry(krate.to_string()).or_default().push(short);
    }
    let Some((krate, mut names)) = by_crate.into_iter().max_by_key(|(_, n)| n.len()) else {
        return "(core components only)".into();
    };
    names.sort();
    names.truncate(4);
    format!("{krate}: {}", names.join(", "))
}

fn write_shift(world: &mut World) {
    let (frames, frame, plan) = {
        let mut audit = world.resource_mut::<ShiftAudit>();
        audit.frames += 1;
        (audit.frames, audit.frame, audit.plan.clone())
    };
    if let Plan::Trip(_) = plan
        && frames == frame + TRIP_FRAMES
    {
        let back = call_api(world, "space.travel", serde_json::json!({ "home": true }));
        engine_core::log::info!("shift audit: coming home: {back:?}");
        // Back home, everything should be where it was.
        world.resource_mut::<ShiftAudit>().applied = Some((frames, WorldShift::IDENTITY));
        return;
    }
    if frames != frame {
        return;
    }
    let mut placed = world.query::<(Entity, &GlobalTransform)>();
    let before: HashMap<Entity, DVec3> = placed
        .iter(world)
        .map(|(e, g)| (e, g.0.translation))
        .collect();
    let mut kinds_before: HashMap<String, usize> = HashMap::new();
    for &e in before.keys() {
        *kinds_before.entry(kind(world, e)).or_default() += 1;
    }
    let heights_before = heights(world, &before);
    match plan {
        Plan::Shift(shift) => {
            engine_core::log::info!(
                "shift audit: shifting the world by {:?}, turning {:.3}°, at frame {frame} \
                 ({} placed entities)",
                shift.translation,
                shift.rotation.angle_between(DQuat::IDENTITY).to_degrees(),
                before.len()
            );
            world.write_message(shift);
        }
        Plan::Walk(d) => {
            let mut foci =
                world.query_filtered::<&mut Transform, (With<SimulationFocus>, Without<ChildOf>)>();
            for mut t in foci.iter_mut(world) {
                t.translation.z += d;
            }
            engine_core::log::info!(
                "shift audit: moving the focus {d:.0} m north at frame {frame} ({} placed \
                 entities)",
                before.len()
            );
        }
        Plan::Trip(body) => {
            let went = call_api(world, "space.travel", serde_json::json!({ "body": body }));
            engine_core::log::info!(
                "shift audit: going to {body} at frame {frame} ({} placed entities): {went:?}",
                before.len()
            );
        }
    }
    let mut audit = world.resource_mut::<ShiftAudit>();
    audit.before = before;
    audit.kinds_before = kinds_before;
    audit.heights_before = heights_before;
}

/// `WorldShiftSet::Apply`: remember the first shift after the audit's frame (not for trips,
/// which come back to where they started).
fn catch_shift(mut shifts: MessageReader<WorldShift>, mut audit: ResMut<ShiftAudit>) {
    let all = shifts.read().copied().reduce(|a, b| a.then(&b));
    if audit.applied.is_some() || audit.frames < audit.frame || matches!(audit.plan, Plan::Trip(_))
    {
        return;
    }
    if let Some(shift) = all {
        let frame = audit.frames;
        engine_core::log::info!(
            "shift audit: the world shifted at frame {frame} by {:?}, turning {:.3}°",
            shift.translation,
            shift.rotation.angle_between(DQuat::IDENTITY).to_degrees(),
        );
        audit.applied = Some((frame, shift));
    }
}

/// Height above the ground (along +Y) of each placed entity over ground.
fn heights(world: &World, placed: &HashMap<Entity, DVec3>) -> HashMap<Entity, f64> {
    let Some(ground) = world.get_resource::<Ground>() else {
        return HashMap::new();
    };
    placed
        .iter()
        .filter_map(|(&e, &p)| ground.height(p).map(|h| (e, p.y - h)))
        .collect()
}

fn report(world: &mut World) {
    let (frames, (shifted_at, shift)) = {
        let a = world.resource::<ShiftAudit>();
        let Some(applied) = a.applied else {
            return;
        };
        (a.frames, applied)
    };
    let Some(after) = frames.checked_sub(shifted_at) else {
        return;
    };
    if !REPORT_AFTER.contains(&after) {
        return;
    }
    let mut placed = world.query::<(Entity, &GlobalTransform)>();
    let now: Vec<(Entity, DVec3)> = placed
        .iter(world)
        .map(|(e, g)| (e, g.0.translation))
        .collect();
    // A test shift doesn't move the planet's ground; a trip or a walk does.
    let real = !matches!(world.resource::<ShiftAudit>().plan, Plan::Shift(_));
    let heights_now = if real {
        heights(world, &now.iter().copied().collect())
    } else {
        HashMap::new()
    };
    let audit = world.resource::<ShiftAudit>();
    let mut floating: HashMap<String, Row> = HashMap::new();
    let mut buried: HashMap<String, Row> = HashMap::new();
    for &(e, p) in &now {
        let (Some(was), Some(is)) = (audit.heights_before.get(&e), heights_now.get(&e)) else {
            continue;
        };
        let bucket = match is - was {
            d if d > 20.0 => &mut floating,
            d if d < -20.0 => &mut buried,
            _ => continue,
        };
        let off = DVec3::new(0.0, is - was, 0.0);
        bucket.entry(kind(world, e)).or_insert((0, p, off)).0 += 1;
    }
    let mut stayed: HashMap<String, Row> = HashMap::new();
    let mut elsewhere: HashMap<String, Row> = HashMap::new();
    let mut kinds_now: HashMap<String, usize> = HashMap::new();
    let mut followed = 0;
    for &(e, p) in &now {
        let k = kind(world, e);
        *kinds_now.entry(k.clone()).or_default() += 1;
        let Some(&old) = audit.before.get(&e) else {
            continue;
        };
        let expected = shift.point(old);
        let moved = (expected - old).length();
        // Half the way it should have moved (at least a meter) tells the buckets apart; what
        // should have hardly moved can't be told.
        let far = (moved * 0.5).max(1.0);
        let bucket = if moved < 2.0 {
            // A trip: back where it was, or not.
            if (p - expected).length() > 2.0 && shift == WorldShift::IDENTITY {
                &mut elsewhere
            } else {
                followed += 1;
                continue;
            }
        } else if (p - old).length() < far {
            &mut stayed
        } else if (p - expected).length() > far {
            &mut elsewhere
        } else {
            followed += 1;
            continue;
        };
        let row = bucket.entry(k).or_insert((0, p, p - expected));
        row.0 += 1;
    }
    let mut out = format!(
        "shift audit, {after} frames after shifting by {:?}: {followed} followed, {} stayed at \
         their old coordinates, {} went elsewhere, {} floating 20 m higher above the ground, {} \
         buried\n",
        shift.translation,
        stayed.values().map(|r| r.0).sum::<usize>(),
        elsewhere.values().map(|r| r.0).sum::<usize>(),
        floating.values().map(|r| r.0).sum::<usize>(),
        buried.values().map(|r| r.0).sum::<usize>(),
    );
    for (title, bucket) in [
        ("stayed", &stayed),
        ("went elsewhere", &elsewhere),
        ("floating", &floating),
        ("buried", &buried),
    ] {
        let mut rows: Vec<_> = bucket.iter().collect();
        rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (k, (n, p, off)) in rows.into_iter().take(ROWS) {
            out.push_str(&format!(
                "  {title}: {n:>5} x {k} (one at {:.0} {:.0} {:.0}, {:.1} m off: {:.1} {:.1} \
                 {:.1})\n",
                p.x,
                p.y,
                p.z,
                off.length(),
                off.x,
                off.y,
                off.z
            ));
        }
    }
    let mut grown: Vec<(String, usize, usize)> = kinds_now
        .iter()
        .filter_map(|(k, &n)| {
            let was = audit.kinds_before.get(k).copied().unwrap_or(0);
            (n > was + was / 10 + 2).then(|| (k.clone(), was, n))
        })
        .collect();
    grown.sort_by_key(|g| std::cmp::Reverse(g.2 - g.1));
    for (k, was, n) in grown.into_iter().take(ROWS) {
        out.push_str(&format!("  appeared: {k}: {was} -> {n}\n"));
    }
    engine_core::log::info!("{out}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse() {
        let moved = |spec: &str| match parse(spec) {
            Some((f, Plan::Shift(s))) => Some((f, s.translation)),
            _ => None,
        };
        assert_eq!(
            moved("600:0,2000,0"),
            Some((600, DVec3::new(0.0, 2000.0, 0.0)))
        );
        assert_eq!(
            moved(" 5 : 1.5, -2, 3 "),
            Some((5, DVec3::new(1.5, -2.0, 3.0)))
        );
        assert_eq!(parse("9:trip: Moon"), Some((9, Plan::Trip("Moon".into()))));
        assert_eq!(parse("9:walk:45000"), Some((9, Plan::Walk(45_000.0))));
        assert_eq!(parse("600"), None);
        assert_eq!(parse("x:1,2,3"), None);
    }

    #[test]
    fn rehome_brings_the_spot_north_to_the_origin() {
        let Some((frame, Plan::Shift(s))) = parse("10:rehome:40000") else {
            panic!("not a shift");
        };
        assert_eq!(frame, 10);
        let a = 40_000.0 / PLANET_RADIUS_M;
        let center = DVec3::new(0.0, -PLANET_RADIUS_M, 0.0);
        let spot = center + DVec3::new(0.0, a.cos(), a.sin()) * PLANET_RADIUS_M;
        assert!(s.point(spot).length() < 1e-3, "{:?}", s.point(spot));
        // The old origin is now about 40 km south and sunk below the horizon.
        let old = s.point(DVec3::ZERO);
        assert!((old.z + 40_000.0).abs() < 10.0, "{old:?}");
        assert!(old.y < -100.0, "{old:?}");
    }
}

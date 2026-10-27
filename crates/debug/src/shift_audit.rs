//! World shift audit: finds what doesn't follow a re-anchor.
//!
//! With `ENGINE_SHIFT_AUDIT=<frame>:<x>,<y>,<z>` (meters, e.g. `600:0,2000,0`) the engine writes
//! one [`WorldShift`] of that translation at that frame, as the space module does when the
//! focus goes to orbit and back. Every entity with a [`GlobalTransform`] should end up moved by
//! exactly that much. Some time later the audit logs the ones that didn't, grouped by their
//! module's components:
//!
//! * "stayed": still near its old coordinates, so something re-places it from a world point
//!   that wasn't shifted (a demo path, a pivot, a home, a cached target);
//! * "went elsewhere": far from both, usually a world point shifted twice;
//! * "appeared": entities spawned after the shift, by kind, a duplicate when a streamer still
//!   thinks the camera is where it was.
//!
//! Fix one by registering the world points with `app.shift_components::<C>()` or
//! `shift_resource::<R>()` (engine_world, AGENTS.md "Units and coordinates").

use std::collections::HashMap;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::{WorldShift, WorldShiftSet};

/// Frames between the shift and each report.
const REPORT_AFTER: [u32; 2] = [60, 600];
/// Kinds listed per report.
const ROWS: usize = 25;

#[derive(Resource)]
struct ShiftAudit {
    frame: u32,
    translation: DVec3,
    /// Each entity's world position just before the shift.
    before: HashMap<Entity, DVec3>,
    /// Entities per kind just before the shift.
    kinds_before: HashMap<String, usize>,
    frames: u32,
}

/// Parse `"<frame>:<x>,<y>,<z>"`.
fn parse(spec: &str) -> Option<(u32, DVec3)> {
    let (frame, v) = spec.split_once(':')?;
    let mut parts = v.split(',').map(|p| p.trim().parse::<f64>());
    let (x, y, z) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    Some((frame.trim().parse().ok()?, DVec3::new(x, y, z)))
}

/// Install the audit when `ENGINE_SHIFT_AUDIT` asks for it.
pub(crate) fn install(app: &mut App) {
    let Ok(spec) = std::env::var("ENGINE_SHIFT_AUDIT") else {
        return;
    };
    let Some((frame, translation)) = parse(&spec) else {
        engine_core::log::warn!(
            "ENGINE_SHIFT_AUDIT={spec:?} not understood; use <frame>:<x>,<y>,<z>, e.g. 600:0,2000,0"
        );
        return;
    };
    app.insert_resource(ShiftAudit {
        frame,
        translation,
        before: HashMap::new(),
        kinds_before: HashMap::new(),
        frames: 0,
    })
    .add_systems(PostUpdate, write_shift.in_set(WorldShiftSet::Write))
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
    let (due, translation) = {
        let mut audit = world.resource_mut::<ShiftAudit>();
        audit.frames += 1;
        (audit.frames == audit.frame, audit.translation)
    };
    if !due {
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
    engine_core::log::info!(
        "shift audit: shifting the world by {translation:?} at frame {} ({} placed entities)",
        world.resource::<ShiftAudit>().frame,
        before.len()
    );
    world.write_message(WorldShift::translate(translation));
    let mut audit = world.resource_mut::<ShiftAudit>();
    audit.before = before;
    audit.kinds_before = kinds_before;
}

fn report(world: &mut World) {
    let (shifted_at, frames, translation) = {
        let a = world.resource::<ShiftAudit>();
        (a.frame, a.frames, a.translation)
    };
    let Some(after) = frames.checked_sub(shifted_at) else {
        return;
    };
    if !REPORT_AFTER.contains(&after) {
        return;
    }
    let far = translation.length() * 0.5;
    let mut placed = world.query::<(Entity, &GlobalTransform)>();
    let now: Vec<(Entity, DVec3)> = placed
        .iter(world)
        .map(|(e, g)| (e, g.0.translation))
        .collect();
    let audit = world.resource::<ShiftAudit>();
    let mut stayed: HashMap<String, (usize, DVec3)> = HashMap::new();
    let mut elsewhere: HashMap<String, (usize, DVec3)> = HashMap::new();
    let mut kinds_now: HashMap<String, usize> = HashMap::new();
    let mut followed = 0;
    for &(e, p) in &now {
        let k = kind(world, e);
        *kinds_now.entry(k.clone()).or_default() += 1;
        let Some(&old) = audit.before.get(&e) else {
            continue;
        };
        let expected = old + translation;
        let bucket = if (p - old).length() < far {
            &mut stayed
        } else if (p - expected).length() > far {
            &mut elsewhere
        } else {
            followed += 1;
            continue;
        };
        let row = bucket.entry(k).or_insert((0, p));
        row.0 += 1;
    }
    let mut out = format!(
        "shift audit, {after} frames after shifting by {translation:?}: {followed} followed, \
         {} stayed at their old coordinates, {} went elsewhere\n",
        stayed.values().map(|r| r.0).sum::<usize>(),
        elsewhere.values().map(|r| r.0).sum::<usize>(),
    );
    for (title, bucket) in [("stayed", &stayed), ("went elsewhere", &elsewhere)] {
        let mut rows: Vec<_> = bucket.iter().collect();
        rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (k, (n, p)) in rows.into_iter().take(ROWS) {
            out.push_str(&format!(
                "  {title}: {n:>5} x {k} (one at {:.0} {:.0} {:.0})\n",
                p.x, p.y, p.z
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
        assert_eq!(
            parse("600:0,2000,0"),
            Some((600, DVec3::new(0.0, 2000.0, 0.0)))
        );
        assert_eq!(
            parse(" 5 : 1.5, -2, 3 "),
            Some((5, DVec3::new(1.5, -2.0, 3.0)))
        );
        assert_eq!(parse("600"), None);
        assert_eq!(parse("x:1,2,3"), None);
    }
}

//! Townsfolk on foot: people step out of lived-in houses and stroll the streets for a while,
//! keeping to one side of the road, then go back indoors. Each is a row in an instanced draw
//! (three outfits, three stride poses), so hundreds cost a handful of draws.

use engine::prelude::*;
use engine::render::{InstanceRow, MeshInstances};
use std::collections::HashMap;

use crate::art::{Mat, Parts, hexc, v3};
use crate::draw::Art;
use crate::grid::{self, Grid};
use crate::island;
use crate::looks::ModelKey;
use crate::sim::{Building, Economy, House};

/// Walker models: outfit × stride pose.
pub const WALKER_MODELS: [[ModelKey; 3]; 3] = [
    ["walker_0_0", "walker_0_1", "walker_0_2"],
    ["walker_1_0", "walker_1_1", "walker_1_2"],
    ["walker_2_0", "walker_2_1", "walker_2_2"],
];

/// Most people out at once, and roughly one out per this many residents.
const MAX_WALKERS: usize = 360;
const PEOPLE_PER_WALKER: f32 = 5.0;
const SPEED: f32 = 1.35;
/// How far from the middle of the road people keep, meters.
const SIDE: f32 = 1.25;

struct Walker {
    /// Road cells walked so far and the one being walked to.
    from: IVec2,
    to: IVec2,
    /// 0..1 along from → to.
    t: f32,
    /// Cells left before going home.
    left: u32,
    outfit: usize,
    phase: f32,
    side: f32,
    rng: u64,
    tint: [f32; 3],
}

#[derive(Resource, Default)]
pub struct Walkers {
    people: Vec<Walker>,
    anchors: HashMap<(ModelKey, Mat), Entity>,
    spawn_clock: f32,
    seed: u64,
}

fn next(rng: &mut u64) -> u64 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    *rng
}

fn unit(rng: &mut u64) -> f32 {
    (next(rng) >> 40) as f32 / (1u64 << 24) as f32
}

/// The next road cell from `at`, not turning back unless the road ends.
fn step(roads: &std::collections::HashSet<IVec2>, from: IVec2, at: IVec2, rng: &mut u64) -> IVec2 {
    let mut ways: Vec<IVec2> = [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y]
        .into_iter()
        .map(|d| at + d)
        .filter(|c| roads.contains(c) && *c != from)
        .collect();
    if ways.is_empty() {
        return from;
    }
    // Mostly straight on.
    let ahead = at + (at - from);
    if ways.contains(&ahead) && unit(rng) < 0.65 {
        return ahead;
    }
    ways.sort_by_key(|c| (c.x, c.y));
    ways[(next(rng) % ways.len() as u64) as usize]
}

/// Let people out, walk them, draw them.
pub fn walk(world: &mut World) {
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    let dt = world.resource::<Time>().delta_secs() * world.resource::<Economy>().speed.min(4.0);
    let roads = world.resource::<Grid>().roads.clone();
    // Doorsteps: the road cell in front of each lived-in house, and how many live there.
    let mut want = 0.0f32;
    let mut doors: Vec<(IVec2, f32)> = Vec::new();
    {
        let mut q = world.query::<(&Building, &House)>();
        for (b, h) in q.iter(world) {
            if h.residents < 0.5 {
                continue;
            }
            want += h.residents / PEOPLE_PER_WALKER;
            let front = grid::front_dir(b.rot);
            if let Some(c) = grid::footprint(b.kind, b.origin, b.rot)
                .into_iter()
                .map(|c| c + front)
                .find(|c| roads.contains(c))
            {
                doors.push((c, h.residents));
            }
        }
    }
    let target = (want as usize).min(MAX_WALKERS);
    let mut w = std::mem::take(&mut *world.resource_mut::<Walkers>());
    if w.seed == 0 {
        w.seed = 0x9E37_79B9_7F4A_7C15;
    }
    // Let a few out each second until there are enough.
    w.spawn_clock += dt;
    while w.spawn_clock > 0.05 && w.people.len() < target && !doors.is_empty() {
        w.spawn_clock -= 0.05;
        let mut rng = next(&mut w.seed);
        let (door, _) = doors[(next(&mut rng) % doors.len() as u64) as usize];
        let first = step(&roads, door, door, &mut rng);
        let outfit = (next(&mut rng) % 3) as usize;
        let k = 0.85 + 0.3 * unit(&mut rng);
        w.people.push(Walker {
            from: door,
            to: first,
            t: unit(&mut rng),
            left: 12 + (next(&mut rng) % 40) as u32,
            outfit,
            phase: unit(&mut rng) * 3.0,
            side: if unit(&mut rng) < 0.5 { -1.0 } else { 1.0 },
            rng,
            tint: [k, k * (0.95 + 0.1 * unit(&mut rng)), k],
        });
    }
    if w.spawn_clock > 0.05 {
        w.spawn_clock = 0.05;
    }
    // Walk.
    let mut rows: HashMap<ModelKey, Vec<InstanceRow>> = HashMap::new();
    w.people.retain_mut(|p| {
        if !roads.contains(&p.to) {
            return false;
        }
        p.t += dt * SPEED / crate::defs::CELL_M as f32;
        p.phase += dt * SPEED / 0.7;
        while p.t >= 1.0 {
            p.t -= 1.0;
            if p.left == 0 {
                return false;
            }
            p.left -= 1;
            let n = step(&roads, p.from, p.to, &mut p.rng);
            p.from = p.to;
            p.to = n;
        }
        let a = grid::cell_center(p.from);
        let b = grid::cell_center(p.to);
        let d = (b - a).normalize_or_zero();
        let along = a + (b - a) * p.t as f64;
        let side = DVec2::new(-d.y, d.x) * (SIDE * p.side) as f64;
        let at = along + side;
        let y = island::height(at.x, at.y) + 0.08;
        let pose = (p.phase as usize) % 4;
        let model = WALKER_MODELS[p.outfit][[0, 1, 2, 1][pose]];
        rows.entry(model).or_default().push(InstanceRow {
            position: Vec3::new(at.x as f32, y as f32, at.y as f32),
            yaw: (-d.y).atan2(d.x) as f32,
            scale: 1.0,
            tint: p.tint,
        });
        true
    });
    // Rewrite the instanced rows (every model, so ones nobody uses empty out).
    for outfit in WALKER_MODELS {
        for key in outfit {
            let list = rows.remove(key).unwrap_or_default();
            let Some(model) = art.models.get(key) else {
                continue;
            };
            for (mesh, mat) in model.iter() {
                let anchor = w.anchors.get(&(key, *mat)).copied();
                match anchor {
                    Some(e) => {
                        if let Some(mut inst) = world.get_mut::<MeshInstances>(e) {
                            *inst = MeshInstances::new(list.clone());
                        }
                    }
                    None => {
                        let mut r = MeshRenderer::new(*mesh, art.mats.get(*mat));
                        r.cast_shadows = true;
                        let e = world
                            .spawn((
                                Name::new(format!("walkers {key}")),
                                Transform::default(),
                                r,
                                MeshInstances::new(list.clone()),
                            ))
                            .id();
                        w.anchors.insert((key, *mat), e);
                    }
                }
            }
        }
    }
    *world.resource_mut::<Walkers>() = w;
}

/// How many people are out walking.
pub fn count(world: &World) -> usize {
    world
        .get_resource::<Walkers>()
        .map_or(0, |w| w.people.len())
}

/// A person about 1.75 m tall facing +X, in `outfit` (0 fisherman, 1 labourer, 2 woman in a
/// long skirt) at stride `pose` (0 left foot forward, 1 feet together, 2 right foot forward).
pub fn walker(outfit: usize, pose: usize) -> Parts {
    let mut p = Parts::default();
    let skin = hexc(0xd8a888);
    let (coat, legs, hat) = match outfit {
        0 => (hexc(0x34465a), hexc(0x3a3430), hexc(0x2a2a2a)),
        1 => (hexc(0x8a5a3a), hexc(0x4a4038), hexc(0x5a4a30)),
        _ => (hexc(0x7a3a3a), hexc(0x4a2a2a), hexc(0xe8e0d0)),
    };
    let stride = [0.24f32, 0.0, -0.24][pose.min(2)];
    // Legs from the hips to the feet, one forward and one back.
    for (z, s) in [(-0.09f32, stride), (0.09, -stride)] {
        p.beam(Mat::Paint, v3(0.0, 0.92, z), v3(s, 0.06, z), 0.13, legs);
        p.box_at(
            Mat::Paint,
            v3(s + 0.05, 0.05, z),
            v3(0.13, 0.05, 0.06),
            0.0,
            hexc(0x221a14),
        );
    }
    if outfit == 2 {
        // Long skirt over the legs.
        p.cuboid(
            Mat::Paint,
            v3(-0.2, 0.12, -0.22),
            v3(0.2, 0.95, 0.22),
            legs,
            true,
        );
    }
    // Body and arms swinging against the legs.
    p.cuboid(
        Mat::Paint,
        v3(-0.12, 0.9, -0.19),
        v3(0.13, 1.46, 0.19),
        coat,
        true,
    );
    for (z, s) in [(-0.25f32, -stride), (0.25, stride)] {
        p.beam(
            Mat::Paint,
            v3(0.0, 1.42, z),
            v3(s * 0.8, 0.88, z * 1.05),
            0.1,
            coat,
        );
        p.box_at(
            Mat::Paint,
            v3(s * 0.85, 0.84, z * 1.05),
            v3(0.05, 0.05, 0.05),
            0.0,
            skin,
        );
    }
    // Head and hat (a cap, a felt hat, a bonnet).
    p.box_at(
        Mat::Paint,
        v3(0.01, 1.6, 0.0),
        v3(0.1, 0.12, 0.09),
        0.0,
        skin,
    );
    match outfit {
        0 => p.cuboid(
            Mat::Paint,
            v3(-0.11, 1.7, -0.11),
            v3(0.16, 1.77, 0.11),
            hat,
            true,
        ),
        1 => {
            p.cuboid(
                Mat::Paint,
                v3(-0.16, 1.7, -0.16),
                v3(0.16, 1.72, 0.16),
                hat,
                true,
            );
            p.cuboid(
                Mat::Paint,
                v3(-0.1, 1.72, -0.1),
                v3(0.1, 1.84, 0.1),
                hat,
                true,
            );
        }
        _ => p.cuboid(
            Mat::Paint,
            v3(-0.12, 1.62, -0.12),
            v3(0.08, 1.76, 0.12),
            hat,
            true,
        ),
    }
    p.finish()
}

/// Build a walker model by its key (`walker_<outfit>_<pose>`).
pub fn build(key: &str) -> Option<Parts> {
    let rest = key.strip_prefix("walker_")?;
    let mut it = rest.split('_');
    let outfit = it.next()?.parse().ok()?;
    let pose = it.next()?.parse().ok()?;
    Some(walker(outfit, pose))
}

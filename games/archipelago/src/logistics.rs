//! Carts: every finished batch rides the roads from its workshop to the harbour's warehouse
//! (and every input rides out), so the town's chains can be seen working.

use std::sync::Arc;

use engine::prelude::*;

use crate::defs::Good;
use crate::draw::{Art, Body, spawn_model};
use crate::island;
use crate::sim::{Building, Economy, RoadLink, Shipments};

/// Carts on the roads at most; past this, goods arrive at once.
pub const MAX_CARTS: usize = 160;
/// Cart speed, m/s of game time.
pub const CART_SPEED: f64 = 6.0;

#[derive(Component, Clone, Debug)]
pub struct Cart {
    pub path: Arc<Vec<DVec3>>,
    /// Cumulative lengths along the path.
    pub lengths: Arc<Vec<f64>>,
    pub s: f64,
    /// Driving from the warehouse to the workshop.
    pub reverse: bool,
    pub good: Good,
    pub island: usize,
    /// Add the good to the warehouse on arrival.
    pub deliver: bool,
}

fn lengths(path: &[DVec3]) -> Vec<f64> {
    let mut out = Vec::with_capacity(path.len());
    let mut acc = 0.0;
    out.push(0.0);
    for w in path.windows(2) {
        acc += (w[1] - w[0]).length();
        out.push(acc);
    }
    out
}

/// Point and direction `s` meters along a polyline.
pub fn along(path: &[DVec3], lengths: &[f64], s: f64) -> (DVec3, DVec3) {
    let total = *lengths.last().unwrap_or(&0.0);
    let s = s.clamp(0.0, total);
    let i = lengths
        .partition_point(|&l| l <= s)
        .clamp(1, path.len().max(2) - 1);
    let (a, b) = (path[i - 1], path[i]);
    let seg = (lengths[i] - lengths[i - 1]).max(1e-6);
    let t = (s - lengths[i - 1]) / seg;
    (a + (b - a) * t, (b - a).normalize_or_zero())
}

/// Turn this step's shipments into carts.
pub fn dispatch_carts(world: &mut World) {
    let jobs = std::mem::take(&mut world.resource_mut::<Shipments>().carts);
    if jobs.is_empty() {
        return;
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        // Headless economy runs: deliver at once.
        for (e, g, out) in jobs {
            if out && let Some(b) = world.get::<Building>(e).map(|b| b.island) {
                world.resource_mut::<Economy>().islands[b].add(g, 1.0);
            }
        }
        return;
    };
    let mut carts = world.query::<&Cart>().iter(world).count();
    for (e, g, out) in jobs {
        let isle = world.get::<Building>(e).map(|b| b.island);
        let path = world.get::<RoadLink>(e).and_then(|l| l.path.clone());
        let (Some(isle), Some(path)) = (isle, path) else {
            continue;
        };
        if carts >= MAX_CARTS || path.len() < 2 {
            if out {
                world.resource_mut::<Economy>().islands[isle].add(g, 1.0);
            }
            continue;
        }
        carts += 1;
        let lengths = Arc::new(lengths(&path));
        let cart = Cart {
            path: path.clone(),
            lengths: lengths.clone(),
            s: 0.0,
            reverse: !out,
            good: g,
            island: isle,
            deliver: out,
        };
        let parts = spawn_model(
            world,
            &art,
            &art.cart.clone(),
            Transform::from_translation(path[0]),
            true,
        );
        world.spawn((
            Name::new(format!("Cart: {}", g.name())),
            Transform::from_translation(path[0]),
            cart,
            Body { parts },
        ));
    }
}

/// Drive carts along their roads; deliver at the end.
pub fn drive_carts(world: &mut World) {
    let dt =
        world.resource::<Time>().delta_secs() as f64 * world.resource::<Economy>().speed as f64;
    let mut done = Vec::new();
    let mut q = world.query::<(Entity, &mut Cart, &mut Transform)>();
    for (e, mut c, mut t) in q.iter_mut(world) {
        c.s += CART_SPEED * dt;
        let total = *c.lengths.last().unwrap_or(&0.0);
        if c.s >= total {
            done.push((e, c.island, c.good, c.deliver));
            continue;
        }
        let s = if c.reverse { total - c.s } else { c.s };
        let (p, mut d) = along(&c.path, &c.lengths, s);
        if c.reverse {
            d = -d;
        }
        let ground = island::height(p.x, p.z).max(p.y - 0.5);
        t.translation = DVec3::new(p.x, ground + 0.02, p.z);
        let yaw = (-d.z).atan2(d.x) as f32;
        if d.length_squared() > 0.0 {
            t.rotation = Quat::from_rotation_y(yaw);
        }
    }
    for (e, isle, g, deliver) in done {
        if deliver {
            world.resource_mut::<Economy>().islands[isle].add(g, 1.0);
        }
        if let Some(body) = world.get::<Body>(e).cloned() {
            for p in body.parts {
                world.despawn(p);
            }
        }
        world.despawn(e);
    }
}

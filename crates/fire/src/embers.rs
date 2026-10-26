//! Embers (firebrands): glowing bits lofted by a fire's updraft, carried by the wind, and
//! sometimes starting new fires where they land ("spotting"), which is how real fires jump
//! roads and rivers.

use std::collections::HashMap;

use engine_core::math::{DVec3, IVec2, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::par_chunks_mut;

/// Embers flown per task.
const CHUNK: usize = 256;
/// Embers look the ground up in a shared grid of this many meters a cell: hundreds of them
/// fly over the same few square meters of burning grass, and a planet's ground is dear to
/// ask, so each cell is asked once.
const GROUND_CELL_M: f64 = 1.0;
/// Seconds before the grid is asked afresh (the ground can be dug or blasted).
const GROUND_KEEP_SECS: f32 = 10.0;

/// One ember.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ember {
    pub position: DVec3,
    pub velocity: Vec3,
    /// 0..1, falls as it cools; it can only light fuel while hot.
    pub heat: f32,
    /// Seconds it has flown.
    pub age: f32,
    /// Seconds of updraft lift left.
    pub lift: f32,
    /// Size, meters.
    pub size: f32,
    /// Seconds until it burns out.
    pub life: f32,
    /// Can start a fire where it lands (sparks from a campfire ring can't).
    pub ignites: bool,
}

impl Default for Ember {
    fn default() -> Self {
        Self {
            position: DVec3::ZERO,
            velocity: Vec3::ZERO,
            heat: 1.0,
            age: 0.0,
            lift: 0.0,
            size: 0.01,
            life: 5.0,
            ignites: true,
        }
    }
}

impl Ember {
    /// Fly for `dt`: rise with the plume, follow the air, fall slowly.
    fn fly(&mut self, dt: f32, up: Vec3, g: f32, wind: &(impl Fn(DVec3) -> Vec3 + Sync)) {
        self.age += dt;
        self.heat -= dt / self.life.max(0.1);
        let mut air = wind(self.position);
        if self.lift > 0.0 {
            // Riding the plume: rises with the hot gas.
            air += up * (2.0 + 5.0 * (self.lift / 2.0).min(1.0));
            self.lift -= dt;
        }
        // Light flakes follow the air closely and fall at a few m/s terminal velocity.
        let k = 1.0 - (-3.0 * dt / self.size.max(0.005).sqrt() * 0.1).exp();
        self.velocity += (air - self.velocity) * k.clamp(0.0, 1.0);
        self.velocity -= up * g * 0.25 * dt;
        self.position += (self.velocity * dt).as_dvec3();
    }
}

/// The ground cell under a point.
fn ground_cell(p: DVec3) -> IVec2 {
    IVec2::new(
        (p.x / GROUND_CELL_M).floor() as i32,
        (p.z / GROUND_CELL_M).floor() as i32,
    )
}

/// All embers in flight.
#[derive(Resource, Debug, Clone, Default)]
pub struct Embers {
    pub list: Vec<Ember>,
    /// Where embers landed hot enough to try lighting fuel (drained by the fire step).
    pub landings: Vec<(DVec3, f32)>,
    /// Ground height at the center of each cell embers have flown over (−∞ for none).
    ground: HashMap<IVec2, f64>,
    ground_age: f32,
}

impl Embers {
    /// Forget the ground heights looked up so far (the world moved or the ground changed).
    pub fn forget_ground(&mut self) {
        self.ground.clear();
        self.ground_age = 0.0;
    }

    /// Advance every ember by `dt`. `wind(p)` gives the air velocity, `ground(p)` the ground
    /// height under a point (or `None` for no ground).
    pub fn step(
        &mut self,
        dt: f32,
        gravity: Vec3,
        wind: impl Fn(DVec3) -> Vec3 + Sync,
        ground: impl Fn(DVec3) -> Option<f64> + Sync,
    ) {
        let up = (-gravity).normalize_or(Vec3::Y);
        let g = gravity.length();
        let wind = &wind;
        par_chunks_mut(&mut self.list, CHUNK, |_, chunk| {
            for e in chunk {
                e.fly(dt, up, g, wind);
            }
        });
        self.ground_age += dt;
        if self.ground_age > GROUND_KEEP_SECS {
            self.forget_ground();
        }
        // Cells no ember has been over yet, looked up together.
        let mut cells: Vec<IVec2> = self
            .list
            .iter()
            .map(|e| ground_cell(e.position))
            .filter(|c| !self.ground.contains_key(c))
            .collect();
        cells.sort_unstable_by_key(|c| (c.x, c.y));
        cells.dedup();
        let mut missing: Vec<(IVec2, f64)> =
            cells.into_iter().map(|c| (c, f64::NEG_INFINITY)).collect();
        let ground = &ground;
        par_chunks_mut(&mut missing, 16, |_, chunk| {
            for (c, h) in chunk {
                let center = (c.as_dvec2() + 0.5) * GROUND_CELL_M;
                *h = ground(DVec3::new(center.x, 0.0, center.y)).unwrap_or(f64::NEG_INFINITY);
            }
        });
        self.ground.extend(missing);
        let mut i = 0;
        while i < self.list.len() {
            let e = &self.list[i];
            let landed = self
                .ground
                .get(&ground_cell(e.position))
                .is_some_and(|h| e.position.y <= *h);
            if landed && e.heat > 0.0 && e.ignites {
                self.landings.push((e.position, e.heat));
            }
            if landed || e.heat <= 0.0 {
                self.list.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a step of a full load of embers takes in a real wind field (run by hand:
    /// `cargo test --release -p engine_fire --lib -- --ignored --nocapture ember_cost`).
    #[test]
    #[ignore]
    fn ember_cost() {
        engine_core::tasks::init_task_pools();
        let wind = engine_world::WindField::default();
        let mut embers = Embers::default();
        for i in 0..3000 {
            embers.list.push(Ember {
                position: DVec3::new((i % 60) as f64 * 0.5, 1.0 + (i % 7) as f64, (i / 60) as f64),
                lift: 1.0,
                life: 300.0,
                ..Default::default()
            });
        }
        let started = std::time::Instant::now();
        for _ in 0..60 {
            embers.step(
                1.0 / 60.0,
                Vec3::new(0.0, -9.81, 0.0),
                |p| wind.sample(p),
                |_| Some(-5.0),
            );
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0 / 60.0;
        println!("{} embers: {ms:.3} ms per step", embers.list.len());
    }

    #[test]
    fn embers_ask_the_ground_now_and_then_not_every_step() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        engine_core::tasks::init_task_pools();
        let mut embers = Embers::default();
        // Low embers over a grass fire, in a breeze.
        for i in 0..500 {
            embers.list.push(Ember {
                position: DVec3::new(i as f64 * 0.1, 1.0 + (i % 7) as f64 * 0.2, 0.0),
                lift: 0.5,
                life: 30.0,
                ..Default::default()
            });
        }
        let asked = AtomicUsize::new(0);
        let steps = 60;
        for _ in 0..steps {
            embers.step(
                1.0 / 60.0,
                Vec3::new(0.0, -9.81, 0.0),
                |_| Vec3::new(4.0, 0.0, 0.0),
                |p| {
                    asked.fetch_add(1, Ordering::Relaxed);
                    Some(0.02 * p.x)
                },
            );
        }
        let per_step = asked.into_inner() as f32 / (500 * steps) as f32;
        assert!(
            per_step < 0.2,
            "asked the ground {per_step} times per ember per step"
        );
    }

    #[test]
    fn embers_rise_drift_downwind_and_land() {
        let mut embers = Embers::default();
        embers.list.push(Ember {
            position: DVec3::new(0.0, 1.0, 0.0),
            velocity: Vec3::ZERO,
            heat: 1.0,
            age: 0.0,
            lift: 2.0,
            size: 0.02,
            life: 30.0,
            ignites: true,
        });
        let mut max_y: f64 = 0.0;
        for _ in 0..2000 {
            embers.step(
                0.02,
                Vec3::new(0.0, -9.81, 0.0),
                |_| Vec3::new(6.0, 0.0, 0.0),
                |_| Some(0.0),
            );
            if let Some(e) = embers.list.first() {
                max_y = max_y.max(e.position.y);
            }
        }
        assert!(embers.list.is_empty());
        assert_eq!(embers.landings.len(), 1);
        let (p, heat) = embers.landings[0];
        assert!(max_y > 5.0, "embers should loft, max {max_y}");
        assert!(p.x > 15.0, "embers should drift downwind, landed at {p:?}");
        assert!(heat > 0.0);
    }
}

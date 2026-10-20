//! Queries the AI makes many times a tick allocate nothing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use engine_core::prelude::*;
use engine_npc::world_model::{Obstacle, ObstacleShape, TacticalWorld};
use engine_world::Ground;

/// Counts this thread's allocations.
struct Counting;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.with(Cell::get)
}

/// `blocked_at` (which a fleeing animal calls for every way it might turn) allocates nothing,
/// near walls, a pillar and a big slab or out in the open, and still finds every one of them.
#[test]
fn blocked_at_allocates_nothing() {
    let e = |i: u32| Entity::from_raw_u32(i).unwrap();
    let wall = |i: u32, at: DVec3, half: Vec3| {
        Obstacle::new(e(i), at, Quat::IDENTITY, ObstacleShape::Box { half })
    };
    let world = TacticalWorld::from_obstacles(
        vec![
            wall(1, DVec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.3)),
            wall(2, DVec3::new(6.0, 1.0, 4.0), Vec3::new(0.3, 1.0, 2.0)),
            Obstacle::new(
                e(3),
                DVec3::new(-5.0, 1.5, 5.0),
                Quat::IDENTITY,
                ObstacleShape::Cylinder {
                    radius: 0.5,
                    half_height: 1.5,
                },
            ),
            // Bigger than a hash cell's worth: kept on its own list.
            wall(4, DVec3::new(0.0, 0.5, 400.0), Vec3::new(300.0, 0.5, 2.0)),
        ],
        &Ground::default(),
    );
    let probes = [
        (DVec3::new(0.0, 0.0, 0.5), true),
        (DVec3::new(0.0, 0.0, 2.0), false),
        (DVec3::new(6.4, 0.0, 4.0), true),
        (DVec3::new(-5.0, 0.0, 5.6), true),
        (DVec3::new(-5.0, 0.0, 7.0), false),
        (DVec3::new(120.0, 0.0, 401.0), true),
        (DVec3::new(120.0, 0.0, 395.0), false),
        (DVec3::new(50.0, 0.0, -50.0), false),
    ];
    let before = allocs();
    let mut hits = [false; 8];
    for _ in 0..200 {
        for (k, &(p, _)) in probes.iter().enumerate() {
            hits[k] = world.blocked_at(p, 0.4, 1.2, None);
        }
    }
    let made = allocs() - before;
    for (k, &(p, blocked)) in probes.iter().enumerate() {
        assert_eq!(hits[k], blocked, "at {p}");
    }
    // Leaving an obstacle out leaves it out.
    assert!(!world.blocked_at(probes[0].0, 0.4, 1.2, Some(0)));
    assert_eq!(
        made,
        0,
        "{made} allocations in {} queries",
        200 * probes.len()
    );
}

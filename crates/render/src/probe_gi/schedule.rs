//! Which probes trace each frame.
//!
//! Probes that scroll into the grid as the camera moves wait in a queue until they are traced,
//! and their storage slots are cleared the frame they come in, so `probe_gi` skips them (and
//! fades toward the sky guess by their share) instead of sampling the probe that scrolled out
//! of the far side. RTXGI does the same: scrolling "invalidates only the planes of probes on
//! the edges of the volume" and "all interior probes remain stationary", a scrolled probe is
//! written black (`ProbeBlendingCS.hlsl`), and it relies on tracing all of them that frame. A
//! budgeted software tracer can't, so the queue spreads them over the next frames.
//!
//! Before this, more scrolled-in probes than the frame's budget started every probe over (a new
//! epoch), and the round robin relit the box in storage order: rows of light marching across
//! it with every step (playtests 2 and 3). Now every probe starts over only on a jump (more
//! than half the grid new, a new grid), and then they relight nearest the camera first.

use std::collections::VecDeque;

use engine_core::prelude::*;

use super::Grid;

/// This frame's work.
#[derive(Default, Debug)]
pub(super) struct Plan {
    /// Storage slots to trace, each once.
    pub traced: Vec<u32>,
    /// Slots that scrolled in this frame and aren't traced yet: written empty.
    pub cleared: Vec<u32>,
    /// Probes that scrolled in this frame.
    pub scrolled_in: u32,
}

pub(super) struct Schedule {
    /// Bumped when every probe's history is void (a jump, a new grid): `probe_gi` ignores probes
    /// last written in another epoch.
    pub epoch: u32,
    /// Every-probe restarts so far.
    pub resets: u32,
    /// Slots waiting for their first trace since they scrolled in, oldest first.
    queue: VecDeque<u32>,
    queued: Vec<bool>,
    /// The frame each slot was last planned in (to plan each at most once a frame).
    planned: Vec<u64>,
    frame: u64,
    /// The next slot the round robin updates.
    cursor: u32,
    /// Every probe starts over at the next plan (`relight`).
    relight: bool,
    /// Grid coordinates (as storage indices of an unscrolled grid) nearest the center first,
    /// for the dims they were sorted for.
    nearest: (UVec3, Vec<u32>),
}

impl Schedule {
    pub fn new() -> Self {
        Self {
            epoch: 1,
            resets: 0,
            queue: VecDeque::new(),
            queued: Vec::new(),
            planned: Vec::new(),
            frame: 0,
            cursor: 0,
            relight: false,
            nearest: (UVec3::ZERO, Vec::new()),
        }
    }

    /// Every probe starts over at the next plan, nearest the camera first, as after a jump: the
    /// light they hold is void (the main light jumped).
    pub fn relight(&mut self) {
        self.relight = true;
    }

    /// Slots waiting for their first trace.
    pub fn pending(&self) -> u32 {
        self.queue.len() as u32
    }

    /// Plan `per_frame` traces for `grid`, which was `old` last frame. `queue` false is the old
    /// way, to compare (`ENGINE_GI_SCROLL_QUEUE=off`): more probes scrolled in than the budget
    /// start every probe over, relit in storage order.
    pub fn plan(&mut self, old: Option<Grid>, grid: Grid, per_frame: u32, queue: bool) -> Plan {
        self.frame += 1;
        let count = grid.count();
        let per_frame = per_frame.clamp(1, count) as usize;
        if self.queued.len() != count as usize {
            self.queued = vec![false; count as usize];
            self.planned = vec![0; count as usize];
            self.queue.clear();
            self.cursor = 0;
        }
        let mut plan = Plan::default();
        let mut scrolled = Vec::new();
        let relight = std::mem::take(&mut self.relight);
        match old {
            // A new epoch voids every probe, those that scrolled in with the rest.
            Some(_) if relight => self.reset(grid),
            Some(old) if old.dims == grid.dims && old.spacing == grid.spacing => {
                if old.origin != grid.origin {
                    let dims = grid.dims;
                    for z in 0..dims.z {
                        for y in 0..dims.y {
                            for x in 0..dims.x {
                                let g = UVec3::new(x, y, z);
                                if !old.contains(grid.origin + g.as_ivec3()) {
                                    scrolled.push(grid.slot(g));
                                }
                            }
                        }
                    }
                    plan.scrolled_in = scrolled.len() as u32;
                    if !queue && scrolled.len() > per_frame {
                        scrolled.clear();
                        self.restart();
                        self.queue.clear();
                        self.queued.fill(false);
                    } else if scrolled.len() as u32 > count / 2 {
                        scrolled.clear();
                        self.reset(grid);
                    } else {
                        for &s in &scrolled {
                            if !self.queued[s as usize] {
                                self.queued[s as usize] = true;
                                self.queue.push_back(s);
                            }
                        }
                    }
                }
            }
            _ => {
                plan.scrolled_in = count;
                self.reset(grid);
            }
        }

        // The queue first: all the budget while much of the grid waits, else three quarters,
        // so the probes already lit keep following the scene.
        let share = if !queue || self.queue.len() > count as usize / 4 {
            per_frame
        } else {
            (per_frame * 3 / 4).max(1)
        };
        while plan.traced.len() < share {
            let Some(s) = self.queue.pop_front() else {
                break;
            };
            self.queued[s as usize] = false;
            self.planned[s as usize] = self.frame;
            plan.traced.push(s);
        }
        // Then the rest in turn, skipping the queue's.
        let mut steps = 0;
        while plan.traced.len() < per_frame && steps < count {
            let s = self.cursor;
            self.cursor = (self.cursor + 1) % count;
            steps += 1;
            if self.queued[s as usize] || self.planned[s as usize] == self.frame {
                continue;
            }
            self.planned[s as usize] = self.frame;
            plan.traced.push(s);
        }
        for s in scrolled {
            if self.planned[s as usize] != self.frame {
                self.planned[s as usize] = self.frame;
                plan.cleared.push(s);
            }
        }
        plan
    }

    /// Every probe starts over: a new epoch, and all of them queued nearest the center first.
    fn reset(&mut self, grid: Grid) {
        self.restart();
        if self.nearest.0 != grid.dims {
            self.nearest = (grid.dims, nearest_first(grid.dims));
        }
        let unscrolled = Grid {
            origin: IVec3::ZERO,
            ..grid
        };
        self.queue.clear();
        for &i in &self.nearest.1 {
            let d = grid.dims;
            let g = UVec3::new(i % d.x, (i / d.x) % d.y, i / (d.x * d.y));
            debug_assert_eq!(unscrolled.slot(g), i);
            let s = grid.slot(g);
            self.queued[s as usize] = true;
            self.queue.push_back(s);
        }
    }
}

impl Schedule {
    /// A new epoch: every probe's history is void.
    fn restart(&mut self) {
        self.epoch += 1;
        if self.epoch >= 2048 {
            self.epoch = 1;
        }
        self.resets += 1;
    }
}

/// A grid's coordinates (as `x + dims.x * (y + dims.y * z)`) by distance from its center.
fn nearest_first(dims: UVec3) -> Vec<u32> {
    let center = dims.as_vec3() * 0.5 - 0.5;
    let mut order: Vec<(u32, u32)> = (0..dims.x * dims.y * dims.z)
        .map(|i| {
            let g = UVec3::new(i % dims.x, (i / dims.x) % dims.y, i / (dims.x * dims.y));
            let d = (g.as_vec3() - center).length_squared();
            ((d * 16.0) as u32, i)
        })
        .collect();
    order.sort_unstable();
    order.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(origin: IVec3) -> Grid {
        Grid {
            dims: UVec3::new(8, 4, 8),
            spacing: 1.5,
            origin,
        }
    }

    /// Each slot at most once a frame, traced or cleared.
    fn assert_distinct(plan: &Plan) {
        let mut all: Vec<u32> = plan.traced.iter().chain(&plan.cleared).copied().collect();
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n, "a slot planned twice in a frame");
    }

    #[test]
    fn a_one_cell_move_never_starts_every_probe_over() {
        let mut s = Schedule::new();
        let a = grid(IVec3::ZERO);
        // The first frame is a reset: every probe queued, nearest the center first.
        let first = s.plan(None, a, 16, true);
        assert_eq!((s.epoch, s.resets), (2, 1));
        assert_eq!(first.traced.len(), 16);
        let center = a.dims.as_vec3() * 0.5 - 0.5;
        let far = |slot: u32| {
            // `a` is unscrolled: its slots are its coordinates.
            let d = a.dims;
            let g = UVec3::new(slot % d.x, (slot / d.x) % d.y, slot / (d.x * d.y));
            (g.as_vec3() - center).length()
        };
        assert!(first.traced.iter().all(|&t| far(t) < 1.8), "nearest first");
        // Drain it.
        while s.pending() > 0 {
            assert_distinct(&s.plan(Some(a), a, 16, true));
        }

        // A step along x brings in one plane (4 x 8 = 32 probes), over a budget of 16.
        let b = grid(IVec3::X);
        let step = s.plan(Some(a), b, 16, true);
        assert_eq!((s.epoch, s.resets), (2, 1), "no reset for a step");
        assert_eq!(step.scrolled_in, 32);
        // Three quarters of the budget go to the queue, the rest round robin.
        assert_eq!(step.traced.len(), 16);
        assert_eq!(step.cleared.len(), 32 - 12);
        assert_eq!(s.pending(), 20);
        assert_distinct(&step);
        // The scrolled-in slots are the plane at the new far side, x = 7 in b.
        let plane: Vec<u32> = (0..8)
            .flat_map(|z| (0..4).map(move |y| b.slot(UVec3::new(7, y, z))))
            .collect();
        for slot in step.traced.iter().take(12).chain(&step.cleared) {
            assert!(plane.contains(slot));
        }
        // Standing still, the queue drains and every one of them is traced once.
        let mut traced: Vec<u32> = step.traced[..12].to_vec();
        while s.pending() > 0 {
            let p = s.plan(Some(b), b, 16, true);
            assert!(p.cleared.is_empty());
            assert_distinct(&p);
            traced.extend(p.traced.iter().filter(|t| plane.contains(t)));
        }
        traced.sort_unstable();
        traced.dedup();
        assert_eq!(traced.len(), 32);
        assert_eq!(s.resets, 1);
    }

    #[test]
    fn a_jump_starts_every_probe_over_nearest_first() {
        let mut s = Schedule::new();
        let a = grid(IVec3::ZERO);
        s.plan(None, a, 64, true);
        // Half the box along x: more than half the grid is new.
        let b = grid(IVec3::new(5, 0, 0));
        let p = s.plan(Some(a), b, 64, true);
        assert_eq!((s.epoch, s.resets), (3, 2));
        assert!(p.cleared.is_empty(), "the new epoch voids them all");
        assert_eq!(p.traced.len(), 64);
        assert_eq!(s.pending(), b.count() - 64);
        // Nearest b's center first: storage slots of b's middle.
        let near = b.slot(UVec3::new(4, 2, 4));
        assert!(p.traced.contains(&near));
        let corner = b.slot(UVec3::ZERO);
        assert!(!p.traced.contains(&corner));
    }

    #[test]
    fn a_relight_starts_every_probe_over_nearest_first() {
        let mut s = Schedule::new();
        let a = grid(IVec3::ZERO);
        s.plan(None, a, 256, true);
        while s.pending() > 0 {
            s.plan(Some(a), a, 256, true);
        }
        let epoch = s.epoch;
        s.relight();
        let p = s.plan(Some(a), a, 64, true);
        assert_eq!(s.epoch, epoch + 1, "the old light is void");
        assert_eq!(p.traced.len(), 64);
        assert_eq!(s.pending(), a.count() - 64);
        assert!(p.traced.contains(&a.slot(UVec3::new(4, 2, 4))));
        // Once only.
        s.plan(Some(a), a, 64, true);
        assert_eq!(s.epoch, epoch + 1);
    }

    #[test]
    fn steps_faster_than_the_queue_drains_keep_each_slot_queued_once() {
        let mut s = Schedule::new();
        let mut old = grid(IVec3::ZERO);
        s.plan(None, old, 256, true);
        while s.pending() > 0 {
            s.plan(Some(old), old, 256, true);
        }
        for i in 1..=3 {
            let g = grid(IVec3::new(i, 0, 0));
            let p = s.plan(Some(old), g, 8, true);
            assert_distinct(&p);
            old = g;
        }
        assert_eq!(s.resets, 1);
        // Three planes in; 6 a frame traced from the queue, then all 8 once it holds more than a
        // quarter of the grid.
        assert_eq!(s.pending(), 3 * 32 - 6 - 6 - 8);
        let mut queue: Vec<u32> = s.queue.iter().copied().collect();
        queue.sort_unstable();
        queue.dedup();
        assert_eq!(queue.len() as u32, s.pending());
    }

    #[test]
    fn without_the_queue_a_step_over_the_budget_starts_every_probe_over() {
        let mut s = Schedule::new();
        let a = grid(IVec3::ZERO);
        s.plan(None, a, 256, false);
        let p = s.plan(Some(a), grid(IVec3::X), 16, false);
        assert_eq!((s.epoch, s.resets), (3, 2));
        assert_eq!(s.pending(), 0);
        assert!(p.cleared.is_empty());
        // Within the budget the plane is traced that frame, as before.
        let p = s.plan(Some(grid(IVec3::X)), grid(IVec3::new(2, 0, 0)), 64, false);
        assert_eq!((s.resets, p.traced.len(), s.pending()), (2, 64, 0));
    }
}

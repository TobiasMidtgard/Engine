//! Choosing where to stand. Candidate spots (cover points, a ring around the enemy at the
//! preferred range, high ground, the current spot) are scored on what a good fighter weighs:
//! hidden from the enemy but able to shoot, at the right distance, on its flank, higher up, not
//! seen by the enemy's friends, not bunched with allies, away from fire and explosives, and not
//! too far to run. The score weights depend on what the NPC is trying to do, and smarter NPCs
//! (`logic`) weigh more spots and more of these factors.

use engine_core::prelude::*;
use engine_world::HeatField;

use crate::agent::NpcAction;
use crate::nav::{NavCtx, PathStatus};
use crate::world_model::{CoverHeight, Sight};

/// How much each factor counts (0 = ignored).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PositionWeights {
    /// Being at the preferred distance.
    pub range: f32,
    /// Hidden from the main enemy (crouched).
    pub cover: f32,
    /// Able to shoot the enemy from there (standing or leaning out).
    pub can_shoot: f32,
    /// Penalty per other enemy that can see the spot.
    pub exposure: f32,
    /// Being to the side of or behind the enemy.
    pub flank: f32,
    /// Coming from a different direction than allies (pincer; negative: the same direction).
    pub pincer: f32,
    /// Height above the enemy.
    pub height: f32,
    /// Penalty for distance to run.
    pub travel: f32,
    /// Penalty for crowding allies.
    pub spacing: f32,
    /// Being near allies (regrouping).
    pub regroup: f32,
    /// Penalty for heat and explosives.
    pub hazard: f32,
    /// Bonus for keeping the current destination (avoids dithering).
    pub stay: f32,
    /// Being off the enemy's side: square to the way its cover faces
    /// ([`PositionQuery::front`]), within about 20° of it (a squad's flankers).
    #[serde(default)]
    pub side: f32,
}

/// How far from square to the enemy's front a side spot may be (the cosine of the angle off
/// its front: 0.4 is 66° to 114°); farther round or less, it counts against the spot.
pub const SIDE_BAND: f32 = 0.4;

impl PositionWeights {
    /// Weights for an action, scaled by the NPC's tactical skill.
    pub fn for_action(action: NpcAction, logic: f32) -> Self {
        let l = logic.clamp(0.0, 1.0);
        match action {
            NpcAction::TakeCover => Self {
                range: 0.3,
                cover: 2.2,
                can_shoot: 0.6 * l,
                exposure: 1.2 * l,
                travel: 1.4,
                spacing: 0.6,
                hazard: 1.5,
                stay: 0.5,
                ..Self::default()
            },
            NpcAction::Flank => Self {
                range: 0.5,
                cover: 0.7,
                can_shoot: 1.0,
                exposure: 0.8,
                flank: 2.4,
                pincer: 0.8,
                height: 0.2,
                travel: 0.35,
                spacing: 0.6,
                hazard: 1.5,
                stay: 0.4,
                ..Self::default()
            },
            NpcAction::SeekHighGround => Self {
                range: 0.4,
                cover: 0.4,
                can_shoot: 1.2,
                exposure: 0.4,
                height: 3.0,
                travel: 0.35,
                spacing: 0.5,
                hazard: 1.5,
                stay: 0.6,
                ..Self::default()
            },
            NpcAction::Retreat => Self {
                range: 1.0,
                cover: 1.6,
                exposure: 1.0,
                travel: 0.4,
                regroup: 1.0,
                hazard: 1.5,
                stay: 0.5,
                ..Self::default()
            },
            _ => Self {
                range: 1.0,
                cover: 0.9 * l,
                can_shoot: 1.6,
                exposure: 0.7 * l,
                flank: 0.35 * l,
                pincer: 0.3 * l,
                height: 0.6 * l,
                travel: 0.7,
                spacing: 0.6,
                hazard: 1.2,
                stay: 0.5,
                ..Self::default()
            },
        }
    }
}

/// Smoke letting less light than this through hides a spot from a shooter (and a target
/// from us).
pub const SMOKE_HIDES: f32 = 0.25;

/// Most cells a pick's path search may look at for one spot (about 1,500 m² of half-meter
/// cells): a spot farther round than that is a long detour, which the travel weight already
/// scores down, and an unreachable one would search the whole window.
const PROBE_CELLS: usize = 6000;

/// What the NPC knows when picking a spot.
#[derive(Debug, Clone)]
pub struct PositionQuery {
    pub me: Entity,
    /// Feet position now.
    pub from: DVec3,
    pub target: Entity,
    /// Where it thinks the enemy is (feet).
    pub target_pos: DVec3,
    pub target_forward: Vec3,
    /// Other known enemies (feet).
    pub other_threats: Vec<DVec3>,
    /// Ally positions and the spots they've claimed.
    pub allies: Vec<DVec3>,
    /// Directions (from the target, XZ) allies attack from.
    pub ally_angles: Vec<Vec3>,
    /// The way the enemy's cover faces (from the target, XZ), for [`PositionWeights::side`].
    pub front: Option<Vec3>,
    /// Preferred distance to the enemy, meters.
    pub ideal_range: f32,
    pub current_destination: Option<DVec3>,
    pub logic: f32,
    pub weights: PositionWeights,
    /// Explosives to keep away from: (center, radius).
    pub explosives: Vec<(DVec3, f32)>,
    /// Search radius around the NPC, meters.
    pub search_radius: f32,
}

/// A scored spot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    pub position: DVec3,
    pub score: f32,
    pub reason: &'static str,
    pub covered: bool,
    pub low_cover: bool,
}

/// What `choose_position` needs from the world.
pub struct TacticsCtx<'a, 'b> {
    pub sight: Sight<'a>,
    pub nav: &'b mut NavCtx<'a>,
    pub heat: &'a HeatField,
}

/// Chunks of ground nobody has sampled yet that one [`choose_position`] look builds; the rest
/// wait for `warm_nav` (one such chunk costs ~10-15 ms on the VM).
const COLD_CHUNKS_PER_LOOK: usize = 1;

/// Pick the best reachable spot. Writes the path to it into `path` (so the caller can follow
/// it without searching again). `scored` receives the top candidates for debugging.
pub fn choose_position(
    ctx: &mut TacticsCtx,
    q: &PositionQuery,
    path: &mut Vec<DVec3>,
    scored: &mut Vec<Spot>,
) -> Option<Spot> {
    let _span = tracing::info_span!("choose position").entered();
    scored.clear();
    let w = q.weights;
    let budget = (12.0 + 44.0 * q.logic.clamp(0.0, 1.0)) as usize;
    let mut raw: Vec<(DVec3, &'static str, Option<CoverHeight>)> = Vec::with_capacity(budget * 2);
    raw.push((q.from, "hold", None));
    if let Some(d) = q.current_destination {
        raw.push((d, "keep", None));
    }
    // Cover points between here and the enemy.
    if w.cover > 0.0 || w.can_shoot > 0.0 {
        let mut idx = Vec::new();
        ctx.sight
            .world
            .cover_near(q.from, q.search_radius as f64, &mut idx);
        idx.sort_by(|&a, &b| {
            let pa = ctx.sight.world.cover[a as usize].position;
            let pb = ctx.sight.world.cover[b as usize].position;
            (pa.distance_squared(q.from) + pa.distance_squared(q.target_pos) * 0.5)
                .total_cmp(&(pb.distance_squared(q.from) + pb.distance_squared(q.target_pos) * 0.5))
        });
        for &i in idx.iter().take(budget) {
            let c = ctx.sight.world.cover[i as usize];
            // Only cover that faces away from the enemy hides us from it.
            let to_enemy = Vec3::new(
                (q.target_pos.x - c.position.x) as f32,
                0.0,
                (q.target_pos.z - c.position.z) as f32,
            )
            .normalize_or_zero();
            if c.normal.dot(to_enemy) < -0.2 {
                raw.push((c.position, "cover", Some(c.height)));
            }
        }
    }
    // A ring around the enemy at the preferred range (and a bit nearer and farther).
    let dirs = (8.0 + 10.0 * q.logic) as usize;
    let base_angle = (q.from.z - q.target_pos.z).atan2(q.from.x - q.target_pos.x) as f32;
    for k in 0..dirs {
        let a = base_angle + k as f32 / dirs as f32 * TAU;
        for f in [0.8f32, 1.15] {
            let r = (q.ideal_range * f) as f64;
            let p = q.target_pos + DVec3::new(a.cos() as f64 * r, 0.0, a.sin() as f64 * r);
            raw.push((p, "range", None));
        }
    }
    // High ground near the enemy.
    let _high_span = tracing::info_span!("choose position: high ground").entered();
    if w.height > 0.0 {
        let step = 2.0f64;
        let r = (q.ideal_range as f64 + 6.0).min(28.0);
        let n = (r / step) as i32;
        let mut found = 0;
        // Ground not worked out yet is worked out a piece per look, not all at once (a spot on
        // it is weighed on a later look; `warm_nav` works it out ahead in any case).
        let mut built = false;
        for i in -n..=n {
            for j in -n..=n {
                let p = q.target_pos + DVec3::new(i as f64 * step, 0.0, j as f64 * step);
                let probe = DVec3::new(p.x, p.y + 50.0, p.z);
                let (h, ok) = match ctx.nav.peek(probe) {
                    Some(c) => c,
                    None if !built => {
                        built = true;
                        ctx.nav.sample(probe)
                    }
                    None => continue,
                };
                if ok && h > q.target_pos.y + 0.9 && h < q.target_pos.y + 8.0 {
                    raw.push((DVec3::new(p.x, h, p.z), "high_ground", None));
                    found += 1;
                }
            }
        }
        let _ = found;
    }
    drop(_high_span);

    // The ground under every spot, worked out at once (snapping each below would build its
    // chunk alone, one after another), with the snap's reach around it. Where we stand and
    // where we're going always; of ground nobody has sampled yet only a chunk or two a look,
    // the rest is sampled off the main thread and its spots weighed on a later look.
    let ground_span = tracing::info_span!("choose position: ground").entered();
    ctx.nav.ensure_around(
        raw.iter()
            .filter(|r| matches!(r.1, "hold" | "keep"))
            .map(|r| r.0),
        1.5,
    );
    ctx.nav
        .ensure_around_within(raw.iter().map(|r| r.0), 1.5, COLD_CHUNKS_PER_LOOK);
    drop(ground_span);
    // Snap every spot onto the walkable surface (the nav grid, so one after another), then
    // weigh them side by side on the leaf pool: each spot's sight lines are a physics ray and
    // a terrain walk, two to five of them (a look took 14 ms at the fortress on the VM).
    let snap_span = tracing::info_span!("choose position: snap").entered();
    let mut snapped: Vec<(DVec3, &'static str, Option<CoverHeight>)> =
        Vec::with_capacity(raw.len());
    let target_eye = q.target_pos + DVec3::Y * 1.6;
    let target_chest = q.target_pos + DVec3::Y * 1.2;
    let ignore = [q.me, q.target];
    for (p, reason, cover_height) in raw {
        if !ctx.nav.built_around(p, 1.5) {
            continue;
        }
        // Snap onto the walkable surface.
        let probe = DVec3::new(p.x, p.y + 1.0, p.z);
        let (h, ok) = ctx.nav.sample(probe);
        let p = if ok && (h - p.y).abs() < 2.5 {
            DVec3::new(p.x, h, p.z)
        } else if let Some(n) = ctx.nav.nearest_walkable(p, 1.2) {
            n
        } else {
            continue;
        };
        snapped.push((p, reason, cover_height));
    }
    drop(snap_span);
    let _score_span = tracing::info_span!("choose position: score").entered();
    let (sight, heat) = (&ctx.sight, ctx.heat);
    // Everything but the sight lines: cheap, worked out for every spot.
    let cheap = |p: DVec3| -> f32 {
        let mut s = 0.0f32;
        let flat = |a: DVec3, b: DVec3| DVec2::new(a.x - b.x, a.z - b.z).length() as f32;
        let d = flat(p, q.target_pos);
        if w.range > 0.0 {
            let e = ((d - q.ideal_range) / q.ideal_range.max(1.0)).clamp(-2.0, 2.0);
            s -= w.range * e * e;
        }
        let dir_from_target = Vec3::new(
            (p.x - q.target_pos.x) as f32,
            0.0,
            (p.z - q.target_pos.z) as f32,
        )
        .normalize_or_zero();
        if w.flank > 0.0 {
            let behind = (1.0 - q.target_forward.dot(dir_from_target)) * 0.5;
            s += w.flank * behind;
        }
        if w.side > 0.0
            && let Some(front) = q.front
        {
            let c = front.dot(dir_from_target) / SIDE_BAND;
            s += w.side * (1.0 - c * c).max(-2.0);
        }
        if w.pincer != 0.0 && !q.ally_angles.is_empty() {
            let sep = q
                .ally_angles
                .iter()
                .map(|a| (1.0 - a.dot(dir_from_target)) * 0.5)
                .fold(1.0f32, f32::min);
            s += w.pincer * sep;
        }
        if w.height > 0.0 {
            s += w.height * (((p.y - q.target_pos.y) as f32).clamp(0.0, 4.0) / 4.0);
        }
        s -= w.travel * flat(p, q.from) / 15.0;
        let mut crowd = 0.0;
        let mut nearest_ally = f32::INFINITY;
        for &a in &q.allies {
            let da = flat(p, a);
            nearest_ally = nearest_ally.min(da);
            if da < 3.5 {
                crowd += 1.0 - da / 3.5;
            }
        }
        s -= w.spacing * crowd;
        if w.regroup > 0.0 && nearest_ally.is_finite() {
            s += w.regroup * (1.0 - (nearest_ally / 15.0).min(1.0));
        }
        let heat = heat.sample(p + DVec3::Y * 0.5);
        if heat > 25.0 {
            s -= w.hazard * 3.0 * (heat / 100.0).min(2.0);
        }
        for &(c, r) in &q.explosives {
            if flat(p, c) < r {
                s -= w.hazard;
            }
        }
        if let Some(cur) = q.current_destination
            && flat(p, cur) < 1.5
        {
            s += w.stay;
        }
        s
    };
    // The sight lines (cover from the target, a shot at it, exposure to other threats): a
    // physics ray and a terrain walk each, about 450 a look at the fortress (VM), most of its
    // 12 ms. What they add to a spot's score, and whether it is covered.
    //
    // `floor` is what the sight lines must add for the spot to reach the sixth best weighed
    // so far (−∞ until six are weighed): once the lines left can't get it there, the rest
    // are skipped and the spot's best possible score is returned instead. It stays under the
    // sixth best, so the six best and their scores are exactly those of weighing every line.
    let sighted = |p: DVec3, reason: &'static str, floor: f32| -> (f32, bool) {
        let mut s = 0.0f32;
        let crouch = p + DVec3::Y * 0.95;
        let stand = p + DVec3::Y * 1.5;
        // Solid cover stops bullets; thick smoke only hides you (worth a bit less).
        let solid = !sight.clear(target_eye, crouch, &ignore);
        let concealed = !solid && sight.haze(target_eye, crouch) < SMOKE_HIDES;
        if solid {
            s += w.cover;
        } else if concealed {
            s += w.cover * 0.7;
        }
        let covered = solid || concealed;
        // The shot lines add at most `can_shoot`, the exposure lines only take away.
        if s + w.can_shoot.max(0.0) < floor {
            return (s + w.can_shoot.max(0.0), covered);
        }
        if w.can_shoot > 0.0 {
            let mut shoot = sight.visibility(stand, target_chest, &ignore) > SMOKE_HIDES;
            if !shoot && reason == "cover" {
                // Lean out around a tall edge.
                let side = Vec3::new(
                    (q.target_pos.z - p.z) as f32,
                    0.0,
                    -(q.target_pos.x - p.x) as f32,
                )
                .normalize_or_zero();
                for sgn in [-1.0f32, 1.0] {
                    if sight.visibility(
                        stand + (side * 0.8 * sgn).as_dvec3(),
                        target_chest,
                        &ignore,
                    ) > SMOKE_HIDES
                    {
                        shoot = true;
                        break;
                    }
                }
            }
            if shoot {
                s += w.can_shoot;
            }
        }
        if w.exposure > 0.0 {
            for &t in &q.other_threats {
                if s < floor {
                    break;
                }
                if sight.visibility(t + DVec3::Y * 1.6, crouch, &ignore) > SMOKE_HIDES {
                    s -= w.exposure;
                }
            }
        }
        (s, covered)
    };
    // Sight lines add at most this much: spots whose cheap score plus it can't reach the
    // sixth best weighed so far are never weighed (best first, branch and bound). The six
    // best (`scored`) and their order come out as if every spot were weighed.
    let most = w.cover.max(0.0) + w.can_shoot.max(0.0);
    let mut order: Vec<(f32, usize)> = snapped
        .iter()
        .enumerate()
        .map(|(i, &(p, ..))| (cheap(p), i))
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    const KEEP: usize = 6;
    // Spots weighed side by side on the leaf pool, a batch at a time. Smaller batches let
    // the bound stop sooner: at the arena (VM, 4 cores) the pool ran them about one at a
    // time anyway (a look weighed ~45 spots of ~28 µs in 1.6 ms).
    const BATCH: usize = 8;
    let mut weighed: Vec<(Spot, usize)> = Vec::with_capacity(order.len());
    let mut kept: Vec<f32> = Vec::with_capacity(order.len());
    let mut next = 0;
    while next < order.len() {
        if kept.len() >= KEEP {
            kept.sort_by(|a, b| b.total_cmp(a));
            if order[next].0 + most < kept[KEEP - 1] {
                break;
            }
        }
        let sixth = if kept.len() >= KEEP {
            kept[KEEP - 1]
        } else {
            f32::NEG_INFINITY
        };
        let batch = &order[next..(next + BATCH).min(order.len())];
        let mut out: Vec<Option<Spot>> = vec![None; batch.len()];
        engine_core::tasks::par_chunks_mut(&mut out, 1, |i, one| {
            let Some(slot) = one.first_mut() else {
                return;
            };
            let (base, idx) = batch[i];
            let (p, reason, cover_height) = snapped[idx];
            let floor = if weigh_every_line() {
                f32::NEG_INFINITY
            } else {
                sixth - base
            };
            let (seen, covered) = sighted(p, reason, floor);
            *slot = Some(Spot {
                position: p,
                score: base + seen,
                reason,
                covered,
                low_cover: cover_height == Some(CoverHeight::Low),
            });
        });
        for (spot, &(_, idx)) in out.into_iter().zip(batch) {
            if let Some(spot) = spot {
                kept.push(spot.score);
                weighed.push((spot, idx));
            }
        }
        next += batch.len();
    }
    weighed.sort_by(|a, b| b.0.score.total_cmp(&a.0.score).then(a.1.cmp(&b.1)));
    let spots: Vec<Spot> = weighed.into_iter().map(|(s, _)| s).collect();
    drop(_score_span);
    scored.extend(spots.iter().take(6).copied());
    // Take the best one we can actually walk to.
    let _paths_span = tracing::info_span!("choose position: paths").entered();
    for spot in spots.iter().take(4) {
        if spot.position.distance(q.from) < 0.6 {
            path.clear();
            return Some(*spot);
        }
        let status =
            ctx.nav
                .find_path_within(q.from, spot.position, &mut |_| 0.0, path, PROBE_CELLS);
        let end_ok = path.last().is_some_and(|e| e.distance(spot.position) < 1.5);
        if status == PathStatus::Complete || end_ok {
            return Some(*spot);
        }
    }
    path.clear();
    None
}

#[cfg(test)]
thread_local! {
    /// Tests turn the sight-line bound off to check it changes nothing.
    static WEIGH_EVERY_LINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether [`choose_position`] weighs every sight line of every spot it weighs (tests only).
fn weigh_every_line() -> bool {
    #[cfg(test)]
    {
        WEIGH_EVERY_LINE.with(|w| w.get())
    }
    #[cfg(not(test))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::NavGrid;
    use crate::world_model::{Obstacle, ObstacleShape, TacticalWorld};
    use engine_world::{Ground, Water};

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).unwrap()
    }

    fn query(
        action: NpcAction,
        logic: f32,
        from: DVec3,
        target: DVec3,
        fwd: Vec3,
    ) -> PositionQuery {
        PositionQuery {
            me: e(100),
            from,
            target: e(101),
            target_pos: target,
            target_forward: fwd,
            other_threats: vec![],
            allies: vec![],
            ally_angles: vec![],
            front: None,
            ideal_range: 12.0,
            current_destination: None,
            logic,
            weights: PositionWeights::for_action(action, logic),
            explosives: vec![],
            search_radius: 25.0,
        }
    }

    #[test]
    fn take_cover_picks_a_spot_hidden_from_the_enemy() {
        // A wall between the NPC and the enemy, off to the side.
        let world = TacticalWorld::from_obstacles(
            vec![Obstacle::new(
                e(1),
                DVec3::new(4.0, 0.6, 0.0),
                Quat::IDENTITY,
                ObstacleShape::Box {
                    half: Vec3::new(2.0, 0.6, 0.3),
                },
            )],
            &Ground::default(),
        );
        let (ground, water) = (Ground::default(), Water::default());
        let mut grid = NavGrid::default();
        let mut nav = grid.ctx(&world, &ground, &water);
        let heat = HeatField::default();
        let mut ctx = TacticsCtx {
            sight: Sight {
                world: &world,
                physics: None,
                ground: &ground,
                obscurance: None,
            },
            nav: &mut nav,
            heat: &heat,
        };
        let from = DVec3::new(0.0, 0.0, -4.0);
        let target = DVec3::new(4.0, 0.0, 12.0);
        let q = query(NpcAction::TakeCover, 1.0, from, target, Vec3::NEG_Z);
        let mut path = Vec::new();
        let mut scored = Vec::new();
        let spot = choose_position(&mut ctx, &q, &mut path, &mut scored).unwrap();
        assert!(spot.covered, "{spot:?} {scored:?}");
        assert!(
            spot.position.z < 0.0,
            "behind the wall from the enemy: {spot:?}"
        );
        assert!(spot.low_cover, "{spot:?}");
    }

    #[test]
    fn smart_flankers_go_around_dumb_ones_dont() {
        let world = TacticalWorld::from_obstacles(Vec::new(), &Ground::default());
        let (ground, water) = (Ground::default(), Water::default());
        let heat = HeatField::default();
        // Enemy at the origin facing -Z (toward the NPC).
        let from = DVec3::new(0.0, 0.0, -14.0);
        let target = DVec3::ZERO;
        let behind = |logic: f32| {
            let mut grid = NavGrid::default();
            let mut nav = grid.ctx(&world, &ground, &water);
            let mut ctx = TacticsCtx {
                sight: Sight {
                    world: &world,
                    physics: None,
                    ground: &ground,
                    obscurance: None,
                },
                nav: &mut nav,
                heat: &heat,
            };
            let action = if logic > 0.5 {
                NpcAction::Flank
            } else {
                NpcAction::Engage
            };
            let q = query(action, logic, from, target, Vec3::NEG_Z);
            let mut path = Vec::new();
            let mut scored = Vec::new();
            let spot = choose_position(&mut ctx, &q, &mut path, &mut scored).unwrap();
            let dir = Vec3::new(spot.position.x as f32, 0.0, spot.position.z as f32).normalize();
            (1.0 - Vec3::NEG_Z.dot(dir)) * 0.5
        };
        let smart = behind(1.0);
        let dumb = behind(0.05);
        assert!(smart > 0.6, "smart flank value {smart}");
        assert!(dumb < 0.3, "dumb flank value {dumb}");
    }

    #[test]
    fn skipping_sight_lines_keeps_the_six_best() {
        // A yard of walls and crates, the enemy and two more threats around it: the bound
        // that skips lines must leave the six best spots and their scores exactly as they
        // are with every line weighed.
        let mut obstacles = Vec::new();
        for i in 0..16u32 {
            let a = i as f64 * 0.9;
            let r = 4.0 + (i % 5) as f64 * 2.5;
            let tall = i % 3 == 0;
            obstacles.push(Obstacle::new(
                e(10 + i),
                DVec3::new(a.cos() * r, if tall { 1.1 } else { 0.5 }, a.sin() * r),
                Quat::from_rotation_y(a as f32),
                ObstacleShape::Box {
                    half: Vec3::new(1.5, if tall { 1.1 } else { 0.5 }, 0.3),
                },
            ));
        }
        let world = TacticalWorld::from_obstacles(obstacles, &Ground::default());
        let (ground, water) = (Ground::default(), Water::default());
        let heat = HeatField::default();
        for (action, logic) in [
            (NpcAction::TakeCover, 1.0),
            (NpcAction::Engage, 0.8),
            (NpcAction::Flank, 1.0),
            (NpcAction::Engage, 0.1),
        ] {
            let pick = |every: bool| {
                WEIGH_EVERY_LINE.with(|w| w.set(every));
                let mut grid = NavGrid::default();
                let mut nav = grid.ctx(&world, &ground, &water);
                let mut ctx = TacticsCtx {
                    sight: Sight {
                        world: &world,
                        physics: None,
                        ground: &ground,
                        obscurance: None,
                    },
                    nav: &mut nav,
                    heat: &heat,
                };
                let mut q = query(
                    action,
                    logic,
                    DVec3::new(-2.0, 0.0, -10.0),
                    DVec3::new(3.0, 0.0, 14.0),
                    Vec3::NEG_Z,
                );
                q.other_threats = vec![DVec3::new(-16.0, 0.0, 6.0), DVec3::new(15.0, 0.0, 2.0)];
                let (mut path, mut scored) = (Vec::new(), Vec::new());
                let spot = choose_position(&mut ctx, &q, &mut path, &mut scored);
                WEIGH_EVERY_LINE.with(|w| w.set(false));
                let six: Vec<_> = scored
                    .iter()
                    .map(|s| (s.position, s.score, s.covered))
                    .collect();
                (spot.map(|s| s.position), six)
            };
            let (bounded, every) = (pick(false), pick(true));
            assert_eq!(bounded.1.len(), 6, "{action:?}");
            assert_eq!(bounded, every, "{action:?} logic {logic}");
        }
    }
}

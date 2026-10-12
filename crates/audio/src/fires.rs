//! Fires near the listener. Burning ground (cells of [`HeatField`] hot enough for flames) is
//! gathered into patches, each played as one [`FireSound`] sized by its burning area, and
//! burning [`Combustible`]s (trees, buildings, props) play on their own. Only the loudest few
//! play; the rest fade out.

use std::collections::HashMap;

use engine_core::math::{DVec3, IVec2, Vec3};
use engine_core::prelude::*;
use engine_world::{BurnState, Combustible, Env};

use crate::ambience::FireSound;
use crate::{Audio, AudioCue, AudioSettings, PlayOptions, Sound, VoiceId};

/// Fires farther away than this aren't played, meters.
pub const FIRE_RANGE_M: f64 = 90.0;
/// Most fires playing at once.
pub const FIRE_VOICES: usize = 5;
/// Ground hotter than this (°C above ambient) is in flames.
pub const FLAME_C: f32 = 300.0;
/// Burning ground is gathered into square patches this wide, meters.
const PATCH_M: f64 = 10.0;
/// How often the fires are gathered again, seconds.
const SCAN_INTERVAL: f64 = 0.25;
/// A burning area within this of a playing ground fire is that fire (moved or grown), meters.
const MATCH_M: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FireKey {
    /// Burning ground: matched to the playing fire nearest to it.
    Ground,
    /// A burning thing: matched by entity.
    Object(Entity),
}

struct Fire {
    key: FireKey,
    position: DVec3,
    /// Area in flames, m².
    size: f32,
    /// How loud it is where the listener stands (for picking the loudest).
    score: f32,
}

#[derive(Default)]
pub(crate) struct FireAmbience {
    voices: Vec<Playing>,
    next_scan: f64,
}

struct Playing {
    key: FireKey,
    id: VoiceId,
    position: DVec3,
}

impl FireAmbience {
    /// The playing fire that `f` continues, if any.
    fn find(&self, f: &Fire, claimed: &[bool]) -> Option<usize> {
        match f.key {
            FireKey::Object(_) => self.voices.iter().position(|v| v.key == f.key),
            FireKey::Ground => self
                .voices
                .iter()
                .enumerate()
                .filter(|(i, v)| v.key == FireKey::Ground && !claimed[*i])
                .map(|(i, v)| (i, v.position.distance(f.position)))
                .filter(|(_, d)| *d < MATCH_M)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i),
        }
    }
}

/// A fire's size as a sound source: bigger fires are bigger sources, so they stay loud
/// farther out.
fn source_radius(size: f32) -> f32 {
    1.0 + 0.5 * size.sqrt()
}

/// Area in flames of a burning object with `fuel_kg` left, m² (a 60 kg tree ≈ 9 m²).
fn object_flame_area(fuel_kg: f32) -> f32 {
    (0.6 * fuel_kg.max(0.0).powf(2.0 / 3.0)).clamp(0.3, 600.0)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn fire_ambience(
    mut audio: ResMut<Audio>,
    settings: Res<AudioSettings>,
    env: Env,
    objects: Query<(Entity, &GlobalTransform, &Combustible)>,
    time: Res<Time>,
    mut cues: MessageWriter<AudioCue>,
    mut state: Local<FireAmbience>,
) {
    let now = time.real_elapsed_secs_f64();
    if now < state.next_scan {
        return;
    }
    state.next_scan = now + SCAN_INTERVAL;
    if !audio.is_running() || !settings.fires {
        for v in state.voices.drain(..) {
            audio.stop(v.id, 1.0);
        }
        return;
    }
    let listener = audio.listener_position();
    let mut fires = gather_fires(&env, &objects, listener);

    // Loudest first; a fire already playing gets a head start so two similar fires don't
    // keep swapping places.
    let none = vec![false; state.voices.len()];
    for f in &mut fires {
        if state.find(f, &none).is_some() {
            f.score *= 1.5;
        }
    }
    fires.sort_by(|a, b| b.score.total_cmp(&a.score));
    fires.truncate(FIRE_VOICES);
    fires.retain(|f| f.score > 1e-3);

    let fs = audio.sample_rate();
    let mut claimed = vec![false; state.voices.len()];
    let mut started = Vec::new();
    for f in fires {
        if let Some(i) = state.find(&f, &claimed)
            && audio.is_playing(state.voices[i].id)
        {
            claimed[i] = true;
            let v = &mut state.voices[i];
            v.position = f.position;
            audio.set_param(v.id, "size", f.size);
            audio.set_position(v.id, f.position, Vec3::ZERO);
            audio.set_last(v.id, f.position);
            continue;
        }
        let seed = audio.next_seed();
        let mut sound = FireSound::new(fs, seed);
        sound.set_param("size", f.size);
        let id = audio.play(
            sound,
            PlayOptions::at(f.position)
                .range(source_radius(f.size), FIRE_RANGE_M as f32 + 30.0)
                .fade_in(1.5),
        );
        if id.is_none() {
            continue;
        }
        started.push(Playing {
            key: f.key,
            id,
            position: f.position,
        });
        cues.write(AudioCue {
            voice: id,
            position: Some(f.position),
            name: "fire".into(),
            loudness: FireSound::level(f.size),
        });
    }
    // Fade out the ones that went out or got quieter than the rest.
    let mut i = 0;
    state.voices.retain(|v| {
        let keep = claimed[i];
        i += 1;
        if !keep {
            audio.stop(v.id, 2.0);
        }
        keep
    });
    state.voices.extend(started);
}

/// Every fire within [`FIRE_RANGE_M`] of `listener`.
fn gather_fires(
    env: &Env,
    objects: &Query<(Entity, &GlobalTransform, &Combustible)>,
    listener: DVec3,
) -> Vec<Fire> {
    let mut fires = Vec::new();
    let score = |p: DVec3, size: f32| {
        let r = source_radius(size);
        FireSound::level(size) * r / (p.distance(listener) as f32).max(r)
    };

    // Burning ground: each connected burning area is one fire; big ones are split into
    // patches so a long fire front sounds from along its length.
    let grid = &env.heat.grid;
    let mut hot: HashMap<IVec2, f32> = HashMap::new();
    if grid.width > 0 && grid.height > 0 {
        let reach = DVec3::splat(FIRE_RANGE_M);
        let lo = grid.cell_at(listener - reach).max(IVec2::ZERO);
        let hi = grid
            .cell_at(listener + reach)
            .min(IVec2::new(grid.width as i32 - 1, grid.height as i32 - 1));
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let c = IVec2::new(x, y);
                if let Some(t) = grid.get(c)
                    && t >= FLAME_C
                {
                    hot.insert(c, t);
                }
            }
        }
    }
    let cell_area = (grid.cell * grid.cell) as f64;
    let mut stack = Vec::new();
    let mut component = Vec::new();
    let mut seen: std::collections::HashSet<IVec2> = std::collections::HashSet::new();
    let mut cells: Vec<IVec2> = hot.keys().copied().collect();
    cells.sort_by_key(|c| (c.y, c.x));
    for start in cells {
        if !seen.insert(start) {
            continue;
        }
        component.clear();
        stack.push(start);
        while let Some(c) = stack.pop() {
            component.push(c);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let n = c + IVec2::new(dx, dy);
                    if hot.contains_key(&n) && seen.insert(n) {
                        stack.push(n);
                    }
                }
            }
        }
        let (min, max) = component.iter().fold(
            (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN)),
            |(a, b), c| (a.min(*c), b.max(*c)),
        );
        let extent = (max - min + IVec2::ONE).max_element() as f64 * grid.cell as f64;
        // Area-weighted centre and total area per patch (one patch for a small fire).
        let mut patches: HashMap<IVec2, (f64, DVec3, f64)> = HashMap::new();
        for c in &component {
            let cc = grid.cell_center(*c);
            let key = if extent <= 2.0 * PATCH_M {
                IVec2::ZERO
            } else {
                IVec2::new(
                    (cc.x / PATCH_M).floor() as i32,
                    (cc.y / PATCH_M).floor() as i32,
                )
            };
            let w = (hot[c] / FLAME_C) as f64;
            let e = patches.entry(key).or_insert((0.0, DVec3::ZERO, 0.0));
            e.0 += cell_area;
            e.1 += DVec3::new(cc.x, 0.0, cc.y) * w;
            e.2 += w;
        }
        for (area, sum, weight) in patches.into_values() {
            let mut p = sum / weight;
            p.y = env
                .ground(DVec3::new(p.x, listener.y, p.z))
                .map(|g| g.height)
                .unwrap_or(listener.y - 1.6)
                + 0.6;
            if p.distance(listener) > FIRE_RANGE_M {
                continue;
            }
            let size = area as f32;
            fires.push(Fire {
                key: FireKey::Ground,
                position: p,
                size,
                score: score(p, size),
            });
        }
    }

    // Burning things. Smouldering ones only tick and hiss a little.
    for (e, t, c) in objects {
        let size = match c.state {
            BurnState::Burning => object_flame_area(c.fuel_kg),
            BurnState::Smoldering => 0.1,
            _ => continue,
        };
        let up = env.up(t.translation()).as_dvec3();
        let p = t.translation() + up * (c.fuel_kg.max(0.0).cbrt() * 0.35).clamp(0.3, 6.0) as f64;
        if p.distance(listener) > FIRE_RANGE_M {
            continue;
        }
        fires.push(Fire {
            key: FireKey::Object(e),
            position: p,
            size,
            score: score(p, size),
        });
    }
    fires
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigger_fires_are_bigger_sources() {
        assert!(source_radius(1.0) < source_radius(100.0));
        assert!(object_flame_area(5.0) < object_flame_area(60.0));
        let tree = object_flame_area(60.0);
        assert!((5.0..15.0).contains(&tree), "{tree}");
        assert!(object_flame_area(1e6) <= 600.0);
    }
}

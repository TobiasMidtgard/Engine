//! Cross-fades between a plant's levels of detail, so trees and plants don't pop as the camera
//! moves (playtest #33). The part that changes takes its new mesh at once and dithers in while
//! a short-lived ghost beside it, carrying the old mesh, dithers out ([`overlapped`]). The new
//! one comes in fully before the old one starts to go: two alpha-tested crowns (leaf cards and
//! leaf-by-leaf, say) cover different pixels, so a complementary swap ([`DitherFade::pair`])
//! thins the crown to half its leaves mid-way and the sky shows through, where overlapping
//! briefly draws both, which reads as a crown no thinner than either. TAA smooths the dither, as Unity's LOD Group "Cross Fade" and Unreal's dithered LOD transitions do. The
//! fade runs on time (`FADE_S`), so it finishes even when the camera stops, and the switch
//! distances have a band of hysteresis ([`HYSTERESIS`]) so a camera resting on a boundary
//! doesn't flicker. Research: engine-design/foliage-research.md, section 4.
//!
//! Swaps happen at once (no fade) on a plant's first look, after a camera jump (a teleport or
//! a tour cut), past [`MAX_FADES_PER_FRAME`] in a frame, and on parts already dithering (a
//! streaming cell fading in owns their `DitherFade`).

use crate::visuals::{Part, apply};
use engine_core::prelude::*;
use engine_render::{Camera, DitherFade, MeshRenderer};

/// How long a cross-fade takes (seconds).
pub const FADE_S: f32 = 0.5;
/// Most fades started in one frame; past it parts switch at once (a camera turning onto a
/// whole new forest shouldn't dither it all).
pub const MAX_FADES_PER_FRAME: usize = 96;
/// The camera moving further than this in a frame is a cut: everything switches at once.
pub const JUMP_M: f64 = 30.0;
/// Switch distances may be passed by this share before a shown level gives way (both ways).
pub const HYSTERESIS: f32 = 0.08;

/// A part dithering in, and the ghost dithering out its old look (if it had one).
#[derive(Component, Debug, Clone, Copy)]
pub struct PartFade {
    ghost: Option<Entity>,
    t: f32,
}

impl PartFade {
    /// A part with nothing before it, dithering in from nothing.
    pub(crate) fn appearing() -> Self {
        Self {
            ghost: None,
            t: 0.0,
        }
    }
}

/// A part's old look, fading out beside it (despawned when the fade ends).
#[derive(Component, Debug, Clone, Copy)]
pub struct FadeGhost;

/// Whether levels cross-fade, and this frame's fading: how many started, and whether the
/// camera cut.
#[derive(Resource, Debug)]
pub struct LodFades {
    /// Cross-fade (true) or switch at once (for comparisons).
    pub enabled: bool,
    started: usize,
    cut: bool,
    camera: Option<DVec3>,
    /// Frames since the last cut.
    since_cut: u32,
}

impl Default for LodFades {
    fn default() -> Self {
        Self {
            enabled: true,
            started: 0,
            cut: false,
            camera: None,
            since_cut: 0,
        }
    }
}

impl LodFades {
    /// Whether a swap seen now should fade (and count it).
    fn take(&mut self) -> bool {
        if !self.enabled || self.cut || self.started >= MAX_FADES_PER_FRAME {
            return false;
        }
        self.started += 1;
        true
    }

    /// Whether what streams in or out now should fade: fades are on and the camera has moved
    /// smoothly for [`SETTLED_FRAMES`] (what lands just after a cut is part of the new view).
    pub(crate) fn settled(&self) -> bool {
        self.enabled && self.since_cut >= SETTLED_FRAMES
    }
}

/// Frames after a cut before streamed cells fade in and out rather than appear and go.
pub const SETTLED_FRAMES: u32 = 30;

/// Keep `shown` while the distance is within [`HYSTERESIS`] of where it is still right:
/// `pick(d)` is the level wanted at distance `d`.
pub fn with_hysteresis(
    shown: [Part; 3],
    distance: f32,
    pick: impl Fn(f32) -> [Part; 3],
) -> [Part; 3] {
    let want = pick(distance);
    if want == shown {
        return want;
    }
    if pick(distance * (1.0 - HYSTERESIS)) == shown || pick(distance * (1.0 + HYSTERESIS)) == shown
    {
        return shown;
    }
    want
}

/// Switch `r` (the part entity `part`, a child of `parent` at `transform`) from `old` to
/// `new`, cross-fading when it can. `busy` says the part is already fading (by us or by
/// streaming): it then switches at once and the running fade carries on.
#[allow(clippy::too_many_arguments)]
pub fn swap(
    commands: &mut Commands,
    fades: &mut LodFades,
    parent: Entity,
    part: Entity,
    transform: Transform,
    r: &mut MeshRenderer,
    old: Part,
    new: Part,
    busy: bool,
) {
    apply(r, new);
    if busy || (old.is_none() && new.is_none()) || !fades.take() {
        return;
    }
    let ghost = old.map(|old| {
        let mut g = r.clone();
        apply(&mut g, Some(old));
        commands
            .spawn((transform, ChildOf(parent), g, DitherFade(0.0), FadeGhost))
            .id()
    });
    if new.is_some() {
        commands
            .entity(part)
            .insert((DitherFade(-1.0), PartFade { ghost, t: 0.0 }));
    } else if let Some(ghost) = ghost {
        // Nothing comes in: the ghost alone goes out, timed by a fade on the hidden part.
        commands.entity(part).insert(PartFade {
            ghost: Some(ghost),
            t: 0.0,
        });
    }
}

/// The old look's and the new one's dither `t` of the way through a fade (0..=1): the new one
/// comes in over the first half, the old one goes over the second, on opposite patterns.
pub fn overlapped(t: f32) -> (DitherFade, DitherFade) {
    let t = t.clamp(0.0, 1.0);
    (
        DitherFade((2.0 * t - 1.0).max(0.0)),
        DitherFade(-(1.0 - 2.0 * t).max(0.0)),
    )
}

/// Note a camera cut at the start of the frame and reset the frame's count.
pub fn begin_fades(mut fades: ResMut<LodFades>, cams: Query<(&Camera, &GlobalTransform)>) {
    let at = crate::visuals::camera_position(&cams);
    fades.cut = match (fades.camera, at) {
        (Some(a), Some(b)) => a.distance(b) > JUMP_M,
        _ => true,
    };
    fades.camera = at;
    fades.started = 0;
    fades.since_cut = if fades.cut {
        0
    } else {
        fades.since_cut.saturating_add(1)
    };
}

/// Move every fade on, and end the finished ones.
pub fn advance_fades(
    mut commands: Commands,
    time: Res<Time>,
    mut parts: Query<(Entity, &mut PartFade, Option<&mut DitherFade>), Without<FadeGhost>>,
    mut ghosts: Query<&mut DitherFade, With<FadeGhost>>,
) {
    let step = time.delta_secs() / FADE_S;
    for (e, mut fade, dither) in &mut parts {
        fade.t = (fade.t + step).min(1.0);
        let (out, into) = overlapped(fade.t);
        if let Some(g) = fade.ghost
            && let Ok(mut d) = ghosts.get_mut(g)
        {
            *d = out;
        }
        if fade.t >= 1.0 {
            if let Some(g) = fade.ghost
                && let Ok(mut g) = commands.get_entity(g)
            {
                g.despawn();
            }
            commands.entity(e).remove::<(PartFade, DitherFade)>();
        } else if let Some(mut d) = dither {
            *d = into;
        }
    }
}

/// Whether a part entity is already dithering (fading by us, or by streaming).
pub type Busy = Has<DitherFade>;

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::Handle;

    fn level(n: u32) -> [Part; 3] {
        [
            Some((Handle::from_index(n), Handle::from_index(n))),
            None,
            None,
        ]
    }

    #[test]
    fn a_camera_resting_on_a_switch_distance_never_flickers() {
        // Near level within 50 m, far beyond.
        let pick = |d: f32| if d < 50.0 { level(1) } else { level(2) };
        let mut shown = pick(40.0);
        let mut switches = 0;
        // Jitter back and forth across the boundary by up to 2 m.
        for k in 0..200 {
            let d = 50.0 + 2.0 * ((k as f32) * 1.7).sin();
            let want = with_hysteresis(shown, d, pick);
            if want != shown {
                switches += 1;
                shown = want;
            }
        }
        assert!(switches <= 1, "{switches} switches");
        // Well past the band it does switch, both ways.
        assert_eq!(with_hysteresis(level(1), 60.0, pick), level(2));
        assert_eq!(with_hysteresis(level(2), 40.0, pick), level(1));
    }

    #[test]
    fn a_fade_never_thins_the_crown() {
        // Whatever the coverage of each, at every step one of the two shows whole.
        for k in 0..=20 {
            let (out, into) = overlapped(k as f32 / 20.0);
            assert!(
                out.hidden() == 0.0 || into.hidden() == 0.0,
                "{k}: {out:?} {into:?}"
            );
        }
        assert_eq!(overlapped(0.0).1.hidden(), 1.0);
        assert_eq!(overlapped(1.0).0.hidden(), 1.0);
    }

    #[test]
    fn a_cut_or_a_crowd_of_swaps_switches_at_once() {
        let mut f = LodFades {
            cut: true,
            ..Default::default()
        };
        assert!(!f.take());
        f.cut = false;
        for _ in 0..MAX_FADES_PER_FRAME {
            assert!(f.take());
        }
        assert!(!f.take());
    }
}

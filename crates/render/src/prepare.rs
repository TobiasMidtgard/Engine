//! The features' shared prepares, side by side (see [`RenderFeature::prepares_shared`]).
//!
//! Every frame the renderer runs, in this order:
//!
//! 1. `prepare_serial` of each feature that prepares shared, in feature order, with the world
//!    borrowed mutably (queries made, resources inserted);
//! 2. all their `prepare_shared`s at once, plus the probe GI's gather and the ground's weather
//!    map upload, with the world borrowed
//!    shared: the longest (by last frame's time) on the calling thread, the rest on the
//!    renderer's own small pool, longest first (Graham's longest-processing-time order);
//! 3. what each left in its [`PrepareOut`], applied in feature order;
//! 4. `prepare` of the features that don't prepare shared, in feature order, as before.
//!
//! The pool is the renderer's own rather than the engine's compute pool: a feature that splits
//! its prepare over the compute pool (the meshes' gather) then opens that pool's scope from a
//! thread outside it, as it does from the main thread, instead of a scope nested inside one of
//! the pool's own tasks, where the waiting thread can pick up another system's whole task
//! (nested-scope stalls). Rayon's `in_place_scope` lets the jobs borrow the frame's data.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bevy_ecs::prelude::World;

use crate::features::{GpuShared, PrepareOut, RenderFeature};
use crate::frame::{FrameUniform, ViewData};
use crate::probe_gi::ProbeGi;
use crate::settings::RenderSettings;

/// Most threads the pool gets (besides the calling thread): the features that prepare shared
/// are fewer than this.
const MAX_THREADS: usize = 8;

/// The pool the shared prepares run on (made on first use).
pub(crate) fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .saturating_sub(1)
            .clamp(1, MAX_THREADS);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("render prepare {i}"))
            .build()
            .expect("the render prepare threads start")
    })
}

/// `name` as a `&'static str` (inner step names are static): each distinct name is kept once.
pub(crate) fn static_label(name: &str) -> &'static str {
    static LABELS: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut labels = LABELS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(label) = labels.get(name) {
        return label;
    }
    let label: &'static str = Box::leak(name.to_owned().into_boxed_str());
    labels.insert(name.to_owned(), label);
    label
}

/// The probe GI's gather, run beside the features' shared prepares (main view only).
pub(crate) struct GiJob<'a> {
    pub gi: &'a mut ProbeGi,
    pub uniform: &'a mut FrameUniform,
    /// Its time last frame (ms), to order the jobs.
    pub last_ms: f32,
}

/// The ground's weather map upload (`surface_weather`), run beside the features' shared
/// prepares (main view only): encoding a changed grid takes a few milliseconds.
pub(crate) struct SurfaceJob<'a> {
    pub map: &'a mut crate::surface_weather::SurfaceWeatherMap,
    pub real_dt: f32,
    /// Set to whether the map's texture was recreated.
    pub resized: &'a mut bool,
}

/// What [`run`] did with the probe GI.
#[derive(Default, Clone, Copy)]
pub(crate) struct GiDone {
    /// `ProbeGi::extract`'s result: the atlas was recreated.
    pub atlas_moved: bool,
    pub ms: f32,
}

/// Run the shared prepares of `features` whose `shared` flag is set, and `gi`, side by side.
/// Each feature's time goes in its `outs[i].ms`, and `last_ms[i]` (used to order the next
/// frame's jobs). Nothing is applied to the world yet.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    features: &mut [Box<dyn RenderFeature>],
    shared: &[bool],
    outs: &mut [PrepareOut],
    last_ms: &mut [f32],
    gi: Option<GiJob<'_>>,
    surface: Option<SurfaceJob<'_>>,
    world: &World,
    gpu: &GpuShared,
    view: &ViewData,
    settings: &RenderSettings,
) -> GiDone {
    let mut gi_done = GiDone::default();
    {
        type Job<'a> = Box<dyn FnOnce() + Send + 'a>;
        let mut jobs: Vec<(f32, Job<'_>)> = Vec::with_capacity(features.len() + 1);
        for ((feature, out), (&on, &ms)) in features
            .iter_mut()
            .zip(outs.iter_mut())
            .zip(shared.iter().zip(last_ms.iter()))
        {
            if !on {
                continue;
            }
            jobs.push((
                ms,
                Box::new(move || {
                    let started = Instant::now();
                    let _span = tracing::info_span!(
                        target: "engine_render",
                        "render_step",
                        name = %format_args!("prepare {}", feature.name())
                    )
                    .entered();
                    feature.prepare_shared(world, gpu, view, settings, out);
                    out.ms = started.elapsed().as_secs_f32() * 1000.0;
                }),
            ));
        }
        if let Some(GiJob {
            gi,
            uniform,
            last_ms,
        }) = gi
        {
            let done = &mut gi_done;
            jobs.push((
                last_ms,
                Box::new(move || {
                    let started = Instant::now();
                    let _span =
                        tracing::info_span!(target: "engine_render", "render_step", name = "probe gi")
                            .entered();
                    done.atlas_moved = gi.extract(world, gpu, view, settings, uniform);
                    done.ms = started.elapsed().as_secs_f32() * 1000.0;
                }),
            ));
        }
        if let Some(SurfaceJob {
            map,
            real_dt,
            resized,
        }) = surface
        {
            // Usually nothing to do (the grid changed or not); last in line.
            jobs.push((
                0.0,
                Box::new(move || {
                    let _span = tracing::info_span!(
                        target: "engine_render",
                        "render_step",
                        name = "surface weather"
                    )
                    .entered();
                    *resized = map.prepare(world, gpu, real_dt);
                }),
            ));
        }
        // Longest first; the longest on this thread.
        jobs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut jobs = jobs.into_iter().map(|(_, job)| job);
        if let Some(first) = jobs.next() {
            let rest: Vec<Job<'_>> = jobs.collect();
            if rest.is_empty() {
                first();
            } else {
                pool().in_place_scope(|scope| {
                    for job in rest {
                        scope.spawn(move |_| job());
                    }
                    first();
                });
            }
        }
    }
    for ((out, ms), &on) in outs.iter().zip(last_ms.iter_mut()).zip(shared) {
        if on {
            *ms = out.ms;
        }
    }
    gi_done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_kept_once() {
        let a = static_label("prepare test label");
        let b = static_label(&String::from("prepare test label"));
        assert!(std::ptr::eq(a, b));
    }
}

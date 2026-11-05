//! Spray mist where falls land: volumetric fog over each plunge pool, sized by the fall.
//!
//! A plunging jet drags air down into its pool, and the air boils back up full of droplets: a
//! dense cloud round the foot, and a thinner veil the wind and the spray's own outflow carry
//! off downstream. How much spray a fall throws grows with its power P = ρ g Q H (Q the
//! flow, H the drop): a 3 m cascade wets the rocks beside it, a 20 m river fall stands in its
//! own cloud, Niagara's rises above the gorge (waterfall-volume-research.md). The clouds are
//! [`FogVolume`]s, so the volumetric fog lights them: sunbeams catch them, the cliff shades
//! them, and the camera can walk into them.
//!
//! The cloud stands between the camera and the curtain, so it is kept thin enough (a few
//! tenths of extinction across it) that the falls still read white through it: a denser one
//! dims them to grey wherever it lies in a shadow, as a tree's on the cliff top.
//!
//! [`falls_mist_system`] keeps a small pool of these entities ([`FallsMist`]) over the plunges
//! nearest the camera, moving and resizing them a few times a second as the falls change.

use engine_core::math::{DVec3, Quat, Vec2};
use engine_core::prelude::*;
use engine_render::FogVolume;

use crate::state::SharedWater;

/// Most plunges given a cloud at once (each makes two volumes; the fog draws up to 64).
pub const MAX_MISTS: usize = 6;
/// Plunges further from the camera than this get none, meters.
const MIST_REACH_M: f64 = 700.0;
/// Plunges closer than this to one another share one cloud, meters.
const MERGE_M: f64 = 6.0;
/// The clouds are moved this often, seconds.
const PERIOD_S: f32 = 0.25;
/// Falls of less than this power throw no cloud worth drawing, watts (a 1 m drop of 30 l/s).
const LEAST_POWER_W: f32 = 300.0;
/// Spray: the light it scatters is near white.
const SPRAY_ALBEDO: [f32; 3] = [0.92, 0.94, 0.97];

/// One of the clouds [`falls_mist_system`] keeps: the `slot`th nearest plunge's dense cloud
/// (`veil` false) or the thin veil drifting off it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallsMist {
    pub slot: usize,
    pub veil: bool,
}

/// Where a fall lands (world), its horizontal velocity there (m/s) and its power (W), as
/// [`Falls::plunges`](crate::flow::Falls::plunges) gives them.
pub type Plunge = (DVec3, Vec2, f32);

/// The clouds for the plunges nearest `camera`: plunges near one another merged (their power
/// added, placed by it), at most [`MAX_MISTS`], nearest first.
pub fn mist_sites(plunges: &[Plunge], camera: DVec3) -> Vec<Plunge> {
    let mut near: Vec<Plunge> = plunges
        .iter()
        .copied()
        .filter(|(foot, _, power)| *power >= LEAST_POWER_W && foot.distance(camera) < MIST_REACH_M)
        .collect();
    near.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut sites: Vec<Plunge> = Vec::new();
    for (foot, away, power) in near {
        match sites.iter_mut().find(|s| s.0.distance(foot) < MERGE_M) {
            Some(site) => {
                let w = power / (site.2 + power);
                site.0 = site.0.lerp(foot, w as f64);
                site.1 = site.1.lerp(away, w);
                site.2 += power;
            }
            None => sites.push((foot, away, power)),
        }
    }
    sites.sort_by(|a, b| a.0.distance(camera).total_cmp(&b.0.distance(camera)));
    sites.truncate(MAX_MISTS);
    sites
}

/// The dense cloud and the veil over a plunge: where each sits (world), its yaw, and the fog.
/// Both grow with the root of flow times drop (m⁴/s; P / ρg): about 2 m across under a small
/// cascade, a 14 m cloud and a 30 m veil under 5 m³/s falling 20 m.
pub fn mist_clouds((foot, away, power): Plunge) -> [(DVec3, Quat, FogVolume); 2] {
    let size = (power / 9810.0).max(0.0).sqrt();
    let out = away.normalize_or(Vec2::Y);
    let yaw = Quat::from_rotation_y(out.x.atan2(out.y));
    let out = DVec3::new(out.x as f64, 0.0, out.y as f64);
    let radius = (1.5 + 0.55 * size).clamp(2.0, 40.0);
    let height = (1.0 + 0.3 * size).clamp(1.5, 25.0);
    let cloud = FogVolume::ellipsoid([radius, height, radius])
        // Thin enough that the falls read white through it: about 0.6 of extinction across
        // its middle however big it grows (a flood's cloud is wider, not denser to the eye;
        // a fixed density let a flash flood's grey it out behind 1.7 and read as the weaker fall).
        .with_density((0.02 + 0.003 * size).min(0.3 / radius))
        .with_albedo(SPRAY_ALBEDO)
        .with_edge_falloff(0.5 * radius)
        .with_noise(0.8, (0.3 * radius).clamp(1.0, 8.0))
        .with_anisotropy(0.7);
    // The veil, longer downstream (local z) than across.
    let (wide, high) = (2.2 * radius, 1.6 * height);
    let veil = FogVolume::ellipsoid([wide * 0.8, high, wide * 1.2])
        .with_density((0.004 + 0.0006 * size).min(0.012).min(0.06 / wide))
        .with_albedo(SPRAY_ALBEDO)
        .with_edge_falloff(0.6 * wide)
        .with_noise(0.7, (0.3 * wide).clamp(2.0, 12.0))
        .with_anisotropy(0.7);
    let r = radius as f64;
    [
        (
            foot + out * (0.3 * r) + DVec3::Y * (0.5 * height as f64),
            yaw,
            cloud,
        ),
        (
            foot + out * (1.6 * r) + DVec3::Y * (0.7 * high as f64),
            yaw,
            veil,
        ),
    ]
}

/// Keeps a cloud of spray over each plunge near the camera (see the module docs).
pub fn falls_mist_system(
    mut commands: Commands,
    time: Res<Time>,
    shared: Res<SharedWater>,
    focus: Res<engine_world::FocusPoints>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    mut mists: Query<(Entity, &FallsMist, &mut FogVolume, &mut Transform)>,
    mut since: Local<f32>,
) {
    *since += time.delta_secs();
    if *since < PERIOD_S {
        return;
    }
    *since = 0.0;
    let Some(camera) = focus.points.first().copied().or_else(|| {
        cameras
            .iter()
            .filter(|(c, _)| c.active)
            .max_by_key(|(c, _)| c.order)
            .map(|(_, g)| g.translation())
    }) else {
        return;
    };
    let plunges: Vec<Plunge> = {
        let state = shared.0.read();
        state
            .flows
            .iter()
            .flat_map(|sim| sim.falls().plunges.iter().copied())
            .collect()
    };
    let sites = mist_sites(&plunges, camera);
    let mut wanted: Vec<(FallsMist, DVec3, Quat, FogVolume)> = Vec::new();
    for (slot, site) in sites.into_iter().enumerate() {
        for (k, (at, yaw, fog)) in mist_clouds(site).into_iter().enumerate() {
            let veil = k == 1;
            wanted.push((FallsMist { slot, veil }, at, yaw, fog));
        }
    }
    for (entity, mist, mut fog, mut transform) in &mut mists {
        match wanted.iter().position(|w| w.0 == *mist) {
            Some(i) => {
                let (_, at, yaw, want) = wanted.swap_remove(i);
                if *fog != want {
                    *fog = want;
                }
                if transform.translation != at || transform.rotation != yaw {
                    transform.translation = at;
                    transform.rotation = yaw;
                }
            }
            None => commands.entity(entity).despawn(),
        }
    }
    for (mist, at, yaw, fog) in wanted {
        commands.spawn((
            mist,
            fog,
            Transform::from_translation(at).with_rotation(yaw),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;

    #[test]
    fn bigger_falls_stand_in_bigger_clouds() {
        // 5 m³/s over 20 m, and 0.2 m³/s over 3 m.
        let big = mist_clouds((DVec3::ZERO, Vec2::X, 9810.0 * 5.0 * 20.0));
        let small = mist_clouds((DVec3::ZERO, Vec2::X, 9810.0 * 0.2 * 3.0));
        assert!(big[0].2.half_extents[0] > 3.0 * small[0].2.half_extents[0]);
        assert!(big[0].2.density > small[0].2.density);
        // A flash flood (six times that) stands in a wider cloud, no harder to see through.
        let flood = mist_clouds((DVec3::ZERO, Vec2::X, 9810.0 * 30.0 * 20.0));
        let across = |c: &FogVolume| 2.0 * c.half_extents[0] * c.density;
        assert!(flood[0].2.half_extents[0] > big[0].2.half_extents[0] * 1.5);
        assert!(across(&flood[0].2) < 0.61 && across(&big[0].2) < 0.61);
        // The veil drifts off downstream (+x here), thinner and wider than the cloud.
        assert!(big[1].0.x > big[0].0.x + 5.0);
        assert!(big[1].2.density < big[0].2.density * 0.5);
        assert!(big[1].2.half_extents[2] > big[0].2.half_extents[2]);
        // Its long axis (local z) turned downstream.
        let long = big[1].1 * Vec3::Z;
        assert!(long.x > 0.99, "{long}");
    }

    #[test]
    fn nearby_plunges_share_a_cloud_and_far_ones_get_none() {
        let camera = DVec3::ZERO;
        let plunges = [
            (DVec3::new(10.0, 0.0, 0.0), Vec2::X, 50_000.0),
            (DVec3::new(12.0, 0.0, 1.0), Vec2::X, 30_000.0),
            (DVec3::new(40.0, 0.0, 0.0), Vec2::Y, 20_000.0),
            (DVec3::new(5_000.0, 0.0, 0.0), Vec2::X, 1e6),
            (DVec3::new(20.0, 0.0, 0.0), Vec2::X, 10.0),
        ];
        let sites = mist_sites(&plunges, camera);
        assert_eq!(sites.len(), 2, "{sites:?}");
        assert!((sites[0].2 - 80_000.0).abs() < 1.0);
        assert!(sites[0].0.x > 10.0 && sites[0].0.x < 12.0);
        assert_eq!(sites[1].0.x, 40.0);
    }
}

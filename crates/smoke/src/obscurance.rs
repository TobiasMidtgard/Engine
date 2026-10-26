//! What the rest of the engine sees of the smoke: after every simulation step, a copy of each
//! volume's density goes to `engine_world::Obscurance` under the name "smoke", so NPC AI,
//! gameplay and UI can ask how much smoke is between two points without depending on this
//! module.

use std::sync::Arc;

use engine_core::math::{DVec3, UVec3};
use engine_core::prelude::*;
use engine_world::{Obscurance, ObscuranceQuery};

use crate::domain::{Domain, clip_box};
use crate::settings::SmokeSettings;
use crate::sim::SmokeDomains;

/// The name smoke registers under in [`Obscurance`].
pub const SOURCE: &str = "smoke";

/// One volume's smoke, frozen at a simulation step.
#[derive(Debug, Clone)]
struct Frozen {
    origin: DVec3,
    cell: f32,
    dims: UVec3,
    /// Density per voxel, kg/m³ (x fastest, then y, then z).
    density: Vec<f32>,
}

impl Frozen {
    fn of(d: &Domain) -> Self {
        Self {
            origin: d.origin,
            cell: d.cell,
            dims: d.dims,
            density: d.voxels.iter().map(|v| v.density).collect(),
        }
    }

    fn size(&self) -> DVec3 {
        self.dims.as_dvec3() * self.cell as f64
    }

    fn inside(&self, p: DVec3) -> bool {
        let q = p - self.origin;
        let s = self.size();
        q.x >= 0.0 && q.y >= 0.0 && q.z >= 0.0 && q.x < s.x && q.y < s.y && q.z < s.z
    }

    /// Density at a point inside the box, blended between voxel centers the way the
    /// simulation samples it (below the bottom layer counts as the bottom layer).
    fn density(&self, p: DVec3) -> f32 {
        let q = ((p - self.origin) / self.cell as f64).as_vec3() - 0.5;
        let i = q.floor();
        let f = q - i;
        let i = i.as_ivec3();
        let mut sum = 0.0;
        for corner in 0..8 {
            let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let w = (if dx == 1 { f.x } else { 1.0 - f.x })
                * (if dy == 1 { f.y } else { 1.0 - f.y })
                * (if dz == 1 { f.z } else { 1.0 - f.z });
            let (x, y, z) = (i.x + dx, (i.y + dy).max(0), i.z + dz);
            if w <= 0.0
                || x < 0
                || z < 0
                || x >= self.dims.x as i32
                || y >= self.dims.y as i32
                || z >= self.dims.z as i32
            {
                continue;
            }
            let k = x as u32 + self.dims.x * (y as u32 + self.dims.y * z as u32);
            sum += self.density[k as usize] * w;
        }
        sum
    }

    /// Optical depth along a–b per unit extinction (kg/m²), the way `Domain::transmittance`
    /// steps it.
    fn column(&self, a: DVec3, b: DVec3) -> f32 {
        let Some((t0, t1)) = clip_box(self.origin, self.origin + self.size(), a, b) else {
            return 0.0;
        };
        let len = (b - a).length() * (t1 - t0);
        let steps = ((len / (self.cell as f64 * 0.5)).ceil() as usize).clamp(1, 4096);
        let ds = (len / steps as f64) as f32;
        (0..steps)
            .map(|i| {
                let t = t0 + (t1 - t0) * (i as f64 + 0.5) / steps as f64;
                self.density(a.lerp(b, t)) * ds
            })
            .sum()
    }
}

/// All the smoke, frozen at a simulation step: the [`ObscuranceQuery`] this module registers.
#[derive(Debug, Clone, Default)]
pub struct SmokeSnapshot {
    /// m²/kg: how much one kilogram of smoke per cubic meter dims light, per meter.
    extinction_m2_kg: f32,
    volumes: Vec<Arc<Frozen>>,
}

impl SmokeSnapshot {
    /// A copy of every volume that has smoke in it.
    pub fn of(domains: &SmokeDomains, extinction_m2_kg: f32) -> Self {
        Self::reusing(domains, extinction_m2_kg, &mut Vec::new())
    }

    /// Like [`of`](Self::of), copying only the volumes that changed since the copies in
    /// `kept` (by id and version) were made; `kept` ends up holding this snapshot's copies.
    fn reusing(
        domains: &SmokeDomains,
        extinction_m2_kg: f32,
        kept: &mut Vec<(u32, u64, Arc<Frozen>)>,
    ) -> Self {
        let old = std::mem::take(kept);
        for d in &domains.list {
            if d.domain.mass_kg <= 1e-6 && d.domain.max_density <= 0.0 {
                continue;
            }
            let version = d.domain.version;
            let frozen = old
                .iter()
                .find(|(id, v, _)| *id == d.id && *v == version)
                .map(|(.., f)| f.clone())
                .unwrap_or_else(|| Arc::new(Frozen::of(&d.domain)));
            kept.push((d.id, version, frozen));
        }
        Self {
            extinction_m2_kg: extinction_m2_kg.max(0.0),
            volumes: kept.iter().map(|(.., f)| f.clone()).collect(),
        }
    }
}

impl ObscuranceQuery for SmokeSnapshot {
    fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        if from == to {
            return 1.0;
        }
        let tau: f32 = self.volumes.iter().map(|v| v.column(from, to)).sum();
        (-tau * self.extinction_m2_kg).exp()
    }

    fn extinction(&self, p: DVec3) -> f32 {
        self.volumes
            .iter()
            .filter(|v| v.inside(p))
            .map(|v| v.density(p))
            .sum::<f32>()
            * self.extinction_m2_kg
    }
}

/// The last snapshot's copy of each volume, by id and version, for the next one to reuse.
#[derive(Default)]
pub struct Copies(Vec<(u32, u64, Arc<Frozen>)>);

/// Each volume's id and version, and the thickness setting: what a snapshot was taken of.
type Taken = (Vec<(u32, u64)>, f32);

/// Hand [`Obscurance`] a fresh snapshot whenever a volume has stepped, been cleared, come or
/// gone, or the thickness setting changed.
pub fn publish(
    settings: Res<SmokeSettings>,
    domains: Res<SmokeDomains>,
    mut obscurance: ResMut<Obscurance>,
    mut seen: Local<Option<Taken>>,
    mut kept: Local<Copies>,
) {
    let now = (
        domains
            .list
            .iter()
            .map(|d| (d.id, d.domain.version))
            .collect::<Vec<_>>(),
        settings.extinction_m2_kg,
    );
    if seen.as_ref() == Some(&now) {
        return;
    }
    *seen = Some(now);
    obscurance.set(
        SOURCE,
        Arc::new(SmokeSnapshot::reusing(
            &domains,
            settings.extinction_m2_kg,
            &mut kept.0,
        )),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;
    use engine_world::check_obscurance;

    fn puffs() -> SmokeDomains {
        let mut domains = SmokeDomains::default();
        let mut d = Domain::new(DVec3::new(-8.0, 0.0, -8.0), 0.5, UVec3::new(32, 16, 32));
        d.splat(
            DVec3::new(0.0, 2.0, 0.0),
            2.0,
            0.4,
            20.0,
            Vec3::ZERO,
            Vec3::ONE,
        );
        // Smoke on the floor, so rays near the ground meet the bottom layer.
        d.splat(
            DVec3::new(4.0, 0.2, 3.0),
            1.5,
            0.2,
            0.0,
            Vec3::ZERO,
            Vec3::ONE,
        );
        domains.add(None, d);
        // A second, overlapping volume with a puff of its own.
        let mut e = Domain::new(DVec3::new(-2.0, 0.0, -2.0), 1.0, UVec3::new(10, 8, 10));
        e.splat(
            DVec3::new(2.0, 3.0, 2.0),
            1.5,
            0.3,
            20.0,
            Vec3::ZERO,
            Vec3::ONE,
        );
        domains.add(None, e);
        domains
    }

    #[test]
    fn the_snapshot_passes_the_obscurance_contract() {
        let snap = SmokeSnapshot::of(&puffs(), 100.0);
        let segments = [
            // Straight through the middle puff.
            (DVec3::new(-12.0, 2.0, 0.0), DVec3::new(12.0, 2.0, 0.0)),
            // Through both volumes and both puffs.
            (DVec3::new(-6.0, 1.0, -6.0), DVec3::new(7.0, 4.0, 6.0)),
            // Down through the floor puff into the ground.
            (DVec3::new(4.0, 6.0, 3.0), DVec3::new(4.0, -2.0, 3.0)),
            // Starting inside the smoke.
            (DVec3::new(0.2, 2.1, 0.1), DVec3::new(3.0, 5.0, -9.0)),
            // Clear air beside it.
            (DVec3::new(-30.0, 2.0, 30.0), DVec3::new(30.0, 2.0, 30.0)),
        ];
        for (a, b) in segments {
            check_obscurance(&snap, a, b).unwrap_or_else(|e| panic!("{a} -> {b}: {e}"));
        }
        // Thick enough to hide behind.
        assert!(snap.transmittance(segments[0].0, segments[0].1) < 0.3);
        assert_eq!(snap.transmittance(segments[4].0, segments[4].1), 1.0);
    }

    #[test]
    fn the_snapshot_matches_the_simulation() {
        let domains = puffs();
        let extinction = 20.0;
        let snap = SmokeSnapshot::of(&domains, extinction);
        let (a, b) = (DVec3::new(-6.0, 1.0, -6.0), DVec3::new(7.0, 4.0, 6.0));
        let live = domains.transmittance(a, b, extinction);
        assert!(
            live < 0.95 && (snap.transmittance(a, b) - live).abs() < 1e-4,
            "{live}"
        );
        for p in [DVec3::new(0.3, 2.2, -0.4), DVec3::new(2.2, 2.8, 1.7)] {
            let live: f32 = domains
                .list
                .iter()
                .filter(|d| d.domain.contains(p))
                .map(|d| d.domain.sample(p).density * extinction)
                .sum();
            assert!(live > 0.0 && (snap.extinction(p) - live).abs() < 1e-3 * live.max(1.0));
        }
    }
}

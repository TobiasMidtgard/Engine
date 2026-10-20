//! Bracket fungi on deadwood: the shelves and hooves that fruit from standing dead trees, old
//! logs and stumps a few years after a tree dies. Each host has its own: tinder hooves on birch
//! snags and birch polypores on its logs and stumps, red-belted conks on conifers, tiers of
//! sulphur shelf on oak snags and turkey tail rosettes on oak logs and stumps.
//!
//! A bracket is one closed, smooth-shaded surface: its top (rising to the bark, with
//! concentric growth ridges and color zones), a rounded margin and the pale pore surface
//! underneath. Brackets are built in their host's model space with their backs sunk into the
//! bark and their shelves level once the host stands (snags and stumps: +Y up) or lies (logs:
//! `felling::log_rest_transform` turns the model's +X up). Colors are vertex colors, so one
//! plain material draws every fungus.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Rng, Vec2, Vec3, smoothstep};
use engine_render::Mesh;

use super::MeshBuilder;
use super::tree::point_at;
use crate::species::{BarkStyle, PlantForm, PlantSpecies, srgb};

/// A bracket fungus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fungus {
    /// Fomes fomentarius: grey hooves banded with growth ridges, on standing dead birch.
    TinderHoof,
    /// Fomitopsis betulina: smooth, buff-white kidneys with a thick rolled margin, on birch.
    BirchPolypore,
    /// Fomitopsis pinicola: dark hooves with an orange-red belt and a cream margin, on conifers.
    RedBelt,
    /// Laetiporus sulphureus: tiers of wavy orange shelves with lemon margins, on oak.
    SulphurShelf,
    /// Trametes versicolor: clusters of thin, multicolored banded rosettes, on hardwood.
    TurkeyTail,
}

/// Where on a dead tree fungi grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// A standing dead trunk (+Y up).
    Snag,
    /// A trunk lying on the ground (the model's +X up).
    Log,
    /// The stump left by felling (+Y up).
    Stump,
}

impl Host {
    fn up(self) -> Vec3 {
        match self {
            Host::Log => Vec3::X,
            Host::Snag | Host::Stump => Vec3::Y,
        }
    }
}

/// The fungus a species' deadwood grows on `host`, or `None` for plants without a trunk.
pub fn host_fungus(sp: &PlantSpecies, host: Host) -> Option<Fungus> {
    if !sp.is_tree() {
        return None;
    }
    Some(match (sp.shape.bark, sp.form, host) {
        (BarkStyle::Birch, _, Host::Snag) => Fungus::TinderHoof,
        (BarkStyle::Birch, ..) => Fungus::BirchPolypore,
        (BarkStyle::Pine, ..) | (_, PlantForm::Conifer, _) => Fungus::RedBelt,
        (_, _, Host::Snag) => Fungus::SulphurShelf,
        _ => Fungus::TurkeyTail,
    })
}

/// How a fungus grows and looks.
struct Look {
    /// Half-width of a shelf along the bark, meters.
    width: (f32, f32),
    /// How far a shelf reaches out from the bark, × its half-width.
    reach: f32,
    /// How far the top rises toward the bark above the margin, × reach, and how full its
    /// profile is (the exponent of `1 - t`: below 1 domed like a hoof, above 1 thin and flat).
    rise: f32,
    profile: f32,
    /// How far shelves tilt off level, radians: up (+), or down (−) so their tops face outward,
    /// as thin, leathery rosettes do on an upright trunk.
    tilt: f32,
    /// How far the pore surface bulges down toward the bark, × reach.
    belly: f32,
    /// Half the margin's thickness, × half-width.
    lip: f32,
    /// How far the margin bends down, × reach.
    droop: f32,
    /// Lobes of the margin in plan (× reach) and up and down (× reach).
    wave: f32,
    flutter: f32,
    /// Growth ridges on the top and their height (× reach).
    ridges: f32,
    ridge: f32,
    /// Top colors from the bark out (sRGB), and whether they alternate in bands (turkey
    /// tail) or blend from one to the next.
    zones: &'static [u32],
    banded: bool,
    /// The growing margin, how far out it starts (0..1), and the pore surface (sRGB).
    margin: u32,
    margin_from: f32,
    pores: u32,
    /// Shelves per cluster and their spacing (× half-width).
    tiers: (u32, u32),
    gap: f32,
    /// Clusters on a snag, a log and a stump.
    clusters: [(u32, u32); 3],
    /// Mesh resolution: across and out.
    grid: (usize, usize),
}

impl Fungus {
    fn look(self) -> Look {
        match self {
            Fungus::TinderHoof => Look {
                width: (0.07, 0.13),
                reach: 0.8,
                rise: 1.35,
                profile: 0.5,
                tilt: 0.0,
                belly: 0.22,
                lip: 0.1,
                droop: 0.0,
                wave: 0.04,
                flutter: 0.0,
                ridges: 6.0,
                ridge: 0.06,
                zones: &[0x3b3631, 0x58534d, 0x77726b, 0x6b6156],
                banded: false,
                margin: 0x957d60,
                margin_from: 0.8,
                pores: 0x6e5c48,
                tiers: (1, 1),
                gap: 0.0,
                clusters: [(3, 6), (2, 4), (1, 2)],
                grid: (13, 8),
            },
            Fungus::BirchPolypore => Look {
                width: (0.05, 0.11),
                reach: 1.0,
                rise: 0.3,
                profile: 0.75,
                tilt: 0.05,
                belly: 0.06,
                lip: 0.17,
                droop: 0.18,
                wave: 0.06,
                flutter: 0.02,
                ridges: 0.0,
                ridge: 0.0,
                zones: &[0x8f7759, 0xbfae92, 0xd2c5ad],
                banded: false,
                margin: 0xe0d6c3,
                margin_from: 0.7,
                pores: 0xe9e3d5,
                tiers: (1, 1),
                gap: 0.0,
                clusters: [(3, 6), (5, 9), (2, 3)],
                grid: (13, 7),
            },
            Fungus::RedBelt => Look {
                width: (0.07, 0.13),
                reach: 0.85,
                rise: 0.8,
                profile: 0.6,
                tilt: 0.0,
                belly: 0.15,
                lip: 0.12,
                droop: 0.0,
                wave: 0.05,
                flutter: 0.0,
                ridges: 4.0,
                ridge: 0.03,
                zones: &[0x1d1511, 0x2c1c15, 0x6a2814, 0x9a3f19],
                banded: false,
                margin: 0xd9c59b,
                margin_from: 0.88,
                pores: 0xd6c49c,
                tiers: (1, 1),
                gap: 0.0,
                clusters: [(2, 5), (4, 8), (1, 3)],
                grid: (13, 8),
            },
            Fungus::SulphurShelf => Look {
                width: (0.07, 0.14),
                reach: 0.75,
                rise: 0.14,
                profile: 1.2,
                tilt: 0.15,
                belly: 0.05,
                lip: 0.07,
                droop: 0.12,
                wave: 0.1,
                flutter: 0.1,
                ridges: 0.0,
                ridge: 0.0,
                zones: &[0xc85a17, 0xdf7a1d, 0xeb9a28],
                banded: false,
                margin: 0xf2c843,
                margin_from: 0.7,
                pores: 0xefd04a,
                tiers: (3, 6),
                gap: 0.5,
                clusters: [(1, 3), (1, 3), (1, 2)],
                grid: (13, 7),
            },
            Fungus::TurkeyTail => Look {
                width: (0.03, 0.06),
                reach: 0.85,
                rise: 0.08,
                profile: 1.5,
                tilt: -0.85,
                belly: 0.02,
                lip: 0.05,
                droop: 0.06,
                wave: 0.1,
                flutter: 0.07,
                ridges: 8.0,
                ridge: 0.012,
                zones: &[
                    0x2e221b, 0x8a7a62, 0x4a3a2e, 0x3a4552, 0x6b5a48, 0x2a2622, 0x9a958c, 0x45362a,
                ],
                banded: true,
                margin: 0xdcd0b4,
                margin_from: 0.85,
                pores: 0xe4ddce,
                tiers: (3, 8),
                gap: 0.55,
                clusters: [(2, 4), (5, 10), (2, 4)],
                grid: (11, 6),
            },
        }
    }
}

/// Fungi fruiting from a trunk (`path` with `radius`, both in model space) between fractions
/// `along.0` and `along.1` of its length. `None` when the trunk is too thin for this fungus.
pub fn on_trunk(
    path: &[Vec3],
    radius: &[f32],
    host: Host,
    fungus: Fungus,
    along: (f32, f32),
    seed: u64,
) -> Option<Mesh> {
    if path.len() < 2 {
        return None;
    }
    let look = fungus.look();
    let mut rng = Rng::new(seed ^ 0xf0_9915 ^ ((host as u64) << 40));
    let length: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    let up = host.up();
    let (lo, hi) = look.clusters[host as usize];
    let clusters = rng.range_i32(lo as i32, hi as i32 + 1);
    // Fungi favour one side of a standing trunk (the hyphae spread up and down a column of
    // decay); on a log, the two flanks.
    let side = rng.range(0.0, TAU);
    let mut b = MeshBuilder::new();
    for _ in 0..clusters {
        let s = rng.range(along.0, along.1);
        let a = match host {
            Host::Log => {
                let flank = if rng.range(0.0, 1.0) < 0.5 { 1.0 } else { -1.0 };
                flank * rng.range(1.25, 2.05)
            }
            _ if rng.range(0.0, 1.0) < 0.7 => side + rng.range(-0.7, 0.7),
            _ => rng.range(0.0, TAU),
        };
        let shelves = rng.range_i32(look.tiers.0 as i32, look.tiers.1 as i32 + 1);
        // Big trunks carry big brackets.
        let girth = (point_at(path, radius, s).2 / 0.2).clamp(0.7, 1.5);
        let width = rng.range(look.width.0, look.width.1) * girth;
        for j in 0..shelves {
            let w = width * rng.range(0.7, 1.1);
            // Tiers climb the trunk (standing) or the log's flank toward its top.
            let step = j as f32 * look.gap * width;
            let (s, a) = match host {
                Host::Log => (
                    s + rng.range(-1.2, 1.2) * width / length,
                    a - a.signum() * step / point_at(path, radius, s).2.max(0.02),
                ),
                _ => (s + step / length, a + rng.range(-0.25, 0.25)),
            };
            let (p, axis, r) = point_at(path, radius, s.clamp(0.0, 1.0));
            // Too thin a trunk for this fungus: skip it here.
            if w > r * 1.3 || r < 0.03 {
                continue;
            }
            let e1 = (up - axis * up.dot(axis))
                .try_normalize()
                .unwrap_or_else(|| {
                    let x = Vec3::X - axis * axis.x;
                    x.normalize_or(Vec3::Z)
                });
            let e2 = axis.cross(e1);
            let radial = e1 * a.cos() + e2 * a.sin();
            let out = (radial - up * radial.dot(up)).normalize_or(radial);
            let lying = host == Host::Log;
            bracket(
                &mut b,
                &look,
                p + radial * r * 0.96,
                out,
                up,
                r,
                lying,
                w,
                &mut rng,
            );
        }
    }
    if b.is_empty() {
        return None;
    }
    let mut mesh = b.finish();
    mesh.compute_normals();
    Some(mesh)
}

/// One shelf attached at `at` on bark of radius `bark_r`, reaching along `out` (level), with
/// `up` up, `half_width` wide either side. On a `lying` trunk the bark curves away above and
/// below the shelf as well as to its sides.
#[allow(clippy::too_many_arguments)]
fn bracket(
    b: &mut MeshBuilder,
    look: &Look,
    at: Vec3,
    out: Vec3,
    up: Vec3,
    bark_r: f32,
    lying: bool,
    half_width: f32,
    rng: &mut Rng,
) {
    let side = up.cross(out);
    // Tilt the shelf up about the line it grows from.
    let tilt = look.tilt * rng.range(0.7, 1.2);
    let (out, up) = (
        out * tilt.cos() + up * tilt.sin(),
        up * tilt.cos() - out * tilt.sin(),
    );
    let reach = half_width * look.reach * rng.range(0.85, 1.15);
    let rise = reach * look.rise * rng.range(0.8, 1.2);
    let belly = reach * look.belly;
    let lip = half_width * look.lip;
    let droop = reach * look.droop * rng.range(0.6, 1.3);
    let ridge = reach * look.ridge;
    let phase = [0, 1, 2].map(|_| rng.range(0.0, TAU));
    let bright = rng.range(0.85, 1.12);
    let zone_shift = rng.range(0.0, 1.0);
    // The back sits just inside the bark; on a log, deep enough for the top and the belly to
    // meet the bark curving away from them.
    let curve = if lying {
        (rise + belly).powi(2) / (2.0 * bark_r)
    } else {
        0.0
    };
    let inset = 0.004 + 0.05 * (rise + belly) + curve;
    let (nx, nt) = look.grid;
    let lip_rows = 3;
    let rows = nt + lip_rows + nt;
    let base = b.vertices.len() as u32;
    let col = |hex: u32| Vec3::from(srgb(hex)) * bright;
    for j in 0..rows {
        for i in 0..nx {
            let x = i as f32 / (nx - 1) as f32 * 2.0 - 1.0;
            let fat = (1.0 - x * x).max(0.0);
            let taper = fat.powf(0.35);
            let lobes =
                1.0 + look.wave * ((x * 5.3 + phase[0]).sin() + 0.5 * (x * 11.1 + phase[1]).sin());
            let reach_x = reach * fat.sqrt() * lobes;
            let flutter = reach * look.flutter * (x * 4.3 + phase[2]).sin();
            // Where along the shelf (0 at the bark .. 1 at the margin), how high, how far past
            // the margin (round the lip), and on which face.
            let (t, h, beyond, face) = if j < nt {
                let t = j as f32 / (nt - 1) as f32;
                let crest = 0.5 - 0.5 * (TAU * look.ridges * t).cos();
                let h = lip + rise * (1.0 - t).powf(look.profile) + ridge * crest * (1.0 - 0.5 * t);
                (t, h * taper, 0.0, 0)
            } else if j < nt + lip_rows {
                let phi = (j - nt + 1) as f32 / (lip_rows + 1) as f32 * PI;
                (1.0, lip * phi.cos() * taper, lip * phi.sin() * taper, 1)
            } else {
                let t = 1.0 - (j - nt - lip_rows) as f32 / (nt - 1) as f32;
                (t, -(lip + belly * (1.0 - t).powi(2)) * taper, 0.0, 2)
            };
            let bend = -droop * t * t + flutter * t * t;
            // Follow the bark round: the shelf's sides lie back against the curved trunk.
            let lateral = half_width * x;
            let sag = bark_r - (bark_r * bark_r - lateral * lateral).max(0.0).sqrt();
            let p = at
                + out * (reach_x * t + beyond * fat.sqrt() - inset - sag)
                + side * lateral
                + up * (h + bend);
            let color = match face {
                0 => {
                    let crest = 0.5 - 0.5 * (TAU * look.ridges * t).cos();
                    let zone = if look.banded {
                        let k = (t * look.ridges * 1.4 + zone_shift * 3.0 + 0.3 * x).floor();
                        col(look.zones[k as usize % look.zones.len()])
                    } else {
                        let f = t * (look.zones.len() - 1) as f32;
                        let k = (f as usize).min(look.zones.len() - 2);
                        col(look.zones[k]).lerp(col(look.zones[k + 1]), f - k as f32)
                    };
                    let c = zone * (0.86 + 0.14 * crest);
                    let from = look.margin_from;
                    let c = c.lerp(col(look.margin), smoothstep(from, from + 0.2, t));
                    // Dark where the shelf meets the bark.
                    c * (0.55 + 0.45 * smoothstep(0.0, 0.2, t))
                }
                1 => col(look.margin),
                _ => col(look.pores) * (0.5 + 0.5 * smoothstep(0.0, 0.45, t)),
            };
            b.vertex(
                p,
                up,
                Vec2::new(0.5 + 0.5 * x, j as f32 / (rows - 1) as f32),
                color.extend(1.0),
            );
        }
    }
    // Quads wound so the top faces up, the margin out and the pores down.
    for j in 0..rows as u32 - 1 {
        for i in 0..nx as u32 - 1 {
            let at = |j: u32, i: u32| base + j * nx as u32 + i;
            let (a, bb, c, d) = (at(j, i), at(j + 1, i), at(j, i + 1), at(j + 1, i + 1));
            b.tri(a, bb, c);
            b.tri(c, bb, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::PlantLibrary;

    fn trunk() -> (Vec<Vec3>, Vec<f32>) {
        let path: Vec<Vec3> = (0..8).map(|i| Vec3::Y * (0.5 + i as f32 * 0.6)).collect();
        let radius = (0..8).map(|i| 0.22 - i as f32 * 0.015).collect();
        (path, radius)
    }

    #[test]
    fn each_host_species_grows_its_own_fungus() {
        let lib = PlantLibrary::default();
        let sp = |n: &str| lib.get(lib.id(n).unwrap()).clone();
        assert_eq!(
            host_fungus(&sp("birch"), Host::Snag),
            Some(Fungus::TinderHoof)
        );
        assert_eq!(
            host_fungus(&sp("birch"), Host::Log),
            Some(Fungus::BirchPolypore)
        );
        assert_eq!(host_fungus(&sp("spruce"), Host::Log), Some(Fungus::RedBelt));
        assert_eq!(
            host_fungus(&sp("oak"), Host::Snag),
            Some(Fungus::SulphurShelf)
        );
        assert_eq!(
            host_fungus(&sp("oak"), Host::Stump),
            Some(Fungus::TurkeyTail)
        );
        assert_eq!(host_fungus(&sp("fern"), Host::Log), None);
    }

    #[test]
    fn shelves_sit_on_the_bark_level_and_face_the_right_way() {
        let (path, radius) = trunk();
        for fungus in [Fungus::TinderHoof, Fungus::SulphurShelf, Fungus::TurkeyTail] {
            let mesh = on_trunk(&path, &radius, Host::Snag, fungus, (0.1, 0.7), 7).unwrap();
            for v in &mesh.vertices {
                let p = Vec3::from(v.position);
                let r = Vec2::new(p.x, p.z).length();
                // Nothing reaches absurdly far from the bark.
                assert!(r < 0.22 + 0.3, "{fungus:?} vertex at {p:?}");
            }
            // Most of the top faces up, most of the underside down: the shelves are level (hoofs
            // slope steeply, thin rosettes droop to face outward).
            let ups = mesh.vertices.iter().filter(|v| v.normal[1] > 0.2).count();
            let downs = mesh.vertices.iter().filter(|v| v.normal[1] < -0.2).count();
            assert!(ups > mesh.vertices.len() / 4, "{fungus:?}: {ups} up");
            assert!(downs > mesh.vertices.len() / 6, "{fungus:?}: {downs} down");
        }
    }

    #[test]
    fn log_fungi_grow_from_the_flanks_level_once_the_log_lies() {
        let (path, radius) = trunk();
        let mesh = on_trunk(
            &path,
            &radius,
            Host::Log,
            Fungus::BirchPolypore,
            (0.1, 0.9),
            3,
        )
        .unwrap();
        // The log lies with the model's +X up: shelves stick out sideways (±Z), and their
        // tops face +X.
        let (mut flank, mut top) = (0, 0);
        for v in &mesh.vertices {
            let p = Vec3::from(v.position);
            if p.z.abs() > p.x.abs() {
                flank += 1;
            }
            if v.normal[0] > 0.5 {
                top += 1;
            }
        }
        assert!(flank > mesh.vertices.len() * 3 / 4, "{flank}");
        assert!(top > mesh.vertices.len() / 4, "{top}");
    }

    #[test]
    fn every_tree_fruits_on_its_snag_log_and_stump() {
        let lib = PlantLibrary::default();
        for name in ["oak", "birch", "pine", "spruce"] {
            let sp = lib.get(lib.id(name).unwrap());
            for seed in 0..4u64 {
                let m = crate::meshgen::tree::woody(sp, seed * 0x9e37_79b9, false);
                assert!(m.snag_fungi.is_some(), "{name} {seed}: snag");
                assert!(m.log_fungi.is_some(), "{name} {seed}: log");
                assert!(m.stump_fungi.is_some(), "{name} {seed}: stump");
            }
        }
    }

    #[test]
    fn thin_trunks_grow_no_big_brackets() {
        let path: Vec<Vec3> = (0..5).map(|i| Vec3::Y * i as f32 * 0.5).collect();
        let radius = vec![0.02; 5];
        assert!(
            on_trunk(
                &path,
                &radius,
                Host::Snag,
                Fungus::TinderHoof,
                (0.0, 1.0),
                1
            )
            .is_none()
        );
    }
}

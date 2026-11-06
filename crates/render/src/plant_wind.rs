//! Trees and shrubs bending on wind bones (engine-design/foliage-wind-research.md §3.1-3.2).
//!
//! A plant mesh registered in [`PlantWindMeshes`] with its [`PlantSkeleton`] and a species
//! slot moves on its bones instead of the legacy sway (`StandardMaterial::wind_sway` times the
//! vertex alpha):
//! - each vertex names its bone in its color alpha ([`pack_plant_vertex`]);
//! - the shader (`plant_wind` in `shaders/lib_wind.wgsl`) turns it about its bone's pivot and
//!   then its parents' pivots, deepest first, so a spray flutters on a branch that sways on a
//!   limb that nods on a trunk;
//! - the trunk leans with the gusts that passed its root a quarter, three quarters and five
//!   quarters of a sway period ago, so it lags a gust, overshoots and swings back like a damped
//!   tree, with no state kept;
//! - each species answers through its [`PlantWindSpecies`] slot: sway rates, the wind speeds
//!   at which leaves, branches and the crown start moving, how far sprays twist to show their
//!   paler undersides.
//!
//! The material must still have `wind_sway > 0`: the passes that keep swaying meshes out of
//! static shadow images and give them their own motion vectors key on it. Its value is unused.
//! All levels of detail of one tree should share one `Arc<PlantSkeleton>`, so a switch between
//! them doesn't pop; coarser levels draw fewer bone levels ([`PlantWindMesh::detail`]).

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_assets::Handle;
use engine_core::prelude::*;

use crate::mesh::Mesh;

/// The most bones a skeleton holds: 14 bits of the packed vertex word.
pub const MAX_PLANT_BONES: usize = (1 << 14) - 1;

/// Words per species slot in the shader's table (`PLANT_SPECIES_WORDS` in lib_wind.wgsl).
const SPECIES_WORDS: usize = 9;

/// One bone of a plant: a limb, branch or leaf spray turning about its pivot, or the trunk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlantBone {
    /// Mesh space, meters.
    pub pivot: Vec3,
    /// Pivot to tip, meters (the trunk's: the tree's height).
    pub length: f32,
    /// Unit direction from pivot to tip.
    pub axis: Vec3,
    /// A spray's card facing (its twist turns this away from the wind); for other bones any
    /// unit vector across the axis.
    pub face: Vec3,
    /// The bone it hangs from (none for the trunk). Parents come before their children.
    pub parent: Option<u16>,
    /// 0 trunk (bent by the lean, not turned), 1 limb, 2 branch, 3 spray.
    pub level: u8,
    /// Its own place in the sway cycle, 0..1, so neighbors don't move in step.
    pub phase: f32,
    /// 1 for a typical bone of its level; thicker ones more (0.25..4). Divides the bend.
    pub stiffness: f32,
}

/// A plant's bones, the trunk (level 0) first and every parent before its children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlantSkeleton {
    pub bones: Vec<PlantBone>,
}

impl PlantSkeleton {
    /// How far a point of the mesh can move at most (m, mesh space) when each level turns by
    /// up to its `most` angle (radians: limb, branch, spray) and the trunk's top leans by
    /// `lean_m`: how much culling bounds grow.
    pub fn reach_m(&self, most: [f32; 3], lean_m: f32) -> f32 {
        let mut reach = vec![0.0f32; self.bones.len()];
        let mut furthest = 0.0f32;
        for (i, b) in self.bones.iter().enumerate() {
            let own = match b.level {
                1..=3 => b.length * most[b.level as usize - 1].min(std::f32::consts::PI),
                _ => 0.0,
            };
            let up = b
                .parent
                .and_then(|p| reach.get(p as usize).copied())
                .unwrap_or(0.0);
            reach[i] = own + up;
            furthest = furthest.max(reach[i]);
        }
        furthest + lean_m
    }
}

/// A vertex's color alpha on a bone plant: its bone (14 bits), tip weight 0..1 (5 bits; 0 at
/// the bone's pivot, 1 at its tip), edge 0..1 (4 bits; how far out across a leaf card, where
/// it flutters most) and whether it is on a leaf card's underside (1 bit: its albedo is
/// multiplied by the species' `underside_gain`), as an integer an `f32` holds exactly.
/// `engine_foliage::meshgen::wind_bones::pack` is the same.
pub fn pack_plant_vertex(bone: u16, tip: f32, edge: f32, underside: bool) -> f32 {
    let bone = u32::from(bone).min(MAX_PLANT_BONES as u32);
    let tip = (tip.clamp(0.0, 1.0) * 31.0).round() as u32;
    let edge = (edge.clamp(0.0, 1.0) * 15.0).round() as u32;
    (bone | tip << 14 | edge << 19 | u32::from(underside) << 23) as f32
}

/// How one species moves in the wind (engine-design/foliage-wind-research.md §3.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlantSpeciesWind {
    /// The trunk's sway rate, Hz (0.2-0.3 for forest trees, faster for small ones).
    pub f0_hz: f32,
    /// Damping ratio (about 0.09 in leaf, 0.04 bare): how far the trunk overshoots a gust.
    pub damping: f32,
    /// 1 for a reference tree; divides every bend and the lean.
    pub stiffness: f32,
    /// Limb and branch sway rates as multiples of `f0_hz`.
    pub limb_freq: f32,
    pub branch_freq: f32,
    /// Leaf flutter, Hz (0: none, needles).
    pub flutter_hz: f32,
    /// Wind speeds (m/s) where leaf flutter, branch sway and crown bending start and reach
    /// full strength. Limbs answer halfway between branches and the crown.
    pub flutter_ramp: [f32; 2],
    pub branch_ramp: [f32; 2],
    pub crown_ramp: [f32; 2],
    /// Sprays twisting away from the wind: onset and full wind (m/s) and the most they turn
    /// (degrees; past 90 their undersides face the viewer). 0 degrees: none.
    pub flip: [f32; 3],
    /// Albedo multiplier of a leaf's underside.
    pub underside_gain: f32,
    /// The most a limb, branch and spray turn in a gale, degrees, for a bone of stiffness 1.
    pub bend_deg: [f32; 3],
    /// Leaf flutter's amplitude at a card's tip, meters.
    pub flutter_m: f32,
    /// The trunk's lean at its top in a gale (15 m/s), as a share of its height.
    pub gale_lean: f32,
    /// How far each bone sways about its bent pose, as a share of its bend (0: held still in
    /// the bent pose).
    pub sway: f32,
    /// Share of the ground's snow the crown holds on the upper side of its sprays, cards,
    /// branches and limbs (0: none). Dense conifers hold the most (spruce and fir about 1, pine
    /// less), bare broadleaves only on their limbs (about 0.3), larch little.
    pub snow_hold: f32,
}

impl Default for PlantSpeciesWind {
    fn default() -> Self {
        // A middling broadleaf (engine_foliage::wind::PlantWind's default): about 1 m at the
        // top of a 20 m tree in a gale.
        Self {
            f0_hz: 0.35,
            damping: 0.09,
            stiffness: 1.0,
            limb_freq: 1.9,
            branch_freq: 3.4,
            flutter_hz: 3.0,
            flutter_ramp: [2.0, 5.0],
            branch_ramp: [4.5, 9.0],
            crown_ramp: [8.0, 15.0],
            flip: [8.0, 15.0, 110.0],
            underside_gain: 1.3,
            bend_deg: [5.0, 10.0, 18.0],
            flutter_m: 0.02,
            gale_lean: 0.05,
            sway: 0.35,
            snow_hold: 0.6,
        }
    }
}

impl PlantSpeciesWind {
    /// The most each level turns (radians: limb, branch, spray), stiffness applied.
    pub fn most_rad(&self) -> [f32; 3] {
        let s = self.stiffness.max(0.05);
        self.bend_deg.map(|d| d.to_radians() / s)
    }

    /// The trunk's lean at its top in a gale, meters, for a plant `height_m` tall.
    pub fn gale_lean_m(&self, height_m: f32) -> f32 {
        self.gale_lean * height_m / self.stiffness.max(0.05)
    }
}

/// The species slots bone plants answer the wind through (filled by the foliage module, read
/// by the renderer each frame). [`PlantWindMesh::species`] indexes `slots`; a missing slot
/// reads as the default.
#[derive(Resource, Debug, Clone, Default)]
pub struct PlantWindSpecies {
    pub slots: Vec<PlantSpeciesWind>,
}

/// How one plant mesh moves on its bones.
#[derive(Debug, Clone)]
pub struct PlantWindMesh {
    /// Shared by all its levels of detail.
    pub skeleton: Arc<PlantSkeleton>,
    /// Its species' slot in [`PlantWindSpecies`].
    pub species: u16,
    /// The deepest bone level this mesh turns: 3 near and hero levels (sprays, branches,
    /// limbs), 2 the middle level, 1 far levels, 0 the trunk's lean only.
    pub detail: u8,
}

/// The plant meshes that move on wind bones, by mesh. Any other mesh keeps the legacy sway.
#[derive(Resource, Debug, Clone, Default)]
pub struct PlantWindMeshes {
    meshes: bevy_platform::collections::HashMap<u32, PlantWindMesh>,
    version: u64,
}

impl PlantWindMeshes {
    /// Make `mesh` move on its bones (replacing any earlier entry).
    pub fn insert(&mut self, mesh: Handle<Mesh>, entry: PlantWindMesh) {
        self.meshes.insert(mesh.index(), entry);
        self.version += 1;
    }

    /// Back to the legacy sway.
    pub fn remove(&mut self, mesh: Handle<Mesh>) -> Option<PlantWindMesh> {
        let old = self.meshes.remove(&mesh.index());
        if old.is_some() {
            self.version += 1;
        }
        old
    }

    pub fn get(&self, mesh: Handle<Mesh>) -> Option<&PlantWindMesh> {
        self.meshes.get(&mesh.index())
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }
}

/// One bone as the shader reads it (two `vec4`s; see lib_wind.wgsl).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuBone {
    pivot_length: [f32; 4],
    words: [u32; 4],
}

/// Octahedral encoding of a unit vector as two snorm16s (x low, y high), as WGSL's
/// `unpack2x16snorm` reads them.
fn oct_encode(v: Vec3) -> u32 {
    let v = v.try_normalize().unwrap_or(Vec3::Y);
    let l1 = v.x.abs() + v.y.abs() + v.z.abs();
    let mut x = v.x / l1;
    let mut y = v.y / l1;
    if v.z < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    let snorm = |f: f32| ((f.clamp(-1.0, 1.0) * 32767.0).round() as i16) as u16 as u32;
    snorm(x) | snorm(y) << 16
}

fn unorm_pair(a: f32, b: f32) -> u32 {
    let unorm = |f: f32| (f.clamp(0.0, 1.0) * 65535.0).round() as u32;
    unorm(a) | unorm(b) << 16
}

impl GpuBone {
    fn new(b: &PlantBone) -> Self {
        let parent = b.parent.map_or(0xffff, u32::from);
        Self {
            pivot_length: b.pivot.extend(b.length.max(1e-3)).to_array(),
            words: [
                oct_encode(b.axis),
                oct_encode(b.face),
                parent | u32::from(b.level.min(3)) << 16,
                unorm_pair(b.phase.rem_euclid(1.0), b.stiffness / 4.0),
            ],
        }
    }
}

/// The shader's table (`plant_wind_words`): header, species slots (rewritten each frame for
/// their phases), mesh entries and bones (rewritten when [`PlantWindMeshes`] changes).
#[derive(Default)]
pub(crate) struct PlantWindTable {
    /// The version of [`PlantWindMeshes`] it was built from (`None`: never built).
    version: Option<u64>,
    /// Mesh slot index → table entry + 1, and how far its points move at most (m).
    entries: bevy_platform::collections::HashMap<u32, (u32, f32)>,
    /// The species slots it was laid out for.
    species: usize,
    pub(crate) words: Vec<[f32; 4]>,
    /// Whether the mesh entries and bones changed since the last upload.
    pub(crate) rebuilt: bool,
}

impl PlantWindTable {
    /// The table entry + 1 of mesh slot `index` (0: the legacy sway) and how far its points
    /// move at most (m, mesh space).
    pub(crate) fn entry(&self, index: u32) -> (u32, f32) {
        self.entries.get(&index).copied().unwrap_or((0, 0.0))
    }

    /// How many words the header and species slots take (rewritten each frame).
    pub(crate) fn species_words(&self) -> usize {
        1 + self.species * SPECIES_WORDS
    }

    /// Lay out the table again if the meshes or the species count changed, then write the
    /// species' phases for `seconds` now and `seconds_prev` last frame.
    pub(crate) fn update(
        &mut self,
        meshes: Option<&PlantWindMeshes>,
        species: Option<&PlantWindSpecies>,
        seconds: f64,
        seconds_prev: f64,
    ) {
        let slots: &[PlantSpeciesWind] = species.map_or(&[], |s| &s.slots);
        let version = meshes.map_or(0, |m| m.version);
        let count = slots.len().max(1);
        self.rebuilt = false;
        if self.version != Some(version) || self.species != count || self.words.is_empty() {
            self.layout(meshes, slots, count);
            self.version = Some(version);
            self.species = count;
            self.rebuilt = true;
        }
        let default = PlantSpeciesWind::default();
        for s in 0..count {
            let sp = slots.get(s).unwrap_or(&default);
            let base = 1 + s * SPECIES_WORDS;
            self.words[base..base + SPECIES_WORDS].copy_from_slice(&species_words(
                sp,
                seconds,
                seconds_prev,
            ));
        }
    }

    fn layout(
        &mut self,
        meshes: Option<&PlantWindMeshes>,
        slots: &[PlantSpeciesWind],
        count: usize,
    ) {
        self.entries.clear();
        let mut list: Vec<(u32, &PlantWindMesh)> = meshes
            .map(|m| m.meshes.iter().map(|(&k, v)| (k, v)).collect())
            .unwrap_or_default();
        list.sort_by_key(|(k, _)| *k);
        // Skeletons shared by levels of detail are laid out once.
        let mut placed: HashMap<*const PlantSkeleton, u32> = HashMap::new();
        let mut bones: Vec<GpuBone> = Vec::new();
        let mut mesh_words: Vec<[f32; 4]> = Vec::with_capacity(list.len());
        let default = PlantSpeciesWind::default();
        for (index, m) in list {
            let key = Arc::as_ptr(&m.skeleton);
            let first = *placed.entry(key).or_insert_with(|| {
                let first = bones.len() as u32;
                let n = m.skeleton.bones.len().min(MAX_PLANT_BONES);
                bones.extend(m.skeleton.bones[..n].iter().map(GpuBone::new));
                if n == 0 {
                    // A tree with no bones: a trunk of 1 m that never turns.
                    bones.push(GpuBone::new(&PlantBone {
                        pivot: Vec3::ZERO,
                        length: 1.0,
                        axis: Vec3::Y,
                        face: Vec3::X,
                        parent: None,
                        level: 0,
                        phase: 0.0,
                        stiffness: 1.0,
                    }));
                }
                first
            });
            let slot = usize::from(m.species).min(count - 1);
            let sp = slots.get(slot).unwrap_or(&default);
            let height = m.skeleton.bones.first().map_or(1.0, |b| b.length);
            // Bounds: every level at 1.5 times its gale bend plus a little, the lean likewise.
            let most = sp.most_rad().map(|a| a * 1.5 * 1.4 * 1.5);
            let reach = m.skeleton.reach_m(most, sp.gale_lean_m(height) * 2.5) + sp.flutter_m;
            mesh_words.push(bytemuck::cast([
                first,
                slot as u32,
                u32::from(m.detail.min(3)),
                reach.to_bits(),
            ]));
            self.entries.insert(index, (mesh_words.len() as u32, reach));
        }
        let mesh_start = 1 + count * SPECIES_WORDS;
        let bone_start = mesh_start + mesh_words.len();
        self.words.clear();
        self.words.push(bytemuck::cast([
            count as u32,
            mesh_start as u32,
            bone_start as u32,
            0u32,
        ]));
        self.words.resize(mesh_start, [0.0; 4]);
        self.words.extend(mesh_words);
        self.words
            .extend(bytemuck::cast_slice::<GpuBone, [f32; 4]>(&bones));
    }
}

/// One species slot's words (see lib_wind.wgsl): its phases now and last frame, its ramps,
/// bends, flip and trunk.
fn species_words(
    sp: &PlantSpeciesWind,
    seconds: f64,
    seconds_prev: f64,
) -> [[f32; 4]; SPECIES_WORDS] {
    let f0 = f64::from(sp.f0_hz.max(0.0));
    let rates = [
        f0,
        f0 * f64::from(sp.limb_freq),
        f0 * f64::from(sp.branch_freq),
        f64::from(sp.flutter_hz.max(0.0)),
    ];
    let cycles = |t: f64, hz: f64| (t * hz).rem_euclid(1.0) as f32;
    let phases = |t: f64| rates.map(|hz| cycles(t, hz));
    // Second modes at 1.6 times the limb and branch rates break the loop.
    let modes = [
        cycles(seconds, rates[1] * 1.6),
        cycles(seconds, rates[2] * 1.6),
        cycles(seconds_prev, rates[1] * 1.6),
        cycles(seconds_prev, rates[2] * 1.6),
    ];
    let limb = [
        0.5 * (sp.branch_ramp[0] + sp.crown_ramp[0]),
        0.5 * (sp.branch_ramp[1] + sp.crown_ramp[1]),
    ];
    let most = sp.most_rad();
    let flutter_m = if sp.flutter_hz > 0.0 {
        sp.flutter_m
    } else {
        0.0
    };
    // Lean per drag per meter of height: `gale_lean` at the drag of 15 m/s.
    let gale_drag = 15.0 * 15.0 / (6.0 + 15.0);
    let lean = sp.gale_lean / sp.stiffness.max(0.05) / gale_drag;
    [
        phases(seconds),
        phases(seconds_prev),
        modes,
        [
            sp.crown_ramp[0],
            limb[0],
            sp.branch_ramp[0],
            sp.flutter_ramp[0],
        ],
        [
            sp.crown_ramp[1],
            limb[1],
            sp.branch_ramp[1],
            sp.flutter_ramp[1],
        ],
        [most[0], most[1], most[2], flutter_m],
        [
            sp.flip[0],
            sp.flip[1],
            sp.flip[2].max(0.0).to_radians(),
            sp.underside_gain,
        ],
        [
            sp.f0_hz.max(0.0),
            (-std::f32::consts::PI * sp.damping.clamp(0.0, 1.0)).exp(),
            lean,
            sp.sway.max(0.0),
        ],
        [sp.snow_hold.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vertex_word_survives_a_float() {
        let w = pack_plant_vertex(16383, 1.0, 1.0, true);
        assert!(w < (1u32 << 24) as f32 && w.fract() == 0.0);
        let w = w as u32;
        assert_eq!(w & 0x3fff, 16383);
        assert_eq!((w >> 14) & 31, 31);
        assert_eq!((w >> 19) & 15, 15);
        assert_eq!((w >> 23) & 1, 1);
        assert_eq!(pack_plant_vertex(5, 0.0, 0.0, false), 5.0);
    }

    #[test]
    fn octahedral_axes_come_back() {
        for v in [
            Vec3::Y,
            -Vec3::Z,
            Vec3::new(0.3, -0.8, -0.5).normalize(),
            Vec3::new(-0.9, 0.1, 0.4).normalize(),
        ] {
            let w = oct_encode(v);
            let x = (w & 0xffff) as u16 as i16 as f32 / 32767.0;
            let y = (w >> 16) as u16 as i16 as f32 / 32767.0;
            let mut d = Vec3::new(x, y, 1.0 - x.abs() - y.abs());
            let t = (-d.z).max(0.0);
            d.x += if d.x >= 0.0 { -t } else { t };
            d.y += if d.y >= 0.0 { -t } else { t };
            assert!(d.normalize().dot(v) > 0.9999, "{v:?} came back as {d:?}");
        }
    }

    #[test]
    fn levels_of_detail_share_one_skeleton_in_the_table() {
        let skeleton = Arc::new(PlantSkeleton {
            bones: vec![
                PlantBone {
                    pivot: Vec3::ZERO,
                    length: 10.0,
                    axis: Vec3::Y,
                    face: Vec3::X,
                    parent: None,
                    level: 0,
                    phase: 0.0,
                    stiffness: 1.0,
                },
                PlantBone {
                    pivot: Vec3::Y * 5.0,
                    length: 3.0,
                    axis: Vec3::X,
                    face: Vec3::Y,
                    parent: Some(0),
                    level: 1,
                    phase: 0.4,
                    stiffness: 1.0,
                },
            ],
        });
        let mut meshes = PlantWindMeshes::default();
        for (i, detail) in [(3u32, 3u8), (4, 1)] {
            meshes.insert(
                Handle::from_index(i),
                PlantWindMesh {
                    skeleton: skeleton.clone(),
                    species: 0,
                    detail,
                },
            );
        }
        let mut table = PlantWindTable::default();
        table.update(Some(&meshes), None, 1.0, 0.98);
        assert!(table.rebuilt);
        let header: [u32; 4] = bytemuck::cast(table.words[0]);
        assert_eq!(header[0], 1);
        // Two mesh entries, two bones of two words each.
        assert_eq!(table.words.len(), header[2] as usize + 4);
        let (a, reach) = table.entry(3);
        let (b, _) = table.entry(4);
        assert!(a > 0 && b > 0 && a != b);
        assert!(reach > 0.0 && reach < 5.0, "{reach}");
        let ea: [u32; 4] = bytemuck::cast(table.words[header[1] as usize + a as usize - 1]);
        let eb: [u32; 4] = bytemuck::cast(table.words[header[1] as usize + b as usize - 1]);
        assert_eq!(ea[0], eb[0], "one skeleton, one set of bones");
        assert_eq!((ea[2], eb[2]), (3, 1));
        // Phases move on each frame without laying the table out again.
        let before = table.words[1];
        table.update(Some(&meshes), None, 1.1, 1.0);
        assert!(!table.rebuilt);
        assert_ne!(table.words[1], before);
        assert_eq!(table.entry(99), (0, 0.0));
    }
}

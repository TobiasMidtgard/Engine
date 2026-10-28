//! City lights made from the world's own lamps.
//!
//! Every static [`PointLight`] on a planet is registered here once, in the planet's own
//! coordinates, and bucketed on the terrain's cube quadtree with the lumens under each node.
//! The renderer asks for an emissive page (a small texture) for each drawn patch that has
//! lamps under it and splats them in with [`PlanetLamps::splat`]: each lamp lights a pool of
//! ground around it, conserving its light whether the pool covers hundreds of texels (seen
//! from a plane) or a fraction of one (from orbit). So a town the game builds glows on the
//! night side from orbit, the way the procedural population lights the rest of the planet.
//! Up close the renderer draws the lamps themselves and the pages fade out.
//!
//! Lights that keep moving (vehicles, spells, muzzle flashes) or are faint are left out: from
//! afar they don't count, and they would make pages stale every frame.

use std::collections::{HashMap, HashSet};

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::PointLight;
use engine_render::frame::ViewData;
use engine_world::{CellMember, StreamedCells};

use crate::Terrains;
use crate::cube::{NodeKey, SurfaceMap};

/// Quadtree level lamps are bucketed at (about 600 m nodes on Earth). Pages finer than this
/// read their leaf's lamps and keep those that fall inside.
pub const LEAF_LEVEL: u8 = 14;
/// Texels per side of an emissive page.
pub const PAGE_RES: u32 = 128;
/// Lamps fainter than this (lumens) aren't seen from afar: candles, glowing mushrooms.
pub const MIN_LUMENS: f32 = 200.0;
/// A lamp that moves farther than this (m) between registrations has moved.
const MOVE_M: f64 = 3.0;
/// A lamp that moved this many times is mobile and left out for good.
const MOBILE_AFTER: u32 = 3;
/// Planets whose sea level is farther than this from a lamp (m) don't own it.
const REACH_M: f64 = 50_000.0;

/// Share of a lamp's lumens that leave the ground toward a viewer above, as luminance
/// (cd/m²) × area × π: half of it falls on the ground, which reflects ~20 % of it (asphalt,
/// soil); 5 % goes up directly (a full cut-off streetlight sends 0–3 %, an old globe ~25 %),
/// spread over the upper hemisphere.
const SEEN_FROM_ABOVE: f32 = 0.5 * 0.95 * 0.2 + 0.05 / std::f32::consts::TAU;

/// One registered lamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lamp {
    /// Terrain-local position, meters.
    pub local: DVec3,
    /// Face and face coordinates of its position.
    pub face: u8,
    pub ab: [f64; 2],
    /// Lumens.
    pub lumens: f32,
    /// Linear RGB, normalized so its luminance is 1.
    pub color: Vec3,
    /// Radius of the pool of ground it lights, meters.
    pub pool_m: f32,
}

impl Lamp {
    fn of(map: &SurfaceMap, local: DVec3, light: &PointLight) -> Option<Self> {
        let (face, a, b) = map.face_coords(local)?;
        let c = Vec3::from_array(light.color).max(Vec3::ZERO);
        let luma = c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        Some(Self {
            local,
            face,
            ab: [a, b],
            lumens: light.intensity_lm,
            color: if luma > 1e-4 { c / luma } else { Vec3::ONE },
            // Most of a lamp's light lands within about half its range.
            pool_m: (light.range * 0.4).clamp(2.0, 60.0),
        })
    }
}

/// A registered lamp: its entity's bits while the entity lives, or a kept id once a streamed
/// cell unloaded it ([`KEPT`] set).
pub type LampId = u64;

/// Set on the ids of lamps kept after their cell unloaded.
const KEPT: LampId = 1 << 63;

fn live(e: Entity) -> LampId {
    e.to_bits() & !KEPT
}

/// Lumens under a quadtree node, and a version bumped whenever a lamp under it changes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeLamps {
    pub lumens: f32,
    pub count: u32,
    pub version: u64,
}

/// The lamps on one planet.
#[derive(Debug, Default)]
pub struct PlanetLamps {
    lamps: HashMap<LampId, (Lamp, NodeKey)>,
    leaves: HashMap<NodeKey, Vec<LampId>>,
    /// Every node from the roots down to [`LEAF_LEVEL`] with lamps under it.
    nodes: HashMap<NodeKey, NodeLamps>,
    next_version: u64,
}

impl PlanetLamps {
    pub fn len(&self) -> usize {
        self.lamps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lamps.is_empty()
    }

    fn touch(&mut self, leaf: NodeKey, lumens: f32, count: i32) {
        self.next_version += 1;
        let version = self.next_version;
        let mut key = Some(leaf);
        while let Some(k) = key {
            let n = self.nodes.entry(k).or_default();
            n.lumens = (n.lumens + lumens).max(0.0);
            n.count = (n.count as i32 + count).max(0) as u32;
            n.version = version;
            if n.count == 0 {
                self.nodes.remove(&k);
            }
            key = k.parent();
        }
    }

    pub fn insert(&mut self, map: &SurfaceMap, id: LampId, lamp: Lamp) {
        self.remove(id);
        let Some(leaf) = map.node_at(lamp.local, LEAF_LEVEL) else {
            return;
        };
        self.lamps.insert(id, (lamp, leaf));
        self.leaves.entry(leaf).or_default().push(id);
        self.touch(leaf, lamp.lumens, 1);
    }

    /// Give a lamp a new id; its light stays where it is (no page goes stale).
    fn rekey(&mut self, from: LampId, to: LampId) {
        let Some((lamp, leaf)) = self.lamps.remove(&from) else {
            return;
        };
        self.lamps.insert(to, (lamp, leaf));
        if let Some(list) = self.leaves.get_mut(&leaf) {
            for id in list.iter_mut().filter(|id| **id == from) {
                *id = to;
            }
        }
    }

    pub fn remove(&mut self, id: LampId) -> Option<Lamp> {
        let (lamp, leaf) = self.lamps.remove(&id)?;
        if let Some(list) = self.leaves.get_mut(&leaf) {
            list.retain(|e| *e != id);
            if list.is_empty() {
                self.leaves.remove(&leaf);
            }
        }
        self.touch(leaf, -lamp.lumens, -1);
        Some(lamp)
    }

    pub fn get(&self, id: LampId) -> Option<&Lamp> {
        self.lamps.get(&id).map(|(l, _)| l)
    }

    /// The lamps under `key` (at most [`LEAF_LEVEL`] deep: finer keys read their leaf).
    fn bucket(&self, key: NodeKey) -> NodeKey {
        let mut k = key;
        while k.level > LEAF_LEVEL {
            k = k.parent().expect("level above 0");
        }
        k
    }

    /// Lumens, count and version under `key` (finer than the leaves: its leaf's).
    pub fn under(&self, key: NodeKey) -> Option<NodeLamps> {
        self.nodes.get(&self.bucket(key)).copied()
    }

    /// The emissive page of `key`: `PAGE_RES`² texels of linear RGB luminance (cd/m²) seen
    /// from above, row-major from the patch's lower face corner (`+a` along a row, `+b` down
    /// the rows).
    pub fn splat(&self, map: &SurfaceMap, key: NodeKey) -> Vec<[f32; 3]> {
        let res = PAGE_RES as usize;
        let mut page = vec![[0.0f32; 3]; res * res];
        let bucket = self.bucket(key);
        let (corner, size) = key.face_rect();
        // Texel edge along the surface (the tangent warp keeps it within ~1.4× of this
        // across a face).
        let texel_m = (map.node_size(key.level) / PAGE_RES as f64) as f32;
        let texel_area = texel_m * texel_m;
        let shift = LEAF_LEVEL - bucket.level;
        for (leaf, list) in &self.leaves {
            if leaf.face != bucket.face
                || leaf.x >> shift != bucket.x
                || leaf.y >> shift != bucket.y
            {
                continue;
            }
            for e in list {
                let Some((lamp, _)) = self.lamps.get(e) else {
                    continue;
                };
                if lamp.face != key.face {
                    continue;
                }
                let px = ((lamp.ab[0] - corner.x) / size * res as f64) as f32 - 0.5;
                let py = ((lamp.ab[1] - corner.y) / size * res as f64) as f32 - 0.5;
                let r = lamp.pool_m / texel_m;
                // Each texel's luminance from its share `w` of the lamp's light.
                let light = lamp.color * (lamp.lumens * SEEN_FROM_ABOVE / std::f32::consts::PI);
                let mut add = |x: i32, y: i32, w: f32| {
                    if x >= 0 && y >= 0 && (x as usize) < res && (y as usize) < res && w > 0.0 {
                        let t = &mut page[y as usize * res + x as usize];
                        let l = light * (w / texel_area);
                        t[0] += l.x;
                        t[1] += l.y;
                        t[2] += l.z;
                    }
                };
                if r <= 1.0 {
                    // Smaller than a texel: a tent over the four nearest.
                    let (x0, y0) = (px.floor(), py.floor());
                    let (fx, fy) = (px - x0, py - y0);
                    let (x0, y0) = (x0 as i32, y0 as i32);
                    add(x0, y0, (1.0 - fx) * (1.0 - fy));
                    add(x0 + 1, y0, fx * (1.0 - fy));
                    add(x0, y0 + 1, (1.0 - fx) * fy);
                    add(x0 + 1, y0 + 1, fx * fy);
                    continue;
                }
                // A pool brightest under the lamp, (1 − d²/r²)², normalized over its texels.
                let reach = r.ceil() as i32;
                let (cx, cy) = (px.round() as i32, py.round() as i32);
                if cx + reach < 0
                    || cy + reach < 0
                    || cx - reach >= res as i32
                    || cy - reach >= res as i32
                {
                    continue;
                }
                let weight = |x: i32, y: i32| {
                    let d2 = ((x as f32 - px).powi(2) + (y as f32 - py).powi(2)) / (r * r);
                    if d2 < 1.0 { (1.0 - d2).powi(2) } else { 0.0 }
                };
                // Normalized over the whole pool (also the part off this page, which the
                // neighbor's page holds), analytically: ∫(1 − d²/r²)² dA = πr²/3.
                let total = std::f32::consts::PI * r * r / 3.0;
                for y in (cy - reach).max(0)..=(cy + reach).min(res as i32 - 1) {
                    for x in (cx - reach).max(0)..=(cx + reach).min(res as i32 - 1) {
                        add(x, y, weight(x, y) / total);
                    }
                }
            }
        }
        page
    }
}

/// Lamps of a streamed cell, by its key.
#[derive(Debug, Default)]
struct CellLamps {
    /// Registered lamps the cell spawned, by entity.
    live: HashSet<Entity>,
    /// Lamps kept after the cell unloaded them: (planet, id).
    kept: Vec<(Entity, LampId)>,
}

/// The lamps of every planet, kept in step with the world's `PointLight`s.
///
/// A lamp a streamed cell spawned outlives the cell's unloading: a town seen from a plane or
/// from orbit is long out of load reach, and its lamps are static. Its light goes when the
/// cell is retired, or is replaced when the cell loads again and spawns its lamps anew.
#[derive(Resource, Debug, Default)]
pub struct CityLights {
    pub planets: HashMap<Entity, PlanetLamps>,
    /// Each registered lamp entity: its planet, how often it moved, its cell's key.
    owner: HashMap<Entity, (Entity, u32, Option<String>)>,
    cells: HashMap<String, CellLamps>,
    /// Lamps left out for moving.
    mobile: HashSet<Entity>,
    /// Terrains seen last frame (a new one rescans every light).
    terrains_seen: usize,
    next_kept: LampId,
}

impl CityLights {
    /// Lamps registered on all planets (kept ones included).
    pub fn lamp_count(&self) -> usize {
        self.planets.values().map(PlanetLamps::len).sum()
    }

    /// Lumens registered on all planets.
    pub fn lumens(&self) -> f32 {
        self.planets
            .values()
            .flat_map(|p| p.lamps.values())
            .map(|(l, _)| l.lumens)
            .sum()
    }

    fn forget(&mut self, entity: Entity) {
        if let Some((planet, _, cell)) = self.owner.remove(&entity) {
            if let Some(p) = self.planets.get_mut(&planet) {
                p.remove(live(entity));
            }
            if let Some(c) = cell.and_then(|k| self.cells.get_mut(&k)) {
                c.live.remove(&entity);
            }
        }
    }

    /// The lamp's entity is gone: a streamed cell's lamp is kept, any other forgotten.
    fn despawned(&mut self, entity: Entity) {
        let Some((planet, _, Some(cell))) = self.owner.remove(&entity) else {
            self.forget(entity);
            return;
        };
        self.next_kept += 1;
        let id = KEPT | self.next_kept;
        if let Some(p) = self.planets.get_mut(&planet) {
            p.rekey(live(entity), id);
        }
        let c = self.cells.entry(cell).or_default();
        c.live.remove(&entity);
        c.kept.push((planet, id));
    }

    /// Drop the lamps kept for `cell`.
    fn drop_kept(&mut self, cell: &str) {
        if let Some(c) = self.cells.get_mut(cell) {
            for (planet, id) in c.kept.drain(..) {
                if let Some(p) = self.planets.get_mut(&planet) {
                    p.remove(id);
                }
            }
        }
    }
}

/// The streamed cell an entity belongs to (its own or its root's `CellMember`).
fn cell_of(
    mut e: Entity,
    members: &Query<(Option<&CellMember>, Option<&ChildOf>)>,
    cells: Option<&StreamedCells>,
) -> Option<String> {
    for _ in 0..64 {
        let (member, parent) = members.get(e).ok()?;
        if let Some(m) = member {
            return cells?.get(m.0).map(|c| c.key.clone());
        }
        e = parent?.parent();
    }
    None
}

/// Registers new, changed and removed lamps (after transforms and terrains are up to date).
#[allow(clippy::too_many_arguments)]
pub(crate) fn register_lamps(
    mut city: ResMut<CityLights>,
    terrains: Res<Terrains>,
    streamed: Option<Res<StreamedCells>>,
    lights: Query<(Entity, Ref<PointLight>, Ref<GlobalTransform>)>,
    members: Query<(Option<&CellMember>, Option<&ChildOf>)>,
    mut removed: RemovedComponents<PointLight>,
) {
    let city = &mut *city;
    for e in removed.read() {
        city.mobile.remove(&e);
        city.despawned(e);
    }
    // A retired cell (its scene unloaded or replaced) takes its kept lamps with it.
    if let Some(streamed) = &streamed {
        let gone: Vec<String> = city
            .cells
            .iter()
            .filter(|(_, c)| !c.kept.is_empty())
            .filter(|(k, _)| {
                streamed
                    .find(k)
                    .and_then(|id| streamed.get(id))
                    .is_none_or(|c| c.retired)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in gone {
            city.drop_kept(&k);
        }
    }
    city.cells
        .retain(|_, c| !c.live.is_empty() || !c.kept.is_empty());
    city.planets
        .retain(|planet, _| terrains.entries.contains_key(planet));
    let rescan = terrains.entries.len() != city.terrains_seen;
    city.terrains_seen = terrains.entries.len();
    for (e, light, gt) in &lights {
        if !(rescan || light.is_changed() || gt.is_changed()) || city.mobile.contains(&e) {
            continue;
        }
        if light.intensity_lm < MIN_LUMENS {
            city.forget(e);
            continue;
        }
        let p = gt.translation();
        let Some((planet, t)) = terrains
            .entries
            .iter()
            .filter(|(_, t)| t.source.map.is_sphere() && t.altitude_hint(p).abs() < REACH_M)
            .min_by(|a, b| {
                a.1.altitude_hint(p)
                    .abs()
                    .total_cmp(&b.1.altitude_hint(p).abs())
            })
        else {
            city.forget(e);
            continue;
        };
        let local = t.to_local(p);
        let Some(lamp) = Lamp::of(&t.source.map, local, &light) else {
            continue;
        };
        let mut moves = 0;
        let cell;
        if let Some((was_on, was_moves, was_cell)) = city.owner.get(&e).cloned() {
            moves = was_moves;
            cell = was_cell;
            if let Some(old) = city
                .planets
                .get(&was_on)
                .and_then(|p| p.get(live(e)))
                .copied()
            {
                let moved = was_on != *planet || (old.local - local).length() > MOVE_M;
                // (A world shift changes the transforms, not where the lamp stands, and a
                // flicker isn't worth new pages.)
                let dimmed = (old.lumens - lamp.lumens).abs() > 0.25 * old.lumens
                    || old.color.distance(lamp.color) > 0.1
                    || (old.pool_m - lamp.pool_m).abs() > 0.25 * old.pool_m;
                if moved {
                    moves += 1;
                    if moves >= MOBILE_AFTER {
                        city.forget(e);
                        city.mobile.insert(e);
                        continue;
                    }
                } else if !dimmed {
                    continue;
                }
            }
        } else {
            cell = cell_of(e, &members, streamed.as_deref());
            // The cell loaded again: what it spawns now replaces what was kept of it.
            if let Some(k) = &cell {
                city.drop_kept(k);
            }
        }
        city.forget(e);
        let planet = *planet;
        city.planets
            .entry(planet)
            .or_default()
            .insert(&t.source.map, live(e), lamp);
        if let Some(k) = &cell {
            city.cells.entry(k.clone()).or_default().live.insert(e);
        }
        city.owner.insert(e, (planet, moves, cell));
    }
}

/// Pages finer than this level aren't made: finer patches draw with their ancestor's page
/// (texels about a meter across on Earth; the renderer lights the ground itself closer in).
pub const PAGE_LEVEL_MAX: u8 = LEAF_LEVEL + 2;
/// Emissive pages kept on the GPU.
const PAGE_LAYERS: u32 = 96;
/// Most pages splatted in one frame (the rest draw with an older or coarser page meanwhile).
const SPLATS_PER_FRAME: usize = 6;
/// What a patch without lamps draws with.
pub(crate) const NO_PAGE: [f32; 4] = [0.0, 0.0, 1.0, -1.0];

struct Page {
    layer: u32,
    version: u64,
    used: u64,
}

/// The emissive pages on the GPU: one texture-array layer per page, LRU-recycled.
pub(crate) struct NightPages {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pages: HashMap<(Entity, NodeKey), Page>,
    free: Vec<u32>,
    frame: u64,
    splats_left: usize,
}

impl NightPages {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain night pages"),
            size: wgpu::Extent3d {
                width: PAGE_RES,
                height: PAGE_RES,
                depth_or_array_layers: PAGE_LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            texture,
            view,
            pages: HashMap::new(),
            free: (0..PAGE_LAYERS).rev().collect(),
            frame: 0,
            splats_left: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.splats_left = SPLATS_PER_FRAME;
    }

    /// Pages resident (for diagnostics).
    pub fn resident(&self) -> usize {
        self.pages.len()
    }

    /// What a drawn patch `key` of `planet` samples its night glow from: the page's face
    /// corner (a, b), edge length in face coordinates, and `layer · 8 + face` (−1: none).
    /// Makes or refreshes its page when the frame's budget allows; meanwhile an older page or
    /// a coarser one stands in.
    pub fn lookup(
        &mut self,
        queue: &wgpu::Queue,
        planet: Entity,
        lamps: &PlanetLamps,
        map: &SurfaceMap,
        key: NodeKey,
    ) -> [f32; 4] {
        let mut k = key;
        while k.level > PAGE_LEVEL_MAX {
            k = k.parent().expect("level above 0");
        }
        let Some(under) = lamps.under(k) else {
            return NO_PAGE;
        };
        let frame = self.frame;
        if let Some(p) = self.pages.get_mut(&(planet, k)) {
            p.used = frame;
            if p.version == under.version {
                return pack(k, p.layer);
            }
        }
        if self.splats_left > 0
            && let Some(layer) = self.alloc(planet, k)
        {
            self.splats_left -= 1;
            self.upload(queue, layer, &lamps.splat(map, k));
            self.pages.insert(
                (planet, k),
                Page {
                    layer,
                    version: under.version,
                    used: frame,
                },
            );
            return pack(k, layer);
        }
        // Stale, or an ancestor's.
        let mut a = Some(k);
        while let Some(ak) = a {
            if let Some(p) = self.pages.get_mut(&(planet, ak)) {
                p.used = frame;
                return pack(ak, p.layer);
            }
            a = ak.parent();
        }
        NO_PAGE
    }

    /// A layer for a new page of `k`: its old one, a free one, or the least recently used
    /// page's (not one used this frame).
    fn alloc(&mut self, planet: Entity, k: NodeKey) -> Option<u32> {
        if let Some(p) = self.pages.remove(&(planet, k)) {
            return Some(p.layer);
        }
        if let Some(layer) = self.free.pop() {
            return Some(layer);
        }
        let oldest = self
            .pages
            .iter()
            .filter(|(_, p)| p.used < self.frame)
            .min_by_key(|(_, p)| p.used)
            .map(|(key, _)| *key)?;
        self.pages.remove(&oldest).map(|p| p.layer)
    }

    /// Forget the pages of planets that are gone.
    pub fn retain_planets(&mut self, keep: impl Fn(&Entity) -> bool) {
        let free = &mut self.free;
        self.pages.retain(|(planet, _), p| {
            let kept = keep(planet);
            if !kept {
                free.push(p.layer);
            }
            kept
        });
    }

    fn upload(&self, queue: &wgpu::Queue, layer: u32, page: &[[f32; 3]]) {
        let mut bytes = Vec::with_capacity(page.len() * 8);
        for t in page {
            for c in [t[0], t[1], t[2], 1.0] {
                bytes.extend_from_slice(&crate::render::f16_bits(c.min(60_000.0)).to_le_bytes());
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PAGE_RES * 8),
                rows_per_image: Some(PAGE_RES),
            },
            wgpu::Extent3d {
                width: PAGE_RES,
                height: PAGE_RES,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Distance from the camera (m) beyond which lamps glow from their pages: where the renderer
/// stops drawing lights itself (it draws the nearest `max_lights` in view), but no farther
/// than [`HANDOFF_MAX_M`], where its pools of light shrink toward a pixel. 0 with no lamps.
pub(crate) fn handoff_distance(world: &mut World, view: &ViewData, max_lights: u32) -> f32 {
    if !world
        .get_resource::<CityLights>()
        .is_some_and(|c| c.planets.values().any(|p| !p.is_empty()))
    {
        return 0.0;
    }
    let mut q = world.query::<(&PointLight, &GlobalTransform)>();
    let mut d2: Vec<f32> = q
        .iter(world)
        .filter_map(|(l, gt)| {
            let rel = (gt.translation() - view.origin).as_vec3();
            view.frustum
                .intersects_sphere(rel, l.range)
                .then(|| rel.length_squared())
        })
        .collect();
    let n = max_lights as usize;
    if d2.len() <= n {
        return HANDOFF_MAX_M;
    }
    let (_, nth, _) = d2.select_nth_unstable_by(n, |a, b| a.total_cmp(b));
    nth.sqrt().min(HANDOFF_MAX_M)
}

/// Farthest the renderer's own lights are left to light the ground (m).
const HANDOFF_MAX_M: f32 = 2_000.0;

fn pack(k: NodeKey, layer: u32) -> [f32; 4] {
    let (corner, size) = k.face_rect();
    [
        corner.x as f32,
        corner.y as f32,
        size as f32,
        (layer * 8 + k.face as u32) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cube::TerrainDomain;

    const R: f64 = 6_371_000.0;

    fn lamp(map: &SurfaceMap, local: DVec3, lumens: f32, range: f32) -> Lamp {
        Lamp::of(
            map,
            local,
            &PointLight {
                color: [1.0, 1.0, 1.0],
                intensity_lm: lumens,
                range,
                radius: 0.1,
            },
        )
        .unwrap()
    }

    fn sum(page: &[[f32; 3]]) -> f32 {
        page.iter().map(|t| t[1]).sum()
    }

    #[test]
    fn a_lamps_light_is_the_same_on_every_page_that_holds_it() {
        let map = SurfaceMap::new(TerrainDomain::Sphere, R);
        let mut p = PlanetLamps::default();
        // In the middle of the finest page, so its whole pool lands on every page.
        let fine = map
            .node_at(DVec3::new(0.3, 1.0, 0.2).normalize() * R, 17)
            .unwrap();
        let (corner, size) = fine.face_rect();
        let (at, _, _) = map.base(fine.face, corner.x + size * 0.5, corner.y + size * 0.5);
        p.insert(
            &map,
            live(Entity::from_raw_u32(1).unwrap()),
            lamp(&map, at, 20_000.0, 40.0),
        );
        let mut seen = Vec::new();
        for level in [6u8, 10, 14, 17] {
            let key = map.node_at(at, level).unwrap();
            let texel = map.node_size(level) / PAGE_RES as f64;
            // Luminance × area × π = the lumens seen from above.
            let lm = sum(&p.splat(&map, key)) as f64 * texel * texel * std::f64::consts::PI;
            seen.push(lm);
        }
        let want = 20_000.0 * SEEN_FROM_ABOVE as f64;
        for lm in &seen {
            assert!((lm / want - 1.0).abs() < 0.15, "{seen:?} vs {want}");
        }
    }

    #[test]
    fn lumens_add_up_the_tree_and_go_when_the_lamp_does() {
        let map = SurfaceMap::new(TerrainDomain::Sphere, R);
        let mut p = PlanetLamps::default();
        let at = DVec3::new(-0.2, 0.1, 1.0).normalize() * R;
        let near = at + DVec3::X * 30.0;
        p.insert(
            &map,
            live(Entity::from_raw_u32(1).unwrap()),
            lamp(&map, at, 1000.0, 20.0),
        );
        p.insert(
            &map,
            live(Entity::from_raw_u32(2).unwrap()),
            lamp(&map, near, 3000.0, 20.0),
        );
        let root = map.node_at(at, 0).unwrap();
        let under = p.under(root).unwrap();
        assert_eq!(under.count, 2);
        assert!((under.lumens - 4000.0).abs() < 1e-3);
        let v = under.version;
        p.remove(live(Entity::from_raw_u32(2).unwrap()));
        let under = p.under(root).unwrap();
        assert_eq!(under.count, 1);
        assert!(under.version > v);
        p.remove(live(Entity::from_raw_u32(1).unwrap()));
        assert!(p.under(root).is_none());
        assert!(p.is_empty());
    }

    #[test]
    fn pages_hold_only_their_own_lamps() {
        let map = SurfaceMap::new(TerrainDomain::Sphere, R);
        let mut p = PlanetLamps::default();
        let at = DVec3::new(0.1, 1.0, -0.3).normalize() * R;
        p.insert(
            &map,
            live(Entity::from_raw_u32(1).unwrap()),
            lamp(&map, at, 20_000.0, 10.0),
        );
        let key = map.node_at(at, 16).unwrap();
        assert!(sum(&p.splat(&map, key)) > 0.0);
        // A neighbor two patches over sees none of it.
        let other = NodeKey {
            x: key.x + 2,
            ..key
        };
        assert_eq!(sum(&p.splat(&map, other)), 0.0);
    }
}

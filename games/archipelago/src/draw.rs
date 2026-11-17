//! Turning models into draws: every building model is one instanced row set per material
//! (rewritten only when something is built), every ship, airship and cart a few entities
//! that move together.

use std::collections::HashMap;
use std::sync::Arc;

use engine::prelude::*;
use engine::render::{InstanceRow, Mesh, MeshInstances};

use crate::art::{Mat, Mats, Parts, make_mats};
use crate::globe::{self, Frame, Local};
use crate::grid;
use crate::looks::{MODELS, ModelKey, Piece, build_model, mill_sails};
use crate::sim::Building;
use crate::vessels::{Craft, airship_props, cart, craft_model};

/// A model ready to draw: one mesh per material.
pub type Model = Arc<Vec<(Handle<Mesh>, Mat)>>;

/// Every model the game draws.
#[derive(Resource, Clone)]
pub struct Art {
    pub mats: Mats,
    pub models: HashMap<ModelKey, Model>,
    pub crafts: HashMap<Craft, Model>,
    pub sails: Model,
    pub props: Model,
    pub paddles: Model,
    pub cart: Model,
    /// Triangles per model, for the budget test and the perf panel.
    pub triangles: HashMap<String, usize>,
}

fn upload(world: &mut World, parts: Parts) -> Model {
    let mut out = Vec::new();
    for (m, mesh) in parts.meshes {
        if mesh.triangle_count() == 0 {
            continue;
        }
        let h = world.resource_mut::<Assets<Mesh>>().add(mesh);
        out.push((h, m));
    }
    Arc::new(out)
}

/// Bake the materials and build every model (on all cores).
pub fn make_art(world: &mut World) -> Art {
    let started = std::time::Instant::now();
    let mats = make_mats(world);
    let built: Vec<Parts> = engine_ui::demo::art::par_map(MODELS.len(), |i| build_model(MODELS[i]));
    let crafts: Vec<Parts> =
        engine_ui::demo::art::par_map(Craft::ALL.len(), |i| craft_model(Craft::ALL[i]));
    let mut triangles = HashMap::new();
    let mut models = HashMap::new();
    for (key, parts) in MODELS.iter().zip(built) {
        triangles.insert(key.to_string(), parts.triangles());
        models.insert(*key, upload(world, parts));
    }
    let mut craft_models = HashMap::new();
    for (c, parts) in Craft::ALL.iter().zip(crafts) {
        triangles.insert(format!("{c:?}"), parts.triangles());
        craft_models.insert(*c, upload(world, parts));
    }
    let sails = upload(world, mill_sails());
    let props = upload(world, airship_props());
    let paddles = upload(world, crate::vessels::paddle_wheel());
    let cart = upload(world, cart());
    engine::core::log::info!(
        "archipelago art: {} models in {:.2} s",
        models.len() + craft_models.len(),
        started.elapsed().as_secs_f32()
    );
    Art {
        mats,
        models,
        crafts: craft_models,
        sails,
        props,
        paddles,
        cart,
        triangles,
    }
}

/// What a building is drawn with, in its own space.
#[derive(Component, Clone, Debug)]
pub struct Look(pub Arc<Vec<Piece>>);

/// One copy of a model, in map space.
#[derive(Clone, Copy, Debug)]
pub struct MapRow {
    pub at: DVec3,
    /// Turn about up, as on the map.
    pub yaw: f32,
    pub scale: f32,
    pub tint: [f32; 3],
}

/// Instanced copies kept per ground tile: one entity per (key, tile), each standing flat on
/// the planet at its tile's middle with its rows placed exactly where the round ground is.
pub struct Tiled<K> {
    pub anchors: HashMap<(K, IVec2), Entity>,
}

impl<K> Default for Tiled<K> {
    fn default() -> Self {
        Self {
            anchors: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + std::hash::Hash> Tiled<K> {
    /// Replace every set's rows by `groups`; sets that got none are emptied. `renderer` makes
    /// the mesh and material of a key the first time a tile needs it. Returns the row count.
    pub fn set(
        &mut self,
        world: &mut World,
        groups: HashMap<K, Vec<MapRow>>,
        mut renderer: impl FnMut(K) -> Option<MeshRenderer>,
    ) -> usize {
        let frame = *world.resource::<Frame>();
        let mut locals: HashMap<IVec2, Local> = HashMap::new();
        let mut sets: HashMap<(K, IVec2), Vec<InstanceRow>> = HashMap::new();
        let mut count = 0;
        for (key, rows) in groups {
            for r in rows {
                let tile = globe::tile_of(r.at.x, r.at.z);
                let l = locals.entry(tile).or_insert_with(|| Local::tile(tile));
                sets.entry((key, tile)).or_default().push(InstanceRow {
                    position: l.put(r.at),
                    yaw: r.yaw,
                    scale: r.scale,
                    tint: r.tint,
                });
                count += 1;
            }
        }
        for (key, e) in &self.anchors {
            if !sets.contains_key(key)
                && let Some(mut inst) = world.get_mut::<MeshInstances>(*e)
                && !inst.rows.is_empty()
            {
                inst.rows = Arc::from(Vec::new());
            }
        }
        for (key, list) in sets {
            if let Some(&e) = self.anchors.get(&key) {
                if let Some(mut inst) = world.get_mut::<MeshInstances>(e) {
                    *inst = MeshInstances::new(list);
                }
                continue;
            }
            let Some(r) = renderer(key.0) else {
                continue;
            };
            let e = world
                .spawn((frame.local(&locals[&key.1]), r, MeshInstances::new(list)))
                .id();
            self.anchors.insert(key, e);
        }
        count
    }
}

/// The renderer of one material of a model.
pub fn model_renderer(art: &Art, key: (ModelKey, Mat), shadows: bool) -> Option<MeshRenderer> {
    let (mesh, mat) = art.models.get(key.0)?.iter().find(|(_, m)| *m == key.1)?;
    let mut r = MeshRenderer::new(*mesh, art.mats.get(*mat));
    r.cast_shadows = shadows && mat.casts_shadows();
    Some(r)
}

/// The buildings' instanced sets, one per (model, material) and ground tile.
#[derive(Resource, Default)]
pub struct Drawn {
    pub sets: Tiled<(ModelKey, Mat)>,
    pub dirty: bool,
    /// Rows drawn last rebuild, for the perf panel.
    pub rows: usize,
}

/// Rewrite the instanced rows when buildings changed.
pub fn redraw_buildings(world: &mut World) {
    if !world.resource::<Drawn>().dirty {
        return;
    }
    let Some(art) = world.get_resource::<Art>().cloned() else {
        return;
    };
    let mut groups: HashMap<(ModelKey, Mat), Vec<MapRow>> = HashMap::new();
    let mut q = world.query::<(&Building, &Look)>();
    for (b, look) in q.iter(world) {
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let base = DVec3::new(c.x, b.base_y, c.y);
        let yaw = grid::yaw(b.rot);
        let rot = Quat::from_rotation_y(yaw);
        for piece in look.0.iter() {
            let Some(model) = art.models.get(piece.model) else {
                continue;
            };
            let row = MapRow {
                at: base + (rot * piece.pos).as_dvec3(),
                yaw: yaw + piece.yaw,
                scale: piece.scale,
                tint: piece.tint,
            };
            for (_, mat) in model.iter() {
                groups.entry((piece.model, *mat)).or_default().push(row);
            }
        }
    }
    let mut drawn = std::mem::take(&mut *world.resource_mut::<Drawn>());
    drawn.rows = drawn
        .sets
        .set(world, groups, |key| model_renderer(&art, key, true));
    drawn.dirty = false;
    *world.resource_mut::<Drawn>() = drawn;
}

/// Entities that draw one moving thing; all share its transform.
#[derive(Component, Clone, Debug, Default)]
pub struct Body {
    pub parts: Vec<Entity>,
}

/// Spawn a model's parts at `at`, returning their entities.
pub fn spawn_model(
    world: &mut World,
    art: &Art,
    model: &Model,
    at: Transform,
    shadows: bool,
) -> Vec<Entity> {
    model
        .iter()
        .map(|(mesh, mat)| {
            let mut r = MeshRenderer::new(*mesh, art.mats.get(*mat));
            r.cast_shadows = shadows && mat.casts_shadows();
            world.spawn((at, r)).id()
        })
        .collect()
}

/// Copy each [`Body`]'s transform to its parts.
pub fn follow_bodies(
    bodies: Query<(&Transform, &Body), Changed<Transform>>,
    mut parts: Query<&mut Transform, Without<Body>>,
) {
    for (t, body) in &bodies {
        for &e in &body.parts {
            if let Ok(mut pt) = parts.get_mut(e) {
                *pt = *t;
            }
        }
    }
}

/// A part that turns about its own axis (windmill sails, propellers).
#[derive(Component, Clone, Debug)]
pub struct Spin {
    /// The transform it turns from: in its body's space, or in the map's at `at`.
    pub base: Transform,
    /// The map point `base` is measured from, for a part of no body (a windmill's sails).
    pub at: DVec3,
    /// Local axis and speed, rad/s.
    pub axis: Vec3,
    pub rate: f32,
    pub angle: f32,
    /// Moves along with this body, if any (its transform times `base`).
    pub on: Option<Entity>,
    /// Turns only while this building works (its producer's efficiency).
    pub driven_by: Option<Entity>,
}

#[allow(clippy::type_complexity)]
pub fn spin_parts(
    time: Res<Time>,
    econ: Res<crate::sim::Economy>,
    frame: Res<Frame>,
    mut q: Query<(&mut Spin, &mut Transform), Without<Body>>,
    bodies: Query<&Transform, With<Body>>,
    producers: Query<&crate::sim::Producer>,
    vessels: Query<&crate::fleet::Vessel>,
) {
    let dt = time.delta_secs() * econ.speed.min(2.0);
    for (mut s, mut t) in &mut q {
        let mut k = s.driven_by.map_or(1.0, |e| {
            producers.get(e).map_or(0.0, |p| p.efficiency.max(0.15))
        });
        // Propellers and paddle wheels turn with the way the craft is making.
        if let Some(v) = s.on.and_then(|e| vessels.get(e).ok()) {
            k *= (v.speed / v.craft.speed()).clamp(0.08, 1.0);
        }
        s.angle = (s.angle + s.rate * dt * k) % std::f32::consts::TAU;
        let local = Transform {
            rotation: s.base.rotation * Quat::from_axis_angle(s.axis, s.angle),
            ..s.base
        };
        let stand;
        let parent = match s.on.and_then(|e| bodies.get(e).ok()) {
            Some(parent) => parent,
            None => {
                stand = frame.stand(s.at, 0.0);
                &stand
            }
        };
        *t = Transform {
            translation: parent.translation
                + (parent.rotation * local.translation.as_vec3()).as_dvec3(),
            rotation: parent.rotation * local.rotation,
            scale: local.scale,
        };
    }
}

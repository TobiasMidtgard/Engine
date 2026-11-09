//! Turning models into draws: every building model is one instanced row set per material
//! (rewritten only when something is built), every ship, airship and cart a few entities
//! that move together.

use std::collections::HashMap;
use std::sync::Arc;

use engine::prelude::*;
use engine::render::{InstanceRow, Mesh, MeshInstances};

use crate::art::{Mat, Mats, Parts, make_mats};
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

/// The instanced row sets, one entity per (model, material).
#[derive(Resource, Default)]
pub struct Drawn {
    pub anchors: HashMap<(ModelKey, Mat), Entity>,
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
    let mut groups: HashMap<(ModelKey, Mat), Vec<InstanceRow>> = HashMap::new();
    let mut q = world.query::<(&Building, &Look)>();
    for (b, look) in q.iter(world) {
        let c = grid::footprint_center(b.kind, b.origin, b.rot);
        let base = Vec3::new(c.x as f32, b.base_y as f32, c.y as f32);
        let yaw = grid::yaw(b.rot);
        let rot = Quat::from_rotation_y(yaw);
        for piece in look.0.iter() {
            let Some(model) = art.models.get(piece.model) else {
                continue;
            };
            let row = InstanceRow {
                position: base + rot * piece.pos,
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
    let mut rows = 0;
    // Empty out sets that lost every row.
    for (key, e) in &drawn.anchors {
        if !groups.contains_key(key)
            && let Some(mut inst) = world.get_mut::<MeshInstances>(*e)
        {
            inst.rows = Arc::from(Vec::new());
        }
    }
    for (key, list) in groups {
        rows += list.len();
        if let Some(&e) = drawn.anchors.get(&key) {
            if let Some(mut inst) = world.get_mut::<MeshInstances>(e) {
                *inst = MeshInstances::new(list);
            }
            continue;
        }
        let model = &art.models[key.0];
        let Some((mesh, mat)) = model.iter().find(|(_, m)| *m == key.1) else {
            continue;
        };
        let mut r = MeshRenderer::new(*mesh, art.mats.get(*mat));
        r.cast_shadows = mat.casts_shadows();
        let e = world
            .spawn((
                Name::new(format!("{} {:?}", key.0, key.1)),
                Transform::default(),
                r,
                MeshInstances::new(list),
            ))
            .id();
        drawn.anchors.insert(key, e);
    }
    drawn.rows = rows;
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
    /// The transform it turns from.
    pub base: Transform,
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
        *t = match s.on.and_then(|e| bodies.get(e).ok()) {
            Some(parent) => Transform {
                translation: parent.translation
                    + (parent.rotation * local.translation.as_vec3()).as_dvec3(),
                rotation: parent.rotation * local.rotation,
                scale: local.scale,
            },
            None => local,
        };
    }
}

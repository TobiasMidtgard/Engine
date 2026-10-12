//! glTF 2.0 models: `.gltf` (JSON + `.bin` + image files, or embedded `data:` URIs) and `.glb`.
//!
//! Loading `models/tree.glb` adds these assets (keys are what `MeshRef::Asset`,
//! `MaterialRef::Asset` and [`AssetServer::load`](engine_assets::AssetServer::load) take):
//!
//! | Key | Asset |
//! |---|---|
//! | `models/tree.glb#Mesh{i}/Primitive{j}` | [`Mesh`] (normals computed when missing) |
//! | `models/tree.glb#Mesh{i}` | alias of the primitive when mesh `i` has exactly one |
//! | `models/tree.glb#Material{i}` | [`StandardMaterial`] with its textures |
//! | `models/tree.glb#Image{i}` | [`Image`] (sRGB if used as base color/emissive) |
//! | `models/tree.glb#Scene{i}` | [`GltfScene`]: the node tree |
//! | `models/tree.glb` | the default scene ([`GltfScene`]); also the [`Mesh`] and [`StandardMaterial`] when the file has a single primitive |
//!
//! Spawn a scene with [`spawn_gltf_scene`] (now, from `&mut World`), [`GltfSceneSpawner`] /
//! [`spawn_gltf_scene_deferred`] (from systems, once loaded) or the `render.spawn_gltf` API.
//! Spawned entities reference assets by key, so hot reloading the file updates them in place.
//!
//! Units and axes match the engine (meters, +Y up, right-handed); no conversion is applied.
//! Skins, animations and morph targets are not interpreted here (the animation module reads
//! them from the file itself); each [`GltfNode`] records its glTF index and skin so they can
//! be matched up. Texture samplers are not honored: every material texture uses the engine's
//! trilinear, repeating, anisotropic sampler. Only `TEXCOORD_0` is used.

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use engine_assets::{Asset, AssetLoader, Assets, Handle, LoadContext};
use engine_core::prelude::*;

use crate::components::{MaterialRef, MeshRef, MeshRenderer};
use crate::material::{AlphaMode, StandardMaterial};
use crate::mesh::{Mesh, Vertex};
use crate::texture::Image;

/// Luminance (cd/m²) of a glTF emissive factor of 1.0 (scaled further by
/// `KHR_materials_emissive_strength`). glTF emission is relative; this makes a fully
/// emissive surface clearly glow at night and stay visible in daylight.
pub const GLTF_EMISSIVE_LUMINANCE: f32 = 1000.0;

/// The node tree of one glTF scene. Asset keys: `"path#Scene{i}"`, and `"path"` for the
/// file's default scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GltfScene {
    /// Scene name from the file, if any.
    pub name: Option<String>,
    /// The file this came from (relative to the asset root).
    pub source: String,
    /// Every node of the file, indexed like the file's `nodes` array.
    pub nodes: Vec<GltfNode>,
    /// Indices into `nodes` of this scene's root nodes.
    pub roots: Vec<usize>,
}

impl Asset for GltfScene {}

impl GltfScene {
    /// Number of nodes reachable from the roots.
    pub fn reachable_nodes(&self) -> usize {
        let mut seen = HashSet::new();
        let mut stack: Vec<usize> = self.roots.clone();
        while let Some(i) = stack.pop() {
            if i < self.nodes.len() && seen.insert(i) {
                stack.extend(self.nodes[i].children.iter().copied());
            }
        }
        seen.len()
    }
}

/// One glTF node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GltfNode {
    pub name: Option<String>,
    /// Local transform relative to the parent node (meters).
    pub transform: Transform,
    /// Child node indices (into [`GltfScene::nodes`]).
    pub children: Vec<usize>,
    /// What this node draws (one entry per mesh primitive).
    pub primitives: Vec<GltfPrimitive>,
    /// Index of the glTF mesh this node instantiates, if any.
    pub mesh: Option<usize>,
    /// Index of the glTF skin this node uses, if any (for the animation module).
    pub skin: Option<usize>,
}

/// A drawable piece of a node: mesh and material asset keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GltfPrimitive {
    /// `Mesh` key, e.g. `"models/tree.glb#Mesh0/Primitive0"`.
    pub mesh: String,
    /// `StandardMaterial` key (`None`: the engine's default material).
    pub material: Option<String>,
}

impl GltfPrimitive {
    /// A renderer that draws this primitive (assets referenced by key).
    pub fn renderer(&self) -> MeshRenderer {
        MeshRenderer::new(
            MeshRef::Asset(self.mesh.clone()),
            match &self.material {
                Some(key) => MaterialRef::Asset(key.clone()),
                None => MaterialRef::default(),
            },
        )
    }
}

/// Loads `.gltf` and `.glb` files. Registered by `RenderPlugin`.
pub struct GltfLoader;

impl AssetLoader for GltfLoader {
    fn extensions(&self) -> &[&str] {
        &["gltf", "glb"]
    }

    fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String> {
        let gltf =
            gltf::Gltf::from_slice(bytes).map_err(|e| format!("not a valid glTF file: {e}"))?;
        let doc = &gltf.document;
        let buffers = load_buffers(doc, gltf.blob.as_deref(), ctx)?;

        // Images. Color textures (base color, emissive) are sRGB; everything else is data.
        let mut color_images = HashSet::new();
        for m in doc.materials() {
            if let Some(info) = m.pbr_metallic_roughness().base_color_texture() {
                color_images.insert(info.texture().source().index());
            }
            if let Some(info) = m.emissive_texture() {
                color_images.insert(info.texture().source().index());
            }
        }
        let mut image_keys: Vec<Option<String>> = vec![None; doc.images().len()];
        for image in doc.images() {
            let i = image.index();
            match load_image(&image, &buffers, ctx, color_images.contains(&i)) {
                Ok(img) => image_keys[i] = Some(ctx.add_labeled(&format!("Image{i}"), img)),
                Err(e) => tracing::warn!("{}: image {i} skipped: {e}", ctx.path()),
            }
        }

        // Materials.
        let mut material_labels: Vec<String> = Vec::new();
        for m in doc.materials() {
            let Some(i) = m.index() else { continue };
            let (base, textures) = convert_material(&m, &image_keys);
            let label = format!("Material{i}");
            ctx.add_with(Some(&label), move |resolver| {
                let mut mat = base;
                let handles: [Option<Handle<Image>>; 5] = std::array::from_fn(|slot| {
                    textures[slot]
                        .as_deref()
                        .map(|k| resolver.handle::<Image>(k))
                });
                mat.base_color_texture = handles[0];
                mat.normal_texture = handles[1];
                mat.metallic_roughness_texture = handles[2];
                mat.emissive_texture = handles[3];
                mat.occlusion_texture = handles[4];
                mat
            });
            if material_labels.len() <= i {
                material_labels.resize(i + 1, String::new());
            }
            material_labels[i] = label;
        }
        let material_key = |m: &gltf::Material| -> Option<String> {
            m.index()
                .and_then(|i| material_labels.get(i))
                .filter(|l| !l.is_empty())
                .map(|l| ctx.key(Some(l)))
        };

        // Meshes: one engine mesh per primitive.
        let mut mesh_prims: Vec<Vec<Option<GltfPrimitive>>> = Vec::new();
        let mut prim_labels: Vec<(String, Option<String>)> = Vec::new();
        let mut meshes_out: Vec<(String, Mesh)> = Vec::new();
        for mesh in doc.meshes() {
            let mut prims = Vec::new();
            for prim in mesh.primitives() {
                let label = format!("Mesh{}/Primitive{}", mesh.index(), prim.index());
                match read_primitive(&prim, &buffers) {
                    Ok(Some(m)) => {
                        let material = material_key(&prim.material());
                        prims.push(Some(GltfPrimitive {
                            mesh: ctx.key(Some(&label)),
                            material: material.clone(),
                        }));
                        prim_labels.push((
                            label.clone(),
                            prim.material().index().map(|i| format!("Material{i}")),
                        ));
                        meshes_out.push((label, m));
                    }
                    Ok(None) => prims.push(None),
                    Err(e) => {
                        tracing::warn!("{}: {label} skipped: {e}", ctx.path());
                        prims.push(None);
                    }
                }
            }
            mesh_prims.push(prims);
        }
        for (label, m) in meshes_out {
            ctx.add_labeled(&label, m);
        }
        for (i, prims) in mesh_prims.iter().enumerate() {
            if prims.len() == 1 && prims[0].is_some() {
                ctx.alias::<Mesh>(Some(&format!("Mesh{i}")), &format!("Mesh{i}/Primitive0"));
            }
        }
        // A file with a single primitive can be used by its path alone.
        if let [(label, material)] = prim_labels.as_slice() {
            ctx.alias::<Mesh>(None, label);
            if let Some(material) = material {
                ctx.alias::<StandardMaterial>(None, material);
            }
        }

        // Node tree.
        let nodes: Vec<GltfNode> = doc
            .nodes()
            .map(|node| {
                let (t, r, s) = node.transform().decomposed();
                let rotation = Quat::from_xyzw(r[0], r[1], r[2], r[3]);
                let rotation = if rotation.length_squared() > 1e-12 {
                    rotation.normalize()
                } else {
                    Quat::IDENTITY
                };
                let primitives = node
                    .mesh()
                    .and_then(|m| mesh_prims.get(m.index()))
                    .map(|prims| prims.iter().flatten().cloned().collect())
                    .unwrap_or_default();
                GltfNode {
                    name: node.name().map(String::from),
                    transform: Transform {
                        translation: DVec3::new(t[0] as f64, t[1] as f64, t[2] as f64),
                        rotation,
                        scale: Vec3::from(s),
                    },
                    children: node.children().map(|c| c.index()).collect(),
                    primitives,
                    mesh: node.mesh().map(|m| m.index()),
                    skin: node.skin().map(|s| s.index()),
                }
            })
            .collect();

        // Scenes (a file without scenes gets one with every parentless node).
        let mut scenes: Vec<(Option<String>, Vec<usize>)> = doc
            .scenes()
            .map(|s| {
                (
                    s.name().map(String::from),
                    s.nodes().map(|n| n.index()).collect(),
                )
            })
            .collect();
        if scenes.is_empty() {
            let children: HashSet<usize> = nodes
                .iter()
                .flat_map(|n| n.children.iter().copied())
                .collect();
            let roots = (0..nodes.len()).filter(|i| !children.contains(i)).collect();
            scenes.push((None, roots));
        }
        let default_scene = doc.default_scene().map(|s| s.index()).unwrap_or(0);
        let source = ctx.path().to_string();
        let scene_count = scenes.len();
        for (i, (name, roots)) in scenes.into_iter().enumerate() {
            ctx.add_labeled(
                &format!("Scene{i}"),
                GltfScene {
                    name,
                    source: source.clone(),
                    nodes: nodes.clone(),
                    roots,
                },
            );
        }
        ctx.alias::<GltfScene>(
            None,
            &format!("Scene{}", default_scene.min(scene_count - 1)),
        );
        Ok(())
    }
}

/// Buffer contents: GLB binary chunk, embedded base64 `data:` URIs, or files next to the
/// `.gltf` (tracked as dependencies for hot reload).
fn load_buffers(
    doc: &gltf::Document,
    blob: Option<&[u8]>,
    ctx: &mut LoadContext,
) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    for buffer in doc.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => blob
                .map(|b| b.to_vec())
                .ok_or("the file refers to a GLB binary chunk but has none")?,
            gltf::buffer::Source::Uri(uri) => match decode_data_uri(uri) {
                Some(data) => data?,
                None => ctx.read_file(&percent_decode(uri))?,
            },
        };
        if data.len() < buffer.length() {
            return Err(format!(
                "buffer {} is truncated ({} of {} bytes)",
                buffer.index(),
                data.len(),
                buffer.length()
            ));
        }
        out.push(data);
    }
    Ok(out)
}

fn load_image(
    image: &gltf::Image,
    buffers: &[Vec<u8>],
    ctx: &mut LoadContext,
    srgb: bool,
) -> Result<Image, String> {
    let (bytes, ext): (Vec<u8>, Option<String>) = match image.source() {
        gltf::image::Source::View { view, mime_type } => {
            let buffer = buffers.get(view.buffer().index()).ok_or("missing buffer")?;
            let start = view.offset();
            let end = start + view.length();
            let bytes = buffer
                .get(start..end)
                .ok_or("image buffer view is out of range")?
                .to_vec();
            (bytes, mime_extension(mime_type))
        }
        gltf::image::Source::Uri { uri, mime_type } => match decode_data_uri(uri) {
            Some(data) => (data?, mime_type.and_then(mime_extension)),
            None => {
                let path = percent_decode(uri);
                let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
                (
                    ctx.read_file(&path)?,
                    ext.or_else(|| mime_type.and_then(mime_extension)),
                )
            }
        },
    };
    let mut img = Image::decode(&bytes, ext.as_deref(), srgb)?;
    img.generate_mips();
    Ok(img)
}

fn mime_extension(mime: &str) -> Option<String> {
    match mime {
        "image/png" => Some("png".into()),
        "image/jpeg" | "image/jpg" => Some("jpg".into()),
        _ => None,
    }
}

/// Material factors plus the image keys of its five texture slots.
fn convert_material(
    m: &gltf::Material,
    image_keys: &[Option<String>],
) -> (StandardMaterial, [Option<String>; 5]) {
    let pbr = m.pbr_metallic_roughness();
    let key_for = |texture: gltf::Texture, tex_coord: u32, what: &str| -> Option<String> {
        if tex_coord != 0 {
            tracing::debug!(
                "{what} texture uses TEXCOORD_{tex_coord}; only TEXCOORD_0 is supported, ignoring it"
            );
            return None;
        }
        image_keys.get(texture.source().index()).cloned().flatten()
    };
    let mut uv_scale = [1.0, 1.0];
    let base_tex = pbr.base_color_texture().and_then(|info| {
        if let Some(t) = info.texture_transform() {
            uv_scale = t.scale();
        }
        key_for(info.texture(), info.tex_coord(), "base color")
    });
    let mr_tex = pbr
        .metallic_roughness_texture()
        .and_then(|info| key_for(info.texture(), info.tex_coord(), "metallic/roughness"));
    let (normal_tex, normal_scale) = match m.normal_texture() {
        Some(n) => (key_for(n.texture(), n.tex_coord(), "normal"), n.scale()),
        None => (None, 1.0),
    };
    let (occlusion_tex, occlusion_strength) = match m.occlusion_texture() {
        Some(o) => (
            key_for(o.texture(), o.tex_coord(), "occlusion"),
            o.strength(),
        ),
        None => (None, 1.0),
    };
    let emissive_tex = m
        .emissive_texture()
        .and_then(|info| key_for(info.texture(), info.tex_coord(), "emissive"));
    let material = StandardMaterial {
        base_color: pbr.base_color_factor(),
        metallic: pbr.metallic_factor(),
        perceptual_roughness: pbr.roughness_factor(),
        emissive: m.emissive_factor(),
        emissive_strength: GLTF_EMISSIVE_LUMINANCE * m.emissive_strength().unwrap_or(1.0),
        alpha_mode: match m.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => AlphaMode::Mask(m.alpha_cutoff().unwrap_or(0.5)),
            gltf::material::AlphaMode::Blend => AlphaMode::Blend,
        },
        double_sided: m.double_sided(),
        unlit: m.unlit(),
        uv_scale,
        normal_scale,
        occlusion_strength,
        ..Default::default()
    };
    (
        material,
        [base_tex, normal_tex, mr_tex, emissive_tex, occlusion_tex],
    )
}

/// Triangles of one primitive; `Ok(None)` for primitives without positions or with a
/// non-triangle mode (points and lines are skipped).
fn read_primitive(prim: &gltf::Primitive, buffers: &[Vec<u8>]) -> Result<Option<Mesh>, String> {
    use gltf::mesh::Mode;
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.as_slice()));
    let Some(positions) = reader.read_positions() else {
        return Ok(None);
    };
    let positions: Vec<[f32; 3]> = positions.collect();
    let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|n| n.collect());
    let uvs: Option<Vec<[f32; 2]>> = reader.read_tex_coords(0).map(|t| t.into_f32().collect());
    let colors: Option<Vec<[f32; 4]>> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
    let count = positions.len();
    let vertices: Vec<Vertex> = (0..count)
        .map(|i| Vertex {
            position: positions[i],
            normal: normals
                .as_ref()
                .and_then(|n| n.get(i).copied())
                .unwrap_or([0.0, 1.0, 0.0]),
            uv: uvs
                .as_ref()
                .and_then(|t| t.get(i).copied())
                .unwrap_or([0.0, 0.0]),
            color: colors
                .as_ref()
                .and_then(|c| c.get(i).copied())
                .unwrap_or([1.0; 4]),
        })
        .collect();
    let raw: Vec<u32> = match reader.read_indices() {
        Some(indices) => indices.into_u32().collect(),
        None => (0..count as u32).collect(),
    };
    let mut indices: Vec<u32> = match prim.mode() {
        Mode::Triangles => raw[..raw.len() / 3 * 3].to_vec(),
        Mode::TriangleStrip => (0..raw.len().saturating_sub(2))
            .flat_map(|i| {
                if i % 2 == 0 {
                    [raw[i], raw[i + 1], raw[i + 2]]
                } else {
                    [raw[i], raw[i + 2], raw[i + 1]]
                }
            })
            .collect(),
        Mode::TriangleFan => (1..raw.len().saturating_sub(1))
            .flat_map(|i| [raw[i], raw[i + 1], raw[0]])
            .collect(),
        other => {
            tracing::debug!("skipping a {other:?} primitive (only triangles are drawn)");
            return Ok(None);
        }
    };
    // Drop triangles that point past the vertex data instead of failing the whole model.
    if indices.iter().any(|&i| i as usize >= count) {
        indices = indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| t.iter().all(|&i| (i as usize) < count))
            .flatten()
            .copied()
            .collect();
    }
    let mut mesh = Mesh::new(vertices, indices);
    if normals.is_none() {
        // glTF: "When normals are not specified, client implementations MUST calculate flat
        // normals."
        mesh.compute_flat_normals();
    }
    Ok(Some(mesh))
}

/// `data:[<mime>][;base64],<data>` → bytes. `None` if `uri` is not a data URI.
fn decode_data_uri(uri: &str) -> Option<Result<Vec<u8>, String>> {
    let rest = uri.strip_prefix("data:")?;
    let Some((meta, data)) = rest.split_once(',') else {
        return Some(Err("malformed data URI (no comma)".into()));
    };
    Some(if meta.ends_with(";base64") {
        base64_decode(data)
    } else {
        Ok(percent_decode(data).into_bytes())
    })
}

/// Standard or URL-safe base64, padding optional, whitespace ignored.
pub(crate) fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return Err(format!("invalid base64 character '{}'", c as char)),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Decode `%XX` escapes in a URI (relative file names with spaces etc.).
fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Spawn a loaded scene's node tree now. Returns the root entity (with `root` as its
/// transform and the scene or file name as its `Name`). Nodes become entities with
/// `Transform`, `Name` and `ChildOf`; nodes with one primitive get a [`MeshRenderer`], nodes
/// with several get one child entity per primitive.
///
/// ```ignore
/// engine_assets::load_now(world, "models/tree.glb")?;
/// let scene = world.resource::<Assets<GltfScene>>().by_key("models/tree.glb").unwrap();
/// let tree = spawn_gltf_scene(world, scene, Transform::from_xyz(3.0, 0.0, -2.0))?;
/// ```
pub fn spawn_gltf_scene(
    world: &mut World,
    scene: Handle<GltfScene>,
    root: Transform,
) -> Result<Entity, String> {
    let data = world
        .get_resource::<Assets<GltfScene>>()
        .and_then(|a| a.get(scene))
        .cloned()
        .ok_or_else(|| {
            "this glTF scene is not loaded yet: wait for assets.status to say 'loaded', or use \
             GltfSceneSpawner / spawn_gltf_scene_deferred"
                .to_string()
        })?;
    let entity = world.spawn(root).id();
    spawn_nodes(world, &data, entity);
    Ok(entity)
}

/// Spawn the scene under `root` (an existing entity).
fn spawn_nodes(world: &mut World, scene: &GltfScene, root: Entity) {
    if world.get::<Name>(root).is_none() {
        let name = scene.name.clone().unwrap_or_else(|| {
            scene
                .source
                .rsplit('/')
                .next()
                .unwrap_or(&scene.source)
                .to_string()
        });
        world.entity_mut(root).insert(Name::new(name));
    }
    let mut visited = HashSet::new();
    let mut stack: Vec<(usize, Entity)> = scene.roots.iter().rev().map(|&r| (r, root)).collect();
    while let Some((index, parent)) = stack.pop() {
        // glTF node graphs are trees; guard against malformed files that share or loop nodes.
        if !visited.insert(index) {
            continue;
        }
        let Some(node) = scene.nodes.get(index) else {
            continue;
        };
        let mut e = world.spawn((node.transform, ChildOf(parent)));
        if let Some(name) = &node.name {
            e.insert(Name::new(name.clone()));
        }
        if let [primitive] = node.primitives.as_slice() {
            e.insert(primitive.renderer());
        }
        let id = e.id();
        if node.primitives.len() > 1 {
            for (j, primitive) in node.primitives.iter().enumerate() {
                let name = format!("{}/{j}", node.name.as_deref().unwrap_or("primitive"));
                world.spawn((
                    Transform::IDENTITY,
                    ChildOf(id),
                    primitive.renderer(),
                    Name::new(name),
                ));
            }
        }
        for &child in node.children.iter().rev() {
            stack.push((child, id));
        }
    }
}

/// Put this on an entity to spawn a glTF scene under it as soon as the scene is loaded; the
/// component is removed afterwards. Get the handle with
/// `server.load(&mut scenes, "models/tree.glb")`. (Not saved in scene files: save the spawned
/// entities instead.)
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(Transform)]
pub struct GltfSceneSpawner(pub Handle<GltfScene>);

/// Spawn an entity at `root` that receives the scene's nodes once the scene is loaded (from
/// a system, with `Commands`). Returns the root entity right away.
pub fn spawn_gltf_scene_deferred(
    commands: &mut Commands,
    scene: Handle<GltfScene>,
    root: Transform,
) -> Entity {
    commands.spawn((root, GltfSceneSpawner(scene))).id()
}

/// Engine system (`PostUpdate`, before transform propagation): spawn pending
/// [`GltfSceneSpawner`]s whose scene has loaded.
pub fn spawn_pending_gltf_scenes(
    mut commands: Commands,
    scenes: Option<Res<Assets<GltfScene>>>,
    pending: Query<(Entity, &GltfSceneSpawner)>,
) {
    let Some(scenes) = scenes else {
        return;
    };
    for (entity, spawner) in &pending {
        let Some(scene) = scenes.get(spawner.0) else {
            continue;
        };
        let scene = scene.clone();
        commands.entity(entity).remove::<GltfSceneSpawner>();
        commands.queue(move |world: &mut World| {
            if world.get_entity(entity).is_ok() {
                spawn_nodes(world, &scene, entity);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_data_uris() {
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ").unwrap(), b"hello world");
        assert_eq!(base64_decode("AAEC/w==").unwrap(), [0, 1, 2, 255]);
        assert!(base64_decode("@@").is_err());
        let d = decode_data_uri("data:application/octet-stream;base64,AAEC")
            .unwrap()
            .unwrap();
        assert_eq!(d, [0, 1, 2]);
        assert!(decode_data_uri("tree.bin").is_none());
        assert_eq!(percent_decode("my%20tree.bin"), "my tree.bin");
        assert_eq!(percent_decode("100%"), "100%");
    }
}

//! Rigged characters from glTF 2.0 (`.glb`, or `.gltf` with its buffers and images):
//! skeleton, skinned mesh with its materials and textures, and animation clips.
//!
//! Everything is converted to engine conventions on import: meters, +Y up, rigid bones (node
//! scales are baked into positions), and an optional turn about +Y so the character faces −Z
//! like the engine's own rigs. Clips are resampled at a fixed rate from the file's node
//! animation, so any mix of keyed translation, rotation and scale (and scaled ancestor nodes,
//! as FBX exports often have) plays correctly on the converted skeleton.
//!
//! Animation-only files (a library of clips exported with the same skeleton) import with
//! [`import_clips`], matched to an existing skeleton by bone name.

use std::collections::HashMap;
use std::path::Path;

use engine_assets::Handle;
use engine_core::math::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::{AlphaMode, Image, StandardMaterial};

use crate::clip::{AnimationClip, BoneTrack, Track};
use crate::skeleton::{BoneDef, Skeleton};
use crate::skin::{SkinVertex, SkinnedMesh, Submesh};
use crate::xform::Xf;

/// How to convert a file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportOptions {
    /// Name of the skeleton (clips keep their names from the file).
    pub name: String,
    /// Extra uniform scale applied after the file's own units (1 = as authored, in meters).
    pub scale: f32,
    /// If set, scale so the bind pose is this tall (meters); overrides `scale`.
    pub height: Option<f32>,
    /// Turn about +Y applied to everything (degrees). 180 turns a +Z-facing glTF character
    /// to face −Z like the engine's rigs.
    pub yaw_deg: f32,
    /// Clip resampling rate (keys per second).
    pub sample_rate: f32,
    /// Remove this bone's forward travel from looping clips so they play in place (the
    /// travel speed is kept in [`ImportedClipInfo`]). Usually the hips/pelvis bone.
    pub in_place_bone: Option<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            name: "imported".into(),
            scale: 1.0,
            height: None,
            yaw_deg: 180.0,
            sample_rate: 30.0,
            in_place_bone: None,
        }
    }
}

/// A material of an imported mesh. Texture fields index [`ImportedModel::images`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMaterial {
    pub name: String,
    pub base_color: [f32; 4],
    pub base_color_texture: Option<usize>,
    pub normal_texture: Option<usize>,
    /// glTF metallic-roughness texture: G = roughness, B = metallic.
    pub metallic_roughness_texture: Option<usize>,
    pub roughness: f32,
    pub metallic: f32,
    /// Alpha-tested cutout (hair cards, eyelashes), or blended treated as cutout at 0.5.
    pub alpha_cutoff: Option<f32>,
    pub double_sided: bool,
}

impl ImportedMaterial {
    /// As an engine material, with `images` the handles of [`ImportedModel::images`] (same
    /// order).
    pub fn to_standard(&self, images: &[Handle<Image>]) -> StandardMaterial {
        let texture = |i: Option<usize>| i.and_then(|i| images.get(i).copied());
        StandardMaterial {
            base_color: self.base_color,
            perceptual_roughness: self.roughness,
            metallic: self.metallic,
            alpha_mode: match self.alpha_cutoff {
                Some(c) => AlphaMode::Mask(c),
                None => AlphaMode::Opaque,
            },
            double_sided: self.double_sided,
            base_color_texture: texture(self.base_color_texture),
            normal_texture: texture(self.normal_texture),
            metallic_roughness_texture: texture(self.metallic_roughness_texture),
            ..Default::default()
        }
    }
}

/// Facts about an imported clip.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedClipInfo {
    pub name: String,
    /// Forward travel speed removed by `in_place_bone`, m/s (0 if none).
    pub speed: f32,
}

/// What a file contained, converted.
#[derive(Debug, Clone)]
pub struct ImportedModel {
    pub skeleton: Skeleton,
    pub mesh: SkinnedMesh,
    pub materials: Vec<ImportedMaterial>,
    /// Decoded images (base color images are sRGB, others linear), with mips generated.
    pub images: Vec<Image>,
    pub clips: Vec<AnimationClip>,
    pub clip_info: Vec<ImportedClipInfo>,
    /// Scene units → meters factor that was applied (pass it to [`import_clips`] for
    /// animation files authored in the same units).
    pub unit_scale: f32,
}

struct Document {
    doc: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
    base: Option<std::path::PathBuf>,
}

fn open(bytes: &[u8], base: Option<&Path>) -> Result<Document, String> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| format!("not a glTF file: {e}"))?;
    let gltf::Gltf { document, blob } = gltf;
    let buffers = gltf::import_buffers(&document, base, blob)
        .map_err(|e| format!("could not read the glTF buffers: {e}"))?;
    Ok(Document {
        doc: document,
        buffers,
        base: base.map(Path::to_path_buf),
    })
}

/// Node transforms in the scene: local TRS per node, parent per node.
struct Nodes {
    parent: Vec<Option<usize>>,
    local: Vec<(Vec3, Quat, Vec3)>,
    names: Vec<String>,
}

impl Nodes {
    fn new(doc: &gltf::Document) -> Self {
        let n = doc.nodes().len();
        let mut parent = vec![None; n];
        let mut local = Vec::with_capacity(n);
        let mut names = Vec::with_capacity(n);
        for node in doc.nodes() {
            for c in node.children() {
                parent[c.index()] = Some(node.index());
            }
            let (t, r, s) = node.transform().decomposed();
            local.push((
                Vec3::from(t),
                Quat::from_array(r).normalize(),
                Vec3::from(s),
            ));
            names.push(
                node.name()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("node{}", node.index())),
            );
        }
        // Unique names (bones are looked up by name).
        let mut seen: HashMap<String, usize> = HashMap::new();
        for name in &mut names {
            let count = seen.entry(name.clone()).or_insert(0);
            if *count > 0 {
                *name = format!("{name}.{count}");
            }
            *count += 1;
        }
        Self {
            parent,
            local,
            names,
        }
    }

    /// Global matrices for the given local transforms.
    fn globals(&self, local: &[(Vec3, Quat, Vec3)]) -> Vec<Mat4> {
        let n = local.len();
        let mut out: Vec<Option<Mat4>> = vec![None; n];
        fn resolve(
            i: usize,
            parent: &[Option<usize>],
            local: &[(Vec3, Quat, Vec3)],
            out: &mut Vec<Option<Mat4>>,
        ) -> Mat4 {
            if let Some(m) = out[i] {
                return m;
            }
            let (t, r, s) = local[i];
            let m = Mat4::from_scale_rotation_translation(s, r, t);
            let g = match parent[i] {
                Some(p) => resolve(p, parent, local, out) * m,
                None => m,
            };
            out[i] = Some(g);
            g
        }
        (0..n)
            .map(|i| resolve(i, &self.parent, local, &mut out))
            .collect()
    }

    fn ancestors_and_self(&self, i: usize) -> Vec<usize> {
        let mut chain = vec![i];
        let mut cur = i;
        while let Some(p) = self.parent[cur] {
            chain.push(p);
            cur = p;
        }
        chain
    }
}

/// Scene space → engine model space: turn about Y and scale.
#[derive(Debug, Clone, Copy)]
struct Convert {
    yaw: Quat,
    scale: f32,
}

impl Convert {
    fn point(&self, p: Vec3) -> Vec3 {
        self.yaw * (p * self.scale)
    }

    /// A scene-space node matrix → a rigid engine transform (scale dropped).
    fn rigid(&self, m: &Mat4) -> Xf {
        let (_s, r, t) = m.to_scale_rotation_translation();
        Xf::from_tr(self.point(t), (self.yaw * r).normalize())
    }
}

/// Import a rigged model (and any animations in the same file).
pub fn import_gltf(
    bytes: &[u8],
    base: Option<&Path>,
    options: &ImportOptions,
) -> Result<ImportedModel, String> {
    let d = open(bytes, base)?;
    let nodes = Nodes::new(&d.doc);
    let rest_globals = nodes.globals(&nodes.local);

    // The skin, and every node that is a joint or an ancestor of one: that's the skeleton.
    let skin = d
        .doc
        .skins()
        .next()
        .ok_or("the file has no skin (no skeleton to animate)")?;
    let joint_nodes: Vec<usize> = skin.joints().map(|j| j.index()).collect();
    let mut in_skeleton = vec![false; nodes.parent.len()];
    for &j in &joint_nodes {
        for a in nodes.ancestors_and_self(j) {
            in_skeleton[a] = true;
        }
    }
    let reader = skin.reader(|b| Some(&d.buffers[b.index()]));
    let ibms: Vec<Mat4> = match reader.read_inverse_bind_matrices() {
        Some(it) => it.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
        None => vec![Mat4::IDENTITY; joint_nodes.len()],
    };

    // Collect the meshes: skinned primitives, and rigid meshes parented under a bone.
    struct Prim {
        positions: Vec<Vec3>,
        normals: Vec<Vec3>,
        uvs: Vec<Vec2>,
        colors: Vec<Vec4>,
        joints: Vec<[u16; 4]>,
        weights: Vec<[f32; 4]>,
        indices: Vec<u32>,
        material: Option<usize>,
    }
    let mut prims: Vec<Prim> = Vec::new();
    for node in d.doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let skinned = node.skin().is_some_and(|s| s.index() == skin.index());
        // A rigid mesh rides on its nearest skeleton ancestor.
        let rigid_bone = if skinned {
            None
        } else {
            nodes
                .ancestors_and_self(node.index())
                .into_iter()
                .find(|&a| in_skeleton[a])
        };
        if !skinned && rigid_bone.is_none() {
            continue;
        }
        for p in mesh.primitives() {
            if p.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let r = p.reader(|b| Some(&d.buffers[b.index()]));
            let Some(pos) = r.read_positions() else {
                continue;
            };
            let positions: Vec<Vec3> = pos.map(Vec3::from).collect();
            let n = positions.len();
            let normals: Vec<Vec3> = r
                .read_normals()
                .map(|it| it.map(Vec3::from).collect())
                .unwrap_or_default();
            let uvs: Vec<Vec2> = r
                .read_tex_coords(0)
                .map(|it| it.into_f32().map(Vec2::from).collect())
                .unwrap_or_else(|| vec![Vec2::ZERO; n]);
            let colors: Vec<Vec4> = r
                .read_colors(0)
                .map(|it| it.into_rgba_f32().map(Vec4::from).collect())
                .unwrap_or_else(|| vec![Vec4::ONE; n]);
            let indices: Vec<u32> = r
                .read_indices()
                .map(|it| it.into_u32().collect())
                .unwrap_or_else(|| (0..n as u32).collect());
            let (joints, weights) = if skinned {
                let joints: Vec<[u16; 4]> = r
                    .read_joints(0)
                    .map(|it| it.into_u16().collect())
                    .ok_or("a skinned primitive has no JOINTS_0")?;
                let weights: Vec<[f32; 4]> = r
                    .read_weights(0)
                    .map(|it| it.into_f32().collect())
                    .ok_or("a skinned primitive has no WEIGHTS_0")?;
                (joints, weights)
            } else {
                // Fully weighted to the bone (as a joint index past the skin's joints,
                // resolved below).
                let bone = rigid_bone.unwrap_or(0);
                let slot = joint_nodes.len() as u16 + bone as u16;
                (vec![[slot, 0, 0, 0]; n], vec![[1.0, 0.0, 0.0, 0.0]; n])
            };
            // Rigid meshes: bring their vertices into the bone's space through the node.
            let (positions, normals) = if skinned {
                (positions, normals)
            } else {
                let bone = rigid_bone.unwrap_or(0);
                let to_bone = rest_globals[bone].inverse() * rest_globals[node.index()];
                let nm = Mat3::from_mat4(to_bone);
                (
                    positions
                        .iter()
                        .map(|p| to_bone.transform_point3(*p))
                        .collect(),
                    normals
                        .iter()
                        .map(|v| (nm * *v).normalize_or_zero())
                        .collect(),
                )
            };
            prims.push(Prim {
                positions,
                normals,
                uvs,
                colors,
                joints,
                weights,
                indices,
                material: p.material().index(),
            });
        }
    }
    if prims.is_empty() {
        return Err("the skin has no triangle meshes".into());
    }

    // Skin matrices at the rest pose: where each vertex sits in scene space.
    // Joint slot j < joints: skin joint; slot joints + node: rigid mesh on that node (its
    // vertices are already in the node's space).
    let skin_matrix = |slot: u16| -> Mat4 {
        let s = slot as usize;
        if s < joint_nodes.len() {
            rest_globals[joint_nodes[s]] * ibms[s]
        } else {
            rest_globals[s - joint_nodes.len()]
        }
    };

    // Units: measure the rest-pose mesh height in scene space.
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in &prims {
        for (i, v) in p.positions.iter().enumerate() {
            let m = blend(&p.joints[i], &p.weights[i], &skin_matrix);
            let w = m.transform_point3(*v);
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    let scene_height = (hi.y - lo.y).max(1e-6);
    let scale = match options.height {
        Some(h) => h / scene_height,
        None => options.scale,
    };
    let conv = Convert {
        yaw: Quat::from_rotation_y(options.yaw_deg.to_radians()),
        scale,
    };

    // Skeleton: skeleton nodes in parent-first order, rigid rest transforms.
    let bone_nodes: Vec<usize> = (0..nodes.parent.len())
        .filter(|&i| in_skeleton[i])
        .collect();
    let rigid_globals: Vec<Option<Xf>> = (0..nodes.parent.len())
        .map(|i| in_skeleton[i].then(|| conv.rigid(&rest_globals[i])))
        .collect();
    let mut bones = Vec::with_capacity(bone_nodes.len());
    for &i in &bone_nodes {
        let g = rigid_globals[i].expect("skeleton node");
        let (parent, local) = match nodes.parent[i] {
            Some(p) if in_skeleton[p] => {
                let pg = rigid_globals[p].expect("skeleton parent");
                (Some(nodes.names[p].clone()), pg.inverse().mul(&g))
            }
            _ => (None, g),
        };
        bones.push(BoneDef {
            name: nodes.names[i].clone(),
            parent,
            rest: local,
        });
    }
    let skeleton = Skeleton::new(&options.name, bones)?;

    // Mesh joints: the skin's joints, then any bones that carry rigid meshes.
    let mut mesh = SkinnedMesh::default();
    let mut slot_to_joint: HashMap<u16, u16> = HashMap::new();
    let mut joint_index = |slot: u16, mesh: &mut SkinnedMesh| -> u16 {
        *slot_to_joint.entry(slot).or_insert_with(|| {
            let s = slot as usize;
            let node = if s < joint_nodes.len() {
                joint_nodes[s]
            } else {
                s - joint_nodes.len()
            };
            let g = rigid_globals[node].unwrap_or(Xf::IDENTITY);
            mesh.joints.push(nodes.names[node].clone());
            mesh.inverse_bind.push(g.inverse().to_mat4());
            (mesh.joints.len() - 1) as u16
        })
    };
    for p in &prims {
        let base = mesh.vertices.len() as u32;
        let first = mesh.indices.len() as u32;
        let has_normals = p.normals.len() == p.positions.len();
        for i in 0..p.positions.len() {
            let m = blend(&p.joints[i], &p.weights[i], &skin_matrix);
            let pos = conv.point(m.transform_point3(p.positions[i]));
            let normal = if has_normals {
                conv.yaw * (Mat3::from_mat4(m) * p.normals[i]).normalize_or(Vec3::Y)
            } else {
                Vec3::Y
            };
            let mut joints = [0u16; 4];
            let mut weights = p.weights[i];
            let total: f32 = weights.iter().sum();
            if total > 1e-6 {
                for w in &mut weights {
                    *w /= total;
                }
            } else {
                weights = [1.0, 0.0, 0.0, 0.0];
            }
            for k in 0..4 {
                joints[k] = if weights[k] > 0.0 {
                    joint_index(p.joints[i][k], &mut mesh)
                } else {
                    0
                };
            }
            mesh.vertices.push(SkinVertex {
                position: pos,
                normal,
                uv: p.uvs[i],
                color: p.colors[i],
                joints,
                weights,
                pattern: pos,
                axis: Vec3::Y,
                mask: Vec4::new(0.0, 0.0, 0.0, 0.0),
                fur: 0.0,
            });
        }
        // glTF winding is counter-clockwise, like ours; a mirroring node would flip it, but
        // skinned meshes ignore their node transform.
        mesh.indices.extend(p.indices.iter().map(|i| base + i));
        mesh.submeshes.push(Submesh {
            first_index: first,
            index_count: p.indices.len() as u32,
            surface: p.material.unwrap_or(usize::MAX),
        });
        if !has_normals {
            // Filled in below for the whole mesh.
        }
    }
    if prims.iter().any(|p| p.normals.len() != p.positions.len()) {
        mesh.compute_normals();
    }
    set_limb_axes(&mut mesh, &skeleton);

    // Materials and images.
    let mut images = Vec::new();
    let mut image_slots: HashMap<(usize, bool), usize> = HashMap::new();
    let mut materials = Vec::new();
    for m in d.doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let mut texture = |info: Option<gltf::texture::Texture>, srgb: bool| -> Option<usize> {
            let tex = info?;
            let key = (tex.source().index(), srgb);
            if let Some(&i) = image_slots.get(&key) {
                return Some(i);
            }
            match load_image(&d, tex.source(), srgb) {
                Ok(img) => {
                    images.push(img);
                    image_slots.insert(key, images.len() - 1);
                    Some(images.len() - 1)
                }
                Err(e) => {
                    tracing::warn!("glTF image {}: {e}", tex.source().index());
                    None
                }
            }
        };
        let base_color_texture = texture(pbr.base_color_texture().map(|t| t.texture()), true);
        let metallic_roughness_texture =
            texture(pbr.metallic_roughness_texture().map(|t| t.texture()), false);
        let normal_texture = texture(m.normal_texture().map(|t| t.texture()), false);
        materials.push(ImportedMaterial {
            name: m.name().unwrap_or("material").to_string(),
            base_color: pbr.base_color_factor(),
            base_color_texture,
            normal_texture,
            metallic_roughness_texture,
            roughness: pbr.roughness_factor(),
            metallic: pbr.metallic_factor(),
            alpha_cutoff: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => None,
                gltf::material::AlphaMode::Mask => Some(m.alpha_cutoff().unwrap_or(0.5)),
                gltf::material::AlphaMode::Blend => Some(0.5),
            },
            double_sided: m.double_sided(),
        });
    }
    for s in &mut mesh.submeshes {
        if s.surface >= materials.len() {
            s.surface = 0;
        }
    }
    if materials.is_empty() {
        materials.push(ImportedMaterial {
            name: "default".into(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            roughness: 0.6,
            metallic: 0.0,
            alpha_cutoff: None,
            double_sided: false,
        });
    }

    let (clips, clip_info) = convert_clips(&d, &nodes, &skeleton, conv, options);
    Ok(ImportedModel {
        skeleton,
        mesh,
        materials,
        images,
        clips,
        clip_info,
        unit_scale: scale,
    })
}

/// Import the animations of a file onto an existing skeleton (matched by bone name). Use the
/// `unit_scale` the model was imported with so both line up.
pub fn import_clips(
    bytes: &[u8],
    base: Option<&Path>,
    skeleton: &Skeleton,
    unit_scale: f32,
    options: &ImportOptions,
) -> Result<(Vec<AnimationClip>, Vec<ImportedClipInfo>), String> {
    let d = open(bytes, base)?;
    let nodes = Nodes::new(&d.doc);
    let conv = Convert {
        yaw: Quat::from_rotation_y(options.yaw_deg.to_radians()),
        scale: unit_scale,
    };
    let (clips, info) = convert_clips(&d, &nodes, skeleton, conv, options);
    if clips.is_empty() {
        return Err("the file has no animations".into());
    }
    Ok((clips, info))
}

/// Skin matrix of one vertex at the rest pose.
fn blend(joints: &[u16; 4], weights: &[f32; 4], skin_matrix: &impl Fn(u16) -> Mat4) -> Mat4 {
    let total: f32 = weights.iter().sum();
    if total <= 1e-6 {
        return skin_matrix(joints[0]);
    }
    let mut m = Mat4::ZERO;
    for k in 0..4 {
        if weights[k] > 0.0 {
            m += skin_matrix(joints[k]) * (weights[k] / total);
        }
    }
    m
}

/// Each vertex's limb axis: the direction of its main bone toward that bone's first child
/// (or along the bone from its parent at chain ends). Folds run across it, veins along it.
fn set_limb_axes(mesh: &mut SkinnedMesh, skeleton: &Skeleton) {
    let n = skeleton.len();
    let mut axis = vec![Vec3::Y; n];
    for (b, axis) in axis.iter_mut().enumerate() {
        let head = skeleton.rest_model[b].t;
        let child = (0..n).find(|&c| skeleton.parents[c] == Some(b as u16));
        let dir = match (child, skeleton.parents[b].map(usize::from)) {
            (Some(c), _) => skeleton.rest_model[c].t - head,
            (None, Some(p)) => head - skeleton.rest_model[p].t,
            _ => Vec3::Y,
        };
        *axis = dir.normalize_or(Vec3::Y);
    }
    let joint_bone: Vec<Option<usize>> = mesh.joints.iter().map(|j| skeleton.index_of(j)).collect();
    for v in &mut mesh.vertices {
        let mut best = 0;
        for k in 1..4 {
            if v.weights[k] > v.weights[best] {
                best = k;
            }
        }
        if let Some(Some(b)) = joint_bone.get(v.joints[best] as usize) {
            v.axis = axis[*b];
        }
    }
}

fn load_image(d: &Document, image: gltf::Image, srgb: bool) -> Result<Image, String> {
    let (bytes, ext): (Vec<u8>, Option<String>) = match image.source() {
        gltf::image::Source::View { view, mime_type } => {
            let buf = &d.buffers[view.buffer().index()];
            let start = view.offset();
            let end = start + view.length();
            let ext = mime_type.rsplit('/').next().map(str::to_string);
            (
                buf.get(start..end)
                    .ok_or("image view out of range")?
                    .to_vec(),
                ext,
            )
        }
        gltf::image::Source::Uri { uri, mime_type } => {
            if let Some(rest) = uri.strip_prefix("data:") {
                let (meta, data) = rest.split_once(',').ok_or("bad data URI")?;
                if !meta.ends_with(";base64") {
                    return Err("only base64 data URIs are supported".into());
                }
                let ext = mime_type
                    .or_else(|| meta.split(';').next())
                    .and_then(|m| m.rsplit('/').next())
                    .map(str::to_string);
                (decode_base64(data)?, ext)
            } else {
                let base = d
                    .base
                    .as_deref()
                    .ok_or("an external image needs a base path")?;
                let path = base.join(percent_decode(uri));
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase());
                (
                    std::fs::read(&path)
                        .map_err(|e| format!("could not read {}: {e}", path.display()))?,
                    ext,
                )
            }
        }
    };
    let ext = ext.map(|e| if e == "jpeg" { "jpg".to_string() } else { e });
    let mut img = Image::decode(&bytes, ext.as_deref(), srgb)?;
    img.generate_mips();
    Ok(img)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = value(c).ok_or("bad base64")?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Resample every animation of the file onto `skeleton`.
fn convert_clips(
    d: &Document,
    nodes: &Nodes,
    skeleton: &Skeleton,
    conv: Convert,
    options: &ImportOptions,
) -> (Vec<AnimationClip>, Vec<ImportedClipInfo>) {
    // Skeleton bone → file node, by name.
    let node_of: Vec<Option<usize>> = (0..skeleton.len())
        .map(|b| nodes.names.iter().position(|n| *n == skeleton.names[b]))
        .collect();
    let mut clips = Vec::new();
    let mut info = Vec::new();
    for (ai, anim) in d.doc.animations().enumerate() {
        // Channels: node → (translation, rotation, scale) samplers.
        let mut channels: HashMap<usize, [Option<Sampled>; 3]> = HashMap::new();
        let mut duration = 0.0f32;
        for ch in anim.channels() {
            let r = ch.reader(|b| Some(&d.buffers[b.index()]));
            let Some(times) = r.read_inputs() else {
                continue;
            };
            let times: Vec<f32> = times.collect();
            let interp = ch.sampler().interpolation();
            let Some(outputs) = r.read_outputs() else {
                continue;
            };
            let (slot, values): (usize, Vec<[f32; 4]>) = match outputs {
                gltf::animation::util::ReadOutputs::Translations(it) => {
                    (0, it.map(|v| [v[0], v[1], v[2], 0.0]).collect())
                }
                gltf::animation::util::ReadOutputs::Rotations(it) => (1, it.into_f32().collect()),
                gltf::animation::util::ReadOutputs::Scales(it) => {
                    (2, it.map(|v| [v[0], v[1], v[2], 0.0]).collect())
                }
                gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => continue,
            };
            if let Some(&last) = times.last() {
                duration = duration.max(last);
            }
            channels.entry(ch.target().node().index()).or_default()[slot] = Some(Sampled {
                times,
                values,
                interp,
            });
        }
        if channels.is_empty() || duration <= 0.0 {
            continue;
        }
        let name = anim
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("animation{ai}"));
        let rate = options.sample_rate.max(1.0);
        let frames = ((duration * rate).ceil() as usize).max(1) + 1;
        let mut tracks: Vec<BoneTrack> = (0..skeleton.len())
            .filter_map(|b| {
                node_of[b].map(|_| BoneTrack {
                    bone: skeleton.names[b].clone(),
                    ..Default::default()
                })
            })
            .collect();
        let track_of: Vec<Option<usize>> = {
            let mut k = 0;
            (0..skeleton.len())
                .map(|b| {
                    node_of[b].map(|_| {
                        k += 1;
                        k - 1
                    })
                })
                .collect()
        };
        let mut local = nodes.local.clone();
        for f in 0..frames {
            let t = (f as f32 / rate).min(duration);
            for (&node, ch) in &channels {
                let (mut tr, mut rot, mut sc) = nodes.local[node];
                if let Some(s) = &ch[0] {
                    let v = s.sample(t, false);
                    tr = Vec3::new(v[0], v[1], v[2]);
                }
                if let Some(s) = &ch[1] {
                    rot = Quat::from_array(s.sample(t, true)).normalize();
                }
                if let Some(s) = &ch[2] {
                    let v = s.sample(t, false);
                    sc = Vec3::new(v[0], v[1], v[2]);
                }
                local[node] = (tr, rot, sc);
            }
            let globals = nodes.globals(&local);
            for b in 0..skeleton.len() {
                let (Some(node), Some(k)) = (node_of[b], track_of[b]) else {
                    continue;
                };
                let g = conv.rigid(&globals[node]);
                let l = match skeleton.parents[b]
                    .map(usize::from)
                    .and_then(|p| node_of[p])
                {
                    Some(pn) => conv.rigid(&globals[pn]).inverse().mul(&g),
                    None => g,
                };
                let tr = &mut tracks[k];
                tr.translation.times.push(t);
                tr.translation.values.push(l.t);
                tr.rotation.times.push(t);
                // Keep rotations in one hemisphere so interpolation takes the short way.
                let r = match tr.rotation.values.last() {
                    Some(prev) if prev.dot(l.r) < 0.0 => -l.r,
                    _ => l.r,
                };
                tr.rotation.values.push(r);
            }
        }
        let mut clip = AnimationClip {
            name: name.clone(),
            duration,
            looping: true,
            tracks,
            ..Default::default()
        };
        let speed = match &options.in_place_bone {
            Some(bone) => make_in_place(&mut clip, bone),
            None => 0.0,
        };
        info.push(ImportedClipInfo { name, speed });
        clips.push(clip);
    }
    (clips, info)
}

/// Remove a bone's horizontal travel over the clip (keeping its sway), so a walk plays in
/// place. Returns the travel speed in m/s.
pub fn make_in_place(clip: &mut AnimationClip, bone: &str) -> f32 {
    let Some(track) = clip.tracks.iter_mut().find(|t| t.bone == bone) else {
        return 0.0;
    };
    let tr: &mut Track<Vec3> = &mut track.translation;
    let (Some(&first), Some(&last)) = (tr.values.first(), tr.values.last()) else {
        return 0.0;
    };
    let travel = Vec3::new(last.x - first.x, 0.0, last.z - first.z);
    let duration = clip.duration.max(1e-6);
    for (t, v) in tr.times.iter().zip(tr.values.iter_mut()) {
        let w = t / duration;
        *v -= travel * w;
    }
    travel.length() / duration
}

struct Sampled {
    times: Vec<f32>,
    values: Vec<[f32; 4]>,
    interp: gltf::animation::Interpolation,
}

impl Sampled {
    fn sample(&self, t: f32, quat: bool) -> [f32; 4] {
        use gltf::animation::Interpolation;
        let n = self.times.len();
        let cubic = self.interp == Interpolation::CubicSpline;
        // Cubic spline outputs are (in-tangent, value, out-tangent) triples.
        let value = |i: usize| -> [f32; 4] {
            if cubic {
                self.values[i * 3 + 1]
            } else {
                self.values[i]
            }
        };
        if n == 0 || (cubic && self.values.len() < n * 3) || (!cubic && self.values.len() < n) {
            return [0.0, 0.0, 0.0, 1.0];
        }
        if t <= self.times[0] || n == 1 {
            return value(0);
        }
        if t >= self.times[n - 1] {
            return value(n - 1);
        }
        let hi = self.times.partition_point(|&k| k <= t);
        let lo = hi - 1;
        let span = (self.times[hi] - self.times[lo]).max(1e-9);
        let w = (t - self.times[lo]) / span;
        let (a, b) = (value(lo), value(hi));
        match self.interp {
            Interpolation::Step => a,
            Interpolation::CubicSpline => {
                let (t2, t3) = (w * w, w * w * w);
                let out_a = self.values[lo * 3 + 2];
                let in_b = self.values[hi * 3];
                let mut r = [0.0; 4];
                for k in 0..4 {
                    r[k] = (2.0 * t3 - 3.0 * t2 + 1.0) * a[k]
                        + (t3 - 2.0 * t2 + w) * span * out_a[k]
                        + (-2.0 * t3 + 3.0 * t2) * b[k]
                        + (t3 - t2) * span * in_b[k];
                }
                if quat {
                    Quat::from_array(r).normalize().to_array()
                } else {
                    r
                }
            }
            Interpolation::Linear => {
                if quat {
                    Quat::from_array(a).slerp(Quat::from_array(b), w).to_array()
                } else {
                    let mut r = [0.0; 4];
                    for k in 0..4 {
                        r[k] = a[k] + (b[k] - a[k]) * w;
                    }
                    r
                }
            }
        }
    }
}

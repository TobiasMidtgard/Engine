//! Converting other 3D formats to glTF at import, as Godot 4 does: FBX and OBJ files are read
//! with ufbx (the library Godot 4.3+ uses for FBX), already converted to metres, right-handed
//! and Y-up, and written as one self-contained `.glb`: the node tree, meshes split by
//! material, PBR materials and their textures (PNG/JPEG as they are, other image formats
//! converted to PNG), skins, and every animation take baked to a glTF animation.
//!
//! Skins follow what Blender's and Godot's glTF exporters do: all skinned meshes share one skin
//! whose joints are the union of their bones, each joint's inverse bind matrix is the inverse
//! of the bone's world transform at bind time, and skinned meshes are written in world bind
//! space on their own root nodes (glTF ignores a skinned mesh node's transform). Takes are
//! baked with `ufbx::bake_anim` (like Unity's "Resample Curves": Euler curves become
//! quaternion keys at a fixed rate) and start at time 0.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// What a conversion produced, for the log.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    pub nodes: usize,
    pub meshes: usize,
    pub materials: usize,
    pub textures: usize,
    pub triangles: usize,
    /// Things left out (missing textures, skins, animations).
    pub notes: Vec<String>,
}

impl Summary {
    pub fn describe(&self) -> String {
        let mut text = format!(
            "{} nodes, {} meshes ({} triangles), {} materials, {} textures",
            self.nodes, self.meshes, self.triangles, self.materials, self.textures
        );
        for note in &self.notes {
            text.push_str("; ");
            text.push_str(note);
        }
        text
    }
}

/// Formats [`to_glb`] reads.
pub fn converts(extension: &str) -> bool {
    matches!(extension, "fbx" | "obj" | "blend")
}

/// Read `source` (FBX, OBJ, or `.blend` through Blender) and write it as a `.glb` at `out`.
pub fn to_glb(source: &Path, out: &Path) -> Result<Summary, String> {
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "blend" {
        blend_to_glb(source, out)
    } else {
        ufbx_to_glb(source, out)
    }
}

/// Blender, if it's installed: `$BLENDER`, `blender` on the PATH, or its usual install folders.
pub fn find_blender() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("BLENDER").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    let names = ["blender", "blender.exe"];
    if let Some(path) = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .flat_map(|dir| names.map(|n| dir.join(n)))
        .find(|p| p.is_file())
    {
        return Some(path);
    }
    let mut installs: Vec<PathBuf> = [
        "C:/Program Files/Blender Foundation",
        "C:/Program Files (x86)/Blender Foundation",
    ]
    .iter()
    .filter_map(|dir| std::fs::read_dir(dir).ok())
    .flat_map(|entries| entries.flatten().map(|e| e.path().join("blender.exe")))
    .filter(|p| p.is_file())
    .collect();
    // The newest version sorts last ("Blender 4.2" < "Blender 5.1").
    installs.sort();
    installs.pop().or_else(|| {
        [
            "/Applications/Blender.app/Contents/MacOS/Blender",
            "/snap/bin/blender",
        ]
        .map(PathBuf::from)
        .into_iter()
        .find(|p| p.is_file())
    })
}

/// Have Blender export a `.blend` file as glTF, the way Godot imports `.blend` files: Blender
/// runs in the background with factory settings and its own glTF exporter.
fn blend_to_glb(source: &Path, out: &Path) -> Result<Summary, String> {
    let blender = find_blender().ok_or(
        "importing .blend files needs Blender installed (or set BLENDER to blender's path); \
         or export glTF (.glb) or FBX from Blender yourself",
    )?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let script = format!(
        "import bpy; bpy.ops.export_scene.gltf(filepath={:?}, export_format='GLB', \
         export_apply=True, export_yup=True)",
        out.to_string_lossy()
    );
    let result = std::process::Command::new(&blender)
        .arg("--background")
        .arg("--factory-startup")
        .arg(source)
        .arg("--python-exit-code")
        .arg("1")
        .arg("--python-expr")
        .arg(&script)
        .output()
        .map_err(|e| format!("couldn't run {}: {e}", blender.display()))?;
    if !result.status.success() || !out.is_file() {
        let log = String::from_utf8_lossy(&result.stderr);
        let last = log
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(format!(
            "Blender couldn't export {}: {last}",
            source.display()
        ));
    }
    let mut summary = inspect(out)?;
    summary.notes.push("exported by Blender".into());
    Ok(summary)
}

/// What a glTF or GLB file holds, without loading its meshes (the import preview).
pub fn inspect(path: &Path) -> Result<Summary, String> {
    let gltf = gltf::Gltf::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc = &gltf.document;
    let triangles = doc
        .meshes()
        .flat_map(|m| m.primitives())
        .map(|p| {
            p.indices()
                .map(|a| a.count())
                .or_else(|| p.get(&gltf::Semantic::Positions).map(|a| a.count()))
                .unwrap_or(0)
                / 3
        })
        .sum();
    let mut notes = Vec::new();
    if doc.skins().len() > 0 {
        notes.push(format!("{} skins", doc.skins().len()));
    }
    if doc.animations().len() > 0 {
        notes.push(format!("{} animations", doc.animations().len()));
    }
    Ok(Summary {
        nodes: doc.nodes().len(),
        meshes: doc.meshes().len(),
        materials: doc.materials().len(),
        textures: doc.textures().len(),
        triangles,
        notes,
    })
}

/// Read an FBX or OBJ file with ufbx and write it as a `.glb` at `out`.
fn ufbx_to_glb(source: &Path, out: &Path) -> Result<Summary, String> {
    let load = |blender_materials: bool| {
        let opts = ufbx::LoadOpts {
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_unit_meters: 1.0,
            space_conversion: ufbx::SpaceConversion::ModifyGeometry,
            geometry_transform_handling: ufbx::GeometryTransformHandling::ModifyGeometry,
            generate_missing_normals: true,
            load_external_files: true,
            ignore_missing_external_files: true,
            use_blender_pbr_material: blender_materials,
            ..Default::default()
        };
        ufbx::load_file(&source.to_string_lossy(), opts)
            .map_err(|e| format!("can't read {}: {}", source.display(), &*e.description))
    };
    let mut scene = load(false)?;
    // Blender writes its Principled BSDF into FBX's legacy fields its own way (metallic in
    // the reflection factor, and so on); ufbx reads them right when told the file is Blender's.
    if matches!(
        scene.metadata.exporter,
        ufbx::Exporter::BlenderBinary | ufbx::Exporter::BlenderAscii
    ) {
        scene = load(true)?;
    }
    let dir = source.parent().unwrap_or(Path::new("."));
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut glb = Glb::default();
    let mut summary = Summary::default();

    // Materials and their textures.
    let mut material_index = HashMap::new();
    let mut texture_index: HashMap<u32, Option<usize>> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    for material in &scene.materials {
        let pbr = &material.pbr;
        let mut texture = |map: &ufbx::MaterialMap, glb: &mut Glb, summary: &mut Summary| {
            let tex = map.texture.as_ref()?;
            *texture_index
                .entry(tex.element.typed_id)
                .or_insert_with(|| match texture_bytes(tex, dir, &stem) {
                    Some((bytes, mime)) => {
                        summary.textures += 1;
                        Some(glb.image(&bytes, mime))
                    }
                    None => {
                        missing.push(
                            Path::new(&*tex.filename)
                                .file_name()
                                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                        );
                        None
                    }
                })
        };
        let base = &pbr.base_color.value_vec4;
        let factor = if pbr.base_factor.has_value {
            pbr.base_factor.value_vec4.x
        } else {
            1.0
        };
        let opacity = if pbr.opacity.has_value {
            pbr.opacity.value_vec4.x
        } else {
            1.0
        };
        // Glass: our materials have no transmission yet, so see-through becomes alpha.
        let transmission = if pbr.transmission_factor.has_value {
            pbr.transmission_factor.value_vec4.x.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let alpha = (opacity * (1.0 - 0.85 * transmission)).clamp(0.0, 1.0);
        let mut pbr_json = json!({
            "baseColorFactor": [base.x * factor, base.y * factor, base.z * factor, alpha],
            "metallicFactor": if pbr.metalness.has_value { pbr.metalness.value_vec4.x } else { 0.0 },
            "roughnessFactor": if pbr.roughness.has_value { pbr.roughness.value_vec4.x } else { 0.5 },
        });
        if let Some(t) = texture(&pbr.base_color, &mut glb, &mut summary) {
            pbr_json["baseColorTexture"] = json!({ "index": t });
        }
        let mut mat = json!({
            "name": &*material.element.name,
            "pbrMetallicRoughness": pbr_json,
        });
        if let Some(t) = texture(&pbr.normal_map, &mut glb, &mut summary) {
            mat["normalTexture"] = json!({ "index": t });
        }
        let e = &pbr.emission_color.value_vec4;
        let strength = if pbr.emission_factor.has_value {
            pbr.emission_factor.value_vec4.x
        } else {
            1.0
        };
        if pbr.emission_color.has_value && (e.x + e.y + e.z) * strength > 0.0 {
            mat["emissiveFactor"] = json!([e.x * strength, e.y * strength, e.z * strength]);
        }
        if alpha < 0.999 {
            mat["alphaMode"] = json!("BLEND");
        }
        material_index.insert(material.element.typed_id, glb.materials.len());
        glb.materials.push(mat);
    }
    summary.materials = glb.materials.len();
    missing.sort();
    missing.dedup();
    if let Some(first) = missing.first() {
        summary.notes.push(match missing.len() {
            1 => format!("texture {first} not found"),
            n => format!("{n} textures not found ({first}, ...)"),
        });
    }

    let mut node_index = HashMap::new();
    for node in &scene.nodes {
        if node.is_root {
            continue;
        }
        node_index.insert(node.element.typed_id, node_index.len());
    }

    // One skin for every skinned mesh: joints by bone node, inverse bind matrices from the
    // bones' world transforms at bind time.
    let mut joints: Vec<usize> = Vec::new();
    let mut inverse_binds: Vec<[f32; 16]> = Vec::new();
    let mut joint_slot: HashMap<u32, u16> = HashMap::new();
    for skin in &scene.skin_deformers {
        for cluster in &skin.clusters {
            let Some(bone) = cluster.bone_node.as_ref() else {
                continue;
            };
            let Some(&node) = node_index.get(&bone.element.typed_id) else {
                continue;
            };
            joint_slot.entry(bone.element.typed_id).or_insert_with(|| {
                joints.push(node);
                inverse_binds.push(mat4(&ufbx::matrix_invert(&cluster.bind_to_world)));
                (joints.len() - 1) as u16
            });
        }
    }

    // Meshes, one primitive per material.
    let mut mesh_index = HashMap::new();
    let mut skinned_meshes = Vec::new();
    for mesh in &scene.meshes {
        // A skinned mesh is written in world bind space (its first bone cluster says where the
        // geometry sat when it was bound), with up to four bone weights per vertex.
        let skin = mesh
            .skin_deformers
            .first()
            .filter(|s| s.clusters.iter().any(|c| c.bone_node.is_some()));
        let to_world = skin.map(|s| s.clusters[0].geometry_to_world);
        let normals_to_world = to_world.as_ref().map(ufbx::matrix_for_normals);
        let mut primitives = Vec::new();
        let mut tri = vec![0u32; mesh.max_face_triangles * 3];
        for part in &mesh.material_parts {
            if part.num_triangles == 0 {
                continue;
            }
            let mut vertices: Vec<[f32; 8]> = Vec::new();
            let mut weights: Vec<([u16; 4], [f32; 4])> = Vec::new();
            let mut seen: HashMap<[u32; 9], u32> = HashMap::new();
            let mut indices: Vec<u32> = Vec::new();
            for &face in &part.face_indices {
                let face = mesh.faces[face as usize];
                let n = mesh.triangulate_face(&mut tri, face) as usize;
                for &ix in &tri[..n * 3] {
                    let ix = ix as usize;
                    let vertex = mesh.vertex_indices[ix];
                    let mut p = mesh.vertex_position[ix];
                    let mut nrm = if mesh.vertex_normal.exists {
                        mesh.vertex_normal[ix]
                    } else {
                        ufbx::Vec3 {
                            x: 0.0,
                            y: 1.0,
                            z: 0.0,
                        }
                    };
                    if let (Some(m), Some(nm)) = (&to_world, &normals_to_world) {
                        p = ufbx::transform_position(m, p);
                        nrm = ufbx::transform_direction(nm, nrm);
                        let len = (nrm.x * nrm.x + nrm.y * nrm.y + nrm.z * nrm.z).sqrt();
                        if len > 1e-12 {
                            nrm = ufbx::Vec3 {
                                x: nrm.x / len,
                                y: nrm.y / len,
                                z: nrm.z / len,
                            };
                        }
                    }
                    let uv = if mesh.vertex_uv.exists {
                        mesh.vertex_uv[ix]
                    } else {
                        ufbx::Vec2 { x: 0.0, y: 0.0 }
                    };
                    // FBX UVs start at the bottom left, glTF's at the top left.
                    let v = [
                        p.x as f32,
                        p.y as f32,
                        p.z as f32,
                        nrm.x as f32,
                        nrm.y as f32,
                        nrm.z as f32,
                        uv.x as f32,
                        1.0 - uv.y as f32,
                    ];
                    // The FBX vertex is part of the key: its bone weights belong to it.
                    let mut key = [vertex; 9];
                    for (k, f) in key.iter_mut().zip(v) {
                        *k = f.to_bits();
                    }
                    let index = *seen.entry(key).or_insert_with(|| {
                        vertices.push(v);
                        if let Some(skin) = skin {
                            weights.push(bone_weights(skin, vertex as usize, &joint_slot));
                        }
                        vertices.len() as u32 - 1
                    });
                    indices.push(index);
                }
            }
            summary.triangles += indices.len() / 3;
            let material = mesh
                .materials
                .get(part.index as usize)
                .and_then(|m| material_index.get(&m.element.typed_id).copied());
            let skin_weights = skin.is_some().then_some(&weights[..]);
            primitives.push(glb.primitive(&vertices, skin_weights, &indices, material));
        }
        if primitives.is_empty() {
            continue;
        }
        if skin.is_some() {
            skinned_meshes.push(mesh.element.typed_id);
        }
        mesh_index.insert(mesh.element.typed_id, glb.meshes.len());
        glb.meshes
            .push(json!({ "name": &*mesh.element.name, "primitives": primitives }));
    }
    summary.meshes = glb.meshes.len();

    // The node tree (the FBX root node itself is left out; its children are the scene).
    let mut nodes = vec![Value::Null; node_index.len()];
    for node in &scene.nodes {
        let Some(&i) = node_index.get(&node.element.typed_id) else {
            continue;
        };
        let t = &node.local_transform;
        let mut json = json!({
            "name": &*node.element.name,
            "translation": [t.translation.x, t.translation.y, t.translation.z],
            "rotation": [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
            "scale": [t.scale.x, t.scale.y, t.scale.z],
        });
        if let Some(mesh) = node.mesh.as_ref()
            && let Some(m) = mesh_index.get(&mesh.element.typed_id)
            && !skinned_meshes.contains(&mesh.element.typed_id)
        {
            json["mesh"] = json!(m);
        }
        let children: Vec<usize> = node
            .children
            .iter()
            .filter_map(|c| node_index.get(&c.element.typed_id).copied())
            .collect();
        if !children.is_empty() {
            json["children"] = json!(children);
        }
        nodes[i] = json;
    }
    let mut roots: Vec<usize> = scene
        .root_node
        .children
        .iter()
        .filter_map(|c| node_index.get(&c.element.typed_id).copied())
        .collect();

    // Skinned meshes on their own root nodes, sharing the skin.
    if !skinned_meshes.is_empty() {
        let inverse_binds: Vec<f32> = inverse_binds.concat();
        let bytes: Vec<u8> = inverse_binds.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = glb.view(&bytes, None);
        let accessor = glb.accessor(view, FLOAT, joints.len(), "MAT4");
        glb.skins.push(json!({
            "name": "skin",
            "joints": joints,
            "inverseBindMatrices": accessor,
        }));
        for node in &scene.nodes {
            let Some(mesh) = node.mesh.as_ref() else {
                continue;
            };
            if !skinned_meshes.contains(&mesh.element.typed_id) {
                continue;
            }
            roots.push(nodes.len());
            nodes.push(json!({
                "name": &*node.element.name,
                "mesh": mesh_index[&mesh.element.typed_id],
                "skin": 0,
            }));
        }
        summary
            .notes
            .push(format!("skinned to {} bones", joints.len()));
    }

    // Every take, baked.
    for stack in &scene.anim_stacks {
        let opts = ufbx::BakeOpts {
            resample_rate: 30.0,
            trim_start_time: true,
            ..Default::default()
        };
        let baked = match ufbx::bake_anim(&scene, &stack.anim, opts) {
            Ok(baked) => baked,
            Err(e) => {
                summary.notes.push(format!(
                    "animation {} not baked: {}",
                    &*stack.element.name, &*e.description
                ));
                continue;
            }
        };
        if let Some(animation) = glb.animation(&stack.element.name, &baked, &node_index) {
            glb.animations.push(animation);
        }
    }
    if !glb.animations.is_empty() {
        summary
            .notes
            .push(format!("{} animations", glb.animations.len()));
    }
    summary.nodes = nodes.len();
    glb.write(out, nodes, roots, &stem)?;
    Ok(summary)
}

/// A ufbx affine matrix as a glTF column-major 4×4.
fn mat4(m: &ufbx::Matrix) -> [f32; 16] {
    [
        m.m00, m.m10, m.m20, 0.0, m.m01, m.m11, m.m21, 0.0, m.m02, m.m12, m.m22, 0.0, m.m03, m.m13,
        m.m23, 1.0,
    ]
    .map(|x| x as f32)
}

/// A vertex's four strongest bone weights (as skin joint slots), summing to one.
fn bone_weights(
    skin: &ufbx::SkinDeformer,
    vertex: usize,
    joint_slot: &HashMap<u32, u16>,
) -> ([u16; 4], [f32; 4]) {
    let mut all: Vec<(u16, f32)> = skin.vertices.get(vertex).map_or_else(Vec::new, |v| {
        let begin = v.weight_begin as usize;
        (begin..begin + v.num_weights as usize)
            .filter_map(|i| skin.weights.get(i))
            .filter_map(|w| {
                let bone = skin
                    .clusters
                    .get(w.cluster_index as usize)?
                    .bone_node
                    .as_ref()?;
                Some((*joint_slot.get(&bone.element.typed_id)?, w.weight as f32))
            })
            .filter(|&(_, w)| w > 0.0)
            .collect()
    });
    all.sort_by(|a, b| b.1.total_cmp(&a.1));
    all.truncate(4);
    let total: f32 = all.iter().map(|w| w.1).sum();
    let mut joints = [0u16; 4];
    let mut weights = [0.0f32; 4];
    if total <= 0.0 {
        // Unweighted vertices ride on the first bone rather than collapsing to the origin.
        weights[0] = 1.0;
    }
    for (k, (joint, weight)) in all.into_iter().enumerate() {
        joints[k] = joint;
        weights[k] = weight / total;
    }
    (joints, weights)
}

/// A texture's image file, found the way DCC exports leave them: embedded, at its recorded
/// path, beside the source, or in Blender's/Maya's `<name>.fbm/` folder. PNG and JPEG are
/// kept; other formats are converted to PNG.
fn texture_bytes(tex: &ufbx::Texture, dir: &Path, stem: &str) -> Option<(Vec<u8>, &'static str)> {
    let bytes = if !tex.content.is_empty() {
        tex.content.to_vec()
    } else {
        let name = Path::new(&*tex.filename)
            .file_name()
            .or_else(|| Path::new(&*tex.relative_filename).file_name())?
            .to_owned();
        let candidates = [
            Path::new(&*tex.absolute_filename).to_path_buf(),
            Path::new(&*tex.filename).to_path_buf(),
            dir.join(&*tex.relative_filename),
            dir.join(&name),
            dir.join(format!("{stem}.fbm")).join(&name),
            dir.join("textures").join(&name),
        ];
        let found = candidates.iter().find(|p| p.is_file())?;
        std::fs::read(found).ok()?
    };
    match image::guess_format(&bytes).ok()? {
        image::ImageFormat::Png => Some((bytes, "image/png")),
        image::ImageFormat::Jpeg => Some((bytes, "image/jpeg")),
        _ => {
            let decoded = image::load_from_memory(&bytes).ok()?;
            let mut png = std::io::Cursor::new(Vec::new());
            decoded.write_to(&mut png, image::ImageFormat::Png).ok()?;
            Some((png.into_inner(), "image/png"))
        }
    }
}

const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UINT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// A glTF binary being assembled: JSON arrays plus one buffer.
#[derive(Default)]
struct Glb {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    images: Vec<Value>,
    textures: Vec<Value>,
    materials: Vec<Value>,
    meshes: Vec<Value>,
    skins: Vec<Value>,
    animations: Vec<Value>,
}

impl Glb {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
        });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(view);
        self.views.len() - 1
    }

    fn accessor(&mut self, view: usize, component: u32, count: usize, kind: &str) -> usize {
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": kind,
        }));
        self.accessors.len() - 1
    }

    /// A float accessor over `values` (`width` floats per element), with min and max.
    fn floats(&mut self, values: &[f32], width: usize, kind: &str) -> usize {
        let bytes: Vec<u8> = values.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, None);
        let accessor = self.accessor(view, FLOAT, values.len() / width, kind);
        let mut min = vec![f32::INFINITY; width];
        let mut max = vec![f32::NEG_INFINITY; width];
        for element in values.chunks(width) {
            for k in 0..width {
                min[k] = min[k].min(element[k]);
                max[k] = max[k].max(element[k]);
            }
        }
        self.accessors[accessor]["min"] = json!(min);
        self.accessors[accessor]["max"] = json!(max);
        accessor
    }

    /// One primitive: position, normal and uv per vertex (interleaved as 8 floats), and bone
    /// joints and weights for a skinned mesh.
    fn primitive(
        &mut self,
        vertices: &[[f32; 8]],
        skin: Option<&[([u16; 4], [f32; 4])]>,
        indices: &[u32],
        material: Option<usize>,
    ) -> Value {
        let mut attributes = serde_json::Map::new();
        for (name, range, kind) in [
            ("POSITION", 0..3, "VEC3"),
            ("NORMAL", 3..6, "VEC3"),
            ("TEXCOORD_0", 6..8, "VEC2"),
        ] {
            let bytes: Vec<u8> = vertices
                .iter()
                .flat_map(|v| v[range.clone()].iter().flat_map(|f| f.to_le_bytes()))
                .collect();
            let view = self.view(&bytes, Some(ARRAY_BUFFER));
            let accessor = self.accessor(view, FLOAT, vertices.len(), kind);
            if name == "POSITION" {
                let mut min = [f32::INFINITY; 3];
                let mut max = [f32::NEG_INFINITY; 3];
                for v in vertices {
                    for k in 0..3 {
                        min[k] = min[k].min(v[k]);
                        max[k] = max[k].max(v[k]);
                    }
                }
                self.accessors[accessor]["min"] = json!(min);
                self.accessors[accessor]["max"] = json!(max);
            }
            attributes.insert(name.into(), json!(accessor));
        }
        if let Some(skin) = skin {
            let bytes: Vec<u8> = skin
                .iter()
                .flat_map(|(j, _)| j.iter().flat_map(|i| i.to_le_bytes()))
                .collect();
            let view = self.view(&bytes, Some(ARRAY_BUFFER));
            let joints = self.accessor(view, UNSIGNED_SHORT, skin.len(), "VEC4");
            let bytes: Vec<u8> = skin
                .iter()
                .flat_map(|(_, w)| w.iter().flat_map(|f| f.to_le_bytes()))
                .collect();
            let view = self.view(&bytes, Some(ARRAY_BUFFER));
            let weights = self.accessor(view, FLOAT, skin.len(), "VEC4");
            attributes.insert("JOINTS_0".into(), json!(joints));
            attributes.insert("WEIGHTS_0".into(), json!(weights));
        }
        let bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
        let index_accessor = self.accessor(view, UINT, indices.len(), "SCALAR");
        let mut primitive = json!({ "attributes": attributes, "indices": index_accessor });
        if let Some(m) = material {
            primitive["material"] = json!(m);
        }
        primitive
    }

    /// A baked take as a glTF animation: translation, rotation and scale channels for every
    /// node that moves, with linear interpolation between the baked keys.
    fn animation(
        &mut self,
        name: &str,
        baked: &ufbx::BakedAnim,
        node_index: &HashMap<u32, usize>,
    ) -> Option<Value> {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        let mut times: HashMap<Vec<u32>, usize> = HashMap::new();
        let mut add = |glb: &mut Glb, node: usize, path: &str, keys: Vec<(f64, Vec<f64>)>| {
            if keys.is_empty() {
                return;
            }
            let t: Vec<f32> = keys.iter().map(|k| k.0 as f32).collect();
            // Tracks baked at the same rate share one time accessor.
            let input = *times
                .entry(t.iter().map(|x| x.to_bits()).collect())
                .or_insert_with(|| glb.floats(&t, 1, "SCALAR"));
            let width = keys[0].1.len();
            let values: Vec<f32> = keys
                .iter()
                .flat_map(|k| k.1.iter().map(|&x| x as f32))
                .collect();
            let output = glb.floats(&values, width, if width == 4 { "VEC4" } else { "VEC3" });
            samplers.push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
            channels.push(json!({
                "sampler": samplers.len() - 1,
                "target": { "node": node, "path": path },
            }));
        };
        for track in &baked.nodes {
            let Some(&node) = node_index.get(&track.typed_id) else {
                continue;
            };
            let vec3 = |keys: &[ufbx::BakedVec3]| -> Vec<(f64, Vec<f64>)> {
                keys.iter()
                    .map(|k| (k.time, vec![k.value.x, k.value.y, k.value.z]))
                    .collect()
            };
            add(self, node, "translation", vec3(&track.translation_keys));
            add(
                self,
                node,
                "rotation",
                track
                    .rotation_keys
                    .iter()
                    .map(|k| (k.time, vec![k.value.x, k.value.y, k.value.z, k.value.w]))
                    .collect(),
            );
            add(self, node, "scale", vec3(&track.scale_keys));
        }
        if channels.is_empty() {
            return None;
        }
        Some(json!({ "name": name, "samplers": samplers, "channels": channels }))
    }

    /// An embedded image and a texture that samples it; returns the texture index.
    fn image(&mut self, bytes: &[u8], mime: &str) -> usize {
        let view = self.view(bytes, None);
        self.images
            .push(json!({ "bufferView": view, "mimeType": mime }));
        self.textures
            .push(json!({ "source": self.images.len() - 1 }));
        self.textures.len() - 1
    }

    fn write(
        mut self,
        out: &Path,
        nodes: Vec<Value>,
        roots: Vec<usize>,
        name: &str,
    ) -> Result<(), String> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut doc = json!({
            "asset": { "version": "2.0", "generator": "engine editor import (ufbx)" },
            "scene": 0,
            "scenes": [{ "name": name, "nodes": roots }],
            "nodes": nodes,
            "buffers": [{ "byteLength": self.bin.len() }],
            "bufferViews": self.views,
            "accessors": self.accessors,
        });
        for (key, list) in [
            ("meshes", self.meshes),
            ("materials", self.materials),
            ("textures", self.textures),
            ("images", self.images),
            ("skins", self.skins),
            ("animations", self.animations),
        ] {
            if !list.is_empty() {
                doc[key] = Value::Array(list);
            }
        }
        let mut json = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + self.bin.len();
        let mut file = Vec::with_capacity(total);
        file.extend_from_slice(b"glTF");
        file.extend_from_slice(&2u32.to_le_bytes());
        file.extend_from_slice(&(total as u32).to_le_bytes());
        file.extend_from_slice(&(json.len() as u32).to_le_bytes());
        file.extend_from_slice(b"JSON");
        file.extend_from_slice(&json);
        file.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        file.extend_from_slice(b"BIN\0");
        file.extend_from_slice(&self.bin);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(out, file).map_err(|e| format!("{}: {e}", out.display()))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn obj_becomes_a_glb_the_gltf_reader_accepts() {
        let dir = std::env::temp_dir().join(format!("editor-convert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A unit quad (two triangles once triangulated) with a material.
        std::fs::write(dir.join("quad.mtl"), "newmtl Red\nKd 1 0 0\n").unwrap();
        std::fs::write(
            dir.join("quad.obj"),
            "mtllib quad.mtl\no Quad\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
             vn 0 0 1\nusemtl Red\nf 1/1/1 2/2/1 3/3/1 4/4/1\n",
        )
        .unwrap();
        let out = dir.join("quad.glb");
        let summary = to_glb(&dir.join("quad.obj"), &out).unwrap();
        assert_eq!(summary.meshes, 1);
        assert_eq!(summary.triangles, 2);
        assert_eq!(summary.materials, 1);

        let (doc, buffers, _) = gltf::import(&out).expect("a valid glb");
        let mesh = doc.meshes().next().unwrap();
        let prim = mesh.primitives().next().unwrap();
        let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
        assert_eq!(
            reader.read_positions().unwrap().count(),
            4,
            "corners shared"
        );
        assert_eq!(reader.read_indices().unwrap().into_u32().count(), 6);
        let color = prim.material().pbr_metallic_roughness().base_color_factor();
        assert_eq!(color[..3], [1.0, 0.0, 0.0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A two-bone bar written by hand in ASCII FBX (1 m units): the lower two corners on the
    /// root bone, the upper two on the tip bone 1 m up, and a one-second take bending the tip
    /// 90° about Z.
    pub(crate) const BENDING_BAR: &str = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
    FBXHeaderVersion: 1003
    FBXVersion: 7400
}
GlobalSettings:  {
    Version: 1000
    Properties70:  {
        P: "UpAxis", "int", "Integer", "",1
        P: "UpAxisSign", "int", "Integer", "",1
        P: "FrontAxis", "int", "Integer", "",2
        P: "FrontAxisSign", "int", "Integer", "",1
        P: "CoordAxis", "int", "Integer", "",0
        P: "CoordAxisSign", "int", "Integer", "",1
        P: "UnitScaleFactor", "double", "Number", "",100
    }
}
Objects:  {
    Geometry: 1001, "Geometry::Bar", "Mesh" {
        Vertices: *12 {
            a: -0.1,0,0,0.1,0,0,0.1,2,0,-0.1,2,0
        }
        PolygonVertexIndex: *4 {
            a: 0,1,2,-4
        }
    }
    Model: 2001, "Model::Bar", "Mesh" {
        Version: 232
    }
    Model: 3001, "Model::Root", "LimbNode" {
        Version: 232
    }
    Model: 3002, "Model::Tip", "LimbNode" {
        Version: 232
        Properties70:  {
            P: "Lcl Translation", "Lcl Translation", "", "A",0,1,0
        }
    }
    Deformer: 4001, "Deformer::Skin", "Skin" {
        Version: 101
    }
    Deformer: 4002, "SubDeformer::Root", "Cluster" {
        Version: 100
        Indexes: *2 {
            a: 0,1
        }
        Weights: *2 {
            a: 1,1
        }
        Transform: *16 {
            a: 1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1
        }
        TransformLink: *16 {
            a: 1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1
        }
    }
    Deformer: 4003, "SubDeformer::Tip", "Cluster" {
        Version: 100
        Indexes: *2 {
            a: 2,3
        }
        Weights: *2 {
            a: 1,1
        }
        Transform: *16 {
            a: 1,0,0,0,0,1,0,0,0,0,1,0,0,-1,0,1
        }
        TransformLink: *16 {
            a: 1,0,0,0,0,1,0,0,0,0,1,0,0,1,0,1
        }
    }
    AnimationStack: 5001, "AnimStack::Bend", "" {
        Properties70:  {
            P: "LocalStop", "KTime", "Time", "",46186158000
        }
    }
    AnimationLayer: 5002, "AnimLayer::Base", "" {
    }
    AnimationCurveNode: 5003, "AnimCurveNode::R", "" {
        Properties70:  {
            P: "d|X", "Number", "", "A",0
            P: "d|Y", "Number", "", "A",0
            P: "d|Z", "Number", "", "A",0
        }
    }
    AnimationCurve: 5004, "AnimCurve::", "" {
        Default: 0
        KeyVer: 4009
        KeyTime: *2 {
            a: 0,46186158000
        }
        KeyValueFloat: *2 {
            a: 0,90
        }
        KeyAttrFlags: *1 {
            a: 264
        }
        KeyAttrDataFloat: *4 {
            a: 0,0,0,0
        }
        KeyAttrRefCount: *1 {
            a: 2
        }
    }
}
Connections:  {
    C: "OO",2001,0
    C: "OO",1001,2001
    C: "OO",3001,0
    C: "OO",3002,3001
    C: "OO",4001,1001
    C: "OO",4002,4001
    C: "OO",4003,4001
    C: "OO",3001,4002
    C: "OO",3002,4003
    C: "OO",5002,5001
    C: "OO",5003,5002
    C: "OP",5003,3002, "Lcl Rotation"
    C: "OP",5004,5003, "d|Z"
}
"#;

    #[test]
    fn fbx_skins_and_takes_play_through_the_anim_importer() {
        let dir = std::env::temp_dir().join(format!("editor-convert-skin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bar.fbx"), BENDING_BAR).unwrap();
        let out = dir.join("bar.glb");
        let summary = to_glb(&dir.join("bar.fbx"), &out).unwrap();
        assert!(
            summary.notes.contains(&"skinned to 2 bones".to_string()),
            "{summary:?}"
        );
        assert!(
            summary.notes.contains(&"1 animations".to_string()),
            "{summary:?}"
        );

        // A valid glTF whose skinned mesh sits in world bind space on a root node.
        let (doc, buffers, _) = gltf::import(&out).expect("a valid glb");
        let skin = doc.skins().next().unwrap();
        assert_eq!(skin.joints().count(), 2);
        let mesh_node = doc.nodes().find(|n| n.skin().is_some()).unwrap();
        assert!(
            doc.scenes()
                .next()
                .unwrap()
                .nodes()
                .any(|n| n.index() == mesh_node.index())
        );
        let prim = mesh_node.mesh().unwrap().primitives().next().unwrap();
        let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
        let positions: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
        let joints: Vec<[u16; 4]> = reader.read_joints(0).unwrap().into_u16().collect();
        let tip = skin.joints().position(|j| j.name() == Some("Tip")).unwrap() as u16;
        for (p, j) in positions.iter().zip(&joints) {
            assert_eq!(
                j[0] == tip,
                p[1] > 1.0,
                "upper corners ride on the tip: {p:?} {j:?}"
            );
        }
        let animation = doc.animations().next().unwrap();
        assert_eq!(animation.name(), Some("Bend"));
        // The tip ends the take turned 90° about Z.
        let bend = animation
            .channels()
            .find(|c| {
                c.target().node().name() == Some("Tip")
                    && c.target().property() == gltf::animation::Property::Rotation
            })
            .unwrap();
        let r = bend.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
        let gltf::animation::util::ReadOutputs::Rotations(rotations) = r.read_outputs().unwrap()
        else {
            panic!("rotation keys");
        };
        let last = rotations.into_f32().last().unwrap();
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (last[2].abs() - half).abs() < 1e-3 && (last[3].abs() - half).abs() < 1e-3,
            "{last:?}"
        );

        // The anim crate's importer takes it: two bones, one clip a second long.
        let bytes = std::fs::read(&out).unwrap();
        let model = engine_anim::import::import_gltf(
            &bytes,
            Some(&dir),
            &engine_anim::import::ImportOptions {
                yaw_deg: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(model.skeleton.len(), 2);
        assert_eq!(model.clips.len(), 1);
        assert!(
            (model.clips[0].duration - 1.0).abs() < 0.05,
            "{}",
            model.clips[0].duration
        );
        let top = model.mesh.bounds().max.y;
        assert!((top - 2.0).abs() < 1e-3, "bind pose kept: {top}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

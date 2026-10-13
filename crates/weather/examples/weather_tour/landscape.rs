//! Builds the landscape around the station: textures, terrain chunks, trees, rocks, grass and
//! the farm track, and registers them as meshes, images and materials.
//!
//! Level of detail is static, by distance from the station (every ground-level shot is taken
//! within ~20 m of it; the high-altitude shot looks at ground kilometers away): full 3D trees
//! within ~170 m, camera-facing impostor cards (baked from the same trees) out to 3 km, and
//! the baked terrain colors beyond.

use std::sync::Arc;
use std::time::Instant;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::Vertex;
use engine_render::prelude::*;
use engine_world::{Ground, SurfaceMaterials};

use crate::noise::{Rng, smoothstep};
use crate::rocks::{self, Boulder};
use crate::terrain::{self, GroundPaint, MACRO_A_HALF, MACRO_B_HALF, RingMaterial};
use crate::texgen::{self, TextureSet};
use crate::trees::{self, BakeInput, Broadleaf, Conifer, ImpostorFrame, TreeMeshes};

/// Totals reported after building.
#[derive(Default, Debug, Clone, Copy)]
pub struct Budget {
    pub triangles: usize,
    pub texels: usize,
    pub entities: usize,
}

/// Registers assets in the world and counts what was spawned.
pub struct Registry<'w> {
    pub world: &'w mut World,
    pub budget: Budget,
}

/// Handles of a material's images.
#[derive(Clone, Copy)]
pub struct TexHandles {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub orm: Handle<Image>,
}

impl Registry<'_> {
    pub fn image(&mut self, image: Image) -> Handle<Image> {
        self.budget.texels += (image.width * image.height) as usize;
        self.world.resource_mut::<Assets<Image>>().add(image)
    }

    pub fn textures(&mut self, set: TextureSet) -> TexHandles {
        TexHandles {
            albedo: self.image(set.albedo),
            normal: self.image(set.normal),
            orm: self.image(set.orm),
        }
    }

    pub fn mesh(&mut self, mesh: Mesh) -> Handle<Mesh> {
        self.world.resource_mut::<Assets<Mesh>>().add(mesh)
    }

    pub fn material(&mut self, material: StandardMaterial) -> Handle<StandardMaterial> {
        self.world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material)
    }

    /// Spawn one instance of a mesh.
    pub fn spawn(
        &mut self,
        transform: Transform,
        mesh: Handle<Mesh>,
        material: Handle<StandardMaterial>,
        shadows: bool,
    ) -> Entity {
        let tris = self
            .world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .map_or(0, Mesh::triangle_count);
        self.budget.triangles += tris;
        self.budget.entities += 1;
        let mut renderer = MeshRenderer::new(mesh, material);
        renderer.cast_shadows = shadows;
        self.world.spawn((transform, renderer)).id()
    }
}

/// A textured material from a texture set.
pub fn textured(t: TexHandles) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 1.0,
        base_color_texture: Some(t.albedo),
        normal_texture: Some(t.normal),
        metallic_roughness_texture: Some(t.orm),
        occlusion_texture: Some(t.orm),
        ..Default::default()
    }
}

/// Foliage: alpha-tested, translucent, swaying. Single-sided (cards carry both windings).
fn foliage(t: TexHandles, sway: f32) -> StandardMaterial {
    StandardMaterial {
        alpha_mode: AlphaMode::Mask(0.5),
        subsurface: 0.55,
        wind_sway: sway,
        normal_scale: 0.8,
        ..textured(t)
    }
}

struct Paint {
    grass_mean: Vec3,
    /// The track's center line (coarse), for the worn grass along it.
    track: Vec<DVec2>,
}

impl Paint {
    /// Vertex tint that turns the meadow texture into the ground color at a point.
    fn tint(&self, x: f64, z: f64, h: f64, n: Vec3) -> Vec3 {
        let mut albedo = terrain::ground_look(x, z, h, n).albedo;
        // Worn, browner grass along the track (only where it can be near).
        if x.abs() < 60.0 && (-20.0..360.0).contains(&z) {
            let d = track_distance(&self.track, x, z);
            let wear = smoothstep(2.8, 1.3, d) as f32;
            albedo = albedo.lerp(Vec3::new(0.095, 0.085, 0.045), wear * 0.55);
        }
        albedo / self.grass_mean
    }
}

impl GroundPaint for Paint {
    fn paint(&self, material: RingMaterial, x: f64, z: f64, h: f64, n: Vec3) -> (Vec2, Vec4) {
        match material {
            RingMaterial::Near => {
                let tile = texgen::GRASS_TILE_M as f64;
                (
                    Vec2::new((x / tile) as f32, (z / tile) as f32),
                    self.tint(x, z, h, n).extend(1.0),
                )
            }
            RingMaterial::MacroA => (macro_uv(x, z, MACRO_A_HALF), Vec4::ONE),
            RingMaterial::MacroB => (macro_uv(x, z, MACRO_B_HALF), Vec4::ONE),
        }
    }
}

fn macro_uv(x: f64, z: f64, half: f64) -> Vec2 {
    Vec2::new(
        ((x + half) / (2.0 * half)) as f32,
        ((z + half) / (2.0 * half)) as f32,
    )
}

fn stage(name: &str, started: &mut Instant) {
    println!("  {name}: {:.2} s", started.elapsed().as_secs_f32());
    *started = Instant::now();
}

/// Ground position (on the rendered terrain surface) at x, z.
fn ground(x: f64, z: f64) -> DVec3 {
    DVec3::new(x, terrain::rendered_height(x, z), z)
}

// ------------------------------------------------------------------ the farm track

/// Center line of the farm track, from the station's south edge out across the meadow.
const TRACK: [(f64, f64); 11] = [
    (-1.0, 8.6),
    (-2.2, 16.0),
    (-6.5, 28.0),
    (-9.0, 44.0),
    (-4.5, 66.0),
    (5.0, 92.0),
    (11.0, 125.0),
    (7.0, 165.0),
    (-6.0, 215.0),
    (-12.0, 270.0),
    (-8.0, 330.0),
];

/// Points every `step` m along the track (Catmull-Rom through `TRACK`).
fn track_points(step: f64) -> Vec<DVec2> {
    let k: Vec<DVec2> = TRACK.iter().map(|&(x, z)| DVec2::new(x, z)).collect();
    let mut fine = Vec::new();
    for i in 0..k.len() - 1 {
        let p0 = k[i.saturating_sub(1)];
        let (p1, p2) = (k[i], k[i + 1]);
        let p3 = k[(i + 2).min(k.len() - 1)];
        for s in 0..40 {
            let t = s as f64 / 40.0;
            let (t2, t3) = (t * t, t * t * t);
            fine.push(
                (p1 * 2.0
                    + (p2 - p0) * t
                    + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2
                    + (p3 - p0 + (p1 - p2) * 3.0) * t3)
                    * 0.5,
            );
        }
    }
    fine.push(k[k.len() - 1]);
    // Resample evenly.
    let mut out = vec![fine[0]];
    let mut carried = 0.0;
    for w in fine.windows(2) {
        let seg = (w[1] - w[0]).length();
        let mut d = step - carried;
        while d <= seg {
            out.push(w[0] + (w[1] - w[0]) * (d / seg));
            d += step;
        }
        carried = seg - (d - step);
    }
    out
}

/// Horizontal distance from (x, z) to the track's center line.
fn track_distance(points: &[DVec2], x: f64, z: f64) -> f64 {
    let p = DVec2::new(x, z);
    points
        .windows(2)
        .map(|w| {
            let d = w[1] - w[0];
            let t = ((p - w[0]).dot(d) / d.length_squared().max(1e-9)).clamp(0.0, 1.0);
            (p - (w[0] + d * t)).length()
        })
        .fold(f64::MAX, f64::min)
}

/// The track as a ribbon lying on the terrain (3 cm above it), U across, V along.
fn track_mesh(points: &[DVec2], origin: DVec3) -> Mesh {
    let mut mesh = Mesh::default();
    let width = texgen::TRACK_WIDTH_M as f64;
    let across = [0.0, 0.25, 0.5, 0.75, 1.0];
    let mut v = 0.0;
    for (i, p) in points.iter().enumerate() {
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(points.len() - 1)];
        let dir = (b - a).normalize_or(DVec2::Y);
        let right = DVec2::new(-dir.y, dir.x);
        if i > 0 {
            v += (points[i] - points[i - 1]).length();
        }
        // Fade out at the far end.
        let fade = 1.0 - smoothstep(points.len() as f64 * 0.8, points.len() as f64, i as f64);
        for &u in &across {
            let q = *p + right * ((u - 0.5) * width);
            let h = terrain::rendered_height(q.x, q.y) + 0.03;
            let n = terrain::normal(q.x, q.y, 0.75);
            let pos = (DVec3::new(q.x, h, q.y) - origin).as_vec3();
            mesh.vertices.push(
                Vertex::new(
                    pos,
                    n,
                    Vec2::new(u as f32, (v / texgen::TRACK_TILE_M as f64) as f32),
                )
                .with_color(Vec4::new(1.0, 1.0, 1.0, fade as f32)),
            );
        }
    }
    let row = across.len() as u32;
    for i in 0..points.len() as u32 - 1 {
        for j in 0..row - 1 {
            let a = i * row + j;
            // Rows run along +V; keep the triangles facing up.
            mesh.indices
                .extend_from_slice(&[a, a + 1, a + row + 1, a, a + row + 1, a + row]);
        }
    }
    fix_winding_up(&mut mesh);
    mesh
}

/// Flip triangles whose geometric normal points down (the ribbon's handedness depends on
/// the direction of travel).
fn fix_winding_up(mesh: &mut Mesh) {
    for tri in mesh.indices.as_chunks_mut::<3>().0 {
        let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
        if n.y < 0.0 {
            tri.swap(1, 2);
        }
    }
}

// ------------------------------------------------------------------ grass tufts

/// A star of three bent grass cards, `size` m tall, at `at` (relative to the chunk origin).
fn add_tuft(mesh: &mut Mesh, at: Vec3, size: f32, rng: &mut Rng, tint: Vec3) {
    let variant = if rng.chance(0.3) { 1 } else { 0 };
    let (v0, v1) = (variant as f32 * 0.5, variant as f32 * 0.5 + 0.5);
    let rot = rng.f32() * std::f32::consts::TAU;
    let width = size * rng.range(1.1, 1.6);
    for k in 0..3 {
        let a = rot + k as f32 * std::f32::consts::PI / 3.0 + rng.signed() * 0.3;
        let face = Vec3::new(a.cos(), 0.0, a.sin());
        let lean = Vec3::new(rng.signed(), 0.0, rng.signed()) * 0.25;
        let up = (Vec3::Y + lean).normalize();
        let u_off = rng.f32() * 0.5;
        let shade = rng.range(0.85, 1.12);
        // Grass shades like the ground under it: normals mostly up.
        let n = (Vec3::Y + face * 0.2).normalize();
        let base = mesh.vertices.len() as u32;
        let right = up.cross(face).normalize();
        for (row, (h, v)) in [(0.0, v1), (1.0, v0)].into_iter().enumerate() {
            let c = at + up * (size * h) + face * (0.1 * size * h);
            let ao = 0.72 + 0.4 * row as f32;
            let col = (tint * shade * ao).extend(1.0);
            for (side, u) in [(-0.5f32, u_off), (0.5, u_off + 0.5)] {
                mesh.vertices.push(
                    Vertex::new(c + right * (width * side), n, Vec2::new(u, v)).with_color(col),
                );
            }
        }
        trees::two_sided(mesh, &[base, base + 1, base + 3, base, base + 3, base + 2]);
    }
}

// ------------------------------------------------------------------ build

/// A distant tree: position, template, scale, mirrored, brightness.
type FarTree = (DVec3, usize, f32, bool, f32);

struct Template {
    meshes: TreeMeshes,
    wood_material: usize,
    leaf_material: usize,
}

/// Build everything. Returns the totals.
pub fn build(world: &mut World) -> Budget {
    let all = Instant::now();
    let mut t = Instant::now();

    // The weather and everything else read the terrain through the Ground contract.
    let (grass_id, rock_id) = {
        let m = world.resource::<SurfaceMaterials>();
        (m.id_or_default("grass"), m.id_or_default("rock"))
    };
    world.insert_resource(Ground(Arc::new(terrain::TerrainGround {
        grass: grass_id,
        rock: rock_id,
    })));
    // The sky draws the planet beyond the terrain in this color.
    world.resource_mut::<Atmosphere>().ground_color = [0.045, 0.06, 0.03];

    // ---- textures
    let leaf_green = [
        [0.045, 0.085, 0.020],
        [0.070, 0.115, 0.030],
        [0.14, 0.15, 0.045],
    ];
    let leaf_birch = [
        [0.075, 0.12, 0.030],
        [0.11, 0.16, 0.040],
        [0.20, 0.19, 0.06],
    ];
    let leaf_bush = [
        [0.030, 0.060, 0.018],
        [0.050, 0.085, 0.025],
        [0.10, 0.11, 0.035],
    ];
    let (meadow, macro_a, macro_b, sets) = std::thread::scope(|s| {
        let meadow = s.spawn(|| texgen::meadow(1024));
        let others = s.spawn(move || {
            (
                texgen::bark(512, 1024),
                texgen::birch_bark(512, 1024),
                texgen::leaf_cluster(512, 3, leaf_green),
                texgen::leaf_cluster(512, 4, leaf_birch),
                texgen::leaf_cluster(512, 5, leaf_bush),
                texgen::conifer_frond(256, 512, 6),
                texgen::rock(1024),
                texgen::grass_blades(512, 512),
                texgen::track(256, 1024),
            )
        });
        let macro_a = terrain::bake_macro(MACRO_A_HALF, 2048);
        let macro_b = terrain::bake_macro(MACRO_B_HALF, 1024);
        (
            meadow.join().expect("meadow"),
            macro_a,
            macro_b,
            others.join().expect("textures"),
        )
    });
    let (bark, birch, leaves_a, leaves_b, leaves_c, frond, rock_tex, blades, track_tex) = sets;
    stage("textures", &mut t);

    // ---- tree templates
    let templates_spec: Vec<(&str, TreeMeshes, usize, usize)> = vec![
        (
            "oak",
            trees::broadleaf(&Broadleaf {
                seed: 11,
                height: 14.0,
                trunk_radius: 0.36,
                crown_base: 0.25,
                crown_width: 0.42,
                scaffolds: 8,
                scaffold_angle: 0.95,
                tropism: 0.15,
                card: 1.7,
                leafiness: 1.6,
                twigs: true,
                shell: 1.6,
                bush: false,
            }),
            0,
            0,
        ),
        (
            "lime",
            trees::broadleaf(&Broadleaf {
                seed: 12,
                height: 12.0,
                trunk_radius: 0.28,
                crown_base: 0.22,
                crown_width: 0.36,
                scaffolds: 7,
                scaffold_angle: 0.8,
                tropism: 0.3,
                card: 1.5,
                leafiness: 1.8,
                twigs: true,
                shell: 1.7,
                bush: false,
            }),
            0,
            0,
        ),
        (
            "birch",
            trees::broadleaf(&Broadleaf {
                seed: 13,
                height: 13.0,
                trunk_radius: 0.17,
                crown_base: 0.3,
                crown_width: 0.24,
                scaffolds: 9,
                scaffold_angle: 0.6,
                tropism: -0.1,
                card: 1.1,
                leafiness: 2.2,
                twigs: true,
                shell: 1.5,
                bush: false,
            }),
            1,
            1,
        ),
        (
            "spruce",
            trees::conifer(&Conifer {
                seed: 14,
                height: 19.0,
                trunk_radius: 0.3,
                crown_base: 0.1,
                width: 0.2,
                whorl: 0.5,
                branches_per_whorl: 6,
                droop: 0.4,
                comb: 0.7,
            }),
            0,
            3,
        ),
        (
            "fir",
            trees::conifer(&Conifer {
                seed: 15,
                height: 13.0,
                trunk_radius: 0.22,
                crown_base: 0.06,
                width: 0.24,
                whorl: 0.42,
                branches_per_whorl: 6,
                droop: 0.12,
                comb: 0.0,
            }),
            0,
            3,
        ),
        (
            "bush",
            trees::broadleaf(&Broadleaf {
                seed: 16,
                height: 2.6,
                trunk_radius: 0.05,
                crown_base: 0.05,
                crown_width: 0.6,
                scaffolds: 7,
                scaffold_angle: 0.7,
                tropism: 0.2,
                card: 0.95,
                leafiness: 2.0,
                twigs: false,
                shell: 2.2,
                bush: true,
            }),
            0,
            2,
        ),
    ];
    for (name, m, _, _) in &templates_spec {
        println!(
            "    {name}: {} wood + {} leaf triangles",
            m.wood.triangle_count(),
            m.leaves.triangle_count()
        );
    }
    stage("trees", &mut t);

    // ---- impostors for distant trees (oak, lime, birch, spruce, fir; two views each)
    const CELL_W: usize = 256;
    const CELL_H: usize = 512;
    let bark_images = [&bark.albedo, &birch.albedo];
    let leaf_images = [
        &leaves_a.albedo,
        &leaves_b.albedo,
        &leaves_c.albedo,
        &frond.albedo,
    ];
    let impostor_templates = [0usize, 1, 2, 3, 4];
    let mut frames: Vec<ImpostorFrame> = Vec::new();
    let jobs: Vec<(usize, f32)> = impostor_templates
        .iter()
        .flat_map(|&k| [(k, 0.0f32), (k, 1.9)])
        .collect();
    for &k in &impostor_templates {
        let (_, m, _, _) = &templates_spec[k];
        frames.push(trees::impostor_frame(
            m.radius,
            m.height,
            CELL_W as f32 / CELL_H as f32,
        ));
    }
    let cells = texgen::par_map(jobs.len(), |j| {
        let (k, yaw) = jobs[j];
        let (_, m, wood_mat, leaf_mat) = &templates_spec[k];
        let inputs = [
            BakeInput {
                mesh: &m.wood,
                albedo: bark_images[*wood_mat],
                masked: false,
                uv_scale: Vec2::ONE,
            },
            BakeInput {
                mesh: &m.leaves,
                albedo: leaf_images[*leaf_mat],
                masked: true,
                uv_scale: Vec2::ONE,
            },
        ];
        let frame = frames[impostor_templates.iter().position(|&x| x == k).unwrap()];
        trees::bake_impostor(&inputs, frame, yaw, CELL_W, CELL_H)
    });
    let atlas = trees::impostor_atlas(&cells, CELL_W, CELL_H);
    let atlas_cells = cells.len();
    stage("impostors", &mut t);

    let mut reg = Registry {
        world,
        budget: Budget::default(),
    };

    // ---- terrain
    let paint = Paint {
        grass_mean: meadow.mean,
        track: track_points(1.0),
    };
    let blade_mean = blades.mean;
    let near = textured(reg.textures(meadow));
    let near = reg.material(near);
    let macro_material = |reg: &mut Registry, m: terrain::MacroTextures| {
        let handles = TexHandles {
            albedo: reg.image(m.albedo),
            normal: reg.image(m.normal),
            orm: reg.image(m.orm),
        };
        let mat = textured(handles);
        reg.material(mat)
    };
    let far_a = macro_material(&mut reg, macro_a);
    let far_b = macro_material(&mut reg, macro_b);
    let chunks = terrain::chunks();
    let meshes = texgen::par_map(chunks.len(), |i| terrain::chunk_mesh(&chunks[i], &paint));
    for (chunk, (mesh, center)) in chunks.iter().zip(meshes) {
        let material = match chunk.ring.material() {
            RingMaterial::Near => near,
            RingMaterial::MacroA => far_a,
            RingMaterial::MacroB => far_b,
        };
        let handle = reg.mesh(mesh);
        // The terrain receives shadows but doesn't cast them (the meadow is gentle and the
        // hills are beyond the shadow distance); it would double the shadow passes' cost.
        reg.spawn(Transform::from_translation(center), handle, material, false);
    }
    stage("terrain", &mut t);

    // ---- materials
    const SWAY: f32 = 0.18;
    let wood_materials = [
        StandardMaterial {
            wind_sway: SWAY,
            ..textured(reg.textures(bark))
        },
        StandardMaterial {
            wind_sway: SWAY,
            ..textured(reg.textures(birch))
        },
    ]
    .map(|m| reg.material(m));
    let leaf_materials = [
        foliage(reg.textures(leaves_a), SWAY),
        foliage(reg.textures(leaves_b), SWAY),
        foliage(reg.textures(leaves_c), SWAY * 0.5),
        StandardMaterial {
            subsurface: 0.35,
            ..foliage(reg.textures(frond), SWAY * 0.6)
        },
    ]
    .map(|m| reg.material(m));
    let impostor_material = {
        let h = reg.textures(atlas);
        let m = StandardMaterial {
            alpha_mode: AlphaMode::Mask(0.5),
            subsurface: 0.4,
            ..textured(h)
        };
        reg.material(m)
    };
    let rock_material = {
        let h = reg.textures(rock_tex);
        reg.material(textured(h))
    };
    let tuft_material = {
        let h = reg.textures(blades);
        let m = StandardMaterial {
            alpha_mode: AlphaMode::Mask(0.45),
            subsurface: 0.4,
            wind_sway: 0.06,
            normal_scale: 0.5,
            // Upward normals would turn wet blades into mirrors of the sky.
            receives_wetness: false,
            ..textured(h)
        };
        reg.material(m)
    };
    let track_material = {
        let h = reg.textures(track_tex);
        let m = StandardMaterial {
            alpha_mode: AlphaMode::Mask(0.5),
            ..textured(h)
        };
        reg.material(m)
    };

    // ---- hero trees (full 3D near the station)
    let templates: Vec<Template> = templates_spec
        .into_iter()
        .map(|(_, meshes, wood_material, leaf_material)| Template {
            meshes,
            wood_material,
            leaf_material,
        })
        .collect();
    let handles: Vec<(Handle<Mesh>, Handle<Mesh>)> = templates
        .iter()
        .map(|tp| {
            (
                reg.mesh(tp.meshes.wood.clone()),
                reg.mesh(tp.meshes.leaves.clone()),
            )
        })
        .collect();
    // Shadow maps are the expensive part of foliage on a software rasterizer: only trees
    // whose shadows the cameras can see cast them (within ~115 m; bushes within 60 m).
    let place_tree = |reg: &mut Registry, kind: usize, x: f64, z: f64, scale: f32, yaw: f32| {
        let tp = &templates[kind];
        let at = ground(x, z);
        let tr = Transform::from_translation(at)
            .with_rotation(Quat::from_rotation_y(yaw))
            .with_scale(Vec3::splat(scale));
        let reach = if kind == 5 { 60.0 } else { 115.0 };
        let shadows = x.hypot(z) < reach;
        reg.spawn(
            tr,
            handles[kind].0,
            wood_materials[tp.wood_material],
            shadows,
        );
        reg.spawn(
            tr,
            handles[kind].1,
            leaf_materials[tp.leaf_material],
            shadows,
        );
    };
    const OAK: usize = 0;
    const LIME: usize = 1;
    const BIRCH: usize = 2;
    const SPRUCE: usize = 3;
    const FIR: usize = 4;
    const BUSH: usize = 5;
    // Composed for the shots (cameras stand south of the station, +Z, looking north-ish to
    // −Z; the sunset and the storm are to the west, +X).
    let hero: &[(usize, f64, f64, f32)] = &[
        (OAK, -30.0, -36.0, 1.1),
        (LIME, 44.0, -30.0, 1.0),
        (BIRCH, 24.0, -50.0, 0.95),
        (BIRCH, 30.0, -57.0, 1.05),
        (BIRCH, 19.0, -60.0, 0.85),
        (SPRUCE, -58.0, -72.0, 1.0),
        (SPRUCE, -49.0, -82.0, 1.1),
        (FIR, -64.0, -86.0, 1.0),
        (FIR, -41.0, -94.0, 0.9),
        (OAK, 86.0, 16.0, 0.95),
        (SPRUCE, 72.0, -62.0, 1.0),
        (FIR, 80.0, -70.0, 0.9),
        (LIME, -70.0, 18.0, 1.1),
        (OAK, 5.0, -105.0, 1.2),
        (LIME, -12.0, -128.0, 1.0),
        (BIRCH, 62.0, 48.0, 1.0),
        (SPRUCE, 58.0, 58.0, 0.9),
        (LIME, -95.0, -40.0, 1.0),
        (OAK, 110.0, -95.0, 1.1),
        (FIR, -110.0, -110.0, 1.0),
    ];
    let mut rng = Rng::new(2024);
    for &(kind, x, z, s) in hero.iter() {
        let yaw = rng.f32() * std::f32::consts::TAU;
        place_tree(&mut reg, kind, x, z, s, yaw);
    }
    // A hedgerow behind the station: bushes with a few trees in it.
    let (a, b) = (DVec2::new(-95.0, -55.0), DVec2::new(55.0, -88.0));
    let len = (b - a).length();
    let mut d = 0.0;
    while d < len {
        let p =
            a + (b - a) * (d / len) + DVec2::new(rng.signed() as f64, rng.signed() as f64) * 0.8;
        if rng.chance(0.08) {
            let kind = if rng.chance(0.5) { LIME } else { BIRCH };
            place_tree(
                &mut reg,
                kind,
                p.x,
                p.y,
                rng.range(0.7, 0.95),
                rng.f32() * std::f32::consts::TAU,
            );
        } else {
            place_tree(
                &mut reg,
                BUSH,
                p.x,
                p.y,
                rng.range(0.8, 1.35),
                rng.f32() * std::f32::consts::TAU,
            );
        }
        d += rng.range(2.2, 3.4) as f64;
    }
    // Bushes along the track and scattered.
    let track = track_points(0.5);
    for i in (30..track.len()).step_by(23) {
        let p = track[i];
        let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
        let off = DVec2::new(side * rng.range(2.6, 4.5) as f64, rng.signed() as f64);
        if p.length() < 25.0 {
            continue;
        }
        place_tree(
            &mut reg,
            BUSH,
            p.x + off.x,
            p.y + off.y,
            rng.range(0.7, 1.2),
            rng.f32() * std::f32::consts::TAU,
        );
    }
    stage("hero trees", &mut t);

    // ---- distant trees: impostor cards facing the station, merged per 500 m cell
    let spacing = |r: f64| -> f64 {
        if r < 800.0 {
            8.0
        } else if r < 1600.0 {
            12.0
        } else {
            18.0
        }
    };
    const FAR_MIN: f64 = 170.0;
    const FAR_MAX: f64 = 3000.0;
    let rows: Vec<f64> = {
        let mut v = Vec::new();
        let mut z = -FAR_MAX;
        while z < FAR_MAX {
            v.push(z);
            z += 7.0;
        }
        v
    };
    // Candidates on a jittered grid (coarser further out), accepted by forest density.
    let found: Vec<Vec<FarTree>> = texgen::par_map(rows.len(), |ri| {
        let z0 = rows[ri];
        let mut out = Vec::new();
        let mut x = -FAR_MAX;
        let mut r = Rng::new(ri as u64 * 7919 + 1);
        while x < FAR_MAX {
            let s = spacing((x * x + z0 * z0).sqrt());
            // Only keep this row on the coarser grids where it lines up.
            let row_ok =
                ((z0 + FAR_MAX) / 7.0).round() as i64 % (s / 7.0).round().max(1.0) as i64 == 0;
            if !row_ok {
                x += s;
                continue;
            }
            let px = x + r.signed() as f64 * s * 0.45;
            let pz = z0 + r.signed() as f64 * s * 0.45;
            x += s;
            let rr = (px * px + pz * pz).sqrt();
            if !(FAR_MIN..FAR_MAX).contains(&rr) {
                continue;
            }
            let h = terrain::height(px, pz);
            let n = terrain::normal(px, pz, 3.0);
            let density = terrain::forest_density(px, pz, h, n);
            // Lone trees in the open, forests where the density is high.
            // Far away the baked forest color carries the forest; cards add the texture of
            // the canopy and the tree line.
            let keep = 1.0 - 0.55 * smoothstep(900.0, 2500.0, rr);
            let mut p = (density * 0.9 * keep).max(0.004 * smoothstep(150.0, 400.0, rr));
            // Hedgerow trees along the field borders.
            let farm = terrain::farmland_share(px, pz, h, n, density);
            if farm > 0.3 {
                let (_, border) = terrain::farmland(px, pz);
                if border < 3.5 {
                    p = p.max(0.22 * farm * (1.0 + terrain::world_noise(px, pz, 80.0, 9)));
                }
            }
            if r.f32() as f64 >= p {
                continue;
            }
            let conifer_share = smoothstep(
                380.0,
                800.0,
                h + 120.0 * terrain::world_noise(px, pz, 300.0, 3),
            );
            let kind = if r.f32() < conifer_share as f32 {
                if r.chance(0.6) { 3 } else { 4 }
            } else {
                [0, 1, 2, 0, 1][(r.next_u32() % 5) as usize]
            };
            let scale = r.range(0.8, 1.2) * if rr > 1600.0 { 1.25 } else { 1.0 };
            let y = terrain::rendered_height(px, pz);
            out.push((
                DVec3::new(px, y, pz),
                kind,
                scale,
                r.chance(0.5),
                r.range(0.85, 1.15),
            ));
        }
        out
    });
    let mut cells_map: std::collections::HashMap<(i64, i64), Vec<FarTree>> =
        std::collections::HashMap::new();
    let mut far_count = 0;
    for list in found {
        for item in list {
            let key = (
                (item.0.x / 500.0).floor() as i64,
                (item.0.z / 500.0).floor() as i64,
            );
            cells_map.entry(key).or_default().push(item);
            far_count += 1;
        }
    }
    for ((cx, cz), items) in cells_map {
        let origin = DVec3::new(cx as f64 * 500.0 + 250.0, 0.0, cz as f64 * 500.0 + 250.0);
        let mut mesh = Mesh::default();
        for (p, kind, scale, mirror, tone) in items {
            let local = (p - origin).as_vec3();
            let toward = (-p).as_vec3();
            let view = if mirror { 1 } else { 0 };
            let tint = Vec4::new(tone * 1.02, tone, tone * 0.95, 1.0);
            trees::add_billboard(
                &mut mesh,
                local,
                toward,
                frames[kind],
                scale,
                kind * 2 + view,
                atlas_cells,
                mirror,
                tint,
            );
        }
        let handle = reg.mesh(mesh);
        // Sun shadows reach 350 m: only cells that close cast them.
        let near_cell = origin.length() < 350.0 + 360.0;
        reg.spawn(
            Transform::from_translation(origin),
            handle,
            impostor_material,
            near_cell,
        );
    }
    stage(&format!("distant trees ({far_count})"), &mut t);

    // ---- rocks
    let rock_templates: Vec<Handle<Mesh>> = (0..5)
        .map(|k| {
            let m = rocks::boulder(&Boulder {
                seed: 100 + k,
                size: Vec3::new(2.0, 1.2 + 0.3 * (k % 3) as f32, 1.6),
                detail: if k < 3 { 4 } else { 3 },
                moss: 0.8,
            });
            reg.mesh(m)
        })
        .collect();
    let rock_spots: &[(f64, f64, f32)] = &[
        (-22.0, -14.0, 1.3),
        (-24.5, -11.5, 0.6),
        (20.0, -20.0, 0.9),
        (-35.0, -30.0, 1.8),
        (-27.0, -40.0, 0.7),
        (38.0, -42.0, 1.1),
        (35.0, 12.0, 1.4),
        (33.0, 14.5, 0.6),
        (-40.0, 22.0, 1.0),
        (60.0, -12.0, 2.2),
        (63.0, -9.0, 0.9),
        (-60.0, -50.0, 2.6),
        (15.0, -75.0, 1.5),
        (95.0, 30.0, 3.0),
        (-85.0, 0.0, 2.0),
        (-15.0, 38.0, 0.8),
        (8.0, 30.0, 0.5),
    ];
    for (i, &(x, z, s)) in rock_spots.iter().enumerate() {
        let tr = Transform::from_translation(ground(x, z) - DVec3::Y * (0.05 * s as f64))
            .with_rotation(Quat::from_rotation_y(rng.f32() * std::f32::consts::TAU))
            .with_scale(Vec3::new(s, s * rng.range(0.8, 1.1), s));
        reg.spawn(
            tr,
            rock_templates[i % rock_templates.len()],
            rock_material,
            true,
        );
    }
    // Small stones scattered along the track edges and in the meadow.
    for _ in 0..70 {
        let i = (rng.next_u32() as usize) % track.len().min(260);
        let p = track[i];
        let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
        let q = p + DVec2::new(side * rng.range(1.7, 3.0) as f64, rng.signed() as f64);
        if q.x.abs() < 14.0 && q.y.abs() < 9.0 {
            continue;
        }
        let s = rng.range(0.12, 0.35);
        let tr = Transform::from_translation(ground(q.x, q.y))
            .with_rotation(Quat::from_rotation_y(rng.f32() * std::f32::consts::TAU))
            .with_scale(Vec3::splat(s));
        reg.spawn(
            tr,
            rock_templates[3 + (rng.next_u32() % 2) as usize],
            rock_material,
            true,
        );
    }
    stage("rocks", &mut t);

    // ---- the farm track
    let track_origin = DVec3::new(track[0].x, 0.0, track[0].y);
    let tm = track_mesh(&track, track_origin);
    let th = reg.mesh(tm);
    reg.spawn(
        Transform::from_translation(track_origin),
        th,
        track_material,
        false,
    );

    // ---- grass tufts near the station, merged per 10 m cell
    let tuft_paint = |x: f64, z: f64| -> Vec3 {
        let h = terrain::height(x, z);
        let n = terrain::normal(x, z, 0.5);
        terrain::ground_look(x, z, h, n).albedo / blade_mean
    };
    const TUFT_R: f64 = 42.0;
    let mut tufts = 0;
    let cell = 10.0;
    let n_cells = (TUFT_R / cell).ceil() as i32;
    for cz in -n_cells..n_cells {
        for cx in -n_cells..n_cells {
            let origin = DVec3::new(
                cx as f64 * cell + cell * 0.5,
                0.0,
                cz as f64 * cell + cell * 0.5,
            );
            let mut mesh = Mesh::default();
            let mut r = Rng::new((cx * 131 + cz * 977 + 5000) as u64);
            let count = 190;
            for _ in 0..count {
                let x = origin.x + r.signed() as f64 * cell * 0.5;
                let z = origin.z + r.signed() as f64 * cell * 0.5;
                let rr = (x * x + z * z).sqrt();
                // Denser near the station, thinning out by 48 m.
                if r.f32() as f64 > 1.0 - smoothstep(22.0, TUFT_R, rr) * 0.85 || rr > TUFT_R {
                    continue;
                }
                // Not on the station's paving or in the wheel ruts.
                if x.abs() < 13.6 && z.abs() < 8.6 {
                    continue;
                }
                let on_track = if rr < 140.0 {
                    track_distance(&track[..track.len().min(300)], x, z)
                } else {
                    f64::MAX
                };
                // Off the bare soil of the track, apart from a few in its middle strip.
                if on_track < 1.35 && (on_track > 0.35 || r.chance(0.6)) {
                    continue;
                }
                // Patchy: some clumps of grass are taller.
                let patch = terrain::world_noise(x, z, 3.0, 17) * 0.5 + 0.5;
                let trampled = if on_track < 0.4 { 0.55 } else { 1.0 };
                let size = r.range(0.22, 0.42) * (0.75 + 0.6 * patch as f32) * trampled;
                let y = terrain::rendered_height(x, z);
                let at = (DVec3::new(x, y - 0.02, z) - origin).as_vec3();
                add_tuft(&mut mesh, at, size, &mut r, tuft_paint(x, z));
                tufts += 1;
            }
            if !mesh.indices.is_empty() {
                let h = reg.mesh(mesh);
                reg.spawn(Transform::from_translation(origin), h, tuft_material, false);
            }
        }
    }
    stage(&format!("grass ({tufts} tufts)"), &mut t);

    let budget = reg.budget;
    println!(
        "landscape: {} triangles, {} entities, {:.1} M texels, built in {:.2} s",
        budget.triangles,
        budget.entities,
        budget.texels as f64 / 1e6,
        all.elapsed().as_secs_f32()
    );
    budget
}

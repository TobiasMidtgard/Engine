//! Thumbnails for the Content panel's models, the way Unity's Project window, Unreal's Content
//! Browser and Godot's FileSystem dock show assets: each file is drawn once on a worker thread
//! (Unity's `AssetPreview` and Godot's `EditorResourcePreview` also make previews off the UI
//! thread) and cached on disk keyed by the file's path, size and modification time, so the
//! next session shows it at once and an edited file gets a new picture.
//!
//! The picture is a small software rasterizer over the glTF file: every mesh in the default
//! scene with its base color factor and texture, alpha cutouts, lit by a key light and a sky
//! fill, seen from the front three-quarter view (glTF faces +Z) through an orthographic
//! camera fitted to the model, 2× supersampled.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use engine::prelude::*;
use image::RgbaImage;

/// Why a file has no picture when it only holds animations.
const NOTHING_TO_DRAW: &str = "nothing to draw";

/// Picture edge in pixels (tiles draw it at whatever size the panel asks).
pub const SIZE: u32 = 128;

/// A picture the worker finished (or why it couldn't), by asset key.
type Finished = (String, Result<RgbaImage, String>);

/// The thumbnails made so far and the worker making the rest.
#[derive(Resource)]
pub struct Thumbs {
    /// By asset key: the texture, or `None` when the file couldn't be drawn.
    ready: HashMap<String, Option<egui::TextureHandle>>,
    asked: HashSet<String>,
    /// Animation-only files (nothing to draw): shown with a clip icon.
    clips: HashSet<String>,
    to_worker: Option<Sender<(String, PathBuf)>>,
    /// (A resource must be `Sync`; only the UI ever reads it.)
    from_worker: Option<Mutex<Receiver<Finished>>>,
    /// Tile edge in the Content panel, points.
    pub tile: f32,
}

impl Default for Thumbs {
    fn default() -> Self {
        Self {
            ready: HashMap::new(),
            asked: HashSet::new(),
            clips: HashSet::new(),
            to_worker: None,
            from_worker: None,
            tile: 72.0,
        }
    }
}

impl Thumbs {
    /// The thumbnail of `key` (the file at `path`), asking the worker for it the first time.
    pub fn get(&mut self, key: &str, path: &Path) -> Option<&egui::TextureHandle> {
        if !self.asked.contains(key) {
            self.asked.insert(key.to_string());
            if self.to_worker.is_none() {
                let (to_worker, jobs) = channel::<(String, PathBuf)>();
                let (done, from_worker) = channel();
                std::thread::Builder::new()
                    .name("thumbnails".into())
                    .spawn(move || {
                        for (key, path) in jobs {
                            if done.send((key, cached(&path))).is_err() {
                                break;
                            }
                        }
                    })
                    .ok()?;
                self.to_worker = Some(to_worker);
                self.from_worker = Some(Mutex::new(from_worker));
            }
            if let Some(tx) = &self.to_worker {
                let _ = tx.send((key.to_string(), path.to_path_buf()));
            }
        }
        self.ready.get(key).and_then(|t| t.as_ref())
    }

    /// Turn finished pictures into textures (once a frame, from the UI).
    pub fn receive(&mut self, ctx: &egui::Context) {
        let Some(Ok(rx)) = self.from_worker.as_mut().map(Mutex::get_mut) else {
            return;
        };
        for (key, result) in rx.try_iter() {
            let texture = match result {
                Ok(img) => Some(ctx.load_texture(
                    format!("thumb:{key}"),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [img.width() as usize, img.height() as usize],
                        img.as_raw(),
                    ),
                    egui::TextureOptions::LINEAR,
                )),
                Err(e) if e == NOTHING_TO_DRAW => {
                    self.clips.insert(key.clone());
                    None
                }
                Err(e) => {
                    engine::core::log::warn!("thumbnail of {key}: {e}");
                    None
                }
            };
            self.ready.insert(key, texture);
        }
    }

    /// Whether `key` turned out to hold only animations.
    pub fn is_clip(&self, key: &str) -> bool {
        self.clips.contains(key)
    }

    /// Whether pictures are still being made.
    pub fn busy(&self) -> bool {
        self.asked.len() > self.ready.len()
    }

    /// Forget `key` so its picture is made again (the file changed).
    pub fn forget(&mut self, key: &str) {
        self.asked.remove(key);
        self.ready.remove(key);
    }
}

/// Where thumbnails are kept between sessions: the user's cache folder.
fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("engine-editor").join("thumbnails")
}

/// The cached picture of `path`, made and stored when there is none for this version of the
/// file.
fn cached(path: &Path) -> Result<RgbaImage, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .hash(&mut hash);
    meta.len().hash(&mut hash);
    meta.modified().ok().hash(&mut hash);
    SIZE.hash(&mut hash);
    let file = cache_dir().join(format!("{:016x}.png", hash.finish()));
    if let Ok(img) = image::open(&file) {
        return Ok(img.to_rgba8());
    }
    let img = render(path, SIZE)?;
    if std::fs::create_dir_all(cache_dir()).is_ok() {
        let _ = img.save(&file);
    }
    Ok(img)
}

/// A material as the rasterizer sees it.
struct Surface {
    color: Vec4,
    texture: Option<usize>,
    cutoff: Option<f32>,
}

/// A decoded texture, RGBA8.
struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<[u8; 4]>,
}

impl Texture {
    fn new(data: &gltf::image::Data) -> Option<Self> {
        use gltf::image::Format;
        let n = (data.width * data.height) as usize;
        let pixels: Vec<[u8; 4]> = match data.format {
            Format::R8G8B8A8 => data.pixels.as_chunks::<4>().0.to_vec(),
            Format::R8G8B8 => data
                .pixels
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            Format::R8 => data.pixels.iter().map(|&v| [v, v, v, 255]).collect(),
            Format::R8G8 => data
                .pixels
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            _ => return None,
        };
        (pixels.len() == n).then_some(Self {
            width: data.width as usize,
            height: data.height as usize,
            pixels,
        })
    }

    /// Linear color at `uv` (repeat wrap, nearest: the picture is far smaller than the image).
    fn sample(&self, uv: Vec2) -> Vec4 {
        let x = ((uv.x.rem_euclid(1.0)) * self.width as f32) as usize;
        let y = ((uv.y.rem_euclid(1.0)) * self.height as f32) as usize;
        let p = self.pixels[y.min(self.height - 1) * self.width + x.min(self.width - 1)];
        Vec4::new(
            srgb_to_linear(p[0]),
            srgb_to_linear(p[1]),
            srgb_to_linear(p[2]),
            p[3] as f32 / 255.0,
        )
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// One triangle in world space.
struct Tri {
    p: [Vec3; 3],
    n: [Vec3; 3],
    uv: [Vec2; 3],
    surface: usize,
}

/// Draw the glTF file at `path` as a `size`² picture with a transparent background.
pub fn render(path: &Path, size: u32) -> Result<RgbaImage, String> {
    let (doc, buffers, images) =
        gltf::import(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let textures: Vec<Option<Texture>> = images.iter().map(Texture::new).collect();
    let mut surfaces: Vec<Surface> = doc
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            Surface {
                color: Vec4::from(pbr.base_color_factor()),
                texture: pbr
                    .base_color_texture()
                    .map(|t| t.texture().source().index())
                    .filter(|&i| textures.get(i).is_some_and(|t| t.is_some())),
                cutoff: (m.alpha_mode() == gltf::material::AlphaMode::Mask)
                    .then(|| m.alpha_cutoff().unwrap_or(0.5)),
            }
        })
        .collect();
    let default_surface = surfaces.len();
    surfaces.push(Surface {
        color: Vec4::new(0.8, 0.8, 0.8, 1.0),
        texture: None,
        cutoff: None,
    });

    // Every triangle of the default scene, in world space.
    let mut tris = Vec::new();
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .ok_or("the file has no scene")?;
    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let global = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            // A skinned mesh is drawn in its bind pose, which glTF puts in the skin's space
            // rather than under its node.
            let m = if node.skin().is_some() {
                Mat4::IDENTITY
            } else {
                global
            };
            let nm = Mat3::from_mat4(m).inverse().transpose();
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let r = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(positions) = r.read_positions() else {
                    continue;
                };
                let p: Vec<Vec3> = positions
                    .map(|v| m.transform_point3(Vec3::from(v)))
                    .collect();
                let n: Vec<Vec3> = r
                    .read_normals()
                    .map(|it| {
                        it.map(|v| (nm * Vec3::from(v)).normalize_or_zero())
                            .collect()
                    })
                    .unwrap_or_default();
                let uv: Vec<Vec2> = r
                    .read_tex_coords(0)
                    .map(|it| it.into_f32().map(Vec2::from).collect())
                    .unwrap_or_default();
                let indices: Vec<u32> = r
                    .read_indices()
                    .map(|it| it.into_u32().collect())
                    .unwrap_or_else(|| (0..p.len() as u32).collect());
                let surface = prim.material().index().unwrap_or(default_surface);
                for t in indices.as_chunks::<3>().0 {
                    let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
                    if a.max(b).max(c) >= p.len() {
                        continue;
                    }
                    let face = (p[b] - p[a]).cross(p[c] - p[a]).normalize_or_zero();
                    let normal = |i: usize| n.get(i).copied().unwrap_or(face);
                    let tex = |i: usize| uv.get(i).copied().unwrap_or(Vec2::ZERO);
                    tris.push(Tri {
                        p: [p[a], p[b], p[c]],
                        n: [normal(a), normal(b), normal(c)],
                        uv: [tex(a), tex(b), tex(c)],
                        surface,
                    });
                }
            }
        }
        stack.extend(node.children().map(|c| (c, global)));
    }
    if tris.is_empty() {
        return Err(NOTHING_TO_DRAW.into());
    }
    Ok(rasterize(&tris, &surfaces, &textures, size))
}

/// Draw the triangles from the front three-quarter view, fitted to the picture.
fn rasterize(
    tris: &[Tri],
    surfaces: &[Surface],
    textures: &[Option<Texture>],
    size: u32,
) -> RgbaImage {
    let ss = (size * 2) as usize;
    // Camera: in front (+Z), a little to the right and above, looking at the model.
    let back = Vec3::new(0.55, 0.45, 1.0).normalize();
    let right = Vec3::Y.cross(back).normalize();
    let up = back.cross(right);
    let view = |p: Vec3| Vec3::new(p.dot(right), p.dot(up), p.dot(back));
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for t in tris {
        for p in t.p {
            let v = view(p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    let extent = (hi - lo).truncate().max_element().max(1e-6);
    let scale = ss as f32 * 0.88 / extent;
    let mid = (lo + hi) * 0.5;
    let to_screen = |p: Vec3| {
        let v = view(p);
        Vec3::new(
            (v.x - mid.x) * scale + ss as f32 * 0.5,
            (mid.y - v.y) * scale + ss as f32 * 0.5,
            v.z,
        )
    };
    let light = Vec3::new(-0.45, 0.8, 0.55).normalize();
    let mut depth = vec![f32::NEG_INFINITY; ss * ss];
    let mut color = vec![Vec4::ZERO; ss * ss];
    for t in tris {
        let s = t.p.map(to_screen);
        let area = (s[1].x - s[0].x) * (s[2].y - s[0].y) - (s[2].x - s[0].x) * (s[1].y - s[0].y);
        if area.abs() < 1e-9 {
            continue;
        }
        let x0 = s
            .iter()
            .map(|v| v.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let x1 = (s
            .iter()
            .map(|v| v.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as usize)
            .min(ss - 1);
        let y0 = s
            .iter()
            .map(|v| v.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let y1 = (s
            .iter()
            .map(|v| v.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as usize)
            .min(ss - 1);
        if x0 > x1 || y0 > y1 {
            continue;
        }
        let surface = &surfaces[t.surface.min(surfaces.len() - 1)];
        let texture = surface.texture.and_then(|i| textures[i].as_ref());
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((s[1].x - px) * (s[2].y - py) - (s[2].x - px) * (s[1].y - py)) / area;
                let w1 = ((s[2].x - px) * (s[0].y - py) - (s[0].x - px) * (s[2].y - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = s[0].z * w0 + s[1].z * w1 + s[2].z * w2;
                let i = y * ss + x;
                if z <= depth[i] {
                    continue;
                }
                let uv = t.uv[0] * w0 + t.uv[1] * w1 + t.uv[2] * w2;
                let mut albedo = surface.color;
                if let Some(tex) = texture {
                    albedo *= tex.sample(uv);
                }
                if surface.cutoff.is_some_and(|c| albedo.w < c) {
                    continue;
                }
                let mut n = (t.n[0] * w0 + t.n[1] * w1 + t.n[2] * w2).normalize_or(Vec3::Y);
                // Both sides lit (open meshes and double-sided cards).
                if n.dot(back) < 0.0 {
                    n = -n;
                }
                let key = n.dot(light).max(0.0);
                let sky = 0.5 + 0.5 * n.y;
                let rim = (1.0 - n.dot(back).max(0.0)).powi(3) * 0.25;
                let lit = albedo.truncate() * (0.95 * key + 0.35 * sky) + Vec3::splat(rim);
                depth[i] = z;
                color[i] = lit.extend(1.0);
            }
        }
    }
    // 2×2 down to the picture, alpha-weighted.
    let mut img = RgbaImage::new(size, size);
    for y in 0..size as usize {
        for x in 0..size as usize {
            let mut sum = Vec4::ZERO;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let c = color[(y * 2 + dy) * ss + x * 2 + dx];
                sum += Vec4::new(c.x * c.w, c.y * c.w, c.z * c.w, c.w);
            }
            let a = sum.w / 4.0;
            let rgb = if sum.w > 0.0 {
                sum.truncate() / sum.w
            } else {
                Vec3::ZERO
            };
            img.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    (linear_to_srgb(rgb.x) * 255.0).round() as u8,
                    (linear_to_srgb(rgb.y) * 255.0).round() as u8,
                    (linear_to_srgb(rgb.z) * 255.0).round() as u8,
                    (a * 255.0).round() as u8,
                ]),
            );
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_obj_quad_draws_red_in_the_middle_and_clear_around() {
        let dir = std::env::temp_dir().join(format!("editor-thumbs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("quad.mtl"), "newmtl Red\nKd 1 0 0\n").unwrap();
        std::fs::write(
            dir.join("quad.obj"),
            "mtllib quad.mtl\no Quad\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvn 0 0 1\n\
             usemtl Red\nf 1//1 2//1 3//1 4//1\n",
        )
        .unwrap();
        crate::convert::to_glb(&dir.join("quad.obj"), &dir.join("quad.glb")).unwrap();
        let img = render(&dir.join("quad.glb"), 32).unwrap();
        let middle = img.get_pixel(16, 16);
        assert_eq!(middle[3], 255, "covered");
        assert!(
            middle[0] > 100 && middle[1] < 30 && middle[2] < 30,
            "lit red: {middle:?}"
        );
        assert_eq!(img.get_pixel(0, 0)[3], 0, "the corner stays clear");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

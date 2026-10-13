//! The fighters: armored sword fighters in four [`Style`]s, built from procedural parts.
//!
//! Every part is a rigid mesh on one [`Bone`] of the [rig](super::rig). Parts are modeled
//! in the bind pose (character space: feet at the origin, facing −Z) from a few shapes:
//! body volumes (superellipse sections stacked along an axis, so armor plates can be laid
//! over the undersuit at a small offset), helmet shells (superellipsoids), thick plates with
//! rounded rims, lofted fingers, and the katana's hand-built blade. Everything on the same
//! bone with the same material is merged into one mesh, registered once per style under
//! `movement/fighter/<style>/<bone>/<material>`.
//!
//! Materials are baked PBR texture sets (painted armor with chipped edges and scratches,
//! carbon fiber twill, rubberized undersuit with grip dots, woven cloth, leather, molded
//! rubber, iron mail, machined metal, polished and hardened steel, the handle's silk cord
//! and ray skin), plus emissive accents in the style's color.

use std::collections::{BTreeMap, HashMap};
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::OnceLock;

use engine_assets::Handle;
use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{Image, MeshRenderer, StandardMaterial};

use super::meshgen::{Cap, MeshBuilder, Section};
use super::rig::{self, BLADE_LENGTH, BONES, Bone, Xf};
use super::texgen::{
    Surface, TextureSet, bake, fbm, hex, perlin, rand2, ridged, smoothstep, value, worley,
};
use super::{Blade, Library, NoGhost};

/// How a fighter looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Style {
    /// The parkour runner: a tactical pilot in a plate carrier and cargo trousers, a close
    /// helmet with a narrow visor and a respirator, a jump kit, teal accents.
    Pilot,
    /// The player: a samurai in white and blue lacquer, a ribbed helmet with gilt horns over
    /// a sculpted face mask.
    Duelist,
    /// The elite: a samurai in black and crimson lacquer, a riveted helmet with a crimson
    /// crescent over a snarling demon mask with glowing eyes and fangs.
    Elite,
    /// A bulkier soldier: burnt-orange plates over a khaki carrier and trousers, a brimmed
    /// helmet over a rubber gas mask.
    Grunt,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Pilot, Style::Duelist, Style::Elite, Style::Grunt];

    pub fn key(self) -> &'static str {
        match self {
            Style::Pilot => "pilot",
            Style::Duelist => "duelist",
            Style::Elite => "elite",
            Style::Grunt => "grunt",
        }
    }

    /// Linear RGB of the style's glow: accents, visor, the blade's edge and its trail.
    pub fn glow(self) -> [f32; 3] {
        hex(self.palette().glow).to_array()
    }

    /// Linear RGB for afterimages and trails (a little lighter than the glow).
    pub fn trail(self) -> [f32; 3] {
        match self {
            Style::Pilot => [0.2, 1.0, 0.75],
            Style::Duelist => [0.3, 0.75, 1.0],
            Style::Elite => [1.0, 0.12, 0.18],
            Style::Grunt => [1.0, 0.5, 0.2],
        }
    }

    fn palette(self) -> Palette {
        match self {
            Style::Pilot => Palette {
                paint_a: (0x2e3337, 0.52),
                paint_b: (0x7b8085, 0.5),
                suit: 0x1b1e21,
                cloth: 0x3d4036,
                sash: 0x3a3d33,
                leather: 0x2b2621,
                trim: (0x6e7378, 0.38),
                glow: 0x2bffd2,
                glass: 0x05080a,
                ito: 0x141414,
                same: 0xcfd6d4,
                lacquer: (0x14181c, 0.18),
                wear: 0.8,
                glow_nits: 2200.0,
                edge_nits: 3500.0,
            },
            Style::Duelist => Palette {
                paint_a: (0xd5cfc1, 0.4),
                paint_b: (0x22305a, 0.42),
                suit: 0x1b2030,
                cloth: 0x1c233d,
                sash: 0x6f5a36,
                leather: 0x3a291d,
                trim: (0xb08a48, 0.32),
                glow: 0x8fdcff,
                glass: 0x04070c,
                ito: 0x1b3c8a,
                same: 0xece8dc,
                lacquer: (0x0d1838, 0.14),
                wear: 0.55,
                glow_nits: 2000.0,
                edge_nits: 3500.0,
            },
            Style::Elite => Palette {
                paint_a: (0x17171a, 0.4),
                paint_b: (0x5c0b14, 0.42),
                suit: 0x0e0e10,
                cloth: 0x1a191d,
                sash: 0x1f1d22,
                leather: 0x170f0f,
                trim: (0x2e2e33, 0.28),
                glow: 0xff1e30,
                glass: 0x030303,
                ito: 0x0b0b0b,
                same: 0x8a1020,
                lacquer: (0x5c0612, 0.12),
                wear: 0.35,
                glow_nits: 3200.0,
                edge_nits: 5000.0,
            },
            Style::Grunt => Palette {
                paint_a: (0x7e4b2e, 0.6),
                paint_b: (0x565a4e, 0.6),
                suit: 0x2c2e27,
                cloth: 0x5a5442,
                sash: 0x6a6450,
                leather: 0x4a3524,
                trim: (0x8a8d90, 0.45),
                glow: 0xff9a30,
                glass: 0x0a0806,
                ito: 0x3a2a1e,
                same: 0xd8d0c0,
                lacquer: (0x2a1c14, 0.3),
                wear: 1.0,
                glow_nits: 1600.0,
                edge_nits: 2200.0,
            },
        }
    }
}

/// Colors and finishes of a style.
struct Palette {
    /// Primary and secondary paint (sRGB hex, roughness).
    paint_a: (u32, f32),
    paint_b: (u32, f32),
    suit: u32,
    /// Woven cloth: hakama trousers.
    cloth: u32,
    /// The waist sash of the hakama styles.
    sash: u32,
    leather: u32,
    /// Fittings (sRGB hex, roughness): gold, steel, black chrome.
    trim: (u32, f32),
    glow: u32,
    glass: u32,
    /// Handle wrap cord and the ray skin under it.
    ito: u32,
    same: u32,
    /// Scabbard lacquer (sRGB hex, roughness).
    lacquer: (u32, f32),
    /// How worn the paint is (chips, scratches), 0..1.
    wear: f32,
    /// Emissive strength of accents and the blade's edge (cd/m²).
    glow_nits: f32,
    edge_nits: f32,
}

/// The materials a fighter is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mat {
    /// Primary painted armor.
    PaintA,
    /// Secondary painted armor.
    PaintB,
    /// Undersuit: a technical twill.
    Suit,
    /// Heavy woven cloth: hakama trousers.
    Cloth,
    /// Woven sash.
    Sash,
    Carbon,
    Leather,
    /// Machined metal fittings.
    Trim,
    /// Boot soles.
    Rubber,
    /// Molded rubber: a gas mask's face.
    Mask,
    /// Blackened iron mail sewn onto a cloth backing (the samurai's sleeves).
    Mail,
    /// Emissive accent lines.
    Glow,
    /// Dark glossy visor glass.
    Glass,
    /// Emissive visor and eye lenses.
    Lens,
    /// Polished blade steel.
    Steel,
    /// The frosted, hardened edge zone of the blade.
    Hamon,
    /// The blade's glowing cutting edge.
    Edge,
    /// The handle's cord wrap.
    Wrap,
    /// Ray skin under the wrap.
    Same,
    /// Scabbard lacquer.
    Lacquer,
}

impl Mat {
    pub const ALL: [Mat; 20] = [
        Mat::PaintA,
        Mat::PaintB,
        Mat::Suit,
        Mat::Cloth,
        Mat::Sash,
        Mat::Carbon,
        Mat::Leather,
        Mat::Trim,
        Mat::Rubber,
        Mat::Mask,
        Mat::Mail,
        Mat::Glow,
        Mat::Glass,
        Mat::Lens,
        Mat::Steel,
        Mat::Hamon,
        Mat::Edge,
        Mat::Wrap,
        Mat::Same,
        Mat::Lacquer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mat::PaintA => "paint_a",
            Mat::PaintB => "paint_b",
            Mat::Suit => "suit",
            Mat::Cloth => "cloth",
            Mat::Sash => "sash",
            Mat::Carbon => "carbon",
            Mat::Leather => "leather",
            Mat::Trim => "trim",
            Mat::Rubber => "rubber",
            Mat::Mask => "mask",
            Mat::Mail => "mail",
            Mat::Glow => "glow",
            Mat::Glass => "glass",
            Mat::Lens => "lens",
            Mat::Steel => "steel",
            Mat::Hamon => "hamon",
            Mat::Edge => "edge",
            Mat::Wrap => "wrap",
            Mat::Same => "same",
            Mat::Lacquer => "lacquer",
        }
    }

    /// The same for every style.
    fn shared(self) -> bool {
        matches!(
            self,
            Mat::Carbon | Mat::Rubber | Mat::Mask | Mat::Mail | Mat::Steel | Mat::Hamon
        )
    }

    /// Material asset key (without the `movement/` prefix).
    pub fn key(self, style: Style) -> String {
        if self.shared() {
            format!("fighter/{}", self.name())
        } else {
            format!("fighter/{}/{}", style.key(), self.name())
        }
    }

    /// Glows (no shadows, no need to be dense).
    pub fn emissive(self) -> bool {
        matches!(self, Mat::Glow | Mat::Lens | Mat::Edge)
    }
}

/// One mesh of a fighter: what bone it rides on, its mesh and material asset keys (without
/// the `movement/` prefix; see [`super::mesh`] and [`super::material`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub bone: Bone,
    pub mat: Mat,
    pub mesh: String,
    pub material: String,
}

/// The parts of a fighter of this style (meshes are not built; listed once per style).
pub fn parts(style: Style) -> &'static [Part] {
    static PARTS: OnceLock<[Vec<Part>; 4]> = OnceLock::new();
    let all = PARTS.get_or_init(|| {
        Style::ALL.map(|style| {
            let mut kit = Kit::new(style, false);
            describe(&mut kit);
            kit.parts
                .keys()
                .map(|&(bone, mat)| Part {
                    bone,
                    mat,
                    mesh: mesh_key(style, bone, mat),
                    material: mat.key(style),
                })
                .collect()
        })
    });
    &all[style as usize]
}

/// Builds every mesh of a style: (part, mesh). Used by [`generate`] and to count triangles.
pub fn build(style: Style) -> Vec<(Part, MeshBuilder)> {
    let mut kit = Kit::new(style, true);
    describe(&mut kit);
    kit.parts
        .into_iter()
        .filter(|(_, m)| !m.is_empty())
        .map(|((bone, mat), m)| {
            (
                Part {
                    bone,
                    mat,
                    mesh: mesh_key(style, bone, mat),
                    material: mat.key(style),
                },
                m,
            )
        })
        .collect()
}

/// Triangles in one fighter of this style.
pub fn triangle_count(style: Style) -> usize {
    build(style).iter().map(|(_, m)| m.triangle_count()).sum()
}

fn mesh_key(style: Style, bone: Bone, mat: Mat) -> String {
    format!("fighter/{}/{}/{}", style.key(), bone.name(), mat.name())
}

/// The look generator: registers every fighter mesh and material.
pub fn generate(lib: &mut Library) {
    materials(lib);
    for style in Style::ALL {
        for (part, mesh) in build(style) {
            lib.mesh(&part.mesh, mesh);
        }
    }
}

// ------------------------------------------------------------------------ spawning

/// The entities of a spawned fighter body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// One entity per bone, indexed by [`Bone::index`]. The first [`Bone::ANIMATED`] are
    /// children of the owner and are posed with [`bone_transform`]; the sword rides on the
    /// right hand and the blade (marked with [`Blade`]) on the sword.
    pub bones: [Entity; BONES],
    /// At the blade's point.
    pub tip: Entity,
}

/// A bone's local transform under the owner for a solved bone (character space), with the
/// feet `lift` m above the owner's origin (negative: below).
pub fn bone_transform(x: Xf, lift: f32) -> Transform {
    Transform::from_translation((x.pos + Vec3::Y * lift).as_dvec3()).with_rotation(x.rot)
}

/// Spawns a fighter's skeleton and every part of its armor and sword under `owner`, in the
/// bind pose, with the feet `lift` m above the owner's origin. The meshes and materials
/// come from the look library ([`generate`]).
pub fn spawn_body(commands: &mut Commands, owner: Entity, style: Style, lift: f32) -> Body {
    let bind = rig::solve(&rig::Pose::BIND);
    let mut bones = [Entity::PLACEHOLDER; BONES];
    for bone in Bone::ALL {
        let (parent, t) = match bone {
            Bone::Sword => (
                bones[Bone::HandR.index()],
                bone_transform(rig::grip_r(), 0.0),
            ),
            Bone::Blade => (
                bones[Bone::Sword.index()],
                bone_transform(Xf::at(Vec3::Z * rig::BLADE_BASE_Z), 0.0),
            ),
            b => (owner, bone_transform(bind[b.index()], lift)),
        };
        bones[bone.index()] = commands
            .spawn((Name::new(bone.name()), t, ChildOf(parent)))
            .id();
    }
    for part in parts(style) {
        let mut r = MeshRenderer::new(super::mesh(&part.mesh), super::material(&part.material));
        if part.mat.emissive() {
            r = r.without_shadows();
        }
        commands.spawn((Transform::default(), r, ChildOf(bones[part.bone.index()])));
    }
    let blade = bones[Bone::Blade.index()];
    let tip = commands
        .spawn((
            Name::new("blade_tip"),
            Transform::from_translation(rig::blade_tip().as_dvec3()),
            ChildOf(blade),
            NoGhost,
        ))
        .id();
    commands.entity(blade).insert(Blade {
        owner,
        tip,
        color: style.glow(),
    });
    Body { bones, tip }
}

// ------------------------------------------------------------------------ the kit

/// Collects parts by (bone, material). In manifest mode the mesh closures never run.
struct Kit {
    style: Style,
    build: bool,
    parts: BTreeMap<(Bone, Mat), MeshBuilder>,
}

impl Kit {
    fn new(style: Style, build: bool) -> Self {
        Self {
            style,
            build,
            parts: BTreeMap::new(),
        }
    }

    /// Adds a mesh already in `bone`'s own frame.
    fn local(&mut self, bone: Bone, mat: Mat, f: impl FnOnce() -> MeshBuilder) {
        let entry = self.parts.entry((bone, mat)).or_default();
        if self.build {
            let m = f();
            entry.append(&m, Mat4::IDENTITY);
        }
    }

    /// Adds a mesh modeled in the bind pose.
    fn bind(&mut self, bone: Bone, mat: Mat, f: impl FnOnce() -> MeshBuilder) {
        self.local(bone, mat, || {
            f().transformed(Mat4::from_translation(-bone.joint()))
        });
    }

    /// Adds a right-side mesh (bind pose) and its mirror image on the left bone.
    fn pair(&mut self, bone_r: Bone, mat: Mat, f: impl FnOnce() -> MeshBuilder) {
        let bone_l = bone_r.mirror();
        if !self.build {
            self.parts.entry((bone_r, mat)).or_default();
            self.parts.entry((bone_l, mat)).or_default();
            return;
        }
        let m = f();
        let mirrored = m.transformed(Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        self.bind(bone_r, mat, || m);
        self.bind(bone_l, mat, || mirrored);
    }
}

// ------------------------------------------------------------------------ shape helpers

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    smoothstep(a, b, x)
}

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// A transform from basis vectors and an origin.
fn basis(x: Vec3, y: Vec3, z: Vec3, o: Vec3) -> Mat4 {
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), o.extend(1.0))
}

/// `m` plus its mirror image across x = 0.
fn mirrored(m: MeshBuilder) -> MeshBuilder {
    let mut out = m.clone();
    out.append(&m, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
    out
}

/// Averages normals of vertices that share a position (seams, poles).
fn weld_normals(m: &mut MeshBuilder) {
    let mut acc: HashMap<(i32, i32, i32), Vec3> = HashMap::new();
    let key = |p: [f32; 3]| {
        let q = |x: f32| (x * 2e4).round() as i32;
        (q(p[0]), q(p[1]), q(p[2]))
    };
    for v in &m.vertices {
        *acc.entry(key(v.position)).or_default() += Vec3::from(v.normal);
    }
    for v in &mut m.vertices {
        if let Some(n) = acc.get(&key(v.position)) {
            v.normal = n.normalize_or(Vec3::from(v.normal)).to_array();
        }
    }
}

/// A smooth grid surface over `f(u, v)` (u, v in 0..1), `nu` × `nv` quads, facing
/// `∂f/∂u × ∂f/∂v`. UVs in meters along u and v.
fn surface(nu: usize, nv: usize, f: impl Fn(f32, f32) -> Vec3) -> MeshBuilder {
    let cols = nu + 1;
    let mut pts = Vec::with_capacity(cols * (nv + 1));
    for j in 0..=nv {
        for i in 0..=nu {
            pts.push(f(i as f32 / nu as f32, j as f32 / nv as f32));
        }
    }
    let mut us = vec![0.0f32; pts.len()];
    let mut vs = vec![0.0f32; pts.len()];
    for j in 0..=nv {
        for i in 1..=nu {
            let k = j * cols + i;
            us[k] = us[k - 1] + pts[k].distance(pts[k - 1]);
        }
    }
    for i in 0..=nu {
        for j in 1..=nv {
            let k = j * cols + i;
            vs[k] = vs[k - cols] + pts[k].distance(pts[k - cols]);
        }
    }
    let mut m = MeshBuilder::new();
    for k in 0..pts.len() {
        m.vertex(pts[k], Vec3::Y, Vec2::new(us[k], vs[k]));
    }
    for j in 0..nv {
        for i in 0..nu {
            let a = (j * cols + i) as u32;
            let c = cols as u32;
            m.quad(a, a + 1, a + c + 1, a + c);
        }
    }
    m.smooth_normals();
    weld_normals(&mut m);
    m.remove_degenerate();
    m
}

/// Thickness, pillow bulge and rim shading of a [`plate`].
#[derive(Debug, Clone, Copy)]
struct Pl {
    /// Thickness, m.
    t: f32,
    /// Extra height in the middle, m.
    bulge: f32,
    /// Vertex color multiplier on the rounded rim (worn, lighter or dirtier edges).
    edge: f32,
}

impl Pl {
    fn new(t: f32, bulge: f32, edge: f32) -> Self {
        Self { t, bulge, edge }
    }

    /// A thin strip (accent lines, inlays).
    fn strip() -> Self {
        Self::new(0.0022, 0.0, 1.0)
    }
}

/// Turns `m` (an open sheet) to face away from `inside`.
fn face_out(m: &mut MeshBuilder, inside: Vec3) {
    let Some(t) = m.indices.as_chunks::<3>().0.first().copied() else {
        return;
    };
    let p = t.map(|i| Vec3::from(m.vertices[i as usize].position));
    if (p[1] - p[0]).cross(p[2] - p[0]).dot(p[0] - inside) < 0.0 {
        m.flip();
    }
}

/// A thick plate lying on the surface `f(u, v)` (u, v in 0..1): its outer face sits
/// `pl.t` (+ bulge) out along the surface normal (away from `inside`), and its border is a
/// rounded bead that curls back down to the surface. Open underneath.
fn plate(nu: usize, nv: usize, pl: Pl, inside: Vec3, f: impl Fn(f32, f32) -> Vec3) -> MeshBuilder {
    let e = 1e-3;
    let grad = |u: f32, v: f32| {
        let du = f((u + e).min(1.0), v) - f((u - e).max(0.0), v);
        let dv = f(u, (v + e).min(1.0)) - f(u, (v - e).max(0.0));
        (du, dv)
    };
    let raw_n = |u: f32, v: f32| {
        let (du, dv) = grad(u, v);
        du.cross(dv).normalize_or(Vec3::Y)
    };
    let sgn = if raw_n(0.5, 0.5).dot(f(0.5, 0.5) - inside) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let nrm = |u: f32, v: f32| raw_n(u, v) * sgn;
    let lift = |u: f32, v: f32| pl.t + pl.bulge * (PI * u).sin() * (PI * v).sin();
    let mut m = surface(nu, nv, |u, v| f(u, v) + nrm(u, v) * lift(u, v));
    if sgn < 0.0 {
        m.flip();
    }
    // The border loop with the outward in-surface direction at each point.
    let mut border: Vec<(f32, f32, Vec3)> = Vec::new();
    let side = |u: f32, v: f32, which: u8| -> Vec3 {
        let (du, dv) = grad(u, v);
        let (du, dv) = (du.normalize_or(Vec3::X), dv.normalize_or(Vec3::Y));
        match which {
            0 => -dv,
            1 => du,
            2 => dv,
            _ => -du,
        }
    };
    let corner =
        |u: f32, v: f32, a: u8, b: u8| (side(u, v, a) + side(u, v, b)).normalize_or(Vec3::Y);
    for i in 0..=nu {
        let u = i as f32 / nu as f32;
        let d = match i {
            0 => corner(u, 0.0, 0, 3),
            _ if i == nu => corner(u, 0.0, 0, 1),
            _ => side(u, 0.0, 0),
        };
        border.push((u, 0.0, d));
    }
    for j in 0..=nv {
        let v = j as f32 / nv as f32;
        let d = match j {
            0 => corner(1.0, v, 0, 1),
            _ if j == nv => corner(1.0, v, 1, 2),
            _ => side(1.0, v, 1),
        };
        border.push((1.0, v, d));
    }
    for i in (0..=nu).rev() {
        let u = i as f32 / nu as f32;
        let d = match i {
            0 => corner(u, 1.0, 2, 3),
            _ if i == nu => corner(u, 1.0, 1, 2),
            _ => side(u, 1.0, 2),
        };
        border.push((u, 1.0, d));
    }
    for j in (0..=nv).rev() {
        let v = j as f32 / nv as f32;
        let d = match j {
            0 => corner(0.0, v, 0, 3),
            _ if j == nv => corner(0.0, v, 2, 3),
            _ => side(0.0, v, 3),
        };
        border.push((0.0, v, d));
    }
    const K: usize = 3;
    let mut rim = MeshBuilder::new();
    let mut along = 0.0;
    let mut prev: Option<Vec3> = None;
    for &(u, v, d) in &border {
        let p = f(u, v);
        let n = nrm(u, v);
        let edge = (d - n * d.dot(n)).normalize_or(d);
        let t = lift(u, v);
        if let Some(q) = prev {
            along += p.distance(q);
        }
        prev = Some(p);
        for k in 0..=K {
            let th = k as f32 / K as f32 * PI;
            let (s, c) = th.sin_cos();
            let pos = p + n * (t * 0.5 * (1.0 + c)) + edge * (t * 0.5 * s);
            let i = rim.vertex(pos, n * c + edge * s, Vec2::new(along, th * t * 0.5));
            let tint = 1.0 + (pl.edge - 1.0) * s;
            rim.vertices[i as usize].color = [tint, tint, tint, 1.0];
        }
    }
    let row = (K + 1) as u32;
    for b in 0..border.len() as u32 - 1 {
        for k in 0..K as u32 {
            let a = b * row + k;
            rim.quad(a, a + row, a + row + 1, a + 1);
        }
    }
    // Wind the rim to face along its normals.
    if let Some(t) = rim.indices.as_chunks::<3>().0.iter().find(|t| {
        let p = t.map(|i| Vec3::from(rim.vertices[i as usize].position));
        (p[1] - p[0]).cross(p[2] - p[0]).length_squared() > 1e-14
    }) {
        let p = t.map(|i| Vec3::from(rim.vertices[i as usize].position));
        let n = Vec3::from(rim.vertices[t[0] as usize].normal);
        if (p[1] - p[0]).cross(p[2] - p[0]).dot(n) < 0.0 {
            for t in rim.indices.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
    }
    rim.remove_degenerate();
    m.append(&rim, Mat4::IDENTITY);
    m
}

/// [`plate`] split into panels at the fractions `cu` (along u) and `cv` (along v), with gaps
/// of `gap` (u and v fractions) between neighbors, so the plate reads as several panels
/// with seams between them.
#[allow(clippy::too_many_arguments)]
fn split_plate(
    nu: usize,
    nv: usize,
    pl: Pl,
    inside: Vec3,
    cu: &[f32],
    cv: &[f32],
    gap: (f32, f32),
    f: impl Fn(f32, f32) -> Vec3,
) -> MeshBuilder {
    let spans = |cuts: &[f32], gap: f32| {
        let mut e = vec![0.0];
        e.extend_from_slice(cuts);
        e.push(1.0);
        e.windows(2)
            .map(|w| {
                let a = if w[0] > 0.0 { w[0] + gap * 0.5 } else { 0.0 };
                let b = if w[1] < 1.0 { w[1] - gap * 0.5 } else { 1.0 };
                (a, b)
            })
            .collect::<Vec<_>>()
    };
    let mut m = MeshBuilder::new();
    for &(u0, u1) in &spans(cu, gap.0) {
        for &(v0, v1) in &spans(cv, gap.1) {
            let cols = ((nu as f32 * (u1 - u0)).round() as usize).max(2);
            let rows = ((nv as f32 * (v1 - v0)).round() as usize).max(1);
            m.append(
                &plate(cols, rows, pl, inside, |uu, vv| {
                    f(lerp(u0, u1, uu), lerp(v0, v1, vv))
                }),
                Mat4::IDENTITY,
            );
        }
    }
    m
}

/// Hex bolt heads on a volume at (angle, axis) points, `g` out.
fn bolts(v: &Vol, at: &[(f32, f32)], g: f32) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    for &(phi, t) in at {
        let p = v.at(t, phi, g);
        let e = 1e-3;
        let du = v.at(t, phi + e, g) - v.at(t, phi - e, g);
        let dv = v.at(t + e, phi, g) - v.at(t - e, phi, g);
        let mut out = du.cross(dv).normalize_or(Vec3::NEG_Z);
        if out.dot(p - v.center(t)) < 0.0 {
            out = -out;
        }
        m.append(
            &disk(0.0042, 0.0026, out, p + out * 0.0008, 6),
            Mat4::IDENTITY,
        );
    }
    m
}

/// A row of slits (vents) on a volume, along the axis from `t0` at angle `phi`, `g` out.
fn vents(v: &Vol, phi: f32, t0: f32, n: usize, step: f32, g: f32, size: Vec2) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    for k in 0..n {
        let t = t0 + k as f32 * step;
        let p = v.at(t, phi, g);
        let e = 1e-3;
        let across = (v.at(t, phi + e, g) - v.at(t, phi - e, g)).normalize_or(Vec3::X);
        let along = (v.at(t + e, phi, g) - v.at(t - e, phi, g)).normalize_or(Vec3::Y);
        m.append(
            &MeshBuilder::chamfer_box(v3(size.x, size.y, 0.004), 0.0012).transformed(basis(
                across,
                along,
                across.cross(along),
                p,
            )),
            Mat4::IDENTITY,
        );
    }
    m
}

/// A body volume: superellipse sections along Y (or along Z for `Axis::Z`), interpolated
/// smoothly between keys. Key values: center (x, y-or-z), half width, front (or top) and
/// back (or bottom) half depth, exponent.
#[derive(Debug, Clone)]
struct Vol {
    keys: Vec<(f32, [f32; 6])>,
    along_z: bool,
}

impl Vol {
    fn y(keys: &[(f32, [f32; 6])]) -> Self {
        Self {
            keys: keys.to_vec(),
            along_z: false,
        }
    }

    fn z(keys: &[(f32, [f32; 6])]) -> Self {
        Self {
            keys: keys.to_vec(),
            along_z: true,
        }
    }

    /// Radial scale (bulkier or slimmer).
    fn scaled(mut self, s: f32) -> Self {
        for (_, k) in &mut self.keys {
            k[2] *= s;
            k[3] *= s;
            k[4] *= s;
        }
        self
    }

    fn params(&self, t: f32) -> [f32; 6] {
        let keys = &self.keys;
        let n = keys.len();
        if t <= keys[0].0 {
            return keys[0].1;
        }
        if t >= keys[n - 1].0 {
            return keys[n - 1].1;
        }
        let i = keys.iter().position(|k| k.0 > t).unwrap_or(n - 1) - 1;
        let (t0, t1) = (keys[i].0, keys[i + 1].0);
        let s = (t - t0) / (t1 - t0);
        let p0 = keys[i.saturating_sub(1)].1;
        let p1 = keys[i].1;
        let p2 = keys[i + 1].1;
        let p3 = keys[(i + 2).min(n - 1)].1;
        let mut out = [0.0; 6];
        for k in 0..6 {
            let (a, b, c, d) = (p0[k], p1[k], p2[k], p3[k]);
            out[k] = 0.5
                * (2.0 * b
                    + (-a + c) * s
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * s * s
                    + (-a + 3.0 * b - 3.0 * c + d) * s * s * s);
        }
        out
    }

    fn center(&self, t: f32) -> Vec3 {
        let k = self.params(t);
        if self.along_z {
            v3(k[0], k[1], t)
        } else {
            v3(k[0], t, k[1])
        }
    }

    /// The point at `t` along the axis and angle `phi` around it, grown by `g`. Around Y:
    /// phi 0 = right (+X), π/2 = front (−Z), π = left, 3π/2 = back. Around Z: 0 = right,
    /// π/2 = up, π = left, 3π/2 = down.
    fn at(&self, t: f32, phi: f32, g: f32) -> Vec3 {
        let [cx, cy, hx, hf, hb, e] = self.params(t);
        let (s, c) = phi.sin_cos();
        let p = 2.0 / e.max(1.0);
        let x = c.signum() * c.abs().powf(p) * (hx + g);
        let d = if s >= 0.0 { hf } else { hb };
        let w = s.signum() * s.abs().powf(p) * (d + g);
        if self.along_z {
            v3(cx + x, cy + w, t)
        } else {
            v3(cx + x, t, cy - w)
        }
    }

    /// A closed tube from `t0` to `t1`, with domes of height `cap0`/`cap1` (0 = open).
    #[allow(clippy::too_many_arguments)]
    fn tube(
        &self,
        t0: f32,
        t1: f32,
        rows: usize,
        ring: usize,
        g: f32,
        cap0: f32,
        cap1: f32,
    ) -> MeshBuilder {
        let k0 = if cap0 > 0.0 { 4 } else { 0 };
        let k1 = if cap1 > 0.0 { 4 } else { 0 };
        let total = k0 + rows + k1;
        let axis = if self.along_z { Vec3::Z } else { Vec3::Y };
        let mut m = surface(ring, total, |u, v| {
            let j = (v * total as f32).round() as usize;
            let phi = u * TAU;
            // Lofts along Z face inward with this winding; mirror the angle.
            let phi = if self.along_z { -phi } else { phi };
            if j < k0 {
                let a = (k0 - j) as f32 / k0 as f32 * FRAC_PI_2;
                let c = self.center(t0);
                c + (self.at(t0, phi, g) - c) * a.cos() - axis * cap0 * a.sin()
            } else if j > k0 + rows {
                let a = (j - k0 - rows) as f32 / k1 as f32 * FRAC_PI_2;
                let c = self.center(t1);
                c + (self.at(t1, phi, g) - c) * a.cos() + axis * cap1 * a.sin()
            } else {
                let s = (j - k0) as f32 / rows as f32;
                self.at(lerp(t0, t1, s), phi, g)
            }
        });
        if self.along_z {
            m.flip();
        }
        m
    }

    /// [`Vol::plate`] split into panels at the fractions `cu` (around) and `cv` (along).
    #[allow(clippy::too_many_arguments)]
    fn panels(
        &self,
        phi: (f32, f32),
        t: (f32, f32),
        g: f32,
        pl: Pl,
        nu: usize,
        nv: usize,
        cu: &[f32],
        cv: &[f32],
    ) -> MeshBuilder {
        self.shaped_panels(phi, |_| t, |_, _| g, pl, nu, nv, cu, cv)
    }

    /// [`Vol::shaped`] split into panels at the fractions `cu` (around) and `cv` (along),
    /// with 4.5 mm seams between them.
    #[allow(clippy::too_many_arguments)]
    fn shaped_panels(
        &self,
        phi: (f32, f32),
        bounds: impl Fn(f32) -> (f32, f32),
        g: impl Fn(f32, f32) -> f32,
        pl: Pl,
        nu: usize,
        nv: usize,
        cu: &[f32],
        cv: &[f32],
    ) -> MeshBuilder {
        let (a, b) = bounds(0.5);
        let mid = (a + b) * 0.5;
        let k = self.params(mid);
        let r = (k[2].max(k[3]) + g(0.5, 0.5)).max(0.02);
        let gap = (
            0.0045 / (r * (phi.1 - phi.0).abs()).max(1e-3),
            0.0045 / (b - a).abs().max(1e-3),
        );
        split_plate(nu, nv, pl, self.center(mid), cu, cv, gap, |u, v| {
            let (t0, t1) = bounds(u);
            self.at(lerp(t0, t1, v), lerp(phi.0, phi.1, u), g(u, v))
        })
    }

    /// A plate over the patch `phi0..phi1` × `t0..t1`, grown by `g`.
    #[allow(clippy::too_many_arguments)]
    fn plate(
        &self,
        phi: (f32, f32),
        t: (f32, f32),
        g: f32,
        pl: Pl,
        nu: usize,
        nv: usize,
    ) -> MeshBuilder {
        let inside = self.center((t.0 + t.1) * 0.5);
        plate(nu, nv, pl, inside, |u, v| {
            self.at(lerp(t.0, t.1, v), lerp(phi.0, phi.1, u), g)
        })
    }

    /// A plate whose bounds vary: `bounds(u) = (t0, t1)` along the angle.
    #[allow(clippy::too_many_arguments)]
    fn shaped(
        &self,
        phi: (f32, f32),
        bounds: impl Fn(f32) -> (f32, f32),
        g: impl Fn(f32, f32) -> f32,
        pl: Pl,
        nu: usize,
        nv: usize,
    ) -> MeshBuilder {
        let (a, b) = bounds(0.5);
        let inside = self.center((a + b) * 0.5);
        plate(nu, nv, pl, inside, |u, v| {
            let (t0, t1) = bounds(u);
            self.at(lerp(t0, t1, v), lerp(phi.0, phi.1, u), g(u, v))
        })
    }
}

/// A superellipsoid (helmets, pauldron caps).
#[derive(Debug, Clone, Copy)]
struct Shell {
    c: Vec3,
    rx: f32,
    up: f32,
    down: f32,
    front: f32,
    back: f32,
    e: f32,
}

impl Shell {
    /// The point at azimuth `phi` (0 = right, π/2 = front, π = left, 3π/2 = back) and polar
    /// angle `th` (0 = top, π = bottom), grown by `g`.
    fn at(&self, phi: f32, th: f32, g: f32) -> Vec3 {
        let p = 2.0 / self.e;
        let se = |x: f32| x.signum() * x.abs().powf(p);
        let (sp, cp) = phi.sin_cos();
        let (st, ct) = th.sin_cos();
        let ring = st.abs().powf(p);
        let x = (self.rx + g) * se(cp) * ring;
        let y = (if ct >= 0.0 { self.up } else { self.down } + g) * se(ct);
        let z = -(if sp >= 0.0 { self.front } else { self.back } + g) * se(sp) * ring;
        self.c + v3(x, y, z)
    }

    /// The polar angle at which the (ungrown) shell reaches height `y`.
    fn polar_at(&self, y: f32) -> f32 {
        let k = self.e * 0.5;
        if y >= self.c.y {
            ((y - self.c.y) / self.up).clamp(0.0, 1.0).powf(k).acos()
        } else {
            PI - ((self.c.y - y) / self.down).clamp(0.0, 1.0).powf(k).acos()
        }
    }

    /// A closed cap from the top of the shell down to the polar angle `low(phi)`: an `n`×`n`
    /// plate whose square is mapped onto the disk, so the only border is the cap's rim (no
    /// seam, no pinched apex).
    fn dome(&self, low: impl Fn(f32) -> f32, g: f32, pl: Pl, n: usize) -> MeshBuilder {
        plate(n, n, pl, self.c, |u, v| {
            let (a, b) = (2.0 * u - 1.0, 2.0 * v - 1.0);
            let x = a * (1.0 - b * b * 0.5).sqrt();
            let y = b * (1.0 - a * a * 0.5).sqrt();
            let r = (x * x + y * y).sqrt().min(1.0);
            let phi = y.atan2(x);
            self.at(phi, r * low(phi), g)
        })
    }

    /// A plate from azimuth `phi.0` to `phi.1` between the heights `y.0` (top) and `y.1`.
    #[allow(clippy::too_many_arguments)]
    fn band(
        &self,
        phi: (f32, f32),
        y: (f32, f32),
        g: f32,
        pl: Pl,
        nu: usize,
        nv: usize,
    ) -> MeshBuilder {
        self.plate(phi, (self.polar_at(y.0), self.polar_at(y.1)), g, pl, nu, nv)
    }

    fn plate(
        &self,
        phi: (f32, f32),
        th: (f32, f32),
        g: f32,
        pl: Pl,
        nu: usize,
        nv: usize,
    ) -> MeshBuilder {
        plate(nu, nv, pl, self.c, |u, v| {
            self.at(lerp(phi.0, phi.1, u), lerp(th.0, th.1, v), g)
        })
    }
}

/// A lofted digit (finger, thumb) along `path`, sections from `r0` to `r1` (half width,
/// half thickness), the width along `across`. Round tip.
fn digit(path: &[Vec3], r0: Vec2, r1: Vec2, across: Vec3) -> MeshBuilder {
    // Resample to a few sections, evenly by length.
    let mut len = vec![0.0f32];
    for w in path.windows(2) {
        len.push(len.last().unwrap() + w[0].distance(w[1]));
    }
    let total = len.last().copied().unwrap_or(0.0).max(1e-6);
    let n = 7;
    let sections: Vec<Section> = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let want = t * total;
            let k = len
                .iter()
                .position(|&l| l >= want)
                .unwrap_or(len.len() - 1)
                .max(1);
            let f = ((want - len[k - 1]) / (len[k] - len[k - 1]).max(1e-6)).clamp(0.0, 1.0);
            let p = path[k - 1].lerp(path[k], f);
            Section::rounded(p, r0.lerp(r1, t))
                .with_exponent(2.8)
                .with_right(across)
        })
        .collect();
    MeshBuilder::loft(&sections, 8, Cap::Flat, Cap::Round)
}

/// A rounded box placed with a basis (x, y, z axes scaled by size).
fn block(size: Vec3, radius: f32, x: Vec3, y: Vec3, o: Vec3) -> MeshBuilder {
    let z = x.cross(y).normalize_or(Vec3::Z);
    MeshBuilder::rounded_box(size, radius, 1).transformed(basis(
        x.normalize_or(Vec3::X),
        y.normalize_or(Vec3::Y),
        z,
        o,
    ))
}

/// A disk (short cylinder with chamfered faces) facing `axis` at `o`.
fn disk(radius: f32, thick: f32, axis: Vec3, o: Vec3, segments: u32) -> MeshBuilder {
    let b = thick * 0.3;
    let profile = [
        Vec2::new(0.0005, -thick * 0.5),
        Vec2::new(radius - b, -thick * 0.5),
        Vec2::new(radius, -thick * 0.5 + b),
        Vec2::new(radius, thick * 0.5 - b),
        Vec2::new(radius - b, thick * 0.5),
        Vec2::new(0.0005, thick * 0.5),
    ];
    let rot = Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y));
    MeshBuilder::lathe(&profile, segments).transformed(Mat4::from_rotation_translation(rot, o))
}

/// A spun profile around `axis` at `o`.
fn spun(profile: &[Vec2], segments: u32, axis: Vec3, o: Vec3) -> MeshBuilder {
    let rot = Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y));
    MeshBuilder::lathe(profile, segments).transformed(Mat4::from_rotation_translation(rot, o))
}

/// A fin (crest, horn blade) along `path` (thickness `half_t`), standing out along `up` by
/// `height(t)` at path fraction t.
fn fin(path: &[Vec3], half_t: f32, height: impl Fn(f32) -> f32, side: Vec3) -> MeshBuilder {
    let n = path.len();
    let sections: Vec<Section> = path
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let t = i as f32 / (n - 1) as f32;
            let h = height(t).max(0.0008);
            Section::rounded(p, Vec2::new(half_t * (1.0 - 0.5 * t), h * 0.5))
                .with_exponent(3.0)
                .with_right(side)
                .with_offset(Vec2::new(0.0, h * 0.5))
        })
        .collect();
    MeshBuilder::loft(&sections, 10, Cap::Flat, Cap::Flat)
}

// ------------------------------------------------------------------------ the body

/// Proportions and armor weight of a style.
#[derive(Debug, Clone, Copy)]
struct Spec {
    style: Style,
    /// Radial scale of the body and armor.
    bulk: f32,
    /// Armor plate thickness, m.
    t: f32,
    /// Rim tint of the primary and secondary paint (edge wear).
    edge_a: f32,
    edge_b: f32,
}

impl Spec {
    fn of(style: Style) -> Self {
        match style {
            Style::Pilot => Spec {
                style,
                bulk: 0.97,
                t: 0.008,
                edge_a: 1.8,
                edge_b: 0.85,
            },
            Style::Duelist => Spec {
                style,
                bulk: 1.0,
                t: 0.011,
                edge_a: 0.82,
                edge_b: 1.5,
            },
            Style::Elite => Spec {
                style,
                bulk: 0.96,
                t: 0.01,
                edge_a: 3.0,
                edge_b: 1.4,
            },
            Style::Grunt => Spec {
                style,
                bulk: 1.09,
                t: 0.015,
                edge_a: 0.8,
                edge_b: 1.7,
            },
        }
    }

    fn a(&self, bulge: f32) -> Pl {
        Pl::new(self.t, bulge, self.edge_a)
    }

    fn b(&self, bulge: f32) -> Pl {
        Pl::new(self.t * 0.85, bulge, self.edge_b)
    }
}

fn describe(kit: &mut Kit) {
    let s = Spec::of(kit.style);
    torso(kit, &s);
    pelvis(kit, &s);
    arms(kit, &s);
    hands(kit, &s);
    legs(kit, &s);
    boots(kit, &s);
    helmet(kit, &s);
    back(kit, &s);
    katana(kit, &s);
    scabbard(kit, &s);
}

fn torso_vol(bulk: f32) -> Vol {
    Vol::y(&[
        (0.8, [0.0, 0.015, 0.125, 0.085, 0.09, 2.4]),
        (0.86, [0.0, 0.01, 0.152, 0.098, 0.112, 2.5]),
        (0.93, [0.0, 0.0, 0.163, 0.104, 0.118, 2.6]),
        (1.0, [0.0, 0.0, 0.15, 0.1, 0.104, 2.6]),
        (1.07, [0.0, 0.0, 0.134, 0.093, 0.088, 2.5]),
        (1.15, [0.0, 0.0, 0.143, 0.099, 0.093, 2.5]),
        (1.24, [0.0, -0.004, 0.164, 0.114, 0.1, 2.6]),
        (1.33, [0.0, -0.008, 0.176, 0.124, 0.104, 2.8]),
        (1.41, [0.0, -0.004, 0.181, 0.118, 0.104, 3.0]),
        (1.46, [0.0, 0.0, 0.168, 0.1, 0.1, 3.2]),
        (1.5, [0.0, 0.01, 0.11, 0.07, 0.078, 2.6]),
        (1.535, [0.0, 0.02, 0.062, 0.058, 0.06, 2.0]),
    ])
    .scaled(bulk)
}

fn torso(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let v = torso_vol(s.bulk);
    let style = s.style;
    kit.bind(Spine, Mat::Suit, || {
        v.tube(1.02, 1.27, 8, 32, 0.0, 0.0, 0.0)
    });
    kit.bind(Chest, Mat::Suit, || {
        v.tube(1.2, 1.535, 12, 32, 0.0, 0.0, 0.0)
    });
    kit.bind(Neck, Mat::Suit, || {
        let n = Vol::y(&[
            (1.47, [0.0, 0.02, 0.068, 0.064, 0.066, 2.0]),
            (1.58, [0.0, 0.014, 0.062, 0.06, 0.062, 2.0]),
            (1.66, [0.0, 0.01, 0.058, 0.056, 0.058, 2.0]),
        ]);
        let mut m = n.tube(1.47, 1.66, 6, 24, 0.0, 0.0, 0.0);
        // Ribbed neck seal.
        for k in 0..3 {
            let y = 1.56 + k as f32 * 0.018;
            m.append(
                &n.tube(y, y + 0.008, 1, 24, 0.004, 0.0, 0.0),
                Mat4::IDENTITY,
            );
        }
        m
    });

    let g = 0.012;
    match style {
        Style::Elite => {
            // Sleek angular breastplate: two pectoral plates meeting in a V, crimson inlay.
            let pec = v.shaped(
                (0.4, FRAC_PI_2 - 0.035),
                |u| (1.215 - 0.05 * u, 1.45 - 0.012 * u),
                |u, _| g + 0.006 * u,
                s.a(0.007),
                16,
                12,
            );
            kit.bind(Chest, Mat::PaintA, || mirrored(pec));
            kit.bind(Chest, Mat::Glow, || {
                mirrored(v.shaped(
                    (0.55, FRAC_PI_2 - 0.05),
                    |u| {
                        let b = 1.215 - 0.05 * u + 0.012;
                        (b, b + 0.006)
                    },
                    |u, _| g + 0.006 * u + s.t + 0.0015,
                    Pl::strip(),
                    14,
                    1,
                ))
            });
            kit.bind(Chest, Mat::PaintB, || {
                mirrored(v.shaped(
                    (FRAC_PI_2 - 0.13, FRAC_PI_2 - 0.02),
                    |u| (1.17 + 0.02 * u, 1.43),
                    |_, _| g + s.t + 0.002,
                    Pl::new(0.004, 0.0, 1.3),
                    3,
                    10,
                ))
            });
        }
        Style::Pilot | Style::Grunt => carrier(kit, s, &v, g),
        Style::Duelist => {
            // A laced dō: a breastplate under the gorget over three lames laced in blue,
            // each lame overlapping the one below.
            let phi = (0.34, PI - 0.34);
            let lames = Lame::shingled(1.37, 3, 0.064, 0.014, g, 0.0005, 0.0045);
            let top = lames[0].g.1 + lames[0].t + 0.0012;
            let plate_y = |u: f32| (1.356, 1.452 - 0.014 * (2.0 * u - 1.0).powi(2));
            kit.bind(Chest, Mat::PaintA, || {
                let mut m = v.shaped(phi, plate_y, |_, _| top, s.a(0.004), 22, 6);
                for l in &lames {
                    m.append(&l.plate(&v, phi, 0.002, s.edge_a, 22, 4), Mat4::IDENTITY);
                }
                m
            });
            kit.bind(Chest, Mat::Wrap, || {
                let cols = [-0.75, -0.25, 0.25, 0.75].map(|c| FRAC_PI_2 + c);
                Lame::lacing(&lames, &v, &cols, 0.0016)
            });
            kit.bind(Chest, Mat::Trim, || {
                mirrored(bolts(&v, &[(0.55, 1.425)], top + s.t + 0.002))
            });
            kit.bind(Chest, Mat::Glow, || {
                v.shaped(
                    (0.52, PI - 0.52),
                    |_| (1.3615, 1.3665),
                    |_, _| top + s.t + 0.0014,
                    Pl::strip(),
                    16,
                    1,
                )
            });
        }
    }
    // Side and back plates (the soldiers' carrier covers theirs).
    if !matches!(style, Style::Pilot | Style::Grunt) {
        kit.bind(Chest, Mat::PaintB, || {
            mirrored(v.panels(
                (-0.32, 0.36),
                (1.215, 1.34),
                g * 0.7,
                s.b(0.004),
                8,
                6,
                &[0.5],
                &[],
            ))
        });
        kit.bind(Chest, Mat::PaintA, || {
            mirrored(v.panels(
                (PI + 0.38, 1.5 * PI - 0.03),
                (1.22, 1.45),
                g,
                s.a(0.006),
                10,
                10,
                &[],
                &[0.45],
            ))
        });
        kit.bind(Chest, Mat::Rubber, || {
            mirrored(vents(
                &v,
                -0.05,
                1.37,
                4,
                0.016,
                g * 0.7 + 0.002,
                Vec2::new(0.035, 0.005),
            ))
        });
    }
    // Gorget: stands up around the neck, flaring, lower in front so the chin clears it.
    let gorget = if style == Style::Pilot {
        Mat::Suit
    } else {
        Mat::PaintB
    };
    kit.bind(Chest, gorget, || {
        let flare = |_: f32, vv: f32| g + 0.004 + vv * 0.022;
        let front = v.shaped(
            (-0.55, PI + 0.55),
            |u| (1.495, 1.6 - 0.045 * (1.0 - (2.0 * u - 1.0).abs())),
            flare,
            s.b(0.003),
            18,
            5,
        );
        let mut m = front;
        m.append(
            &v.shaped(
                (PI + 0.45, TAU - 0.45),
                |_| (1.495, 1.605),
                flare,
                s.b(0.003),
                8,
                5,
            ),
            Mat4::IDENTITY,
        );
        m
    });
    // Abdomen: three overlapping lames.
    let lame_mat = if style == Style::Grunt {
        Mat::PaintB
    } else {
        Mat::PaintA
    };
    // The soldiers' carrier and pouches cover their bellies instead.
    if !matches!(style, Style::Pilot | Style::Grunt) {
        kit.bind(Spine, lame_mat, || {
            let mut m = MeshBuilder::new();
            for k in 0..3 {
                let y0 = 1.06 + k as f32 * 0.054;
                let w = if style == Style::Elite { 0.75 } else { 0.62 };
                m.append(
                    &v.plate(
                        (w, PI - w),
                        (y0, y0 + 0.062),
                        0.008 + 0.0035 * k as f32,
                        Pl::new(
                            s.t * 0.8,
                            0.003,
                            if lame_mat == Mat::PaintA {
                                s.edge_a
                            } else {
                                s.edge_b
                            },
                        ),
                        14,
                        3,
                    ),
                    Mat4::IDENTITY,
                );
            }
            m
        });
    }
    // The samurai styles lace their abdomen lames like the shoulders.
    let lacing = match style {
        Style::Duelist => Some(Mat::Wrap),
        Style::Elite => Some(Mat::PaintB),
        _ => None,
    };
    if let Some(cord) = lacing {
        kit.bind(Spine, cord, || {
            let lames: Vec<Lame> = (0..3)
                .map(|k| {
                    let y0 = 1.06 + k as f32 * 0.054;
                    let g = 0.008 + 0.0035 * k as f32;
                    Lame {
                        y: (y0, y0 + 0.062),
                        g: (g, g),
                        t: s.t * 0.8,
                    }
                })
                .collect();
            let cols = [-0.75, -0.25, 0.25, 0.75].map(|c| FRAC_PI_2 + c);
            Lame::lacing(&lames, &v, &cols, 0.0024)
        });
    }
    if style == Style::Elite {
        kit.bind(Spine, Mat::Glow, || {
            v.plate(
                (FRAC_PI_2 - 0.02, FRAC_PI_2 + 0.02),
                (1.07, 1.215),
                0.008 + 0.0035 * 2.0 + s.t * 0.8 + 0.002,
                Pl::strip(),
                1,
                8,
            )
        });
    }
}

fn pelvis(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let v = torso_vol(s.bulk);
    let style = s.style;
    kit.bind(Pelvis, Mat::Suit, || {
        v.tube(0.84, 1.1, 9, 32, 0.0, 0.07, 0.0)
    });
    // Belt with a buckle and pouches; the hakama styles wear a sash over it instead.
    let hakama = matches!(style, Style::Duelist | Style::Elite);
    kit.bind(Pelvis, Mat::Leather, || {
        let mut m = if hakama {
            MeshBuilder::new()
        } else {
            v.plate(
                (-FRAC_PI_2 + 0.07, 1.5 * PI - 0.07),
                (0.995, 1.045),
                0.012,
                Pl::new(0.006, 0.0, 1.25),
                36,
                2,
            )
        };
        let pouch = |phi: f32, size: Vec3| {
            let p = v.at(0.99, phi, 0.012 + size.z * 0.5);
            let out = (p - v.center(0.99)).normalize_or(Vec3::Z);
            let x = Vec3::Y.cross(out).normalize_or(Vec3::X);
            let mut b = block(size, 0.008, x, Vec3::Y, p);
            b.append(
                &block(
                    v3(size.x * 1.04, size.y * 0.35, size.z * 1.1),
                    0.006,
                    x,
                    Vec3::Y,
                    p + Vec3::Y * size.y * 0.36 + out * 0.002,
                ),
                Mat4::IDENTITY,
            );
            b
        };
        m.append(&pouch(-0.55, v3(0.06, 0.07, 0.035)), Mat4::IDENTITY);
        m.append(&pouch(-1.05, v3(0.055, 0.07, 0.035)), Mat4::IDENTITY);
        if style != Style::Elite {
            m.append(&pouch(-2.3, v3(0.07, 0.06, 0.03)), Mat4::IDENTITY);
        }
        // The scabbard's hanger on the left hip.
        let mouth = rig::SHEATH_MOUTH;
        m.append(
            &block(
                v3(0.03, 0.07, 0.012),
                0.004,
                v3(0.3, 0.0, 1.0),
                Vec3::Y,
                mouth + v3(0.02, 0.025, 0.0),
            ),
            Mat4::IDENTITY,
        );
        m
    });
    kit.bind(Pelvis, Mat::Trim, || {
        let p = v.at(1.02, FRAC_PI_2, 0.022);
        let mut m = if hakama {
            MeshBuilder::new()
        } else {
            MeshBuilder::chamfer_box(v3(0.064, 0.046, 0.012), 0.003)
                .transformed(Mat4::from_translation(p))
        };
        for phi in [-0.55f32, -1.05] {
            let q = v.at(1.005, phi, 0.05);
            let out = (q - v.center(1.005)).normalize_or(Vec3::Z);
            m.append(&disk(0.006, 0.004, out, q, 10), Mat4::IDENTITY);
        }
        m
    });
    if hakama {
        kit.bind(Pelvis, Mat::Sash, || obi(&v));
    } else {
        kit.bind(Pelvis, Mat::Glow, || {
            let p = v.at(1.02, FRAC_PI_2, 0.0285);
            MeshBuilder::rounded_box(v3(0.03, 0.008, 0.002), 0.001, 1)
                .transformed(Mat4::from_translation(p))
        });
    }
    // Faulds front and back.
    if style != Style::Pilot {
        let fauld = if style == Style::Elite {
            Mat::PaintA
        } else {
            Mat::PaintB
        };
        kit.bind(Pelvis, fauld, || {
            v.shaped(
                (FRAC_PI_2 - 0.45, FRAC_PI_2 + 0.45),
                |u| (0.9 + 0.03 * (u - 0.5).abs() * 2.0, 0.99),
                |_, _| 0.013,
                s.b(0.004),
                10,
                5,
            )
        });
        kit.bind(Pelvis, Mat::PaintA, || {
            v.plate((PI + 0.5, TAU - 0.5), (0.9, 0.99), 0.013, s.a(0.005), 14, 5)
        });
    }
    if matches!(style, Style::Pilot | Style::Grunt) {
        // Cargo trousers: the seat and fly, gathered under the belt.
        kit.bind(Pelvis, Mat::Cloth, || {
            let mut m = plate(48, 8, Pl::new(0.004, 0.0, 0.85), v.center(0.95), |u, w| {
                let phi = -FRAC_PI_2 + u * TAU;
                let y = lerp(0.86, 1.05, w);
                let gather = 0.002 * (phi * 9.0).sin() * smooth(0.99, 1.04, y);
                v.at(y, phi, 0.007 + gather)
            });
            // Fly seam.
            m.append(
                &v.plate(
                    (FRAC_PI_2 + 0.03, FRAC_PI_2 + 0.1),
                    (0.9, 0.995),
                    0.011,
                    Pl::new(0.003, 0.0, 0.8),
                    2,
                    6,
                ),
                Mat4::IDENTITY,
            );
            m
        });
    }
    // Tassets over the thighs (they swing with the legs).
    let skirt =
        |y: f32, phi: f32, g: f32| v.at(1.0, phi, g + (1.0 - y) * 0.32) - Vec3::Y * (1.0 - y);
    let tasset = |phi: (f32, f32), y: (f32, f32), g: f32, pl: Pl, nu: usize| {
        plate(nu, 4, pl, v3(0.0, 1.0, 0.0), |u, vv| {
            skirt(lerp(y.0, y.1, vv), lerp(phi.0, phi.1, u), g)
        })
    };
    match style {
        Style::Elite => {
            // Long, layered blades down the thighs.
            kit.pair(TassetR, Mat::PaintA, || {
                let mut m = MeshBuilder::new();
                for k in 0..4 {
                    let y1 = 0.995 - k as f32 * 0.085;
                    m.append(
                        &tasset(
                            (0.12, 1.12 - k as f32 * 0.04),
                            (y1 - 0.1, y1),
                            0.03 - 0.004 * k as f32,
                            s.a(0.004),
                            10,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.pair(TassetR, Mat::Glow, || {
                tasset(
                    (0.14, 1.0),
                    (0.665, 0.672),
                    0.018 + s.t + 0.003,
                    Pl::strip(),
                    10,
                )
            });
            kit.pair(TassetR, Mat::PaintB, || {
                tasset(
                    (0.12, 1.12),
                    (0.975, 0.995),
                    0.03 + s.t + 0.001,
                    Pl::new(0.004, 0.0, 1.3),
                    10,
                )
            });
        }
        Style::Grunt => {
            kit.pair(TassetR, Mat::PaintA, || {
                let mut m = tasset((0.1, 1.2), (0.86, 0.995), 0.028, s.a(0.006), 10);
                m.append(
                    &tasset((0.12, 1.15), (0.77, 0.875), 0.022, s.a(0.005), 10),
                    Mat4::IDENTITY,
                );
                m
            });
        }
        Style::Duelist => {
            kit.pair(TassetR, Mat::PaintA, || {
                let mut m = tasset((0.12, 1.2), (0.875, 0.995), 0.026, s.a(0.005), 10);
                m.append(
                    &tasset((0.15, 1.15), (0.78, 0.89), 0.02, s.a(0.004), 10),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.pair(TassetR, Mat::PaintB, || {
                tasset(
                    (0.15, 1.15),
                    (0.78, 0.795),
                    0.02 + s.t + 0.001,
                    Pl::new(0.004, 0.0, 1.4),
                    10,
                )
            });
        }
        Style::Pilot => {
            // Small carbon hip guards, and a holster pouch on the right thigh.
            kit.pair(TassetR, Mat::Carbon, || {
                tasset(
                    (0.25, 0.95),
                    (0.9, 0.995),
                    0.024,
                    Pl::new(0.006, 0.003, 1.0),
                    8,
                )
            });
            kit.local(TassetR, Mat::Leather, || {
                let p = v3(0.2, 0.86, -0.01) - Bone::TassetR.joint();
                block(v3(0.03, 0.12, 0.075), 0.01, Vec3::Z, Vec3::Y, p)
            });
        }
    }
}

/// The sash wound twice around the waist over the armor's lower edge, knotted in front
/// right of center (clear of the sword) with two tails hanging from the knot.
fn obi(v: &Vol) -> MeshBuilder {
    let g = 0.024;
    let cloth = |t: f32| Pl::new(t, 0.0015, 0.85);
    let mut m = v.plate(
        (-FRAC_PI_2 + 0.03, 1.5 * PI - 0.03),
        (0.976, 1.074),
        g,
        cloth(0.006),
        44,
        3,
    );
    // The second turn crosses the front on a slant.
    m.append(
        &v.shaped(
            (FRAC_PI_2 - 1.9, FRAC_PI_2 + 1.3),
            |u| (0.986 + 0.03 * u, 1.034 + 0.03 * u),
            |_, _| g + 0.0072,
            cloth(0.0045),
            26,
            2,
        ),
        Mat4::IDENTITY,
    );
    let phi = FRAC_PI_2 - 0.42;
    let p = v.at(1.03, phi, g + 0.026);
    let out = (p - v.center(1.03)).with_y(0.0).normalize_or(Vec3::NEG_Z);
    let across = Vec3::Y.cross(out).normalize_or(Vec3::X);
    m.append(
        &block(v3(0.058, 0.044, 0.03), 0.013, across, Vec3::Y, p),
        Mat4::IDENTITY,
    );
    // Tails: fall from the knot, splay a little and end on a slant.
    for (lean, len, w) in [(-0.2f32, 0.15f32, 0.046f32), (0.16, 0.115, 0.04)] {
        let down = (Vec3::NEG_Y + across * lean).normalize();
        let top = p - Vec3::Y * 0.012 + out * 0.004;
        m.append(
            &plate(2, 6, cloth(0.004), top - out, |u, t| {
                let l = len * (1.0 - 0.18 * u * lean.signum());
                let bow = (PI * t).sin() * 0.008 + t * 0.006;
                top + across * (u - 0.5) * w + down * t * l + out * bow
            }),
            Mat4::IDENTITY,
        );
    }
    m
}

/// The soldiers' plate carrier: padded cloth panels front and back (cut away under the
/// arms) over hard plates, a cummerbund round the sides, shoulder straps and magazine
/// pouches across the belly. The pilot's plate covers the upper chest in two halves and it
/// carries a radio; the grunt's covers the whole front.
fn carrier(kit: &mut Kit, s: &Spec, v: &Vol, g: f32) {
    use Bone::*;
    let grunt = s.style == Style::Grunt;
    let pad = Pl::new(0.008, 0.007, 0.85);
    let top = |u: f32| 1.445 - 0.08 * smooth(0.45, 1.0, (2.0 * u - 1.0).abs());
    let front = (0.5, PI - 0.5);
    let back = (PI + 0.48, TAU - 0.48);
    let low = 1.13;
    // The padded front's outer surface at (phi, y).
    let front_g = move |phi: f32, y: f32| {
        let u = ((phi - front.0) / (front.1 - front.0)).clamp(0.0, 1.0);
        let w = ((y - low) / (top(u) - low)).clamp(0.0, 1.0);
        g + 0.002 + pad.t + pad.bulge * (PI * u).sin() * (PI * w).sin()
    };
    // Magazine pouches across the belly: (angle, height, size).
    let pouches: Vec<(f32, f32, Vec3)> = [-1.0f32, 0.0, 1.0]
        .iter()
        .map(|k| (FRAC_PI_2 + k * 0.38, 1.19, v3(0.064, 0.088, 0.032)))
        .collect();
    let pouch =
        move |&(phi, y, size): &(f32, f32, Vec3)| vest_pouch(v, y, phi, front_g(phi, y), size);
    kit.bind(Chest, Mat::Cloth, || {
        let mut m = v.shaped(front, |u| (low, top(u)), |_, _| g + 0.002, pad, 22, 10);
        m.append(
            &v.shaped(
                back,
                |u| (low, top(u) + 0.01),
                |_, _| g + 0.002,
                pad,
                20,
                10,
            ),
            Mat4::IDENTITY,
        );
        // Cummerbund, and shoulder straps from the panels' top corners up to the collar.
        let band = Pl::new(0.005, 0.002, 0.85);
        m.append(
            &mirrored(v.plate((-0.6, 0.6), (1.12, 1.27), g - 0.002, band, 12, 4)),
            Mat4::IDENTITY,
        );
        for (a, b) in [(0.66, 0.96), (TAU - 0.96, TAU - 0.66)] {
            m.append(
                &mirrored(v.shaped(
                    (a, b),
                    |_| (1.39, 1.53),
                    |_, vv| g + 0.002 + 0.006 * vv,
                    band,
                    4,
                    8,
                )),
                Mat4::IDENTITY,
            );
        }
        for p in &pouches {
            m.append(&pouch(p).0, Mat4::IDENTITY);
        }
        m
    });
    // The hard plate: the pilot's over the upper chest in two halves, the grunt's over the
    // whole front with a raised center.
    let plate_y = move |u: f32| {
        if grunt {
            (1.25, top(u) - 0.012)
        } else {
            (1.3, 1.432 - 0.012 * (2.0 * u - 1.0).powi(2))
        }
    };
    let plate_phi = if grunt {
        (0.6, PI - 0.6)
    } else {
        (0.66, PI - 0.66)
    };
    let plate_g = move |u: f32, vv: f32| {
        let phi = lerp(plate_phi.0, plate_phi.1, u);
        let (y0, y1) = plate_y(u);
        front_g(phi, lerp(y0, y1, vv)) + 0.0015
    };
    kit.bind(Chest, Mat::PaintA, || {
        v.shaped_panels(
            plate_phi,
            plate_y,
            plate_g,
            s.a(if grunt { 0.008 } else { 0.004 }),
            18,
            8,
            &[0.5],
            &[],
        )
    });
    kit.bind(Chest, Mat::Trim, || {
        let mut m = MeshBuilder::new();
        for p in &pouches {
            let (_, (snap, out)) = pouch(p);
            m.append(&disk(0.0055, 0.003, out, snap, 10), Mat4::IDENTITY);
        }
        // Plate bolts in the top corners.
        let (phi, y) = if grunt { (0.7, 1.4) } else { (0.74, 1.415) };
        m.append(
            &mirrored(bolts(v, &[(phi, y)], front_g(phi, y) + s.t + 0.003)),
            Mat4::IDENTITY,
        );
        m
    });
    if grunt {
        // A stenciled band across the plate and a lamp on the right of the chest.
        kit.bind(Chest, Mat::PaintB, || {
            v.shaped(
                (FRAC_PI_2 - 0.17, FRAC_PI_2 + 0.17),
                |_| (1.27, 1.42),
                |u, vv| {
                    let phi = lerp(FRAC_PI_2 - 0.17, FRAC_PI_2 + 0.17, u);
                    front_g(phi, lerp(1.27, 1.42, vv)) + s.t + 0.0035
                },
                Pl::new(0.003, 0.0, s.edge_b),
                4,
                8,
            )
        });
        kit.bind(Chest, Mat::Glow, || {
            let (phi, y) = (FRAC_PI_2 - 0.55, 1.36);
            let p = v.at(y, phi, front_g(phi, y) + s.t + 0.004);
            let out = (p - v.center(y)).with_y(0.0).normalize_or(Vec3::NEG_Z);
            disk(0.008, 0.003, out, p, 12)
        });
        return;
    }
    // The pilot's radio: a slim box in a pouch on the left of the chest, a stub antenna
    // and a status light.
    let radio = (FRAC_PI_2 + 0.62, 1.29, v3(0.05, 0.07, 0.028));
    kit.bind(Chest, Mat::Carbon, || {
        let (phi, y, _) = radio;
        let mut m = pouch(&radio).0;
        let p = v.at(y + 0.05, phi, front_g(phi, y) + 0.018);
        m.append(
            &MeshBuilder::tube(&[p, p + v3(0.004, 0.06, 0.0)], 0.0035, 8),
            Mat4::IDENTITY,
        );
        m
    });
    kit.bind(Chest, Mat::Glow, || {
        let (_, (snap, out)) = pouch(&radio);
        let mut m = disk(0.004, 0.002, out, snap + out * 0.001, 10);
        // A status strip on the right of the plate.
        m.append(
            &v.shaped(
                (0.84, 0.98),
                |_| (1.318, 1.324),
                |u, vv| front_g(lerp(0.84, 0.98, u), 1.32 + vv * 0.006) + s.t + 0.003,
                Pl::strip(),
                4,
                1,
            ),
            Mat4::IDENTITY,
        );
        m
    });
}

/// A pouch on a body volume at height `y` and angle `phi`, its back `g` off the volume: the
/// pouch with its flap, and where the flap's snap goes (position, outward normal).
fn vest_pouch(v: &Vol, y: f32, phi: f32, g: f32, size: Vec3) -> (MeshBuilder, (Vec3, Vec3)) {
    let base = v.at(y, phi, g);
    let out = (base - v.center(y)).with_y(0.0).normalize_or(Vec3::NEG_Z);
    let across = Vec3::Y.cross(out).normalize_or(Vec3::X);
    let p = base + out * size.z * 0.5;
    let mut m = block(size, 0.009, across, Vec3::Y, p);
    // The flap: over the top, a little wider and proud of the front.
    m.append(
        &block(
            v3(size.x * 1.06, size.y * 0.42, size.z * 1.1),
            0.007,
            across,
            Vec3::Y,
            p + Vec3::Y * size.y * 0.32 + out * 0.003,
        ),
        Mat4::IDENTITY,
    );
    let snap = p + Vec3::Y * size.y * 0.2 + out * (size.z * 0.55 + 0.0045);
    (m, (snap, out))
}

fn shoulder_vol(scale: f32, exp: f32) -> Vol {
    Vol::y(&[
        (1.24, [0.205, 0.02, 0.068, 0.068, 0.068, exp]),
        (1.33, [0.205, 0.02, 0.078, 0.078, 0.078, exp]),
        (1.41, [0.2, 0.02, 0.083, 0.082, 0.082, exp]),
        (1.47, [0.19, 0.02, 0.075, 0.075, 0.075, exp]),
        (1.51, [0.172, 0.02, 0.055, 0.06, 0.06, exp]),
        (1.535, [0.158, 0.02, 0.028, 0.034, 0.034, 2.0]),
    ])
    .scaled(scale)
}

/// The dome over the shoulder that pauldron caps follow, closed at the top.
fn shoulder_shell(scale: f32, e: f32) -> Shell {
    Shell {
        c: v3(0.195, 1.425, 0.02),
        rx: 0.086 * scale,
        up: 0.1,
        down: 0.09 * scale,
        front: 0.083 * scale,
        back: 0.083 * scale,
        e,
    }
}

/// One lame of a sode (shoulder guard) on a shoulder volume.
#[derive(Debug, Clone, Copy)]
struct Lame {
    /// Bottom and top height.
    y: (f32, f32),
    /// Growth off the volume at the bottom and at the top (it flares out downward).
    g: (f32, f32),
    /// Thickness.
    t: f32,
}

impl Lame {
    /// `n` lames `h` tall hanging from `top`, each overlapping the one above by `lap` and
    /// lying on top of it (one plate thickness `t` further out), each flaring out by `flare`
    /// toward its lower edge. The first starts `g0` off the volume.
    fn stack(top: f32, n: usize, h: f32, lap: f32, g0: f32, flare: f32, t: f32) -> Vec<Lame> {
        let step = flare + t + 0.0012;
        (0..n)
            .map(|k| {
                let y1 = top - k as f32 * (h - lap);
                let g1 = g0 + k as f32 * step;
                Lame {
                    y: (y1 - h, y1),
                    g: (g1 + flare, g1),
                    t,
                }
            })
            .collect()
    }

    /// Like [`Lame::stack`], but each lame lies over the one below it (shingled downward):
    /// the lowest starts `g0` off the volume.
    fn shingled(top: f32, n: usize, h: f32, lap: f32, g0: f32, flare: f32, t: f32) -> Vec<Lame> {
        let step = flare + t + 0.0012;
        (0..n)
            .map(|k| {
                let y1 = top - k as f32 * (h - lap);
                let g1 = g0 + (n - 1 - k) as f32 * step;
                Lame {
                    y: (y1 - h, y1),
                    g: (g1 + flare, g1),
                    t,
                }
            })
            .collect()
    }

    fn g_at(&self, y: f32) -> f32 {
        let v = ((y - self.y.0) / (self.y.1 - self.y.0)).clamp(0.0, 1.0);
        lerp(self.g.0, self.g.1, v)
    }

    fn plate(
        &self,
        sv: &Vol,
        phi: (f32, f32),
        bulge: f32,
        edge: f32,
        nu: usize,
        nv: usize,
    ) -> MeshBuilder {
        let l = *self;
        sv.shaped(
            phi,
            |_| l.y,
            |_, v| lerp(l.g.0, l.g.1, v),
            Pl::new(l.t, bulge, edge),
            nu,
            nv,
        )
    }

    /// A strip along the lame between `from` and `to` above its lower edge, `off` above its
    /// face.
    fn edge(&self, sv: &Vol, phi: (f32, f32), from: f32, to: f32, off: f32, pl: Pl) -> MeshBuilder {
        let l = *self;
        let y = (l.y.0 + from, l.y.0 + to);
        sv.shaped(
            phi,
            |_| y,
            |_, v| l.g_at(lerp(y.0, y.1, v)) + l.t + off,
            pl,
            14,
            1,
        )
    }

    /// Pairs of cords laced down every lame at the angles `cols`, `off` above the faces.
    fn lacing(lames: &[Lame], sv: &Vol, cols: &[f32], off: f32) -> MeshBuilder {
        let mut m = MeshBuilder::new();
        for l in lames {
            let y = (l.y.0 + 0.003, l.y.1 - 0.002);
            let k = sv.params((y.0 + y.1) * 0.5);
            let r = k[2].max(k[3]) + l.g.1;
            let (w, d) = (0.0021 / r, 0.0042 / r);
            for &c in cols {
                for side in [-1.0f32, 1.0] {
                    let a = c + side * d;
                    let mut cord = surface(1, 3, |u, v| {
                        let h = lerp(y.0, y.1, v);
                        sv.at(h, lerp(a - w, a + w, u), l.g_at(h) + l.t + off)
                    });
                    face_out(&mut cord, sv.center((y.0 + y.1) * 0.5));
                    m.append(&cord, Mat4::IDENTITY);
                }
            }
        }
        m
    }
}

fn arms(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let b = s.bulk;
    let upper = Vol::y(&[
        (1.12, [0.19, 0.025, 0.044, 0.046, 0.046, 2.2]),
        (1.15, [0.19, 0.025, 0.046, 0.048, 0.048, 2.2]),
        (1.22, [0.19, 0.02, 0.05, 0.052, 0.05, 2.2]),
        (1.32, [0.19, 0.02, 0.058, 0.056, 0.055, 2.3]),
        (1.42, [0.186, 0.02, 0.062, 0.06, 0.06, 2.3]),
        (1.49, [0.18, 0.02, 0.054, 0.054, 0.054, 2.2]),
    ])
    .scaled(b);
    let fore = Vol::y(&[
        (0.875, [0.19, 0.02, 0.022, 0.03, 0.03, 2.4]),
        (0.92, [0.19, 0.02, 0.025, 0.033, 0.033, 2.4]),
        (1.02, [0.19, 0.02, 0.034, 0.042, 0.04, 2.3]),
        (1.1, [0.19, 0.022, 0.042, 0.046, 0.045, 2.2]),
        (1.16, [0.19, 0.025, 0.044, 0.046, 0.046, 2.2]),
        (1.19, [0.19, 0.028, 0.04, 0.04, 0.04, 2.0]),
    ])
    .scaled(b);
    kit.pair(UpperArmR, Mat::Suit, || {
        upper.tube(1.13, 1.49, 9, 24, 0.0, 0.03, 0.04)
    });
    if matches!(s.style, Style::Pilot | Style::Grunt) {
        // A field shirt's sleeve with soft creases toward the elbow.
        kit.pair(UpperArmR, Mat::Cloth, || {
            hakama_leg(&upper, (1.13, 1.48), 32, 9, |phi, y| {
                let crease = 0.5 + 0.5 * (phi * 3.0 + y * 30.0).sin();
                0.004 + 0.003 * crease * smooth(1.4, 1.18, y)
            })
        });
    }
    if matches!(s.style, Style::Duelist | Style::Elite) {
        // Kote: a fitted cloth sleeve over the upper arm, creased around the elbow.
        kit.pair(UpperArmR, Mat::Cloth, || {
            hakama_leg(&upper, (1.14, 1.47), 40, 10, |phi, y| {
                let crease = 0.5 + 0.5 * (phi * 4.0 + y * 26.0).sin();
                0.005 + 0.0045 * crease * smooth(1.42, 1.2, y)
            })
        });
    }
    kit.pair(ForearmR, Mat::Suit, || {
        fore.tube(0.88, 1.17, 9, 24, 0.0, 0.0, 0.035)
    });
    let style = s.style;
    // Upper arm armor.
    if style != Style::Pilot {
        kit.pair(UpperArmR, Mat::PaintB, || {
            // Over the kote and the grunt's sleeve.
            let g = if matches!(style, Style::Duelist | Style::Elite) {
                0.015
            } else {
                0.013
            };
            upper.plate((-1.15, 1.15), (1.2, 1.35), g, s.b(0.004), 12, 6)
        });
    } else {
        kit.pair(UpperArmR, Mat::Leather, || {
            upper.plate(
                (-PI + 0.2, PI - 0.2),
                (1.26, 1.285),
                0.011,
                Pl::new(0.004, 0.0, 1.2),
                24,
                1,
            )
        });
    }
    // Elbow cop.
    kit.pair(ForearmR, Mat::PaintB, || {
        fore.plate(
            (1.5 * PI - 1.1, 1.5 * PI + 1.1),
            (1.095, 1.2),
            0.01,
            s.b(0.012),
            10,
            6,
        )
    });
    if matches!(style, Style::Duelist | Style::Elite) {
        kote(kit, s, &fore);
    } else {
        // Vambrace with an accent line.
        let vt = s.t * 0.8;
        kit.pair(ForearmR, Mat::PaintA, || {
            fore.shaped_panels(
                (-1.45, 1.45),
                |u| (0.915 + 0.015 * (1.0 - (u - 0.5).abs() * 2.0), 1.095),
                |_, _| 0.007,
                Pl::new(vt, 0.004, s.edge_a),
                14,
                8,
                &[],
                &[0.42],
            )
        });
        kit.pair(ForearmR, Mat::Glow, || {
            fore.plate(
                (-0.07, 0.07),
                (0.95, 1.07),
                0.007 + vt + 0.0045,
                Pl::strip(),
                1,
                8,
            )
        });
    }
    // Pauldrons: a cap over the shoulder and a sode of overlapping lames hanging from it.
    match style {
        Style::Duelist => {
            let sv = shoulder_vol(1.0 * b, 2.3);
            let sh = shoulder_shell(1.0 * b, 2.8);
            let lames = Lame::stack(1.468, 4, 0.056, 0.013, 0.012, 0.003, 0.0055);
            let phi = (-1.62, 1.62);
            kit.pair(PauldronR, Mat::PaintA, || {
                let low = sh.polar_at(1.452);
                let mut m = sh.dome(|_| low, 0.02, s.a(0.006), 12);
                for l in &lames {
                    m.append(&l.plate(&sv, phi, 0.0015, s.edge_a, 16, 4), Mat4::IDENTITY);
                }
                m
            });
            kit.pair(PauldronR, Mat::Wrap, || {
                Lame::lacing(&lames, &sv, &[-1.12, -0.38, 0.38, 1.12], 0.0018)
            });
            kit.pair(PauldronR, Mat::Trim, || {
                let l = &lames[lames.len() - 1];
                l.edge(&sv, phi, 0.0, 0.007, 0.0008, Pl::new(0.0025, 0.0, 1.0))
            });
            kit.pair(PauldronR, Mat::Glow, || {
                sh.band(
                    (-1.4, 1.4),
                    (1.468, 1.462),
                    0.02 + s.t + 0.0018,
                    Pl::strip(),
                    12,
                    1,
                )
            });
        }
        Style::Elite => {
            let sv = shoulder_vol(0.95 * b, 2.6);
            let sh = shoulder_shell(0.95 * b, 2.6);
            let lames = Lame::stack(1.462, 4, 0.052, 0.012, 0.011, 0.0035, 0.005);
            let phi = (-1.6, 1.6);
            kit.pair(PauldronR, Mat::PaintA, || {
                let low = sh.polar_at(1.448);
                let mut m = sh.dome(|_| low, 0.014, s.a(0.006), 12);
                for l in &lames {
                    m.append(&l.plate(&sv, phi, 0.0012, s.edge_a, 16, 4), Mat4::IDENTITY);
                }
                m
            });
            kit.pair(PauldronR, Mat::PaintB, || {
                Lame::lacing(&lames, &sv, &[-1.1, -0.37, 0.37, 1.1], 0.0015)
            });
            // Swept crimson blade rising off the shoulder.
            kit.pair(PauldronR, Mat::PaintB, || {
                let path: Vec<Vec3> = (0..10)
                    .map(|i| {
                        let t = i as f32 / 9.0;
                        sh.at(
                            lerp(1.4, -1.7, t),
                            sh.polar_at(lerp(1.505, 1.462, t)),
                            0.014 + s.t * 0.8,
                        )
                    })
                    .collect();
                fin(&path, 0.005, |t| 0.012 + 0.05 * t * t, v3(0.6, 0.8, 0.0))
            });
            kit.pair(PauldronR, Mat::Glow, || {
                let l = &lames[lames.len() - 1];
                l.edge(&sv, (-1.5, 1.5), 0.004, 0.0075, 0.0014, Pl::strip())
            });
        }
        Style::Grunt => {
            let sv = shoulder_vol(1.08 * b, 3.0);
            let sh = shoulder_shell(1.02 * b, 3.0);
            let lames = Lame::stack(1.45, 3, 0.07, 0.016, 0.017, 0.003, 0.008);
            let phi = (-1.7, 1.7);
            kit.pair(PauldronR, Mat::PaintA, || {
                let low = sh.polar_at(1.44);
                let mut m = sh.dome(|_| low, 0.02, s.a(0.0), 12);
                for l in &lames {
                    m.append(&l.plate(&sv, phi, 0.002, s.edge_a, 16, 4), Mat4::IDENTITY);
                }
                m
            });
            kit.pair(PauldronR, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                for l in &lames {
                    for k in 0..4 {
                        let p = -1.14 + k as f32 * 0.76;
                        let y = l.y.0 + 0.014;
                        let g = l.g_at(y) + l.t + 0.0015;
                        let q = sv.at(y, p, g);
                        let out = (q - sv.center(y)).normalize_or(Vec3::X);
                        m.append(&disk(0.0055, 0.004, out, q, 8), Mat4::IDENTITY);
                    }
                }
                m
            });
            kit.pair(PauldronR, Mat::PaintB, || {
                // Hazard stripes on the cap.
                let mut m = MeshBuilder::new();
                for k in 0..3 {
                    let phi = -0.5 + k as f32 * 0.38;
                    m.append(
                        &sh.band(
                            (phi, phi + 0.16),
                            (1.5, 1.445),
                            0.02 + s.t + 0.0008,
                            Pl::new(0.002, 0.0, 1.0),
                            2,
                            4,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
        }
        Style::Pilot => {
            let sh = shoulder_shell(0.82 * b, 2.4);
            kit.pair(PauldronR, Mat::Carbon, || {
                let low = sh.polar_at(1.36);
                sh.dome(|_| low, 0.008, Pl::new(0.007, 0.0, 1.0), 12)
            });
            kit.pair(PauldronR, Mat::PaintA, || {
                sh.band(
                    (-0.9, 0.9),
                    (1.5, 1.41),
                    0.008 + 0.007 + 0.001,
                    s.a(0.003),
                    10,
                    5,
                )
            });
        }
    }
    // Pilot: a wrist computer with a glowing screen on the left forearm.
    if style == Style::Pilot {
        kit.bind(ForearmL, Mat::Trim, || {
            let p = fore.at(0.97, 0.0, 0.016) * v3(-1.0, 1.0, 1.0);
            block(v3(0.016, 0.06, 0.045), 0.004, Vec3::X, Vec3::Y, p)
        });
        kit.bind(ForearmL, Mat::Glow, || {
            let p = fore.at(0.97, 0.0, 0.025) * v3(-1.0, 1.0, 1.0);
            block(v3(0.002, 0.044, 0.032), 0.0008, Vec3::X, Vec3::Y, p)
        });
    }
}

/// Kote: the samurai's armored sleeve below the elbow. The cloth sleeve is faced with mail
/// over the back and sides of the forearm, and three lacquered splints are laced on over
/// the mail, a glowing inlay down the middle one.
fn kote(kit: &mut Kit, s: &Spec, fore: &Vol) {
    use Bone::ForearmR;
    kit.pair(ForearmR, Mat::Cloth, || {
        hakama_leg(fore, (0.9, 1.165), 36, 8, |phi, y| {
            let crease = 0.5 + 0.5 * (phi * 4.0 + y * 40.0).sin();
            0.003 + 0.003 * crease * smooth(1.08, 1.16, y)
        })
    });
    // Over the sleeve's cloth (its own 4 mm included).
    let g_mail = 0.0105;
    kit.pair(ForearmR, Mat::Mail, || {
        fore.plate(
            (-1.85, 1.75),
            (0.912, 1.118),
            g_mail,
            Pl::new(0.0012, 0.0, 0.9),
            24,
            8,
        )
    });
    // (center angle, half width, wrist end, elbow end); the back of the forearm faces
    // out (+X) and a little back.
    let splints = [
        (-0.84, 0.25, 0.938, 1.082),
        (-0.2, 0.3, 0.924, 1.098),
        (0.44, 0.25, 0.938, 1.082),
    ];
    let g_splint = g_mail + 0.0016;
    let pl = Pl::new(s.t * 0.75, 0.0016, s.edge_a);
    kit.pair(ForearmR, Mat::PaintA, || {
        let mut m = MeshBuilder::new();
        for &(c, w, y0, y1) in &splints {
            m.append(
                &fore.shaped(
                    (c - w, c + w),
                    |u| {
                        let e = (2.0 * u - 1.0).powi(2);
                        (y0 + 0.01 * e, y1 - 0.014 * e)
                    },
                    |_, _| g_splint,
                    pl,
                    6,
                    8,
                ),
                Mat4::IDENTITY,
            );
        }
        m
    });
    // Each splint is lashed on at two places.
    let top = g_splint + pl.t + pl.bulge + 0.0004;
    let ties = [0.962, 1.058];
    kit.pair(ForearmR, Mat::Wrap, || {
        let mut m = MeshBuilder::new();
        for &(c, w, _, _) in &splints {
            for &y in &ties {
                m.append(
                    &fore.plate(
                        (c - w - 0.04, c + w + 0.04),
                        (y - 0.0035, y + 0.0035),
                        top,
                        Pl::new(0.0016, 0.0, 1.0),
                        6,
                        1,
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
        m
    });
    kit.pair(ForearmR, Mat::Glow, || {
        let c = splints[1].0;
        fore.plate(
            (c - 0.035, c + 0.035),
            (ties[0] + 0.01, ties[1] - 0.01),
            top - 0.0002,
            Pl::strip(),
            1,
            6,
        )
    });
}

/// Knuckle positions (MCP joints) of the right hand along the grip, hand frame.
const KNUCKLES: [(f32, f32); 4] = [
    (-0.03, 0.085),
    (-0.01, 0.092),
    (0.01, 0.088),
    (0.029, 0.072),
];

fn hands(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let style = s.style;
    let palm = Vol::y(&[
        (-0.1, [0.004, 0.002, 0.016, 0.042, 0.042, 3.4]),
        (-0.092, [0.004, 0.002, 0.017, 0.044, 0.044, 3.4]),
        (-0.06, [0.004, 0.0, 0.019, 0.043, 0.043, 3.4]),
        (-0.02, [0.002, 0.0, 0.021, 0.035, 0.035, 3.0]),
        (0.012, [0.0, 0.0, 0.021, 0.027, 0.027, 2.5]),
    ]);
    let glove = if style == Style::Pilot {
        Mat::Suit
    } else {
        Mat::Leather
    };
    let armor = if style == Style::Pilot {
        Mat::Carbon
    } else {
        Mat::PaintB
    };
    let grip = rig::grip_r();
    let axis = grip.rot * Vec3::Z;
    // Right fist around the grip.
    let fist_fingers = || {
        let mut m = MeshBuilder::new();
        let mut plates = MeshBuilder::new();
        for (k, &(z, len)) in KNUCKLES.iter().enumerate() {
            let knuckle = v3(0.0, -0.09 - 0.004 * (k == 1) as i32 as f32, z);
            let c = grip.pos + axis * (knuckle - grip.pos).dot(axis);
            let r0 = knuckle - c;
            let rho0 = r0.length();
            let wrap = 0.0145 + 0.0078;
            let mut pts = vec![knuckle];
            let mut travelled = 0.0;
            let mut a = 0.0f32;
            let mut phal = Vec::new();
            while travelled < len && pts.len() < 64 {
                a += 0.08;
                let rho = lerp(rho0, wrap, smooth(0.0, 0.9, a));
                let dir = Quat::from_axis_angle(axis, -a) * r0.normalize();
                let p = c + dir * rho;
                travelled += p.distance(*pts.last().unwrap());
                if (phal.is_empty() && travelled > len * 0.48)
                    || (phal.len() == 1 && travelled > len * 0.78)
                {
                    phal.push(pts.len());
                }
                pts.push(p);
            }
            let thick = Vec2::new(0.0088, 0.0078);
            m.append(&digit(&pts, thick, thick * 0.82, axis), Mat4::IDENTITY);
            // Armor on the backs of the phalanges.
            let bounds = [
                0,
                phal.first().copied().unwrap_or(pts.len() / 2),
                phal.get(1).copied().unwrap_or(pts.len() - 2),
                pts.len() - 1,
            ];
            for w in bounds.windows(2).take(2) {
                let (i0, i1) = (w[0], w[1].max(w[0] + 1));
                let mid = (pts[i0] + pts[i1]) * 0.5;
                let tan = (pts[i1] - pts[i0]).normalize_or(Vec3::NEG_Y);
                let out = (mid - (c + axis * (mid - c).dot(axis))).normalize_or(Vec3::X);
                let l = pts[i0].distance(pts[i1]) * 0.85;
                plates.append(
                    &block(v3(0.0175, l, 0.006), 0.0025, axis, tan, mid + out * 0.0085),
                    Mat4::IDENTITY,
                );
            }
        }
        (m, plates)
    };
    let thumb = || {
        let path = [
            v3(-0.008, -0.026, -0.03),
            v3(-0.026, -0.044, -0.046),
            v3(-0.044, -0.058, -0.046),
            v3(-0.056, -0.068, -0.036),
            v3(-0.062, -0.074, -0.022),
        ];
        let m = digit(
            &path,
            Vec2::new(0.011, 0.0095),
            Vec2::new(0.0085, 0.0075),
            v3(0.3, 0.7, -0.5),
        );
        let mid = (path[2] + path[3]) * 0.5;
        let plate = block(
            v3(0.016, 0.022, 0.006),
            0.0025,
            v3(0.3, 0.7, -0.5).normalize(),
            (path[3] - path[2]).normalize(),
            mid + v3(-0.004, -0.004, -0.008),
        );
        (m, plate)
    };
    let palm_mesh = || palm.tube(-0.092, 0.012, 6, 20, 0.0, 0.012, 0.0);
    let back_plate = || {
        let mut m = palm.plate(
            (-0.95, 0.95),
            (-0.088, -0.012),
            0.002,
            Pl::new(0.006, 0.003, s.edge_b),
            8,
            6,
        );
        // Knuckle ridge.
        m.append(
            &palm.plate(
                (-1.2, 1.2),
                (-0.1, -0.084),
                0.008,
                Pl::new(0.006, 0.002, s.edge_b),
                10,
                2,
            ),
            Mat4::IDENTITY,
        );
        m
    };
    let cuff = || {
        let cv = Vol::y(&[
            (-0.012, [0.0, 0.0, 0.024, 0.032, 0.032, 2.6]),
            (0.03, [0.0, 0.0, 0.03, 0.038, 0.038, 2.6]),
        ]);
        cv.plate(
            (-PI + 0.25, PI - 0.25),
            (-0.01, 0.028),
            0.004,
            Pl::new(s.t * 0.7, 0.0, s.edge_a),
            20,
            3,
        )
    };
    let mirror = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    // Right hand.
    kit.local(HandR, glove, || {
        let (f, _) = fist_fingers();
        let (t, _) = thumb();
        let mut m = palm_mesh();
        m.append(&f, Mat4::IDENTITY);
        m.append(&t, Mat4::IDENTITY);
        m
    });
    kit.local(HandR, armor, || {
        let (_, p) = fist_fingers();
        let (_, tp) = thumb();
        let mut m = back_plate();
        m.append(&p, Mat4::IDENTITY);
        m.append(&tp, Mat4::IDENTITY);
        m
    });
    kit.local(HandR, Mat::PaintA, cuff);
    // Left hand: palm and thumb mirrored, fingers on their own bone.
    kit.local(HandL, glove, || {
        let (t, _) = thumb();
        let mut m = palm_mesh();
        m.append(&t, Mat4::IDENTITY);
        m.transformed(mirror)
    });
    kit.local(HandL, armor, || {
        let (_, tp) = thumb();
        let mut m = back_plate();
        m.append(&tp, Mat4::IDENTITY);
        m.transformed(mirror)
    });
    kit.local(HandL, Mat::PaintA, || cuff().transformed(mirror));
    // Left fingers: straight from the knuckles with a relaxed curl toward the palm (+X).
    let left_fingers = || {
        let mut m = MeshBuilder::new();
        let mut plates = MeshBuilder::new();
        for &(z, len) in &KNUCKLES {
            let start = v3(0.0, -0.004 * (z.abs() < 0.015) as i32 as f32, z)
                - v3(rig::KNUCKLE_L.x, 0.0, 0.0);
            let (l1, l2) = (len * 0.48, len * 0.78);
            let mut pts = vec![start];
            let mut dir = Vec3::NEG_Y;
            let step = 0.004;
            let mut sdist = 0.0;
            let mut joints = Vec::new();
            while sdist < len {
                let bend = 0.6
                    * (smooth(l1 - 0.006, l1 + 0.006, sdist + step)
                        - smooth(l1 - 0.006, l1 + 0.006, sdist))
                    + 0.6
                        * (smooth(l2 - 0.005, l2 + 0.005, sdist + step)
                            - smooth(l2 - 0.005, l2 + 0.005, sdist));
                dir = Quat::from_rotation_z(bend) * dir;
                let p = *pts.last().unwrap() + dir * step;
                sdist += step;
                if (joints.is_empty() && sdist > l1) || (joints.len() == 1 && sdist > l2) {
                    joints.push(pts.len());
                }
                pts.push(p);
            }
            let thick = Vec2::new(0.0088, 0.0078);
            m.append(&digit(&pts, thick, thick * 0.82, Vec3::Z), Mat4::IDENTITY);
            let bounds = [
                0,
                joints.first().copied().unwrap_or(pts.len() / 2),
                joints.get(1).copied().unwrap_or(pts.len() - 2),
            ];
            for w in bounds.windows(2) {
                let (i0, i1) = (w[0], w[1].max(w[0] + 1));
                let mid = (pts[i0] + pts[i1]) * 0.5;
                let tan = (pts[i1] - pts[i0]).normalize_or(Vec3::NEG_Y);
                let out = Vec3::Z.cross(tan).normalize_or(Vec3::NEG_X);
                let l = pts[i0].distance(pts[i1]) * 0.85;
                // The back of the left hand faces −X.
                let out = if out.x > 0.0 { -out } else { out };
                plates.append(
                    &block(
                        v3(0.0175, l, 0.006),
                        0.0025,
                        Vec3::Z,
                        tan,
                        mid + out * 0.0085,
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
        (m, plates)
    };
    kit.local(FingersL, glove, || left_fingers().0);
    kit.local(FingersL, armor, || left_fingers().1);
}

/// Where the hakama's thigh part ends, just below the knee (bind pose).
const HAKAMA_HEM: f32 = 0.465;

/// How far the hakama stands off the leg at angle `phi` (0 = outside, π = inside, toward
/// the other leg), `open` 0..1 down the leg: wide outside, in front and behind, close
/// between the legs, with knife pleats opening toward the hem and a soft drape.
fn hakama_g(phi: f32, open: f32) -> f32 {
    let inner = ((1.0 - phi.cos()) * 0.5).powi(2);
    let flare = lerp(0.046, 0.012, inner) * open.max(0.0).powf(1.3);
    // Knife pleats: the cloth rises across each pleat and drops back at its fold.
    let s = (phi / TAU * 15.0).rem_euclid(1.0);
    let pleat = s * s * (0.0035 + 0.0075 * open) * (1.0 - inner);
    let drape = 0.0035 * open * (phi * 3.0 + 1.3).sin();
    0.011 + flare + pleat + drape
}

/// A cloth tube around a limb volume between the heights `y`, `g(phi, y)` off it, with a
/// hem; the seam runs down the inside of the limb.
fn hakama_leg(
    v: &Vol,
    y: (f32, f32),
    nu: usize,
    nv: usize,
    g: impl Fn(f32, f32) -> f32,
) -> MeshBuilder {
    plate(
        nu,
        nv,
        Pl::new(0.004, 0.0, 0.8),
        v.center((y.0 + y.1) * 0.5),
        |u, w| {
            let phi = PI + u * TAU;
            let h = lerp(y.0, y.1, w);
            v.at(h, phi, g(phi, h))
        },
    )
}

fn legs(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let b = s.bulk;
    let style = s.style;
    let thigh = Vol::y(&[
        (0.47, [0.092, 0.0, 0.05, 0.05, 0.05, 2.0]),
        (0.52, [0.092, 0.0, 0.055, 0.057, 0.055, 2.2]),
        (0.62, [0.093, -0.005, 0.062, 0.065, 0.062, 2.3]),
        (0.75, [0.095, -0.008, 0.074, 0.08, 0.075, 2.3]),
        (0.86, [0.097, -0.005, 0.084, 0.088, 0.085, 2.3]),
        (0.95, [0.095, 0.0, 0.087, 0.09, 0.09, 2.3]),
        (1.0, [0.09, 0.0, 0.08, 0.085, 0.085, 2.2]),
    ])
    .scaled(b);
    let shin = Vol::y(&[
        (0.06, [0.095, 0.0, 0.034, 0.036, 0.036, 2.2]),
        (0.18, [0.095, 0.004, 0.038, 0.04, 0.042, 2.2]),
        (0.3, [0.095, 0.008, 0.046, 0.045, 0.056, 2.2]),
        (0.42, [0.093, 0.004, 0.05, 0.05, 0.055, 2.2]),
        (0.5, [0.092, 0.0, 0.052, 0.055, 0.05, 2.2]),
        (0.56, [0.092, 0.0, 0.048, 0.05, 0.048, 2.0]),
    ])
    .scaled(b);
    kit.pair(ThighR, Mat::Suit, || {
        thigh.tube(0.5, 1.0, 10, 24, 0.0, 0.04, 0.0)
    });
    kit.pair(ShinR, Mat::Suit, || {
        shin.tube(0.08, 0.53, 10, 22, 0.0, 0.0, 0.03)
    });
    // Thigh armor; the hakama styles wear wide pleated trousers instead.
    let hakama = matches!(style, Style::Duelist | Style::Elite);
    match style {
        _ if hakama => {
            kit.pair(ThighR, Mat::Cloth, || {
                hakama_leg(&thigh, (HAKAMA_HEM, 1.035), 84, 16, |phi, y| {
                    hakama_g(phi, (1.035 - y) / (1.035 - HAKAMA_HEM))
                })
            });
            kit.pair(ShinR, Mat::Cloth, || {
                // Bloused below the knee and gathered into the greave.
                hakama_leg(&shin, (0.385, 0.575), 72, 8, |phi, y| {
                    let open = smooth(0.39, 0.475, y);
                    lerp(0.0045, hakama_g(phi, 0.55) - 0.012, open)
                })
            });
        }
        Style::Pilot | Style::Grunt => {
            // Cargo trousers: soft folds, bunched at the knee, a pocket on the outer thigh,
            // bloused into the boots.
            kit.pair(ThighR, Mat::Cloth, || {
                hakama_leg(&thigh, (0.46, 1.035), 56, 22, |phi, y| {
                    // Long folds down the thigh, drag lines from the crotch, and the cloth
                    // bunching above the knee.
                    let fold = (phi * 3.0 + y * 23.0).sin() * 0.5 + 0.5;
                    let drag =
                        ((phi * 5.0 - y * 47.0).sin() * 0.5 + 0.5).powi(3) * smooth(0.7, 0.95, y);
                    let knee = smooth(0.62, 0.5, y) * (0.5 + 0.5 * (y * 150.0 + phi).sin()).powi(2);
                    0.008 + 0.005 * fold + 0.004 * drag + 0.007 * knee
                })
            });
            kit.pair(ThighR, Mat::Cloth, || {
                let (body, _) = vest_pouch(&thigh, 0.73, 0.15, 0.009, v3(0.105, 0.135, 0.03));
                body
            });
            kit.pair(ThighR, Mat::Trim, || {
                let (_, (snap, out)) =
                    vest_pouch(&thigh, 0.73, 0.15, 0.009, v3(0.105, 0.135, 0.03));
                disk(0.006, 0.003, out, snap, 10)
            });
            kit.pair(ShinR, Mat::Cloth, || {
                hakama_leg(&shin, (0.165, 0.575), 44, 18, |phi, y| {
                    let fold = ((phi * 3.0 - y * 37.0).sin() * 0.5 + 0.5).powi(2) * 2.0;
                    // Bloused over the boot top, behind the greave.
                    let blouse = smooth(0.3, 0.2, y)
                        * (1.0 - smooth(0.2, 0.165, y))
                        * smooth(1.1, 1.5, wrap_angle(phi - FRAC_PI_2).abs());
                    0.006 + 0.002 * fold + 0.012 * blouse
                })
            });
        }
        _ => {
            kit.pair(ThighR, Mat::PaintA, || {
                thigh.shaped_panels(
                    (-0.35, FRAC_PI_2 + 0.75),
                    |u| (0.6 + 0.03 * u, 0.86 - 0.02 * u),
                    |_, _| 0.01,
                    s.a(0.008),
                    14,
                    10,
                    &[],
                    &[0.48],
                )
            });
            kit.pair(ThighR, Mat::Trim, || {
                bolts(
                    &thigh,
                    &[
                        (-0.25, 0.84),
                        (-0.25, 0.62),
                        (FRAC_PI_2 + 0.62, 0.84),
                        (FRAC_PI_2 + 0.62, 0.64),
                    ],
                    0.01 + s.t + 0.001,
                )
            });
            kit.pair(ThighR, Mat::Glow, || {
                thigh.plate(
                    (-0.25, -0.2),
                    (0.64, 0.82),
                    0.01 + s.t + 0.003,
                    Pl::strip(),
                    1,
                    8,
                )
            });
            kit.pair(ThighR, Mat::Leather, || {
                thigh.plate(
                    (-PI + 0.15, PI - 0.15),
                    (0.565, 0.59),
                    0.004,
                    Pl::new(0.004, 0.0, 1.2),
                    28,
                    1,
                )
            });
        }
    }
    // Knee cop (strapped over the hakama).
    let knee = if hakama { 0.036 } else { 0.016 };
    kit.pair(ShinR, Mat::PaintB, || {
        let mut m = shin.plate(
            (FRAC_PI_2 - 0.9, FRAC_PI_2 + 0.9),
            (0.45, 0.565),
            knee,
            s.b(if hakama { 0.006 } else { 0.014 }),
            12,
            8,
        );
        m.append(
            &shin.plate(
                (FRAC_PI_2 - 0.8, FRAC_PI_2 + 0.8),
                (0.42, 0.455),
                knee - 0.004,
                s.b(0.004),
                10,
                3,
            ),
            Mat4::IDENTITY,
        );
        m
    });
    kit.pair(ShinR, Mat::Glow, || {
        let p = shin.at(0.52, FRAC_PI_2, knee + s.t + 0.012);
        disk(0.009, 0.003, Vec3::NEG_Z, p, 16)
    });
    // Greave: on the hakama styles, splints (narrow plates over a leather backing).
    if hakama {
        let splint = |k: i32| {
            let mid = 1.0 - k.abs() as f32 / 2.0;
            let phi = FRAC_PI_2 + k as f32 * 0.47;
            let y = (0.215, 0.4 + 0.022 * mid);
            let g = move |v: f32| 0.016 + 0.003 * mid + 0.002 * v;
            (phi, y, g)
        };
        let pl = Pl::new(s.t * 0.8, 0.003, s.edge_a);
        kit.pair(ShinR, Mat::Leather, || {
            shin.plate(
                (FRAC_PI_2 - 1.3, FRAC_PI_2 + 1.3),
                (0.2, 0.415),
                0.009,
                Pl::new(0.003, 0.0, 1.1),
                18,
                6,
            )
        });
        kit.pair(ShinR, Mat::PaintA, || {
            let mut m = MeshBuilder::new();
            for k in -2..=2 {
                let (phi, y, g) = splint(k);
                m.append(
                    &shin.shaped((phi - 0.19, phi + 0.19), |_| y, |_, v| g(v), pl, 4, 8),
                    Mat4::IDENTITY,
                );
            }
            m
        });
        kit.pair(ShinR, Mat::Glow, || {
            // Down the middle splint, on its pillowed face.
            let (phi, y, g) = splint(0);
            let span = (0.25, 0.39);
            shin.shaped(
                (phi - 0.03, phi + 0.03),
                |_| span,
                |_, v| {
                    let w = (lerp(span.0, span.1, v) - y.0) / (y.1 - y.0);
                    g(w) + pl.t + pl.bulge * (PI * w).sin() + 0.0012
                },
                Pl::strip(),
                1,
                8,
            )
        });
    } else {
        // Over the pilot's trousers.
        let lift = if style == Style::Pilot || style == Style::Grunt {
            0.005
        } else {
            0.0
        };
        kit.pair(ShinR, Mat::PaintA, || {
            shin.shaped_panels(
                (FRAC_PI_2 - 1.2, FRAC_PI_2 + 1.2),
                |u| (0.2 + 0.03 * (u - 0.5).abs() * 2.0, 0.425),
                |u, _| lift + 0.009 + 0.006 * (1.0 - (2.0 * u - 1.0).abs()),
                s.a(0.006),
                14,
                10,
                &[0.5],
                &[],
            )
        });
        kit.pair(ShinR, Mat::Glow, || {
            shin.plate(
                (FRAC_PI_2 - 0.035, FRAC_PI_2 + 0.035),
                (0.24, 0.4),
                lift + 0.009 + s.t + 0.0045,
                Pl::strip(),
                1,
                8,
            )
        });
    }
    // Gaiter over the boot top.
    kit.pair(ShinR, Mat::Leather, || {
        let mut m = shin.tube(0.1, 0.205, 3, 28, 0.009, 0.0, 0.0);
        m.append(
            &shin.plate(
                (-PI + 0.3, PI - 0.3),
                (0.175, 0.195),
                0.013,
                Pl::new(0.004, 0.0, 1.2),
                24,
                1,
            ),
            Mat4::IDENTITY,
        );
        m
    });
}

fn boots(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let style = s.style;
    let y = 0.014;
    let boot = Vol::z(&[
        (-0.2, [0.095, 0.03 + y, 0.03, 0.018, 0.026, 2.4]),
        (-0.17, [0.094, 0.036 + y, 0.042, 0.025, 0.033, 2.8]),
        (-0.12, [0.093, 0.042 + y, 0.049, 0.032, 0.04, 3.2]),
        (-0.06, [0.094, 0.055 + y, 0.047, 0.047, 0.052, 3.2]),
        (0.0, [0.095, 0.072 + y, 0.044, 0.065, 0.07, 3.0]),
        (0.04, [0.096, 0.075 + y, 0.042, 0.06, 0.073, 2.8]),
        (0.07, [0.096, 0.07 + y, 0.036, 0.05, 0.068, 2.5]),
    ])
    .scaled(s.bulk.max(1.0));
    let shell = if style == Style::Pilot {
        Mat::Carbon
    } else {
        Mat::PaintB
    };
    kit.pair(FootR, shell, || {
        boot.tube(-0.2, 0.07, 10, 20, 0.0, 0.012, 0.012)
    });
    // Sole: a beveled slab under the boot.
    kit.pair(FootR, Mat::Rubber, || {
        let outline: Vec<Vec2> = (0..40)
            .map(|i| {
                let a = i as f32 / 40.0 * TAU;
                let (sn, cs) = a.sin_cos();
                // Footprint: narrow heel (+z), wide ball, rounded toe (−z).
                let z = -0.065
                    + 0.15 * sn.signum() * sn.abs().powf(0.8) * if sn > 0.0 { 0.95 } else { 0.98 };
                let half = lerp(0.04, 0.054, smooth(0.07, -0.1, z))
                    * (1.0 - smooth(-0.15, -0.215, z) * 0.45);
                Vec2::new(0.095 + cs.signum() * cs.abs().powf(0.7) * half, -z)
            })
            .collect();
        let m = MeshBuilder::extrude(&outline, 0.022, 0.004);
        let mut m = m.transformed(Mat4::from_rotation_x(-FRAC_PI_2));
        m.planar_uvs(1.0);
        m
    });
    kit.pair(FootR, Mat::PaintA, || {
        let mut m = plate(12, 6, s.a(0.004), boot.center(-0.13), |u, v| {
            boot.at(lerp(-0.205, -0.085, v), lerp(0.25, PI - 0.25, u), 0.004)
        });
        m.append(
            &plate(10, 5, s.a(0.003), boot.center(0.0), |u, v| {
                boot.at(lerp(-0.07, 0.01, v), lerp(0.45, PI - 0.45, u), 0.004)
            }),
            Mat4::IDENTITY,
        );
        m
    });
    kit.pair(FootR, Mat::Trim, || {
        let mut m = plate(
            10,
            4,
            Pl::new(0.006, 0.002, 1.0),
            boot.center(0.05),
            |u, v| boot.at(lerp(0.035, 0.074, v), lerp(-0.4, PI + 0.4, u), 0.004),
        );
        for side in [-1.0f32, 1.0] {
            let p = v3(0.095 + side * 0.052 * s.bulk.max(1.0), 0.088, 0.0);
            m.append(
                &disk(
                    0.006,
                    0.006,
                    v3(side, 0.0, 0.0),
                    p + v3(side * 0.006, 0.0, 0.0),
                    10,
                ),
                Mat4::IDENTITY,
            );
        }
        m
    });
    kit.pair(FootR, Mat::PaintB, || {
        let mut m = MeshBuilder::new();
        for side in [-1.0f32, 1.0] {
            let p = v3(0.095 + side * 0.048 * s.bulk.max(1.0), 0.088, 0.0);
            m.append(
                &disk(0.026, 0.008, v3(side, 0.0, 0.0), p, 20),
                Mat4::IDENTITY,
            );
        }
        m
    });
    kit.pair(FootR, Mat::Glow, || {
        let mut m = MeshBuilder::new();
        for side in [-1.0f32, 1.0] {
            let p = v3(0.095 + side * 0.0525 * s.bulk.max(1.0), 0.088, 0.0);
            let ring = spun(
                &[
                    Vec2::new(0.016, -0.001),
                    Vec2::new(0.019, -0.001),
                    Vec2::new(0.019, 0.001),
                    Vec2::new(0.016, 0.001),
                ],
                20,
                v3(side, 0.0, 0.0),
                p,
            );
            m.append(&ring, Mat4::IDENTITY);
        }
        m
    });
}

fn helmet_shell(style: Style) -> Shell {
    let base = Shell {
        c: v3(0.0, 1.69, 0.0),
        rx: 0.106,
        up: 0.13,
        down: 0.12,
        front: 0.121,
        back: 0.123,
        e: 2.4,
    };
    match style {
        Style::Grunt => Shell {
            rx: 0.114,
            up: 0.138,
            front: 0.124,
            back: 0.126,
            e: 2.2,
            ..base
        },
        Style::Pilot => Shell {
            rx: 0.102,
            up: 0.124,
            front: 0.118,
            back: 0.121,
            e: 2.6,
            ..base
        },
        Style::Elite => Shell {
            rx: 0.1,
            up: 0.128,
            front: 0.12,
            back: 0.127,
            e: 2.6,
            ..base
        },
        _ => base,
    }
}

/// The signed angle from `a` to `b`, wrapped to −π..π.
fn wrap_angle(d: f32) -> f32 {
    (d + PI).rem_euclid(TAU) - PI
}

/// A point on the helmet's center line: `a` < 0 toward the face, > 0 toward the back (|a| is
/// the polar angle), moved `x` m sideways.
fn sagittal(h: &Shell, a: f32, x: f32, g: f32) -> Vec3 {
    let base = if a < 0.0 {
        h.at(FRAC_PI_2, -a, g)
    } else {
        h.at(FRAC_PI_2 + PI, a, g)
    };
    base + Vec3::X * x
}

/// Something small sitting on the helmet at (`phi`, `th`), `g` out: its frame (position,
/// across, down, out).
fn on_shell(h: &Shell, phi: f32, th: f32, g: f32) -> (Vec3, Vec3, Vec3, Vec3) {
    let p = h.at(phi, th, g);
    let e = 1e-3;
    let across = (h.at(phi - e, th, g) - h.at(phi + e, th, g)).normalize_or(Vec3::X);
    let down = (h.at(phi, th + e, g) - h.at(phi, th - e, g)).normalize_or(Vec3::NEG_Y);
    let out = across.cross(down).normalize_or(Vec3::NEG_Z);
    (p, across, down, out)
}

/// A row of dark vent slits on the helmet, centered on (`phi`, `th`).
fn shell_vents(h: &Shell, phi: f32, th: f32, g: f32, n: usize, step: f32) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    for k in 0..n {
        let t = th + (k as f32 - (n as f32 - 1.0) * 0.5) * step;
        let (p, across, down, _) = on_shell(h, phi, t, g);
        m.append(
            &MeshBuilder::chamfer_box(v3(0.026, 0.0045, 0.004), 0.0012).transformed(basis(
                across,
                -down,
                across.cross(-down),
                p,
            )),
            Mat4::IDENTITY,
        );
    }
    m
}

/// A shikoro: a helmet's neck guard, `n` overlapping lames flaring out from the skull below
/// the polar angle `rim`, around the back between the azimuths `phi`.
#[derive(Debug, Clone, Copy)]
struct Shikoro {
    h: Shell,
    rim: f32,
    phi: (f32, f32),
    /// Outward run per meter of drop.
    slope: f32,
    /// How far the first lame stands off the skull.
    g: f32,
    /// Lame height, overlap and thickness.
    lh: f32,
    lap: f32,
    t: f32,
    n: usize,
}

impl Shikoro {
    /// The point at azimuth `phi`, `d` below the rim, `g` out.
    fn at(&self, phi: f32, d: f32, g: f32) -> Vec3 {
        let out = v3(phi.cos(), 0.0, -phi.sin());
        self.h.at(phi, self.rim, 0.0) + out * (d * self.slope + g) - Vec3::Y * d
    }

    /// Drop range and offset of lame `k` (each lies one thickness over the one above).
    fn lame(&self, k: usize) -> ((f32, f32), f32) {
        let d0 = k as f32 * (self.lh - self.lap);
        ((d0, d0 + self.lh), self.g + k as f32 * (self.t + 0.0012))
    }

    fn plates(&self, bulge: f32, edge: f32) -> MeshBuilder {
        let mut m = MeshBuilder::new();
        for k in 0..self.n {
            let (d, g) = self.lame(k);
            m.append(
                &plate(30, 3, Pl::new(self.t, bulge, edge), self.h.c, |u, v| {
                    self.at(lerp(self.phi.0, self.phi.1, u), lerp(d.0, d.1, v), g)
                }),
                Mat4::IDENTITY,
            );
        }
        m
    }

    /// Pairs of cords down every lame at the angles `cols` from the back.
    fn lacing(&self, cols: &[f32]) -> MeshBuilder {
        let back = (self.phi.0 + self.phi.1) * 0.5;
        let r = self.h.rx + self.g + 0.02;
        let (w, sep) = (0.0019 / r, 0.0042 / r);
        let mut m = MeshBuilder::new();
        for k in 0..self.n {
            let (d, g) = self.lame(k);
            let g = g + self.t + 0.0016;
            for &c in cols {
                for side in [-1.0f32, 1.0] {
                    let a = back + c + side * sep;
                    let mut cord = surface(1, 3, |u, v| {
                        self.at(lerp(a - w, a + w, u), lerp(d.0 + 0.003, d.1 - 0.002, v), g)
                    });
                    face_out(&mut cord, self.h.c);
                    m.append(&cord, Mat4::IDENTITY);
                }
            }
        }
        m
    }

    /// A point on the wing (the top lames' front end turned back over the guard) at the
    /// end `end`: `u` 0..1 from its folded front edge back over `span` radians, `v` 0..1
    /// down its `drop`, `lift` off its face. It lifts off the guard toward the back.
    fn wing_at(&self, end: f32, span: f32, drop: f32, u: f32, v: f32, lift: f32) -> Vec3 {
        let into = if end == self.phi.0 { 1.0 } else { -1.0 };
        let g = self.g + self.n as f32 * (self.t + 0.0012) + 0.001;
        let h = drop * (1.0 - 0.3 * u * u);
        self.at(
            end + into * lerp(-0.1, span, u),
            v * h - 0.006,
            g + 0.014 * u + lift,
        )
    }

    /// The two wings.
    fn wings(&self, span: f32, drop: f32, pl: Pl) -> MeshBuilder {
        let mut m = MeshBuilder::new();
        for end in [self.phi.0, self.phi.1] {
            m.append(
                &plate(8, 6, pl, self.h.c, |u, v| {
                    self.wing_at(end, span, drop, u, v, 0.0)
                }),
                Mat4::IDENTITY,
            );
        }
        m
    }

    /// A round crest badge on each wing, `off` out from its face.
    fn crests(&self, span: f32, drop: f32, off: f32) -> MeshBuilder {
        let mut m = MeshBuilder::new();
        for end in [self.phi.0, self.phi.1] {
            let c = self.wing_at(end, span, drop, 0.45, 0.45, off);
            let n = (self.wing_at(end, span, drop, 0.45, 0.45, off + 0.01) - c).normalize();
            m.append(&disk(0.012, 0.003, n, c, 16), Mat4::IDENTITY);
        }
        m
    }

    /// A glowing line along the bottom lame's lower edge.
    fn hem(&self) -> MeshBuilder {
        let (d, g) = self.lame(self.n - 1);
        plate(30, 1, Pl::strip(), self.h.c, |u, v| {
            self.at(
                lerp(self.phi.0, self.phi.1, u),
                lerp(d.1 - 0.0075, d.1 - 0.0035, v),
                g + self.t + 0.0012,
            )
        })
    }
}

fn helmet(kit: &mut Kit, s: &Spec) {
    use Bone::Head;
    let style = s.style;
    let h = helmet_shell(style);
    let fr = FRAC_PI_2;
    let inner = if style == Style::Elite {
        Mat::Carbon
    } else {
        Mat::Suit
    };
    // The under-helmet: a closed dome that hides every gap between plates.
    kit.bind(Head, inner, || {
        surface(36, 14, |u, v| h.at(-u * TAU, lerp(0.001, 2.65, v), -0.006))
    });
    // The skull: two halves meeting at a seam down the back and a crown/skirt seam at ear
    // level, open over the face below the brow. `ridge` raises a keel along the center
    // line (0 = none).
    let skull = move |pl: Pl, face: f32, low: f32, ridge: f32| {
        split_plate(
            40,
            16,
            pl,
            h.c,
            &[0.5],
            &[0.6],
            (0.004, 0.012),
            move |u, v| {
                let th = lerp(0.04, low, v);
                let open = face * smooth(0.8, 1.1, th);
                let phi = lerp(fr + 0.025 + open, fr + TAU - 0.025 - open, u);
                let d = wrap_angle(phi - fr)
                    .abs()
                    .min(wrap_angle(phi - fr - PI).abs());
                let keel = (1.0 - d / 0.3).max(0.0).powi(2) * (1.0 - smooth(1.0, 1.7, th));
                h.at(phi, th, ridge * keel)
            },
        )
    };
    match style {
        Style::Duelist => {
            // A ribbed bowl (suji-bachi): plain plates under raised ribs converging on a
            // gilt rosette at the crown, the three down the front gilt; a brow band fills
            // the bowl's front above the visor.
            kit.bind(Head, Mat::PaintA, || {
                let mut m = skull(s.a(0.0), 0.72, 2.5, 0.0);
                m.append(
                    &h.plate((fr - 0.86, fr + 0.86), (0.84, 1.26), 0.0, s.a(0.0), 18, 5),
                    Mat4::IDENTITY,
                );
                m
            });
            const RIBS: usize = 32;
            let gilt = |k: usize| matches!(k, 0 | 1 | 31);
            // A rib: a rounded bead 2 mm high, sinking into the bowl at both ends.
            let rib = move |k: usize| {
                let phi = fr + k as f32 * TAU / RIBS as f32;
                let low = lerp(1.1, 1.48, smooth(0.7, 1.3, wrap_angle(phi - fr).abs()));
                let mut m = surface(4, 10, move |u, v| {
                    let rise = (PI * u).sin().max(0.0).sqrt()
                        * smooth(0.0, 0.08, v)
                        * smooth(1.0, 0.92, v);
                    let g = s.t - 0.0004 + 0.0022 * rise;
                    h.at(phi + (u - 0.5) * 0.045, lerp(0.17, low, v), g)
                });
                face_out(&mut m, h.c);
                m
            };
            kit.bind(Head, Mat::PaintA, || {
                let mut m = MeshBuilder::new();
                for k in (0..RIBS).filter(|&k| !gilt(k)) {
                    m.append(&rib(k), Mat4::IDENTITY);
                }
                m
            });
            // The peak (mabizashi) over the visor, its root riveted to the brow band.
            let root = 1.16;
            let peak = move |u: f32, v: f32| {
                let c = 2.0 * u - 1.0;
                let phi = fr + c * 1.05;
                let len = 0.008 + 0.034 * (1.0 - c * c).sqrt();
                let out = v3(phi.cos(), 0.0, -phi.sin());
                h.at(phi, root, s.t - 0.001) + out * (len * v)
                    - Vec3::Y * (0.16 * len * v - 0.004 * v * v)
            };
            let peak_up = move |u: f32, v: f32| {
                let e = 1e-3;
                let n = (peak(u + e, v) - peak(u - e, v)).cross(peak(u, v + e) - peak(u, v - e));
                let n = n.normalize_or(Vec3::Y);
                if n.y < 0.0 { -n } else { n }
            };
            let peak_t = 0.0045;
            kit.bind(Head, Mat::PaintB, || {
                let mut m = plate(20, 4, Pl::new(peak_t, 0.0, s.edge_b), h.c, peak);
                // The crest's mount, centered over the peak.
                m.append(
                    &h.plate(
                        (fr - 0.2, fr + 0.2),
                        (0.9, 1.13),
                        s.t - 0.0004,
                        Pl::new(0.0035, 0.0015, s.edge_b),
                        6,
                        5,
                    ),
                    Mat4::IDENTITY,
                );
                m
            });
            // Gilt fittings: the ribs down the front, the crown rosette, the peak's edge
            // and rivets, a crest badge and the horns (kuwagata) rising off it.
            kit.bind(Head, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                for k in (0..RIBS).filter(|&k| gilt(k)) {
                    m.append(&rib(k), Mat4::IDENTITY);
                }
                let crown = h.at(0.0, 0.0, s.t - 0.0006);
                let rosette = [
                    Vec2::new(0.024, -0.004),
                    Vec2::new(0.02, -0.0015),
                    Vec2::new(0.013, 0.0025),
                    Vec2::new(0.008, 0.004),
                    Vec2::new(0.0005, 0.004),
                ];
                m.append(&spun(&rosette, 24, Vec3::Y, crown), Mat4::IDENTITY);
                m.append(
                    &plate(20, 1, Pl::new(0.002, 0.0, 1.0), h.c, move |u, v| {
                        peak(u, lerp(0.84, 0.97, v)) + peak_up(u, 0.9) * (peak_t + 0.0001)
                    }),
                    Mat4::IDENTITY,
                );
                for u in [0.12, 0.31, 0.5, 0.69, 0.88] {
                    let n = peak_up(u, 0.16);
                    m.append(
                        &disk(0.003, 0.0022, n, peak(u, 0.16) + n * peak_t, 8),
                        Mat4::IDENTITY,
                    );
                }
                let (badge, _, _, out) = on_shell(&h, fr, 1.0, s.t + 0.0034);
                m.append(&disk(0.014, 0.0035, out, badge, 20), Mat4::IDENTITY);
                for side in [-1.0f32, 1.0] {
                    let at = |t: f32| {
                        badge
                            + v3(side * (0.011 + 0.056 * t.powf(1.4)), 0.15 * t, 0.0)
                            + out * (0.008 * t - 0.034 * t * t)
                    };
                    let sections: Vec<Section> = (0..=12)
                        .map(|i| {
                            let t = i as f32 / 12.0;
                            let tangent = (at(t + 0.01) - at(t - 0.01)).normalize_or(Vec3::Y);
                            let half = 0.0065 - 0.004 * t + 0.002 * (PI * t).sin();
                            Section::rounded(at(t), Vec2::new(half, 0.0014))
                                .with_exponent(2.6)
                                .with_right(tangent.cross(out).normalize_or(Vec3::X))
                        })
                        .collect();
                    m.append(
                        &MeshBuilder::loft(&sections, 10, Cap::Round, Cap::Round),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Head, Mat::Lens, || {
                let (p, _, _, out) = on_shell(&h, fr, 1.0, s.t + 0.0034);
                disk(0.0055, 0.0012, out, p + out * 0.0018, 14)
            });
            // The menpo: a sculpted half mask with a nose and high cheekbones, cut across at
            // an open mouth; the jaw below has a jutting lower lip and chin.
            let face_g = move |phi: f32, th: f32| {
                let d = wrap_angle(phi - fr);
                let nose = (-(d / 0.075).powi(2)).exp()
                    * smooth(1.6, 1.84, th)
                    * (1.0 - smooth(1.86, 1.94, th));
                let cheek =
                    (-((d.abs() - 0.42) / 0.16).powi(2) - ((th - 1.76) / 0.14).powi(2)).exp();
                let chin = (-(d / 0.24).powi(2) - ((th - 2.3) / 0.12).powi(2)).exp();
                0.004 + 0.013 * smooth(1.8, 2.44, th) + 0.02 * nose + 0.009 * cheek + 0.006 * chin
            };
            let face_pl = Pl::new(0.006, 0.0, s.edge_a);
            // The upper lip and the lower lip at u across the face: the mouth is open in
            // the middle and closes toward its corners.
            let mouth = |u: f32| 1.96 + 0.12 * smooth(0.15, 0.4, (u - 0.5).abs());
            let jaw_top =
                move |u: f32| mouth(u) + lerp(0.1, 0.015, smooth(0.1, 0.4, (u - 0.5).abs()));
            let lip = jaw_top(0.5);
            let jaw_g = move |phi: f32, th: f32| {
                face_g(phi, th) + 0.004 * (1.0 - smooth(lip, lip + 0.12, th))
            };
            kit.bind(Head, Mat::PaintA, || {
                let mut m = plate(22, 12, face_pl, h.c, move |u, v| {
                    let th = lerp(1.6, mouth(u), v);
                    let w = lerp(0.84, 0.66, smooth(1.6, 2.1, th));
                    let phi = lerp(fr - w, fr + w, u);
                    h.at(phi, th, face_g(phi, th))
                });
                m.append(
                    &plate(18, 8, face_pl, h.c, move |u, v| {
                        let th = lerp(jaw_top(u), 2.46, v);
                        let w = lerp(0.64, 0.4, smooth(lip, 2.46, th));
                        let phi = lerp(fr - w, fr + w, u);
                        h.at(phi, th, jaw_g(phi, th))
                    }),
                    Mat4::IDENTITY,
                );
                m
            });
            // In the mouth, a gilt grille; on the cheeks, the pegs a helmet cord ties to;
            // under the chin, a drain.
            kit.bind(Head, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                for i in 0..9 {
                    let phi = fr + (i as f32 - 4.0) * 0.045;
                    let th = (mouth(0.5) + lip) * 0.5;
                    let (p, across, down, _) = on_shell(&h, phi, th, face_g(phi, th) - 0.003);
                    m.append(
                        &block(v3(0.0022, 0.013, 0.004), 0.0007, across, -down, p),
                        Mat4::IDENTITY,
                    );
                }
                for side in [-1.0f32, 1.0] {
                    let (phi, th) = (fr + side * 0.58, 2.0);
                    let (p, _, _, out) = on_shell(&h, phi, th, face_g(phi, th) + face_pl.t);
                    m.append(&disk(0.0045, 0.005, out, p, 10), Mat4::IDENTITY);
                }
                let th = 2.42;
                let (p, _, down, out) = on_shell(&h, fr, th, jaw_g(fr, th) + face_pl.t);
                m.append(
                    &disk(0.0035, 0.007, (out + down).normalize(), p, 10),
                    Mat4::IDENTITY,
                );
                m
            });
            // The throat guard (yodare-kake): laced lames hanging from under the jaw.
            let throat = Shikoro {
                h,
                rim: 2.36,
                phi: (fr - 0.62, fr + 0.62),
                slope: 0.45,
                g: 0.004,
                lh: 0.022,
                lap: 0.006,
                t: 0.0045,
                n: 3,
            };
            kit.bind(Head, Mat::PaintA, || throat.plates(0.0012, s.edge_a));
            kit.bind(Head, Mat::Wrap, || throat.lacing(&[-0.36, 0.0, 0.36]));
            kit.bind(Head, Mat::Glow, || throat.hem());
            // The neck guard: laced lames flaring out around the back, their front ends
            // turned back as wings.
            let shikoro = Shikoro {
                h,
                rim: 1.62,
                phi: (fr + 1.0, fr + TAU - 1.0),
                slope: 0.5,
                g: 0.018,
                lh: 0.034,
                lap: 0.008,
                t: 0.0055,
                n: 4,
            };
            let (span, drop) = (0.5, 0.072);
            kit.bind(Head, Mat::PaintA, || shikoro.plates(0.0015, s.edge_a));
            kit.bind(Head, Mat::Wrap, || {
                shikoro.lacing(&[-1.6, -0.8, 0.0, 0.8, 1.6])
            });
            kit.bind(Head, Mat::PaintB, || shikoro.wings(span, drop, s.b(0.002)));
            kit.bind(Head, Mat::Trim, || {
                shikoro.crests(span, drop, s.t * 0.85 + 0.0015)
            });
            kit.bind(Head, Mat::Glow, || shikoro.hem());
            // Wide visor: dark glass with a glowing sight line.
            kit.bind(Head, Mat::Glass, || {
                h.plate(
                    (fr - 0.84, fr + 0.84),
                    (1.24, 1.64),
                    -0.002,
                    Pl::new(0.006, 0.002, 1.0),
                    16,
                    4,
                )
            });
            kit.bind(Head, Mat::Lens, || {
                let mut m = h.plate(
                    (fr - 0.6, fr + 0.6),
                    (1.425, 1.44),
                    0.0048,
                    Pl::new(0.0012, 0.0, 1.0),
                    14,
                    1,
                );
                for side in [-1.0f32, 1.0] {
                    m.append(
                        &h.plate(
                            (fr + side * 0.66 - 0.06, fr + side * 0.66 + 0.06),
                            (1.38, 1.47),
                            0.0048,
                            Pl::new(0.0012, 0.0, 1.0),
                            3,
                            2,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
        }
        Style::Elite => {
            // A black-lacquered riveted bowl (hoshi-bachi) over a demon's face (an oni
            // somen): heavy brows over slanted glowing eyes, a short broad nose, snarl folds and
            // an open mouth with fangs and embers inside, lips lacquered crimson.
            kit.bind(Head, Mat::PaintA, || skull(s.a(0.0), 0.82, 2.45, 0.012));
            // The bowl's plates meet at flanged seams, each under a row of domed rivets
            // shrinking toward the crown; the seams over the face stop at the brow plate.
            const SEAMS: usize = 20;
            let seam = |k: usize| fr + (k as f32 + 0.5) * TAU / SEAMS as f32;
            let seam_low = |phi: f32| {
                if wrap_angle(phi - fr).abs() < 1.0 {
                    0.78
                } else {
                    1.72
                }
            };
            let flange_g = s.t - 0.0004;
            kit.bind(Head, Mat::PaintA, || {
                let mut m = MeshBuilder::new();
                for k in 0..SEAMS {
                    let phi = seam(k);
                    let low = seam_low(phi);
                    let mut f = surface(3, 12, move |u, v| {
                        let rise = (PI * u).sin().max(0.0).sqrt()
                            * smooth(0.0, 0.06, v)
                            * smooth(1.0, 0.94, v);
                        h.at(
                            phi + (u - 0.5) * 0.036,
                            lerp(0.16, low, v),
                            flange_g + 0.0012 * rise,
                        )
                    });
                    face_out(&mut f, h.c);
                    m.append(&f, Mat4::IDENTITY);
                }
                m
            });
            kit.bind(Head, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                for k in 0..SEAMS {
                    let phi = seam(k);
                    let low = seam_low(phi) - 0.05;
                    let n = ((low - 0.3) / 0.16).round().max(1.0) as usize;
                    for i in 0..=n {
                        let th = lerp(0.3, low, i as f32 / n as f32);
                        let r = lerp(0.0021, 0.0037, smooth(0.25, 1.4, th));
                        let (p, _, _, out) = on_shell(&h, phi, th, flange_g + 0.0009);
                        let dome = [
                            Vec2::new(r, -0.0006),
                            Vec2::new(r * 0.72, r * 0.42),
                            Vec2::new(0.0004, r * 0.62),
                        ];
                        m.append(&spun(&dome, 6, out, p), Mat4::IDENTITY);
                    }
                }
                // The crown's rosette (tehen kanamono).
                let crown = h.at(0.0, 0.0, s.t - 0.0006);
                let rosette = [
                    Vec2::new(0.021, -0.004),
                    Vec2::new(0.018, -0.0012),
                    Vec2::new(0.011, 0.0024),
                    Vec2::new(0.006, 0.0036),
                    Vec2::new(0.0005, 0.0036),
                ];
                m.append(&spun(&rosette, 20, Vec3::Y, crown), Mat4::IDENTITY);
                // The crest's mount on the brow.
                let (badge, _, _, out) = on_shell(&h, fr, 0.9, 0.031);
                m.append(&disk(0.011, 0.004, out, badge, 18), Mat4::IDENTITY);
                m
            });
            // Scowling crimson brow plate, dipping between the eyes, with a crescent crest
            // (maedate) rising off it and swept blades off the back of the bowl.
            kit.bind(Head, Mat::PaintB, || {
                let mut m = plate(16, 6, s.b(0.003), h.c, |u, v| {
                    let phi = lerp(fr - 0.95, fr + 0.95, u);
                    let dip = 1.0 - (wrap_angle(phi - fr) / 0.95).abs();
                    h.at(
                        phi,
                        lerp(0.8, 1.16 + 0.1 * dip, v),
                        0.006 + 0.012 * dip + 0.01 * v,
                    )
                });
                let (badge, _, _, out) = on_shell(&h, fr, 0.9, 0.031);
                let base = badge + out * 0.003;
                let sections: Vec<Section> = (0..=20)
                    .map(|i| {
                        let x = lerp(-1.0, 1.0, i as f32 / 20.0);
                        let a = x.abs();
                        let up = 0.012 + 0.1 * a.powf(1.6);
                        let p = base + v3(x * 0.16, up, up * 0.55 + 0.01 * a);
                        let half = lerp(0.011, 0.0012, a.powf(0.8));
                        Section::rounded(p, Vec2::new(half, 0.0022))
                            .with_exponent(2.6)
                            .with_right(v3(0.0, 1.0, -0.4))
                    })
                    .collect();
                m.append(
                    &MeshBuilder::loft(&sections, 10, Cap::Round, Cap::Round),
                    Mat4::IDENTITY,
                );
                for side in [-1.0f32, 1.0] {
                    let path: Vec<Vec3> = (0..12)
                        .map(|i| {
                            let t = i as f32 / 11.0;
                            sagittal(&h, lerp(0.7, 1.45, t), side * 0.03, 0.006)
                                + v3(side * 0.012 * t, 0.015 * t, 0.07 * t * t)
                        })
                        .collect();
                    m.append(
                        &fin(
                            &path,
                            0.004,
                            |t| 0.006 + 0.03 * t * (1.0 - 0.5 * t),
                            v3(side, 0.15, 0.0),
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            // The demon's face: its height off the shell at (phi, th).
            let eye_th = |a: f32| lerp(1.47, 1.33, ((a - 0.09) / 0.41).clamp(0.0, 1.0));
            let bump = |x: f32, w: f32| (-(x / w).powi(2)).exp();
            let face_g = move |phi: f32, th: f32| {
                let d = wrap_angle(phi - fr);
                let a = d.abs();
                // Heavy brows with a hard lower edge, meeting low between the eyes.
                let tb = 1.39 - (a - 0.09) * 0.34;
                let brow = smooth(tb - 0.1, tb - 0.01, th)
                    * (1.0 - smooth(tb, tb + 0.05, th))
                    * (1.0 - smooth(0.5, 0.64, a));
                // Deep-set eyes.
                let socket = bump(a - 0.3, 0.16) * bump(th - eye_th(a) - 0.01, 0.05);
                // A short, broad nose swelling to a heavy tip between flared wings.
                let nose = bump(d, lerp(0.06, 0.11, smooth(1.6, 1.82, th)))
                    * (0.003 + 0.02 * smooth(1.58, 1.81, th))
                    * (1.0 - smooth(1.84, 1.91, th))
                    + 0.006 * bump(d, 0.08) * bump(th - 1.8, 0.045);
                let wings = bump(a - 0.12, 0.045) * bump(th - 1.84, 0.04);
                // Snarl folds from the nose's wings down to the mouth's corners.
                let fold = bump(th - (1.77 + (a - 0.15) * 1.33), 0.04)
                    * smooth(0.11, 0.16, a)
                    * (1.0 - smooth(0.36, 0.44, a));
                let cheek = bump(a - 0.52, 0.16) * bump(th - 1.68, 0.14);
                let chin = bump(d, 0.2) * bump(th - 2.33, 0.1);
                0.006 + 0.016 * smooth(1.95, 2.46, th) + 0.012 * brow - 0.005 * socket
                    + nose
                    + 0.013 * wings
                    + 0.008 * fold
                    + 0.012 * cheek
                    + 0.008 * chin
            };
            let face_pl = Pl::new(0.006, 0.0, s.edge_a);
            let mask_w = |th: f32| lerp(0.9, 0.36, smooth(1.3, 2.46, th));
            let face_phi = move |u: f32, th: f32| fr + (2.0 * u - 1.0) * mask_w(th);
            // The upper lip and the lower lip at u across the face: a wide snarl, open in
            // the middle; toward the corners the jaw overlaps the cheeks.
            let mouth = |u: f32| 1.92 + 0.1 * smooth(0.1, 0.34, (u - 0.5).abs());
            let jaw_top =
                move |u: f32| mouth(u) + lerp(0.17, -0.012, smooth(0.06, 0.34, (u - 0.5).abs()));
            let lip = jaw_top(0.5);
            let jaw_g = move |phi: f32, th: f32| {
                face_g(phi, th) + 0.004 * (1.0 - smooth(lip, lip + 0.12, th))
            };
            kit.bind(Head, Mat::PaintA, || {
                let mut m = plate(36, 30, face_pl, h.c, move |u, v| {
                    let th = lerp(1.14, mouth(u), v);
                    let phi = face_phi(u, th);
                    h.at(phi, th, face_g(phi, th))
                });
                m.append(
                    &plate(28, 10, face_pl, h.c, move |u, v| {
                        let th = lerp(jaw_top(u), 2.46, v);
                        let phi = face_phi(u, th);
                        h.at(phi, th, jaw_g(phi, th))
                    }),
                    Mat4::IDENTITY,
                );
                m
            });
            // Crimson lacquered lips.
            kit.bind(Head, Mat::PaintB, || {
                let mut m = plate(24, 1, Pl::new(0.0018, 0.0, 1.0), h.c, move |u, v| {
                    let uu = lerp(0.16, 0.84, u);
                    let th = mouth(uu) - lerp(0.03, 0.001, v);
                    let phi = face_phi(uu, th);
                    h.at(phi, th, face_g(phi, th) + face_pl.t - 0.0006)
                });
                m.append(
                    &plate(20, 1, Pl::new(0.0018, 0.0, 1.0), h.c, move |u, v| {
                        let uu = lerp(0.22, 0.78, u);
                        let th = jaw_top(uu) + lerp(0.001, 0.032, v);
                        let phi = face_phi(uu, th);
                        h.at(phi, th, jaw_g(phi, th) + face_pl.t - 0.0006)
                    }),
                    Mat4::IDENTITY,
                );
                m
            });
            // Inside the mouth: crimson lacquer with embers at the back; nostrils.
            kit.bind(Head, Mat::PaintB, || {
                h.plate(
                    (fr - 0.44, fr + 0.44),
                    (1.86, 2.16),
                    0.0,
                    Pl::new(0.002, 0.0, 1.0),
                    14,
                    4,
                )
            });
            kit.bind(Head, Mat::Glass, || {
                let mut m = MeshBuilder::new();
                for side in [-1.0f32, 1.0] {
                    let (phi, th) = (fr + side * 0.055, 1.875);
                    let (p, _, down, out) =
                        on_shell(&h, phi, th, face_g(phi, th) + face_pl.t - 0.0008);
                    m.append(
                        &disk(0.0032, 0.0014, (out * 0.6 + down).normalize(), p, 10),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Head, Mat::Lens, || {
                h.plate(
                    (fr - 0.26, fr + 0.26),
                    (1.98, 2.03),
                    0.0021,
                    Pl::new(0.0012, 0.0, 1.0),
                    10,
                    1,
                )
            });
            // Fangs: canines hanging over the lower lip, small teeth between, tusks
            // rising from the jaw in front of the upper lip.
            let tooth = move |u: f32, upper: bool, len: f32, half: f32, splay: f32| {
                let th = if upper {
                    mouth(u) - 0.006
                } else {
                    jaw_top(u) + 0.006
                };
                let phi = face_phi(u, th);
                let g = if upper {
                    face_g(phi, th) + 0.004
                } else {
                    jaw_g(phi, th) + 0.004
                };
                let (p, across, down, out) = on_shell(&h, phi, th, g);
                let dir = if upper { down } else { -down };
                let sections: Vec<Section> = (0..=5)
                    .map(|i| {
                        let t = i as f32 / 5.0;
                        let c = p + dir * (len * t) + out * (splay * t * t);
                        let w = lerp(half, 0.0004, t.powf(0.9));
                        Section::rounded(c, Vec2::new(w, w * 0.75))
                            .with_exponent(2.2)
                            .with_right(across)
                    })
                    .collect();
                MeshBuilder::loft(&sections, 6, Cap::Flat, Cap::Round)
            };
            kit.bind(Head, Mat::Hamon, || {
                let mut m = MeshBuilder::new();
                for (du, upper, len, half, splay) in [
                    (0.17, true, 0.024, 0.0038, 0.008),
                    (0.1, true, 0.011, 0.0027, 0.002),
                    (0.035, true, 0.01, 0.0026, 0.001),
                    (0.23, false, 0.026, 0.004, 0.009),
                    (0.12, false, 0.008, 0.0022, 0.001),
                    (0.045, false, 0.008, 0.0022, 0.001),
                ] {
                    for side in [-1.0f32, 1.0] {
                        m.append(
                            &tooth(0.5 + side * du, upper, len, half, splay),
                            Mat4::IDENTITY,
                        );
                    }
                }
                m
            });
            // Slanted eye slits under the brows: dark recesses with a red glow.
            let face_top = move |phi: f32, th: f32| face_g(phi, th) + face_pl.t;
            let eye = move |grow: f32, g: f32, t: f32| {
                mirrored(plate(10, 2, Pl::new(t, 0.0, 1.0), h.c, move |u, v| {
                    let phi = lerp(fr - 0.09, fr - 0.5, u);
                    let th = lerp(1.47, 1.33, u) + (v - 0.5) * lerp(0.075, 0.04, u) * grow;
                    h.at(phi, th, face_top(phi, th) + g)
                }))
            };
            kit.bind(Head, Mat::Glass, || eye(1.6, -0.0005, 0.002));
            kit.bind(Head, Mat::Lens, || eye(0.8, 0.0016, 0.0012));
            // The throat guard (yodare-kake): laced lames hanging from under the jaw.
            let throat = Shikoro {
                h,
                rim: 2.38,
                phi: (fr - 0.52, fr + 0.52),
                slope: 0.42,
                g: 0.004,
                lh: 0.022,
                lap: 0.006,
                t: 0.0045,
                n: 3,
            };
            kit.bind(Head, Mat::PaintA, || throat.plates(0.0012, s.edge_a));
            kit.bind(Head, Mat::PaintB, || throat.lacing(&[-0.3, 0.0, 0.3]));
            // A steep, close neck guard laced in crimson, its front ends turned back as
            // crimson wings (fukigaeshi) with crests.
            let shikoro = Shikoro {
                h,
                rim: 1.78,
                phi: (fr + 1.25, fr + TAU - 1.25),
                slope: 0.38,
                g: 0.016,
                lh: 0.034,
                lap: 0.008,
                t: 0.005,
                n: 3,
            };
            let (span, drop) = (0.42, 0.066);
            kit.bind(Head, Mat::PaintA, || shikoro.plates(0.001, s.edge_a));
            kit.bind(Head, Mat::PaintB, || {
                let mut m = shikoro.lacing(&[-1.5, -0.75, 0.0, 0.75, 1.5]);
                m.append(&shikoro.wings(span, drop, s.b(0.002)), Mat4::IDENTITY);
                m
            });
            kit.bind(Head, Mat::Trim, || {
                shikoro.crests(span, drop, s.t * 0.85 + 0.0015)
            });
            kit.bind(Head, Mat::Glow, || {
                let mut m = shikoro.hem();
                m.append(&throat.hem(), Mat4::IDENTITY);
                m
            });
        }
        Style::Grunt => {
            // A brimmed ballistic helmet over a black rubber gas mask: round glass eyes in
            // metal rims, a nose bridge, a muzzle with a voicemitter, filters either side.
            kit.bind(Head, Mat::PaintA, || skull(s.a(0.008), 0.95, 1.95, 0.0));
            // Brim.
            kit.bind(Head, Mat::PaintA, || {
                plate(28, 3, s.a(0.002), h.c, |u, v| {
                    let th = lerp(1.18, 1.32, v);
                    let phi = lerp(fr - 1.3, fr + 1.3, u);
                    let front = (1.0 - (wrap_angle(phi - fr) / 1.3).abs()).max(0.0);
                    h.at(phi, th, 0.01 + 0.035 * front * v)
                })
            });
            // The mask's outer surface off the shell at (phi, th).
            let mask_g = move |phi: f32, th: f32| {
                let d = wrap_angle(phi - fr);
                let muzzle = (-(d / 0.55).powi(2)).exp()
                    * smooth(1.5, 1.95, th)
                    * (1.0 - smooth(2.15, 2.42, th));
                let bridge = (-(d / 0.13).powi(2)).exp()
                    * smooth(1.25, 1.45, th)
                    * (1.0 - smooth(1.5, 1.72, th));
                0.004 + 0.028 * muzzle + 0.009 * bridge
            };
            let mask_pl = Pl::new(0.004, 0.0, 0.9);
            let eye = |side: f32| {
                let phi = fr - side * 0.37;
                on_shell(&h, phi, 1.44, mask_g(phi, 1.44) + mask_pl.t)
            };
            let mouth = || on_shell(&h, fr, 1.98, mask_g(fr, 1.98) + mask_pl.t);
            kit.bind(Head, Mat::Mask, || {
                plate(26, 16, mask_pl, h.c, |u, v| {
                    let phi = lerp(fr - 1.05, fr + 1.05, u);
                    let th = lerp(1.24, 2.4, v);
                    h.at(phi, th, mask_g(phi, th))
                })
            });
            kit.bind(Head, Mat::Rubber, || {
                let mut m = MeshBuilder::new();
                // Intake slats across the voicemitter.
                let (p, across, down, out) = mouth();
                for k in 0..4 {
                    let q = p + out * 0.019 + down * (k as f32 - 1.5) * 0.0085;
                    m.append(
                        &MeshBuilder::chamfer_box(v3(0.04, 0.0032, 0.004), 0.001)
                            .transformed(basis(across, -down, across.cross(-down), q)),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Head, Mat::PaintB, || {
                // Center ridge and ear cups.
                let mut m = plate(3, 14, s.b(0.002), h.c, |u, v| {
                    sagittal(&h, lerp(-1.15, 1.4, v), (u - 0.5) * 0.014, 0.009)
                });
                for side in [-1.0f32, 1.0] {
                    let p = h.at(if side > 0.0 { 0.0 } else { PI }, 1.6, 0.0);
                    m.append(
                        &disk(
                            0.034,
                            0.022,
                            v3(side, 0.0, 0.0),
                            p + v3(side * 0.008, 0.0, 0.0),
                            24,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                // Filter canisters on the muzzle's sides, angled down and back.
                for side in [-1.0f32, 1.0] {
                    let (p, across, down, out) = on_shell(&h, fr - side * 0.5, 2.02, 0.0);
                    let dir = (out * 0.75 + across * side * 0.35 + down * 0.45).normalize();
                    m.append(
                        &spun(
                            &[
                                Vec2::new(0.004, 0.0),
                                Vec2::new(0.028, 0.0),
                                Vec2::new(0.031, 0.006),
                                Vec2::new(0.031, 0.034),
                                Vec2::new(0.026, 0.04),
                                Vec2::new(0.004, 0.04),
                            ],
                            22,
                            dir,
                            p + out * (mask_g(fr - side * 0.5, 2.02) + 0.006),
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Head, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                // Eye rims.
                for side in [-1.0f32, 1.0] {
                    let (p, _, _, out) = eye(side);
                    m.append(
                        &spun(
                            &[
                                Vec2::new(0.023, 0.007),
                                Vec2::new(0.03, 0.006),
                                Vec2::new(0.032, -0.002),
                                Vec2::new(0.028, -0.004),
                            ],
                            28,
                            out,
                            p,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                // The voicemitter: a short drum on the muzzle.
                let (p, _, _, out) = mouth();
                m.append(
                    &spun(
                        &[
                            Vec2::new(0.024, -0.004),
                            Vec2::new(0.03, -0.004),
                            Vec2::new(0.031, 0.016),
                            Vec2::new(0.026, 0.019),
                            Vec2::new(0.024, 0.016),
                        ],
                        24,
                        out,
                        p,
                    ),
                    Mat4::IDENTITY,
                );
                // Filter caps.
                for side in [-1.0f32, 1.0] {
                    let (q, across, down, o) = on_shell(&h, fr - side * 0.5, 2.02, 0.0);
                    let dir = (o * 0.75 + across * side * 0.35 + down * 0.45).normalize();
                    let base = q + o * (mask_g(fr - side * 0.5, 2.02) + 0.006);
                    m.append(
                        &disk(0.027, 0.004, dir, base + dir * 0.041, 20),
                        Mat4::IDENTITY,
                    );
                }
                // A lamp on the left of the brim.
                let (p, _, _, out) = on_shell(&h, fr + 0.9, 1.15, 0.012);
                m.append(
                    &spun(
                        &[
                            Vec2::new(0.003, 0.0),
                            Vec2::new(0.014, 0.0),
                            Vec2::new(0.015, 0.018),
                            Vec2::new(0.012, 0.02),
                        ],
                        16,
                        out,
                        p,
                    ),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.bind(Head, Mat::Glass, || {
                let mut m = MeshBuilder::new();
                for side in [-1.0f32, 1.0] {
                    let (p, _, _, out) = eye(side);
                    m.append(
                        &disk(0.025, 0.004, out, p + out * 0.003, 28),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Head, Mat::Lens, || {
                let (p, _, _, out) = on_shell(&h, fr + 0.9, 1.15, 0.012);
                disk(0.012, 0.002, out, p + out * 0.019, 16)
            });
        }
        Style::Pilot => {
            // A close helmet: a narrow wraparound visor between a brow ridge and a
            // faceplate with a respirator, an optics cluster over the right eye.
            kit.bind(Head, Mat::PaintA, || skull(s.a(0.005), 0.92, 2.3, 0.0));
            // The visor's top and bottom (polar angles) at azimuth `phi`: it narrows and
            // sweeps up toward the temples.
            let visor = |phi: f32| {
                let c = (wrap_angle(phi - fr) / 0.97).clamp(-1.0, 1.0);
                (1.13 + 0.04 * c * c, 1.54 - 0.17 * c * c)
            };
            let visor_g = |phi: f32| {
                let c = (wrap_angle(phi - fr) / 0.97).clamp(-1.0, 1.0);
                0.004 + 0.007 * (1.0 - c * c)
            };
            kit.bind(Head, Mat::Glass, || {
                plate(22, 6, Pl::new(0.004, 0.002, 1.0), h.c, |u, v| {
                    let phi = lerp(fr - 0.97, fr + 0.97, u);
                    let (t0, t1) = visor(phi);
                    h.at(phi, lerp(t0 - 0.02, t1 + 0.02, v), visor_g(phi))
                })
            });
            // The faceplate: from under the visor to the chin, narrowing, with a soft keel
            // down the middle; it stands off the skull toward the chin.
            let mask_g = move |phi: f32, th: f32| {
                let d = (wrap_angle(phi - fr) / 0.97).abs();
                let keel = (1.0 - d / 0.35).max(0.0).powf(1.5);
                0.006 + 0.01 * smooth(1.6, 2.3, th) + 0.007 * keel * smooth(1.6, 1.9, th)
            };
            let mask_at = move |u: f32, v: f32, lift: f32| {
                let phi_top = lerp(fr - 0.99, fr + 0.99, u);
                let top = visor(phi_top).1 - 0.01;
                let th = lerp(top, 2.42, v);
                let squeeze = lerp(1.0, 0.48, smooth(1.65, 2.42, th));
                let phi = fr + wrap_angle(phi_top - fr) * squeeze;
                h.at(phi, th, mask_g(phi, th) + lift)
            };
            let mask_pl = s.a(0.003);
            kit.bind(Head, Mat::PaintA, || {
                // Cheek plates either side of a center plate, a jaw plate under them.
                split_plate(
                    20,
                    12,
                    mask_pl,
                    h.c,
                    &[0.3, 0.7],
                    &[0.62],
                    (0.012, 0.03),
                    |u, v| mask_at(u, v, 0.0),
                )
            });
            // The respirator on the faceplate: a rounded housing with intake slits.
            let snout = || {
                let (p, across, down, out) = on_shell(&h, fr, 2.04, mask_g(fr, 2.04));
                (p + out * (mask_pl.t + 0.012), across, down, out)
            };
            kit.bind(Head, Mat::Carbon, || {
                let (p, across, down, out) = snout();
                let mut m = block(v3(0.064, 0.05, 0.026), 0.011, across, -down, p);
                // Filter disks either side.
                for side in [-1.0f32, 1.0] {
                    let q = p + across * side * 0.036 + out * -0.004 + down * 0.004;
                    let n = (out + across * side * 0.9).normalize();
                    m.append(&disk(0.017, 0.012, n, q, 18), Mat4::IDENTITY);
                }
                // A low ridge down the crown.
                let path: Vec<Vec3> = (0..14)
                    .map(|i| sagittal(&h, lerp(-0.6, 1.5, i as f32 / 13.0), 0.0, 0.004))
                    .collect();
                m.append(
                    &fin(&path, 0.008, |t| 0.005 + 0.005 * (PI * t).sin(), Vec3::X),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.bind(Head, Mat::Rubber, || {
                let (p, across, down, out) = snout();
                let mut m = MeshBuilder::new();
                for k in 0..3 {
                    let q = p + out * 0.0135 + down * (k as f32 - 1.0) * 0.011;
                    m.append(
                        &MeshBuilder::chamfer_box(v3(0.036, 0.0035, 0.004), 0.0012)
                            .transformed(basis(across, -down, across.cross(-down), q)),
                        Mat4::IDENTITY,
                    );
                }
                for side in [-1.0f32, 1.0] {
                    let q = p + across * side * 0.036 + out * -0.004 + down * 0.004;
                    let n = (out + across * side * 0.9).normalize();
                    m.append(&disk(0.012, 0.004, n, q + n * 0.006, 16), Mat4::IDENTITY);
                }
                // Cheek vents.
                for side in [-1.0f32, 1.0] {
                    let phi = fr + side * 0.5;
                    m.append(
                        &shell_vents(&h, phi, 1.98, mask_g(phi, 1.98) + mask_pl.t, 3, 0.055),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Head, Mat::PaintB, || {
                // Brow ridge over the visor, overhanging it a little.
                let mut m = plate(18, 4, s.b(0.003), h.c, |u, v| {
                    let phi = lerp(fr - 1.0, fr + 1.0, u);
                    let c = wrap_angle(phi - fr) / 1.0;
                    h.at(
                        phi,
                        lerp(0.98, visor(phi).0 + 0.015, v),
                        0.006 + (0.004 + 0.008 * (1.0 - c * c)) * v,
                    )
                });
                // Cheek stripes along the faceplate's sides.
                m.append(
                    &mirrored(plate(8, 2, s.b(0.0), h.c, |u, v| {
                        mask_at(lerp(0.035, 0.12, v), lerp(0.08, 0.75, u), mask_pl.t + 0.001)
                    })),
                    Mat4::IDENTITY,
                );
                m
            });
            // Optics: three lenses in bezels over the right eye, and a sight line.
            let optics = [(fr - 0.4, 1.3), (fr - 0.4, 1.43), (fr - 0.26, 1.365)];
            let optic = move |k: usize| {
                let (phi, th) = optics[k];
                let (p, _, _, out) = on_shell(&h, phi, th, visor_g(phi) + 0.004);
                (p, out)
            };
            kit.bind(Head, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                for k in 0..3 {
                    let (p, out) = optic(k);
                    let r = if k == 2 { 0.011 } else { 0.009 };
                    m.append(
                        &spun(
                            &[
                                Vec2::new(r - 0.002, 0.009),
                                Vec2::new(r + 0.003, 0.008),
                                Vec2::new(r + 0.004, -0.004),
                                Vec2::new(r + 0.001, -0.005),
                            ],
                            20,
                            out,
                            p,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                for side in [-1.0f32, 1.0] {
                    let p = h.at(if side > 0.0 { 0.0 } else { PI }, 1.5, 0.002);
                    m.append(
                        &disk(0.028, 0.018, v3(side, 0.0, 0.0), p, 24),
                        Mat4::IDENTITY,
                    );
                }
                // Antenna on the left ear module.
                let a0 = h.at(PI, 1.5, 0.004) + v3(-0.012, 0.012, 0.012);
                m.append(
                    &MeshBuilder::tube(
                        &[
                            a0,
                            a0 + v3(-0.008, 0.08, 0.045),
                            a0 + v3(-0.012, 0.15, 0.11),
                        ],
                        0.002,
                        8,
                    ),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.bind(Head, Mat::Lens, || {
                let mut m = MeshBuilder::new();
                for k in 0..3 {
                    let (p, out) = optic(k);
                    let r = if k == 2 { 0.0085 } else { 0.0065 };
                    m.append(&disk(r, 0.002, out, p + out * 0.006, 16), Mat4::IDENTITY);
                }
                // The sight line across the visor, under its glass.
                m.append(
                    &plate(18, 1, Pl::new(0.0008, 0.0, 1.0), h.c, |u, v| {
                        let phi = lerp(fr - 0.8, fr + 0.8, u);
                        let (t0, t1) = visor(phi);
                        let mid = lerp(t0, t1, 0.48);
                        // On the glass's outer face (thickness and pillow).
                        let ug = (phi - (fr - 0.97)) / 1.94;
                        let glass = 0.004 + 0.002 * (PI * ug).sin();
                        h.at(phi, mid + (v - 0.5) * 0.006, visor_g(phi) + glass + 0.0006)
                    }),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.bind(Head, Mat::Glow, || {
                let mut m = MeshBuilder::new();
                for side in [-1.0f32, 1.0] {
                    let p = h.at(if side > 0.0 { 0.0 } else { PI }, 1.5, 0.012);
                    m.append(
                        &spun(
                            &[
                                Vec2::new(0.014, -0.001),
                                Vec2::new(0.018, -0.001),
                                Vec2::new(0.018, 0.001),
                                Vec2::new(0.014, 0.001),
                            ],
                            24,
                            v3(side, 0.0, 0.0),
                            p,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
        }
    }
}

fn back(kit: &mut Kit, s: &Spec) {
    use Bone::*;
    let v = torso_vol(s.bulk);
    match s.style {
        Style::Pilot => {
            // Jump kit: pack, twin thrusters, fuel cells, cables; on the carrier's back.
            let c = v3(0.0, 1.3, v.at(1.3, 1.5 * PI, 0.0).z + 0.064);
            kit.bind(Chest, Mat::PaintA, || {
                let mut m = block(v3(0.2, 0.25, 0.06), 0.02, Vec3::X, Vec3::Y, c);
                m.append(
                    &block(
                        v3(0.12, 0.16, 0.03),
                        0.012,
                        Vec3::X,
                        Vec3::Y,
                        c + v3(0.0, 0.0, 0.035),
                    ),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.bind(Chest, Mat::Carbon, || {
                let mut m = MeshBuilder::new();
                for side in [-1.0f32, 1.0] {
                    m.append(
                        &block(
                            v3(0.05, 0.2, 0.05),
                            0.012,
                            Vec3::X,
                            Vec3::Y,
                            c + v3(side * 0.12, -0.04, 0.0),
                        ),
                        Mat4::IDENTITY,
                    );
                }
                // Fuel cells across the top.
                m.append(
                    &spun(
                        &[
                            Vec2::new(0.001, -0.11),
                            Vec2::new(0.02, -0.11),
                            Vec2::new(0.022, -0.1),
                            Vec2::new(0.022, 0.1),
                            Vec2::new(0.02, 0.11),
                            Vec2::new(0.001, 0.11),
                        ],
                        16,
                        Vec3::X,
                        c + v3(0.0, 0.14, 0.012),
                    ),
                    Mat4::IDENTITY,
                );
                m
            });
            kit.bind(Chest, Mat::Trim, || {
                let mut m = MeshBuilder::new();
                for side in [-1.0f32, 1.0] {
                    let o = c + v3(side * 0.12, -0.15, 0.0);
                    let dir = v3(side * 0.15, -1.0, 0.3).normalize();
                    m.append(
                        &spun(
                            &[
                                Vec2::new(0.012, 0.0),
                                Vec2::new(0.022, 0.0),
                                Vec2::new(0.024, -0.02),
                                Vec2::new(0.03, -0.05),
                                Vec2::new(0.034, -0.065),
                                Vec2::new(0.029, -0.066),
                                Vec2::new(0.024, -0.05),
                                Vec2::new(0.016, -0.02),
                                Vec2::new(0.012, -0.02),
                            ],
                            24,
                            -dir,
                            o,
                        ),
                        Mat4::IDENTITY,
                    );
                    // Cable to the hip.
                    m.append(
                        &MeshBuilder::tube(
                            &[
                                o + v3(0.0, 0.05, 0.02),
                                o + v3(side * 0.02, -0.08, 0.02),
                                o + v3(side * 0.03, -0.2, -0.04),
                            ],
                            0.006,
                            8,
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Chest, Mat::Glow, || {
                let mut m = MeshBuilder::new();
                for side in [-1.0f32, 1.0] {
                    let o = c + v3(side * 0.12, -0.15, 0.0);
                    let dir = v3(side * 0.15, -1.0, 0.3).normalize();
                    m.append(&disk(0.026, 0.002, dir, o + dir * 0.06, 20), Mat4::IDENTITY);
                }
                m.append(
                    &block(
                        v3(0.1, 0.006, 0.002),
                        0.0008,
                        Vec3::X,
                        Vec3::Y,
                        c + v3(0.0, 0.05, 0.051),
                    ),
                    Mat4::IDENTITY,
                );
                m.append(
                    &block(
                        v3(0.1, 0.006, 0.002),
                        0.0008,
                        Vec3::X,
                        Vec3::Y,
                        c + v3(0.0, 0.03, 0.051),
                    ),
                    Mat4::IDENTITY,
                );
                m
            });
        }
        Style::Grunt => {
            let c = v3(0.0, 1.3, v.at(1.3, 1.5 * PI, 0.0).z + 0.06);
            kit.bind(Chest, Mat::PaintB, || {
                block(v3(0.24, 0.28, 0.08), 0.02, Vec3::X, Vec3::Y, c)
            });
            kit.bind(Chest, Mat::Leather, || {
                let mut m = block(
                    v3(0.2, 0.08, 0.05),
                    0.012,
                    Vec3::X,
                    Vec3::Y,
                    c + v3(0.0, -0.17, 0.0),
                );
                for side in [-1.0f32, 1.0] {
                    m.append(
                        &block(
                            v3(0.06, 0.18, 0.05),
                            0.012,
                            Vec3::X,
                            Vec3::Y,
                            c + v3(side * 0.155, -0.02, -0.01),
                        ),
                        Mat4::IDENTITY,
                    );
                }
                m
            });
            kit.bind(Chest, Mat::Trim, || {
                let a0 = c + v3(0.09, 0.14, 0.02);
                let mut m = MeshBuilder::tube(&[a0, a0 + v3(0.0, 0.3, 0.03)], 0.003, 8);
                m.append(&disk(0.014, 0.014, Vec3::Y, a0, 12), Mat4::IDENTITY);
                m
            });
            kit.bind(Chest, Mat::Glow, || {
                block(
                    v3(0.03, 0.01, 0.002),
                    0.001,
                    Vec3::X,
                    Vec3::Y,
                    c + v3(-0.06, 0.08, 0.041),
                )
            });
        }
        Style::Elite | Style::Duelist => {
            // A raised spine plate over the back.
            kit.bind(Chest, Mat::PaintB, || {
                v.plate(
                    (1.5 * PI - 0.1, 1.5 * PI + 0.1),
                    (1.24, 1.47),
                    0.025,
                    s.b(0.004),
                    4,
                    10,
                )
            });
            if s.style == Style::Elite {
                kit.bind(Chest, Mat::Glow, || {
                    v.plate(
                        (1.5 * PI - 0.018, 1.5 * PI + 0.018),
                        (1.26, 1.45),
                        0.025 + s.t + 0.003,
                        Pl::strip(),
                        1,
                        8,
                    )
                });
            }
        }
    }
}

// ------------------------------------------------------------------------ the katana

/// Blade cross-section points from the back to the edge at `s` m from the guard (one side,
/// x ≥ 0): (point, material of the face from this point to the next).
fn blade_section(s: f32) -> Vec<(Vec2, Mat)> {
    let (back, edge, th) = rig::blade_profile(s);
    let w = back - edge;
    let wave = 0.06 * (s * 55.0).sin() + 0.035 * (s * 131.0 + 1.3).sin() + 0.02 * (s * 289.0).sin();
    let hamon = edge + w * (0.3 + wave).clamp(0.18, 0.45);
    vec![
        (Vec2::new(0.0, back), Mat::Steel),
        (Vec2::new(th * 0.32, back - 0.0012), Mat::Steel),
        (Vec2::new(th * 0.5, back - w * 0.27), Mat::Steel),
        (Vec2::new(th * 0.47, back - w * 0.5), Mat::Steel),
        (Vec2::new(th * 0.38, hamon), Mat::Hamon),
        (Vec2::new(th * 0.12, edge + 0.0026), Mat::Edge),
        (Vec2::new(0.0, edge), Mat::Edge),
    ]
}

/// The blade: one mesh per material, blade space (base at the origin, −Z).
fn blade_meshes() -> Vec<(Mat, MeshBuilder)> {
    let stations: Vec<f32> = (0..=80)
        .map(|i| {
            let t = i as f32 / 80.0;
            // Denser toward the point.
            BLADE_LENGTH * (1.0 - (1.0 - t).powf(1.4))
        })
        .collect();
    let mut out: Vec<(Mat, MeshBuilder)> = Vec::new();
    let n_pts = blade_section(0.0).len();
    for face in 0..n_pts - 1 {
        let mat = blade_section(0.0)[face].1;
        for side in [1.0f32, -1.0] {
            let mut m = MeshBuilder::new();
            let mut along = 0.0;
            for (k, &s) in stations.iter().enumerate() {
                let sec = blade_section(s);
                let next = blade_section((s + 0.002).min(BLADE_LENGTH));
                let a = sec[face].0;
                let b = sec[face + 1].0;
                let pa = v3(a.x * side, a.y, -s);
                let pb = v3(b.x * side, b.y, -s);
                let na = v3(next[face].0.x * side, next[face].0.y, -(s + 0.002));
                let tangent = (na - pa).normalize_or(Vec3::NEG_Z);
                let across = pb - pa;
                let mut n = tangent.cross(across).normalize_or(Vec3::X * side);
                if n.x * side < 0.0 && n.dot(v3(side, 0.0, 0.0)) < 0.0 && face < 5 {
                    n = -n;
                }
                if k > 0 {
                    along += stations[k] - stations[k - 1];
                }
                let w = across.length();
                m.vertex(pa, n, Vec2::new(0.0, along));
                m.vertex(pb, n, Vec2::new(w, along));
            }
            for k in 0..stations.len() as u32 - 1 {
                let a = k * 2;
                m.quad(a, a + 1, a + 3, a + 2);
            }
            // Face normals out of the blade: fix winding per side.
            let t = m.indices[0..3].to_vec();
            let p: Vec<Vec3> = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].position))
                .collect();
            let gn = (p[1] - p[0]).cross(p[2] - p[0]);
            let vn = Vec3::from(m.vertices[t[0] as usize].normal);
            if gn.dot(vn) < 0.0 {
                for t in m.indices.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            // Normals must point away from the blade's center plane (x = 0) or up/down.
            let mid = m.vertices.len() / 2;
            let c = Vec3::from(m.vertices[mid].position);
            let nn = Vec3::from(m.vertices[mid].normal);
            if nn.x * side < -0.2 || (nn.x.abs() < 0.2 && (nn.y * (c.y - 0.0)) < 0.0 && face == 0) {
                m.flip();
            }
            m.remove_degenerate();
            match out.iter_mut().find(|(mm, _)| *mm == mat) {
                Some((_, acc)) => {
                    acc.append(&m, Mat4::IDENTITY);
                }
                None => out.push((mat, m)),
            }
        }
    }
    out
}

fn katana(kit: &mut Kit, _s: &Spec) {
    use Bone::{Blade, Sword};
    // Handle: an oval core of ray skin with a waist, wrapped in four crossing cords that
    // leave diamond windows on every face.
    let z0 = -0.03;
    let z1 = 0.196;
    let pitch = 0.0305;
    let core = move |th: f32, z: f32, g: f32| {
        let v = ((z - z0) / (z1 - z0)).clamp(0.0, 1.0);
        let waist = 1.0 - 0.07 * (PI * v).sin();
        let p = 2.0 / 2.4;
        let (sn, cs) = th.sin_cos();
        v3(
            cs.signum() * cs.abs().powf(p) * (0.0122 * waist + g),
            sn.signum() * sn.abs().powf(p) * (0.0162 * waist + g),
            z,
        )
    };
    kit.local(Sword, Mat::Same, || {
        let mut m = surface(20, 12, |u, v| core(-u * TAU, lerp(z0, z1, v), 0.0));
        m.flip();
        m
    });
    kit.local(Sword, Mat::Wrap, || {
        let mut m = MeshBuilder::new();
        let r = 0.0143;
        let width = 0.0118;
        let steps = ((z1 - z0) / pitch * 7.0).round() as usize;
        for (family, lift) in [(1.0f32, 0.0019), (-1.0, 0.0014)] {
            // Direction of the cord in (arc length, z) and the direction across it.
            let d = Vec2::new(family * PI * r / pitch, 1.0).normalize();
            let n = Vec2::new(-d.y, d.x);
            for offset in [0.0, PI] {
                let cols = 3;
                let base = m.vertices.len() as u32;
                for j in 0..=steps {
                    let z = lerp(z0 + 0.002, z1 - 0.002, j as f32 / steps as f32);
                    let th = family * PI * (z - z0) / pitch + offset;
                    for k in 0..=cols {
                        let x = (k as f32 / cols as f32 - 0.5) * width;
                        let h = lift * (1.0 - (2.0 * x / width).powi(2)).max(0.0).sqrt();
                        let (thk, zk) = (th + n.x * x / r, (z + n.y * x).clamp(z0, z1));
                        let pos = core(thk, zk, h + 0.0002);
                        let out = (core(thk, zk, 0.001) - core(thk, zk, 0.0)).normalize_or(Vec3::X);
                        m.vertex(
                            pos,
                            out,
                            Vec2::new(
                                k as f32 / cols as f32,
                                j as f32 / steps as f32 * (z1 - z0) / d.y / 0.012,
                            ),
                        );
                    }
                }
                let row = cols as u32 + 1;
                for j in 0..steps as u32 {
                    for k in 0..cols as u32 {
                        let a = base + j * row + k;
                        m.quad(a, a + 1, a + row + 1, a + row);
                    }
                }
            }
        }
        // Face outward.
        let t = &m.indices[0..3];
        let p: Vec<Vec3> = t
            .iter()
            .map(|&i| Vec3::from(m.vertices[i as usize].position))
            .collect();
        if (p[1] - p[0])
            .cross(p[2] - p[0])
            .dot(Vec3::from(m.vertices[t[0] as usize].normal))
            < 0.0
        {
            m.flip();
        }
        m.smooth_normals();
        m
    });
    kit.local(Sword, Mat::Trim, || {
        let oval = |r: f32, z: f32, len: f32, exp: f32| {
            let secs: Vec<Section> = (0..2)
                .map(|i| {
                    Section::rounded(v3(0.0, 0.0, z + i as f32 * len), Vec2::new(r * 0.76, r))
                        .with_exponent(exp)
                        .with_right(Vec3::X)
                })
                .collect();
            MeshBuilder::loft(&secs, 28, Cap::Flat, Cap::Flat)
        };
        // Fuchi collar, kashira pommel cap.
        let mut m = oval(0.0178, -0.044, 0.016, 2.6);
        let kashira: Vec<Section> = (0..6)
            .map(|i| {
                let t = i as f32 / 5.0;
                let r = 0.0172 * (1.0 - 0.35 * t * t);
                Section::rounded(
                    v3(0.0, 0.0, 0.193 + 0.022 * t.powf(0.7)),
                    Vec2::new(r * 0.76, r),
                )
                .with_exponent(2.6)
                .with_right(Vec3::X)
            })
            .collect();
        m.append(
            &MeshBuilder::loft(&kashira, 28, Cap::Flat, Cap::Flat),
            Mat4::IDENTITY,
        );
        // Seppa washers either side of the guard.
        m.append(&oval(0.021, -0.058, 0.004, 2.4), Mat4::IDENTITY);
        m.append(&oval(0.021, -0.046, 0.003, 2.4), Mat4::IDENTITY);
        // Menuki ornaments in a window on each face.
        for side in [-1.0f32, 1.0] {
            let z = -0.03 + 3.5 * 0.0305;
            let secs: Vec<Section> = (0..5)
                .map(|i| {
                    let t = i as f32 / 4.0;
                    let r = 0.0035 * (PI * t).sin().max(0.25);
                    Section::ellipse(
                        v3(side * 0.0121, 0.0, z - 0.008 + 0.016 * t),
                        Vec2::new(0.0011, r),
                    )
                    .with_right(Vec3::X)
                })
                .collect();
            m.append(
                &MeshBuilder::loft(&secs, 8, Cap::Round, Cap::Round),
                Mat4::IDENTITY,
            );
        }
        m
    });
    // Tsuba: a four-lobed guard.
    kit.local(Sword, Mat::Carbon, || {
        let outline: Vec<Vec2> = (0..64)
            .map(|i| {
                let a = i as f32 / 64.0 * TAU;
                let r = 0.036 + 0.0035 * (4.0 * a).cos();
                Vec2::new(a.cos() * r * 0.94, a.sin() * r * 1.06)
            })
            .collect();
        let mut m = MeshBuilder::extrude(&outline, 0.007, 0.0015)
            .transformed(Mat4::from_translation(v3(0.0, 0.0, -0.053)));
        m.planar_uvs(1.0);
        m
    });
    kit.local(Sword, Mat::Glow, || {
        // A glowing ring set into the guard's face.
        spun(
            &[
                Vec2::new(0.026, -0.0005),
                Vec2::new(0.029, -0.0005),
                Vec2::new(0.029, 0.0005),
                Vec2::new(0.026, 0.0005),
            ],
            40,
            Vec3::Z,
            v3(0.0, 0.0, -0.0535),
        )
    });
    // Blade (on its own bone, base at the guard).
    let blade = if kit.build {
        blade_meshes()
    } else {
        [Mat::Steel, Mat::Hamon, Mat::Edge]
            .map(|m| (m, MeshBuilder::new()))
            .to_vec()
    };
    for (mat, mesh) in blade {
        kit.local(Blade, mat, || mesh);
    }
    // Habaki: the collar locking the blade, in trim metal.
    kit.local(Blade, Mat::Trim, || {
        let secs: Vec<Section> = (0..4)
            .map(|i| {
                let t = i as f32 / 3.0;
                let s = t * 0.034;
                let (back, edge, th) = rig::blade_profile(s);
                let h = (back - edge) * 0.5 + 0.0028 - 0.0008 * t;
                Section::rounded(
                    v3(0.0, (back + edge) * 0.5, -0.002 - s),
                    Vec2::new(th * 0.5 + 0.003 - 0.0008 * t, h),
                )
                .with_exponent(3.2)
                .with_right(Vec3::X)
            })
            .collect();
        MeshBuilder::loft(&secs, 24, Cap::Flat, Cap::Flat)
    });
}

/// The scabbard, in sheath space (mouth at the origin, along −Z, edge −Y).
fn scabbard(kit: &mut Kit, _s: &Spec) {
    use Bone::Sheath;
    let len = 0.77;
    let section = move |sd: f32, grow: f32| {
        let (back, edge, _) = rig::blade_profile((sd + 0.01).clamp(0.0, BLADE_LENGTH - 0.07));
        let cy = (back + edge) * 0.5;
        let taper = 1.0 - 0.15 * (sd / len);
        Section::rounded(
            v3(0.0, cy, -sd),
            Vec2::new((0.0125 + grow) * taper, (0.0205 + grow) * taper),
        )
        .with_exponent(2.4)
        .with_right(Vec3::X)
    };
    kit.local(Sheath, Mat::Lacquer, || {
        let secs: Vec<Section> = (0..=12)
            .map(|i| section(0.02 + i as f32 / 12.0 * (len - 0.045), 0.0))
            .collect();
        MeshBuilder::loft(&secs, 24, Cap::Flat, Cap::Flat)
    });
    kit.local(Sheath, Mat::Trim, || {
        // Koiguchi (mouth), kojiri (end cap), kurikata (cord knob).
        let mut m = MeshBuilder::loft(
            &[section(0.0, 0.0012), section(0.026, 0.0012)],
            24,
            Cap::Flat,
            Cap::Flat,
        );
        let mut end: Vec<Section> = (0..5)
            .map(|i| section(len - 0.03 + i as f32 * 0.006, 0.001))
            .collect();
        if let Some(last) = end.last_mut() {
            last.half *= 0.8;
        }
        m.append(
            &MeshBuilder::loft(&end, 24, Cap::Flat, Cap::Flat),
            Mat4::IDENTITY,
        );
        let k = section(0.11, 0.0);
        m.append(
            &block(
                v3(0.008, 0.012, 0.024),
                0.003,
                Vec3::X,
                Vec3::Y,
                k.center + v3(-0.014, 0.0, 0.0),
            ),
            Mat4::IDENTITY,
        );
        m
    });
    kit.local(Sheath, Mat::Leather, || {
        // Sageo cord wound around the knob.
        let mut m = MeshBuilder::new();
        for k in 0..3 {
            let sd = 0.095 + k as f32 * 0.012;
            let c = section(sd, 0.0).center;
            let path: Vec<Vec3> = (0..=24)
                .map(|i| {
                    let a = i as f32 / 24.0 * TAU;
                    c + v3(a.cos() * 0.0145, a.sin() * 0.0225, -0.004 * (a / TAU))
                })
                .collect();
            m.append(&MeshBuilder::tube(&path, 0.0022, 6), Mat4::IDENTITY);
        }
        m
    });
    kit.local(Sheath, Mat::Glow, || {
        // A thin inlay along the outer side.
        let secs: Vec<Section> = (0..=30)
            .map(|i| {
                let sd = 0.04 + i as f32 / 30.0 * (len - 0.1);
                let c = section(sd, 0.0);
                Section::rounded(
                    c.center + v3(-c.half.x - 0.0004, 0.0, 0.0),
                    Vec2::new(0.0008, 0.0016),
                )
                .with_right(Vec3::Z.cross(Vec3::Y))
            })
            .collect();
        MeshBuilder::loft(&secs, 6, Cap::Flat, Cap::Flat)
    });
}

// ------------------------------------------------------------------------ materials

/// Uploaded images of one baked set (shared by several materials).
#[derive(Clone, Copy)]
struct Tex {
    base: Handle<Image>,
    normal: Handle<Image>,
    mr: Handle<Image>,
    ao: Handle<Image>,
}

impl Tex {
    fn upload(lib: &mut Library, set: TextureSet) -> Self {
        Self {
            base: lib.images.add(set.base_color),
            normal: lib.images.add(set.normal),
            mr: lib.images.add(set.metallic_roughness),
            ao: lib.images.add(set.occlusion),
        }
    }

    fn apply(self, m: StandardMaterial) -> StandardMaterial {
        m.with_base_color_texture(self.base)
            .with_normal_texture(self.normal)
            .with_metallic_roughness_texture(self.mr)
            .with_occlusion_texture(self.ao)
    }
}

fn color(hex_rgb: u32) -> [f32; 3] {
    hex(hex_rgb).to_array()
}

fn tint(hex_rgb: u32) -> StandardMaterial {
    let c = color(hex_rgb);
    StandardMaterial::color(c[0], c[1], c[2])
}

fn materials(lib: &mut Library) {
    let size = lib.texture_size.max(64);
    let half = (size / 2).max(64);
    let suit = Tex::upload(lib, bake(half, 0.2, 1.5, suit_surface));
    let cloth = Tex::upload(lib, bake(half, 0.35, 1.4, cloth_surface));
    let leather = Tex::upload(lib, bake(half, 0.4, 1.2, leather_surface));
    let trim = Tex::upload(lib, bake(half, 0.25, 1.0, trim_surface));
    let carbon = Tex::upload(lib, bake(half, 0.2, 1.0, carbon_surface));
    let rubber = Tex::upload(lib, bake(half / 2, 0.15, 1.5, rubber_surface));
    let mask = Tex::upload(lib, bake(half / 2, 0.25, 1.0, mask_surface));
    let mail = Tex::upload(lib, bake(half, 0.08, 1.4, mail_surface));
    let steel = Tex::upload(lib, bake(half, 0.3, 0.6, steel_surface));
    let hamon = Tex::upload(lib, bake(half, 0.3, 0.8, hamon_surface));
    let shared = |m: Mat| format!("fighter/{}", m.name());
    lib.material(
        &shared(Mat::Carbon),
        carbon.apply(StandardMaterial::color(1.0, 1.0, 1.0).with_uv_scale(5.0, 5.0)),
    );
    lib.material(
        &shared(Mat::Rubber),
        rubber.apply(StandardMaterial::color(1.0, 1.0, 1.0).with_uv_scale(6.6, 6.6)),
    );
    lib.material(
        &shared(Mat::Mask),
        mask.apply(
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_roughness(1.0)
                .with_uv_scale(4.0, 4.0),
        ),
    );
    lib.material(
        &shared(Mat::Mail),
        mail.apply(
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_metallic(1.0)
                .with_roughness(1.0)
                .with_uv_scale(12.5, 12.5),
        ),
    );
    lib.material(
        &shared(Mat::Steel),
        steel.apply(
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_metallic(1.0)
                .with_uv_scale(3.3, 3.3),
        ),
    );
    lib.material(
        &shared(Mat::Hamon),
        hamon.apply(
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_metallic(1.0)
                .with_uv_scale(3.3, 3.3),
        ),
    );
    for style in Style::ALL {
        let p = style.palette();
        let key = |m: Mat| m.key(style);
        let seed = style as u32 * 977 + 13;
        let (a_hex, a_rough) = p.paint_a;
        let (b_hex, b_rough) = p.paint_b;
        let wear = p.wear;
        let paint_a = Tex::upload(
            lib,
            bake(size, 0.6, 1.6, move |uv| {
                paint_surface(uv, hex(a_hex), a_rough, wear, seed)
            }),
        );
        let paint_b = Tex::upload(
            lib,
            bake(size, 0.6, 1.6, move |uv| {
                paint_surface(uv, hex(b_hex), b_rough, wear * 0.6, seed + 401)
            }),
        );
        let (ito, same) = (hex(p.ito), hex(p.same));
        let wrap = Tex::upload(lib, bake(256, 0.012, 2.0, move |uv| cord_surface(uv, ito)));
        let skin = Tex::upload(lib, bake(256, 0.03, 1.5, move |uv| same_surface(uv, same)));
        // The baked roughness is absolute (the material's factor multiplies it). Plates are
        // open underneath, and the insides of lames, guards and wings show at their ends.
        let painted = |t: Tex| {
            let mut m = t.apply(
                StandardMaterial::color(1.0, 1.0, 1.0)
                    .with_roughness(1.0)
                    .with_uv_scale(1.67, 1.67),
            );
            m.double_sided = true;
            m
        };
        lib.material(&key(Mat::PaintA), painted(paint_a));
        lib.material(&key(Mat::PaintB), painted(paint_b));
        lib.material(
            &key(Mat::Suit),
            suit.apply(tint(p.suit).with_roughness(1.0).with_uv_scale(5.0, 5.0)),
        );
        // Cloth is seen from inside too (under the hem, inside the sash's tails).
        let woven = |c: u32, scale: f32| {
            let mut m = cloth.apply(tint(c).with_roughness(1.0).with_uv_scale(scale, scale));
            m.double_sided = true;
            m
        };
        lib.material(&key(Mat::Cloth), woven(p.cloth, 4.0));
        lib.material(&key(Mat::Sash), woven(p.sash, 7.0));
        lib.material(
            &key(Mat::Leather),
            leather.apply(tint(p.leather).with_roughness(0.9).with_uv_scale(2.5, 2.5)),
        );
        let (trim_hex, trim_rough) = p.trim;
        lib.material(
            &key(Mat::Trim),
            trim.apply(
                tint(trim_hex)
                    .with_metallic(1.0)
                    .with_roughness(trim_rough / 0.33)
                    .with_uv_scale(4.0, 4.0),
            ),
        );
        lib.material(
            &key(Mat::Wrap),
            wrap.apply(StandardMaterial::color(1.0, 1.0, 1.0)),
        );
        lib.material(
            &key(Mat::Same),
            skin.apply(StandardMaterial::color(1.0, 1.0, 1.0).with_uv_scale(33.0, 33.0)),
        );
        let glow = color(p.glow);
        let glow_mat = |nits: f32| {
            StandardMaterial::color(glow[0], glow[1], glow[2])
                .with_roughness(0.3)
                .with_emissive(glow, nits)
        };
        lib.material(&key(Mat::Glow), glow_mat(p.glow_nits));
        lib.material(&key(Mat::Lens), glow_mat(p.glow_nits * 1.4));
        lib.material(&key(Mat::Edge), glow_mat(p.edge_nits));
        let glass = color(p.glass);
        let mut glass_mat = StandardMaterial::color(glass[0], glass[1], glass[2])
            .with_roughness(0.06)
            .with_metallic(0.0);
        glass_mat.reflectance = 0.9;
        lib.material(&key(Mat::Glass), glass_mat);
        let (lq, lq_rough) = p.lacquer;
        let mut lacquer = tint(lq).with_roughness(lq_rough);
        lacquer.reflectance = 0.6;
        lib.material(&key(Mat::Lacquer), lacquer);
    }
}

// ------------------------------------------------------------------------ texture surfaces

/// Painted armor: color with soft tonal variation and orange peel, clustered chips down to
/// primer and bare metal, fine scratches and grime.
fn paint_surface(uv: Vec2, color: Vec3, rough: f32, wear: f32, seed: u32) -> Surface {
    let lo = fbm(uv * 3.0, 3, 4, 0.5, seed);
    let mid = fbm(uv * 12.0, 12, 3, 0.5, seed + 7);
    let peel = perlin(uv * 96.0, 96, seed + 3) * 0.6 + perlin(uv * 192.0, 192, seed + 4) * 0.4;
    let cluster = fbm(uv * 4.0, 4, 3, 0.5, seed + 11) * 0.5 + 0.5;
    let detail = fbm(uv * 36.0, 36, 4, 0.55, seed + 17) * 0.5 + 0.5;
    let f = detail * 0.7 + cluster * 0.55 * wear;
    let chip = smoothstep(0.83, 0.86, f);
    let primer = smoothstep(0.79, 0.81, f) - chip;
    let scratch_n = ridged(uv * 24.0, 24, 3, seed + 23);
    let scratch_mask = smoothstep(0.55, 0.75, value(uv * 6.0, 6, seed + 29)) * wear;
    let scratch = smoothstep(0.93, 0.985, scratch_n) * scratch_mask;
    let grime = smoothstep(0.1, 0.9, lo * 0.5 + 0.5) * 0.18 * wear;
    let paint = color * (1.0 + 0.05 * lo + 0.03 * mid) * (1.0 - grime);
    let metal = Vec3::new(0.52, 0.53, 0.55) * (0.9 + 0.1 * mid);
    let primer_c = Vec3::new(0.32, 0.33, 0.33);
    let scratched = paint.lerp(Vec3::splat(0.6).min(paint * 1.8 + 0.08), 0.6);
    let mut albedo = paint.lerp(scratched, scratch);
    albedo = albedo.lerp(primer_c, primer.max(0.0));
    albedo = albedo.lerp(metal, chip);
    Surface {
        albedo,
        height: peel * 0.000018 - chip * 0.00012 - primer.max(0.0) * 0.00006 - scratch * 0.00003,
        roughness: (rough + 0.06 * mid + 0.15 * grime + 0.15 * scratch)
            .lerp_to(0.32, chip)
            .lerp_to(0.65, primer.max(0.0))
            .clamp(0.05, 1.0),
        metallic: chip,
        ao: 1.0 - 0.25 * chip - 0.1 * primer.max(0.0),
    }
}

trait LerpTo {
    fn lerp_to(self, b: f32, t: f32) -> f32;
}

impl LerpTo for f32 {
    fn lerp_to(self, b: f32, t: f32) -> f32 {
        self + (b - self) * t
    }
}

/// Undersuit: a technical twill (diagonal ribs), a little sheen, with a ribbed stretch panel
/// band and soft creases (white: the material tints it).
fn suit_surface(uv: Vec2) -> Surface {
    let n = 160.0;
    let rib = ((uv.x + uv.y) * n).fract();
    let rib = (PI * rib).sin().max(0.0).powf(0.7);
    let slub = perlin(Vec2::new(uv.x * 512.0, uv.y * 32.0), 512, 5) * 0.5 + 0.5;
    let band = smoothstep(
        0.02,
        0.0,
        (uv.y * 4.0).fract().min(1.0 - (uv.y * 4.0).fract()) - 0.008,
    );
    let crease = ridged(uv * 3.0, 3, 4, 7);
    let lo = fbm(uv * 4.0, 4, 3, 0.5, 9);
    Surface {
        albedo: Vec3::splat((0.8 + 0.1 * rib + 0.05 * slub - 0.1 * band) * (1.0 + 0.07 * lo)),
        height: rib * 0.00004 - band * 0.0002 - crease * crease * 0.0006,
        roughness: (0.78 - 0.1 * rib + 0.05 * lo).clamp(0.3, 1.0),
        metallic: 0.0,
        ao: 0.82 + 0.18 * rib - 0.15 * band,
    }
}

/// Heavy woven cloth (cotton or linen): plain weave with slubby yarns, fuzz and soft fold
/// creases (white: the material tints it).
fn cloth_surface(uv: Vec2) -> Surface {
    let n = 96.0;
    let p = uv * n;
    let (ix, iy) = (p.x.floor() as i32, p.y.floor() as i32);
    let f = p - p.floor();
    let over = (ix + iy).rem_euclid(2) == 0;
    let (across, along) = if over { (f.x, f.y) } else { (f.y, f.x) };
    let yarn = (PI * across).sin().max(0.0).powf(0.6) * (0.8 + 0.2 * (PI * along).sin());
    let slub = rand2(if over { ix } else { iy }, 0, 211) * 0.5 + rand2(ix / 3, iy / 3, 212) * 0.5;
    let fuzz = perlin(uv * 384.0, 384, 213) * 0.5 + 0.5;
    let fold = ridged(uv * 2.0, 2, 4, 214);
    let fade = fbm(uv * 3.0, 3, 4, 0.5, 215) * 0.5 + 0.5;
    Surface {
        albedo: Vec3::splat(
            (0.74 + 0.16 * yarn + 0.08 * slub + 0.04 * fuzz)
                * (0.92 + 0.16 * fade)
                * (1.0 - 0.12 * fold),
        ),
        height: yarn * 0.00008 + slub * 0.00002 - fold * fold * 0.0012,
        roughness: (0.9 - 0.06 * yarn + 0.04 * fuzz).clamp(0.4, 1.0),
        metallic: 0.0,
        ao: 0.72 + 0.28 * yarn,
    }
}

/// Leather: pebbled grain, creases and worn patches (white: the material tints it).
fn leather_surface(uv: Vec2) -> Surface {
    let (d1, d2, id) = worley(uv * 70.0, 70, 31);
    let crease = smoothstep(0.0, 0.12, d2 - d1);
    let wrinkle = ridged(uv * 5.0, 5, 4, 37);
    let worn = smoothstep(0.2, 0.8, fbm(uv * 3.0, 3, 4, 0.5, 41) * 0.5 + 0.5);
    Surface {
        albedo: Vec3::splat((0.8 + 0.15 * crease + 0.06 * id) * (1.0 + 0.35 * worn)),
        height: crease * 0.00012 - wrinkle * wrinkle * 0.0004,
        roughness: (0.62 - 0.12 * crease - 0.15 * worn).clamp(0.25, 1.0),
        metallic: 0.0,
        ao: 0.75 + 0.25 * crease,
    }
}

/// Machined metal: brushed streaks and smudges (white: the material tints it).
fn trim_surface(uv: Vec2) -> Surface {
    let row = (uv.y * 512.0) as i32;
    let streak = rand2(0, row, 51) * 0.6 + rand2(1, (uv.y * 128.0) as i32, 52) * 0.4;
    let smudge = fbm(uv * 5.0, 5, 4, 0.5, 53) * 0.5 + 0.5;
    let pits = smoothstep(0.08, 0.02, worley(uv * 90.0, 90, 54).0);
    Surface {
        albedo: Vec3::splat(0.86 + 0.1 * streak - 0.1 * pits),
        height: streak * 0.000004 - pits * 0.00003,
        roughness: (0.3 + 0.12 * streak + 0.18 * smudge + 0.2 * pits).clamp(0.05, 1.0),
        metallic: 1.0,
        ao: 1.0 - 0.3 * pits,
    }
}

/// Carbon fiber, 2×2 twill under a clear coat.
fn carbon_surface(uv: Vec2) -> Surface {
    let n = 56.0;
    let p = uv * n;
    let (ix, iy) = (p.x.floor() as i32, p.y.floor() as i32);
    let f = p - p.floor();
    let warp = (ix - iy).rem_euclid(4) < 2;
    let (across, along) = if warp { (f.x, f.y) } else { (f.y, f.x) };
    let profile = (PI * across).sin().max(0.0).powf(0.5);
    let crimp = 0.75 + 0.25 * (PI * along).sin();
    let fiber = rand2(
        if warp { (p.x * 12.0) as i32 } else { iy * 7 },
        if warp { ix * 7 } else { (p.y * 12.0) as i32 },
        61,
    );
    let sheen = if warp { 0.032 } else { 0.016 };
    Surface {
        albedo: Vec3::new(1.0, 1.0, 1.05) * sheen * (0.75 + 0.35 * profile * crimp + 0.15 * fiber),
        height: profile * crimp * 0.00012,
        roughness: 0.2 + 0.08 * (1.0 - profile),
        metallic: 0.0,
        ao: 0.7 + 0.3 * profile,
    }
}

/// Boot soles: dark rubber with chevron tread.
fn rubber_surface(uv: Vec2) -> Surface {
    let x = (uv.x * 8.0).fract();
    let chev = (uv.y * 12.0 + (x - 0.5).abs() * 1.5).fract();
    let groove = smoothstep(0.08, 0.02, chev.min(1.0 - chev));
    let grit = perlin(uv * 128.0, 128, 71) * 0.5 + 0.5;
    Surface {
        albedo: Vec3::splat(0.035 + 0.01 * grit),
        height: -groove * 0.0008 + grit * 0.00002,
        roughness: 0.88 + 0.08 * grit,
        metallic: 0.0,
        ao: 1.0 - 0.5 * groove,
    }
}

/// Molded rubber: near black, satin, faintly mottled where the mold's release agent pooled.
fn mask_surface(uv: Vec2) -> Surface {
    let lo = fbm(uv * 3.0, 3, 4, 0.5, 81) * 0.5 + 0.5;
    let fine = perlin(uv * 64.0, 64, 83) * 0.5 + 0.5;
    Surface {
        albedo: Vec3::splat(0.032 + 0.012 * lo + 0.004 * fine),
        height: fine * 0.000015,
        roughness: 0.48 + 0.14 * lo + 0.05 * fine,
        metallic: 0.0,
        ao: 1.0,
    }
}

/// Japanese mail (so-gusari): round rings, each linked to its four diagonal neighbors,
/// 8 mm apart, in blackened iron rubbed bright on the raised wire; dark cloth shows
/// through the gaps.
fn mail_surface(uv: Vec2) -> Surface {
    let p = uv * 10.0;
    // How far up the wire of the nearest ring of a lattice (0 = off the wire, 1 = its crown).
    let ring = |q: Vec2, r: f32, w: f32| {
        let d = (q - q.round()).length();
        let x = ((d - r) / w).abs();
        (1.0 - x * x).max(0.0).sqrt()
    };
    let big = ring(p, 0.33, 0.085);
    let small = ring(p + Vec2::splat(0.5), 0.2, 0.065);
    // The small links pass over the big rings on one side and under on the other.
    let side = ((p - p.round()).x * PI).sin();
    let (h_big, h_small) = (big * (0.9 - 0.15 * side), small * (0.9 + 0.15 * side));
    let h = h_big.max(h_small);
    let wire = smoothstep(0.0, 0.25, h);
    let rust = fbm(uv * 5.0, 5, 3, 0.5, 131) * 0.5 + 0.5;
    let grit = perlin(uv * 160.0, 160, 133) * 0.5 + 0.5;
    let iron = Vec3::new(0.2, 0.2, 0.21) * (0.7 + 0.5 * h) * (0.92 + 0.08 * grit)
        + Vec3::new(0.06, 0.025, 0.0) * smoothstep(0.6, 0.85, rust) * (1.0 - h);
    Surface {
        albedo: Vec3::splat(0.025).lerp(iron, wire),
        height: h * 0.0011,
        roughness: lerp(0.9, 0.5 - 0.18 * h + 0.1 * rust, wire),
        metallic: wire,
        ao: lerp(0.35, 0.65 + 0.35 * h, wire),
    }
}

/// Polished blade steel: fine polishing lines along the blade.
fn steel_surface(uv: Vec2) -> Surface {
    let col = (uv.x * 1024.0) as i32;
    let line = rand2(col, 0, 81) * 0.7 + rand2(col / 4, 1, 82) * 0.3;
    let haze = fbm(uv * 4.0, 4, 3, 0.5, 83) * 0.5 + 0.5;
    Surface {
        albedo: Vec3::new(0.86, 0.87, 0.89) * (0.97 + 0.04 * line),
        height: line * 0.000002,
        roughness: 0.1 + 0.05 * line + 0.06 * haze,
        metallic: 1.0,
        ao: 1.0,
    }
}

/// The hardened edge zone: frosty, sparkling crystals (nie) and mist (nioi).
fn hamon_surface(uv: Vec2) -> Surface {
    let sparkle = rand2((uv.x * 900.0) as i32, (uv.y * 900.0) as i32, 91);
    let nie = smoothstep(0.82, 0.95, sparkle);
    let mist = fbm(uv * 10.0, 10, 4, 0.55, 93) * 0.5 + 0.5;
    Surface {
        albedo: Vec3::new(0.9, 0.91, 0.93) * (0.94 + 0.06 * mist + 0.05 * nie),
        height: nie * 0.000004,
        roughness: (0.3 + 0.12 * mist - 0.12 * nie).clamp(0.05, 1.0),
        metallic: 1.0,
        ao: 1.0,
    }
}

/// The handle's silk cord: twisted strands running diagonally (u across the cord, v along
/// it).
fn cord_surface(uv: Vec2, ito: Vec3) -> Surface {
    let strand = (uv.y * 3.0 + uv.x * 1.2).fract();
    let profile = (PI * strand).sin().max(0.0).powf(0.6);
    let fiber = rand2((uv.x * 64.0) as i32, (uv.y * 256.0) as i32, 107);
    let edge = 1.0 - (2.0 * uv.x - 1.0).abs().powi(4);
    Surface {
        albedo: ito * (0.6 + 0.45 * profile + 0.08 * fiber) * (0.75 + 0.25 * edge),
        height: profile * 0.00025,
        roughness: 0.5 + 0.15 * (1.0 - profile),
        metallic: 0.0,
        ao: 0.6 + 0.4 * profile,
    }
}

/// Ray skin: packed rounded nodules, a few larger ones.
fn same_surface(uv: Vec2, same: Vec3) -> Surface {
    let (d1, d2, id) = worley(uv * 40.0, 40, 101);
    let nodule = smoothstep(0.55, 0.05, d1);
    let gap = smoothstep(0.0, 0.08, d2 - d1);
    Surface {
        albedo: same * (0.78 + 0.18 * nodule + 0.06 * id) * (0.8 + 0.2 * gap),
        height: nodule * 0.0003,
        roughness: 0.42 + 0.2 * (1.0 - nodule),
        metallic: 0.0,
        ao: 0.7 + 0.3 * gap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_has_a_full_set_of_parts() {
        for style in Style::ALL {
            let parts = parts(style);
            for bone in Bone::ALL {
                assert!(
                    parts.iter().any(|p| p.bone == bone),
                    "{style:?} has nothing on {bone:?}"
                );
            }
            assert!(parts.iter().any(|p| p.mat == Mat::Edge));
            assert!(parts.iter().any(|p| p.mat == Mat::Wrap));
        }
    }

    #[test]
    fn manifest_matches_the_built_meshes() {
        for style in Style::ALL {
            let manifest = parts(style);
            let built = build(style);
            assert_eq!(
                manifest.len(),
                built.len(),
                "{style:?}: every part builds a mesh"
            );
            for ((a, mesh), b) in built.iter().zip(manifest) {
                assert_eq!(a, b);
                assert!(mesh.triangle_count() > 0);
            }
        }
    }

    #[test]
    fn meshes_are_finite_and_in_budget() {
        for style in Style::ALL {
            let mut total = 0;
            for (part, m) in build(style) {
                total += m.triangle_count();
                for v in &m.vertices {
                    let ok = v
                        .position
                        .iter()
                        .chain(&v.normal)
                        .chain(&v.uv)
                        .all(|x| x.is_finite());
                    assert!(ok, "{} has a broken vertex {v:?}", part.mesh);
                    let n = Vec3::from(v.normal).length();
                    assert!(
                        (n - 1.0).abs() < 0.01,
                        "{} has a normal of length {n}",
                        part.mesh
                    );
                }
            }
            assert!(
                (30_000..=100_000).contains(&total),
                "{style:?}: {total} triangles"
            );
        }
    }

    #[test]
    fn plates_face_outward() {
        let v = torso_vol(1.0);
        let m = v.plate(
            (0.5, 1.5),
            (1.25, 1.4),
            0.01,
            Pl::new(0.01, 0.005, 1.0),
            8,
            6,
        );
        // The outer face's normals point away from the axis.
        let c = v.center(1.33);
        let out = m
            .vertices
            .iter()
            .filter(|vert| Vec3::from(vert.normal).dot(Vec3::from(vert.position) - c) > 0.0)
            .count();
        assert!(
            out * 10 > m.vertices.len() * 6,
            "{out} of {}",
            m.vertices.len()
        );
        // Triangles wind with their normals.
        let mut agree = 0;
        for t in m.indices.as_chunks::<3>().0 {
            let p = t.map(|i| Vec3::from(m.vertices[i as usize].position));
            let n = Vec3::from(m.vertices[t[0] as usize].normal);
            if (p[1] - p[0]).cross(p[2] - p[0]).dot(n) > 0.0 {
                agree += 1;
            }
        }
        assert!(agree * 10 > m.triangle_count() * 9);
    }

    #[test]
    fn the_blade_reaches_the_tip() {
        let parts = blade_meshes();
        let (lo, hi) = parts
            .iter()
            .map(|(_, m)| m.bounds())
            .fold((Vec3::MAX, Vec3::MIN), |(a, b), (l, h)| {
                (a.min(l), b.max(h))
            });
        assert!((lo.z + BLADE_LENGTH).abs() < 1e-3, "{lo}");
        assert!(hi.z <= 1e-4);
        let tip = rig::blade_tip();
        assert!(tip.y > lo.y && tip.y < hi.y);
    }
}

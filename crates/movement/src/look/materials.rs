//! A shared library of baked, tiling PBR materials, registered under `movement/mat/<name>`.
//!
//! Every material is baked once (see [`texgen::bake`]) and its `uv_scale` is set from the
//! real size one texture tile covers, so meshes with UVs in meters (everything from
//! [`MeshBuilder`](super::MeshBuilder)) get the right texel density without further tuning.
//! Tinted variants share their textures: the textures carry the detail (pores, grain, chips,
//! wear) and `base_color` carries the hue.
//!
//! ```ignore
//! MeshRenderer::new(look::mesh("course/wall"), look::materials::material(materials::CONCRETE))
//! ```
//!
//! | Key (`movement/mat/…`) | Surface | Tile (m) |
//! |---|---|---|
//! | `concrete` | weathered cast concrete: pores, bug holes, hairline cracks, stains | 2.5 |
//! | `concrete_panel` | architectural precast panel: form-tie holes with rust streaks, plywood seams, darker panel edges. Map U 0..1 across one panel (2.4 m nominal), V in meters | 2.4 × 2.5 |
//! | `painted_steel`, `…_dark`, `…_yellow`, `…_orange`, `…_white`, `…_olive` | paint over steel with orange peel, chips and scratches | 1 |
//! | `brushed_metal` | brushed aluminum | 0.5 |
//! | `rubber` | anti-slip rubber with coin studs and scuffs | 0.5 |
//! | `hazard` | worn yellow and black 45° hazard stripes | 0.5 |
//! | `tread_plate` | steel diamond plate | 0.3 |
//! | `wood_planks` | worn floor boards (run along U, 18 cm wide, 1 m long) with pegs | 2 |
//! | `wood`, `wood_dark` | sawn timber, grain along U | 1 |
//! | `stone`, `stone_dark` | carved granite with tool marks and lichen | 1.5 |
//! | `sand` | raked gravel, furrows run along U | 1 |
//! | `brass` | brass with tarnish | 0.5 |
//! | `cloth`, `cloth_indigo`, `cloth_crimson`, `cloth_black` | woven cotton | 0.25 |
//! | `lacquer`, `lacquer_black` | vermilion (or black) lacquer over wood, worn to the undercoat | 1 |
//! | `paper`, `paper_lit` | washi paper (`paper_lit` glows warm, for lanterns) | 0.5 |

use engine_assets::Handle;
use engine_core::math::{Vec2, Vec3};
use engine_render::{Image, MaterialRef, StandardMaterial};

use super::Library;
use super::texgen::{Surface, TextureSet, bake, fbm, hex, rand2, ridged, smoothstep, worley};

pub const CONCRETE: &str = "concrete";
pub const CONCRETE_PANEL: &str = "concrete_panel";
pub const PAINTED_STEEL: &str = "painted_steel";
pub const PAINTED_STEEL_DARK: &str = "painted_steel_dark";
pub const PAINTED_STEEL_YELLOW: &str = "painted_steel_yellow";
pub const PAINTED_STEEL_ORANGE: &str = "painted_steel_orange";
pub const PAINTED_STEEL_WHITE: &str = "painted_steel_white";
pub const PAINTED_STEEL_OLIVE: &str = "painted_steel_olive";
pub const BRUSHED_METAL: &str = "brushed_metal";
pub const RUBBER: &str = "rubber";
pub const HAZARD: &str = "hazard";
pub const TREAD_PLATE: &str = "tread_plate";
pub const WOOD_PLANKS: &str = "wood_planks";
pub const WOOD: &str = "wood";
pub const WOOD_DARK: &str = "wood_dark";
pub const STONE: &str = "stone";
pub const STONE_DARK: &str = "stone_dark";
pub const SAND: &str = "sand";
pub const BRASS: &str = "brass";
pub const CLOTH: &str = "cloth";
pub const CLOTH_INDIGO: &str = "cloth_indigo";
pub const CLOTH_CRIMSON: &str = "cloth_crimson";
pub const CLOTH_BLACK: &str = "cloth_black";
pub const LACQUER: &str = "lacquer";
pub const LACQUER_BLACK: &str = "lacquer_black";
pub const PAPER: &str = "paper";
pub const PAPER_LIT: &str = "paper_lit";

/// Every material this module registers.
pub const ALL: &[&str] = &[
    CONCRETE,
    CONCRETE_PANEL,
    PAINTED_STEEL,
    PAINTED_STEEL_DARK,
    PAINTED_STEEL_YELLOW,
    PAINTED_STEEL_ORANGE,
    PAINTED_STEEL_WHITE,
    PAINTED_STEEL_OLIVE,
    BRUSHED_METAL,
    RUBBER,
    HAZARD,
    TREAD_PLATE,
    WOOD_PLANKS,
    WOOD,
    WOOD_DARK,
    STONE,
    STONE_DARK,
    SAND,
    BRASS,
    CLOTH,
    CLOTH_INDIGO,
    CLOTH_CRIMSON,
    CLOTH_BLACK,
    LACQUER,
    LACQUER_BLACK,
    PAPER,
    PAPER_LIT,
];

/// Size of one `concrete_panel` tile, meters (U across a panel, V up the wall).
pub const PANEL_TILE: [f32; 2] = [2.4, 2.5];

/// Asset key (under `movement/`) of a shared material: `mat/<name>`.
pub fn key(name: &str) -> String {
    format!("mat/{name}")
}

/// A shared material by name (one of [`ALL`]).
pub fn material(name: &str) -> MaterialRef {
    super::material(&key(name))
}

/// The images of a baked set, added to the library once and shared by its variants.
#[derive(Debug, Clone, Copy)]
struct Baked {
    base: Handle<Image>,
    normal: Handle<Image>,
    mr: Handle<Image>,
    ao: Handle<Image>,
}

impl Baked {
    fn add(lib: &mut Library, set: TextureSet) -> Self {
        Self {
            base: lib.images.add(set.base_color),
            normal: lib.images.add(set.normal),
            mr: lib.images.add(set.metallic_roughness),
            ao: lib.images.add(set.occlusion),
        }
    }

    /// A material using these textures, tinted (linear RGB), tiling every `tile` meters.
    fn material(&self, tint: Vec3, tile: [f32; 2]) -> StandardMaterial {
        StandardMaterial {
            base_color: [tint.x, tint.y, tint.z, 1.0],
            metallic: 1.0,
            perceptual_roughness: 1.0,
            uv_scale: [1.0 / tile[0], 1.0 / tile[1]],
            base_color_texture: Some(self.base),
            normal_texture: Some(self.normal),
            metallic_roughness_texture: Some(self.mr),
            occlusion_texture: Some(self.ao),
            ..Default::default()
        }
    }
}

/// Bakes and registers every shared material (a [`Generator`](super::Generator)).
pub fn generate(lib: &mut Library) {
    let full = lib.texture_size.max(16);
    let half = (full / 2).max(16);
    let put = |lib: &mut Library, name: &str, m: StandardMaterial| {
        lib.material(&key(name), m);
    };

    let concrete = Baked::add(lib, bake(full, 2.5, 1.0, concrete));
    put(lib, CONCRETE, concrete.material(Vec3::ONE, [2.5, 2.5]));

    let panel = Baked::add(lib, bake(full, 2.45, 1.0, concrete_panel));
    put(lib, CONCRETE_PANEL, panel.material(Vec3::ONE, PANEL_TILE));

    let steel = Baked::add(lib, bake(full, 1.0, 1.0, painted_steel));
    for (name, tint) in [
        (PAINTED_STEEL, hex(0x9aa0a6)),
        (PAINTED_STEEL_DARK, hex(0x41454b)),
        (PAINTED_STEEL_YELLOW, hex(0xf0b414)),
        (PAINTED_STEEL_ORANGE, hex(0xe0602a)),
        (PAINTED_STEEL_WHITE, hex(0xe8e8e4)),
        (PAINTED_STEEL_OLIVE, hex(0x5c6448)),
    ] {
        put(lib, name, steel.material(tint, [1.0, 1.0]));
    }

    let brushed = Baked::add(lib, bake(half, 0.5, 1.0, brushed_metal));
    put(lib, BRUSHED_METAL, brushed.material(Vec3::ONE, [0.5, 0.5]));

    let rubber_set = Baked::add(lib, bake(half, 0.5, 1.0, rubber));
    put(lib, RUBBER, rubber_set.material(Vec3::ONE, [0.5, 0.5]));

    let hazard_set = Baked::add(lib, bake(half, 0.5, 1.0, hazard));
    put(lib, HAZARD, hazard_set.material(Vec3::ONE, [0.5, 0.5]));

    let tread = Baked::add(lib, bake(half, 0.3, 1.0, tread_plate));
    put(lib, TREAD_PLATE, tread.material(Vec3::ONE, [0.3, 0.3]));

    let planks = Baked::add(lib, bake(full, 2.0, 1.0, wood_planks));
    put(lib, WOOD_PLANKS, planks.material(Vec3::ONE, [2.0, 2.0]));

    let wood_set = Baked::add(lib, bake(full, 1.0, 1.0, wood));
    put(lib, WOOD, wood_set.material(Vec3::ONE, [1.0, 1.0]));
    put(lib, WOOD_DARK, wood_set.material(hex(0x5a4436), [1.0, 1.0]));

    let stone_set = Baked::add(lib, bake(full, 1.5, 1.0, stone));
    put(lib, STONE, stone_set.material(Vec3::ONE, [1.5, 1.5]));
    put(
        lib,
        STONE_DARK,
        stone_set.material(hex(0x8a8c90), [1.5, 1.5]),
    );

    let sand_set = Baked::add(lib, bake(full, 1.0, 1.0, sand));
    put(lib, SAND, sand_set.material(Vec3::ONE, [1.0, 1.0]));

    let brass_set = Baked::add(lib, bake(half, 0.5, 1.0, brass));
    put(lib, BRASS, brass_set.material(Vec3::ONE, [0.5, 0.5]));

    let cloth_set = Baked::add(lib, bake(half, 0.25, 1.0, cloth));
    for (name, tint) in [
        (CLOTH, hex(0xe9e1cf)),
        (CLOTH_INDIGO, hex(0x26324f)),
        (CLOTH_CRIMSON, hex(0x8c1a1a)),
        (CLOTH_BLACK, hex(0x2a2a2c)),
    ] {
        let mut m = cloth_set.material(tint, [0.25, 0.25]);
        m.subsurface = 0.35;
        m.double_sided = true;
        put(lib, name, m);
    }

    let lacquer_set = Baked::add(lib, bake(full, 1.0, 1.0, lacquer));
    put(
        lib,
        LACQUER,
        lacquer_set.material(hex(0xc8361c), [1.0, 1.0]),
    );
    put(
        lib,
        LACQUER_BLACK,
        lacquer_set.material(hex(0x262422), [1.0, 1.0]),
    );

    let paper_set = Baked::add(lib, bake(half, 0.5, 1.0, paper));
    let mut paper_m = paper_set.material(Vec3::ONE, [0.5, 0.5]);
    paper_m.subsurface = 0.6;
    paper_m.double_sided = true;
    put(lib, PAPER, paper_m);
    let lit = paper_m
        .with_emissive([1.0, 0.55, 0.22], 220.0)
        .with_emissive_texture(paper_set.base);
    put(lib, PAPER_LIT, lit);
}

// ------------------------------------------------------------------------ noise helpers

/// Value noise, about −1..1, tiling with separate periods along x and y (cells).
fn vnoise(p: Vec2, period: (i32, i32), seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (ix, iy) = (i.x as i32, i.y as i32);
    let r = |x: i32, y: i32| rand2(x.rem_euclid(period.0), y.rem_euclid(period.1), seed);
    let fade = |t: f32| t * t * (3.0 - 2.0 * t);
    let (u, v) = (fade(f.x), fade(f.y));
    let a = r(ix, iy) + (r(ix + 1, iy) - r(ix, iy)) * u;
    let b = r(ix, iy + 1) + (r(ix + 1, iy + 1) - r(ix, iy + 1)) * u;
    (a + (b - a) * v) * 2.0 - 1.0
}

/// Fractal [`vnoise`] with separate periods (streaks, grain, fibers).
fn fbm_a(p: Vec2, period: (i32, i32), octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut q, mut per) = (p, period);
    for o in 0..octaves {
        sum += vnoise(q, per, seed.wrapping_add(o * 977)) * amp;
        norm += amp;
        amp *= 0.5;
        q *= 2.0;
        per = (per.0 * 2, per.1 * 2);
    }
    sum / norm
}

fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn mixf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

// ------------------------------------------------------------------------ surfaces

/// Shared concrete body: blotches, mottling, aggregate grain and bug holes. Returns
/// (tone multiplier, height m, pit 0..1, grain −1..1).
fn concrete_body(uv: Vec2, seed: u32) -> (f32, f32, f32, f32) {
    let blotch = fbm(uv * 3.0, 3, 5, 0.55, seed);
    let mottle = fbm(uv * 12.0, 12, 4, 0.5, seed + 1);
    let grain = fbm(uv * 96.0, 96, 2, 0.5, seed + 2);
    let (d1, _, id) = worley(uv * 70.0, 70, seed + 3);
    let pit = if id < 0.07 {
        smoothstep(0.22, 0.08, d1)
    } else {
        0.0
    };
    let tone = 1.0 + 0.11 * blotch + 0.07 * mottle + 0.05 * grain;
    let height = 0.0016 * mottle + 0.0005 * grain - 0.0025 * pit;
    (tone, height, pit, grain)
}

/// Weathered cast concrete (floors, the tower, plinths). 2.5 m tile.
fn concrete(uv: Vec2) -> Surface {
    let (tone, mut height, pit, grain) = concrete_body(uv, 11);
    let (c1, c2, _) = worley(uv * 7.0, 7, 15);
    let crack =
        smoothstep(0.03, 0.0, c2 - c1) * smoothstep(0.25, 0.55, fbm(uv * 4.0, 4, 3, 0.5, 16));
    let stain = smoothstep(0.05, 0.55, fbm(uv * 2.0, 2, 5, 0.6, 17));
    let oil = smoothstep(0.55, 0.75, fbm(uv * 5.0, 5, 4, 0.5, 18));
    let warm = fbm(uv * 6.0, 6, 3, 0.5, 19) * 0.03;
    let base = hex(0x8f8d88) * Vec3::new(1.0 + warm, 1.0, 1.0 - warm);
    let albedo = base
        * tone
        * (1.0 - 0.22 * stain)
        * (1.0 - 0.35 * oil)
        * (1.0 - 0.55 * pit)
        * (1.0 - 0.45 * crack);
    height -= 0.0009 * crack;
    Surface {
        albedo,
        height,
        roughness: (0.9 + 0.05 * grain - 0.12 * stain - 0.25 * oil).clamp(0.4, 1.0),
        metallic: 0.0,
        ao: 1.0 - 0.6 * pit - 0.4 * crack,
    }
}

/// Precast architectural concrete panel (2.4 m across U, 2.5 m along V): a 4 × 4 grid of
/// form-tie holes with rust streaks running down from them, plywood seams and darker edges.
fn concrete_panel(uv: Vec2) -> Surface {
    let [w, h] = PANEL_TILE;
    let (tone, mut height, pit, grain) = concrete_body(uv, 41);
    // Nearest tie hole.
    let col = (uv.x * 4.0).floor();
    let row = (uv.y * 4.0).floor();
    let hu = (col + 0.5) / 4.0;
    let hv = (row + 0.5) / 4.0;
    let dx = (uv.x - hu) * w;
    let dy = (uv.y - hv) * h;
    let r = (dx * dx + dy * dy).sqrt();
    let hole = smoothstep(0.016, 0.0135, r);
    let cone = smoothstep(0.036, 0.016, r) * (1.0 - hole);
    let lip = smoothstep(0.044, 0.036, r) * smoothstep(0.03, 0.036, r);
    height += -0.014 * hole - 0.007 * cone * (0.036 - r) / 0.02;
    // Streak running down (toward −V, down the wall) from the hole above this point.
    let above = ((uv.y * 4.0 - 0.5).ceil() + 0.5) / 4.0;
    let below_m = ((above - uv.y) * h).max(0.0);
    let hole_id = rand2(col as i32, (above * 4.0).floor() as i32 & 3, 47);
    let spread = 0.010 + below_m * 0.035;
    let wobble = fbm_a(Vec2::new(uv.x * 24.0, uv.y * 3.0), (24, 3), 3, 48) * 0.004;
    let streak = (-((dx + wobble) / spread).powi(2)).exp()
        * (-below_m / (0.35 + 0.9 * hole_id)).exp()
        * (0.35 + 0.65 * hole_id)
        * (0.75 + 0.25 * fbm_a(Vec2::new(uv.x * 40.0, uv.y * 60.0), (40, 60), 2, 49));
    // Plywood seams (a thin raised fin and a faint line), panel-edge grime, pour bands.
    let seam_u = smoothstep(0.0025, 0.0, ((uv.x - 0.5).abs()) * w);
    let seam_v = smoothstep(0.0025, 0.0, (fract(uv.y * 2.0 + 0.5) - 0.5).abs() * h * 0.5);
    let seam = seam_u.max(seam_v);
    height += 0.0004 * seam;
    let edge = smoothstep(0.12, 0.0, uv.x.min(1.0 - uv.x) * w);
    let band = fbm_a(Vec2::new(uv.x * 2.0, uv.y * 8.0), (2, 8), 3, 50);
    let base = hex(0xa3a19b);
    let rust = Vec3::new(0.62, 0.5, 0.4);
    let mut albedo = base * tone * (1.0 + 0.05 * band) * (1.0 - 0.55 * pit);
    albedo *= mix3(Vec3::ONE, rust, streak * 0.9);
    albedo *= 1.0 - 0.12 * seam - 0.16 * edge - 0.08 * lip;
    albedo = mix3(albedo, Vec3::new(0.035, 0.033, 0.03), hole * 0.95);
    albedo = mix3(albedo, base * 0.55, cone * 0.85);
    Surface {
        albedo,
        height,
        roughness: (0.86 + 0.06 * grain - 0.1 * streak).clamp(0.5, 1.0),
        metallic: 0.0,
        ao: 1.0 - 0.6 * pit - 0.7 * hole - 0.3 * cone,
    }
}

/// Paint over steel: orange peel, chips and scratches showing dark steel, light grime. The
/// paint is near white (tint it with `base_color`).
fn painted_steel(uv: Vec2) -> Surface {
    let peel = fbm(uv * 48.0, 48, 2, 0.5, 31);
    let wear = fbm(uv * 5.0, 5, 5, 0.55, 32);
    let chips_n = fbm(uv * 22.0, 22, 4, 0.5, 33);
    let chip = smoothstep(0.43, 0.48, chips_n * 0.65 + wear * 0.45);
    let sc = ridged(Vec2::new(uv.x * 18.0, uv.y * 18.0), 18, 3, 34);
    let scratch = smoothstep(0.93, 0.985, sc) * smoothstep(-0.1, 0.4, wear);
    let dirt = smoothstep(-0.25, 0.65, fbm(uv * 3.0, 3, 4, 0.5, 35));
    let rust_n = fbm(uv * 30.0, 30, 3, 0.5, 36) * 0.5 + 0.5;
    let paint = Vec3::splat(0.78) * (1.0 - 0.18 * dirt) * (1.0 + 0.03 * peel);
    let metal = mix3(
        Vec3::new(0.16, 0.15, 0.14),
        Vec3::new(0.16, 0.07, 0.03),
        rust_n,
    );
    let bare = chip.max(scratch * 0.85);
    Surface {
        albedo: mix3(paint, metal, bare),
        height: 0.00025 * peel - 0.0003 * chip - 0.0001 * scratch,
        roughness: mixf(0.4 + 0.22 * dirt + 0.03 * peel, 0.5 + 0.3 * rust_n, bare),
        metallic: 0.35 * bare * (1.0 - rust_n * 0.6),
        ao: 1.0 - 0.15 * chip,
    }
}

/// Brushed aluminum: fine streaks along U, smudges.
fn brushed_metal(uv: Vec2) -> Surface {
    let streak = fbm_a(Vec2::new(uv.x * 3.0, uv.y * 320.0), (3, 320), 3, 61);
    let smudge = fbm(uv * 4.0, 4, 4, 0.5, 62);
    Surface {
        albedo: Vec3::new(0.86, 0.87, 0.88) * (0.93 + 0.05 * streak - 0.04 * smudge),
        height: 0.00004 * streak,
        roughness: (0.3 + 0.08 * streak + 0.1 * smudge).clamp(0.15, 0.6),
        metallic: 1.0,
        ao: 1.0,
    }
}

/// Anti-slip rubber: a grid of coin studs, grain and lighter scuffs.
fn rubber(uv: Vec2) -> Surface {
    let n = 20.0;
    let c = uv * n;
    let f = c - c.floor() - Vec2::splat(0.5);
    let r = f.length();
    let stud = smoothstep(0.34, 0.28, r);
    let grain = fbm(uv * 128.0, 128, 2, 0.5, 71);
    let scuff = smoothstep(0.8, 0.95, ridged(uv * 6.0, 6, 3, 72)) * 0.6;
    let dust = smoothstep(0.0, 0.7, fbm(uv * 3.0, 3, 3, 0.5, 73));
    Surface {
        albedo: Vec3::splat(0.035 + 0.006 * grain + 0.03 * scuff + 0.02 * dust * (1.0 - stud)),
        height: 0.0012 * stud + 0.00008 * grain,
        roughness: (0.88 + 0.06 * grain - 0.12 * scuff * stud).clamp(0.5, 1.0),
        metallic: 0.0,
        ao: 0.85 + 0.15 * stud,
    }
}

/// Worn yellow and black hazard stripes at 45° (8.8 cm stripes).
fn hazard(uv: Vec2) -> Surface {
    let s = fract((uv.x + uv.y) * 2.0);
    let aa = 0.006;
    let yellow_m = smoothstep(0.0, aa, s) * smoothstep(0.5, 0.5 - aa, s);
    let wear = fbm(uv * 4.0, 4, 5, 0.55, 81);
    let chips = smoothstep(0.45, 0.5, fbm(uv * 16.0, 16, 4, 0.5, 82) * 0.6 + wear * 0.5);
    let scratch = smoothstep(0.94, 0.99, ridged(uv * 10.0, 10, 3, 83)) * smoothstep(0.0, 0.4, wear);
    let dirt = smoothstep(-0.3, 0.7, fbm(uv * 2.0, 2, 4, 0.5, 84));
    let paint = mix3(Vec3::splat(0.022), hex(0xf2b70c), yellow_m) * (1.0 - 0.25 * dirt);
    let bare = chips.max(scratch * 0.8);
    let metal = Vec3::new(0.13, 0.11, 0.09);
    Surface {
        albedo: mix3(paint, metal, bare),
        height: -0.0003 * chips,
        roughness: mixf(0.45 + 0.25 * dirt, 0.6, bare),
        metallic: 0.3 * bare,
        ao: 1.0 - 0.1 * chips,
    }
}

/// Steel diamond plate: raised bars in alternating 45° directions.
fn tread_plate(uv: Vec2) -> Surface {
    let n = 8.0;
    let c = uv * n;
    let cell = c.floor();
    let f = c - cell - Vec2::splat(0.5);
    let flip = if (cell.x + cell.y) as i32 % 2 == 0 {
        1.0
    } else {
        -1.0
    };
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let a = (f.x + flip * f.y) * k;
    let b = (-flip * f.x + f.y) * k;
    let d = ((a / 0.34).powi(2) + (b / 0.085).powi(2)).sqrt();
    let bump = smoothstep(1.0, 0.65, d);
    let dirt = smoothstep(-0.2, 0.6, fbm(uv * 3.0, 3, 4, 0.5, 91));
    let grain = fbm(uv * 64.0, 64, 2, 0.5, 92);
    Surface {
        albedo: Vec3::splat(0.5 + 0.04 * grain) * (1.0 - 0.45 * dirt * (1.0 - bump)),
        height: 0.0016 * bump,
        roughness: (0.55 - 0.2 * bump + 0.25 * dirt * (1.0 - bump)).clamp(0.2, 1.0),
        metallic: 1.0 - 0.4 * dirt * (1.0 - bump),
        ao: 0.75 + 0.25 * bump,
    }
}

/// Worn wooden floor boards: 11 rows of boards along U per 2 m tile, 1 m long, staggered,
/// with gaps, pegs, grain and per-board color.
fn wood_planks(uv: Vec2) -> Surface {
    let rows = 11.0;
    let width_m = 2.0 / rows;
    let rv = uv.y * rows;
    let row = rv.floor();
    let fv = rv - row;
    let row_i = row as i32;
    let offset = rand2(row_i, 0, 51);
    let pu = uv.x * 2.0 + offset * 2.0;
    let k = pu.floor();
    let fu = pu - k;
    let board = (k as i32).rem_euclid(2);
    let id = rand2(row_i, board, 52);
    let id2 = rand2(row_i, board, 53);
    // Gaps between boards (sides and butt ends).
    let side_m = fv.min(1.0 - fv) * width_m;
    let end_m = fu.min(1.0 - fu);
    let gap = smoothstep(0.0028, 0.0008, side_m).max(smoothstep(0.003, 0.001, end_m));
    let edge = smoothstep(0.012, 0.002, side_m).max(smoothstep(0.015, 0.002, end_m));
    // Grain: growth rings warped along the board, fine fibers.
    let warp = fbm_a(Vec2::new(uv.x * 4.0, fv * 1.5 + id * 11.0), (4, 997), 3, 54);
    let ring = 0.5 + 0.5 * (std::f32::consts::TAU * (fv * 2.6 + warp * 1.6 + id * 3.0)).sin();
    let late = ring.powf(6.0);
    let fiber = fbm_a(
        Vec2::new(uv.x * 24.0, fv * 70.0 + id * 50.0),
        (24, 997),
        2,
        55,
    );
    // Pegs near the butt ends.
    let peg_m = |pu_: f32, pv_: f32| {
        let du = (fu - pu_) * 1.0;
        let dv = (fv - pv_) * width_m;
        (du * du + dv * dv).sqrt()
    };
    let peg_d = peg_m(0.035, 0.5).min(peg_m(0.965, 0.5));
    let peg = smoothstep(0.0075, 0.0055, peg_d);
    let wear =
        (1.0 - (fv - 0.5).abs() * 2.0).powf(2.0) * (0.5 + 0.5 * fbm(uv * 3.0, 3, 3, 0.5, 56));
    let hue = Vec3::new(1.0 + 0.08 * (id2 - 0.5), 1.0, 1.0 - 0.15 * (id2 - 0.5));
    let base = hex(0xb08458) * (0.8 + 0.35 * id) * hue;
    let mut albedo = base * (1.0 - 0.32 * late) * (0.94 + 0.06 * fiber) * (1.0 + 0.12 * wear);
    albedo *= 1.0 - 0.35 * edge;
    albedo = mix3(albedo, Vec3::splat(0.012), gap);
    albedo = mix3(albedo, base * 0.35, peg);
    Surface {
        albedo,
        height: -0.004 * gap - 0.0006 * edge + 0.00025 * late + 0.00012 * fiber - 0.0003 * peg,
        roughness: (0.72 + 0.1 * fiber + 0.08 * late - 0.12 * wear + 0.3 * gap).clamp(0.3, 1.0),
        metallic: 0.0,
        ao: 1.0 - 0.75 * gap - 0.2 * edge,
    }
}

/// Sawn timber, grain along U (light; tint with `base_color`).
fn wood(uv: Vec2) -> Surface {
    let warp = fbm_a(Vec2::new(uv.x * 3.0, uv.y * 5.0), (3, 5), 3, 101);
    let ring = 0.5 + 0.5 * (std::f32::consts::TAU * (uv.y * 14.0 + warp * 2.2)).sin();
    let late = ring.powf(5.0);
    let fiber = fbm_a(Vec2::new(uv.x * 16.0, uv.y * 220.0), (16, 220), 2, 102);
    let (kd, _, kid) = worley(uv * 3.0, 3, 103);
    let knot = if kid < 0.25 {
        smoothstep(0.12, 0.03, kd)
    } else {
        0.0
    };
    let check = smoothstep(
        0.96,
        0.995,
        ridged(Vec2::new(uv.x * 2.0, uv.y * 2.0), 2, 2, 104),
    ) * smoothstep(0.3, 0.6, fbm(uv * 4.0, 4, 2, 0.5, 105));
    let base = hex(0xb08a62);
    let albedo = base
        * (1.0 - 0.3 * late)
        * (0.93 + 0.07 * fiber)
        * (1.0 - 0.5 * knot)
        * (1.0 - 0.5 * check);
    Surface {
        albedo,
        height: 0.0003 * late + 0.00015 * fiber - 0.001 * check,
        roughness: (0.66 + 0.1 * fiber + 0.06 * late).clamp(0.3, 1.0),
        metallic: 0.0,
        ao: 1.0 - 0.4 * check - 0.2 * knot,
    }
}

/// Carved granite: speckled minerals, bush-hammer pits, tool tracks, lichen.
fn stone(uv: Vec2) -> Surface {
    let blotch = fbm(uv * 4.0, 4, 5, 0.55, 111);
    let speck = fbm(uv * 256.0, 256, 2, 0.5, 112);
    let dark_fleck = smoothstep(0.35, 0.55, speck);
    let light_fleck = smoothstep(-0.35, -0.55, speck);
    let (d1, _, id) = worley(uv * 120.0, 120, 113);
    let pit = if id < 0.12 {
        smoothstep(0.3, 0.1, d1)
    } else {
        0.0
    };
    let tool = (std::f32::consts::TAU
        * ((uv.x + uv.y) * 48.0 + 0.6 * fbm(uv * 8.0, 8, 2, 0.5, 114)))
    .sin();
    let lichen = smoothstep(0.35, 0.6, fbm(uv * 6.0, 6, 5, 0.55, 115));
    let base = hex(0x9a978f) * (1.0 + 0.14 * blotch);
    let mut albedo = base * (1.0 - 0.22 * dark_fleck) * (1.0 + 0.15 * light_fleck);
    albedo *= 1.0 - 0.18 * pit;
    albedo = mix3(albedo, hex(0x7f8466) * (0.9 + 0.2 * speck), lichen * 0.55);
    Surface {
        albedo,
        height: 0.0012 * blotch - 0.0009 * pit + 0.00018 * tool + 0.0002 * lichen,
        roughness: (0.86 + 0.06 * speck - 0.05 * light_fleck).clamp(0.5, 1.0),
        metallic: 0.0,
        ao: 1.0 - 0.45 * pit,
    }
}

/// Raked gravel: 16 furrows per meter running along U, gently wavering, with grains.
fn sand(uv: Vec2) -> Surface {
    let waver = fbm(uv * 3.0, 3, 3, 0.5, 121) * 0.28;
    let phase = uv.y * 16.0 + waver;
    let crest = 0.5 + 0.5 * (std::f32::consts::TAU * phase).cos();
    let profile = crest.powf(1.4);
    let grain = fbm(uv * 384.0, 384, 2, 0.5, 122);
    let pebble = smoothstep(0.55, 0.75, fbm(uv * 160.0, 160, 2, 0.5, 123));
    let tone = fbm(uv * 4.0, 4, 3, 0.5, 124);
    let base = hex(0xc4beb2) * (1.0 + 0.05 * tone);
    Surface {
        albedo: base * (0.9 + 0.1 * grain) * (0.82 + 0.18 * profile) * (1.0 - 0.25 * pebble),
        height: 0.007 * profile + 0.0005 * grain + 0.0008 * pebble,
        roughness: 0.95,
        metallic: 0.0,
        ao: 0.7 + 0.3 * profile,
    }
}

/// Brass with tarnish and fine polishing streaks.
fn brass(uv: Vec2) -> Surface {
    let tarnish = smoothstep(-0.15, 0.55, fbm(uv * 5.0, 5, 5, 0.55, 131));
    let streak = fbm_a(Vec2::new(uv.x * 3.0, uv.y * 160.0), (3, 160), 2, 132);
    let bright = Vec3::new(0.91, 0.74, 0.42);
    let dull = Vec3::new(0.42, 0.33, 0.18);
    Surface {
        albedo: mix3(bright, dull, tarnish * 0.75) * (0.97 + 0.03 * streak),
        height: 0.00003 * streak,
        roughness: (0.24 + 0.3 * tarnish + 0.05 * streak).clamp(0.1, 0.8),
        metallic: 1.0,
        ao: 1.0,
    }
}

/// Plain-woven cotton (near white; tint with `base_color`).
fn cloth(uv: Vec2) -> Surface {
    let n = 96.0;
    let c = uv * n;
    let cell = c.floor();
    let f = c - cell;
    let over = (cell.x as i32 + cell.y as i32) % 2 == 0;
    let pi = std::f32::consts::PI;
    let (a, b) = ((pi * f.x).sin(), (pi * f.y).sin());
    let h = if over {
        a * (0.55 + 0.45 * b)
    } else {
        b * (0.55 + 0.45 * a)
    };
    let slub_u = fbm_a(Vec2::new(uv.x * 96.0, uv.y * 4.0), (96, 4), 2, 141);
    let slub_v = fbm_a(Vec2::new(uv.x * 4.0, uv.y * 96.0), (4, 96), 2, 142);
    let slub = if over { slub_u } else { slub_v };
    let fade = fbm(uv * 2.0, 2, 3, 0.5, 143);
    Surface {
        albedo: Vec3::splat(0.82) * (0.84 + 0.16 * h) * (0.95 + 0.05 * slub) * (1.0 + 0.04 * fade),
        height: 0.00035 * h,
        roughness: 0.9,
        metallic: 0.0,
        ao: 0.8 + 0.2 * h,
    }
}

/// Lacquer over wood (near white; tint it): faint grain, crazing, worn through to a black
/// undercoat in spots.
fn lacquer(uv: Vec2) -> Surface {
    let warp = fbm_a(Vec2::new(uv.x * 3.0, uv.y * 5.0), (3, 5), 3, 151);
    let ring = 0.5 + 0.5 * (std::f32::consts::TAU * (uv.y * 12.0 + warp * 2.0)).sin();
    let wear = smoothstep(0.42, 0.5, fbm(uv * 7.0, 7, 5, 0.55, 152));
    let (c1, c2, _) = worley(uv * 28.0, 28, 153);
    let craze = smoothstep(0.03, 0.0, c2 - c1) * 0.6;
    let dust = smoothstep(-0.2, 0.6, fbm(uv * 3.0, 3, 4, 0.5, 154));
    let paint = Vec3::splat(0.8) * (1.0 - 0.08 * dust) * (1.0 - 0.15 * craze);
    Surface {
        albedo: mix3(paint, Vec3::splat(0.03), wear),
        height: 0.00015 * ring - 0.0002 * wear - 0.00005 * craze,
        roughness: (0.3 + 0.25 * dust + 0.25 * wear).clamp(0.1, 1.0),
        metallic: 0.0,
        ao: 1.0 - 0.1 * wear,
    }
}

/// Washi paper: long fibers and slight cloudiness.
fn paper(uv: Vec2) -> Surface {
    let f1 = ridged(Vec2::new(uv.x * 24.0, uv.y * 24.0), 24, 3, 161);
    let f2 = ridged(Vec2::new(uv.y * 16.0, uv.x * 16.0), 16, 3, 162);
    let fibers = smoothstep(0.85, 0.98, f1.max(f2));
    let cloud = fbm(uv * 6.0, 6, 4, 0.5, 163);
    Surface {
        albedo: hex(0xf0e6cf) * (0.93 + 0.05 * cloud) * (1.0 - 0.08 * fibers),
        height: 0.00008 * fibers,
        roughness: 0.92,
        metallic: 0.0,
        ao: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::Assets;
    use engine_render::Mesh;

    #[test]
    fn every_material_is_registered_with_textures_and_tiling() {
        let (mut meshes, mut images, mut materials) = (
            Assets::<Mesh>::default(),
            Assets::<Image>::default(),
            Assets::<StandardMaterial>::default(),
        );
        let mut lib = Library {
            meshes: &mut meshes,
            images: &mut images,
            materials: &mut materials,
            texture_size: 16,
        };
        generate(&mut lib);
        for name in ALL {
            let h = materials
                .by_key(&format!("movement/{}", key(name)))
                .unwrap_or_else(|| panic!("{name} missing"));
            let m = materials.get(h).unwrap();
            assert!(
                m.base_color_texture.is_some() && m.normal_texture.is_some(),
                "{name}"
            );
            assert!(
                m.uv_scale[0] > 0.3 && m.uv_scale[0] < 5.0,
                "{name} {:?}",
                m.uv_scale
            );
        }
    }

    #[test]
    fn surfaces_stay_in_range() {
        type SurfaceFn = fn(Vec2) -> Surface;
        let fns: [(&str, SurfaceFn); 15] = [
            ("concrete", concrete),
            ("concrete_panel", concrete_panel),
            ("painted_steel", painted_steel),
            ("brushed_metal", brushed_metal),
            ("rubber", rubber),
            ("hazard", hazard),
            ("tread_plate", tread_plate),
            ("wood_planks", wood_planks),
            ("wood", wood),
            ("stone", stone),
            ("sand", sand),
            ("brass", brass),
            ("cloth", cloth),
            ("lacquer", lacquer),
            ("paper", paper),
        ];
        for (name, f) in fns {
            for i in 0..64 {
                let uv = Vec2::new(rand2(i, 1, 7), rand2(i, 2, 7));
                let s = f(uv);
                assert!(s.albedo.is_finite() && s.height.is_finite(), "{name}");
                assert!(
                    s.albedo.max_element() <= 1.0 && s.albedo.min_element() >= 0.0,
                    "{name} {:?}",
                    s.albedo
                );
                assert!((0.0..=1.0).contains(&s.roughness), "{name}");
                assert!((0.0..=1.0).contains(&s.ao), "{name}");
            }
        }
    }
}

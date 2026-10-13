//! Set dressing for the movement showcase: the parkour course as a pilot training facility
//! and the duel ring as a samurai duel arena.
//!
//! [`generate`] builds every mesh and the course's own materials once and registers them under
//! `movement/course/...`; [`dress_course`], [`dress_ring`] and [`dress_level_column`] spawn
//! them around the demo (the demo calls them from `spawn_demo`). Nothing here adds a collider:
//! the demo's boxes stay exactly what they were, and every face a character stands or runs on
//! is drawn on a box face (paint, inlays and trims sit a few millimeters proud; the bigger
//! overhangs are where nobody stands).
//!
//! - **Course**: precast concrete wall panels with chamfered V-joints and form-tie holes on
//!   steel strongbacks and kicker braces, cyan guide lights recessed in the joint the runner
//!   wall-runs along, a hazard-striped slide bar on bolted posts, a concrete tower with
//!   railings, a caged ladder, floodlights and a door, a start gantry, floodlight masts,
//!   numbered backlit signs, floor paint, cables, conduits and vents. Behind it the grapple
//!   station ([`grapple_tower`]): a nine-meter tower with a glowing anchor plate to hook.
//! - **Ring**: a stone plinth with an octagonal board floor inside a carved stone band, an
//!   engraved ring inlay that glows faintly, raked-gravel corners and apron, four stone
//!   lanterns with warm lights, a torii gate, nobori banners, a taiko drum, a roofed stand
//!   with a crested curtain and paper lanterns, and stepping stones toward the course.
//! - **Level column**: the ten level cells stacked in a black-lacquered shrine frame with
//!   brass rings, a small hip roof and a sacred rope.
//!
//! Meshes carry baked cavity and grime in their vertex colors; materials come from
//! [`materials`](super::materials) (`movement/mat/...`).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, Image, MaterialRef, MeshRef, MeshRenderer, PointLight, StandardMaterial,
};

use super::materials::PANEL_TILE;
use super::texgen::{Surface, bake, fbm, hex, perlin, rand2, smoothstep};
use super::{Cap, Library, MeshBuilder, Section};
use crate::demo::layout;

mod freight;
mod grapple_tower;

type M = MeshBuilder;

// ============================================================================ keys

/// Key prefix of every mesh and material this module registers (under `movement/`).
pub const PREFIX: &str = "course/";

/// Mesh key of one level cell (the demo's `LevelCube`s use it).
pub const LEVEL_CELL: &str = "course/level_cell";

// Materials of our own (shared ones are `mat/...`).
const PAINT: &str = "course/paint";
const SIGN: &str = "course/sign";
const GLOW_CYAN: &str = "course/glow_cyan";
const GLOW_AMBER: &str = "course/glow_amber";
const GLOW_WHITE: &str = "course/glow_white";
const GLOW_RED: &str = "course/glow_red";
const GLOW_WARM: &str = "course/glow_warm";
const INLAY: &str = "course/inlay";
const CREST_WHITE: &str = "course/crest_white";
const CREST_BRASS: &str = "course/crest_brass";
const CREST_INDIGO: &str = "course/crest_indigo";
const KAWARA: &str = "course/kawara";
const CABLE: &str = "course/cable";
const LENS: &str = "course/lens";

const CONCRETE: &str = "mat/concrete";
const PANEL: &str = "mat/concrete_panel";
const STEEL: &str = "mat/painted_steel";
const STEEL_DARK: &str = "mat/painted_steel_dark";
const STEEL_YELLOW: &str = "mat/painted_steel_yellow";
const STEEL_ORANGE: &str = "mat/painted_steel_orange";
const STEEL_WHITE: &str = "mat/painted_steel_white";
const STEEL_OLIVE: &str = "mat/painted_steel_olive";
const OXIDE: &str = "mat/weathered_oxide";
const ROOF_GREY: &str = "mat/weathered_grey";
const RUST: &str = "mat/rusted_steel";
const BALLAST: &str = "mat/ballast";
const ALU: &str = "mat/brushed_metal";
const RUBBER: &str = "mat/rubber";
const HAZARD: &str = "mat/hazard";
const TREAD: &str = "mat/tread_plate";
const PLANKS: &str = "mat/wood_planks";
const WOOD_DARK: &str = "mat/wood_dark";
const STONE: &str = "mat/stone";
const STONE_DARK: &str = "mat/stone_dark";
const SAND: &str = "mat/sand";
const BRASS: &str = "mat/brass";
const CLOTH: &str = "mat/cloth";
const CLOTH_INDIGO: &str = "mat/cloth_indigo";
const CLOTH_CRIMSON: &str = "mat/cloth_crimson";
const LACQUER: &str = "mat/lacquer";
const LACQUER_BLACK: &str = "mat/lacquer_black";
const PAPER_LIT: &str = "mat/paper_lit";

/// Mesh groups spawned once at the demo origin: (group, material key, casts shadows). The
/// mesh key is [`kit_key`]`(group, material)`.
const COURSE_SET: &[(&str, &str, bool)] = &[
    ("pad", CONCRETE, true),
    ("pad", HAZARD, false),
    ("pad", STEEL_DARK, false),
    ("pad", GLOW_CYAN, false),
    ("pad", PAINT, false),
    ("walls", PANEL, true),
    ("walls", STEEL_DARK, true),
    ("walls", STEEL_YELLOW, true),
    ("walls", STEEL, true),
    ("walls", ALU, false),
    ("walls", GLOW_CYAN, false),
    ("walls", CABLE, true),
    ("walls", HAZARD, false),
    ("walls", PAINT, false),
    ("bar", HAZARD, true),
    ("bar", STEEL_DARK, true),
    ("bar", ALU, true),
    ("bar", GLOW_AMBER, false),
    ("bar", SIGN, false),
    ("bar", RUBBER, true),
    ("tower", PANEL, true),
    ("tower", CONCRETE, true),
    ("tower", STEEL_DARK, true),
    ("tower", STEEL_YELLOW, true),
    ("tower", STEEL_ORANGE, true),
    ("tower", STEEL, true),
    ("tower", ALU, true),
    ("tower", TREAD, true),
    ("tower", HAZARD, false),
    ("tower", GLOW_WHITE, false),
    ("tower", GLOW_RED, false),
    ("tower", SIGN, false),
    ("tower", PAINT, false),
    ("tower", CABLE, true),
    ("gantry", STEEL_DARK, true),
    ("gantry", CONCRETE, true),
    ("gantry", ALU, true),
    ("gantry", SIGN, false),
    ("gantry", GLOW_WHITE, false),
    ("masts", STEEL, true),
    ("masts", STEEL_DARK, true),
    ("masts", CONCRETE, true),
    ("masts", ALU, true),
    ("masts", GLOW_WHITE, false),
    ("masts", GLOW_RED, false),
    ("signs", STEEL_DARK, true),
    ("signs", ALU, true),
    ("signs", SIGN, false),
    ("signs", CONCRETE, true),
    ("props", STEEL_ORANGE, true),
    ("props", STEEL, true),
    ("props", ALU, true),
    ("props", STEEL_DARK, true),
    ("props", CONCRETE, true),
    ("props", STEEL_YELLOW, false),
    ("props", STEEL_OLIVE, true),
    ("props", RUBBER, true),
    ("props", STEEL_WHITE, false),
    ("ledge", CONCRETE, true),
    ("ledge", PANEL, true),
    ("ledge", STEEL_DARK, true),
    ("ledge", ALU, false),
    ("ledge", GLOW_CYAN, false),
    ("ledge", HAZARD, false),
    ("ledge", PAINT, false),
    ("grapple", CONCRETE, true),
    ("grapple", PANEL, true),
    ("grapple", STEEL_DARK, true),
    ("grapple", STEEL_YELLOW, true),
    ("grapple", STEEL, true),
    ("grapple", STEEL_ORANGE, true),
    ("grapple", ALU, true),
    ("grapple", TREAD, true),
    ("grapple", CABLE, true),
    ("grapple", HAZARD, false),
    ("grapple", GLOW_CYAN, false),
    ("grapple", GLOW_WHITE, false),
    ("grapple", GLOW_RED, false),
    ("grapple", PAINT, false),
];

/// Mesh groups spawned once at the ring center.
const RING_SET: &[(&str, &str, bool)] = &[
    ("plinth", STONE, true),
    ("plinth", STONE_DARK, true),
    ("plinth", PLANKS, true),
    ("plinth", SAND, true),
    ("plinth", INLAY, false),
    ("plinth", BRASS, false),
    ("plinth", CREST_BRASS, false),
    ("apron", SAND, true),
    ("apron", STONE, true),
    ("torii", LACQUER, true),
    ("torii", LACQUER_BLACK, true),
    ("torii", STONE, true),
    ("torii", BRASS, true),
    ("torii", CREST_BRASS, false),
    ("stands", WOOD_DARK, true),
    ("stands", PLANKS, true),
    ("stands", STONE, true),
    ("stands", KAWARA, true),
    ("stands", CLOTH, true),
    ("stands", CLOTH_INDIGO, true),
    ("stands", CREST_INDIGO, false),
    ("stands", LACQUER_BLACK, true),
    ("stands", PAPER_LIT, false),
    ("stands", BRASS, true),
    ("drum", WOOD_DARK, true),
    ("drum", LACQUER, true),
    ("drum", CLOTH, true),
    ("drum", BRASS, true),
    ("drum", STONE, true),
    ("path", STONE, true),
];

/// Meshes placed several times around the ring: (key, material, casts shadows).
const LANTERN_SET: &[(&str, &str, bool)] = &[
    ("lantern", STONE, true),
    ("lantern", PAPER_LIT, false),
    ("lantern", GLOW_WARM, false),
    ("lantern_rake", SAND, true),
];
const BANNER_SET: &[(&str, &str, bool)] = &[
    ("banner", LACQUER_BLACK, true),
    ("banner", STONE, true),
    ("banner", BRASS, true),
];
const COLUMN_SET: &[(&str, &str, bool)] = &[
    ("column", LACQUER_BLACK, true),
    ("column", BRASS, true),
    ("column", STONE, true),
    ("column", KAWARA, true),
    ("column", CLOTH, true),
    ("column", PAPER_LIT, false),
];

/// Mesh key (under `movement/`) of a kit group drawn with `material`.
pub fn kit_key(group: &str, material: &str) -> String {
    format!("{PREFIX}{group}/{}", material.replace('/', "_"))
}

fn material_ref(key: &str) -> MaterialRef {
    super::material(key)
}

/// Geometry of one group, sorted by material.
#[derive(Default)]
struct Kit {
    parts: HashMap<&'static str, M>,
}

impl Kit {
    fn add(&mut self, material: &'static str, m: M) -> &mut Self {
        self.parts
            .entry(material)
            .or_default()
            .append(&m, Mat4::IDENTITY);
        self
    }

    fn register(self, lib: &mut Library, group: &str) {
        for (mat, m) in self.parts {
            if !m.is_empty() {
                lib.mesh(&kit_key(group, mat), m);
            }
        }
    }
}

// ============================================================================ generator

/// Builds every course and ring mesh and the course's own materials (a
/// [`Generator`](super::Generator)).
pub fn generate(lib: &mut Library) {
    own_materials(lib);
    build_pad().register(lib, "pad");
    build_walls().register(lib, "walls");
    build_bar().register(lib, "bar");
    build_tower().register(lib, "tower");
    build_gantry().register(lib, "gantry");
    build_masts().register(lib, "masts");
    build_signs().register(lib, "signs");
    build_props().register(lib, "props");
    build_ledge().register(lib, "ledge");
    grapple_tower::build().register(lib, "grapple");
    freight::build_track().register(lib, "freight");
    freight::build_car().register(lib, "freight_car");
    build_plinth().register(lib, "plinth");
    build_apron().register(lib, "apron");
    build_torii().register(lib, "torii");
    build_stands().register(lib, "stands");
    build_drum().register(lib, "drum");
    build_path().register(lib, "path");
    build_lantern().register(lib, "lantern");
    build_lantern_rake().register(lib, "lantern_rake");
    build_banner().register(lib, "banner");
    for (i, cloth) in [CLOTH_INDIGO, CLOTH_CRIMSON].into_iter().enumerate() {
        let (c, crest) = build_banner_cloth(cloth);
        c.register(lib, &format!("banner_cloth{i}"));
        crest.register(lib, &format!("banner_crest{i}"));
    }
    build_column().register(lib, "column");
    lib.mesh(LEVEL_CELL, M::rounded_box(Vec3::splat(0.38), 0.03, 3));
}

// ============================================================================ spawning

fn spawn_part(
    commands: &mut Commands,
    at: DVec3,
    rotation: Quat,
    mesh: &str,
    material: &str,
    shadows: bool,
) {
    let mut r = MeshRenderer::new(
        MeshRef::Asset(format!("movement/{mesh}")),
        material_ref(material),
    );
    r.cast_shadows = shadows;
    commands.spawn((Transform::from_translation(at).with_rotation(rotation), r));
}

fn spawn_set(commands: &mut Commands, at: DVec3, rotation: Quat, set: &[(&str, &str, bool)]) {
    for &(group, mat, shadows) in set {
        spawn_part(commands, at, rotation, &kit_key(group, mat), mat, shadows);
    }
}

fn light(commands: &mut Commands, at: DVec3, color: [f32; 3], lumens: f32, range: f32) {
    commands.spawn((
        Name::new("Course light"),
        Transform::from_translation(at),
        PointLight {
            color,
            intensity_lm: lumens,
            range,
            radius: 0.08,
        },
    ));
}

/// Where the course's point lights are, relative to the demo origin.
const COURSE_LIGHTS: &[([f64; 3], [f32; 3], f32, f32)] = &[
    // Tower floodlights.
    ([24.55, 8.2, 7.95], [0.85, 0.92, 1.0], 9000.0, 28.0),
    ([24.55, 8.2, 14.05], [0.85, 0.92, 1.0], 9000.0, 28.0),
    // Masts.
    ([-27.0, 10.2, 17.9], [0.9, 0.95, 1.0], 26000.0, 45.0),
    ([27.0, 10.2, 6.1], [0.9, 0.95, 1.0], 26000.0, 45.0),
    // The grapple station's floodlight, and the glow of its target.
    ([3.75, 6.6, 19.75], [0.85, 0.92, 1.0], 9000.0, 26.0),
    ([8.0, 8.65, 27.6], [0.37, 0.85, 1.0], 700.0, 5.0),
    // Under the freight platform's lamp heads.
    ([-22.0, 5.9, 25.6], [1.0, 0.86, 0.68], 5000.0, 16.0),
    ([-14.0, 5.9, 25.6], [1.0, 0.86, 0.68], 5000.0, 16.0),
    ([-6.0, 5.9, 25.6], [1.0, 0.86, 0.68], 5000.0, 16.0),
];

/// Spawns the freight car's body, trucks and fittings as children of the moving `car` entity
/// (its transform is the center of its collider box).
pub fn dress_freight_car(commands: &mut Commands, car: Entity) {
    for &(group, mat, shadows) in freight::CAR_SET {
        let mut r = MeshRenderer::new(
            MeshRef::Asset(format!("movement/{}", kit_key(group, mat))),
            material_ref(mat),
        );
        r.cast_shadows = shadows;
        commands.spawn((Transform::default(), r, ChildOf(car)));
    }
}

/// Spawns the parkour course's set dressing around the demo `origin` (no colliders).
pub fn dress_course(commands: &mut Commands, origin: DVec3) {
    spawn_set(commands, origin, Quat::IDENTITY, COURSE_SET);
    spawn_set(commands, origin, Quat::IDENTITY, freight::TRACK_SET);
    for &(p, color, lm, range) in COURSE_LIGHTS {
        light(commands, origin + DVec3::from_array(p), color, lm, range);
    }
}

/// Lanterns around the ring: position relative to the ring center and yaw.
const LANTERNS: [(f32, f32, f32); 4] = [
    (-9.0, 9.0, 0.3),
    (9.0, 9.0, -0.3),
    (-9.0, -9.0, 0.2),
    (9.0, -9.0, -0.2),
];
/// Banners: x, z relative to the ring center, cloth variant.
const BANNERS: [(f32, f32, usize); 4] = [
    (-10.9, -5.0, 0),
    (-10.9, 5.0, 1),
    (10.9, -5.0, 1),
    (10.9, 5.0, 0),
];
/// Height of the lantern flame above the ground, meters.
const LANTERN_FLAME: f32 = 1.52;

/// Spawns the duel ring's set dressing around the ring center `ring` (ground level): the
/// plinth (on the ring collider), torii, lanterns with lights, banners, drum, stands dressing
/// and the stepping-stone path. No colliders.
pub fn dress_ring(commands: &mut Commands, ring: DVec3) {
    spawn_set(commands, ring, Quat::IDENTITY, RING_SET);
    let warm = [1.0, 0.55, 0.22];
    for (x, z, yaw) in LANTERNS {
        let at = ring + DVec3::new(x as f64, 0.0, z as f64);
        spawn_set(commands, at, Quat::from_rotation_y(yaw), LANTERN_SET);
        light(
            commands,
            at + DVec3::new(0.0, LANTERN_FLAME as f64, 0.0),
            warm,
            2400.0,
            16.0,
        );
    }
    for (x, z, variant) in BANNERS {
        let at = ring + DVec3::new(x as f64, 0.0, z as f64);
        // The cloth hangs toward the ring side of the pole.
        let rot = Quat::from_rotation_y(if x < 0.0 { 0.0 } else { PI });
        spawn_set(commands, at, rot, BANNER_SET);
        let cloth = [CLOTH_INDIGO, CLOTH_CRIMSON][variant];
        spawn_part(
            commands,
            at,
            rot,
            &kit_key(&format!("banner_cloth{variant}"), cloth),
            cloth,
            true,
        );
        spawn_part(
            commands,
            at,
            rot,
            &kit_key(&format!("banner_crest{variant}"), CREST_WHITE),
            CREST_WHITE,
            false,
        );
    }
    // Paper lanterns under the stand's eaves.
    for x in STAND_LANTERNS_X {
        light(
            commands,
            ring + DVec3::new(x as f64, STAND_LANTERN_Y as f64, STAND_FRONT as f64 + 0.05),
            warm,
            900.0,
            10.0,
        );
    }
}

/// Spawns the shrine frame around the level meter. `base` is the ground point under the
/// stack of level cells (the cells themselves are the demo's `LevelCube`s, drawn with
/// [`LEVEL_CELL`]).
pub fn dress_level_column(commands: &mut Commands, base: DVec3) {
    spawn_set(commands, base, Quat::IDENTITY, COLUMN_SET);
}

/// The mesh of one level cell.
pub fn level_cell() -> MeshRef {
    super::mesh(LEVEL_CELL)
}

// ============================================================================ materials

fn own_materials(lib: &mut Library) {
    // Floor and wall paint: white stencils with chips; vertex colors tint them.
    let paint_img = lib.images.add(paint_atlas(1024));
    lib.material(
        PAINT,
        StandardMaterial {
            base_color: [1.0; 4],
            perceptual_roughness: 0.62,
            alpha_mode: AlphaMode::Mask(0.5),
            base_color_texture: Some(paint_img),
            ..Default::default()
        },
    );
    // Backlit signs.
    let sign_img = lib.images.add(sign_atlas(1024));
    lib.material(
        SIGN,
        StandardMaterial {
            base_color: [0.03, 0.03, 0.035, 1.0],
            perceptual_roughness: 0.25,
            emissive: [1.0, 1.0, 1.0],
            emissive_strength: 3500.0,
            emissive_texture: Some(sign_img),
            ..Default::default()
        },
    );
    let glow = |rgb: u32, nits: f32| {
        let c = hex(rgb);
        StandardMaterial {
            base_color: [c.x, c.y, c.z, 1.0],
            perceptual_roughness: 0.3,
            emissive: [c.x, c.y, c.z],
            emissive_strength: nits,
            receives_wetness: false,
            ..Default::default()
        }
    };
    lib.material(GLOW_CYAN, glow(0x5fd8ff, 6000.0));
    lib.material(GLOW_AMBER, glow(0xffa21f, 5000.0));
    lib.material(GLOW_WHITE, glow(0xf2f6ff, 12000.0));
    lib.material(GLOW_RED, glow(0xff2a1c, 4000.0));
    lib.material(GLOW_WARM, glow(0xff9a40, 900.0));
    lib.material(
        LENS,
        StandardMaterial::hex(0x1a1c20)
            .with_roughness(0.08)
            .with_metallic(0.0),
    );
    lib.material(KAWARA, StandardMaterial::hex(0x3b3e44).with_roughness(0.42));
    lib.material(CABLE, StandardMaterial::hex(0x141414).with_roughness(0.55));

    // Crest decals (mitsudomoe).
    let crest = lib.images.add(crest_image(512));
    let decal = |rgb: u32, rough: f32, metal: f32| {
        let c = hex(rgb);
        StandardMaterial {
            base_color: [c.x, c.y, c.z, 1.0],
            perceptual_roughness: rough,
            metallic: metal,
            alpha_mode: AlphaMode::Mask(0.5),
            base_color_texture: Some(crest),
            double_sided: true,
            ..Default::default()
        }
    };
    lib.material(CREST_WHITE, decal(0xeee6d4, 0.85, 0.0));
    lib.material(CREST_INDIGO, decal(0x1e2a48, 0.85, 0.0));
    lib.material(CREST_BRASS, decal(0xe0b060, 0.3, 1.0));

    // The ring inlay: carved stone band with glowing grooves.
    let (set, glow_img) = inlay_textures(lib.texture_size.clamp(16, 1024));
    let emissive = lib.images.add(glow_img);
    let inlay = lib.textured(
        set,
        StandardMaterial {
            base_color: [1.0; 4],
            metallic: 1.0,
            perceptual_roughness: 1.0,
            emissive: [0.35, 0.78, 1.0],
            emissive_strength: 70.0,
            uv_scale: [1.0 / INLAY_TILE, 1.0],
            ..Default::default()
        },
    );
    lib.material(
        INLAY,
        StandardMaterial {
            emissive_texture: Some(emissive),
            ..inlay
        },
    );
}

// ---------------------------------------------------------------------------- stroke font

/// Polylines of a glyph on a 4 × 6 grid (y up).
fn glyph(c: char) -> &'static [&'static [(f32, f32)]] {
    match c {
        '0' | 'O' => &[&[
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 1.0),
            (1.0, 0.0),
        ]],
        '1' => &[
            &[(0.7, 4.6), (2.2, 6.0), (2.2, 0.0)],
            &[(0.7, 0.0), (3.7, 0.0)],
        ],
        '2' => &[&[
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.8),
            (0.0, 0.4),
            (0.0, 0.0),
            (4.0, 0.0),
        ]],
        '3' => &[
            &[
                (0.0, 5.0),
                (1.0, 6.0),
                (3.0, 6.0),
                (4.0, 5.0),
                (4.0, 4.0),
                (3.0, 3.0),
                (1.4, 3.0),
            ],
            &[
                (3.0, 3.0),
                (4.0, 2.0),
                (4.0, 1.0),
                (3.0, 0.0),
                (1.0, 0.0),
                (0.0, 1.0),
            ],
        ],
        '4' => &[&[(3.0, 0.0), (3.0, 6.0), (0.0, 2.0), (0.0, 1.6), (4.0, 1.6)]],
        '5' => &[&[
            (4.0, 6.0),
            (0.2, 6.0),
            (0.0, 3.2),
            (3.0, 3.4),
            (4.0, 2.4),
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
        ]],
        '6' => &[&[
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.4),
            (3.0, 3.4),
            (0.0, 3.4),
        ]],
        '7' => &[&[(0.0, 6.0), (4.0, 6.0), (4.0, 5.2), (1.6, 0.0)]],
        '8' => &[&[
            (1.0, 3.0),
            (0.0, 4.0),
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 4.0),
            (3.0, 3.0),
            (1.0, 3.0),
            (0.0, 2.0),
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.0),
            (3.0, 3.0),
        ]],
        '9' => &[&[
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 3.6),
            (1.0, 2.6),
            (4.0, 2.6),
        ]],
        'A' => &[
            &[
                (0.0, 0.0),
                (0.0, 5.0),
                (1.0, 6.0),
                (3.0, 6.0),
                (4.0, 5.0),
                (4.0, 0.0),
            ],
            &[(0.0, 2.6), (4.0, 2.6)],
        ],
        'C' => &[&[
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
        ]],
        'D' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 1.0),
            (3.0, 0.0),
            (0.0, 0.0),
        ]],
        'E' => &[
            &[(4.0, 6.0), (0.0, 6.0), (0.0, 0.0), (4.0, 0.0)],
            &[(0.0, 3.0), (3.0, 3.0)],
        ],
        'F' => &[
            &[(4.0, 6.0), (0.0, 6.0), (0.0, 0.0)],
            &[(0.0, 3.0), (3.0, 3.0)],
        ],
        'G' => &[&[
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.8),
            (2.2, 2.8),
        ]],
        'H' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 0.0), (4.0, 6.0)],
            &[(0.0, 3.0), (4.0, 3.0)],
        ],
        'I' => &[
            &[(2.0, 0.0), (2.0, 6.0)],
            &[(0.8, 0.0), (3.2, 0.0)],
            &[(0.8, 6.0), (3.2, 6.0)],
        ],
        'J' => &[&[(4.0, 6.0), (4.0, 1.0), (3.0, 0.0), (1.0, 0.0), (0.0, 1.0)]],
        'K' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 6.0), (0.0, 2.4)],
            &[(1.4, 3.6), (4.0, 0.0)],
        ],
        'L' => &[&[(0.0, 6.0), (0.0, 0.0), (4.0, 0.0)]],
        'M' => &[&[(0.0, 0.0), (0.0, 6.0), (2.0, 3.2), (4.0, 6.0), (4.0, 0.0)]],
        'N' => &[&[(0.0, 0.0), (0.0, 6.0), (4.0, 0.0), (4.0, 6.0)]],
        'P' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.6),
            (3.0, 2.6),
            (0.0, 2.6),
        ]],
        'R' => &[
            &[
                (0.0, 0.0),
                (0.0, 6.0),
                (3.0, 6.0),
                (4.0, 5.0),
                (4.0, 3.6),
                (3.0, 2.6),
                (0.0, 2.6),
            ],
            &[(2.2, 2.6), (4.0, 0.0)],
        ],
        'S' => &[&[
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 4.0),
            (1.0, 3.0),
            (3.0, 3.0),
            (4.0, 2.0),
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
        ]],
        'T' => &[&[(0.0, 6.0), (4.0, 6.0)], &[(2.0, 6.0), (2.0, 0.0)]],
        'U' => &[&[
            (0.0, 6.0),
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 6.0),
        ]],
        'W' => &[&[(0.0, 6.0), (0.0, 0.0), (2.0, 2.8), (4.0, 0.0), (4.0, 6.0)]],
        '>' => &[&[(0.6, 6.0), (3.6, 3.0), (0.6, 0.0)]],
        '<' => &[&[(3.4, 6.0), (0.4, 3.0), (3.4, 0.0)]],
        '-' => &[&[(0.6, 3.0), (3.4, 3.0)]],
        _ => &[],
    }
}

const GLYPH_ADVANCE: f32 = 5.4;
const SPACE_ADVANCE: f32 = 3.0;

/// Width of `text` in glyph units (a glyph is 4 wide and 6 tall).
fn text_width(text: &str) -> f32 {
    let mut x: f32 = 0.0;
    let mut w: f32 = 0.0;
    for c in text.chars() {
        if c == ' ' {
            x += SPACE_ADVANCE;
        } else {
            w = x + 4.0;
            x += GLYPH_ADVANCE;
        }
    }
    w
}

/// An RGBA float canvas for atlases.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: usize, h: usize, fill: [f32; 4]) -> Self {
        Self {
            w,
            h,
            px: vec![fill; w * h],
        }
    }

    /// Paints `color` over the canvas with coverage `f(x, y)` inside a pixel rectangle.
    fn paint(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: [f32; 4],
        f: impl Fn(f32, f32) -> f32,
    ) {
        let xa = x0.floor().max(0.0) as usize;
        let ya = y0.floor().max(0.0) as usize;
        let xb = (x1.ceil() as usize).min(self.w);
        let yb = (y1.ceil() as usize).min(self.h);
        for y in ya..yb {
            for x in xa..xb {
                let a = f(x as f32 + 0.5, y as f32 + 0.5).clamp(0.0, 1.0) * color[3];
                if a <= 0.0 {
                    continue;
                }
                let p = &mut self.px[y * self.w + x];
                for c in 0..3 {
                    p[c] = p[c] * (1.0 - a) + color[c] * a;
                }
                p[3] = p[3].max(a);
            }
        }
    }

    /// Strokes `text` with its baseline at `base_y` and left edge at `x0` (pixels), `scale`
    /// pixels per glyph unit, `stroke` glyph units wide.
    fn text(&mut self, text: &str, x0: f32, base_y: f32, scale: f32, stroke: f32, color: [f32; 4]) {
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        let mut x = 0.0;
        for c in text.chars() {
            if c == ' ' {
                x += SPACE_ADVANCE;
                continue;
            }
            for line in glyph(c) {
                for w in line.windows(2) {
                    let p = |(gx, gy): (f32, f32)| {
                        Vec2::new(x0 + (x + gx) * scale, base_y - gy * scale)
                    };
                    segs.push((p(w[0]), p(w[1])));
                }
            }
            x += GLYPH_ADVANCE;
        }
        let r = stroke * scale * 0.5;
        for (a, b) in segs {
            let (lo, hi) = (
                a.min(b) - Vec2::splat(r + 2.0),
                a.max(b) + Vec2::splat(r + 2.0),
            );
            self.paint(lo.x, lo.y, hi.x, hi.y, color, |x, y| {
                let p = Vec2::new(x, y);
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                r - p.distance(a + ab * t) + 0.5
            });
        }
    }

    /// A filled rectangle with rounded corners (pixels).
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, color: [f32; 4]) {
        let c = Vec2::new(x0 + x1, y0 + y1) * 0.5;
        let half = Vec2::new(x1 - x0, y1 - y0) * 0.5 - Vec2::splat(radius);
        self.paint(x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0, color, |x, y| {
            let q = (Vec2::new(x, y) - c).abs() - half;
            let d = q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - radius;
            0.5 - d
        });
    }

    fn image(&self, srgb: bool) -> Image {
        let enc = |c: f32| {
            let c = c.clamp(0.0, 1.0);
            let s = if !srgb {
                c
            } else if c <= 0.003_130_8 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0 + 0.5) as u8
        };
        let mut data = Vec::with_capacity(self.w * self.h * 4);
        for p in &self.px {
            data.extend_from_slice(&[
                enc(p[0]),
                enc(p[1]),
                enc(p[2]),
                (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            ]);
        }
        Image::new(self.w as u32, self.h as u32, data, srgb)
    }
}

// ---------------------------------------------------------------------------- paint atlas

/// Rows of the paint atlas (stencils painted on floors and walls): text, atlas line and left
/// edge in px (short words share a line).
const PAINT_ROWS: &[(&str, usize, f32)] = &[
    ("01", 0, 0.0),
    ("02", 1, 0.0),
    ("03", 2, 0.0),
    ("04", 3, 0.0),
    ("05", 4, 0.0),
    ("SLIDE", 5, 0.0),
    ("START", 6, 0.0),
    ("WALL RUN", 7, 0.0),
    ("JUMP", 8, 0.0),
    (">>>", 9, 0.0),
    ("07", 0, 160.0),
    ("GRAPPLE", 1, 160.0),
    ("08", 0, 320.0),
    ("RIDE", 2, 160.0),
    ("MIND THE GAP", 3, 160.0),
    ("MTRN 4108", 4, 160.0),
];
/// The freight station's paint rows.
const PAINT_08: usize = 12;
const PAINT_RIDE: usize = 13;
const PAINT_MIND: usize = 14;
const PAINT_MARK: usize = 15;
const PAINT_ROW_PX: f32 = 96.0;
const PAINT_SCALE: f32 = 12.0;
const PAINT_PAD: f32 = 12.0;
const ATLAS_PX: f32 = 1024.0;

/// Atlas rectangle (top-left uv, bottom-right uv) and width / height of a paint row.
fn paint_rect(row: usize) -> (Vec2, Vec2, f32) {
    let (text, line, x0) = PAINT_ROWS[row];
    let w = text_width(text) * PAINT_SCALE + 2.0 * PAINT_PAD;
    let y0 = line as f32 * PAINT_ROW_PX;
    (
        Vec2::new(x0 / ATLAS_PX, y0 / ATLAS_PX),
        Vec2::new((x0 + w) / ATLAS_PX, (y0 + PAINT_ROW_PX) / ATLAS_PX),
        w / PAINT_ROW_PX,
    )
}

/// The solid (worn) strip of the paint atlas, for lines and boxes.
fn paint_solid() -> (Vec2, Vec2) {
    (
        Vec2::new(20.0 / ATLAS_PX, 972.0 / ATLAS_PX),
        Vec2::new(1004.0 / ATLAS_PX, 1012.0 / ATLAS_PX),
    )
}

fn paint_atlas(size: u32) -> Image {
    let n = size as usize;
    let k = size as f32 / ATLAS_PX;
    let mut c = Canvas::new(n, n, [1.0, 1.0, 1.0, 0.0]);
    for &(text, line, x0) in PAINT_ROWS {
        let y0 = line as f32 * PAINT_ROW_PX;
        c.text(
            text,
            (x0 + PAINT_PAD) * k,
            (y0 + PAINT_ROW_PX - PAINT_PAD) * k,
            PAINT_SCALE * k,
            1.05,
            [1.0, 1.0, 1.0, 1.0],
        );
    }
    c.rect(
        4.0 * k,
        964.0 * k,
        1020.0 * k,
        1020.0 * k,
        0.0,
        [1.0, 1.0, 1.0, 1.0],
    );
    // Wear: chips and scuffs take paint away, grime darkens it.
    for y in 0..n {
        for x in 0..n {
            let uv = Vec2::new((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            let chips = fbm(uv * 48.0, 48, 4, 0.5, 201) * 0.6 + fbm(uv * 6.0, 6, 3, 0.5, 202) * 0.5;
            let keep = smoothstep(0.42, 0.3, chips);
            let grime = 0.78 + 0.18 * (fbm(uv * 12.0, 12, 4, 0.5, 203) * 0.5 + 0.5);
            let p = &mut c.px[y * n + x];
            p[3] *= keep;
            for ch in p.iter_mut().take(3) {
                *ch *= grime;
            }
        }
    }
    c.image(true)
}

// ---------------------------------------------------------------------------- sign atlas

/// Rows of the backlit sign atlas: text, atlas line and left edge in px (a short sign shares
/// its line). A leading two-digit number gets an orange box.
const SIGN_ROWS: &[(&str, usize, f32)] = &[
    ("01 SLIDE", 0, 0.0),
    ("02 WALL RUN", 1, 0.0),
    ("03 WALL JUMP", 2, 0.0),
    ("04 MANTLE", 3, 0.0),
    ("05 JUMP", 4, 0.0),
    ("PILOT TRAINING", 5, 0.0),
    ("START", 6, 0.0),
    ("06 LEDGE", 7, 0.0),
    ("07 GRAPPLE", 6, 384.0),
    ("08 RIDE", 4, 460.0),
];
const SIGN_ROW_PX: f32 = 128.0;
const SIGN_SCALE: f32 = 11.0;

/// Atlas rectangle and width / height of a sign row.
fn sign_rect(row: usize) -> (Vec2, Vec2, f32) {
    let (text, line, x0) = SIGN_ROWS[row];
    let w = sign_row_width(text);
    let y0 = line as f32 * SIGN_ROW_PX;
    (
        Vec2::new(x0 / ATLAS_PX, y0 / ATLAS_PX),
        Vec2::new((x0 + w) / ATLAS_PX, (y0 + SIGN_ROW_PX) / ATLAS_PX),
        w / SIGN_ROW_PX,
    )
}

fn sign_row_width(text: &str) -> f32 {
    let numbered = text.len() > 3 && text.as_bytes()[0].is_ascii_digit();
    let lead = if numbered { 26.0 } else { 34.0 };
    lead + text_width(text) * SIGN_SCALE + 34.0 + if numbered { 16.0 } else { 0.0 }
}

fn sign_atlas(size: u32) -> Image {
    let n = size as usize;
    let k = size as f32 / ATLAS_PX;
    let mut c = Canvas::new(n, n, [0.012, 0.014, 0.018, 1.0]);
    let orange = [1.0, 0.36, 0.06, 1.0];
    let white = [0.95, 0.96, 1.0, 1.0];
    for &(text, line, x0) in SIGN_ROWS {
        let y0 = line as f32 * SIGN_ROW_PX;
        let w = sign_row_width(text);
        // A thin cyan rule along the bottom.
        c.rect(
            (x0 + 14.0) * k,
            (y0 + 112.0) * k,
            (x0 + w - 14.0) * k,
            (y0 + 116.0) * k,
            1.0,
            [0.2, 0.75, 1.0, 1.0],
        );
        let base = (y0 + 98.0) * k;
        let numbered = text.len() > 3 && text.as_bytes()[0].is_ascii_digit();
        if numbered {
            let (num, word) = text.split_at(2);
            let box_w = text_width(num) * SIGN_SCALE + 28.0;
            c.rect(
                (x0 + 12.0) * k,
                (y0 + 14.0) * k,
                (x0 + 12.0 + box_w) * k,
                (y0 + 106.0) * k,
                6.0 * k,
                orange,
            );
            c.text(
                num,
                (x0 + 26.0) * k,
                base,
                SIGN_SCALE * k,
                1.15,
                [0.02, 0.02, 0.02, 1.0],
            );
            let x = x0 + 12.0 + box_w + 16.0 - SPACE_ADVANCE * SIGN_SCALE;
            c.text(
                word,
                (x + SPACE_ADVANCE * SIGN_SCALE) * k,
                base,
                SIGN_SCALE * k,
                0.95,
                white,
            );
        } else {
            c.text(text, (x0 + 34.0) * k, base, SIGN_SCALE * k, 0.95, white);
        }
    }
    c.image(true)
}

// ---------------------------------------------------------------------------- crest

/// A mitsudomoe (three comma) crest in a ring: white, alpha = crest.
fn crest_image(size: u32) -> Image {
    let n = size as usize;
    let mut c = Canvas::new(n, n, [1.0, 1.0, 1.0, 0.0]);
    let px = 1.0 / n as f32;
    c.paint(
        0.0,
        0.0,
        n as f32,
        n as f32,
        [1.0, 1.0, 1.0, 1.0],
        |x, y| {
            let p = (Vec2::new(x, y) * px - Vec2::splat(0.5)) * 2.0; // −1..1
            let rho = p.length();
            let phi = p.y.atan2(p.x);
            let aa = 2.0 * px * 2.0;
            // Outer ring.
            let ring = (0.045 - (rho - 0.9).abs()) / aa + 0.5;
            let mut best = ring.clamp(0.0, 1.0);
            for k in 0..3 {
                let a0 = k as f32 * TAU / 3.0 + 0.4;
                // Head.
                let head_c = Vec2::new(a0.cos(), a0.sin()) * 0.33;
                let head = (0.25 - p.distance(head_c)) / aa + 0.5;
                // Tail: spirals out from the head, thinning.
                let mut d = (phi - a0).rem_euclid(TAU);
                if d > TAU * 0.45 {
                    d -= TAU;
                }
                let t = d / (TAU * 0.4);
                let tail = if (0.0..=1.0).contains(&t) {
                    let r = 0.33 + 0.33 * t;
                    let w = 0.25 * (1.0 - t).powf(0.9);
                    (w - (rho - r).abs()) / aa + 0.5
                } else {
                    0.0
                };
                best = best.max(head.clamp(0.0, 1.0)).max(tail.clamp(0.0, 1.0));
            }
            best
        },
    );
    c.image(true)
}

// ---------------------------------------------------------------------------- inlay

/// Meters of ring circumference one inlay tile covers.
const INLAY_TILE: f32 = 0.75;
/// Inner and outer radius of the inlay band, meters.
const INLAY_R: (f32, f32) = (6.55, 6.9);

/// Groove mask of the inlay pattern at `uv` (U along the ring, one tile; V across the band).
fn inlay_groove(uv: Vec2) -> f32 {
    let band = INLAY_R.1 - INLAY_R.0;
    let (u_m, v_m) = (uv.x * INLAY_TILE, uv.y * band);
    let line = |v: f32, w: f32| smoothstep(w, w * 0.5, (v_m - v).abs());
    let mut g = line(0.05, 0.009).max(line(band - 0.05, 0.009));
    // A diamond between the lines, and dots at the tile ends.
    let c = Vec2::new(INLAY_TILE * 0.5, band * 0.5);
    let q = (Vec2::new(u_m, v_m) - c).abs();
    let diamond = q.x / 0.13 + q.y / 0.095;
    g = g.max(smoothstep(0.012, 0.004, (diamond - 1.0).abs() * 0.1));
    g = g.max(smoothstep(0.03, 0.022, Vec2::new(u_m, v_m).distance(c)));
    for end in [0.0, INLAY_TILE] {
        let d = Vec2::new(u_m - end, v_m - band * 0.5).length();
        g = g.max(smoothstep(0.022, 0.016, d));
    }
    // Ticks between dot and diamond.
    for t in [0.17, INLAY_TILE - 0.17] {
        let tick =
            smoothstep(0.005, 0.0025, (u_m - t).abs()) * smoothstep(0.06, 0.05, (v_m - c.y).abs());
        g = g.max(tick);
    }
    g
}

fn inlay_textures(size: u32) -> (super::TextureSet, Image) {
    let set = bake(size, INLAY_TILE, 1.0, |uv| {
        let g = inlay_groove(uv);
        let speck = fbm(uv * 64.0, 64, 2, 0.5, 301);
        let tone = fbm(uv * 4.0, 4, 3, 0.5, 302);
        let stone = hex(0x3d3f44) * (1.0 + 0.12 * tone + 0.08 * speck);
        Surface {
            albedo: stone * (1.0 - 0.75 * g),
            height: -0.003 * g + 0.0003 * speck,
            roughness: 0.55 + 0.2 * tone.abs() + 0.2 * g,
            metallic: 0.0,
            ao: 1.0 - 0.5 * g,
        }
    });
    let n = size as usize;
    let mut data = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let uv = Vec2::new((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            let g = inlay_groove(uv);
            let v = (g.powf(1.5) * 255.0) as u8;
            data.extend_from_slice(&[v, v, v, 255]);
        }
    }
    (set, Image::new(size, size, data, true))
}

// ============================================================================ geometry helpers

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(v3(x, y, z))
}

fn rt(rotation: Quat, t: Vec3) -> Mat4 {
    Mat4::from_rotation_translation(rotation, t)
}

/// A box of full `size` centered on the origin with flat 45° chamfers of `bevel` on every
/// edge, every face split into quads about `cell` meters across (so vertex colors can carry
/// grime). Flat shaded; UVs are left for the caller.
fn chamfer_grid(size: Vec3, bevel: f32, cell: f32) -> M {
    let half = size * 0.5;
    let b = bevel.clamp(0.0, half.min_element() * 0.9);
    let inner = half - Vec3::splat(b);
    let samples = |h: f32, i: f32| -> Vec<f32> {
        let n = ((2.0 * i) / cell.max(0.02)).ceil().max(1.0) as usize;
        let mut out = Vec::with_capacity(n + 3);
        if b > 0.0 {
            out.push(-h);
        }
        for k in 0..=n {
            out.push(-i + 2.0 * i * k as f32 / n as f32);
        }
        if b > 0.0 {
            out.push(h);
        }
        out
    };
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut m = M::new();
    for (normal, u_axis, v_axis) in faces {
        let ext = |a: Vec3| a.abs().dot(half);
        let inn = |a: Vec3| a.abs().dot(inner);
        let us = samples(ext(u_axis), inn(u_axis));
        let vs = samples(ext(v_axis), inn(v_axis));
        let base = m.vertices.len() as u32;
        for &bv in &vs {
            for &a in &us {
                let q = normal * ext(normal) + u_axis * a + v_axis * bv;
                let core = q.clamp(-inner, inner);
                let d = q - core;
                let l1 = d.x.abs() + d.y.abs() + d.z.abs();
                let p = if l1 > 1e-7 { core + d * (b / l1) } else { q };
                m.vertex(p, normal, Vec2::ZERO);
            }
        }
        let row = us.len() as u32;
        for iv in 0..vs.len() as u32 - 1 {
            for iu in 0..row - 1 {
                let a = base + iv * row + iu;
                m.quad(a, a + 1, a + row + 1, a + row);
            }
        }
    }
    m.remove_degenerate();
    m.flat_shaded()
}

/// A chamfered box from `min` to `max` with box-mapped UVs in meters.
fn slab(min: Vec3, max: Vec3, bevel: f32, cell: f32) -> M {
    let (min, max) = (min.min(max), min.max(max));
    let mut m =
        chamfer_grid(max - min, bevel, cell).transformed(Mat4::from_translation((min + max) * 0.5));
    m.planar_uvs(1.0);
    m
}

/// A plain box (small bevel, no subdivision) from `min` to `max`.
fn bx(min: Vec3, max: Vec3) -> M {
    let bevel = ((max - min).abs().min_element() * 0.12).min(0.01);
    slab(min, max, bevel, 100.0)
}

/// A rotated box: `size` centered at `center` turned by `rotation`, box-mapped in meters.
fn rbox(center: Vec3, size: Vec3, rotation: Quat, bevel: f32) -> M {
    let mut m = chamfer_grid(size, bevel, 100.0).transformed(rt(rotation, center));
    m.planar_uvs(1.0);
    m
}

/// Re-maps a concrete panel's UVs so U runs 0..1 across the panel (in tile units) along
/// `axis` from `start` over `width` meters, and V is height in meters.
fn panel_uvs(m: &mut M, axis: Vec3, start: f32, width: f32) {
    let tile = PANEL_TILE[0];
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let u = (p.dot(axis) - start) / width * tile;
        v.uv = if n.y.abs() > 0.7 {
            [u, p.dot(axis.cross(Vec3::Y))]
        } else {
            [u, p.y]
        };
    }
}

/// A tube along `path` with `radius`, its UVs in meters along the path.
fn pipe(path: &[Vec3], radius: f32, segments: u32) -> M {
    M::tube(path, radius, segments)
}

/// A straight tube from `a` to `b`.
fn rod(a: Vec3, b: Vec3, radius: f32, segments: u32) -> M {
    M::tube(&[a, b], radius, segments)
}

/// A hanging cable between `a` and `b` sagging `sag` meters at the middle.
fn sagging(a: Vec3, b: Vec3, sag: f32, radius: f32) -> M {
    let n = 12;
    let path: Vec<Vec3> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            a.lerp(b, t) - Vec3::Y * (sag * 4.0 * t * (1.0 - t))
        })
        .collect();
    M::tube(&path, radius, 8)
}

/// A hex bolt head with a washer, its axis along +Y, sitting on y = 0.
fn bolt(radius: f32) -> M {
    let mut m = M::lathe(
        &[
            Vec2::new(radius * 1.45, 0.0),
            Vec2::new(radius * 1.45, radius * 0.18),
            Vec2::new(radius, radius * 0.2),
            Vec2::new(radius, radius * 0.75),
            Vec2::new(radius * 0.8, radius * 0.9),
            Vec2::new(0.0001, radius * 0.92),
        ],
        6,
    )
    .flat_shaded();
    m.planar_uvs(4.0);
    m
}

/// Bolts on a surface facing `normal`, at `points`.
fn bolts(m: &mut M, points: &[Vec3], normal: Vec3, radius: f32) {
    let b = bolt(radius);
    let q = Quat::from_rotation_arc(Vec3::Y, normal.normalize());
    for &p in points {
        m.append(&b, rt(q, p));
    }
}

/// A flat decal quad. `right` × `up` is its normal; `uv0` is the atlas corner at the top
/// left and `uv1` at the bottom right; `color` tints it (linear RGBA).
#[allow(clippy::too_many_arguments)]
fn decal(
    m: &mut M,
    center: Vec3,
    right: Vec3,
    up: Vec3,
    size: Vec2,
    uv0: Vec2,
    uv1: Vec2,
    color: Vec4,
) {
    let (r, u) = (right * size.x * 0.5, up * size.y * 0.5);
    let n = right.cross(up).normalize();
    let base = m.vertices.len() as u32;
    for (p, uv) in [
        (center - r - u, Vec2::new(uv0.x, uv1.y)),
        (center + r - u, Vec2::new(uv1.x, uv1.y)),
        (center + r + u, Vec2::new(uv1.x, uv0.y)),
        (center - r + u, Vec2::new(uv0.x, uv0.y)),
    ] {
        let i = m.vertex(p, n, uv);
        m.vertices[i as usize].color = color.to_array();
    }
    m.quad(base, base + 1, base + 2, base + 3);
}

/// A painted stencil from the paint atlas: glyphs `glyph_h` meters tall.
fn stencil(m: &mut M, row: usize, center: Vec3, right: Vec3, up: Vec3, glyph_h: f32, color: Vec4) {
    let (uv0, uv1, aspect) = paint_rect(row);
    let h = glyph_h * PAINT_ROW_PX / (PAINT_ROW_PX - 2.0 * PAINT_PAD);
    decal(
        m,
        center,
        right,
        up,
        Vec2::new(h * aspect, h),
        uv0,
        uv1,
        color,
    );
}

/// A painted solid line or box from the paint atlas.
fn paint_box(m: &mut M, center: Vec3, right: Vec3, up: Vec3, size: Vec2, color: Vec4) {
    let (uv0, uv1) = paint_solid();
    decal(m, center, right, up, size, uv0, uv1, color);
}

/// A backlit sign face from the sign atlas, `height` meters tall; returns its width.
fn sign_face(m: &mut M, row: usize, center: Vec3, right: Vec3, up: Vec3, height: f32) -> f32 {
    let (uv0, uv1, aspect) = sign_rect(row);
    let w = height * aspect;
    decal(
        m,
        center,
        right,
        up,
        Vec2::new(w, height),
        uv0,
        uv1,
        Vec4::ONE,
    );
    w
}

/// Smooth noise for vertex shading, about −1..1.
fn pn(x: f32, y: f32, seed: u32) -> f32 {
    perlin(Vec2::new(x, y), 4096, seed)
}

/// Outside distance from `p` to an axis-aligned rectangle in the XZ plane.
fn rect_dist(p: Vec2, min: Vec2, max: Vec2) -> f32 {
    let d = (min - p).max(p - max).max(Vec2::ZERO);
    d.length()
}

/// Grime for vertical concrete: dirt and splash at the foot, rain streaks from the top,
/// blotches, dust on top faces. `along` maps a position to a horizontal coordinate.
fn wall_grime(m: &mut M, y0: f32, y1: f32, seed: u32, along: impl Fn(Vec3) -> f32) {
    m.shade(|p, n| {
        let a = along(p);
        // Splash and damp at the foot, ragged along the wall.
        let rise = 0.35 + 0.5 * (pn(a * 0.9, 7.5, seed + 3) * 0.5 + 0.5);
        let foot = 0.5 + 0.5 * smoothstep(0.0, rise, p.y - y0);
        let col = pn(a * 1.3, 0.5, seed) * 0.5 + 0.5;
        let len = 0.5 + 2.8 * (pn(a * 0.5, 3.5, seed + 1) * 0.5 + 0.5);
        let drip = (-(y1 - p.y).max(0.0) / len).exp() * smoothstep(0.35, 0.8, col);
        let blotch = 1.0 + 0.14 * pn(a * 0.45, p.y * 0.45, seed + 2);
        let top = if n.y > 0.5 { 0.85 } else { 1.0 };
        foot * (1.0 - 0.42 * drip) * blotch * top
    });
}

// ============================================================================ the course

/// Course layout (relative to the demo origin), from the demo's collider boxes.
struct CourseLayout {
    pad_min: Vec3,
    pad_max: Vec3,
    wall_a: (Vec3, Vec3),
    wall_b: (Vec3, Vec3),
    tower: (Vec3, Vec3),
    bar: (Vec3, Vec3),
    posts: [(Vec3, Vec3); 2],
    ledge_pad: (Vec3, Vec3),
    ledge_wall: (Vec3, Vec3),
}

fn bounds(b: layout::Block) -> (Vec3, Vec3) {
    let c = DVec3::from_array(b.center).as_vec3();
    let h = Vec3::from(b.size) * 0.5;
    (c - h, c + h)
}

fn course_layout() -> CourseLayout {
    let (pad_min, pad_max) = bounds(layout::RUNWAY);
    CourseLayout {
        pad_min,
        pad_max,
        wall_a: bounds(layout::WALL_A),
        wall_b: bounds(layout::WALL_B),
        tower: bounds(layout::TOWER),
        bar: bounds(layout::SLIDE_BAR),
        posts: [
            bounds(layout::SLIDE_POSTS[0]),
            bounds(layout::SLIDE_POSTS[1]),
        ],
        ledge_pad: bounds(layout::LEDGE_PAD),
        ledge_wall: bounds(layout::LEDGE_WALL),
    }
}

fn paint_color(rgb: u32) -> Vec4 {
    hex(rgb).extend(1.0)
}

const WHITE_PAINT: u32 = 0xe8e6df;
const YELLOW_PAINT: u32 = 0xf2b414;
const ORANGE_PAINT: u32 = 0xec5a1c;

/// The runway pad: saw-cut concrete slabs, hazard-banded edges, floor lights, paint.
fn build_pad() -> Kit {
    let l = course_layout();
    let mut kit = Kit::default();
    let (x0, x1, z0, z1) = (l.pad_min.x, l.pad_max.x, l.pad_min.z, l.pad_max.z);
    let top = l.pad_max.y;
    let wa = l.wall_a;
    let wb = l.wall_b;
    // Joints every 4 m along X, and in line with both wall faces along Z.
    let xs: Vec<f32> = (0..=((x1 - x0) / 4.0).round() as i32)
        .map(|i| x0 + i as f32 * 4.0)
        .collect();
    let zs = [z0, wb.1.z, wa.0.z, z1];
    let g = 0.003;
    let mut concrete = M::new();
    for xw in xs.windows(2) {
        for zw in zs.windows(2) {
            let ga = |v: f32, edge: f32| if (v - edge).abs() < 1e-3 { 0.0 } else { g };
            let min = v3(xw[0] + ga(xw[0], x0), -0.3, zw[0] + ga(zw[0], z0));
            let max = v3(xw[1] - ga(xw[1], x1), top, zw[1] - ga(zw[1], z1));
            concrete.append(&slab(min, max, 0.008, 0.7), Mat4::IDENTITY);
        }
    }
    let tower = l.tower;
    concrete.shade(|p, _| {
        let q = Vec2::new(p.x, p.z);
        let lane = (-((p.z - 12.0) / 1.4).powi(2)).exp()
            * (0.6 + 0.4 * (pn(p.x * 0.3, 1.0, 5) * 0.5 + 0.5));
        let mut ao = 1.0f32;
        for (mn, mx) in [wa, wb, tower] {
            let d = rect_dist(q, Vec2::new(mn.x, mn.z), Vec2::new(mx.x, mx.z));
            ao = ao.min(1.0 - 0.4 * (-d / 0.35).exp());
        }
        let blot = 0.9 + 0.1 * pn(p.x * 0.25, p.z * 0.25, 6);
        let edge = 1.0
            - 0.18
                * (-(q.y - z0)
                    .min(z1 - q.y)
                    .min(q.x - x0)
                    .min(x1 - q.x)
                    .max(0.0)
                    / 0.5)
                    .exp();
        blot * ao * (1.0 - 0.12 * lane) * edge
    });
    kit.add(CONCRETE, concrete);

    // Hazard bands on the slab's outer sides.
    let mut hz = M::new();
    let t = 0.004;
    hz.append(
        &slab(v3(x0, -0.02, z0 - t), v3(x1, top - 0.012, z0), 0.0, 100.0),
        Mat4::IDENTITY,
    );
    hz.append(
        &slab(v3(x0, -0.02, z1), v3(x1, top - 0.012, z1 + t), 0.0, 100.0),
        Mat4::IDENTITY,
    );
    hz.append(
        &slab(v3(x0 - t, -0.02, z0), v3(x0, top - 0.012, z1), 0.0, 100.0),
        Mat4::IDENTITY,
    );
    hz.append(
        &slab(v3(x1, -0.02, z0), v3(x1 + t, top - 0.012, z1), 0.0, 100.0),
        Mat4::IDENTITY,
    );
    // Hazard floor band under the slide bar.
    let bar = l.bar;
    hz.append(
        &slab(
            v3(bar.0.x - 0.35, top, bar.0.z - 0.1),
            v3(bar.1.x + 0.35, top + 0.002, bar.1.z + 0.1),
            0.0,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // Landing zone edge beyond the tower.
    hz.append(
        &slab(
            v3(tower.1.x + 0.6, top, 9.2),
            v3(tower.1.x + 0.9, top + 0.002, 12.8),
            0.0,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    kit.add(HAZARD, hz);

    // Floor lights: steel rings with cyan lenses along both long edges.
    let ring = M::lathe(
        &[
            Vec2::new(0.075, 0.0),
            Vec2::new(0.075, 0.003),
            Vec2::new(0.062, 0.007),
            Vec2::new(0.05, 0.007),
            Vec2::new(0.05, 0.004),
            Vec2::new(0.0001, 0.004),
        ],
        16,
    );
    let lens = M::lathe(
        &[
            Vec2::new(0.048, 0.0),
            Vec2::new(0.048, 0.0055),
            Vec2::new(0.0001, 0.0065),
        ],
        16,
    );
    let mut rings = M::new();
    let mut lenses = M::new();
    let mut x = x0 + 1.0;
    while x < x1 {
        for z in [z0 + 0.32, z1 - 0.32] {
            let behind_a = z > 12.0 && x > wa.0.x - 0.5 && x < wa.1.x + 0.5;
            let behind_b = z < 12.0 && x > wb.0.x - 0.5 && x < tower.1.x + 0.5;
            if behind_a || behind_b {
                continue;
            }
            rings.append(&ring, at(x, top, z));
            lenses.append(&lens, at(x, top, z));
        }
        x += 2.0;
    }
    kit.add(STEEL_DARK, rings);
    kit.add(GLOW_CYAN, lenses);

    // Paint.
    let mut p = M::new();
    let y = top + 0.0025;
    let white = paint_color(WHITE_PAINT);
    let yellow = paint_color(YELLOW_PAINT);
    let orange = paint_color(ORANGE_PAINT);
    // Lane edges (dashed) along the whole pad.
    for z in [z0 + 0.7, z1 - 0.7] {
        let mut xd = x0 + 0.8;
        while xd + 1.2 < x1 {
            p_line(&mut p, v3(xd + 0.6, y, z), Vec3::X, 1.2, 0.1, white);
            xd += 2.0;
        }
    }
    // Start box and line, number and word, for a runner heading +X.
    let start_x = layout::RUN_START[0] as f32;
    p_line(
        &mut p,
        v3(start_x + 1.4, y, 12.0),
        Vec3::Z,
        6.4,
        0.22,
        white,
    );
    for z in [8.8, 15.2] {
        p_line(&mut p, v3(start_x - 0.6, y, z), Vec3::X, 4.0, 0.12, white);
    }
    stencil(
        &mut p,
        0,
        v3(start_x - 1.2, y, 12.0),
        Vec3::Z,
        Vec3::X,
        1.6,
        orange,
    );
    stencil(
        &mut p,
        6,
        v3(start_x + 2.4, y, 12.0),
        Vec3::Z,
        Vec3::X,
        0.55,
        white,
    );
    // Slide: word, chevrons toward the bar.
    let bar_x = (bar.0.x + bar.1.x) * 0.5;
    stencil(
        &mut p,
        5,
        v3(bar_x - 4.2, y, 12.0),
        Vec3::Z,
        Vec3::X,
        0.7,
        yellow,
    );
    for (i, zc) in [10.3, 13.7].into_iter().enumerate() {
        let _ = i;
        stencil(
            &mut p,
            9,
            v3(bar_x - 2.0, y, zc),
            Vec3::X,
            Vec3::NEG_Z,
            0.5,
            yellow,
        );
    }
    // Wall-run approach arrows and the gap between the walls.
    stencil(
        &mut p,
        9,
        v3(wa.0.x - 2.4, y, 13.3),
        Vec3::X,
        Vec3::NEG_Z,
        0.45,
        white,
    );
    stencil(
        &mut p,
        9,
        v3((wa.1.x + wb.0.x) * 0.5, y, 12.0),
        Vec3::X,
        Vec3::NEG_Z,
        0.45,
        white,
    );
    // Wall bases: run zones.
    p_line(
        &mut p,
        v3((wa.0.x + wa.1.x) * 0.5, y, wa.0.z - 0.35),
        Vec3::X,
        wa.1.x - wa.0.x,
        0.08,
        yellow,
    );
    p_line(
        &mut p,
        v3((wb.0.x + wb.1.x) * 0.5, y, wb.1.z + 0.35),
        Vec3::X,
        wb.1.x - wb.0.x,
        0.08,
        yellow,
    );
    // Landing target past the tower.
    let lx = tower.1.x + 1.9;
    stencil(&mut p, 4, v3(lx, y, 11.0), Vec3::Z, Vec3::X, 1.3, white);
    // Slab numbers (every other joint, small, like survey marks).
    for (k, xs) in (1usize..).zip(xs.iter().skip(1).step_by(2)) {
        if *xs >= x1 - 1.0 {
            break;
        }
        stencil(
            &mut p,
            k % 5,
            v3(*xs + 0.5, y, z0 + 0.32),
            Vec3::X,
            Vec3::NEG_Z,
            0.14,
            paint_color(0xb8b6b0),
        );
    }
    kit.add(PAINT, p);
    kit
}

/// A painted line on the floor, `len` long along `dir`, `width` wide.
fn p_line(m: &mut M, center: Vec3, dir: Vec3, len: f32, width: f32, color: Vec4) {
    let up = dir.cross(Vec3::Y).normalize() * -1.0;
    // right = dir, up chosen so right × up = +Y.
    let up = if dir.cross(up).y > 0.0 { up } else { -up };
    paint_box(m, center, dir, up, Vec2::new(len, width), color);
}

/// Pieces of one precast wall, built in the wall's own frame: face at z = 0 facing −Z, back at
/// z = `t`, x from 0 to `len`, y from `y0` to `y1` (absolute).
struct WallSpec {
    len: f32,
    t: f32,
    y0: f32,
    y1: f32,
    panels: usize,
    /// Big number painted on the face (paint atlas row), with the "WALL RUN" label beside it.
    number: Option<usize>,
    /// Height of the chevron row on the face.
    chevron_y: f32,
    /// The runner runs toward −x in this frame (chevrons point that way).
    run_neg_x: bool,
    /// Kicker braces, conduits, a junction box and a cable on the back (a free-standing wall).
    braced: bool,
    seed: u32,
}

/// Height of the cyan guide light (the joint between the lower and upper panels).
const GUIDE_Y: f32 = 2.0;

fn wall_parts(s: &WallSpec, kit: &mut Kit, to_world: Mat4) {
    let (len, t, y0, y1) = (s.len, s.t, s.y0, s.y1);
    let w = len / s.panels as f32;
    let g = 0.005;
    let reveal = 0.05;
    let mut concrete = M::new();
    for i in 0..s.panels {
        let xa = i as f32 * w;
        let xb = xa + w;
        for (ya, yb) in [(y0, GUIDE_Y - reveal), (GUIDE_Y + reveal, y1)] {
            let mut p = slab(v3(xa + g, ya, 0.0), v3(xb - g, yb, t), 0.022, 0.3);
            panel_uvs(&mut p, Vec3::X, xa, w);
            concrete.append(&p, Mat4::IDENTITY);
        }
    }
    let mut concrete = concrete.transformed(to_world);
    let world_along = {
        let inv = to_world.inverse();
        move |p: Vec3| inv.transform_point3(p).x
    };
    wall_grime(&mut concrete, y0, y1, s.seed, world_along);
    kit.add(PANEL, concrete);

    // Guide light: aluminum channel recessed in the joint, a diffuser per panel.
    let mut alu = M::new();
    alu.append(
        &bx(
            v3(0.02, GUIDE_Y - reveal + 0.004, 0.026),
            v3(len - 0.02, GUIDE_Y + reveal - 0.004, 0.07),
        ),
        Mat4::IDENTITY,
    );
    let mut glow = M::new();
    for i in 0..s.panels {
        let xa = i as f32 * w + 0.07;
        let xb = (i + 1) as f32 * w - 0.07;
        glow.append(
            &bx(v3(xa, GUIDE_Y - 0.022, 0.02), v3(xb, GUIDE_Y + 0.022, 0.03)),
            Mat4::IDENTITY,
        );
        // End caps of each diffuser.
        for x in [xa - 0.025, xb] {
            alu.append(
                &bx(
                    v3(x, GUIDE_Y - 0.03, 0.018),
                    v3(x + 0.025, GUIDE_Y + 0.03, 0.03),
                ),
                Mat4::IDENTITY,
            );
        }
    }
    kit.add(ALU, alu.transformed(to_world));
    kit.add(GLOW_CYAN, glow.transformed(to_world));

    // Steel: coping cap, end channels, strongbacks, base angles.
    let mut steel = M::new();
    let o = 0.015;
    steel.append(
        &bx(v3(-o, y1, -o), v3(len + o, y1 + 0.012, t + o)),
        Mat4::IDENTITY,
    );
    steel.append(
        &bx(v3(-o, y1 - 0.09, -o), v3(len + o, y1, -0.004)),
        Mat4::IDENTITY,
    );
    steel.append(
        &bx(v3(-o, y1 - 0.09, t + 0.004), v3(len + o, y1, t + o)),
        Mat4::IDENTITY,
    );
    let mut cap_bolts = Vec::new();
    let mut x = 0.3;
    while x < len {
        cap_bolts.push(v3(x, y1 - 0.045, -o));
        x += 0.6;
    }
    bolts(&mut steel, &cap_bolts, Vec3::NEG_Z, 0.008);
    // End channels: the end plate is hazard-striped (the wall's edge reads from the run-up).
    let mut ends = M::new();
    for end in [0.0, len] {
        let s_ = if end == 0.0 { -1.0 } else { 1.0 };
        let (xa, xb) = if end == 0.0 {
            (-0.012, 0.0)
        } else {
            (len, len + 0.012)
        };
        ends.append(
            &bx(v3(xa, y0, -0.012), v3(xb, y1 - 0.09, t + 0.012)),
            Mat4::IDENTITY,
        );
        let fx = if end == 0.0 {
            (-0.012, 0.08)
        } else {
            (len - 0.08, len + 0.012)
        };
        steel.append(
            &bx(v3(fx.0, y0, -0.012), v3(fx.1, y1 - 0.09, -0.002)),
            Mat4::IDENTITY,
        );
        steel.append(
            &bx(v3(fx.0, y0, t + 0.002), v3(fx.1, y1 - 0.09, t + 0.012)),
            Mat4::IDENTITY,
        );
        let mut pts = Vec::new();
        let mut y = y0 + 0.35;
        while y < y1 - 0.2 {
            pts.push(v3(end - s_ * 0.04, y, -0.012));
            y += 0.55;
        }
        bolts(&mut steel, &pts, Vec3::NEG_Z, 0.009);
    }
    // Strongbacks on the back at every joint, with kicker braces to floor anchors.
    let mut braces = M::new();
    let reach = 1.65;
    for i in 1..s.panels {
        let x = i as f32 * w;
        let (ya, yb) = (y0 + 0.2, y1 - 0.25);
        steel.append(
            &bx(v3(x - 0.08, ya, t), v3(x + 0.08, yb, t + 0.012)),
            Mat4::IDENTITY,
        );
        steel.append(
            &bx(v3(x - 0.006, ya, t + 0.012), v3(x + 0.006, yb, t + 0.15)),
            Mat4::IDENTITY,
        );
        steel.append(
            &bx(v3(x - 0.08, ya, t + 0.15), v3(x + 0.08, yb, t + 0.165)),
            Mat4::IDENTITY,
        );
        let mut pts = Vec::new();
        let mut y = ya + 0.2;
        while y < yb {
            pts.push(v3(x - 0.05, y, t + 0.165));
            pts.push(v3(x + 0.05, y, t + 0.165));
            y += 0.9;
        }
        bolts(&mut steel, &pts, Vec3::Z, 0.008);
        if !s.braced {
            continue;
        }
        // Brace: clevis at the strongback, pipe down to a bolted floor plate.
        let top_pt = v3(x, 3.3, t + 0.2);
        let foot = v3(x, y0 + 0.07, t + reach);
        steel.append(
            &bx(v3(x - 0.04, 3.2, t + 0.165), v3(x + 0.04, 3.42, t + 0.2)),
            Mat4::IDENTITY,
        );
        braces.append(&rod(top_pt, foot, 0.045, 12), Mat4::IDENTITY);
        let knee_from = top_pt.lerp(foot, 0.55);
        braces.append(
            &rod(knee_from, v3(x, y0 + 0.05, t + 0.45), 0.03, 10),
            Mat4::IDENTITY,
        );
        for f in [foot, v3(x, y0 + 0.05, t + 0.45)] {
            steel.append(
                &bx(
                    v3(f.x - 0.13, y0, f.z - 0.13),
                    v3(f.x + 0.13, y0 + 0.02, f.z + 0.13),
                ),
                Mat4::IDENTITY,
            );
            let pts: Vec<Vec3> = [(-0.09, -0.09), (0.09, -0.09), (-0.09, 0.09), (0.09, 0.09)]
                .iter()
                .map(|(a, b)| v3(f.x + a, y0 + 0.02, f.z + b))
                .collect();
            bolts(&mut steel, &pts, Vec3::Y, 0.012);
        }
        // Turnbuckle sleeve on the brace.
        let mid = top_pt.lerp(foot, 0.3);
        let dir = (foot - top_pt).normalize();
        braces.append(
            &rod(mid - dir * 0.15, mid + dir * 0.15, 0.058, 12),
            Mat4::IDENTITY,
        );
    }
    kit.add(STEEL_DARK, steel.transformed(to_world));
    kit.add(HAZARD, ends.transformed(to_world));
    if s.braced {
        wall_back(s, kit, to_world, braces);
    }
    wall_face(s, kit, to_world);
}

/// Conduits, a junction box and a cable on the back of a free-standing wall, and its braces.
fn wall_back(s: &WallSpec, kit: &mut Kit, to_world: Mat4, braces: M) {
    let (len, t, y0) = (s.len, s.t, s.y0);
    let w = len / s.panels as f32;
    let mut steel = M::new();
    // Conduits along the back on brackets, dropping to a junction box.
    let mut conduit = M::new();
    let cz = t + 0.21;
    for (k, y) in [4.55f32, 4.63].into_iter().enumerate() {
        let r = 0.022;
        let x_drop = 0.35 + k as f32 * 0.09;
        conduit.append(
            &pipe(
                &[
                    v3(len - 0.3, y, cz),
                    v3(x_drop + 0.15, y, cz),
                    v3(x_drop + 0.03, y - 0.03, cz),
                    v3(x_drop, y - 0.15, cz),
                    v3(x_drop, 1.25, cz),
                ],
                r,
                10,
            ),
            Mat4::IDENTITY,
        );
    }
    for i in 1..s.panels {
        let x = i as f32 * w;
        steel.append(
            &bx(v3(x - 0.03, 4.5, t + 0.165), v3(x + 0.03, 4.68, cz + 0.03)),
            Mat4::IDENTITY,
        );
    }
    let jb = (v3(0.18, 0.75, t + 0.012), v3(0.72, 1.3, t + 0.2));
    conduit.append(&slab(jb.0, jb.1, 0.012, 100.0), Mat4::IDENTITY);
    conduit.append(
        &bx(v3(0.26, 1.15, jb.1.z), v3(0.42, 1.22, jb.1.z + 0.004)),
        Mat4::IDENTITY,
    );
    kit.add(STEEL, conduit.transformed(to_world));
    kit.add(STEEL_DARK, steel.transformed(to_world));
    kit.add(STEEL_YELLOW, braces.transformed(to_world));

    // A heavy cable from the junction box down to the floor and along the wall's back.
    let mut cable = M::new();
    let cable_path: Vec<Vec3> = {
        let mut p = vec![
            v3(0.45, 0.78, t + 0.12),
            v3(0.47, 0.4, t + 0.22),
            v3(0.55, y0 + 0.035, t + 0.4),
        ];
        let mut x = 1.2;
        let mut k = 0;
        while x < len - 0.6 {
            let z = t
                + 0.4
                + 0.12 * pn(x * 0.7, 1.0, s.seed + 9)
                + if k % 2 == 0 { 0.05 } else { -0.03 };
            p.push(v3(x, y0 + 0.035, z));
            x += 0.9;
            k += 1;
        }
        p.push(v3(len + 0.4, y0 + 0.035, t + 0.5));
        p
    };
    cable.append(&pipe(&cable_path, 0.032, 10), Mat4::IDENTITY);
    kit.add(CABLE, cable.transformed(to_world));
}

/// Face paint: hazard foot band, a big number and its label, a row of chevrons.
fn wall_face(s: &WallSpec, kit: &mut Kit, to_world: Mat4) {
    let (len, y0) = (s.len, s.y0);
    let w = len / s.panels as f32;
    let mut hz = M::new();
    hz.append(
        &slab(v3(0.0, y0, -0.003), v3(len, y0 + 0.28, 0.0), 0.0, 100.0),
        Mat4::IDENTITY,
    );
    kit.add(HAZARD, hz.transformed(to_world));
    let mut paint = M::new();
    let right = Vec3::NEG_X;
    let face = -0.004;
    let num_x = if s.run_neg_x { len - 1.25 } else { 1.25 };
    if let Some(number) = s.number {
        stencil(
            &mut paint,
            number,
            v3(num_x, 3.75, face),
            right,
            Vec3::Y,
            1.3,
            paint_color(ORANGE_PAINT),
        );
        // Wall label next to the number.
        let word_x = if s.run_neg_x {
            num_x - 2.2
        } else {
            num_x + 2.2
        };
        stencil(
            &mut paint,
            7,
            v3(word_x, 3.5, face),
            right,
            Vec3::Y,
            0.32,
            paint_color(WHITE_PAINT),
        );
    }
    let chev_row = 9;
    let (mut x, last) = if s.number.is_some() {
        (2.4, len - 1.8)
    } else {
        (w * 0.5, len - w * 0.5 + 0.01)
    };
    while x < last {
        if s.number.is_none() || (x - num_x).abs() > 1.4 {
            let dir = if s.run_neg_x { right } else { -right };
            stencil(
                &mut paint,
                chev_row,
                v3(x, s.chevron_y, face),
                dir,
                Vec3::Y,
                0.36,
                paint_color(WHITE_PAINT),
            );
        }
        x += w;
    }
    kit.add(PAINT, paint.transformed(to_world));
}

fn build_walls() -> Kit {
    let l = course_layout();
    let mut kit = Kit::default();
    let (a0, a1) = l.wall_a;
    let (b0, b1) = l.wall_b;
    // Wall A: face toward −Z at its min z; frame x runs with world +X.
    wall_parts(
        &WallSpec {
            len: a1.x - a0.x,
            t: a1.z - a0.z,
            y0: a0.y,
            y1: a1.y,
            panels: 5,
            number: Some(1),
            chevron_y: GUIDE_Y + 0.42,
            run_neg_x: false,
            braced: true,
            seed: 401,
        },
        &mut kit,
        at(a0.x, 0.0, a0.z),
    );
    // Wall B: face toward +Z at its max z; frame turned half a turn (frame x runs to −X).
    wall_parts(
        &WallSpec {
            len: b1.x - b0.x,
            t: b1.z - b0.z,
            y0: b0.y,
            y1: b1.y,
            panels: 5,
            number: Some(2),
            chevron_y: GUIDE_Y + 0.42,
            run_neg_x: true,
            braced: true,
            seed: 402,
        },
        &mut kit,
        rt(Quat::from_rotation_y(PI), v3(b1.x, 0.0, b1.z)),
    );
    kit
}

/// The slide bar: hazard-striped steel beam on bolted posts, beacons, a sign.
/// The ledge station: a slab between the course and the ring with a precast block on it, too
/// high to climb straight onto. Pilots catch its steel coping, shimmy along it (the chevrons
/// under the coping point the way), climb up and walk off the far end onto the landing box.
fn build_ledge() -> Kit {
    let l = course_layout();
    let mut kit = Kit::default();
    let (p0, p1) = l.ledge_pad;
    let (w0, w1) = l.ledge_wall;
    let top = p1.y;
    // The slab in 3 m bays, with a joint in line with the block's face.
    let xs: Vec<f32> = (0..=((p1.x - p0.x) / 3.0).round() as i32)
        .map(|i| p0.x + i as f32 * 3.0)
        .collect();
    let zs = [p0.z, w0.z, w1.z, p1.z];
    let g = 0.003;
    let mut concrete = M::new();
    for xw in xs.windows(2) {
        for zw in zs.windows(2) {
            let ga = |v: f32, edge: f32| if (v - edge).abs() < 1e-3 { 0.0 } else { g };
            let min = v3(xw[0] + ga(xw[0], p0.x), -0.3, zw[0] + ga(zw[0], p0.z));
            let max = v3(xw[1] - ga(xw[1], p1.x), top, zw[1] - ga(zw[1], p1.z));
            concrete.append(&slab(min, max, 0.008, 0.7), Mat4::IDENTITY);
        }
    }
    concrete.shade(|p, _| {
        let q = Vec2::new(p.x, p.z);
        let d = rect_dist(q, Vec2::new(w0.x, w0.z), Vec2::new(w1.x, w1.z));
        let ao = 1.0 - 0.4 * (-d / 0.35).exp();
        // Scuffed where pilots take off and land.
        let takeoff = (-((p.z - (w1.z + 1.4)) / 0.6).powi(2)).exp()
            * smoothstep(w0.x - 0.5, w0.x + 0.5, p.x)
            * smoothstep(w1.x + 0.5, w1.x - 0.5, p.x);
        let landing = (-(Vec2::new(p.x - (w1.x + 2.0), p.z - (w0.z + w1.z) * 0.5).length() / 1.2)
            .powi(2))
        .exp();
        let blot = 0.9 + 0.1 * pn(p.x * 0.25, p.z * 0.25, 16);
        blot * ao * (1.0 - 0.1 * takeoff - 0.12 * landing)
    });
    kit.add(CONCRETE, concrete);

    // Hazard bands on the slab's sides.
    let mut hz = M::new();
    let t = 0.004;
    for (min, max) in [
        (v3(p0.x, -0.02, p0.z - t), v3(p1.x, top - 0.012, p0.z)),
        (v3(p0.x, -0.02, p1.z), v3(p1.x, top - 0.012, p1.z + t)),
        (v3(p0.x - t, -0.02, p0.z), v3(p0.x, top - 0.012, p1.z)),
        (v3(p1.x, -0.02, p0.z), v3(p1.x + t, top - 0.012, p1.z)),
    ] {
        hz.append(&slab(min, max, 0.0, 100.0), Mat4::IDENTITY);
    }
    kit.add(HAZARD, hz);

    // The block: a precast wall like the course's, face toward +Z (frame x runs to −X).
    wall_parts(
        &WallSpec {
            len: w1.x - w0.x,
            t: w1.z - w0.z,
            y0: w0.y,
            y1: w1.y,
            panels: 3,
            number: None,
            chevron_y: w1.y - 0.55,
            run_neg_x: true,
            braced: false,
            seed: 403,
        },
        &mut kit,
        rt(Quat::from_rotation_y(PI), v3(w1.x, 0.0, w1.z)),
    );

    // Floor paint: the take-off line a stride from the face, a landing box past the far end.
    let mut p = M::new();
    let y = top + 0.0025;
    let yellow = paint_color(YELLOW_PAINT);
    let white = paint_color(WHITE_PAINT);
    p_line(
        &mut p,
        v3((w0.x + w1.x) * 0.5, y, w1.z + 1.4),
        Vec3::X,
        w1.x - w0.x,
        0.08,
        yellow,
    );
    let mid_z = (w0.z + w1.z) * 0.5;
    let land = v3(w1.x + 2.0, y, mid_z);
    for (dx, dz, len, dir) in [
        (0.0, -1.2, 2.4, Vec3::X),
        (0.0, 1.2, 2.4, Vec3::X),
        (-1.2, 0.0, 2.48, Vec3::Z),
        (1.2, 0.0, 2.48, Vec3::Z),
    ] {
        p_line(&mut p, land + v3(dx, 0.0, dz), dir, len, 0.08, white);
    }
    stencil(&mut p, 9, land, Vec3::X, Vec3::NEG_Z, 0.45, white);
    kit.add(PAINT, p);
    kit
}

fn build_bar() -> Kit {
    let l = course_layout();
    let mut kit = Kit::default();
    let (b0, b1) = l.bar;
    let top = l.pad_max.y;
    let cx = (b0.x + b1.x) * 0.5;
    let post_half = 0.14;
    let mut steel = M::new();
    let mut alu = M::new();
    let mut rubber = M::new();
    let mut beacon = M::new();
    let zc: Vec<f32> = l.posts.iter().map(|(a, b)| (a.z + b.z) * 0.5).collect();
    // Beam between the posts: exactly the collider's height (bottom 1.15 m).
    let mut beam = slab(
        v3(b0.x + 0.01, b0.y, zc[0] + post_half),
        v3(b1.x - 0.01, b1.y, zc[1] - post_half),
        0.014,
        0.5,
    );
    beam.shade(|p, _| 0.85 + 0.15 * smoothstep(b0.y, b1.y, p.y));
    kit.add(HAZARD, beam);
    for (i, &z) in zc.iter().enumerate() {
        let (pa, pb) = l.posts[i];
        let ph = pb.y;
        steel.append(
            &slab(
                v3(cx - post_half, top, z - post_half),
                v3(cx + post_half, ph, z + post_half),
                0.012,
                0.4,
            ),
            Mat4::IDENTITY,
        );
        steel.append(
            &bx(
                v3(pa.x - 0.01, ph, pa.z - 0.01),
                v3(pb.x + 0.01, ph + 0.02, pb.z + 0.01),
            ),
            Mat4::IDENTITY,
        );
        // Base plate, anchors, gussets.
        steel.append(
            &slab(
                v3(cx - 0.24, top, z - 0.24),
                v3(cx + 0.24, top + 0.025, z + 0.24),
                0.006,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let anchors: Vec<Vec3> = [(-0.18, -0.18), (0.18, -0.18), (-0.18, 0.18), (0.18, 0.18)]
            .iter()
            .map(|(a, b)| v3(cx + a, top + 0.025, z + b))
            .collect();
        bolts(&mut steel, &anchors, Vec3::Y, 0.016);
        let gusset = M::extrude(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.09, 0.0),
                Vec2::new(0.0, 0.16),
            ],
            0.012,
            0.0,
        );
        for k in 0..4 {
            let q = Quat::from_rotation_y(k as f32 * FRAC_PI_2);
            let out = q * Vec3::X;
            let pos = v3(cx, top + 0.025, z) + out * post_half + (q * Vec3::Z) * -0.006;
            steel.append(&gusset, rt(q, pos));
        }
        // Rubber bumper sleeve where shins meet the post.
        rubber.append(
            &slab(
                v3(cx - post_half - 0.012, 0.45, z - post_half - 0.012),
                v3(cx + post_half + 0.012, 0.95, z + post_half + 0.012),
                0.01,
                0.2,
            ),
            Mat4::IDENTITY,
        );
        // Bracket plates holding the beam, with bolts.
        let inward = if i == 0 { 1.0 } else { -1.0 };
        for side in [-1.0f32, 1.0] {
            let x = cx + side * (post_half + 0.006);
            steel.append(
                &bx(
                    v3(x - 0.006, b0.y - 0.02, z - 0.1),
                    v3(x + 0.006, b1.y + 0.02, z + inward * 0.32),
                ),
                Mat4::IDENTITY,
            );
            let pts: Vec<Vec3> = [0.08f32, 0.24]
                .iter()
                .flat_map(|dz| {
                    [b0.y + 0.1, b1.y - 0.1].map(|y| v3(x + side * 0.006, y, z + inward * dz))
                })
                .collect();
            bolts(&mut steel, &pts, Vec3::X * side, 0.011);
        }
        // Beacon on the post.
        let by = ph + 0.02;
        steel.append(
            &M::lathe(
                &[
                    Vec2::new(0.07, 0.0),
                    Vec2::new(0.07, 0.035),
                    Vec2::new(0.055, 0.04),
                ],
                16,
            ),
            at(cx, by, z),
        );
        beacon.append(
            &M::lathe(
                &[
                    Vec2::new(0.052, 0.0),
                    Vec2::new(0.052, 0.07),
                    Vec2::new(0.035, 0.11),
                    Vec2::new(0.0001, 0.12),
                ],
                16,
            ),
            at(cx, by + 0.04, z),
        );
        for k in 0..4 {
            let a = k as f32 * FRAC_PI_2 + 0.4;
            let d = v3(a.cos(), 0.0, a.sin()) * 0.06;
            alu.append(
                &rod(
                    v3(cx, by + 0.04, z) + d,
                    v3(cx, by + 0.15, z) + d * 0.6,
                    0.004,
                    5,
                ),
                Mat4::IDENTITY,
            );
        }
    }
    // Sign box on the beam, facing both ways.
    let sy = b1.y + 0.1;
    let (sw, sh) = (1.55, 0.42);
    alu.append(
        &slab(
            v3(cx - 0.05, sy, 12.0 - sw * 0.5),
            v3(cx + 0.05, sy + sh, 12.0 + sw * 0.5),
            0.01,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for dz in [-0.45, 0.45] {
        steel.append(
            &rod(
                v3(cx, b1.y, 12.0 + dz),
                v3(cx, sy + 0.02, 12.0 + dz),
                0.02,
                8,
            ),
            Mat4::IDENTITY,
        );
    }
    let mut sign = M::new();
    for side in [-1.0f32, 1.0] {
        let right = if side < 0.0 { Vec3::Z } else { Vec3::NEG_Z };
        sign_face(
            &mut sign,
            0,
            v3(cx + side * 0.052, sy + sh * 0.5, 12.0),
            right,
            Vec3::Y,
            sh - 0.05,
        );
    }
    kit.add(STEEL_DARK, steel);
    kit.add(ALU, alu);
    kit.add(RUBBER, rubber);
    kit.add(GLOW_AMBER, beacon);
    kit.add(SIGN, sign);
    kit
}

/// The tower: concrete panels around a core, steel nosing and corner guards, railings, a caged
/// ladder, floodlights, a door, vents, pipes and paint.
fn build_tower() -> Kit {
    let l = course_layout();
    let mut kit = Kit::default();
    let (t0, t1) = l.tower;
    let top = t1.y;
    let base = l.pad_max.y;
    let th = 0.2;
    let g = 0.005;
    // Core (hidden) and the top slab.
    let mut concrete = slab(
        v3(t0.x + 0.1, base, t0.z + 0.1),
        v3(t1.x - 0.1, top - 0.12, t1.z - 0.1),
        0.0,
        2.0,
    );
    concrete.append(
        &slab(v3(t0.x, top - 0.12, t0.z), v3(t1.x, top, t1.z), 0.012, 0.5),
        Mat4::IDENTITY,
    );
    concrete.shade(|p, n| {
        let worn = (-((p.z - 10.6) / 1.1).powi(2)).exp();
        let top_face = if n.y > 0.5 { 0.92 - 0.12 * worn } else { 1.0 };
        top_face * (0.92 + 0.08 * pn(p.x * 0.6, p.z * 0.6, 410))
    });
    kit.add(CONCRETE, concrete);
    // Face panels: three per long face, two per short face.
    let mut panels = M::new();
    let ytop = top - 0.12;
    let inset = 0.07;
    // West and east faces (normal ±X), panels along Z.
    for (x_out, x_in) in [(t0.x, t0.x + th), (t1.x, t1.x - th)] {
        let (za, zb) = (t0.z + inset, t1.z - inset);
        let n = 3;
        let w = (zb - za) / n as f32;
        for i in 0..n {
            let z0 = za + i as f32 * w;
            let mut p = slab(
                v3(x_out.min(x_in), base, z0 + g),
                v3(x_out.max(x_in), ytop, z0 + w - g),
                0.02,
                0.3,
            );
            panel_uvs(&mut p, Vec3::Z, z0, w);
            panels.append(&p, Mat4::IDENTITY);
        }
    }
    // South and north faces (normal ±Z), panels along X.
    for (z_out, z_in) in [(t0.z, t0.z + th), (t1.z, t1.z - th)] {
        let (xa, xb) = (t0.x + inset, t1.x - inset);
        let n = 2;
        let w = (xb - xa) / n as f32;
        for i in 0..n {
            let x0 = xa + i as f32 * w;
            let mut p = slab(
                v3(x0 + g, base, z_out.min(z_in)),
                v3(x0 + w - g, ytop, z_out.max(z_in)),
                0.02,
                0.3,
            );
            panel_uvs(&mut p, Vec3::X, x0, w);
            panels.append(&p, Mat4::IDENTITY);
        }
    }
    wall_grime(&mut panels, base, ytop, 411, |p| p.x + p.z);
    kit.add(PANEL, panels);

    let mut dark = M::new();
    let mut yellow = M::new();
    let mut steel = M::new();
    let mut alu = M::new();
    let mut white = M::new();
    let mut red = M::new();
    // Nosing angles on the top edges, corner guards on the vertical edges.
    let a = 0.07;
    let s = 0.006;
    dark.append(
        &bx(v3(t0.x - s, top - a, t0.z - s), v3(t1.x + s, top, t0.z)),
        Mat4::IDENTITY,
    );
    dark.append(
        &bx(v3(t0.x - s, top - a, t1.z), v3(t1.x + s, top, t1.z + s)),
        Mat4::IDENTITY,
    );
    for x in [t0.x, t1.x] {
        let (xa, xb) = if x == t0.x { (x - s, x) } else { (x, x + s) };
        yellow.append(
            &bx(v3(xa, top - a, t0.z), v3(xb, top, t1.z)),
            Mat4::IDENTITY,
        );
    }
    for (x, z) in [(t0.x, t0.z), (t1.x, t0.z), (t0.x, t1.z), (t1.x, t1.z)] {
        let sx = if x == t0.x { -1.0 } else { 1.0 };
        let sz = if z == t0.z { -1.0 } else { 1.0 };
        let leg = 0.11;
        dark.append(
            &bx(
                v3(x.min(x + sx * s), base, z.min(z - sz * leg)),
                v3(x.max(x + sx * s), top - a, z.max(z - sz * leg)),
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &bx(
                v3(x.min(x - sx * leg), base, z.min(z + sz * s)),
                v3(x.max(x - sx * leg), top - a, z.max(z + sz * s)),
            ),
            Mat4::IDENTITY,
        );
    }
    // Railings on the north and south edges, wrapping a little onto the east edge (the jump
    // gap stays open) and the north-west corner (the mantle edge stays open).
    let rail_y = [top + 0.55, top + 1.07];
    let rail_off = 0.05;
    let ladder_x = t1.x - 1.8;
    for side in [-1.0f32, 1.0] {
        let z = if side < 0.0 {
            t0.z - rail_off
        } else {
            t1.z + rail_off
        };
        let x_start = if side < 0.0 { t0.x + 0.6 } else { t0.x + 0.15 };
        let mut posts = vec![x_start];
        let mut x = x_start;
        while x + 1.6 < t1.x - 0.2 {
            x += 1.5;
            posts.push(x);
        }
        posts.push(t1.x - 0.1);
        let gap = side > 0.0;
        for &px in &posts {
            if gap && (px - ladder_x).abs() < 0.45 {
                continue;
            }
            yellow.append(
                &bx(
                    v3(px - 0.025, top - 0.35, z - 0.025),
                    v3(px + 0.025, rail_y[1], z + 0.025),
                ),
                Mat4::IDENTITY,
            );
            for y in [top - 0.25, top - 0.08] {
                dark.append(
                    &bx(
                        v3(px - 0.04, y - 0.04, z - side * rail_off),
                        v3(px + 0.04, y + 0.04, z - side * 0.02),
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
        // Rails (split around the ladder on the north side).
        let spans: Vec<(f32, f32)> = if gap {
            vec![(x_start, ladder_x - 0.42), (ladder_x + 0.42, t1.x - 0.1)]
        } else {
            vec![(x_start, t1.x - 0.1)]
        };
        for (xa, xb) in spans {
            for (k, &y) in rail_y.iter().enumerate() {
                let r = if k == 1 { 0.026 } else { 0.019 };
                yellow.append(&rod(v3(xa, y, z), v3(xb, y, z), r, 10), Mat4::IDENTITY);
            }
            yellow.append(
                &bx(
                    v3(xa, top, z - side * 0.06 - 0.004),
                    v3(xb, top + 0.1, z - side * 0.06 + 0.004),
                ),
                Mat4::IDENTITY,
            );
        }
        // Return along the east edge.
        let xr = t1.x + rail_off;
        let (za, zb) = if side < 0.0 {
            (t0.z - rail_off, t0.z + 1.3)
        } else {
            (t1.z - 1.3, t1.z + rail_off)
        };
        let zend = if side < 0.0 { zb } else { za };
        yellow.append(
            &bx(
                v3(xr - 0.025, top - 0.35, zend - 0.025),
                v3(xr + 0.025, rail_y[1], zend + 0.025),
            ),
            Mat4::IDENTITY,
        );
        for (k, &y) in rail_y.iter().enumerate() {
            let r = if k == 1 { 0.026 } else { 0.019 };
            yellow.append(&rod(v3(xr, y, za), v3(xr, y, zb), r, 10), Mat4::IDENTITY);
            yellow.append(
                &rod(v3(t1.x - 0.1, y, z), v3(xr, y, z), r, 10),
                Mat4::IDENTITY,
            );
        }
    }
    // North-west corner return (short).
    {
        let xr = t0.x - rail_off;
        let (za, zb) = (t1.z - 0.9, t1.z + rail_off);
        yellow.append(
            &bx(
                v3(xr - 0.025, top - 0.35, za - 0.025),
                v3(xr + 0.025, rail_y[1], za + 0.025),
            ),
            Mat4::IDENTITY,
        );
        for (k, &y) in rail_y.iter().enumerate() {
            let r = if k == 1 { 0.026 } else { 0.019 };
            yellow.append(&rod(v3(xr, y, za), v3(xr, y, zb), r, 10), Mat4::IDENTITY);
            yellow.append(
                &rod(v3(xr, y, zb), v3(t0.x + 0.15, y, zb), r, 10),
                Mat4::IDENTITY,
            );
        }
    }
    // Caged ladder on the north face.
    let lz = t1.z + 0.2;
    for sx in [-0.23f32, 0.23] {
        let x = ladder_x + sx;
        steel.append(
            &pipe(
                &[
                    v3(x, base, lz),
                    v3(x, top + 0.95, lz),
                    v3(x, top + 1.08, lz - 0.08),
                    v3(x, top + 1.0, lz - 0.35),
                    v3(x, top, lz - 0.42),
                ],
                0.022,
                10,
            ),
            Mat4::IDENTITY,
        );
        let mut y = base + 0.6;
        while y < top {
            dark.append(
                &bx(v3(x - 0.02, y - 0.03, t1.z), v3(x + 0.02, y + 0.03, lz)),
                Mat4::IDENTITY,
            );
            y += 1.6;
        }
    }
    let mut y = base + 0.3;
    while y < top + 0.05 {
        steel.append(
            &rod(
                v3(ladder_x - 0.23, y, lz),
                v3(ladder_x + 0.23, y, lz),
                0.016,
                8,
            ),
            Mat4::IDENTITY,
        );
        y += 0.3;
    }
    let cage_c = |y: f32| v3(ladder_x, y, lz + 0.02);
    let mut y = 2.5;
    while y < top + 1.05 {
        let hoop: Vec<Vec3> = (0..=12)
            .map(|k| {
                let a = k as f32 / 12.0 * PI;
                cage_c(y) + v3(-a.cos() * 0.36, 0.0, a.sin() * 0.38)
            })
            .collect();
        steel.append(&pipe(&hoop, 0.012, 6), Mat4::IDENTITY);
        y += 0.75;
    }
    for k in 1..6 {
        let a = k as f32 / 6.0 * PI;
        let o = v3(-a.cos() * 0.36, 0.0, a.sin() * 0.38);
        steel.append(
            &rod(cage_c(2.5) + o, cage_c(top + 1.0) + o, 0.01, 5),
            Mat4::IDENTITY,
        );
    }
    // Floodlight poles on the east corners, aiming back over the course.
    for z in [t0.z + 0.45, t1.z - 0.45] {
        let px = t1.x - 0.35;
        steel.append(
            &slab(
                v3(px - 0.15, top, z - 0.15),
                v3(px + 0.15, top + 0.02, z + 0.15),
                0.005,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        steel.append(
            &M::lathe(
                &[
                    Vec2::new(0.055, 0.0),
                    Vec2::new(0.05, 2.6),
                    Vec2::new(0.04, 2.7),
                ],
                12,
            ),
            at(px, top, z),
        );
        let head = v3(px - 0.2, top + 2.65, z);
        let aim = Quat::from_rotation_z(0.55) * Quat::from_rotation_y(FRAC_PI_2);
        dark.append(&rbox(head, v3(0.5, 0.32, 0.12), aim, 0.012), Mat4::IDENTITY);
        // Cooling fins on the back.
        for k in 0..6 {
            let off = aim * v3(-0.2 + k as f32 * 0.08, 0.0, 0.08);
            dark.append(
                &rbox(head + off, v3(0.012, 0.28, 0.05), aim, 0.0),
                Mat4::IDENTITY,
            );
        }
        white.append(
            &rbox(
                head + aim * v3(0.0, 0.0, -0.062),
                v3(0.44, 0.26, 0.006),
                aim,
                0.0,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &rbox(
                v3(px - 0.08, top + 2.62, z),
                v3(0.16, 0.04, 0.05),
                Quat::IDENTITY,
                0.005,
            ),
            Mat4::IDENTITY,
        );
        red.append(
            &M::lathe(
                &[
                    Vec2::new(0.045, 0.0),
                    Vec2::new(0.045, 0.03),
                    Vec2::new(0.03, 0.07),
                    Vec2::new(0.0001, 0.08),
                ],
                12,
            ),
            at(px, top + 2.7, z),
        );
    }
    // Door on the east face, with a canopy and a lamp.
    let door_z = 12.7;
    let dx = t1.x;
    let mut orange = M::new();
    dark.append(
        &bx(
            v3(dx, base, door_z - 0.62),
            v3(dx + 0.06, base + 2.36, door_z - 0.52),
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &bx(
            v3(dx, base, door_z + 0.52),
            v3(dx + 0.06, base + 2.36, door_z + 0.62),
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &bx(
            v3(dx, base + 2.26, door_z - 0.62),
            v3(dx + 0.06, base + 2.36, door_z + 0.62),
        ),
        Mat4::IDENTITY,
    );
    orange.append(
        &slab(
            v3(dx, base + 0.01, door_z - 0.52),
            v3(dx + 0.04, base + 2.26, door_z + 0.52),
            0.008,
            0.4,
        ),
        Mat4::IDENTITY,
    );
    for (y0, y1) in [(0.25, 0.75), (1.5, 2.0)] {
        orange.append(
            &bx(
                v3(dx + 0.04, base + y0, door_z - 0.4),
                v3(dx + 0.05, base + y1, door_z + 0.4),
            ),
            Mat4::IDENTITY,
        );
    }
    alu.append(
        &rod(
            v3(dx + 0.1, base + 1.0, door_z - 0.38),
            v3(dx + 0.1, base + 1.15, door_z - 0.38),
            0.014,
            8,
        ),
        Mat4::IDENTITY,
    );
    alu.append(
        &bx(
            v3(dx + 0.04, base + 0.98, door_z - 0.41),
            v3(dx + 0.06, base + 1.17, door_z - 0.35),
        ),
        Mat4::IDENTITY,
    );
    dark.append(
        &slab(
            v3(dx, base + 2.5, door_z - 0.8),
            v3(dx + 0.7, base + 2.54, door_z + 0.8),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for zz in [door_z - 0.7, door_z + 0.7] {
        dark.append(
            &pipe(
                &[v3(dx, base + 2.15, zz), v3(dx + 0.66, base + 2.5, zz)],
                0.015,
                6,
            ),
            Mat4::IDENTITY,
        );
    }
    dark.append(
        &bx(
            v3(dx, base + 2.36, door_z - 0.1),
            v3(dx + 0.14, base + 2.46, door_z + 0.1),
        ),
        Mat4::IDENTITY,
    );
    white.append(
        &bx(
            v3(dx + 0.02, base + 2.355, door_z - 0.08),
            v3(dx + 0.13, base + 2.36, door_z + 0.08),
        ),
        Mat4::IDENTITY,
    );
    kit.add(STEEL_ORANGE, orange);
    // Louvered vent on the east face, above the jump.
    let (vz0, vz1, vy0, vy1) = (8.7, 10.3, 3.0, 4.1);
    alu.append(
        &slab(
            v3(dx, vy0, vz0),
            v3(dx + 0.08, vy0 + 0.05, vz1),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    alu.append(
        &slab(
            v3(dx, vy1 - 0.05, vz0),
            v3(dx + 0.08, vy1, vz1),
            0.006,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for z in [vz0, vz1 - 0.05] {
        alu.append(
            &slab(v3(dx, vy0, z), v3(dx + 0.08, vy1, z + 0.05), 0.006, 100.0),
            Mat4::IDENTITY,
        );
    }
    dark.append(
        &bx(
            v3(dx - 0.02, vy0 + 0.05, vz0 + 0.05),
            v3(dx + 0.01, vy1 - 0.05, vz1 - 0.05),
        ),
        Mat4::IDENTITY,
    );
    let mut y = vy0 + 0.1;
    while y < vy1 - 0.08 {
        alu.append(
            &rbox(
                v3(dx + 0.045, y, (vz0 + vz1) * 0.5),
                v3(0.075, 0.008, vz1 - vz0 - 0.1),
                Quat::from_rotation_z(-0.7),
                0.0,
            ),
            Mat4::IDENTITY,
        );
        y += 0.075;
    }
    // Pipes and a cabinet on the south face (toward the ring).
    let sz = t0.z;
    for (k, px) in [20.2f32, 20.42].into_iter().enumerate() {
        let r = 0.045 - k as f32 * 0.012;
        steel.append(
            &pipe(
                &[
                    v3(px, base, sz - 0.12),
                    v3(px, 4.4, sz - 0.12),
                    v3(px, 4.58, sz - 0.07),
                    v3(px, 4.62, sz + 0.02),
                ],
                r,
                12,
            ),
            Mat4::IDENTITY,
        );
        let mut y = base + 0.6;
        while y < 4.3 {
            dark.append(
                &bx(
                    v3(px - r - 0.012, y, sz - 0.12 - r - 0.012),
                    v3(px + r + 0.012, y + 0.04, sz),
                ),
                Mat4::IDENTITY,
            );
            y += 1.1;
        }
    }
    steel.append(
        &slab(
            v3(21.2, base, sz - 0.38),
            v3(22.1, base + 1.3, sz),
            0.015,
            0.3,
        ),
        Mat4::IDENTITY,
    );
    for k in 0..6 {
        let y = base + 0.85 + k as f32 * 0.05;
        dark.append(
            &bx(v3(21.35, y, sz - 0.385), v3(21.95, y + 0.022, sz - 0.378)),
            Mat4::IDENTITY,
        );
    }
    steel.append(
        &bx(
            v3(21.17, base + 1.3, sz - 0.41),
            v3(22.13, base + 1.34, sz + 0.0),
        ),
        Mat4::IDENTITY,
    );
    // Roof hatch and a vent stack, out of the runner's way.
    steel.append(
        &slab(v3(19.8, top, 13.3), v3(20.9, top + 0.22, 14.2), 0.02, 0.5),
        Mat4::IDENTITY,
    );
    dark.append(
        &bx(v3(19.78, top + 0.22, 13.28), v3(20.92, top + 0.25, 14.22)),
        Mat4::IDENTITY,
    );
    steel.append(
        &pipe(
            &[
                v3(24.0, top, 8.3),
                v3(24.0, top + 0.7, 8.3),
                v3(24.0, top + 0.86, 8.42),
                v3(24.0, top + 0.82, 8.6),
            ],
            0.09,
            14,
        ),
        Mat4::IDENTITY,
    );
    // A cable tray down the south face from the floodlight.
    let mut cable = M::new();
    cable.append(
        &sagging(
            v3(t1.x - 0.35, top + 1.4, t0.z + 0.45),
            v3(t1.x - 0.5, top + 0.05, t0.z + 0.2),
            0.25,
            0.012,
        ),
        Mat4::IDENTITY,
    );
    cable.append(
        &sagging(
            v3(t1.x - 0.35, top + 1.4, t1.z - 0.45),
            v3(t1.x - 0.5, top + 0.05, t1.z - 0.2),
            0.25,
            0.012,
        ),
        Mat4::IDENTITY,
    );
    cable.append(
        &pipe(
            &[
                v3(t1.x - 0.5, top + 0.05, t0.z + 0.2),
                v3(t1.x - 0.5, top + 0.02, 9.0),
                v3(t1.x - 0.5, top + 0.02, 13.0),
                v3(t1.x - 0.5, top + 0.05, t1.z - 0.2),
            ],
            0.014,
            6,
        ),
        Mat4::IDENTITY,
    );
    kit.add(CABLE, cable);
    kit.add(STEEL_DARK, dark);
    kit.add(STEEL_YELLOW, yellow);
    kit.add(STEEL, steel);
    kit.add(ALU, alu);
    kit.add(GLOW_WHITE, white);
    kit.add(GLOW_RED, red);
    // Tread plate landing in front of the door.
    kit.add(
        TREAD,
        slab(
            v3(t1.x, base, door_z - 0.8),
            v3(t1.x + 1.0, base + 0.012, door_z + 0.8),
            0.004,
            100.0,
        ),
    );

    // Paint: hazard strips on the mantle and jump edges, numbers and words.
    let mut hz = M::new();
    hz.append(
        &slab(
            v3(t0.x, top, t0.z),
            v3(t0.x + 0.3, top + 0.002, t1.z),
            0.0,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    hz.append(
        &slab(
            v3(t1.x - 0.3, top, t0.z),
            v3(t1.x, top + 0.002, t1.z),
            0.0,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    kit.add(HAZARD, hz);
    let mut paint = M::new();
    let y = top + 0.003;
    stencil(
        &mut paint,
        4,
        v3(t1.x - 1.2, y, 11.0),
        Vec3::Z,
        Vec3::X,
        0.9,
        paint_color(ORANGE_PAINT),
    );
    stencil(
        &mut paint,
        8,
        v3(t1.x - 2.35, y, 11.0),
        Vec3::Z,
        Vec3::X,
        0.42,
        paint_color(WHITE_PAINT),
    );
    stencil(
        &mut paint,
        9,
        v3(t0.x + 1.5, y, 11.0),
        Vec3::X,
        Vec3::NEG_Z,
        0.4,
        paint_color(WHITE_PAINT),
    );
    // Big number on the west face (seen from the walls) and the south face.
    stencil(
        &mut paint,
        3,
        v3(t0.x - 0.004, 3.7, 12.6),
        Vec3::Z,
        Vec3::Y,
        1.5,
        paint_color(ORANGE_PAINT),
    );
    stencil(
        &mut paint,
        3,
        v3(23.6, 3.6, t0.z - 0.004),
        Vec3::NEG_X,
        Vec3::Y,
        1.5,
        paint_color(ORANGE_PAINT),
    );
    kit.add(PAINT, paint);
    // Backlit sign on the west face: "04 MANTLE".
    let mut sign = M::new();
    let sh = 0.42;
    let (_, _, aspect) = sign_rect(3);
    let sw = sh * aspect;
    let sc = v3(t0.x - 0.06, 2.0, 12.6);
    let mut box_ = slab(
        v3(sc.x - 0.05, sc.y - sh * 0.5 - 0.03, sc.z - sw * 0.5 - 0.03),
        v3(t0.x, sc.y + sh * 0.5 + 0.03, sc.z + sw * 0.5 + 0.03),
        0.01,
        100.0,
    );
    box_.shade(|_, _| 0.8);
    kit.add(ALU, box_);
    sign_face(
        &mut sign,
        3,
        v3(sc.x - 0.052, sc.y, sc.z),
        Vec3::Z,
        Vec3::Y,
        sh,
    );
    kit.add(SIGN, sign);
    kit
}

/// The start gantry: a box truss on two columns over the start, with a big backlit sign.
fn build_gantry() -> Kit {
    let l = course_layout();
    let mut kit = Kit::default();
    let gx = -21.3;
    let (za, zb) = (l.pad_min.z - 0.75, l.pad_max.z + 0.75);
    let mut dark = M::new();
    let mut concrete = M::new();
    let mut alu = M::new();
    let mut glow = M::new();
    let (y_bot, y_top) = (5.55, 6.25);
    for z in [za, zb] {
        concrete.append(
            &slab(
                v3(gx - 0.45, -0.3, z - 0.45),
                v3(gx + 0.45, 0.22, z + 0.45),
                0.03,
                0.4,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &slab(
                v3(gx - 0.28, 0.22, z - 0.28),
                v3(gx + 0.28, 0.25, z + 0.28),
                0.006,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let pts: Vec<Vec3> = [(-0.21, -0.21), (0.21, -0.21), (-0.21, 0.21), (0.21, 0.21)]
            .iter()
            .map(|(a, b)| v3(gx + a, 0.25, z + b))
            .collect();
        bolts(&mut dark, &pts, Vec3::Y, 0.018);
        dark.append(
            &slab(
                v3(gx - 0.17, 0.25, z - 0.17),
                v3(gx + 0.17, y_top + 0.15, z + 0.17),
                0.012,
                0.5,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &bx(
                v3(gx - 0.2, y_top + 0.15, z - 0.2),
                v3(gx + 0.2, y_top + 0.18, z + 0.2),
            ),
            Mat4::IDENTITY,
        );
    }
    // Box truss: four chords, verticals and diagonals on both faces.
    let (z0, z1) = (za + 0.17, zb - 0.17);
    for x in [gx - 0.25, gx + 0.25] {
        for y in [y_bot, y_top] {
            dark.append(
                &slab(
                    v3(x - 0.05, y - 0.05, z0),
                    v3(x + 0.05, y + 0.05, z1),
                    0.008,
                    1.0,
                ),
                Mat4::IDENTITY,
            );
        }
        let n = 11;
        for k in 0..=n {
            let z = z0 + (z1 - z0) * k as f32 / n as f32;
            dark.append(
                &rod(v3(x, y_bot, z), v3(x, y_top, z), 0.028, 8),
                Mat4::IDENTITY,
            );
            if k < n {
                let zn = z0 + (z1 - z0) * (k + 1) as f32 / n as f32;
                let (ya, yb) = if k % 2 == 0 {
                    (y_bot, y_top)
                } else {
                    (y_top, y_bot)
                };
                dark.append(&rod(v3(x, ya, z), v3(x, yb, zn), 0.024, 8), Mat4::IDENTITY);
            }
        }
    }
    let n = 6;
    for k in 0..=n {
        let z = z0 + (z1 - z0) * k as f32 / n as f32;
        for y in [y_bot, y_top] {
            dark.append(
                &rod(v3(gx - 0.25, y, z), v3(gx + 0.25, y, z), 0.022, 8),
                Mat4::IDENTITY,
            );
        }
    }
    // The sign box, hung from the truss.
    let (sy0, sy1) = (4.35, 5.3);
    let (sz0, sz1) = (8.6, 15.4);
    alu.append(
        &slab(v3(gx - 0.17, sy0, sz0), v3(gx + 0.17, sy1, sz1), 0.02, 0.8),
        Mat4::IDENTITY,
    );
    for z in [9.5, 14.5] {
        dark.append(
            &rod(v3(gx, sy1, z), v3(gx, y_bot, z), 0.025, 8),
            Mat4::IDENTITY,
        );
    }
    let mut sign = M::new();
    let h = sy1 - sy0 - 0.16;
    for side in [-1.0f32, 1.0] {
        let right = if side < 0.0 { Vec3::Z } else { Vec3::NEG_Z };
        sign_face(
            &mut sign,
            5,
            v3(gx + side * 0.172, (sy0 + sy1) * 0.5, 12.0),
            right,
            Vec3::Y,
            h,
        );
    }
    // Downlights under the bottom chords.
    for z in [8.0, 10.6, 13.4, 16.0] {
        dark.append(
            &bx(
                v3(gx - 0.12, y_bot - 0.14, z - 0.2),
                v3(gx + 0.12, y_bot - 0.05, z + 0.2),
            ),
            Mat4::IDENTITY,
        );
        glow.append(
            &bx(
                v3(gx - 0.1, y_bot - 0.145, z - 0.18),
                v3(gx + 0.1, y_bot - 0.14, z + 0.18),
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(STEEL_DARK, dark);
    kit.add(CONCRETE, concrete);
    kit.add(ALU, alu);
    kit.add(SIGN, sign);
    kit.add(GLOW_WHITE, glow);
    kit
}

/// Two floodlight masts at opposite corners of the course.
fn build_masts() -> Kit {
    let mut kit = Kit::default();
    let mut steel = M::new();
    let mut dark = M::new();
    let mut concrete = M::new();
    let mut alu = M::new();
    let mut white = M::new();
    let mut red = M::new();
    for (x, z, yaw) in [(-27.4f32, 18.3f32, 0.75f32), (27.4, 5.7, 0.75 + PI)] {
        concrete.append(
            &M::lathe(
                &[
                    Vec2::new(0.55, -0.3),
                    Vec2::new(0.55, 0.25),
                    Vec2::new(0.45, 0.32),
                ],
                24,
            )
            .flat_shaded(),
            at(x, 0.0, z),
        );
        dark.append(
            &M::lathe(
                &[
                    Vec2::new(0.32, 0.0),
                    Vec2::new(0.32, 0.03),
                    Vec2::new(0.0001, 0.03),
                ],
                16,
            ),
            at(x, 0.32, z),
        );
        let pts: Vec<Vec3> = (0..6)
            .map(|k| {
                let a = k as f32 / 6.0 * TAU;
                v3(x + a.cos() * 0.25, 0.35, z + a.sin() * 0.25)
            })
            .collect();
        bolts(&mut dark, &pts, Vec3::Y, 0.02);
        steel.append(
            &M::lathe(
                &[
                    Vec2::new(0.16, 0.0),
                    Vec2::new(0.09, 9.6),
                    Vec2::new(0.07, 9.7),
                ],
                16,
            ),
            at(x, 0.35, z),
        );
        // Climbing pegs.
        let mut y = 2.5;
        let mut k = 0;
        while y < 9.2 {
            let a = if k % 2 == 0 { 0.0 } else { PI };
            let d = v3(a.cos(), 0.0, a.sin());
            dark.append(
                &rod(v3(x, y, z) + d * 0.08, v3(x, y, z) + d * 0.26, 0.012, 6),
                Mat4::IDENTITY,
            );
            y += 0.4;
            k += 1;
        }
        // Head frame with four LED floods aimed at the course.
        let q = Quat::from_rotation_y(yaw);
        let hc = v3(x, 9.9, z);
        dark.append(&rbox(hc, v3(1.6, 0.08, 0.08), q, 0.01), Mat4::IDENTITY);
        dark.append(
            &rbox(hc + v3(0.0, 0.6, 0.0), v3(1.6, 0.08, 0.08), q, 0.01),
            Mat4::IDENTITY,
        );
        for k in 0..4 {
            let off = q * v3(-0.6 + k as f32 * 0.4, 0.3, 0.12);
            let aim = q * Quat::from_rotation_x(-0.5);
            alu.append(
                &rbox(hc + off, v3(0.34, 0.24, 0.1), aim, 0.012),
                Mat4::IDENTITY,
            );
            white.append(
                &rbox(
                    hc + off + aim * v3(0.0, 0.0, 0.052),
                    v3(0.3, 0.2, 0.005),
                    aim,
                    0.0,
                ),
                Mat4::IDENTITY,
            );
        }
        alu.append(
            &slab(
                v3(x - 0.2, 4.0, z + 0.1),
                v3(x + 0.2, 4.6, z + 0.3),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        red.append(
            &M::lathe(
                &[
                    Vec2::new(0.05, 0.0),
                    Vec2::new(0.05, 0.03),
                    Vec2::new(0.03, 0.08),
                    Vec2::new(0.0001, 0.09),
                ],
                12,
            ),
            at(x, 10.05, z),
        );
    }
    kit.add(STEEL, steel);
    kit.add(STEEL_DARK, dark);
    kit.add(CONCRETE, concrete);
    kit.add(ALU, alu);
    kit.add(GLOW_WHITE, white);
    kit.add(GLOW_RED, red);
    kit
}

/// Numbered sign boxes on posts beside the stations.
fn build_signs() -> Kit {
    let mut kit = Kit::default();
    let mut dark = M::new();
    let mut alu = M::new();
    let mut sign = M::new();
    let mut concrete = M::new();
    // (x, z, sign row, yaw: facing direction angle around Y; 0 faces −X)
    let l = course_layout();
    let spots = [
        (l.wall_a.0.x - 1.3, l.pad_max.z - 0.45, 1usize, -0.6f32),
        (
            (l.wall_a.1.x + l.wall_b.0.x) * 0.5,
            l.pad_min.z + 0.45,
            2,
            0.6,
        ),
        // The ledge station's sign, by the block's start.
        (
            l.ledge_wall.0.x - 1.1,
            l.ledge_wall.1.z + 0.6,
            7,
            FRAC_PI_2 - 0.5,
        ),
        // The grapple station's, at the pad's front corner, facing the course.
        (12.45, 19.4, 8, 1.1),
        // The freight station's, past the platform's west end, facing along the track.
        (-27.6, 24.3, 9, FRAC_PI_2),
    ];
    for (x, z, row, yaw) in spots {
        let q = Quat::from_rotation_y(yaw);
        concrete.append(
            &slab(
                v3(x - 0.25, 0.1, z - 0.25),
                v3(x + 0.25, 0.22, z + 0.25),
                0.02,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        dark.append(
            &M::lathe(&[Vec2::new(0.05, 0.0), Vec2::new(0.05, 2.55)], 12),
            at(x, 0.22, z),
        );
        let (_, _, aspect) = sign_rect(row);
        let sh = 0.38;
        let sw = sh * aspect;
        let c = v3(x, 2.4, z);
        alu.append(
            &rbox(c, v3(0.1, sh + 0.06, sw + 0.06), q, 0.01),
            Mat4::IDENTITY,
        );
        // Sign faces on both sides (the face looks along its normal = right × up).
        for side in [-1.0f32, 1.0] {
            let normal = q * Vec3::X * side;
            let right = Vec3::Y.cross(normal).normalize();
            sign_face(&mut sign, row, c + normal * 0.052, right, Vec3::Y, sh);
        }
    }
    kit.add(STEEL_DARK, dark);
    kit.add(ALU, alu);
    kit.add(SIGN, sign);
    kit.add(CONCRETE, concrete);
    kit
}

// ---------------------------------------------------------------------------- props

/// A corrugated steel sheet `len` long (+X) and `h` tall (+Y): trapezoid ridges every
/// `pitch` meters, ridge faces on z = 0, valleys `depth` behind them; the sheet looks along −Z.
fn corrugated(len: f32, h: f32, pitch: f32, depth: f32) -> M {
    let n = (len / pitch).round().max(1.0) as usize;
    let p = len / n as f32;
    let mut prof = Vec::with_capacity(n * 4 + 1);
    for i in 0..n {
        let x = i as f32 * p;
        prof.push(Vec2::new(x, 0.0));
        prof.push(Vec2::new(x + p * 0.3, 0.0));
        prof.push(Vec2::new(x + p * 0.42, depth));
        prof.push(Vec2::new(x + p * 0.88, depth));
    }
    prof.push(Vec2::new(len, 0.0));
    let mut m = M::new();
    for w in prof.windows(2) {
        let (a, b) = (w[0], w[1]);
        m.flat_quad(
            [
                v3(a.x, 0.0, a.y),
                v3(a.x, h, a.y),
                v3(b.x, h, b.y),
                v3(b.x, 0.0, b.y),
            ],
            [
                Vec2::new(a.x, 0.0),
                Vec2::new(a.x, h),
                Vec2::new(b.x, h),
                Vec2::new(b.x, 0.0),
            ],
        );
    }
    m
}

/// A 20 ft shipping container with its corner at the origin: 6.06 m along +X, 2.44 m along
/// +Z, 2.59 m tall. Corrugated sides and front end, cargo doors with locking bars at +X.
/// Returns (painted shell, galvanized hardware, dark castings and seals).
fn container(seed: u32) -> (M, M, M) {
    let (l, w, h) = (6.06f32, 2.44f32, 2.59f32);
    let mut shell = M::new();
    let mut hw = M::new();
    let mut dark = M::new();
    let post = 0.16;
    // Corner posts and rails.
    for (x, z) in [
        (0.0, 0.0),
        (l - post, 0.0),
        (0.0, w - post),
        (l - post, w - post),
    ] {
        shell.append(
            &slab(v3(x, 0.0, z), v3(x + post, h, z + post), 0.008, 100.0),
            Mat4::IDENTITY,
        );
    }
    for (y0, y1) in [(0.0, 0.16), (h - 0.12, h)] {
        for (z0, z1) in [(0.0, 0.05), (w - 0.05, w)] {
            shell.append(
                &slab(v3(post, y0, z0), v3(l - post, y1, z1), 0.006, 100.0),
                Mat4::IDENTITY,
            );
        }
        for (x0, x1) in [(0.0, 0.05), (l - 0.05, l)] {
            shell.append(
                &slab(v3(x0, y0, post), v3(x1, y1, w - post), 0.006, 100.0),
                Mat4::IDENTITY,
            );
        }
    }
    // Corrugated sides and front end.
    let ch = h - 0.28;
    let side = corrugated(l - 2.0 * post, ch, 0.278, 0.036);
    shell.append(&side, at(post, 0.16, 0.02));
    shell.append(
        &side,
        rt(Quat::from_rotation_y(PI), v3(l - post, 0.16, w - 0.02)),
    );
    let end = corrugated(w - 2.0 * post, ch, 0.28, 0.03);
    shell.append(
        &end,
        rt(Quat::from_rotation_y(FRAC_PI_2), v3(0.02, 0.16, w - post)),
    );
    // Roof: a slightly dished panel between the rails, and the floor edge.
    shell.append(
        &slab(
            v3(post, h - 0.07, 0.05),
            v3(l - post, h - 0.035, w - 0.05),
            0.004,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    shell.append(
        &slab(
            v3(post, 0.02, 0.05),
            v3(l - post, 0.16, w - 0.05),
            0.004,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // Cargo doors: two leaves of vertical corrugation with locking bars, hinges and seals.
    let leaf = (w - 2.0 * post) * 0.5 - 0.008;
    let door = corrugated(leaf, h - 0.3, 0.2, 0.022);
    for k in 0..2 {
        let z0 = post + k as f32 * (leaf + 0.016);
        shell.append(
            &door,
            rt(Quat::from_rotation_y(-FRAC_PI_2), v3(l + 0.006, 0.15, z0)),
        );
        dark.append(
            &bx(
                v3(l - 0.01, 0.15, z0 + leaf - 0.004),
                v3(l + 0.01, h - 0.15, z0 + leaf + 0.004),
            ),
            Mat4::IDENTITY,
        );
        for dz in [0.28f32, leaf - 0.28] {
            let z = z0 + dz;
            hw.append(
                &rod(v3(l + 0.045, 0.12, z), v3(l + 0.045, h - 0.1, z), 0.016, 8),
                Mat4::IDENTITY,
            );
            for y in [0.22, h - 0.22] {
                hw.append(
                    &bx(
                        v3(l + 0.01, y - 0.05, z - 0.04),
                        v3(l + 0.07, y + 0.05, z + 0.04),
                    ),
                    Mat4::IDENTITY,
                );
            }
            // Handle.
            hw.append(
                &bx(v3(l + 0.03, 1.15, z - 0.02), v3(l + 0.075, 1.2, z + 0.24)),
                Mat4::IDENTITY,
            );
            for y in [0.9, 1.6, 2.1] {
                hw.append(
                    &bx(
                        v3(l + 0.01, y - 0.025, z - 0.035),
                        v3(l + 0.06, y + 0.025, z + 0.035),
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
        for y in [0.45, 1.3, 2.15] {
            let zh = if k == 0 { post - 0.02 } else { w - post + 0.02 };
            hw.append(
                &bx(
                    v3(l - 0.02, y - 0.06, zh - 0.03),
                    v3(l + 0.04, y + 0.06, zh + 0.03),
                ),
                Mat4::IDENTITY,
            );
        }
    }
    // Corner castings, a little proud of the frame.
    let (cx, cy, cz) = (0.178, 0.118, 0.162);
    for (x, z) in [(0.0, 0.0), (l - cx, 0.0), (0.0, w - cz), (l - cx, w - cz)] {
        for y in [0.0, h - cy] {
            dark.append(
                &slab(
                    v3(x - 0.004, y, z - 0.004),
                    v3(x + cx + 0.004, y + cy + 0.004, z + cz + 0.004),
                    0.01,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
    }
    // Rust and grime: dark feet, streaks under the roof rail, scuffs.
    let s = seed;
    shell.shade(|p, _| {
        let feet = 0.62 + 0.38 * smoothstep(0.0, 0.7, p.y);
        let col = pn(p.x * 2.2 + p.z * 2.2, 0.5, s) * 0.5 + 0.5;
        let streak = (-(h - p.y) / (0.4 + 0.9 * col)).exp() * smoothstep(0.45, 0.8, col);
        let blot = 1.0 + 0.12 * pn(p.x * 0.6 + p.z * 0.3, p.y * 0.6, s + 1);
        feet * (1.0 - 0.38 * streak) * blot
    });
    hw.shade(|p, _| 0.7 + 0.3 * smoothstep(0.0, 0.8, p.y));
    (shell, hw, dark)
}

/// A precast F-shape safety barrier 3 m long along +Z, standing on y = 0, centered on x = 0.
fn barrier() -> M {
    let outline = [
        Vec2::new(-0.3, 0.0),
        Vec2::new(0.3, 0.0),
        Vec2::new(0.3, 0.08),
        Vec2::new(0.13, 0.33),
        Vec2::new(0.1, 0.81),
        Vec2::new(-0.1, 0.81),
        Vec2::new(-0.13, 0.33),
        Vec2::new(-0.3, 0.08),
    ];
    let mut m = M::extrude(&outline, 2.96, 0.02).transformed(at(0.0, 0.0, -1.48));
    m.planar_uvs(1.0);
    m.shade(|p, n| {
        let foot = 0.6 + 0.4 * smoothstep(0.0, 0.45, p.y);
        let chip = 1.0 + 0.1 * pn(p.z * 1.7, p.y * 2.0, 71);
        let top = if n.y > 0.7 { 0.88 } else { 1.0 };
        foot * chip * top
    });
    m
}

/// A rugged equipment case `size` with its base center at the origin: ribbed body, dark
/// corner guards and latches. Returns (body, guards).
fn case(size: Vec3) -> (M, M) {
    let half = size * 0.5;
    let mut body = M::new();
    let mut guards = M::new();
    body.append(
        &slab(
            v3(-half.x, 0.04, -half.z),
            v3(half.x, size.y - 0.01, half.z),
            0.025,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    // Lid seam rib and two horizontal ribs.
    for y in [size.y * 0.32, size.y * 0.68] {
        body.append(
            &slab(
                v3(-half.x - 0.012, y - 0.02, -half.z - 0.012),
                v3(half.x + 0.012, y + 0.02, half.z + 0.012),
                0.006,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    // Feet and corner guards.
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let c = v3(sx * (half.x - 0.05), 0.0, sz * (half.z - 0.05));
        guards.append(
            &slab(
                c - v3(0.065, 0.0, 0.065),
                c + v3(0.065, 0.05, 0.065),
                0.012,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        let t = v3(sx * (half.x - 0.035), size.y - 0.06, sz * (half.z - 0.035));
        guards.append(
            &slab(
                t - v3(0.05, 0.0, 0.05),
                t + v3(0.05, 0.075, 0.05),
                0.012,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    // Latches on the long faces.
    for sz in [-1.0f32, 1.0] {
        for x in [-half.x * 0.55, half.x * 0.55] {
            let z = sz * (half.z + 0.012);
            guards.append(
                &slab(
                    v3(x - 0.05, size.y * 0.5 - 0.06, z.min(z + sz * 0.025)),
                    v3(x + 0.05, size.y * 0.5 + 0.06, z.max(z + sz * 0.025)),
                    0.006,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
        // Handle.
        let z = sz * (half.z + 0.03);
        guards.append(
            &rod(
                v3(-0.16, size.y * 0.82, z),
                v3(0.16, size.y * 0.82, z),
                0.014,
                8,
            ),
            Mat4::IDENTITY,
        );
    }
    body.shade(|p, _| 0.75 + 0.25 * smoothstep(0.0, size.y, p.y));
    (body, guards)
}

/// A traffic cone with its base at the origin. Returns (orange body, white band, black base).
fn cone() -> (M, M, M) {
    let body = M::lathe(
        &[
            Vec2::new(0.16, 0.035),
            Vec2::new(0.155, 0.05),
            Vec2::new(0.035, 0.7),
            Vec2::new(0.02, 0.72),
            Vec2::new(0.0001, 0.72),
        ],
        20,
    );
    let band = M::lathe(
        &[
            Vec2::new(0.101, 0.35),
            Vec2::new(0.098, 0.36),
            Vec2::new(0.066, 0.52),
            Vec2::new(0.063, 0.53),
        ],
        20,
    );
    let mut base = slab(v3(-0.2, 0.0, -0.2), v3(0.2, 0.035, 0.2), 0.012, 100.0);
    base.shade(|_, _| 0.8);
    (body, band, base)
}

/// Props around the course, all off the runner's line: shipping containers north of the
/// walls and the tower, safety barriers along the south edge, equipment cases by the start
/// and the tower, cones at the slide bar.
fn build_props() -> Kit {
    let mut kit = Kit::default();
    let mut orange = M::new();
    let mut grey = M::new();
    let mut hw = M::new();
    let mut dark = M::new();
    let (shell_a, hw_a, dark_a) = container(81);
    let (shell_b, hw_b, dark_b) = container(82);
    // Two stacked behind wall A, one behind the tower (turned so its doors face west).
    for (shell, h_, d_, to, grey_) in [
        (&shell_a, &hw_a, &dark_a, at(-7.4, 0.0, 18.4), false),
        (&shell_b, &hw_b, &dark_b, at(-7.2, 2.59, 18.45), true),
        (
            &shell_b,
            &hw_b,
            &dark_b,
            rt(Quat::from_rotation_y(PI), v3(30.6, 0.0, 21.2)),
            true,
        ),
    ] {
        if grey_ {
            grey.append(shell, to);
        } else {
            orange.append(shell, to);
        }
        hw.append(h_, to);
        dark.append(d_, to);
    }
    // Barriers along the south edge of the pad, west of the stepping stones.
    let mut concrete = M::new();
    let mut yellow = M::new();
    let b = barrier();
    for i in 0..4 {
        let x = -26.0 + i as f32 * 3.04;
        let to = rt(Quat::from_rotation_y(FRAC_PI_2), v3(x, 0.0, 6.15));
        concrete.append(&b, to);
        // Reflective strip on the course side.
        yellow.append(
            &bx(v3(x - 1.2, 0.6, 6.255), v3(x + 1.2, 0.68, 6.262)),
            Mat4::IDENTITY,
        );
    }
    // Cases: a stack by the start and a pair by the tower.
    let mut cases = M::new();
    let mut guards = M::new();
    for (size, pos, yaw) in [
        (v3(1.2, 0.6, 0.8), v3(-26.6, 0.1, 16.3), 0.05f32),
        (v3(0.9, 0.5, 0.6), v3(-26.5, 0.7, 16.3), -0.12),
        (v3(1.0, 0.7, 0.7), v3(-24.9, 0.1, 16.4), 0.3),
        (v3(1.2, 0.6, 0.8), v3(26.3, 0.1, 16.2), -0.08),
        (v3(0.8, 0.45, 0.6), v3(26.1, 0.7, 16.2), 0.2),
    ] {
        let (body, g) = case(size);
        let to = rt(Quat::from_rotation_y(yaw), pos);
        cases.append(&body, to);
        guards.append(&g, to);
    }
    // Cones flanking the slide bar.
    let (cone_body, cone_band, cone_base) = cone();
    let mut cone_o = M::new();
    let mut cone_w = M::new();
    let mut rubber = M::new();
    for (x, z) in [
        (-15.6, 7.55),
        (-12.4, 7.6),
        (-15.5, 16.45),
        (-12.5, 16.4),
        (-26.0, 7.6),
    ] {
        cone_o.append(&cone_body, at(x, 0.1, z));
        cone_w.append(&cone_band, at(x, 0.1, z));
        rubber.append(&cone_base, at(x, 0.1, z));
    }
    kit.add(STEEL_ORANGE, orange);
    kit.add(STEEL, grey);
    kit.add(ALU, hw);
    kit.add(STEEL_DARK, dark);
    kit.add(CONCRETE, concrete);
    kit.add(STEEL_YELLOW, yellow);
    kit.add(STEEL_OLIVE, cases);
    kit.add(RUBBER, guards);
    kit.add(STEEL_ORANGE, cone_o);
    kit.add(STEEL_WHITE, cone_w);
    kit.add(RUBBER, rubber);
    kit
}

// ============================================================================ the ring

/// Ring layout constants (relative to the ring center).
const RING_HALF: f32 = layout::RING_RADIUS;
/// Apothem of the octagonal board floor.
const FLOOR_APOTHEM: f32 = 7.2;
/// Ring top (the collider's top face).
const RING_TOP: f32 = 0.1;
/// Width of the raked-gravel apron around the plinth.
const APRON: f32 = 1.85;

fn octagon(apothem: f32) -> [Vec2; 8] {
    let r = apothem / (PI / 8.0).cos();
    std::array::from_fn(|k| {
        let a = (k as f32 + 0.5) * TAU / 8.0;
        Vec2::new(a.cos(), a.sin()) * r
    })
}

/// A flat polygon fan in the XZ plane (outline counter-clockwise seen from above), split
/// into rings so vertex colors vary, UVs from `uv(p)`.
fn flat_fan(outline: &[Vec2], center: Vec2, y: f32, rings: usize, uv: impl Fn(Vec2) -> Vec2) -> M {
    let mut m = M::new();
    let n = outline.len();
    for k in 0..n {
        let (a, b) = (outline[k], outline[(k + 1) % n]);
        // Triangle (center, a, b) subdivided into `rings` rows.
        let mut rows: Vec<Vec<u32>> = Vec::new();
        for i in 0..=rings {
            let t = i as f32 / rings as f32;
            let (pa, pb) = (center.lerp(a, t), center.lerp(b, t));
            let row: Vec<u32> = (0..=i)
                .map(|j| {
                    let s = if i == 0 { 0.0 } else { j as f32 / i as f32 };
                    let p = pa.lerp(pb, s);
                    m.vertex(v3(p.x, y, p.y), Vec3::Y, uv(p))
                })
                .collect();
            rows.push(row);
        }
        for i in 0..rings {
            let (r0, r1) = (&rows[i], &rows[i + 1]);
            for j in 0..=i {
                // Upward triangle (r0[j], r1[j+1], r1[j]) wound CCW from above (+Y).
                m.tri(r0[j], r1[j + 1], r1[j]);
                if j < i {
                    m.tri(r0[j], r0[j + 1], r1[j + 1]);
                }
            }
        }
    }
    // Make sure the fan faces up whatever the outline order.
    let up = m
        .indices
        .chunks(3)
        .next()
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
            (b - a).cross(c - a).y
        })
        .unwrap_or(1.0);
    if up < 0.0 {
        for t in m.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    m
}

/// A prism from a convex outline in the XZ plane, from `y0` to `y1`, with a chamfer.
fn prism(outline: &[Vec2], y0: f32, y1: f32, bevel: f32) -> M {
    // Extrude builds along +Z from an XY outline; swap Y and Z (a mirror, which `append`
    // corrects).
    let e = M::extrude(outline, y1 - y0, bevel);
    let swap = Mat4::from_cols(Vec4::X, Vec4::Z, Vec4::Y, Vec4::new(0.0, y0, 0.0, 1.0));
    let mut m = M::new();
    m.append(&e, swap);
    m.planar_uvs(1.0);
    m
}

/// The ring plinth: board floor, carved stone band, raked corners with curbs, the glowing
/// inlay, a brass crest at the center and brass start lines.
fn build_plinth() -> Kit {
    let mut kit = Kit::default();
    let oct_in = octagon(FLOOR_APOTHEM);
    let oct_out = octagon(RING_HALF);
    // Board floor.
    let mut floor = flat_fan(&oct_in, Vec2::ZERO, RING_TOP, 16, |p| p);
    floor.shade(|p, _| {
        let r = Vec2::new(p.x, p.z).length();
        let scuff =
            (-((r - 4.5) / 1.6).powi(2)).exp() * (0.5 + 0.5 * pn(p.x * 0.8, p.z * 0.8, 501));
        let edge = smoothstep(
            FLOOR_APOTHEM - 0.1,
            FLOOR_APOTHEM - 0.9,
            r.min(FLOOR_APOTHEM),
        );
        (0.94 + 0.06 * pn(p.x * 0.3, p.z * 0.3, 502)) * (1.0 - 0.1 * scuff) * (0.82 + 0.18 * edge)
    });
    // Board floor sides (hidden under the band) are not needed: the band covers its edge.
    kit.add(PLANKS, floor);

    // Carved stone band: two stones per side.
    let mut band = M::new();
    for k in 0..8 {
        let (a0, a1) = (oct_in[k], oct_in[(k + 1) % 8]);
        let (b0, b1) = (oct_out[k], oct_out[(k + 1) % 8]);
        for (s0, s1) in [(0.0, 0.5), (0.5, 1.0)] {
            let outline = [
                a0.lerp(a1, s0),
                b0.lerp(b1, s0),
                b0.lerp(b1, s1),
                a0.lerp(a1, s1),
            ];
            let g = 0.006;
            let c = outline.iter().copied().sum::<Vec2>() / 4.0;
            let shrunk: Vec<Vec2> = outline
                .iter()
                .map(|p| *p + (c - *p).normalize() * g)
                .collect();
            band.append(&prism(&shrunk, -0.25, RING_TOP, 0.018), Mat4::IDENTITY);
        }
    }
    band.shade(|p, n| {
        let moss = if n.y < 0.5 {
            0.85 + 0.15 * smoothstep(-0.05, 0.1, p.y)
        } else {
            1.0
        };
        moss * (0.9 + 0.1 * pn(p.x * 1.1, p.z * 1.1, 503))
    });
    kit.add(STONE, band);

    // Corners: raked gravel trays at the collider's height with stone curbs on the outside.
    let mut sand = M::new();
    let mut curb = M::new();
    for (sx, sz) in [(1.0f32, 1.0f32), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
        let corner = Vec2::new(sx, sz) * RING_HALF;
        let k = (if sx > 0.0 {
            if sz > 0.0 { 0 } else { 7 }
        } else if sz > 0.0 {
            2
        } else {
            5
        }) as usize;
        let _ = k;
        // The corner triangle: between the octagon's diagonal edge and the square corner.
        let cut = RING_HALF * (PI / 8.0).tan();
        let p_a = Vec2::new(sx * RING_HALF, sz * cut);
        let p_b = Vec2::new(sx * cut, sz * RING_HALF);
        let curb_w = 0.22;
        let inner_corner = corner - Vec2::new(sx, sz) * curb_w;
        // Gravel tray inside the curbs.
        let tri_a = Vec2::new(sx * (RING_HALF - curb_w), sz * (cut + 0.08));
        let tri_b = Vec2::new(sx * (cut + 0.08), sz * (RING_HALF - curb_w));
        let diag = Vec2::new(sx, sz).normalize();
        let tray = [tri_a, inner_corner, tri_b];
        let c = (tray[0] + tray[1] + tray[2]) / 3.0;
        let mut s = flat_fan(&tray, c, RING_TOP - 0.008, 6, move |p| {
            // Furrows parallel to the diagonal edge.
            let along = Vec2::new(-diag.y, diag.x);
            Vec2::new(p.dot(along), p.dot(diag))
        });
        s.shade(|p, _| {
            0.88 + 0.12
                * smoothstep(
                    0.0,
                    0.6,
                    (Vec2::new(p.x, p.z) - Vec2::new(p.x, p.z).normalize() * 0.0).length() * 0.0
                        + 0.6,
                )
        });
        sand.append(&s, Mat4::IDENTITY);
        // Curbs along the two outer edges.
        let e1 = [
            p_a,
            corner,
            corner - Vec2::new(sx * curb_w, 0.0),
            p_a - Vec2::new(sx * curb_w, 0.0),
        ];
        let e2 = [
            corner - Vec2::new(0.0, sz * curb_w),
            corner,
            p_b,
            p_b - Vec2::new(0.0, sz * curb_w),
        ];
        for e in [e1, e2] {
            // Flush with the ring's top (the collider): a fighter can stand on it.
            curb.append(&prism(&e, -0.25, RING_TOP + 0.002, 0.012), Mat4::IDENTITY);
        }
        // Low curb on the diagonal side too (between the tray and the band).
        let d1 = [p_a, tri_a, tri_b, p_b];
        curb.append(&prism(&d1, -0.25, RING_TOP, 0.01), Mat4::IDENTITY);
    }
    kit.add(SAND, sand);
    curb.shade(|p, _| 0.85 + 0.15 * pn(p.x, p.z, 504));
    kit.add(STONE_DARK, curb);

    // The inlay ring: a band of dark carved stone with glowing grooves, flush with the
    // floor (a hair above it).
    let mut inlay = M::new();
    let seg = 192;
    let (r0, r1) = INLAY_R;
    let circ = TAU * (r0 + r1) * 0.5;
    let tiles = (circ / INLAY_TILE).round();
    for i in 0..=seg {
        let a = i as f32 / seg as f32 * TAU;
        let d = v3(a.cos(), 0.0, -a.sin());
        let u = i as f32 / seg as f32 * tiles * INLAY_TILE;
        inlay.vertex(
            d * r0 + Vec3::Y * (RING_TOP + 0.0015),
            Vec3::Y,
            Vec2::new(u, 0.0),
        );
        inlay.vertex(
            d * r1 + Vec3::Y * (RING_TOP + 0.0015),
            Vec3::Y,
            Vec2::new(u, 1.0),
        );
    }
    for i in 0..seg as u32 {
        let a = i * 2;
        inlay.quad(a, a + 2, a + 3, a + 1);
    }
    // Ensure it faces up.
    {
        let t = &inlay.indices[0..3];
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(inlay.vertices[i as usize].position));
        if (b - a).cross(c - a).y < 0.0 {
            inlay.flip();
            for v in &mut inlay.vertices {
                v.normal = [0.0, 1.0, 0.0];
            }
        }
    }
    kit.add(INLAY, inlay);

    // Brass rims along the inlay, a brass crest in a dark stone disc at the center, start
    // lines.
    let mut brass = M::new();
    for r in [r0 - 0.012, r1 + 0.012] {
        let ring: Vec<Vec3> = (0..=160)
            .map(|i| {
                let a = i as f32 / 160.0 * TAU;
                v3(a.cos() * r, RING_TOP - 0.002, a.sin() * r)
            })
            .collect();
        brass.append(&M::tube(&ring, 0.009, 6), Mat4::IDENTITY);
    }
    for z in [-3.7f32, 3.7] {
        brass.append(
            &bx(
                v3(-0.45, RING_TOP - 0.01, z - 0.025),
                v3(0.45, RING_TOP + 0.002, z + 0.025),
            ),
            Mat4::IDENTITY,
        );
    }
    kit.add(BRASS, brass);
    let disc = flat_fan(
        &(0..32)
            .map(|k| {
                let a = k as f32 / 32.0 * TAU;
                Vec2::new(a.cos(), a.sin()) * 0.95
            })
            .collect::<Vec<_>>(),
        Vec2::ZERO,
        RING_TOP + 0.001,
        3,
        |p| p,
    );
    kit.add(STONE_DARK, disc);
    let mut crest = M::new();
    decal(
        &mut crest,
        v3(0.0, RING_TOP + 0.0025, 0.0),
        Vec3::X,
        Vec3::NEG_Z,
        Vec2::splat(1.6),
        Vec2::ZERO,
        Vec2::ONE,
        Vec4::ONE,
    );
    kit.add(CREST_BRASS, crest);
    kit
}

/// The raked-gravel apron around the plinth with a stone kerb.
fn build_apron() -> Kit {
    let mut kit = Kit::default();
    let mut sand = M::new();
    let (h0, h1) = (RING_HALF, RING_HALF + APRON);
    for k in 0..4 {
        let q = Quat::from_rotation_y(k as f32 * FRAC_PI_2);
        // One trapezoid (mitered at the corners), in the +X side frame.
        let quad = [
            Vec2::new(h0, -h0),
            Vec2::new(h1, -h1),
            Vec2::new(h1, h1),
            Vec2::new(h0, h0),
        ];
        let c = Vec2::new((h0 + h1) * 0.5, 0.0);
        let s = flat_fan(&quad, c, 0.012, 8, |p| Vec2::new(p.y, p.x));
        sand.append(&s, Mat4::from_quat(q));
    }
    sand.shade(|p, _| {
        let d = p.x.abs().max(p.z.abs()) - RING_HALF;
        (0.8 + 0.2 * smoothstep(0.0, 0.5, d)) * (0.94 + 0.06 * pn(p.x * 0.7, p.z * 0.7, 520))
    });
    kit.add(SAND, sand);
    let mut kerb = M::new();
    let (k0, k1) = (h1, h1 + 0.16);
    for k in 0..4 {
        let q = Quat::from_rotation_y(k as f32 * FRAC_PI_2);
        let n = 12;
        for i in 0..n {
            let za = -k1 + (2.0 * k1) * i as f32 / n as f32;
            let zb = -k1 + (2.0 * k1) * (i + 1) as f32 / n as f32;
            let s = slab(
                v3(k0, -0.15, za + 0.004),
                v3(k1, 0.07, zb - 0.004),
                0.018,
                100.0,
            );
            kerb.append(&s, Mat4::from_quat(q));
        }
    }
    kerb.shade(|p, _| 0.85 + 0.15 * pn(p.x * 2.0, p.z * 2.0, 521));
    kerb.planar_uvs(1.0);
    kit.add(STONE, kerb);
    kit
}

/// The torii gate on the course side of the ring.
fn build_torii() -> Kit {
    let mut kit = Kit::default();
    let z = RING_HALF + APRON + 0.85;
    let px = 3.3;
    let mut red = M::new();
    let mut black = M::new();
    let mut stone = M::new();
    let mut brass = M::new();
    for sx in [-1.0f32, 1.0] {
        let x = sx * px;
        stone.append(
            &M::lathe(
                &[
                    Vec2::new(0.48, -0.2),
                    Vec2::new(0.48, 0.18),
                    Vec2::new(0.4, 0.3),
                    Vec2::new(0.32, 0.32),
                ],
                8,
            )
            .flat_shaded(),
            at(x, 0.0, z),
        );
        black.append(
            &M::lathe(
                &[
                    Vec2::new(0.3, 0.0),
                    Vec2::new(0.3, 0.6),
                    Vec2::new(0.285, 0.64),
                    Vec2::new(0.255, 0.66),
                ],
                28,
            ),
            at(x, 0.3, z),
        );
        // Pillar: slightly tapered.
        let mut pillar = M::lathe(&[Vec2::new(0.255, 0.0), Vec2::new(0.235, 5.25)], 28);
        for v in &mut pillar.vertices {
            v.uv = [v.uv[0] * 0.25, v.uv[1]];
        }
        red.append(&pillar, at(x, 0.95, z));
        // Brass bands on the sleeve.
        for y in [0.36, 0.86] {
            brass.append(
                &M::lathe(&[Vec2::new(0.305, 0.0), Vec2::new(0.305, 0.035)], 28),
                at(x, y, z),
            );
        }
    }
    // Nuki: through beam with wedges.
    let ny = 4.55;
    red.append(
        &slab(
            v3(-px - 0.85, ny - 0.17, z - 0.11),
            v3(px + 0.85, ny + 0.17, z + 0.11),
            0.012,
            0.5,
        ),
        Mat4::IDENTITY,
    );
    for sx in [-1.0f32, 1.0] {
        black.append(
            &bx(
                v3(sx * px - 0.06, ny - 0.17, z + 0.11),
                v3(sx * px + 0.06, ny + 0.17, z + 0.16),
            ),
            Mat4::IDENTITY,
        );
        black.append(
            &bx(
                v3(sx * px - 0.06, ny - 0.17, z - 0.16),
                v3(sx * px + 0.06, ny + 0.17, z - 0.11),
            ),
            Mat4::IDENTITY,
        );
    }
    // Gakuzuka strut and the plaque.
    red.append(
        &bx(v3(-0.13, ny + 0.17, z - 0.09), v3(0.13, 5.62, z + 0.09)),
        Mat4::IDENTITY,
    );
    black.append(
        &slab(
            v3(-0.38, 4.78, z - 0.17),
            v3(0.38, 5.48, z + 0.17),
            0.03,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    brass.append(
        &bx(v3(-0.33, 4.83, z - 0.175), v3(0.33, 5.43, z - 0.168)),
        Mat4::IDENTITY,
    );
    brass.append(
        &bx(v3(-0.33, 4.83, z + 0.168), v3(0.33, 5.43, z + 0.175)),
        Mat4::IDENTITY,
    );
    black.append(
        &bx(v3(-0.3, 4.86, z - 0.178), v3(0.3, 5.4, z + 0.178)),
        Mat4::IDENTITY,
    );
    let mut crest = M::new();
    for (side, right) in [(-1.0f32, Vec3::X), (1.0, Vec3::NEG_X)] {
        decal(
            &mut crest,
            v3(0.0, 5.13, z + side * 0.181),
            right,
            Vec3::Y,
            Vec2::splat(0.46),
            Vec2::ZERO,
            Vec2::ONE,
            Vec4::ONE,
        );
    }
    kit.add(CREST_BRASS, crest);
    // Shimaki (red) and kasagi (black) with upswept ends.
    let beam = |half_len: f32, y: f32, rise: f32, half: Vec2| {
        let n = 24;
        let sections: Vec<Section> = (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32 * 2.0 - 1.0;
                let x = t * half_len;
                let lift = rise * t.abs().powf(2.6);
                Section::rounded(v3(x, y + lift, z), half)
                    .with_exponent(10.0)
                    .with_right(Vec3::Z)
            })
            .collect();
        M::loft(&sections, 16, Cap::Flat, Cap::Flat)
    };
    red.append(
        &beam(4.9, 5.75, 0.18, Vec2::new(0.17, 0.13)),
        Mat4::IDENTITY,
    );
    black.append(
        &beam(5.35, 6.08, 0.36, Vec2::new(0.24, 0.2)),
        Mat4::IDENTITY,
    );
    // Copper-green cap strip on the kasagi.
    kit.add(LACQUER, red);
    kit.add(LACQUER_BLACK, black);
    stone.shade(|p, _| 0.8 + 0.2 * smoothstep(-0.1, 0.3, p.y));
    kit.add(STONE, stone);
    kit.add(BRASS, brass);
    kit
}

/// Where the paper lanterns hang under the stand's eaves (x), height and the front line.
const STAND_LANTERNS_X: [f32; 2] = [-4.2, 4.2];
const STAND_LANTERN_Y: f32 = 2.25;
const STAND_FRONT: f32 = -9.85;

/// The stand behind the ring: a timber deck on a stone base, a tiled gable roof on posts,
/// a crested curtain behind the waiting fighters and paper lanterns.
fn build_stands() -> Kit {
    let mut kit = Kit::default();
    let (s0, s1) = bounds(layout::STANDS);
    let ring = DVec3::from_array(layout::RING).as_vec3();
    let (s0, s1) = (s0 - ring, s1 - ring);
    let top = s1.y;
    let mut wood = M::new();
    let mut planks = M::new();
    let mut stone = M::new();
    let mut roof = M::new();
    let mut cloth = M::new();
    let mut black = M::new();
    let mut brass = M::new();
    // Stone base and the deck (boards along X, exactly the collider's top).
    stone.append(
        &slab(v3(s0.x, -0.25, s0.z), v3(s1.x, top - 0.12, s1.z), 0.02, 0.6),
        Mat4::IDENTITY,
    );
    let mut deck = slab(
        v3(s0.x + 0.01, top - 0.12, s0.z + 0.01),
        v3(s1.x - 0.01, top, s1.z - 0.01),
        0.008,
        0.6,
    );
    deck.shade(|p, n| {
        if n.y > 0.5 {
            0.92 + 0.08 * pn(p.x * 0.5, p.z, 530)
        } else {
            0.7
        }
    });
    planks.append(&deck, Mat4::IDENTITY);
    // Fascia beam along the front.
    wood.append(
        &slab(
            v3(s0.x, top - 0.16, s1.z - 0.02),
            v3(s1.x, top - 0.02, s1.z + 0.06),
            0.01,
            1.0,
        ),
        Mat4::IDENTITY,
    );
    // Posts, beams, roof.
    let (zf, zb) = (s1.z + 0.12, s0.z - 0.14);
    let xs_front = [s0.x - 0.15, s1.x + 0.15];
    let xs_back = [s0.x - 0.15, -2.7, 2.7, s1.x + 0.15];
    let eave = 2.75;
    let ridge = 3.85;
    let post = |m: &mut M, x: f32, z: f32| {
        m.append(
            &slab(
                v3(x - 0.1, top - 0.12, z - 0.1),
                v3(x + 0.1, eave + 0.05, z + 0.1),
                0.012,
                0.6,
            ),
            Mat4::IDENTITY,
        );
    };
    for x in xs_front {
        post(&mut wood, x, zf);
        stone.append(
            &M::lathe(
                &[
                    Vec2::new(0.2, -0.1),
                    Vec2::new(0.2, 0.12),
                    Vec2::new(0.15, 0.2),
                ],
                8,
            )
            .flat_shaded(),
            at(x, 0.0, zf),
        );
    }
    for x in xs_back {
        post(&mut wood, x, zb);
    }
    for (z, xs) in [(zf, xs_front.to_vec()), (zb, xs_back.to_vec())] {
        let (xa, xb) = (xs[0] - 0.3, xs[xs.len() - 1] + 0.3);
        wood.append(
            &slab(
                v3(xa, eave + 0.05, z - 0.12),
                v3(xb, eave + 0.35, z + 0.12),
                0.015,
                1.0,
            ),
            Mat4::IDENTITY,
        );
    }
    // Tie beams across, ridge beam, rafters.
    let zm = (zf + zb) * 0.5;
    let half_depth = (zf - zb) * 0.5 + 0.75;
    for x in [s0.x - 0.15, -2.7, 2.7, s1.x + 0.15] {
        wood.append(
            &slab(
                v3(x - 0.08, eave + 0.35, zb - 0.1),
                v3(x + 0.08, eave + 0.55, zf + 0.1),
                0.012,
                1.0,
            ),
            Mat4::IDENTITY,
        );
        wood.append(
            &slab(
                v3(x - 0.06, eave + 0.55, zm - 0.06),
                v3(x + 0.06, ridge - 0.12, zm + 0.06),
                0.01,
                1.0,
            ),
            Mat4::IDENTITY,
        );
    }
    wood.append(
        &slab(
            v3(s0.x - 0.9, ridge - 0.22, zm - 0.09),
            v3(s1.x + 0.9, ridge - 0.04, zm + 0.09),
            0.012,
            1.0,
        ),
        Mat4::IDENTITY,
    );
    let slope_len = (half_depth * half_depth + (ridge - eave - 0.25).powi(2)).sqrt();
    let pitch = ((ridge - eave - 0.25) / half_depth).atan();
    let mut x = s0.x - 0.8;
    while x <= s1.x + 0.81 {
        for side in [-1.0f32, 1.0] {
            let q = Quat::from_rotation_x(side * pitch);
            let c = v3(
                x,
                (ridge + eave + 0.25) * 0.5 - 0.1,
                zm + side * half_depth * 0.5,
            );
            wood.append(
                &rbox(c, v3(0.07, 0.09, slope_len), q, 0.008),
                Mat4::IDENTITY,
            );
        }
        x += 0.8;
    }
    // Roof: boards and rows of round tiles running down each slope, a ridge cap.
    let roof_x = (s0.x - 1.0, s1.x + 1.0);
    for side in [-1.0f32, 1.0] {
        let q = Quat::from_rotation_x(side * pitch);
        let c = v3(
            0.0,
            (ridge + eave + 0.25) * 0.5 - 0.01,
            zm + side * half_depth * 0.5,
        );
        let n = q * Vec3::Y;
        roof.append(
            &rbox(c, v3(roof_x.1 - roof_x.0, 0.06, slope_len + 0.1), q, 0.01),
            Mat4::IDENTITY,
        );
        let pitch_x = 0.27;
        let mut x = roof_x.0 + 0.12;
        while x < roof_x.1 - 0.1 {
            let a = c + v3(x, 0.0, 0.0) + n * 0.03 + q * v3(0.0, 0.0, -slope_len * 0.5);
            let b = c + v3(x, 0.0, 0.0) + n * 0.03 + q * v3(0.0, 0.0, slope_len * 0.5);
            let mut tile = M::loft(
                &[
                    Section::ellipse(a, Vec2::new(0.085, 0.06)).with_right(Vec3::X),
                    Section::ellipse(b, Vec2::new(0.085, 0.06)).with_right(Vec3::X),
                ],
                10,
                Cap::Flat,
                Cap::Flat,
            );
            tile.shade(|_, _| 1.0);
            roof.append(&tile, Mat4::IDENTITY);
            x += pitch_x;
        }
    }
    roof.append(
        &M::loft(
            &[
                Section::rounded(v3(roof_x.0 - 0.1, ridge + 0.08, zm), Vec2::new(0.16, 0.2))
                    .with_right(Vec3::Z),
                Section::rounded(v3(roof_x.1 + 0.1, ridge + 0.08, zm), Vec2::new(0.16, 0.2))
                    .with_right(Vec3::Z),
            ],
            12,
            Cap::Flat,
            Cap::Flat,
        ),
        Mat4::IDENTITY,
    );
    for x in [roof_x.0 - 0.15, roof_x.1 + 0.15] {
        roof.append(
            &slab(
                v3(x - 0.08, ridge - 0.05, zm - 0.3),
                v3(x + 0.08, ridge + 0.5, zm + 0.3),
                0.04,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    roof.shade(|p, n| (0.85 + 0.15 * pn(p.x * 0.4, p.z, 531)) * if n.y < 0.0 { 0.6 } else { 1.0 });
    // Curtain behind the fighters (jinmaku), gently rippled, with crests.
    let (cx0, cx1) = (s0.x - 0.05, s1.x + 0.05);
    let (cy0, cy1) = (top + 0.05, 2.55);
    let cz = zb + 0.13;
    let (nx, ny) = (96, 10);
    let ripple = |x: f32, y: f32| {
        0.05 * (x * 2.2).sin() * (0.4 + 0.6 * (cy1 - y) / (cy1 - cy0))
            + 0.02 * (x * 5.3 + 1.0).sin()
    };
    let mut curtain = M::new();
    for j in 0..=ny {
        for i in 0..=nx {
            let x = cx0 + (cx1 - cx0) * i as f32 / nx as f32;
            let y = cy0 + (cy1 - cy0) * j as f32 / ny as f32;
            curtain.vertex(v3(x, y, cz + ripple(x, y)), Vec3::Z, Vec2::new(x, y));
        }
    }
    let row = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = (j * row + i) as u32;
            curtain.quad(a, a + 1, a + 1 + row as u32, a + row as u32);
        }
    }
    curtain.smooth_normals();
    curtain.shade(|p, _| 0.75 + 0.25 * smoothstep(cy0, cy0 + 0.8, p.y));
    cloth.append(&curtain, Mat4::IDENTITY);
    black.append(
        &rod(
            v3(cx0 - 0.3, cy1 + 0.04, cz),
            v3(cx1 + 0.3, cy1 + 0.04, cz),
            0.035,
            10,
        ),
        Mat4::IDENTITY,
    );
    // Dark bands at the top and bottom of the curtain.
    let mut band = M::new();
    for (ya, yb) in [(cy1 - 0.42, cy1 - 0.12), (cy0 + 0.15, cy0 + 0.4)] {
        for i in 0..nx {
            let xa = cx0 + (cx1 - cx0) * i as f32 / nx as f32;
            let xb = cx0 + (cx1 - cx0) * (i + 1) as f32 / nx as f32;
            let za = cz + ripple(xa, (ya + yb) * 0.5) + 0.004;
            let zb2 = cz + ripple(xb, (ya + yb) * 0.5) + 0.004;
            band.flat_quad(
                [
                    v3(xa, ya, za),
                    v3(xb, ya, zb2),
                    v3(xb, yb, zb2),
                    v3(xa, yb, za),
                ],
                [
                    Vec2::new(xa, ya),
                    Vec2::new(xb, ya),
                    Vec2::new(xb, yb),
                    Vec2::new(xa, yb),
                ],
            );
        }
    }
    // Flip so the band faces +Z (toward the ring).
    kit.add(CLOTH_INDIGO, band);
    let mut crest = M::new();
    for k in 0..5 {
        let x = -6.4 + k as f32 * 3.2;
        let y = 1.42;
        let z = cz + ripple(x, y) + 0.006;
        decal(
            &mut crest,
            v3(x, y, z),
            Vec3::X,
            Vec3::Y,
            Vec2::splat(0.95),
            Vec2::ZERO,
            Vec2::ONE,
            Vec4::ONE,
        );
    }
    kit.add(CREST_INDIGO, crest);
    // Paper lanterns (chochin) hanging under the front beam.
    let mut paper = M::new();
    for x in STAND_LANTERNS_X {
        let c = v3(x, STAND_LANTERN_Y, STAND_FRONT + 0.05);
        let mut prof = Vec::new();
        for k in 0..=12 {
            let t = k as f32 / 12.0;
            let a = (t - 0.5) * PI * 0.92;
            let rib = if k % 2 == 0 { 1.0 } else { 0.97 };
            prof.push(Vec2::new(0.2 * a.cos() * rib + 0.02, -0.3 + 0.6 * t));
        }
        paper.append(&M::lathe(&prof, 20), Mat4::from_translation(c));
        for (y, h) in [(-0.33, 0.05), (0.28, 0.05)] {
            black.append(
                &M::lathe(&[Vec2::new(0.1, 0.0), Vec2::new(0.1, h)], 16),
                Mat4::from_translation(c + v3(0.0, y, 0.0)),
            );
        }
        black.append(
            &rod(c + v3(0.0, 0.33, 0.0), v3(x, eave + 0.05, c.z), 0.008, 5),
            Mat4::IDENTITY,
        );
        brass.append(
            &M::lathe(&[Vec2::new(0.02, 0.0), Vec2::new(0.02, 0.04)], 8),
            Mat4::from_translation(c + v3(0.0, 0.33, 0.0)),
        );
    }
    kit.add(WOOD_DARK, wood);
    kit.add(PLANKS, planks);
    stone.shade(|p, _| 0.8 + 0.2 * smoothstep(-0.1, 0.2, p.y));
    kit.add(STONE, stone);
    kit.add(KAWARA, roof);
    kit.add(CLOTH, cloth);
    kit.add(LACQUER_BLACK, black);
    kit.add(PAPER_LIT, paper);
    kit.add(BRASS, brass);
    kit
}

/// A taiko drum on a stand, on the −X side of the ring, facing it.
fn build_drum() -> Kit {
    let mut kit = Kit::default();
    let c = v3(-RING_HALF - APRON - 1.2, 1.15, 0.0);
    let mut wood = M::new();
    let mut body = M::new();
    let mut skin = M::new();
    let mut brass = M::new();
    let mut stone = M::new();
    // Barrel along X.
    let to_x = Quat::from_rotation_z(-FRAC_PI_2);
    let half = 0.42;
    let prof: Vec<Vec2> = (0..=12)
        .map(|k| {
            let t = k as f32 / 12.0;
            let y = -half + 2.0 * half * t;
            Vec2::new(0.5 + 0.08 * (1.0 - (2.0 * t - 1.0).powi(2)), y)
        })
        .collect();
    body.append(&M::lathe(&prof, 36), rt(to_x, c));
    for sx in [-1.0f32, 1.0] {
        let disc: Vec<Vec2> = (0..36)
            .map(|k| {
                let a = k as f32 / 36.0 * TAU;
                Vec2::new(a.cos(), a.sin()) * 0.5
            })
            .collect();
        let mut d = flat_fan(&disc, Vec2::ZERO, 0.0, 4, |p| p);
        if sx < 0.0 {
            d.flip();
        }
        let q = Quat::from_rotation_z(-sx * FRAC_PI_2);
        skin.append(&d, rt(q, c + v3(sx * (half + 0.004), 0.0, 0.0)));
        // Tacks around the rim.
        for k in 0..28 {
            let a = k as f32 / 28.0 * TAU;
            let p = c + v3(sx * (half - 0.03), a.sin() * 0.53, a.cos() * 0.53);
            brass.append(
                &M::lathe(
                    &[
                        Vec2::new(0.014, 0.0),
                        Vec2::new(0.012, 0.008),
                        Vec2::new(0.0001, 0.012),
                    ],
                    6,
                ),
                rt(
                    Quat::from_rotation_arc(Vec3::Y, v3(0.0, a.sin(), a.cos())),
                    p,
                ),
            );
        }
        // Carry rings.
    }
    for side in [-1.0f32, 1.0] {
        brass.append(
            &M::lathe(&[Vec2::new(0.035, 0.0), Vec2::new(0.035, 0.02)], 12),
            rt(
                Quat::from_rotation_x(FRAC_PI_2),
                c + v3(0.0, 0.0, side * 0.57),
            ),
        );
    }
    // Stand: two A-frames joined by rails, a cradle.
    for x in [c.x - 0.3, c.x + 0.3] {
        for sz in [-1.0f32, 1.0] {
            wood.append(
                &pipe(&[v3(x, 0.0, sz * 0.75), v3(x, 0.75, sz * 0.42)], 0.05, 8),
                Mat4::IDENTITY,
            );
        }
        wood.append(
            &slab(
                v3(x - 0.05, 0.65, -0.55),
                v3(x + 0.05, 0.75, 0.55),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
        wood.append(
            &slab(
                v3(x - 0.045, 0.1, -0.7),
                v3(x + 0.045, 0.18, 0.7),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    for sz in [-0.7f32, 0.7] {
        wood.append(
            &slab(
                v3(c.x - 0.35, 0.1, sz - 0.04),
                v3(c.x + 0.35, 0.18, sz + 0.04),
                0.01,
                100.0,
            ),
            Mat4::IDENTITY,
        );
    }
    // Sticks resting on the stand.
    wood.append(
        &rod(
            v3(c.x - 0.25, 0.79, 0.45),
            v3(c.x + 0.2, 0.8, 0.62),
            0.018,
            8,
        ),
        Mat4::IDENTITY,
    );
    wood.append(
        &rod(
            v3(c.x - 0.22, 0.81, 0.5),
            v3(c.x + 0.25, 0.82, 0.66),
            0.018,
            8,
        ),
        Mat4::IDENTITY,
    );
    stone.append(
        &slab(
            v3(c.x - 0.7, -0.1, -0.95),
            v3(c.x + 0.7, 0.03, 0.95),
            0.02,
            0.5,
        ),
        Mat4::IDENTITY,
    );
    kit.add(WOOD_DARK, wood);
    kit.add(LACQUER, body);
    kit.add(CLOTH, skin);
    kit.add(BRASS, brass);
    kit.add(STONE, stone);
    kit
}

/// Stepping stones from the torii toward the course.
fn build_path() -> Kit {
    let mut kit = Kit::default();
    let mut stone = M::new();
    let start = RING_HALF + APRON + 1.9;
    let end = 20.3;
    let mut z = start;
    let mut k = 0;
    while z < end {
        let w = 0.28 + 0.06 * rand2(k, 0, 540);
        let x = if k % 2 == 0 { -0.22 } else { 0.22 } + 0.08 * (rand2(k, 1, 540) - 0.5);
        let n = 7;
        let outline: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * TAU + rand2(k, 2, 540);
                let r = w * (0.85 + 0.25 * rand2(k, i + 10, 540));
                Vec2::new(x + a.cos() * r * 1.15, z + a.sin() * r)
            })
            .collect();
        stone.append(&prism(&outline, -0.15, 0.035, 0.02), Mat4::IDENTITY);
        z += 0.78;
        k += 1;
    }
    stone.shade(|p, _| 0.82 + 0.18 * pn(p.x * 3.0, p.z * 3.0, 541));
    kit.add(STONE, stone);
    kit
}

/// A kasuga-style stone lantern (about 2.2 m), standing on y = 0 (local).
fn build_lantern() -> Kit {
    let mut kit = Kit::default();
    let mut stone = M::new();
    let hexl = |profile: &[Vec2]| {
        let mut m = M::lathe(profile, 6).flat_shaded();
        m.planar_uvs(1.0);
        m
    };
    let round = |profile: &[Vec2]| M::lathe(profile, 20);
    // Base (kiso) with a lotus step.
    stone.append(
        &hexl(&[
            Vec2::new(0.46, -0.1),
            Vec2::new(0.46, 0.12),
            Vec2::new(0.4, 0.16),
            Vec2::new(0.3, 0.2),
            Vec2::new(0.16, 0.24),
        ]),
        Mat4::IDENTITY,
    );
    // Pillar (sao) with a ring.
    stone.append(
        &round(&[
            Vec2::new(0.12, 0.22),
            Vec2::new(0.11, 0.62),
            Vec2::new(0.13, 0.64),
            Vec2::new(0.13, 0.7),
            Vec2::new(0.11, 0.72),
            Vec2::new(0.1, 1.08),
        ]),
        Mat4::IDENTITY,
    );
    // Middle platform (chudai).
    stone.append(
        &hexl(&[
            Vec2::new(0.11, 1.06),
            Vec2::new(0.2, 1.12),
            Vec2::new(0.33, 1.2),
            Vec2::new(0.34, 1.27),
            Vec2::new(0.28, 1.3),
        ]),
        Mat4::IDENTITY,
    );
    // Fire box (hibukuro): six corner posts between plates; paper windows and a flame inside.
    let fb = (1.3, 1.74);
    let rr = 0.25;
    let mut paper = M::new();
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        let p = v3(a.cos() * rr, 0.0, a.sin() * rr);
        stone.append(
            &rbox(
                p + v3(0.0, (fb.0 + fb.1) * 0.5, 0.0),
                v3(0.07, fb.1 - fb.0, 0.07),
                Quat::from_rotation_y(-a),
                0.01,
            ),
            Mat4::IDENTITY,
        );
        // Windows on alternate faces: paper; the others carved stone panels with a round hole.
        let a_mid = a + TAU / 12.0;
        let n = v3(a_mid.cos(), 0.0, a_mid.sin());
        let face_c = n * (rr * (PI / 6.0).cos() - 0.01) + v3(0.0, (fb.0 + fb.1) * 0.5, 0.0);
        let right = Vec3::Y.cross(n).normalize();
        if k % 2 == 0 {
            let mut q = M::new();
            q.flat_quad(
                [
                    face_c - right * 0.11 - Vec3::Y * 0.17,
                    face_c + right * 0.11 - Vec3::Y * 0.17,
                    face_c + right * 0.11 + Vec3::Y * 0.17,
                    face_c - right * 0.11 + Vec3::Y * 0.17,
                ],
                [
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.22, 0.0),
                    Vec2::new(0.22, 0.34),
                    Vec2::new(0.0, 0.34),
                ],
            );
            // Facing outward: right × up = n.
            paper.append(&q, Mat4::IDENTITY);
            // Lattice bars over the paper.
            for t in [-0.04f32, 0.04] {
                stone.append(
                    &rbox(
                        face_c + right * t + n * 0.008,
                        v3(0.012, 0.34, 0.012),
                        Quat::from_rotation_y(-a_mid),
                        0.0,
                    ),
                    Mat4::IDENTITY,
                );
            }
            stone.append(
                &rbox(
                    face_c + n * 0.008,
                    v3(0.22, 0.012, 0.012),
                    Quat::from_rotation_y(-a_mid + FRAC_PI_2) * Quat::from_rotation_y(FRAC_PI_2),
                    0.0,
                ),
                Mat4::IDENTITY,
            );
        } else {
            // Stone panel with a moon window: an outer frame of four pieces.
            let q = Quat::from_rotation_y(FRAC_PI_2 - a_mid);
            for (dx, dy, sx, sy) in [
                (0.0, 0.15, 0.24, 0.05),
                (0.0, -0.15, 0.24, 0.05),
                (0.09, 0.0, 0.05, 0.26),
                (-0.09, 0.0, 0.05, 0.26),
            ] {
                stone.append(
                    &rbox(
                        face_c + right * dx + Vec3::Y * dy,
                        v3(sx, sy, 0.04),
                        q,
                        0.006,
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
    }
    stone.append(
        &hexl(&[
            Vec2::new(0.3, fb.1),
            Vec2::new(0.3, fb.1 + 0.04),
            Vec2::new(0.24, fb.1 + 0.05),
        ]),
        Mat4::IDENTITY,
    );
    // Roof (kasa) with upturned corners.
    let ry = fb.1 + 0.04;
    stone.append(
        &hexl(&[
            Vec2::new(0.24, ry),
            Vec2::new(0.5, ry + 0.03),
            Vec2::new(0.52, ry + 0.07),
            Vec2::new(0.42, ry + 0.11),
            Vec2::new(0.2, ry + 0.22),
            Vec2::new(0.09, ry + 0.27),
        ]),
        Mat4::IDENTITY,
    );
    for k in 0..6 {
        let a = k as f32 / 6.0 * TAU;
        let d = v3(a.cos(), 0.0, a.sin());
        stone.append(
            &pipe(
                &[
                    d * 0.47 + Vec3::Y * (ry + 0.07),
                    d * 0.56 + Vec3::Y * (ry + 0.1),
                    d * 0.58 + Vec3::Y * (ry + 0.16),
                    d * 0.55 + Vec3::Y * (ry + 0.19),
                ],
                0.028,
                8,
            ),
            Mat4::IDENTITY,
        );
    }
    // Jewel (hoju) on a lotus.
    stone.append(
        &round(&[
            Vec2::new(0.1, ry + 0.26),
            Vec2::new(0.12, ry + 0.31),
            Vec2::new(0.07, ry + 0.34),
            Vec2::new(0.1, ry + 0.4),
            Vec2::new(0.09, ry + 0.46),
            Vec2::new(0.04, ry + 0.52),
            Vec2::new(0.0001, ry + 0.55),
        ]),
        Mat4::IDENTITY,
    );
    // Weathering: moss on up-facing surfaces, dark toward the ground.
    stone.shade(|p, n| {
        let ground = 0.7 + 0.3 * smoothstep(0.0, 0.5, p.y);
        let up = n.y.max(0.0);
        ground * (1.0 - 0.18 * up * (0.5 + 0.5 * pn(p.x * 8.0, p.z * 8.0 + p.y * 3.0, 550)))
    });
    for v in &mut stone.vertices {
        let up = v.normal[1].max(0.0);
        let moss = up
            * smoothstep(
                -0.2,
                0.6,
                pn(
                    v.position[0] * 6.0,
                    v.position[2] * 6.0 + v.position[1] * 4.0,
                    551,
                ),
            );
        v.color[0] *= 1.0 - 0.25 * moss;
        v.color[2] *= 1.0 - 0.35 * moss;
    }
    kit.add(STONE, stone);
    kit.add(PAPER_LIT, paper);
    // The flame glow inside.
    let flame = M::lathe(
        &[
            Vec2::new(0.05, 0.0),
            Vec2::new(0.07, 0.08),
            Vec2::new(0.04, 0.17),
            Vec2::new(0.0001, 0.24),
        ],
        10,
    );
    let mut g = M::new();
    g.append(&flame, at(0.0, fb.0 + 0.05, 0.0));
    kit.add(GLOW_WARM, g);
    kit
}

/// Concentric raked circles in the gravel around a lantern.
fn build_lantern_rake() -> Kit {
    let mut kit = Kit::default();
    let (r0, r1) = (0.5, 1.15);
    let seg = 48;
    let rings = 6;
    let mut m = M::new();
    for j in 0..=rings {
        let r = r0 + (r1 - r0) * j as f32 / rings as f32;
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * TAU;
            let p = v3(a.cos() * r, 0.016, -a.sin() * r);
            m.vertex(p, Vec3::Y, Vec2::new(a * 0.8, r));
        }
    }
    let row = seg + 1;
    for j in 0..rings {
        for i in 0..seg {
            let a = (j * row + i) as u32;
            m.quad(a, a + row as u32, a + row as u32 + 1, a + 1);
        }
    }
    let t = &m.indices[0..3];
    let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
    if (b - a).cross(c - a).y < 0.0 {
        for t in m.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    m.shade(|p, _| 0.85 + 0.15 * smoothstep(r0, r0 + 0.3, Vec2::new(p.x, p.z).length()));
    kit.add(SAND, m);
    kit
}

/// A nobori banner pole with its crossbar and stone foot (local: pole at the origin, cloth
/// toward +X).
fn build_banner() -> Kit {
    let mut kit = Kit::default();
    let mut black = M::new();
    let mut stone = M::new();
    let mut brass = M::new();
    black.append(
        &M::lathe(&[Vec2::new(0.04, 0.0), Vec2::new(0.035, 5.4)], 12),
        at(0.0, 0.2, 0.0),
    );
    black.append(
        &rod(
            v3(-0.02, BANNER_TOP + 0.03, 0.0),
            v3(BANNER_W + 0.08, BANNER_TOP + 0.03, 0.0),
            0.02,
            8,
        ),
        Mat4::IDENTITY,
    );
    brass.append(
        &M::lathe(
            &[
                Vec2::new(0.045, 0.0),
                Vec2::new(0.045, 0.05),
                Vec2::new(0.02, 0.12),
                Vec2::new(0.0001, 0.2),
            ],
            12,
        ),
        at(0.0, 5.6, 0.0),
    );
    stone.append(
        &slab(v3(-0.25, -0.1, -0.25), v3(0.25, 0.22, 0.25), 0.025, 100.0),
        Mat4::IDENTITY,
    );
    brass.append(
        &M::lathe(&[Vec2::new(0.06, 0.0), Vec2::new(0.06, 0.06)], 12),
        at(0.0, 0.22, 0.0),
    );
    kit.add(LACQUER_BLACK, black);
    kit.add(STONE, stone);
    kit.add(BRASS, brass);
    kit
}

const BANNER_TOP: f32 = 5.0;
const BANNER_W: f32 = 0.72;
const BANNER_H: f32 = 3.5;

/// A banner cloth (and its crest overlay), local to the pole: hanging from the crossbar
/// toward +X, rippling a little.
fn build_banner_cloth(cloth: &'static str) -> (Kit, Kit) {
    let (nx, ny) = (6, 28);
    let x0 = 0.06;
    let disp = |x: f32, y: f32| {
        let t = (x - x0) / BANNER_W;
        0.05 * ((BANNER_TOP - y) * 2.1 + 0.6).sin() * t + 0.02 * ((BANNER_TOP - y) * 5.0).sin() * t
    };
    let grid = |u0: f32, u1: f32, v0: f32, v1: f32, off: f32, uv_norm: bool, flip: bool| {
        let mut m = M::new();
        for j in 0..=ny {
            for i in 0..=nx {
                let s = i as f32 / nx as f32;
                let t = j as f32 / ny as f32;
                let x = x0 + BANNER_W * (u0 + (u1 - u0) * s);
                let y = BANNER_TOP - BANNER_H * (v0 + (v1 - v0) * t);
                let uv = if uv_norm {
                    Vec2::new(if flip { 1.0 - s } else { s }, t)
                } else {
                    Vec2::new(x, y)
                };
                m.vertex(v3(x, y, disp(x, y) + off), Vec3::Z, uv);
            }
        }
        let row = (nx + 1) as u32;
        for j in 0..ny as u32 {
            for i in 0..nx as u32 {
                let a = j * row + i;
                if flip {
                    m.quad(a, a + row, a + row + 1, a + 1);
                } else {
                    m.quad(a, a + 1, a + row + 1, a + row);
                }
            }
        }
        m.smooth_normals();
        m
    };
    let mut c = grid(0.0, 1.0, 0.0, 1.0, 0.0, false, true);
    c.shade(|p, _| {
        0.82 + 0.18 * smoothstep(BANNER_TOP - BANNER_H, BANNER_TOP - BANNER_H + 1.0, p.y)
    });
    let mut cloth_kit = Kit::default();
    cloth_kit.add(cloth, c);
    // Crest in the upper part, on both faces.
    let mut crest = Kit::default();
    let side = 0.6 * BANNER_W;
    let (u0, u1) = ((1.0 - side / BANNER_W) * 0.5, (1.0 + side / BANNER_W) * 0.5);
    let (v0, v1) = (0.12, 0.12 + side / BANNER_H);
    crest.add(CREST_WHITE, grid(u0, u1, v0, v1, 0.004, true, true));
    crest.add(CREST_WHITE, grid(u0, u1, v0, v1, -0.004, true, false));
    (cloth_kit, crest)
}

/// The shrine frame around the level cells (local: ground point under the stack).
fn build_column() -> Kit {
    let mut kit = Kit::default();
    let mut black = M::new();
    let mut brass = M::new();
    let mut stone = M::new();
    let mut roof = M::new();
    let mut rope = M::new();
    let mut paper = M::new();
    let first = layout::LEVEL_METER[1] as f32;
    let step = 0.5;
    let top_cell = first + 9.0 * step + 0.19;
    // Stone base.
    stone.append(
        &slab(v3(-0.5, -0.15, -0.5), v3(0.5, 0.08, 0.5), 0.03, 0.3),
        Mat4::IDENTITY,
    );
    stone.append(
        &slab(v3(-0.38, 0.08, -0.38), v3(0.38, 0.14, 0.38), 0.015, 100.0),
        Mat4::IDENTITY,
    );
    stone.shade(|p, _| 0.8 + 0.2 * smoothstep(-0.1, 0.12, p.y));
    // Corner posts.
    let pc = 0.27;
    let y_top = top_cell + 0.12;
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        black.append(
            &slab(
                v3(sx * pc - 0.04, 0.14, sz * pc - 0.04),
                v3(sx * pc + 0.04, y_top, sz * pc + 0.04),
                0.008,
                0.5,
            ),
            Mat4::IDENTITY,
        );
    }
    // Frames between the cells: square rings of black lacquer with brass corners.
    let ring = |m: &mut M, b: &mut M, y: f32, h: f32| {
        let o = pc + 0.045;
        for (min, max) in [
            (v3(-o, y - h, -o), v3(o, y + h, -o + 0.05)),
            (v3(-o, y - h, o - 0.05), v3(o, y + h, o)),
            (v3(-o, y - h, -o), v3(-o + 0.05, y + h, o)),
            (v3(o - 0.05, y - h, -o), v3(o, y + h, o)),
        ] {
            m.append(&slab(min, max, 0.006, 100.0), Mat4::IDENTITY);
        }
        for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            b.append(
                &slab(
                    v3(sx * o - 0.035, y - h - 0.008, sz * o - 0.035),
                    v3(sx * o + 0.035, y + h + 0.008, sz * o + 0.035),
                    0.006,
                    100.0,
                ),
                Mat4::IDENTITY,
            );
        }
    };
    for k in 0..=10 {
        let y = first - 0.25 + k as f32 * step;
        ring(&mut black, &mut brass, y.max(0.17), 0.028);
    }
    // Kumiko lattice in front of every cell, so a lit cell glows through it like a shoji
    // lantern: two uprights and a rail on each face, between the posts and the frames.
    let lat = 0.212;
    for i in 0..10 {
        let y = first + i as f32 * step;
        for (n, r) in [
            (Vec3::X, Vec3::Z),
            (Vec3::NEG_X, Vec3::Z),
            (Vec3::Z, Vec3::X),
            (Vec3::NEG_Z, Vec3::X),
        ] {
            let c = n * lat + Vec3::Y * y;
            for off in [-0.075f32, 0.075] {
                let p = c + r * off;
                black.append(
                    &bx(
                        p - r * 0.008 - n * 0.008 - Vec3::Y * 0.22,
                        p + r * 0.008 + n * 0.008 + Vec3::Y * 0.22,
                    ),
                    Mat4::IDENTITY,
                );
            }
            for off in [-0.06f32, 0.06] {
                let p = c + Vec3::Y * off;
                black.append(
                    &bx(
                        p - r * 0.232 - n * 0.008 - Vec3::Y * 0.007,
                        p + r * 0.232 + n * 0.008 + Vec3::Y * 0.007,
                    ),
                    Mat4::IDENTITY,
                );
            }
        }
    }
    // Roof: a hip roof with upswept corners and a finial.
    let ry = y_top;
    let mut r = M::lathe(
        &[
            Vec2::new(0.36, ry),
            Vec2::new(0.86, ry + 0.07),
            Vec2::new(0.96, ry + 0.13),
            Vec2::new(0.95, ry + 0.17),
            Vec2::new(0.8, ry + 0.2),
            Vec2::new(0.52, ry + 0.3),
            Vec2::new(0.26, ry + 0.44),
            Vec2::new(0.09, ry + 0.52),
        ],
        4,
    )
    .flat_shaded();
    r = r.transformed(Mat4::from_quat(Quat::from_rotation_y(PI / 4.0)));
    r.planar_uvs(1.0);
    roof.append(&r, Mat4::IDENTITY);
    // Rafter ends under the eaves and the beam the roof sits on.
    black.append(
        &slab(
            v3(-0.4, ry - 0.06, -0.4),
            v3(0.4, ry + 0.02, 0.4),
            0.01,
            100.0,
        ),
        Mat4::IDENTITY,
    );
    for (n, r) in [
        (Vec3::X, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z),
        (Vec3::Z, Vec3::X),
        (Vec3::NEG_Z, Vec3::X),
    ] {
        for k in -3..=3 {
            let p = n * 0.42 + r * (k as f32 * 0.09) + Vec3::Y * (ry + 0.015);
            black.append(
                &bx(
                    p - r * 0.016 - n * 0.06 - Vec3::Y * 0.02,
                    p + r * 0.016 + n * 0.12 + Vec3::Y * 0.02,
                ),
                Mat4::IDENTITY,
            );
        }
    }
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let d = v3(sx, 0.0, sz).normalize();
        roof.append(
            &pipe(
                &[
                    d * 0.7 + Vec3::Y * (ry + 0.2),
                    d * 0.88 + Vec3::Y * (ry + 0.17),
                    d * 0.98 + Vec3::Y * (ry + 0.19),
                    d * 1.03 + Vec3::Y * (ry + 0.27),
                ],
                0.03,
                8,
            ),
            Mat4::IDENTITY,
        );
        // A small bell under each corner.
        brass.append(
            &M::lathe(
                &[
                    Vec2::new(0.0001, ry + 0.02),
                    Vec2::new(0.03, ry + 0.0),
                    Vec2::new(0.035, ry - 0.06),
                    Vec2::new(0.045, ry - 0.08),
                    Vec2::new(0.0001, ry - 0.08),
                ],
                12,
            ),
            Mat4::from_translation(d * 0.86 + Vec3::Y * 0.05),
        );
    }
    brass.append(
        &M::lathe(
            &[
                Vec2::new(0.07, ry + 0.5),
                Vec2::new(0.09, ry + 0.56),
                Vec2::new(0.05, ry + 0.62),
                Vec2::new(0.08, ry + 0.68),
                Vec2::new(0.06, ry + 0.76),
                Vec2::new(0.0001, ry + 0.88),
            ],
            16,
        ),
        Mat4::IDENTITY,
    );
    // Sacred rope around the posts near the top, with paper streamers.
    let ropey = top_cell - 0.05;
    let o = pc + 0.09;
    let mut path = Vec::new();
    for k in 0..=64 {
        let t = k as f32 / 64.0;
        let a = t * TAU + PI / 4.0;
        let sq = v3(a.cos(), 0.0, a.sin());
        let m = sq.x.abs().max(sq.z.abs());
        path.push(v3(sq.x / m * o, ropey, sq.z / m * o));
    }
    for strand in 0..2 {
        let twisted: Vec<Vec3> = path
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let a = i as f32 * 1.1 + strand as f32 * PI;
                *p + v3(0.0, a.sin() * 0.018, 0.0) + p.normalize() * a.cos() * 0.018
            })
            .collect();
        rope.append(&M::tube(&twisted, 0.022, 8), Mat4::IDENTITY);
    }
    for (sx, sz) in [(0.0f32, -1.0f32), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
        let base = v3(sx * (o + 0.01), ropey - 0.03, sz * (o + 0.01));
        let n = v3(sx, 0.0, sz);
        let right = Vec3::Y.cross(n).normalize();
        // Zigzag shide: four steps down.
        for k in 0..4 {
            let y = base.y - 0.06 - k as f32 * 0.07;
            let off = if k % 2 == 0 { -0.03 } else { 0.03 };
            let c = v3(base.x, y, base.z) + right * off + n * 0.004;
            let mut q = M::new();
            q.flat_quad(
                [
                    c - right * 0.035 - Vec3::Y * 0.035,
                    c + right * 0.035 - Vec3::Y * 0.035,
                    c + right * 0.035 + Vec3::Y * 0.035,
                    c - right * 0.035 + Vec3::Y * 0.035,
                ],
                [
                    Vec2::ZERO,
                    Vec2::new(0.07, 0.0),
                    Vec2::new(0.07, 0.07),
                    Vec2::new(0.0, 0.07),
                ],
            );
            paper.append(&q, Mat4::IDENTITY);
        }
    }
    kit.add(LACQUER_BLACK, black);
    kit.add(BRASS, brass);
    kit.add(STONE, stone);
    kit.add(KAWARA, roof);
    kit.add(CLOTH, rope);
    kit.add(PAPER_LIT, paper);
    kit
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::Assets;
    use engine_render::Mesh;

    fn library() -> (Assets<Mesh>, Assets<Image>, Assets<StandardMaterial>) {
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
        crate::look::materials::generate(&mut lib);
        generate(&mut lib);
        (meshes, images, materials)
    }

    #[test]
    fn every_spawned_part_has_a_mesh_and_a_material() {
        let (meshes, _, materials) = library();
        let sets = [
            COURSE_SET,
            freight::TRACK_SET,
            freight::CAR_SET,
            RING_SET,
            LANTERN_SET,
            BANNER_SET,
            COLUMN_SET,
        ];
        for set in sets {
            for &(group, mat, _) in set {
                let key = format!("movement/{}", kit_key(group, mat));
                let h = meshes
                    .by_key(&key)
                    .unwrap_or_else(|| panic!("no mesh {key}"));
                assert!(meshes.get(h).unwrap().triangle_count() > 0, "{key}");
                assert!(
                    materials.by_key(&format!("movement/{mat}")).is_some(),
                    "no material {mat}"
                );
            }
        }
        for v in 0..2 {
            for (g, m) in [
                (format!("banner_cloth{v}"), [CLOTH_INDIGO, CLOTH_CRIMSON][v]),
                (format!("banner_crest{v}"), CREST_WHITE),
            ] {
                assert!(
                    meshes
                        .by_key(&format!("movement/{}", kit_key(&g, m)))
                        .is_some(),
                    "{g}"
                );
            }
        }
        assert!(meshes.by_key(&format!("movement/{LEVEL_CELL}")).is_some());
    }

    #[test]
    fn every_generated_mesh_is_spawned() {
        let (meshes, _, _) = library();
        let mut spawned: Vec<String> = [
            COURSE_SET,
            freight::TRACK_SET,
            freight::CAR_SET,
            RING_SET,
            LANTERN_SET,
            BANNER_SET,
            COLUMN_SET,
        ]
        .iter()
        .flat_map(|s| {
            s.iter()
                .map(|&(g, m, _)| format!("movement/{}", kit_key(g, m)))
        })
        .collect();
        for v in 0..2 {
            spawned.push(format!(
                "movement/{}",
                kit_key(
                    &format!("banner_cloth{v}"),
                    [CLOTH_INDIGO, CLOTH_CRIMSON][v]
                )
            ));
            spawned.push(format!(
                "movement/{}",
                kit_key(&format!("banner_crest{v}"), CREST_WHITE)
            ));
        }
        spawned.push(format!("movement/{LEVEL_CELL}"));
        for (key, _) in meshes.keys() {
            if key.starts_with("movement/course/") {
                assert!(
                    spawned.iter().any(|s| s == key),
                    "{key} is built but never spawned"
                );
            }
        }
    }

    #[test]
    fn the_walls_run_faces_and_tower_top_match_the_colliders() {
        let (meshes, _, _) = library();
        let l = course_layout();
        let get = |group: &str, mat: &str| {
            let h = meshes
                .by_key(&format!("movement/{}", kit_key(group, mat)))
                .unwrap();
            meshes.get(h).unwrap().clone()
        };
        // Wall panels: nothing in front of either wall face, faces reach them.
        let walls = get("walls", PANEL);
        let (a0, a1) = l.wall_a;
        let (b0, b1) = l.wall_b;
        let mut reach_a: f32 = 1.0;
        let mut reach_b: f32 = -1.0;
        for v in &walls.vertices {
            let p = Vec3::from(v.position);
            if p.x > a0.x + 0.1 && p.x < a1.x - 0.1 && p.y > 0.5 && p.z < 15.5 {
                assert!(p.z >= a0.z - 1e-4, "wall A panel in front of its face: {p}");
                reach_a = reach_a.min(p.z - a0.z);
            }
            if p.x > b0.x + 0.1 && p.x < b1.x - 0.1 && p.y > 0.5 && p.z > 8.5 {
                assert!(p.z <= b1.z + 1e-4, "wall B panel in front of its face: {p}");
                reach_b = reach_b.max(p.z - b1.z);
            }
        }
        assert!(
            reach_a.abs() < 1e-4 && reach_b.abs() < 1e-4,
            "{reach_a} {reach_b}"
        );
        // Tower top is exactly the collider top.
        let tower = get("tower", CONCRETE);
        let top = tower
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert!((top - layout::TOWER_TOP as f32).abs() < 1e-4, "{top}");
        // Slide bar underside stays at the collider's.
        let bar = get("bar", HAZARD);
        let low = bar
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MAX, f32::min);
        assert!((low - l.bar.0.y).abs() < 1e-4, "{low}");
        // The grapple tower: its top and the face the hook bites are the collider's.
        let (g0, g1) = bounds(layout::GRAPPLE_TOWER);
        let top = get("grapple", CONCRETE)
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert!((top - g1.y).abs() < 1e-4, "{top}");
        let face = get("grapple", PANEL)
            .vertices
            .iter()
            .map(|v| v.position[2])
            .fold(f32::MAX, f32::min);
        assert!((face - g0.z).abs() < 1e-4, "{face}");
        // The freight platform's top and the car's roof and sides are their colliders'.
        let (f0, f1) = bounds(layout::FREIGHT_PLATFORM);
        for mat in [CONCRETE, STONE] {
            let top = get("freight", mat)
                .vertices
                .iter()
                .map(|v| v.position[1])
                .fold(f32::MIN, f32::max);
            assert!((top - f1.y).abs() < 1e-4, "{mat} {top}");
        }
        let half = Vec3::from(layout::FREIGHT_CAR_SIZE) * 0.5;
        let car = get("freight_car", OXIDE);
        let roof = get("freight_car", ROOF_GREY)
            .vertices
            .iter()
            .filter(|v| v.position[2].abs() < half.z - 0.1)
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert!(roof >= half.y && roof < half.y + 0.01, "{roof}");
        let mut side: f32 = 0.0;
        for v in &car.vertices {
            let p = Vec3::from(v.position);
            assert!(
                p.z.abs() <= half.z + 1e-4,
                "car body outside its collider: {p}"
            );
            if p.x.abs() < half.x - 0.2 && p.y > 0.0 {
                side = side.max(p.z.abs());
            }
        }
        assert!((side - half.z).abs() < 1e-4, "{side}");
        assert!(f0.z < f1.z);
        // Ring floor at the collider top.
        let floor = get("plinth", PLANKS);
        for v in &floor.vertices {
            assert!((v.position[1] - RING_TOP).abs() < 1e-5);
        }
    }

    #[test]
    fn atlases_have_their_text() {
        let paint = paint_atlas(256);
        let alpha = paint.data.chunks(4).filter(|p| p[3] > 128).count();
        assert!(alpha > 1000, "{alpha}");
        for row in 0..PAINT_ROWS.len() {
            let (uv0, uv1, aspect) = paint_rect(row);
            assert!(
                uv1.x <= 1.0 && uv1.y <= 1.0 && uv0.y < uv1.y && aspect > 0.5,
                "{row}"
            );
        }
        for row in 0..SIGN_ROWS.len() {
            let (_, uv1, _) = sign_rect(row);
            assert!(uv1.x <= 1.0, "sign row {row} too wide");
        }
        // Rows sharing an atlas line don't overlap.
        let rects = |n: usize, rect: &dyn Fn(usize) -> (Vec2, Vec2, f32)| {
            for a in 0..n {
                for b in a + 1..n {
                    let ((a0, a1, _), (b0, b1, _)) = (rect(a), rect(b));
                    let apart = a1.x <= b0.x || b1.x <= a0.x || a1.y <= b0.y || b1.y <= a0.y;
                    assert!(apart, "atlas rows {a} and {b} overlap");
                }
            }
        };
        rects(PAINT_ROWS.len(), &paint_rect);
        rects(SIGN_ROWS.len(), &sign_rect);
        let crest = crest_image(64);
        assert!(crest.data.chunks(4).filter(|p| p[3] > 128).count() > 300);
    }

    #[test]
    fn light_budget() {
        let lights = COURSE_LIGHTS.len() + LANTERNS.len() + STAND_LANTERNS_X.len();
        assert!(lights <= 16, "{lights} point lights");
    }

    #[test]
    fn triangle_budget() {
        let (meshes, _, _) = library();
        let tris = |group: &str, mat: &str| {
            let key = format!("movement/{}", kit_key(group, mat));
            meshes
                .get(meshes.by_key(&key).unwrap())
                .unwrap()
                .triangle_count()
        };
        let sum =
            |set: &[(&str, &str, bool)]| -> usize { set.iter().map(|&(g, m, _)| tris(g, m)).sum() };
        let mut banners = sum(BANNER_SET);
        for v in 0..2 {
            banners += tris(
                &format!("banner_cloth{v}"),
                [CLOTH_INDIGO, CLOTH_CRIMSON][v],
            );
            banners += tris(&format!("banner_crest{v}"), CREST_WHITE);
        }
        let course = sum(COURSE_SET) + sum(freight::TRACK_SET) + sum(freight::CAR_SET);
        let ring = sum(RING_SET) + LANTERNS.len() * sum(LANTERN_SET) + BANNERS.len() * banners / 2;
        let column = sum(COLUMN_SET);
        let total = course + ring + column;
        eprintln!("triangles: course {course}, ring {ring}, column {column}, total {total}");
        assert!(total < 600_000, "{total} triangles");
    }
}

//! Assembles the sport plane: builds every mesh, bakes the atlases, defines the materials
//! (once per world) and spawns the drawn parts under a vehicle body.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{AlphaMode, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial};

use crate::aero::Aircraft;
use crate::visuals::{PropellerPart, PropellerVisual, SurfaceVisual, WheelVisual};
use crate::wheels::Wheels;

use super::plane_paint::*;
use super::plane_parts::*;
use super::sport_plane::*;
use super::surfaces::*;
use super::toolkit::*;

/// Body atlas islands (u, v of their corner) next to the fuselage's own unwrap.
pub const FILLET_UPPER_UV: Vec2 = Vec2::new(0.80, 0.795);
pub const FILLET_LOWER_UV: Vec2 = Vec2::new(0.885, 0.795);
pub const PANT_UV: Vec2 = Vec2::new(0.004, 0.874);

/// Main wheel axle at rest (right side) and nose wheel axle at rest, body frame.
pub const MAIN_AXLE: Vec3 = Vec3::new(1.2, -0.62, 0.45);
pub const NOSE_AXLE: Vec3 = Vec3::new(0.0, -0.67, -2.4);
const MAIN_TIRE: (f32, f32) = (0.25, 0.14);
const NOSE_TIRE: (f32, f32) = (0.20, 0.12);

/// A control surface's pivot.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    Flap,
    Aileron,
    Elevator,
    Rudder,
}

#[derive(Clone, Copy, Debug)]
pub struct Pivot {
    pub control: Control,
    pub side: f32,
    /// Pivot origin (hinge midpoint) and rotation at zero deflection, body frame.
    pub at: Vec3,
    pub base: Quat,
    /// Hinge axis in the pivot frame (sign set so + deflection moves the trailing edge
    /// toward the surface's −up, as the simulation defines it).
    pub hinge: Vec3,
    /// Hinge midpoint in the panel frame the surface is built in.
    pub mid: Vec3,
}

impl Control {
    fn name(self) -> &'static str {
        match self {
            Control::Flap => "flap",
            Control::Aileron => "aileron",
            Control::Elevator => "elevator",
            Control::Rudder => "rudder",
        }
    }
}

pub fn pivot(control: Control, side: f32) -> Pivot {
    let (a, b, to_body, base_r) = match control {
        Control::Flap | Control::Aileron => {
            let (a, b) = wing_hinge(if control == Control::Flap { 0 } else { 1 });
            (a, b, wing_to_body(1.0), wing_rotation(1.0))
        }
        Control::Elevator => {
            let p = tail_panel();
            (
                p.hinge_pt(ELEVATOR_SPAN.0).0,
                p.hinge_pt(ELEVATOR_SPAN.1).0,
                tail_to_body(1.0),
                tail_rotation(),
            )
        }
        Control::Rudder => {
            let p = fin_panel();
            (
                p.hinge_pt(RUDDER_SPAN.0).0,
                p.hinge_pt(RUDDER_SPAN.1).0,
                fin_to_body(),
                Quat::from_rotation_z(FRAC_PI_2),
            )
        }
    };
    let mid = (a + b) * 0.5;
    let d = (b - a).normalize();
    let at = to_body.transform_point3(mid);
    let mirror = Vec3::new(-1.0, 1.0, 1.0);
    if side < 0.0 {
        let base = match control {
            Control::Flap | Control::Aileron => wing_rotation(-1.0),
            _ => base_r,
        };
        Pivot {
            control,
            side,
            at: at * mirror,
            base,
            hinge: d * mirror,
            mid,
        }
    } else {
        Pivot {
            control,
            side,
            at,
            base: base_r,
            hinge: -d,
            mid,
        }
    }
}

fn pivot_name(p: &Pivot) -> String {
    let side = if p.control == Control::Rudder {
        ""
    } else if p.side < 0.0 {
        "_l"
    } else {
        "_r"
    };
    format!("{}{side}", p.control.name())
}

/// Every control surface of the plane, by the simulated surface it follows.
pub fn pivots() -> [Pivot; 7] {
    [
        pivot(Control::Flap, -1.0),
        pivot(Control::Flap, 1.0),
        pivot(Control::Aileron, -1.0),
        pivot(Control::Aileron, 1.0),
        pivot(Control::Elevator, -1.0),
        pivot(Control::Elevator, 1.0),
        pivot(Control::Rudder, 1.0),
    ]
}

/// Static wicks on a control surface's trailing edge (panel frame), dark, with UVs on a
/// plain stretch of the atlas.
fn wicks(p: &Panel, spans: &[f32], uv: Vec2, outward: Vec3) -> Mesh {
    let mut m = Mesh::default();
    for &s in spans {
        let te = (p.pt(s, 1.0, true) + p.pt(s, 1.0, false)) * 0.5;
        let dir = (Vec3::Z + outward * 0.25).normalize();
        m.merge(&wick(te - Vec3::Z * 0.004, dir));
    }
    for v in &mut m.vertices {
        v.uv = uv.to_array();
        v.color = [0.035, 0.035, 0.035, 1.0];
    }
    m
}

fn pivot_mesh(pv: &Pivot) -> Mesh {
    let raw = match pv.control {
        Control::Flap => wing_control(0),
        Control::Aileron => merged([
            wing_control(1),
            wicks(&wing_panel(), &[3.85, 4.30], Vec2::new(0.2, 0.6), Vec3::X),
        ]),
        Control::Elevator => merged([
            elevator(),
            wicks(&tail_panel(), &[1.55], Vec2::new(0.1, 0.2), Vec3::X),
        ]),
        Control::Rudder => merged([
            rudder(),
            wicks(&fin_panel(), &[1.45], Vec2::new(0.1, 0.6), Vec3::X),
        ]),
    };
    let local = transformed(raw, Mat4::from_translation(-pv.mid));
    if pv.side < 0.0 {
        mirrored_x(&local)
    } else {
        local
    }
}

// ---------------------------------------------------------------- materials

fn mat(hex_color: u32, rough: f32, metal: f32) -> StandardMaterial {
    StandardMaterial::hex(hex_color)
        .with_roughness(rough)
        .with_metallic(metal)
}

/// A white-based material for textures and vertex colors.
fn textured(rough: f32, metal: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: rough,
        metallic: metal,
        ..Default::default()
    }
}

fn lamp(color: Vec3, strength: f32) -> StandardMaterial {
    let mut m = StandardMaterial::color(color.x, color.y, color.z).with_roughness(0.08);
    m.emissive = color.to_array();
    m.emissive_strength = strength;
    m.reflectance = 0.7;
    m
}

fn glass_material() -> StandardMaterial {
    let mut m = StandardMaterial::hex(0x2a3438).with_roughness(0.02);
    m.base_color[3] = 0.16;
    m.alpha_mode = AlphaMode::Blend;
    m.reflectance = 1.0;
    m.double_sided = true;
    m.receives_wetness = false;
    m
}

// ---------------------------------------------------------------- build

/// Mesh parts: (name, material) for every fixed part spawned on the body.
const FIXED: &[(&str, &str, bool)] = &[
    ("body", "body", true),
    ("pant_inner", "dark", true),
    ("lips", "white", true),
    ("wing", "wing", true),
    ("tail", "tail", true),
    ("canopy", "glass", false),
    ("canopy_frame", "frame", true),
    ("cockpit", "interior", true),
    ("seats", "seat", true),
    ("belts", "belt", true),
    ("controls", "vertex", true),
    ("panel", "panel", true),
    ("screens", "screen", false),
    ("exhaust", "exhaust", true),
    ("black", "black", true),
    ("nose_strut", "chrome", true),
    ("nose_fork", "white", true),
    ("metal", "metal", true),
    ("nav_r", "nav_green", false),
    ("nav_l", "nav_red", false),
    ("strobes", "strobe", false),
    ("beacons", "beacon", false),
];

/// Every mesh, texture and material of the sport plane.
pub fn build() -> ModelAssets {
    let mut out = ModelAssets::default();
    let f = Fuselage::new();

    // ---- body: fuselage, root fillets, wheel fairings
    let fuselage = fuselage_mesh(&f);
    let fillet_up = root_fillet(&f, true, FILLET_UPPER_UV.x, FILLET_UPPER_UV.y);
    let fillet_lo = root_fillet(&f, false, FILLET_LOWER_UV.x, FILLET_LOWER_UV.y);
    let (pant, pant_inner) = wheel_pant(MAIN_AXLE, MAIN_TIRE.0, PANT_UV.x, PANT_UV.y);
    let mut atlas = Atlas::new(1024, 2048);
    atlas.raster(&fuselage, BODY_FUSELAGE);
    atlas.raster(&fillet_up, BODY_FILLET);
    atlas.raster(&fillet_lo, BODY_FILLET);
    atlas.raster(&pant, BODY_PANT);
    out.baked(&key("tex/body"), paint_body(&atlas, &f));
    let sides = merged([fillet_up, fillet_lo, pant]);
    out.mesh(&key("body"), merged([fuselage, mirrored_x(&sides), sides]));
    out.mesh(
        &key("pant_inner"),
        merged([mirrored_x(&pant_inner), pant_inner]),
    );

    // ---- wings
    let wing_parts = wing_fixed();
    let mut atlas = Atlas::new(1024, 2048);
    for (part, m) in &wing_parts {
        atlas.raster(m, *part);
    }
    atlas.raster(&wing_control(0), PART_CONTROL);
    atlas.raster(&wing_control(1), PART_CONTROL);
    out.baked(&key("tex/wing"), paint_wing(&atlas));
    let wing = merged(wing_parts.into_iter().map(|(_, m)| m));
    out.mesh(
        &key("wing"),
        merged([
            transformed(wing.clone(), wing_to_body(1.0)),
            transformed(wing, wing_to_body(-1.0)),
        ]),
    );

    // ---- tail
    let tail_parts = tailplane_fixed();
    let fin_parts = fin_fixed();
    let mut atlas = Atlas::new(1024, 1024);
    for (part, m) in &tail_parts {
        atlas.raster(m, *part);
    }
    for (part, m) in &fin_parts {
        atlas.raster(m, *part + FIN_PART_OFFSET);
    }
    atlas.raster(&elevator(), PART_CONTROL);
    atlas.raster(&rudder(), PART_CONTROL + FIN_PART_OFFSET);
    out.baked(&key("tex/tail"), paint_tail(&atlas));
    let tail = merged(tail_parts.into_iter().map(|(_, m)| m));
    let fin = merged(fin_parts.into_iter().map(|(_, m)| m));
    out.mesh(
        &key("tail"),
        merged([
            transformed(tail.clone(), tail_to_body(1.0)),
            transformed(tail, tail_to_body(-1.0)),
            transformed(fin, fin_to_body()),
        ]),
    );

    // ---- control surfaces
    for pv in pivots() {
        out.mesh(&key(&pivot_name(&pv)), pivot_mesh(&pv));
    }

    // ---- cowl details, canopy, cockpit
    let mut lips = Mesh::default();
    for (c, n, r) in inlets(&f) {
        lips.merge(&lip_ring(c, n, r, 0.011, 1.0, 1.0));
    }
    let (cc, cn, half) = chin_inlet(&f);
    lips.merge(&lip_ring(
        cc,
        cn,
        1.0,
        0.009,
        half.x + 0.004,
        half.y + 0.004,
    ));
    out.mesh(&key("lips"), lips);
    out.mesh(&key("canopy"), canopy_mesh(&f));
    out.mesh(&key("canopy_frame"), canopy_frame(&f));
    let (tub, panel_plate) = cockpit_shell(&f);
    out.mesh(&key("cockpit"), tub);
    let (seat, belts) = seats();
    out.mesh(&key("seats"), seat);
    out.mesh(&key("belts"), belts);
    out.mesh(&key("controls"), controls());
    out.mesh(&key("panel"), merged([panel_plate, panel_furniture()]));
    out.mesh(&key("screens"), screens());
    let (pipes, ends) = exhausts();
    out.mesh(&key("exhaust"), pipes);
    // Dark parts: exhaust bores, the disc behind the spinner, the inlet ducts' backs.
    let cowl_back: Vec<Vec3> = (0..48)
        .map(|k| {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            f.point(Z_NOSE + 0.03, a, false) * Vec3::new(0.97, 0.97, 1.0)
        })
        .collect();
    let (metal_small, black_small) = small_parts(&f);
    out.mesh(
        &key("black"),
        merged([
            ends,
            fan(&cowl_back, Vec3::NEG_Z, |_| Vec2::ZERO),
            black_small,
        ]),
    );
    out.mesh(&key("metal"), metal_small);

    // ---- gear
    let (steel, fork) = nose_gear(NOSE_AXLE, NOSE_TIRE.0, NOSE_TIRE.1);
    out.mesh(&key("nose_strut"), steel);
    out.mesh(&key("nose_fork"), fork);
    out.mesh(&key("tire_main"), tire(MAIN_TIRE.0, MAIN_TIRE.1));
    out.mesh(&key("tire_nose"), tire(NOSE_TIRE.0, NOSE_TIRE.1));
    out.mesh(&key("hub_main"), hub(MAIN_TIRE.0, MAIN_TIRE.1));
    out.mesh(&key("hub_nose"), hub(NOSE_TIRE.0, NOSE_TIRE.1));

    // ---- propeller
    out.mesh(&key("spinner"), spinner());
    out.mesh(&key("blades"), blades());
    out.mesh(&key("prop_disc"), prop_disc());

    // ---- lights
    let (nav, strobe, white) = wingtip_lights();
    let lens_nav = lens(nav, Vec3::new(0.024, 0.02, 0.05), wing_rotation(1.0));
    let lens_strobe = lens(strobe, Vec3::new(0.018, 0.016, 0.03), wing_rotation(1.0));
    let lens_white = lens(white, Vec3::new(0.016, 0.012, 0.025), wing_rotation(1.0));
    out.mesh(&key("nav_r"), lens_nav.clone());
    out.mesh(&key("nav_l"), mirrored_x(&lens_nav));
    let strobes = merged([lens_strobe, lens_white]);
    out.mesh(&key("strobes"), merged([mirrored_x(&strobes), strobes]));
    out.mesh(&key("beacons"), beacons(&f));

    // ---- textures
    let (tc, tn, to) = tire_textures();
    out.image(&key("tex/tire/color"), tc);
    out.image(&key("tex/tire/normal"), tn);
    out.image(&key("tex/tire/orm"), to);
    out.image(&key("tex/leather/normal"), leather_normal());
    out.image(&key("tex/screens/glow"), screen_image());

    // ---- materials
    let k = |n: &str| key(&format!("mat/{n}"));
    for (name, tex) in [("body", "body"), ("wing", "wing"), ("tail", "tail")] {
        let mut m = textured(1.0, 1.0);
        m.reflectance = 0.6;
        out.material(MaterialDef::baked(&k(name), m, &key(&format!("tex/{tex}"))));
    }
    out.material(MaterialDef::baked(
        &k("tire"),
        textured(1.0, 1.0),
        &key("tex/tire"),
    ));
    out.material(MaterialDef::plain(&k("glass"), glass_material()));
    out.material(MaterialDef::plain(&k("white"), mat(WHITE, 0.2, 0.0)));
    out.material(MaterialDef::plain(&k("dark"), mat(0x1c1c1e, 0.8, 0.0)));
    out.material(MaterialDef::plain(&k("frame"), mat(0x141518, 0.45, 0.0)));
    out.material(MaterialDef::plain(&k("interior"), mat(0x5a5e66, 0.8, 0.0)));
    let mut seat = MaterialDef::plain(&k("seat"), {
        let mut m = mat(0x3c4048, 0.78, 0.0);
        m.uv_scale = [4.0, 4.0];
        m
    });
    seat.normal = Some(key("tex/leather/normal"));
    out.material(seat);
    out.material(MaterialDef::plain(&k("belt"), mat(0x8c1c18, 0.85, 0.0)));
    out.material(MaterialDef::plain(&k("vertex"), textured(0.62, 0.0)));
    out.material(MaterialDef::plain(&k("panel"), mat(0x1b1c1f, 0.55, 0.0)));
    let mut screen = MaterialDef::plain(&k("screen"), {
        let mut m = mat(0x050607, 0.12, 0.0);
        m.emissive = [1.0; 3];
        m.emissive_strength = 1400.0;
        m
    });
    screen.glow = Some(key("tex/screens/glow"));
    out.material(screen);
    out.material(MaterialDef::plain(&k("exhaust"), {
        let mut m = textured(0.42, 1.0);
        m.base_color = [0.80, 0.77, 0.72, 1.0];
        m
    }));
    out.material(MaterialDef::plain(&k("black"), mat(0x0a0a0a, 0.7, 0.0)));
    out.material(MaterialDef::plain(&k("chrome"), mat(0xd2d5da, 0.16, 1.0)));
    out.material(MaterialDef::plain(&k("metal"), mat(0xa4a8ae, 0.35, 1.0)));
    out.material(MaterialDef::plain(&k("hub"), mat(0xb9bdc3, 0.33, 0.9)));
    out.material(MaterialDef::plain(&k("spinner"), mat(NAVY, 0.18, 0.0)));
    out.material(MaterialDef::plain(&k("blades"), textured(0.45, 0.0)));
    out.material(MaterialDef::plain(&k("prop_disc"), {
        let mut m = textured(0.6, 0.0);
        m.alpha_mode = AlphaMode::Blend;
        m.double_sided = true;
        m.receives_wetness = false;
        m
    }));
    out.material(MaterialDef::plain(
        &k("nav_green"),
        lamp(Vec3::new(0.05, 1.0, 0.25), 2500.0),
    ));
    out.material(MaterialDef::plain(
        &k("nav_red"),
        lamp(Vec3::new(1.0, 0.04, 0.02), 2500.0),
    ));
    out.material(MaterialDef::plain(
        &k("strobe"),
        lamp(Vec3::new(0.9, 0.92, 1.0), 900.0),
    ));
    out.material(MaterialDef::plain(
        &k("beacon"),
        lamp(Vec3::new(1.0, 0.06, 0.02), 2000.0),
    ));
    out
}

/// The beacons: on top of the fin and under the belly.
fn beacons(f: &Fuselage) -> Mesh {
    let dome = |c: Vec3, up: f32| -> Mesh {
        let mut m = lathe(
            &[
                (0.0, 0.045),
                (0.012, 0.044),
                (0.024, 0.036),
                (0.030, 0.022),
                (0.032, 0.0),
            ],
            20,
            false,
        );
        // Lathe about +Z with the dome toward +Z: stand it on its base.
        m = transformed(m, Mat4::from_rotation_x(-FRAC_PI_2 * up));
        transformed(m, Mat4::from_translation(c))
    };
    let fin_top = Vec3::new(0.0, 1.835, 4.47);
    let belly = Vec3::new(0.0, f.bottom_at(1.05) + 0.004, 1.05);
    merged([dome(fin_top, 1.0), dome(belly, -1.0)])
}

// ---------------------------------------------------------------- spawn

fn spawn_part(
    commands: &mut Commands,
    parent: Entity,
    mesh: &str,
    material: &str,
    transform: Transform,
    shadows: bool,
) -> Entity {
    let mut r = MeshRenderer::new(
        MeshRef::Asset(key(mesh)),
        MaterialRef::Asset(key(&format!("mat/{material}"))),
    );
    r.cast_shadows = shadows;
    commands.spawn((transform, r, ChildOf(parent))).id()
}

/// Spawn the sport plane's drawn parts under `body` (the vehicle carrying `aircraft` and
/// `wheels`); the model's assets are built the first time.
pub fn spawn(commands: &mut Commands, body: Entity, aircraft: &Aircraft, wheels: &Wheels) {
    commands.queue(|world: &mut World| ensure(world, &key("body"), build));
    for &(mesh, material, shadows) in FIXED {
        spawn_part(
            commands,
            body,
            mesh,
            material,
            Transform::default(),
            shadows,
        );
    }
    // Control surfaces: match each pivot to the simulated surface it follows.
    for pv in pivots() {
        let index = aircraft.surfaces.iter().position(|s| {
            let n = s.name.as_str();
            let side_ok = pv.control == Control::Rudder || (s.position.x < 0.0) == (pv.side < 0.0);
            side_ok
                && match pv.control {
                    Control::Flap => n.contains("inner"),
                    Control::Aileron => n.contains("outer"),
                    Control::Elevator => n.contains("tailplane"),
                    Control::Rudder => n.contains("fin"),
                }
        });
        let Some(index) = index else { continue };
        let material = if pv.control == Control::Rudder || pv.control == Control::Elevator {
            "tail"
        } else {
            "wing"
        };
        let pivot = commands
            .spawn((
                Transform::from_translation(pv.at.as_dvec3()).with_rotation(pv.base),
                SurfaceVisual {
                    index,
                    hinge: pv.hinge,
                    base: pv.base,
                },
                ChildOf(body),
            ))
            .id();
        spawn_part(
            commands,
            pivot,
            &pivot_name(&pv),
            material,
            Transform::default(),
            true,
        );
    }
    // Propeller.
    let prop = commands
        .spawn((
            Transform::from_translation(Vec3::new(0.0, 0.0, Z_PROP).as_dvec3()),
            PropellerVisual {
                index: 0,
                base: Quat::IDENTITY,
            },
            ChildOf(body),
        ))
        .id();
    spawn_part(
        commands,
        prop,
        "spinner",
        "spinner",
        Transform::default(),
        true,
    );
    let blades = spawn_part(
        commands,
        prop,
        "blades",
        "blades",
        Transform::default(),
        true,
    );
    commands.entity(blades).insert(PropellerPart {
        index: 0,
        blur: false,
    });
    let disc = spawn_part(
        commands,
        prop,
        "prop_disc",
        "prop_disc",
        Transform::from_scale(Vec3::splat(1e-3)),
        false,
    );
    commands.entity(disc).insert(PropellerPart {
        index: 0,
        blur: true,
    });
    // Wheels.
    for (i, w) in wheels.wheels.iter().enumerate() {
        let mount = w.mount - Vec3::Y * w.suspension.travel_m * 0.6;
        let wheel = commands
            .spawn((
                Transform::from_translation(mount.as_dvec3()),
                WheelVisual {
                    index: i,
                    retracts: false,
                },
                ChildOf(body),
            ))
            .id();
        let size = if w.radius > 0.22 { "main" } else { "nose" };
        // Hubs face out: the +X face goes outboard.
        let turn = if w.mount.x < 0.0 {
            Quat::from_rotation_y(PI)
        } else {
            Quat::IDENTITY
        };
        spawn_part(
            commands,
            wheel,
            &format!("tire_{size}"),
            "tire",
            Transform::from_rotation(turn),
            true,
        );
        spawn_part(
            commands,
            wheel,
            &format!("hub_{size}"),
            "hub",
            Transform::from_rotation(turn),
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uv_range(m: &Mesh) -> (Vec2, Vec2) {
        let mut lo = Vec2::splat(f32::MAX);
        let mut hi = Vec2::splat(f32::MIN);
        for v in &m.vertices {
            lo = lo.min(Vec2::from(v.uv));
            hi = hi.max(Vec2::from(v.uv));
        }
        (lo, hi)
    }

    fn check(name: &str, parts: &[(u8, Mesh)], w: usize, h: usize) {
        let mut atlas = Atlas::new(w, h);
        for (part, m) in parts {
            let (lo, hi) = uv_range(m);
            eprintln!("{name} part {part}: uv {lo:?}..{hi:?}");
            assert!(
                lo.cmpge(Vec2::ZERO).all() && hi.cmple(Vec2::ONE).all(),
                "{name} part {part} leaves the atlas: {lo:?}..{hi:?}"
            );
            atlas.raster(m, *part);
        }
        eprintln!("{name}: {} overlapping texels", atlas.overlaps);
        assert!(
            atlas.overlaps < 40,
            "{name}: {} overlapping texels",
            atlas.overlaps
        );
    }

    #[test]
    fn atlas_islands_stay_apart() {
        let f = Fuselage::new();
        let (pant, _) = wheel_pant(MAIN_AXLE, MAIN_TIRE.0, PANT_UV.x, PANT_UV.y);
        check(
            "body",
            &[
                (BODY_FUSELAGE, fuselage_mesh(&f)),
                (
                    BODY_FILLET,
                    root_fillet(&f, true, FILLET_UPPER_UV.x, FILLET_UPPER_UV.y),
                ),
                (
                    BODY_FILLET + 10,
                    root_fillet(&f, false, FILLET_LOWER_UV.x, FILLET_LOWER_UV.y),
                ),
                (BODY_PANT, pant),
            ],
            1024,
            2048,
        );
        let mut wing = wing_fixed();
        wing.push((PART_CONTROL, wing_control(0)));
        wing.push((PART_CONTROL + 10, wing_control(1)));
        check("wing", &wing, 1024, 2048);
        let mut tail = tailplane_fixed();
        tail.extend(
            fin_fixed()
                .into_iter()
                .map(|(p, m)| (p + FIN_PART_OFFSET, m)),
        );
        tail.push((PART_CONTROL, elevator()));
        tail.push((PART_CONTROL + FIN_PART_OFFSET, rudder()));
        check("tail", &tail, 1024, 1024);
    }
}

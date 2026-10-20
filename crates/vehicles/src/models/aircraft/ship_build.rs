//! Assembles the spaceplane: builds every mesh, bakes the atlases, defines the materials
//! (once per world) and spawns the drawn parts under a vehicle body.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, StandardMaterial, Vertex,
};

use engine_assets::Assets;
use engine_world::{Exhaust, Exhausts};

use crate::aero::Aircraft;
use crate::exhaust::thruster_kind;
use crate::ship::{Thruster, ThrusterRole};
use crate::visuals::{
    GearFold, HeatGlow, PlasmaVisual, PlasmaWake, PlumeVisual, SurfaceVisual, WheelVisual,
};
use crate::wheels::Wheels;

use super::ship_gear::{self, Underside};
use super::ship_paint::*;
use super::ship_parts::*;
use super::ship_surfaces as ss;
use super::spaceplane::*;
use super::toolkit::{smoothstep, *};

/// Axles at rest (right main), body frame.
pub const NOSE_AXLE: Vec3 = Vec3::new(0.0, -2.23, -7.5);
pub const MAIN_AXLE: Vec3 = Vec3::new(2.2, -2.22, 4.0);
/// Twin tires: radius, width of each, gap between them.
const NOSE_TIRES: (f32, f32, f32) = (0.5, 0.2, 0.14);
const MAIN_TIRES: (f32, f32, f32) = (0.6, 0.24, 0.16);
/// Gear wells in the belly (center x, z and half sizes), also painted by the hull painter.
/// The legs swing forward into them ([`super::ship_gear`]), so they run ahead of the axles.
pub const NOSE_WELL: (Vec2, Vec2) = (Vec2::new(0.0, -7.98), Vec2::new(0.40, 0.84));
pub const MAIN_WELL: (Vec2, Vec2) = (Vec2::new(2.10, 3.50), Vec2::new(0.385, 0.96));

// ---------------------------------------------------------------- control surfaces

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    ElevonIn,
    ElevonOut,
    BodyFlap,
    Rudder,
}

#[derive(Clone, Copy, Debug)]
pub struct Pivot {
    pub control: Control,
    pub side: f32,
    /// Pivot origin and rotation at zero deflection, body frame.
    pub at: Vec3,
    pub base: Quat,
    /// Hinge axis in the pivot frame (+ deflection moves the trailing edge down).
    pub hinge: Vec3,
    /// Hinge midpoint in the frame the surface is built in.
    pub mid: Vec3,
}

pub fn pivot(control: Control, side: f32) -> Pivot {
    let mirror = Vec3::new(-1.0, 1.0, 1.0);
    match control {
        Control::ElevonIn | Control::ElevonOut => {
            let (s0, s1) = if control == Control::ElevonIn {
                ELEVON_IN
            } else {
                ELEVON_OUT
            };
            let p = wing_panel();
            let (a, b) = (p.hinge_pt(s0).0, p.hinge_pt(s1).0);
            let mid = (a + b) * 0.5;
            let d = (b - a).normalize();
            let at = wing_to_body(1.0).transform_point3(mid);
            if side < 0.0 {
                Pivot {
                    control,
                    side,
                    at: at * mirror,
                    base: wing_rotation(-1.0),
                    hinge: d * mirror,
                    mid,
                }
            } else {
                Pivot {
                    control,
                    side,
                    at,
                    base: wing_rotation(1.0),
                    hinge: -d,
                    mid,
                }
            }
        }
        Control::BodyFlap => Pivot {
            control,
            side,
            at: FLAP_HINGE,
            base: Quat::IDENTITY,
            hinge: Vec3::NEG_X,
            mid: Vec3::ZERO,
        },
        Control::Rudder => {
            let p = fin_panel();
            let (a, b) = (p.hinge_pt(RUDDER_SPAN.0).0, p.hinge_pt(RUDDER_SPAN.1).0);
            let mid = (a + b) * 0.5;
            Pivot {
                control,
                side,
                at: fin_to_body().transform_point3(mid),
                base: Quat::from_rotation_z(FRAC_PI_2),
                hinge: -(b - a).normalize(),
                mid,
            }
        }
    }
}

pub fn pivots() -> [Pivot; 7] {
    [
        pivot(Control::ElevonIn, -1.0),
        pivot(Control::ElevonIn, 1.0),
        pivot(Control::ElevonOut, -1.0),
        pivot(Control::ElevonOut, 1.0),
        pivot(Control::BodyFlap, -1.0),
        pivot(Control::BodyFlap, 1.0),
        pivot(Control::Rudder, 1.0),
    ]
}

fn pivot_name(p: &Pivot) -> String {
    let side = if p.side < 0.0 { "l" } else { "r" };
    match p.control {
        Control::ElevonIn => format!("elevon_in_{side}"),
        Control::ElevonOut => format!("elevon_out_{side}"),
        Control::BodyFlap => format!("body_flap_{side}"),
        Control::Rudder => "rudder".into(),
    }
}

fn elevon(control: Control) -> Mesh {
    let (s0, s1) = if control == Control::ElevonIn {
        ELEVON_IN
    } else {
        ELEVON_OUT
    };
    let rows = ((s1 - s0) / 0.12).ceil() as usize;
    ss::control(&wing_panel(), s0, s1, rows, &wing_uv)
}

fn rudder() -> Mesh {
    let rows = ((RUDDER_SPAN.1 - RUDDER_SPAN.0) / 0.15).ceil() as usize;
    ss::control(&fin_panel(), RUDDER_SPAN.0, RUDDER_SPAN.1, rows, &fin_uv)
}

/// A control surface's mesh in its pivot frame (the right side's built in its own frame and
/// mirrored for the left).
fn pivot_mesh(pv: &Pivot) -> Mesh {
    let raw = match pv.control {
        Control::ElevonIn | Control::ElevonOut => elevon(pv.control),
        Control::BodyFlap => body_flap(&flap_uv),
        Control::Rudder => rudder(),
    };
    let local = transformed(raw, Mat4::from_translation(-pv.mid));
    if pv.side < 0.0 && pv.control != Control::Rudder {
        mirrored_x(&local)
    } else {
        local
    }
}

// ---------------------------------------------------------------- fixed surfaces

fn wing_fixed() -> Mesh {
    ss::fixed(
        &wing_panel(),
        WING_S0,
        WING_TIP,
        &[ELEVON_IN, ELEVON_OUT],
        0.12,
        36,
        true,
        &wing_uv,
    )
}

fn fin_fixed() -> Mesh {
    ss::fixed(
        &fin_panel(),
        FIN_S0,
        FIN_TIP,
        &[RUDDER_SPAN],
        0.15,
        26,
        true,
        &fin_uv,
    )
}

fn oms_pod() -> Mesh {
    pod(
        OMS_HALF,
        2.4,
        OMS_Z.0,
        OMS_Z.1,
        &oms_radius(),
        44,
        48,
        &oms_uv,
    )
}

fn tip_pod() -> Mesh {
    pod(
        Vec2::splat(TIP_POD_R),
        2.0,
        TIP_POD_Z.0,
        TIP_POD_Z.1,
        &tip_pod_radius(),
        30,
        28,
        &tip_pod_uv,
    )
}

/// Wing atlas part of a texel by its UV (see the island layout in [`super::spaceplane`]).
pub fn wing_part(uv: Vec2) -> u8 {
    let (u, v) = (uv.x, uv.y);
    if u >= 0.95 && v >= 0.95 {
        W_EDGE
    } else if u < 0.305 {
        W_WING_LOWER
    } else if u < 0.612 {
        W_WING_UPPER
    } else if v >= 0.65 && (0.74..0.87).contains(&u) {
        if v < 0.743 {
            W_FLAP_LOWER
        } else {
            W_FLAP_UPPER
        }
    } else if u < 0.82 {
        if v < 0.32 { W_FIN_LEFT } else { W_FIN_RIGHT }
    } else if v < 0.39 {
        W_OMS
    } else {
        W_TIP_POD
    }
}

/// Every mesh the wing atlas covers, in the frames its painter expects.
fn wing_atlas_meshes() -> [Mesh; 9] {
    [
        wing_fixed(),
        elevon(Control::ElevonIn),
        elevon(Control::ElevonOut),
        fin_fixed(),
        rudder(),
        body_flap(&flap_uv),
        oms_pod(),
        tip_pod(),
        edge_patch(),
    ]
}

// ---------------------------------------------------------------- thrusters

/// Where an RCS port sits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PortOn {
    Hull,
    Oms,
    TipPod,
}

/// An RCS thruster's port on the skin: point and outward normal (body frame).
pub fn rcs_port(h: &Hull, t: &Thruster) -> (Vec3, Vec3, PortOn) {
    let e = -t.direction.normalize_or(Vec3::Y);
    let p = t.position;
    let sx = if p.x < 0.0 { -1.0 } else { 1.0 };
    if p.x.abs() > 5.0 {
        let r = TIP_POD_R * tip_pod_radius().at(p.z);
        let y = TIP_POD.y + e.y.signum() * r;
        return (
            Vec3::new(TIP_POD.x * sx, y, p.z),
            Vec3::Y * e.y.signum(),
            PortOn::TipPod,
        );
    }
    let in_oms = (OMS_Z.0 + 2.0..OMS_Z.1 - 0.7).contains(&p.z)
        && (p.x.abs() - OMS_CENTER.x).abs() < OMS_HALF.x * 0.8;
    if e.y > 0.7 && in_oms {
        let (q, n) = oms_top(p.x.abs(), p.z);
        return (
            Vec3::new(q.x * sx, q.y, q.z),
            Vec3::new(n.x * sx, n.y, n.z),
            PortOn::Oms,
        );
    }
    let (q, n) = h.exit(p, e);
    (q, n, PortOn::Hull)
}

/// Where a thruster's gas leaves the model, and its stand-in plume's full length.
#[derive(Debug, Clone, Copy)]
pub struct NozzleExit {
    pub at: Vec3,
    /// The way the gas leaves (unit).
    pub dir: Vec3,
    pub radius: f32,
    pub length: f32,
    pub kind: &'static str,
}

/// Each thruster's nozzle exit, in thruster order: the main bells' rims, the hover wells
/// (a little inside the belly), the RCS ports on the skin.
pub fn nozzle_exits(h: &Hull, thrusters: &[Thruster]) -> Vec<NozzleExit> {
    thrusters
        .iter()
        .map(|t| {
            let exhaust = -t.direction.normalize_or(Vec3::Z);
            let (at, dir, radius, length, kind) = match t.role {
                ThrusterRole::Main => (
                    t.position + exhaust * BELL_EXIT.0,
                    exhaust,
                    BELL_EXIT.1 * 0.95,
                    16.0,
                    "main",
                ),
                ThrusterRole::Hover => {
                    let (p, n) = h.exit(Vec3::new(t.position.x, -1.0, t.position.z), Vec3::NEG_Y);
                    (p - n * 0.2, n, HOVER_R * 0.8, 7.0, "hover")
                }
                ThrusterRole::Rcs => {
                    let (p, n, _) = rcs_port(h, t);
                    (p + n * 0.01, n, 0.1, 1.4, "rcs")
                }
            };
            NozzleExit {
                at,
                dir,
                radius,
                length,
                kind,
            }
        })
        .collect()
}

/// Port plates, hover wells and the like for the painters.
pub fn layout(h: &Hull, thrusters: &[Thruster]) -> Layout {
    let mut lay = Layout {
        hull_ports: Vec::new(),
        oms_ports: Vec::new(),
        tip_ports: Vec::new(),
        hover: Vec::new(),
    };
    for t in thrusters {
        match t.role {
            ThrusterRole::Rcs => {
                let (p, n, on) = rcs_port(h, t);
                let right = |v: Vec3| Vec3::new(v.x.abs(), v.y, v.z);
                let rn = |v: Vec3| Vec3::new(v.x * p.x.signum(), v.y, v.z);
                match on {
                    PortOn::Hull => lay.hull_ports.push((p, n)),
                    PortOn::Oms => lay
                        .oms_ports
                        .push((right(p) - Vec3::new(OMS_CENTER.x, OMS_CENTER.y, 0.0), rn(n))),
                    PortOn::TipPod => lay
                        .tip_ports
                        .push((right(p) - Vec3::new(TIP_POD.x, TIP_POD.y, 0.0), rn(n))),
                }
            }
            ThrusterRole::Hover => lay.hover.push(Vec2::new(t.position.x, t.position.z)),
            ThrusterRole::Main => {}
        }
    }
    lay
}

/// Nested plume shells, outside in: radius at the nozzle and at the end, alpha at the
/// nozzle, alpha fade exponent along the plume. Seen through the middle a ray crosses every
/// shell, at the edge only the outer one, so the glow falls off softly toward the sides.
type Shells = [(f32, f32, f32, f32); 4];
const PLUME_SHELLS: Shells = [
    (1.0, 1.7, 0.10, 1.6),
    (0.8, 1.15, 0.12, 1.25),
    (0.58, 0.55, 0.15, 1.0),
    (0.34, 0.1, 0.22, 0.8),
];
/// Hover plumes: a hot narrow core in a fainter, wider glow (they're mostly seen close to
/// the nozzle, where the ground cuts them off).
const HOVER_SHELLS: Shells = [
    (1.0, 2.2, 0.04, 2.0),
    (0.75, 1.5, 0.06, 1.6),
    (0.5, 0.8, 0.1, 1.2),
    (0.26, 0.12, 0.32, 0.9),
];

/// A unit plume along +Z (radius 1 at the nozzle, length 1): the shells of
/// `shells`, each in its own quarter of the glow texture; alpha in vertex colors.
fn plume_mesh(shells: &Shells) -> Mesh {
    let seg = 24;
    let n = 20;
    let ts: Vec<f32> = (0..=n).map(|i| (i as f32 / n as f32).powf(1.3)).collect();
    let mut out = Mesh::default();
    for (k, &(r0, r1, a0, fade)) in shells.iter().enumerate() {
        let radius = |t: f32| r0 + (r1 - r0) * t.powf(0.8);
        let rows: Vec<Vec<Vec3>> = ts
            .iter()
            .map(|&t| {
                (0..seg)
                    .map(|j| {
                        let a = j as f32 / seg as f32 * TAU;
                        Vec3::new(a.cos() * radius(t), a.sin() * radius(t), t)
                    })
                    .collect()
            })
            .collect();
        let u0 = k as f32 * 0.25;
        let mut m = grid(&rows, true, false, |i, j| {
            Vec2::new(u0 + 0.01 + 0.23 * j as f32 / seg as f32, ts[i])
        });
        for (i, v) in m.vertices.iter_mut().enumerate() {
            let t = ts[i / (seg + 1)];
            v.color = [1.0, 1.0, 1.0, a0 * (1.0 - t).powf(fade)];
        }
        out.merge(&m);
    }
    out
}

/// The plume's glow along its length, one quarter per shell (outside in); the core has
/// shock diamonds.
fn plume_glow() -> Image {
    image_fn(64, 256, true, |u, v| {
        let shell = ((u * 4.0) as usize).min(3);
        let b = match shell {
            0 => 0.35 * (1.0 - v).powf(1.5),
            1 => 0.5 * (1.0 - v).powf(1.2),
            2 => 0.7 * (1.0 - v),
            _ => {
                let diamonds = (v * 8.0 * TAU).cos().max(0.0).powi(6) * smoothstep(0.7, 0.05, v);
                0.85 * (1.0 - v).powf(0.7) + 0.5 * diamonds
            }
        };
        let c = srgb(Vec3::splat(b.min(1.0)));
        [c[0], c[1], c[2], 255]
    })
}

/// Shells of the re-entry wake, outside in: radius where it leaves the ship (1 = the
/// wake's radius) and at its end, opacity, and how fast it cools along its length. Many faint
/// shells read as a soft column of gas instead of a tube. The outer gas glows pink-violet
/// ([`WAKE_COLOR`]); the core, hottest and cooling fastest, orange-white ([`HOT_COLOR`]).
const WAKE_OUTER: [(f32, f32, f32, f32); 6] = [
    (1.0, 2.6, 0.022, 1.8),
    (0.9, 2.3, 0.026, 2.0),
    (0.8, 2.05, 0.03, 2.2),
    (0.7, 1.8, 0.034, 2.4),
    (0.61, 1.55, 0.038, 2.6),
    (0.52, 1.32, 0.042, 2.8),
];
const WAKE_CORE: [(f32, f32, f32, f32); 6] = [
    (0.44, 1.1, 0.04, 3.2),
    (0.37, 0.9, 0.045, 3.6),
    (0.3, 0.72, 0.05, 4.0),
    (0.23, 0.55, 0.06, 4.4),
    (0.17, 0.4, 0.07, 4.8),
    (0.11, 0.26, 0.09, 5.2),
];

/// Streamers peeling off the wingtips, where the shock layer spills round the wing's edge:
/// thin, hot and short-lived.
const STREAMER_SHELLS: [(f32, f32, f32, f32); 4] = [
    (1.0, 3.2, 0.05, 2.0),
    (0.7, 2.3, 0.07, 3.0),
    (0.45, 1.5, 0.1, 4.0),
    (0.25, 0.75, 0.14, 5.0),
];

/// A unit re-entry wake along +Z (radius 1 where it leaves the ship, length 1) from
/// `shells`, drawn as a flat strip across local X that [`crate::visuals::plasma_visuals`]
/// turns about the axis to face the camera. Across the strip each shell adds the glow a
/// column of hot gas of its radius shows side on (densest on the axis, fading smoothly to
/// its edge), so the wake reads as one soft volume with no rim at each shell. It starts at
/// `neck` of its radius right behind the ship (the air closing in behind the body) and widens
/// like a cone. It fades in over its first few percent, then out as each shell cools, and is
/// streaked along its length like the turbulent gas it is.
fn wake_mesh(shells: &[(f32, f32, f32, f32)], cols: usize, rows: usize, neck: f32) -> Mesh {
    let ts: Vec<f32> = (0..=rows)
        .map(|i| (i as f32 / rows as f32).powf(1.6))
        .collect();
    let radius = |r0: f32, r1: f32, t: f32| r0 * neck + (r1 - r0 * neck) * t.powf(0.7);
    // Out to where the widest shell's glow has faded.
    let half = |t: f32| {
        shells
            .iter()
            .map(|s| radius(s.0, s.1, t))
            .fold(0.0, f32::max)
            * 1.3
    };
    let strip: Vec<Vec<Vec3>> = ts
        .iter()
        .map(|&t| {
            (0..=cols)
                .map(|j| Vec3::new((j as f32 / cols as f32 * 2.0 - 1.0) * half(t), 0.0, t))
                .collect()
        })
        .collect();
    let mut m = grid(&strip, false, false, |i, j| {
        Vec2::new(j as f32 / cols as f32, ts[i])
    });
    for v in m.vertices.iter_mut() {
        let (x, t) = (v.position[0], v.position[2]);
        let mut a = 0.0;
        for (k, &(r0, r1, a0, cooling)) in shells.iter().enumerate() {
            let phase = k as f32 * 1.9;
            let u = x / radius(r0, r1, t);
            let streaks = 0.6
                + 0.22 * (9.0 * u + 7.0 * t + phase).sin()
                + 0.18 * (23.0 * u - 13.0 * t + 2.0 * phase).sin();
            let glow = smoothstep(0.0, 0.03, t) * (-cooling * t).exp() * (1.0 - t);
            // A shell seen side on: twice its own opacity on the axis (front and back).
            a += 2.0 * a0 * 1.6 * glow * streaks * (-2.2 * u * u).exp();
        }
        v.color = [1.0, 1.0, 1.0, a.min(1.0)];
    }
    m
}

/// Where the nose cap is, on the body's long axis, m.
const NOSE_Z: f32 = -10.8;
/// The angle of attack the shock layer is drawn for: the one the entry flies.
const SHEATH_AOA_DEG: f32 = 40.0;
/// Shock-layer shells over the skin, inside out: standoff from the skin facing straight
/// into the flow (m) and opacity there. Faces turned away from the flow get none. The gas
/// nearest the skin is the hottest, orange-white; farther out it glows pink.
const SHEATH_HOT: [(f32, f32); 3] = [(0.07, 0.085), (0.2, 0.075), (0.38, 0.065)];
const SHEATH_OUTER: [(f32, f32); 3] = [(0.62, 0.05), (0.92, 0.036), (1.3, 0.022)];

/// Cell size the shock layer's skin is simplified to, m: a soft glow needs no detail.
const SHEATH_CELL: f32 = 0.6;

/// `mesh` simplified by merging its vertices in cubes of `cell` meters (apart where their
/// normals point different ways, so a thin wing's two sides stay apart), dropping the
/// triangles that collapse.
fn coarse(mesh: &Mesh, cell: f32) -> Mesh {
    let mut cells: HashMap<[i32; 6], u32> = HashMap::new();
    let mut sums: Vec<(Vec3, Vec3, u32)> = Vec::new();
    let remap: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            let c = (p / cell).floor().as_ivec3();
            let d = (n * 1.4).round().as_ivec3();
            let k = *cells
                .entry([c.x, c.y, c.z, d.x, d.y, d.z])
                .or_insert_with(|| {
                    sums.push((Vec3::ZERO, Vec3::ZERO, 0));
                    sums.len() as u32 - 1
                });
            let s = &mut sums[k as usize];
            s.0 += p;
            s.1 += n;
            s.2 += 1;
            k
        })
        .collect();
    let vertices = sums
        .iter()
        .map(|&(p, n, count)| Vertex::new(p / count as f32, n.normalize_or(Vec3::Y), Vec2::ZERO))
        .collect();
    let mut indices = Vec::new();
    for tri in mesh.indices.as_chunks::<3>().0 {
        let t = tri.map(|i| remap[i as usize]);
        if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
            indices.extend_from_slice(&t);
        }
    }
    Mesh::new(vertices, indices)
}

/// The shock layer of a re-entry: shells over `skins` pushed out along their normals,
/// thickest where the skin faces into the airflow at [`SHEATH_AOA_DEG`]. It glows brightest
/// where the heating is: at the nose cap and the leading edges, fading aft along the belly
/// as the boundary layer thickens (heating falls off as 1/√distance from the nose, and the
/// gas's glow faster still).
fn sheath_mesh(skins: &[&Mesh], shells: &[(f32, f32)]) -> Mesh {
    let a = SHEATH_AOA_DEG.to_radians();
    // The way the ship moves through the air, in its own axes.
    let into = Vec3::new(0.0, -a.sin(), -a.cos());
    let mut out = Mesh::default();
    let skins: Vec<Mesh> = skins.iter().map(|m| coarse(m, SHEATH_CELL)).collect();
    for &(standoff, opacity) in shells {
        for skin in &skins {
            let mut used = vec![u32::MAX; skin.vertices.len()];
            let mut vertices: Vec<Vertex> = Vec::new();
            let mut indices = Vec::new();
            for tri in skin.indices.as_chunks::<3>().0 {
                let facing = |i: u32| Vec3::from(skin.vertices[i as usize].normal).dot(into);
                if tri.iter().all(|&i| facing(i) <= 0.02) {
                    continue;
                }
                for &i in tri {
                    if used[i as usize] == u32::MAX {
                        used[i as usize] = vertices.len() as u32;
                        let mut v = skin.vertices[i as usize];
                        let n = Vec3::from(v.normal);
                        let f = facing(i).max(0.0);
                        let aft = (v.position[2] - NOSE_Z).max(0.0);
                        let heat = (2.0 / (2.0 + aft)).max((-n.z).max(0.0).powi(2));
                        v.position =
                            (Vec3::from(v.position) + n * standoff * (0.3 + 0.7 * f)).to_array();
                        v.color = [1.0, 1.0, 1.0, opacity * f * f * heat];
                        vertices.push(v);
                    }
                    indices.push(used[i as usize]);
                }
            }
            out.merge(&Mesh::new(vertices, indices));
        }
    }
    out
}

// ---------------------------------------------------------------- materials

fn mat(hex_color: u32, rough: f32, metal: f32) -> StandardMaterial {
    StandardMaterial::hex(hex_color)
        .with_roughness(rough)
        .with_metallic(metal)
}

fn textured(rgb: [f32; 3], rough: f32, metal: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        perceptual_roughness: rough,
        metallic: metal,
        ..Default::default()
    }
}

fn plume_material(color: [f32; 3], strength: f32) -> StandardMaterial {
    let mut m = StandardMaterial::color(0.0, 0.0, 0.0).with_emissive(color, strength);
    m.unlit = true;
    m.alpha_mode = AlphaMode::Blend;
    m.receives_wetness = false;
    m.double_sided = true;
    m
}

// ---------------------------------------------------------------- build

/// Fixed parts: (mesh, material, casts shadows, glows with heat).
/// Shares of the heat-shield (nose) heating above ambient the rest of the skin gets,
/// re-entering nose high: the shock layer is hottest at the nose cap and leading edges,
/// a little cooler along the belly, and the top, in the lee, runs far cooler (a shuttle's
/// nose 1 600 K, belly 1 300 to 1 500 K, upper tiles 600 to 900 K).
const BELLY_HEAT: f32 = 0.85;
const LEEWARD_HEAT: f32 = 0.45;

/// Heat share for a part by its mesh name's suffix (see [`split_windward`]).
fn heat_share(mesh: &str) -> f32 {
    if mesh.ends_with("_leeward") {
        LEEWARD_HEAT
    } else if mesh.ends_with("_windward") {
        BELLY_HEAT
    } else {
        1.0
    }
}

/// The faces of `mesh` facing forward (nose cap, leading edges), facing down (the belly)
/// and the rest, as three meshes on the same vertices.
fn split_windward(mesh: &Mesh) -> (Mesh, Mesh, Mesh) {
    let (mut nose, mut windward, mut leeward) = (Vec::new(), Vec::new(), Vec::new());
    for tri in mesh.indices.as_chunks::<3>().0 {
        let n = tri
            .iter()
            .map(|&i| Vec3::from(mesh.vertices[i as usize].normal))
            .sum::<Vec3>()
            .normalize_or_zero();
        let side = if n.z < -0.6 {
            &mut nose
        } else if n.y < -0.15 {
            &mut windward
        } else {
            &mut leeward
        };
        side.extend_from_slice(tri);
    }
    let part = |indices| Mesh::new(mesh.vertices.clone(), indices);
    (part(nose), part(windward), part(leeward))
}

const FIXED: &[(&str, &str, bool, bool)] = &[
    ("hull_nose", "hull", true, true),
    ("hull_windward", "hull", true, true),
    ("hull_leeward", "hull", true, true),
    ("base", "base", true, false),
    ("wing_nose", "wing", true, true),
    ("wing_windward", "wing", true, true),
    ("wing_leeward", "wing", true, true),
    ("bells", "bell", true, false),
    ("bell_inside", "bell_inside", true, false),
    ("engine_hw", "engine_hw", true, false),
    ("hover_metal", "rcs", true, false),
    ("hover_dark", "dark", true, false),
    ("rcs_body", "rcs", true, false),
    ("rcs_inside", "black", false, false),
    ("frames", "frame", true, false),
    ("hatch", "metal", true, false),
    ("dock", "gear", true, false),
    ("dock_hatch", "frame", true, false),
    ("dock_metal", "metal", true, false),
];

/// Every mesh, texture and material of the spaceplane.
pub fn build() -> ModelAssets {
    let mut out = ModelAssets::default();
    let h = Hull::new();
    let thrusters = crate::presets::spaceplane_thrusters();
    let lay = layout(&h, &thrusters);

    // ---- hull, cut open at the hover engines
    let holes: Vec<(f32, f32, f32)> = lay
        .hover
        .iter()
        .map(|c| (c.x, c.y, HOVER_R + 0.03))
        .collect();
    let hull = hull_mesh(&h, &holes);
    let mut atlas = Atlas::new(HULL_ATLAS.0, HULL_ATLAS.1);
    atlas.raster(&hull, 1);
    out.baked(&key("tex/hull"), paint_hull(&atlas, &h, &lay));
    let (nose, windward, leeward) = split_windward(&hull);
    out.mesh(&key("hull_nose"), nose);
    out.mesh(&key("hull_windward"), windward);
    out.mesh(&key("hull_leeward"), leeward);
    let mut sheath_hot = sheath_mesh(&[&hull], &SHEATH_HOT);
    let mut sheath = sheath_mesh(&[&hull], &SHEATH_OUTER);
    out.mesh(&key("base"), base_mesh(&h));

    // ---- wings, fin, pods, control surfaces (one atlas)
    let mut atlas = Atlas::new(1024, 1024);
    let parts = wing_atlas_meshes();
    for m in &parts {
        atlas.raster_with(m, wing_part);
    }
    out.baked(&key("tex/wing"), paint_wing(&atlas, &lay));
    let [wing, _, _, fin, _, _, oms, tip, _] = parts;
    let oms = transformed(oms, Mat4::from_translation(OMS_CENTER.extend(0.0)));
    let tip = transformed(tip, Mat4::from_translation(TIP_POD));
    let right = merged([transformed(wing, wing_to_body(1.0)), oms, tip]);
    let wing = merged([mirrored_x(&right), right, transformed(fin, fin_to_body())]);
    let (nose, windward, leeward) = split_windward(&wing);
    out.mesh(&key("wing_nose"), nose);
    out.mesh(&key("wing_windward"), windward);
    out.mesh(&key("wing_leeward"), leeward);
    sheath_hot.merge(&sheath_mesh(&[&wing], &SHEATH_HOT));
    sheath.merge(&sheath_mesh(&[&wing], &SHEATH_OUTER));
    out.mesh(&key("plasma_sheath_hot"), sheath_hot);
    out.mesh(&key("plasma_sheath"), sheath);
    for pv in pivots() {
        out.mesh(&key(&pivot_name(&pv)), pivot_mesh(&pv));
    }

    // ---- engines and thrusters
    let mut bells = Mesh::default();
    let mut insides = Mesh::default();
    let mut hardware = Mesh::default();
    let mut hover_metal = Mesh::default();
    let mut hover_dark = Mesh::default();
    let mut rcs_body = Mesh::default();
    let mut rcs_inside = Mesh::default();
    for t in &thrusters {
        match t.role {
            ThrusterRole::Main => {
                let (b, i, hw) = main_engine(t.position);
                bells.merge(&b);
                insides.merge(&i);
                hardware.merge(&hw);
            }
            ThrusterRole::Hover => {
                let (m, d) = hover_engine(&h, t.position.x, t.position.z);
                hover_metal.merge(&m);
                hover_dark.merge(&d);
            }
            ThrusterRole::Rcs => {
                let (p, n, _) = rcs_port(&h, t);
                let (b, i) = rcs_quad(p, n);
                rcs_body.merge(&b);
                rcs_inside.merge(&i);
            }
        }
    }
    out.mesh(&key("bells"), bells);
    out.mesh(&key("bell_inside"), insides);
    out.mesh(&key("engine_hw"), hardware);
    out.mesh(&key("hover_metal"), hover_metal);
    out.mesh(&key("hover_dark"), hover_dark);
    out.mesh(&key("rcs_body"), rcs_body);
    out.mesh(&key("rcs_inside"), rcs_inside);
    out.mesh(&key("plume"), plume_mesh(&PLUME_SHELLS));
    out.mesh(&key("plume_soft"), plume_mesh(&HOVER_SHELLS));
    out.mesh(&key("plasma_wake"), wake_mesh(&WAKE_OUTER, 32, 24, 0.6));
    out.mesh(&key("plasma_wake_hot"), wake_mesh(&WAKE_CORE, 32, 24, 0.6));
    out.mesh(
        &key("plasma_streamer"),
        wake_mesh(&STREAMER_SHELLS, 12, 16, 1.0),
    );

    // ---- details
    out.mesh(&key("frames"), window_frames(&h));
    out.mesh(&key("hatch"), hatch_hardware(&h));
    let (dock, dock_hatch, dock_metal) = docking_port(&h);
    out.mesh(&key("dock"), dock);
    out.mesh(&key("dock_hatch"), dock_hatch);
    out.mesh(&key("dock_metal"), dock_metal);

    // ---- landing gear: legs and doors (folding), tires and hubs (turning)
    // (The whole skins are drawn as their nose, windward and leeward parts.)
    let underside = Underside::new(&[&hull, &wing], Vec2::new(-2.6, -9.0), Vec2::new(2.6, 5.0));
    for (name, mesh) in super::ship_gear::meshes(&|x, z| underside.at(x, z)) {
        out.mesh(&key(name), mesh);
    }
    for (name, (r, w, gap)) in [("nose", NOSE_TIRES), ("main", MAIN_TIRES)] {
        let (tires, hubs) = twin_tires(r, w, gap);
        out.mesh(&key(&format!("tires_{name}")), tires);
        out.mesh(&key(&format!("hubs_{name}")), hubs);
    }

    // ---- textures
    out.image(&key("tex/bell/normal"), bell_normal());
    let (tc, tn, to) = tile_set();
    out.image(&key("tex/tiles/color"), tc);
    out.image(&key("tex/tiles/normal"), tn);
    out.image(&key("tex/tiles/orm"), to);
    out.image(&key("tex/base/normal"), base_quilt());
    let (tc, tn, to) = super::plane_paint::tire_textures();
    out.image(&key("tex/tire/color"), tc);
    out.image(&key("tex/tire/normal"), tn);
    out.image(&key("tex/tire/orm"), to);
    out.image(&key("tex/plume/glow"), plume_glow());

    // ---- materials
    let k = |n: &str| key(&format!("mat/{n}"));
    for name in ["hull", "wing"] {
        let mut m = textured([1.0; 3], 1.0, 1.0);
        m.reflectance = 0.5;
        let mut def = MaterialDef::baked(&k(name), m, &key(&format!("tex/{name}")));
        def.glow = Some(key(&format!("tex/{name}/glow")));
        out.material(def);
    }
    let mut base = MaterialDef::plain(&k("base"), mat(0x2b2b2d, 0.85, 0.0));
    base.normal = Some(key("tex/base/normal"));
    out.material(base);
    let mut bell = MaterialDef::plain(&k("bell"), textured([0.44, 0.42, 0.40], 0.52, 0.85));
    bell.normal = Some(key("tex/bell/normal"));
    out.material(bell);
    let mut inside =
        MaterialDef::plain(&k("bell_inside"), textured([0.075, 0.07, 0.065], 0.6, 0.6));
    inside.normal = Some(key("tex/bell/normal"));
    out.material(inside);
    out.material(MaterialDef::plain(
        &k("engine_hw"),
        textured([0.56, 0.56, 0.58], 0.4, 0.9),
    ));
    out.material(MaterialDef::plain(&k("metal"), mat(0x9a9ea4, 0.38, 1.0)));
    out.material(MaterialDef::plain(
        &k("dark"),
        textured([0.07, 0.07, 0.075], 0.7, 0.2),
    ));
    out.material(MaterialDef::plain(&k("rcs"), mat(0x2e2f31, 0.5, 0.6)));
    out.material(MaterialDef::plain(&k("black"), mat(0x050505, 0.8, 0.0)));
    out.material(MaterialDef::plain(&k("frame"), mat(0x1c1d20, 0.5, 0.0)));
    out.material(MaterialDef::plain(&k("gear"), mat(0xb4b7bb, 0.4, 0.7)));
    out.material(MaterialDef::plain(&k("chrome"), mat(0xd2d5da, 0.15, 1.0)));
    let mut door = MaterialDef::baked(
        &k("door"),
        {
            let mut m = textured([1.0; 3], 1.0, 1.0);
            m.uv_scale = [1.0 / 0.6, 1.0 / 0.6];
            m
        },
        &key("tex/tiles"),
    );
    door.glow = None;
    out.material(door);
    out.material(MaterialDef::baked(
        &k("tire"),
        textured([1.0; 3], 1.0, 1.0),
        &key("tex/tire"),
    ));
    out.material(MaterialDef::plain(&k("hub"), mat(0x8a8e93, 0.55, 0.6)));
    for (name, color, strength) in [
        ("plume_main", [0.70, 0.82, 1.0], 60_000.0),
        ("plume_hover", [1.0, 0.70, 0.42], 80_000.0),
        ("plume_rcs", [0.9, 0.92, 1.0], 25_000.0),
    ] {
        let mut def = MaterialDef::plain(&k(name), plume_material(color, strength));
        def.glow = Some(key("tex/plume/glow"));
        out.material(def);
    }
    out
}

// ---------------------------------------------------------------- spawn

/// Glowing air of a re-entry: nitrogen and oxygen lines, pink-orange.
const SHEATH_COLOR: [f32; 3] = [1.0, 0.4, 0.66];
const WAKE_COLOR: [f32; 3] = [0.92, 0.4, 0.72];
/// The hottest gas, nearest the skin and in the wake's core: orange-white.
const HOT_COLOR: [f32; 3] = [1.0, 0.68, 0.42];
/// Where the right wingtip's streamer peels off (the left one is its mirror), m.
const WINGTIP_EDGE: Vec3 = Vec3::new(7.1, -0.75, 9.6);
/// Half extents of the hull and wings (span, height, length), m.
const SKIN_HALF: Vec3 = Vec3::new(7.2, 2.6, 10.8);

fn renderer(mesh: &str, material: &str, shadows: bool) -> MeshRenderer {
    let mut r = MeshRenderer::new(
        MeshRef::Asset(key(mesh)),
        MaterialRef::Asset(key(&format!("mat/{material}"))),
    );
    r.cast_shadows = shadows;
    r
}

fn spawn_part(
    commands: &mut Commands,
    parent: Entity,
    mesh: &str,
    material: &str,
    transform: Transform,
    shadows: bool,
) -> Entity {
    commands
        .spawn((
            transform,
            renderer(mesh, material, shadows),
            ChildOf(parent),
        ))
        .id()
}

/// Spawn the spaceplane's drawn parts under `body` (the vehicle carrying `aircraft`,
/// `wheels` and a spacecraft with `thrusters`); the model's assets are built the first time.
pub fn spawn(
    commands: &mut Commands,
    body: Entity,
    aircraft: &Aircraft,
    wheels: &Wheels,
    thrusters: &[Thruster],
) {
    commands.queue(|world: &mut World| ensure(world, &key("hull_nose"), build));
    // Parts that glow with re-entry heat, by material.
    let mut glowing: Vec<(Entity, &'static str, f32)> = Vec::new();
    for &(mesh, material, shadows, glows) in FIXED {
        let e = spawn_part(
            commands,
            body,
            mesh,
            material,
            Transform::default(),
            shadows,
        );
        if glows {
            glowing.push((e, material, heat_share(mesh)));
        }
    }
    // Control surfaces: the elevons and body-flap halves follow their side's delta wing,
    // the rudder the fin. The pivot draws the surface itself, so it can glow too (heat
    // glow looks at the drawn entity's parent).
    for pv in pivots() {
        let index = aircraft.surfaces.iter().position(|s| {
            if pv.control == Control::Rudder {
                s.name.contains("fin")
            } else {
                !s.name.contains("fin") && (s.position.x < 0.0) == (pv.side < 0.0)
            }
        });
        let Some(index) = index else { continue };
        let e = commands
            .spawn((
                Transform::from_translation(pv.at.as_dvec3()).with_rotation(pv.base),
                SurfaceVisual {
                    index,
                    hinge: pv.hinge,
                    base: pv.base,
                },
                renderer(&pivot_name(&pv), "wing", true),
                ChildOf(body),
            ))
            .id();
        glowing.push((e, "wing", BELLY_HEAT));
    }
    // Landing gear: legs and doors fold with the gear position.
    let h = Hull::new();
    for (mesh, material, fold) in ship_gear::parts(&h) {
        let turn = fold.rotation(aircraft.gear);
        commands.spawn((
            Transform::from_translation((fold.pivot - turn * fold.pivot).as_dvec3())
                .with_rotation(turn),
            renderer(mesh, material, true),
            fold,
            ChildOf(body),
        ));
    }
    // Wheels: twin tires on each leg, folding with it, never hanging below its axle.
    let (nose_fold, main_fold) = ship_gear::leg_folds();
    for (i, w) in wheels.wheels.iter().enumerate() {
        let (fold, axle): (GearFold, Vec3) = if w.mount.x.abs() < 0.1 {
            (nose_fold, NOSE_AXLE)
        } else if w.mount.x > 0.0 {
            (main_fold, MAIN_AXLE)
        } else {
            (main_fold.mirrored(), MAIN_AXLE * Vec3::new(-1.0, 1.0, 1.0))
        };
        let mount = w.mount - Vec3::Y * w.suspension.travel_m * 0.6;
        let wheel = commands
            .spawn((
                Transform::from_translation(mount.as_dvec3()),
                WheelVisual {
                    index: i,
                    retracts: false,
                    max_travel: (w.mount.y - axle.y).max(0.0),
                },
                fold,
                ChildOf(body),
            ))
            .id();
        let size = if w.radius > 0.55 { "main" } else { "nose" };
        spawn_part(
            commands,
            wheel,
            &format!("tires_{size}"),
            "tire",
            Transform::default(),
            true,
        );
        spawn_part(
            commands,
            wheel,
            &format!("hubs_{size}"),
            "hub",
            Transform::default(),
            true,
        );
    }
    // Plumes at every thruster's nozzle exit, and the same nozzles for the effects modules.
    let exits = nozzle_exits(&h, thrusters);
    commands.entity(body).insert(Exhausts(
        exits
            .iter()
            .zip(thrusters)
            .map(|(n, t)| Exhaust::new(thruster_kind(t.role), n.at, n.dir, n.radius))
            .collect(),
    ));
    for (
        i,
        &NozzleExit {
            at,
            dir,
            radius,
            length,
            kind,
        },
    ) in exits.iter().enumerate()
    {
        let mesh = if kind == "hover" {
            "plume_soft"
        } else {
            "plume"
        };
        let mut r = renderer(mesh, &format!("plume_{kind}"), false);
        r.visible = false;
        commands.spawn((
            Transform::from_translation(at.as_dvec3())
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, dir))
                .with_scale(Vec3::new(radius, radius, 0.01)),
            r,
            PlumeVisual {
                index: i,
                length,
                radius,
            },
            ChildOf(body),
        ));
    }
    // Re-entry plasma: the shock layer over the windward skin and the wake streaming behind,
    // each an orange-white hot part and a pink outer part, and streamers off the wingtips.
    let wake = PlasmaWake {
        from: None,
        skin: SKIN_HALF,
        radius: 4.5,
        length: 160.0,
    };
    let streamer = |from: Vec3| PlasmaWake {
        from: Some(from),
        radius: 0.7,
        length: 70.0,
        ..wake
    };
    for (mesh, base, wake) in [
        (
            "plasma_sheath_hot",
            plume_material(HOT_COLOR, 70_000.0),
            None,
        ),
        (
            "plasma_sheath",
            plume_material(SHEATH_COLOR, 30_000.0),
            None,
        ),
        (
            "plasma_wake_hot",
            plume_material(HOT_COLOR, 90_000.0),
            Some(wake),
        ),
        (
            "plasma_wake",
            plume_material(WAKE_COLOR, 40_000.0),
            Some(wake),
        ),
        (
            "plasma_streamer",
            plume_material(HOT_COLOR, 60_000.0),
            Some(streamer(WINGTIP_EDGE)),
        ),
        (
            "plasma_streamer",
            plume_material(HOT_COLOR, 60_000.0),
            Some(streamer(WINGTIP_EDGE * Vec3::new(-1.0, 1.0, 1.0))),
        ),
    ] {
        let mut r = MeshRenderer::new(MeshRef::Asset(key(mesh)), MaterialRef::from(base));
        r.cast_shadows = false;
        r.visible = false;
        commands.spawn((
            Transform::default(),
            r,
            PlasmaVisual {
                base,
                level: 0,
                wake,
            },
            ChildOf(body),
        ));
    }
    // Heat glow starts from each part's own material, once the materials exist.
    commands.queue(move |world: &mut World| {
        let Some(materials) = world.get_resource::<Assets<StandardMaterial>>() else {
            return;
        };
        let found: Vec<(Entity, StandardMaterial, f32)> = glowing
            .into_iter()
            .filter_map(|(e, m, share)| {
                let h = materials.by_key(&key(&format!("mat/{m}")))?;
                Some((e, *materials.get(h)?, share))
            })
            .collect();
        for (e, base, share) in found {
            if let Ok(mut entity) = world.get_entity_mut(e) {
                entity.insert(HeatGlow {
                    base,
                    level: 0,
                    share,
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wing_atlas_islands_stay_apart() {
        let mut atlas = Atlas::new(1024, 1024);
        for (k, m) in wing_atlas_meshes().iter().enumerate() {
            let mut lo = Vec2::splat(f32::MAX);
            let mut hi = Vec2::splat(f32::MIN);
            for v in &m.vertices {
                lo = lo.min(Vec2::from(v.uv));
                hi = hi.max(Vec2::from(v.uv));
            }
            eprintln!("wing atlas mesh {k}: uv {lo:?}..{hi:?}");
            assert!(
                lo.cmpge(Vec2::ZERO).all() && hi.cmple(Vec2::ONE).all(),
                "mesh {k}"
            );
            // Each mesh's texels must land in the part its UVs claim.
            atlas.raster_with(m, |_| 100 + k as u8);
        }
        eprintln!("wing atlas: {} overlapping texels", atlas.overlaps);
        assert!(atlas.overlaps < 40, "{} overlapping texels", atlas.overlaps);
        let h = Hull::new();
        let mut atlas = Atlas::new(HULL_ATLAS.0, HULL_ATLAS.1);
        atlas.raster(&hull_mesh(&h, &[]), 1);
        let covered = atlas.part.iter().filter(|&&p| p != 0).count();
        eprintln!("hull atlas: {covered} texels covered");
        assert!(covered > HULL_ATLAS.0 * HULL_ATLAS.1 / 2);
    }

    #[test]
    fn rcs_ports_sit_on_the_skin() {
        let h = Hull::new();
        for t in crate::presets::spaceplane_thrusters() {
            if t.role != ThrusterRole::Rcs {
                continue;
            }
            let (p, n, on) = rcs_port(&h, &t);
            let e = -t.direction.normalize();
            eprintln!("{}: {on:?} at {p:?}, normal {n:?}", t.name);
            assert!(n.dot(e) > 0.3, "{} faces away from its exhaust", t.name);
            assert!(
                (p - t.position).length() < 1.2,
                "{} port far from thruster",
                t.name
            );
        }
    }
}

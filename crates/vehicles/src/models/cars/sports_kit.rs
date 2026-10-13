//! Every mesh and texture of the sports car: the sculpted body split into paint, trim, liners
//! and lamp housings; glass; lights; grille inserts; carbon aero parts; exhausts; mirrors,
//! wipers, handles and badges; the interior; wheels.

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Image, Mesh, Vertex};

use super::atlas::{Atlas, View};
use super::details::{
    Proj, Strip, draped_slab, honeycomb, sculpt, surface_point, surface_polyline, torus, tube_on,
};
use super::mesher::{self, Assign, split, surface_nets, vtx};
use super::parts::{
    self, draped_grid, lathe_x, loft_rings, merge, mirror_x, orient_like_normals, transformed, trs,
};
use super::sdf::{self, Curve, rmax, sd_capsule, sd_ellipsoid, sd_round_box, smin, sstep};
use super::sports::{self, Body, GROUND, Windows};
use super::tex::{self, Canvas};
use super::{FRIT_RANGE, Kit, frit_texture, gather, key, wheel_meshes};

pub const PREFIX: &str = "vehicles/cars/sports";

/// The body atlas: 2048², about 2.4 mm per pixel.
pub fn atlas() -> Atlas {
    Atlas::new(2048, -2.42, 2.42, 1.0, 1.32, GROUND)
}

/// Seat positions (x of each seat center, z of the seat origin), seat-floor height above
/// the ground.
pub const SEATS: [f32; 2] = [-0.37, 0.37];
pub const SEAT_Z: f32 = 0.52;
pub const SEAT_FLOOR: f32 = 0.2;
/// Steering wheel center (driver on the left) and its axis (toward the driver).
pub const WHEEL_CENTER: Vec3 = Vec3::new(-0.37, 0.76 - GROUND, 0.10);

pub fn steering_axis() -> Vec3 {
    Vec3::new(0.0, 0.42, 0.91).normalize()
}

/// Build the sports car's kit: the body skin, the paint atlas and the other parts on their
/// own threads.
pub fn build() -> Kit {
    let body = Body::new();
    let win = body.windows();
    let atlas = atlas();
    let (body, win, atlas) = (&body, &win, &atlas);
    gather(vec![
        ("skin", Box::new(move || skin(body, win, atlas))),
        (
            "paint atlas",
            Box::new(move || {
                let (color, normal) = paint_atlas(body, atlas);
                Kit {
                    meshes: Vec::new(),
                    images: vec![
                        (key(PREFIX, "atlas_color"), color),
                        (key(PREFIX, "atlas_normal"), normal),
                    ],
                }
            }),
        ),
        ("glass, cabin", Box::new(move || glass_and_cabin(body, win))),
        (
            "exterior parts",
            Box::new(move || exterior_parts(body, atlas)),
        ),
        ("interior, wheels", Box::new(interior_parts)),
    ])
}

/// The body skin: painted panels (atlas UVs), liners, lamp housings, recesses and trim.
fn skin(body: &Body, win: &Windows, atlas: &Atlas) -> Kit {
    let shell = |p: Vec3| body.shell(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Body skin -----------------------------------------------------------------------
    let raw = surface_nets(
        &shell,
        Vec3::new(-1.05, -GROUND - 0.02, -2.42),
        Vec3::new(1.05, 0.86, 2.42),
        0.028,
        true,
        1.3,
    );
    let raw = mesher::refine(&raw, &shell, 2, 24.0, 0.0004);
    let window = |p: Vec3| body.window_field(win, p, 0.0, 0.0);
    let pocket = |p: Vec3| body.pocket_field(p);
    let liner = |p: Vec3| body.liner_field(p);
    let trim = |p: Vec3| body.trim_field(p);
    let raw = mesher::decimate(&raw, 0.0006, 8.0, 0.25, true, |p| {
        let (w, l, t, k) = (window(p), liner(p), trim(p), pocket(p));
        if w.abs() < 0.015 || l.abs() < 0.015 || t.abs() < 0.012 || k.abs() < 0.012 {
            0.0
        } else if l < 0.0 || (k < 0.0 && body.features.at(p) != Some(sports::Feature::Headlight)) {
            // Wheel-well liners and pocket floors: mostly hidden (not the headlight housings,
            // seen through their lenses).
            8.0
        } else {
            1.0
        }
    });
    let cm = mesher::cut(&raw, &[&window, &pocket, &liner, &trim]);
    let meshes = split(
        &cm.raw,
        5,
        |c, n, t| {
            if cm.side(0, t) < 0.0 {
                return None;
            }
            let mode = View::of(n) as u8;
            if cm.side(1, t) < 0.0 {
                let mesh = match body.features.at(c) {
                    Some(sports::Feature::Headlight) => 2,
                    _ => 3,
                };
                return Some(Assign { mesh, mode: 0 });
            }
            if cm.side(2, t) < 0.0 {
                return Some(Assign { mesh: 1, mode: 0 });
            }
            if cm.side(3, t) < 0.0 {
                return Some(Assign { mesh: 4, mode });
            }
            Some(Assign { mesh: 0, mode })
        },
        |a, _, p, n| match a.mesh {
            0 => vtx(p, n, atlas.uv(View::from_mode(a.mode), p), 1.0),
            4 => vtx(p, n, metric_uv(View::from_mode(a.mode), p), 1.0),
            _ => vtx(p, n, Vec2::new(p.z, -p.y), 1.0),
        },
    );
    let mut it = meshes.into_iter();
    for name in ["paint", "liner", "lamp_housing", "recess", "trim"] {
        kit.meshes.push((k(name), it.next().unwrap()));
    }
    kit
}

/// Glass and the cabin shell.
fn glass_and_cabin(body: &Body, win: &Windows) -> Kit {
    let shell = |p: Vec3| body.shell(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Glass ----------------------------------------------------------------------------
    let pane_mesh = |pane: &sports::Pane, ns: usize, nt: usize| {
        draped_grid(
            &shell,
            ns,
            nt,
            |s, t| pane.ray(s, t),
            0.002,
            |s, t, _| {
                let d = -pane.sd(pane.point(s, t));
                Vec2::new((d / pane.frit / FRIT_RANGE).clamp(0.0, 0.999), 0.5)
            },
        )
    };
    kit.meshes
        .push((k("glass_front"), pane_mesh(&win.windshield, 32, 18)));
    kit.meshes
        .push((k("glass_back"), pane_mesh(&win.back, 32, 18)));
    let side = pane_mesh(&win.side, 64, 10);
    kit.meshes.push((k("glass_left"), mirror_x(&side)));
    kit.meshes.push((k("glass_right"), side));
    kit.images.push((k("frit"), frit_texture()));

    // --- Cabin shell (the inside of the skin, 4 cm in) ---------------------------------------
    let cabin = |p: Vec3| {
        let h = p.y + GROUND;
        (shell(p) + 0.04)
            .max(-0.42 - p.z)
            .max(p.z - 1.75)
            .max(SEAT_FLOOR - h)
    };
    let mut raw = surface_nets(
        &cabin,
        Vec3::new(-1.0, -0.3, -0.5),
        Vec3::new(1.0, 0.85, 1.8),
        0.042,
        true,
        1.3,
    );
    mesher::flip(&mut raw);
    let inner_window = |p: Vec3| body.window_field(win, p, 0.04, 0.0);
    let raw = mesher::decimate(&raw, 0.001, 10.0, 0.3, true, |p| {
        if inner_window(p).abs() < 0.02 {
            0.0
        } else {
            1.0
        }
    });
    let cm = mesher::cut(&raw, &[&inner_window]);
    let inner = split(
        &cm.raw,
        1,
        |_, _, t| (cm.side(0, t) >= 0.0).then_some(Assign { mesh: 0, mode: 0 }),
        |_, _, p, n| {
            let ao = 0.35 + 0.65 * sstep(-0.25, 0.5, p.y);
            vtx(p, n, Vec2::new(p.z, p.y), ao)
        },
    );
    kit.meshes
        .push((k("cabin"), inner.into_iter().next().unwrap()));
    kit
}

/// Lights, grille, carbon aero, exhausts and trim parts.
fn exterior_parts(body: &Body, atlas: &Atlas) -> Kit {
    let shell = |p: Vec3| body.shell(p);
    let plain = |p: Vec3| body.shell_plain(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Lights ---------------------------------------------------------------------------
    lights(&mut kit, &plain);

    // --- Grille and intakes -----------------------------------------------------------------
    let grille = honeycomb(
        &plain,
        Proj::Front,
        GROUND,
        &body.features.grille,
        0.02,
        0.0034,
        0.022,
        0.035,
    );
    let intake_r = honeycomb(
        &plain,
        Proj::Front,
        GROUND,
        &body.features.intake,
        0.018,
        0.0032,
        0.02,
        0.03,
    );
    let mesh_parts = merge([grille, mirror_x(&intake_r), intake_r]);
    kit.meshes.push((k("grille"), mesh_parts));

    // --- Carbon aero, exhausts --------------------------------------------------------------
    kit.meshes.push((k("carbon"), aero(&plain)));
    kit.meshes.push((k("exhaust"), exhausts(body, &plain)));

    // --- Mirrors, wipers, handles, badges -----------------------------------------------------
    let blank = atlas.uv(View::Bottom, Vec3::ZERO);
    let (mirror_paint, mirror_black, mirror_glass) =
        mirror_at(&shell, Vec2::new(-0.40, 0.845), GROUND, blank);
    kit.meshes.push((
        k("mirror_paint"),
        merge([mirror_x(&mirror_paint), mirror_paint]),
    ));
    kit.meshes.push((
        k("mirror_black"),
        merge([mirror_x(&mirror_black), mirror_black]),
    ));
    kit.meshes.push((
        k("mirror_glass"),
        merge([mirror_x(&mirror_glass), mirror_glass]),
    ));
    kit.meshes.push((k("wipers"), wipers(&shell)));
    let handle = door_handle(&shell);
    kit.meshes
        .push((k("handles"), merge([mirror_x(&handle), handle])));
    let (badge_chrome, badge_enamel) = badges_at(
        &shell,
        &plain,
        GROUND,
        &[
            (Proj::Front, Vec2::new(0.0, 0.50)),
            (Proj::Back, Vec2::new(0.0, 0.832)),
        ],
        Some(("VELARO", Vec2::new(0.30, 0.818))),
    );
    kit.meshes.push((k("chrome"), badge_chrome));
    kit.meshes.push((k("enamel"), badge_enamel));
    kit
}

/// Seats, dashboard, steering wheel and the wheels.
fn interior_parts() -> Kit {
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Interior --------------------------------------------------------------------------
    kit.meshes.push((k("seat"), seat()));
    let (dash, screens) = dashboard();
    kit.meshes.push((k("dash"), dash));
    kit.meshes.push((k("screens"), screens));
    kit.meshes.push((
        k("steering"),
        steering_wheel_at(WHEEL_CENTER, steering_axis(), 0.175, 0.0),
    ));
    kit.images.push((k("screen"), screen_texture()));

    // --- Wheels ----------------------------------------------------------------------------
    wheel_meshes(&mut kit, PREFIX, &super::SPORTS_WHEEL, 6);
    kit
}

/// World-scale planar UVs (meters) for tiling trim textures.
pub(super) fn metric_uv(view: View, p: Vec3) -> Vec2 {
    match view {
        View::Side => Vec2::new(p.z, -p.y),
        View::Top | View::Bottom => Vec2::new(p.x, p.z),
        View::Front | View::Back => Vec2::new(p.x, -p.y),
    }
}

pub(super) fn pts(points: &[(f32, f32)]) -> Vec<Vec2> {
    points.iter().map(|&(a, b)| Vec2::new(a, b)).collect()
}

pub(super) fn closed(mut v: Vec<Vec2>) -> Vec<Vec2> {
    v.push(v[0]);
    v
}

/// Paint atlas: white with dark shut lines (base color multiplier) and a normal map with the
/// lines' grooves and a faint orange peel.
fn paint_atlas(body: &Body, atlas: &Atlas) -> (Image, Image) {
    let shell = |p: Vec3| body.shell(p);
    let mut cv = Canvas::new(atlas.size, atlas.size);
    let reach = 4.0;
    let mut lines: Vec<(Proj, Vec<Vec2>)> = Vec::new();
    // Door: front edge behind the fender vent, sill, rear edge ahead of the haunch, and the
    // top along the beltline seal.
    let mut door = pts(&[
        (-0.54, 0.80),
        (-0.556, 0.62),
        (-0.56, 0.42),
        (-0.545, 0.285),
        (-0.505, 0.245),
        (0.55, 0.245),
        (0.60, 0.26),
        (0.645, 0.38),
        (0.69, 0.58),
        (0.73, 0.78),
    ]);
    let n = 24;
    for i in 0..=n {
        let z = 0.73 + (-0.54 - 0.73) * i as f32 / n as f32;
        door.push(Vec2::new(z, body.belt_at(z) + 0.006));
    }
    door.push(Vec2::new(-0.54, 0.80));
    lines.push((Proj::Side, door));
    // Door handle recess.
    let mut handle = Vec::new();
    for i in 0..=24 {
        let a = i as f32 / 24.0 * std::f32::consts::TAU;
        handle.push(Vec2::new(0.47 + 0.095 * a.cos(), 0.808 + 0.022 * a.sin()));
    }
    lines.push((Proj::Side, handle));
    // Front bumper seam: from below the headlight's outer corner down to the arch.
    lines.push((
        Proj::Side,
        pts(&[(-2.0, 0.52), (-1.93, 0.44), (-1.82, 0.40), (-1.74, 0.36)]),
    ));
    // Hood: clamshell between the fender crests, from the cowl to above the grille.
    lines.push((
        Proj::Top,
        closed(pts(&[
            (-0.58, -0.33),
            (0.58, -0.33),
            (0.605, -0.9),
            (0.585, -1.5),
            (0.53, -1.93),
            (0.40, -2.04),
            (0.0, -2.09),
            (-0.40, -2.04),
            (-0.53, -1.93),
            (-0.585, -1.5),
            (-0.605, -0.9),
        ])),
    ));
    // Trunk lid: deck behind the backlight, down the tail to the light bar.
    lines.push((
        Proj::Top,
        pts(&[
            (0.63, 2.32),
            (0.63, 1.95),
            (0.60, 1.85),
            (0.0, 1.83),
            (-0.60, 1.85),
            (-0.63, 1.95),
            (-0.63, 2.32),
        ]),
    ));
    lines.push((
        Proj::Back,
        pts(&[
            (0.63, 0.975),
            (0.635, 0.93),
            (0.0, 0.926),
            (-0.635, 0.93),
            (-0.63, 0.975),
        ]),
    ));
    for (proj, line) in &lines {
        let on = surface_polyline(&shell, *proj, line, GROUND, 0.008);
        atlas.stamp(&mut cv, &on, reach, 1);
    }
    let groove = |d: f32| 1.0 - sstep(0.2, 2.4, d);
    // A fine metallic-flake grain everywhere (a 64² tile of random tilts), groove slopes
    // near the lines.
    let flake: Vec<[u8; 4]> = (0..64 * 64)
        .map(|i| {
            let (x, y) = (i % 64, i / 64);
            tex::encode_normal(
                0.012 * (tex::hash(x, y, 1) - 0.5),
                0.012 * (tex::hash(x, y, 2) - 0.5),
            )
        })
        .collect();
    tex::par_image2(atlas.size, atlas.size, (true, false), |x, y| {
        let (x, y) = (x as i32, y as i32);
        let d = cv.at(x, y);
        if d >= 4.0 {
            return ([255; 4], flake[((y & 63) * 64 + (x & 63)) as usize]);
        }
        let fx = 0.012 * (tex::hash(x & 63, y & 63, 1) - 0.5);
        let fy = 0.012 * (tex::hash(x & 63, y & 63, 2) - 0.5);
        let c = tex::srgb(1.0 - 0.94 * (1.0 - sstep(0.4, 1.6, d)));
        let gx = groove(cv.at(x + 1, y)) - groove(cv.at(x - 1, y));
        let gy = groove(cv.at(x, y + 1)) - groove(cv.at(x, y - 1));
        (
            [c, c, c, 255],
            tex::encode_normal(0.45 * gx + fx, -0.45 * gy + fy),
        )
    })
}

/// Headlight clusters (cover lens, projector modules with chrome bowls, LED light guides,
/// amber indicator) and the full-width tail light bar with reverse segments.
fn lights(kit: &mut Kit, plain: &(dyn Fn(Vec3) -> f32 + Sync)) {
    let k = |name: &str| key(PREFIX, name);
    // Top and bottom edges of the headlight outline as smooth curves over |x|.
    let top = Curve::new(&[
        (0.45, 0.575),
        (0.52, 0.61),
        (0.63, 0.626),
        (0.77, 0.627),
        (0.855, 0.603),
        (0.865, 0.57),
    ]);
    let bot = Curve::new(&[
        (0.45, 0.575),
        (0.52, 0.548),
        (0.63, 0.535),
        (0.79, 0.543),
        (0.865, 0.57),
    ]);
    let (x0, x1) = (0.45, 0.865);
    let lo = |s: f32| {
        let x = x0 + (x1 - x0) * s;
        Vec2::new(x, bot.at(x))
    };
    let hi = |s: f32| {
        let x = x0 + (x1 - x0) * s;
        Vec2::new(x, top.at(x))
    };
    let strip = Strip { lo: &lo, hi: &hi };
    let lens = draped_slab(
        plain,
        Proj::Front,
        GROUND,
        &strip,
        28,
        8,
        &|_, _| 0.0008,
        false,
        &|s, t| Vec2::new(s, t),
    );
    kit.meshes
        .push((k("head_lens"), merge([mirror_x(&lens), lens])));
    // Projector modules: chrome reflector bowl, black glass lens, chrome ring.
    let mut chrome = Vec::new();
    let mut glass = Vec::new();
    for (q, r) in [
        (Vec2::new(0.565, 0.582), 0.026f32),
        (Vec2::new(0.695, 0.586), 0.024),
    ] {
        if let Some((c, g)) = lamp_module(plain, q, r, GROUND, 0.02) {
            chrome.push(c);
            glass.push(g);
        }
    }
    let chrome = merge(chrome);
    let glass = merge(glass);
    kit.meshes
        .push((k("head_chrome"), merge([mirror_x(&chrome), chrome])));
    kit.meshes
        .push((k("head_projector"), merge([mirror_x(&glass), glass])));
    // Daytime running light: a light guide under the lens along the top edge, hooking down
    // the outer end; plus a short lower guide under the projectors.
    let mut guide = Vec::new();
    for i in 0..=30 {
        let x = 0.475 + (0.835 - 0.475) * i as f32 / 30.0;
        guide.push(Vec2::new(x, top.at(x) - 0.009));
    }
    guide.push(Vec2::new(0.848, 0.586));
    guide.push(Vec2::new(0.846, 0.572));
    let on = surface_polyline(plain, Proj::Front, &guide, GROUND, 0.012);
    let drl_top = tube_on(&on, -0.007, 0.0042, 6);
    let mut lower = Vec::new();
    for i in 0..=12 {
        let x = 0.60 + (0.76 - 0.60) * i as f32 / 12.0;
        lower.push(Vec2::new(x, bot.at(x) + 0.008));
    }
    let on = surface_polyline(plain, Proj::Front, &lower, GROUND, 0.012);
    let drl_low = tube_on(&on, -0.008, 0.003, 6);
    let drl = merge([drl_top, drl_low]);
    kit.meshes.push((k("drl"), merge([mirror_x(&drl), drl])));
    let mut amber = Vec::new();
    for i in 0..=6 {
        let x = 0.79 + (0.852 - 0.79) * i as f32 / 6.0;
        amber.push(Vec2::new(x, bot.at(x) + 0.006));
    }
    let on = surface_polyline(plain, Proj::Front, &amber, GROUND, 0.005);
    let amber = tube_on(&on, -0.008, 0.0035, 6);
    kit.meshes
        .push((k("amber"), merge([mirror_x(&amber), amber])));

    // Tail lights: tall clusters at the corners joined by a slim full-width bar, proud of
    // the tail, with the reverse lamps let into the bar.
    let tail_lo = Curve::new(&[
        (0.0, 0.876),
        (0.56, 0.874),
        (0.64, 0.826),
        (0.80, 0.818),
        (0.875, 0.845),
    ]);
    let tail_hi = Curve::new(&[
        (0.0, 0.906),
        (0.56, 0.906),
        (0.64, 0.918),
        (0.80, 0.921),
        (0.875, 0.918),
    ]);
    let segment = |xa: f32, xb: f32, ns: usize| {
        let lo = |s: f32| {
            let x = xa + (xb - xa) * s;
            Vec2::new(x, tail_lo.at(x.abs()))
        };
        let hi = |s: f32| {
            let x = xa + (xb - xa) * s;
            Vec2::new(x, tail_hi.at(x.abs()))
        };
        let strip = Strip { lo: &lo, hi: &hi };
        draped_slab(
            plain,
            Proj::Back,
            GROUND,
            &strip,
            ns,
            4,
            &|_, t| 0.004 + 0.003 * (1.0 - (2.0 * t - 1.0).powi(2)),
            true,
            &|s, t| Vec2::new(s * (xb - xa) * 8.0, t),
        )
    };
    let red = merge([
        segment(-0.875, -0.43, 32),
        segment(-0.27, 0.27, 24),
        segment(0.43, 0.875, 32),
    ]);
    kit.meshes.push((k("tail_red"), red));
    kit.meshes.push((
        k("tail_reverse"),
        merge([segment(-0.43, -0.27, 10), segment(0.27, 0.43, 10)]),
    ));
    kit.images.push((k("tail_glow"), tail_texture()));
}

/// A projector module behind a cover lens at front-view point `q` (|x|, h): chrome reflector
/// bowl and ring, and the black glass dome (radius `r`), its face `back` meters behind the
/// lens, looking forward: (chrome, glass).
pub(super) fn lamp_module(
    plain: &(dyn Fn(Vec3) -> f32 + Sync),
    q: Vec2,
    r: f32,
    ground: f32,
    back: f32,
) -> Option<(Mesh, Mesh)> {
    let (p, n) = surface_point(plain, Proj::Front, q, ground)?;
    // Lathe +x → the module's axis, halfway between straight ahead and the face normal.
    let axis = (Vec3::NEG_Z + n.normalize_or(Vec3::NEG_Z)).normalize_or(Vec3::NEG_Z);
    let at = trs(
        p - axis * back,
        Quat::from_rotation_arc(Vec3::X, axis),
        Vec3::ONE,
    );
    let bowl = lathe_x(
        &[
            Vec2::new(-0.03, r * 0.5),
            Vec2::new(-0.022, r * 0.85),
            Vec2::new(-0.012, r + 0.004),
            Vec2::new(0.0, r + 0.012),
            Vec2::new(0.004, r + 0.016),
        ],
        28,
        &[],
    );
    let ring = flipped(lathe_x(
        &[
            Vec2::new(-0.004, r),
            Vec2::new(0.002, r + 0.0015),
            Vec2::new(0.004, r + 0.004),
            Vec2::new(-0.002, r + 0.006),
        ],
        28,
        &[],
    ));
    let dome = lathe_x(
        &[
            Vec2::new(0.012, 0.0),
            Vec2::new(0.010, r * 0.5),
            Vec2::new(0.005, r * 0.85),
            Vec2::new(-0.002, r),
        ],
        28,
        &[],
    );
    Some((transformed(merge([bowl, ring]), at), transformed(dome, at)))
}

/// Light-guide pattern for the tail bar (u along the bar, v across): three bright lines.
pub(super) fn tail_texture() -> Image {
    tex::par_image(64, 64, true, |x, y| {
        let v = (y as f32 + 0.5) / 64.0;
        let line = |c: f32, w: f32| (1.0 - ((v - c).abs() / w).min(1.0)).powi(2);
        let u = (x as f32 + 0.5) / 64.0;
        let cells = 0.85 + 0.15 * (u * std::f32::consts::TAU * 2.0).cos();
        let g = (0.22 + line(0.25, 0.08) + line(0.5, 0.06) + line(0.75, 0.08)).min(1.0) * cells;
        let c = tex::srgb(g);
        [c, c, c, 255]
    })
}

/// Splitter, diffuser fins and the deck lip, in one carbon mesh (planar UVs in meters).
fn aero(plain: &(dyn Fn(Vec3) -> f32 + Sync)) -> Mesh {
    // Splitter: a blade under the nose following the bumper's plan curve.
    let n = 40;
    let mut rings = Vec::new();
    for i in 0..=n {
        let s = i as f32 / n as f32;
        let x = -0.80 + 1.60 * s;
        let Some((p, _)) = surface_point(plain, Proj::Front, Vec2::new(x, 0.21), GROUND) else {
            continue;
        };
        let taper = 1.0 - (x / 0.80).powi(6);
        let zf = p.z - 0.05 * taper - 0.004;
        let zb = p.z + 0.30;
        let (yt, yb) = (0.186 - GROUND, 0.172 - GROUND);
        let e = 0.004;
        rings.push(vec![
            Vec3::new(x, yt, zb),
            Vec3::new(x, yt, zf + e),
            Vec3::new(x, yt - e * 0.6, zf),
            Vec3::new(x, yb + e * 0.6, zf),
            Vec3::new(x, yb, zf + e),
            Vec3::new(x, yb, zb),
        ]);
    }
    let mut splitter = loft_rings(&rings, true);
    // Diffuser fins under the rear: vertical plates hanging from the floor.
    let mut fins = Vec::new();
    for xc in [-0.30f32, -0.15, 0.0, 0.15, 0.30] {
        let mut rings = Vec::new();
        let stations = 10;
        for i in 0..=stations {
            let z = 1.62 + (2.03 - 1.62) * i as f32 / stations as f32;
            // The floor here (search up from below).
            let floor_y = sdf::raycast(plain, Vec3::new(xc, -GROUND - 0.05, z), Vec3::Y, 1.0)
                .map(|p| p.y)
                .unwrap_or(0.15 - GROUND);
            let depth = 0.075 * sstep(1.62, 1.98, z);
            let (top, bot) = (floor_y + 0.02, floor_y - depth - 0.002);
            let t = 0.0025;
            rings.push(vec![
                Vec3::new(xc - t, top, z),
                Vec3::new(xc + t, top, z),
                Vec3::new(xc + t, bot, z),
                Vec3::new(xc - t, bot, z),
            ]);
        }
        fins.push(loft_rings(&rings, true));
    }
    // Deck lip: a carbon ducktail along the trunk's trailing edge.
    // It follows the deck's trailing edge in plan: per column, the last point (searching
    // back) where the deck still faces up.
    let cols = 40;
    let edge: Vec<f32> = (0..=cols)
        .map(|i| {
            let x = -0.66 + 1.32 * i as f32 / cols as f32;
            let mut last = 2.10;
            let mut z = 2.10;
            while z < 2.30 {
                match surface_point(plain, Proj::Top, Vec2::new(x, z), GROUND) {
                    Some((_, n)) if n.y > 0.8 => last = z,
                    _ => break,
                }
                z += 0.005;
            }
            last
        })
        .collect();
    let edge_at = |s: f32| {
        let f = s * cols as f32;
        let i = (f as usize).min(cols - 1);
        edge[i] + (edge[i + 1] - edge[i]) * (f - i as f32)
    };
    let lo = |s: f32| Vec2::new(-0.66 + 1.32 * s, edge_at(s) - 0.10);
    let hi = |s: f32| Vec2::new(-0.66 + 1.32 * s, edge_at(s) - 0.004);
    let strip = Strip { lo: &lo, hi: &hi };
    let lip = draped_slab(
        plain,
        Proj::Top,
        GROUND,
        &strip,
        cols,
        10,
        &|s, t| {
            let ends = sstep(0.0, 0.06, s) * sstep(1.0, 0.94, s);
            0.003 + 0.019 * t * t * ends
        },
        true,
        &|_, _| Vec2::ZERO,
    );
    splitter.merge(&merge(fins));
    splitter.merge(&lip);
    for v in &mut splitter.vertices {
        v.uv = [v.position[0] * 1.0, v.position[2] * 1.0];
    }
    splitter
}

/// Twin exhaust tips set into the rear valance.
fn exhausts(body: &Body, plain: &(dyn Fn(Vec3) -> f32 + Sync)) -> Mesh {
    let (c, r) = body.features.exhaust;
    let ro = r - 0.006;
    let mut parts = Vec::new();
    for side in [-1.0f32, 1.0] {
        let q = Vec2::new(side * c.x, c.y);
        // Find the tail surface around the hole's rim.
        let rim = surface_point(plain, Proj::Back, q + Vec2::new(0.0, r + 0.01), GROUND)
            .map(|(p, _)| p.z)
            .unwrap_or(2.0);
        let prof = [
            Vec2::new(-0.12, ro - 0.004),
            Vec2::new(0.006, ro - 0.004),
            Vec2::new(0.014, ro - 0.001),
            Vec2::new(0.016, ro + 0.002),
            Vec2::new(0.012, ro + 0.004),
            Vec2::new(-0.03, ro + 0.003),
        ];
        let mut tip = lathe_x(&prof, 32, &[]);
        for v in &mut tip.vertices {
            let depth = (-v.position[0]).clamp(0.0, 0.12) / 0.12;
            let inner = v.normal[1] * v.position[1] + v.normal[2] * v.position[2] < 0.0;
            let c = if inner {
                0.55 - 0.5 * depth.sqrt()
            } else {
                1.0
            };
            v.color = [c, c * 0.97, c * 0.94, 1.0];
        }
        // Lathe +x → +z (rearward), centered on the hole, lip 1.2 cm proud of the rim.
        let at = trs(
            Vec3::new(q.x, q.y - GROUND, rim + 0.004),
            Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
            Vec3::ONE,
        );
        parts.push(transformed(tip, at));
    }
    merge(parts)
}

/// Lathe parts traced the other way round.
pub(super) fn flipped(mut m: Mesh) -> Mesh {
    for v in &mut m.vertices {
        v.normal = (-Vec3::from(v.normal)).to_array();
    }
    orient_like_normals(&mut m);
    m
}

/// Right-side door mirror on the skin at side-view point `q` (z, h): (painted housing, black
/// base, mirror glass). The painted housing gets UV `blank` (a plain spot of the paint atlas).
pub(super) fn mirror_at(
    shell: &(dyn Fn(Vec3) -> f32 + Sync),
    q: Vec2,
    ground: f32,
    blank: Vec2,
) -> (Mesh, Mesh, Mesh) {
    let base = surface_point(shell, Proj::Side, q, ground)
        .map(|(p, _)| p)
        .unwrap_or(Vec3::new(0.86, q.y - ground, q.x));
    let hc = Vec3::new(0.14, 0.075, 0.0);
    let field = move |p: Vec3| {
        let q = p - hc;
        // A rounded wedge, taller toward its outer end, with a flat back for the glass.
        let t = ((q.x + 0.11) / 0.22).clamp(0.0, 1.0);
        let sy = 0.8 + 0.2 * t;
        let qs = Vec3::new(q.x, q.y / sy, q.z);
        let block = sd_round_box(
            qs - Vec3::new(0.0, 0.0, -0.012),
            Vec3::new(0.105, 0.047, 0.04),
            0.036,
        );
        let nose = sd_ellipsoid(
            qs - Vec3::new(0.0, 0.0, -0.03),
            Vec3::new(0.104, 0.046, 0.062),
        );
        let housing = smin(block, nose, 0.02) * sy;
        let housing = rmax(housing, q.z - 0.022, 0.008);
        let arm = sd_capsule(
            p,
            Vec3::new(-0.015, 0.006, 0.012),
            Vec3::new(0.065, 0.05, 0.0),
            0.011,
        );
        let foot = sd_round_box(
            p - Vec3::new(-0.004, 0.004, 0.0),
            Vec3::new(0.016, 0.02, 0.05),
            0.01,
        );
        smin(smin(housing, arm, 0.015), foot, 0.008)
    };
    let raw = mesher::surface_nets(
        &field,
        Vec3::new(-0.04, -0.04, -0.1),
        Vec3::new(0.26, 0.16, 0.07),
        0.009,
        false,
        1.2,
    );
    let raw = mesher::refine(&raw, &field, 1, 28.0, 0.0003);
    let raw = mesher::decimate(&raw, 0.0005, 10.0, 0.1, false, |_| 1.0);
    let meshes = split(
        &raw,
        2,
        |c, _, _| {
            // Painted housing; black arm, foot and the rim around the glass.
            let painted = c.x > 0.055 && c.z < 0.016;
            Some(Assign {
                mesh: if painted { 0 } else { 1 },
                mode: 0,
            })
        },
        |_, _, p, n| vtx(p, n, blank, 1.0),
    );
    let mut it = meshes.into_iter();
    let place = trs(base, Quat::IDENTITY, Vec3::ONE);
    let paint = transformed(it.next().unwrap(), place);
    let black = transformed(it.next().unwrap(), place);
    // The glass: a rounded rectangle on the flat rear face.
    let mut glass = Mesh::default();
    let n = 32;
    let c = hc + Vec3::Z * 0.0235;
    glass
        .vertices
        .push(Vertex::new(c, Vec3::Z, Vec2::splat(0.5)));
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (s, co) = a.sin_cos();
        // Superellipse outline, following the housing's taper.
        let x = co.signum() * co.abs().powf(0.45) * 0.086;
        let y = s.signum() * s.abs().powf(0.45) * 0.036 * (0.86 + 0.14 * (x / 0.086));
        glass.vertices.push(Vertex::new(
            c + Vec3::new(x, y, 0.0),
            Vec3::Z,
            Vec2::new(0.5 + x, 0.5 + y),
        ));
    }
    for i in 0..n as u32 {
        glass
            .indices
            .extend_from_slice(&[0, 1 + i, 1 + (i + 1) % n as u32]);
    }
    orient_like_normals(&mut glass);
    (paint, black, transformed(glass, place))
}

/// Two wiper arms with blades parked along the bottom of the windshield.
fn wipers(shell: &(dyn Fn(Vec3) -> f32 + Sync)) -> Mesh {
    let mut parts = Vec::new();
    for (pivot_x, x_from, x_to) in [(-0.40f32, -0.64f32, -0.04f32), (0.16, 0.0, 0.54)] {
        let line = |x: f32| Vec2::new(x, -0.215 + 0.07 * (x / 0.62).powi(2) + 0.055);
        let blade: Vec<Vec2> = (0..=24)
            .map(|i| line(x_from + (x_to - x_from) * i as f32 / 24.0))
            .collect();
        let on = surface_polyline(shell, Proj::Top, &blade, GROUND, 0.02);
        parts.push(tube_on(&on, 0.006, 0.0045, 6));
        // Arm: from the pivot on the cowl to the blade's middle, a little above the glass.
        let mid = line(0.5 * (x_from + x_to));
        let pivot = Vec2::new(pivot_x, -0.30);
        let arm_line: Vec<Vec2> = (0..=10).map(|i| pivot.lerp(mid, i as f32 / 10.0)).collect();
        let on = surface_polyline(shell, Proj::Top, &arm_line, GROUND, 0.02);
        parts.push(tube_on(&on, 0.014, 0.006, 6));
        if let Some((p, n)) = surface_point(shell, Proj::Top, pivot, GROUND) {
            parts.push(transformed(
                lathe_x(
                    &[
                        Vec2::new(0.0, 0.016),
                        Vec2::new(0.012, 0.015),
                        Vec2::new(0.016, 0.008),
                        Vec2::new(0.017, 0.0),
                    ],
                    12,
                    &[],
                ),
                trs(p, Quat::from_rotation_arc(Vec3::X, n), Vec3::ONE),
            ));
        }
    }
    merge(parts)
}

/// A flush door handle on the right door.
fn door_handle(shell: &(dyn Fn(Vec3) -> f32 + Sync)) -> Mesh {
    let lo = |s: f32| Vec2::new(0.39 + 0.16 * s, 0.799);
    let hi = |s: f32| Vec2::new(0.39 + 0.16 * s, 0.817);
    let strip = Strip { lo: &lo, hi: &hi };
    draped_slab(
        shell,
        Proj::Side,
        GROUND,
        &strip,
        12,
        3,
        &|s, _| 0.0025 * sstep(0.0, 0.12, s) * sstep(1.0, 0.88, s) + 0.0008,
        true,
        &|s, t| Vec2::new(s, t),
    )
}

/// Badges (chrome ring and emblem on blue enamel) at plane points `spots`, and chrome model
/// lettering on the tail (back view, baseline-left corner): (chrome, enamel).
pub(super) fn badges_at(
    shell: &(dyn Fn(Vec3) -> f32 + Sync),
    plain: &(dyn Fn(Vec3) -> f32 + Sync),
    ground: f32,
    spots: &[(Proj, Vec2)],
    lettering: Option<(&str, Vec2)>,
) -> (Mesh, Mesh) {
    let mut chrome = Vec::new();
    let mut enamel = Vec::new();
    for &(proj, q) in spots {
        let Some((p, n)) = surface_point(plain, proj, q, ground) else {
            continue;
        };
        let at = trs(p, Quat::from_rotation_arc(Vec3::X, n), Vec3::ONE);
        let r = 0.032;
        let ring = lathe_x(
            &[
                Vec2::new(-0.001, r + 0.004),
                Vec2::new(0.004, r + 0.003),
                Vec2::new(0.006, r),
                Vec2::new(0.004, r - 0.004),
            ],
            40,
            &[],
        );
        let disc = flipped(lathe_x(
            &[
                Vec2::new(0.0045, 0.0),
                Vec2::new(0.004, r * 0.7),
                Vec2::new(0.003, r - 0.003),
            ],
            40,
            &[],
        ));
        // Emblem: the maker's winged V (a V crossed by a wing bar), raised chrome. Outline
        // points are (up, across) in the badge plane.
        let bar = |a: Vec2, b: Vec2, w: f32| -> Mesh {
            let d = (b - a).normalize_or(Vec2::X);
            let n = Vec2::new(-d.y, d.x) * w;
            prism(&[a + n, b + n, b - n, a - n], 0.0045, 0.0065)
        };
        let emblem = merge([
            bar(Vec2::new(0.017, -0.017), Vec2::new(-0.017, 0.0), 0.0032),
            bar(Vec2::new(-0.017, 0.0), Vec2::new(0.017, 0.017), 0.0032),
            bar(Vec2::new(0.002, -0.026), Vec2::new(0.002, 0.026), 0.0022),
        ]);
        chrome.push(transformed(merge([ring, emblem]), at));
        enamel.push(transformed(disc, at));
    }
    if let Some((text, at)) = lettering {
        for (a, b) in tex::text_segments(text, at, 0.026, Vec2::X, Vec2::Y) {
            let on = surface_polyline(shell, Proj::Back, &[a, b], ground, 0.004);
            if on.len() >= 2 {
                chrome.push(tube_on(&on, 0.0012, 0.0016, 5));
            }
        }
    }
    (merge(chrome), merge(enamel))
}

/// An extruded 2D outline (in the badge's y–z plane, x axial from `x0` to `x1`).
pub(super) fn prism(outline: &[Vec2], x0: f32, x1: f32) -> Mesh {
    let n = outline.len();
    let mut m = Mesh::default();
    let c = outline.iter().copied().sum::<Vec2>() / n as f32;
    for &x in &[x0, x1] {
        let base = m.vertices.len() as u32;
        m.vertices
            .push(Vertex::new(Vec3::new(x, c.x, c.y), Vec3::X, Vec2::ZERO));
        for p in outline {
            m.vertices.push(Vertex::new(
                Vec3::new(x, p.x, p.y),
                Vec3::X * x.signum(),
                Vec2::ZERO,
            ));
        }
        for i in 0..n as u32 {
            m.indices
                .extend_from_slice(&[base, base + 1 + i, base + 1 + (i + 1) % n as u32]);
        }
    }
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let d = (b - a).normalize_or(Vec2::X);
        let nrm = Vec3::new(0.0, d.y, -d.x);
        let base = m.vertices.len() as u32;
        for (x, p) in [(x0, a), (x0, b), (x1, b), (x1, a)] {
            m.vertices
                .push(Vertex::new(Vec3::new(x, p.x, p.y), nrm, Vec2::ZERO));
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    // Cap normals: front +x, back −x.
    for v in &mut m.vertices[..2 * (n + 1)] {
        v.normal = if (v.position[0] - x1).abs() < 1e-6 {
            [1.0, 0.0, 0.0]
        } else {
            [-1.0, 0.0, 0.0]
        };
    }
    parts::fix_winding_outward(&mut m);
    orient_like_normals(&mut m);
    m
}

/// A sports bucket seat in its own frame (x across, y up from the seat floor, z back),
/// vertex-colored: black leather bolsters, dark red inserts, dark frame.
fn seat() -> Mesh {
    let rot = |p: Vec3, c: Vec3, deg: f32| Quat::from_rotation_x(-deg.to_radians()) * (p - c);
    let field = |p: Vec3| {
        let cushion = sd_round_box(
            rot(p, Vec3::new(0.0, 0.13, -0.05), -7.0),
            Vec3::new(0.21, 0.045, 0.23),
            0.035,
        );
        let cb = |s: f32| {
            sd_capsule(
                p,
                Vec3::new(s * 0.2, 0.19, -0.25),
                Vec3::new(s * 0.2, 0.17, 0.12),
                0.055,
            )
        };
        let back = sd_round_box(
            rot(p, Vec3::new(0.0, 0.47, 0.24), 18.0),
            Vec3::new(0.225, 0.31, 0.05),
            0.04,
        );
        let bb = |s: f32| {
            sd_capsule(
                p,
                Vec3::new(s * 0.21, 0.24, 0.23),
                Vec3::new(s * 0.185, 0.62, 0.35),
                0.062,
            )
        };
        let head = sd_round_box(
            rot(p, Vec3::new(0.0, 0.83, 0.355), 18.0),
            Vec3::new(0.12, 0.085, 0.045),
            0.04,
        );
        let frame = sd_round_box(
            p - Vec3::new(0.0, 0.045, -0.02),
            Vec3::new(0.16, 0.045, 0.2),
            0.02,
        );
        let mut d = smin(cushion, back, 0.03);
        d = smin(d, cb(1.0), 0.03);
        d = smin(d, cb(-1.0), 0.03);
        d = smin(d, bb(1.0), 0.035);
        d = smin(d, bb(-1.0), 0.035);
        d = smin(d, head, 0.05);
        smin(d, frame, 0.01)
    };
    sculpt(
        &field,
        Vec3::new(-0.32, -0.02, -0.36),
        Vec3::new(0.32, 0.98, 0.48),
        0.017,
        true,
        0.001,
        &|p, n| {
            let insert_cushion = p.x.abs() < 0.13 && n.y > 0.55 && p.z < 0.15 && p.y > 0.12;
            let insert_back = p.x.abs() < 0.14 && n.z < -0.55 && p.y > 0.25 && p.y < 0.72;
            if p.y < 0.07 {
                Vec4::new(0.02, 0.02, 0.022, 1.0)
            } else if insert_cushion || insert_back {
                Vec4::new(0.16, 0.025, 0.02, 1.0)
            } else {
                Vec4::new(0.035, 0.035, 0.037, 1.0)
            }
        },
    )
}

/// Dashboard (with the driver's binnacle, center stack and console) and the two screens.
fn dashboard() -> (Mesh, Mesh) {
    let y = |h: f32| h - GROUND;
    let field = |p: Vec3| {
        let dash = sd_round_box(
            p - Vec3::new(0.0, y(0.70), -0.27),
            Vec3::new(0.72, 0.12, 0.19),
            0.05,
        );
        let binnacle = sd_round_box(
            p - Vec3::new(SEATS[0], y(0.83), -0.12),
            Vec3::new(0.16, 0.035, 0.09),
            0.03,
        );
        let stack = sd_round_box(
            p - Vec3::new(0.0, y(0.50), -0.20),
            Vec3::new(0.13, 0.20, 0.12),
            0.04,
        );
        let console = sd_round_box(
            p - Vec3::new(0.0, y(0.33), 0.30),
            Vec3::new(0.10, 0.12, 0.48),
            0.04,
        );
        smin(smin(smin(dash, binnacle, 0.04), stack, 0.05), console, 0.05)
    };
    let dash = sculpt(
        &field,
        Vec3::new(-0.8, y(0.18), -0.5),
        Vec3::new(0.8, y(0.9), 0.82),
        0.02,
        false,
        0.0012,
        &|p, n| {
            // A carbon-look inlay band across the dash face.
            let band = n.z > 0.5 && (y(0.66)..y(0.70)).contains(&p.y);
            if band {
                Vec4::new(0.25, 0.25, 0.26, 1.0)
            } else {
                Vec4::ONE
            }
        },
    );
    // Gear selector on the console.
    let knob = lathe_x(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.012),
            Vec2::new(0.03, 0.014),
            Vec2::new(0.045, 0.02),
            Vec2::new(0.06, 0.012),
            Vec2::new(0.064, 0.0),
        ],
        16,
        &[],
    );
    let knob = transformed(
        flipped(knob),
        trs(
            Vec3::new(0.0, y(0.43), 0.18),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Vec3::ONE,
        ),
    );
    let mut dash = dash;
    dash.merge(&knob);
    // Screens: instrument display in the binnacle, center display on the stack.
    let screen = |c: Vec3, w: f32, h: f32, tilt: f32, u0: f32, u1: f32| {
        let rot = Quat::from_rotation_x(-tilt.to_radians());
        let n = rot * Vec3::Z;
        let (right, up) = (rot * Vec3::X, rot * Vec3::Y);
        let mut m = Mesh::default();
        for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = c + right * (sx * w * 0.5) + up * (sy * h * 0.5);
            let uv = Vec2::new(
                if sx < 0.0 { u0 } else { u1 },
                if sy < 0.0 { 1.0 } else { 0.0 },
            );
            m.vertices.push(Vertex::new(p, n, uv));
        }
        m.indices = vec![0, 1, 2, 0, 2, 3];
        orient_like_normals(&mut m);
        m
    };
    let screens = merge([
        screen(
            Vec3::new(SEATS[0], y(0.80), -0.035),
            0.27,
            0.09,
            12.0,
            0.0,
            0.62,
        ),
        screen(Vec3::new(0.0, y(0.66), -0.07), 0.19, 0.11, 18.0, 0.64, 1.0),
    ]);
    (dash, screens)
}

/// Three-spoke steering wheel (rim radius `r`, dished `dish` meters toward the driver at the
/// hub) with its column, centered at `c` with its axis `axis` toward the driver.
pub(super) fn steering_wheel_at(c: Vec3, axis: Vec3, r: f32, dish: f32) -> Mesh {
    let rim = torus(c, axis, r, 0.016, 40, 8);
    let rot = Quat::from_rotation_arc(Vec3::X, axis);
    let mut parts = vec![tint(rim, Vec4::new(0.03, 0.03, 0.032, 1.0))];
    // Top-center marker stripe.
    let up = (Vec3::Y - axis * axis.y).normalize();
    let marker = torus(c, axis, r, 0.0168, 40, 8);
    let marker = keep(marker, |p| (p - c).normalize().dot(up) > 0.995);
    parts.push(tint(marker, Vec4::new(0.8, 0.55, 0.05, 1.0)));
    let right = axis.cross(up).normalize();
    for dir in [right, -right, -up] {
        let a = c + dir * 0.03 - axis * (0.012 + dish);
        let b = c + dir * (r - 0.01) - axis * 0.004;
        parts.push(tint(
            parts::tube(&[a, b], 0.011, 8),
            Vec4::new(0.05, 0.05, 0.055, 1.0),
        ));
    }
    let hub = lathe_x(
        &[
            Vec2::new(-0.05, 0.03),
            Vec2::new(-0.02, 0.055),
            Vec2::new(0.0, 0.058),
            Vec2::new(0.008, 0.045),
            Vec2::new(0.01, 0.0),
        ],
        24,
        &[],
    );
    parts.push(tint(
        transformed(flipped(hub), trs(c - axis * dish, rot, Vec3::ONE)),
        Vec4::new(0.06, 0.06, 0.065, 1.0),
    ));
    let column = parts::tube(
        &[c - axis * (0.04 + dish), c - axis * (0.30 + dish)],
        0.035,
        12,
    );
    parts.push(tint(column, Vec4::new(0.03, 0.03, 0.03, 1.0)));
    merge(parts)
}

pub(super) fn tint(m: Mesh, c: Vec4) -> Mesh {
    parts::tint(m, c)
}

/// Keep the triangles whose vertices all pass `f`.
pub(super) fn keep(mut m: Mesh, f: impl Fn(Vec3) -> bool) -> Mesh {
    let ok: Vec<bool> = m
        .vertices
        .iter()
        .map(|v| f(Vec3::from(v.position)))
        .collect();
    let idx = std::mem::take(&mut m.indices);
    for t in idx.as_chunks::<3>().0.iter() {
        if t.iter().all(|&i| ok[i as usize]) {
            m.indices.extend_from_slice(t);
        }
    }
    m
}

/// Instrument and center displays (u 0..0.62 instruments, 0.64..1 center screen).
pub(super) fn screen_texture() -> Image {
    let (w, h) = (512u32, 128u32);
    let dial = |p: Vec2, c: Vec2, r: f32, needle: f32| -> f32 {
        let d = p - c;
        let rho = d.length();
        let ring = (1.0 - ((rho - r).abs() / 1.2).min(1.0)) * 0.9;
        let a = d.y.atan2(d.x);
        // Ticks around the lower three quarters.
        let ticks = if (r - 7.0..r - 1.0).contains(&rho) && (a < 2.2 || a > 2.4) {
            let t = (a * 12.0 / std::f32::consts::PI).fract();
            if !(0.12..=0.88).contains(&t) {
                0.8
            } else {
                0.0
            }
        } else {
            0.0
        };
        let dir = Vec2::new(needle.cos(), needle.sin());
        let along = d.dot(dir);
        let off = (d - dir * along).length();
        let nd = if (0.0..r - 4.0).contains(&along) && off < 1.3 {
            1.0
        } else {
            0.0
        };
        (ring + ticks).max(nd * 1.4)
    };
    tex::par_image(w, h, true, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, h as f32 - y as f32 - 0.5);
        let u = p.x / w as f32;
        let (r, g, b) = if u < 0.62 {
            let a = dial(p, Vec2::new(75.0, 64.0), 50.0, 2.6);
            let b = dial(p, Vec2::new(245.0, 64.0), 50.0, 1.2);
            let bar = if (140.0..180.0).contains(&p.x) && (30.0..36.0).contains(&p.y) {
                0.6
            } else {
                0.0
            };
            let v = a.max(b).max(bar);
            let red = if (p - Vec2::new(245.0, 64.0)).length() > 44.0 && p.x > 270.0 && p.y > 64.0 {
                1.0
            } else {
                0.0
            };
            (
                0.02 + v * (0.85 + 0.15 * red),
                0.03 + v * 0.85 * (1.0 - 0.7 * red),
                0.04 + v * 0.95 * (1.0 - 0.8 * red),
            )
        } else if u > 0.64 {
            // A map-like center display: dark blue field, roads and a route.
            let q = (p - Vec2::new(0.82 * w as f32, 64.0)) / 30.0;
            let roads = ((q.x * 1.7 + q.y * 0.4).sin().abs() < 0.06) as i32 as f32
                + ((q.y * 2.1 - q.x * 0.3).sin().abs() < 0.05) as i32 as f32;
            let route = ((q.y - 0.3 * q.x * q.x).abs() < 0.05) as i32 as f32;
            (
                0.03 + 0.25 * roads + 0.1 * route,
                0.06 + 0.28 * roads + 0.55 * route,
                0.12 + 0.3 * roads + 0.9 * route,
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        [tex::srgb(r), tex::srgb(g), tex::srgb(b), 255]
    })
}

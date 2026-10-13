//! Every mesh and texture of the rally car: the sculpted body with its livery (decals, shut
//! lines and gravel grime baked into one atlas with color, normal and metallic-roughness
//! maps), glass and the windshield banner, the cabin shell, lights, grille meshes, the rear
//! wing, mud flaps, skid plate, exhaust, mirrors, wiper, hood pins, tow straps, antenna, roll
//! cage, bucket seats with harnesses, dash and controls, the optional hood light pod, and
//! gravel wheels.

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Image, Mesh, Vertex};

use super::atlas::{Atlas, View};
use super::details::{
    Proj, Strip, draped_slab, honeycomb, ribbon_on, sculpt, surface_point, surface_polyline, torus,
    tube_on,
};
use super::glass::Pane;
use super::mesher::{self, Assign, Field, split, surface_nets, vtx};
use super::parts::{
    self, draped_grid, lathe_x, loft_rings, merge, mirror_x, orient_like_normals, transformed, trs,
};
use super::rally::{ARCH, Body, FRONT_AXLE, GROUND, REAR_AXLE, WHEEL_H, Windows, arch_rho};
use super::sdf::{
    self, Curve, Table, bump, raycast, rmax, sd_capsule, sd_round_box, sd_round_rect, smin, sstep,
};
use super::sports::{Feature, LIP};
use super::sports_kit::{
    badges_at, lamp_module, mirror_at, pts, screen_texture, steering_wheel_at, tail_texture, tint,
};
use super::tex::{self, Canvas};
use super::{FRIT_RANGE, Kit, frit_texture, gather, key, wheel_meshes};

pub const PREFIX: &str = "vehicles/cars/rally";

/// The door handle's center in side view (z, h).
const HANDLE: (f32, f32) = (0.43, 0.862);

/// The body atlas: 2048² with separate left and right side views, about 3.3 mm per pixel.
pub fn atlas() -> Atlas {
    Atlas::split_sides(2048, -2.12, 2.12, 1.0, 1.56, GROUND)
}

/// Seat positions (x of each seat center, z of the seat origin), seat-floor height above
/// the ground.
pub const SEATS: [f32; 2] = [-0.36, 0.36];
pub const SEAT_Z: f32 = -0.10;
pub const SEAT_FLOOR: f32 = 0.26;
/// Steering wheel center (driver on the left).
pub const WHEEL_CENTER: Vec3 = Vec3::new(-0.36, 0.80 - GROUND, -0.50);
/// The cage's main hoop (z), behind the seats; its harness bar height above the ground.
const HOOP_Z: f32 = 0.36;
const HARNESS_BAR_H: f32 = 0.92;
/// Hood light pod: lamp centers (|x|), center height, lamp radius and the pod's front face.
const POD_LAMPS: [f32; 2] = [0.165, 0.495];
const POD_H: f32 = 0.86;
const POD_R: f32 = 0.066;
const POD_FACE: f32 = -2.03;

pub fn steering_axis() -> Vec3 {
    Vec3::new(0.0, 0.35, 0.94).normalize()
}

/// Build the rally car's kit: the body skin, the livery and the other parts on their own
/// threads.
pub fn build() -> Kit {
    let body = Body::new();
    let win = body.windows();
    let atlas = atlas();
    let (body, win, atlas) = (&body, &win, &atlas);
    gather(vec![
        ("skin", Box::new(move || skin(body, win, atlas))),
        (
            "livery",
            Box::new(move || {
                let [color, normal, mr] = livery(body, win, atlas);
                let k = |name: &str| key(PREFIX, name);
                Kit {
                    meshes: Vec::new(),
                    images: vec![
                        (k("atlas_color"), color),
                        (k("atlas_normal"), normal),
                        (k("atlas_mr"), mr),
                    ],
                }
            }),
        ),
        ("glass, cabin", Box::new(move || glass_and_cabin(body, win))),
        (
            "exterior parts",
            Box::new(move || exterior_parts(body, atlas)),
        ),
        (
            "interior, pod, wheels",
            Box::new(move || interior_parts(body)),
        ),
    ])
}

/// The body skin: painted panels (atlas UVs), wheel-well liners, lamp housings and recesses.
fn skin(body: &Body, win: &Windows, atlas: &Atlas) -> Kit {
    let shell = |p: Vec3| body.shell(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Body skin -----------------------------------------------------------------------
    let raw = surface_nets(
        &shell,
        Vec3::new(-1.02, -GROUND - 0.02, -2.12),
        Vec3::new(1.02, 1.56 - GROUND, 2.12),
        0.028,
        true,
        1.3,
    );
    let raw = mesher::refine(&raw, &shell, 2, 24.0, 0.0004);
    let window = |p: Vec3| body.window_field(win, p, 0.0, 0.0);
    let pocket = |p: Vec3| body.pocket_field(p);
    let liner = |p: Vec3| body.liner_field(p);
    let raw = mesher::decimate(&raw, 0.0008, 8.0, 0.25, true, |p| {
        let (w, l, c) = (window(p), liner(p), pocket(p));
        if w.abs() < 0.015 || l.abs() < 0.015 || c.abs() < 0.012 {
            0.0
        } else if l < 0.0
            || (c < 0.0 && body.features.near(body, p, LIP) != Some(Feature::Headlight))
        {
            // Wheel-well liners and pocket floors: mostly hidden (not the headlight housings,
            // seen through their lenses).
            8.0
        } else {
            1.0
        }
    });
    let cm = mesher::cut(&raw, &[&window, &pocket, &liner]);
    let meshes = split(
        &cm.raw,
        4,
        |c, n, t| {
            if cm.side(0, t) < 0.0 {
                return None;
            }
            if cm.side(1, t) < 0.0 {
                let mesh = match body.features.near(body, c, LIP) {
                    Some(Feature::Headlight) => 2,
                    _ => 3,
                };
                return Some(Assign { mesh, mode: 0 });
            }
            if cm.side(2, t) < 0.0 {
                return Some(Assign { mesh: 1, mode: 0 });
            }
            // Sideways, forward and backward faces: the front bumper (ahead of the flare seams)
            // takes the front view, so the two views meet in the seam groove; the rear bumper
            // takes the back view; everything else (flare ends included) the side view.
            let mut view = View::of(n);
            if matches!(view, View::Side | View::Front | View::Back) {
                let h = c.y + GROUND;
                let ahead = c.z < FRONT_AXLE
                    && arch_rho(c.z - FRONT_AXLE, (h - WHEEL_H).max(0.0)) > ARCH + 0.2;
                view = if ahead {
                    View::Front
                } else if view == View::Back && c.z >= 1.6 {
                    View::Back
                } else {
                    View::Side
                };
            }
            Some(Assign {
                mesh: 0,
                mode: view as u8,
            })
        },
        |a, _, p, n| match a.mesh {
            0 => vtx(p, n, atlas.uv(View::from_mode(a.mode), p), 1.0),
            _ => vtx(p, n, Vec2::new(p.z, -p.y), 1.0),
        },
    );
    let mut it = meshes.into_iter();
    for name in ["paint", "liner", "lamp_housing", "recess"] {
        kit.meshes.push((k(name), it.next().unwrap()));
    }
    kit
}

/// Glass, the windshield banner and the cabin shell.
fn glass_and_cabin(body: &Body, win: &Windows) -> Kit {
    let shell = |p: Vec3| body.shell(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Glass and the windshield banner ----------------------------------------------------
    let pane_mesh = |pane: &Pane, ns: usize, nt: usize| {
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
        .push((k("glass_back"), pane_mesh(&win.back, 24, 12)));
    let side = merge(win.sides.iter().map(|p| pane_mesh(p, 20, 8)));
    kit.meshes.push((k("glass_left"), mirror_x(&side)));
    kit.meshes.push((k("glass_right"), side));
    kit.images.push((k("frit"), frit_texture()));
    let (b0, b1) = (0.74, 0.92);
    let banner = draped_grid(
        &shell,
        32,
        3,
        |s, t| win.windshield.ray(s, b0 + (b1 - b0) * t),
        0.0035,
        |s, t, _| Vec2::new(1.0 - s, 1.0 - t),
    );
    kit.meshes.push((k("banner"), banner));
    kit.images.push((k("banner_tex"), banner_texture()));

    // --- Cabin shell (the inside of the skin, 4 cm in) ---------------------------------------
    let cabin = |p: Vec3| {
        let h = p.y + GROUND;
        (shell(p) + 0.04)
            .max(-0.86 - p.z)
            .max(p.z - 1.92)
            .max(SEAT_FLOOR - 0.02 - h)
    };
    let mut raw = surface_nets(
        &cabin,
        Vec3::new(-1.0, 0.2 - GROUND, -0.9),
        Vec3::new(1.0, 1.52 - GROUND, 1.98),
        0.042,
        true,
        1.3,
    );
    mesher::flip(&mut raw);
    let inner_window = |p: Vec3| body.window_field(win, p, 0.04, 0.0);
    let raw = mesher::decimate(&raw, 0.0032, 10.0, 0.3, true, |p| {
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
            let ao = 0.4 + 0.6 * sstep(-0.15, 0.6, p.y);
            vtx(p, n, Vec2::new(p.z, p.y), ao)
        },
    );
    kit.meshes
        .push((k("cabin"), inner.into_iter().next().unwrap()));
    kit
}

/// Lights, grilles, aero, underbody and trim parts.
fn exterior_parts(body: &Body, atlas: &Atlas) -> Kit {
    let shell = |p: Vec3| body.shell(p);
    let plain = |p: Vec3| body.shell_plain(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Lights, grilles ------------------------------------------------------------------
    lights(&mut kit, &plain);
    kit.meshes.push((k("grille"), grilles(body, &plain)));

    // --- Wing, flaps, skid plate, exhaust ---------------------------------------------------
    let (wing, plates) = rear_wing(&plain);
    kit.meshes.push((k("wing"), wing));
    kit.meshes.push((k("wing_plate"), plates));
    kit.images.push((k("plate_tex"), plate_texture()));
    let mut flaps = Vec::new();
    for (z, top) in [(FRONT_AXLE + 0.43, 0.30f32), (REAR_AXLE + 0.43, 0.32)] {
        flaps.push(mud_flap(0.655, 0.925, z, top, 0.035));
        flaps.push(mud_flap(-0.925, -0.655, z, top, 0.035));
    }
    kit.meshes.push((k("flaps"), merge(flaps)));
    kit.images.push((k("flap_tex"), flap_texture()));
    let (skid, bolts) = skid_plate();
    kit.meshes.push((k("skid"), skid));
    let (color, normal) = brushed_texture();
    kit.images.push((k("brushed_color"), color));
    kit.images.push((k("brushed_normal"), normal));
    kit.meshes.push((k("exhaust"), exhaust(body, &plain)));
    kit.meshes.push((k("number_plate"), number_plate(&plain)));
    kit.images.push((k("number_tex"), number_texture()));

    // --- Mirrors, wiper, handles, pins, straps, antenna, badges -----------------------------
    let blank = atlas.uv(View::Side, Vec3::new(1.0, 1.45 - GROUND, -1.6));
    let mz = -0.62;
    let (mirror_paint, mirror_black, mirror_glass) = mirror_at(
        &shell,
        Vec2::new(mz, body.belt_at(mz) - 0.012),
        GROUND,
        blank,
    );
    kit.meshes.push((
        k("mirror_paint"),
        merge([mirror_x(&mirror_paint), mirror_paint]),
    ));
    let handles = door_handles(&shell);
    kit.meshes.push((
        k("mirror_black"),
        merge([
            mirror_x(&mirror_black),
            mirror_black,
            mirror_x(&handles),
            handles,
        ]),
    ));
    kit.meshes.push((
        k("mirror_glass"),
        merge([mirror_x(&mirror_glass), mirror_glass]),
    ));
    kit.meshes
        .push((k("wipers"), merge([wiper(&shell), antenna(&shell)])));
    let (badge_chrome, badge_enamel) = badges_at(
        &shell,
        &plain,
        GROUND,
        &[
            (Proj::Front, Vec2::new(0.0, 0.66)),
            (Proj::Back, Vec2::new(0.0, 0.99)),
        ],
        None,
    );
    kit.meshes
        .push((k("chrome"), merge([badge_chrome, hood_pins(&shell), bolts])));
    kit.meshes.push((k("enamel"), badge_enamel));
    kit.meshes.push((k("straps"), tow_straps(&plain)));
    kit
}

/// Roll cage, seats, dash and controls, the hood light pod and the wheels.
fn interior_parts(body: &Body) -> Kit {
    let plain = |p: Vec3| body.shell_plain(p);
    let mut kit = Kit::default();
    let k = |name: &str| key(PREFIX, name);
    // --- Interior --------------------------------------------------------------------------
    kit.meshes.push((k("cage"), roll_cage(body)));
    kit.meshes.push((k("seat"), bucket_seat()));
    let (dash, screens, controls) = dash_and_controls();
    kit.meshes.push((k("dash"), dash));
    kit.meshes.push((k("screens"), screens));
    kit.meshes.push((k("controls"), controls));
    kit.images.push((k("screen"), screen_texture()));

    // --- Hood light pod (drawn only when fitted) ----------------------------------------------
    let pod = light_pod(&plain);
    kit.meshes.push((k("pod_shell"), pod.shell));
    kit.meshes.push((k("pod_chrome"), pod.bezels));
    kit.meshes.push((k("pod_reflector"), pod.reflectors));
    kit.meshes.push((k("pod_lens"), pod.lenses));
    kit.meshes.push((k("pod_led"), pod.leds));

    // --- Wheels ----------------------------------------------------------------------------
    wheel_meshes(&mut kit, PREFIX, &super::RALLY_WHEEL, 4);
    kit
}

// ---------------------------------------------------------------------------- livery

/// Linear color from an sRGB hex value.
fn lin(hex: u32) -> Vec3 {
    let c = |v: u32| {
        let s = (v & 255) as f32 / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3::new(c(hex >> 16), c(hex >> 8), c(hex))
}

/// A paint layer: linear color, metallic, roughness, and whether it carries metallic flake.
#[derive(Clone, Copy)]
struct Coat {
    color: Vec3,
    metal: f32,
    rough: f32,
    flake: f32,
}

impl Coat {
    const fn new(color: Vec3, metal: f32, rough: f32, flake: f32) -> Self {
        Self {
            color,
            metal,
            rough,
            flake,
        }
    }

    fn mix(self, o: Coat, t: f32) -> Coat {
        if t <= 0.0 {
            return self;
        }
        if t >= 1.0 {
            return o;
        }
        Coat {
            color: self.color.lerp(o.color, t),
            metal: self.metal + (o.metal - self.metal) * t,
            rough: self.rough + (o.rough - self.rough) * t,
            flake: self.flake + (o.flake - self.flake) * t,
        }
    }
}

/// Plane point of an atlas view as a body point (side views on the given side).
fn plane_point(view: View, q: Vec2, left: bool) -> Vec3 {
    match view {
        View::Side => Vec3::new(if left { -1.0 } else { 1.0 }, q.y - GROUND, q.x),
        View::Top => Vec3::new(q.y, 0.0, q.x),
        _ => Vec3::new(q.x, q.y - GROUND, 0.0),
    }
}

/// Anti-aliased coverage of a signed distance (meters) at `px` meters per pixel.
#[inline]
fn fill(sd: f32, px: f32) -> f32 {
    (0.5 - sd / px).clamp(0.0, 1.0)
}

/// The livery atlas: color, normal and metallic-roughness maps. Deep metallic blue with a
/// white lower sweep edged in orange and white pinstripes, door number panels, roof number,
/// front and rear plates, fictional sponsor lettering and logos, blacked-out pillars, black
/// plastic lower trim, shut lines, and gravel dust thrown up behind the wheels.
fn livery(body: &Body, win: &Windows, atlas: &Atlas) -> [Image; 3] {
    let shell = |p: Vec3| body.shell(p);
    let size = atlas.size;
    let px_m = 1.0 / atlas.ppm;
    let blue = Coat::new(lin(0x1747a6), 0.55, 0.2, 1.0);
    let white = Coat::new(lin(0xf2f2ef), 0.0, 0.24, 0.0);
    let orange = Coat::new(lin(0xff5a1f), 0.0, 0.26, 0.0);
    let black = Coat::new(lin(0x101012), 0.0, 0.3, 0.0);
    let navy = Coat::new(lin(0x0d1b3d), 0.0, 0.28, 0.0);
    let gloss_black = Coat::new(lin(0x060607), 0.0, 0.12, 0.0);
    let plastic = Coat::new(lin(0x1b1b1c), 0.0, 0.62, 0.0);
    let underbody = Coat::new(lin(0x24211e), 0.0, 0.8, 0.0);
    let dust_light = lin(0xb5a17f);
    let dust_dark = lin(0x8a7558);

    // --- Shut lines -----------------------------------------------------------------------
    let mut cv = Canvas::new(size, size);
    let reach = 4.0;
    let belt = |z: f32| body.belt_at(z);
    let mut side_lines: Vec<Vec<Vec2>> = Vec::new();
    // Flare seams around both arches (the bolt-on box flares).
    for zc in [FRONT_AXLE, REAR_AXLE] {
        side_lines.push(arch_contour(zc, ARCH + 0.2, 0.25));
    }
    // Door: front edge down to the flare, sill, rear edge.
    side_lines.push(pts(&[(-0.80, belt(-0.80)), (-0.80, 0.83)]));
    side_lines.push(pts(&[(-0.66, 0.29), (0.58, 0.29), (0.58, belt(0.58))]));
    // Window frame around the door glass, seal around the quarter light.
    side_lines.push(offset_loop(&win.sides[0].outline, 0.018));
    side_lines.push(offset_loop(&win.sides[1].outline, 0.012));
    // Bumper seams down to the flares.
    side_lines.push(pts(&[(1.975, 0.80), (1.90, 0.76), (1.81, 0.71)]));
    // Door handle recess.
    let (z, h) = HANDLE;
    side_lines.push(
        (0..=20)
            .map(|i| {
                let a = i as f32 / 20.0 * std::f32::consts::TAU;
                Vec2::new(z + 0.075 * a.cos(), h + 0.02 * a.sin())
            })
            .collect(),
    );
    let mirror = |on: &[(Vec3, Vec3)]| -> Vec<(Vec3, Vec3)> {
        on.iter()
            .map(|(p, n)| (Vec3::new(-p.x, p.y, p.z), Vec3::new(-n.x, n.y, n.z)))
            .collect()
    };
    for line in &side_lines {
        let on = surface_polyline(&shell, Proj::Side, line, GROUND, 0.008);
        atlas.stamp(&mut cv, &on, reach, 1);
        atlas.stamp(&mut cv, &mirror(&on), reach, 1);
    }
    // Hood (plan view) and hatch (rear view, its hinge in plan view).
    let hood = pts(&[
        (-0.62, -0.83),
        (0.62, -0.83),
        (0.665, -1.20),
        (0.655, -1.60),
        (0.60, -1.85),
        (0.40, -1.905),
        (0.0, -1.94),
        (-0.40, -1.905),
        (-0.60, -1.85),
        (-0.655, -1.60),
        (-0.665, -1.20),
        (-0.62, -0.83),
    ]);
    let hatch = pts(&[
        (-0.55, 1.29),
        (-0.60, 1.035),
        (-0.49, 1.02),
        (-0.49, 0.80),
        (0.49, 0.80),
        (0.49, 1.02),
        (0.60, 1.035),
        (0.55, 1.29),
    ]);
    for (proj, line) in [
        (Proj::Top, hood),
        (Proj::Back, hatch),
        (Proj::Top, pts(&[(-0.56, 1.675), (0.56, 1.675)])),
    ] {
        let on = surface_polyline(&shell, proj, &line, GROUND, 0.008);
        atlas.stamp(&mut cv, &on, reach, 1);
    }

    // --- Lettering: one canvas, a tag per item (its coat and stroke half-width) -------------
    let mut tx = Canvas::new(size, size);
    let mut styles: Vec<(Coat, f32)> = vec![(white, 0.0)];
    {
        // `dir` and `up`: the viewer's right and up in the view's plane coordinates.
        let mut text = |view: View,
                        left: bool,
                        s: &str,
                        center: Vec2,
                        h: f32,
                        (dir, up): (Vec2, Vec2),
                        coat: Coat,
                        weight: f32| {
            let w = tex::text_width(s, h);
            let origin = center - dir * (w * 0.5) - up * (h * 0.5);
            let hw = weight * h / 6.0 * atlas.ppm;
            let tag = styles.len() as u8;
            styles.push((coat, hw));
            for (a, b) in tex::text_segments(s, origin, h, dir, up) {
                let pa = atlas.pixel(view, plane_point(view, a, left));
                let pb = atlas.pixel(view, plane_point(view, b, left));
                tx.segment(pa, pb, hw + 1.5, tag);
            }
        };
        for left in [false, true] {
            // Reading direction along z: toward the front on the right side.
            let dir = (if left { Vec2::X } else { Vec2::NEG_X }, Vec2::Y);
            text(
                View::Side,
                left,
                "27",
                Vec2::new(-0.12, 0.70),
                0.15,
                dir,
                black,
                0.8,
            );
            text(
                View::Side,
                left,
                "KAVRO",
                Vec2::new(0.68, 0.72),
                0.10,
                dir,
                white,
                0.6,
            );
            text(
                View::Side,
                left,
                "ZENTRIQ",
                Vec2::new(-0.53, 0.585),
                0.045,
                dir,
                white,
                0.55,
            );
            text(
                View::Side,
                left,
                "A.VENTRIS",
                Vec2::new(-0.12, 0.873),
                0.028,
                dir,
                white,
                0.5,
            );
            text(
                View::Side,
                left,
                "K.HOLM",
                Vec2::new(-0.12, 0.835),
                0.028,
                dir,
                white,
                0.5,
            );
            text(
                View::Side,
                left,
                "VORANTE",
                Vec2::new(0.02, 0.33),
                0.05,
                dir,
                navy,
                0.6,
            );
        }
        // Roof number readable from behind, hood lettering readable from the front.
        text(
            View::Top,
            false,
            "27",
            Vec2::new(0.85, 0.0),
            0.30,
            (Vec2::Y, Vec2::NEG_X),
            black,
            0.8,
        );
        text(
            View::Top,
            false,
            "KAVRO",
            Vec2::new(-1.02, 0.0),
            0.10,
            (Vec2::NEG_Y, Vec2::X),
            white,
            0.6,
        );
        text(
            View::Back,
            false,
            "27",
            Vec2::new(0.0, 0.90),
            0.05,
            (Vec2::X, Vec2::Y),
            black,
            0.75,
        );
        text(
            View::Back,
            false,
            "KAVRO",
            Vec2::new(0.0, 0.50),
            0.075,
            (Vec2::X, Vec2::Y),
            navy,
            0.6,
        );
    }

    // --- Shapes ---------------------------------------------------------------------------
    let wave = Curve::new(&[
        (-2.15, 0.60),
        (-1.6, 0.56),
        (-0.9, 0.47),
        (0.0, 0.44),
        (0.9, 0.50),
        (1.5, 0.63),
        (2.15, 0.74),
    ]);
    let corner_h = Table::from_fn(-1.0, 2.05, 400, |z| (body.corner(z).1, 0.0));
    // The sweep: white below the wave line, an orange stripe and a white pinstripe above.
    let sweep = |c: Coat, h: f32, w: f32| -> Coat {
        let c = c.mix(white, fill(h - w, px_m));
        let c = c.mix(orange, fill((h - (w + 0.018)).abs() - 0.008, px_m));
        c.mix(white, fill((h - (w + 0.037)).abs() - 0.003, px_m))
    };
    // A white panel with an orange inner border.
    let panel = |c: Coat, sd: f32| -> Coat {
        let c = c.mix(white, fill(sd, px_m));
        c.mix(orange, fill((sd + 0.008).abs() - 0.004, px_m))
    };
    let trim_line = |z: f32, h: f32| body.trim_field(Vec3::new(0.9, h - GROUND, z));
    let base = |view: View, q: Vec2| -> Coat {
        match view {
            View::Side => {
                let (z, h) = (q.x, q.y);
                let mut c = sweep(blue, h, wave.at(z));
                // Blacked-out window surrounds and pillars between the door glass.
                if (-0.70..1.62).contains(&z) && h > belt(z) + 0.004 && h < corner_h.at(z) - 0.035 {
                    c = gloss_black;
                }
                c = panel(
                    c,
                    sd_round_rect(q - Vec2::new(-0.12, 0.70), Vec2::new(0.16, 0.105), 0.03),
                );
                // Sponsor logo: an orange disc with a white chevron.
                let l = q - Vec2::new(0.68, 0.855);
                c = c.mix(orange, fill(l.length() - 0.048, px_m));
                let chevron = sdf::seg_dist2(l, Vec2::new(-0.024, 0.016), Vec2::new(0.0, -0.016))
                    .min(sdf::seg_dist2(
                        l,
                        Vec2::new(0.0, -0.016),
                        Vec2::new(0.024, 0.016),
                    ))
                    .sqrt();
                c = c.mix(white, fill(chevron - 0.007, px_m));
                c.mix(plastic, fill(trim_line(z, h), px_m))
            }
            View::Top => {
                // Roof number panel.
                let (z, x) = (q.x, q.y);
                panel(
                    blue,
                    sd_round_rect(Vec2::new(z - 0.85, x), Vec2::new(0.22, 0.29), 0.04),
                )
            }
            View::Front => {
                let (x, h) = (q.x, q.y);
                // Meets the side's sweep line at the flare seams.
                // The number plate is its own part (`number_plate`).
                let c = sweep(blue, h, 0.505 + 0.061 * (x / 0.85).powi(2));
                c.mix(plastic, fill(trim_line(-2.05, h), px_m))
            }
            _ => {
                let (x, h) = (q.x, q.y);
                let mut c = sweep(blue, h, 0.66 + 0.08 * (x / 0.85).powi(2));
                c = panel(
                    c,
                    sd_round_rect(q - Vec2::new(0.0, 0.90), Vec2::new(0.15, 0.04), 0.012),
                );
                c.mix(plastic, fill(trim_line(2.05, h), px_m))
            }
        }
    };
    // How much gravel dust a spot collects (0..1).
    let grime = |view: View, q: Vec2| -> f32 {
        match view {
            View::Side => {
                let (z, h) = (q.x, q.y);
                let mut g = 0.72 * (1.0 - sstep(0.16, 0.5, h));
                for zc in [FRONT_AXLE, REAR_AXLE] {
                    let dz = z - zc;
                    // (The arch distance is never below |dz|.)
                    if dz.abs() < ARCH + 0.14 {
                        g = g.max(0.8 * bump(ARCH + 0.04, 0.09, arch_rho(dz, h - WHEEL_H)));
                    }
                    let behind = sstep(0.2, 0.5, dz) * sstep(1.5, 0.8, dz);
                    g = g.max(behind * (1.0 - sstep(0.3, 0.9, h)));
                }
                g
            }
            View::Front => 0.12 + 0.75 * (1.0 - sstep(0.22, 0.52, q.y)),
            View::Back => (0.95 * (1.0 - sstep(0.3, 1.0, q.y))).max(0.22),
            _ => 0.06 + 0.12 * sstep(-1.75, -1.95, q.x),
        }
    };
    // Metallic flake: a 64² tile of random tilts.
    let flake: Vec<(f32, f32)> = (0..64 * 64)
        .map(|i| {
            let (x, y) = (i % 64, i / 64);
            (
                0.015 * (tex::hash(x, y, 1) - 0.5),
                0.015 * (tex::hash(x, y, 2) - 0.5),
            )
        })
        .collect();
    let groove = |d: f32| 1.0 - sstep(0.2, 2.4, d);
    // Dust blotches: a tiling fractal noise in atlas pixels (period 512 px ≈ 1.7 m, blotches
    // ≈ 14 cm down to 2 cm).
    let tile = 512u32;
    let blotch = tex::par_buffer(tile, tile, |x, y| {
        tex::fbm(
            Vec2::new(x as f32, y as f32) * (12.0 / tile as f32),
            4,
            20,
            12,
        )
    });
    let encode = |c: Coat, nx: f32, ny: f32| -> [[u8; 4]; 3] {
        [
            [
                tex::srgb(c.color.x),
                tex::srgb(c.color.y),
                tex::srgb(c.color.z),
                255,
            ],
            tex::encode_normal(nx, ny),
            [
                0,
                (c.rough.clamp(0.0, 1.0) * 255.0) as u8,
                (c.metal.clamp(0.0, 1.0) * 255.0) as u8,
                255,
            ],
        ]
    };
    tex::par_images(size, size, [true, false, false], |x, y| {
        let (xi, yi) = (x as i32, y as i32);
        let px = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
        let (view, q, left) = match atlas.region_at(px) {
            Some(r) => r,
            None => return encode(underbody, 0.0, 0.0),
        };
        let mut c = base(view, q);
        // Lettering.
        let i = (y * size + x) as usize;
        let d = tx.dist[i];
        if d < 40.0 {
            let (coat, hw) = styles[tx.tag[i] as usize];
            c = c.mix(coat, (hw + 0.5 - d).clamp(0.0, 1.0));
        }
        // Shut lines: dark gaps and groove slopes in the normal map.
        let (fx, fy) = flake[((yi & 63) * 64 + (xi & 63)) as usize];
        let (mut nx, mut ny) = (fx * c.flake, fy * c.flake);
        let ds = cv.dist[i];
        if ds < 4.0 {
            c.color *= 1.0 - 0.92 * (1.0 - sstep(0.4, 1.6, ds));
            nx += 0.45 * (groove(cv.at(xi + 1, yi)) - groove(cv.at(xi - 1, yi)));
            ny -= 0.45 * (groove(cv.at(xi, yi + 1)) - groove(cv.at(xi, yi - 1)));
        }
        // Gravel dust: blotches thickening toward the sills and behind the wheels, fine
        // speckles, gritty normals.
        let g = grime(view, q);
        if g > 0.02 {
            let n = blotch[((yi & 511) * 512 + (xi & 511)) as usize];
            let t = g + 0.7 * (n - 0.5);
            let mut cov = sstep(0.42, 0.64, t);
            // Speckles: round flecks, one chance per 8 × 8 pixel cell where the dust is thick.
            let (cx, cy) = (xi >> 3, yi >> 3);
            if g > 0.35 && tex::hash(cx, cy, 60 + left as u32) < 0.5 * (g - 0.35) {
                let off = Vec2::new(tex::hash(cx, cy, 61), tex::hash(cx, cy, 62)) * 4.0
                    + Vec2::splat(2.0);
                let r = 0.8 + 1.4 * tex::hash(cx, cy, 63);
                let d = (Vec2::new((xi & 7) as f32 + 0.5, (yi & 7) as f32 + 0.5) - off).length();
                cov = cov.max(0.85 * (r + 0.5 - d).clamp(0.0, 1.0));
            }
            if cov > 0.0 {
                let dust = Coat::new(
                    dust_dark.lerp(dust_light, (2.0 * n - 0.4).clamp(0.0, 1.0)),
                    0.0,
                    0.88,
                    0.0,
                );
                c = c.mix(dust, cov * 0.94);
                nx += cov * 0.18 * (tex::hash(xi, yi, 70) - 0.5);
                ny += cov * 0.18 * (tex::hash(xi, yi, 71) - 0.5);
            }
        }
        encode(c, nx, ny)
    })
}

/// Side-view contour of an arch seam at `rho` around axle `zc`, from height `h0` on the front
/// side over the top to `h0` on the rear side.
fn arch_contour(zc: f32, rho: f32, h0: f32) -> Vec<Vec2> {
    let mut out = Vec::new();
    let n = 40;
    for i in 0..=n {
        // Angle from the front-bottom through the top to the rear-bottom.
        let a = std::f32::consts::PI * (i as f32 / n as f32);
        let (dz, dh) = (-a.cos(), a.sin());
        // Scale the direction onto the rho level set (cube norm above the axle).
        let r = arch_rho(dz, dh);
        let p = Vec2::new(zc + dz / r * rho, WHEEL_H + dh / r * rho);
        out.push(p);
    }
    // Extend straight down to `h0` at both ends.
    let first = out[0];
    let last = *out.last().unwrap();
    out.insert(0, Vec2::new(first.x, h0));
    out.push(Vec2::new(last.x, h0));
    out
}

/// A closed polygon grown outward by `d` (vertex normals from the neighbouring edges).
fn offset_loop(poly: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = poly.len();
    let area: f32 = (0..n).map(|i| poly[i].perp_dot(poly[(i + 1) % n])).sum();
    let s = if area > 0.0 { 1.0 } else { -1.0 };
    let mut out: Vec<Vec2> = (0..n)
        .map(|i| {
            let (a, b, c) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let e1 = (b - a).normalize_or(Vec2::X);
            let e2 = (c - b).normalize_or(Vec2::X);
            // Outward normals of a counter-clockwise loop point right of the edges.
            let n1 = Vec2::new(e1.y, -e1.x) * s;
            let n2 = Vec2::new(e2.y, -e2.x) * s;
            let m = (n1 + n2).normalize_or(n1);
            b + m * (d / m.dot(n1).max(0.5))
        })
        .collect();
    out.push(out[0]);
    out
}

/// The windshield banner: team name on dark navy with orange bars (u reads from the outside).
fn banner_texture() -> Image {
    let (w, h) = (1024u32, 128u32);
    let mut cv = Canvas::new(w, h);
    let size = 74.0;
    let text = "ZENTRIQ";
    let tw = tex::text_width(text, size);
    let origin = Vec2::new((w as f32 - tw) * 0.5, (h as f32 + size) * 0.5 - 4.0);
    let hw = 0.62 * size / 6.0;
    for (a, b) in tex::text_segments(text, origin, size, Vec2::X, Vec2::NEG_Y) {
        cv.segment(a, b, hw + 1.5, 1);
    }
    let navy = lin(0x0d1b3d);
    let orange = lin(0xff5a1f);
    tex::par_image(w, h, true, |x, y| {
        let d = cv.at(x as i32, y as i32);
        let t = (hw + 0.5 - d).clamp(0.0, 1.0);
        let u = (x as f32 + 0.5) / w as f32;
        let v = (y as f32 + 0.5) / h as f32;
        let bar =
            ((0.06..0.26).contains(&u) || (0.74..0.94).contains(&u)) && (0.40..0.56).contains(&v);
        let line = (0.86..0.91).contains(&v);
        let mut c = navy;
        if bar || line {
            c = orange;
        }
        c = c.lerp(Vec3::splat(0.92), t);
        [tex::srgb(c.x), tex::srgb(c.y), tex::srgb(c.z), 255]
    })
}

// ---------------------------------------------------------------------------- lights

/// Headlights (cover lens, reflector and projector modules, LED guides, amber indicator) and
/// tail lights at the hatch corners with reverse lamps and a central rain light.
fn lights(kit: &mut Kit, plain: Field) {
    let k = |name: &str| key(PREFIX, name);
    let top = Curve::new(&[(0.49, 0.668), (0.58, 0.731), (0.80, 0.748), (0.866, 0.711)]);
    let bot = Curve::new(&[(0.49, 0.668), (0.56, 0.626), (0.74, 0.621), (0.866, 0.642)]);
    let (x0, x1) = (0.49, 0.866);
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
        24,
        8,
        &|_, _| 0.0008,
        false,
        &|s, t| Vec2::new(s, t),
    );
    kit.meshes
        .push((k("head_lens"), merge([mirror_x(&lens), lens])));
    let mut chrome = Vec::new();
    let mut glass = Vec::new();
    for (q, r) in [
        (Vec2::new(0.615, 0.68), 0.032f32),
        (Vec2::new(0.765, 0.688), 0.027),
    ] {
        if let Some((c, g)) = lamp_module(plain, q, r, GROUND, 0.024) {
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
    // LED guides along the top and bottom of the lens.
    let guide: Vec<Vec2> = (0..=24)
        .map(|i| {
            let x = 0.52 + (0.845 - 0.52) * i as f32 / 24.0;
            Vec2::new(x, top.at(x) - 0.009)
        })
        .collect();
    let on = surface_polyline(plain, Proj::Front, &guide, GROUND, 0.012);
    let drl_top = tube_on(&on, -0.007, 0.0042, 6);
    let lower: Vec<Vec2> = (0..=12)
        .map(|i| {
            let x = 0.56 + (0.80 - 0.56) * i as f32 / 12.0;
            Vec2::new(x, bot.at(x) + 0.008)
        })
        .collect();
    let on = surface_polyline(plain, Proj::Front, &lower, GROUND, 0.012);
    let drl = merge([drl_top, tube_on(&on, -0.008, 0.003, 6)]);
    kit.meshes.push((k("drl"), merge([mirror_x(&drl), drl])));
    let on = surface_polyline(
        plain,
        Proj::Front,
        &pts(&[(0.853, 0.656), (0.856, 0.700)]),
        GROUND,
        0.005,
    );
    let amber = tube_on(&on, -0.008, 0.0045, 6);
    kit.meshes
        .push((k("amber"), merge([mirror_x(&amber), amber])));

    // Tail lights.
    let tail_lo = Curve::new(&[(0.0, 0.855), (0.50, 0.855), (0.64, 0.84), (0.79, 0.846)]);
    let tail_hi = Curve::new(&[(0.0, 0.995), (0.50, 0.995), (0.64, 1.02), (0.79, 1.03)]);
    let segment =
        |xa: f32, xb: f32, ns: usize, lo_h: &dyn Fn(f32) -> f32, hi_h: &dyn Fn(f32) -> f32| {
            let lo = |s: f32| {
                let x = xa + (xb - xa) * s;
                Vec2::new(x, lo_h(x.abs()))
            };
            let hi = |s: f32| {
                let x = xa + (xb - xa) * s;
                Vec2::new(x, hi_h(x.abs()))
            };
            let strip = Strip { lo: &lo, hi: &hi };
            draped_slab(
                plain,
                Proj::Back,
                GROUND,
                &strip,
                ns,
                6,
                &|_, t| 0.004 + 0.004 * (1.0 - (2.0 * t - 1.0).powi(2)),
                true,
                &|s, t| Vec2::new(s * (xb - xa) * 8.0, t),
            )
        };
    let lo_h = |x: f32| tail_lo.at(x);
    let hi_h = |x: f32| tail_hi.at(x);
    let rain_lo = |_: f32| 1.262;
    let rain_hi = |_: f32| 1.279;
    let red = merge([
        segment(-0.79, -0.565, 14, &lo_h, &hi_h),
        segment(0.565, 0.79, 14, &lo_h, &hi_h),
        segment(-0.10, 0.10, 8, &rain_lo, &rain_hi),
    ]);
    kit.meshes.push((k("tail_red"), red));
    kit.meshes.push((
        k("tail_reverse"),
        merge([
            segment(-0.555, -0.50, 4, &lo_h, &hi_h),
            segment(0.50, 0.555, 4, &lo_h, &hi_h),
        ]),
    ));
    kit.images.push((k("tail_glow"), tail_texture()));
}

/// Louvre slats across a vent outline in plan view (x, z): blades `pitch` apart along z, `chord`
/// deep, leaning back by `tilt_deg` from vertical... toward the rear, their top edges `inset`
/// below the plain surface (ends reaching 1 cm into the pocket walls).
fn slats(
    plain: Field,
    outline: &[Vec2],
    pitch: f32,
    chord: f32,
    tilt_deg: f32,
    inset: f32,
) -> Mesh {
    let n = outline.len();
    let (z0, z1) = outline
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    let (s, c) = tilt_deg.to_radians().sin_cos();
    let mut parts = Vec::new();
    let mut z = z0 + pitch * 0.5;
    while z < z1 {
        // The outline's x extent at this z.
        let (mut xa, mut xb) = (f32::MAX, f32::MIN);
        for i in 0..n {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            if (a.y - z) * (b.y - z) <= 0.0 && (a.y - b.y).abs() > 1e-6 {
                let x = a.x + (b.x - a.x) * (z - a.y) / (b.y - a.y);
                xa = xa.min(x);
                xb = xb.max(x);
            }
        }
        if xa < xb {
            let (xa, xb) = (xa - 0.01, xb + 0.01);
            let cols = 6;
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for j in 0..=cols {
                let x = xa + (xb - xa) * j as f32 / cols as f32;
                let y = surface_point(plain, Proj::Top, Vec2::new(x, z), GROUND)
                    .map_or(0.4, |(p, _)| p.y)
                    - inset;
                top.push(Vec3::new(x, y, z));
                bottom.push(Vec3::new(x, y - chord * c, z + chord * s));
            }
            parts.push(slab(&[top, bottom], 0.0025, &|_, _, _| Vec2::ZERO));
        }
        z += pitch;
    }
    merge(parts)
}

/// Honeycomb inserts: grille, lower intake, bumper corner vents and the roof scoop's mouth;
/// slatted hood louvres and roof vent.
fn grilles(body: &Body, plain: Field) -> Mesh {
    let f = &body.features;
    let grille = honeycomb(
        plain,
        Proj::Front,
        GROUND,
        &f.grille,
        0.020,
        0.0033,
        0.012,
        0.035,
    );
    let intake = honeycomb(
        plain,
        Proj::Front,
        GROUND,
        &f.intake,
        0.022,
        0.0035,
        0.013,
        0.045,
    );
    let corner = honeycomb(
        plain,
        Proj::Front,
        GROUND,
        &f.corner,
        0.017,
        0.003,
        0.012,
        0.03,
    );
    let louvre = slats(plain, &f.louvre, 0.028, 0.026, 50.0, 0.006);
    let roof = slats(plain, &f.roof_vent_outline(), 0.026, 0.02, 45.0, 0.005);
    let r = body.roof_at(0.02, 0.0);
    let mouth = pts(&[
        (-0.14, r + 0.008),
        (0.14, r + 0.008),
        (0.14, r + 0.065),
        (-0.14, r + 0.065),
    ]);
    let plane = |p: Vec3| 0.05 - p.z;
    let scoop = honeycomb(
        &plane,
        Proj::Front,
        GROUND,
        &mouth,
        0.015,
        0.0026,
        0.014,
        0.0,
    );
    merge([
        grille,
        intake,
        mirror_x(&corner),
        corner,
        mirror_x(&louvre),
        louvre,
        roof,
        scoop,
    ])
}

// ---------------------------------------------------------------------------- aero and underbody

/// A cambered airfoil section (unit chord, closed loop from the trailing edge over the top to
/// the leading edge and back under), NACA-style thickness `thick` and camber `camber`.
fn airfoil(n: usize, thick: f32, camber: f32) -> Vec<Vec2> {
    let p = 0.4;
    let xs: Vec<f32> = (0..=n)
        .map(|i| 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / n as f32).cos()))
        .collect();
    let yt = |x: f32| {
        5.0 * thick
            * (0.2969 * x.sqrt() - 0.126 * x - 0.3516 * x * x + 0.2843 * x.powi(3)
                - 0.1036 * x.powi(4))
    };
    let yc = |x: f32| {
        if x < p {
            camber / (p * p) * (2.0 * p * x - x * x)
        } else {
            camber / ((1.0 - p) * (1.0 - p)) * (1.0 - 2.0 * p + 2.0 * p * x - x * x)
        }
    };
    let mut out: Vec<Vec2> = xs
        .iter()
        .rev()
        .map(|&x| Vec2::new(x, yc(x) + yt(x).max(0.0015)))
        .collect();
    out.extend(
        xs.iter()
            .skip(1)
            .map(|&x| Vec2::new(x, yc(x) - yt(x).max(0.0015))),
    );
    out.pop();
    out
}

/// One wing element spanning |x| ≤ `half`: an inverted airfoil (downforce) with its leading
/// edge at side-view point `le` (z, h), chord `chord`, trailing edge raised by `aoa` degrees.
fn wing_element(le: Vec2, chord: f32, aoa: f32, thick: f32, camber: f32, half: f32) -> Mesh {
    let a = aoa.to_radians();
    let along = Vec2::new(a.cos(), a.sin());
    let up = Vec2::new(-a.sin(), a.cos());
    let section: Vec<Vec2> = airfoil(14, thick, camber)
        .into_iter()
        .map(|q| le + (along * q.x - up * q.y) * chord)
        .collect();
    let ring = |x: f32| -> Vec<Vec3> {
        section
            .iter()
            .map(|q| Vec3::new(x, q.y - GROUND, q.x))
            .collect()
    };
    loft_rings(&[ring(-half), ring(half)], true)
}

/// Trailing edge of a wing element.
fn trailing_edge(le: Vec2, chord: f32, aoa: f32) -> Vec2 {
    let a = aoa.to_radians();
    le + Vec2::new(a.cos(), a.sin()) * chord
}

/// An outline in side view (z, h) extruded across x from `x0` to `x1`, with UVs from
/// `uv(z, h, nx)` (`nx`: the face's normal x, ±1 on the flat faces, 0 on the edges).
fn plate_x(outline: &[Vec2], x0: f32, x1: f32, uv: &dyn Fn(f32, f32, f32) -> Vec2) -> Mesh {
    let n = outline.len();
    let mut m = Mesh::default();
    let c = outline.iter().copied().sum::<Vec2>() / n as f32;
    let p3 = |x: f32, q: Vec2| Vec3::new(x, q.y - GROUND, q.x);
    for (x, nx) in [(x0, -1.0f32), (x1, 1.0)] {
        let base = m.vertices.len() as u32;
        m.vertices
            .push(Vertex::new(p3(x, c), Vec3::X * nx, uv(c.x, c.y, nx)));
        for q in outline {
            m.vertices
                .push(Vertex::new(p3(x, *q), Vec3::X * nx, uv(q.x, q.y, nx)));
        }
        for i in 0..n as u32 {
            m.indices
                .extend_from_slice(&[base, base + 1 + i, base + 1 + (i + 1) % n as u32]);
        }
    }
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let d = (b - a).normalize_or(Vec2::X);
        let mut nrm = Vec3::new(0.0, -d.x, d.y);
        let away = (a + b) * 0.5 - c;
        if nrm.dot(Vec3::new(0.0, away.y, away.x)) < 0.0 {
            nrm = -nrm;
        }
        let base = m.vertices.len() as u32;
        for (x, q) in [(x0, a), (x0, b), (x1, b), (x1, a)] {
            m.vertices
                .push(Vertex::new(p3(x, q), nrm, uv(q.x, q.y, 0.0)));
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    orient_like_normals(&mut m);
    m
}

/// The rear wing: main plane and flap with a gurney lip on two swan-neck pylons (carbon), and
/// the painted endplates: (carbon, endplates).
fn rear_wing(plain: Field) -> (Mesh, Mesh) {
    let half = 0.715;
    let le = Vec2::new(1.70, 1.36);
    let main = wing_element(le, 0.26, 8.0, 0.12, 0.05, half);
    let te = trailing_edge(le, 0.26, 8.0);
    let flap_le = te + Vec2::new(-0.03, 0.02);
    let flap = wing_element(flap_le, 0.09, 28.0, 0.10, 0.05, half);
    let fte = trailing_edge(flap_le, 0.09, 28.0);
    let gurney = plate_x(
        &[
            fte + Vec2::new(-0.004, -0.004),
            fte + Vec2::new(0.0, -0.004),
            fte + Vec2::new(0.002, 0.014),
            fte + Vec2::new(-0.002, 0.014),
        ],
        -half,
        half,
        &|_, _, _| Vec2::ZERO,
    );
    // Pylons from the hatch top up into the main plane.
    let pylon_outline = pts(&[
        (1.58, 1.29),
        (1.72, 1.25),
        (1.88, 1.365),
        (1.80, 1.37),
        (1.66, 1.335),
    ]);
    let mut carbon = vec![main, flap, gurney];
    for x in [-0.30f32, 0.30] {
        carbon.push(plate_x(&pylon_outline, x - 0.006, x + 0.006, &|_, _, _| {
            Vec2::ZERO
        }));
    }
    let mut carbon = merge(carbon);
    for v in &mut carbon.vertices {
        v.uv = [v.position[0], v.position[2] + v.position[1]];
    }
    // Endplates standing on the C-pillars and rear quarters: the lower edge follows the body
    // 1.5 cm below its surface, the top sweeps up to the flap. Decals read right from either
    // side.
    let skin = |z: f32| {
        let from = Vec3::new(half + 0.004, 1.8 - GROUND, z);
        raycast(plain, from, Vec3::NEG_Y, 1.4).map_or(1.0, |p| p.y + GROUND) - 0.015
    };
    let (z0, z1) = (1.60, 2.0);
    let mut outline: Vec<Vec2> = (0..=12)
        .map(|i| {
            let z = z0 + (z1 - z0) * i as f32 / 12.0;
            Vec2::new(z, skin(z))
        })
        .collect();
    outline.extend(pts(&[
        (2.025, 1.36),
        (2.025, 1.47),
        (1.95, 1.49),
        (1.80, 1.465),
        (1.70, 1.425),
        (1.63, 1.375),
    ]));
    let (h0, h1) = outline
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.y), b.max(q.y)));
    let (z0, z1) = (z0, 2.025);
    let uv = |z: f32, h: f32, nx: f32| {
        let u = if nx > 0.0 {
            (z1 - z) / (z1 - z0)
        } else {
            (z - z0) / (z1 - z0)
        };
        Vec2::new(u, (h1 - h) / (h1 - h0))
    };
    let right = plate_x(&outline, half, half + 0.008, &uv);
    let left = plate_x(&outline, -half - 0.008, -half, &uv);
    (carbon, merge([right, left]))
}

/// Endplate decal: blue with white lettering and an orange stripe.
fn plate_texture() -> Image {
    let (w, h) = (512u32, 256u32);
    let mut cv = Canvas::new(w, h);
    let size = 64.0;
    let text = "KAVRO";
    let tw = tex::text_width(text, size);
    let origin = Vec2::new((w as f32 - tw) * 0.5, 150.0);
    let hw = 0.62 * size / 6.0;
    for (a, b) in tex::text_segments(text, origin, size, Vec2::X, Vec2::NEG_Y) {
        cv.segment(a, b, hw + 1.5, 1);
    }
    let blue = lin(0x1747a6);
    let orange = lin(0xff5a1f);
    tex::par_image(w, h, true, |x, y| {
        let d = cv.at(x as i32, y as i32);
        let t = (hw + 0.5 - d).clamp(0.0, 1.0);
        let v = (y as f32 + 0.5) / h as f32;
        let u = (x as f32 + 0.5) / w as f32;
        let stripe = (v - (0.80 - 0.15 * u)).abs() < 0.035;
        let c = if stripe { orange } else { blue }.lerp(Vec3::splat(0.92), t);
        [tex::srgb(c.x), tex::srgb(c.y), tex::srgb(c.z), 255]
    })
}

/// The front number plate: a 30 × 6 cm plate standing 7 mm proud of the bumper bar between
/// the grille and the intake (painted on, its edges would wrap into their rims).
fn number_plate(plain: Field) -> Mesh {
    const HALF: Vec2 = Vec2::new(0.15, 0.03);
    const MID: f32 = 0.5525;
    let cols = 10;
    // Rows top to bottom, so the grid's normal (and the textured face) points forward.
    let grid: Vec<Vec<Vec3>> = [MID + HALF.y, MID - HALF.y]
        .iter()
        .map(|&h| {
            (0..=cols)
                .map(|j| {
                    let x = -HALF.x + 2.0 * HALF.x * j as f32 / cols as f32;
                    surface_point(plain, Proj::Front, Vec2::new(x, MID), GROUND)
                        .map(|(p, n)| {
                            let up = Vec3::X.cross(n).normalize_or(Vec3::Y);
                            p + n * 0.007 + up * (h - MID)
                        })
                        .unwrap_or(Vec3::new(x, h - GROUND, -2.05))
                })
                .collect()
        })
        .collect();
    slab(&grid, 0.003, &|i, j, back| {
        if back {
            return Vec2::new(0.5, 0.5);
        }
        // Seen from the front, the car's +x is on the left of the plate.
        Vec2::new(1.0 - j as f32 / cols as f32, i as f32)
    })
}

/// The number plate's face: white with an orange border and the black 27.
fn number_texture() -> Image {
    let (w, h) = (512u32, 102u32);
    let mut cv = Canvas::new(w, h);
    let size = 66.0;
    let text = "27";
    let tw = tex::text_width(text, size);
    let origin = Vec2::new((w as f32 - tw) * 0.5, (h as f32 + size) * 0.5);
    let hw = 0.75 * size / 6.0;
    for (a, b) in tex::text_segments(text, origin, size, Vec2::X, Vec2::NEG_Y) {
        cv.segment(a, b, hw + 1.5, 1);
    }
    let white = lin(0xf2f2ef);
    let orange = lin(0xff5a1f);
    let black = lin(0x101012);
    tex::par_image(w, h, true, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let edge = fx.min(w as f32 - fx).min(fy).min(h as f32 - fy);
        let ink = (hw + 0.5 - cv.at(x as i32, y as i32)).clamp(0.0, 1.0);
        let border = (edge > 5.0 && edge < 12.0) as u8 as f32;
        let c = white.lerp(orange, border).lerp(black, ink);
        [tex::srgb(c.x), tex::srgb(c.y), tex::srgb(c.z), 255]
    })
}

/// A thin plate through a grid of points (rows × columns), `t` thick, both faces offset along
/// the grid normal, with border walls. `uv(row, col, back)` per face.
fn slab(grid: &[Vec<Vec3>], t: f32, uv: &dyn Fn(usize, usize, bool) -> Vec2) -> Mesh {
    let nr = grid.len();
    let nc = grid[0].len();
    let at = |i: usize, j: usize| grid[i][j];
    let normal = |i: usize, j: usize| {
        let du = at(i, (j + 1).min(nc - 1)) - at(i, j.saturating_sub(1));
        let dv = at((i + 1).min(nr - 1), j) - at(i.saturating_sub(1), j);
        du.cross(dv).normalize_or(Vec3::Z)
    };
    let mut m = Mesh::default();
    for (back, sign) in [(false, 1.0f32), (true, -1.0)] {
        let base = m.vertices.len() as u32;
        for i in 0..nr {
            for j in 0..nc {
                let n = normal(i, j) * sign;
                m.vertices
                    .push(Vertex::new(at(i, j) + n * (t * 0.5), n, uv(i, j, back)));
            }
        }
        for i in 0..nr - 1 {
            for j in 0..nc - 1 {
                let a = base + (i * nc + j) as u32;
                let (b, c, d) = (a + 1, a + nc as u32 + 1, a + nc as u32);
                m.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    let mut border = Vec::new();
    for j in 0..nc - 1 {
        border.push((0, j));
    }
    for i in 0..nr - 1 {
        border.push((i, nc - 1));
    }
    for j in (1..nc).rev() {
        border.push((nr - 1, j));
    }
    for i in (1..nr).rev() {
        border.push((i, 0));
    }
    let center = grid.iter().flatten().copied().sum::<Vec3>() / (nr * nc) as f32;
    for k in 0..border.len() {
        let (a, b) = (border[k], border[(k + 1) % border.len()]);
        let (pa, pb) = (at(a.0, a.1), at(b.0, b.1));
        let (na, nb) = (normal(a.0, a.1), normal(b.0, b.1));
        let mut out = (pb - pa).cross(na).normalize_or(Vec3::Y);
        if out.dot(pa - center) < 0.0 {
            out = -out;
        }
        let base = m.vertices.len() as u32;
        let u = uv(a.0, a.1, false);
        for p in [
            pa + na * (t * 0.5),
            pb + nb * (t * 0.5),
            pb - nb * (t * 0.5),
            pa - na * (t * 0.5),
        ] {
            m.vertices.push(Vertex::new(p, out, u));
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    orient_like_normals(&mut m);
    m
}

/// A rubber mud flap behind a wheel, spanning x0..x1 (increasing), hanging from `top` to
/// `bottom` (heights above the ground) at `z`, its lower edge swept back by the airflow.
fn mud_flap(x0: f32, x1: f32, z: f32, top: f32, bottom: f32) -> Mesh {
    let (nu, nv) = (4, 6);
    let grid: Vec<Vec<Vec3>> = (0..=nv)
        .map(|i| {
            let v = i as f32 / nv as f32;
            (0..=nu)
                .map(|j| {
                    let u = j as f32 / nu as f32;
                    Vec3::new(
                        x0 + (x1 - x0) * u,
                        top + (bottom - top) * v - GROUND,
                        z + 0.03 * v * v,
                    )
                })
                .collect()
        })
        .collect();
    // The rear face reads left to right from behind; the front face mirrored.
    slab(&grid, 0.01, &|i, j, back| {
        let u = j as f32 / nu as f32;
        let v = i as f32 / nv as f32;
        let normal_faces_back = (grid[0][1] - grid[0][0]).cross(grid[1][0] - grid[0][0]).z > 0.0;
        let rear = back != normal_faces_back;
        Vec2::new(if rear { u } else { 1.0 - u }, v)
    })
}

/// Mud flap print: red rubber, white fictional logo and lettering, mud along the bottom.
fn flap_texture() -> Image {
    let n = 256u32;
    let mut cv = Canvas::new(n, n);
    let size = 26.0;
    let text = "MORVIK";
    let tw = tex::text_width(text, size);
    let origin = Vec2::new((n as f32 - tw) * 0.5, 150.0);
    let hw = 0.6 * size / 6.0;
    for (a, b) in tex::text_segments(text, origin, size, Vec2::X, Vec2::NEG_Y) {
        cv.segment(a, b, hw + 1.5, 1);
    }
    // Logo: two nested chevrons.
    for (dy, tag) in [(0.0f32, 2u8), (16.0, 2)] {
        let c = Vec2::new(128.0, 80.0 + dy);
        cv.polyline(
            &[
                c + Vec2::new(-28.0, -10.0),
                c + Vec2::new(0.0, 10.0),
                c + Vec2::new(28.0, -10.0),
            ],
            7.0,
            tag,
        );
    }
    let red = lin(0xd8341a);
    let mud = lin(0x5d4f3f);
    tex::par_image(n, n, true, |x, y| {
        let d = cv.at(x as i32, y as i32);
        let hw = if cv.tag[(y * n + x) as usize] == 2 {
            4.5
        } else {
            hw
        };
        let t = (hw + 0.5 - d).clamp(0.0, 1.0);
        let v = (y as f32 + 0.5) / n as f32;
        let q = Vec2::new(x as f32, y as f32) / n as f32;
        let mut c = red.lerp(Vec3::splat(0.9), t);
        let g = sstep(0.62, 0.95, v) + 0.25;
        let m = sstep(0.45, 0.6, g + 0.5 * (tex::fbm(q * 9.0, 3, 5, 0) - 0.5));
        c = c.lerp(mud, m * 0.9);
        [tex::srgb(c.x), tex::srgb(c.y), tex::srgb(c.z), 255]
    })
}

/// The front skid plate (brushed aluminum, metric UVs) and its bolt heads.
fn skid_plate() -> (Mesh, Mesh) {
    let prof = [
        (-1.968f32, 0.262f32),
        (-1.945, 0.228),
        (-1.905, 0.200),
        (-1.84, 0.186),
        (-1.70, 0.182),
        (-1.30, 0.182),
        (-0.95, 0.182),
    ];
    let cols = 10;
    let grid: Vec<Vec<Vec3>> = prof
        .iter()
        .map(|&(z, h)| {
            let half = 0.62 - 0.06 * sstep(-1.80, -1.97, z);
            let wrap = sstep(-1.80, -1.95, z);
            (0..=cols)
                .map(|j| {
                    let x = -half + 2.0 * half * j as f32 / cols as f32;
                    // The lip follows the bumper's plan curve.
                    Vec3::new(x, h - GROUND, z + wrap * 0.20 * (x / 0.9).powi(2))
                })
                .collect()
        })
        .collect();
    let plate = slab(&grid, 0.006, &|i, j, _| {
        let p = grid[i][j];
        Vec2::new(p.z * 2.0, p.x * 2.0)
    });
    let mut bolts = Vec::new();
    for &x in &[-0.30f32, 0.0, 0.30] {
        let j = ((x + 0.56) / 1.12 * cols as f32).round() as usize;
        let a = grid[0][j];
        let b = grid[1][j];
        let p = (a + b) * 0.5;
        let along = (b - a).normalize_or(Vec3::Y);
        let n = Vec3::X.cross(along).normalize_or(Vec3::NEG_Z);
        let n = if n.z > 0.0 { -n } else { n };
        let head = lathe_x(
            &[
                Vec2::new(0.0, 0.0105),
                Vec2::new(0.002, 0.0095),
                Vec2::new(0.0045, 0.005),
                Vec2::new(0.005, 0.0),
            ],
            8,
            &[],
        );
        bolts.push(transformed(
            head,
            trs(
                p + n * 0.003,
                Quat::from_rotation_arc(Vec3::X, n),
                Vec3::ONE,
            ),
        ));
    }
    (plate, merge(bolts))
}

/// Brushed aluminum: fine lines along u (color variation and normal grooves), 256² tiling.
fn brushed_texture() -> (Image, Image) {
    let n = 256u32;
    let line = |y: i32, x: i32| -> f32 {
        // Streaks: per-row noise with slow variation along the row.
        let r = tex::hash(0, y.rem_euclid(n as i32), 3);
        let s = tex::value_noise(Vec2::new(x as f32 / 64.0, y as f32 * 0.5), 4, 0);
        0.6 * r + 0.4 * s
    };
    let dust = lin(0x8a7c68);
    tex::par_image2(n, n, (true, false), |x, y| {
        let (x, y) = (x as i32, y as i32);
        let v = line(y, x);
        let a = 0.42 + 0.12 * v;
        let q = Vec2::new(x as f32, y as f32) / n as f32;
        let cov = sstep(0.45, 0.7, tex::fbm(q * 4.0, 4, 9, 4)) * 0.85;
        let c = Vec3::splat(a).lerp(dust, cov);
        let dy = line(y + 1, x) - line(y - 1, x);
        (
            [tex::srgb(c.x), tex::srgb(c.y), tex::srgb(c.z), 255],
            tex::encode_normal(0.0, -0.25 * dy * (1.0 - cov)),
        )
    })
}

/// The big single exhaust tip in the rear valance (heat-tinted).
fn exhaust(body: &Body, plain: Field) -> Mesh {
    let (c, r) = body.features.exhaust;
    let ro = r - 0.006;
    let rim = surface_point(plain, Proj::Back, c + Vec2::new(0.0, r + 0.01), GROUND)
        .map(|(p, _)| p.z)
        .unwrap_or(2.0);
    let prof = [
        Vec2::new(-0.16, ro - 0.004),
        Vec2::new(0.008, ro - 0.004),
        Vec2::new(0.016, ro - 0.001),
        Vec2::new(0.018, ro + 0.002),
        Vec2::new(0.014, ro + 0.004),
        Vec2::new(-0.03, ro + 0.003),
    ];
    let mut tip = lathe_x(&prof, 32, &[]);
    for v in &mut tip.vertices {
        let depth = (-v.position[0]).clamp(0.0, 0.16) / 0.16;
        let inner = v.normal[1] * v.position[1] + v.normal[2] * v.position[2] < 0.0;
        let col = if inner {
            let k = 0.5 - 0.46 * depth.sqrt();
            Vec4::new(k * 0.9, k * 0.8, k * 0.75, 1.0)
        } else {
            // Bronze to blue heat tint toward the body.
            let t = ((0.018 - v.position[0]) / 0.05).clamp(0.0, 1.0);
            Vec4::new(1.0, 0.82, 0.62, 1.0).lerp(Vec4::new(0.55, 0.6, 0.85, 1.0), t)
        };
        v.color = col.to_array();
    }
    let at = trs(
        Vec3::new(c.x, c.y - GROUND, rim + 0.004),
        Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
        Vec3::ONE,
    );
    transformed(tip, at)
}

// ---------------------------------------------------------------------------- small parts

/// The door's pull handle (right side).
fn door_handles(shell: Field) -> Mesh {
    let mut parts = Vec::new();
    {
        let (z, h) = HANDLE;
        let lo = move |s: f32| Vec2::new(z - 0.06 + 0.12 * s, h - 0.009);
        let hi = move |s: f32| Vec2::new(z - 0.06 + 0.12 * s, h + 0.009);
        let strip = Strip { lo: &lo, hi: &hi };
        parts.push(draped_slab(
            shell,
            Proj::Side,
            GROUND,
            &strip,
            10,
            3,
            &|s, _| 0.0025 * sstep(0.0, 0.15, s) * sstep(1.0, 0.85, s) + 0.0008,
            true,
            &|s, t| Vec2::new(s, t),
        ));
    }
    merge(parts)
}

/// One long wiper parked along the bottom of the windshield, pivoting near the middle.
fn wiper(shell: Field) -> Mesh {
    let line = |x: f32| Vec2::new(x, -0.79 + 0.05 * (x / 0.65).powi(2) + 0.045);
    let blade: Vec<Vec2> = (0..=28)
        .map(|i| line(-0.56 + 0.98 * i as f32 / 28.0))
        .collect();
    let mut parts = Vec::new();
    let on = surface_polyline(shell, Proj::Top, &blade, GROUND, 0.02);
    parts.push(tube_on(&on, 0.006, 0.0045, 6));
    let mid = line(-0.07);
    let pivot = Vec2::new(0.12, -0.83);
    let arm: Vec<Vec2> = (0..=10).map(|i| pivot.lerp(mid, i as f32 / 10.0)).collect();
    let on = surface_polyline(shell, Proj::Top, &arm, GROUND, 0.02);
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
    merge(parts)
}

/// A short roof antenna: base and a whip raked back.
fn antenna(shell: Field) -> Mesh {
    let Some((p, n)) = surface_point(shell, Proj::Top, Vec2::new(0.0, 1.52), GROUND) else {
        return Mesh::default();
    };
    let base = lathe_x(
        &[
            Vec2::new(0.0, 0.014),
            Vec2::new(0.01, 0.012),
            Vec2::new(0.022, 0.006),
            Vec2::new(0.026, 0.0),
        ],
        10,
        &[],
    );
    let base = transformed(base, trs(p, Quat::from_rotation_arc(Vec3::X, n), Vec3::ONE));
    let tip = p + n * 0.02 + (Vec3::Y * 0.92 + Vec3::Z * 0.4).normalize() * 0.30;
    merge([base, parts::tube(&[p + n * 0.02, tip], 0.0018, 5)])
}

/// Quick-release hood pins near the hood's front corners.
fn hood_pins(shell: Field) -> Mesh {
    let mut parts = Vec::new();
    for x in [-0.50f32, 0.50] {
        let Some((p, n)) = surface_point(shell, Proj::Top, Vec2::new(x, -1.80), GROUND) else {
            continue;
        };
        let rot = Quat::from_rotation_arc(Vec3::X, n);
        let pin = lathe_x(
            &[
                Vec2::new(-0.002, 0.016),
                Vec2::new(0.002, 0.016),
                Vec2::new(0.004, 0.006),
                Vec2::new(0.016, 0.0055),
                Vec2::new(0.02, 0.003),
                Vec2::new(0.021, 0.0),
            ],
            12,
            &[1],
        );
        parts.push(transformed(pin, trs(p, rot, Vec3::ONE)));
        // The clip: a wire loop lying on the hood beside the pin.
        let side = n.cross(Vec3::Z).normalize_or(Vec3::X);
        parts.push(torus(p + n * 0.012 + side * 0.022, n, 0.018, 0.0016, 16, 4));
    }
    merge(parts)
}

/// Fabric tow straps: a loop at the front intake corner and one at the rear bumper.
fn tow_straps(plain: Field) -> Mesh {
    let mut parts = Vec::new();
    for (proj, q, back) in [
        (Proj::Front, Vec2::new(0.50, 0.295), false),
        (Proj::Back, Vec2::new(-0.56, 0.36), true),
    ] {
        let Some((p, _)) = surface_point(plain, proj, q, GROUND) else {
            continue;
        };
        let out = if back { Vec3::Z } else { Vec3::NEG_Z };
        // A teardrop loop in the vertical plane through the anchor, hanging out and down.
        let n = 18;
        let loop_pts: Vec<(Vec3, Vec3)> = (0..=n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let pt = p
                    + Vec3::NEG_Y * (0.045 * (1.0 - a.cos()))
                    + out * (0.006 + 0.02 * a.sin().max(-0.2));
                let tangent = Vec3::NEG_Y * (0.045 * a.sin()) + out * (0.02 * a.cos());
                (pt, tangent.cross(Vec3::X).normalize_or(out))
            })
            .collect();
        parts.push(ribbon_on(&loop_pts, 0.0, 0.03));
        // Both faces of the strap.
        let mut back_face = ribbon_on(&loop_pts, 0.0, 0.03);
        for v in &mut back_face.vertices {
            v.normal = (-Vec3::from(v.normal)).to_array();
        }
        orient_like_normals(&mut back_face);
        parts.push(back_face);
    }
    tint(merge(parts), Vec4::new(0.85, 0.09, 0.03, 1.0))
}

// ---------------------------------------------------------------------------- interior

/// The roll cage: main hoop with diagonal and harness bar, A-pillar hoop, roof rails and roof
/// cross, door bars, rear stays to the strut towers.
fn roll_cage(body: &Body) -> Mesh {
    let inset = 0.075;
    let r = 0.019;
    let y = |h: f32| h - GROUND;
    let side_x = |z: f32, h: f32| -> f32 {
        let x = if h > body.belt_at(z) {
            body.gh_side(z, h)
        } else {
            body.half_width(z, h)
        };
        x - inset
    };
    let roof_h = |z: f32, x: f32| body.roof_at(z, x) - inset;
    let corner = |z: f32| {
        let (x, h) = body.corner(z);
        Vec2::new(x - inset, h - inset)
    };
    // One side of a hoop in the plane z (from the floor up and over to the centerline) as
    // (x, h) points.
    let half_hoop = |z: f32, floor: f32| -> Vec<Vec2> {
        let c = corner(z);
        let mut v = Vec::new();
        let h_top = c.y - 0.08;
        for i in 0..=5 {
            let h = floor + (h_top - floor) * i as f32 / 5.0;
            v.push(Vec2::new(side_x(z, h), h));
        }
        let a = *v.last().unwrap();
        let e = Vec2::new(c.x - 0.08, roof_h(z, c.x - 0.08));
        for i in 1..=5 {
            let t = i as f32 / 5.0;
            v.push(a * (1.0 - t) * (1.0 - t) + c * (2.0 * t * (1.0 - t)) + e * (t * t));
        }
        for i in 1..=3 {
            let x = e.x * (1.0 - i as f32 / 3.0);
            v.push(Vec2::new(x, roof_h(z, x)));
        }
        v
    };
    let full = |half: &[Vec2], z: f32| -> Vec<Vec3> {
        let mut pts: Vec<Vec3> = half
            .iter()
            .rev()
            .map(|q| Vec3::new(-q.x, y(q.y), z))
            .collect();
        pts.extend(half.iter().skip(1).map(|q| Vec3::new(q.x, y(q.y), z)));
        pts
    };
    let mut tubes: Vec<Vec<Vec3>> = Vec::new();
    // Main hoop.
    let hoop = half_hoop(HOOP_Z, 0.30);
    tubes.push(full(&hoop, HOOP_Z));
    // Front hoop: down tubes at the firewall, up the A-pillars, across the header.
    let header_z = -0.12;
    let mut a_pillar: Vec<Vec3> = Vec::new();
    for i in 0..=3 {
        let h = 0.30 + (0.93 - 0.30) * i as f32 / 3.0;
        a_pillar.push(Vec3::new(side_x(-0.84, h), y(h), -0.84));
    }
    for i in 0..=8 {
        let z = -0.78 + (header_z + 0.02 + 0.78) * i as f32 / 8.0;
        let c = corner(z);
        a_pillar.push(Vec3::new(c.x, y(c.y), z));
    }
    let hc = corner(header_z);
    let mut header: Vec<Vec3> = Vec::new();
    for i in 0..=8 {
        let x = -(hc.x - 0.04) + 2.0 * (hc.x - 0.04) * i as f32 / 8.0;
        header.push(Vec3::new(x, y(roof_h(header_z, x)), header_z));
    }
    let mut front: Vec<Vec3> = a_pillar.iter().map(|p| Vec3::new(-p.x, p.y, p.z)).collect();
    front.extend(header);
    front.extend(a_pillar.iter().rev().copied());
    tubes.push(front);
    for s in [-1.0f32, 1.0] {
        // Roof rail from the header corner to the main hoop.
        let rail: Vec<Vec3> = (0..=6)
            .map(|i| {
                let z = header_z + (HOOP_Z - header_z) * i as f32 / 6.0;
                let c = corner(z);
                Vec3::new(s * c.x, y(c.y), z)
            })
            .collect();
        tubes.push(rail);
        // Door bars: an X between the A-pillar foot and the main hoop.
        for (ha, hb) in [(0.45f32, 0.86f32), (0.86, 0.42)] {
            let (za, zb) = (-0.72f32, HOOP_Z);
            let bar: Vec<Vec3> = (0..=6)
                .map(|i| {
                    let t = i as f32 / 6.0;
                    let (z, h) = (za + (zb - za) * t, ha + (hb - ha) * t);
                    Vec3::new(s * (side_x(z, h) - 0.01), y(h), z)
                })
                .collect();
            tubes.push(bar);
        }
        // Rear stay from the hoop's top corner to the strut tower.
        let c = corner(HOOP_Z);
        tubes.push(vec![
            Vec3::new(s * (c.x - 0.04), y(c.y - 0.03), HOOP_Z),
            Vec3::new(s * 0.52, y(0.80), 1.45),
        ]);
    }
    // Roof cross.
    let hoop_top = corner(HOOP_Z);
    for s in [-1.0f32, 1.0] {
        let cross: Vec<Vec3> = (0..=8)
            .map(|i| {
                let t = i as f32 / 8.0;
                let z = header_z + (HOOP_Z - header_z) * t;
                let x = s * ((hc.x - 0.06) * (1.0 - t) - (hoop_top.x - 0.06) * t);
                Vec3::new(x, y(roof_h(z, x.abs()) - 0.005), z)
            })
            .collect();
        tubes.push(cross);
    }
    // Hoop diagonal, harness bar, rear cross tube, dash bar.
    let bottom = Vec2::new(side_x(HOOP_Z, 0.32), 0.32);
    let top = Vec2::new(-(hoop_top.x - 0.06), hoop_top.y - 0.03);
    tubes.push(vec![
        Vec3::new(bottom.x, y(bottom.y), HOOP_Z),
        Vec3::new(top.x, y(top.y), HOOP_Z),
    ]);
    let hb = side_x(HOOP_Z, HARNESS_BAR_H);
    tubes.push(vec![
        Vec3::new(-hb, y(HARNESS_BAR_H), HOOP_Z),
        Vec3::new(hb, y(HARNESS_BAR_H), HOOP_Z),
    ]);
    tubes.push(vec![
        Vec3::new(-0.52, y(0.80), 1.45),
        Vec3::new(0.52, y(0.80), 1.45),
    ]);
    let db = side_x(-0.80, 0.86);
    tubes.push(vec![
        Vec3::new(-db, y(0.86), -0.80),
        Vec3::new(db, y(0.86), -0.80),
    ]);
    merge(tubes.iter().map(|t| parts::tube(t, r, 8)))
}

/// The bucket seat's field in its own frame (x across, y up from the seat floor, z back).
fn seat_field(p: Vec3) -> f32 {
    let rot = |p: Vec3, c: Vec3, deg: f32| Quat::from_rotation_x(-deg.to_radians()) * (p - c);
    let q = Vec3::new(p.x.abs(), p.y, p.z);
    let cushion = sd_round_box(
        rot(p, Vec3::new(0.0, 0.11, -0.06), -5.0),
        Vec3::new(0.2, 0.05, 0.22),
        0.035,
    );
    let thigh = sd_capsule(
        q,
        Vec3::new(0.19, 0.15, -0.27),
        Vec3::new(0.205, 0.17, 0.08),
        0.055,
    );
    let back = sd_round_box(
        rot(p, Vec3::new(0.0, 0.47, 0.22), 14.0),
        Vec3::new(0.21, 0.33, 0.04),
        0.035,
    );
    // Shoulder wings wrapping forward around the torso, head wings.
    let wing = sd_round_box(
        rot(q, Vec3::new(0.215, 0.50, 0.12), 14.0),
        Vec3::new(0.03, 0.27, 0.15),
        0.028,
    );
    let head = sd_round_box(
        rot(q, Vec3::new(0.155, 0.82, 0.22), 14.0),
        Vec3::new(0.03, 0.10, 0.11),
        0.028,
    );
    let rails = sd_round_box(
        q - Vec3::new(0.17, 0.025, -0.03),
        Vec3::new(0.018, 0.03, 0.24),
        0.008,
    );
    let mut d = smin(cushion, back, 0.04);
    d = smin(d, thigh, 0.03);
    d = smin(d, wing, 0.04);
    d = smin(d, head, 0.04);
    d = d.min(rails);
    // Harness slots through the back at shoulder height.
    let slot = sd_round_box(
        rot(q, Vec3::new(0.085, 0.685, 0.22), 14.0),
        Vec3::new(0.035, 0.014, 0.10),
        0.006,
    );
    rmax(d, -slot, 0.004)
}

/// A composite bucket seat with a six-point harness lying on it (seat frame), vertex-colored.
fn bucket_seat() -> Mesh {
    let field = |p: Vec3| seat_field(p);
    let seat = sculpt(
        &field,
        Vec3::new(-0.30, -0.02, -0.38),
        Vec3::new(0.30, 0.98, 0.42),
        0.023,
        true,
        0.0024,
        &|p, n| {
            let insert_cushion = p.x.abs() < 0.15 && n.y > 0.6 && p.z < 0.12 && p.y > 0.1;
            let insert_back = p.x.abs() < 0.15 && n.z < -0.6 && p.y > 0.22 && p.y < 0.8;
            // Charcoal fabric shell (albedo ~0.05) with blue inserts on a gray frame.
            if p.y < 0.06 {
                Vec4::new(0.09, 0.09, 0.095, 1.0)
            } else if insert_cushion || insert_back {
                Vec4::new(0.045, 0.07, 0.16, 1.0)
            } else {
                Vec4::new(0.048, 0.048, 0.052, 1.0)
            }
        },
    );
    // Harness straps laid onto the seat surface.
    let grad = |p: Vec3| sdf::gradient(&field, p, 0.002).normalize_or(Vec3::Y);
    let onto = |mut p: Vec3, off: f32| -> (Vec3, Vec3) {
        for _ in 0..8 {
            let d = field(p);
            p -= grad(p) * (d - off);
        }
        (p, grad(p))
    };
    let lay = |path: &[Vec3], n: usize| -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::new();
        for w in path.windows(2) {
            for i in 0..n {
                out.push(onto(w[0].lerp(w[1], i as f32 / n as f32), 0.006));
            }
        }
        out.push(onto(*path.last().unwrap(), 0.006));
        out
    };
    let bar = Vec3::new(0.0, HARNESS_BAR_H - SEAT_FLOOR + 0.02, HOOP_Z - SEAT_Z);
    let buckle = Vec3::new(0.0, 0.19, -0.08);
    let mut straps = Vec::new();
    for s in [-1.0f32, 1.0] {
        // Shoulder strap: from the harness bar through the slot, down the backrest.
        let slot_back = Vec3::new(s * 0.085, 0.69, 0.31);
        let mut path = vec![
            (bar + Vec3::X * (s * 0.085), Vec3::Y),
            (slot_back, Vec3::NEG_Z),
        ];
        path.extend(lay(
            &[
                Vec3::new(s * 0.085, 0.66, 0.16),
                Vec3::new(s * 0.07, 0.45, 0.10),
                Vec3::new(s * 0.045, 0.27, 0.06),
                Vec3::new(s * 0.03, 0.22, -0.02),
                buckle + Vec3::new(s * 0.02, 0.0, 0.01),
            ],
            4,
        ));
        straps.push(ribbon_on(&path, 0.0, 0.07));
        // Lap belt from the cushion side to the buckle.
        let lap = lay(
            &[
                Vec3::new(s * 0.22, 0.20, 0.05),
                Vec3::new(s * 0.12, 0.20, -0.02),
                buckle + Vec3::new(s * 0.035, 0.0, 0.0),
            ],
            4,
        );
        straps.push(ribbon_on(&lap, 0.0, 0.065));
        // Crotch strap from the cushion front.
        let crotch = lay(
            &[
                Vec3::new(s * 0.03, 0.14, -0.27),
                Vec3::new(s * 0.025, 0.18, -0.15),
                buckle + Vec3::new(s * 0.012, 0.0, -0.02),
            ],
            3,
        );
        straps.push(ribbon_on(&crotch, 0.0, 0.045));
    }
    let straps = tint(merge(straps), Vec4::new(0.7, 0.08, 0.03, 1.0));
    let (bp, bn) = onto(buckle, 0.012);
    let disc = lathe_x(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.03),
            Vec2::new(-0.006, 0.034),
        ],
        16,
        &[],
    );
    let disc = tint(
        transformed(
            disc,
            trs(bp, Quat::from_rotation_arc(Vec3::X, bn), Vec3::ONE),
        ),
        Vec4::new(0.5, 0.5, 0.53, 1.0),
    );
    merge([seat, straps, disc])
}

/// Dash with the driver's display binnacle, switch panel and co-driver's trip computer; the
/// two displays; and the controls (steering wheel, hydraulic handbrake, sequential shifter,
/// switches, fire extinguisher), vertex-colored.
fn dash_and_controls() -> (Mesh, Mesh, Mesh) {
    let y = |h: f32| h - GROUND;
    let field = |p: Vec3| {
        let dash = sd_round_box(
            p - Vec3::new(0.0, y(0.85), -0.74),
            Vec3::new(0.70, 0.08, 0.12),
            0.04,
        );
        let binnacle = sd_round_box(
            p - Vec3::new(SEATS[0], y(0.935), -0.66),
            Vec3::new(0.12, 0.03, 0.06),
            0.025,
        );
        let stack = sd_round_box(
            p - Vec3::new(0.0, y(0.67), -0.68),
            Vec3::new(0.15, 0.16, 0.07),
            0.03,
        );
        let tunnel = sd_round_box(
            p - Vec3::new(0.0, y(0.36), -0.22),
            Vec3::new(0.11, 0.10, 0.46),
            0.04,
        );
        let trip = sd_round_box(
            p - Vec3::new(SEATS[1], y(0.945), -0.70),
            Vec3::new(0.09, 0.03, 0.05),
            0.02,
        );
        smin(
            smin(smin(smin(dash, binnacle, 0.03), stack, 0.04), tunnel, 0.05),
            trip,
            0.02,
        )
    };
    let dash = sculpt(
        &field,
        Vec3::new(-0.78, y(0.22), -0.90),
        Vec3::new(0.78, y(0.99), 0.28),
        0.022,
        false,
        0.0012,
        &|_, _| Vec4::ONE,
    );
    let screen = |c: Vec3, w: f32, h: f32, u0: f32, u1: f32| {
        let mut m = Mesh::default();
        for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = c + Vec3::new(sx * w * 0.5, sy * h * 0.5, 0.0);
            let uv = Vec2::new(
                if sx < 0.0 { u0 } else { u1 },
                if sy < 0.0 { 1.0 } else { 0.0 },
            );
            m.vertices.push(Vertex::new(p, Vec3::Z, uv));
        }
        m.indices = vec![0, 1, 2, 0, 2, 3];
        orient_like_normals(&mut m);
        m
    };
    let screens = merge([
        screen(Vec3::new(SEATS[0], y(0.935), -0.598), 0.19, 0.05, 0.0, 0.62),
        screen(
            Vec3::new(SEATS[1], y(0.945), -0.648),
            0.14,
            0.045,
            0.64,
            1.0,
        ),
    ]);
    let mut controls = vec![steering_wheel_at(
        WHEEL_CENTER,
        steering_axis(),
        0.165,
        0.055,
    )];
    // Hydraulic handbrake and sequential shifter beside the driver.
    let lever = |a: Vec3, b: Vec3, r: f32, grip: Vec4| -> Mesh {
        let d = (b - a).normalize();
        let shaft = tint(
            parts::tube(&[a, b - d * 0.1], r, 8),
            Vec4::new(0.25, 0.25, 0.27, 1.0),
        );
        let handle = tint(parts::tube(&[b - d * 0.11, b], r * 1.6, 8), grip);
        merge([shaft, handle])
    };
    controls.push(lever(
        Vec3::new(-0.13, y(0.46), -0.16),
        Vec3::new(-0.16, y(0.84), -0.33),
        0.011,
        Vec4::new(0.03, 0.03, 0.03, 1.0),
    ));
    controls.push(lever(
        Vec3::new(-0.11, y(0.46), -0.36),
        Vec3::new(-0.19, y(0.80), -0.45),
        0.009,
        Vec4::new(0.04, 0.04, 0.045, 1.0),
    ));
    let knob = lathe_x(
        &[
            Vec2::new(-0.02, 0.0),
            Vec2::new(-0.018, 0.014),
            Vec2::new(0.0, 0.021),
            Vec2::new(0.018, 0.014),
            Vec2::new(0.022, 0.0),
        ],
        10,
        &[],
    );
    controls.push(tint(
        transformed(
            knob,
            trs(
                Vec3::new(-0.19, y(0.81), -0.455),
                Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                Vec3::ONE,
            ),
        ),
        Vec4::new(0.6, 0.05, 0.02, 1.0),
    ));
    // Switch panel on the center stack.
    let toggle = lathe_x(
        &[
            Vec2::new(0.0, 0.009),
            Vec2::new(0.004, 0.009),
            Vec2::new(0.006, 0.005),
            Vec2::new(0.016, 0.004),
            Vec2::new(0.018, 0.0),
        ],
        6,
        &[1],
    );
    for row in 0..4 {
        for col in 0..5 {
            let c = Vec3::new(
                -0.10 + 0.05 * col as f32,
                y(0.575 + 0.05 * row as f32),
                -0.61,
            );
            let color = match (row + col) % 4 {
                0 => Vec4::new(0.55, 0.03, 0.02, 1.0),
                1 => Vec4::new(0.6, 0.6, 0.6, 1.0),
                2 => Vec4::new(0.6, 0.42, 0.02, 1.0),
                _ => Vec4::new(0.03, 0.03, 0.03, 1.0),
            };
            controls.push(tint(
                transformed(
                    toggle.clone(),
                    trs(
                        c,
                        Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                        Vec3::ONE,
                    ),
                ),
                color,
            ));
        }
    }
    // Fire extinguisher on the tunnel between the seats.
    let bottle = lathe_x(
        &[
            Vec2::new(-0.17, 0.0),
            Vec2::new(-0.17, 0.05),
            Vec2::new(-0.15, 0.06),
            Vec2::new(0.12, 0.06),
            Vec2::new(0.15, 0.04),
            Vec2::new(0.18, 0.015),
            Vec2::new(0.19, 0.0),
        ],
        14,
        &[1],
    );
    controls.push(tint(
        transformed(
            bottle,
            trs(
                Vec3::new(0.0, y(0.52), 0.30),
                Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                Vec3::ONE,
            ),
        ),
        Vec4::new(0.7, 0.04, 0.02, 1.0),
    ));
    (dash, screens, merge(controls))
}

/// The hood light pod: a black housing on the hood's front with four round lamps (chrome
/// reflectors, clear lenses, LED emitters): (housing, chrome, lenses, LEDs).
fn light_pod(plain: Field) -> LightPod {
    let field = |p: Vec3| {
        let h = p.y + GROUND;
        let b = sd_round_box(
            p - Vec3::new(0.0, POD_H - GROUND, POD_FACE + 0.17),
            Vec3::new(0.665, 0.09, 0.17),
            0.045,
        );
        // It sits on the hood: only what is outside the body (4 mm clear).
        let mut d = b.max(-(plain(p) - 0.004));
        let ax = p.x.abs();
        for x in POD_LAMPS {
            let radial = Vec2::new(ax - x, h - POD_H).length();
            let bore = (radial - POD_R).max(p.z - (POD_FACE + 0.06));
            d = rmax(d, -bore, 0.005);
        }
        d
    };
    let shell = sculpt(
        &field,
        Vec3::new(-0.72, 0.76 - GROUND, POD_FACE - 0.04),
        Vec3::new(0.72, 0.99 - GROUND, POD_FACE + 0.38),
        0.02,
        true,
        0.0018,
        &|_, _| Vec4::ONE,
    );
    let r = POD_R;
    let bowl = lathe_x(
        &[
            Vec2::new(-0.055, 0.016),
            Vec2::new(-0.042, r * 0.62),
            Vec2::new(-0.022, r * 0.9),
            Vec2::new(0.0, r),
        ],
        32,
        &[],
    );
    let bezel = lathe_x(
        &[
            Vec2::new(0.004, r - 0.004),
            Vec2::new(0.013, r - 0.002),
            Vec2::new(0.016, r + 0.004),
            Vec2::new(0.012, r + 0.010),
            Vec2::new(0.0, r + 0.012),
        ],
        32,
        &[],
    );
    let lens = lathe_x(
        &[
            Vec2::new(0.010, 0.0),
            Vec2::new(0.009, r * 0.5),
            Vec2::new(0.006, r * 0.85),
            Vec2::new(0.004, r - 0.003),
        ],
        32,
        &[],
    );
    let led = lathe_x(
        &[
            Vec2::new(-0.036, 0.0),
            Vec2::new(-0.036, 0.024),
            Vec2::new(-0.040, 0.030),
            Vec2::new(-0.046, 0.031),
        ],
        16,
        &[],
    );
    let (mut chrome, mut bowls, mut lenses, mut leds) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for x in POD_LAMPS {
        for s in [-1.0f32, 1.0] {
            // Lathe +x → −z (forward), face at the bore mouth.
            let at = trs(
                Vec3::new(s * x, POD_H - GROUND, POD_FACE + 0.012),
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                Vec3::ONE,
            );
            chrome.push(transformed(bezel.clone(), at));
            bowls.push(transformed(bowl.clone(), at));
            lenses.push(transformed(lens.clone(), at));
            leds.push(transformed(led.clone(), at));
        }
    }
    LightPod {
        shell,
        bezels: merge(chrome),
        reflectors: merge(bowls),
        lenses: merge(lenses),
        leds: merge(leds),
    }
}

/// The hood light pod's meshes.
struct LightPod {
    shell: Mesh,
    bezels: Mesh,
    reflectors: Mesh,
    lenses: Mesh,
    leds: Mesh,
}

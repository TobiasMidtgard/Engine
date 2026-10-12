//! Builds the weather station into the world: generates the textures, meshes and materials
//! once (registered under `weather/station/…` keys, so a second station reuses them) and
//! spawns every part as a child of one station root entity.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, StandardMaterial,
};

use super::meshgen::{Builder, at, ground_occlusion, heightfield, lathe, transform};
use super::noise::{Rng, fbm, hex, smooth, sq};
use super::props::{self, Scatter, offset_uv};
use super::{
    Anemometer, Glow, LedSegment, RainGauge, SnowStake, SockSegment, StreetLamp, ThermometerColumn,
    WeatherStation, WindSock, WindVane, signs, texgen,
};
use crate::surface::{GroundPatch, WeatherShelter};

/// Every texture, material and mesh the station uses (handles into the asset stores).
#[derive(Resource, Clone)]
pub(crate) struct StationKit {
    m: Mats,
    mesh: Meshes,
}

#[derive(Clone, Copy)]
struct TexSetH {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    orm: Handle<Image>,
}

#[derive(Clone)]
struct Mats {
    asphalt: Handle<StandardMaterial>,
    asphalt_patch: Handle<StandardMaterial>,
    sealant: Handle<StandardMaterial>,
    paint_white: Handle<StandardMaterial>,
    paint_yellow: Handle<StandardMaterial>,
    lettering: Handle<StandardMaterial>,
    paver: Handle<StandardMaterial>,
    setts: Handle<StandardMaterial>,
    joint: Handle<StandardMaterial>,
    sand: Handle<StandardMaterial>,
    concrete: Handle<StandardMaterial>,
    cast_stone: Handle<StandardMaterial>,
    timber: Handle<StandardMaterial>,
    bench_wood: Handle<StandardMaterial>,
    steel: Handle<StandardMaterial>,
    white_metal: Handle<StandardMaterial>,
    white_wood: Handle<StandardMaterial>,
    galv: Handle<StandardMaterial>,
    glass: Handle<StandardMaterial>,
    clear_glass: Handle<StandardMaterial>,
    chrome: Handle<StandardMaterial>,
    brass: Handle<StandardMaterial>,
    black: Handle<StandardMaterial>,
    void: Handle<StandardMaterial>,
    retro: Handle<StandardMaterial>,
    soffit: Handle<StandardMaterial>,
    red_liquid: Handle<StandardMaterial>,
    water: Handle<StandardMaterial>,
    snow: Handle<StandardMaterial>,
    fabric_orange: Handle<StandardMaterial>,
    fabric_white: Handle<StandardMaterial>,
    iron: Handle<StandardMaterial>,
    solar: Handle<StandardMaterial>,
    plastic_black: Handle<StandardMaterial>,
    lamp_lens: Handle<StandardMaterial>,
    shelter_led: Handle<StandardMaterial>,
    poster: Handle<StandardMaterial>,
    beacon: Handle<StandardMaterial>,
    led_on: Handle<StandardMaterial>,
    led_off: Handle<StandardMaterial>,
    plate: Handle<StandardMaterial>,
    sign: Handle<StandardMaterial>,
    thermo_board: Handle<StandardMaterial>,
    stake: Handle<StandardMaterial>,
    rain_scale: Handle<StandardMaterial>,
    display_face: Handle<StandardMaterial>,
    leaves: Handle<StandardMaterial>,
    grass: Handle<StandardMaterial>,
    flag: Handle<StandardMaterial>,
    timetable: Handle<StandardMaterial>,
}

#[derive(Clone)]
struct Meshes {
    asphalt: Handle<Mesh>,
    patch: Handle<Mesh>,
    sealant: Handle<Mesh>,
    lines_white: Handle<Mesh>,
    lines_yellow: Handle<Mesh>,
    lettering: Handle<Mesh>,
    kerbs: Handle<Mesh>,
    pavers: Handle<Mesh>,
    joint: Handle<Mesh>,
    setts: Handle<Mesh>,
    sand: Handle<Mesh>,
    sleepers: Handle<Mesh>,
    manhole_cover: Handle<Mesh>,
    manhole_ring: Handle<Mesh>,
    gully: Handle<Mesh>,
    gully_hole: Handle<Mesh>,
    shelter: [Handle<Mesh>; 9],
    lamp: [Handle<Mesh>; 4],
    lamp_light: Vec3,
    bollard: Handle<Mesh>,
    bollard_bands: Handle<Mesh>,
    plinth: Handle<Mesh>,
    plinth_cup: Handle<Mesh>,
    ball: Handle<Mesh>,
    mast: [Handle<Mesh>; 5],
    mast_points: [Vec3; 3],
    rotor: Handle<Mesh>,
    cups: Handle<Mesh>,
    vane: Handle<Mesh>,
    vane_nose: Handle<Mesh>,
    sock_pole: Handle<Mesh>,
    sock_frame: Handle<Mesh>,
    sock_bands: Vec<Handle<Mesh>>,
    screen: Handle<Mesh>,
    screen_hw: Handle<Mesh>,
    gauge: [Handle<Mesh>; 4],
    gauge_water: Handle<Mesh>,
    stake: Handle<Mesh>,
    mound: Handle<Mesh>,
    thermo: [Handle<Mesh>; 6],
    thermo_column: Handle<Mesh>,
    seg_h: Handle<Mesh>,
    seg_v: Handle<Mesh>,
    led_dot: Handle<Mesh>,
    fence: Handle<Mesh>,
    fence_hw: Handle<Mesh>,
    sign: [Handle<Mesh>; 3],
    flag: [Handle<Mesh>; 5],
    leaves: Handle<Mesh>,
    tufts: Handle<Mesh>,
}

const KEY: &str = "weather/station";

// Layout (meters, station frame: +Z north, the cameras mostly look from +Z).
/// Asphalt yard half size; its top is `YARD_Y` above the ground.
const YARD: (f32, f32) = (7.0, 5.0);
const YARD_Y: f32 = 0.008;
/// Stone patio and sand pit: centers (x) and half sizes.
const PATIO: (f32, f32, f32) = (-9.75, 2.5, 3.0);
const SANDPIT: (f32, f32, f32) = (9.75, 2.5, 3.0);
const SHELTER_AT: Vec3 = Vec3::new(-3.5, 0.0, -3.3);
const BALL_AT: Vec3 = Vec3::new(1.1, 0.0, 2.7);
const BALL_R: f32 = 0.36;
const MAST_AT: Vec3 = Vec3::new(10.2, 0.0, -5.6);
const SOCK_AT: Vec3 = Vec3::new(6.3, 0.0, -6.9);
const THERMO_AT: Vec3 = Vec3::new(2.9, 0.0, -5.75);
const ENCLOSURE: (f32, f32, f32, f32) = (-12.6, -8.6, 3.7, 7.5);
const SCREEN_AT: Vec3 = Vec3::new(-10.9, 0.0, 5.0);
const GAUGE_AT: Vec3 = Vec3::new(-9.4, 0.0, 6.5);
const STAKE_AT: Vec3 = Vec3::new(-9.9, 0.0, 4.4);
const SIGN_AT: Vec3 = Vec3::new(-11.2, 0.0, 7.85);
const FLAG_AT: Vec3 = Vec3::new(0.7, 0.0, -2.85);
const LAMPS: [(Vec3, f32); 3] = [
    (Vec3::new(-7.55, 0.0, 5.6), -FRAC_PI_4),
    (Vec3::new(7.55, 0.0, -5.6), PI - FRAC_PI_4),
    (Vec3::new(0.4, 0.0, -5.65), -FRAC_PI_2),
];
const BOLLARDS_X: [f32; 3] = [-4.2, -1.4, 4.4];
/// Sealed cracks in the asphalt: start, heading (radians), steps of 0.32 m.
const CRACKS: [(f32, f32, f32, u32); 4] = [
    (-6.6, -3.6, 0.25, 22),
    (-1.2, 4.7, -1.35, 14),
    (5.8, 1.4, 2.6, 18),
    (-4.2, 0.2, 0.1, 9),
];

/// The crack center lines, clipped to the yard.
fn crack_lines() -> Vec<Vec<Vec2>> {
    CRACKS
        .iter()
        .enumerate()
        .map(|(i, &(x, z, dir, steps))| {
            props::crack_path(Vec2::new(x, z), dir, steps, 0.32, 310 + i as u32)
                .into_iter()
                .take_while(|p| p.x.abs() < YARD.0 - 0.1 && p.y.abs() < YARD.1 - 0.1)
                .collect()
        })
        .collect()
}

/// Where leaves gather and grass grows: against the kerbs, inside the shelter's back, around
/// posts, poles and anchors, in the cracks and along the paving edges.
fn litter() -> (Vec<Scatter>, Vec<Scatter>) {
    let mut rng = Rng::new(901);
    let mut leaves = Vec::new();
    let mut tufts = Vec::new();
    let (hx, hz) = YARD;
    let leaf = |rng: &mut Rng, p: Vec2, y: f32| Scatter {
        pos: Vec3::new(p.x, y + 0.002, p.y),
        yaw: rng.next() * TAU,
        size: 0.055 + 0.04 * rng.next(),
        variant: (rng.next() * 4.0) as u32 % 4,
        tint: 0.75 + 0.35 * rng.next(),
    };
    let tuft = |rng: &mut Rng, p: Vec2, y: f32, size: f32| Scatter {
        pos: Vec3::new(p.x, y, p.y),
        yaw: rng.next() * TAU,
        size: size * (0.7 + 0.5 * rng.next()),
        variant: (rng.next() * 2.0) as u32 % 2,
        tint: 0.8 + 0.3 * rng.next(),
    };
    // Along each kerb: leaves drift against it on the asphalt, grass grows behind it and a
    // few weeds come up in the joint.
    let sides = [
        (Vec2::new(-hx, -hz), Vec2::new(hx, -hz), Vec2::NEG_Y),
        (Vec2::new(hx, hz), Vec2::new(-hx, hz), Vec2::Y),
        (Vec2::new(-hx, hz), Vec2::new(-hx, -hz), Vec2::NEG_X),
        (Vec2::new(hx, -hz), Vec2::new(hx, hz), Vec2::X),
    ];
    for (i, (a, b, out)) in sides.into_iter().enumerate() {
        let len = a.distance(b);
        let dir = (b - a) / len;
        let mut t = 0.1;
        while t < len - 0.1 {
            let p = a + dir * t;
            let drift = fbm(p.x * 0.8, p.y * 0.8, sq(0), 2, 910 + i as u32) * 0.5 + 0.5;
            for _ in 0..2 {
                if rng.next() < drift * drift * 1.4 {
                    let inward = 0.015 + 0.3 * rng.next().powi(2);
                    let q = p - out * inward + dir * rng.signed() * 0.1;
                    leaves.push(leaf(&mut rng, q, yard_y(q.x, q.y)));
                }
            }
            if rng.next() < 0.75 {
                let q = p + out * (0.17 + 0.1 * rng.next()) + dir * rng.signed() * 0.05;
                tufts.push(tuft(&mut rng, q, 0.0, 1.0));
            }
            if rng.next() < 0.07 {
                let q = p - out * (0.012 + 0.02 * rng.next());
                tufts.push(tuft(&mut rng, q, yard_y(q.x, q.y), 0.4));
            }
            t += 0.17;
        }
    }
    // Weeds in the cracks.
    for line in crack_lines() {
        for p in line.iter().skip(2).step_by(4) {
            if rng.next() < 0.6 {
                tufts.push(tuft(&mut rng, *p, yard_y(p.x, p.y), 0.32));
            }
        }
    }
    // Leaves blown into the shelter against its back glass, and under the bench.
    for _ in 0..34 {
        let q = Vec2::new(
            SHELTER_AT.x + rng.signed() * 2.2,
            SHELTER_AT.z - 0.62 + 0.4 * rng.next().powi(2),
        );
        leaves.push(leaf(&mut rng, q, yard_y(q.x, q.y)));
    }
    // Around bollards, the plinth and the bus stop pole.
    let around = |rng: &mut Rng, c: Vec2, r: f32, n: u32, y: f32, leaves: &mut Vec<Scatter>| {
        for _ in 0..n {
            let a = rng.next() * TAU;
            let q = c + Vec2::new(a.cos(), a.sin()) * (r + 0.25 * rng.next().powi(2));
            leaves.push(leaf(rng, q, y));
        }
    };
    for x in BOLLARDS_X {
        around(
            &mut rng,
            Vec2::new(x, hz - 0.15),
            0.08,
            3,
            yard_y(x, hz - 0.15),
            &mut leaves,
        );
    }
    around(
        &mut rng,
        Vec2::new(BALL_AT.x, BALL_AT.z),
        0.32,
        6,
        YARD_Y,
        &mut leaves,
    );
    around(
        &mut rng,
        Vec2::new(FLAG_AT.x, FLAG_AT.z),
        0.06,
        4,
        YARD_Y,
        &mut leaves,
    );
    // A scatter over the patio and the grass by the enclosure.
    let (pc, phx, phz) = PATIO;
    for _ in 0..26 {
        let q = Vec2::new(pc + rng.signed() * (phx - 0.2), rng.signed() * (phz - 0.2));
        leaves.push(leaf(&mut rng, q, 0.016));
    }
    let (e0, e1, e2, e3) = ENCLOSURE;
    for _ in 0..30 {
        let q = Vec2::new(e0 + (e1 - e0) * rng.next(), e2 + (e3 - e2) * rng.next());
        leaves.push(leaf(&mut rng, q, 0.003));
    }
    // Grass round everything standing in the grass: fence line, poles, legs, anchors.
    let fence = [
        (Vec2::new(e0, e2), Vec2::new(e1, e2)),
        (Vec2::new(e1, e2), Vec2::new(e1, e3)),
        (Vec2::new(e1, e3), Vec2::new(e0, e3)),
        (Vec2::new(e0, e3), Vec2::new(e0, e2)),
    ];
    for (a, b) in fence {
        let len = a.distance(b);
        let mut t = 0.0;
        while t <= len {
            if rng.next() < 0.7 {
                let q = a + (b - a) * (t / len) + Vec2::new(rng.signed(), rng.signed()) * 0.12;
                tufts.push(tuft(&mut rng, q, 0.0, 1.0));
            }
            t += 0.45;
        }
    }
    let mut feet: Vec<(Vec2, f32)> = vec![
        (Vec2::new(SOCK_AT.x, SOCK_AT.z), 0.12),
        (Vec2::new(MAST_AT.x, MAST_AT.z), 0.45),
        (Vec2::new(THERMO_AT.x - 0.27, THERMO_AT.z), 0.06),
        (Vec2::new(THERMO_AT.x + 0.27, THERMO_AT.z), 0.06),
        (Vec2::new(SCREEN_AT.x, SCREEN_AT.z), 0.4),
        (Vec2::new(GAUGE_AT.x, GAUGE_AT.z), 0.08),
        (Vec2::new(STAKE_AT.x, STAKE_AT.z), 0.05),
        (Vec2::new(SIGN_AT.x - 0.45, SIGN_AT.z), 0.05),
        (Vec2::new(SIGN_AT.x + 0.45, SIGN_AT.z), 0.05),
    ];
    for (p, _) in LAMPS {
        feet.push((Vec2::new(p.x, p.z), 0.2));
    }
    for k in 0..3 {
        let a = [PI, PI / 3.0, -PI / 3.0][k];
        let anchor = Vec2::new(MAST_AT.x, MAST_AT.z) + Vec2::new(a.sin(), a.cos()) * 2.2;
        feet.push((anchor, 0.2));
    }
    for (c, r) in feet {
        let n = 3 + (rng.next() * 3.0) as u32;
        for _ in 0..n {
            let a = rng.next() * TAU;
            let q = c + Vec2::new(a.cos(), a.sin()) * (r + 0.05 + 0.12 * rng.next());
            if q.x.abs() < hx + 0.2 && q.y.abs() < hz + 0.2 {
                continue;
            }
            tufts.push(tuft(&mut rng, q, 0.0, 1.0));
        }
    }
    // Weeds along the outside of the patio and the sand pit.
    for (cx, hw, hd) in [PATIO, SANDPIT] {
        let ring = [
            (
                Vec2::new(cx - hw, -hd),
                Vec2::new(cx + hw, -hd),
                Vec2::NEG_Y,
            ),
            (Vec2::new(cx + hw, hd), Vec2::new(cx - hw, hd), Vec2::Y),
            (Vec2::new(cx - hw, hd), Vec2::new(cx - hw, -hd), Vec2::NEG_X),
            (Vec2::new(cx + hw, -hd), Vec2::new(cx + hw, hd), Vec2::X),
        ];
        for (a, b, out) in ring {
            let len = a.distance(b);
            let mut t = 0.15;
            while t < len {
                let q = a + (b - a) * (t / len) + out * (0.06 + 0.08 * rng.next());
                // Keep the strip between the yard and its neighbours clear of the kerb.
                if rng.next() < 0.5 && !(q.x.abs() < hx + 0.17 && q.y.abs() < hz + 0.17) {
                    tufts.push(tuft(&mut rng, q, 0.0, 0.9));
                }
                t += 0.4;
            }
        }
    }
    (leaves, tufts)
}

/// Height of the asphalt surface at (x, z): two shallow wheel ruts along the bus lane.
fn yard_y(x: f32, z: f32) -> f32 {
    let rut = |zc: f32| {
        let d = (z - zc) / 0.24;
        (-d * d).exp()
    };
    let lane = smooth(-YARD.0, -YARD.0 + 1.5, x) * smooth(YARD.0, YARD.0 - 1.5, x);
    YARD_Y - 0.004 * (rut(-1.15) + rut(0.9)) * lane
}

/// Build the station at `origin` (call through `spawn_demo`).
pub(crate) fn build(world: &mut World, origin: DVec3) {
    let started = std::time::Instant::now();
    world.init_resource::<Assets<Image>>();
    world.init_resource::<Assets<Mesh>>();
    world.init_resource::<Assets<StandardMaterial>>();
    let kit = match world.get_resource::<StationKit>() {
        Some(kit) => kit.clone(),
        None => {
            let kit = make_kit(world);
            world.insert_resource(kit.clone());
            kit
        }
    };
    spawn(world, origin, &kit);
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
        d.set("weather.station_build_ms", ms);
    }
}

fn make_kit(world: &mut World) -> StationKit {
    let tex = make_textures(world);
    let m = make_materials(world, &tex);
    let mesh = make_meshes(world);
    StationKit { m, mesh }
}

/// All the station's textures, generated in parallel the first time.
struct Textures {
    asphalt: TexSetH,
    stone: TexSetH,
    sand: TexSetH,
    concrete: TexSetH,
    wood: TexSetH,
    galv: TexSetH,
    fabric: TexSetH,
    paint: TexSetH,
    iron: TexSetH,
    solar: TexSetH,
    white_paint: Handle<Image>,
    wood_paint: TexSetH,
    glass: (Handle<Image>, Handle<Image>),
    road: TexSetH,
    lettering: Handle<Image>,
    thermo: Handle<Image>,
    stake: Handle<Image>,
    rain: Handle<Image>,
    sign: Handle<Image>,
    plate: Handle<Image>,
    poster: Handle<Image>,
    face: Handle<Image>,
    leaves: TexSetH,
    grass: TexSetH,
    flag: Handle<Image>,
    timetable: Handle<Image>,
}

fn manhole_pattern(u: f32, v: f32) -> f32 {
    let p = Vec2::new(u - 0.5, v - 0.5) * 2.0;
    let r = p.length();
    let ring = smooth(0.06, 0.04, (r - 0.9).abs()).max(smooth(0.03, 0.015, (r - 0.62).abs()));
    let grid = {
        let g = p * 7.0;
        let f = Vec2::new(g.x - g.x.round(), g.y - g.y.round()).abs();
        smooth(0.32, 0.26, f.x.max(f.y))
    };
    let boss = smooth(0.12, 0.09, r);
    let inside = smooth(0.6, 0.58, r);
    (ring + grid * inside * (1.0 - smooth(0.13, 0.15, r) * 0.0) + boss).min(1.0)
        * smooth(0.99, 0.97, r)
}

fn make_textures(world: &mut World) -> Textures {
    let started = std::time::Instant::now();
    // Everything is independent: generate on scoped threads (each recipe is itself parallel).
    #[allow(clippy::type_complexity)]
    let (
        (asphalt, stone, sand, concrete, wood, galv, fabric),
        (paint, wood_paint, iron, solar, glass, road, extras),
        ((leaves, grass), (flag, timetable)),
    ) = std::thread::scope(|s| {
        // The asphalt is the biggest job: start it first.
        let a = s.spawn(texgen::asphalt);
        let b = s.spawn(texgen::stone);
        let c = s.spawn(texgen::sand);
        let d = s.spawn(texgen::concrete);
        let e = s.spawn(texgen::wood);
        let f = s.spawn(texgen::galvanised);
        let g = s.spawn(texgen::fabric);
        let pa = s.spawn(|| texgen::painted(&[hex(0x383e42), hex(0xe9e8e2)], false));
        let pb = s.spawn(|| texgen::painted(&[hex(0xecebe5)], true));
        let pc = s.spawn(|| texgen::cast_iron(manhole_pattern));
        let pd = s.spawn(texgen::solar);
        let pe = s.spawn(texgen::glass);
        let pf = s.spawn(texgen::road_paint);
        let ph = s.spawn(|| (texgen::leaves(), texgen::grass_tufts()));
        let pi = s.spawn(|| (signs::bus_flag(), signs::timetable()));
        let pg = s.spawn(|| {
            (
                signs::bus_stop_lettering(),
                signs::thermometer_scale(),
                signs::snow_stake(),
                signs::rain_scale(),
                signs::station_sign(),
                signs::shelter_plate(),
                signs::poster(),
                signs::display_face(),
            )
        });
        (
            (
                a.join().expect("asphalt"),
                b.join().expect("stone"),
                c.join().expect("sand"),
                d.join().expect("concrete"),
                e.join().expect("wood"),
                f.join().expect("galvanised"),
                g.join().expect("fabric"),
            ),
            (
                pa.join().expect("paint"),
                pb.join().expect("painted wood"),
                pc.join().expect("iron"),
                pd.join().expect("solar"),
                pe.join().expect("glass"),
                pf.join().expect("road paint"),
                pg.join().expect("signs"),
            ),
            (ph.join().expect("litter"), pi.join().expect("bus flag")),
        )
    });
    let mut images = world.resource_mut::<Assets<Image>>();
    let put = |images: &mut Assets<Image>, name: &str, image: Image| {
        images.insert_keyed(&format!("{KEY}/tex/{name}"), image)
    };
    let mut set = |name: &str, s: texgen::TexSet| TexSetH {
        albedo: put(&mut images, &format!("{name}.albedo"), s.albedo),
        normal: put(&mut images, &format!("{name}.normal"), s.normal),
        orm: put(&mut images, &format!("{name}.orm"), s.orm),
    };
    let asphalt = set("asphalt", asphalt);
    let stone = set("stone", stone);
    let sand = set("sand", sand);
    let concrete = set("concrete", concrete);
    let wood = set("wood", wood);
    let galv = set("galvanised", galv);
    let fabric = set("fabric", fabric);
    let iron = set("cast_iron", iron);
    let solar = set("solar", solar);
    let road = set("road_paint", road);
    let leaves = set("leaves", leaves);
    let grass = set("grass_tufts", grass);
    let (mut paint_albedo, paint_n, paint_orm) = paint;
    let white_paint_img = paint_albedo.pop().expect("white paint");
    let anthracite = paint_albedo.pop().expect("anthracite paint");
    let paint = set(
        "paint",
        texgen::TexSet {
            albedo: anthracite,
            normal: paint_n,
            orm: paint_orm,
        },
    );
    let (mut wp_albedo, wp_n, wp_orm) = wood_paint;
    let wood_paint = set(
        "white_wood",
        texgen::TexSet {
            albedo: wp_albedo.pop().expect("white wood"),
            normal: wp_n,
            orm: wp_orm,
        },
    );
    let (lettering, thermo, stake, rain, sign, plate, poster, face) = extras;
    let images = &mut *images;
    let textures = Textures {
        asphalt,
        stone,
        sand,
        concrete,
        wood,
        galv,
        fabric,
        paint,
        iron,
        solar,
        white_paint: put(images, "white_paint.albedo", white_paint_img),
        wood_paint,
        glass: (
            put(images, "glass.albedo", glass.0),
            put(images, "glass.orm", glass.1),
        ),
        road,
        lettering: put(images, "bus_stop", lettering),
        thermo: put(images, "thermometer_scale", thermo),
        stake: put(images, "snow_stake", stake),
        rain: put(images, "rain_scale", rain),
        sign: put(images, "sign", sign),
        plate: put(images, "shelter_plate", plate),
        poster: put(images, "poster", poster),
        face: put(images, "display_face", face),
        leaves,
        grass,
        flag: put(images, "bus_flag", flag),
        timetable: put(images, "timetable", timetable),
    };
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
        d.set("weather.station_textures_ms", ms);
    }
    textures
}

fn pbr(t: TexSetH, uv: [f32; 2]) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 1.0,
        metallic: 1.0,
        base_color_texture: Some(t.albedo),
        normal_texture: Some(t.normal),
        metallic_roughness_texture: Some(t.orm),
        occlusion_texture: Some(t.orm),
        uv_scale: uv,
        ..Default::default()
    }
}

fn tinted(mut m: StandardMaterial, rgb: [f32; 3]) -> StandardMaterial {
    m.base_color = [rgb[0], rgb[1], rgb[2], m.base_color[3]];
    m
}

fn make_materials(world: &mut World, t: &Textures) -> Mats {
    let mut mats = world.resource_mut::<Assets<StandardMaterial>>();
    let mut add =
        |name: &str, m: StandardMaterial| mats.insert_keyed(&format!("{KEY}/mat/{name}"), m);
    let painted = |albedo: Handle<Image>, uv: f32| {
        let mut m = pbr(t.paint, [uv, uv]);
        m.base_color_texture = Some(albedo);
        m
    };
    let decal = |tex: TexSetH, rgb: [f32; 3]| {
        let mut m = tinted(pbr(tex, [1.0, 1.0]), rgb);
        m.alpha_mode = AlphaMode::Mask(0.5);
        m
    };
    let flat = |rgb: [f32; 3], rough: f32, metal: f32| {
        StandardMaterial::color(rgb[0], rgb[1], rgb[2])
            .with_roughness(rough)
            .with_metallic(metal)
    };
    let printed = |img: Handle<Image>, rough: f32| StandardMaterial {
        base_color_texture: Some(img),
        base_color: [1.0; 4],
        perceptual_roughness: rough,
        ..Default::default()
    };
    let glow = |img: Option<Handle<Image>>, base: [f32; 3], color: [f32; 3], rough: f32| {
        StandardMaterial {
            base_color: [base[0], base[1], base[2], 1.0],
            base_color_texture: img,
            emissive_texture: img,
            emissive: color,
            emissive_strength: 0.0,
            perceptual_roughness: rough,
            receives_wetness: false,
            ..Default::default()
        }
    };
    let mut glass = StandardMaterial {
        base_color: [1.0; 4],
        base_color_texture: Some(t.glass.0),
        metallic_roughness_texture: Some(t.glass.1),
        perceptual_roughness: 1.0,
        metallic: 0.0,
        alpha_mode: AlphaMode::Blend,
        receives_wetness: false,
        uv_scale: [2.0, 1.0 / 2.2],
        ..Default::default()
    };
    // A little more reflective than float glass so the panes read through the alpha blend.
    glass.reflectance = 0.7;
    let mut patch = tinted(pbr(t.asphalt, [0.5, 0.5]), [0.62, 0.62, 0.64]);
    patch.normal_scale = 0.7;
    let mut stone = pbr(t.stone, [1.0, 1.0]);
    stone.normal_scale = 1.3;
    let mut iron = pbr(t.iron, [1.0, 1.0]);
    iron.normal_scale = 1.0;
    Mats {
        asphalt: add("asphalt", pbr(t.asphalt, [0.5, 0.5])),
        asphalt_patch: add("asphalt_patch", patch),
        sealant: add("sealant", {
            let mut m = tinted(pbr(t.asphalt, [0.5, 0.5]), [0.25, 0.25, 0.26]);
            m.normal_scale = 0.25;
            m.perceptual_roughness = 0.5;
            m
        }),
        paint_white: add("paint_white", decal(t.road, [0.95, 0.95, 0.92])),
        paint_yellow: add("paint_yellow", decal(t.road, hex(0xf0b41c))),
        lettering: add("lettering", {
            let mut m = flat(hex(0xf0b41c), 0.6, 0.0);
            m.base_color_texture = Some(t.lettering);
            m.alpha_mode = AlphaMode::Mask(0.5);
            m
        }),
        paver: add("paver", stone),
        setts: add(
            "setts",
            tinted(pbr(t.stone, [1.6, 1.6]), [0.56, 0.56, 0.58]),
        ),
        joint: add("joint", tinted(pbr(t.sand, [1.5, 1.5]), [0.34, 0.32, 0.3])),
        sand: add("sand", pbr(t.sand, [1.0 / 1.5, 1.0 / 1.5])),
        concrete: add("concrete", pbr(t.concrete, [1.0, 1.0])),
        cast_stone: add("cast_stone", {
            let mut m = tinted(pbr(t.concrete, [1.4, 1.4]), [1.05, 1.03, 1.0]);
            m.perceptual_roughness = 0.75;
            m
        }),
        timber: add("timber", tinted(pbr(t.wood, [1.0, 4.0]), [0.82, 0.8, 0.76])),
        bench_wood: add(
            "bench_wood",
            tinted(pbr(t.wood, [1.0, 4.0]), [1.1, 1.0, 0.9]),
        ),
        steel: add("steel", painted(t.paint.albedo, 2.0)),
        white_metal: add("white_metal", painted(t.white_paint, 2.0)),
        white_wood: add("white_wood", pbr(t.wood_paint, [2.0, 2.0])),
        galv: add("galvanised", pbr(t.galv, [1.0 / 0.6, 1.0 / 0.6])),
        glass: add("glass", glass),
        clear_glass: add(
            "clear_glass",
            StandardMaterial::color(0.9, 0.95, 0.97)
                .with_roughness(0.04)
                .with_alpha(0.16),
        ),
        chrome: add("chrome", flat([0.96, 0.96, 0.97], 0.03, 1.0)),
        brass: add("brass", flat(hex(0xd8b06a), 0.32, 1.0)),
        black: add("rubber", flat([0.02, 0.02, 0.02], 0.55, 0.0)),
        void: add("void", {
            let mut m = flat([0.0, 0.0, 0.0], 1.0, 0.0);
            m.receives_wetness = false;
            m
        }),
        retro: add("retroreflective", flat([0.85, 0.86, 0.86], 0.2, 0.0)),
        soffit: add("soffit", painted(t.white_paint, 2.0)),
        red_liquid: add("red_liquid", flat(hex(0xb5121b), 0.2, 0.0)),
        water: add(
            "gauge_water",
            StandardMaterial::color(0.35, 0.55, 0.75)
                .with_roughness(0.05)
                .with_alpha(0.7),
        ),
        snow: add("snow", {
            let mut m = flat([0.82, 0.84, 0.88], 0.8, 0.0).with_subsurface(0.3);
            m.receives_wetness = false;
            m
        }),
        fabric_orange: add("fabric_orange", {
            let mut m = tinted(pbr(t.fabric, [25.0, 25.0]), hex(0xff5512));
            m.double_sided = true;
            m.subsurface = 0.35;
            m
        }),
        fabric_white: add("fabric_white", {
            let mut m = tinted(pbr(t.fabric, [25.0, 25.0]), [0.95, 0.95, 0.93]);
            m.double_sided = true;
            m.subsurface = 0.35;
            m
        }),
        iron: add("cast_iron", iron),
        solar: add("solar", pbr(t.solar, [1.0 / 0.5, 1.0 / 0.98])),
        plastic_black: add("black_plastic", flat([0.018, 0.018, 0.02], 0.3, 0.0)),
        lamp_lens: add(
            "lamp_lens",
            glow(None, [0.8, 0.8, 0.78], [1.0, 0.86, 0.68], 0.25),
        ),
        shelter_led: add(
            "shelter_led",
            glow(None, [0.85, 0.85, 0.85], [1.0, 0.94, 0.85], 0.3),
        ),
        poster: add(
            "poster",
            glow(Some(t.poster), [1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 0.15),
        ),
        beacon: add(
            "beacon",
            glow(None, [0.5, 0.03, 0.02], [1.0, 0.05, 0.02], 0.2),
        ),
        led_on: add(
            "led_on",
            glow(None, [0.25, 0.05, 0.01], [1.0, 0.32, 0.04], 0.3),
        ),
        led_off: add("led_off", flat([0.04, 0.012, 0.008], 0.35, 0.0)),
        plate: add("shelter_plate", printed(t.plate, 0.4)),
        sign: add("sign", printed(t.sign, 0.35)),
        thermo_board: add("thermometer_board", {
            let mut m = printed(t.thermo, 0.3);
            m.uv_scale = [1.0 / props::THERMO_BOARD.0, 1.0 / props::THERMO_BOARD.1];
            m
        }),
        stake: add("snow_stake", printed(t.stake, 0.45)),
        rain_scale: add("rain_scale", printed(t.rain, 0.3)),
        display_face: add("display_face", printed(t.face, 0.2)),
        leaves: add("leaves", {
            let mut m = pbr(t.leaves, [1.0, 1.0]);
            m.alpha_mode = AlphaMode::Mask(0.5);
            m.subsurface = 0.3;
            m
        }),
        grass: add("grass_tufts", {
            let mut m = pbr(t.grass, [1.0, 1.0]);
            m.alpha_mode = AlphaMode::Mask(0.4);
            m.subsurface = 0.45;
            m.normal_scale = 0.5;
            m.wind_sway = 0.035;
            m
        }),
        flag: add("bus_flag", printed(t.flag, 0.3)),
        timetable: add("timetable", printed(t.timetable, 0.6)),
    }
}

fn make_meshes(world: &mut World) -> Meshes {
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let mut add = |name: &str, m: Mesh| meshes.insert_keyed(&format!("{KEY}/mesh/{name}"), m);
    // Ground.
    let (hx, hz) = YARD;
    let asphalt = heightfield((-hx, -hz), (hx, hz), (168, 120), |x, z| {
        let macro_tone = 1.0 + 0.1 * fbm(x * 0.35 + 7.3, z * 0.35, sq(0), 4, 201);
        // Wheel tracks are polished a little lighter; buses drip oil between them, most in
        // the stop box where they stand, and cars over their bays.
        let rut = (YARD_Y - yard_y(x, z)) / 0.004;
        let lane = smooth(-hx, -hx + 1.5, x) * smooth(hx, hx - 1.5, x);
        let in_box = smooth(-6.8, -6.0, x) * smooth(2.0, 1.2, x);
        let blobs = smooth(0.05, 0.55, fbm(x * 1.7, z * 1.7, sq(0), 4, 203));
        let spots = smooth(0.35, 0.75, fbm(x * 4.5, z * 4.5, sq(0), 3, 204));
        let band = (-((z + 0.12) / 0.32).powi(2)).exp() * lane;
        let mut oil = band * (0.25 + 0.75 * blobs) * (0.35 + 0.65 * in_box);
        for bx in [3.85f32, 5.95] {
            let d = Vec2::new((x - bx) / 0.55, (z + 1.7) / 0.8).length();
            oil = oil.max(smooth(1.0, 0.2, d) * (0.4 + 0.6 * blobs));
        }
        oil = (oil + 0.5 * spots * band).min(1.0);
        let edge = (hx - x.abs()).min(hz - z.abs());
        let dirt = 1.0 - 0.16 * smooth(0.4, 0.0, edge);
        let k = macro_tone * dirt * (1.0 + 0.07 * rut) * (1.0 - 0.32 * oil);
        let warm = 1.0 - 0.04 * oil;
        (yard_y(x, z), Vec4::new(k, k * warm, k * warm * warm, 1.0))
    });
    let patch_rect = (2.3f32, 4.7f32, -4.6f32, -2.7f32);
    let mut patch = heightfield(
        (patch_rect.0, patch_rect.2),
        (patch_rect.1, patch_rect.3),
        (12, 10),
        |x, z| (yard_y(x, z) + 0.0012, Vec4::ONE),
    );
    offset_uv(&mut patch, 0.37, 0.71);
    let mut seal = Builder::new();
    let ribbon_y = |x: f32, z: f32| yard_y(x, z) + 0.0016;
    // Sealed seam around the patch.
    let (px0, px1, pz0, pz1) = patch_rect;
    let corners = [
        Vec2::new(px0, pz0),
        Vec2::new(px1, pz0),
        Vec2::new(px1, pz1),
        Vec2::new(px0, pz1),
        Vec2::new(px0, pz0),
    ];
    let mut seam = Vec::new();
    for w in corners.windows(2) {
        for k in 0..8 {
            seam.push(w[0] + (w[1] - w[0]) * (k as f32 / 8.0));
        }
    }
    seam.push(corners[4]);
    seal.add(&props::sealant(&seam, 0.06, ribbon_y, 301), Mat4::IDENTITY);
    for (i, line) in crack_lines().into_iter().enumerate() {
        if line.len() > 2 {
            seal.add(
                &props::sealant(&line, 0.035, ribbon_y, 320 + i as u32),
                Mat4::IDENTITY,
            );
        }
    }
    // Road markings: a yellow bus-stop box in front of the shelter, white parking bays.
    let line = |b: &mut Builder, a: Vec2, c: Vec2, w: f32| {
        let n = ((a.distance(c) / 0.5).ceil() as u32).max(1);
        let pts: Vec<Vec2> = (0..=n)
            .map(|k| a + (c - a) * (k as f32 / n as f32))
            .collect();
        let mut m = props::sealant(&pts, w, |x, z| yard_y(x, z) + 0.0022, 0);
        // Constant width (the sealant ribbon wobbles): rebuild from the center line.
        m.vertices.iter_mut().for_each(|v| v.uv[0] *= 1.0);
        let dir = (c - a).normalize();
        let side = Vec2::new(-dir.y, dir.x);
        for (k, v) in m.vertices.iter_mut().enumerate() {
            let p = pts[k / 2];
            let q = p + side * w * 0.5 * if k % 2 == 0 { 1.0 } else { -1.0 };
            v.position = [q.x, yard_y(q.x, q.y) + 0.0022, q.y];
        }
        b.add(&m, Mat4::IDENTITY);
    };
    let mut yellow = Builder::new();
    let (bx0, bx1, bz0, bz1) = (-6.4f32, 1.6f32, -2.1f32, 0.9f32);
    for (a, c) in [
        (Vec2::new(bx0, bz0), Vec2::new(bx1, bz0)),
        (Vec2::new(bx0, bz1), Vec2::new(bx1, bz1)),
        (Vec2::new(bx0, bz0), Vec2::new(bx0, bz1)),
        (Vec2::new(bx1, bz0), Vec2::new(bx1, bz1)),
    ] {
        line(&mut yellow, a, c, 0.15);
    }
    let mut white = Builder::new();
    for x in [2.6f32, 5.1] {
        line(&mut white, Vec2::new(x, -4.85), Vec2::new(x, -0.6), 0.1);
    }
    line(&mut white, Vec2::new(2.6, -0.6), Vec2::new(6.85, -0.6), 0.1);
    let lettering = {
        let mut q = props::quad(Vec3::ZERO, Vec3::X, Vec3::NEG_Z, 5.6, 1.4);
        transform(&mut q, at(-2.4, YARD_Y + 0.0024, -0.6));
        q
    };
    let kerbs = props::kerbs(-hx, hx, -hz, hz, 0.15, 0.07, 401);
    let (pc, phx, phz) = PATIO;
    let pavers = props::pavers(pc - phx, pc + phx, -phz, phz, 0.016, 402);
    let joint = heightfield((pc - phx, -phz), (pc + phx, phz), (4, 4), |_, _| {
        (0.006, Vec4::ONE)
    });
    let setts = props::setts(pc - phx, pc + phx, -phz, phz, 0.1, 0.02, 403);
    let (sc, shx, shz) = SANDPIT;
    let sand = heightfield((sc - shx, -shz), (sc + shx, shz), (50, 60), |x, z| {
        let edge = (shx - (x - sc).abs()).min(shz - z.abs());
        let rise = smooth(0.0, 0.5, edge);
        let mound = fbm(x * 0.45, z * 0.45 + 3.0, sq(0), 4, 501) * 0.5 + 0.5;
        let dents = fbm(x * 2.2, z * 2.2, sq(0), 3, 502);
        let y = 0.004 + rise * (0.03 * mound + 0.006 * dents);
        let k = 0.96 + 0.08 * mound - 0.12 * smooth(0.25, 0.0, edge);
        (y, Vec4::new(k, k, k, 1.0))
    });
    let sleepers = props::sleepers(sc - shx, sc + shx, -shz, shz, 0.1, 0.12, 404);
    let (cover, ring) = props::manhole(0.31);
    let (gully, gully_hole) = props::gully(0.5, 0.36);
    // Props.
    let s = props::shelter(601);
    let mut s_steel = s.steel;
    ground_occlusion(&mut s_steel, 0.25, 0.55);
    let shelter = [
        add("shelter.steel", s_steel),
        add("shelter.galv", s.galv),
        add("shelter.glass", s.glass),
        add("shelter.wood", s.wood),
        add("shelter.soffit", s.soffit),
        add("shelter.led", s.led),
        add("shelter.poster", s.poster),
        add("shelter.plate", s.plate),
        add("shelter.black", s.black),
    ];
    let l = props::street_lamp();
    let lamp_light = l.light;
    let lamp = [
        add("lamp.steel", l.steel),
        add("lamp.galv", l.galv),
        add("lamp.concrete", l.concrete),
        add("lamp.lens", l.lens),
    ];
    let (bollard, bands) = props::bollard();
    let (plinth, cup) = props::plinth();
    let ball = lathe(&props::sphere_profile(BALL_R, 48), 96, 80.0, false);
    let guy_dirs = [PI, PI / 3.0, -PI / 3.0];
    let m = props::mast(7.0, 2.2, guy_dirs);
    let mast_points = [m.anemometer_at, m.vane_at, m.beacon_at];
    let mut mast_concrete = m.concrete;
    ground_occlusion(&mut mast_concrete, 0.1, 0.7);
    let mast = [
        add("mast.galv", m.galv),
        add("mast.concrete", mast_concrete),
        add("mast.white", m.white),
        add("mast.solar", m.solar),
        add("mast.beacon", m.beacon),
    ];
    let (rotor, cups) = props::anemometer_rotor();
    let (vane, nose) = props::vane_rotor();
    let sock_bands: Vec<Handle<Mesh>> = (0..5)
        .map(|k| {
            let t0 = k as f32 / 5.0;
            let t1 = (k + 1) as f32 / 5.0;
            let r = |t: f32| 0.19 - 0.09 * t;
            add(
                &format!("windsock.band{k}"),
                props::sock_band(r(t0), r(t1), SOCK_BAND_LEN, 700 + k),
            )
        })
        .collect();
    let mut screen = props::stevenson_screen();
    ground_occlusion(&mut screen, 0.3, 0.6);
    let (g_body, g_glass, g_brass, g_plate) = props::rain_gauge();
    let gauge_water = lathe(
        &[(0.0, 0.0), (0.0095, 0.0), (0.0095, 1.0), (0.0, 1.0)],
        16,
        30.0,
        false,
    );
    let t = props::thermometer();
    let column = lathe(
        &[(0.0, 0.0), (0.0105, 0.0), (0.0105, 1.0), (0.0, 1.0)],
        14,
        30.0,
        false,
    );
    let seg_h = props::led_segment(SEG_W, SEG_T);
    let mut seg_v = props::led_segment(SEG_H * 0.5, SEG_T);
    transform(&mut seg_v, Mat4::from_rotation_z(FRAC_PI_2));
    let led_dot = lathe(
        &[(0.0, 0.0), (0.009, 0.0), (0.009, 0.004), (0.0, 0.004)],
        12,
        30.0,
        false,
    );
    let (e0, e1, e2, e3) = ENCLOSURE;
    let (fence, fence_hw) = props::fence(e0, e1, e2, e3, (5.0, 6.1), 801);
    let (sign_posts, sign_edge, sign_face) = props::sign(1.4, 0.44, 0.62);
    let flag = props::bus_flag();
    let (leaves, tufts) = litter();
    Meshes {
        asphalt: add("asphalt", asphalt),
        patch: add("asphalt_patch", patch),
        sealant: add("sealant", seal.finish()),
        lines_white: add("lines_white", white.finish()),
        lines_yellow: add("lines_yellow", yellow.finish()),
        lettering: add("bus_stop_lettering", lettering),
        kerbs: add("kerbs", kerbs),
        pavers: add("pavers", pavers),
        joint: add("paver_joints", joint),
        setts: add("setts", setts),
        sand: add("sand", sand),
        sleepers: add("sleepers", sleepers),
        manhole_cover: add("manhole_cover", cover),
        manhole_ring: add("manhole_ring", ring),
        gully: add("gully", gully),
        gully_hole: add("gully_hole", gully_hole),
        shelter,
        lamp,
        lamp_light,
        bollard: add("bollard", bollard),
        bollard_bands: add("bollard_bands", bands),
        plinth: add("plinth", plinth),
        plinth_cup: add("plinth_cup", cup),
        ball: add("chrome_ball", ball),
        mast,
        mast_points,
        rotor: add("anemometer.rotor", rotor),
        cups: add("anemometer.cups", cups),
        vane: add("vane", vane),
        vane_nose: add("vane.nose", nose),
        sock_pole: add("windsock.pole", props::windsock_pole(SOCK_POLE_H)),
        sock_frame: add("windsock.frame", props::windsock_frame(0.2)),
        sock_bands,
        screen: add("stevenson_screen", screen),
        screen_hw: add("stevenson_screen.hardware", props::screen_hardware()),
        gauge: [
            add("rain_gauge.body", g_body),
            add("rain_gauge.glass", g_glass),
            add("rain_gauge.brass", g_brass),
            add("rain_gauge.scale", g_plate),
        ],
        gauge_water: add("rain_gauge.water", gauge_water),
        stake: add("snow_stake", props::snow_stake()),
        mound: add("snow_mound", props::snow_mound()),
        thermo: [
            add("thermometer.galv", t.galv),
            add("thermometer.board", t.board),
            add("thermometer.glass", t.glass),
            add("thermometer.bulb", t.bulb),
            add("thermometer.housing", t.housing),
            add("thermometer.face", t.face),
        ],
        thermo_column: add("thermometer.column", column),
        seg_h: add("led.segment_h", seg_h),
        seg_v: add("led.segment_v", seg_v),
        led_dot: add("led.dot", led_dot),
        fence: add("fence", fence),
        fence_hw: add("fence.hardware", fence_hw),
        sign: [
            add("sign.posts", sign_posts),
            add("sign.edge", sign_edge),
            add("sign.face", sign_face),
        ],
        flag: [
            add("bus_flag.galv", flag.galv),
            add("bus_flag.steel", flag.steel),
            add("bus_flag.face", flag.face),
            add("bus_flag.sheet", flag.sheet),
            add("bus_flag.glass", flag.glass),
        ],
        leaves: add("leaves", props::leaf_litter(&leaves)),
        tufts: add("grass_tufts", props::tufts(&tufts)),
    }
}

/// Length of each of the windsock's five fabric bands, meters.
pub(crate) const SOCK_BAND_LEN: f32 = 0.4;
const SOCK_POLE_H: f32 = 4.6;
/// Seven-segment digit size: width, height, stroke.
const SEG_W: f32 = 0.085;
const SEG_H: f32 = 0.17;
const SEG_T: f32 = 0.02;

/// Spawns parts under the station root.
struct Spawner<'w> {
    world: &'w mut World,
    root: Entity,
}

impl Spawner<'_> {
    fn part(
        &mut self,
        parent: Entity,
        mesh: Handle<Mesh>,
        mat: Handle<StandardMaterial>,
        pos: Vec3,
        rot: Quat,
    ) -> Entity {
        self.world
            .spawn((
                Transform::from_translation(pos.as_dvec3()).with_rotation(rot),
                MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(mat)),
                ChildOf(parent),
            ))
            .id()
    }

    fn at(&mut self, mesh: Handle<Mesh>, mat: Handle<StandardMaterial>, pos: Vec3) -> Entity {
        let root = self.root;
        self.part(root, mesh, mat, pos, Quat::IDENTITY)
    }

    fn group(&mut self, name: &str, pos: Vec3, rot: Quat) -> Entity {
        let root = self.root;
        self.world
            .spawn((
                Name::new(format!("Weather station: {name}")),
                Transform::from_translation(pos.as_dvec3()).with_rotation(rot),
                ChildOf(root),
            ))
            .id()
    }

    fn no_shadows(&mut self, e: Entity) {
        if let Some(mut r) = self.world.get_mut::<MeshRenderer>(e) {
            r.cast_shadows = false;
        }
    }
}

fn spawn(world: &mut World, origin: DVec3, kit: &StationKit) {
    let (m, k) = (&kit.m, &kit.mesh);
    let root = world
        .spawn((
            Name::new("Weather station"),
            WeatherStation,
            Transform::from_translation(origin),
        ))
        .id();
    let mut s = Spawner { world, root };
    let id = Quat::IDENTITY;

    // Ground surfaces: asphalt pools, sand soaks, stone in between. Tops sit at the ground.
    let (hx, hz) = YARD;
    let yard = s.at(k.asphalt, m.asphalt, Vec3::ZERO);
    s.world.entity_mut(yard).insert(GroundPatch {
        half_extents: [hx, hz],
        absorbency: 0.03,
        puddles: 0.5,
    });
    for (mesh, mat) in [
        (k.patch, m.asphalt_patch),
        (k.sealant, m.sealant),
        (k.lines_white, m.paint_white),
        (k.lines_yellow, m.paint_yellow),
        (k.lettering, m.lettering),
    ] {
        let e = s.at(mesh, mat, Vec3::ZERO);
        s.no_shadows(e);
    }
    s.at(k.kerbs, m.concrete, Vec3::ZERO);
    let (pc, phx, phz) = PATIO;
    s.at(k.pavers, m.paver, Vec3::ZERO);
    // Footprints are centered on their entity: park each on a helper at the center.
    let patio_patch = s.group("stone patio", Vec3::new(pc, 0.0, 0.0), id);
    s.world.entity_mut(patio_patch).insert(GroundPatch {
        half_extents: [phx, phz],
        absorbency: 0.15,
        puddles: 0.3,
    });
    let joints = s.at(k.joint, m.joint, Vec3::ZERO);
    s.no_shadows(joints);
    s.at(k.setts, m.setts, Vec3::ZERO);
    let (sc, shx, shz) = SANDPIT;
    let sand = s.group("sand pit", Vec3::new(sc, 0.0, 0.0), id);
    s.world.entity_mut(sand).insert(GroundPatch {
        half_extents: [shx, shz],
        absorbency: 1.0,
        puddles: 0.05,
    });
    s.at(k.sand, m.sand, Vec3::ZERO);
    s.at(k.sleepers, m.timber, Vec3::ZERO);
    let mh = Vec3::new(-5.7, yard_y(-5.7, 2.6) + 0.0015, 2.6);
    s.at(k.manhole_cover, m.iron, mh);
    s.at(k.manhole_ring, m.iron, mh);
    let gully = Vec3::new(-0.75, yard_y(-0.75, -2.35) + 0.003, -2.35);
    s.at(k.gully, m.iron, gully);
    let hole = s.at(k.gully_hole, m.void, gully);
    s.no_shadows(hole);

    // Shelter: the ground under it stays dry.
    let shelter = s.group("shelter", SHELTER_AT, id);
    let mats = [
        m.steel,
        m.galv,
        m.glass,
        m.bench_wood,
        m.soffit,
        m.shelter_led,
        m.poster,
        m.plate,
        m.black,
    ];
    for (mesh, mat) in k.shelter.iter().zip(mats) {
        s.part(shelter, *mesh, mat, Vec3::ZERO, id);
    }
    let roof_y = props::shelter_roof_y(0.0) + 0.16;
    let cover = s.group(
        "shelter roof",
        SHELTER_AT + Vec3::new(0.0, roof_y, -0.0),
        id,
    );
    s.world.entity_mut(cover).insert(WeatherShelter {
        half_extents: [2.32, 0.95],
    });
    s.world.spawn((
        Transform::from_xyz(0.0, roof_y as f64 - 0.32, -0.1),
        PointLight {
            color: [1.0, 0.92, 0.8],
            intensity_lm: 0.0,
            range: 9.0,
            radius: 0.4,
        },
        StreetLamp { lumens: 1500.0 },
        ChildOf(shelter),
    ));
    for (mat, day, night) in [(m.shelter_led, 0.0, 1200.0), (m.poster, 0.0, 120.0)] {
        s.world.spawn((
            Glow {
                material: mat,
                day_cd_m2: day,
                night_cd_m2: night,
            },
            ChildOf(shelter),
        ));
    }

    // Street lamps around the yard, arms reaching over it.
    for (i, (pos, yaw)) in LAMPS.into_iter().enumerate() {
        let g = s.group(
            &format!("street lamp {}", i + 1),
            pos,
            Quat::from_rotation_y(yaw),
        );
        s.part(g, k.lamp[0], m.steel, Vec3::ZERO, id);
        s.part(g, k.lamp[1], m.galv, Vec3::ZERO, id);
        s.part(g, k.lamp[2], m.concrete, Vec3::ZERO, id);
        s.part(g, k.lamp[3], m.lamp_lens, Vec3::ZERO, id);
        s.world.spawn((
            Transform::from_translation(k.lamp_light.as_dvec3()),
            PointLight {
                color: [1.0, 0.84, 0.62],
                intensity_lm: 0.0,
                range: 22.0,
                radius: 0.12,
            },
            StreetLamp { lumens: 4500.0 },
            ChildOf(g),
        ));
    }
    s.world.spawn((
        Glow {
            material: m.lamp_lens,
            day_cd_m2: 0.0,
            night_cd_m2: 12_000.0,
        },
        ChildOf(root),
    ));

    // Bollards along the south kerb.
    for x in BOLLARDS_X {
        let g = s.group("bollard", Vec3::new(x, 0.0, YARD.1 + 0.45), id);
        s.part(g, k.bollard, m.steel, Vec3::ZERO, id);
        s.part(g, k.bollard_bands, m.retro, Vec3::ZERO, id);
    }

    // Chrome ball on its pedestal: reflects the sky and the clouds.
    let ball = s.group("chrome ball", BALL_AT, id);
    s.part(ball, k.plinth, m.cast_stone, Vec3::ZERO, id);
    s.part(ball, k.plinth_cup, m.brass, Vec3::ZERO, id);
    s.part(
        ball,
        k.ball,
        m.chrome,
        Vec3::new(0.0, props::plinth_ball_y(BALL_R), 0.0),
        id,
    );

    // Wind mast: anemometer, wind vane, beacon.
    let mast = s.group("wind mast", MAST_AT, id);
    let mast_mats = [m.galv, m.concrete, m.white_metal, m.solar, m.beacon];
    for (mesh, mat) in k.mast.iter().zip(mast_mats) {
        s.part(mast, *mesh, mat, Vec3::ZERO, id);
    }
    s.world.spawn((
        Glow {
            material: m.beacon,
            day_cd_m2: 0.0,
            night_cd_m2: 3000.0,
        },
        ChildOf(mast),
    ));
    let [anemo_at, vane_at, _] = k.mast_points;
    let rotor = s
        .world
        .spawn((
            Name::new("Weather station: anemometer"),
            Transform::from_translation(anemo_at.as_dvec3()),
            Anemometer::default(),
            ChildOf(mast),
        ))
        .id();
    s.part(rotor, k.rotor, m.galv, Vec3::ZERO, id);
    s.part(rotor, k.cups, m.plastic_black, Vec3::ZERO, id);
    let vane = s
        .world
        .spawn((
            Name::new("Weather station: wind vane"),
            Transform::from_translation(vane_at.as_dvec3()),
            WindVane::default(),
            ChildOf(mast),
        ))
        .id();
    s.part(vane, k.vane, m.galv, Vec3::ZERO, id);
    s.part(vane, k.vane_nose, m.brass, Vec3::ZERO, id);

    // Windsock: pole, swivel frame and five fabric bands that bend one after another.
    let pole = s.group("windsock", SOCK_AT, id);
    s.part(pole, k.sock_pole, m.galv, Vec3::ZERO, id);
    let throat = 0.2;
    let sock = s
        .world
        .spawn((
            Name::new("Weather station: windsock swivel"),
            Transform::from_xyz(0.0, (SOCK_POLE_H + 0.2 + throat) as f64, 0.0),
            WindSock::default(),
            ChildOf(pole),
        ))
        .id();
    s.part(sock, k.sock_frame, m.galv, Vec3::ZERO, id);
    let mut parent = sock;
    for (i, band) in k.sock_bands.iter().enumerate() {
        let z = if i == 0 { -0.02 } else { SOCK_BAND_LEN - 0.01 };
        let seg = s
            .world
            .spawn((
                Transform::from_xyz(0.0, 0.0, z as f64),
                SockSegment {
                    index: i as u32,
                    sock,
                },
                ChildOf(parent),
            ))
            .id();
        let mat = if i % 2 == 0 {
            m.fabric_orange
        } else {
            m.fabric_white
        };
        s.part(seg, *band, mat, Vec3::ZERO, id);
        parent = seg;
    }

    // Thermometer board with the digital display.
    let th = s.group("thermometer", THERMO_AT, id);
    let th_mats = [
        m.galv,
        m.thermo_board,
        m.clear_glass,
        m.red_liquid,
        m.steel,
        m.display_face,
    ];
    for (mesh, mat) in k.thermo.iter().zip(th_mats) {
        s.part(th, *mesh, mat, Vec3::ZERO, id);
    }
    s.world.spawn((
        Transform::from_xyz(0.0, props::THERMO_SCALE_Y0 as f64, 0.055),
        MeshRenderer::new(
            MeshRef::Handle(k.thermo_column),
            MaterialRef::Handle(m.red_liquid),
        ),
        ThermometerColumn {
            base: DVec3::new(0.0, props::THERMO_SCALE_Y0 as f64 - 0.12, 0.055),
            full_m: props::THERMO_SCALE_LEN + 0.12,
            min_c: -30.0 - 0.12 / props::THERMO_SCALE_LEN * 80.0,
            max_c: 50.0,
        },
        ChildOf(th),
    ));
    s.world.spawn((
        Glow {
            material: m.led_on,
            day_cd_m2: 3500.0,
            night_cd_m2: 450.0,
        },
        ChildOf(th),
    ));
    let display = Vec3::new(0.0, props::DISPLAY_Y, 0.064);
    let digit_x = [-0.3f32, -0.175, -0.05, 0.085];
    let digit_y = -0.02;
    let segs: [(Vec3, bool); 7] = [
        (Vec3::new(0.0, SEG_H * 0.5 - SEG_T * 0.5, 0.0), true),
        (Vec3::new(SEG_W * 0.5, SEG_H * 0.25, 0.0), false),
        (Vec3::new(SEG_W * 0.5, -SEG_H * 0.25, 0.0), false),
        (Vec3::new(0.0, -SEG_H * 0.5 + SEG_T * 0.5, 0.0), true),
        (Vec3::new(-SEG_W * 0.5, -SEG_H * 0.25, 0.0), false),
        (Vec3::new(-SEG_W * 0.5, SEG_H * 0.25, 0.0), false),
        (Vec3::ZERO, true),
    ];
    for (pos, x) in digit_x.iter().enumerate() {
        for (bit, (off, horizontal)) in segs.iter().enumerate() {
            let mesh = if *horizontal { k.seg_h } else { k.seg_v };
            let p = display + Vec3::new(*x, digit_y, 0.0) + *off;
            let e = s.part(th, mesh, m.led_off, p, Quat::from_rotation_z(0.0));
            s.world.entity_mut(e).insert(LedSegment {
                position: pos as u8,
                bit: bit as u8,
                on: m.led_on,
                off: m.led_off,
            });
        }
    }
    let dp = s.part(
        th,
        k.led_dot,
        m.led_off,
        display + Vec3::new(0.02, digit_y - SEG_H * 0.5 + 0.01, 0.0),
        Quat::from_rotation_x(FRAC_PI_2),
    );
    s.world.entity_mut(dp).insert(LedSegment {
        position: 4,
        bit: 0,
        on: m.led_on,
        off: m.led_off,
    });
    for (i, y) in signs::DISPLAY_UNIT_Y.iter().enumerate() {
        let e = s.part(
            th,
            k.led_dot,
            m.led_off,
            display + Vec3::new(0.18, *y, 0.0),
            Quat::from_rotation_x(FRAC_PI_2),
        );
        s.world.entity_mut(e).insert(LedSegment {
            position: 5,
            bit: i as u8,
            on: m.led_on,
            off: m.led_off,
        });
    }

    // Instrument enclosure: fence, Stevenson screen, rain gauge, snow stake, sign.
    s.at(k.fence, m.timber, Vec3::ZERO);
    s.at(k.fence_hw, m.galv, Vec3::ZERO);
    let scr = s.group("Stevenson screen", SCREEN_AT, id);
    s.part(scr, k.screen, m.white_wood, Vec3::ZERO, id);
    s.part(scr, k.screen_hw, m.brass, Vec3::ZERO, id);
    let gauge = s.group("rain gauge", GAUGE_AT, Quat::from_rotation_y(0.3));
    let g_mats = [m.galv, m.clear_glass, m.brass, m.rain_scale];
    for (mesh, mat) in k.gauge.iter().zip(g_mats) {
        s.part(gauge, *mesh, mat, Vec3::ZERO, id);
    }
    s.world.spawn((
        Transform::from_xyz(0.0, props::RAIN_TUBE_Y0 as f64, 0.105),
        MeshRenderer::new(MeshRef::Handle(k.gauge_water), MaterialRef::Handle(m.water)),
        RainGauge {
            base: DVec3::new(0.0, props::RAIN_TUBE_Y0 as f64, 0.105),
            full_m: props::RAIN_TUBE_LEN,
            capacity_mm: 30.0,
            collected_mm: 0.0,
        },
        ChildOf(gauge),
    ));
    let stake = s.group("snow stake", STAKE_AT, Quat::from_rotation_y(0.4));
    s.part(stake, k.stake, m.stake, Vec3::ZERO, id);
    s.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::new(1.0, 0.0001, 1.0)),
        MeshRenderer::new(MeshRef::Handle(k.mound), MaterialRef::Handle(m.snow)),
        SnowStake { base: DVec3::ZERO },
        ChildOf(stake),
    ));
    let sign = s.group("sign", SIGN_AT, Quat::from_rotation_y(-0.25));
    s.part(sign, k.sign[0], m.galv, Vec3::ZERO, id);
    s.part(sign, k.sign[1], m.steel, Vec3::ZERO, id);
    s.part(sign, k.sign[2], m.sign, Vec3::ZERO, id);

    // Bus stop flag with its timetable, at the head of the stop box.
    let flag = s.group("bus stop flag", FLAG_AT, Quat::from_rotation_y(-FRAC_PI_2));
    let flag_mats = [m.galv, m.steel, m.flag, m.timetable, m.clear_glass];
    for (mesh, mat) in k.flag.iter().zip(flag_mats) {
        s.part(flag, *mesh, mat, Vec3::ZERO, id);
    }

    // Fallen leaves and grass tufts.
    for (mesh, mat) in [(k.leaves, m.leaves), (k.tufts, m.grass)] {
        let e = s.at(mesh, mat, Vec3::ZERO);
        s.no_shadows(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_tops_sit_at_the_ground() {
        // The yard's top is within a centimeter of the ground everywhere (puddles are drawn on
        // it), and the ruts stay above the ground plane.
        for x in [-6.9f32, -3.0, 0.0, 4.0, 6.9] {
            for z in [-4.9f32, -1.15, 0.0, 0.9, 4.9] {
                let y = yard_y(x, z);
                assert!((0.0005..0.01).contains(&y), "{x},{z}: {y}");
            }
        }
    }

    #[test]
    fn station_stays_in_budget() {
        let mut world = World::new();
        world.init_resource::<Diagnostics>();
        let started = std::time::Instant::now();
        build(&mut world, DVec3::ZERO);
        let first = started.elapsed();
        let textures_ms = world
            .resource::<Diagnostics>()
            .get("weather.station_textures_ms")
            .unwrap_or(0.0);
        // A second station reuses every asset.
        let images_before = world.resource::<Assets<Image>>().keys().count();
        build(&mut world, DVec3::new(100.0, 0.0, 0.0));
        assert_eq!(
            world.resource::<Assets<Image>>().keys().count(),
            images_before
        );

        let mut q = world.query::<&MeshRenderer>();
        let handles: Vec<Handle<Mesh>> = q
            .iter(&world)
            .filter_map(|r| match &r.mesh {
                MeshRef::Handle(h) => Some(*h),
                _ => None,
            })
            .collect();
        let meshes = world.resource::<Assets<Mesh>>();
        let parts = handles.len();
        let triangles: usize = handles
            .iter()
            .map(|h| meshes.get(*h).map_or(0, |m| m.indices.len() / 3))
            .sum();
        // Two stations were spawned.
        let (triangles, parts) = (triangles / 2, parts / 2);
        let images = world.resource::<Assets<Image>>();
        let texels: usize = images
            .keys()
            .filter_map(|(_, h)| images.get(h))
            .map(|i| (i.width * i.height) as usize)
            .sum();
        let largest = images
            .keys()
            .filter_map(|(_, h)| images.get(h))
            .map(|i| i.width.max(i.height))
            .max()
            .unwrap_or(0);
        println!(
            "station: {triangles} triangles in {parts} parts, {images_before} textures, \
             {:.1} Mtexels (largest {largest}), built in {:.0} ms ({textures_ms:.0} ms textures)",
            texels as f64 / 1e6,
            first.as_secs_f64() * 1000.0
        );
        assert!(triangles < 300_000, "{triangles} triangles");
        assert!(largest <= 2048, "{largest}");
        assert!(world.query::<&WeatherStation>().iter(&world).count() == 2);
    }
}

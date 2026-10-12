//! The showcase map: one zone per engine system, laid out around a central plaza.
//!
//! Each zone has placeholder content built from primitives. When a module lands, its
//! `demo::spawn_demo(commands, zone.center)` replaces the placeholder (see each module's
//! README), so the layout, camera bookmarks and screenshot tour stay the same.

use engine::prelude::*;

/// A themed area of the map.
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub key: &'static str,
    pub name: &'static str,
    pub center: DVec3,
    pub radius: f64,
    /// Which module's demo goes here.
    pub module: &'static str,
}

pub const ZONES: &[Zone] = &[
    Zone { key: "plaza", name: "Material plaza", center: DVec3::new(0.0, 0.0, 0.0), radius: 40.0, module: "render" },
    Zone { key: "forest", name: "Living forest", center: DVec3::new(-190.0, 0.0, -120.0), radius: 90.0, module: "foliage" },
    Zone { key: "lake", name: "Lake", center: DVec3::new(170.0, 0.0, -140.0), radius: 80.0, module: "water" },
    Zone { key: "cave", name: "Mushroom cave", center: DVec3::new(0.0, 0.0, 260.0), radius: 40.0, module: "foliage" },
    Zone { key: "fortress", name: "Siege fortress", center: DVec3::new(-230.0, 0.0, 170.0), radius: 60.0, module: "destruction" },
    Zone { key: "arena", name: "Combat arena", center: DVec3::new(-70.0, 0.0, 170.0), radius: 45.0, module: "npc, movement" },
    Zone { key: "field", name: "Grass field", center: DVec3::new(130.0, 0.0, 150.0), radius: 60.0, module: "fire, smoke, weather" },
    Zone { key: "track", name: "Vehicle track", center: DVec3::new(0.0, 0.0, -430.0), radius: 160.0, module: "vehicles" },
    Zone { key: "spaceport", name: "Spaceport", center: DVec3::new(400.0, 0.0, 230.0), radius: 120.0, module: "space, vehicles" },
];

pub fn zone(key: &str) -> &'static Zone {
    ZONES.iter().find(|z| z.key == key).expect("zone")
}

/// Shared materials so instances batch.
struct Palette {
    grass: StandardMaterial,
    rock: StandardMaterial,
    dark_rock: StandardMaterial,
    bark: StandardMaterial,
    leaves: [StandardMaterial; 3],
    concrete: StandardMaterial,
    asphalt: StandardMaterial,
    brick: StandardMaterial,
    metal: StandardMaterial,
    wood: StandardMaterial,
}

impl Palette {
    fn new() -> Self {
        let leaf = |hex| {
            StandardMaterial::hex(hex)
                .with_roughness(0.7)
                .with_subsurface(0.45)
                .with_wind_sway(0.35)
        };
        Self {
            grass: StandardMaterial::hex(0x55743a).with_roughness(0.95),
            rock: StandardMaterial::hex(0x8a8580).with_roughness(0.85),
            dark_rock: StandardMaterial::hex(0x3e3b39).with_roughness(0.9),
            bark: StandardMaterial::hex(0x5a4030).with_roughness(0.9),
            leaves: [leaf(0x3f6b2a), leaf(0x557f2c), leaf(0x2f5a2f)],
            concrete: StandardMaterial::hex(0xa8a49c).with_roughness(0.8),
            asphalt: StandardMaterial::hex(0x2c2c2e).with_roughness(0.75),
            brick: StandardMaterial::hex(0x9c8670).with_roughness(0.9),
            metal: StandardMaterial::hex(0xc8ccd4).with_metallic(1.0).with_roughness(0.3),
            wood: StandardMaterial::hex(0x8a6239).with_roughness(0.8),
        }
    }
}

fn spawn(commands: &mut Commands, shape: Shape, material: StandardMaterial, transform: Transform) {
    commands.spawn((transform, MeshRenderer::new(shape, material)));
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

/// Build the whole map.
pub fn spawn_world(commands: &mut Commands, seed: u64) {
    let pal = Palette::new();
    let mut rng = Rng::new(seed);

    // Ground: 3 km of grass (the planet continues beyond it).
    spawn(
        commands,
        Shape::Plane { size: [3000.0, 3000.0], subdivisions: 1 },
        pal.grass,
        Transform::default(),
    );

    plaza(commands, &pal);
    forest(commands, &pal, &mut rng);
    lake(commands, &pal, &mut rng);
    cave(commands, &pal, &mut rng);
    fortress(commands, &pal);
    arena(commands, &pal);
    field(commands, &pal, &mut rng);
    track(commands, &pal);
    spaceport(commands, &pal);
    moon(commands);
}

fn plaza(commands: &mut Commands, pal: &Palette) {
    let c = zone("plaza").center;
    spawn(commands, Shape::Cylinder { radius: 30.0, height: 0.2, segments: 64 }, pal.concrete, at(c + DVec3::new(0.0, 0.1, 0.0)));
    // Roughness ladder: dielectric (front) and metal (back).
    for i in 0..7 {
        let r = i as f32 / 6.0;
        let x = (i as f64 - 3.0) * 2.6;
        spawn(
            commands,
            Shape::Sphere { radius: 1.0, segments: 48 },
            StandardMaterial::hex(0xc0392b).with_roughness(r.max(0.05)),
            at(c + DVec3::new(x, 1.2, 4.0)),
        );
        spawn(
            commands,
            Shape::Sphere { radius: 1.0, segments: 48 },
            StandardMaterial::hex(0xe6b450).with_metallic(1.0).with_roughness(r.max(0.05)),
            at(c + DVec3::new(x, 1.2, 0.0)),
        );
    }
    // Emissive ring (bloom) and a glass orb.
    spawn(
        commands,
        Shape::Torus { radius: 2.5, tube: 0.25, segments: 64 },
        StandardMaterial::hex(0x202830).with_emissive([0.3, 0.7, 1.0], 30_000.0),
        Transform::from_translation(c + DVec3::new(0.0, 3.5, -6.0)).with_rotation(Quat::from_rotation_x(1.5708)),
    );
    let mut glass = StandardMaterial::hex(0xd8f0ff).with_roughness(0.05).with_alpha(0.25);
    glass.reflectance = 0.6;
    spawn(commands, Shape::Sphere { radius: 1.5, segments: 64 }, glass, at(c + DVec3::new(0.0, 3.5, -6.0)));
    // Pedestals.
    for (i, x) in [-9.0, 9.0].into_iter().enumerate() {
        spawn(commands, cuboid(2.0, 1.0, 2.0), pal.concrete, at(c + DVec3::new(x, 0.5, -6.0)));
        let shape = if i == 0 {
            Shape::Capsule { radius: 0.6, height: 2.4, segments: 32 }
        } else {
            Shape::Cone { radius: 0.9, height: 2.2, segments: 48 }
        };
        spawn(commands, shape, pal.metal, at(c + DVec3::new(x, 2.2, -6.0)));
    }
}

fn tree(commands: &mut Commands, pal: &Palette, rng: &mut Rng, base: DVec3) {
    let h = rng.range(4.0, 9.0);
    let r = rng.range(0.18, 0.35);
    spawn(
        commands,
        Shape::Cylinder { radius: r, height: h, segments: 10 },
        pal.bark,
        at(base + DVec3::new(0.0, h as f64 * 0.5, 0.0)),
    );
    let leaves = pal.leaves[rng.range_i32(0, 3) as usize];
    let conifer = rng.chance(0.5);
    let layers = if conifer { 3 } else { 2 };
    for l in 0..layers {
        let y = h as f64 * (0.55 + 0.2 * l as f64);
        let size = rng.range(1.8, 3.0) * (1.0 - 0.22 * l as f32);
        let shape = if conifer {
            Shape::Cone { radius: size, height: size * 1.6, segments: 12 }
        } else {
            Shape::Sphere { radius: size, segments: 14 }
        };
        let jitter = DVec3::new(rng.range(-0.3, 0.3) as f64, 0.0, rng.range(-0.3, 0.3) as f64);
        spawn(commands, shape, leaves, at(base + jitter + DVec3::new(0.0, y, 0.0)));
    }
}

fn forest(commands: &mut Commands, pal: &Palette, rng: &mut Rng) {
    let z = zone("forest");
    for _ in 0..360 {
        let p = rng.in_unit_disk() * z.radius as f32;
        tree(commands, pal, rng, z.center + DVec3::new(p.x as f64, 0.0, p.y as f64));
    }
    for _ in 0..40 {
        let p = rng.in_unit_disk() * z.radius as f32;
        let s = rng.range(0.4, 1.6);
        spawn(
            commands,
            Shape::Sphere { radius: 1.0, segments: 12 },
            pal.rock,
            Transform::from_translation(z.center + DVec3::new(p.x as f64, 0.2, p.y as f64))
                .with_scale(Vec3::new(s * 1.4, s * 0.7, s)),
        );
    }
}

fn lake(commands: &mut Commands, pal: &Palette, rng: &mut Rng) {
    let z = zone("lake");
    let mut water = StandardMaterial::hex(0x1d4250).with_roughness(0.04).with_alpha(0.86);
    water.reflectance = 0.5;
    water.receives_wetness = false;
    spawn(
        commands,
        Shape::Cylinder { radius: z.radius as f32 * 0.8, height: 0.02, segments: 96 },
        water,
        at(z.center + DVec3::new(0.0, 0.03, 0.0)),
    );
    for i in 0..60 {
        let a = i as f64 / 60.0 * std::f64::consts::TAU;
        let r = z.radius * 0.8 + rng.range(-2.0, 3.0) as f64;
        let s = rng.range(0.5, 1.8);
        spawn(
            commands,
            Shape::Sphere { radius: 1.0, segments: 12 },
            pal.rock,
            Transform::from_translation(z.center + DVec3::new(a.cos() * r, 0.1, a.sin() * r))
                .with_scale(Vec3::new(s * 1.5, s * 0.6, s * 1.2)),
        );
    }
    // A jetty.
    for i in 0..10 {
        spawn(
            commands,
            cuboid(2.5, 0.2, 2.0),
            pal.wood,
            at(z.center + DVec3::new(-z.radius * 0.8 + 2.0 + i as f64 * 2.0, 0.6, 0.0)),
        );
    }
}

fn cave(commands: &mut Commands, pal: &Palette, rng: &mut Rng) {
    let c = zone("cave").center;
    let (w, h, d) = (30.0f32, 9.0f32, 34.0f32);
    // Walls and roof (entrance on the -Z side facing the plaza).
    spawn(commands, cuboid(w, h, 2.0), pal.dark_rock, at(c + DVec3::new(0.0, h as f64 * 0.5, d as f64 * 0.5)));
    spawn(commands, cuboid(2.0, h, d), pal.dark_rock, at(c + DVec3::new(-w as f64 * 0.5, h as f64 * 0.5, 0.0)));
    spawn(commands, cuboid(2.0, h, d), pal.dark_rock, at(c + DVec3::new(w as f64 * 0.5, h as f64 * 0.5, 0.0)));
    spawn(commands, cuboid(w + 2.0, 3.0, d + 2.0), pal.dark_rock, at(c + DVec3::new(0.0, h as f64 + 1.5, 0.0)));
    for x in [-1.0, 1.0] {
        spawn(commands, cuboid(10.0, h, 2.0), pal.dark_rock, at(c + DVec3::new(x * 10.0, h as f64 * 0.5, -d as f64 * 0.5)));
    }
    // Glowing mushrooms around a mineral pool.
    let mut pool = StandardMaterial::hex(0x0e2a33).with_roughness(0.05);
    pool.receives_wetness = false;
    spawn(commands, Shape::Cylinder { radius: 5.0, height: 0.04, segments: 48 }, pool, at(c + DVec3::new(0.0, 0.03, 4.0)));
    let caps = [
        StandardMaterial::hex(0x1a3a40).with_emissive([0.2, 1.0, 0.9], 400.0).with_subsurface(0.5),
        StandardMaterial::hex(0x2a2040).with_emissive([0.6, 0.3, 1.0], 400.0).with_subsurface(0.5),
    ];
    let stem = StandardMaterial::hex(0xd8d0c0).with_roughness(0.6).with_subsurface(0.6);
    for i in 0..40 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = rng.range(5.5, 12.0);
        let base = c + DVec3::new((a.cos() * r) as f64, 0.0, (a.sin() * r) as f64 + 4.0);
        let s = rng.range(0.4, 1.4);
        spawn(commands, Shape::Cylinder { radius: 0.08 * s, height: 0.6 * s, segments: 8 }, stem, at(base + DVec3::new(0.0, 0.3 * s as f64, 0.0)));
        spawn(
            commands,
            Shape::Sphere { radius: 0.35 * s, segments: 16 },
            caps[i % 2],
            Transform::from_translation(base + DVec3::new(0.0, 0.62 * s as f64, 0.0)).with_scale(Vec3::new(1.0, 0.45, 1.0)),
        );
        if i % 5 == 0 {
            commands.spawn((
                at(base + DVec3::new(0.0, 0.9, 0.0)),
                PointLight {
                    color: if i % 2 == 0 { [0.3, 1.0, 0.9] } else { [0.7, 0.4, 1.0] },
                    intensity_lm: 250.0,
                    range: 9.0,
                    radius: 0.2,
                },
            ));
        }
    }
}

fn fortress(commands: &mut Commands, pal: &Palette) {
    let c = zone("fortress").center;
    // Brick wall: 24 × 8 bricks with an offset bond.
    for row in 0..8 {
        for i in 0..24 {
            let x = (i as f64 - 12.0) * 2.0 + if row % 2 == 0 { 0.0 } else { 1.0 };
            spawn(commands, cuboid(1.96, 0.98, 1.0), pal.brick, at(c + DVec3::new(x, 0.5 + row as f64, 0.0)));
        }
    }
    for x in [-26.0, 26.0] {
        spawn(commands, Shape::Cylinder { radius: 4.0, height: 14.0, segments: 32 }, pal.brick, at(c + DVec3::new(x, 7.0, 0.0)));
        spawn(commands, Shape::Cone { radius: 4.6, height: 5.0, segments: 32 }, StandardMaterial::hex(0x6e3b2a), at(c + DVec3::new(x, 16.5, 0.0)));
    }
    // Wooden barricades in front.
    for i in 0..4 {
        spawn(commands, cuboid(4.0, 2.0, 0.3), pal.wood, at(c + DVec3::new(-12.0 + i as f64 * 8.0, 1.0, -10.0)));
    }
}

fn arena(commands: &mut Commands, pal: &Palette) {
    let c = zone("arena").center;
    spawn(commands, Shape::Cylinder { radius: 40.0, height: 0.3, segments: 64 }, StandardMaterial::hex(0x8c7a5b).with_roughness(0.95), at(c + DVec3::new(0.0, 0.15, 0.0)));
    for i in 0..8 {
        let a = i as f64 / 8.0 * std::f64::consts::TAU;
        spawn(commands, Shape::Cylinder { radius: 1.2, height: 8.0, segments: 24 }, pal.concrete, at(c + DVec3::new(a.cos() * 18.0, 4.0, a.sin() * 18.0)));
    }
    // Cover, a ramp and a wall-run wall.
    for (x, z) in [(-8.0, 4.0), (6.0, -6.0), (0.0, 10.0)] {
        spawn(commands, cuboid(4.0, 1.2, 1.0), pal.concrete, at(c + DVec3::new(x, 0.6, z)));
    }
    spawn(
        commands,
        cuboid(4.0, 0.5, 10.0),
        pal.concrete,
        Transform::from_translation(c + DVec3::new(12.0, 1.6, 8.0)).with_rotation(Quat::from_rotation_x(0.35)),
    );
    spawn(commands, cuboid(0.8, 6.0, 24.0), pal.concrete, at(c + DVec3::new(-24.0, 3.0, 0.0)));
    // Placeholder fighters.
    let red = StandardMaterial::hex(0xa83232).with_roughness(0.5);
    let blue = StandardMaterial::hex(0x3260a8).with_roughness(0.5);
    for (i, (x, z)) in [(-3.0, -2.0), (4.0, 3.0), (2.0, -8.0)].into_iter().enumerate() {
        spawn(commands, Shape::Capsule { radius: 0.4, height: 1.8, segments: 16 }, red, at(c + DVec3::new(x, 0.9 + 0.3, z)));
        if i == 0 {
            spawn(commands, Shape::Capsule { radius: 0.4, height: 1.8, segments: 16 }, blue, at(c + DVec3::new(0.0, 1.2, 0.0)));
        }
    }
}

fn field(commands: &mut Commands, pal: &Palette, rng: &mut Rng) {
    let z = zone("field");
    let blades = [
        StandardMaterial::hex(0x6b8f3a).with_roughness(0.8).with_subsurface(0.5).with_wind_sway(0.25),
        StandardMaterial::hex(0x8a9a40).with_roughness(0.8).with_subsurface(0.5).with_wind_sway(0.25),
    ];
    for i in 0..2500 {
        let p = rng.in_unit_disk() * z.radius as f32;
        let h = rng.range(0.5, 1.1);
        spawn(
            commands,
            Shape::Cone { radius: 0.06, height: h, segments: 4 },
            blades[i % 2],
            Transform::from_translation(z.center + DVec3::new(p.x as f64, h as f64 * 0.5, p.y as f64))
                .with_rotation(Quat::from_rotation_z(rng.range(-0.15, 0.15))),
        );
    }
    for i in 0..6 {
        spawn(commands, cuboid(1.0, 1.0, 1.0), pal.wood, at(z.center + DVec3::new(-10.0 + i as f64 * 3.5, 0.5, -z.radius * 0.5)));
    }
    spawn(commands, Shape::Cylinder { radius: 2.0, height: 0.3, segments: 24 }, pal.rock, at(z.center + DVec3::new(0.0, 0.15, 0.0)));
}

fn track(commands: &mut Commands, pal: &Palette) {
    let z = zone("track");
    let segs = 48;
    let (rx, rz) = (z.radius, z.radius * 0.55);
    for i in 0..segs {
        let a0 = i as f64 / segs as f64 * std::f64::consts::TAU;
        let a1 = (i + 1) as f64 / segs as f64 * std::f64::consts::TAU;
        let p0 = DVec3::new(a0.cos() * rx, 0.0, a0.sin() * rz);
        let p1 = DVec3::new(a1.cos() * rx, 0.0, a1.sin() * rz);
        let mid = (p0 + p1) * 0.5;
        let dir = (p1 - p0).as_vec3();
        let len = dir.length();
        let yaw = (-dir.z).atan2(dir.x);
        spawn(
            commands,
            cuboid(len + 0.6, 0.05, 12.0),
            pal.asphalt,
            Transform::from_translation(z.center + mid + DVec3::new(0.0, 0.03, 0.0)).with_rotation(Quat::from_rotation_y(yaw)),
        );
    }
    spawn(
        commands,
        cuboid(10.0, 0.5, 12.0),
        pal.concrete,
        Transform::from_translation(z.center + DVec3::new(0.0, 1.2, rz)).with_rotation(Quat::from_rotation_z(0.25)),
    );
    // A car placeholder.
    let body = StandardMaterial::hex(0xb02020).with_metallic(0.6).with_roughness(0.25);
    let car = z.center + DVec3::new(rx, 0.0, 0.0);
    spawn(commands, cuboid(1.9, 0.7, 4.4), body, at(car + DVec3::new(0.0, 0.75, 0.0)));
    spawn(commands, cuboid(1.7, 0.55, 2.2), body, at(car + DVec3::new(0.0, 1.35, -0.3)));
    for (x, zz) in [(-0.95, 1.4), (0.95, 1.4), (-0.95, -1.4), (0.95, -1.4)] {
        spawn(
            commands,
            Shape::Cylinder { radius: 0.36, height: 0.25, segments: 20 },
            StandardMaterial::hex(0x151515).with_roughness(0.9),
            Transform::from_translation(car + DVec3::new(x, 0.36, zz)).with_rotation(Quat::from_rotation_z(1.5708)),
        );
    }
}

fn spaceport(commands: &mut Commands, pal: &Palette) {
    let c = zone("spaceport").center;
    spawn(commands, Shape::Cylinder { radius: 30.0, height: 0.4, segments: 64 }, pal.concrete, at(c + DVec3::new(0.0, 0.2, 0.0)));
    for i in 0..12 {
        let a = i as f64 / 12.0 * std::f64::consts::TAU;
        commands.spawn((
            at(c + DVec3::new(a.cos() * 28.0, 0.6, a.sin() * 28.0)),
            PointLight { color: [1.0, 0.5, 0.2], intensity_lm: 1500.0, range: 12.0, radius: 0.1 },
        ));
        spawn(
            commands,
            Shape::Sphere { radius: 0.2, segments: 8 },
            StandardMaterial::hex(0x331100).with_emissive([1.0, 0.5, 0.2], 3000.0),
            at(c + DVec3::new(a.cos() * 28.0, 0.5, a.sin() * 28.0)),
        );
    }
    // Ship placeholder.
    let hull = StandardMaterial::hex(0xdfe3e8).with_metallic(0.8).with_roughness(0.35);
    let ship = c + DVec3::new(0.0, 4.0, 0.0);
    let along_z = Quat::from_rotation_x(1.5708);
    spawn(commands, Shape::Capsule { radius: 2.2, height: 16.0, segments: 32 }, hull, Transform::from_translation(ship).with_rotation(along_z));
    spawn(commands, Shape::Cone { radius: 2.2, height: 4.0, segments: 32 }, hull, Transform::from_translation(ship + DVec3::new(0.0, 0.0, -9.5)).with_rotation(Quat::from_rotation_x(-1.5708)));
    spawn(commands, cuboid(16.0, 0.4, 5.0), hull, at(ship + DVec3::new(0.0, -0.5, 3.0)));
    spawn(
        commands,
        Shape::Cylinder { radius: 1.4, height: 1.0, segments: 24 },
        StandardMaterial::hex(0x101820).with_emissive([0.4, 0.7, 1.0], 20_000.0),
        Transform::from_translation(ship + DVec3::new(0.0, 0.0, 8.3)).with_rotation(along_z),
    );
    // Scale: a 600 m tower of rings.
    let tower = c + DVec3::new(70.0, 0.0, 60.0);
    for i in 0..40 {
        let y = i as f64 * 15.0;
        let r = 14.0 - i as f32 * 0.25;
        spawn(commands, Shape::Cylinder { radius: r, height: 12.0, segments: 48 }, pal.concrete, at(tower + DVec3::new(0.0, y + 6.0, 0.0)));
        spawn(commands, Shape::Torus { radius: r + 1.0, tube: 0.5, segments: 48 }, pal.metal, at(tower + DVec3::new(0.0, y + 13.5, 0.0)));
    }
    spawn(
        commands,
        Shape::Sphere { radius: 2.0, segments: 24 },
        StandardMaterial::hex(0x400000).with_emissive([1.0, 0.1, 0.05], 50_000.0),
        at(tower + DVec3::new(0.0, 606.0, 0.0)),
    );
}

/// The Moon at its real size and distance: 1 737 km radius, 384 400 km away.
fn moon(commands: &mut Commands) {
    let dir = DVec3::new(-0.45, 0.35, -0.82).normalize();
    spawn(
        commands,
        Shape::Sphere { radius: 1_737_400.0, segments: 96 },
        StandardMaterial::hex(0x9a9690).with_roughness(0.95),
        at(dir * 384_400_000.0),
    );
}

/// The Moon's direction (for camera bookmarks).
pub fn moon_position() -> DVec3 {
    DVec3::new(-0.45, 0.35, -0.82).normalize() * 384_400_000.0
}

//! The physics proving ground: a 70 × 50 m concrete yard showing what the rigid-body module
//! does. `games/showcase` places it in its physics zone; on its own:
//!
//! ```sh
//! cargo run -p engine_physics --release --example proving_ground -- out_dir
//! ```
//!
//! Around `origin` (x right, z toward the default camera), back row to front row:
//!
//! * a 30-crate tower that stands perfectly still,
//! * a bin that 250 crates, barrels and balls rain into,
//! * a snake of dominoes that knocks itself over,
//! * three ragdolls stumbling down a steep staircase,
//! * a wrecking ball on a chain swinging into a crate wall,
//! * a seesaw catapult: a steel ball lands on one end and throws a crate,
//! * a motor-driven paddle wheel churning a bin of balls and blocks,
//! * a sagging plank bridge that crates drop onto,
//! * a kinematic turntable carrying crates around,
//! * rubber balls bouncing in the middle.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::{MeshRenderer, Shape, StandardMaterial};
use engine_world::physics::{
    Collider, ColliderShape, Joint, JointKind, JointMotor, RigidBody, Velocity,
};
use engine_world::{SurfaceId, SurfaceMaterial, SurfaceMaterials};

/// Size of the concrete yard (x, z), meters. Its top is at `origin.y`.
pub const YARD_SIZE: [f64; 2] = [70.0, 50.0];

/// Build the proving ground around `origin` (the center of the yard's top surface).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| build(world, origin));
}

struct Yard<'w> {
    world: &'w mut World,
    origin: DVec3,
    concrete: SurfaceId,
    wood: SurfaceId,
    metal: SurfaceId,
    rubber: SurfaceId,
}

fn mat(rgb: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(rgb).with_roughness(roughness)
}

fn cuboid_mesh(half: Vec3) -> Shape {
    Shape::Cuboid {
        size: (half * 2.0).to_array(),
    }
}

impl Yard<'_> {
    fn at(&self, local: DVec3) -> DVec3 {
        self.origin + local
    }

    /// A static box (walls, posts, steps).
    fn block(&mut self, local: DVec3, half: Vec3, rotation: Quat, color: u32) -> Entity {
        let at = self.at(local);
        self.world
            .spawn((
                Transform::from_translation(at).with_rotation(rotation),
                RigidBody::fixed(),
                Collider::cuboid(half).with_surface(self.concrete),
                MeshRenderer::new(cuboid_mesh(half), mat(color, 0.85)),
            ))
            .id()
    }

    fn body(
        &mut self,
        local: DVec3,
        rotation: Quat,
        collider: Collider,
        mesh: Shape,
        material: StandardMaterial,
    ) -> Entity {
        let at = self.at(local);
        self.world
            .spawn((
                Transform::from_translation(at).with_rotation(rotation),
                RigidBody::dynamic(),
                collider,
                Velocity::default(),
                MeshRenderer::new(mesh, material),
            ))
            .id()
    }

    /// A wooden crate (density 300 kg/m³: a crate with stuff in it).
    fn crate_box(&mut self, local: DVec3, half: Vec3, rotation: Quat, color: u32) -> Entity {
        let c = Collider::cuboid(half)
            .with_surface(self.wood)
            .with_density(300.0);
        self.body(local, rotation, c, cuboid_mesh(half), mat(color, 0.8))
    }

    fn ball(
        &mut self,
        local: DVec3,
        radius: f32,
        density: f32,
        surface: SurfaceId,
        m: StandardMaterial,
    ) -> Entity {
        let c = Collider::sphere(radius)
            .with_surface(surface)
            .with_density(density);
        let mesh = Shape::Sphere {
            radius,
            segments: 24,
        };
        self.body(local, Quat::IDENTITY, c, mesh, m)
    }

    fn capsule(
        &mut self,
        local: DVec3,
        rotation: Quat,
        radius: f32,
        half_height: f32,
        color: u32,
    ) -> Entity {
        let c = Collider::capsule(radius, half_height).with_density(1000.0);
        let mesh = Shape::Capsule {
            radius,
            height: 2.0 * (half_height + radius),
            segments: 16,
        };
        self.body(local, rotation, c, mesh, mat(color, 0.7))
    }

    /// A joint at world point `at` (yard-local), with anchors computed from the bodies' poses.
    fn joint(&mut self, kind: JointKind, a: Entity, b: Option<Entity>, local: DVec3) -> Entity {
        let at = self.at(local);
        let ta = *self.world.get::<Transform>(a).unwrap();
        let anchor_a = ta.rotation.inverse() * (at - ta.translation).as_vec3();
        let anchor_b = match b {
            Some(b) => {
                let tb = *self.world.get::<Transform>(b).unwrap();
                (tb.rotation.inverse() * (at - tb.translation).as_vec3()).as_dvec3()
            }
            None => at,
        };
        self.world
            .spawn(Joint {
                kind,
                body_a: a,
                body_b: b,
                anchor_a,
                anchor_b,
                break_force: None,
                collide_connected: false,
            })
            .id()
    }

    fn name(&mut self, e: Entity, name: &str) {
        self.world.entity_mut(e).insert(Name::new(name.to_string()));
    }
}

fn build(world: &mut World, origin: DVec3) {
    let (concrete, wood, metal, rubber) = {
        let mut table = world.get_resource_or_init::<SurfaceMaterials>();
        let rubber = match table.id("rubber") {
            Some(id) => id,
            None => table.add(SurfaceMaterial {
                name: "rubber".into(),
                friction: 0.9,
                restitution: 0.85,
                density: 1100.0,
                footstep_sound: "footstep.rubber".into(),
                impact_effect: "impact.rubber".into(),
                ..Default::default()
            }),
        };
        (
            table.id_or_default("concrete"),
            table.id_or_default("wood"),
            table.id_or_default("metal"),
            rubber,
        )
    };
    let mut y = Yard {
        world,
        origin,
        concrete,
        wood,
        metal,
        rubber,
    };
    let half = Vec3::new(YARD_SIZE[0] as f32 * 0.5, 0.25, YARD_SIZE[1] as f32 * 0.5);
    let pad = y.block(DVec3::new(0.0, -0.25, 0.0), half, Quat::IDENTITY, 0x8c8b86);
    y.name(pad, "Proving ground");
    // A curb around the yard keeps rolling crates and balls in.
    let (hx, hz, curb_h, curb_t) = (half.x, half.z, 0.8f32, 0.4f32);
    for s in [-1.0f32, 1.0] {
        y.block(
            DVec3::new(0.0, curb_h as f64 * 0.5, (s * (hz - curb_t * 0.5)) as f64),
            Vec3::new(hx, curb_h * 0.5, curb_t * 0.5),
            Quat::IDENTITY,
            0xc9a227,
        );
        y.block(
            DVec3::new((s * (hx - curb_t * 0.5)) as f64, curb_h as f64 * 0.5, 0.0),
            Vec3::new(curb_t * 0.5, curb_h * 0.5, hz - curb_t),
            Quat::IDENTITY,
            0xc9a227,
        );
    }

    tower(&mut y, DVec3::new(-28.0, 0.0, -16.0));
    pile(&mut y, DVec3::new(-14.0, 0.0, -14.0));
    dominoes(&mut y, DVec3::new(-3.0, 0.0, -16.0));
    ragdoll_stairs(&mut y, DVec3::new(24.0, 0.0, -10.0));
    wrecking_ball(&mut y, DVec3::new(-26.0, 0.0, 10.0));
    seesaw(&mut y, DVec3::new(-10.0, 0.0, 10.0));
    paddle_wheel(&mut y, DVec3::new(4.0, 0.0, 10.0));
    bridge(&mut y, DVec3::new(18.0, 0.0, 10.0));
    turntable(&mut y, DVec3::new(30.0, 0.0, 10.0));
    bouncy_balls(&mut y, DVec3::new(6.0, 0.0, -3.0));
}

/// Deterministic pseudo-random numbers in 0..1 (the demo looks the same every run).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

const CRATE_COLORS: [u32; 4] = [0xb07a45, 0x9c6a3c, 0xc48d55, 0x8a5a32];

fn tower(y: &mut Yard, base: DVec3) {
    let half = Vec3::splat(0.3);
    for i in 0..30 {
        let c = CRATE_COLORS[i % CRATE_COLORS.len()];
        let e = y.crate_box(
            base + DVec3::new(0.0, 0.3 + 0.6 * i as f64, 0.0),
            half,
            Quat::IDENTITY,
            c,
        );
        if i == 29 {
            y.name(e, "Tower top crate");
        }
    }
}

/// A bin (8 × 8 m inside) and 250 crates, barrels and balls dropped into it.
fn pile(y: &mut Yard, center: DVec3) {
    let wall = 0x6f6d68;
    for (dx, dz, hx, hz) in [
        (4.15, 0.0, 0.15, 4.3),
        (-4.15, 0.0, 0.15, 4.3),
        (0.0, 4.15, 4.0, 0.15),
        (0.0, -4.15, 4.0, 0.15),
    ] {
        y.block(
            center + DVec3::new(dx, 0.8, dz),
            Vec3::new(hx, 0.8, hz),
            Quat::IDENTITY,
            wall,
        );
    }
    let mut rng = Rng(0x5eed_1234);
    for i in 0..250 {
        let (gx, gz, layer) = ((i % 6) as f64, ((i / 6) % 6) as f64, (i / 36) as f64);
        let at = center
            + DVec3::new(
                (gx - 2.5) * 1.2 + rng.next() as f64 * 0.2,
                2.5 + layer * 1.3,
                (gz - 2.5) * 1.2 + rng.next() as f64 * 0.2,
            );
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            rng.next() * TAU,
            rng.next() * TAU,
            rng.next() * TAU,
        );
        match i % 5 {
            0..=2 => {
                let half = Vec3::new(
                    0.25 + rng.next() * 0.2,
                    0.2 + rng.next() * 0.15,
                    0.25 + rng.next() * 0.15,
                );
                let c = CRATE_COLORS[i % CRATE_COLORS.len()];
                y.crate_box(at, half, rotation, c);
            }
            3 => {
                // An oil barrel.
                let c = Collider {
                    shape: ColliderShape::Cylinder {
                        radius: 0.3,
                        half_height: 0.42,
                    },
                    surface: y.metal,
                    density: 400.0,
                    ..Default::default()
                };
                let mesh = Shape::Cylinder {
                    radius: 0.3,
                    height: 0.84,
                    segments: 20,
                };
                let color = [0x2f5f9e, 0xb8392f, 0x3c7d3a][i % 3];
                y.body(at, rotation, c, mesh, mat(color, 0.5).with_metallic(0.6));
            }
            _ => {
                let s = y.rubber;
                y.ball(at, 0.3, 300.0, s, mat(0xd8c23a, 0.6));
            }
        }
    }
}

/// Dominoes along an S curve; the first one is already tipping.
fn dominoes(y: &mut Yard, start: DVec3) {
    let curve = |t: f64| {
        DVec3::new(
            start.x + 17.0 * t,
            0.0,
            start.z + 4.0 * (t * 3.0 * std::f64::consts::PI).sin(),
        )
    };
    // Walk the curve in fine steps and drop a domino every 0.5 m.
    let mut placed = Vec::new();
    let (mut last, mut acc) = (curve(0.0), 0.5);
    for k in 1..=4000 {
        let p = curve(k as f64 / 4000.0);
        acc += p.distance(last);
        if acc >= 0.5 {
            let dir = (p - last).normalize();
            placed.push((p, dir));
            acc = 0.0;
        }
        last = p;
    }
    let half = Vec3::new(0.25, 0.5, 0.06);
    for (i, (p, dir)) in placed.iter().enumerate() {
        let d = dir.as_vec3();
        // The domino's thin side (local z) faces along the path.
        let rotation = Quat::from_rotation_arc(Vec3::Z, d);
        let color = if i % 2 == 0 { 0xf2efe6 } else { 0x1d1d22 };
        let c = Collider::cuboid(half)
            .with_surface(y.wood)
            .with_density(700.0);
        let e = y.body(
            *p + DVec3::Y * 0.5,
            rotation,
            c,
            cuboid_mesh(half),
            mat(color, 0.35),
        );
        if i == 0 {
            y.name(e, "First domino");
            // Tip it forward: spin about the horizontal axis across the path.
            y.world.get_mut::<Velocity>(e).unwrap().angular = Vec3::Y.cross(d) * 2.5;
        }
    }
}

/// A 10-part ragdoll with feet at `feet`, stumbling forward (+z). Returns its chest.
fn ragdoll(y: &mut Yard, feet: DVec3, shirt: u32) -> Entity {
    let (skin, pants) = (0xe0b090, 0x2c3140);
    let o = feet;
    let pelvis = {
        let half = Vec3::new(0.16, 0.1, 0.1);
        let c = Collider::cuboid(half).with_density(1000.0);
        y.body(
            o + DVec3::new(0.0, 1.0, 0.0),
            Quat::IDENTITY,
            c,
            cuboid_mesh(half),
            mat(pants, 0.8),
        )
    };
    let chest = y.capsule(
        o + DVec3::new(0.0, 1.38, 0.0),
        Quat::IDENTITY,
        0.15,
        0.12,
        shirt,
    );
    let head = y.ball(
        o + DVec3::new(0.0, 1.8, 0.0),
        0.12,
        1000.0,
        SurfaceId::DEFAULT,
        mat(skin, 0.6),
    );
    let mut parts = vec![pelvis, chest, head];
    let cone = |c: f32| JointKind::Spherical {
        cone_limit: Some(c),
    };
    let hinge = JointKind::Revolute {
        axis: Vec3::X,
        limits: Some([-2.2, 0.0]),
        motor: None,
    };
    y.joint(
        cone(0.5),
        pelvis,
        Some(chest),
        o + DVec3::new(0.0, 1.1, 0.0),
    );
    y.joint(cone(0.6), chest, Some(head), o + DVec3::new(0.0, 1.66, 0.0));
    for side in [-1.0, 1.0] {
        let upper = y.capsule(
            o + DVec3::new(side * 0.3, 1.38, 0.0),
            Quat::IDENTITY,
            0.05,
            0.1,
            shirt,
        );
        let fore = y.capsule(
            o + DVec3::new(side * 0.3, 1.08, 0.0),
            Quat::IDENTITY,
            0.045,
            0.1,
            skin,
        );
        let thigh = y.capsule(
            o + DVec3::new(side * 0.1, 0.7, 0.0),
            Quat::IDENTITY,
            0.07,
            0.14,
            pants,
        );
        let shin = y.capsule(
            o + DVec3::new(side * 0.1, 0.28, 0.0),
            Quat::IDENTITY,
            0.06,
            0.15,
            pants,
        );
        y.joint(
            cone(1.4),
            chest,
            Some(upper),
            o + DVec3::new(side * 0.3, 1.55, 0.0),
        );
        y.joint(
            hinge.clone(),
            upper,
            Some(fore),
            o + DVec3::new(side * 0.3, 1.23, 0.0),
        );
        y.joint(
            cone(1.2),
            pelvis,
            Some(thigh),
            o + DVec3::new(side * 0.1, 0.92, 0.0),
        );
        y.joint(
            hinge.clone(),
            thigh,
            Some(shin),
            o + DVec3::new(side * 0.1, 0.5, 0.0),
        );
        parts.extend([upper, fore, thigh, shin]);
    }
    // Stumbling forward: the whole body moves, the upper body a little faster so it pitches.
    for p in parts {
        let v = if p == chest || p == head {
            RAGDOLL_PUSH + 1.0
        } else {
            RAGDOLL_PUSH
        };
        y.world.get_mut::<Velocity>(p).unwrap().linear = Vec3::new(0.0, 0.5, v);
    }
    chest
}

const RAGDOLL_PUSH: f32 = 3.5;

/// Fourteen steep steps up toward -z with a landing, and three ragdolls stumbling down them.
fn ragdoll_stairs(y: &mut Yard, foot: DVec3) {
    let (rise, run, steps) = (0.22, 0.32, 14);
    for i in 0..steps {
        let h = rise * (i + 1) as f64;
        y.block(
            foot + DVec3::new(0.0, h * 0.5, -run * (i as f64 + 0.5)),
            Vec3::new(3.0, h as f32 * 0.5, run as f32 * 0.5),
            Quat::IDENTITY,
            0x9a958c,
        );
    }
    let top = rise * steps as f64;
    let landing = foot.z - run * steps as f64 - 1.75;
    y.block(
        DVec3::new(foot.x, top * 0.5, landing),
        Vec3::new(3.0, top as f32 * 0.5, 1.75),
        Quat::IDENTITY,
        0x9a958c,
    );
    for (dx, shirt, color) in [
        (-1.6, 0x3a6ea5, "blue"),
        (0.0, 0xb4413a, "red"),
        (1.6, 0x4e8a3e, "green"),
    ] {
        let chest = ragdoll(y, DVec3::new(foot.x + dx, top, landing + 1.3), shirt);
        y.name(chest, &format!("Ragdoll ({color})"));
    }
}

/// A gantry with a steel ball on a 6-link chain, let go at 70°, and a crate wall in its path.
fn wrecking_ball(y: &mut Yard, base: DVec3) {
    let steel = 0x9aa0a8;
    let pivot = base + DVec3::new(0.0, 7.6, 0.0);
    for dz in [-2.4, 2.4] {
        y.block(
            base + DVec3::new(0.0, 3.9, dz),
            Vec3::new(0.25, 3.9, 0.25),
            Quat::IDENTITY,
            0x5a5650,
        );
    }
    y.block(
        pivot + DVec3::Y * 0.3,
        Vec3::new(0.25, 0.25, 2.65),
        Quat::IDENTITY,
        0x5a5650,
    );

    let angle = 70f32.to_radians();
    // Pulled back toward -x (the yard's edge), so the wall flies into the yard.
    let down = Vec3::new(-angle.sin(), -angle.cos(), 0.0).as_dvec3();
    let rotation = Quat::from_rotation_z(-angle);
    let link = 0.8;
    let mut prev: Option<Entity> = None;
    for i in 0..6 {
        let center = pivot + down * (link * (i as f64 + 0.5));
        let e = y.capsule(center, rotation, 0.07, 0.33, 0x3b3b3e);
        let top = pivot + down * (link * i as f64);
        let kind = JointKind::Spherical { cone_limit: None };
        match prev {
            None => y.joint(kind, e, None, top),
            Some(p) => y.joint(kind, p, Some(e), top),
        };
        prev = Some(e);
    }
    let radius = 0.8;
    let center = pivot + down * (link * 6.0 + radius as f64);
    let s = y.metal;
    let ball = y.ball(
        center,
        radius,
        7800.0,
        s,
        mat(steel, 0.3).with_metallic(1.0),
    );
    y.world.get_mut::<Transform>(ball).unwrap().rotation = rotation;
    y.name(ball, "Wrecking ball");
    y.joint(
        JointKind::Spherical { cone_limit: None },
        prev.unwrap(),
        Some(ball),
        pivot + down * (link * 6.0),
    );

    // The wall: 6 × 4 crates on the far side of the swing.
    let half = Vec3::splat(0.4);
    for row in 0..4 {
        for col in 0..6 {
            let c = CRATE_COLORS[(row + col) % CRATE_COLORS.len()];
            y.crate_box(
                base + DVec3::new(1.8, 0.4 + 0.8 * row as f64, -2.0 + 0.8 * col as f64),
                half,
                Quat::IDENTITY,
                c,
            );
        }
    }
}

/// A plank on a hinge; a 7 t steel ball lands on one end and throws the crate on the other.
fn seesaw(y: &mut Yard, base: DVec3) {
    y.block(
        base + DVec3::new(0.0, 0.4, 0.0),
        Vec3::new(0.3, 0.4, 0.6),
        Quat::IDENTITY,
        0x5a5650,
    );
    let half = Vec3::new(4.0, 0.1, 0.6);
    let plank_c = Collider::cuboid(half)
        .with_surface(y.wood)
        .with_density(300.0);
    let plank = y.body(
        base + DVec3::new(0.0, 0.9, 0.0),
        Quat::IDENTITY,
        plank_c,
        cuboid_mesh(half),
        mat(0xa8763e, 0.8),
    );
    y.name(plank, "Seesaw");
    y.joint(
        JointKind::Revolute {
            axis: Vec3::Z,
            limits: Some([-0.35, 0.35]),
            motor: None,
        },
        plank,
        None,
        base + DVec3::new(0.0, 0.8, 0.0),
    );
    let c = y.crate_box(
        base + DVec3::new(-3.5, 1.4, 0.0),
        Vec3::splat(0.4),
        Quat::IDENTITY,
        0xc48d55,
    );
    if let Some(mut col) = y.world.get_mut::<Collider>(c) {
        col.density = 120.0;
    }
    y.name(c, "Catapulted crate");
    let s = y.metal;
    let ball = y.ball(
        base + DVec3::new(3.5, 9.0, 0.0),
        0.6,
        7800.0,
        s,
        mat(0x9aa0a8, 0.3).with_metallic(1.0),
    );
    y.name(ball, "Seesaw ball");
}

/// A bin with a four-paddle wheel turned by a hinge motor, full of balls and blocks.
fn paddle_wheel(y: &mut Yard, center: DVec3) {
    let wall = 0x6f6d68;
    for (dx, dz, hx, hz) in [
        (3.15, 0.0, 0.15, 2.8),
        (-3.15, 0.0, 0.15, 2.8),
        (0.0, 2.65, 3.0, 0.15),
        (0.0, -2.65, 3.0, 0.15),
    ] {
        y.block(
            center + DVec3::new(dx, 0.7, dz),
            Vec3::new(hx, 0.7, hz),
            Quat::IDENTITY,
            wall,
        );
    }
    let hub_at = center + DVec3::new(0.0, 2.0, 0.0);
    let wheel = y
        .world
        .spawn((
            Transform::from_translation(y.at(hub_at)),
            RigidBody::dynamic(),
            Velocity::default(),
            Name::new("Paddle wheel"),
        ))
        .id();
    let along_x = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    y.world.spawn((
        Transform::default().with_rotation(along_x),
        Collider {
            shape: ColliderShape::Cylinder {
                radius: 0.3,
                half_height: 2.7,
            },
            surface: y.metal,
            density: 2000.0,
            ..Default::default()
        },
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 0.3,
                height: 5.4,
                segments: 24,
            },
            mat(0x55585e, 0.4).with_metallic(1.0),
        ),
        ChildOf(wheel),
    ));
    let half = Vec3::new(2.6, 0.75, 0.05);
    for k in 0..4 {
        let r = Quat::from_rotation_x(k as f32 * std::f32::consts::FRAC_PI_2);
        y.world.spawn((
            Transform::from_translation((r * Vec3::new(0.0, 1.05, 0.0)).as_dvec3())
                .with_rotation(r),
            Collider::cuboid(half)
                .with_surface(y.wood)
                .with_density(600.0),
            MeshRenderer::new(cuboid_mesh(half), mat(0x7a4f2a, 0.8)),
            ChildOf(wheel),
        ));
    }
    y.joint(
        JointKind::Revolute {
            axis: Vec3::X,
            limits: None,
            motor: Some(JointMotor {
                target_velocity: 1.2,
                target_position: None,
                stiffness: 0.0,
                damping: 1000.0,
                max_force: 2.0e6,
            }),
        },
        wheel,
        None,
        hub_at,
    );
    let mut rng = Rng(77);
    for i in 0..50 {
        let at = center
            + DVec3::new(
                (rng.next() as f64 - 0.5) * 5.0,
                3.5 + i as f64 * 0.15,
                (rng.next() as f64 - 0.5) * 4.0,
            );
        if i % 3 == 0 {
            y.crate_box(
                at,
                Vec3::splat(0.22),
                Quat::from_rotation_y(rng.next() * TAU),
                0x2f8f8a,
            );
        } else {
            let color = [0xe0573a, 0x3a8fe0, 0xe0c03a][i % 3];
            let s = y.rubber;
            y.ball(at, 0.25, 200.0, s, mat(color, 0.5));
        }
    }
}

/// Fifteen 0.82 m planks hinged together between two posts 12 m apart, so the deck sags about
/// 1.2 m; three crates drop onto it.
fn bridge(y: &mut Yard, center: DVec3) {
    let (span, planks, plank, height) = (12.0f64, 15usize, 0.82f64, 4.5f64);
    for dz in [-1.0, 1.0] {
        y.block(
            center + DVec3::new(0.0, height * 0.5, dz * (span * 0.5 + 0.3)),
            Vec3::new(1.3, height as f32 * 0.5, 0.3),
            Quat::IDENTITY,
            0x5a5650,
        );
    }
    // Hang the planks on a parabola whose arc length equals the deck length.
    let arc = |sag: f64| {
        let n = 2000;
        let p = |k: usize| {
            let z = -span * 0.5 + span * k as f64 / n as f64;
            DVec3::new(0.0, -sag * (1.0 - (2.0 * z / span).powi(2)), z)
        };
        let pts: Vec<DVec3> = (0..=n).map(p).collect();
        let mut lengths = vec![0.0];
        for w in pts.windows(2) {
            lengths.push(lengths.last().unwrap() + w[0].distance(w[1]));
        }
        (pts, lengths)
    };
    let deck = planks as f64 * plank;
    let (mut lo, mut hi) = (0.0, span);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if *arc(mid).1.last().unwrap() < deck {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (pts, lengths) = arc(0.5 * (lo + hi));
    let point_at = |s: f64| {
        let i = lengths.partition_point(|l| *l < s).clamp(1, pts.len() - 1);
        let t = (s - lengths[i - 1]) / (lengths[i] - lengths[i - 1]).max(1e-12);
        pts[i - 1].lerp(pts[i], t)
    };
    let ends: Vec<DVec3> = (0..=planks)
        .map(|k| center + DVec3::Y * height + point_at(k as f64 * plank))
        .collect();
    let mut prev: Option<Entity> = None;
    for k in 0..planks {
        let (a, b) = (ends[k], ends[k + 1]);
        let dir = (b - a).normalize().as_vec3();
        let half = Vec3::new(1.0, 0.06, (a.distance(b) * 0.5) as f32);
        let c = Collider::cuboid(half)
            .with_surface(y.wood)
            .with_density(500.0);
        let color = if k % 2 == 0 { 0x9c6a3c } else { 0x8a5a32 };
        let plank = y.body(
            (a + b) * 0.5,
            Quat::from_rotation_arc(Vec3::Z, dir),
            c,
            cuboid_mesh(half),
            mat(color, 0.85),
        );
        let hinge = bridge_hinge();
        match prev {
            None => y.joint(hinge, plank, None, a),
            Some(p) => y.joint(hinge, p, Some(plank), a),
        };
        prev = Some(plank);
    }
    y.joint(bridge_hinge(), prev.unwrap(), None, ends[planks]);
    for (i, dz) in [-2.0, 0.5, 3.0].into_iter().enumerate() {
        y.crate_box(
            center + DVec3::new(0.0, 5.5 + i as f64, dz),
            Vec3::splat(0.35),
            Quat::from_rotation_y(0.4 * i as f32),
            CRATE_COLORS[i],
        );
    }
}

/// A kinematic disc turning at 0.4 rad/s with crates riding on it.
/// A free hinge with a little friction, so the deck stops swinging like a real rope bridge.
fn bridge_hinge() -> JointKind {
    JointKind::Revolute {
        axis: Vec3::X,
        limits: None,
        motor: Some(JointMotor {
            target_velocity: 0.0,
            target_position: None,
            stiffness: 0.0,
            damping: 40.0,
            max_force: 400.0,
        }),
    }
}

fn turntable(y: &mut Yard, center: DVec3) {
    let table = y
        .world
        .spawn((
            Transform::from_translation(y.at(center + DVec3::Y * 0.15)),
            RigidBody::kinematic(),
            Collider {
                shape: ColliderShape::Cylinder {
                    radius: 4.0,
                    half_height: 0.15,
                },
                surface: y.concrete,
                ..Default::default()
            },
            Velocity {
                linear: Vec3::ZERO,
                angular: Vec3::new(0.0, 0.4, 0.0),
            },
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 4.0,
                    height: 0.3,
                    segments: 48,
                },
                mat(0x4a4d55, 0.5).with_metallic(0.7),
            ),
            Name::new("Turntable"),
        ))
        .id();
    let _ = table;
    for k in 0..10 {
        let a = k as f64 / 10.0 * std::f64::consts::TAU;
        let at = center + DVec3::new(a.cos() * 3.0, 0.3 + 0.35, a.sin() * 3.0);
        y.crate_box(
            at,
            Vec3::splat(0.35),
            Quat::from_rotation_y(-a as f32),
            CRATE_COLORS[k % 4],
        );
    }
}

/// Rubber balls dropped from different heights.
fn bouncy_balls(y: &mut Yard, center: DVec3) {
    let colors = [0xe0573a, 0x3a8fe0, 0xe0c03a, 0x6ac94a, 0xc04ad0, 0xf08a2a];
    for (i, color) in colors.into_iter().enumerate() {
        let at = center + DVec3::new(i as f64 * 1.6, 4.0 + i as f64 * 1.5, (i % 2) as f64 * 1.2);
        let s = y.rubber;
        y.ball(at, 0.4, 300.0, s, mat(color, 0.4));
    }
}

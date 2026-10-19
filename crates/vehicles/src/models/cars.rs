//! The sports car and the rally car: bodies, glass, interiors, lights, wheels and their
//! materials.
//!
//! Bodies are sculpted as signed-distance fields (`cars/sports.rs`, `cars/rally.rs`), meshed
//! with narrow-band surface nets, refined at creases and simplified (`cars/mesher.rs`), then
//! split into materials along exact cut lines. Everything is generated the first time a car
//! of a kind spawns and shared by key (`vehicles/cars/…`) after that, so a hundred cars use
//! one GPU copy of each mesh and texture.

pub mod atlas;
pub mod details;
pub mod glass;
pub mod mesher;
pub mod parts;
pub mod rally;
pub mod rally_kit;
pub mod sdf;
pub mod sports;
pub mod sports_kit;
pub mod surfaces;
pub mod tex;
pub mod wheel;

use engine_assets::{Assets, Handle};
use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, BoxFace, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, SkyLightVolume,
    StandardMaterial,
};
use engine_world::ExhaustKind;

use crate::exhaust::nozzles;
use crate::visuals::{BrakeLight, WheelVisual};
use crate::wheels::Wheels;

/// Meshes and textures for one kind of car, keyed for the asset stores.
#[derive(Default)]
pub struct Kit {
    pub meshes: Vec<(String, Mesh)>,
    pub images: Vec<(String, Image)>,
}

impl Kit {
    /// Triangles over all meshes (instanced parts counted once).
    pub fn triangles(&self) -> usize {
        self.meshes.iter().map(|(_, m)| m.triangle_count()).sum()
    }
}

fn key(prefix: &str, name: &str) -> String {
    format!("{prefix}/{name}")
}

/// Build every mesh and texture of the sports car (no world needed; used by the spawner and
/// by the timing test).
pub fn sports_car_kit() -> Kit {
    sports_kit::build()
}

/// Build every mesh and texture of the rally car (light pod included).
pub fn rally_car_kit() -> Kit {
    rally_kit::build()
}

/// How many times the car draws each kit mesh (wheel parts four times, seats and calipers
/// twice), for triangle budgets.
pub fn instances(mesh_key: &str) -> usize {
    let name = mesh_key.rsplit('/').next().unwrap_or("");
    match name {
        "tire" | "rim" | "disc" | "backing" => 4,
        "caliper_l" | "caliper_r" | "seat" => 2,
        _ => 1,
    }
}

/// The sports car's 19-inch wheels (front size; the rears are drawn 8 % wider).
pub const SPORTS_WHEEL: wheel::WheelSpec = wheel::WheelSpec {
    radius: 0.33,
    width: 0.25,
    rim: 0.2415,
    style: wheel::RimStyle::TwinSpoke,
    tread: wheel::Tread::Performance,
    disc: 0.19,
};

/// The rally car's 15-inch gravel wheels.
pub const RALLY_WHEEL: wheel::WheelSpec = wheel::WheelSpec {
    radius: 0.32,
    width: 0.22,
    rim: 0.1905,
    style: wheel::RimStyle::Rally,
    tread: wheel::Tread::Gravel,
    disc: 0.142,
};

/// A named kit-building job.
type Job<'a> = (&'static str, Box<dyn FnOnce() -> Kit + Send + 'a>);

/// Run kit-building jobs on their own threads and gather their parts (in job order). With
/// `CARS_PROFILE` set, prints each job's time.
fn gather(jobs: Vec<Job<'_>>) -> Kit {
    let t0 = std::time::Instant::now();
    let parts: Vec<Kit> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(name, job)| {
                s.spawn(move || {
                    let kit = job();
                    if std::env::var_os("CARS_PROFILE").is_some() {
                        eprintln!("    {name}: done at {:.3} s", t0.elapsed().as_secs_f64());
                    }
                    kit
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a kit job panicked"))
            .collect()
    });
    let mut kit = Kit::default();
    for part in parts {
        kit.meshes.extend(part.meshes);
        kit.images.extend(part.images);
    }
    kit
}

/// Wheel meshes and their textures (tire lettering and tread, disc face).
fn wheel_meshes(kit: &mut Kit, prefix: &str, spec: &wheel::WheelSpec, pistons: u32) {
    let mut tire = wheel::tire(spec);
    if spec.tread == wheel::Tread::Gravel {
        tire.merge(&wheel::gravel_blocks(spec, 44));
    }
    kit.meshes.push((key(prefix, "tire"), tire));
    kit.meshes.push((key(prefix, "rim"), wheel::rim(spec)));
    kit.meshes.push((key(prefix, "disc"), wheel::disc(spec)));
    let caliper = wheel::caliper(spec, pistons);
    kit.meshes
        .push((key(prefix, "caliper_l"), parts::mirror_x(&caliper)));
    kit.meshes.push((key(prefix, "caliper_r"), caliper));
    kit.meshes
        .push((key(prefix, "backing"), wheel::backing(spec)));
    let (brand, size) = match spec.tread {
        wheel::Tread::Performance => ("VORANTE SPORT", "275/35 ZR19 97Y"),
        wheel::Tread::Gravel => ("VORANTE GRAVEL", "205/65 R15 RALLY"),
    };
    let (color, normal) = surfaces::tire(spec, brand, size);
    kit.images.push((key(prefix, "tire_color"), color));
    kit.images.push((key(prefix, "tire_normal"), normal));
    kit.images.push((
        key(prefix, "disc_face"),
        surfaces::disc(spec.tread == wheel::Tread::Performance),
    ));
    let (color, normal) = surfaces::carbon();
    kit.images.push((key(prefix, "carbon_color"), color));
    kit.images.push((key(prefix, "carbon_normal"), normal));
}

/// Materials of one car's wheels.
#[derive(Clone, Copy)]
struct WheelLook {
    tire: StandardMaterial,
    rim: StandardMaterial,
    disc: StandardMaterial,
    caliper: StandardMaterial,
}

impl WheelLook {
    /// Tire and disc materials with the kit's textures, rim and caliper as given.
    fn new(world: &World, prefix: &str, rim: StandardMaterial, caliper: StandardMaterial) -> Self {
        let mut tire = StandardMaterial::hex(0x2a2a2a).with_roughness(0.82);
        if let Some(h) = image(world, &key(prefix, "tire_color")) {
            tire = tire.with_base_color_texture(h);
        }
        if let Some(h) = image(world, &key(prefix, "tire_normal")) {
            tire = tire.with_normal_texture(h);
        }
        let mut disc = StandardMaterial::hex(0x9a9a96)
            .with_metallic(1.0)
            .with_roughness(0.42);
        if let Some(h) = image(world, &key(prefix, "disc_face")) {
            disc = disc.with_base_color_texture(h);
        }
        Self {
            tire,
            rim,
            disc,
            caliper,
        }
    }
}

/// Wheels: tire, rim and disc spin with [`WheelVisual`]; the caliper and dust shield hang on
/// the body at the wheel's rest position (they steer with the wheel once a caliper system
/// drives them; see [`CaliperVisual`]).
#[allow(clippy::too_many_arguments)]
fn spawn_wheels(
    world: &mut World,
    body: Entity,
    prefix: &str,
    wheels: &Wheels,
    look: WheelLook,
    ground: f32,
    base_width: f32,
) {
    let backing = StandardMaterial::hex(0x1b1b1c).with_roughness(0.8);
    for (i, w) in wheels.wheels.iter().enumerate() {
        let rest = Vec3::new(w.mount.x, w.radius - ground, w.mount.z);
        let left = w.mount.x < 0.0;
        let turn = if left {
            Quat::from_rotation_y(std::f32::consts::PI)
        } else {
            Quat::IDENTITY
        };
        let scale = Vec3::new(w.width / base_width, 1.0, 1.0);
        let wheel = world
            .spawn((
                Transform::from_translation(rest.as_dvec3()),
                WheelVisual {
                    index: i,
                    retracts: false,
                    max_travel: f32::MAX,
                },
                ChildOf(body),
            ))
            .id();
        let local = Transform::from_rotation(turn).with_scale(scale);
        spawn_part(world, wheel, &key(prefix, "tire"), look.tire, local);
        spawn_part(world, wheel, &key(prefix, "rim"), look.rim, local);
        spawn_part(world, wheel, &key(prefix, "disc"), look.disc, local);
        let fixed = Transform::from_translation(rest.as_dvec3()).with_rotation(turn);
        let cal = if left { "caliper_l" } else { "caliper_r" };
        let c = spawn_part(
            world,
            body,
            &key(prefix, cal),
            look.caliper,
            Transform::from_translation(rest.as_dvec3()),
        );
        world.entity_mut(c).insert(CaliperVisual {
            index: i,
            turn: false,
        });
        let b = spawn_part(world, body, &key(prefix, "backing"), backing, fixed);
        world.entity_mut(b).insert(CaliperVisual {
            index: i,
            turn: left,
        });
    }
}

/// A part that follows wheel `index`'s steering and suspension travel without spinning
/// (brake calipers, dust shields); `turn` adds half a turn about Y (left-side parts shared
/// with the right). Until [`caliper_visuals`] runs in the app it stays at the wheel's rest
/// position.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CaliperVisual {
    pub index: usize,
    pub turn: bool,
}

/// Moves [`CaliperVisual`] parts with their wheel's steering and suspension (no spin).
pub fn caliper_visuals(
    vehicles: Query<&Wheels>,
    mut q: Query<(&ChildOf, &CaliperVisual, &mut Transform)>,
) {
    for (parent, vis, mut t) in &mut q {
        let Ok(wheels) = vehicles.get(parent.parent()) else {
            continue;
        };
        let Some(w) = wheels.wheels.get(vis.index) else {
            continue;
        };
        let st = &w.state;
        let travel = if st.grounded {
            st.spring_length
        } else {
            w.suspension.travel_m
        };
        let turn = if vis.turn {
            Quat::from_rotation_y(std::f32::consts::PI)
        } else {
            Quat::IDENTITY
        };
        let translation = (w.mount - Vec3::Y * travel).as_dvec3();
        let rotation = Quat::from_rotation_y(-st.steer) * turn;
        if t.translation != translation || t.rotation != rotation {
            t.translation = translation;
            t.rotation = rotation;
        }
    }
}

/// Frit UVs cover this many frit widths across 0..1.
const FRIT_RANGE: f32 = 16.0;

/// The glass edge: an opaque black ceramic band fading into tinted glass (u = distance from
/// the edge in frit widths / `FRIT_RANGE`).
fn frit_texture() -> Image {
    let w = 1024u32;
    Image::from_fn(w, 4, true, |x, _| {
        let d = (x as f32 + 0.5) / w as f32 * FRIT_RANGE;
        // Dot-matrix fade from the solid band into the glass.
        let t = sdf::sstep(0.62, 0.95, d);
        let rgb = [
            (4.0 + t * 20.0) as u8,
            (4.0 + t * 27.0) as u8,
            (5.0 + t * 30.0) as u8,
        ];
        let a = 1.0 + (0.42 - 1.0) * t;
        [rgb[0], rgb[1], rgb[2], (a * 255.0) as u8]
    })
}

/// Insert a kit's meshes and images unless they are already there (cheap after the first
/// car of a kind).
fn ensure_kit(world: &mut World, probe: &str, make: fn() -> Kit) -> bool {
    let Some(meshes) = world.get_resource::<Assets<Mesh>>() else {
        return false;
    };
    if meshes.by_key(probe).is_some() {
        return true;
    }
    let kit = make();
    if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
        for (k, img) in kit.images {
            images.insert_keyed(&k, img);
        }
    }
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    for (k, m) in kit.meshes {
        meshes.insert_keyed(&k, m);
    }
    true
}

fn image(world: &World, key: &str) -> Option<Handle<Image>> {
    world
        .get_resource::<Assets<Image>>()
        .and_then(|a| a.by_key(key))
}

fn spawn_part(
    world: &mut World,
    parent: Entity,
    mesh: &str,
    material: impl Into<MaterialRef>,
    transform: Transform,
) -> Entity {
    world
        .spawn((
            transform,
            MeshRenderer::new(MeshRef::Asset(mesh.to_string()), material),
            ChildOf(parent),
        ))
        .id()
}

/// A lamp that switches between `off` and `on` with the brakes (or reverse gear).
fn spawn_lamp(
    world: &mut World,
    body: Entity,
    mesh: &str,
    off: StandardMaterial,
    on: StandardMaterial,
    reverse: bool,
) {
    world.spawn((
        Transform::default(),
        MeshRenderer::new(MeshRef::Asset(mesh.to_string()), off),
        BrakeLight { off, on, reverse },
        ChildOf(body),
    ));
}

/// Materials shared by both cars.
struct Finishes {
    chrome: StandardMaterial,
    satin_chrome: StandardMaterial,
    gloss_black: StandardMaterial,
    satin_black: StandardMaterial,
    carbon: StandardMaterial,
    lens: StandardMaterial,
    projector: StandardMaterial,
    drl: StandardMaterial,
    amber: StandardMaterial,
    tail_off: StandardMaterial,
    tail_on: StandardMaterial,
    reverse_off: StandardMaterial,
    reverse_on: StandardMaterial,
    exhaust: StandardMaterial,
    mirror_glass: StandardMaterial,
    leather: StandardMaterial,
    dash: StandardMaterial,
    screen: StandardMaterial,
    liner: StandardMaterial,
    housing: StandardMaterial,
    recess: StandardMaterial,
    cabin: StandardMaterial,
}

impl Finishes {
    fn new(world: &World, prefix: &str) -> Self {
        let tex = |name: &str| image(world, &key(prefix, name));
        let mut carbon = StandardMaterial::color(1.0, 1.0, 1.0)
            .with_roughness(0.38)
            .with_uv_scale(22.0, 22.0);
        if let Some(h) = tex("carbon_color") {
            carbon = carbon.with_base_color_texture(h);
        } else {
            carbon.base_color = [0.03, 0.03, 0.032, 1.0];
        }
        if let Some(h) = tex("carbon_normal") {
            carbon = carbon.with_normal_texture(h);
        }
        carbon.reflectance = 0.4;
        let mut lens = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.02);
        lens.base_color = [0.9, 0.92, 0.95, 0.07];
        lens.alpha_mode = AlphaMode::Blend;
        lens.reflectance = 0.7;
        lens.receives_wetness = false;
        let mut projector = StandardMaterial::hex(0x0b0d11).with_roughness(0.04);
        projector.reflectance = 0.9;
        let glow = tex("tail_glow");
        let with_glow = |m: StandardMaterial| match glow {
            Some(h) => m.with_emissive_texture(h),
            None => m,
        };
        let tail_off = with_glow(
            StandardMaterial::hex(0x2e0303)
                .with_roughness(0.08)
                .with_emissive([1.0, 0.04, 0.02], 240.0),
        );
        let tail_on = with_glow(
            StandardMaterial::hex(0x8a0707)
                .with_roughness(0.1)
                .with_emissive([1.0, 0.04, 0.02], 2_600.0),
        );
        let reverse_off = StandardMaterial::hex(0x76787c).with_roughness(0.05);
        let reverse_on = reverse_off.with_emissive([1.0, 1.0, 1.0], 2_000.0);
        let mut screen = StandardMaterial::hex(0x000000).with_roughness(0.15);
        if let Some(h) = tex("screen") {
            screen = screen
                .with_emissive_texture(h)
                .with_emissive([1.0, 1.0, 1.0], 160.0);
        }
        Self {
            chrome: StandardMaterial::hex(0xeeeeee)
                .with_metallic(1.0)
                .with_roughness(0.07),
            satin_chrome: StandardMaterial::hex(0xc4c4c6)
                .with_metallic(1.0)
                .with_roughness(0.3),
            gloss_black: StandardMaterial::hex(0x0a0a0a).with_roughness(0.28),
            satin_black: StandardMaterial::hex(0x121213).with_roughness(0.55),
            carbon,
            lens,
            projector,
            drl: StandardMaterial::hex(0xffffff).with_emissive([1.0, 0.97, 0.92], 5_500.0),
            amber: StandardMaterial::hex(0xff8f10)
                .with_roughness(0.15)
                .with_emissive([1.0, 0.45, 0.02], 25.0),
            tail_off,
            tail_on,
            reverse_off,
            reverse_on,
            exhaust: StandardMaterial::hex(0xd6d2ca)
                .with_metallic(1.0)
                .with_roughness(0.18),
            mirror_glass: StandardMaterial::hex(0xe6e8ea)
                .with_metallic(1.0)
                .with_roughness(0.02),
            leather: StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.55),
            dash: StandardMaterial::hex(0x1e1e20).with_roughness(0.72),
            screen,
            liner: StandardMaterial::hex(0x141414).with_roughness(0.85),
            housing: StandardMaterial::hex(0x2a2c30)
                .with_metallic(1.0)
                .with_roughness(0.28),
            recess: StandardMaterial::hex(0x0e0e0f).with_roughness(0.6),
            cabin: StandardMaterial::hex(0x2b2b2d).with_roughness(0.75),
        }
    }
}

/// Draw the sports car on `body` (a vehicle entity with these `wheels`) in `paint` (sRGB hex).
pub fn sports_car_visuals(commands: &mut Commands, body: Entity, wheels: &Wheels, paint: u32) {
    // The twin tailpipes, for the effects modules.
    let tips = sports_kit::exhaust_exits(&sports::Body::new());
    commands
        .entity(body)
        .insert(nozzles(ExhaustKind::Tailpipe, &tips));
    let wheels = wheels.clone();
    commands.queue(move |world: &mut World| {
        let p = sports_kit::PREFIX;
        let k = |name: &str| key(p, name);
        ensure_kit(world, &k("paint"), sports_car_kit);
        let f = Finishes::new(world, p);
        let mut paint_mat = StandardMaterial::hex(paint)
            .with_metallic(0.35)
            .with_roughness(0.14);
        paint_mat.reflectance = 0.6;
        if let Some(h) = image(world, &k("atlas_color")) {
            paint_mat = paint_mat.with_base_color_texture(h);
        }
        if let Some(h) = image(world, &k("atlas_normal")) {
            paint_mat = paint_mat.with_normal_texture(h);
        }
        let glass = glass_material(image(world, &k("frit")));
        let at0 = Transform::default();
        for (name, mat) in [
            ("paint", paint_mat),
            ("mirror_paint", paint_mat),
            ("liner", f.liner),
            ("lamp_housing", f.housing),
            ("recess", f.recess),
            ("trim", f.carbon),
            ("carbon", f.carbon),
            ("glass_front", glass),
            ("glass_back", glass),
            ("glass_left", glass),
            ("glass_right", glass),
            ("cabin", f.cabin),
            ("head_lens", f.lens),
            ("head_chrome", f.chrome),
            ("head_projector", f.projector),
            ("drl", f.drl),
            ("amber", f.amber),
            ("grille", f.gloss_black),
            ("exhaust", f.exhaust),
            ("mirror_black", f.satin_black),
            ("mirror_glass", f.mirror_glass),
            ("wipers", f.satin_black),
            ("handles", f.satin_chrome),
            ("chrome", f.chrome),
            (
                "enamel",
                StandardMaterial::hex(0x0d2f7a).with_roughness(0.08),
            ),
            ("dash", f.dash),
            ("screens", f.screen),
            ("steering", f.leather),
        ] {
            spawn_part(world, body, &k(name), mat, at0);
        }
        spawn_lamp(world, body, &k("tail_red"), f.tail_off, f.tail_on, false);
        spawn_lamp(
            world,
            body,
            &k("tail_reverse"),
            f.reverse_off,
            f.reverse_on,
            true,
        );
        for x in sports_kit::SEATS {
            let at = Transform::from_xyz(
                x as f64,
                (sports_kit::SEAT_FLOOR - sports::GROUND) as f64,
                sports_kit::SEAT_Z as f64,
            );
            spawn_part(world, body, &k("seat"), f.leather, at);
        }
        world.spawn((
            Transform::from_xyz(0.0, 0.25, 0.75),
            // Sky light comes in through the side glass (the doors below it are dark).
            SkyLightVolume::new([0.74, 0.47, 0.88])
                .with_sky_light(0.45)
                .open(BoxFace::PosX)
                .open(BoxFace::NegX)
                .with_fade(0.8),
            ChildOf(body),
        ));
        let rim = StandardMaterial::color(1.0, 1.0, 1.0)
            .with_metallic(1.0)
            .with_roughness(0.24);
        let caliper = StandardMaterial::hex(0xc8102e).with_roughness(0.3);
        let look = WheelLook::new(world, p, rim, caliper);
        spawn_wheels(
            world,
            body,
            p,
            &wheels,
            look,
            sports::GROUND,
            SPORTS_WHEEL.width,
        );
    });
}

/// Draw the rally car (livery, dust, cage and all) on `body`, a vehicle entity with these
/// `wheels`.
pub fn rally_car_visuals(commands: &mut Commands, body: Entity, wheels: &Wheels) {
    // The single big tailpipe, for the effects modules.
    let tips = rally_kit::exhaust_exits(&rally::Body::new());
    commands
        .entity(body)
        .insert(nozzles(ExhaustKind::Tailpipe, &tips));
    let wheels = wheels.clone();
    commands.queue(move |world: &mut World| {
        let p = rally_kit::PREFIX;
        let k = |name: &str| key(p, name);
        ensure_kit(world, &k("paint"), rally_car_kit);
        let f = Finishes::new(world, p);
        let tex = |name: &str| image(world, &k(name));
        // The livery atlas carries color, metallic and roughness.
        let mut livery = StandardMaterial::color(1.0, 1.0, 1.0)
            .with_metallic(1.0)
            .with_roughness(1.0);
        livery.reflectance = 0.55;
        match (tex("atlas_color"), tex("atlas_mr")) {
            (Some(c), Some(mr)) => {
                livery = livery
                    .with_base_color_texture(c)
                    .with_metallic_roughness_texture(mr);
            }
            _ => {
                livery = StandardMaterial::hex(0x1747a6)
                    .with_metallic(0.5)
                    .with_roughness(0.22);
            }
        }
        if let Some(h) = tex("atlas_normal") {
            livery = livery.with_normal_texture(h);
        }
        let textured = |name: &str, m: StandardMaterial| match tex(name) {
            Some(h) => m.with_base_color_texture(h),
            None => m,
        };
        let banner = textured(
            "banner_tex",
            StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.4),
        );
        let plate = textured(
            "plate_tex",
            StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.3),
        );
        let number = textured(
            "number_tex",
            StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.35),
        );
        let flaps = textured(
            "flap_tex",
            StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.78),
        );
        let mut skid = textured(
            "brushed_color",
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_metallic(1.0)
                .with_roughness(0.34)
                .with_uv_scale(1.0, 1.0),
        );
        if let Some(h) = tex("brushed_normal") {
            skid = skid.with_normal_texture(h);
        }
        let fabric = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.85);
        let glass = glass_material(tex("frit"));
        let at0 = Transform::default();
        for (name, mat) in [
            ("paint", livery),
            ("mirror_paint", livery),
            (
                "liner",
                StandardMaterial::hex(0x2e2924).with_roughness(0.95),
            ),
            ("lamp_housing", f.housing),
            ("recess", f.recess),
            ("glass_front", glass),
            ("glass_back", glass),
            ("glass_left", glass),
            ("glass_right", glass),
            ("banner", banner),
            ("cabin", StandardMaterial::hex(0x8a8d91).with_roughness(0.7)),
            ("head_lens", f.lens),
            ("head_chrome", f.chrome),
            ("head_projector", f.projector),
            ("drl", f.drl),
            ("amber", f.amber),
            ("grille", f.satin_black),
            ("wing", f.carbon),
            ("wing_plate", plate),
            ("number_plate", number),
            ("flaps", flaps),
            ("skid", skid),
            ("exhaust", f.exhaust),
            ("mirror_black", f.satin_black),
            ("mirror_glass", f.mirror_glass),
            ("wipers", f.satin_black),
            ("chrome", f.chrome),
            (
                "enamel",
                StandardMaterial::hex(0x0d2f7a).with_roughness(0.08),
            ),
            ("straps", fabric),
            ("cage", StandardMaterial::hex(0xc4c6c9).with_roughness(0.42)),
            ("dash", StandardMaterial::hex(0x18181a).with_roughness(0.9)),
            ("screens", f.screen),
            ("controls", f.leather),
        ] {
            spawn_part(world, body, &k(name), mat, at0);
        }
        spawn_lamp(world, body, &k("tail_red"), f.tail_off, f.tail_on, false);
        spawn_lamp(
            world,
            body,
            &k("tail_reverse"),
            f.reverse_off,
            f.reverse_on,
            true,
        );
        for x in rally_kit::SEATS {
            let at = Transform::from_xyz(
                x as f64,
                (rally_kit::SEAT_FLOOR - rally::GROUND) as f64,
                rally_kit::SEAT_Z as f64,
            );
            spawn_part(world, body, &k("seat"), fabric, at);
        }
        world.spawn((
            Transform::from_xyz(0.0, 0.38, 0.52),
            SkyLightVolume::new([0.78, 0.57, 1.38])
                .with_sky_light(0.6)
                .open(BoxFace::PosX)
                .open(BoxFace::NegX)
                .with_fade(0.9),
            ChildOf(body),
        ));
        let rim = StandardMaterial::hex(0xefefec).with_roughness(0.38);
        let caliper = StandardMaterial::hex(0xd09a1c).with_roughness(0.35);
        let mut look = WheelLook::new(world, p, rim, caliper);
        look.tire.base_color = StandardMaterial::hex(0x3e3934).base_color;
        look.tire.perceptual_roughness = 0.9;
        spawn_wheels(
            world,
            body,
            p,
            &wheels,
            look,
            rally::GROUND,
            RALLY_WHEEL.width,
        );
    });
}

/// Fit the rally car's hood light pod (four LED lamps, lit) to `body`.
pub fn rally_light_pod(commands: &mut Commands, body: Entity) {
    commands.queue(move |world: &mut World| {
        let p = rally_kit::PREFIX;
        let k = |name: &str| key(p, name);
        ensure_kit(world, &k("paint"), rally_car_kit);
        let f = Finishes::new(world, p);
        // The lamps are lit: a faint glow in the lenses around the bright LEDs.
        let lens = f.lens.with_emissive([1.0, 0.97, 0.9], 30.0);
        for (name, mat) in [
            ("pod_shell", f.satin_black),
            ("pod_chrome", f.chrome),
            ("pod_reflector", f.housing),
            ("pod_lens", lens),
            ("pod_led", f.drl),
        ] {
            spawn_part(world, body, &k(name), mat, Transform::default());
        }
    });
}

/// Tinted, glossy, blended car glass; `frit` adds the black edge band.
fn glass_material(frit: Option<Handle<Image>>) -> StandardMaterial {
    let mut m = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.03);
    m.alpha_mode = AlphaMode::Blend;
    m.reflectance = 0.6;
    m.double_sided = true;
    m.receives_wetness = false;
    match frit {
        Some(h) => m.with_base_color_texture(h),
        None => {
            m.base_color = [0.008, 0.012, 0.014, 0.42];
            m
        }
    }
}

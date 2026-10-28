//! A mounted rifle on a tripod: walk up to its handles and press the use key ("E  Fire"). It
//! swings to where you look, within its arc, and fires a burst through whatever is there:
//! walls, rock, crates, building pieces. The shots are `ProjectileTrace`s, the same bullets
//! the demo's scripted rifle bursts fire, so everything that reacts to gunfire reacts to it.
//!
//! Like the fixed emplacements of Battlefield or Squad, but without taking over the camera:
//! you aim by looking, and the gun traverses toward it at [`MountedGun::slew_rad_s`] before
//! it fires. While a burst is on, its [`Interactable`] is off (no prompt).

use engine_core::math::{DVec3, EulerRot, Quat, Rng, Vec3};
use engine_core::prelude::*;
use engine_render::{Camera, MeshRenderer, Shape, StandardMaterial};
use engine_world::physics::{Collider, ColliderShape, PhysicsQueries, QueryFilter};
use engine_world::{Interact, Interactable, NoiseEvent, ProjectileTrace};

/// Height of the gun's pivot over the ground, m.
pub const PIVOT_M: f32 = 1.05;
/// How far out the gun aims along the player's view when nothing is in it (or there is no
/// physics module to ask), m.
const CONVERGE_M: f32 = 25.0;
/// The muzzle, in the swivel's frame (forward is −Z), m.
const MUZZLE: Vec3 = Vec3::new(0.0, 0.04, -1.15);

/// A rifle on a tripod that the player fires with the use key (see the module docs). Put it
/// on the tripod's entity with [`spawn_mounted_gun`]; its forward is local −Z.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MountedGun {
    /// Rounds per press.
    pub burst: u32,
    pub rounds_per_sec: f32,
    /// How far it traverses either side of its forward, degrees.
    pub arc_deg: f32,
    /// Lowest and highest elevation, degrees.
    pub pitch_deg: [f32; 2],
    /// How far its bullets carry, m.
    pub range_m: f32,
    /// Muzzle speed, m/s.
    pub speed: f32,
    /// Random spread of each round, radians.
    pub spread: f32,
    /// Traverse speed, radians per second.
    pub slew_rad_s: f32,
    /// Where it points now, radians off its forward: (yaw, pitch).
    pub aim: [f32; 2],
    #[serde(skip)]
    target: [f32; 2],
    #[serde(skip)]
    rounds: u32,
    #[serde(skip)]
    wait: f32,
    /// The part that turns (barrel, receiver, handles), a child of the tripod.
    #[serde(skip)]
    pub swivel: Option<Entity>,
}

impl Default for MountedGun {
    fn default() -> Self {
        Self {
            burst: 6,
            rounds_per_sec: 9.0,
            arc_deg: 85.0,
            pitch_deg: [-15.0, 30.0],
            range_m: 150.0,
            speed: 850.0,
            spread: 0.006,
            slew_rad_s: 3.0,
            aim: [0.0, 0.0],
            target: [0.0, 0.0],
            rounds: 0,
            wait: 0.0,
            swivel: None,
        }
    }
}

impl MountedGun {
    /// Rounds still to fire in this burst.
    pub fn firing(&self) -> u32 {
        self.rounds
    }
}

/// The turning part of a [`MountedGun`].
#[derive(Component, Debug, Clone, Copy)]
pub struct GunSwivel;

fn part(
    commands: &mut Commands,
    parent: Entity,
    shape: Shape,
    material: &StandardMaterial,
    at: Vec3,
    rotation: Quat,
) {
    commands.spawn((
        Transform::from_translation(at.as_dvec3()).with_rotation(rotation),
        MeshRenderer::new(shape, *material),
        ChildOf(parent),
    ));
}

fn rod(radius: f32, length: f32) -> Shape {
    Shape::Cylinder {
        radius,
        height: length,
        segments: 10,
    }
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

/// A tripod with a rifle on it at `at` (ground), its forward turned `yaw` radians about +Y
/// from −Z. Returns the tripod's entity.
pub fn spawn_mounted_gun(commands: &mut Commands, at: DVec3, yaw: f32) -> Entity {
    let steel = StandardMaterial::hex(0x2a2c2e)
        .with_roughness(0.45)
        .with_metallic(0.8);
    let worn = StandardMaterial::hex(0x4a4d50)
        .with_roughness(0.6)
        .with_metallic(0.6);
    let wood = StandardMaterial::hex(0x5c3d22).with_roughness(0.7);
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    let base = commands
        .spawn((
            Name::new("Mounted rifle"),
            t,
            GlobalTransform::from(t),
            // Used from behind, at the handles.
            Interactable::new("Fire")
                .reach(2.5)
                .at(Vec3::new(0.0, PIVOT_M, 0.55)),
            Collider {
                shape: ColliderShape::Cuboid {
                    half_extents: Vec3::new(0.45, PIVOT_M * 0.5, 0.45),
                },
                ..Collider::default()
            },
        ))
        .id();
    // Three splayed legs and a post up to the pivot.
    for k in 0..3 {
        let a = k as f32 * std::f32::consts::TAU / 3.0 + 0.5;
        let out = Vec3::new(a.sin(), 0.0, a.cos());
        let foot = out * 0.55;
        let top = Vec3::Y * (PIVOT_M - 0.25);
        let leg = top - foot;
        part(
            commands,
            base,
            rod(0.022, leg.length()),
            &worn,
            (foot + top) * 0.5,
            Quat::from_rotation_arc(Vec3::Y, leg.normalize()),
        );
    }
    part(
        commands,
        base,
        rod(0.04, 0.3),
        &steel,
        Vec3::Y * (PIVOT_M - 0.15),
        Quat::IDENTITY,
    );
    let swivel = commands
        .spawn((
            Name::new("Mounted rifle swivel"),
            Transform::from_xyz(0.0, f64::from(PIVOT_M), 0.0),
            GunSwivel,
            ChildOf(base),
        ))
        .id();
    let along = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    // Receiver, barrel with a jacket and a muzzle brake, a box magazine, the stock and the
    // spade grips behind.
    part(
        commands,
        swivel,
        cuboid(0.12, 0.13, 0.5),
        &steel,
        Vec3::new(0.0, 0.04, -0.05),
        Quat::IDENTITY,
    );
    part(
        commands,
        swivel,
        rod(0.045, 0.45),
        &worn,
        Vec3::new(0.0, 0.04, -0.5),
        along,
    );
    part(
        commands,
        swivel,
        rod(0.016, 0.85),
        &steel,
        Vec3::new(0.0, 0.04, -0.7),
        along,
    );
    part(
        commands,
        swivel,
        rod(0.026, 0.09),
        &steel,
        MUZZLE + Vec3::Z * 0.04,
        along,
    );
    part(
        commands,
        swivel,
        cuboid(0.06, 0.22, 0.09),
        &steel,
        Vec3::new(0.0, -0.11, -0.12),
        Quat::IDENTITY,
    );
    part(
        commands,
        swivel,
        cuboid(0.08, 0.1, 0.3),
        &wood,
        Vec3::new(0.0, 0.0, 0.33),
        Quat::IDENTITY,
    );
    part(
        commands,
        swivel,
        cuboid(0.03, 0.03, 0.12),
        &steel,
        Vec3::new(0.0, 0.12, -0.25),
        Quat::IDENTITY,
    );
    for side in [-1.0f32, 1.0] {
        part(
            commands,
            swivel,
            rod(0.018, 0.14),
            &wood,
            Vec3::new(side * 0.1, -0.03, 0.5),
            Quat::IDENTITY,
        );
    }
    part(
        commands,
        swivel,
        rod(0.012, 0.22),
        &steel,
        Vec3::new(0.0, 0.04, 0.5),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
    );
    let gun = MountedGun {
        swivel: Some(swivel),
        ..MountedGun::default()
    };
    commands.entity(base).insert(gun);
    base
}

/// The use key aims a [`MountedGun`] where the player looks and starts a burst; the gun
/// traverses, fires its rounds and offers itself again (Update).
#[allow(clippy::type_complexity)]
pub fn mounted_gun_system(
    time: Res<Time>,
    mut used: MessageReader<Interact>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    queries: Option<Res<PhysicsQueries>>,
    mut guns: Query<(Entity, &mut MountedGun, &GlobalTransform, &mut Interactable)>,
    mut swivels: Query<(&mut Transform, &GlobalTransform), With<GunSwivel>>,
    mut shots: MessageWriter<ProjectileTrace>,
    mut noise: MessageWriter<NoiseEvent>,
    mut rng: Local<Option<Rng>>,
) {
    let view = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, g)| (g.translation(), g.forward()));
    for u in used.read() {
        let Ok((_, mut gun, gt, mut use_it)) = guns.get_mut(u.target) else {
            continue;
        };
        if gun.rounds > 0 {
            continue;
        }
        // At what the player looks at (the first thing along their view, else a point
        // [`CONVERGE_M`] out), from the pivot, in the tripod's frame, within the gun's arc.
        if let Some((eye, look)) = view {
            let hit = queries.as_deref().and_then(|q| {
                let past = QueryFilter {
                    exclude: vec![u.target, u.actor],
                    ..QueryFilter::default()
                };
                q.0.raycast(eye, look, gun.range_m, &past)
            });
            let point = eye + (look * hit.map_or(CONVERGE_M, |h| h.distance)).as_dvec3();
            let pivot = gt.transform_point(DVec3::new(0.0, f64::from(PIVOT_M), 0.0));
            let d = (gt.rotation().inverse() * (point - pivot).as_vec3()).normalize_or(Vec3::NEG_Z);
            let arc = gun.arc_deg.to_radians();
            let [lo, hi] = gun.pitch_deg.map(f32::to_radians);
            gun.target = [
                (-d.x).atan2(-d.z).clamp(-arc, arc),
                d.y.clamp(-1.0, 1.0).asin().clamp(lo, hi),
            ];
        }
        gun.rounds = gun.burst.max(1);
        gun.wait = 0.0;
        use_it.enabled = false;
    }
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let rng = rng.get_or_insert_with(|| Rng::new(0x6A11_F1E5));
    for (e, mut gun, _, mut use_it) in &mut guns {
        if gun.rounds == 0 && gun.aim == gun.target {
            continue;
        }
        let Some((mut st, sg)) = gun.swivel.and_then(|s| swivels.get_mut(s).ok()) else {
            continue;
        };
        // Traverse toward the target.
        let step = gun.slew_rad_s * dt;
        let (aim, target) = (gun.aim, gun.target);
        let aim = [0, 1].map(|k| aim[k] + (target[k] - aim[k]).clamp(-step, step));
        if aim != gun.aim {
            gun.aim = aim;
            st.rotation = Quat::from_euler(EulerRot::YXZ, aim[0], aim[1], 0.0);
        }
        if gun.rounds == 0 {
            continue;
        }
        let on = (aim[0] - target[0]).abs() < 0.01 && (aim[1] - target[1]).abs() < 0.01;
        gun.wait -= dt;
        if !on || gun.wait > 0.0 {
            continue;
        }
        // The swivel's world frame is last frame's; this frame's turn is small next to it.
        let rotation = sg.rotation();
        let muzzle = sg.transform_point(MUZZLE.as_dvec3());
        if gun.rounds == gun.burst.max(1) {
            noise.write(NoiseEvent {
                position: muzzle,
                radius: 90.0,
                source: Some(e),
            });
        }
        let spread = gun.spread;
        let jitter = Quat::from_euler(
            EulerRot::YXZ,
            rng.range(-spread, spread),
            rng.range(-spread, spread),
            0.0,
        );
        let dir = rotation * jitter * Vec3::NEG_Z;
        shots.write(ProjectileTrace {
            from: muzzle,
            to: muzzle + (dir * gun.range_m).as_dvec3(),
            radius: 0.01,
            speed: gun.speed,
            shooter: Some(e),
        });
        gun.rounds -= 1;
        gun.wait += 1.0 / gun.rounds_per_sec.max(0.1);
        if gun.rounds == 0 {
            use_it.enabled = true;
        }
    }
}

//! Plank doors: every standing doorway piece placed in build mode ([`BuildKind::WoodDoorway`])
//! gets a [`Door`] hung in its opening, a ledged and braced plank door
//! ([`PropKind::PlankDoor`]) hinged on the opening's left (seen from the front, +Z). It swings
//! [`OPEN_DEG`] either way, away from whoever opens it, like a door in Valheim.
//!
//! Each door is an [`Interactable`] ("Open" / "Shut", within [`REACH_M`]): the use key (E,
//! gamepad West) opens or shuts the one the engine picks; scripts send [`DoorRequest`]s and AI agents
//! call `destruction.door`. With a physics module a door is a kinematic body that swings with
//! it: shut, it closes the doorway; open, it stands where it swung to; swinging, it shoves
//! crates and bodies in its way. When its doorway falls, the door swings shut and falls with
//! it; when the doorway is taken down or broken up, the door goes too.

use engine_core::math::{DVec3, Quat, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_input::{Binding, GamepadButton, InputMap, KeyCode};
use engine_world::physics::{Collider, ColliderShape, PhysicsBackend, RigidBody};
use engine_world::{Interact, Interactable, NoiseEvent, SurfaceMaterials};

use crate::build::{BuildKind, BuiltPiece};
use crate::dress::PropModel;
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::props::{PropKind, wall_opening};
use crate::structure::StructuralPiece;

/// How far a door swings open, degrees.
pub const OPEN_DEG: f32 = 95.0;
/// How near (m, to the door's middle) a character must be to open or shut it.
pub const REACH_M: f32 = 2.2;
/// Thickness of a door leaf, m.
pub const LEAF_T: f32 = 0.05;
/// Gap between a door and its lining, m.
const GAP_M: f32 = 0.005;
/// Height of a doorway's threshold board over the foot of the wall, m.
const THRESHOLD_M: f32 = 0.02;

/// A door hung in a doorway piece (a child of it), swinging about a vertical hinge on the
/// opening's left in the middle of the wall's thickness.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Door {
    /// The hinge in its doorway's space, m (see [`leaf`]).
    pub hinge: Vec3,
    /// How far open: 0 shut .. 1 open ([`OPEN_DEG`]).
    pub open: f32,
    /// Opening (true) or shutting (false).
    pub opening: bool,
    /// Which way it opens: +1 out through the front (+Z of its doorway), −1 in through the
    /// back.
    pub toward: f32,
    /// Seconds a full swing takes.
    pub swing_secs: f32,
    /// Its doorway fell: it swings shut and falls with it, and can't be opened.
    pub loose: bool,
}

impl Default for Door {
    fn default() -> Self {
        Self {
            hinge: Vec3::ZERO,
            open: 0.0,
            opening: false,
            toward: -1.0,
            swing_secs: 0.9,
            loose: false,
        }
    }
}

impl Door {
    /// The swing angle now, radians (eased at both ends).
    pub fn angle(&self) -> f32 {
        OPEN_DEG.to_radians() * smoothstep(0.0, 1.0, self.open)
    }

    /// Its transform in its doorway's space for a leaf `size` big.
    pub fn transform(&self, size: Vec3) -> Transform {
        leaf_transform(self.hinge, size, self.angle(), self.toward)
    }

    /// Fully open.
    pub fn is_open(&self) -> bool {
        self.open >= 1.0
    }
}

/// The door hung in this doorway piece.
#[derive(Component, Debug, Clone, Copy)]
pub struct HungDoor(pub Entity);

/// Open, shut or toggle a door.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DoorRequest {
    pub door: Entity,
    /// `Some(true)` opens it, `Some(false)` shuts it, `None` toggles it.
    pub open: Option<bool>,
    /// Who opens it (world position): it swings away from them. `None`: in through the back.
    pub from: Option<DVec3>,
}

/// A doorway's door leaf: (hinge in the doorway's own space, leaf size), for a doorway piece
/// `size` big. The leaf fills the clear opening less a gap all round, standing on the
/// threshold; the hinge is at the opening's left (−X) in the middle of the wall.
pub fn leaf(size: Vec3) -> (Vec3, Vec3) {
    let (lo, hi) = wall_opening(size, true);
    let bottom = lo.y + THRESHOLD_M + GAP_M;
    let top = hi.y - GAP_M;
    let x0 = lo.x + GAP_M;
    let x1 = hi.x - GAP_M;
    (
        Vec3::new(x0, (bottom + top) * 0.5, 0.0),
        Vec3::new(x1 - x0, top - bottom, LEAF_T.min(size.z * 0.6)),
    )
}

/// A door's transform in its doorway's space, `angle` radians open toward `toward`.
pub fn leaf_transform(hinge: Vec3, size: Vec3, angle: f32, toward: f32) -> Transform {
    // Turning about +Y by a positive angle takes the free edge (+X) toward −Z.
    let rotation = Quat::from_rotation_y(-toward.signum() * angle);
    Transform {
        translation: (hinge + rotation * Vec3::X * size.x * 0.5).as_dvec3(),
        rotation,
        scale: Vec3::ONE,
    }
}

/// Bind `interact` (E, gamepad West) unless the game's map has it (`Startup`).
pub fn bind_actions(map: Option<ResMut<InputMap>>) {
    let Some(mut map) = map else {
        return;
    };
    if !map.actions.contains_key("interact") {
        map.bind("interact", Binding::Key(KeyCode::E))
            .bind("interact", Binding::Gamepad(GamepadButton::West));
    }
}

/// Hang a door in every standing doorway piece placed in build mode that has none (Update).
pub fn hang_doors_system(
    mut commands: Commands,
    frames: Query<(Entity, &BuiltPiece, &Transform), (With<StructuralPiece>, Without<HungDoor>)>,
) {
    for (frame, built, t) in &frames {
        if built.kind != BuildKind::WoodDoorway {
            continue;
        }
        let (hinge, size) = leaf(built.kind.size());
        let door = Door {
            hinge,
            ..Door::default()
        };
        // A few looks, picked by spot, so neighbors differ but meshes are shared.
        let p = t.translation;
        let seed = ((p.x * 7.0 + p.z * 13.0).abs() as u64) % 4;
        let id = commands
            .spawn((
                Name::new("Plank door"),
                door.transform(size),
                door,
                Interactable::new("Open").reach(REACH_M),
                PropModel::new(
                    PropKind::PlankDoor {
                        size: size.to_array(),
                    },
                    seed,
                ),
                ChildOf(frame),
            ))
            .id();
        commands.entity(frame).insert(HungDoor(id));
    }
}

/// The use key opens or shuts the door it was pressed at (Update).
pub fn door_input_system(
    mut used: MessageReader<Interact>,
    doors: Query<(), With<Door>>,
    mut out: MessageWriter<DoorRequest>,
) {
    for u in used.read() {
        if doors.contains(u.target) {
            out.write(DoorRequest {
                door: u.target,
                open: None,
                from: Some(u.from),
            });
        }
    }
}

/// Apply [`DoorRequest`]s, swing doors, make them kinematic bodies with a physics module, and
/// let doors whose doorway fell go with it (Update).
///
/// A door is moved by its `Transform`, so the backend carries it there over each step and it
/// pushes what it swings into.
pub fn door_system(
    mut commands: Commands,
    time: Res<Time>,
    backend: Option<Res<PhysicsBackend>>,
    table: Res<DestructionMaterials>,
    surfaces: Res<SurfaceMaterials>,
    mut requests: MessageReader<DoorRequest>,
    mut noise: MessageWriter<NoiseEvent>,
    mut doors: Query<(
        Entity,
        &mut Door,
        &mut Transform,
        &ChildOf,
        &PropModel,
        Has<Collider>,
        Option<&mut Interactable>,
    )>,
    frames: Query<(&Transform, Has<StructuralPiece>), Without<Door>>,
) {
    for r in requests.read() {
        let Ok((_, mut door, _, parent, _, _, _)) = doors.get_mut(r.door) else {
            continue;
        };
        if door.loose {
            continue;
        }
        let open = r.open.unwrap_or(!door.opening);
        if open == door.opening {
            continue;
        }
        let Ok((frame, _)) = frames.get(parent.parent()) else {
            continue;
        };
        // Starting from shut, it swings away from whoever opens it.
        if open
            && door.open <= 0.0
            && let Some(from) = r.from
        {
            let local = frame.rotation.inverse() * (from - frame.translation).as_vec3();
            door.toward = if local.z > 0.0 { -1.0 } else { 1.0 };
        }
        door.opening = open;
        noise.write(NoiseEvent {
            position: frame.translation,
            radius: 12.0,
            source: Some(r.door),
        });
    }
    let dt = time.delta_secs();
    let wood = table.get(DestructibleMaterial::Wood);
    for (e, mut door, mut t, parent, model, has_collider, use_it) in &mut doors {
        let standing = frames.get(parent.parent()).is_ok_and(|(_, s)| s);
        if !standing && !door.loose {
            // The doorway fell: the door swings shut and rides it down.
            door.loose = true;
            door.opening = false;
            commands.entity(e).remove::<(RigidBody, Collider)>();
        }
        // The prompt says what the key does next; a fallen door can't be used.
        if let Some(mut use_it) = use_it {
            let verb = if door.opening { "Shut" } else { "Open" };
            if use_it.verb != verb || use_it.enabled == door.loose {
                use_it.verb = verb.into();
                use_it.enabled = !door.loose;
            }
        }
        let PropKind::PlankDoor { size } = model.kind else {
            continue;
        };
        let size = Vec3::from(size);
        let target = if door.opening { 1.0 } else { 0.0 };
        let rate = dt / door.swing_secs.max(0.05) * if door.loose { 3.0 } else { 1.0 };
        let open = door.open + (target - door.open).clamp(-rate, rate);
        if open != door.open {
            door.open = open;
        }
        let swung = door.transform(size);
        if *t != swung {
            *t = swung;
        }
        if backend.is_some() && !door.loose && !has_collider {
            commands.entity(e).insert((
                RigidBody::kinematic(),
                Collider {
                    shape: ColliderShape::Cuboid {
                        half_extents: size * 0.5 - Vec3::splat(0.002),
                    },
                    surface: surfaces.id_or_default(&wood.surface),
                    density: wood.density,
                    ..Collider::default()
                },
            ));
        }
    }
}

/// The door of a doorway piece, if one hangs in it.
pub fn door_of(world: &World, frame: Entity) -> Option<Entity> {
    world.get::<HungDoor>(frame).map(|h| h.0)
}

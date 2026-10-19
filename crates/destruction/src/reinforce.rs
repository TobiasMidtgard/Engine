//! Siege-style reinforcement: a defender sets steel panels against the back of a wall (a soft
//! drywall, a barricaded room's side). Each panel is its own steel [`DestructibleWall`] on a
//! bolted frame with two hydraulic struts kicked down to the floor behind it: bullets that get
//! through the wall ring off it, a breach charge strips the wall in front and only dents it,
//! and a thermite charge on the wall burns through the panel behind too. A wall wider than
//! [`PANEL_MAX_W`] takes several panels side by side.
//!
//! Send a [`ReinforceRequest`] (AI agents call `destruction.reinforce`): the panels are carried
//! in from behind, pressed home and braced over [`DEPLOY_SECS`].

use engine_core::math::{DVec3, Quat, Vec2, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_world::{Ground, NoiseEvent};

use crate::dress::{GRAVEL_TOP, GravelPad, PropModel};
use crate::material::DestructibleMaterial;
use crate::props::PropKind;
use crate::voxel::{VoxelState, rock_below};
use crate::wall::{DestructibleWall, WallAnchors};

/// Widest single panel, m: a wider wall takes several side by side.
pub const PANEL_MAX_W: f32 = 2.8;
/// Steel plate thickness, m.
pub const PANEL_T: f32 = 0.04;
/// Depth of the frame bolted to a panel's back, m.
pub const FRAME_D: f32 = 0.08;
/// Gap between the wall's back and a panel, m.
const GAP_M: f32 = 0.01;
/// How far behind its place a panel starts (carried in, then pressed home), m.
const CARRY_M: f32 = 0.7;
/// Height of the struts' pivots on a panel's frame, and how far behind it their feet land, m
/// (on the floor behind: the ground, rock or a gravel pad, which may sit above the wall's foot).
pub const STRUT_PIVOT_Y: f32 = 1.35;
pub const STRUT_REACH: f32 = 1.0;
/// Height of a strut foot pad's middle over the floor, m (half its sole's thickness).
const STRUT_FOOT_Y: f32 = 0.013;
/// Seconds to set a panel: pressed home, then its struts kicked down.
pub const DEPLOY_SECS: f32 = 1.8;
/// Share of the deploy spent pressing the panel home (the rest swings the struts down).
const PRESS: f32 = 0.55;

/// On a wall: the reinforcement panels set behind it.
#[derive(Component, Debug, Clone, PartialEq, Default)]
pub struct ReinforcedBy(pub Vec<Entity>);

/// A reinforcement panel (a steel [`DestructibleWall`] of its own) behind `wall`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Reinforcement {
    pub wall: Entity,
    /// Its middle along the wall's local X, m.
    pub offset: f32,
    /// 0 carried in .. 1 set.
    pub deployed: f32,
}

impl Reinforcement {
    /// Fully pressed home and braced.
    pub fn is_set(&self) -> bool {
        self.deployed >= 1.0
    }
}

/// One of a panel's two struts (a child of it), hinged at the frame and swung down from up its
/// back to the floor behind.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Strut {
    /// Its pivot in the panel's space, m.
    pub pivot: Vec3,
    pub length: f32,
    /// Its swing stowed (radians about the panel's X: π points straight up its back, less
    /// tilts it back to clear a low panel's top).
    pub stowed: f32,
    /// Its swing braced, its foot on the floor [`STRUT_REACH`] behind.
    pub braced: f32,
}

/// Reinforce a wall: panels go up behind it (its back, local −Z). Ignored if it already has
/// them.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ReinforceRequest {
    pub wall: Entity,
}

/// The steel plate of a reinforcement panel `width` × `height`.
pub fn panel_wall(width: f32, height: f32, seed: u64) -> DestructibleWall {
    DestructibleWall {
        thickness: PANEL_T,
        cell_size: 0.05,
        material: DestructibleMaterial::Metal,
        anchors: WallAnchors::ALL,
        stud_spacing: 0.0,
        paint: Some([0.13, 0.15, 0.17]),
        seed,
        ..DestructibleWall::soft(width, height)
    }
}

/// Panel middles along a wall `width` wide, and the panels' width.
pub fn panel_layout(width: f32) -> (Vec<f32>, f32) {
    let n = (width / PANEL_MAX_W).ceil().max(1.0) as usize;
    let pw = width / n as f32;
    let offsets = (0..n)
        .map(|k| -width * 0.5 + pw * (k as f32 + 0.5))
        .collect();
    (offsets, pw)
}

/// Strut length and its braced swing (radians about the panel's X; π is stowed up its back)
/// for a pivot `drop` m over the floor.
fn strut_geometry(drop: f32) -> (f32, f32) {
    let drop = (drop - STRUT_FOOT_Y).clamp(0.5, 2.5);
    let length = (drop * drop + STRUT_REACH * STRUT_REACH).sqrt();
    (length, STRUT_REACH.atan2(drop))
}

/// Height of the floor under `p`: the ground, voxel rock or a gravel pad, whichever is highest
/// at or below `p`.
fn floor_at(
    ground: Option<&Ground>,
    rock: &[(&GlobalTransform, &VoxelState)],
    pads: &[(&GravelPad, &GlobalTransform)],
    p: DVec3,
) -> Option<f64> {
    let on_pad = pads.iter().filter_map(|(pad, gt)| {
        let local = gt.affine().inverse().transform_point3(p);
        let inside =
            local.x.abs() <= pad.size[0] as f64 * 0.5 && local.z.abs() <= pad.size[1] as f64 * 0.5;
        inside.then(|| gt.translation().y + GRAVEL_TOP as f64)
    });
    ground
        .and_then(|g| g.height(p))
        .into_iter()
        .chain(rock_below(rock, p))
        .chain(on_pad)
        .filter(|h| *h <= p.y + 0.05)
        .reduce(f64::max)
}

/// A strut's transform in its panel's space, `swing` 0 stowed .. 1 braced.
pub fn strut_transform(strut: &Strut, swing: f32) -> Transform {
    let angle = strut.stowed + (strut.braced - strut.stowed) * swing;
    let rotation = Quat::from_rotation_x(angle);
    // The strut's mesh is centered with its pivot at the top (+Y).
    let center = strut.pivot - rotation * Vec3::Y * strut.length * 0.5;
    Transform {
        translation: center.as_dvec3(),
        rotation,
        scale: Vec3::ONE,
    }
}

/// A panel's transform relative to its wall (wall-local), `press` 0 carried .. 1 home.
fn panel_local(wall: &DestructibleWall, offset: f32, press: f32) -> Transform {
    let z = -(wall.thickness * 0.5 + GAP_M + PANEL_T * 0.5) - (1.0 - press) * CARRY_M;
    Transform::from_translation(DVec3::new(offset as f64, 0.0, z as f64))
}

fn press_of(deployed: f32) -> f32 {
    smoothstep(0.0, PRESS, deployed)
}

fn swing_of(deployed: f32) -> f32 {
    smoothstep(PRESS, 1.0, deployed)
}

fn wall_affine(t: &Transform, gt: Option<&GlobalTransform>, child: bool) -> GlobalTransform {
    match gt {
        Some(g) if child => *g,
        _ => GlobalTransform::from(*t),
    }
}

/// Put up panels for [`ReinforceRequest`]s, carry them home and brace them, and take them
/// away when their wall goes (FixedUpdate, before charges).
#[allow(clippy::type_complexity)]
pub fn reinforce_system(
    mut commands: Commands,
    time: Res<Time>,
    mut requests: MessageReader<ReinforceRequest>,
    mut noise: MessageWriter<NoiseEvent>,
    walls: Query<
        (
            &DestructibleWall,
            &Transform,
            Option<&GlobalTransform>,
            Has<ChildOf>,
            Option<&ReinforcedBy>,
        ),
        Without<Reinforcement>,
    >,
    mut panels: Query<(Entity, &mut Reinforcement, &mut Transform)>,
    mut struts: Query<
        (&Strut, &ChildOf, &mut Transform),
        (Without<Reinforcement>, Without<DestructibleWall>),
    >,
    ground: Option<Res<Ground>>,
    volumes: Query<(&GlobalTransform, &VoxelState)>,
    pads: Query<(&GravelPad, &GlobalTransform)>,
) {
    let rock: Vec<_> = volumes.iter().collect();
    let pads: Vec<_> = pads.iter().collect();
    let mut done = Vec::new();
    for r in requests.read() {
        let Ok((wall, t, gt, child, by)) = walls.get(r.wall) else {
            continue;
        };
        // Two requests for one wall in a step put up one set.
        if by.is_some_and(|b| !b.0.is_empty()) || wall.size[1] < 0.5 || done.contains(&r.wall) {
            continue;
        }
        done.push(r.wall);
        let affine = wall_affine(t, gt, child);
        let (offsets, pw) = panel_layout(wall.size[0]);
        let height = wall.size[1];
        let mut ids = Vec::new();
        for (k, &offset) in offsets.iter().enumerate() {
            let local = panel_local(wall, offset, 0.0);
            let world = affine.mul_transform(&local).to_transform();
            let seed = wall.seed.wrapping_mul(31).wrapping_add(k as u64 + 7);
            let panel = commands
                .spawn((
                    Name::new("Reinforcement panel"),
                    world,
                    GlobalTransform::from(world),
                    panel_wall(pw, height, seed),
                    Reinforcement {
                        wall: r.wall,
                        offset,
                        deployed: 0.0,
                    },
                ))
                .id();
            // The frame on its back, and a strut on each side member.
            let frame = PropKind::ReinforcementFrame { size: [pw, height] };
            commands.spawn((
                Transform::from_xyz(0.0, height as f64 * 0.5, -(PANEL_T + FRAME_D) as f64 * 0.5),
                PropModel::new(frame, seed),
                ChildOf(panel),
            ));
            let home = affine.mul_transform(&panel_local(wall, offset, 1.0));
            for side in [-1.0f32, 1.0] {
                let pivot = Vec3::new(
                    side * (pw * 0.5 - crate::props::FRAME_STRUT_INSET),
                    STRUT_PIVOT_Y,
                    -PANEL_T * 0.5 - FRAME_D * 0.5,
                );
                // Each foot finds the floor where it lands.
                let at = home.transform_point((pivot - Vec3::Z * STRUT_REACH).as_dvec3());
                let drop = floor_at(ground.as_deref(), &rock, &pads, at)
                    .map_or(STRUT_PIVOT_Y, |f| (at.y - f) as f32);
                let (length, braced) = strut_geometry(drop);
                // Stowed up the back, tilted out if the panel is too low for them.
                let room = height - crate::props::FRAME_W - 0.02 - STRUT_PIVOT_Y;
                let stowed = std::f32::consts::PI - (room / length).clamp(-1.0, 1.0).acos();
                let strut = Strut {
                    pivot,
                    length,
                    stowed,
                    braced,
                };
                commands.spawn((
                    strut_transform(&strut, 0.0),
                    strut,
                    PropModel::new(
                        PropKind::HydraulicStrut {
                            length,
                            foot_tilt: braced,
                        },
                        seed + 1,
                    ),
                    ChildOf(panel),
                ));
            }
            ids.push(panel);
        }
        commands.entity(r.wall).insert(ReinforcedBy(ids));
        noise.write(NoiseEvent {
            position: affine.translation(),
            radius: 15.0,
            source: Some(r.wall),
        });
    }
    let dt = time.delta_secs();
    for (e, mut panel, mut t) in &mut panels {
        let Ok((wall, wt, gt, child, _)) = walls.get(panel.wall) else {
            // Its wall is gone: so is the panel.
            commands.entity(e).despawn();
            continue;
        };
        if panel.is_set() {
            continue;
        }
        let before = panel.deployed;
        panel.deployed = (panel.deployed + dt / DEPLOY_SECS).min(1.0);
        let affine = wall_affine(wt, gt, child);
        let local = panel_local(wall, panel.offset, press_of(panel.deployed));
        *t = affine.mul_transform(&local).to_transform();
        // A clang as it hits the wall, a thump as the struts bite.
        for at in [PRESS, 1.0] {
            if before < at && panel.deployed >= at {
                noise.write(NoiseEvent {
                    position: t.translation,
                    radius: 18.0,
                    source: Some(e),
                });
            }
        }
    }
    for (strut, parent, mut t) in &mut struts {
        let Ok((_, panel, _)) = panels.get(parent.parent()) else {
            continue;
        };
        let want = strut_transform(strut, swing_of(panel.deployed));
        if *t != want {
            *t = want;
        }
    }
}

/// Panels behind `wall` and their middles along its X (for thermite burning through it).
pub fn panels_behind(world: &World, wall: Entity) -> Vec<(Entity, f32)> {
    world
        .get::<ReinforcedBy>(wall)
        .map(|b| {
            b.0.iter()
                .filter_map(|&p| world.get::<Reinforcement>(p).map(|r| (p, r.offset)))
                .collect()
        })
        .unwrap_or_default()
}

/// Take a wall's panels down (a rebuild, a script reset).
pub fn remove_reinforcement(world: &mut World, wall: Entity) {
    let Some(by) = world.get::<ReinforcedBy>(wall).cloned() else {
        return;
    };
    for p in by.0 {
        if let Ok(e) = world.get_entity_mut(p) {
            e.despawn();
        }
    }
    world.entity_mut(wall).remove::<ReinforcedBy>();
}

/// The wall-local rectangle `min`..`max` in a panel's own space (its middle at `offset`).
pub fn to_panel(min: Vec2, max: Vec2, offset: f32) -> (Vec2, Vec2) {
    let d = Vec2::new(offset, 0.0);
    (min - d, max - d)
}

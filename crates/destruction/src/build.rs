//! Valheim-style build mode: pick a piece, aim, and its ghost snaps to the pieces around it,
//! tinted with the support it would get (blue on the ground, then green through red) or a
//! deep red where it can't go; place it, or take built pieces down again.
//!
//! A [`Builder`] (on the player's camera, or anything else that aims) aims from its own
//! transform (−Z forward) or along a scripted `aim` ray. Every frame the module casts that ray
//! against building pieces, voxel rock and the ground and sets the ghost on what it hit: on
//! top of the ground or a floor, against the side of a wall or the edge of a floor. Then it
//! snaps: every piece has snap points (the ends and middle of poles and beams; the corners and
//! edge midpoints of walls, floors and roofs, on the top face of the ones that lie flat; the
//! corners, edge midpoints and face centers of blocks), and the ghost moves so that the
//! closest pair of its own and a neighbor's snap points meet, skipping spots where it would
//! sit inside another piece. Its support is predicted the way the structure solver will
//! compute it once placed, so a ghost reaching out too far from anything that holds it up is
//! red and can't be placed.
//!
//! With `use_input`, the builder reads the action map: `build_mode` (B, d-pad left) toggles,
//! `build_place` (left mouse, right trigger) places, `build_remove` (X, north button) takes
//! down the built piece aimed at, `build_rotate` (R, d-pad right) turns the ghost 22.5°,
//! `build_next` (C, left bumper) and `build_piece_1` to `build_piece_9` (keys 1 to 9) pick the
//! piece. Scripts send [`BuildRequest`]s; the AI bridge has `destruction.build`.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_input::{ActionState, Binding, GamepadButton, InputMap, KeyCode, MouseButton};
use engine_render::{MaterialRef, MeshRenderer};
use engine_world::{Combustible, Ground, NoiseEvent};

use crate::assets::DestructionAssets;
use crate::burn::{TIMBER_MOISTURE, WOOD_IGNITION_C};
use crate::dress::PropModel;
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::props::PropKind;
use crate::structure::{Obb, PieceState, ShowSupport, StructuralPiece};
use crate::voxel::VoxelState;

/// Pitch of [`BuildKind::WoodRoof`]: one up in two across (26.6°).
pub const ROOF_PITCH: f32 = 0.463_647_6;
/// Plan depth, eave to ridge, that one roof piece covers, m.
pub const ROOF_RUN: f32 = 2.0;
/// Snap points this close (m) pull the ghost onto each other.
pub const SNAP_RADIUS_M: f32 = 1.0;
/// One press of rotate turns the ghost this much (Valheim's 16 steps).
pub const ROTATE_STEP: f32 = std::f32::consts::PI / 8.0;
/// Pieces whose lowest corner is this close above the ground stand on it (as in the
/// structure solver).
const GROUND_TOLERANCE_M: f64 = 0.08;
/// Pieces this close count as touching (as in the structure solver).
const TOUCH_MARGIN_M: f32 = 0.05;
/// Snap candidates tried, closest first, before giving up on a free spot.
const MAX_SNAP_TRIES: usize = 32;
/// How many seeds (looks) each kind of piece cycles through.
const LOOKS: u64 = 8;

/// A piece the build mode can place: Valheim's hammer menu, in wood and stone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildKind {
    /// Plank floor, 2 × 2 m.
    #[default]
    WoodFloor,
    /// Board wall, 2 × 2 m.
    WoodWall,
    /// Upright post, 2 m.
    WoodPole,
    /// Horizontal beam, 2 m.
    WoodBeam,
    /// Shingle roof, 26.6° (1 in 2), 2 m run.
    WoodRoof,
    /// Shingle roof, 45°, 2 m run.
    WoodRoof45,
    /// Stone slab, 2 × 2 m, 0.4 m thick.
    StoneFloor,
    /// Ashlar block wall, 2 × 1 m, 0.8 m thick.
    StoneWall,
    /// Ashlar pillar, 2 m.
    StonePillar,
}

impl BuildKind {
    pub const ALL: [BuildKind; 9] = [
        BuildKind::WoodFloor,
        BuildKind::WoodWall,
        BuildKind::WoodPole,
        BuildKind::WoodBeam,
        BuildKind::WoodRoof,
        BuildKind::WoodRoof45,
        BuildKind::StoneFloor,
        BuildKind::StoneWall,
        BuildKind::StonePillar,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BuildKind::WoodFloor => "wood_floor",
            BuildKind::WoodWall => "wood_wall",
            BuildKind::WoodPole => "wood_pole",
            BuildKind::WoodBeam => "wood_beam",
            BuildKind::WoodRoof => "wood_roof",
            BuildKind::WoodRoof45 => "wood_roof_45",
            BuildKind::StoneFloor => "stone_floor",
            BuildKind::StoneWall => "stone_wall",
            BuildKind::StonePillar => "stone_pillar",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Every name, comma separated (for error messages).
    pub fn names() -> String {
        Self::ALL.map(|k| k.name()).join(", ")
    }

    /// What a player sees in the menu.
    pub fn label(self) -> &'static str {
        match self {
            BuildKind::WoodFloor => "Wood floor 2×2",
            BuildKind::WoodWall => "Wood wall 2×2",
            BuildKind::WoodPole => "Wood pole 2 m",
            BuildKind::WoodBeam => "Wood beam 2 m",
            BuildKind::WoodRoof => "Wood roof 26°",
            BuildKind::WoodRoof45 => "Wood roof 45°",
            BuildKind::StoneFloor => "Stone floor 2×2",
            BuildKind::StoneWall => "Stone wall 2×1",
            BuildKind::StonePillar => "Stone pillar",
        }
    }

    pub fn material(self) -> DestructibleMaterial {
        match self {
            BuildKind::StoneFloor | BuildKind::StoneWall | BuildKind::StonePillar => {
                DestructibleMaterial::Stone
            }
            _ => DestructibleMaterial::Wood,
        }
    }

    /// Size of the piece's box in its own axes, m (a floor is a board wall laid on its back,
    /// a roof slopes down its X to the eave).
    pub fn size(self) -> Vec3 {
        match self {
            BuildKind::WoodFloor | BuildKind::WoodWall => Vec3::new(2.0, 2.0, 0.1),
            BuildKind::WoodPole => Vec3::new(0.2, 2.0, 0.2),
            BuildKind::WoodBeam => Vec3::new(2.0, 0.2, 0.2),
            BuildKind::WoodRoof => Vec3::new(ROOF_RUN / ROOF_PITCH.cos(), 0.12, 2.0),
            BuildKind::WoodRoof45 => Vec3::new(ROOF_RUN / FRAC_PI_4.cos(), 0.12, 2.0),
            BuildKind::StoneFloor => Vec3::new(2.0, 0.4, 2.0),
            BuildKind::StoneWall => Vec3::new(2.0, 1.0, 0.8),
            BuildKind::StonePillar => Vec3::new(0.6, 2.0, 0.6),
        }
    }

    /// The piece's own turn under the builder's yaw: floors lie their boards flat (face up,
    /// battens underneath, boards along −Z), roofs tip down toward their eave at +X.
    pub fn base_rotation(self) -> Quat {
        match self {
            BuildKind::WoodFloor => Quat::from_rotation_x(-FRAC_PI_2),
            BuildKind::WoodRoof => Quat::from_rotation_z(-ROOF_PITCH),
            BuildKind::WoodRoof45 => Quat::from_rotation_z(-FRAC_PI_4),
            _ => Quat::IDENTITY,
        }
    }

    /// World rotation at a yaw (radians, about +Y).
    pub fn rotation(self, yaw: f32) -> Quat {
        Quat::from_rotation_y(yaw) * self.base_rotation()
    }

    /// How it looks.
    pub fn look(self) -> PropKind {
        let size = self.size().to_array();
        match self {
            BuildKind::WoodFloor | BuildKind::WoodWall => PropKind::BoardWall { size },
            BuildKind::WoodPole | BuildKind::WoodBeam => PropKind::Timber { size },
            BuildKind::WoodRoof | BuildKind::WoodRoof45 => PropKind::ShinglePanel {
                size,
                eave_at_plus_x: true,
            },
            BuildKind::StoneFloor | BuildKind::StoneWall | BuildKind::StonePillar => {
                PropKind::StoneBlock { size }
            }
        }
    }

    /// Fraction of the box that is solid wood (boards have gaps and battens; shingles lie
    /// over rafters), which sets how much a wooden piece has to burn.
    pub fn fill(self) -> f32 {
        match self {
            BuildKind::WoodFloor | BuildKind::WoodWall => 0.5,
            BuildKind::WoodRoof | BuildKind::WoodRoof45 => 0.3,
            _ => 1.0,
        }
    }
}

/// Something that builds: the player's hammer, a scripted builder, an AI. Aims from its
/// transform (−Z forward) unless `aim` is set.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(BuildPreview)]
#[serde(default)]
pub struct Builder {
    /// Build mode is on: the ghost shows and placing works.
    pub active: bool,
    pub kind: BuildKind,
    /// The ghost's turn about +Y, radians (rotate steps by [`ROTATE_STEP`]).
    pub yaw: f32,
    /// How far it reaches, m.
    pub reach_m: f32,
    /// Read the action map (see the module docs).
    pub use_input: bool,
    /// Aim along this ray (world origin and direction) instead of from the transform.
    pub aim: Option<([f64; 3], [f32; 3])>,
    /// Paint built pieces in their support colors while build mode is on.
    pub show_support: bool,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            active: true,
            kind: BuildKind::WoodFloor,
            yaw: 0.0,
            reach_m: 8.0,
            use_input: false,
            aim: None,
            show_support: true,
        }
    }
}

impl Builder {
    /// A player's builder: off until `build_mode` is pressed, reads the action map.
    pub fn player() -> Self {
        Self {
            active: false,
            use_input: true,
            ..Self::default()
        }
    }

    /// Aim from `from` toward `to`.
    pub fn aim_at(&mut self, from: DVec3, to: DVec3) {
        let dir = (to - from).as_vec3().normalize_or_zero();
        self.aim = Some((from.to_array(), dir.to_array()));
    }
}

/// Where the builder's ghost is and whether it can be placed there (written every frame).
#[derive(Component, Debug, Clone, Default)]
pub struct BuildPreview {
    pub placement: Option<Placement>,
    /// The ghost entity drawing it.
    pub ghost: Option<Entity>,
    /// The built piece the builder aims at (what `Remove` takes down).
    pub target: Option<Entity>,
}

/// The translucent preview of the piece about to be placed.
#[derive(Component, Debug, Clone, Copy)]
pub struct BuildGhost {
    pub builder: Entity,
    pub kind: BuildKind,
    /// 0 to 4: support colors (blue on the ground .. red), 5: can't be placed.
    pub level: usize,
}

/// A piece placed in build mode (so build mode can take it down again).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BuiltPiece {
    pub kind: BuildKind,
    /// Who placed it.
    #[serde(skip)]
    pub builder: Option<Entity>,
}

/// Tell a builder what to do.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BuildRequest {
    pub builder: Entity,
    pub action: BuildAction,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BuildAction {
    /// Build mode on or off.
    Toggle,
    Select(BuildKind),
    /// Pick the next (or, negative, previous) piece in [`BuildKind::ALL`].
    Next(i32),
    /// Turn the ghost by this many [`ROTATE_STEP`]s.
    Rotate(i32),
    /// Place the ghost's piece, if it can go there.
    Place,
    /// Take down the built piece aimed at.
    Remove,
}

/// Why a ghost can't be placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProblem {
    /// It would sit inside another piece.
    Blocked,
    /// It touches nothing that could hold it up.
    Floating,
    /// It reaches too far from support (the solver would drop it).
    TooWeak,
}

impl BuildProblem {
    pub fn describe(self) -> &'static str {
        match self {
            BuildProblem::Blocked => "it would sit inside another piece",
            BuildProblem::Floating => "nothing would hold it up",
            BuildProblem::TooWeak => "it is too far from support and would fall",
        }
    }
}

/// Where a ghost goes and what it would be like there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub kind: BuildKind,
    pub center: DVec3,
    pub rotation: Quat,
    /// It snapped onto a neighbor's snap point.
    pub snapped: bool,
    /// It stands on the ground.
    pub grounded: bool,
    /// Support it would get, support units (see [`PieceState`]).
    pub support: f32,
    /// Its material's full support.
    pub max_support: f32,
    pub problem: Option<BuildProblem>,
}

impl Placement {
    pub fn valid(&self) -> bool {
        self.problem.is_none()
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.center).with_rotation(self.rotation)
    }

    pub fn support_fraction(&self) -> f32 {
        if self.max_support <= 0.0 {
            0.0
        } else {
            (self.support / self.max_support).clamp(0.0, 1.0)
        }
    }

    /// Ghost color: the support level it would show (0 blue .. 4 red), 5 when it can't go.
    pub fn level(&self) -> usize {
        if !self.valid() {
            5
        } else {
            crate::assets::support_level(self.grounded, self.support_fraction())
        }
    }

    pub fn obb(&self) -> Obb {
        Obb {
            center: self.center,
            rotation: self.rotation,
            half: self.kind.size() * 0.5,
        }
    }
}

/// A standing building piece, as the build mode sees it.
#[derive(Debug, Clone, Copy)]
pub struct ScenePiece {
    pub entity: Entity,
    pub obb: Obb,
    /// Its current support (0 while it is about to fall).
    pub support: f32,
    pub built: bool,
}

/// What the aim ray hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AimHit {
    pub distance: f32,
    pub point: DVec3,
    pub normal: Vec3,
    /// The building piece hit, if it was one.
    pub piece: Option<Entity>,
}

/// Snap points of a piece's box, world space: the ends and middle of a stick (pole, beam,
/// pillar); the corners and edge midpoints of a panel (wall, floor, roof), on its top face
/// when it lies flat-ish (floors and roofs, so what is built on them stands on top) and on its
/// middle plane when it stands (walls); the corners, edge midpoints and face centers of a
/// block.
pub fn snap_points(obb: &Obb) -> Vec<DVec3> {
    let axes = obb.axes();
    let h = obb.half.to_array();
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| h[b].total_cmp(&h[a]));
    let [long, mid, short] = order;
    let at = |v: Vec3| obb.center + v.as_dvec3();
    let mut out = Vec::with_capacity(26);
    if h[long] > 3.0 * h[mid] {
        for s in [-1.0f32, 0.0, 1.0] {
            out.push(at(axes[long] * h[long] * s));
        }
    } else if h[short] < 0.3 * h[mid] {
        let n = axes[short];
        let face = if n.y.abs() > 0.6 {
            n * (h[short] * n.y.signum())
        } else {
            Vec3::ZERO
        };
        for u in [-1.0f32, 0.0, 1.0] {
            for v in [-1.0f32, 0.0, 1.0] {
                if u != 0.0 || v != 0.0 {
                    out.push(at(face + axes[long] * h[long] * u + axes[mid] * h[mid] * v));
                }
            }
        }
    } else {
        for u in [-1.0f32, 0.0, 1.0] {
            for v in [-1.0f32, 0.0, 1.0] {
                for w in [-1.0f32, 0.0, 1.0] {
                    if u != 0.0 || v != 0.0 || w != 0.0 {
                        out.push(at(axes[0] * h[0] * u
                            + axes[1] * h[1] * v
                            + axes[2] * h[2] * w));
                    }
                }
            }
        }
    }
    out
}

/// The solid middle of a box: what two pieces may not share. Pieces overlap where they join
/// (a floor's edge over a wall's top, a beam's end over a post, a roof's eave sunk into the
/// wall plate), so each box gives up 15 cm all round, and a thin board or roof panel keeps
/// only its middle sheet.
fn core(obb: &Obb) -> Obb {
    let shrink = (obb.half * 0.49).min(Vec3::splat(0.15));
    Obb {
        half: obb.half - shrink,
        ..*obb
    }
}

/// Where a ray enters a box and the face normal there.
pub fn box_hit(obb: &Obb, origin: DVec3, dir: Vec3, max: f32) -> Option<(f32, Vec3)> {
    let t = obb.ray(origin, dir, max)?;
    let p = origin + (dir * t).as_dvec3();
    let local = obb.rotation.inverse() * (p - obb.center).as_vec3();
    let r = local / obb.half.max(Vec3::splat(1e-4));
    let a = r.abs();
    let k = if a.x >= a.y && a.x >= a.z {
        0
    } else if a.y >= a.z {
        1
    } else {
        2
    };
    let mut n = Vec3::ZERO;
    n[k] = if r[k] < 0.0 { -1.0 } else { 1.0 };
    Some((t, obb.rotation * n))
}

/// Where a ray meets the ground (`ground` gives height and normal at a point), marching in
/// 10 cm steps and then bisecting.
pub fn ground_hit(
    ground: &dyn Fn(DVec3) -> Option<(f64, Vec3)>,
    origin: DVec3,
    dir: Vec3,
    max: f32,
) -> Option<(f32, Vec3)> {
    let above = |t: f32| {
        let p = origin + (dir * t).as_dvec3();
        ground(p).map(|(h, n)| (p.y - h, n))
    };
    if above(0.0)?.0 < 0.0 {
        return None;
    }
    let step = 0.1;
    let mut prev = 0.0;
    let mut t = step;
    while t <= max + 1e-4 {
        if above(t)?.0 <= 0.0 {
            let (mut a, mut b) = (prev, t);
            for _ in 0..14 {
                let m = 0.5 * (a + b);
                if above(m)?.0 > 0.0 {
                    a = m;
                } else {
                    b = m;
                }
            }
            return Some((b, above(b)?.1));
        }
        prev = t;
        t += step;
    }
    None
}

/// Nearest of the pieces' hits and another surface's (rock, ground).
pub fn nearest_hit(
    scene: &[ScenePiece],
    origin: DVec3,
    dir: Vec3,
    reach: f32,
    surface: Option<(f32, Vec3)>,
) -> Option<AimHit> {
    let mut best = surface.map(|(t, n)| AimHit {
        distance: t,
        point: origin + (dir * t).as_dvec3(),
        normal: n,
        piece: None,
    });
    for p in scene {
        if let Some((t, n)) = box_hit(&p.obb, origin, dir, reach)
            && best.is_none_or(|b| t < b.distance)
        {
            best = Some(AimHit {
                distance: t,
                point: origin + (dir * t).as_dvec3(),
                normal: n,
                piece: Some(p.entity),
            });
        }
    }
    best
}

/// Where a `kind` piece turned `yaw` goes when aimed at `hit`, snapped to its neighbors in
/// `scene`, and whether it can go there. `ground_height` gives the ground under a point.
pub fn plan(
    kind: BuildKind,
    yaw: f32,
    hit: &AimHit,
    scene: &[ScenePiece],
    ground_height: &dyn Fn(DVec3) -> f64,
    table: &DestructionMaterials,
) -> Placement {
    let rotation = kind.rotation(yaw);
    let half = kind.size() * 0.5;
    let ghost = |center: DVec3| Obb {
        center,
        rotation,
        half,
    };
    let axes = ghost(DVec3::ZERO).axes();
    let extent = |n: Vec3| {
        axes.iter()
            .zip(half.to_array())
            .map(|(a, h)| a.dot(n).abs() * h)
            .sum::<f32>()
    };
    // Set it on what was hit: on top of the ground or a top face, against a side face.
    let n = if hit.piece.is_none() || hit.normal.y > 0.7 {
        Vec3::Y
    } else {
        hit.normal
    };
    let mut center = hit.point + (n * extent(n)).as_dvec3();
    let radius = half.length() as f64;
    let blocked = |c: DVec3| {
        let mine = core(&ghost(c));
        scene.iter().any(|p| {
            (p.obb.center - c).length() < radius + p.obb.half.length() as f64
                && mine.touches(&core(&p.obb), 0.0)
        })
    };
    // Snap: the closest pairs of the ghost's and its neighbors' snap points, closest first;
    // the first that leaves the ghost free wins.
    let reach = radius + SNAP_RADIUS_M as f64;
    let targets: Vec<DVec3> = scene
        .iter()
        .filter(|p| (p.obb.center - center).length() < reach + p.obb.half.length() as f64)
        .flat_map(|p| snap_points(&p.obb))
        .collect();
    let mut pairs: Vec<(f64, DVec3)> = Vec::new();
    for g in snap_points(&ghost(center)) {
        for t in &targets {
            let d = g.distance(*t);
            if d < SNAP_RADIUS_M as f64 {
                pairs.push((d, *t - g));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut snapped = false;
    let mut tried: Vec<DVec3> = Vec::new();
    for (_, offset) in &pairs {
        if tried.iter().any(|o| o.distance(*offset) < 1e-3) {
            continue;
        }
        if tried.len() >= MAX_SNAP_TRIES {
            break;
        }
        tried.push(*offset);
        if !blocked(center + *offset) {
            center += *offset;
            snapped = true;
            break;
        }
    }
    // Every snap is taken: stay where aimed if that is free, else show the best snap, blocked.
    if !snapped
        && blocked(center)
        && let Some((_, offset)) = pairs.first()
    {
        center += *offset;
        snapped = true;
    }
    let is_blocked = blocked(center);
    let obb = ghost(center);
    let props = table.get(kind.material());
    let grounded = obb.min_y() <= ground_height(center) + GROUND_TOLERANCE_M;
    let mut touching = grounded;
    let mut support = if grounded { props.max_support } else { 0.0 };
    if !grounded {
        for p in scene {
            if (p.obb.center - center).length() > radius + p.obb.half.length() as f64 + 0.1
                || !obb.touches(&p.obb, TOUCH_MARGIN_M)
            {
                continue;
            }
            touching = true;
            let d = (p.obb.center - center).as_vec3();
            let v = d.y.abs();
            let h = Vec3::new(d.x, 0.0, d.z).length();
            let s = (p.support - props.vertical_loss_per_m * v - props.horizontal_loss_per_m * h)
                .min(props.max_support);
            support = support.max(s);
        }
    }
    let problem = if is_blocked {
        Some(BuildProblem::Blocked)
    } else if !touching {
        Some(BuildProblem::Floating)
    } else if support < props.min_support {
        Some(BuildProblem::TooWeak)
    } else {
        None
    };
    Placement {
        kind,
        center,
        rotation,
        snapped,
        grounded,
        support: support.max(0.0),
        max_support: props.max_support,
        problem,
    }
}

/// A placed piece (what [`BuildAction::Place`] spawns): its box, look and support colors,
/// and for wood the fuel it holds.
pub fn built_piece(
    kind: BuildKind,
    transform: Transform,
    builder: Option<Entity>,
    show_support: bool,
    table: &DestructionMaterials,
) -> (impl Bundle, Option<Combustible>) {
    let size = kind.size();
    let material = kind.material();
    let mut piece = StructuralPiece::new(material, size);
    if material == DestructibleMaterial::Wood {
        piece.break_energy_j = 6000.0;
    }
    // A few looks per kind, picked by spot, so neighbors differ but meshes are shared.
    let p = transform.translation;
    let seed = ((p.x * 7.0 + p.y * 13.0 + p.z * 17.0).abs() as u64) % LOOKS;
    let fuel = table.get(material).fuel_kg_m3 * size.x * size.y * size.z * kind.fill();
    (
        (
            Name::new(kind.label()),
            transform,
            GlobalTransform::from(transform),
            piece,
            PropModel::new(kind.look(), seed),
            ShowSupport {
                on: show_support,
                children: true,
            },
            BuiltPiece { kind, builder },
        ),
        (fuel > 0.0).then(|| Combustible {
            fuel_kg: fuel,
            moisture: TIMBER_MOISTURE,
            ignition_c: WOOD_IGNITION_C,
            ..Combustible::default()
        }),
    )
}

/// Spawn a [`built_piece`].
pub fn spawn_built_piece(
    commands: &mut Commands,
    kind: BuildKind,
    transform: Transform,
    builder: Option<Entity>,
    show_support: bool,
    table: &DestructionMaterials,
) -> Entity {
    let (bundle, fuel) = built_piece(kind, transform, builder, show_support, table);
    let mut e = commands.spawn(bundle);
    if let Some(fuel) = fuel {
        e.insert(fuel);
    }
    e.id()
}

/// Bind the build actions (see the module docs) unless the game's map already has them
/// (`Startup`).
pub fn bind_actions(map: Option<ResMut<InputMap>>) {
    let Some(mut map) = map else {
        return;
    };
    let mut bind = |action: &str, bindings: &[Binding]| {
        if !map.actions.contains_key(action) {
            for b in bindings {
                map.bind(action, *b);
            }
        }
    };
    bind(
        "build_mode",
        &[
            Binding::Key(KeyCode::B),
            Binding::Gamepad(GamepadButton::DPadLeft),
        ],
    );
    bind(
        "build_place",
        &[
            Binding::Mouse(MouseButton::Left),
            Binding::Gamepad(GamepadButton::RightTrigger),
        ],
    );
    bind(
        "build_remove",
        &[
            Binding::Key(KeyCode::X),
            Binding::Gamepad(GamepadButton::North),
        ],
    );
    bind(
        "build_rotate",
        &[
            Binding::Key(KeyCode::R),
            Binding::Gamepad(GamepadButton::DPadRight),
        ],
    );
    bind(
        "build_next",
        &[
            Binding::Key(KeyCode::C),
            Binding::Gamepad(GamepadButton::LeftBumper),
        ],
    );
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    for (i, key) in digits.into_iter().enumerate() {
        bind(&format!("build_piece_{}", i + 1), &[Binding::Key(key)]);
    }
}

/// Turn presses into [`BuildRequest`]s for builders that read input.
pub fn build_input_system(
    actions: Option<Res<ActionState>>,
    builders: Query<(Entity, &Builder)>,
    mut out: MessageWriter<BuildRequest>,
) {
    let Some(actions) = actions else {
        return;
    };
    for (e, b) in &builders {
        if !b.use_input {
            continue;
        }
        let mut send = |action| {
            out.write(BuildRequest { builder: e, action });
        };
        if actions.just_pressed("build_mode") {
            send(BuildAction::Toggle);
        }
        if !b.active {
            continue;
        }
        if actions.just_pressed("build_next") {
            send(BuildAction::Next(1));
        }
        for (i, kind) in BuildKind::ALL.into_iter().enumerate() {
            if actions.just_pressed(&format!("build_piece_{}", i + 1)) {
                send(BuildAction::Select(kind));
            }
        }
        if actions.just_pressed("build_rotate") {
            send(BuildAction::Rotate(1));
        }
        if actions.just_pressed("build_place") {
            send(BuildAction::Place);
        }
        if actions.just_pressed("build_remove") {
            send(BuildAction::Remove);
        }
    }
}

/// The aim ray of a builder.
fn aim_ray(b: &Builder, gt: &GlobalTransform) -> (DVec3, Vec3) {
    match b.aim {
        Some((o, d)) => (
            DVec3::from_array(o),
            Vec3::from_array(d).normalize_or_zero(),
        ),
        None => (gt.translation(), gt.rotation() * -Vec3::Z),
    }
}

/// The first rock or ground along a ray.
fn surface_hit(
    ground: Option<&Ground>,
    volumes: &Query<(&GlobalTransform, &VoxelState)>,
    origin: DVec3,
    dir: Vec3,
    reach: f32,
) -> Option<(f32, Vec3)> {
    let mut best = ground.and_then(|g| {
        ground_hit(
            &|p| g.sample(p).map(|s| (s.height, s.normal)),
            origin,
            dir,
            reach,
        )
    });
    for (gt, state) in volumes {
        let o = gt.affine().inverse().transform_point3(origin).as_vec3();
        let d = gt.rotation().inverse() * dir;
        if let Some((t, _, n)) = state.raycast(o, d, reach)
            && best.is_none_or(|(b, _)| t < b)
        {
            best = Some((t, gt.rotation() * n));
        }
    }
    best
}

/// Standing pieces within `radius` of `around`.
fn scene_near<'a>(
    pieces: impl Iterator<
        Item = (
            Entity,
            &'a Transform,
            &'a GlobalTransform,
            Option<&'a ChildOf>,
            &'a StructuralPiece,
            &'a PieceState,
            bool,
        ),
    >,
    around: DVec3,
    radius: f64,
) -> Vec<ScenePiece> {
    pieces
        .filter_map(|(e, t, gt, parent, piece, state, built)| {
            // Root pieces read Transform, like the structure solver, so a piece placed this
            // frame is already solid.
            let (center, rotation) = if parent.is_none() {
                (t.translation, t.rotation)
            } else {
                (gt.translation(), gt.rotation())
            };
            ((center - around).length() < radius + piece.half_extents.length() as f64).then(|| {
                ScenePiece {
                    entity: e,
                    obb: Obb {
                        center,
                        rotation,
                        half: piece.half_extents,
                    },
                    support: if state.collapse_in.is_some() {
                        0.0
                    } else {
                        state.support
                    },
                    built,
                }
            })
        })
        .collect()
}

/// Run builders: apply their requests, place and remove pieces, and move their ghosts
/// (Update).
pub fn build_system(
    mut commands: Commands,
    mut requests: MessageReader<BuildRequest>,
    mut noise: MessageWriter<NoiseEvent>,
    table: Res<DestructionMaterials>,
    ground: Option<Res<Ground>>,
    mut builders: Query<(Entity, &mut Builder, &mut BuildPreview, &GlobalTransform)>,
    pieces: Query<(
        Entity,
        &Transform,
        &GlobalTransform,
        Option<&ChildOf>,
        &StructuralPiece,
        &PieceState,
        Has<BuiltPiece>,
    )>,
    volumes: Query<(&GlobalTransform, &VoxelState)>,
    mut ghosts: Query<(Entity, &mut BuildGhost, &mut Transform), Without<StructuralPiece>>,
    mut shown: Query<(&BuiltPiece, &mut ShowSupport)>,
) {
    let requests: Vec<BuildRequest> = requests.read().copied().collect();
    let ground = ground.as_deref();
    let ground_height = |p: DVec3| {
        ground
            .and_then(|g| g.height(p))
            .unwrap_or(f64::NEG_INFINITY)
    };
    let mut placed: Vec<Obb> = Vec::new();
    let mut removed: Vec<Entity> = Vec::new();
    let mut show_any = false;
    for (e, mut b, mut preview, gt) in &mut builders {
        let (origin, dir) = aim_ray(&b, gt);
        let mut scene = scene_near(pieces.iter(), origin, b.reach_m as f64 + 6.0);
        scene.retain(|p| !removed.contains(&p.entity));
        let plan_now = |b: &Builder, scene: &[ScenePiece]| {
            if !b.active || dir == Vec3::ZERO {
                return (None, None);
            }
            let surface = surface_hit(ground, &volumes, origin, dir, b.reach_m);
            let Some(hit) = nearest_hit(scene, origin, dir, b.reach_m, surface) else {
                return (None, None);
            };
            let target = hit
                .piece
                .filter(|p| scene.iter().any(|s| s.entity == *p && s.built));
            (
                Some(plan(b.kind, b.yaw, &hit, scene, &ground_height, &table)),
                target,
            )
        };
        for r in requests.iter().filter(|r| r.builder == e) {
            match r.action {
                BuildAction::Toggle => b.active = !b.active,
                BuildAction::Select(kind) => b.kind = kind,
                BuildAction::Next(n) => {
                    let i = BuildKind::ALL
                        .iter()
                        .position(|k| *k == b.kind)
                        .unwrap_or(0) as i32;
                    let len = BuildKind::ALL.len() as i32;
                    b.kind = BuildKind::ALL[(i + n).rem_euclid(len) as usize];
                }
                BuildAction::Rotate(n) => {
                    b.yaw = (b.yaw + n as f32 * ROTATE_STEP).rem_euclid(std::f32::consts::TAU);
                }
                BuildAction::Place => {
                    if let (Some(p), _) = plan_now(&b, &scene)
                        && p.valid()
                        && !placed.iter().any(|o| core(o).touches(&core(&p.obb()), 0.0))
                    {
                        let piece = spawn_built_piece(
                            &mut commands,
                            p.kind,
                            p.transform(),
                            Some(e),
                            b.show_support,
                            &table,
                        );
                        // Solid at once for the next request this frame.
                        scene.push(ScenePiece {
                            entity: piece,
                            obb: p.obb(),
                            support: p.support,
                            built: true,
                        });
                        placed.push(p.obb());
                        noise.write(NoiseEvent {
                            position: p.center,
                            radius: 20.0,
                            source: Some(e),
                        });
                    }
                }
                BuildAction::Remove => {
                    if let (_, Some(target)) = plan_now(&b, &scene) {
                        commands.entity(target).despawn();
                        scene.retain(|p| p.entity != target);
                        removed.push(target);
                    }
                }
            }
        }
        let (placement, target) = plan_now(&b, &scene);
        preview.placement = placement;
        preview.target = target;
        show_any |= b.active && b.show_support;
        // The ghost: moved along, rebuilt when the piece changes, gone when there is none.
        let ghost = preview.ghost.and_then(|g| ghosts.get_mut(g).ok());
        match (placement, ghost) {
            (Some(p), Some((_, mut look, mut t))) if look.kind == p.kind => {
                *t = p.transform();
                look.level = p.level();
            }
            (p, old) => {
                if let Some((g, ..)) = old {
                    commands.entity(g).despawn();
                }
                preview.ghost = p.map(|p| {
                    let t = p.transform();
                    commands
                        .spawn((
                            Name::new("Build ghost"),
                            t,
                            GlobalTransform::from(t),
                            PropModel::new(p.kind.look(), 0),
                            BuildGhost {
                                builder: e,
                                kind: p.kind,
                                level: p.level(),
                            },
                        ))
                        .id()
                });
            }
        }
    }
    // Ghosts whose builder is gone go too.
    for (g, look, _) in &ghosts {
        if builders.get(look.builder).is_err() {
            commands.entity(g).despawn();
        }
    }
    // Built pieces show their support while someone builds.
    for (_, mut show) in &mut shown {
        if show.on != show_any {
            show.on = show_any;
        }
    }
}

/// Draw ghosts in their translucent support color, without shadows (Update, after the
/// props are dressed).
pub fn ghost_look_system(
    assets: Option<Res<DestructionAssets>>,
    ghosts: Query<(&BuildGhost, &Children)>,
    mut renderers: Query<&mut MeshRenderer>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (g, children) in &ghosts {
        let m = MaterialRef::Handle(assets.ghost(g.kind.material(), g.level));
        for c in children.iter() {
            if let Ok(mut r) = renderers.get_mut(c) {
                if r.material != m {
                    r.material = m.clone();
                }
                if r.cast_shadows {
                    r.cast_shadows = false;
                }
            }
        }
    }
}

/// Plan for the AI bridge: what a `kind` piece aimed from `origin` along `dir` would do.
pub fn plan_in_world(
    world: &mut World,
    kind: BuildKind,
    yaw: f32,
    origin: DVec3,
    dir: Vec3,
    reach: f32,
) -> Option<(Placement, Option<Entity>)> {
    let mut q = world.query::<(
        Entity,
        &Transform,
        &GlobalTransform,
        Option<&ChildOf>,
        &StructuralPiece,
        &PieceState,
        Has<BuiltPiece>,
    )>();
    let scene = scene_near(q.iter(world), origin, reach as f64 + 6.0);
    let ground = world.get_resource::<Ground>().cloned();
    let mut surface = ground.as_ref().and_then(|g| {
        ground_hit(
            &|p| g.sample(p).map(|s| (s.height, s.normal)),
            origin,
            dir,
            reach,
        )
    });
    let mut vq = world.query::<(&GlobalTransform, &VoxelState)>();
    for (gt, state) in vq.iter(world) {
        let o = gt.affine().inverse().transform_point3(origin).as_vec3();
        let d = gt.rotation().inverse() * dir;
        if let Some((t, _, n)) = state.raycast(o, d, reach)
            && surface.is_none_or(|(b, _)| t < b)
        {
            surface = Some((t, gt.rotation() * n));
        }
    }
    let hit = nearest_hit(&scene, origin, dir, reach, surface)?;
    let table = world
        .get_resource::<DestructionMaterials>()
        .cloned()
        .unwrap_or_default();
    let ground_height = |p: DVec3| {
        ground
            .as_ref()
            .and_then(|g| g.height(p))
            .unwrap_or(f64::NEG_INFINITY)
    };
    let target = hit
        .piece
        .filter(|p| scene.iter().any(|s| s.entity == *p && s.built));
    Some((
        plan(kind, yaw, &hit, &scene, &ground_height, &table),
        target,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(e: u32, kind: BuildKind, center: DVec3, yaw: f32, support: f32) -> ScenePiece {
        ScenePiece {
            entity: Entity::from_raw_u32(e).unwrap(),
            obb: Obb {
                center,
                rotation: kind.rotation(yaw),
                half: kind.size() * 0.5,
            },
            support,
            built: true,
        }
    }

    fn flat(_: DVec3) -> f64 {
        0.0
    }

    fn hit_top(point: DVec3, piece: Option<u32>) -> AimHit {
        AimHit {
            distance: 2.0,
            point,
            normal: Vec3::Y,
            piece: piece.map(|e| Entity::from_raw_u32(e).unwrap()),
        }
    }

    #[test]
    fn snap_points_follow_the_shape() {
        let pole = piece(1, BuildKind::WoodPole, DVec3::new(0.0, 1.0, 0.0), 0.0, 0.0);
        let pts = snap_points(&pole.obb);
        assert_eq!(pts.len(), 3);
        assert!(pts.iter().any(|p| p.distance(DVec3::ZERO) < 1e-6));
        let floor = piece(
            2,
            BuildKind::WoodFloor,
            DVec3::new(0.0, 0.05, 0.0),
            0.0,
            0.0,
        );
        let pts = snap_points(&floor.obb);
        assert_eq!(pts.len(), 8);
        // On the walking surface, at the corners and edge midpoints.
        assert!(pts.iter().all(|p| (p.y - 0.1).abs() < 1e-5), "{pts:?}");
        assert!(
            pts.iter()
                .any(|p| p.distance(DVec3::new(1.0, 0.1, -1.0)) < 1e-5)
        );
        let wall = piece(3, BuildKind::WoodWall, DVec3::new(0.0, 1.0, 0.0), 0.0, 0.0);
        let pts = snap_points(&wall.obb);
        assert_eq!(pts.len(), 8);
        assert!(
            pts.iter().all(|p| p.z.abs() < 1e-5),
            "middle plane: {pts:?}"
        );
        let block = piece(4, BuildKind::StoneWall, DVec3::new(0.0, 0.5, 0.0), 0.0, 0.0);
        assert_eq!(snap_points(&block.obb).len(), 26);
    }

    #[test]
    fn a_floor_snaps_beside_a_floor() {
        let table = DestructionMaterials::default();
        let a = piece(
            1,
            BuildKind::WoodFloor,
            DVec3::new(0.0, 0.05, 0.0),
            0.0,
            100.0,
        );
        // Aimed at the ground a little past its +X edge: lands edge to edge.
        let p = plan(
            BuildKind::WoodFloor,
            0.0,
            &hit_top(DVec3::new(1.7, 0.0, 0.2), None),
            &[a],
            &flat,
            &table,
        );
        assert!(p.snapped && p.valid(), "{p:?}");
        assert!(
            p.center.distance(DVec3::new(2.0, 0.05, 0.0)) < 1e-4,
            "{p:?}"
        );
        // Aimed at its middle: every snap would overlap it, so it lies on top where aimed.
        let p = plan(
            BuildKind::WoodFloor,
            0.0,
            &hit_top(DVec3::new(0.3, 0.1, 0.0), Some(1)),
            &[a],
            &flat,
            &table,
        );
        assert!(p.valid(), "{p:?}");
        assert!(!core(&p.obb()).touches(&core(&a.obb), 0.0));
    }

    #[test]
    fn a_wall_stands_on_a_floor_edge_and_floats_without_one() {
        let table = DestructionMaterials::default();
        let floor = piece(
            1,
            BuildKind::StoneFloor,
            DVec3::new(0.0, 0.2, 0.0),
            0.0,
            1000.0,
        );
        let p = plan(
            BuildKind::WoodWall,
            0.0,
            &hit_top(DVec3::new(0.1, 0.4, -0.85), Some(1)),
            &[floor],
            &flat,
            &table,
        );
        assert!(p.valid(), "{p:?}");
        assert!(
            p.center.distance(DVec3::new(0.0, 1.4, -1.0)) < 1e-4,
            "on the back edge: {p:?}"
        );
        assert!(!p.grounded);
        assert!((p.support - 100.0).abs() < 1e-3, "{p:?}");
        // The same wall aimed at the sky beside nothing: nothing holds it.
        let p = plan(
            BuildKind::WoodWall,
            0.0,
            &AimHit {
                distance: 5.0,
                point: DVec3::new(10.0, 3.0, 0.0),
                normal: Vec3::X,
                piece: Some(Entity::from_raw_u32(9).unwrap()),
            },
            &[floor],
            &flat,
            &table,
        );
        assert_eq!(p.problem, Some(BuildProblem::Floating));
    }

    #[test]
    fn a_cantilever_gets_weaker_until_it_cannot_be_placed() {
        let table = DestructionMaterials::default();
        // A floor held by a wall at 75 support; each further floor loses 20/m × 2 m.
        let mut scene = vec![piece(
            1,
            BuildKind::WoodFloor,
            DVec3::new(0.0, 2.35, 0.0),
            0.0,
            75.0,
        )];
        let mut supports = Vec::new();
        for k in 1..4 {
            let edge = 1.0 + 2.0 * (k - 1) as f64;
            let p = plan(
                BuildKind::WoodFloor,
                0.0,
                &AimHit {
                    distance: 2.0,
                    point: DVec3::new(edge, 2.35, 0.0),
                    normal: Vec3::X,
                    piece: Some(scene.last().unwrap().entity),
                },
                &scene,
                &flat,
                &table,
            );
            assert!(
                p.center.distance(DVec3::new(edge + 1.0, 2.35, 0.0)) < 1e-4,
                "{k}: {p:?}"
            );
            supports.push((p.support, p.problem));
            scene.push(piece(
                10 + k as u32,
                BuildKind::WoodFloor,
                p.center,
                0.0,
                p.support,
            ));
        }
        assert!((supports[0].0 - 35.0).abs() < 1e-3, "{supports:?}");
        assert_eq!(supports[0].1, None);
        assert_eq!(supports[1].1, Some(BuildProblem::TooWeak), "{supports:?}");
    }

    #[test]
    fn the_ground_and_boxes_are_hit_where_the_ray_meets_them() {
        let (t, n) = ground_hit(
            &|_| Some((0.0, Vec3::Y)),
            DVec3::new(0.0, 2.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0).normalize(),
            8.0,
        )
        .unwrap();
        assert!((t - 2.0 * 2f32.sqrt()).abs() < 1e-3 && n == Vec3::Y);
        let wall = piece(1, BuildKind::WoodWall, DVec3::new(0.0, 1.0, 3.0), 0.0, 0.0);
        let (t, n) = box_hit(&wall.obb, DVec3::new(0.0, 1.0, 0.0), Vec3::Z, 8.0).unwrap();
        assert!((t - 2.95).abs() < 1e-4);
        assert!(n.distance(-Vec3::Z) < 1e-5);
    }
}

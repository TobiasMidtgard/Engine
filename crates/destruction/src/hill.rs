//! The Valheim-style dig-and-build hill, built by [`spawn_voxel_hill`] around an origin (for the
//! showcase's voxel hill zone). One loop of [`HILL_CYCLE_SECS`]:
//!
//! - a pickaxe drives a tunnel into the hill's rocky side, then opens a chamber too wide for the
//!   rock and soil above it, which caves in;
//! - a hoe cuts and fills a level terrace on the other flank, rounds its edges and levels a
//!   floor pad in its middle;
//! - beside the hill a timber shelter goes up piece by piece in build-mode support colors (blue
//!   on the ground through red at the limit), then loses two posts and half of it comes down;
//! - in the build yard off the hill's other foot a builder in Valheim build mode
//!   ([`crate::build`]) aims, the ghost of each piece snaps into place in the support color it
//!   will have and the piece goes down: a stone plinth, posts, board walls, a loft floor, a
//!   shingle roof and a deck reaching out from the gable. Then the cabin shows its support
//!   colors: the deck's second row orange at the edge of support, and the ghost of a third row
//!   red because nothing could hold it;
//! - on the leveled terrace a second builder lays a stone floor on the bare rock, stands board
//!   walls at its ends, roofs them and walls in the back against the cut bank; then the
//!   shelter shows its support colors, the floor blue because the rock is ground.
//!
//! Then the hill, the hut and the cabin are rebuilt and it starts again.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::{DamageEvent, DamageKind, EditOp, EditShape, SurfaceMaterials, TerrainEdit};

use crate::DemoView;
use crate::build::{BuildAction, BuildKind, BuildRequest, Builder, BuiltPiece};
use crate::debris::Debris;
use crate::dress::PropModel;
use crate::material::DestructibleMaterial;
use crate::props::PropKind;
use crate::structure::{ShowSupport, StructuralPiece};
use crate::voxel::VoxelVolume;

/// Seconds per loop.
pub const HILL_CYCLE_SECS: f32 = 28.0;

/// Hill size (width × height × depth, m) and voxel size.
pub const HILL_SIZE: [f32; 3] = [34.0, 8.0, 30.0];
const HILL_VOXEL: f32 = 0.3;
/// Where the hut stands, relative to the origin (flat ground beside the hill's −Z foot).
const HUT: [f64; 3] = [15.0, 0.0, -10.0];
/// Where the build yard's cabin stands, relative to the origin (flat ground off the hill's
/// −Z/−X foot, its deck reaching out toward +X).
pub const YARD: [f64; 3] = [-14.0, 0.0, -24.0];
/// When the yard's builder aims at its first piece, and the seconds per piece.
const YARD_START: f32 = 0.5;
const YARD_STEP: f32 = 0.5;
/// When the finished cabin shows its support colors (with the red ghost of the last step).
const YARD_SUPPORT_AT: f32 = 14.2;
/// Where the terrace's shelter stands, relative to the origin: the middle of the leveled
/// terrace, on its rock.
pub const TERRACE: [f64; 3] = [8.8, TERRACE_Y, 2.1];
/// When the terrace's builder starts (the hoe is done at 17.8 s), and the seconds per piece.
const TERRACE_START: f32 = 18.3;
const TERRACE_STEP: f32 = 0.45;
/// When the finished shelter shows its support colors (its floor blue on the rock).
const TERRACE_SUPPORT_AT: f32 = 22.0;

const TUNNEL_DIGS: u32 = 15;
const CHAMBER_DIGS: u32 = 15;
const RAISES: u32 = 6;
/// Height of the leveled terrace above the hill's foot, m.
const TERRACE_Y: f64 = 3.6;
/// The hoe's level-ground strokes; each pass averages the soil with its neighbors once, so a
/// few in a row turn the raised mounds' edges into a soft bank.
const SMOOTH_PASSES: u32 = 6;
/// Half size of the floor pad the hoe levels for the terrace's shelter (x, z), m.
const PAD_HALF: [f64; 2] = [1.7, 2.5];

/// The dig-and-build hill's moments.
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "voxel_tunnel",
        name: "Digging a tunnel into the hill",
        eye: [-2.5, 2.2, -16.0],
        target: [-4.0, 1.4, -6.0],
        hour: 10.5,
        warmup_secs: 5.1,
    },
    DemoView {
        key: "voxel_cave_in",
        name: "The tunnel's hall caving in",
        eye: [-1.0, 7.0, -17.0],
        target: [-4.5, 2.5, -2.0],
        hour: 10.5,
        warmup_secs: 10.6,
    },
    DemoView {
        key: "voxel_terrace",
        name: "Raising and leveling a terrace with the hoe",
        eye: [18.0, 8.0, -6.0],
        target: [8.5, 3.5, 2.0],
        hour: 10.5,
        warmup_secs: 17.8,
    },
    DemoView {
        key: "voxel_build",
        name: "Building a cabin in Valheim build mode: a roof piece's ghost snapping on",
        eye: [-20.0, 5.5, -17.0],
        target: [-14.0, 2.0, -24.5],
        hour: 10.5,
        warmup_secs: 10.2,
    },
    DemoView {
        key: "voxel_build_reach",
        name: "The cabin's deck at the limit of its support, a red ghost where it can't go",
        eye: [-3.0, 5.4, -16.0],
        target: [-11.0, 1.9, -24.5],
        hour: 10.5,
        warmup_secs: 18.0,
    },
    DemoView {
        key: "voxel_shelter",
        name: "A shelter built on the leveled terrace: its stone floor is blue, the rock is ground",
        eye: [15.5, 6.8, -3.0],
        target: [8.6, 4.6, 2.1],
        hour: 10.5,
        warmup_secs: 22.2,
    },
    DemoView {
        key: "voxel_hut",
        name: "A timber hut in support colors losing its posts",
        eye: [9.5, 3.5, -16.5],
        target: [15.0, 1.8, -10.0],
        hour: 10.5,
        warmup_secs: 19.5,
    },
];

/// The running hill script.
#[derive(Component, Debug, Clone, Default)]
pub struct HillDemo {
    pub origin: DVec3,
    /// Seconds into the current loop.
    pub time: f32,
    pub cycle: u32,
    next: usize,
    pub hill: Option<Entity>,
    /// Hut pieces built so far this loop.
    pub hut: Vec<Entity>,
    /// The builders of the yard and the terrace (by [`Site`]).
    pub builders: [Option<Entity>; 2],
}

/// Where a builder of the hill works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// The build yard off the hill's foot (the cabin).
    Yard,
    /// The terrace the hoe leveled (the shelter on the rock).
    Terrace,
}

impl Site {
    pub const ALL: [Site; 2] = [Site::Yard, Site::Terrace];

    /// The site relative to the hill's origin.
    pub fn offset(self) -> DVec3 {
        match self {
            Site::Yard => DVec3::from_array(YARD),
            Site::Terrace => DVec3::from_array(TERRACE),
        }
    }

    /// What is built there, relative to [`Site::offset`].
    pub fn plan(self) -> Vec<BuildStep> {
        match self {
            Site::Yard => yard_plan(),
            Site::Terrace => terrace_plan(),
        }
    }

    fn start_and_step(self) -> (f32, f32) {
        match self {
            Site::Yard => (YARD_START, YARD_STEP),
            Site::Terrace => (TERRACE_START, TERRACE_STEP),
        }
    }

    fn builder_name(self) -> &'static str {
        match self {
            Site::Yard => "Yard builder",
            Site::Terrace => "Terrace builder",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Build(usize),
    Tunnel(u32),
    Chamber(u32),
    Raise(u32),
    Level,
    Smooth,
    /// Cut and fill a flat floor pad for the shelter in the rounded terrace.
    Pad,
    KnockPost(usize),
    /// A site's builder picks a piece and aims; then places it.
    Aim(Site, usize),
    Place(Site),
    /// A site's finished building shows its support colors.
    ShowSupport(Site),
    Reset,
}

fn timeline() -> Vec<(f32, Step)> {
    let mut t = Vec::new();
    for k in 0..hut_plan().len() {
        t.push((0.4 + k as f32 * 0.45, Step::Build(k)));
    }
    for k in 0..TUNNEL_DIGS {
        t.push((1.0 + k as f32 * 0.45, Step::Tunnel(k)));
    }
    for k in 0..CHAMBER_DIGS {
        t.push((8.0 + k as f32 * 0.35, Step::Chamber(k)));
    }
    for k in 0..RAISES {
        t.push((14.0 + k as f32 * 0.5, Step::Raise(k)));
    }
    t.push((14.0 + RAISES as f32 * 0.5, Step::Level));
    for k in 0..SMOOTH_PASSES {
        t.push((17.2 + k as f32 * 0.12, Step::Smooth));
    }
    t.push((18.0, Step::Pad));
    for site in Site::ALL {
        let (start, each) = site.start_and_step();
        for (k, step) in site.plan().iter().enumerate() {
            let at = start + k as f32 * each;
            t.push((at, Step::Aim(site, k)));
            if step.place {
                t.push((at + each * 0.8, Step::Place(site)));
            }
        }
    }
    t.push((YARD_SUPPORT_AT, Step::ShowSupport(Site::Yard)));
    t.push((TERRACE_SUPPORT_AT, Step::ShowSupport(Site::Terrace)));
    t.push((18.5, Step::KnockPost(0)));
    t.push((19.6, Step::KnockPost(1)));
    t.push((HILL_CYCLE_SECS - 0.01, Step::Reset));
    t.sort_by(|a, b| a.0.total_cmp(&b.0));
    t
}

/// One piece of the hut: name, center (relative to the hut), rotation, size.
struct PieceSpec {
    name: &'static str,
    at: [f64; 3],
    rotation: Quat,
    size: Vec3,
    look: PropKind,
}

impl PieceSpec {
    /// A squared timber filling the piece.
    fn timber(name: &'static str, at: [f64; 3], size: Vec3) -> Self {
        Self {
            name,
            at,
            rotation: Quat::IDENTITY,
            size,
            look: PropKind::Timber {
                size: size.to_array(),
            },
        }
    }
}

/// The hut (an open timber shelter, so its support runs only through the frame), in build
/// order: posts, top beams, king posts, the ridge and the roof.
fn hut_plan() -> Vec<PieceSpec> {
    let mut v = Vec::new();
    let post = Vec3::new(0.22, 2.6, 0.22);
    for (x, z) in [(-1.9, -1.9), (1.9, -1.9), (1.9, 1.9), (-1.9, 1.9)] {
        v.push(PieceSpec::timber("Hut post", [x, 1.3, z], post));
    }
    for z in [-1.9, 1.9] {
        v.push(PieceSpec::timber(
            "Hut beam",
            [0.0, 2.7, z],
            Vec3::new(4.2, 0.2, 0.2),
        ));
    }
    for x in [-1.9, 1.9] {
        v.push(PieceSpec::timber(
            "Hut beam",
            [x, 2.7, 0.0],
            Vec3::new(0.2, 0.2, 4.2),
        ));
    }
    // A ridge post on each gable and the ridge beam.
    for z in [-1.9, 1.9] {
        v.push(PieceSpec::timber(
            "Hut king post",
            [0.0, 3.35, z],
            Vec3::new(0.18, 1.1, 0.18),
        ));
    }
    v.push(PieceSpec::timber(
        "Hut ridge",
        [0.0, 3.95, 0.0],
        Vec3::new(0.2, 0.2, 4.4),
    ));
    // Two shingled roof panels from the ridge down past the side beams.
    for s in [-1.0f64, 1.0] {
        let size = Vec3::new(2.6, 0.12, 4.6);
        v.push(PieceSpec {
            name: "Hut roof",
            at: [s * 1.05, 3.35, 0.0],
            rotation: Quat::from_rotation_z(-s as f32 * 0.58),
            size,
            look: PropKind::ShinglePanel {
                size: size.to_array(),
                eave_at_plus_x: s > 0.0,
            },
        });
    }
    v
}

/// One piece a hill builder puts down: what it picks, how it turns it, and where it aims
/// (from `from` at `target`, relative to its [`Site`]); `center` is where the piece should
/// snap.
pub struct BuildStep {
    pub kind: BuildKind,
    pub yaw: f32,
    pub from: DVec3,
    pub target: DVec3,
    pub center: DVec3,
    /// Placed (the last step only shows its ghost).
    pub place: bool,
}

/// The yard's cabin in build order: a 4 × 4 m stone plinth, corner posts, board walls (a
/// doorway left in the front), a loft floor on the walls, a 1-in-2 shingle roof, and a deck
/// reaching out from the +X gable two rows deep, then the ghost of a third row.
pub fn yard_plan() -> Vec<BuildStep> {
    let v = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
    let mut steps = Vec::new();
    let mut add = |kind, yaw, from: DVec3, target: DVec3, center: DVec3| {
        steps.push(BuildStep {
            kind,
            yaw,
            from,
            target,
            center,
            place: true,
        });
    };
    let quarters = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)];
    for (x, z) in quarters {
        // Aimed at the ground, from above and to the front.
        let at = v(x, 0.0, z);
        add(
            BuildKind::StoneFloor,
            0.0,
            at + v(0.4, 2.6, 0.8),
            at,
            v(x, 0.2, z),
        );
    }
    for (x, z) in [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)] {
        // Aimed at the plinth just inside its corner.
        let at = v(x * 0.95, 0.4, z * 0.95);
        add(
            BuildKind::WoodPole,
            0.0,
            at + v(-x * 0.2, 2.6, -z * 0.2),
            at,
            v(x, 1.4, z),
        );
    }
    // Walls, aimed at the plinth just inside its edge: back, sides, the front's left half,
    // each turned so its battens are inside.
    for x in [-1.0, 1.0] {
        let at = v(x, 0.4, -1.85);
        add(
            BuildKind::WoodWall,
            PI,
            at + v(0.3, 2.6, 0.95),
            at,
            v(x, 1.4, -2.0),
        );
    }
    for side in [-1.0, 1.0] {
        for z in [-1.0, 1.0] {
            let at = v(side * 1.85, 0.4, z);
            add(
                BuildKind::WoodWall,
                side as f32 * FRAC_PI_2,
                at + v(-side * 0.95, 2.6, 0.3),
                at,
                v(side * 2.0, 1.4, z),
            );
        }
    }
    let at = v(-1.0, 0.4, 1.85);
    add(
        BuildKind::WoodWall,
        0.0,
        at + v(0.3, 2.6, -0.95),
        at,
        v(-1.0, 1.4, 2.0),
    );
    // The loft floor, aimed at the inside of a wall just under its top.
    for (from, at, center) in [
        (
            v(-1.0, 2.25, -0.45),
            v(-1.0, 2.25, -1.95),
            v(-1.0, 2.35, -1.0),
        ),
        (v(1.0, 2.25, -0.45), v(1.0, 2.25, -1.95), v(1.0, 2.35, -1.0)),
        (v(-1.0, 2.25, 0.45), v(-1.0, 2.25, 1.95), v(-1.0, 2.35, 1.0)),
        (v(0.45, 2.25, 1.0), v(1.95, 2.25, 1.0), v(1.0, 2.35, 1.0)),
    ] {
        add(BuildKind::WoodFloor, 0.0, from, at, center);
    }
    // The roof, aimed at the loft floor: each piece's eave lands on the wall line and the two
    // slopes meet over the middle.
    for (side, yaw) in [(-1.0, FRAC_PI_2), (1.0, -FRAC_PI_2)] {
        for x in [-1.0, 1.0] {
            let at = v(x, 2.4, side);
            let rotation = BuildKind::WoodRoof.rotation(yaw);
            let half = BuildKind::WoodRoof.size() * 0.5;
            // Top face: eave at the wall line at loft height, ridge 1 m up over the middle.
            let top_mid = v(x, 2.9, side);
            let center = top_mid - (rotation * Vec3::Y * half.y).as_dvec3();
            add(
                BuildKind::WoodRoof,
                yaw,
                at + v(0.2, 2.5, -side * 0.4),
                at,
                center,
            );
        }
    }
    // The deck: off the outside of the +X walls under their tops, then off the deck's edge.
    for z in [-1.0, 1.0] {
        add(
            BuildKind::WoodFloor,
            0.0,
            v(3.55, 2.25, z),
            v(2.05, 2.25, z),
            v(3.0, 2.35, z),
        );
    }
    for z in [-1.0, 1.0] {
        add(
            BuildKind::WoodFloor,
            0.0,
            v(5.5, 2.35, z),
            v(4.0, 2.35, z),
            v(5.0, 2.35, z),
        );
    }
    steps.push(BuildStep {
        kind: BuildKind::WoodFloor,
        yaw: 0.0,
        from: v(7.5, 2.35, -1.0),
        target: v(6.0, 2.35, -1.0),
        center: v(7.0, 2.35, -1.0),
        place: false,
    });
    steps
}

/// The terrace's shelter in build order, on the rock the hoe leveled (y 0 here): a 2 × 4 m
/// stone floor, board walls at both ends, a 1-in-2 shingle roof from end to end and board
/// walls along the back (against the cut bank) under it, open toward the valley (+X).
pub fn terrace_plan() -> Vec<BuildStep> {
    let v = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
    let mut steps = Vec::new();
    let mut add = |kind, yaw, from: DVec3, target: DVec3, center: DVec3| {
        steps.push(BuildStep {
            kind,
            yaw,
            from,
            target,
            center,
            place: true,
        });
    };
    for z in [-1.0, 1.0] {
        // Aimed at the bare rock, from above and to the front.
        let at = v(0.0, 0.0, z);
        add(
            BuildKind::StoneFloor,
            0.0,
            at + v(0.4, 2.6, 0.8),
            at,
            v(0.0, 0.2, z),
        );
    }
    // The end walls, aimed at the floor just inside its edge, battens inside.
    for (side, yaw) in [(-1.0, PI), (1.0, 0.0)] {
        let at = v(0.0, 0.4, side * 1.85);
        add(
            BuildKind::WoodWall,
            yaw,
            at + v(0.3, 2.6, -side * 0.95),
            at,
            v(0.0, 1.4, side * 2.0),
        );
    }
    // The roof, aimed at the inside of each end wall just under its top: the eave lands on
    // the wall and the two slopes meet over the middle. (With the back walls up first, the
    // roof's gable edge would snap onto their tops instead.)
    for (side, yaw) in [(-1.0, FRAC_PI_2), (1.0, -FRAC_PI_2)] {
        let rotation = BuildKind::WoodRoof.rotation(yaw);
        let half = BuildKind::WoodRoof.size() * 0.5;
        let top_mid = v(0.0, 2.9, side);
        add(
            BuildKind::WoodRoof,
            yaw,
            v(0.3, 2.6, side * 0.5),
            v(0.0, 2.38, side * 1.95),
            top_mid - (rotation * Vec3::Y * half.y).as_dvec3(),
        );
    }
    // Then the back walls under the roof's gable, against the cut bank, aimed from inside.
    for z in [-1.0, 1.0] {
        let at = v(-0.85, 0.4, z);
        add(
            BuildKind::WoodWall,
            -FRAC_PI_2,
            at + v(1.45, 1.5, 0.2),
            at,
            v(-1.0, 1.4, z),
        );
    }
    steps
}

fn hill_volume() -> VoxelVolume {
    let [w, h, d] = HILL_SIZE;
    VoxelVolume::hill(w, h, d, HILL_VOXEL).with_seed(23)
}

/// Build the dig-and-build hill around `origin` (ground level). It is about 34 × 30 m with the
/// hut beside its −Z/+X foot; everything fits in a 26 m radius.
pub fn spawn_voxel_hill(commands: &mut Commands, origin: DVec3) {
    let hill = commands
        .spawn((
            Name::new("Voxel hill"),
            Transform::from_translation(origin),
            hill_volume(),
        ))
        .id();
    let builders = Site::ALL.map(|site| {
        let at = origin + site.offset();
        Some(
            commands
                .spawn((
                    Name::new(site.builder_name()),
                    Transform::from_translation(at),
                    idle_builder(at),
                ))
                .id(),
        )
    });
    commands.spawn((
        Name::new("Hill demo"),
        HillDemo {
            origin,
            hill: Some(hill),
            builders,
            ..Default::default()
        },
    ));
}

/// A site's builder between loops: in build mode, aiming at the sky (no ghost), the pieces in
/// their own colors until the building is done.
fn idle_builder(at: DVec3) -> Builder {
    let mut b = Builder {
        reach_m: 6.0,
        show_support: false,
        ..Builder::default()
    };
    // From high above, so the terrace's builder is not inside the hill before it is dug.
    b.aim_at(at + DVec3::Y * 20.0, at + DVec3::Y * 21.0);
    b
}

/// Advance every [`HillDemo`] (FixedUpdate, `SimSet::Gameplay`).
pub fn hill_script_system(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<HillDemo>>()
        .iter(world)
        .collect();
    let steps = timeline();
    for e in demos {
        let Some(mut demo) = world.get::<HillDemo>(e).cloned() else {
            continue;
        };
        demo.time += dt;
        while demo.next < steps.len() && steps[demo.next].0 <= demo.time {
            let step = steps[demo.next].1;
            demo.next += 1;
            run_step(world, &mut demo, step);
        }
        if demo.time >= HILL_CYCLE_SECS {
            demo.time -= HILL_CYCLE_SECS;
            demo.next = 0;
            demo.cycle += 1;
        }
        if let Some(mut d) = world.get_mut::<HillDemo>(e) {
            *d = demo;
        }
    }
}

/// Start every [`HillDemo`] over: the hill and the hut are rebuilt. Returns how many restarted.
pub fn restart_hills(world: &mut World) -> usize {
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<HillDemo>>()
        .iter(world)
        .collect();
    for &e in &demos {
        let Some(mut demo) = world.get::<HillDemo>(e).cloned() else {
            continue;
        };
        run_step(world, &mut demo, Step::Reset);
        demo.time = 0.0;
        demo.next = 0;
        demo.cycle += 1;
        if let Some(mut d) = world.get_mut::<HillDemo>(e) {
            *d = demo;
        }
    }
    demos.len()
}

fn edit(world: &mut World, center: DVec3, shape: EditShape, op: EditOp, surface: Option<&str>) {
    let surface = surface.and_then(|name| {
        world
            .get_resource::<SurfaceMaterials>()
            .and_then(|s| s.id(name))
    });
    world.write_message(TerrainEdit {
        center,
        shape,
        op,
        surface,
        source: None,
    });
}

fn run_step(world: &mut World, demo: &mut HillDemo, step: Step) {
    let o = demo.origin;
    match step {
        Step::Build(k) => {
            let plan = hut_plan();
            let Some(spec) = plan.get(k) else {
                return;
            };
            let at = o + DVec3::from_array(HUT) + DVec3::from_array(spec.at);
            let t = Transform::from_translation(at).with_rotation(spec.rotation);
            let mut piece = StructuralPiece::new(DestructibleMaterial::Wood, spec.size);
            piece.break_energy_j = 6000.0;
            let e = world
                .spawn((
                    Name::new(spec.name),
                    t,
                    GlobalTransform::from(t),
                    PropModel::new(spec.look, k as u64),
                    piece,
                    ShowSupport {
                        on: true,
                        children: true,
                    },
                ))
                .id();
            demo.hut.push(e);
        }
        Step::Tunnel(k) => {
            // Into the rocky −Z side, a pick's reach at a time.
            let z = -10.5 + k as f64 * 0.6;
            edit(
                world,
                o + DVec3::new(-4.0, 1.25, z),
                EditShape::Sphere { radius: 1.15 },
                EditOp::Remove,
                None,
            );
        }
        Step::Chamber(k) => {
            // Widen the tunnel's end into a hall too wide for its roof (about 9 × 7 m).
            let (col, row) = ((k % 5) as f64, (k / 5) as f64);
            edit(
                world,
                o + DVec3::new(-7.4 + col * 1.4, 1.8, -3.4 + row * 1.8),
                EditShape::Sphere { radius: 1.8 },
                EditOp::Remove,
                None,
            );
        }
        Step::Raise(k) => {
            // Fill the low side of a terrace on the +X flank with soil, a hoe stroke at a time.
            let x = 7.0 + (k % 3) as f64 * 1.8;
            let z = 1.0 + (k / 3) as f64 * 2.2;
            edit(
                world,
                o + DVec3::new(x, TERRACE_Y - 0.9, z),
                EditShape::Box {
                    half_extents: Vec3::new(1.0, 0.9, 1.2),
                    rotation: Quat::IDENTITY,
                },
                EditOp::Add,
                Some("dirt"),
            );
        }
        Step::Level => {
            // Cut the high side down to the same level: cut and fill, one flat terrace.
            edit(
                world,
                o + DVec3::new(8.8, TERRACE_Y + 2.5, 2.1),
                EditShape::Box {
                    half_extents: Vec3::new(3.0, 2.5, 3.4),
                    rotation: Quat::IDENTITY,
                },
                EditOp::Remove,
                None,
            );
        }
        Step::Smooth => {
            // Round the terrace's edges into banks.
            edit(
                world,
                o + DVec3::new(8.8, TERRACE_Y, 2.1),
                EditShape::Sphere { radius: 4.4 },
                EditOp::Smooth { strength: 1.0 },
                None,
            );
        }
        Step::Pad => {
            // Smoothing rounds the terrace into a ramp; the floor needs it flat again where it
            // stands: cut the high side down and fill the low side up, leaving a low cut bank
            // behind the shelter.
            let [hx, hz] = PAD_HALF;
            let at = o + DVec3::from_array(TERRACE);
            for (dy, half_y, op) in [(2.0, 2.0, EditOp::Remove), (-1.2, 1.2, EditOp::Add)] {
                edit(
                    world,
                    at + DVec3::new(0.0, dy, 0.0),
                    EditShape::Box {
                        half_extents: Vec3::new(hx as f32, half_y, hz as f32),
                        rotation: Quat::IDENTITY,
                    },
                    op,
                    (op == EditOp::Add).then_some("dirt"),
                );
            }
        }
        Step::KnockPost(k) => {
            // Both posts on the −X side: that side's beam, the ridge and the roof panel over it
            // lose their path to the ground and fall; the +X half stands.
            let Some(&post) = demo.hut.get([0, 3][k.min(1)]) else {
                return;
            };
            let Some(t) = world.get::<Transform>(post).copied() else {
                return;
            };
            world.write_message(DamageEvent {
                target: post,
                amount: 400.0,
                kind: DamageKind::Blunt,
                point: t.translation + DVec3::new(0.0, -0.5, -0.1),
                impulse: Vec3::new(0.0, 0.0, 60.0),
                source: None,
            });
        }
        Step::Aim(site, k) => {
            let plan = site.plan();
            let (Some(step), Some(b)) = (plan.get(k), demo.builders[site as usize]) else {
                return;
            };
            let at = o + site.offset();
            if let Some(mut builder) = world.get_mut::<Builder>(b) {
                builder.kind = step.kind;
                builder.yaw = step.yaw;
                builder.aim_at(at + step.from, at + step.target);
            }
        }
        Step::ShowSupport(site) => {
            if let Some(mut builder) =
                demo.builders[site as usize].and_then(|b| world.get_mut::<Builder>(b))
            {
                builder.show_support = true;
            }
        }
        Step::Place(site) => {
            if let Some(b) = demo.builders[site as usize] {
                world.write_message(BuildRequest {
                    builder: b,
                    action: BuildAction::Place,
                });
            }
        }
        Step::Reset => {
            for site in Site::ALL {
                let Some(b) = demo.builders[site as usize] else {
                    continue;
                };
                let built: Vec<Entity> = world
                    .query::<(Entity, &BuiltPiece)>()
                    .iter(world)
                    .filter(|(_, p)| p.builder == Some(b))
                    .map(|(e, _)| e)
                    .collect();
                for e in built {
                    world.despawn(e);
                }
                if let Some(mut builder) = world.get_mut::<Builder>(b) {
                    *builder = idle_builder(o + site.offset());
                }
            }
            if let Some(h) = demo.hill
                && let Some(mut v) = world.get_mut::<VoxelVolume>(h)
            {
                v.set_changed();
            }
            for e in demo.hut.drain(..) {
                if world.get::<Debris>(e).is_none() && world.get_entity(e).is_ok() {
                    world.despawn(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{BuildProblem, ScenePiece, ground_hit, nearest_hit, plan};
    use crate::material::DestructionMaterials;
    use crate::structure::{Obb, build_graph, solve_support};

    /// Build a site's plan on flat ground at y 0, checking each aim hits its target and each
    /// piece snaps where planned; the support level of each placed piece.
    fn build_levels(steps: &[BuildStep]) -> Vec<usize> {
        let table = DestructionMaterials::default();
        let ground = |_: DVec3| Some((0.0, Vec3::Y));
        let mut scene: Vec<ScenePiece> = Vec::new();
        let mut levels = Vec::new();
        for (k, step) in steps.iter().enumerate() {
            let dir = (step.target - step.from).as_vec3().normalize();
            let surface = ground_hit(&ground, step.from, dir, 6.0);
            let hit = nearest_hit(&scene, step.from, dir, 6.0, surface)
                .unwrap_or_else(|| panic!("step {k} hits nothing"));
            assert!(
                hit.point.distance(step.target) < 0.02,
                "step {k} aims at {:?} but hits {:?}",
                step.target,
                hit.point
            );
            let p = plan(step.kind, step.yaw, &hit, &scene, &|_| 0.0, &table);
            assert!(
                p.center.distance(step.center) < 1e-3,
                "step {k} ({:?}) lands at {:?}, not {:?}",
                step.kind,
                p.center,
                step.center
            );
            if !step.place {
                assert_eq!(p.problem, Some(BuildProblem::TooWeak), "{p:?}");
                break;
            }
            assert!(p.valid(), "step {k}: {p:?}");
            levels.push(p.level());
            scene.push(ScenePiece {
                entity: Entity::from_raw_u32(k as u32 + 1).unwrap(),
                obb: p.obb(),
                support: 0.0,
                built: true,
            });
            // Supports as the solver will see them.
            let items: Vec<(Entity, Obb, bool)> =
                scene.iter().map(|s| (s.entity, s.obb, false)).collect();
            let graph = build_graph(&items, |_| 0.0);
            let props: Vec<_> = steps[..scene.len()]
                .iter()
                .map(|s| table.get(s.kind.material()))
                .collect();
            for (s, support) in scene.iter_mut().zip(solve_support(&graph, &props)) {
                s.support = support;
            }
        }
        levels
    }

    #[test]
    fn the_yard_cabin_snaps_together_as_planned() {
        let levels = build_levels(&yard_plan());
        // Blue plinth, the deck's second row orange at the edge of support.
        assert!(levels[..4].iter().all(|l| *l == 0), "{levels:?}");
        assert_eq!(levels[levels.len() - 1], 3, "{levels:?}");
        assert_eq!(levels.len(), 27);
    }

    #[test]
    fn the_terrace_shelter_snaps_together_as_planned() {
        let levels = build_levels(&terrace_plan());
        // A blue floor on the rock, green walls and roof.
        assert_eq!(levels, vec![0, 0, 1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn the_plan_builds_from_the_ground_up() {
        let plan = hut_plan();
        assert!(plan.len() >= 11);
        // Posts first, so every later piece has something to rest on.
        assert!(plan[..4].iter().all(|p| p.name == "Hut post"));
        let steps = timeline();
        assert!(steps.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(steps.last().unwrap().0 < HILL_CYCLE_SECS);
    }
}

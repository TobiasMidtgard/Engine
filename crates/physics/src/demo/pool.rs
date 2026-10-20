//! The pool table: a 9 ft table with six pockets, fifteen balls racked at the foot spot and the
//! cue ball breaking them from the head string. The balls step in fine substeps
//! ([`crate::FineSteps`]): the break passes through a tight rack, and a cue ball at 8 m/s moves
//! farther than its own width in an ordinary 8 ms substep.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_world::physics::{Collider, ColliderShape};
use engine_world::{Deformation, SurfaceId, SurfaceMaterial, SurfaceMaterials};

use super::Yard;
use super::art::{Mat, pool as art};

/// A pool ball's radius (57.15 mm across), meters.
pub const BALL: f32 = 0.028575;
/// A pool ball's density: 170 g in a 57.15 mm ball, kg/m³.
const BALL_DENSITY: f32 = 1740.0;
/// Half the playing surface (2.54 × 1.27 m, a 9 ft table), meters.
pub const HALF: [f32; 2] = [1.27, 0.635];
/// The cloth's height above the yard, meters.
pub const BED: f32 = 0.79;
/// How far the cushions reach out from the playing surface, and how high their tops stand
/// above the cloth (the nose, where a ball hits, is 63% of its height up).
const CUSHION: f32 = 0.05;
const CUSHION_TOP: f32 = 0.04;
/// The wooden rails around the cushions: width, and their tops above the cloth.
const RAIL: f32 = 0.13;
const RAIL_TOP: f32 = 0.05;
/// Where the cushions stop short of a corner (along each rail) and of a side pocket's
/// middle: the pockets' mouths.
const CORNER_JAW: f32 = 0.08;
const SIDE_JAW: f32 = 0.068;
/// How far the pocket holes reach into the bed from the corners, and from the long sides at
/// the side pockets.
const CORNER_HOLE: f32 = 0.06;
const SIDE_HOLE: f32 = 0.035;
/// How far the notches cut into the rails reach from the pockets' centers, meters.
pub const NOTCH: f32 = 0.07;
/// How high the hidden walls round the playing surface stand above the cloth.
const WALL_TOP: f32 = 0.2;
/// Where pocketed balls land, below the cloth.
const CATCH: f32 = 0.16;
/// Between two racked balls: more than the solver's 2 mm contact prediction.
const RACK_GAP: f32 = 0.0025;
/// How far each racked ball sits off its place at most (no rack is perfect), meters.
const RACK_SLOP: f32 = 0.001;
/// The balls' substep, seconds. In the default 1 ms the rack's rows still meet as lumps and a
/// second after the break the balls have a third of the energy they have in 0.5 ms, where the
/// blow passes from ball to ball.
const SUBSTEP: f32 = 1.0 / 1920.0;
/// The break: the cue ball's speed, m/s (a firm break), and its aim off the rack's apex.
const BREAK_SPEED: f32 = 8.0;
const BREAK_AIM: f32 = 0.004;

/// What a box of the table is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// The slate under the cloth.
    Bed,
    /// A cloth-covered rubber cushion.
    Cushion,
    /// A wooden rail.
    Rail,
    /// Hidden: the floor pocketed balls land on, and the walls around it.
    Catch,
}

/// The table's boxes (center relative to the middle of the cloth, half extents, part).
pub fn boxes() -> Vec<(Vec3, Vec3, Part)> {
    let [l, w] = HALF;
    let mut out = Vec::new();
    let mut push = |x0: f32, x1: f32, y0: f32, y1: f32, z0: f32, z1: f32, part| {
        out.push((
            Vec3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, (z0 + z1) * 0.5),
            Vec3::new((x1 - x0) * 0.5, (y1 - y0) * 0.5, (z1 - z0) * 0.5),
            part,
        ))
    };
    let (e, bed) = (CUSHION, 0.05);
    // The bed: a band down the middle, and strips along the long sides between the pockets.
    push(
        -l - e,
        l + e,
        -bed,
        0.0,
        -(w - CORNER_HOLE),
        w - CORNER_HOLE,
        Part::Bed,
    );
    for s in [-1.0f32, 1.0] {
        let (z0, z1) = (w - CORNER_HOLE, w + e);
        let (z0, z1) = if s > 0.0 { (z0, z1) } else { (-z1, -z0) };
        push(-(l - CORNER_HOLE), -SIDE_JAW, -bed, 0.0, z0, z1, Part::Bed);
        push(SIDE_JAW, l - CORNER_HOLE, -bed, 0.0, z0, z1, Part::Bed);
        let (z0, z1) = (w - CORNER_HOLE, w - SIDE_HOLE);
        let (z0, z1) = if s > 0.0 { (z0, z1) } else { (-z1, -z0) };
        push(-SIDE_JAW, SIDE_JAW, -bed, 0.0, z0, z1, Part::Bed);
    }
    // Cushions: two on each long rail, one on each short rail.
    for s in [-1.0f32, 1.0] {
        let (z0, z1) = if s > 0.0 { (w, w + e) } else { (-w - e, -w) };
        push(
            -(l - CORNER_JAW),
            -SIDE_JAW,
            0.0,
            CUSHION_TOP,
            z0,
            z1,
            Part::Cushion,
        );
        push(
            SIDE_JAW,
            l - CORNER_JAW,
            0.0,
            CUSHION_TOP,
            z0,
            z1,
            Part::Cushion,
        );
        let (x0, x1) = if s > 0.0 { (l, l + e) } else { (-l - e, -l) };
        push(
            x0,
            x1,
            0.0,
            CUSHION_TOP,
            -(w - CORNER_JAW),
            w - CORNER_JAW,
            Part::Cushion,
        );
    }
    // Rails all the way round, with a notch cut round each pocket: an inner strip broken at
    // the pockets and an unbroken outer strip behind them.
    let (ol, ow) = (l + e + RAIL, w + e + RAIL);
    let [(cx, cz), (_, sz)] = [pockets()[5], pockets()[4]];
    let (z_split, x_split) = (sz + NOTCH, cx + NOTCH);
    for s in [-1.0f32, 1.0] {
        let side = |a: f32, b: f32| if s > 0.0 { (a, b) } else { (-b, -a) };
        let (z0, z1) = side(w + e, z_split);
        push(-(cx - NOTCH), -NOTCH, -0.2, RAIL_TOP, z0, z1, Part::Rail);
        push(NOTCH, cx - NOTCH, -0.2, RAIL_TOP, z0, z1, Part::Rail);
        let (z0, z1) = side(z_split, ow);
        push(-ol, ol, -0.2, RAIL_TOP, z0, z1, Part::Rail);
        let (x0, x1) = side(l + e, x_split);
        push(
            x0,
            x1,
            -0.2,
            RAIL_TOP,
            -(cz - NOTCH),
            cz - NOTCH,
            Part::Rail,
        );
        let (x0, x1) = side(x_split, ol);
        push(x0, x1, -0.2, RAIL_TOP, -z_split, z_split, Part::Rail);
    }
    // Hidden walls over the cushions' noses and the rails' inner edges, so a ball that jumps
    // in the break comes back onto the cloth rather than riding the rails off the table.
    let (top, t) = (WALL_TOP, 0.005);
    for s in [-1.0f32, 1.0] {
        let (z0, z1) = if s > 0.0 { (w, w + t) } else { (-w - t, -w) };
        push(
            -(l - CORNER_JAW),
            -SIDE_JAW,
            CUSHION_TOP,
            top,
            z0,
            z1,
            Part::Catch,
        );
        push(
            SIDE_JAW,
            l - CORNER_JAW,
            CUSHION_TOP,
            top,
            z0,
            z1,
            Part::Catch,
        );
        let (x0, x1) = if s > 0.0 { (l, l + t) } else { (-l - t, -l) };
        push(
            x0,
            x1,
            CUSHION_TOP,
            top,
            -(w - CORNER_JAW),
            w - CORNER_JAW,
            Part::Catch,
        );
        let (z0, z1) = if s > 0.0 {
            (w + e, w + e + t)
        } else {
            (-w - e - t, -w - e)
        };
        push(-l - e - t, l + e + t, RAIL_TOP, top, z0, z1, Part::Catch);
        let (x0, x1) = if s > 0.0 {
            (l + e, l + e + t)
        } else {
            (-l - e - t, -l - e)
        };
        push(x0, x1, RAIL_TOP, top, -(w + e), w + e, Part::Catch);
    }
    // The catch floor under the pockets.
    push(-ol, ol, -CATCH - 0.04, -CATCH, -ow, ow, Part::Catch);
    out
}

/// Pocket centers relative to the middle of the cloth (x, z): four corners and two sides.
pub fn pockets() -> [(f32, f32); 6] {
    let [l, w] = HALF;
    let (c, s) = (CUSHION * 0.45, w + CUSHION * 0.55);
    [
        (-l - c, -w - c),
        (0.0, -s),
        (l + c, -w - c),
        (-l - c, w + c),
        (0.0, s),
        (l + c, w + c),
    ]
}

/// Where the balls start, relative to the middle of the cloth: the cue ball on the head
/// string, then the fifteen in a triangle with its apex on the foot spot (the 8 in the
/// middle of the third row), as `(ball number, x, z)`.
pub fn rack() -> Vec<(u8, f32, f32)> {
    let [l, _] = HALF;
    let pitch = 2.0 * BALL + RACK_GAP;
    let row = pitch * 3f32.sqrt() * 0.5;
    // A legal 8-ball rack: a solid and a stripe in the back corners, the 8 in the middle.
    let order = [1, 9, 2, 10, 8, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15];
    let mut out = vec![(0, -l * 0.5, 0.0)];
    let mut rng = super::Rng(0x8_ba11);
    let mut k = 0;
    for r in 0..5 {
        for i in 0..=r {
            let z = (i as f32 - r as f32 * 0.5) * pitch;
            let mut slop = || (rng.next() - 0.5) * 2.0 * RACK_SLOP;
            let (jx, jz) = (slop(), slop());
            out.push((order[k], l * 0.5 + r as f32 * row + jx, z + jz));
            k += 1;
        }
    }
    out
}

fn surface(y: &mut Yard, name: &str, make: impl FnOnce() -> SurfaceMaterial) -> SurfaceId {
    let mut table = y.world.get_resource_or_init::<SurfaceMaterials>();
    match table.id(name) {
        Some(id) => id,
        None => table.add(make()),
    }
}

/// The pool table with its legs on the yard at `base`, the balls racked, and the cue ball
/// already rolling at the rack.
pub(super) fn pool_table(y: &mut Yard, base: DVec3) {
    let cloth = surface(y, "billiard_cloth", || SurfaceMaterial {
        name: "billiard_cloth".into(),
        friction: 0.2,
        restitution: 0.5,
        density: 1300.0,
        // Hard and fast: the base rolling resistance (0.01) alone, so a rolling ball slows by
        // about 0.1 m/s².
        deformation: Deformation::None,
        footstep_sound: "footstep.fabric".into(),
        impact_effect: "impact.fabric".into(),
        ..Default::default()
    });
    let rubber = surface(y, "cushion_rubber", || SurfaceMaterial {
        name: "cushion_rubber".into(),
        friction: 0.2,
        restitution: 0.85,
        density: 1100.0,
        footstep_sound: "footstep.rubber".into(),
        impact_effect: "impact.rubber".into(),
        ..Default::default()
    });
    let phenolic = surface(y, "phenolic_resin", || SurfaceMaterial {
        name: "phenolic_resin".into(),
        friction: 0.06,
        restitution: 0.94,
        density: BALL_DENSITY,
        footstep_sound: "footstep.plastic".into(),
        impact_effect: "impact.plastic".into(),
        ..Default::default()
    });
    let top = base + DVec3::Y * BED as f64;
    for (at, half, part) in boxes() {
        let surface = match part {
            Part::Bed => cloth,
            Part::Cushion => rubber,
            Part::Rail | Part::Catch => y.wood,
        };
        y.fixed(
            top + at.as_dvec3(),
            Quat::IDENTITY,
            ColliderShape::Cuboid { half_extents: half },
            surface,
        );
    }
    let table = y.parts(
        "pool_table",
        [
            Mat::Felt(art::CLOTH),
            Mat::Lacquer(art::MAHOGANY),
            Mat::Lacquer(art::POCKET),
            Mat::Steel,
            Mat::Lacquer(0xf2ede0),
        ],
        || {
            let t = art::table(&boxes(), &pockets(), BED, HALF);
            [t.cloth, t.wood, t.pockets, t.steel, t.inlay]
        },
    );
    y.scenery(top, Quat::IDENTITY, table);
    for (number, x, z) in rack() {
        let parts = y.parts(
            &format!("pool_ball/{number}"),
            [Mat::Lacquer(0xffffff)],
            || [art::ball(BALL, number)],
        );
        let c = Collider::sphere(BALL)
            .with_surface(phenolic)
            .with_density(BALL_DENSITY);
        // Numbers face the camera (+z), turned a little each so they don't look stamped.
        let turn = Quat::from_rotation_y(number as f32 * 0.7);
        let at = top + DVec3::new(x as f64, (BALL + 0.0005) as f64, z as f64);
        let ball = y.body(at, turn, c, parts);
        y.world.entity_mut(ball).insert(crate::FineSteps {
            substep_secs: SUBSTEP,
        });
        if number == 0 {
            let dir = Vec3::new(1.0, 0.0, BREAK_AIM).normalize();
            y.world
                .get_mut::<engine_world::physics::Velocity>(ball)
                .unwrap()
                .linear = dir * BREAK_SPEED;
            y.name(ball, "Cue ball");
        } else {
            y.name(ball, &format!("Pool ball {number}"));
        }
    }
}

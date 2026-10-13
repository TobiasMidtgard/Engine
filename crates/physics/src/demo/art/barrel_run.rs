//! Meshes for the barrel run: a cast concrete launch ramp topped with asphalt, a 47 m asphalt
//! runway with three painted lanes between galvanized W-beam guardrails, a raked sand pit to
//! stop the barrels and a wall of stacked tires behind it. The demo's colliders come from the
//! same layout ([`deck_profile`], [`deck_collider`] and the constants below), so what the
//! barrels roll on is what is drawn.
//!
//! Station frame: x along the run (barrels roll toward +x), y up from the yard's top, z across.
//! The run is symmetric about z = 0.

use std::f32::consts::{FRAC_PI_2, TAU};

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::kit::{MeshBuilder, srgb};
use super::structures::{
    at, bar, dome_head, fillet_path, grey, grid_colored, hash, hex_nut, i_beam, noise1, plate,
    rot_at, sweep, washer, y_to,
};

/// Lane spacing (m): the lanes' centers are at z = -LANE, 0 and LANE.
pub const LANE: f32 = 1.8;
/// Half the deck's width (m).
pub const HALF_WIDTH: f32 = 2.8;
/// The runway's top above the yard (m): 4 cm of asphalt.
pub const DECK_Y: f32 = 0.04;
/// The ramp's top above the yard (m).
pub const RAMP_HEIGHT: f32 = 2.0;
/// The back of the ramp's platform.
pub const X_BACK: f32 = -34.2;
/// The ramp's crest, where the slope starts (before the fillet rounds it).
pub const X_CREST: f32 = -31.8;
/// The ramp's foot, where the slope meets the runway (before the fillet rounds it).
pub const X_FOOT: f32 = -21.8;
/// Where the asphalt ends and the sand pit starts.
pub const X_PIT: f32 = 25.0;
/// Where the sand pit ends and the tire wall starts.
pub const X_WALL: f32 = 31.4;
/// The tire wall's back.
pub const X_END: f32 = 34.3;
/// The sand's top above the yard (m): 2 cm below the asphalt, so a barrel rolls off the
/// asphalt's edge instead of catching on the pit's.
pub const SAND_Y: f32 = 0.02;
/// How far the ramp's cheek walls stand above the deck, and how thick they are (m).
pub const CHEEK: f32 = 0.45;
pub const CHEEK_T: f32 = 0.25;
/// Where the cheek walls end, past the ramp's foot.
pub const X_CHEEK_END: f32 = X_FOOT + 2.5;
/// The guardrails: the rail's traffic face (|z|), its center height, the post spacing and
/// where the rails start (flared out, then along the deck).
pub const RAIL_Z: f32 = HALF_WIDTH + 0.3;
pub const RAIL_Y: f32 = 0.55;
pub const POST_STEP: f32 = 2.0;
pub const X_RAIL_START: f32 = X_CHEEK_END + 0.5;
/// How far the rails flare out at their start, and over what length.
const FLARE: (f32, f32) = (0.45, 2.0);
/// The tire wall: tires stacked this many high, their radius and width.
pub const TIRE_LAYERS: usize = 6;
pub const TIRE_R: f32 = 0.31;
pub const TIRE_W: f32 = 0.2;
pub const WALL_HEIGHT: f32 = TIRE_LAYERS as f32 * TIRE_W;
/// The wall's half width (z).
pub const WALL_HALF: f32 = RAIL_Z + 0.25;
/// Where the barrels start: on the slope just past the crest's rounding.
pub const X_START: f32 = X_CREST + 1.6;

/// W-beam rail: height, depth (peak to valley) and steel thickness (m).
const W_HEIGHT: f32 = 0.31;
const W_DEPTH: f32 = 0.083;
const W_STEEL: f32 = 0.003;
/// The post and blockout: an I-beam's depth (across the track) and width (along it).
const POST_DEPTH: f32 = 0.152;
const POST_WIDTH: f32 = 0.102;
const BLOCKOUT: f32 = 0.15;

/// The deck's centerline profile (x, y), from the back of the ramp's platform to the pit's
/// edge, its two corners rounded (2 m at the crest, 20 m at the foot), with no more than `step`
/// meters between points.
pub fn deck_profile(step: f32) -> Vec<Vec2> {
    let corners = [
        Vec3::new(X_BACK, RAMP_HEIGHT, 0.0),
        Vec3::new(X_CREST, RAMP_HEIGHT, 0.0),
        Vec3::new(X_FOOT, DECK_Y, 0.0),
        Vec3::new(X_PIT, DECK_Y, 0.0),
    ];
    let round: Vec<Vec2> = fillet_path(&corners, &[2.0, 20.0], 16)
        .into_iter()
        .map(|p| Vec2::new(p.x, p.y))
        .collect();
    let mut out = vec![round[0]];
    for w in round.windows(2) {
        let pieces = (w[0].distance(w[1]) / step).ceil().max(1.0) as usize;
        for k in 1..=pieces {
            out.push(w[0].lerp(w[1], k as f32 / pieces as f32));
        }
    }
    out
}

/// The deck's height at `x` (on the profile; the yard's top beyond it).
pub fn deck_height(profile: &[Vec2], x: f32) -> f32 {
    if x < profile[0].x || x > profile[profile.len() - 1].x {
        return 0.0;
    }
    let i = profile
        .partition_point(|p| p.x < x)
        .clamp(1, profile.len() - 1);
    let (a, b) = (profile[i - 1], profile[i]);
    let t = ((x - a.x) / (b.x - a.x).max(1e-6)).clamp(0.0, 1.0);
    a.y + (b.y - a.y) * t
}

/// The deck's unit normal at `x` (in the xy plane).
fn deck_normal(profile: &[Vec2], x: f32) -> Vec3 {
    let (h0, h1) = (
        deck_height(profile, x - 0.05),
        deck_height(profile, x + 0.05),
    );
    Vec3::new(-(h1 - h0), 0.1, 0.0).normalize()
}

/// A cheek wall's side outline (x, y): the deck's profile raised by `CHEEK`, tapering down to
/// the runway at its end, down to the yard. The walls stand at z = ±(`HALF_WIDTH` + `CHEEK_T`/2).
pub fn cheek_outline(profile: &[Vec2]) -> Vec<Vec2> {
    let taper = X_CHEEK_END - 1.6;
    let mut outline = vec![
        Vec2::new(X_BACK, 0.0),
        Vec2::new(X_CHEEK_END, 0.0),
        Vec2::new(X_CHEEK_END, DECK_Y + 0.14),
        Vec2::new(taper, deck_height(profile, taper) + CHEEK),
    ];
    let top: Vec<Vec2> = profile
        .iter()
        .filter(|p| p.x > X_BACK && p.x < taper)
        .map(|p| Vec2::new(p.x, p.y + CHEEK))
        .collect();
    outline.extend(top.into_iter().rev());
    outline.push(Vec2::new(X_BACK, RAMP_HEIGHT + CHEEK));
    outline
}

/// The ramp's solid body under the deck, for a convex hull (x, y): its top 3 cm under the
/// deck, so the deck's own mesh is the only surface the drums roll on.
pub fn ramp_body_outline() -> [Vec2; 5] {
    let sink = 0.03;
    [
        Vec2::new(X_BACK, 0.0),
        Vec2::new(X_BACK, RAMP_HEIGHT - sink),
        Vec2::new(X_CREST, RAMP_HEIGHT - sink),
        Vec2::new(X_FOOT, (DECK_Y - sink).max(0.0)),
        Vec2::new(X_FOOT, 0.0),
    ]
}

/// The deck's collision mesh: the profile (2 m pieces) across the deck's width, triangles
/// facing up. The quads' diagonals zig-zag: a drum crossing a diagonal turns a hair toward it,
/// and with every diagonal one way the drums all drifted 40 cm the same way over the runway.
pub fn deck_collider() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let profile = deck_profile(2.0);
    let mut vertices = Vec::with_capacity(profile.len() * 2);
    for p in &profile {
        vertices.push(Vec3::new(p.x, p.y, -HALF_WIDTH));
        vertices.push(Vec3::new(p.x, p.y, HALF_WIDTH));
    }
    let indices = (0..profile.len() as u32 - 1)
        .flat_map(|i| {
            let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
            if i % 2 == 0 {
                [[a, b, d], [a, d, c]]
            } else {
                [[a, b, c], [c, b, d]]
            }
        })
        .collect();
    (vertices, indices)
}

/// Where a barrel of `radius` starts in lane `lane` (-1, 0 or 1): its center, resting on the
/// slope.
pub fn barrel_start(lane: i32, radius: f32) -> Vec3 {
    let profile = deck_profile(0.25);
    let n = deck_normal(&profile, X_START);
    Vec3::new(X_START, deck_height(&profile, X_START), lane as f32 * LANE) + n * (radius + 0.005)
}

/// The barrel run's meshes, one per material, in the station frame.
pub struct BarrelRun {
    /// The deck: the ramp's topping and the runway.
    pub asphalt: Mesh,
    /// Cast concrete: the ramp's back wall and cheek walls, the pit's curbs.
    pub concrete: Mesh,
    /// Thermoplastic paint: edge lines, lane dividers, the start bar, distance ticks and lane
    /// numbers.
    pub paint: Mesh,
    /// Galvanized steel: guardrails, posts, blockouts, base plates and bolts, the pit's edge
    /// angle.
    pub galvanized: Mesh,
    /// The raked sand in the pit.
    pub sand: Mesh,
    /// The tire wall (white rubber tinted by vertex colors: black tires, painted rows).
    pub tires: Mesh,
    /// Safety-yellow steel: the start platform's handrail.
    pub yellow: Mesh,
}

pub fn barrel_run() -> BarrelRun {
    let profile = deck_profile(0.25);
    BarrelRun {
        asphalt: asphalt(&profile),
        concrete: concrete(&profile),
        paint: paint(&profile),
        galvanized: galvanized(),
        sand: sand(),
        tires: tires(),
        yellow: handrail(),
    }
}

// ---------------------------------------------------------------------------------------
// Deck
// ---------------------------------------------------------------------------------------

fn asphalt(profile: &[Vec2]) -> Mesh {
    let mut b = MeshBuilder::new();
    // Across: the edges, the edge lines and lane centers get columns of their own.
    let zs = [
        -1.0, -0.97, -0.9, -0.62, -0.32, 0.0, 0.32, 0.62, 0.9, 0.97, 1.0,
    ]
    .map(|f: f32| f * HALF_WIDTH);
    let mut along = vec![0.0f32];
    for w in profile.windows(2) {
        along.push(along.last().unwrap() + w[0].distance(w[1]));
    }
    grid_colored(&mut b, profile.len(), zs.len(), |r, c| {
        let p = profile[r];
        let n = deck_normal(profile, p.x);
        let z = zs[c];
        // Darker where the deck meets the walls and rails (dust, rubber), and in patches.
        let edge = 1.0 - (z.abs() / HALF_WIDTH);
        let dirt = 0.82 + 0.18 * (edge / 0.06).min(1.0);
        let patch = 0.92 + 0.08 * noise1(p.x * 0.35 + z * 0.2, 41);
        (
            Vec3::new(p.x, p.y, z),
            n,
            Vec2::new(along[r], z),
            grey(dirt * patch),
        )
    });
    // The runway's 4 cm sides, past the cheek walls, with a small rounded top edge.
    let side: Vec<Vec2> = profile
        .iter()
        .copied()
        .filter(|p| p.x >= X_CHEEK_END - 0.3)
        .collect();
    for s in [-1.0f32, 1.0] {
        let section = [
            (Vec2::new(-0.012, 0.0), Vec2::Y),
            (Vec2::new(-0.004, -0.003), Vec2::new(0.7, 0.7)),
            (Vec2::new(0.0, -0.012), Vec2::X),
            (Vec2::new(0.0, -DECK_Y), Vec2::X),
        ];
        grid_colored(&mut b, side.len(), section.len(), |r, c| {
            let (q, n) = section[c];
            let p = side[r];
            (
                Vec3::new(p.x, p.y + q.y, s * (HALF_WIDTH + q.x)),
                Vec3::new(0.0, n.y, s * n.x).normalize(),
                Vec2::new(p.x, q.y),
                grey(0.85),
            )
        });
    }
    b.build()
}

// ---------------------------------------------------------------------------------------
// Concrete
// ---------------------------------------------------------------------------------------

fn concrete(profile: &[Vec2]) -> Mesh {
    let mut b = MeshBuilder::new();
    // The ramp's back wall, darker near the ground.
    let zs: Vec<f32> = (0..=8)
        .map(|k| -HALF_WIDTH + 2.0 * HALF_WIDTH * k as f32 / 8.0)
        .collect();
    grid_colored(&mut b, 5, zs.len(), |r, c| {
        let y = RAMP_HEIGHT * r as f32 / 4.0;
        (
            Vec3::new(X_BACK, y, zs[c]),
            -Vec3::X,
            Vec2::new(zs[c], y),
            grey(0.78 + 0.22 * (y / 0.5).min(1.0)),
        )
    });
    let outline = cheek_outline(profile);
    for s in [-1.0f32, 1.0] {
        let z = s * (HALF_WIDTH + CHEEK_T * 0.5);
        b.placed(at(Vec3::new(0.0, 0.0, z)), |b| {
            b.extrude(&outline, CHEEK_T, 0.02, 25.0)
        });
    }
    // The pit's curbs, along both sides of the sand.
    let len = X_WALL - X_PIT;
    for s in [-1.0f32, 1.0] {
        plate(
            &mut b,
            at(Vec3::new(X_PIT + len * 0.5, 0.06, s * (HALF_WIDTH + 0.1))),
            Vec3::new(len * 0.5, 0.06, 0.1),
            0.015,
        );
    }
    b.build()
}

// ---------------------------------------------------------------------------------------
// Paint
// ---------------------------------------------------------------------------------------

/// A painted band on the deck from `x0` to `x1` and `z0` to `z1`, 3 mm thick.
fn band(b: &mut MeshBuilder, profile: &[Vec2], x0: f32, x1: f32, z0: f32, z1: f32, wear: f32) {
    let mut xs = vec![x0];
    xs.extend(profile.iter().map(|p| p.x).filter(|&x| x > x0 && x < x1));
    xs.push(x1);
    let t = 0.003;
    let color = grey(wear);
    // Top, then the two long sides (the ends are too small to see).
    grid_colored(b, xs.len(), 2, |r, c| {
        let x = xs[r];
        let n = deck_normal(profile, x);
        let z = if c == 0 { z0 } else { z1 };
        (
            Vec3::new(x, deck_height(profile, x), z) + n * t,
            n,
            Vec2::new(x, z),
            color,
        )
    });
    for (z, out) in [(z0, -1.0f32), (z1, 1.0)] {
        grid_colored(b, xs.len(), 2, |r, c| {
            let x = xs[r];
            let n = deck_normal(profile, x);
            (
                Vec3::new(x, deck_height(profile, x), z) + n * (t * c as f32),
                Vec3::Z * out,
                Vec2::new(x, t * c as f32),
                color,
            )
        });
    }
}

/// Seven-segment strokes for the lane numbers 1 to 3, as (x0, y0, x1, y1) in a 1 × 2 cell.
fn digit_strokes(d: u32) -> &'static [[f32; 4]] {
    const A: [f32; 4] = [0.0, 1.85, 1.0, 2.0];
    const B: [f32; 4] = [0.85, 1.0, 1.0, 2.0];
    const C: [f32; 4] = [0.85, 0.0, 1.0, 1.0];
    const D: [f32; 4] = [0.0, 0.0, 1.0, 0.15];
    const E: [f32; 4] = [0.0, 0.0, 0.15, 1.0];
    const G: [f32; 4] = [0.0, 0.93, 1.0, 1.08];
    match d {
        1 => &[[0.425, 0.0, 0.575, 2.0]],
        2 => &[A, B, G, E, D],
        _ => &[A, B, G, C, D],
    }
}

fn paint(profile: &[Vec2]) -> Mesh {
    let mut b = MeshBuilder::new();
    let wear = |k: u32| 0.86 + 0.14 * hash(k);
    let (x0, x1) = (X_CREST - 0.6, X_PIT - 0.15);
    // Solid edge lines.
    for s in [-1.0f32, 1.0] {
        let (a, c) = (s * (HALF_WIDTH - 0.27), s * (HALF_WIDTH - 0.15));
        band(&mut b, profile, x0, x1, a.min(c), a.max(c), wear(1));
    }
    // Dashed lane dividers: 2 m on, 2 m off, from the start bar.
    for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        let z = s * LANE * 0.5;
        let mut x = X_CREST + 0.4;
        let mut k = 0;
        while x < x1 {
            let end = (x + 2.0).min(x1);
            band(
                &mut b,
                profile,
                x,
                end,
                z - 0.05,
                z + 0.05,
                wear(10 + 50 * i as u32 + k),
            );
            x += 4.0;
            k += 1;
        }
    }
    // The start bar behind the barrels, across the deck.
    let edge = HALF_WIDTH - 0.27;
    band(
        &mut b,
        profile,
        X_CREST - 0.45,
        X_CREST - 0.25,
        -edge,
        edge,
        wear(3),
    );
    // Distance ticks every 5 m along the runway, inside both edge lines.
    let mut x = (X_FOOT / 5.0).ceil() * 5.0;
    let mut k = 0;
    while x < x1 - 0.5 {
        for s in [-1.0f32, 1.0] {
            let (a, c) = (s * edge, s * (edge - 0.45));
            band(
                &mut b,
                profile,
                x - 0.06,
                x + 0.06,
                a.min(c),
                a.max(c),
                wear(200 + k),
            );
            k += 1;
        }
        x += 5.0;
    }
    // Lane numbers on the runway, 1.4 m long so they read at a low angle, their tops toward
    // the ramp (they face someone at the pit looking back up the run).
    let (len, wide) = (1.4f32, 0.42f32);
    let x_num = X_FOOT + 3.0;
    for lane in -1..=1 {
        let number = (lane + 2) as u32;
        let zc = lane as f32 * LANE;
        for st in digit_strokes(number) {
            // The cell's v (up the digit) runs toward -x and its u (left to right) toward -z:
            // they read the right way round from the pit, looking back up the run.
            let xa = x_num + len * (1.0 - st[3] / 2.0);
            let xb = x_num + len * (1.0 - st[1] / 2.0);
            let za = zc + wide * (0.5 - st[2]);
            let zb = zc + wide * (0.5 - st[0]);
            band(&mut b, profile, xa, xb, za, zb, wear(300 + number));
        }
    }
    b.build()
}

// ---------------------------------------------------------------------------------------
// Guardrails
// ---------------------------------------------------------------------------------------

/// The W-beam's closed cross-section in the sweep frame (side, up), the traffic face at side 0
/// facing `toward` (±1 along side).
fn w_beam_section(toward: f32) -> Vec<(Vec2, Vec2)> {
    let steps = 28;
    // Set-back of the traffic face from the peaks: two waves over the rail's height.
    let depth = |s: f32| 0.5 * W_DEPTH * (1.0 + (2.0 * TAU * s).cos());
    let slope = |s: f32| -0.5 * W_DEPTH * 2.0 * TAU * (2.0 * TAU * s).sin() / W_HEIGHT;
    let front: Vec<(Vec2, Vec2)> = (0..=steps)
        .map(|k| {
            let s = k as f32 / steps as f32;
            let y = (s - 0.5) * W_HEIGHT;
            let n = Vec2::new(toward, slope(s)).normalize();
            (Vec2::new(-toward * depth(s), y), n)
        })
        .collect();
    let mut out = front.clone();
    out.extend(
        front
            .iter()
            .rev()
            .map(|&(p, n)| (p - Vec2::new(toward, 0.0) * W_STEEL, -n)),
    );
    out.push(front[0]);
    out
}

/// One side's rail path (its traffic face): flared out at the start, then along the deck to the
/// tire wall.
fn rail_path(s: f32) -> Vec<Vec3> {
    let (flare, flare_len) = FLARE;
    let corners = [
        Vec3::new(X_RAIL_START, RAIL_Y, s * (RAIL_Z + flare)),
        Vec3::new(X_RAIL_START + flare_len, RAIL_Y, s * RAIL_Z),
        Vec3::new(X_WALL - 0.05, RAIL_Y, s * RAIL_Z),
    ];
    let round = fillet_path(&corners, &[6.0], 8);
    let mut out = vec![round[0]];
    for w in round.windows(2) {
        let pieces = (w[0].distance(w[1]) / 1.0).ceil().max(1.0) as usize;
        for k in 1..=pieces {
            out.push(w[0].lerp(w[1], k as f32 / pieces as f32));
        }
    }
    out
}

/// How far the rail's traffic face is from the deck's center line at `x` (|z|): `RAIL_Z`, more
/// along the flare.
pub fn rail_offset(x: f32) -> f32 {
    let path = rail_path(1.0);
    let i = path.partition_point(|p| p.x < x).clamp(1, path.len() - 1);
    let (a, b) = (path[i - 1], path[i]);
    let t = ((x - a.x) / (b.x - a.x).max(1e-6)).clamp(0.0, 1.0);
    a.z + (b.z - a.z) * t
}

/// Where the rails run straight along the deck (x), past the flare.
pub fn rail_straight() -> (f32, f32) {
    (X_RAIL_START + FLARE.1, X_WALL - 0.05)
}

/// The flare's two ends on the +z side (x, z): the rail's start and where it runs straight.
pub fn rail_flare() -> (Vec2, Vec2) {
    (
        Vec2::new(X_RAIL_START, RAIL_Z + FLARE.0),
        Vec2::new(X_RAIL_START + FLARE.1, RAIL_Z),
    )
}

/// The x of every guardrail post.
pub fn post_xs() -> Vec<f32> {
    let mut xs = Vec::new();
    let mut x = X_RAIL_START + 0.4;
    while x < X_WALL - 0.3 {
        xs.push(x);
        x += POST_STEP;
    }
    xs
}

fn galvanized() -> Mesh {
    let mut b = MeshBuilder::new();
    for s in [-1.0f32, 1.0] {
        let path = rail_path(s);
        // The sweep's side axis is up × along = -z here: traffic (toward z = 0) is +side for
        // the rail at +z.
        b.colored(grey(0.97), |b| {
            sweep(b, &path, false, &w_beam_section(s), Vec3::Y, false)
        });
        for (k, x) in post_xs().into_iter().enumerate() {
            post(&mut b, x, s, k as u32 + if s > 0.0 { 1000 } else { 0 });
        }
    }
    // The pit's edge angle, where the asphalt ends.
    plate(
        &mut b,
        at(Vec3::new(X_PIT + 0.003, DECK_Y - 0.03, 0.0)),
        Vec3::new(0.003, 0.03, HALF_WIDTH),
        0.0015,
    );
    plate(
        &mut b,
        at(Vec3::new(X_PIT - 0.025, DECK_Y - 0.0028, 0.0)),
        Vec3::new(0.028, 0.003, HALF_WIDTH),
        0.0015,
    );
    // Its countersunk anchors.
    for k in 0..12 {
        let z = -HALF_WIDTH + 0.25 + (2.0 * HALF_WIDTH - 0.5) * k as f32 / 11.0;
        dome_head(
            &mut b,
            at(Vec3::new(X_PIT - 0.025, DECK_Y, z)),
            0.009,
            0.003,
            10,
        );
    }
    b.build()
}

/// A guardrail post at `x` on side `s`: an I-beam on a base plate with four anchor nuts, the
/// blockout between it and the rail, and the post bolt through the rail.
fn post(b: &mut MeshBuilder, x: f32, s: f32, seed: u32) {
    let rail_back = rail_offset(x) + W_DEPTH + W_STEEL;
    let z_block = s * (rail_back + BLOCKOUT * 0.5);
    let z_post = s * (rail_back + BLOCKOUT + POST_DEPTH * 0.5);
    let height = RAIL_Y + W_HEIGHT * 0.5 + 0.02;
    let base = 0.016;
    let upright = Quat::from_rotation_x(-FRAC_PI_2);
    let tone = 0.9 + 0.1 * hash(seed);
    b.colored(grey(tone), |b| {
        b.placed(
            rot_at(upright, Vec3::new(x, base + (height - base) * 0.5, z_post)),
            |b| i_beam(b, height - base, POST_DEPTH, POST_WIDTH, 0.008, 0.006),
        );
        b.placed(rot_at(upright, Vec3::new(x, RAIL_Y, z_block)), |b| {
            i_beam(b, 0.36, BLOCKOUT, POST_WIDTH, 0.007, 0.005)
        });
        // The base plate and its anchors.
        plate(
            b,
            at(Vec3::new(x, base * 0.5, z_post)),
            Vec3::new(0.12, base * 0.5, 0.14),
            0.004,
        );
        for (dx, dz) in [(-0.08, -0.1), (0.08, -0.1), (-0.08, 0.1), (0.08, 0.1)] {
            let p = Vec3::new(x + dx, base, z_post + dz);
            washer(b, at(p), 0.009, 0.019, 0.003, 12);
            hex_nut(b, at(p + Vec3::Y * 0.003), 0.024, 0.016);
        }
        // The post bolt: a button head in the rail's middle valley, the nut behind the post's
        // flange.
        let toward = Vec3::Z * -s;
        let face = Vec3::new(x, RAIL_Y, s * (rail_back - W_STEEL));
        dome_head(b, rot_at(y_to(toward), face), 0.014, 0.008, 12);
        let back = Vec3::new(x, RAIL_Y, z_post + s * (POST_DEPTH * 0.5));
        hex_nut(b, rot_at(y_to(-toward), back), 0.024, 0.014);
    });
}

// ---------------------------------------------------------------------------------------
// Sand and tires
// ---------------------------------------------------------------------------------------

fn sand() -> Mesh {
    let mut b = MeshBuilder::new();
    let (cols, rows) = (25usize, 64usize);
    let len = X_WALL - X_PIT;
    let height = |x: f32, z: f32| {
        let t = (x - X_PIT) / len;
        // Soft lumps, sand banked up against the tires, and a little lower along the edges
        // where it was raked from.
        let lumps = 0.006 * (noise1(x * 1.3 + z * 0.4, 61) - 0.5)
            + 0.004 * (noise1(z * 1.7 - x * 0.3, 62) - 0.5);
        let bank = 0.08 * ((t - 0.85) / 0.15).clamp(0.0, 1.0).powi(2);
        SAND_Y + lumps + bank
    };
    let mut pts = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            let x = X_PIT + len * r as f32 / (rows - 1) as f32;
            let z = -HALF_WIDTH + 2.0 * HALF_WIDTH * c as f32 / (cols - 1) as f32;
            pts.push(Vec3::new(x, height(x, z), z));
        }
    }
    let e = 0.05;
    grid_colored(&mut b, rows, cols, |r, c| {
        let p = pts[r * cols + c];
        let dx = height(p.x + e, p.z) - height(p.x - e, p.z);
        let dz = height(p.x, p.z + e) - height(p.x, p.z - e);
        let n = Vec3::new(-dx, 2.0 * e, -dz).normalize();
        // Damp and darker in the shade along the curbs and at the tires.
        let edge = ((HALF_WIDTH - p.z.abs()) / 0.25).min(1.0);
        let wall = ((X_WALL - p.x) / 0.4).min(1.0);
        (p, n, Vec2::new(p.x, p.z), grey(0.8 + 0.2 * edge.min(wall)))
    });
    b.build()
}

/// One car tire lying flat, centered at the origin: tread, rounded shoulders, sidewalls and
/// the beads curling into the bore.
fn tire(b: &mut MeshBuilder) {
    let (r, hw) = (TIRE_R, TIRE_W * 0.5);
    let bead = r * 0.64;
    let profile = [
        Vec2::new(bead - 0.006, -hw + 0.02),
        Vec2::new(bead + 0.01, -hw + 0.003),
        Vec2::new(bead + 0.04, -hw),
        Vec2::new(r - 0.05, -hw + 0.004),
        Vec2::new(r - 0.018, -hw + 0.016),
        Vec2::new(r - 0.004, -hw + 0.04),
        Vec2::new(r, -0.03),
        Vec2::new(r, 0.03),
        Vec2::new(r - 0.004, hw - 0.04),
        Vec2::new(r - 0.018, hw - 0.016),
        Vec2::new(r - 0.05, hw - 0.004),
        Vec2::new(bead + 0.04, hw),
        Vec2::new(bead + 0.01, hw - 0.003),
        Vec2::new(bead - 0.006, hw - 0.02),
        Vec2::new(bead - 0.006, -hw + 0.02),
    ];
    b.lathe(&profile, 22, 50.0);
}

/// Which tire slots of the wall are drawn: the front row, the end columns and the top layer
/// (the rest are hidden inside).
fn tire_slots() -> Vec<(Vec3, u32)> {
    let pitch = 2.0 * TIRE_R + 0.01;
    let cols = ((2.0 * WALL_HALF) / pitch).floor() as i32;
    let rows = ((X_END - X_WALL) / pitch).floor() as i32;
    let z0 = -(cols - 1) as f32 * pitch * 0.5;
    let mut out = Vec::new();
    for layer in 0..TIRE_LAYERS as i32 {
        for row in 0..rows {
            for col in 0..cols {
                let seen =
                    row == 0 || col == 0 || col == cols - 1 || layer == TIRE_LAYERS as i32 - 1;
                if !seen {
                    continue;
                }
                let id = (layer * 1000 + row * 100 + col) as u32;
                let jitter = Vec3::new(
                    0.015 * (hash(id * 3) - 0.5),
                    0.0,
                    0.015 * (hash(id * 3 + 1) - 0.5),
                );
                let p = Vec3::new(
                    X_WALL + TIRE_R + row as f32 * pitch,
                    TIRE_W * (layer as f32 + 0.5),
                    z0 + col as f32 * pitch,
                ) + jitter;
                out.push((p, id));
            }
        }
    }
    out
}

fn tires() -> Mesh {
    let mut b = MeshBuilder::new();
    let black = srgb(0x262626);
    let white = srgb(0xd8d6d0);
    let red = srgb(0xb3322b);
    for (p, id) in tire_slots() {
        let layer = (p.y / TIRE_W) as usize;
        // The front row is painted in alternating red and white layers; the rest are bare.
        let front = p.x < X_WALL + TIRE_R + 0.1;
        let color: Vec4 = match (front, layer % 2) {
            (false, _) => black,
            (true, 0) => red,
            _ => white,
        };
        let shade = 0.88 + 0.12 * hash(id ^ 0x55);
        let turn = Quat::from_rotation_y(hash(id) * TAU)
            * Quat::from_rotation_x(0.02 * (hash(id ^ 7) - 0.5));
        b.colored((color * shade).with_w(1.0), |b| {
            b.placed(rot_at(turn, p), tire)
        });
    }
    b.build()
}

// ---------------------------------------------------------------------------------------
// Start platform handrail
// ---------------------------------------------------------------------------------------

fn handrail() -> Mesh {
    let mut b = MeshBuilder::new();
    let x = X_BACK + 0.12;
    let y0 = RAMP_HEIGHT + 0.005;
    let half = HALF_WIDTH;
    let zs = [-half + 0.08, -LANE * 0.5, LANE * 0.5, half - 0.08];
    for &z in &zs {
        bar(
            &mut b,
            Vec3::new(x, y0, z),
            Vec3::new(x, y0 + 1.05, z),
            0.024,
            12,
        );
        plate(
            &mut b,
            at(Vec3::new(x, y0 + 0.005, z)),
            Vec3::new(0.06, 0.005, 0.06),
            0.002,
        );
    }
    for h in [0.5, 1.05] {
        b.tube(
            &[Vec3::new(x, y0 + h, -half), Vec3::new(x, y0 + h, half)],
            if h > 1.0 { 0.024 } else { 0.018 },
            12,
            false,
            true,
        );
    }
    // A toe board along the back edge.
    plate(
        &mut b,
        at(Vec3::new(x, y0 + 0.075, 0.0)),
        Vec3::new(0.005, 0.075, half),
        0.002,
    );
    b.build()
}

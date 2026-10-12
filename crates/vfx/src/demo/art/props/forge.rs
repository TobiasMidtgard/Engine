//! The forge corner: a London-pattern anvil (horn, face, heel with hardy and pritchel holes,
//! waist and feet) on an iron-banded oak stump, with a hammer leaning against it.

use engine_core::math::{Quat, Vec2, Vec3, smoothstep};
use engine_render::Mesh;

use super::super::meshes::metal::{band, rivet};
use super::super::meshes::{
    Outline, Profile, Slab, block, colored, moved, noise1, paint, revolve, sweep, tube, uv_box,
    warp,
};
use super::super::{Prop, Surface};

/// Height of the anvil's face (where the hammer strikes and the sparks fly).
pub const FACE: f32 = 1.0;
/// Height of the stump's top.
pub const STUMP: f32 = 0.62;

/// The anvil on its stump. The horn points along +X.
pub fn anvil() -> Prop {
    let mut iron = Mesh::default();
    let mut wood = Mesh::default();
    let mut holes = Mesh::default();

    // --- Stump: a lumpy oak round with root flare, end grain on top, two iron bands.
    let r = 0.33;
    let bands = [0.16f32, 0.5];
    let mut prof = Profile::new()
        .to(r + 0.1, -0.02)
        .to(r + 0.07, 0.01)
        .to(r + 0.035, 0.05)
        .to(r + 0.01, 0.1);
    for i in 1..=16 {
        prof = prof.to(r, 0.1 + (STUMP - 0.13) * i as f32 / 16.0);
    }
    let prof = prof
        .arc(r - 0.025, STUMP - 0.025, 0.025, 0.0, 90.0, 3)
        .corner(r - 0.025, STUMP)
        .to(r * 0.6, STUMP)
        .to(r * 0.25, STUMP)
        .to(0.0, STUMP);
    let mut stump = revolve(&prof, 96);
    // Bark furrows: vertical ridges and lumps, weaker on the cut top, pressed flat under the
    // bands so they hug the wood.
    let furrow = |p: Vec3| {
        let a = p.z.atan2(p.x);
        let wobble = noise1(Vec3::new(p.x, p.y * 0.3, p.z), 6.0, 3, 2) * 2.5;
        ((a * 23.0 + wobble).sin() * 0.5 + 0.5).powi(2)
    };
    warp(&mut stump, |p| {
        let side = 1.0 - smoothstep(STUMP - 0.04, STUMP - 0.01, p.y);
        let free = bands
            .iter()
            .map(|b| smoothstep(0.03, 0.06, (p.y - b).abs()))
            .fold(1.0, f32::min);
        let a = p.z.atan2(p.x);
        let lump = noise1(p, 3.0, 5, 3);
        // Five root buttresses flaring into the ground.
        let low = (1.0 - p.y / 0.16).clamp(0.0, 1.0).powi(2);
        let roots = (a * 5.0 + 0.7 + noise1(p, 4.0, 6, 2))
            .cos()
            .max(0.0)
            .powi(3)
            * 0.28
            * low;
        let k = 1.0 + side * free * (0.022 * furrow(p) + 0.015 * lump) + roots - 0.12 * low;
        Vec3::new(p.x * k, p.y, p.z * k)
    });
    // Growth rings and checks on the end grain; dark furrowed bark.
    paint(&mut stump, |p, n| {
        if n.y > 0.7 {
            let rr = Vec2::new(p.x, p.z).length();
            let ring = 0.85 + 0.15 * (rr * 90.0 + noise1(p, 8.0, 7, 2) * 3.0).sin();
            let a = p.z.atan2(p.x);
            let crack = 1.0 - smoothstep(0.0, 0.04, (a * 3.0).sin().abs());
            let check = 1.0 - 0.6 * crack * smoothstep(0.05, 0.3, rr);
            let rim = smoothstep(r - 0.06, r - 0.025, rr);
            Vec3::new(1.25, 1.15, 1.0) * ring * check * (1.0 - 0.45 * rim)
        } else {
            let f = furrow(p);
            Vec3::splat((0.4 + 0.3 * f) * (1.0 + 0.2 * noise1(p, 9.0, 9, 2)))
        }
    });
    uv_box(&mut stump, Vec2::new(0.3, 0.7));
    wood.merge(&stump);
    for y in bands {
        let mut b = band(r + 0.002, 0.05, 0.009, 96);
        b = moved(b, Vec3::Y * y, Quat::IDENTITY);
        iron.merge(&b);
        for k in 0..10 {
            let a = k as f32 / 10.0 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            let rv = moved(
                rivet(0.008),
                d * (r + 0.01) + Vec3::Y * y,
                Quat::from_rotation_arc(Vec3::Y, d),
            );
            iron.merge(&rv);
        }
    }

    // --- Anvil body. Feet, waist, body, table and horn; heel to -X.
    let base_y = STUMP;
    // Feet: a flared block with four splayed feet.
    let feet = Slab::new(Outline::rect(0.17, 0.12), base_y, base_y + 0.07, 0.015)
        .taper(0.8)
        .mesh();
    iron.merge(&feet);
    for (sx, sz) in [(1.0f32, 1.0f32), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let foot = Slab::new(Outline::rect(0.05, 0.035), base_y, base_y + 0.04, 0.012)
            .taper(0.7)
            .mesh();
        let at = Vec3::new(sx * 0.17, 0.0, sz * 0.125);
        iron.merge(&moved(foot, at, Quat::from_rotation_y(-sx * sz * 0.5)));
    }
    // Waist: pinched in the middle.
    let waist = Profile::new()
        .to(0.02, base_y + 0.06)
        .to(0.02, base_y + 0.08)
        .scale(0.78)
        .to(0.02, base_y + 0.13)
        .scale(0.72)
        .to(0.02, base_y + 0.17)
        .scale(0.95)
        .to(0.02, base_y + 0.23)
        .scale(1.0)
        .to(0.02, base_y + 0.25);
    iron.merge(&sweep(&Outline::rect(0.13, 0.065).rounded(3), &waist));
    // Body with the face on top.
    let body_bottom = base_y + 0.24;
    let mut body = Slab::new(
        Outline::new(vec![
            Vec2::new(-0.3, -0.058),
            Vec2::new(0.13, -0.058),
            Vec2::new(0.13, 0.058),
            Vec2::new(-0.3, 0.058),
        ]),
        body_bottom,
        FACE,
        0.012,
    )
    .segs(3)
    .mesh();
    // The heel narrows a little and the body tucks under.
    warp(&mut body, |p| {
        let heel = smoothstep(-0.12, -0.3, p.x);
        let under = 1.0 - smoothstep(body_bottom, FACE - 0.05, p.y);
        Vec3::new(p.x, p.y, p.z * (1.0 - 0.18 * heel - 0.12 * under))
    });
    iron.merge(&body);
    // Table: a step down between face and horn.
    let table = Slab::new(
        Outline::rect(0.045, 0.05),
        body_bottom + 0.02,
        FACE - 0.03,
        0.01,
    )
    .mesh();
    iron.merge(&moved(table, Vec3::X * 0.165, Quat::IDENTITY));
    // Horn: a cone from the table, flattening on top and lifting slightly at the tip.
    let horn_path: Vec<Vec3> = (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0;
            Vec3::new(0.19 + t * 0.3, FACE - 0.07 + 0.02 * t * t, 0.0)
        })
        .collect();
    let mut horn = tube(
        &horn_path,
        |t| 0.056 * (1.0 - t).powf(0.85) + 0.004,
        14,
        true,
    );
    warp(&mut horn, |p| {
        let below = (FACE - 0.07 - p.y).max(0.0);
        Vec3::new(p.x, p.y + below * 0.25, p.z)
    });
    iron.merge(&horn);
    // Hardy (square) and pritchel (round) holes in the heel, as dark insets.
    let hardy = block(Vec3::new(0.026, 0.004, 0.026), 0.002);
    holes.merge(&moved(
        hardy,
        Vec3::new(-0.24, FACE + 0.0002, 0.0),
        Quat::IDENTITY,
    ));
    let pritchel = revolve(
        &Profile::new()
            .corner(0.009, FACE - 0.003)
            .corner(0.009, FACE + 0.0004)
            .to(0.0, FACE + 0.0004),
        12,
    );
    holes.merge(&moved(pritchel, Vec3::new(-0.17, 0.0, 0.0), Quat::IDENTITY));
    // Polished face and horn top, scaled and dark below.
    paint(&mut iron, |p, n| {
        let worn = smoothstep(0.85, 0.98, n.y) * smoothstep(FACE - 0.08, FACE - 0.005, p.y);
        let low = 0.55 + 0.45 * smoothstep(STUMP, FACE - 0.05, p.y);
        Vec3::splat(low + 0.75 * worn) * (1.0 + 0.06 * noise1(p, 25.0, 13, 2))
    });

    // --- A hammer leaning against the stump.
    let lean = Quat::from_rotation_z(-0.35) * Quat::from_rotation_y(0.4);
    let handle: Vec<Vec3> = (0..=4).map(|i| Vec3::Y * (i as f32 * 0.09)).collect();
    let mut h = tube(&handle, |t| 0.014 + 0.003 * t, 8, true);
    paint(&mut h, |_, _| Vec3::splat(0.85));
    let base = Vec3::new(-r - 0.12, 0.0, 0.18);
    wood.merge(&moved(h, base, lean));
    let head = Slab::new(Outline::rect(0.065, 0.022).rounded(2), -0.022, 0.022, 0.006).mesh();
    let head = moved(
        head,
        Vec3::Y * 0.37,
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
    );
    iron.merge(&moved(head, base, lean));

    let mut prop = Prop::new();
    prop.push(Surface::Wood, wood);
    prop.push(Surface::Iron, iron);
    prop.push(Surface::DarkStone, colored(holes, Vec3::splat(0.05)));
    prop
}

//! A backdrop for showing the trail on its own (the examples and the screenshot test, where
//! no landscape surrounds it): flat ground, a lawn that takes over where the trail's own
//! meadow thins out (about 7 m from the path, see [`meadow_reach`]), and a few dozen trees
//! with leafy crowns, kept where their shadows miss the trail under a low afternoon sun
//! travelling along about (-0.55, -0.42, -0.62).

use std::f32::consts::TAU;

use engine_assets::Assets;
use engine_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, GrassField, GrassStyle, Image, MaterialRef, Mesh, MeshRef, MeshRenderer,
    StandardMaterial,
};

use super::ground::procgen::{
    Canvas, Rand, Texel, append, fbm_world, fbm3, icosphere, smooth, smooth_normals_from, tube,
    vert,
};
use super::nearest_on_trail;

/// How far from the trail's centre line its meadow reaches at `(x, z)` (meters from the
/// trail's origin): the hand-off distance to a surrounding lawn, about 7 m with a ragged
/// line. The meadow is gone there and a lawn may start.
pub fn meadow_reach(x: f32, z: f32) -> f32 {
    6.9 + 0.45 * fbm_world(x, z, 2.5, 2, 0x6a4)
}

/// The look of the backdrop's lawn: shorter, finer and a little drier than the trail's
/// meadow, which thins out into it.
pub fn lawn_style() -> GrassStyle {
    GrassStyle {
        blades_per_m2: 300.0,
        height: 0.13,
        height_variation: 0.55,
        width: 0.0045,
        bend: 0.45,
        lush_color: [0.1, 0.2, 0.05],
        dry_color: [0.4, 0.34, 0.17],
        dryness: 0.3,
        wind_response: 0.8,
        draw_distance: 60.0,
        shadow_distance: 12.0,
    }
}

/// Lay the backdrop round a trail at `origin`: ground, lawn and trees.
pub fn spawn_backdrop(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| {
        let style = lawn_style();
        let g = style.ground_color();
        let renders = world.contains_resource::<Assets<Mesh>>()
            && world.contains_resource::<Assets<Image>>()
            && world.contains_resource::<Assets<StandardMaterial>>();
        world.spawn((
            Name::new("Backdrop ground"),
            Transform::from_translation(origin),
            MeshRenderer::new(
                engine_render::Shape::Plane {
                    size: [800.0, 800.0],
                    subdivisions: 1,
                },
                StandardMaterial::color(g[0], g[1], g[2]).with_roughness(0.95),
            ),
        ));
        world.spawn((
            Name::new("Backdrop lawn"),
            lawn(style),
            Transform::from_translation(origin),
        ));
        if renders {
            trees(world, origin);
        }
    });
}

/// The lawn: 160 × 110 m round the trail, starting where the meadow ends, in patches of
/// thicker and thinner grass.
fn lawn(style: GrassStyle) -> GrassField {
    let size = [160.0f32, 110.0];
    let res = [(size[0] / 0.5) as u32 + 1, (size[1] / 0.5) as u32 + 1];
    GrassField::flat(size[0], size[1])
        .with_style(style)
        .sample(res, |x, z| {
            let (_, side) = nearest_on_trail(DVec3::new(x as f64, 0.0, z as f64));
            let start = meadow_reach(x, z);
            let patches = smooth(-0.45, 0.45, fbm_world(x, z, 6.0, 3, 0x1a1));
            let density = smooth(start, start + 1.0, side as f32) * (0.55 + 0.45 * patches);
            let dryness = (0.28 + 0.22 * fbm_world(x, z, 9.0, 2, 0x1a2)).clamp(0.0, 1.0);
            (0.0, density, dryness)
        })
}

/// Where the trees stand: a ring round the trail, none close to it and none whose shadow
/// (from the default low sun) would fall across it.
fn tree_spots() -> Vec<(Vec2, f32, u64)> {
    let sun = Vec2::new(-0.55, -0.62).normalize();
    let mut rng = Rand::new(0x7ee5);
    let mut spots: Vec<(Vec2, f32, u64)> = Vec::new();
    let mut tries = 0;
    while spots.len() < 56 && tries < 4000 {
        tries += 1;
        let a = rng.range(0.0, TAU);
        let r = rng.range(12.0, 52.0);
        let p = Vec2::new(a.cos() * (r + 22.0), a.sin() * r);
        let h = rng.range(5.5, 10.5);
        let crown = 1.6 + h * 0.22;
        let clear = |q: Vec2| {
            let (_, side) = nearest_on_trail(DVec3::new(q.x as f64, 0.0, q.y as f64));
            side as f32 > 1.6 + crown
        };
        // The shadow reaches about 2 m per meter of height, along the sun.
        let reach = 2.0 * (h + crown);
        let shaded = (0..=12).any(|k| !clear(p + sun * reach * k as f32 / 12.0));
        let (_, side) = nearest_on_trail(DVec3::new(p.x as f64, 0.0, p.y as f64));
        // Keep the ground between the wide view's camera (south) and the loop open.
        let foreground = p.y < -8.0 && p.y > -40.0 && p.x.abs() < 34.0;
        if shaded || foreground || (side as f32) < 9.0 {
            continue;
        }
        if spots.iter().any(|(q, _, _)| q.distance(p) < 3.5) {
            continue;
        }
        spots.push((p, h, rng.seed() as u64));
    }
    spots
}

fn trees(world: &mut World, origin: DVec3) {
    let leaf_img = leaf_spray();
    let (albedo, normal) = {
        let mut images = world.resource_mut::<Assets<Image>>();
        (
            images.insert_keyed("backdrop:leaves:albedo", leaf_img.0),
            images.insert_keyed("backdrop:leaves:normal", leaf_img.1),
        )
    };
    let (bark, leaves, core) = {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        let bark = StandardMaterial::hex(0x6b5d50)
            .with_roughness(0.92)
            .with_detail(0.7, 0.02);
        let mut leaves = StandardMaterial::hex(0xffffff)
            .with_roughness(0.72)
            .with_subsurface(0.7)
            .with_wind_sway(0.05);
        leaves.base_color_texture = Some(albedo);
        leaves.normal_texture = Some(normal);
        leaves.alpha_mode = AlphaMode::Mask(0.5);
        leaves.double_sided = true;
        let core = StandardMaterial::hex(0x2a3a1a)
            .with_roughness(0.9)
            .with_subsurface(0.3)
            .with_detail(0.6, 0.05);
        (
            materials.add(bark),
            materials.add(leaves),
            materials.add(core),
        )
    };
    let spots = tree_spots();
    let built: Vec<(Vec2, [Mesh; 3])> = spots
        .iter()
        .map(|&(p, h, seed)| (p, tree(seed, h)))
        .collect();
    let mut handles = Vec::new();
    {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        for (p, [b, l, c]) in built {
            handles.push((p, meshes.add(b), meshes.add(l), meshes.add(c)));
        }
    }
    for (p, b, l, c) in handles {
        let at = origin + DVec3::new(p.x as f64, 0.0, p.y as f64);
        for (mesh, mat) in [(b, bark), (l, leaves), (c, core)] {
            world.spawn((
                Name::new("Backdrop tree"),
                Transform::from_translation(at),
                MeshRenderer {
                    mesh: MeshRef::Handle(mesh),
                    material: MaterialRef::Handle(mat),
                    cast_shadows: true,
                    visible: true,
                },
            ));
        }
    }
}

/// One broadleaf tree `h` meters tall: bark (trunk and limbs), leaf cards and the dark
/// inner foliage that keeps the crown from looking hollow.
fn tree(seed: u64, h: f32) -> [Mesh; 3] {
    let mut rng = Rand::new(seed);
    let mut bark = Mesh::default();
    let mut cards = Mesh::default();
    let mut core = Mesh::default();
    let r0 = 0.11 + h * 0.016;
    let lean = Vec3::new(rng.normal(0.05), 0.0, rng.normal(0.05));
    let trunk_top = h * 0.82;
    let trunk_at = |t: f32| Vec3::new(0.0, trunk_top * t, 0.0) + lean * trunk_top * t * t;
    let trunk_r = move |t: f32| {
        let flare = 1.0 + 0.6 * (1.0 - smooth(0.0, 0.07, t));
        r0 * (1.0 - 0.8 * t) * flare
    };
    let s = seed as u32;
    let pts: Vec<Vec3> = (0..=12).map(|k| trunk_at(k as f32 / 12.0)).collect();
    let ridge_col = |t: f32| {
        let c = Vec3::new(0.62, 0.56, 0.5) * (0.75 + 0.25 * t);
        c.extend(1.0)
    };
    append(
        &mut bark,
        &tube(
            &pts,
            12,
            false,
            move |t, a| {
                trunk_r(t)
                    * (1.0
                        + 0.05 * (a * 7.0 + t * 6.0).sin()
                        + 0.04 * fbm3(Vec3::new(a, t * 8.0, 0.0), 2, s))
            },
            ridge_col,
        ),
        Quat::IDENTITY,
        Vec3::ONE,
        Vec3::ZERO,
        Vec4::ONE,
    );
    // The crown's centre and size.
    let crown_c = Vec3::new(0.0, h * 0.66, 0.0) + lean * h * 0.4;
    let crown_r = Vec3::new(h * 0.3 + 0.5, h * 0.3, h * 0.3 + 0.5);
    let mut clumps: Vec<(Vec3, f32)> = Vec::new();
    let limbs = 5 + rng.below(4);
    let turn = rng.range(0.0, TAU);
    for k in 0..limbs {
        let t0 = rng.range(0.42, 0.78);
        let start = trunk_at(t0);
        let az = turn + k as f32 / limbs as f32 * TAU + rng.normal(0.3);
        let up = rng.range(0.5, 1.1) * (1.2 - t0);
        let dir = Vec3::new(az.cos(), up, az.sin()).normalize();
        let len = h * rng.range(0.24, 0.36) * (1.25 - t0 * 0.6);
        let limb: Vec<Vec3> = (0..=6)
            .map(|i| {
                let t = i as f32 / 6.0;
                start + dir * len * t + Vec3::Y * (0.25 * len * t * (1.0 - t))
            })
            .collect();
        let r = trunk_r(t0) * 0.55;
        append(
            &mut bark,
            &tube(&limb, 8, true, move |t, _| r * (1.0 - 0.75 * t), ridge_col),
            Quat::IDENTITY,
            Vec3::ONE,
            Vec3::ZERO,
            Vec4::ONE,
        );
        let tip = limb[6];
        clumps.push((tip, rng.range(1.05, 1.4) * (0.55 + h * 0.07)));
        clumps.push((
            limb[3] + Vec3::new(rng.normal(0.2), 0.35, rng.normal(0.2)),
            rng.range(0.9, 1.2) * (0.5 + h * 0.06),
        ));
    }
    // Fill the crown's volume and top.
    for _ in 0..9 {
        let d = Vec3::new(rng.normal(0.6), rng.range(-0.2, 1.0), rng.normal(0.6));
        clumps.push((
            crown_c + d * crown_r * 0.7,
            rng.range(0.95, 1.3) * (0.55 + h * 0.07),
        ));
    }
    let hue = rng.range(-1.0, 1.0);
    let base_green = Vec3::new(0.25, 0.34, 0.1).lerp(Vec3::new(0.3, 0.33, 0.12), hue.max(0.0));
    let (unit, tris) = icosphere(2);
    for &(c, r) in &clumps {
        // Dark inner foliage.
        let start = core.indices.len();
        let base = core.vertices.len() as u32;
        let off = Vec3::new(rng.range(0.0, 40.0), rng.range(0.0, 40.0), 0.0);
        for &u in &unit {
            let bump = 1.0 + 0.3 * fbm3(u * 1.7 + off, 3, s ^ 0x55);
            let p = c + u * r * 0.5 * bump;
            core.vertices
                .push(vert(p, u, Vec2::ZERO, Vec4::new(1.0, 1.0, 1.0, 1.0)));
        }
        for t in &tris {
            core.indices
                .extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
        }
        smooth_normals_from(&mut core, start);
        // Leaf sprays on and round it, facing outward from the crown for soft shading.
        let n = 46;
        let tint = rng.range(0.85, 1.12);
        for _ in 0..n {
            let dir =
                Vec3::new(rng.normal(1.0), rng.normal(1.0), rng.normal(1.0)).normalize_or(Vec3::Y);
            let p = c + dir * r * rng.range(0.45, 1.15);
            let size = rng.range(0.55, 0.95) * (0.6 + 0.05 * h);
            let rot = Quat::from_rotation_arc(Vec3::Z, dir)
                * Quat::from_rotation_z(rng.range(0.0, TAU))
                * Quat::from_rotation_x(rng.normal(0.6));
            let rel = p - crown_c;
            let outer = (rel / crown_r).length();
            let ao =
                0.55 + 0.35 * smooth(0.2, 1.0, outer) + 0.2 * smooth(-0.6, 0.8, rel.y / crown_r.y);
            let col = (base_green * tint * ao * rng.range(0.85, 1.15)).extend(1.0);
            let normal = (rel / crown_r).normalize_or(Vec3::Y);
            let normal = (normal + Vec3::Y * 0.6).normalize();
            let b = cards.vertices.len() as u32;
            for (k, (cx, cy)) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)]
                .into_iter()
                .enumerate()
            {
                let q = p + rot * Vec3::new(cx * size, cy * size, 0.0);
                let uv = Vec2::new([0.0, 1.0, 1.0, 0.0][k], [1.0, 1.0, 0.0, 0.0][k]);
                cards.vertices.push(vert(q, normal, uv, col));
            }
            cards
                .indices
                .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }
    // Colour the inner foliage dark, darker toward the bottom.
    for v in &mut core.vertices {
        let rel = Vec3::from(v.position) - crown_c;
        let k = 0.7 + 0.3 * smooth(-1.0, 1.0, rel.y / crown_r.y);
        v.color = (Vec3::new(0.85, 0.95, 0.75) * k).extend(1.0).to_array();
    }
    [bark, cards, core]
}

/// A spray of leaves on a 0.6 m card: pale albedo (vertex colours tint it) with alpha cut to
/// the leaves, and a normal map. Returns (albedo, normal).
fn leaf_spray() -> (Image, Image) {
    let mut c = Canvas::new(256, 256, Vec2::splat(0.6));
    c.fill(|_, _| Texel {
        alpha: 0.0,
        color: Vec3::splat(0.8),
        ..Default::default()
    });
    let mut rng = Rand::new(0x1eaf);
    // Twigs from the middle out, leaves along them.
    for k in 0..7 {
        let a = k as f32 / 7.0 * TAU + rng.normal(0.25);
        let dir = Vec2::new(a.cos(), a.sin());
        let len = rng.range(0.28, 0.44);
        for i in 0..9 {
            let t = (i as f32 + 0.5) / 9.0;
            let at = Vec2::splat(0.5) + dir * len * t;
            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
            let la = a + side * rng.range(0.5, 1.0);
            let ld = Vec2::new(la.cos(), la.sin());
            let l = rng.range(9.0, 15.0) * (1.1 - 0.4 * t);
            let w = l * rng.range(0.38, 0.5);
            let centre = at + ld * l * 0.5 / 256.0;
            let tone = rng.range(0.78, 1.05);
            let curl = rng.range(-1.0, 1.0);
            c.stamp(centre.x, centre.y, l, |dx, dy, tx| {
                let along = (dx * ld.x + dy * ld.y) / l;
                let across = (-dx * ld.y + dy * ld.x) / w;
                // A pointed oval.
                let half = 0.5
                    * (1.0 - (2.0 * along).powi(2)).max(0.0).sqrt()
                    * (1.0 - 0.3 * along.max(0.0));
                let inside = half - across.abs();
                if inside <= 0.0 || along.abs() > 0.5 {
                    return;
                }
                let cov = (inside * w).clamp(0.0, 1.0);
                let vein = 1.0 - smooth(0.0, 0.06, across.abs());
                tx.alpha = tx.alpha.max(cov);
                tx.color = Vec3::splat(tone * (1.0 + 0.12 * vein + 0.08 * curl * across));
                tx.height = 0.0006 * (1.0 - (across / 0.5).powi(2)) + 0.0004 * curl * across;
            });
        }
    }
    let set = c.bake(1.0, 0.0);
    (set.albedo, set.normal)
}

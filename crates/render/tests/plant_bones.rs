//! Trees on wind bones (`engine_render::plant_wind`): a mesh registered in `PlantWindMeshes`
//! turns its limbs about their pivots and leans its trunk with the wind, by its species'
//! response, with each vertex naming its bone in its color alpha. Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    PlantBone, PlantSkeleton, PlantSpeciesWind, PlantWindMesh, PlantWindMeshes, PlantWindSpecies,
    RenderDisabled, capture_now, pack_plant_vertex,
};
use image::RgbaImage;

const W: u32 = 160;
const H: u32 = 160;

/// A 6 m post standing on the ground: its lower half rides the trunk bone, its upper half a
/// limb whose pivot is halfway up, pointing up. Mesh space y 0..6.
fn post() -> Mesh {
    let mut out = Mesh::default();
    for (bone, y0) in [(0u16, 0.0f32), (1, 3.0)] {
        let mut half = Shape::Cuboid {
            size: [0.4, 3.0, 0.4],
        }
        .mesh();
        half.transform(&Mat4::from_translation(Vec3::new(0.0, y0 + 1.5, 0.0)));
        for v in &mut half.vertices {
            let tip = (v.position[1] - y0) / 3.0;
            v.color = [1.0, 1.0, 1.0, pack_plant_vertex(bone, tip, 0.0, false)];
        }
        out.merge(&half);
    }
    out
}

fn skeleton() -> PlantSkeleton {
    PlantSkeleton {
        bones: vec![
            PlantBone {
                pivot: Vec3::ZERO,
                length: 6.0,
                axis: Vec3::Y,
                face: Vec3::X,
                parent: None,
                level: 0,
                phase: 0.0,
                stiffness: 1.0,
            },
            PlantBone {
                pivot: Vec3::Y * 3.0,
                length: 3.0,
                axis: Vec3::Y,
                face: Vec3::Z,
                parent: Some(0),
                level: 1,
                phase: 0.0,
                stiffness: 1.0,
            },
        ],
    }
}

/// The post seen from the side in a steady wind of `speed` m/s along +x (no gust fronts), as
/// a bone plant of species `species`.
fn render_tree(name: &str, speed: f32, species: PlantSpeciesWind) -> Option<RgbaImage> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.bloom_intensity = 0.0;
    }
    app.world.insert_resource(engine_world::WindField {
        prevailing: Vec3::new(speed, 0.0, 0.0),
        gustiness: 0.0,
        ..Default::default()
    });
    let handle = app
        .world
        .resource_mut::<engine_assets::Assets<Mesh>>()
        .add(post());
    let mut meshes = PlantWindMeshes::default();
    meshes.insert(
        handle,
        PlantWindMesh {
            skeleton: Arc::new(skeleton()),
            species: 0,
            detail: 3,
        },
    );
    app.world.insert_resource(meshes);
    app.world.insert_resource(PlantWindSpecies {
        slots: vec![species],
    });
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 3.0, 9.0).looking_at(DVec3::new(0.0, 3.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.3, -0.8, -0.5), Vec3::Y),
    ));
    let mut material = StandardMaterial::color(0.8, 0.2, 0.1);
    // Bone plants keep a sway so the passes treat them as moving; its value is unused.
    material.wind_sway = 1.0;
    w.spawn((Transform::default(), MeshRenderer::new(handle, material)));
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("plant_bones_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean column of the post's red in rows `y0..y1`.
fn red_column(img: &RgbaImage, y0: u32, y1: u32) -> f32 {
    let cols: Vec<u32> = (0..W)
        .filter(|&x| {
            (y0..y1).any(|y| {
                let p = img.get_pixel(x, y);
                p[0] as i32 > p[2] as i32 + 40
            })
        })
        .collect();
    assert!(!cols.is_empty(), "no post in rows {y0}..{y1}");
    cols.iter().map(|&c| c as f32).sum::<f32>() / cols.len() as f32
}

/// The rows of the post's top (about 5.4-6 m) and of its foot (0-1 m), which the camera at
/// 3 m sees about 19 pixels a meter from the middle row.
fn top_and_foot(img: &RgbaImage) -> (f32, f32) {
    (red_column(img, 22, 32), red_column(img, 120, 135))
}

/// No sway, no flutter: only the static bend and lean, so the shots are deterministic.
fn calm_species() -> PlantSpeciesWind {
    PlantSpeciesWind {
        sway: 0.0,
        flutter_hz: 0.0,
        ..Default::default()
    }
}

#[test]
fn limbs_turn_downwind_about_their_pivot() {
    let species = PlantSpeciesWind {
        bend_deg: [20.0, 10.0, 18.0],
        gale_lean: 0.0,
        ..calm_species()
    };
    let Some(still) = render_tree("limb_still", 0.0, species) else {
        return;
    };
    let windy = render_tree("limb_gale", 15.0, species).expect("gpu");
    let (top_still, foot_still) = top_and_foot(&still);
    let (top_windy, foot_windy) = top_and_foot(&windy);
    eprintln!(
        "top {top_still:.1} -> {top_windy:.1} px, foot {foot_still:.1} -> {foot_windy:.1} px"
    );
    // 20 degrees x 0.95 (an upright limb is neither windward nor leeward) on a 3 m limb moves
    // its tip about 1 m downwind, some 19 px; the foot is below the pivot and stays.
    assert!(
        top_windy - top_still > 10.0,
        "the limb should turn downwind: {top_still} -> {top_windy}"
    );
    assert!(
        (foot_windy - foot_still).abs() < 1.0,
        "below its pivot nothing moves: {foot_still} -> {foot_windy}"
    );
}

#[test]
fn the_trunk_leans_with_the_wind_and_its_foot_stays() {
    let species = PlantSpeciesWind {
        bend_deg: [0.0, 0.0, 0.0],
        gale_lean: 0.15,
        ..calm_species()
    };
    let Some(still) = render_tree("trunk_still", 0.0, species) else {
        return;
    };
    let gale = render_tree("trunk_gale", 15.0, species).expect("gpu");
    let breeze = render_tree("trunk_breeze", 5.0, species).expect("gpu");
    let (top_still, foot_still) = top_and_foot(&still);
    let (top_gale, foot_gale) = top_and_foot(&gale);
    let (top_breeze, _) = top_and_foot(&breeze);
    eprintln!(
        "top {top_still:.1} -> breeze {top_breeze:.1} -> gale {top_gale:.1} px, foot {foot_still:.1} -> {foot_gale:.1} px"
    );
    // A gale leans the 6 m post's top 0.9 m (15 % of its height), some 17 px; a breeze's drag is
    // a tenth of a gale's.
    assert!(
        top_gale - top_still > 10.0,
        "the trunk should lean downwind: {top_still} -> {top_gale}"
    );
    assert!(
        top_breeze - top_still < (top_gale - top_still) * 0.4,
        "a breeze leans it far less: {top_breeze} vs {top_gale}"
    );
    assert!(
        (foot_gale - foot_still).abs() < 1.0,
        "its foot stays: {foot_still} -> {foot_gale}"
    );
}

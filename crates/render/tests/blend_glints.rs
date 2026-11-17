//! Blended surfaces lit as surfaces of their own: a clear, smooth sheet of water glints in the
//! sun alike over ground a few centimeters below it and ten meters below, from close and from
//! far (it never takes the ground's screen-space light, shadows' noise and all); it fades out
//! by its vertex alpha, glint and reflections too; and seen from high up it is as hazy as
//! opaque ground there, with the aerial perspective table and without. Needs a GPU or
//! lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::mesh::Vertex;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;

struct Scene {
    /// Height of the camera over the sheet, m (it looks 16° down, toward the sun's image).
    above: f64,
    /// Ground this far under the sheet, m.
    depth: f64,
    sheet: Option<StandardMaterial>,
    /// The sheet's vertex alpha.
    coverage: f32,
    aerial_table: bool,
}

/// Linear radiance (sRGB decoded, mean of the channels) of every pixel.
fn render(name: &str, scene: Scene) -> Option<Vec<f32>> {
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
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.aerial_table = scene.aerial_table;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    let top = scene.depth;
    let above = scene.above;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 17.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, top + above, 0.0)
            .looking_at(DVec3::new(0.0, top, -above * 3.5), Vec3::Y),
    ));
    // Low in front: its mirror image lies in the sheet in the middle of the view.
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.0, -0.28, 1.0), Vec3::Y),
    ));
    let side = 50_000.0;
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [side, side],
                subdivisions: 1,
            },
            StandardMaterial {
                perceptual_roughness: 1.0,
                ..StandardMaterial::color(0.02, 0.02, 0.02)
            },
        ),
    ));
    if let Some(material) = scene.sheet {
        let h = side / 2.0;
        let vertex = |x: f32, z: f32| Vertex {
            position: [x, 0.0, z],
            normal: [0.0, 1.0, 0.0],
            uv: [x, z],
            color: [1.0, 1.0, 1.0, scene.coverage],
        };
        let mesh = Mesh::new(
            vec![vertex(-h, -h), vertex(h, -h), vertex(h, h), vertex(-h, h)],
            vec![0, 2, 1, 0, 3, 2],
        );
        let mesh = w.resource_mut::<engine_assets::Assets<Mesh>>().add(mesh);
        let mut r = MeshRenderer::new(mesh, material);
        r.cast_shadows = false;
        w.spawn((Transform::from_xyz(0.0, top, 0.0), r));
    }
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("blend_glint_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let decode = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(
        img.pixels()
            .map(|p| (decode(p[0]) + decode(p[1]) + decode(p[2])) / 3.0)
            .collect(),
    )
}

/// The sheet's water: clear and glossy, with waves.
fn water() -> StandardMaterial {
    StandardMaterial {
        base_color: [0.02, 0.05, 0.1, 0.01],
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.16,
        reflectance: 0.4,
        detail: 0.7,
        detail_scale: 11.0,
        receives_wetness: false,
        ..Default::default()
    }
}

fn scene(above: f64, depth: f64) -> Scene {
    Scene {
        above,
        depth,
        sheet: Some(water()),
        coverage: 1.0,
        aerial_table: true,
    }
}

/// Mean over the rows below the horizon (the middle and lower part of the view).
fn mean(p: &[f32]) -> f32 {
    let rows = (H as usize / 2 - 10)..H as usize;
    let n = rows.len() * W as usize;
    rows.flat_map(|y| p[y * W as usize..(y + 1) * W as usize].iter())
        .sum::<f32>()
        / n as f32
}

/// The share of those pixels that differ by more than `by` (relative).
fn differ(a: &[f32], b: &[f32], by: f32) -> f32 {
    let rows = (H as usize / 2 - 10)..H as usize;
    let n = rows.len() * W as usize;
    let off = rows
        .flat_map(|y| (y * W as usize)..((y + 1) * W as usize))
        .filter(|&i| (a[i] - b[i]).abs() > by * a[i].max(b[i]).max(1e-3))
        .count();
    off as f32 / n as f32
}

#[test]
fn clear_water_glints_alike_over_shallow_and_deep_ground() {
    for above in [1.7, 300.0] {
        let Some(shallow) = render(&format!("shallow_{above}"), scene(above, 0.4)) else {
            return;
        };
        let deep = render(&format!("deep_{above}"), scene(above, 10.0)).unwrap();
        let brightest = shallow.iter().cloned().fold(0.0, f32::max);
        let (a, b) = (mean(&shallow), mean(&deep));
        let off = differ(&shallow, &deep, 0.1);
        eprintln!(
            "from {above} m: shallow {a:.4}, deep {b:.4}, {:.1}% differ",
            off * 100.0
        );
        assert!(
            brightest > 0.9,
            "no glint from {above} m: brightest {brightest}"
        );
        // From 300 m the sheet over shallow ground took the ground's screen-space light: its
        // shadows' noise as dark dots over a fifth of the water, the glint among them.
        assert!(
            (a - b).abs() < 0.03 * a.max(b) && off < 0.01,
            "from {above} m, the sheet over shallow ground ({a}) and deep ground ({b}) differ, {:.1}% of pixels",
            off * 100.0
        );
    }
}

#[test]
fn a_sheet_of_water_fades_out_by_its_vertex_alpha() {
    let bare = Scene {
        sheet: None,
        ..scene(1.7, 0.4)
    };
    let Some(bare) = render("bare", bare) else {
        return;
    };
    let faded = Scene {
        coverage: 0.0,
        ..scene(1.7, 0.4)
    };
    let faded = render("faded", faded).unwrap();
    let half = Scene {
        coverage: 0.5,
        ..scene(1.7, 0.4)
    };
    let half = render("half", half).unwrap();
    let whole = render("whole", scene(1.7, 0.4)).unwrap();
    let (b, f, h, w) = (mean(&bare), mean(&faded), mean(&half), mean(&whole));
    eprintln!("bare {b:.4}, faded {f:.4}, half {h:.4}, whole {w:.4}");
    // Before, a clear sheet at alpha 0 kept its glint and its reflections whole.
    assert!(
        (f - b).abs() < 0.01 * b,
        "a sheet at vertex alpha 0 still shows: {f} vs {b}"
    );
    assert!(w > b * 1.5, "the whole sheet shows too little: {w} vs {b}");
    // (Not quite half way: the glint's middle is brighter than white either way.)
    assert!(
        h > b + 0.3 * (w - b) && h < w - 0.1 * (w - b),
        "half covered is not between: {h} vs {b}, {w}"
    );
}

#[test]
fn a_blended_sheet_from_high_up_is_as_hazy_as_the_ground() {
    // Rough and all but opaque, the ground's own color: it should look like the ground.
    let sheet = StandardMaterial {
        base_color: [0.02, 0.02, 0.02, 0.999],
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 1.0,
        ..Default::default()
    };
    for table in [true, false] {
        let ground = Scene {
            sheet: None,
            aerial_table: table,
            ..scene(9000.0, 20.0)
        };
        let Some(ground) = render(&format!("haze_ground_{table}"), ground) else {
            return;
        };
        let covered = Scene {
            sheet: Some(sheet),
            aerial_table: table,
            ..scene(9000.0, 20.0)
        };
        let covered = render(&format!("haze_sheet_{table}"), covered).unwrap();
        let (g, c) = (mean(&ground), mean(&covered));
        eprintln!("table {table}: ground {g:.4}, sheet {c:.4}");
        assert!(
            (g - c).abs() < 0.05 * g,
            "with the table {table}, a sheet from 9 km up ({c}) is not as hazy as the ground ({g})"
        );
    }
}

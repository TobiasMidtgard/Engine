//! Granite setts at night seen from a low camera: the showcase plaza's paving after dark, where
//! noise in the lighting shows first. Lit by low point lights, or (`globes`) only by the bounce
//! light of glowing globes, which once spread into speckle.
//!
//! ```sh
//! cargo run -p engine_render --example night_paving -- <out-dir> [variant...]
//! ```
//!
//! Variants: `base`, `no_detail`, `no_parallax`, `no_normal`, `glossy`, `triplanar`; names
//! containing `fog`, `moon` or `globes` add thick ground fog, a full moon or glowing globes.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{MaterialPreset, RenderDisabled, capture_now};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "night_paving".into()),
    );
    let mut variants: Vec<String> = args.iter().skip(1).cloned().collect();
    if variants.is_empty() {
        variants = vec!["base".into()];
    }
    std::fs::create_dir_all(&out).expect("output directory");
    engine_core::log::init("warn");
    for variant in variants {
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        {
            let mut w = app.world.resource_mut::<Window>();
            w.width = 960;
            w.height = 540;
        }
        app.add_plugins(RenderPlugin);
        {
            let mut s = app.world.resource_mut::<RenderSettings>();
            s.film_grain = 0.0;
            s.vignette = 0.0;
        }
        if variant.contains("moon") {
            // A full moon 20 degrees up, ahead and to the left: light toward the camera.
            app.world.spawn((
                DirectionalLight {
                    color: [0.8, 0.85, 1.0],
                    illuminance: 0.3,
                    ..Default::default()
                },
                Transform::default().looking_at(DVec3::new(0.3, -0.34, 0.9), Vec3::Y),
            ));
        } else {
            // The sun well below the horizon (light travelling upward): night.
            app.world.spawn((
                DirectionalLight::default(),
                Transform::default().looking_at(DVec3::new(0.3, 0.4, -0.8), Vec3::Y),
            ));
        }
        let mut material = StandardMaterial::preset(MaterialPreset::Cobblestone)
            .with_texture_scale(1.5)
            .with_tint_hex(0xf2eee8);
        match variant.as_str() {
            "no_detail" => material.detail = 0.0,
            "no_parallax" => material.parallax_depth = 0.0,
            "no_normal" => material.normal_texture = None,
            "glossy" => material.perceptual_roughness = 0.12,
            "triplanar" => material = material.with_triplanar_world(),
            _ => {}
        }
        if variant.contains("fog") {
            let mut fog = app.world.resource_mut::<FogSettings>();
            fog.density = 0.02;
            fog.height_falloff = 0.2;
        }
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Plane {
                    size: [80.0, 80.0],
                    subdivisions: 1,
                },
                material,
            ),
        ));
        let lamps = if variant.contains("globes") { 0 } else { 4 };
        for (x, z, color) in [
            (-3.0, -12.0, [1.0, 0.85, 0.6]),
            (3.0, -10.0, [1.0, 0.2, 0.15]),
            (6.0, -14.0, [1.0, 0.2, 0.15]),
            (-7.0, -8.0, [1.0, 0.85, 0.6]),
        ]
        .into_iter()
        .take(lamps)
        {
            app.world.spawn((
                PointLight {
                    color,
                    intensity_lm: 3000.0,
                    range: 15.0,
                    radius: 0.15,
                },
                Transform::from_xyz(x, 1.2, z),
            ));
        }
        if variant.contains("globes") {
            // Glowing globes on short posts, as on the plaza: their light reaches the paving
            // only as bounce light.
            for (x, z, c) in [
                (-2.0, -9.0, [1.0, 0.15, 0.1]),
                (2.5, -11.0, [1.0, 0.15, 0.1]),
                (-5.0, -13.0, [1.0, 0.8, 0.5]),
            ] {
                app.world.spawn((
                    Transform::from_xyz(x, 1.0, z),
                    MeshRenderer::new(
                        Shape::Sphere {
                            radius: 0.3,
                            segments: 32,
                        },
                        StandardMaterial::color(c[0], c[1], c[2]).with_emissive(c, 200.0),
                    ),
                ));
            }
        }
        app.world.spawn((
            Camera::perspective(60.0),
            Transform::from_translation(DVec3::new(0.0, 4.5, 4.0))
                .looking_at(DVec3::new(0.0, 1.5, -12.0), Vec3::Y),
        ));
        app.update_n(30);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}), nothing rendered", d.0);
            return;
        }
        let path = out.join(format!("{variant}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        println!("{}", path.display());
    }
}

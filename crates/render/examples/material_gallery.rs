//! Render the material presets under sun and sky: one 1280×720 shot per preset (a triplanar
//! sphere and a UV-mapped beveled cube), an overview of every preset, and grazing-light
//! close-ups of brick, rock and wood with and without parallax.
//!
//! ```sh
//! cargo run -p engine_render --example material_gallery -- <out-dir> [shots...]
//! ```
//!
//! Shots are preset names (`brick`, `rusty_steel`, ...), `overview`, `closeup_brick`,
//! `closeup_rock`, `closeup_wood`; no shots renders all of them.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{MaterialPreset, RenderDisabled, capture_now};

const W: u32 = 1280;
const H: u32 = 720;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "material_gallery".into()),
    );
    let wanted: Vec<String> = args.iter().skip(1).map(|s| s.to_lowercase()).collect();
    std::fs::create_dir_all(&out).expect("output directory");
    let wants = |name: &str| wanted.is_empty() || wanted.iter().any(|w| w == name);

    engine_core::log::init("warn");
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
        s.film_grain = 0.0;
        s.vignette = 0.0;
    }
    let mut gallery = Gallery {
        app,
        out,
        spawned: Vec::new(),
    };

    for preset in MaterialPreset::ALL {
        let name = file_name(preset);
        if wants(&name) {
            gallery.preset_shot(preset);
            gallery.capture(&name);
        }
    }
    if wants("overview") {
        gallery.overview();
        gallery.capture("overview");
    }
    for (shot, preset, floor) in [
        ("closeup_brick", MaterialPreset::Brick, false),
        ("closeup_rock", MaterialPreset::Rock, false),
        ("closeup_wood", MaterialPreset::WoodPlanks, true),
    ] {
        if wants(shot) {
            for parallax in [true, false] {
                gallery.close_up(preset, floor, parallax);
                let suffix = if parallax { "" } else { "_flat" };
                gallery.capture(&format!("{shot}{suffix}"));
            }
        }
    }
}

fn file_name(preset: MaterialPreset) -> String {
    preset.name().replace(' ', "_")
}

struct Gallery {
    app: App,
    out: std::path::PathBuf,
    spawned: Vec<Entity>,
}

impl Gallery {
    fn spawn(&mut self, bundle: impl Bundle) {
        let id = self.app.world.spawn(bundle).id();
        self.spawned.push(id);
    }

    fn clear(&mut self) {
        for e in self.spawned.drain(..) {
            self.app.world.despawn(e);
        }
    }

    fn capture(&mut self, name: &str) {
        // Enough frames for temporal anti-aliasing and exposure to settle.
        self.app.update_n(24);
        if let Some(d) = self.app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU ({}), nothing rendered", d.0);
            std::process::exit(0);
        }
        let path = self.out.join(format!("{name}.png"));
        capture_now(&mut self.app.world, &path).expect("screenshot");
        println!("{}", path.display());
    }

    fn camera(&mut self, from: DVec3, to: DVec3, fov: f32) {
        self.spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 14.6 },
                ..Camera::perspective(fov)
            },
            Transform::from_translation(from).looking_at(to, Vec3::Y),
        ));
    }

    fn sun(&mut self, toward: DVec3) {
        self.spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(toward, Vec3::Y),
        ));
    }

    fn ground(&mut self) {
        self.spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Plane {
                    size: [80.0, 80.0],
                    subdivisions: 1,
                },
                StandardMaterial::hex(0x6f6a62).with_roughness(0.9),
            ),
        ));
    }

    /// A triplanar sphere and a beveled cube in the preset, standing on a neutral floor.
    fn preset_shot(&mut self, preset: MaterialPreset) {
        self.clear();
        self.camera(DVec3::new(0.0, 1.35, 3.7), DVec3::new(0.0, 0.75, 0.0), 45.0);
        self.sun(DVec3::new(0.55, -0.55, -0.62));
        self.ground();
        let material = StandardMaterial::preset(preset);
        self.spawn((
            Transform::from_xyz(-0.95, 0.8, 0.0),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.8,
                    segments: 96,
                },
                material.with_triplanar(),
            ),
        ));
        self.spawn((
            Transform::from_xyz(1.0, 0.7, -0.1).with_rotation(Quat::from_rotation_y(-0.55)),
            MeshRenderer::new(
                Shape::RoundedCuboid {
                    size: [1.4, 1.4, 1.4],
                    radius: 0.06,
                    segments: 4,
                },
                material,
            ),
        ));
    }

    /// Every preset as a sphere and a small cube, in a 5 × 3 grid facing the camera.
    fn overview(&mut self) {
        self.clear();
        self.camera(DVec3::new(0.0, 2.2, 9.2), DVec3::new(0.0, 2.05, 0.0), 40.0);
        self.sun(DVec3::new(0.5, -0.5, -0.7));
        self.ground();
        for (i, preset) in MaterialPreset::ALL.into_iter().enumerate() {
            let (col, row) = ((i % 5) as f64, (i / 5) as f64);
            let x = (col - 2.0) * 1.9;
            let y = 3.45 - row * 1.25;
            let material = StandardMaterial::preset(preset);
            self.spawn((
                Transform::from_xyz(x - 0.38, y, 0.0),
                MeshRenderer::new(
                    Shape::Sphere {
                        radius: 0.45,
                        segments: 64,
                    },
                    material.with_triplanar(),
                ),
            ));
            self.spawn((
                Transform::from_xyz(x + 0.5, y, 0.0).with_rotation(Quat::from_rotation_y(-0.6)),
                MeshRenderer::new(
                    Shape::RoundedCuboid {
                        size: [0.6, 0.6, 0.6],
                        radius: 0.03,
                        segments: 3,
                    },
                    material,
                ),
            ));
        }
    }

    /// A wall (or floor) in the preset seen up close at an angle, with the sun grazing it.
    fn close_up(&mut self, preset: MaterialPreset, floor: bool, parallax: bool) {
        self.clear();
        let mut material = StandardMaterial::preset(preset);
        if !parallax {
            material.parallax_depth = 0.0;
        }
        if floor {
            self.camera(DVec3::new(0.0, 1.1, 1.6), DVec3::new(0.25, 0.0, -0.6), 55.0);
            // Sun 12 degrees above the floor, from the left.
            self.sun(DVec3::new(0.97, -0.21, -0.1));
            self.spawn((
                Transform::default(),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [12.0, 12.0],
                        subdivisions: 1,
                    },
                    material,
                ),
            ));
        } else {
            self.ground();
            self.camera(DVec3::new(-0.9, 1.5, 1.7), DVec3::new(0.6, 1.3, 0.0), 55.0);
            // Sun 14 degrees off the wall plane, from the right.
            self.sun(DVec3::new(-0.97, -0.35, -0.24));
            self.spawn((
                Transform::from_xyz(0.0, 1.5, -0.2),
                MeshRenderer::new(
                    Shape::RoundedCuboid {
                        size: [8.0, 3.0, 0.4],
                        radius: 0.03,
                        segments: 3,
                    },
                    material,
                ),
            ));
        }
    }
}

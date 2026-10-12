//! Components games put on entities to make them visible.

use engine_assets::Handle;
use engine_core::prelude::*;

use crate::material::StandardMaterial;
use crate::mesh::{Mesh, Shape};

/// How the camera projects.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// 3D perspective with an infinitely far far-plane and reverse-Z depth (precise from
    /// centimeters to light-years).
    Perspective {
        /// Vertical field of view, degrees.
        fov_y_deg: f32,
        /// Near plane, meters.
        near: f32,
    },
    /// Orthographic: 2D games, 2.5D side-scrollers, isometric views, maps.
    Orthographic {
        /// World units visible vertically.
        height: f32,
        near: f32,
        far: f32,
    },
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Perspective {
            fov_y_deg: 65.0,
            near: 0.05,
        }
    }
}

/// How bright the image is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Exposure {
    /// Fixed exposure value at ISO 100. Bright sun ≈ 15, overcast ≈ 12, indoors ≈ 7,
    /// moonlight ≈ -2.
    Manual { ev100: f32 },
    /// Adapts to scene brightness like an eye, within `[min_ev100, max_ev100]`.
    Auto {
        min_ev100: f32,
        max_ev100: f32,
        /// EV per second adaptation speed.
        speed: f32,
        /// Bias in EV (+1 = twice as bright).
        compensation: f32,
    },
}

impl Default for Exposure {
    fn default() -> Self {
        Exposure::Auto {
            min_ev100: -3.0,
            max_ev100: 18.0,
            speed: 2.0,
            compensation: 0.0,
        }
    }
}

/// A camera. The active camera with the highest `order` renders to the screen.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Camera {
    pub projection: Projection,
    pub exposure: Exposure,
    pub active: bool,
    pub order: i32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            projection: Projection::default(),
            exposure: Exposure::default(),
            active: true,
            order: 0,
        }
    }
}

impl Camera {
    pub fn perspective(fov_y_deg: f32) -> Self {
        Self {
            projection: Projection::Perspective {
                fov_y_deg,
                near: 0.05,
            },
            ..Default::default()
        }
    }

    pub fn orthographic(height: f32) -> Self {
        Self {
            projection: Projection::Orthographic {
                height,
                near: -1000.0,
                far: 1000.0,
            },
            ..Default::default()
        }
    }

    pub fn with_exposure(mut self, exposure: Exposure) -> Self {
        self.exposure = exposure;
        self
    }
}

/// Which mesh to draw: a handle, a procedural shape, or an asset path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshRef {
    Handle(Handle<Mesh>),
    Shape(Shape),
    /// Asset key: a file relative to the asset root, optionally with a `#label` naming one
    /// mesh inside it (`"models/rock.glb"` for a single-mesh model,
    /// `"models/tree.glb#Mesh0/Primitive0"`), or a key a mesh was registered under in code
    /// (`Assets::insert_keyed`). Files start loading on first use; the entity appears once
    /// the mesh has loaded.
    Asset(String),
}

impl From<Shape> for MeshRef {
    fn from(s: Shape) -> Self {
        MeshRef::Shape(s)
    }
}

impl From<Handle<Mesh>> for MeshRef {
    fn from(h: Handle<Mesh>) -> Self {
        MeshRef::Handle(h)
    }
}

/// Which material to use: a handle, inline values, or an asset path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialRef {
    Handle(Handle<StandardMaterial>),
    Inline(StandardMaterial),
    /// Asset key, like [`MeshRef::Asset`]: `"models/tree.glb#Material0"`, or a key a material
    /// was registered under in code.
    Asset(String),
}

impl Default for MaterialRef {
    fn default() -> Self {
        MaterialRef::Inline(StandardMaterial::default())
    }
}

impl From<StandardMaterial> for MaterialRef {
    fn from(m: StandardMaterial) -> Self {
        MaterialRef::Inline(m)
    }
}

impl From<Handle<StandardMaterial>> for MaterialRef {
    fn from(h: Handle<StandardMaterial>) -> Self {
        MaterialRef::Handle(h)
    }
}

/// Draw a mesh with a material at this entity's transform.
///
/// ```ignore
/// commands.spawn((
///     Transform::from_xyz(0.0, 1.0, 0.0),
///     MeshRenderer::new(Shape::Sphere { radius: 0.5, segments: 32 }, StandardMaterial::hex(0xd94f30)),
/// ));
/// ```
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct MeshRenderer {
    pub mesh: MeshRef,
    pub material: MaterialRef,
    pub cast_shadows: bool,
    pub visible: bool,
}

impl Default for MeshRenderer {
    fn default() -> Self {
        Self {
            mesh: MeshRef::Shape(Shape::Cuboid { size: [1.0; 3] }),
            material: MaterialRef::default(),
            cast_shadows: true,
            visible: true,
        }
    }
}

impl MeshRenderer {
    pub fn new(mesh: impl Into<MeshRef>, material: impl Into<MaterialRef>) -> Self {
        Self {
            mesh: mesh.into(),
            material: material.into(),
            cast_shadows: true,
            visible: true,
        }
    }

    pub fn without_shadows(mut self) -> Self {
        self.cast_shadows = false;
        self
    }
}

/// Handles resolved from a [`MeshRenderer`] (managed by the engine).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ResolvedMesh {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

/// The sun or moon: parallel light along the entity's −Z (forward) axis.
/// The weather module normally drives the sun from time of day.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct DirectionalLight {
    /// Linear RGB.
    pub color: [f32; 3],
    /// Lux at the ground (clear noon sun ≈ 110 000, overcast ≈ 10 000, full moon ≈ 0.3).
    pub illuminance: f32,
    pub cast_shadows: bool,
    /// Angular diameter of the light's disc, degrees (sun ≈ 0.53).
    pub angular_size_deg: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            color: [1.0, 0.96, 0.9],
            illuminance: 110_000.0,
            cast_shadows: true,
            angular_size_deg: 0.53,
        }
    }
}

/// Omnidirectional light (torches, lamps, glowing mushrooms, spell bolts).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct PointLight {
    /// Linear RGB.
    pub color: [f32; 3],
    /// Luminous power, lumens (candle ≈ 12, 60 W bulb ≈ 800, campfire ≈ 2 000, streetlight ≈ 20 000).
    pub intensity_lm: f32,
    /// Light has no effect beyond this distance, meters.
    pub range: f32,
    /// Radius of the emitting sphere, meters (softens highlights).
    pub radius: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: [1.0, 0.85, 0.6],
            intensity_lm: 800.0,
            range: 20.0,
            radius: 0.05,
        }
    }
}

/// Extra ambient fill light added everywhere (on top of the sky's light). Keep it small: the
/// sky environment map already lights outdoor scenes; this keeps unlit interiors from being
/// pitch black.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmbientLight {
    /// Linear RGB ambient color.
    pub color: [f32; 3],
    /// Luminance, cd/m² (0.05 ≈ starlight fill, 50 ≈ dim room).
    pub brightness: f32,
}

impl Default for AmbientLight {
    fn default() -> Self {
        Self {
            color: [0.6, 0.7, 1.0],
            brightness: 0.05,
        }
    }
}

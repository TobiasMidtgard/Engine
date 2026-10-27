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

/// The part of the window a camera draws into, in physical pixels from the window's top-left
/// corner (put it on the camera's entity). Without one the camera fills the window. The
/// camera's aspect ratio follows the rectangle; the rest of the window is black under the
/// overlays (UI, editor panels), which still cover the whole window.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Viewport {
    pub position: UVec2,
    pub size: UVec2,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            position: UVec2::ZERO,
            size: UVec2::splat(u32::MAX),
        }
    }
}

impl Viewport {
    pub fn new(position: UVec2, size: UVec2) -> Self {
        Self { position, size }
    }

    /// The rectangle inside a window of `window` pixels (position, size), at least 1×1.
    pub fn within(&self, window: UVec2) -> (UVec2, UVec2) {
        let window = window.max(UVec2::ONE);
        let position = self.position.min(window - UVec2::ONE);
        let size = self.size.min(window - position).max(UVec2::ONE);
        (position, size)
    }
}

/// Renders this camera as an extra view, besides the main one (the active [`Camera`] with the
/// highest `order` that has no `ExtraView`): split screen, picture-in-picture, a scope or a kill
/// cam in a corner, a monitor, mirror or preview showing what another camera sees. Each extra
/// view keeps its own history (anti-aliasing, exposure, shadows, fog), as Unreal's view states
/// and Bevy's per-camera targets do, and renders at its own quality.
///
/// Extra views render before the main view, lowest `Camera::order` first, while their camera
/// is `active`. They cost about what the main view costs at their size (the scene is culled and
/// drawn again), so keep them small or give them a lower `quality`.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtraView {
    /// Where the view goes.
    pub target: ViewTarget,
    /// Quality for this view instead of the game's (`Scalability`), e.g.
    /// `Scalability::preset(QualityPreset::Low)` for an inset.
    pub quality: Option<engine_world::Scalability>,
}

impl ExtraView {
    /// Into the window, at the camera's [`Viewport`].
    pub fn window() -> Self {
        Self::default()
    }

    /// Into `image`, `size` pixels.
    pub fn image(image: Handle<crate::Image>, size: UVec2) -> Self {
        Self {
            target: ViewTarget::Image { image, size },
            quality: None,
        }
    }

    pub fn with_quality(mut self, quality: engine_world::Scalability) -> Self {
        self.quality = Some(quality);
        self
    }
}

/// Where an [`ExtraView`] goes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum ViewTarget {
    /// Into the window at the camera's [`Viewport`] (the whole window without one), over the
    /// main view and under the overlays (UI): split screen, picture-in-picture.
    #[default]
    Window,
    /// Into an image `size` pixels large, which materials (`base_color_texture`, emissive) and
    /// UI images show from the same frame on. Its pixels live on the GPU only: make the image
    /// with [`crate::Image::render_target`].
    Image {
        image: Handle<crate::Image>,
        size: UVec2,
    },
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

/// Keeps a dense [`MeshRenderer`] off the virtual geometry cluster path, so it is always drawn
/// instanced: for debris, fragments and other meshes that change often. Without it, meshes of
/// at least `RenderSettings::virtual_geometry_min_triangles` are drawn as clusters.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NoVirtualGeometry;

/// Keeps an instanced [`MeshRenderer`] at full detail at every distance, in the view and in
/// shadow maps: for meshes the automatic levels of detail would spoil, such as one mesh of many
/// small separate stones (simplifying drops whole stones). Without it, meshes of at least
/// `mesh_lod::MIN_TRIANGLES` get simplified levels picked by their error on screen
/// (`RenderSettings::mesh_lod_pixels`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NoMeshLod;

/// Farthest distance from the camera, in meters, at which this entity's [`MeshRenderer`] is
/// drawn (with its shadow): small props, clutter and details nobody sees from afar. Measured to
/// the mesh's bounding sphere, so a large mesh disappears when all of it is farther. The default
/// (`f32::MAX`) draws it at any distance.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MaxDrawDistance(pub f32);

impl Default for MaxDrawDistance {
    fn default() -> Self {
        Self(f32::MAX)
    }
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
            ..Default::default()
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

/// A side of a box, in the box's local axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BoxFace {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl BoxFace {
    /// Bit of this face in the GPU face mask (+x, -x, +y, -y, +z, -z).
    pub fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// An indoor space: a cave, room, tunnel or ship interior. Inside this box sky light only comes
/// in through its open faces, fading from full at an open face to `sky_light` deeper inside.
///
/// The sky occlusion map sees cover from above only, so it can't tell that a cave's walls keep
/// out the low sky; this volume says so. Fit the box to the inside of the walls (it follows the
/// entity's transform, so a ship's interior moves and turns with the ship; scale is ignored,
/// size it with `half_extents`). Lamps, emissive surfaces and sunlight through openings still
/// light the inside as usual.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct SkyLightVolume {
    /// Half the box size along its local x, y and z, meters.
    pub half_extents: [f32; 3],
    /// Fraction of the sky light left deep inside (0: only lamps and glowing things light it).
    pub sky_light: f32,
    /// Distance over which light from an open face dies out, meters.
    pub fade: f32,
    /// Faces sky light comes in through: a cave mouth, a doorway, an open hangar door.
    pub open_faces: Vec<BoxFace>,
}

impl Default for SkyLightVolume {
    fn default() -> Self {
        Self {
            half_extents: [5.0, 2.5, 5.0],
            sky_light: 0.0,
            fade: 8.0,
            open_faces: Vec::new(),
        }
    }
}

impl SkyLightVolume {
    /// A closed box of the given half size.
    pub fn new(half_extents: [f32; 3]) -> Self {
        Self {
            half_extents,
            ..Default::default()
        }
    }

    /// Let sky light in through `face`.
    pub fn open(mut self, face: BoxFace) -> Self {
        if !self.open_faces.contains(&face) {
            self.open_faces.push(face);
        }
        self
    }

    pub fn with_sky_light(mut self, sky_light: f32) -> Self {
        self.sky_light = sky_light;
        self
    }

    pub fn with_fade(mut self, fade: f32) -> Self {
        self.fade = fade;
        self
    }
}

/// Shape of a [`FogVolume`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FogShape {
    /// A box: rooms, corridors, a bank of mist over a river.
    #[default]
    Box,
    /// An ellipsoid inscribed in the box: puffs of smoke, steam, a cloud of spores.
    Ellipsoid,
}

/// Local fog: a box or ellipsoid of mist, dust, smoke or steam, drawn by the volumetric fog
/// (`RenderSettings::volumetric_fog`).
///
/// It is lit like the height fog ([`FogSettings`](crate::settings::FogSettings)): sunbeams
/// through windows and leaves, sky light where the sky is visible, lamps glowing in it. It
/// follows its entity's transform, scale included (`half_extents` are in the entity's local
/// units). Up to 64 volumes within `volumetric_fog_range` of the camera are drawn, nearest
/// first; they fade out over the last 15% of the range. Fire, smoke and weather modules spawn
/// and animate these; everything is plain data.
///
/// ```ignore
/// // Dusty air filling a 10 x 4 x 8 m room, with drifting wisps.
/// commands.spawn((
///     Transform::from_xyz(0.0, 2.0, 0.0),
///     FogVolume::new([5.0, 2.0, 4.0]).with_density(0.08).with_noise(0.7, 1.5),
/// ));
/// ```
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct FogVolume {
    pub shape: FogShape,
    /// Half the size along the local x, y and z axes (before the entity's scale), meters.
    pub half_extents: [f32; 3],
    /// Extinction per meter where the fog is full (0.02 light mist, 0.1 thick fog, 0.5 smoke
    /// you can't see a hand through). Adds to the height fog.
    pub density: f32,
    /// Albedo, linear RGB: the share of stopped light that is scattered (0.9 mist, 0.3 soot).
    pub albedo: [f32; 3],
    /// Light the fog gives off itself, linear RGB radiance per meter of fog (cd/m² per m;
    /// glowing embers in smoke, spores, magic). Scaled by the edge fade and noise like the
    /// density, and visible even where `density` is 0.
    pub emission: [f32; 3],
    /// Distance over which the fog fades in from the surface of the shape, meters (0 = a hard
    /// edge).
    pub edge_falloff: f32,
    /// 0..1 how much animated 3D noise breaks the fog into wisps and holes (0 = uniform). The
    /// noise drifts with the wind.
    pub noise: f32,
    /// Size of the noise's wisps, meters.
    pub noise_scale: f32,
    /// Henyey-Greenstein anisotropy `g` (-0.95..0.95), as `FogSettings::sun_scattering`.
    pub anisotropy: f32,
}

impl Default for FogVolume {
    fn default() -> Self {
        Self {
            shape: FogShape::Box,
            half_extents: [5.0, 2.5, 5.0],
            density: 0.05,
            albedo: [0.9, 0.9, 0.9],
            emission: [0.0; 3],
            edge_falloff: 1.0,
            noise: 0.0,
            noise_scale: 2.0,
            anisotropy: 0.5,
        }
    }
}

impl FogVolume {
    /// A box of fog of the given half size (meters).
    pub fn new(half_extents: [f32; 3]) -> Self {
        Self {
            half_extents,
            ..Default::default()
        }
    }

    /// An ellipsoid of fog with the given radii (meters).
    pub fn ellipsoid(radii: [f32; 3]) -> Self {
        Self {
            shape: FogShape::Ellipsoid,
            half_extents: radii,
            ..Default::default()
        }
    }

    pub fn with_density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }

    pub fn with_albedo(mut self, albedo: [f32; 3]) -> Self {
        self.albedo = albedo;
        self
    }

    pub fn with_emission(mut self, emission: [f32; 3]) -> Self {
        self.emission = emission;
        self
    }

    pub fn with_edge_falloff(mut self, meters: f32) -> Self {
        self.edge_falloff = meters;
        self
    }

    /// Break the fog up with drifting noise: `strength` 0..1, wisps `scale` meters across.
    pub fn with_noise(mut self, strength: f32, scale: f32) -> Self {
        self.noise = strength;
        self.noise_scale = scale;
        self
    }

    pub fn with_anisotropy(mut self, g: f32) -> Self {
        self.anisotropy = g;
        self
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

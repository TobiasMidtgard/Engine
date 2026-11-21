//! What a feature tells the ray scene ([`crate::RenderFeature::ray_scene`]): the triangles it
//! has ([`RaySceneOut::geometry`]) and where they stand ([`RaySceneOut::place`]).

use engine_core::prelude::*;

/// A feature's own name for one of its geometries or one of its instances. Each feature's
/// names are kept apart, and so are its geometries' from its instances'.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RayKey(pub u64, pub u64);

/// What an instance is to a ray: a ray names the classes it sees (an instance's class is its
/// mask in the top-level structures).
///
/// The frame's rays name two sets so far (lib_ray.wgsl): a ray that asks what it meets (probe
/// light, world reflections) sees `Solid`, `Ground`, `Water` and `NoCast`; a shadow ray
/// (toward the sun or a lamp) sees `Solid`, `Ground` and `ShadowOnly`, which is what the
/// shadow maps draw. Every triangle of what a ray sees stops it, but for a `ShadowOnly`
/// stand-in, which stops a shadow ray only where the ray goes into it (at its outer faces).
/// The rays that see `Sway`, `Leaf` and `Small`, and the asking of single triangles (a `Leaf`
/// card's texture, the back of the `Ground`), come with the steps that place such things;
/// until then no ray sees an instance of those three classes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum RayClass {
    /// Casts a shadow and holds its shape: buildings, rocks, props of 0.3 m and more.
    Solid = 1,
    /// The ground (its triangles will be asked one by one, so that a ray may pass its back).
    Ground = 2,
    /// The opaque parts of plants that move in the wind (trunks, branches).
    Sway = 4,
    /// Cards cut out by their texture: leaves, fences.
    Leaf = 8,
    /// A body of water's surface: light passes it.
    Water = 16,
    /// Casts no shadow: probes and reflections see it, shadow rays pass.
    NoCast = 32,
    /// A caster smaller than 0.3 m, for the sun's own rays and reflections; probes pass.
    Small = 64,
    /// A stand-in that casts the shadow of a thing that casts none itself and is drawn in no
    /// view ([`crate::ShadowOnly`]): shadow rays alone see it, and it stops one only at a
    /// face whose corners run counter-clockwise as the ray sees them (the outside of a closed
    /// shape). The thing it casts for may lie within it, and light that leaves a stand-in is
    /// not stopped by it: such a thing were shaded by its own stand-in otherwise. An open
    /// sheet therefore stops light from its drawn side only.
    ShadowOnly = 128,
}

impl RayClass {
    /// Its bit in a ray's mask.
    pub const fn bit(self) -> u8 {
        self as u8
    }

    /// Whether its triangles stop a ray without being asked: what a feature passes as
    /// `opaque` for the geometries of its instances ([`RaySceneOut::geometry`]). Those of
    /// the other classes are built to be asked one by one, once the rays do.
    pub const fn opaque(self) -> bool {
        !matches!(self, RayClass::Ground | RayClass::Leaf)
    }
}

/// Where the rays are shot from this frame, and how far features should bring their things.
#[derive(Clone, Copy, Debug)]
pub struct RayView {
    /// The camera, world space.
    pub eye: DVec3,
    /// The frame's number (the one `still_from` in [`RaySceneOut::place`] counts in).
    pub frame: u64,
    /// Changes when the camera jumped or a reach changed: look at everything again.
    pub epoch: u32,
    /// The frame of the last world shift the scene followed (0: none): it moved its
    /// instances itself. A thing a feature saw move in that frame moved with the world,
    /// unless it also moved by itself: one not in the scene yet is best taken as holding
    /// still (placed again, should it go on moving, it is a mover from then).
    pub shifted: u64,
    /// Everything that casts a shadow within this distance of the camera belongs in the
    /// scene, whatever its size (meters). (The meshes leave out what is under 0.3 m in
    /// radius until the rays that see [`RayClass::Small`] are there.)
    pub caster_reach: f32,
    /// Beyond `caster_reach`, up to here: what is at least `cut` times its distance across.
    pub far_reach: f32,
    /// Radius over distance below which a far thing is left out.
    pub cut: f32,
    /// Patches of ground a terrain may bring.
    pub ground_patches: u32,
}

/// Where a geometry's triangles are.
pub enum RaySource<'a> {
    /// In GPU buffers that carry `BufferUsages::BLAS_INPUT` (a feature may ask for that usage
    /// only where [`crate::GpuShared::ray_queries`] is true). A vertex starts with its position,
    /// three `f32`; `vertex_stride` is a multiple of 4. Indices count from `first_vertex`.
    Buffers {
        vertex: &'a wgpu::Buffer,
        first_vertex: u32,
        vertex_count: u32,
        vertex_stride: u32,
        index: &'a wgpu::Buffer,
        index_format: wgpu::IndexFormat,
        first_index: u32,
        index_count: u32,
    },
    /// On the CPU: copied to the GPU for the build.
    Cpu {
        positions: &'a [[f32; 3]],
        indices: &'a [u32],
    },
    /// The square from (-1, 0, -1) to (1, 0, 1), facing up.
    UnitSquare,
    /// The disc of radius 1 around the origin in the plane y = 0, facing up.
    UnitDisc,
}

/// What the scene has of a geometry after [`RaySceneOut::geometry`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RayGeometryState {
    /// Built for this version (in this frame's commands at the latest): place its instances.
    Ready,
    /// An older version's serves (its instances may be placed) while this one waits (built
    /// again lately, or the frame's allowance is spent): ask again in a later frame.
    Older,
    /// Not yet (the frame's build allowance is spent): ask again next frame.
    Building,
    /// Never: no triangle, or more than the device takes.
    Refused,
}

/// How a hit looks to a ray that asks ([`RaySceneOut::place`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RayLook {
    /// Mean diffuse colour (linear), what a hit reflects of the light falling on it.
    pub albedo: [f32; 3],
    pub roughness: f32,
    /// Its own glow, cd/m².
    pub emissive: [f32; 3],
    pub metallic: f32,
    /// Its layer in the ray tiles, where its colour varies.
    pub tile: Option<u16>,
    /// The texture's scale (xy) and offset (zw) over the mesh's coordinates.
    pub uv: [f32; 4],
    /// Alpha below which a [`RayClass::Leaf`] texel is a hole.
    pub cutoff: f32,
    /// The share of a [`RayClass::Leaf`] card its texture covers.
    pub coverage: f32,
    /// Lit from both sides; else a ray that meets its back is inside something.
    pub double_sided: bool,
}

impl Default for RayLook {
    fn default() -> Self {
        Self {
            albedo: [0.5; 3],
            roughness: 1.0,
            emissive: [0.0; 3],
            metallic: 0.0,
            tile: None,
            uv: [1.0, 1.0, 0.0, 0.0],
            cutoff: 0.5,
            coverage: 1.0,
            double_sided: false,
        }
    }
}

/// The scene, as one feature fills it in [`crate::RenderFeature::ray_scene`].
pub struct RaySceneOut<'a> {
    pub(super) scene: &'a mut super::RayScene,
    /// The feature's place among the renderer's (its keys' name space).
    pub(super) feature: u16,
}

impl RaySceneOut<'_> {
    /// Geometry `key` at `version`: built now if this frame's allowance holds it. `opaque`:
    /// every triangle stops a ray ([`RayClass::opaque`] of the class its instances have); it
    /// cannot change for a key. Cheap to ask every frame while the answer is
    /// [`RayGeometryState::Building`].
    pub fn geometry(
        &mut self,
        key: RayKey,
        version: u64,
        opaque: bool,
        source: RaySource<'_>,
    ) -> RayGeometryState {
        self.scene
            .geometry(self.feature, key, version, opaque, source)
    }

    /// Whether geometry `key` is in the scene at `version` already, without its source at
    /// hand (so a feature can skip fetching it).
    pub fn has_geometry(&mut self, key: RayKey, version: u64) -> bool {
        self.scene.has_geometry(self.feature, key, version)
    }

    /// Instance `id` of geometry `geometry` stands at `to_world` (mesh to world) from now on.
    /// `still_from`: the frame from which it holds still (0: it never moves); until then it
    /// is in the structure that is built again every frame. Placing an instance as it stands
    /// already changes nothing. False when the geometry has no structure yet, or the scene is
    /// full.
    pub fn place(
        &mut self,
        id: RayKey,
        geometry: RayKey,
        to_world: &DAffine3,
        class: RayClass,
        still_from: u64,
        look: &RayLook,
    ) -> bool {
        self.scene.place(
            self.feature,
            id,
            geometry,
            to_world,
            class,
            still_from,
            look,
        )
    }

    /// Instance `id` is gone (nothing happens if it was never placed).
    pub fn remove(&mut self, id: RayKey) {
        self.scene.remove(self.feature, id);
    }

    /// Whether instance `id` is placed (in a structure, or waiting for room in one).
    pub fn has(&self, id: RayKey) -> bool {
        self.scene.has(self.feature, id)
    }

    /// The feature looked at everything it has since frame `since` (a [`RayView::frame`]):
    /// its instances that were not placed since then are gone. For a feature that cannot
    /// name what left (it lost count, or starts over after [`RayView::epoch`] changed).
    pub fn remove_unplaced_since(&mut self, since: u64) {
        self.scene.remove_unseen(self.feature, since);
    }

    /// The version geometry `key` has a structure for (`None`: it has none), without asking
    /// for it: a geometry nothing asks for or uses is forgotten after a while.
    pub fn geometry_version(&self, key: RayKey) -> Option<u64> {
        self.scene.geometry_version(self.feature, key)
    }

    /// Instances placed with geometry `key` (0 also when the scene has no such geometry).
    pub fn geometry_users(&self, key: RayKey) -> u32 {
        self.scene.geometry_users(self.feature, key)
    }

    /// Whether instance `id` is in a structure as it was last built, before this frame's
    /// builds, in the slot it has now. One placed for the first time this frame is not: this
    /// frame's rays trace it if it moves, and those from the still structure's next build on
    /// (a few frames at most) if it holds still. Nor is one that held still and began to move
    /// this frame, though the still structure shows it where it stood until its next build.
    pub fn is_live(&self, id: RayKey) -> bool {
        self.scene.is_live(self.feature, id)
    }
}

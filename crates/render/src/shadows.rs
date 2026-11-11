//! Cascaded shadow maps for the sun.
//!
//! Each cascade is a square around the camera covering everything within its split distance
//! (so it doesn't depend on where the camera looks), its center snapped to whole shadow texels
//! in world space (f64), so shadows don't shimmer when the camera moves or turns. Casters
//! between the sun and a cascade are "pancaked" onto its near plane in the shader.
//!
//! Every map (the cascades, the far map, the sky occlusion map) keeps a **static image**: the
//! casters that haven't moved for a while, drawn only when the map moves past its margin, the
//! sun turns, or what is in it changes (Unity HDRP's mixed cached shadows, Insomniac's cached
//! cascades, Unreal's static virtual shadow map pages). Each frame the static image is copied
//! and the moving casters are drawn on top ([`ShadowCache`]).

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::settings::RenderSettings;

pub const MAX_CASCADES: usize = 4;

/// Per-frame cascade data (camera-relative).
#[derive(Debug, Clone, Copy)]
pub struct Cascades {
    pub count: u32,
    pub map_size: u32,
    /// Camera-relative → light clip space.
    pub view_proj: [Mat4; MAX_CASCADES],
    /// Light view (camera-relative → light space, looking down −Z).
    pub light_view: [Mat4; MAX_CASCADES],
    /// Half extent of each cascade's square, meters.
    pub half_extent: [f32; MAX_CASCADES],
    /// Depth range of each cascade, meters.
    pub depth_range: [f32; MAX_CASCADES],
    /// Far view distance of each cascade.
    pub splits: [f32; 4],
    /// Center of each cascade's square, absolute world position (snapped to its texels).
    pub center: [DVec3; MAX_CASCADES],
    /// The direction toward the sun each cascade was drawn with.
    pub sun_dir: [Vec3; MAX_CASCADES],
    /// Whether each cascade's static image is drawn again this frame (see [`ShadowCache`]):
    /// features draw their still casters for it (`RenderFeature::draw_shadow_static`) only then.
    pub static_redraw: [bool; MAX_CASCADES],
    /// Where each cascade's casters can be for this frame's view (see [`CasterVolume`]).
    pub casters: [CasterVolume; MAX_CASCADES],
    /// Corners of the part of the view each cascade shades (camera-relative; see
    /// [`CasterVolume`] for the corner order).
    pub slice: [[Vec3; 8]; MAX_CASCADES],
}

impl Default for Cascades {
    fn default() -> Self {
        Self {
            count: 0,
            map_size: 2048,
            view_proj: [Mat4::IDENTITY; MAX_CASCADES],
            light_view: [Mat4::IDENTITY; MAX_CASCADES],
            half_extent: [0.0; MAX_CASCADES],
            depth_range: [0.0; MAX_CASCADES],
            splits: [0.0; 4],
            center: [DVec3::ZERO; MAX_CASCADES],
            sun_dir: [Vec3::Y; MAX_CASCADES],
            static_redraw: [true; MAX_CASCADES],
            casters: [CasterVolume::default(); MAX_CASCADES],
            slice: [[Vec3::ZERO; 8]; MAX_CASCADES],
        }
    }
}

impl Cascades {
    /// Whether a camera-relative bounding sphere can cast a shadow into cascade `i`: inside its
    /// square, and able to shade the part of the view the cascade covers (see
    /// [`CasterVolume`]). Spheres toward the sun are kept (they cast onto the cascade from
    /// outside it).
    pub fn sphere_in_cascade(&self, i: usize, center: Vec3, radius: f32) -> bool {
        self.sphere_in_square(i, center, radius) && self.casters[i].contains_sphere(center, radius)
    }

    /// Whether a camera-relative bounding sphere can cast into cascade `i` anywhere in its
    /// square, whatever the camera looks at: what its static image holds.
    pub fn sphere_in_square(&self, i: usize, center: Vec3, radius: f32) -> bool {
        let p = self.light_view[i].transform_point3(center);
        let e = self.half_extent[i] + radius;
        p.x.abs() <= e && p.y.abs() <= e && -p.z - radius <= self.depth_range[i]
    }

    /// Point cascade `i` at `center` (absolute, on its texel grid) for a camera at `origin`.
    fn place(&mut self, i: usize, center: DVec3, origin: DVec3, sun_dir: Vec3, radius: f32) {
        let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
        let center_rel = (center - origin).as_vec3();
        // Eye sits behind the sphere toward the sun; depth covers the sphere plus extra room.
        let back = radius * 2.0 + 50.0;
        let eye = center_rel + sun_dir * back;
        let light_view =
            Mat4::from_quat(light_rot.as_quat().inverse()) * Mat4::from_translation(-eye);
        let depth = back + radius;
        let light_proj = crate::frame::orthographic_standard(radius, 0.0, depth);
        self.light_view[i] = light_view;
        self.view_proj[i] = light_proj * light_view;
        self.half_extent[i] = radius;
        self.depth_range[i] = depth;
        self.center[i] = center;
        self.sun_dir[i] = sun_dir;
    }
}

/// Where a cascade's casters can be: the frustum slice the cascade shades, swept toward the sun.
///
/// A cascade's square is fitted to a sphere around its slice, so most of it lies outside the
/// view; casters there shade nothing anyone sees. Receiver-aware caster culling (Unity's
/// `ScriptableCullingParameters` shadow caster culling since 2022.2, Unreal's shadow caster
/// frustum built from the view frustum and the light) keeps only casters inside the slice's
/// shadow volume: the slice's faces that face the sun plus planes through its silhouette edges
/// along the sun direction. Planes are camera-relative with inward normals (`n·p + w >= 0`
/// inside). An empty volume keeps everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct CasterVolume {
    planes: [Vec4; 16],
    count: usize,
    /// Added to every caster's radius (m): the shadow filter's reach.
    margin: f32,
}

/// Corner indices of each face of a frustum slice (near 0..4, far 4..8, both in the order
/// −x−y, +x−y, +x+y, −x+y).
const FACES: [[usize; 4]; 6] = [
    [0, 1, 2, 3],
    [4, 5, 6, 7],
    [0, 3, 7, 4],
    [1, 2, 6, 5],
    [0, 1, 5, 4],
    [3, 2, 6, 7],
];
/// Each edge of a frustum slice and the two faces (indices into `FACES`) that meet there.
const EDGES: [(usize, usize, usize, usize); 12] = [
    (0, 1, 0, 4),
    (1, 2, 0, 3),
    (2, 3, 0, 5),
    (3, 0, 0, 2),
    (4, 5, 1, 4),
    (5, 6, 1, 3),
    (6, 7, 1, 5),
    (7, 4, 1, 2),
    (0, 4, 2, 4),
    (1, 5, 4, 3),
    (2, 6, 3, 5),
    (3, 7, 5, 2),
];

impl CasterVolume {
    /// The shadow volume of the slice with camera-relative `corners` for a sun toward `sun_dir`.
    pub fn new(corners: &[Vec3; 8], sun_dir: Vec3, margin: f32) -> Self {
        let inside = corners.iter().copied().sum::<Vec3>() / 8.0;
        let plane = |n: Vec3, a: Vec3| {
            let n = if n.dot(inside - a) < 0.0 { -n } else { n };
            n.extend(-n.dot(a))
        };
        let mut faces = [Vec4::ZERO; 6];
        for (face, f) in faces.iter_mut().zip(FACES) {
            let [a, b, _, d] = f.map(|i| corners[i]);
            let n = (b - a).cross(d - a).normalize_or_zero();
            *face = plane(n, a);
        }
        let lit = faces.map(|f| f.truncate().dot(sun_dir) >= 0.0);
        let mut out = Self {
            margin,
            ..Default::default()
        };
        for (face, &lit) in faces.iter().zip(&lit) {
            if lit && face.truncate() != Vec3::ZERO {
                out.planes[out.count] = *face;
                out.count += 1;
            }
        }
        for (a, b, f, g) in EDGES {
            if lit[f] == lit[g] {
                continue;
            }
            let (a, b) = (corners[a], corners[b]);
            let n = (b - a).cross(sun_dir);
            if n.length_squared() < 1e-12 * (b - a).length_squared() {
                continue;
            }
            out.planes[out.count] = plane(n.normalize(), a);
            out.count += 1;
        }
        out
    }

    /// Whether a camera-relative bounding sphere may shade the slice.
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        let reach = -(radius + self.margin);
        self.planes[..self.count]
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= reach)
    }
}

/// Share of a map's radius kept as margin: the camera can drift this far (of the map's
/// radius) from where the map's static image was drawn before it is drawn again.
pub const STATIC_MARGIN: f64 = 0.08;
/// The sun may turn this far (radians) before static images are drawn again, one map a frame.
const STATIC_SUN_ANGLE: f32 = 0.0015;
/// Past this, every map is drawn again at once.
const STATIC_SUN_ANGLE_MAX: f32 = 0.006;
/// Shadow map texels a receiver's filter can reach from where it projects (the blocker search
/// and the widest PCF kernel in `cascade_visibility`, plus the normal bias).
const FILTER_TEXELS: f32 = 28.0;

/// Index of the far shadow map among the shadow maps (`RenderFeature::draw_shadow_static`):
/// after the cascades (`0..MAX_CASCADES`) and [`SKY_OCCLUSION_VIEW`].
pub const FAR_SHADOW_MAP: usize = SKY_OCCLUSION_VIEW + 1;
/// Number of shadow maps: the cascades, the sky occlusion map and the far map.
pub const SHADOW_MAPS: usize = FAR_SHADOW_MAP + 1;

/// Most people with an inset shadow map of their own (`RenderSettings::shadow_insets`).
pub const MAX_SHADOW_INSETS: usize = 4;
/// People within this of the camera (m, from the camera to their body) get an inset shadow.
pub const INSET_DISTANCE_M: f32 = 6.0;

/// One person's inset shadow map (`RenderSettings::shadow_insets`): an orthographic sun view
/// fitted around their body, camera-relative. Draw into it with
/// [`crate::RenderFeature::draw_inset_shadow`], only the casters [`ShadowInset::casts`] keeps
/// (the person, what they wear and hold), with the same pipeline as the cascades.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowInset {
    /// The body's bounding sphere (camera-relative center, radius in m).
    pub center: Vec3,
    pub radius: f32,
    /// Camera-relative to the map's clip space (depth 0 toward the sun, as the cascades').
    pub view_proj: Mat4,
}

impl ShadowInset {
    /// The map around the sphere at `center` (camera-relative) of `radius`, lit from `sun_dir`.
    pub fn new(center: Vec3, radius: f32, sun_dir: Vec3) -> Self {
        let radius = radius.max(0.1);
        let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
        // The eye sits just outside the sphere toward the sun; depth spans the sphere.
        let eye = center + sun_dir * (radius + 0.25);
        let light_view =
            Mat4::from_quat(light_rot.as_quat().inverse()) * Mat4::from_translation(-eye);
        let light_proj = crate::frame::orthographic_standard(radius, 0.0, 2.0 * radius + 0.5);
        Self {
            center,
            radius,
            view_proj: light_proj * light_view,
        }
    }

    /// Whether a caster's bounding sphere (camera-relative) can draw into this map: it overlaps
    /// the body's sphere.
    pub fn casts(&self, center: Vec3, radius: f32) -> bool {
        center.distance_squared(self.center) <= (self.radius + radius).powi(2)
    }

    /// Meters per texel of a map `size` texels across.
    pub fn texel(&self, size: u32) -> f32 {
        2.0 * self.radius / size.max(1) as f32
    }
}

/// Where a map's static image was drawn.
#[derive(Debug, Clone, Copy)]
struct Placed {
    center: DVec3,
    sun_dir: Vec3,
    half_extent: f32,
    map_size: u32,
}

impl Placed {
    /// Whether a camera at `origin` with the sun toward `sun_dir` can still use the image (the
    /// sun turned less than [`STATIC_SUN_ANGLE_MAX`]); `drift` is how far it may be from the
    /// center.
    fn holds(
        &self,
        origin: DVec3,
        sun_dir: Vec3,
        half_extent: f32,
        map_size: u32,
        drift: f64,
    ) -> bool {
        self.half_extent == half_extent
            && self.map_size == map_size
            && (origin - self.center).length() <= drift
            && self.sun_dir.angle_between(sun_dir) <= STATIC_SUN_ANGLE_MAX
    }

    fn sun_turned(&self, sun_dir: Vec3) -> bool {
        self.sun_dir.angle_between(sun_dir) > STATIC_SUN_ANGLE
    }
}

/// Keeps each shadow map where its static image was drawn while the camera stays within the
/// map's margin ([`STATIC_MARGIN`]) and the sun holds still, and says which static images are
/// drawn again this frame (`Cascades::static_redraw`, `FarShadowView::static_redraw`,
/// `SkyOcclusionView::static_redraw`). Every map is drawn every frame: its static image
/// copied, the moving casters on top.
#[derive(Debug, Default)]
pub struct ShadowCache {
    cascades: [Option<Placed>; MAX_CASCADES],
    sky: Option<Placed>,
    far: Option<Placed>,
    /// Frames since each map's static image was last drawn.
    since: [u32; SHADOW_MAPS],
    /// Frames a map's static image stands at least before a feature's request draws it again
    /// (`static_holds`; 0 = at once). The camera leaving the margin, the sun and a cut don't
    /// wait.
    pub hold: [u32; SHADOW_MAPS],
}

/// How long each map's static image stands before a change in what is in it draws it again
/// (`ShadowCache::hold`), in frames of `frame_s` seconds, for `RenderSettings::
/// shadow_refresh_secs`: the two nearest cascades not at all, the third that long, the last
/// cascade and the sky occlusion map twice and the far shadow map four times that long. A still
/// caster that starts to move is drawn as a moving one at once; until its map is drawn again its
/// old shadow stays where it stood, in maps whose texels are decimeters to meters wide and that
/// only shade the distance (Cyberpunk 2077 refreshes its cached distant cascades every few
/// seconds; Dybisz, SIGGRAPH 2021).
pub fn static_holds(refresh_secs: f32, frame_s: f32) -> [u32; SHADOW_MAPS] {
    let mut hold = [0; SHADOW_MAPS];
    if !(refresh_secs > 0.0 && frame_s > 0.0) {
        return hold;
    }
    let frames = |times: f32| ((refresh_secs * times / frame_s).ceil() as u32).min(100_000);
    for (i, h) in hold.iter_mut().enumerate().take(MAX_CASCADES).skip(2) {
        *h = frames((i - 1) as f32);
    }
    hold[SKY_OCCLUSION_VIEW] = frames(2.0);
    hold[FAR_SHADOW_MAP] = frames(4.0);
    hold
}

impl ShadowCache {
    /// Point this frame's maps (just computed for a camera at `origin`) back at where their
    /// static images were drawn, or mark them to be drawn again: when the camera left the
    /// margin, the sun turned (one map a frame unless it turned a lot), a feature asked
    /// (`requested`, by map index: what is in it changed), or `fresh` (a camera cut, the first
    /// frame, caching off).
    pub fn apply(
        &mut self,
        cascades: &mut Cascades,
        sky: &mut SkyOcclusionView,
        far: &mut FarShadowView,
        origin: DVec3,
        fresh: bool,
        requested: [bool; SHADOW_MAPS],
    ) {
        // A map whose image was drawn a moment ago lets what changed in it wait (`hold`): the
        // feature asks again every frame until the image is drawn.
        let requested: [bool; SHADOW_MAPS] =
            std::array::from_fn(|i| requested[i] && self.since[i] >= self.hold[i]);
        // The sun turned a little: one map a frame, nearest first.
        let mut sun_spent = false;
        let mut sun_due = |last: &Placed, sun: Vec3| {
            let due = !sun_spent && last.sun_turned(sun);
            sun_spent |= due;
            due
        };
        let count = (cascades.count as usize).min(MAX_CASCADES);
        // `i` indexes the placements, the requests and every per-cascade array of the view.
        #[allow(clippy::needless_range_loop)]
        for i in 0..MAX_CASCADES {
            if i >= count {
                self.cascades[i] = None;
                continue;
            }
            let r = cascades.half_extent[i];
            let drift = r as f64 * STATIC_MARGIN / (1.0 + STATIC_MARGIN);
            let keep = self.cascades[i].filter(|l| {
                !fresh
                    && !requested[i]
                    && l.holds(origin, cascades.sun_dir[i], r, cascades.map_size, drift)
            });
            match keep {
                Some(l) if !sun_due(&l, cascades.sun_dir[i]) => {
                    cascades.place(i, l.center, origin, l.sun_dir, l.half_extent);
                    cascades.static_redraw[i] = false;
                }
                _ => {
                    cascades.static_redraw[i] = true;
                    self.cascades[i] = Some(Placed {
                        center: cascades.center[i],
                        sun_dir: cascades.sun_dir[i],
                        half_extent: r,
                        map_size: cascades.map_size,
                    });
                }
            }
        }
        if sky.enabled {
            let r = sky.extent * 0.5;
            let keep = self.sky.filter(|l| {
                let d = origin - l.center;
                !fresh
                    && !requested[SKY_OCCLUSION_VIEW]
                    && l.half_extent == r
                    && l.map_size == sky.map_size
                    && d.x.hypot(d.z) <= r as f64 * STATIC_MARGIN
                    && d.y.abs() <= sky.depth_range as f64 * 0.1
            });
            match keep {
                Some(l) => {
                    *sky = SkyOcclusionView::around(
                        l.center,
                        origin,
                        sky.map_size,
                        sky.extent,
                        sky.depth_range,
                    );
                    sky.static_redraw = false;
                }
                None => {
                    sky.static_redraw = true;
                    self.sky = Some(Placed {
                        center: sky.center,
                        sun_dir: Vec3::Y,
                        half_extent: r,
                        map_size: sky.map_size,
                    });
                }
            }
        } else {
            self.sky = None;
        }
        if far.enabled {
            let drift = far.half_extent as f64 * STATIC_MARGIN;
            let keep = self.far.filter(|l| {
                !fresh
                    && !requested[FAR_SHADOW_MAP]
                    && l.holds(origin, far.sun_dir, far.half_extent, far.map_size, drift)
            });
            match keep {
                Some(l) if !sun_due(&l, far.sun_dir) => {
                    *far = FarShadowView::around(
                        l.center,
                        origin,
                        l.sun_dir,
                        far.map_size,
                        far.half_extent,
                    );
                    far.static_redraw = false;
                }
                _ => {
                    far.static_redraw = true;
                    self.far = Some(Placed {
                        center: far.center,
                        sun_dir: far.sun_dir,
                        half_extent: far.half_extent,
                        map_size: far.map_size,
                    });
                }
            }
        } else {
            self.far = None;
        }
        for i in 0..SHADOW_MAPS {
            let drawn = match i {
                SKY_OCCLUSION_VIEW => sky.enabled && sky.static_redraw,
                FAR_SHADOW_MAP => far.enabled && far.static_redraw,
                _ => i < count && cascades.static_redraw[i],
            };
            self.since[i] = if drawn {
                0
            } else {
                self.since[i].saturating_add(1)
            };
        }
    }
}

/// Top-down depth map around the camera that records the highest surface over each spot.
///
/// Shading uses it for sky visibility (caves, rooms and overhangs don't get sky light), and
/// weather effects use it to keep rain and snow off covered ground (`sky_visibility` in
/// `lib_frame.wgsl`). Features draw into it from `draw_shadow` with
/// [`SKY_OCCLUSION_VIEW`] as the cascade index, using the same shadow pipeline as for the sun.
#[derive(Debug, Clone, Copy)]
pub struct SkyOcclusionView {
    pub enabled: bool,
    pub map_size: u32,
    /// Width of the covered square, meters.
    pub extent: f32,
    /// Depth covered: from `depth_range / 2` above the camera to as far below it.
    pub depth_range: f32,
    /// Camera-relative → occlusion clip space (orthographic, looking down, standard Z).
    pub view_proj: Mat4,
    /// Camera-relative → occlusion view space (looking down −Z), for culling.
    pub view: Mat4,
    /// Center of the square (absolute; on its texel grid, at the camera's height when placed).
    pub center: DVec3,
    /// Whether its static image is drawn again this frame (see [`ShadowCache`]).
    pub static_redraw: bool,
}

/// Cascade index passed to `RenderFeature::draw_shadow` for the sky occlusion map.
pub const SKY_OCCLUSION_VIEW: usize = MAX_CASCADES;

impl Default for SkyOcclusionView {
    fn default() -> Self {
        Self {
            enabled: false,
            map_size: 512,
            extent: 256.0,
            depth_range: 800.0,
            view_proj: Mat4::IDENTITY,
            view: Mat4::IDENTITY,
            center: DVec3::ZERO,
            static_redraw: true,
        }
    }
}

impl SkyOcclusionView {
    /// Build the view for a camera at world position `origin`. The square is snapped to whole
    /// texels in world space so the map doesn't shimmer as the camera moves.
    pub fn new(origin: DVec3, map_size: u32, extent: f32, depth_range: f32, enabled: bool) -> Self {
        let texel = extent as f64 / map_size.max(1) as f64;
        let snapped = DVec3::new(
            (origin.x / texel).floor() * texel,
            origin.y,
            (origin.z / texel).floor() * texel,
        );
        Self {
            enabled,
            ..Self::around(snapped, origin, map_size, extent, depth_range)
        }
    }

    /// The view centered on `center` (absolute, snapped) for a camera at `origin`.
    pub fn around(
        center: DVec3,
        origin: DVec3,
        map_size: u32,
        extent: f32,
        depth_range: f32,
    ) -> Self {
        let eye = (center - origin).as_vec3() + Vec3::Y * (depth_range * 0.5);
        // Looking straight down (−Y) with −Z as "up" in the map.
        let rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let view = Mat4::from_rotation_translation(rotation, eye).inverse();
        let proj = crate::frame::orthographic_standard(extent * 0.5, 0.0, depth_range);
        Self {
            enabled: true,
            map_size,
            extent,
            depth_range,
            view_proj: proj * view,
            view,
            center,
            static_redraw: true,
        }
    }

    /// Whether a camera-relative bounding sphere touches the covered box.
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        let p = self.view.transform_point3(center);
        let e = self.extent * 0.5 + radius;
        p.x.abs() <= e
            && p.y.abs() <= e
            && -p.z + radius >= 0.0
            && -p.z - radius <= self.depth_range
    }

    /// `sky_occlusion_params` in the frame uniform: enabled, extent, texel size, depth range.
    pub fn params(&self) -> [f32; 4] {
        [
            if self.enabled { 1.0 } else { 0.0 },
            self.extent,
            self.extent / self.map_size.max(1) as f32,
            self.depth_range,
        ]
    }
}

/// Layer of the shadow map array that holds the far shadow map (after the cascades).
pub const FAR_SHADOW_LAYER: usize = MAX_CASCADES;

/// One coarse sun shadow map around the camera for what lies beyond the cascades
/// (`RenderSettings::shadow_distance`), out to `RenderSettings::far_shadow_distance`: mountains
/// shade the valley at low sun and ridges shade themselves kilometers away.
///
/// Only big casters draw into it (terrain, mountains, cliffs, large buildings; see
/// [`FarShadowView::casts`]), from [`RenderFeature::draw_far_shadow`](crate::RenderFeature::draw_far_shadow).
/// It covers a square around the camera whose size doesn't depend on where the camera looks,
/// snapped to whole texels in world space, so it neither shimmers nor swims as the camera turns
/// and moves. Shading blends into it over the last tenth of the cascades (`shadow_factor` in
/// `lib_frame.wgsl`).
#[derive(Debug, Clone, Copy)]
pub struct FarShadowView {
    pub enabled: bool,
    pub map_size: u32,
    /// Half the width of the covered square (and the reach from the camera), meters.
    pub half_extent: f32,
    /// Depth range from the light's eye, meters.
    pub depth_range: f32,
    /// Camera-relative → far shadow clip space (orthographic, standard Z).
    pub view_proj: Mat4,
    /// Camera-relative → light view space (looking down −Z), for culling.
    pub light_view: Mat4,
    /// Casters smaller than this bounding radius (meters, three quarters of a texel: houses,
    /// trees and boulders cast, pebbles don't) are left out.
    pub min_caster_radius: f32,
    /// Center of the square (absolute, on its texel grid).
    pub center: DVec3,
    /// The direction toward the sun it is drawn with.
    pub sun_dir: Vec3,
    /// Whether its static image is drawn again this frame (see [`ShadowCache`]).
    pub static_redraw: bool,
}

impl Default for FarShadowView {
    fn default() -> Self {
        Self {
            enabled: false,
            map_size: 2048,
            half_extent: 0.0,
            depth_range: 0.0,
            view_proj: Mat4::IDENTITY,
            light_view: Mat4::IDENTITY,
            min_caster_radius: f32::INFINITY,
            center: DVec3::ZERO,
            sun_dir: Vec3::Y,
            static_redraw: true,
        }
    }
}

impl FarShadowView {
    /// The far view for a camera at world position `origin` with the sun toward `sun_dir`,
    /// reaching `distance` meters, at `map_size`² texels. Disabled when `distance` doesn't reach
    /// past `cascade_reach` (the cascades already cover it).
    pub fn new(
        origin: DVec3,
        sun_dir: Vec3,
        map_size: u32,
        distance: f32,
        cascade_reach: f32,
    ) -> Self {
        if !(distance > cascade_reach && distance > 0.0) || sun_dir.length_squared() < 0.5 {
            return Self::default();
        }
        let map_size = map_size.max(256);
        let radius = distance as f64;
        let texel = 2.0 * radius / map_size as f64;
        let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
        let mut ls = light_rot.inverse() * origin;
        ls.x = (ls.x / texel).floor() * texel;
        ls.y = (ls.y / texel).floor() * texel;
        Self::around(light_rot * ls, origin, sun_dir, map_size, distance)
    }

    /// The far view centered on `center` (absolute, on its texel grid) for a camera at
    /// `origin`.
    pub fn around(
        center: DVec3,
        origin: DVec3,
        sun_dir: Vec3,
        map_size: u32,
        distance: f32,
    ) -> Self {
        let texel = 2.0 * distance / map_size.max(1) as f32;
        let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
        let center_rel = (center - origin).as_vec3();
        // Casters up to two radii toward the sun (mountains across the valley) get their real
        // depth; anything further is pancaked onto the near plane.
        let back = 2.0 * distance + 2000.0;
        let eye = center_rel + sun_dir * back;
        let light_view =
            Mat4::from_quat(light_rot.as_quat().inverse()) * Mat4::from_translation(-eye);
        let depth_range = back + distance;
        let proj = crate::frame::orthographic_standard(distance, 0.0, depth_range);
        Self {
            enabled: true,
            map_size,
            half_extent: distance,
            depth_range,
            view_proj: proj * light_view,
            light_view,
            min_caster_radius: 0.75 * texel,
            center,
            sun_dir,
            static_redraw: true,
        }
    }

    /// Size of one texel, meters.
    pub fn texel(&self) -> f32 {
        2.0 * self.half_extent / self.map_size.max(1) as f32
    }

    /// Whether a camera-relative bounding sphere touches the covered box (or lies toward the
    /// sun from it, so it can shade it).
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        if !self.enabled {
            return false;
        }
        let p = self.light_view.transform_point3(center);
        let e = self.half_extent + radius;
        p.x.abs() <= e && p.y.abs() <= e && -p.z - radius <= self.depth_range
    }

    /// Whether a caster with this camera-relative bounding sphere belongs in the far map: big
    /// enough to show at its resolution, and inside it.
    pub fn casts(&self, center: Vec3, radius: f32) -> bool {
        radius >= self.min_caster_radius && self.contains_sphere(center, radius)
    }

    /// `far_shadow_params` in the frame uniform: reach (m, 0 = off), texel size (m), clip depth
    /// per meter, shadow map layer.
    pub fn params(&self) -> [f32; 4] {
        if !self.enabled {
            return [0.0, 1.0, 0.0, FAR_SHADOW_LAYER as f32];
        }
        [
            self.half_extent,
            self.texel(),
            1.0 / self.depth_range.max(1e-3),
            FAR_SHADOW_LAYER as f32,
        ]
    }
}

/// GPU layout of `struct ShadowView` in `mesh_shadow.wgsl` (and any feature's shadow shader).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ShadowViewUniform {
    pub view_proj: [[f32; 4]; 4],
    pub wind: [f32; 4],
    pub time: [f32; 4],
    pub camera_frac: [f32; 4],
    /// x: the benders set in `benders` (only while swaying moves, `time.w` 0), yzw unused.
    pub bend: [f32; 4],
    /// [`crate::frame::FrameUniform::benders`]: swaying casters bend away from them as in the
    /// scene, so their shadows match.
    pub benders: [[f32; 4]; crate::frame::MAX_BENDERS],
    /// [`crate::frame::FrameUniform::plant_wind`], `gusts` and `gust_offset`: swaying casters
    /// move as in the scene.
    pub plant_wind: [f32; 4],
    pub gusts: [[f32; 4]; 4],
    pub gust_offset: [f32; 4],
}

/// Shadow map texels (meters) from which swaying leaves are drawn at rest: one pose that
/// doesn't change with time (`ShadowViewUniform::time.w` 1), and held by the map's static image
/// like any still caster. Finer maps show their motion and draw them every frame. Unreal stops
/// world position offset in distant shadows the same way (Nanite foliage's animation cutoff).
pub const SWAY_MOVING_TEXEL_M: f32 = 0.03;

/// Whether a shadow map of `texel` meters draws swaying leaves moving (else at rest).
pub fn sway_moves(texel: f32) -> bool {
    texel < SWAY_MOVING_TEXEL_M
}

/// Cascade split distances: blend of logarithmic and uniform (`lambda` 1 = log).
pub fn split_distances(near: f32, far: f32, count: usize, lambda: f32) -> [f32; 4] {
    let mut out = [far; 4];
    for (i, o) in out.iter_mut().enumerate().take(count) {
        let f = (i + 1) as f32 / count as f32;
        let log = near * (far / near).powf(f);
        let uni = near + (far - near) * f;
        *o = lambda * log + (1.0 - lambda) * uni;
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn compute_cascades(
    settings: &RenderSettings,
    origin: DVec3,
    rotation: Quat,
    near: f32,
    fov_y: f32,
    aspect: f32,
    orthographic: bool,
    sun_dir: Vec3,
    proj: &Mat4,
) -> Cascades {
    let count = MAX_CASCADES;
    let map_size = settings.shadow_map_size.max(256);
    let far = settings.shadow_distance.max(near * 4.0 + 1.0);
    // The first cascade starts a little in front of the camera to give it more resolution.
    let first_near = near.max(0.1);
    let splits = split_distances(
        first_near.max(1.0),
        far,
        count,
        settings.shadow_split_lambda,
    );

    let (tx, ty) = if orthographic {
        (1.0 / proj.x_axis.x as f64, 1.0 / proj.y_axis.y as f64)
    } else {
        let ty = (fov_y as f64 * 0.5).tan();
        (ty * aspect as f64, ty)
    };

    // Light basis: looking along −sun_dir.
    let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
    let light_rot_inv = light_rot.inverse();

    let mut out = Cascades {
        count: count as u32,
        map_size,
        splits,
        ..Default::default()
    };
    let mut slice_near = first_near as f64;
    for (i, &split) in splits.iter().enumerate().take(count) {
        let slice_far = split as f64;
        // A square around the camera holding everything within the split distance (shading
        // picks the cascade by distance, `cascade_visibility`), plus the margin the camera may
        // drift while the static image is kept. Quantized so the texel size is stable.
        let radius = slice_far * (1.0 + STATIC_MARGIN);
        let radius = (radius * 16.0).ceil() / 16.0;
        let texel = 2.0 * radius / map_size as f64;

        // Snap the center to the texel grid in light space, in absolute world coordinates.
        let mut ls = light_rot_inv * origin;
        ls.x = (ls.x / texel).floor() * texel;
        ls.y = (ls.y / texel).floor() * texel;
        out.place(i, light_rot * ls, origin, sun_dir, radius as f32);

        // What the cascade shades in this frame's view: its slice, from where shading dithers
        // in from the cascade before (the last tenth of it) to its split. Shading picks by
        // distance, so the slice starts at the view depth that distance has at the screen's
        // corners. Moving casters are culled to it; still casters are drawn for the whole square.
        let from = if i == 0 { slice_near } else { slice_near * 0.9 };
        let from = if i == 0 {
            from
        } else if orthographic {
            (from * from - tx * tx - ty * ty).max(0.0).sqrt()
        } else {
            from / (1.0 + tx * tx + ty * ty).sqrt()
        };
        let corners = [from, slice_far].map(|d| {
            let (x, y) = if orthographic {
                (tx, ty)
            } else {
                (tx * d, ty * d)
            };
            [(-x, -y), (x, -y), (x, y), (-x, y)]
                .map(|(x, y)| rotation * Vec3::new(x as f32, y as f32, -d as f32))
        });
        out.slice[i] = std::array::from_fn(|k| corners[k / 4][k % 4]);
        let filter = FILTER_TEXELS * texel as f32;
        out.casters[i] = CasterVolume::new(&out.slice[i], sun_dir, filter);
        slice_near = slice_far;
    }
    out
}

fn look_rotation_f64(forward: DVec3) -> DQuat {
    let f = forward.normalize();
    let up = if f.y.abs() > 0.999 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let right = f.cross(up).normalize();
    let up = right.cross(f);
    DQuat::from_mat3(&DMat3::from_cols(right, up, -f))
}

/// Tiles along each side of a shadow map in [`ShadowTiles`].
pub const SHADOW_TILE_GRID: u32 = 16;

/// Texels added around what a caster covers in [`ShadowTiles::add_sphere`]: a map whose size
/// is no multiple of the grid has tile edges up to a texel off, and a vertex bias may move a
/// caster by less than one.
const TILE_MARGIN_TEXELS: f32 = 2.0;

/// How camera-relative points land in a shadow map, for [`ShadowTiles::add_projected`]: the
/// map's orthographic projection, taken apart once for many casters.
#[derive(Debug, Clone, Copy)]
pub struct TileProjection {
    /// Clip space `x` and `y` of a point `p`: `across.dot(p.extend(1.0))`, `up.dot(..)`.
    across: Vec4,
    up: Vec4,
    /// A meter's length in clip space along each (the same everywhere: orthographic).
    meter: Vec2,
    /// [`TILE_MARGIN_TEXELS`] in clip space.
    margin: f32,
}

impl TileProjection {
    /// Of shadow map `map` of `view` (none for a number that names no map).
    pub fn of(view: &crate::ViewData, map: usize) -> Option<Self> {
        let (view_proj, size) = match map {
            SKY_OCCLUSION_VIEW => (view.sky_occlusion.view_proj, view.sky_occlusion.map_size),
            FAR_SHADOW_MAP => (view.far_shadow.view_proj, view.far_shadow.map_size),
            c if c < MAX_CASCADES => (view.cascades.view_proj[c], view.cascades.map_size),
            _ => return None,
        };
        Some(Self::new(&view_proj, size))
    }

    /// Of a map drawn with the orthographic `view_proj` at `size`² texels.
    pub fn new(view_proj: &Mat4, size: u32) -> Self {
        let (across, up) = (view_proj.row(0), view_proj.row(1));
        Self {
            across,
            up,
            meter: Vec2::new(across.truncate().length(), up.truncate().length()),
            margin: 2.0 * TILE_MARGIN_TEXELS / size.max(1) as f32,
        }
    }
}

/// Parts of a shadow map: a 16 by 16 grid of tiles over it, a bit a tile (row 0 at the top of
/// the map, as its texels are stored).
///
/// A map holds its static image with the moving casters drawn on top. It is not copied from
/// the image every frame (five maps of 4096² are 335 MB): each feature names the tiles its
/// moving casters draw into ([`crate::RenderFeature::shadow_moving`]), and the next frame
/// only those tiles are put back from the image before that frame's casters are drawn. Where
/// nothing moves nothing is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShadowTiles([u16; SHADOW_TILE_GRID as usize]);

impl ShadowTiles {
    pub const NONE: Self = Self([0; SHADOW_TILE_GRID as usize]);
    /// The whole map: for casters whose place isn't known, or spread all over it.
    pub const ALL: Self = Self([u16::MAX; SHADOW_TILE_GRID as usize]);

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|r| *r == 0)
    }

    /// Number of tiles (of 256).
    pub fn count(&self) -> u32 {
        self.0.iter().map(|r| r.count_ones()).sum()
    }

    pub fn add(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a |= b;
        }
    }

    /// Add the tiles a rectangle touches: `min` and `max` in map coordinates (0 to 1 across,
    /// `y` down from the top). Parts outside the map are left out; a rectangle that isn't
    /// finite takes the whole map.
    pub fn add_rect(&mut self, min: Vec2, max: Vec2) {
        if !(min.is_finite() && max.is_finite()) {
            *self = Self::ALL;
            return;
        }
        if max.x < 0.0 || max.y < 0.0 || min.x > 1.0 || min.y > 1.0 {
            return;
        }
        let n = SHADOW_TILE_GRID as f32;
        let tile = |v: f32| ((v * n).floor().max(0.0) as u32).min(SHADOW_TILE_GRID - 1);
        let (x0, x1) = (tile(min.x), tile(max.x));
        let bits = (u16::MAX >> (15 - (x1 - x0))) << x0;
        for row in &mut self.0[tile(min.y) as usize..=tile(max.y) as usize] {
            *row |= bits;
        }
    }

    /// Add the tiles a caster's bounding sphere (camera-relative, as the features cull with)
    /// draws into in shadow map `map` of `view`: a cascade, [`SKY_OCCLUSION_VIEW`] or
    /// [`FAR_SHADOW_MAP`]. A sphere without a finite size takes the whole map. For many
    /// casters, make the map's [`TileProjection`] once and use
    /// [`add_projected`](Self::add_projected).
    pub fn add_sphere(&mut self, view: &crate::ViewData, map: usize, center: Vec3, radius: f32) {
        if let Some(projection) = TileProjection::of(view, map) {
            self.add_projected(&projection, center, radius);
        }
    }

    /// [`add_sphere`](Self::add_sphere) with the map's projection made beforehand.
    pub fn add_projected(&mut self, map: &TileProjection, center: Vec3, radius: f32) {
        let p = center.extend(1.0);
        let (x, y) = (map.across.dot(p), map.up.dot(p));
        let (rx, ry) = (
            map.meter.x * radius + map.margin,
            map.meter.y * radius + map.margin,
        );
        // Clip space `y` is up, the map's rows go down.
        self.add_rect(
            Vec2::new(0.5 + 0.5 * (x - rx), 0.5 - 0.5 * (y + ry)),
            Vec2::new(0.5 + 0.5 * (x + rx), 0.5 - 0.5 * (y - ry)),
        );
    }

    /// The tiles as rectangles of a `size`² map, `[x, y, width, height]` in texels: runs along
    /// each row, joined with the same run in the rows below.
    pub fn rects(&self, size: u32) -> Vec<[u32; 4]> {
        let n = SHADOW_TILE_GRID;
        let edge = |t: u32| (t as u64 * size as u64 / n as u64) as u32;
        // Rectangles in tiles, [x0, x1, y0, y1), and those that reach the row before.
        let mut out: Vec<[u32; 4]> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for (y, &row) in self.0.iter().enumerate() {
            let y = y as u32;
            let mut reach = Vec::new();
            let mut x = 0;
            while x < n {
                if row & (1 << x) == 0 {
                    x += 1;
                    continue;
                }
                let x0 = x;
                while x < n && row & (1 << x) != 0 {
                    x += 1;
                }
                // The run goes on from the row above, or starts a rectangle.
                match open.iter().find(|&&k| out[k][0] == x0 && out[k][1] == x) {
                    Some(&k) => {
                        out[k][3] = y + 1;
                        reach.push(k);
                    }
                    None => {
                        reach.push(out.len());
                        out.push([x0, x, y, y + 1]);
                    }
                }
            }
            open = reach;
        }
        out.iter()
            .map(|r| {
                let (x, y) = (edge(r[0]), edge(r[2]));
                [x, y, edge(r[1]) - x, edge(r[3]) - y]
            })
            .filter(|r| r[2] > 0 && r[3] > 0)
            .collect()
    }
}

#[cfg(test)]
mod tile_tests {
    use super::*;

    /// Every texel of a `size`² map the rectangles cover, and that none is covered twice.
    fn covered(tiles: &ShadowTiles, size: u32) -> Vec<bool> {
        let mut texels = vec![false; (size * size) as usize];
        for [x, y, w, h] in tiles.rects(size) {
            assert!(x + w <= size && y + h <= size);
            for v in y..y + h {
                for u in x..x + w {
                    let t = &mut texels[(v * size + u) as usize];
                    assert!(!*t, "texel {u},{v} twice");
                    *t = true;
                }
            }
        }
        texels
    }

    #[test]
    fn rectangles_cover_their_tiles_once() {
        let mut tiles = ShadowTiles::NONE;
        assert!(tiles.is_empty() && tiles.rects(4096).is_empty());
        // An L, a lone tile and a block that shares a run with the L for two rows.
        tiles.add_rect(Vec2::new(0.01, 0.01), Vec2::new(0.3, 0.05));
        tiles.add_rect(Vec2::new(0.01, 0.01), Vec2::new(0.05, 0.4));
        tiles.add_rect(Vec2::new(0.9, 0.9), Vec2::new(0.91, 0.91));
        tiles.add_rect(Vec2::new(0.5, 0.2), Vec2::new(0.7, 0.3));
        for size in [256, 1000, 4096] {
            let texels = covered(&tiles, size);
            // The tile of a texel, by the whole-texel edges `rects` gives the tiles.
            let tile = |u: u32| (0..16).rfind(|t| t * size / 16 <= u).unwrap();
            for v in 0..size {
                for u in 0..size {
                    let inside = tiles.0[tile(v) as usize] & (1 << tile(u)) != 0;
                    assert_eq!(texels[(v * size + u) as usize], inside, "{size}: {u},{v}");
                }
            }
        }
        assert_eq!(ShadowTiles::ALL.rects(4096), vec![[0, 0, 4096, 4096]]);
        assert_eq!(ShadowTiles::ALL.count(), 256);
        // The L's long row, its column below, the block and the lone tile.
        assert_eq!(tiles.rects(4096).len(), 4);
    }

    #[test]
    fn a_sphere_takes_the_tiles_it_draws_into() {
        // A cascade of 100 m half extent, sun straight overhead, at the origin.
        let mut c = Cascades {
            count: 1,
            map_size: 4096,
            ..Default::default()
        };
        c.place(0, DVec3::ZERO, DVec3::ZERO, Vec3::Y, 100.0);
        let map = TileProjection::new(&c.view_proj[0], 4096);
        let texel_of = |p: Vec3| {
            let clip = c.view_proj[0].project_point3(p);
            Vec2::new(0.5 + 0.5 * clip.x, 0.5 - 0.5 * clip.y) * 4096.0
        };
        for (center, radius) in [
            (Vec3::new(10.0, 3.0, -40.0), 2.0),
            (Vec3::new(-99.0, 0.0, 99.0), 5.0),
            (Vec3::new(0.0, 50.0, 0.0), 30.0),
        ] {
            let mut tiles = ShadowTiles::NONE;
            tiles.add_projected(&map, center, radius);
            let texels = covered(&tiles, 4096);
            assert!(tiles.count() > 0 && tiles.count() < 256);
            // Points on the sphere land on covered texels (those inside the map).
            for k in 0..64 {
                let (a, b) = (k as f32 * 0.4, k as f32 * 1.3);
                let p = center + radius * Vec3::new(a.cos() * b.sin(), a.sin(), a.cos() * b.cos());
                let t = texel_of(p);
                if t.x >= 0.0 && t.y >= 0.0 && t.x < 4096.0 && t.y < 4096.0 {
                    assert!(texels[t.y as usize * 4096 + t.x as usize], "{center} {p}");
                }
            }
        }
        // Outside the map: nothing; no finite size: everything.
        let mut tiles = ShadowTiles::NONE;
        tiles.add_projected(&map, Vec3::new(500.0, 0.0, 0.0), 10.0);
        assert!(tiles.is_empty());
        tiles.add_projected(&map, Vec3::ZERO, f32::INFINITY);
        assert_eq!(tiles, ShadowTiles::ALL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inset_frames_its_body_toward_the_sun() {
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let center = Vec3::new(1.0, 0.9, -3.0);
        let inset = ShadowInset::new(center, 1.0, sun);
        // The body's sphere fills the map: its center in the middle, depth inside 0..1, and
        // a point toward the sun nearer the map's near plane than one away from it.
        let clip = |p: Vec3| inset.view_proj.project_point3(p);
        let c = clip(center);
        assert!(
            c.x.abs() < 1e-4 && c.y.abs() < 1e-4 && (0.0..1.0).contains(&c.z),
            "{c:?}"
        );
        assert!(clip(center + sun * 0.9).z < clip(center - sun * 0.9).z);
        let side = sun.any_orthonormal_vector();
        assert!((clip(center + side).truncate().length() - 1.0).abs() < 1e-3);
        assert!(inset.casts(center + Vec3::X * 1.5, 0.6));
        assert!(!inset.casts(center + Vec3::X * 3.0, 0.6));
        assert!((inset.texel(2048) - 2.0 / 2048.0).abs() < 1e-7);
    }

    #[test]
    fn splits_increase_to_far() {
        let s = split_distances(1.0, 300.0, 4, 0.8);
        assert!(s[0] < s[1] && s[1] < s[2] && s[2] < s[3]);
        assert!((s[3] - 300.0).abs() < 1e-3);
    }

    #[test]
    fn cascades_are_stable_under_rotation() {
        let settings = RenderSettings::default();
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let origin = DVec3::new(1000.0, 20.0, -5000.0);
        let proj = crate::frame::perspective_infinite_reverse(1.2, 1.6, 0.1);
        let a = compute_cascades(
            &settings,
            origin,
            Quat::IDENTITY,
            0.1,
            1.2,
            1.6,
            false,
            sun,
            &proj,
        );
        let b = compute_cascades(
            &settings,
            origin,
            Quat::from_rotation_y(0.3),
            0.1,
            1.2,
            1.6,
            false,
            sun,
            &proj,
        );
        // Same size regardless of where the camera looks.
        for i in 0..MAX_CASCADES {
            assert_eq!(a.half_extent[i], b.half_extent[i]);
        }
        // A point at the camera is inside cascade 0.
        assert!(a.sphere_in_cascade(0, Vec3::ZERO, 0.5));
        // A point far away is not in cascade 0.
        assert!(!a.sphere_in_cascade(0, Vec3::new(0.0, 0.0, 2000.0), 1.0));
        // Projecting a point near the camera lands inside clip space.
        let c = a.view_proj[0] * Vec4::new(0.0, 0.0, -1.0, 1.0);
        assert!(c.x.abs() <= 1.0 && c.y.abs() <= 1.0 && (0.0..=1.0).contains(&c.z));
    }
}

#[cfg(test)]
mod far_shadow_tests {
    use super::*;

    #[test]
    fn far_view_covers_kilometers_and_keeps_casters_toward_the_sun() {
        let sun = Vec3::new(0.0, 0.2, -1.0).normalize();
        let v = FarShadowView::new(DVec3::new(1234.5, 30.0, -77.0), sun, 2048, 8000.0, 400.0);
        assert!(v.enabled);
        assert!((v.texel() - 16000.0 / 2048.0).abs() < 1e-3);
        // Ground 2 km in front of the camera projects inside the map, with a valid depth.
        let c = v.view_proj * Vec4::new(300.0, 0.0, 2000.0, 1.0);
        assert!(c.x.abs() < 1.0 && c.y.abs() < 1.0 && (0.0..1.0).contains(&c.z));
        // A mountain 6 km toward the sun is kept (it shades the valley), a pebble is not.
        assert!(v.casts(Vec3::new(0.0, 800.0, -6000.0), 1500.0));
        assert!(!v.casts(Vec3::new(0.0, 0.0, -100.0), 0.5));
        // Far beside the map: not kept.
        assert!(!v.casts(Vec3::new(30_000.0, 0.0, 0.0), 500.0));
        // The cascades already reach far enough: off.
        assert!(!FarShadowView::new(DVec3::ZERO, sun, 2048, 300.0, 400.0).enabled);
    }

    #[test]
    fn far_view_is_snapped_to_texels() {
        let sun = Vec3::new(0.3, 0.6, 0.2).normalize();
        let a = FarShadowView::new(DVec3::new(100.0, 0.0, 0.0), sun, 1024, 6000.0, 400.0);
        let b = FarShadowView::new(DVec3::new(100.3, 0.0, 0.1), sun, 1024, 6000.0, 400.0);
        // The same world point lands on the same texel position (up to the camera's own move).
        let world = DVec3::new(2500.0, 120.0, -900.0);
        let pa = a.view_proj * (world - DVec3::new(100.0, 0.0, 0.0)).as_vec3().extend(1.0);
        let pb = b.view_proj * (world - DVec3::new(100.3, 0.0, 0.1)).as_vec3().extend(1.0);
        let texel_ndc = 2.0 / 1024.0;
        let frac = |x: f32| ((x + 1.0) / texel_ndc).fract();
        assert!((frac(pa.x) - frac(pb.x)).abs() < 0.02 || (frac(pa.x) - frac(pb.x)).abs() > 0.98);
    }
}

#[cfg(test)]
mod sky_occlusion_tests {
    use super::*;

    #[test]
    fn sky_occlusion_maps_points_below_the_eye_into_the_unit_square() {
        let v = SkyOcclusionView::new(DVec3::new(1234.5, 10.0, -77.25), 512, 256.0, 800.0, true);
        // A point 3 m above the camera's feet, 20 m away: inside the square, depth in 0..1.
        let clip = v.view_proj * Vec4::new(20.0, 3.0, -15.0, 1.0);
        assert!(clip.x.abs() < 1.0 && clip.y.abs() < 1.0);
        assert!(clip.z > 0.0 && clip.z < 1.0);
        // Higher points have smaller depth (closer to the eye above).
        let higher = v.view_proj * Vec4::new(20.0, 30.0, -15.0, 1.0);
        assert!(higher.z < clip.z);
        assert!(v.contains_sphere(Vec3::new(20.0, 3.0, -15.0), 1.0));
        assert!(!v.contains_sphere(Vec3::new(500.0, 3.0, 0.0), 1.0));
    }

    fn cascades_at(origin: DVec3, sun: Vec3) -> Cascades {
        let proj = crate::frame::perspective_infinite_reverse(1.2, 1.6, 0.1);
        compute_cascades(
            &RenderSettings::default(),
            origin,
            Quat::IDENTITY,
            0.1,
            1.2,
            1.6,
            false,
            sun,
            &proj,
        )
    }

    fn apply(
        cache: &mut ShadowCache,
        origin: DVec3,
        sun: Vec3,
        fresh: bool,
        requested: [bool; SHADOW_MAPS],
    ) -> Cascades {
        let mut c = cascades_at(origin, sun);
        let mut sky = SkyOcclusionView::new(origin, 512, 256.0, 800.0, true);
        let mut far = FarShadowView::new(origin, sun, 2048, 8000.0, c.splits[3]);
        cache.apply(&mut c, &mut sky, &mut far, origin, fresh, requested);
        // A kept map still maps the world where its image was drawn: its center lands in the
        // middle of the map.
        for i in 0..MAX_CASCADES {
            let rel = (c.center[i] - origin).as_vec3();
            let p = c.view_proj[i] * rel.extend(1.0);
            assert!(p.x.abs() < 1e-3 && p.y.abs() < 1e-3, "cascade {i}: {p}");
        }
        c
    }

    #[test]
    fn static_images_are_kept_until_the_camera_leaves_the_margin_or_the_sun_turns() {
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let none = [false; SHADOW_MAPS];
        let mut cache = ShadowCache::default();
        let mut origin = DVec3::new(1000.0, 20.0, -5000.0);
        let first = apply(&mut cache, origin, sun, true, none);
        assert_eq!(first.static_redraw, [true; MAX_CASCADES]);
        // A slow walk: the far cascades keep their images; the nearest, small one is drawn
        // again whenever the camera leaves its margin.
        let mut redrawn = [0; MAX_CASCADES];
        for _ in 0..16 {
            origin.x += 0.05;
            let c = apply(&mut cache, origin, sun, false, none);
            for (n, r) in redrawn.iter_mut().zip(c.static_redraw) {
                *n += r as u32;
            }
        }
        assert_eq!(redrawn[2..], [0, 0], "{redrawn:?}");
        // Leaving the margin redraws.
        let jump = origin + DVec3::new(first.half_extent[3] as f64 * 0.2, 0.0, 0.0);
        let c = apply(&mut cache, jump, sun, false, none);
        assert!(c.static_redraw[3] && c.static_redraw[2]);
        // A feature asking redraws that map alone.
        let mut ask = none;
        ask[1] = true;
        let c = apply(&mut cache, jump, sun, false, ask);
        assert_eq!(c.static_redraw, [false, true, false, false]);
        // A sun turning slowly: one map a frame, until all have the new sun.
        let turned = Quat::from_rotation_y(0.01) * sun;
        let mut frames = 0;
        loop {
            let c = apply(&mut cache, jump, turned, false, none);
            let n = c.static_redraw.iter().filter(|&&r| r).count();
            assert!(n <= 1, "{:?}", c.static_redraw);
            frames += 1;
            if n == 0 && c.sun_dir.iter().all(|s| s.angle_between(turned) < 1e-4) {
                break;
            }
            assert!(frames < 10);
        }
        // A sun turning a lot: everything at once. A camera cut: everything.
        let c = apply(
            &mut cache,
            jump,
            Quat::from_rotation_y(0.05) * sun,
            false,
            none,
        );
        assert_eq!(c.static_redraw, [true; MAX_CASCADES]);
        let c = apply(&mut cache, jump, sun, true, none);
        assert_eq!(c.static_redraw, [true; MAX_CASCADES]);
    }

    #[test]
    fn far_maps_let_changed_casters_wait_and_near_ones_dont() {
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let origin = DVec3::new(1000.0, 20.0, -5000.0);
        // Off: no map waits.
        assert_eq!(static_holds(0.0, 1.0 / 60.0), [0; SHADOW_MAPS]);
        assert_eq!(static_holds(0.25, 0.0), [0; SHADOW_MAPS]);
        // A quarter second at 60 frames a second.
        let hold = static_holds(0.25, 1.0 / 60.0);
        assert_eq!(hold[..4], [0, 0, 15, 30]);
        assert_eq!(hold[SKY_OCCLUSION_VIEW], 30);
        assert_eq!(hold[FAR_SHADOW_MAP], 60);

        let mut cache = ShadowCache {
            hold,
            ..Default::default()
        };
        apply(&mut cache, origin, sun, true, [false; SHADOW_MAPS]);
        // Something changes in every map on every frame: the near cascades are drawn again
        // every frame, the third every 16th, the last every 31st.
        let mut redrawn = [0u32; MAX_CASCADES];
        for _ in 0..62 {
            let c = apply(&mut cache, origin, sun, false, [true; SHADOW_MAPS]);
            for (n, r) in redrawn.iter_mut().zip(c.static_redraw) {
                *n += r as u32;
            }
        }
        assert_eq!(redrawn, [62, 62, 3, 2]);
        // A camera cut doesn't wait.
        let c = apply(&mut cache, origin, sun, true, [false; SHADOW_MAPS]);
        assert_eq!(c.static_redraw, [true; MAX_CASCADES]);
        // Nor does the camera leaving the margin.
        let jump = origin + DVec3::new(c.half_extent[3] as f64 * 0.2, 0.0, 0.0);
        let c = apply(&mut cache, jump, sun, false, [false; SHADOW_MAPS]);
        assert!(c.static_redraw[3] && c.static_redraw[2]);
    }

    #[test]
    fn static_images_are_kept_while_the_camera_turns() {
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let origin = DVec3::new(1000.0, 20.0, -5000.0);
        let proj = crate::frame::perspective_infinite_reverse(1.2, 1.6, 0.1);
        let mut cache = ShadowCache::default();
        for (k, angle) in [0.0f32, 0.6, 2.5].into_iter().enumerate() {
            let mut c = compute_cascades(
                &RenderSettings::default(),
                origin,
                Quat::from_rotation_y(angle),
                0.1,
                1.2,
                1.6,
                false,
                sun,
                &proj,
            );
            let mut sky = SkyOcclusionView::new(origin, 512, 256.0, 800.0, true);
            let mut far = FarShadowView::new(origin, sun, 2048, 8000.0, c.splits[3]);
            cache.apply(
                &mut c,
                &mut sky,
                &mut far,
                origin,
                k == 0,
                [false; SHADOW_MAPS],
            );
            assert_eq!(c.static_redraw, [k == 0; MAX_CASCADES]);
            assert_eq!(sky.static_redraw, k == 0);
            assert_eq!(far.static_redraw, k == 0);
        }
    }
}

#[cfg(test)]
mod caster_culling_tests {
    use super::*;

    fn cascades(rotation: Quat, sun: Vec3) -> Cascades {
        let proj = crate::frame::perspective_infinite_reverse(1.2, 1.6, 0.1);
        compute_cascades(
            &RenderSettings::default(),
            DVec3::new(1000.0, 20.0, -5000.0),
            rotation,
            0.1,
            1.2,
            1.6,
            false,
            sun,
            &proj,
        )
    }

    /// A point inside slice `i`, from barycentric-ish weights in 0..1.
    fn in_slice(c: &Cascades, i: usize, u: f32, v: f32, w: f32) -> Vec3 {
        let k = &c.slice[i];
        let near = k[0].lerp(k[1], u).lerp(k[3].lerp(k[2], u), v);
        let far = k[4].lerp(k[5], u).lerp(k[7].lerp(k[6], u), v);
        near.lerp(far, w)
    }

    #[test]
    fn casters_of_anything_in_view_are_kept() {
        // Conservative: whatever shades a point the cascade covers is kept, for many suns and
        // view directions (a caster anywhere on the ray from the point toward the sun).
        let mut seed = 7u32;
        let mut rand = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..200 {
            let sun = Vec3::new(rand() - 0.5, rand() * 0.9 + 0.05, rand() - 0.5).normalize();
            let rot = Quat::from_euler(EulerRot::YXZ, rand() * 6.3, rand() * 2.0 - 1.0, 0.0);
            let c = cascades(rot, sun);
            for i in 0..MAX_CASCADES {
                let p = in_slice(&c, i, rand(), rand(), rand());
                let toward = rand() * c.depth_range[i] * 0.4;
                // Shading picks a cascade by distance: the slice's far corners, beyond the
                // split, are the next cascade's.
                if p.length() > c.splits[i] {
                    continue;
                }
                assert!(
                    c.sphere_in_cascade(i, p + sun * toward, 0.3),
                    "cascade {i}, sun {sun}, rotation {rot}"
                );
            }
        }
    }

    #[test]
    fn casters_behind_the_camera_are_left_out() {
        // Sun high in the west, camera looking north (−Z): a tree 150 m behind the camera is in
        // the last cascade's square but shades nothing in view; one 200 m ahead is kept.
        let sun = Vec3::new(-0.3, 0.9, 0.1).normalize();
        let c = cascades(Quat::IDENTITY, sun);
        let i = MAX_CASCADES - 1;
        let behind = Vec3::new(0.0, 5.0, 150.0);
        let ahead = Vec3::new(0.0, 5.0, -200.0);
        let box_only = |p: Vec3| {
            let l = c.light_view[i].transform_point3(p);
            l.x.abs() <= c.half_extent[i] + 5.0 && l.y.abs() <= c.half_extent[i] + 5.0
        };
        assert!(box_only(behind));
        assert!(!c.sphere_in_cascade(i, behind, 5.0));
        assert!(c.sphere_in_cascade(i, ahead, 5.0));
    }
}

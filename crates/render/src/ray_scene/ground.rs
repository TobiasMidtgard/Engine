//! The ground colour map (`trial_cut_more::RAY_GROUND_MAP`, step 11 of the ray-traced lighting
//! round): the features that draw it ([`crate::RenderFeature::ground_albedo_version`], the
//! terrain) draw their albedo seen from straight above into one map around the camera (binding
//! 6 of the scene, `ray_ground`), and a `Ground` row of theirs takes the map's colour where a
//! hit falls on it (`ray_surface`, lib_ray.wgsl) in place of its one colour. Drawn again when a
//! version or the map's centre changes, at most every [`REDRAW_FRAMES`] frames, main view only;
//! made while the bit is on and a feature draws it, dropped when not.

use engine_core::prelude::*;

use crate::features::GpuShared;

/// Texels across, and the levels down to one texel.
pub(super) const SIZE: u32 = 1024;
const LEVELS: u32 = SIZE.ilog2() + 1;
/// Meters across: twice the rays' far reach (`RenderSettings::ray_far_reach`, 384), 0.75 m a
/// texel.
pub(super) const EXTENT_M: f32 = 768.0;
/// The grid the centre is put on when it moves (meters).
const CELL_M: f64 = 64.0;
/// The centre moves when the eye is farther from it than this along x or z (meters): within,
/// the map's full colour reaches at least 312 m from the eye along x and z (its 384 m half
/// width less these 40 m and the rays' 32 m fade at its edge); a hit farther off, out to the
/// rays' 384 m, can fall in the fade, where its row's colour mixes in, or past the edge,
/// where it keeps its row's colour. (While a draw waits for the throttle, the rays read the
/// centre last drawn, which can be farther off.)
const REACH_M: f64 = 40.0;
/// Frames from one draw to the next, at least (the default probe update reaches each probe
/// every eighth frame, so a map this old adds at most one update's lag to the light).
pub(super) const REDRAW_FRAMES: u64 = 8;
/// Height of the slab the map sees, centred on the camera (meters).
const DEPTH_M: f32 = 8192.0;
/// MiB while made: the colour with its levels and the depth (9.33).
const MB: f32 = ((((1u64 << (2 * LEVELS)) - 1) / 3 * 4) + (SIZE as u64 * SIZE as u64) * 4) as f32
    / (1u64 << 20) as f32;

/// The ground colour map as a feature draws into it ([`crate::RenderFeature::draw_ground_albedo`]):
/// a square `2 * half_width` m across around `center`, seen from straight above, `size` texels
/// across in [`Self::FORMAT`], row 0 at its least z, column 0 at its least x.
#[derive(Clone, Copy, Debug)]
pub struct GroundAlbedoView {
    /// Its centre, world x and z: on a 64 m grid, or where a world shift carried it.
    pub center: DVec2,
    /// Half its width (m): 384.
    pub half_width: f32,
    /// Texels across: 1024.
    pub size: u32,
    /// The camera (`ViewData::origin`) that `view_proj`'s positions are relative to.
    pub origin: DVec3,
    /// Camera-relative position -> clip: looking down -Y, +x to the right, -z up; reverse-Z,
    /// depth 1 at 4096 m above the camera and 0 at 4096 m below.
    pub view_proj: Mat4,
}

impl GroundAlbedoView {
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    /// Meters a texel (0.75), as [`Self::texel_m`].
    pub const TEXEL_M: f32 = EXTENT_M / SIZE as f32;
    /// The farthest the map reaches from the camera along x or z (meters): half its width and
    /// how far the camera goes from its centre before the centre moves, 424. A feature that
    /// gathers what it draws before the centre is known (in its prepare) gathers this far.
    pub const REACH_FROM_EYE_M: f32 = EXTENT_M * 0.5 + REACH_M as f32;
    /// How far the map sees above and below the camera (meters): 4096, as `view_proj`'s depth.
    pub const HALF_DEPTH_M: f32 = DEPTH_M * 0.5;

    pub(crate) fn new(center: DVec2, origin: DVec3) -> Self {
        let eye = Vec3::new(
            (center.x - origin.x) as f32,
            DEPTH_M * 0.5,
            (center.y - origin.z) as f32,
        );
        // (As `SkyOcclusionView::around`: a proper turn, so a face counter-clockwise from above
        // stays front-facing and culling the back holds.)
        let rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let view = Mat4::from_rotation_translation(rotation, eye).inverse();
        let proj = crate::frame::orthographic_reverse(EXTENT_M, EXTENT_M, 0.0, DEPTH_M);
        Self {
            center,
            half_width: EXTENT_M * 0.5,
            size: SIZE,
            origin,
            view_proj: proj * view,
        }
    }

    /// Meters a texel (0.75).
    pub fn texel_m(&self) -> f32 {
        2.0 * self.half_width / self.size as f32
    }

    /// Whether a camera-relative bounding sphere touches the map's box.
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        let x = (self.center.x - self.origin.x) as f32;
        let z = (self.center.y - self.origin.z) as f32;
        (center.x - x).abs() <= self.half_width + radius
            && (center.z - z).abs() <= self.half_width + radius
            && center.y.abs() <= DEPTH_M * 0.5 + radius
    }
}

/// When the map is drawn, and whose rows read it (the CPU's part, unit-tested).
#[derive(Default)]
struct Plan {
    /// This frame's drawers: (feature number, version), in feature order.
    drawers: Vec<(u16, u64)>,
    /// The features whose `Ground` rows carry the map's bit, as last settled.
    marked: Vec<u16>,
    /// The centre (world x, z) while the map is on.
    center: Option<DVec2>,
    /// The centre and drawers as last drawn (`None`: nothing valid drawn).
    drawn: Option<(DVec2, Vec<(u16, u64)>)>,
    /// The centre drawn this frame.
    due: Option<DVec2>,
    /// The frame of the last draw (0: none).
    last: u64,
    /// Draws since the scene was made.
    draws: u32,
}

impl Plan {
    fn begin_frame(&mut self) {
        self.drawers.clear();
        self.due = None;
    }

    fn drawer(&mut self, feature: u16, version: Option<u64>) {
        if let Some(v) = version {
            self.drawers.push((feature, v));
        }
    }

    fn draws(&self, feature: u16) -> bool {
        self.drawers.iter().any(|&(f, _)| f == feature)
    }

    /// A world shift: a plain move carries the map (its picture stays true where it moved); a
    /// turn takes it off until it is drawn again.
    fn shift(&mut self, translation: DVec3, translation_only: bool) {
        if translation_only {
            let t = DVec2::new(translation.x, translation.z);
            if let Some(c) = &mut self.center {
                *c += t;
            }
            if let Some((c, _)) = &mut self.drawn {
                *c += t;
            }
        } else {
            self.center = None;
            self.drawn = None;
        }
    }

    /// The frame's rule, once every drawer said its version; true while the map is on (`on`,
    /// the bit, and a drawer). `every_frame`: drawn whatever changed (a measurement).
    fn settle(&mut self, eye: DVec3, frame: u64, on: bool, every_frame: bool) -> bool {
        if !on || self.drawers.is_empty() {
            self.center = None;
            self.drawn = None;
            return false;
        }
        let near = |c: &DVec2| (eye.x - c.x).abs() <= REACH_M && (eye.z - c.y).abs() <= REACH_M;
        let center = self
            .center
            .filter(near)
            .unwrap_or_else(|| (DVec2::new(eye.x, eye.z) / CELL_M).round() * CELL_M);
        self.center = Some(center);
        let changed = self
            .drawn
            .as_ref()
            .is_none_or(|(c, d)| *c != center || *d != self.drawers);
        let room = self.last == 0 || frame >= self.last + REDRAW_FRAMES;
        if every_frame || (changed && room) {
            self.due = Some(center);
        }
        true
    }

    /// The features whose rows' bit changes now (feature, whether it reads the map): those
    /// that began or stopped drawing since the last call.
    fn take_marks(&mut self) -> Vec<(u16, bool)> {
        let now: Vec<u16> = self.drawers.iter().map(|&(f, _)| f).collect();
        let mut changes: Vec<(u16, bool)> = now
            .iter()
            .filter(|f| !self.marked.contains(f))
            .map(|&f| (f, true))
            .collect();
        changes.extend(
            self.marked
                .iter()
                .filter(|f| !now.contains(f))
                .map(|&f| (f, false)),
        );
        self.marked = now;
        changes
    }

    fn drew(&mut self, frame: u64) {
        if let Some(c) = self.due.take() {
            self.drawn = Some((c, self.drawers.clone()));
            self.last = frame;
            self.draws += 1;
        }
    }

    /// The submit of `frame` failed: a draw it held drew nothing, so it is drawn again once
    /// the throttle allows, and the rays read their rows' colours meanwhile.
    fn failed(&mut self, frame: u64) {
        if self.last == frame && self.draws > 0 {
            self.drawn = None;
        }
    }

    /// The centre the rays read: drawn this frame, else as last drawn.
    fn read_center(&self) -> Option<DVec2> {
        self.due.or(self.drawn.as_ref().map(|(c, _)| *c))
    }

    /// `RayUniform::ground` for a camera at `origin` (`made`: the map exists): the centre the
    /// rays read, camera-relative, the half width and 1; zeros while the rays read none.
    fn uniform(&self, origin: DVec3, made: bool) -> [f32; 4] {
        match self.read_center().filter(|_| made) {
            Some(c) => [
                (c.x - origin.x) as f32,
                (c.y - origin.z) as f32,
                EXTENT_M * 0.5,
                1.0,
            ],
            None => [0.0; 4],
        }
    }
}

/// `render.ray.ground_*`: whether the rays read the map this frame, its draws since the scene
/// was made, the features that draw it this frame, its textures (MiB; 0 while off).
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct GroundStats {
    pub on: bool,
    pub draws: u32,
    pub drawers: u32,
    pub mb: f32,
}

/// The map's textures, while it is on.
struct Map {
    texture: wgpu::Texture,
    /// Every level (binding 6), level 0 (the draw's target), the draw's depth.
    all: wgpu::TextureView,
    top: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Map {
    fn new(device: &wgpu::Device, depth_format: wgpu::TextureFormat) -> Self {
        let size = wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ray ground map"),
            size,
            mip_level_count: LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GroundAlbedoView::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let all = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let top = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ray ground map level 0"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ray ground map depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: depth_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            all,
            top,
            depth,
        }
    }
}

/// The scene's ground colour map: what binding 6 binds, when it is drawn, and the uniform's
/// part that tells the rays where it lies.
pub(super) struct GroundMap {
    /// What binding 6 binds while no map is made (1x1, one level).
    stand_in: wgpu::TextureView,
    depth_format: wgpu::TextureFormat,
    map: Option<Map>,
    plan: Plan,
    /// The view drawn this frame (`Some` while a draw is due).
    view: Option<GroundAlbedoView>,
}

impl GroundMap {
    pub(super) fn new(stand_in: wgpu::TextureView, depth_format: wgpu::TextureFormat) -> Self {
        Self {
            stand_in,
            depth_format,
            map: None,
            plan: Plan::default(),
            view: None,
        }
    }

    /// What binding 6 binds: the map's every level, or the stand-in.
    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.stand_in, |m| &m.all)
    }

    pub(super) fn begin_frame(&mut self) {
        self.plan.begin_frame();
        self.view = None;
    }

    pub(super) fn shift(&mut self, translation: DVec3, translation_only: bool) {
        self.plan.shift(translation, translation_only);
    }

    /// Feature `feature` draws `version` this frame (`None`: nothing).
    pub(super) fn drawer(&mut self, feature: u16, version: Option<u64>) {
        self.plan.drawer(feature, version);
    }

    /// Whether feature `feature` draws the map this frame (its `Ground` rows read it).
    pub(super) fn draws(&self, feature: u16) -> bool {
        self.plan.draws(feature)
    }

    /// Whether the map is drawn this frame.
    pub(super) fn due(&self) -> bool {
        self.view.is_some()
    }

    /// The frame's rule once every hook ran (see [`Plan::settle`]), the map made or dropped by
    /// it: true when binding 6 must bind another view.
    pub(super) fn settle(
        &mut self,
        device: &wgpu::Device,
        eye: DVec3,
        frame: u64,
        on: bool,
        every_frame: bool,
    ) -> bool {
        let on = self.plan.settle(eye, frame, on, every_frame);
        let regroup = on != self.map.is_some();
        if regroup {
            self.map = on.then(|| Map::new(device, self.depth_format));
        }
        self.view = self.plan.due.map(|c| GroundAlbedoView::new(c, eye));
        regroup
    }

    /// See [`Plan::take_marks`].
    pub(super) fn take_marks(&mut self) -> Vec<(u16, bool)> {
        self.plan.take_marks()
    }

    /// `RayUniform::ground` for a camera at `origin` (see [`Plan::uniform`]).
    pub(super) fn uniform(&self, origin: DVec3) -> [f32; 4] {
        self.plan.uniform(origin, self.map.is_some())
    }

    pub(super) fn stats(&self) -> GroundStats {
        let made = self.map.is_some();
        GroundStats {
            on: made && self.plan.read_center().is_some(),
            draws: self.plan.draws,
            drawers: self.plan.drawers.len() as u32,
            mb: if made { MB } else { 0.0 },
        }
    }

    /// Draw the map when it is due: one render pass on level 0 (colour cleared to transparent
    /// black, depth to 0) in which `draw` has this frame's drawers draw (the view, who draws
    /// which version), then its levels, each from the one above in linear light.
    pub(super) fn encode(
        &mut self,
        shared: &GpuShared,
        encoder: &mut wgpu::CommandEncoder,
        frame: u64,
        draw: impl FnOnce(&mut wgpu::RenderPass<'_>, &GroundAlbedoView, &[(u16, u64)]),
    ) {
        let (Some(map), Some(view)) = (&self.map, self.view.take()) else {
            return;
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ray ground map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &map.top,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &map.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            draw(&mut pass, &view, &self.plan.drawers);
        }
        shared
            .mips
            .record(shared, encoder, &map.texture, GroundAlbedoView::FORMAT);
        self.plan.drew(frame);
    }

    /// After the submit of `frame` (`failed`: wgpu rejected it).
    pub(super) fn after_submit(&mut self, failed: bool, frame: u64) {
        if failed {
            self.plan.failed(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame's rule in order: say the drawers, settle at `eye` in `frame`, draw when due.
    fn frame(plan: &mut Plan, frame: u64, eye: [f64; 2], drawers: &[(u16, Option<u64>)]) -> bool {
        plan.begin_frame();
        for &(f, v) in drawers {
            plan.drawer(f, v);
        }
        plan.settle(DVec3::new(eye[0], 3.0, eye[1]), frame, true, false);
        let due = plan.due.is_some();
        plan.drew(frame);
        due
    }

    /// The map is drawn when it has none, when a drawer's version changes, when a drawer comes
    /// or goes and when the eye leaves its centre by more than 40 m along x or z, and no
    /// sooner than 8 frames after the draw before; it is off without a drawer, drawn at once
    /// when one comes back long enough after, and every frame for the measurement.
    #[test]
    fn the_map_is_drawn_when_its_version_or_centre_changes_at_most_every_8th_frame() {
        let mut plan = Plan::default();
        let one = [(3, Some(7))];
        assert!(frame(&mut plan, 1, [10.0, -20.0], &one));
        assert_eq!(plan.read_center(), Some(DVec2::ZERO));
        assert_eq!(plan.draws, 1);
        for f in 2..=20 {
            assert!(!frame(&mut plan, f, [10.0, -20.0], &one), "frame {f}");
        }
        assert!(frame(&mut plan, 21, [10.0, -20.0], &[(3, Some(8))]));
        for f in 22..=28 {
            assert!(
                !frame(&mut plan, f, [10.0, -20.0], &[(3, Some(9))]),
                "frame {f}"
            );
        }
        assert!(frame(&mut plan, 29, [10.0, -20.0], &[(3, Some(9))]));
        // Nothing to draw is no drawer.
        assert!(!frame(
            &mut plan,
            40,
            [10.0, -20.0],
            &[(3, Some(9)), (6, None)]
        ));
        // A second drawer comes.
        assert!(frame(
            &mut plan,
            41,
            [10.0, -20.0],
            &[(3, Some(9)), (6, Some(1))]
        ));
        let two = [(3, Some(9)), (6, Some(1))];
        // The eye 39.9 m from the centre keeps it; 40.1 m moves it to the grid's next point.
        assert!(!frame(&mut plan, 50, [39.9, -20.0], &two));
        assert!(frame(&mut plan, 51, [40.1, -20.0], &two));
        assert_eq!(plan.read_center(), Some(DVec2::new(64.0, 0.0)));
        // Back to 39.9: 24.1 m from the new centre, kept.
        assert!(!frame(&mut plan, 60, [39.9, -20.0], &two));
        // The same rule along z: 39.9 m keeps it, 40.1 m moves it.
        assert!(!frame(&mut plan, 61, [39.9, -39.9], &two));
        assert!(frame(&mut plan, 62, [39.9, -40.1], &two));
        assert_eq!(plan.read_center(), Some(DVec2::new(64.0, -64.0)));
        // No drawer: off, nothing read.
        plan.begin_frame();
        assert!(!plan.settle(DVec3::new(39.9, 0.0, -20.0), 70, true, false));
        assert_eq!(plan.read_center(), None);
        assert_eq!(plan.uniform(DVec3::ZERO, true), [0.0; 4]);
        // Back 8 or more frames after the last draw: drawn at once.
        assert!(frame(&mut plan, 71, [39.9, -20.0], &two));
        // A drawer goes while the other stays (and the centre and versions stay): drawn again.
        assert!(!frame(&mut plan, 78, [39.9, -20.0], &[(3, Some(9))]));
        assert!(frame(&mut plan, 79, [39.9, -20.0], &[(3, Some(9))]));
        // The bit off: off whatever the drawers say.
        plan.begin_frame();
        plan.drawer(3, Some(9));
        assert!(!plan.settle(DVec3::ZERO, 80, false, false));
        assert_eq!(plan.read_center(), None);
        // The measurement: every frame.
        for f in 90..95 {
            plan.begin_frame();
            plan.drawer(3, Some(9));
            assert!(plan.settle(DVec3::ZERO, f, true, true));
            assert!(plan.due.is_some(), "frame {f}");
            plan.drew(f);
        }
        let draws = plan.draws;
        // A failed submit: the draw it held is drawn again once the throttle allows.
        assert!(frame(&mut plan, 110, [0.0, 0.0], &one));
        plan.failed(110);
        assert_eq!(
            plan.read_center(),
            None,
            "the rays read their rows meanwhile"
        );
        assert!(!frame(&mut plan, 111, [0.0, 0.0], &one));
        assert!(frame(&mut plan, 118, [0.0, 0.0], &one));
        assert_eq!(plan.draws, draws + 2);
    }

    /// A plain world shift carries the map with the world, so the eye moved the same way finds
    /// it where it was and the rays read it as before; a turn takes it off until the throttle
    /// allows the next draw.
    #[test]
    fn a_world_shift_carries_the_map_and_a_turn_takes_it_off() {
        let mut plan = Plan::default();
        let one = [(3, Some(1))];
        let eye = DVec3::new(70.0, 12.0, -100.0);
        assert!(frame(&mut plan, 1, [eye.x, eye.z], &one));
        assert_eq!(plan.read_center(), Some(DVec2::new(64.0, -128.0)));
        let before = plan.uniform(eye, true);
        assert_eq!(before, [-6.0, -28.0, 384.0, 1.0]);
        let t = DVec3::new(1000.0, 5.0, -300.0);
        plan.begin_frame();
        plan.shift(t, true);
        let moved = eye + t;
        assert!(!frame(&mut plan, 20, [moved.x, moved.z], &one));
        assert_eq!(plan.read_center(), Some(DVec2::new(1064.0, -428.0)));
        assert_eq!(plan.uniform(moved, true), before);
        // A turn.
        plan.begin_frame();
        plan.shift(t, false);
        assert_eq!(plan.read_center(), None);
        assert_eq!(plan.uniform(moved, true), [0.0; 4]);
        assert!(frame(&mut plan, 30, [moved.x, moved.z], &one));
        assert!(plan.uniform(moved, true)[3] == 1.0);
        // Within 8 frames of a draw a turn leaves it off until the throttle allows.
        plan.begin_frame();
        plan.shift(t, false);
        assert!(!frame(&mut plan, 31, [moved.x, moved.z], &one));
        assert_eq!(plan.uniform(moved, true), [0.0; 4]);
        assert!(frame(&mut plan, 38, [moved.x, moved.z], &one));
        // Without the map made, the rays read nothing.
        assert_eq!(plan.uniform(moved, false), [0.0; 4]);
    }

    /// A feature's rows carry the map's bit from the frame it draws until the frame it stops:
    /// the changes are said once each.
    #[test]
    fn the_rows_bit_follows_who_draws() {
        let mut plan = Plan::default();
        frame(&mut plan, 1, [0.0, 0.0], &[(3, Some(1)), (5, None)]);
        assert_eq!(plan.take_marks(), vec![(3, true)]);
        frame(&mut plan, 2, [0.0, 0.0], &[(3, Some(2)), (5, Some(1))]);
        assert_eq!(plan.take_marks(), vec![(5, true)]);
        frame(&mut plan, 3, [0.0, 0.0], &[(3, Some(2)), (5, Some(1))]);
        assert_eq!(plan.take_marks(), vec![]);
        frame(&mut plan, 4, [0.0, 0.0], &[(5, Some(1))]);
        assert_eq!(plan.take_marks(), vec![(3, false)]);
        frame(&mut plan, 5, [0.0, 0.0], &[]);
        assert_eq!(plan.take_marks(), vec![(5, false)]);
    }

    /// The map looks straight down with row 0 at the least z and column 0 at the least x,
    /// reverse-Z over 4096 m above and below the camera, and a face counter-clockwise from
    /// above in front.
    #[test]
    fn the_maps_view_looks_straight_down_with_row_0_at_the_least_z() {
        let center = DVec2::new(64.0, -128.0);
        let origin = DVec3::new(70.0, 12.0, -100.0);
        let view = GroundAlbedoView::new(center, origin);
        assert_eq!(view.texel_m(), GroundAlbedoView::TEXEL_M);
        assert_eq!(GroundAlbedoView::REACH_FROM_EYE_M, 424.0);
        let c = Vec3::new(
            (center.x - origin.x) as f32,
            0.0,
            (center.y - origin.z) as f32,
        );
        let ndc = |p: Vec3| {
            let clip = view.view_proj * p.extend(1.0);
            clip.truncate() / clip.w
        };
        let low = ndc(c + Vec3::new(-384.0, 0.0, -384.0));
        assert!(
            (low.x + 1.0).abs() < 1e-5 && (low.y - 1.0).abs() < 1e-5,
            "{low}"
        );
        let high = ndc(c + Vec3::new(384.0, -50.0, 384.0));
        assert!(
            (high.x - 1.0).abs() < 1e-5 && (high.y + 1.0).abs() < 1e-5,
            "{high}"
        );
        assert!((ndc(Vec3::new(c.x, 4096.0, c.z)).z - 1.0).abs() < 1e-5);
        assert!(ndc(Vec3::new(c.x, -4096.0, c.z)).z.abs() < 1e-5);
        assert!((ndc(Vec3::new(c.x, 0.0, c.z)).z - 0.5).abs() < 1e-5);
        // The corners (x0, z1), (x1, z1), (x1, z0) run counter-clockwise seen from above.
        let [a, b, d] = [
            ndc(c + Vec3::new(-10.0, 0.0, 10.0)),
            ndc(c + Vec3::new(10.0, 0.0, 10.0)),
            ndc(c + Vec3::new(10.0, 0.0, -10.0)),
        ];
        let area = (b.x - a.x) * (d.y - a.y) - (d.x - a.x) * (b.y - a.y);
        assert!(area > 0.0, "front-facing in clip space: {area}");
        assert_eq!(view.texel_m(), 0.75);
        assert!(view.contains_sphere(c + Vec3::new(383.0, 0.0, 0.0), 0.5));
        assert!(!view.contains_sphere(c + Vec3::new(385.0, 0.0, 0.0), 0.5));
        assert!(view.contains_sphere(c + Vec3::new(0.0, 0.0, -390.0), 7.0));
        assert!(!view.contains_sphere(c + Vec3::new(0.0, 4200.0, 0.0), 7.0));
        assert!((MB - 9.333).abs() < 0.001, "{MB}");
    }
}

//! Turning the laid-out tree into GPU instances: one instanced quad per box, shadow, glyph,
//! icon or painter primitive. The whole UI is a single draw call.
//!
//! [`DrawCtx`] is also the API custom painters ([`CustomPaint`]) use to draw maps, charts and
//! portraits: rounded boxes, glows, rings, lines, planets, text.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use cosmic_text::{Buffer, CacheKey, FontSystem, SwashCache, SwashContent};
use engine_core::math::Vec2;

use crate::atlas::Atlas;
use crate::color::Color;
use crate::icons::IconLibrary;
use crate::layout::{UiDrawOrder, UiSettings, UiViewport};
use crate::node::*;
use crate::style::Fill;
use crate::text::{Fonts, TextLayouts, TextSpec, from_cosmic};

/// Primitive kinds understood by `ui.wgsl`.
pub mod kind {
    /// Rounded box: fill (solid/linear/radial), border, optional frosted backdrop.
    pub const RECT: u32 = 0;
    /// Gaussian shadow of a rounded box (`uv` holds the box).
    pub const SHADOW: u32 = 1;
    /// Glyph coverage from the mask atlas, tinted.
    pub const GLYPH: u32 = 2;
    /// RGBA from the color atlas (icons, pictures, color glyphs), tinted.
    pub const IMAGE: u32 = 3;
    /// Ellipse outline / arc (`params`: thickness, start, sweep, dash count).
    pub const RING: u32 = 4;
    /// Line segment: quad along +x rotated by `extra.x` (`params.x`: thickness, `y`: dash).
    pub const LINE: u32 = 5;
    /// Soft radial glow (stars, highlights); `params.x` falloff exponent.
    pub const GLOW: u32 = 6;
    /// Orthographic globe (`kind[2]` = planet look index; `uv`: yaw, pitch, sun azimuth, sun
    /// elevation; `params.w` opacity; `extra.y` atmosphere).
    pub const PLANET: u32 = 7;
    /// Equirectangular map of a planet (`uv`: lon0, lat0, lon1, lat1 in radians).
    pub const PLANET_FLAT: u32 = 8;
    /// Procedural spiral galaxy (`uv`: center x, y in rect px, radius px, rotation).
    pub const GALAXY: u32 = 9;
    /// Background star field (`uv`: pan x, y, density, seed).
    pub const STARFIELD: u32 = 10;
}

/// Instance flags (`kind[1]`).
pub mod flag {
    pub const FILL_LINEAR: u32 = 1;
    pub const FILL_RADIAL: u32 = 2;
    pub const BACKDROP: u32 = 4;
    pub const DESATURATE: u32 = 8;
    pub const FLIP_X: u32 = 16;
    /// IMAGE: clip to the rounded box in `border_color` (x0, y0, x1, y1) with `radius`.
    pub const MASK: u32 = 32;
}

/// One quad on the GPU (160 bytes). See `shaders/ui.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct UiInstance {
    /// x, y, w, h in physical px.
    pub rect: [f32; 4],
    /// x0, y0, x1, y1 clip in physical px.
    pub clip: [f32; 4],
    /// Linear RGBA (fill, tint, or gradient start).
    pub color: [f32; 4],
    /// Gradient end / second color.
    pub color2: [f32; 4],
    pub border_color: [f32; 4],
    /// Corner radii tl, tr, br, bl (physical px).
    pub radius: [f32; 4],
    /// Atlas UVs, or kind-specific parameters.
    pub uv: [f32; 4],
    /// Border width, softness, gradient angle, opacity.
    pub params: [f32; 4],
    /// Rotation (radians), kind-specific.
    pub extra: [f32; 4],
    /// Kind, flags, look index, unused.
    pub kind: [u32; 4],
}

impl UiInstance {
    pub fn new(kind: u32, rect: Rect, clip: Rect) -> Self {
        Self {
            rect: [rect.x, rect.y, rect.w, rect.h],
            clip: [clip.x, clip.y, clip.x + clip.w, clip.y + clip.h],
            params: [0.0, 0.0, 0.0, 1.0],
            kind: [kind, 0, 0, 0],
            ..Default::default()
        }
    }
}

/// Surface models of rocky planets, in [`PlanetLook::colors`]`[0][3]` (gas giants are picked
/// by `style[1]` instead).
pub mod surface {
    /// Oceans and biomes from temperature and moisture (terran, ocean, jungle, desert).
    /// Also what 0 means, and the alpha [`Color::to_array`](crate::Color::to_array) leaves.
    pub const LIVING: f32 = 1.0;
    /// Frozen seas crossed by cracks, snowfields, glacier ice and bare rock.
    pub const ICE: f32 = 2.0;
    /// Dark crust with glowing fissures, lava seas under drifting plates.
    pub const VOLCANIC: f32 = 3.0;
    /// Airless regolith, dark maria and craters.
    pub const BARREN: f32 = 4.0;
}

/// Look of a planet for the PLANET / PLANET_FLAT primitives (uploaded as a uniform array).
///
/// The terrain is shared with the CPU: [`planet_noise`](crate::planet_noise) tells game code
/// where the land and sea are.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PlanetLook {
    /// Linear RGB of: deep ocean, shallow ocean / shelf, lush lowland (forest), dry land
    /// (sand, savanna), rock, snow and ice. Living worlds blend biomes from these (jungle,
    /// grassland, steppe, taiga, tundra...); other models use them as their own palette
    /// (gas giants: [2] pale zones, [3] zones, [4] belts, [5] the great storm).
    /// `colors[0][3]` is the [`surface`] model; the other alphas are unused.
    pub colors: [[f32; 4]; 6],
    /// Atmosphere color (rgb) and thickness (a, 0..1; 0 = airless).
    pub atmosphere: [f32; 4],
    /// Cloud color (rgb) and coverage (a, 0..1).
    pub clouds: [f32; 4],
    /// Night-side city lights color (rgb) and amount (a); volcanic worlds: fissure glow color.
    pub lights: [f32; 4],
    /// Seed, sea level (0..1 of the height range), terrain roughness (mountains and relief),
    /// ice line (0..1 of the way from the equator to the poles; also sets the climate: biomes
    /// are temperate halfway there).
    pub shape: [f32; 4],
    /// Dryness (0 = soaked, 0.3 = Earth-like, 1 = desert), gas giant bands (0 = rocky), band
    /// turbulence, ring (0/1).
    pub style: [f32; 4],
}

impl Default for PlanetLook {
    fn default() -> Self {
        let c = |hex: u32| Color::hex(hex).to_array();
        Self {
            colors: [
                c(0x0b2448),
                c(0x1b6a8a),
                c(0x2e5626),
                c(0xb39563),
                c(0x75695c),
                c(0xf0f3f7),
            ],
            atmosphere: [0.3, 0.55, 1.0, 1.0],
            clouds: [1.0, 1.0, 1.0, 0.48],
            lights: [1.0, 0.72, 0.38, 0.7],
            shape: [1.0, 0.555, 1.0, 0.82],
            style: [0.3, 0.0, 0.0, 0.0],
        }
    }
}

/// Up to this many planet looks per frame.
pub const MAX_PLANET_LOOKS: usize = 64;

/// The frame's instances and planet looks.
#[derive(Resource, Default, Debug, Clone)]
pub struct DrawList {
    pub instances: Vec<UiInstance>,
    pub looks: Vec<PlanetLook>,
    /// Whether anything uses the frosted backdrop (the renderer then blurs the scene).
    pub uses_backdrop: bool,
    /// Glyph quads this frame (diagnostics, tests).
    pub glyphs: u32,
}

impl DrawList {
    pub fn clear(&mut self) {
        self.instances.clear();
        self.looks.clear();
        self.uses_backdrop = false;
        self.glyphs = 0;
    }

    pub fn push(&mut self, i: UiInstance) {
        self.instances.push(i);
    }

    /// Add a planet look; returns its index (reuses identical looks).
    pub fn look(&mut self, look: PlanetLook) -> u32 {
        if let Some(i) = self.looks.iter().position(|l| *l == look) {
            return i as u32;
        }
        if self.looks.len() >= MAX_PLANET_LOOKS {
            return 0;
        }
        self.looks.push(look);
        (self.looks.len() - 1) as u32
    }
}

/// Where a glyph or icon lives in an atlas.
#[derive(Debug, Clone, Copy)]
pub struct AtlasSlot {
    /// 0 = mask atlas, 1 = color atlas.
    pub page: u32,
    pub uv: [f32; 4],
    /// Bitmap offset from the pen position (glyphs) and size, physical px.
    pub left: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
}

/// The glyph (mask) and color atlases with their caches.
#[derive(Resource)]
pub struct UiAtlases {
    pub mask: Atlas,
    pub color: Atlas,
    glyphs: HashMap<CacheKey, Option<AtlasSlot>>,
    icons: HashMap<(Arc<str>, u32, u32, bool), Option<AtlasSlot>>,
    /// Something didn't fit this frame: rebuild after growing/clearing.
    pub overflowed: bool,
}

impl Default for UiAtlases {
    fn default() -> Self {
        Self {
            mask: Atlas::new(1024, 1024, 1, 4096),
            color: Atlas::new(2048, 2048, 4, 4096),
            glyphs: HashMap::new(),
            icons: HashMap::new(),
            overflowed: false,
        }
    }
}

impl UiAtlases {
    /// Rasterize (once) and return a glyph's slot.
    pub fn glyph(
        &mut self,
        system: &mut FontSystem,
        swash: &mut SwashCache,
        key: CacheKey,
    ) -> Option<AtlasSlot> {
        if let Some(s) = self.glyphs.get(&key) {
            return *s;
        }
        let image = swash.get_image_uncached(system, key);
        let slot = image.and_then(|img| {
            let (w, h) = (img.placement.width, img.placement.height);
            if w == 0 || h == 0 {
                return None;
            }
            let (atlas, page, pixels) = match img.content {
                SwashContent::Mask => (&mut self.mask, 0, img.data),
                SwashContent::SubpixelMask => {
                    // Average the channels: the UI blends in linear space without subpixel AA.
                    let px = img
                        .data
                        .chunks(4)
                        .map(|p| ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8)
                        .collect();
                    (&mut self.mask, 0, px)
                }
                SwashContent::Color => (&mut self.color, 1, unpremultiply(img.data)),
            };
            let Some((x, y)) = atlas.alloc(w, h) else {
                self.overflowed = true;
                return None;
            };
            atlas.write(x, y, w, h, &pixels);
            Some(AtlasSlot {
                page,
                uv: atlas.uv(x, y, w, h),
                left: img.placement.left as f32,
                top: img.placement.top as f32,
                w: w as f32,
                h: h as f32,
            })
        });
        if !self.overflowed {
            self.glyphs.insert(key, slot);
        }
        slot
    }

    /// Rasterize (once per size) and return an icon's slot.
    pub fn icon(
        &mut self,
        icons: &mut IconLibrary,
        name: &str,
        w: u32,
        h: u32,
        flip: bool,
    ) -> Option<AtlasSlot> {
        let (w, h) = (w.clamp(1, 2048), h.clamp(1, 2048));
        let key = (Arc::<str>::from(name), w, h, flip);
        if let Some(s) = self.icons.get(&key) {
            return *s;
        }
        let slot = icons
            .rasterize(name, w, h, flip)
            .and_then(|(pw, ph, pixels)| {
                let Some((x, y)) = self.color.alloc(pw, ph) else {
                    self.overflowed = true;
                    return None;
                };
                self.color.write(x, y, pw, ph, &pixels);
                Some(AtlasSlot {
                    page: 1,
                    uv: self.color.uv(x, y, pw, ph),
                    left: (w as f32 - pw as f32) * 0.5,
                    top: (h as f32 - ph as f32) * 0.5,
                    w: pw as f32,
                    h: ph as f32,
                })
            });
        if !self.overflowed {
            self.icons.insert(key, slot);
        }
        slot
    }

    /// After an overflow: grow the full atlas (or clear it at the maximum) and drop caches.
    pub fn recover(&mut self) {
        self.overflowed = false;
        for atlas in [&mut self.mask, &mut self.color] {
            if atlas.usage() > 0.9 && !atlas.grow() {
                atlas.clear();
            }
        }
        self.glyphs.clear();
        self.icons.clear();
    }
}

fn unpremultiply(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.chunks_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    px
}

/// Everything a painter needs to draw: the output list, fonts, atlases, icons and frame info.
pub struct DrawCtx<'a> {
    pub list: &'a mut DrawList,
    pub fonts: &'a mut Fonts,
    pub atlases: &'a mut UiAtlases,
    pub icons: &'a mut IconLibrary,
    pub viewport: UiViewport,
    /// Real seconds since start (animations).
    pub time: f32,
    /// The painted node's box, clip, opacity and scale (physical px per logical px × node
    /// scale).
    pub rect: Rect,
    pub clip: Rect,
    pub opacity: f32,
    pub px: f32,
}

impl DrawCtx<'_> {
    fn clip_ok(&self, r: &Rect) -> bool {
        !r.intersect(&self.clip).is_empty()
    }

    /// A rounded box with a solid fill and optional border.
    pub fn rect(&mut self, r: Rect, fill: Color, radius: f32, border: f32, border_color: Color) {
        if !self.clip_ok(&r) {
            return;
        }
        let mut i = UiInstance::new(kind::RECT, r, self.clip);
        i.color = fill.fade(self.opacity).to_array();
        i.radius = [radius; 4];
        i.border_color = border_color.fade(self.opacity).to_array();
        i.params[0] = border;
        self.list.push(i);
    }

    /// A soft glow of `radius` px around `center`; `falloff` > 1 makes the core tighter.
    pub fn glow(&mut self, center: Vec2, radius: f32, color: Color, falloff: f32) {
        let r = Rect::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        );
        if !self.clip_ok(&r) || color.a <= 0.0 {
            return;
        }
        let mut i = UiInstance::new(kind::GLOW, r, self.clip);
        i.color = color.fade(self.opacity).to_array();
        i.params[0] = falloff;
        self.list.push(i);
    }

    /// An ellipse outline (or arc from `start` over `sweep` radians, 0 = up, clockwise);
    /// `dashes` > 0 draws that many dashes.
    #[allow(clippy::too_many_arguments)]
    pub fn ring(
        &mut self,
        center: Vec2,
        radii: Vec2,
        thickness: f32,
        color: Color,
        start: f32,
        sweep: f32,
        dashes: f32,
    ) {
        let pad = thickness + 2.0;
        let r = Rect::new(
            center.x - radii.x - pad,
            center.y - radii.y - pad,
            (radii.x + pad) * 2.0,
            (radii.y + pad) * 2.0,
        );
        if !self.clip_ok(&r) || color.a <= 0.0 {
            return;
        }
        let mut i = UiInstance::new(kind::RING, r, self.clip);
        i.color = color.fade(self.opacity).to_array();
        i.params = [thickness, start, sweep, dashes];
        i.uv = [radii.x, radii.y, 0.0, 0.0];
        self.list.push(i);
    }

    /// A line from `a` to `b`; `dash` > 0 is the dash period in px.
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Color, dash: f32) {
        let d = b - a;
        let len = d.length();
        if len < 0.01 || color.a <= 0.0 {
            return;
        }
        let c = (a + b) * 0.5;
        let w = width + 2.0;
        let r = Rect::new(c.x - len * 0.5 - 1.0, c.y - w * 0.5, len + 2.0, w);
        let bounds = Rect::new(
            a.x.min(b.x) - w,
            a.y.min(b.y) - w,
            d.x.abs() + 2.0 * w,
            d.y.abs() + 2.0 * w,
        );
        if !self.clip_ok(&bounds) {
            return;
        }
        let mut i = UiInstance::new(kind::LINE, r, self.clip);
        i.color = color.fade(self.opacity).to_array();
        i.params = [width, dash, len, 0.0];
        i.extra[0] = d.y.atan2(d.x);
        self.list.push(i);
    }

    /// An icon fitted into `r`.
    pub fn icon(&mut self, name: &str, r: Rect, tint: Color) {
        if !self.clip_ok(&r) {
            return;
        }
        self.icon_rotated(name, r, tint, 0.0);
    }

    /// An icon fitted into `r`, turned by `angle` radians (clockwise) about its center.
    pub fn icon_rotated(&mut self, name: &str, r: Rect, tint: Color, angle: f32) {
        if !self.clip_ok(&r.inflate(r.w.max(r.h) * 0.25)) {
            return;
        }
        let (w, h, k) = raster_size(r.w, r.h);
        if let Some(slot) = self.atlases.icon(self.icons, name, w, h, false) {
            let q = if k == 1.0 && angle == 0.0 {
                Rect::new(
                    r.x.round() + slot.left,
                    r.y.round() + slot.top,
                    slot.w,
                    slot.h,
                )
            } else {
                Rect::new(
                    r.x + slot.left * k,
                    r.y + slot.top * k,
                    slot.w * k,
                    slot.h * k,
                )
            };
            let mut i = UiInstance::new(kind::IMAGE, q, self.clip);
            i.uv = slot.uv;
            i.color = tint.fade(self.opacity).to_array();
            i.extra[0] = angle;
            self.list.push(i);
        }
    }

    /// Shape (cached) and draw a label with its top-left at `pos`. Returns its size.
    pub fn label(&mut self, spec: &TextSpec, pos: Vec2) -> Vec2 {
        let (opacity, clip) = (self.opacity, self.clip);
        let (buffer, size, system, swash) = self.fonts.label_and_raster(spec);
        draw_buffer(
            self.list,
            self.atlases,
            system,
            swash,
            buffer,
            pos,
            spec.color,
            opacity,
            clip,
            None,
            false,
        );
        Vec2::new(size[0], size[1])
    }

    /// Measure a label without drawing it.
    pub fn measure(&mut self, spec: &TextSpec) -> Vec2 {
        let (_, size) = self.fonts.label(spec);
        Vec2::new(size[0], size[1])
    }

    /// A one-line label: `size` in logical px (scaled by the node), anchored at `pos` by
    /// `anchor` (0,0 = top-left, 0.5,0.5 = centered, 1,1 = bottom-right). Returns its box.
    pub fn text(
        &mut self,
        text: &str,
        size: f32,
        weight: u16,
        color: Color,
        pos: Vec2,
        anchor: Vec2,
    ) -> Rect {
        let spec = TextSpec {
            text: text.to_string(),
            size: size * self.px,
            weight,
            color,
            wrap: false,
            ..Default::default()
        };
        self.text_spec(&spec, pos, anchor)
    }

    /// Like [`DrawCtx::text`] with full control over the spec (size in physical px).
    pub fn text_spec(&mut self, spec: &TextSpec, pos: Vec2, anchor: Vec2) -> Rect {
        let size = self.measure(spec);
        let top_left = pos - size * anchor;
        self.label(spec, top_left);
        Rect::new(top_left.x, top_left.y, size.x, size.y)
    }

    /// A box with any fill, per-corner radii and a border.
    pub fn rect_fill(
        &mut self,
        r: Rect,
        fill: Fill,
        radius: [f32; 4],
        border: f32,
        border_color: Color,
    ) {
        if !self.clip_ok(&r) {
            return;
        }
        let mut i = UiInstance::new(kind::RECT, r, self.clip);
        i.radius = radius;
        i.params = [border, 0.0, 0.0, self.opacity];
        i.border_color = border_color.to_array();
        match fill {
            Fill::None => {}
            Fill::Solid(c) => {
                i.color = c.to_array();
                i.color2 = c.to_array();
            }
            Fill::Linear {
                from,
                to,
                angle_deg,
            } => {
                i.color = from.to_array();
                i.color2 = to.to_array();
                i.params[2] = angle_deg.to_radians();
                i.kind[1] |= flag::FILL_LINEAR;
            }
            Fill::Radial { inner, outer } => {
                i.color = inner.to_array();
                i.color2 = outer.to_array();
                i.kind[1] |= flag::FILL_RADIAL;
            }
        }
        self.list.push(i);
    }

    /// A soft drop shadow under the box `r`.
    pub fn shadow(&mut self, r: Rect, radius: f32, blur: f32, color: Color) {
        let quad = r.inflate(blur * 1.5 + 1.0);
        if !self.clip_ok(&quad) {
            return;
        }
        let mut i = UiInstance::new(kind::SHADOW, quad, self.clip);
        i.uv = [r.x, r.y, r.w, r.h];
        i.color = color.fade(self.opacity).to_array();
        i.radius = [radius; 4];
        i.params[1] = blur.max(0.5);
        self.list.push(i);
    }

    /// A lit 3D planet of `radius` px at `center`: `yaw`/`pitch` turn it (radians), the sun
    /// comes from `sun_azimuth`/`sun_elevation` (radians, view space), `halo` is the
    /// atmosphere's width as a fraction of the radius.
    #[allow(clippy::too_many_arguments)]
    pub fn planet(
        &mut self,
        center: Vec2,
        radius: f32,
        look: PlanetLook,
        yaw: f32,
        pitch: f32,
        sun_azimuth: f32,
        sun_elevation: f32,
        halo: f32,
    ) {
        let outer = radius * (1.0 + halo);
        let r = Rect::new(center.x - outer, center.y - outer, outer * 2.0, outer * 2.0);
        if !self.clip_ok(&r) || radius < 0.5 {
            return;
        }
        let look = self.list.look(look);
        let mut i = UiInstance::new(kind::PLANET, r, self.clip);
        i.uv = [yaw, pitch, sun_azimuth, sun_elevation];
        i.extra[1] = halo;
        i.kind[2] = look;
        i.params[3] = self.opacity;
        self.list.push(i);
    }

    /// An equirectangular map of a planet over `r`, from longitude/latitude `lon[0]`,
    /// `lat[0]` (top-left) to `lon[1]`, `lat[1]` (radians). `clouds` and `grid` 0..1.
    #[allow(clippy::too_many_arguments)]
    pub fn planet_flat(
        &mut self,
        r: Rect,
        look: PlanetLook,
        lon: [f32; 2],
        lat: [f32; 2],
        clouds: f32,
        grid: f32,
        radius: f32,
    ) {
        if !self.clip_ok(&r) {
            return;
        }
        let look = self.list.look(look);
        let mut i = UiInstance::new(kind::PLANET_FLAT, r, self.clip);
        i.uv = [lon[0], lat[0], lon[1], lat[1]];
        i.extra[2] = clouds;
        i.extra[3] = grid;
        i.kind[2] = look;
        i.radius = [radius; 4];
        i.params[3] = self.opacity;
        self.list.push(i);
    }

    /// A spiral galaxy centered at `center` with `radius` px, turned by `rotation` and
    /// tilted (`tilt` 1 = face on, 0.3 = steep), drawn inside `area`.
    pub fn galaxy(&mut self, area: Rect, center: Vec2, radius: f32, rotation: f32, tilt: f32) {
        let r = area.intersect(&Rect::new(
            center.x - radius * 1.6,
            center.y - radius * 1.6 * tilt.max(0.05),
            radius * 3.2,
            radius * 3.2 * tilt.max(0.05),
        ));
        if !self.clip_ok(&r) || r.is_empty() {
            return;
        }
        let mut i = UiInstance::new(kind::GALAXY, r, self.clip);
        i.uv = [center.x, center.y, radius, rotation];
        i.extra[1] = tilt;
        i.params[3] = self.opacity;
        self.list.push(i);
    }

    /// Twinkling stars and faint nebulae over `r`; `pan` (px) moves the layers with parallax.
    pub fn starfield(&mut self, r: Rect, pan: Vec2, density: f32, seed: f32, radius: f32) {
        if !self.clip_ok(&r) {
            return;
        }
        let mut i = UiInstance::new(kind::STARFIELD, r, self.clip);
        i.uv = [pan.x, pan.y, density, seed];
        i.radius = [radius; 4];
        i.params[3] = self.opacity;
        self.list.push(i);
    }

    /// Run `f` with a narrower clip rectangle.
    pub fn with_clip<R>(&mut self, clip: Rect, f: impl FnOnce(&mut Self) -> R) -> R {
        let old = self.clip;
        self.clip = old.intersect(&clip);
        let r = f(self);
        self.clip = old;
        r
    }

    /// Run `f` with the opacity multiplied by `k`.
    pub fn faded<R>(&mut self, k: f32, f: impl FnOnce(&mut Self) -> R) -> R {
        let old = self.opacity;
        self.opacity *= k;
        let r = f(self);
        self.opacity = old;
        r
    }
}

/// Draw a shaped buffer with its top-left at `origin`.
#[allow(clippy::too_many_arguments)]
pub fn draw_buffer(
    list: &mut DrawList,
    atlases: &mut UiAtlases,
    system: &mut FontSystem,
    swash: &mut SwashCache,
    buffer: &Buffer,
    origin: Vec2,
    default_color: Color,
    opacity: f32,
    clip: Rect,
    reveal: Option<(f32, &[usize])>,
    shadow: bool,
) {
    let origin = Vec2::new(origin.x.round(), origin.y.round());
    let start = list.instances.len();
    for run in buffer.layout_runs() {
        if origin.y + run.line_top > clip.y + clip.h
            || origin.y + run.line_top + run.line_height < clip.y
        {
            continue;
        }
        let line_base = reveal.and_then(|(_, lines)| lines.get(run.line_i).copied());
        for g in run.glyphs.iter() {
            let mut alpha = 1.0;
            if let (Some((visible, _)), Some(base)) = (reveal, line_base) {
                let idx = base + run.text[..g.start.min(run.text.len())].chars().count();
                let t = visible - idx as f32;
                if t <= 0.0 {
                    continue;
                }
                alpha = t.min(1.0);
            }
            let p = g.physical((origin.x, origin.y + run.line_y), 1.0);
            let Some(slot) = atlases.glyph(system, swash, p.cache_key) else {
                continue;
            };
            let r = Rect::new(
                p.x as f32 + slot.left,
                p.y as f32 - slot.top,
                slot.w,
                slot.h,
            );
            let color = g.color_opt.map(from_cosmic).unwrap_or(default_color);
            let mut i = UiInstance::new(
                if slot.page == 0 {
                    kind::GLYPH
                } else {
                    kind::IMAGE
                },
                r,
                clip,
            );
            i.uv = slot.uv;
            i.color = if slot.page == 0 {
                color.fade(opacity * alpha).to_array()
            } else {
                Color::WHITE.fade(opacity * alpha * color.a).to_array()
            };
            list.push(i);
            list.glyphs += 1;
        }
    }
    if shadow {
        // Insert dark copies of the glyphs underneath, nudged down-right.
        let glyphs: Vec<UiInstance> = list.instances[start..].to_vec();
        let mut shadows: Vec<UiInstance> = glyphs
            .iter()
            .filter(|g| g.kind[0] == kind::GLYPH)
            .map(|g| {
                let mut s = *g;
                s.rect[0] += 1.0;
                s.rect[1] += 1.5;
                s.color = [0.0, 0.0, 0.0, g.color[3] * 0.75];
                s.params[1] = 1.0;
                s
            })
            .collect();
        shadows.extend(glyphs);
        list.instances.truncate(start);
        list.instances.extend(shadows);
    }
}

/// A custom painter: draws the node `entity` (its box is `ctx.rect`).
pub type PaintFn = fn(&mut World, Entity, &mut DrawCtx);

/// Painters by [`CustomPaint::kind`].
#[derive(Resource, Default, Clone)]
pub struct Painters(pub HashMap<String, PaintFn>);

impl Painters {
    pub fn add(&mut self, kind: &str, f: PaintFn) {
        self.0.insert(kind.to_string(), f);
    }
}

/// Build the frame's draw list from the laid-out tree.
pub fn build_draw_list(world: &mut World) {
    let mut list = world.remove_resource::<DrawList>().unwrap_or_default();
    let mut fonts = world.remove_resource::<Fonts>().unwrap_or_default();
    let mut atlases = world.remove_resource::<UiAtlases>().unwrap_or_default();
    let mut icons = world.remove_resource::<IconLibrary>().unwrap_or_default();
    let texts = world.remove_resource::<TextLayouts>().unwrap_or_default();
    for attempt in 0..2 {
        list.clear();
        build(
            world,
            &mut list,
            &mut fonts,
            &mut atlases,
            &mut icons,
            &texts,
        );
        if !atlases.overflowed || attempt == 1 {
            break;
        }
        atlases.recover();
    }
    atlases.overflowed = false;
    world.insert_resource(texts);
    world.insert_resource(icons);
    world.insert_resource(atlases);
    world.insert_resource(fonts);
    world.insert_resource(list);
}

fn build(
    world: &mut World,
    list: &mut DrawList,
    fonts: &mut Fonts,
    atlases: &mut UiAtlases,
    icons: &mut IconLibrary,
    texts: &TextLayouts,
) {
    let settings = world.resource::<UiSettings>().clone();
    if !settings.enabled {
        return;
    }
    let vp = *world.resource::<UiViewport>();
    let time = world
        .resource::<engine_core::prelude::Time>()
        .real_elapsed_secs_f64() as f32;
    let order = world.resource::<UiDrawOrder>().nodes.clone();
    let painters = world
        .get_resource::<Painters>()
        .cloned()
        .unwrap_or_default();
    for e in order {
        let Some(c) = world.get::<ComputedNode>(e) else {
            continue;
        };
        if !c.visible {
            continue;
        }
        let c = c.clone();
        let s = &c.style;
        let px = c.scale * vp.scale;
        let rect = c.rect;
        let op = c.opacity;
        let radius = s.radius.0.map(|r| (r * px).min(rect.w.min(rect.h) * 0.5));

        // Shadow.
        if s.shadow.color.a > 0.0 && s.shadow.blur >= 0.0 {
            let blur = s.shadow.blur * px;
            let bx = rect
                .offset(Vec2::from(s.shadow.offset) * px)
                .inflate(s.shadow.spread * px);
            let quad = bx.inflate(blur * 1.5 + 1.0);
            let mut i = UiInstance::new(kind::SHADOW, quad, c.clip);
            i.uv = [bx.x, bx.y, bx.w, bx.h];
            i.color = s.shadow.color.fade(op).to_array();
            i.radius = radius.map(|r| r + s.shadow.spread * px);
            i.params[1] = blur.max(0.5);
            list.push(i);
        }

        // Box: backdrop, fill, border.
        let has_fill = s.background.is_visible();
        let has_border = s.border_width > 0.0 && s.border_color.a > 0.0;
        if has_fill || has_border || s.backdrop_blur > 0.0 {
            let mut i = UiInstance::new(kind::RECT, rect, c.clip);
            i.radius = radius;
            i.params = [
                if has_border { s.border_width * px } else { 0.0 },
                0.0,
                0.0,
                op,
            ];
            i.border_color = s.border_color.to_array();
            match s.background {
                Fill::None => {}
                Fill::Solid(col) => {
                    i.color = col.to_array();
                    i.color2 = col.to_array();
                }
                Fill::Linear {
                    from,
                    to,
                    angle_deg,
                } => {
                    i.color = from.to_array();
                    i.color2 = to.to_array();
                    i.params[2] = angle_deg.to_radians();
                    i.kind[1] |= flag::FILL_LINEAR;
                }
                Fill::Radial { inner, outer } => {
                    i.color = inner.to_array();
                    i.color2 = outer.to_array();
                    i.kind[1] |= flag::FILL_RADIAL;
                }
            }
            if s.backdrop_blur > 0.0 {
                i.kind[1] |= flag::BACKDROP;
                i.extra[1] = s.backdrop_blur * px;
                list.uses_backdrop = true;
            }
            i.extra[0] = s.rotate_deg.to_radians();
            list.push(i);
        }

        // Content box (inside border and padding), for text and icons.
        let content = content_box(&c, px);

        // Icon.
        if let Some(icon) = world.get::<Icon>(e)
            && !icon.name.is_empty()
        {
            // Cover: fit the icon's shorter side to the box and mask the overflow.
            let mut target = content;
            let cover = icon.cover.then(|| icons.aspect(&icon.name)).flatten();
            if let Some(a) = cover
                && content.w > 0.0
                && content.h > 0.0
            {
                let (cw, ch) = if a > content.w / content.h {
                    (content.h * a, content.h)
                } else {
                    (content.w, content.w / a)
                };
                target = Rect::new(
                    content.x + (content.w - cw) * 0.5,
                    content.y + (content.h - ch) * 0.2,
                    cw,
                    ch,
                );
            }
            let content = target;
            let (w, h, k) = raster_size(content.w, content.h);
            if let Some(slot) = atlases.icon(icons, &icon.name, w, h, icon.flip_x) {
                let q = if k == 1.0 {
                    Rect::new(
                        content.x.round() + slot.left,
                        content.y.round() + slot.top,
                        slot.w,
                        slot.h,
                    )
                } else {
                    Rect::new(
                        content.x + slot.left * k,
                        content.y + slot.top * k,
                        slot.w * k,
                        slot.h * k,
                    )
                };
                let mut i = UiInstance::new(kind::IMAGE, q, c.clip);
                i.uv = slot.uv;
                let tint = icon.tint.unwrap_or(if icons.is_glyph(&icon.name) {
                    c.color
                } else {
                    Color::WHITE
                });
                i.color = tint.fade(op).to_array();
                if icon.desaturate {
                    i.kind[1] |= flag::DESATURATE;
                }
                if cover.is_some() {
                    i.kind[1] |= flag::MASK;
                    i.border_color = [rect.x, rect.y, rect.x + rect.w, rect.y + rect.h];
                    i.radius = radius;
                }
                i.extra[0] = s.rotate_deg.to_radians();
                list.push(i);
            }
        }

        // Text.
        if let (Some(text), Some(entry)) = (world.get::<Text>(e), texts.entries.get(&e)) {
            let clip = if s.overflow != crate::style::Overflow::Visible {
                c.clip.intersect(&rect)
            } else {
                c.clip
            };
            let color = text.color.unwrap_or(c.color);
            let reveal = text.reveal.map(|r| (r, entry.line_chars.as_slice()));
            draw_buffer(
                list,
                atlases,
                &mut fonts.system,
                &mut fonts.swash,
                &entry.buffer,
                content.min(),
                color,
                op,
                clip,
                reveal,
                text.shadow,
            );
        }

        // Custom painters.
        if let Some(p) = world.get::<CustomPaint>(e)
            && let Some(f) = painters.0.get(&p.kind).copied()
        {
            let mut ctx = DrawCtx {
                list,
                fonts,
                atlases,
                icons,
                viewport: vp,
                time,
                rect,
                clip: c.clip.intersect(&rect),
                opacity: op,
                px,
            };
            f(world, e, &mut ctx);
        }

        if settings.debug_outlines {
            let mut i = UiInstance::new(kind::RECT, rect, Rect::EVERYTHING);
            i.border_color = Color::hexa(0xff00ff99).to_array();
            i.params[0] = 1.0;
            i.params[3] = 1.0;
            list.push(i);
        }
    }
}

/// Pixel size to rasterize an icon drawn at `w`x`h` physical px, and the factor to draw that
/// raster at. Small icons are exact (crisp); big ones (portraits) snap to sizes a few percent
/// apart, so a portrait that scales in an animation reuses a handful of rasters instead of
/// rasterizing a new size every frame.
fn raster_size(w: f32, h: f32) -> (u32, u32, f32) {
    let (w, h) = (w.round().max(1.0), h.round().max(1.0));
    let m = w.max(h);
    if m <= 96.0 {
        return (w as u32, h as u32, 1.0);
    }
    // Eighth-octave steps (about 9% apart), rounded up so the raster is never upscaled.
    let bucket = 2f32.powf((m.log2() * 8.0).ceil() / 8.0);
    let f = bucket / m;
    let (rw, rh) = ((w * f).round().max(1.0), (h * f).round().max(1.0));
    (rw as u32, rh as u32, w / rw)
}

/// The node's content box (inside border and padding), physical px.
fn content_box(c: &ComputedNode, px: f32) -> Rect {
    let s = &c.style;
    let resolve = |v: crate::style::Val, basis: f32| match v {
        crate::style::Val::Px(p) => p * px,
        crate::style::Val::Percent(p) => p / 100.0 * basis,
        crate::style::Val::Auto => 0.0,
    };
    let w = c.rect.w;
    let b = s.border_width * px;
    let l = resolve(s.padding.left, w) + b;
    let r = resolve(s.padding.right, w) + b;
    let t = resolve(s.padding.top, w) + b;
    let bt = resolve(s.padding.bottom, w) + b;
    Rect::new(
        c.rect.x + l,
        c.rect.y + t,
        (c.rect.w - l - r).max(0.0),
        (c.rect.h - t - bt).max(0.0),
    )
}

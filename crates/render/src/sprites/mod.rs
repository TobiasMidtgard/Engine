//! 2D and 2.5D rendering: sprites, sprite sheets and their animation, tile maps, parallax
//! layers and pixel-perfect cameras.
//!
//! - [`Sprite`]: an image (or a cell of a sprite sheet) drawn as a quad at its entity's
//!   transform: world-aligned ([`Billboard::Fixed`]), facing the camera
//!   ([`Billboard::FaceCamera`]) or standing upright and turning only around its vertical axis
//!   ([`Billboard::Upright`], paper characters and trees in a 3D diorama).
//! - [`SpriteAtlas`] cuts an image into frames (a grid of cells or explicit rectangles), and
//!   [`SpriteAnimation`] plays a list of them at a frame rate, looping, once or ping-pong, on
//!   game time: slow motion ([`TimeDilation`]) and [`LocalTimeScale`] slow it down too.
//! - [`TileMap`]: a grid of tiles from a tileset atlas, drawn with one instanced draw per
//!   visible chunk of 32 × 32 tiles (uploaded once, redrawn without CPU work while unchanged),
//!   so a level of a million tiles costs a handful of draw calls. Empty tiles cost nothing.
//! - [`Parallax`]: background layers that follow the camera at a fraction of its movement,
//!   optionally repeated side by side without end.
//! - [`PixelPerfect`] on an orthographic camera: whole-number scaling of the art's pixels and a
//!   camera (and sprites) that move in whole texels, so pixel art stays crisp with no shimmer.
//!
//! Sprites and tiles can be unlit (pure 2D: their colors as they are) or lit
//! ([`Sprite::lit`]) by the sun, point lights and sky light with shadows, optionally with a
//! normal map, shaded as thin sheets (paper, cloth, leaves) that let `translucency` of the light
//! from behind through. [`SpriteAlpha::Cutout`] sprites write depth, go through the depth
//! prepass (ambient occlusion, contact shadows, motion vectors) and cast sun shadows, so a paper
//! character in a 3D diorama sorts and shadows correctly against meshes. Blended and additive
//! sprites draw after the opaque scene, sorted by [`Sprite::layer`] and then back to front.
//!
//! ```ignore
//! // A 2D side-scroller: an orthographic camera that keeps 16-texel-per-meter art crisp.
//! commands.spawn((
//!     Camera::orthographic(11.25),
//!     PixelPerfect::new(16.0),
//!     Transform::from_xyz(0.0, 0.0, 10.0),
//! ));
//! // A character from a sprite sheet of 8 × 4 cells of 32 × 32 px, running.
//! commands.spawn((
//!     Transform::from_xyz(0.0, 1.0, 0.0),
//!     Sprite::pixel_art(sheet, [32, 32], 16.0)
//!         .with_atlas(SpriteAtlas::grid(8, 4))
//!         .with_anchor([0.5, 0.0]),
//!     SpriteAnimation::new(8..16, 12.0),
//! ));
//! // A level: tile values are atlas frames + 1 (0 = empty).
//! let mut level = TileMap::new(tileset, SpriteAtlas::grid(16, 16), [400, 30], [1.0, 1.0]);
//! level.set(3, 29, 1);
//! commands.spawn((Transform::default(), level));
//! ```

mod feature;

use engine_assets::Handle;
use engine_core::prelude::*;

pub use feature::SpriteFeature;
#[cfg(test)]
pub(crate) use feature::shader_sources;

use crate::components::{Camera, Projection};
use crate::texture::Image;

/// How a sprite turns toward the camera.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Billboard {
    /// Lies in its entity's local x/y plane, facing +z (a 2D camera looks down −z at it).
    #[default]
    Fixed,
    /// Always parallel to the screen: particles, icons, flat 2.5D characters seen from any
    /// angle. Ignores the entity's rotation.
    FaceCamera,
    /// Stands along its entity's up axis and turns only around it to face the camera, like
    /// characters and trees in 2.5D games (Octopath Traveler, Don't Starve): it never tips over
    /// as the camera looks down on it.
    Upright,
}

/// How the image's texels are filtered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpriteFilter {
    /// Bilinear with mipmaps: smooth, painted or high-resolution art.
    #[default]
    Linear,
    /// Nearest neighbour: every screen pixel shows exactly one texel. Uneven texel widths
    /// appear wherever a texel doesn't cover a whole number of pixels (non-integer scales,
    /// rotations).
    Nearest,
    /// Pixel art: crisp texels like `Nearest`, but a texel edge that falls inside a screen
    /// pixel is blended over that one pixel, so texels stay evenly sized at any scale, rotation
    /// or sub-pixel position. Identical to `Nearest` where texels cover whole pixels (under a
    /// [`PixelPerfect`] camera).
    PixelArt,
}

/// How a sprite's alpha is used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum SpriteAlpha {
    /// Alpha blending over what is behind, after the opaque scene: soft edges, glass, smoke.
    /// Sorted by [`Sprite::layer`], then back to front. Casts no shadow.
    #[default]
    Blend,
    /// Adds its color (times alpha) to what is behind: fire, sparks, magic glows.
    Additive,
    /// Texels with alpha below the threshold are cut away, the rest is opaque: writes depth,
    /// goes through the depth prepass and casts sun shadows (with the cut-out holes). Paper
    /// characters, foliage cards, pixel art with hard edges.
    Cutout(f32),
}

/// How an image is cut into frames: the cells of a sprite sheet or a tileset.
///
/// Frames are numbered from 0. Rectangles are in texels from the image's top-left corner.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum SpriteAtlas {
    /// The whole image is the only frame.
    #[default]
    Whole,
    /// A grid of equal cells, numbered left to right, then top to bottom (frame 0 at the
    /// top-left).
    Grid {
        /// Cells per row.
        columns: u32,
        /// Rows of cells.
        rows: u32,
        /// Size of a cell in texels; `[0, 0]` divides the image (less margins and spacing)
        /// evenly.
        cell: [u32; 2],
        /// Texels before the first cell, from the left and from the top.
        margin: [u32; 2],
        /// Texels between neighbouring cells, horizontally and vertically.
        spacing: [u32; 2],
    },
    /// Explicit frames: `[x, y, width, height]` in texels.
    Rects(Vec<[u32; 4]>),
}

impl SpriteAtlas {
    /// A grid of `columns × rows` equal cells covering the whole image.
    pub fn grid(columns: u32, rows: u32) -> Self {
        SpriteAtlas::Grid {
            columns,
            rows,
            cell: [0, 0],
            margin: [0, 0],
            spacing: [0, 0],
        }
    }

    /// A grid of cells of `cell` texels, with `margin` texels before the first and `spacing`
    /// between neighbours (as packed by most tileset tools).
    pub fn grid_cells(columns: u32, rows: u32, cell: [u32; 2], margin: u32, spacing: u32) -> Self {
        SpriteAtlas::Grid {
            columns,
            rows,
            cell,
            margin: [margin; 2],
            spacing: [spacing; 2],
        }
    }

    /// Number of frames.
    pub fn frame_count(&self) -> u32 {
        match self {
            SpriteAtlas::Whole => 1,
            SpriteAtlas::Grid { columns, rows, .. } => columns * rows,
            SpriteAtlas::Rects(rects) => rects.len() as u32,
        }
    }

    /// The rectangle of `frame` in texels (`[x, y, width, height]` from the top-left) of an
    /// image of `size` texels. Frames past the end wrap around; `None` for an empty atlas.
    pub fn frame_rect(&self, frame: u32, size: [u32; 2]) -> Option<[u32; 4]> {
        let count = self.frame_count();
        if count == 0 {
            return None;
        }
        let frame = frame % count;
        Some(match self {
            SpriteAtlas::Whole => [0, 0, size[0], size[1]],
            SpriteAtlas::Grid {
                columns,
                rows,
                cell,
                margin,
                spacing,
            } => {
                let (columns, rows) = ((*columns).max(1), (*rows).max(1));
                let fit = |axis: usize, n: u32| {
                    if cell[axis] > 0 {
                        cell[axis]
                    } else {
                        size[axis]
                            .saturating_sub(margin[axis] + spacing[axis] * (n - 1))
                            .checked_div(n)
                            .unwrap_or(0)
                    }
                };
                let (w, h) = (fit(0, columns), fit(1, rows));
                let (col, row) = (frame % columns, frame / columns);
                [
                    margin[0] + col * (w + spacing[0]),
                    margin[1] + row * (h + spacing[1]),
                    w,
                    h,
                ]
            }
            SpriteAtlas::Rects(rects) => rects[frame as usize],
        })
    }

    /// The texture coordinates of `frame` in an image of `size` texels: `[u0, v0, u1, v1]`,
    /// top-left to bottom-right.
    pub fn frame_uv(&self, frame: u32, size: [u32; 2]) -> Option<[f32; 4]> {
        let [x, y, w, h] = self.frame_rect(frame, size)?;
        let (sw, sh) = (size[0].max(1) as f32, size[1].max(1) as f32);
        Some([
            x as f32 / sw,
            y as f32 / sh,
            (x + w) as f32 / sw,
            (y + h) as f32 / sh,
        ])
    }
}

/// An image drawn as a quad at its entity's transform (see the [module docs](self)).
///
/// The quad is `size` meters, scaled by the entity's scale, with its `anchor` at the entity's
/// position. JSON: `{"image": 3, "size": [1, 2], "anchor": [0.5, 0], "atlas": {"Grid":
/// {"columns": 8, "rows": 4, "cell": [0, 0], "margin": [0, 0], "spacing": [0, 0]}}, "frame": 5,
/// "billboard": "Upright", "alpha": {"Cutout": 0.5}, "lit": true}`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Sprite {
    /// The picture (sRGB color and alpha). `None` draws a plain quad of `color`.
    pub image: Option<Handle<Image>>,
    /// A tangent-space normal map in the same layout as `image` (+y = up in the image, as in
    /// glTF), for lit sprites: bumps, folds and rounded shapes catch the light.
    pub normal_map: Option<Handle<Image>>,
    /// How `image` is cut into frames (a sprite sheet).
    pub atlas: SpriteAtlas,
    /// The frame of `atlas` shown, from 0 ([`SpriteAnimation`] sets it).
    pub frame: u32,
    /// Width and height in meters (world units), before the entity's scale.
    pub size: [f32; 2],
    /// The point of the sprite at its entity's position, as a fraction of its size from the
    /// bottom-left: `[0.5, 0.5]` the center (default), `[0.5, 0.0]` the feet.
    pub anchor: [f32; 2],
    /// Linear RGBA multiplied with the image; alpha is opacity. Values above 1 make unlit
    /// sprites glow (with bloom).
    pub color: [f32; 4],
    /// Mirror the image left to right (a character facing the other way).
    pub flip_x: bool,
    /// Mirror the image top to bottom.
    pub flip_y: bool,
    /// Draw order between sprites and tile maps at the same depth: higher layers draw in front.
    /// Blended sprites sort by layer, then back to front.
    pub layer: i32,
    pub billboard: Billboard,
    pub filter: SpriteFilter,
    pub alpha: SpriteAlpha,
    /// Lit by the sun, point lights and sky light, with shadows (false: the colors as they are,
    /// pure 2D).
    pub lit: bool,
    /// Lit sprites: perceptual roughness of the surface (paper 0.8, glossy paint 0.3).
    pub roughness: f32,
    /// Lit sprites: how much the shading normal bends outward toward the edges, so a flat
    /// sprite shades like a rounded volume (0 = flat paper, 1 = a ball): tree crowns, bushes.
    pub roundness: f32,
    /// Lit sprites: the share of light from behind that shines through (0 opaque card, 0.3
    /// paper, 0.6 leaves), tinted by the color.
    pub translucency: f32,
    /// Lit sprites: light emitted by the image's colors, cd/m² for white (lanterns, windows).
    pub emissive: f32,
    /// Cast sun shadows (cut-out sprites only).
    pub cast_shadows: bool,
    pub visible: bool,
}

impl Default for Sprite {
    fn default() -> Self {
        Self {
            image: None,
            normal_map: None,
            atlas: SpriteAtlas::Whole,
            frame: 0,
            size: [1.0, 1.0],
            anchor: [0.5, 0.5],
            color: [1.0; 4],
            flip_x: false,
            flip_y: false,
            layer: 0,
            billboard: Billboard::Fixed,
            filter: SpriteFilter::Linear,
            alpha: SpriteAlpha::Blend,
            lit: false,
            roughness: 0.8,
            roundness: 0.0,
            translucency: 0.0,
            emissive: 0.0,
            cast_shadows: true,
            visible: true,
        }
    }
}

impl Sprite {
    /// A sprite showing `image` at `size` meters.
    pub fn new(image: Handle<Image>, size: [f32; 2]) -> Self {
        Self {
            image: Some(image),
            size,
            ..Default::default()
        }
    }

    /// A plain quad of `color` (linear RGBA), `size` meters.
    pub fn solid(color: [f32; 4], size: [f32; 2]) -> Self {
        Self {
            color,
            size,
            ..Default::default()
        }
    }

    /// Pixel art: a frame of `frame_px` texels at `pixels_per_unit` texels per meter, crisp
    /// ([`SpriteFilter::PixelArt`]) and cut out at alpha 0.5.
    pub fn pixel_art(image: Handle<Image>, frame_px: [u32; 2], pixels_per_unit: f32) -> Self {
        let ppu = pixels_per_unit.max(1e-6);
        Self {
            image: Some(image),
            size: [frame_px[0] as f32 / ppu, frame_px[1] as f32 / ppu],
            filter: SpriteFilter::PixelArt,
            alpha: SpriteAlpha::Cutout(0.5),
            ..Default::default()
        }
    }

    pub fn with_atlas(mut self, atlas: SpriteAtlas) -> Self {
        self.atlas = atlas;
        self
    }

    pub fn with_frame(mut self, frame: u32) -> Self {
        self.frame = frame;
        self
    }

    pub fn with_anchor(mut self, anchor: [f32; 2]) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn with_layer(mut self, layer: i32) -> Self {
        self.layer = layer;
        self
    }

    pub fn with_billboard(mut self, billboard: Billboard) -> Self {
        self.billboard = billboard;
        self
    }

    pub fn with_filter(mut self, filter: SpriteFilter) -> Self {
        self.filter = filter;
        self
    }

    pub fn with_alpha(mut self, alpha: SpriteAlpha) -> Self {
        self.alpha = alpha;
        self
    }

    pub fn with_normal_map(mut self, normal_map: Handle<Image>) -> Self {
        self.normal_map = Some(normal_map);
        self
    }

    /// Lit by the scene (sun, point lights, sky light, shadows).
    pub fn lit(mut self) -> Self {
        self.lit = true;
        self
    }

    pub fn with_roundness(mut self, roundness: f32) -> Self {
        self.roundness = roundness;
        self
    }

    pub fn with_translucency(mut self, translucency: f32) -> Self {
        self.translucency = translucency;
        self
    }

    pub fn with_emissive(mut self, emissive: f32) -> Self {
        self.emissive = emissive;
        self
    }

    pub fn flipped_x(mut self, flip: bool) -> Self {
        self.flip_x = flip;
        self
    }
}

/// How a [`SpriteAnimation`] runs past its last frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpriteAnimationMode {
    /// Back to the first frame, forever.
    #[default]
    Loop,
    /// Stop on the last frame.
    Once,
    /// Forward, then backward, forever (first and last frames shown once per swing).
    PingPong,
}

/// Plays frames of its entity's [`Sprite`] atlas, advanced by game time: slow motion
/// ([`TimeDilation`]) slows it, a [`LocalTimeScale`] on the entity scales it, a paused game
/// stops it.
///
/// ```ignore
/// commands.spawn((Sprite::pixel_art(sheet, [32, 32], 16.0).with_atlas(SpriteAtlas::grid(8, 4)),
///                 SpriteAnimation::new(8..16, 12.0)));
/// ```
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpriteAnimation {
    /// Atlas frames in playing order; empty plays every frame of the atlas in order.
    pub frames: Vec<u32>,
    /// Frames per second.
    pub fps: f32,
    pub mode: SpriteAnimationMode,
    /// Playback speed (1 = as authored, 2 = twice as fast, negative plays backward).
    pub speed: f32,
    /// False holds the current frame.
    pub playing: bool,
    /// Seconds into the animation (advanced by game time; set it to jump).
    pub time: f32,
}

impl Default for SpriteAnimation {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            fps: 12.0,
            mode: SpriteAnimationMode::Loop,
            speed: 1.0,
            playing: true,
            time: 0.0,
        }
    }
}

impl SpriteAnimation {
    /// Play `frames` (atlas frame numbers, such as `8..16` or `[0, 1, 2, 1]`) at `fps`, looping.
    pub fn new(frames: impl IntoIterator<Item = u32>, fps: f32) -> Self {
        Self {
            frames: frames.into_iter().collect(),
            fps,
            ..Default::default()
        }
    }

    pub fn with_mode(mut self, mode: SpriteAnimationMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Number of frames played: `frames`, or every frame of an atlas of `atlas_frames`.
    fn length(&self, atlas_frames: u32) -> u32 {
        if self.frames.is_empty() {
            atlas_frames.max(1)
        } else {
            self.frames.len() as u32
        }
    }

    /// Steps in one cycle: `n` frames, or `2n − 2` for ping-pong.
    fn cycle(&self, n: u32) -> u32 {
        match self.mode {
            SpriteAnimationMode::PingPong if n > 1 => 2 * n - 2,
            _ => n,
        }
    }

    /// Seconds one cycle takes (`f32::INFINITY` when `fps` is 0).
    fn period(&self, atlas_frames: u32) -> f32 {
        self.cycle(self.length(atlas_frames)) as f32 / self.fps.abs().max(1e-6)
    }

    /// The atlas frame shown at `time` seconds into the animation.
    pub fn frame_at(&self, time: f32, atlas_frames: u32) -> u32 {
        let n = self.length(atlas_frames);
        let steps = (time * self.fps.abs()).floor().max(0.0);
        let step = match self.mode {
            SpriteAnimationMode::Once => (steps as u32).min(n - 1),
            SpriteAnimationMode::Loop => (steps as u64 % n as u64) as u32,
            SpriteAnimationMode::PingPong => {
                let s = (steps as u64 % self.cycle(n) as u64) as u32;
                if s < n { s } else { 2 * n - 2 - s }
            }
        };
        if self.frames.is_empty() {
            step
        } else {
            self.frames[step as usize]
        }
    }

    /// Whether a [`SpriteAnimationMode::Once`] animation has reached its last frame (playing
    /// forward) or its first (backward). Looping animations never finish.
    pub fn finished(&self, atlas_frames: u32) -> bool {
        match self.mode {
            SpriteAnimationMode::Once if self.speed >= 0.0 => {
                self.time * self.fps.abs() >= (self.length(atlas_frames) - 1) as f32
            }
            SpriteAnimationMode::Once => self.time <= 0.0,
            _ => false,
        }
    }

    /// Advance by `dt` seconds of the entity's game time.
    pub fn advance(&mut self, dt: f32, atlas_frames: u32) {
        if !self.playing || self.fps == 0.0 {
            return;
        }
        let t = self.time + dt * self.speed;
        let period = self.period(atlas_frames);
        self.time = match self.mode {
            // Clamp just past the last frame's start so it stays on it.
            SpriteAnimationMode::Once => t.clamp(0.0, (period - 0.5 / self.fps.abs()).max(0.0)),
            // Wrap so the clock never loses precision however long it runs.
            _ if period.is_finite() && period > 0.0 => t.rem_euclid(period),
            _ => t,
        };
    }
}

/// Advances every [`SpriteAnimation`] by its entity's game time and shows its frame.
pub fn animate_sprites(
    time: Res<Time>,
    mut query: Query<(&mut SpriteAnimation, &mut Sprite, Option<&LocalTimeScale>)>,
) {
    for (mut animation, mut sprite, local) in &mut query {
        let frames = sprite.atlas.frame_count();
        if animation.playing && animation.fps != 0.0 {
            animation.advance(time.delta_for(local), frames);
        }
        let frame = animation.frame_at(animation.time, frames);
        if sprite.frame != frame {
            sprite.frame = frame;
        }
    }
}

/// Tile value flag: mirror the tile left to right (Tiled's convention).
pub const TILE_FLIP_X: u32 = 1 << 31;
/// Tile value flag: mirror the tile top to bottom.
pub const TILE_FLIP_Y: u32 = 1 << 30;
/// Tile value flag: swap the tile's x and y (with the flips: rotations by 90°).
pub const TILE_FLIP_DIAGONAL: u32 = 1 << 29;
/// The bits of a tile value that hold the frame (the rest are flags).
pub const TILE_FRAME_MASK: u32 = !(TILE_FLIP_X | TILE_FLIP_Y | TILE_FLIP_DIAGONAL);

/// A grid of tiles from a tileset image, drawn as one piece: a 2D level, a parallax layer, a
/// floor of a 2.5D diorama.
///
/// The map lies in its entity's local x/y plane (facing +z), tile `(0, 0)` at the top-left,
/// x to the right and rows downward, `anchor` (a fraction of the map's size from its
/// bottom-left corner) at the entity's position. A tile value is the atlas frame plus 1, 0
/// is empty; [`TILE_FLIP_X`], [`TILE_FLIP_Y`] and [`TILE_FLIP_DIAGONAL`] mirror and rotate a
/// tile (the same encoding as Tiled's tile layers). Changing the map uploads it again; an
/// unchanged map costs no CPU time per frame beyond culling its chunks.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct TileMap {
    /// The tileset image.
    pub image: Option<Handle<Image>>,
    /// Lit maps: a normal map in the tileset's layout.
    pub normal_map: Option<Handle<Image>>,
    /// How the tileset is cut into tiles (usually [`SpriteAtlas::grid`]).
    pub atlas: SpriteAtlas,
    /// Columns and rows of tiles.
    pub size: [u32; 2],
    /// Width and height of one tile in meters.
    pub tile_size: [f32; 2],
    /// `size[0] × size[1]` tile values, row by row from the top (see the type docs).
    pub tiles: Vec<u32>,
    /// The point of the map at the entity's position, as a fraction of its size from the
    /// bottom-left (`[0, 0]`, the default: the map extends right and up from it).
    pub anchor: [f32; 2],
    /// Linear RGBA multiplied with every tile (distant layers: fade them toward the sky).
    pub color: [f32; 4],
    /// Draw order against sprites and other maps at the same depth (see [`Sprite::layer`]).
    pub layer: i32,
    pub filter: SpriteFilter,
    pub alpha: SpriteAlpha,
    /// Lit by the scene (see [`Sprite::lit`]).
    pub lit: bool,
    pub roughness: f32,
    pub translucency: f32,
    pub emissive: f32,
    /// Cast sun shadows (cut-out maps only).
    pub cast_shadows: bool,
    pub visible: bool,
}

impl Default for TileMap {
    fn default() -> Self {
        Self {
            image: None,
            normal_map: None,
            atlas: SpriteAtlas::Whole,
            size: [0, 0],
            tile_size: [1.0, 1.0],
            tiles: Vec::new(),
            anchor: [0.0, 0.0],
            color: [1.0; 4],
            layer: 0,
            filter: SpriteFilter::Linear,
            alpha: SpriteAlpha::Cutout(0.5),
            lit: false,
            roughness: 0.8,
            translucency: 0.0,
            emissive: 0.0,
            cast_shadows: true,
            visible: true,
        }
    }
}

impl TileMap {
    /// An empty map of `size` tiles of `tile_size` meters from `image` cut by `atlas`.
    pub fn new(
        image: Handle<Image>,
        atlas: SpriteAtlas,
        size: [u32; 2],
        tile_size: [f32; 2],
    ) -> Self {
        Self {
            image: Some(image),
            atlas,
            size,
            tile_size,
            tiles: vec![0; size[0] as usize * size[1] as usize],
            ..Default::default()
        }
    }

    /// The value at column `x`, row `y` (row 0 at the top); 0 outside the map.
    pub fn get(&self, x: u32, y: u32) -> u32 {
        if x >= self.size[0] || y >= self.size[1] {
            return 0;
        }
        self.tiles
            .get((y * self.size[0] + x) as usize)
            .copied()
            .unwrap_or(0)
    }

    /// Set the value at column `x`, row `y` (row 0 at the top): the atlas frame + 1, 0 for
    /// empty, plus flip flags. Ignored outside the map.
    pub fn set(&mut self, x: u32, y: u32, value: u32) {
        if x >= self.size[0] || y >= self.size[1] {
            return;
        }
        let len = self.size[0] as usize * self.size[1] as usize;
        if self.tiles.len() != len {
            self.tiles.resize(len, 0);
        }
        self.tiles[(y * self.size[0] + x) as usize] = value;
    }

    /// Width and height of the whole map in meters.
    pub fn extent(&self) -> [f32; 2] {
        [
            self.size[0] as f32 * self.tile_size[0],
            self.size[1] as f32 * self.tile_size[1],
        ]
    }

    pub fn with_layer(mut self, layer: i32) -> Self {
        self.layer = layer;
        self
    }

    pub fn with_filter(mut self, filter: SpriteFilter) -> Self {
        self.filter = filter;
        self
    }

    pub fn with_alpha(mut self, alpha: SpriteAlpha) -> Self {
        self.alpha = alpha;
        self
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn with_anchor(mut self, anchor: [f32; 2]) -> Self {
        self.anchor = anchor;
        self
    }

    /// Lit by the scene (sun, point lights, sky light, shadows).
    pub fn lit(mut self) -> Self {
        self.lit = true;
        self
    }
}

/// A background or foreground layer that follows the camera at a fraction of its movement
/// (on a [`Sprite`] or [`TileMap`] entity), for depth in 2D scenes.
///
/// The layer is drawn offset by `(1 − factor)` times the camera's world x and y, so it is at
/// its own position when the camera is at x = y = 0. Only drawing moves; the entity's
/// transform stays where it is.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parallax {
    /// How much of the camera's movement along world x and y the layer keeps: 1 moves with the
    /// world (no parallax), 0.5 half as fast (far hills), 0 stays on screen (the sky), above 1
    /// faster (foreground leaves).
    pub factor: [f32; 2],
    /// Draw copies of the layer every `repeat_x` meters along its x axis to fill the view, so
    /// it never ends (the layer's width for seamless art); 0 = one copy.
    pub repeat_x: f32,
}

impl Default for Parallax {
    fn default() -> Self {
        Self {
            factor: [1.0, 1.0],
            repeat_x: 0.0,
        }
    }
}

impl Parallax {
    /// A layer that keeps `factor` of the camera's movement on both axes.
    pub fn new(factor: f32) -> Self {
        Self {
            factor: [factor, factor],
            repeat_x: 0.0,
        }
    }

    /// Repeat every `width` meters along x.
    pub fn repeating(mut self, width: f32) -> Self {
        self.repeat_x = width;
        self
    }
}

/// What a [`PixelPerfect`] camera moves in whole steps of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelSnap {
    /// Whole art texels: the classic look, nothing ever shifts by less than one art pixel.
    #[default]
    Texel,
    /// Whole screen pixels: smoother scrolling at large scales; texels are still exact blocks.
    ScreenPixel,
}

/// Pixel-perfect rendering for an orthographic [`Camera`] (put it on the camera entity).
///
/// - The view height is set so that one art texel (`1 / pixels_per_unit` meters) covers a
///   whole number of screen pixels: `scale`, or the largest whole number at which the camera
///   still shows at least its `Projection::Orthographic` height.
/// - The camera moves in whole texels (or screen pixels, see [`PixelSnap`]) in its view plane,
///   and so do the corners of sprites and tile maps, so texels never straddle pixels and the
///   picture doesn't shimmer as things move.
/// - Temporal anti-aliasing is off while this camera renders (no sub-pixel jitter, no history
///   blend, no sharpening): pixel art has no edges between texel grid lines to smooth, and the
///   jitter would only blur them. The art is drawn at full resolution and scaled by whole
///   numbers, so lighting, shadows, normal maps and UI keep their full detail.
///
/// Draw art with sizes that are whole texels ([`Sprite::pixel_art`]) and the
/// [`SpriteFilter::PixelArt`] or `Nearest` filter: each texel then shows as an exact
/// `scale × scale` block of identical pixels. Perspective cameras ignore it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PixelPerfect {
    /// Art texels per meter (16 = a 16-pixel tile is one meter).
    pub pixels_per_unit: f32,
    /// Screen pixels per texel; 0 picks the largest that shows the camera's whole height.
    pub scale: u32,
    pub snap: PixelSnap,
}

impl Default for PixelPerfect {
    fn default() -> Self {
        Self {
            pixels_per_unit: 16.0,
            scale: 0,
            snap: PixelSnap::Texel,
        }
    }
}

impl PixelPerfect {
    /// `pixels_per_unit` art texels per meter, automatic scale, whole-texel snapping.
    pub fn new(pixels_per_unit: f32) -> Self {
        Self {
            pixels_per_unit,
            ..Default::default()
        }
    }

    pub fn with_scale(mut self, scale: u32) -> Self {
        self.scale = scale;
        self
    }

    pub fn with_snap(mut self, snap: PixelSnap) -> Self {
        self.snap = snap;
        self
    }

    /// The grid this camera renders on, for a viewport `viewport_height` pixels tall and an
    /// orthographic camera that asks for `height` meters.
    pub fn grid(&self, height: f32, viewport_height: u32) -> PixelGrid {
        let ppu = (self.pixels_per_unit as f64).max(1e-6);
        let rows = viewport_height.max(1) as f64;
        let scale = if self.scale > 0 {
            self.scale
        } else {
            // Whole texels shown at scale k: rows / k; the largest k that shows `height`.
            let fit = rows / (height.max(1e-6) as f64 * ppu);
            (fit.floor() as u32).max(1)
        };
        let pixel = 1.0 / (ppu * scale as f64);
        PixelGrid {
            scale,
            texel: 1.0 / ppu,
            pixel,
            height: (rows * pixel) as f32,
            step: match self.snap {
                PixelSnap::Texel => 1.0 / ppu,
                PixelSnap::ScreenPixel => pixel,
            },
        }
    }
}

/// What a [`PixelPerfect`] camera resolves to for a viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelGrid {
    /// Screen pixels per art texel.
    pub scale: u32,
    /// Size of an art texel, meters.
    pub texel: f64,
    /// Size of a screen pixel, meters.
    pub pixel: f64,
    /// The orthographic view height that gives this scale, meters.
    pub height: f32,
    /// What the camera and sprite corners snap to, meters.
    pub step: f64,
}

impl PixelGrid {
    /// `point` moved by less than a step within the plane of `right` and `up` (unit vectors,
    /// the camera's axes) to the nearest multiple of the step along each.
    pub fn snap_point(&self, point: DVec3, right: DVec3, up: DVec3) -> DVec3 {
        let snap = |v: f64| (v / self.step).round() * self.step - v;
        point + right * snap(point.dot(right)) + up * snap(point.dot(up))
    }

    /// The camera position to render from: `origin` moved within its view plane so that the
    /// screen's left and bottom edges (of a `width × height` pixel viewport) lie on the step
    /// grid, and with them every pixel edge on the pixel grid. A point on the step grid then
    /// projects onto a pixel corner.
    pub fn snap_camera(&self, origin: DVec3, rotation: Quat, width: u32, height: u32) -> DVec3 {
        let right = (rotation * Vec3::X).as_dvec3().normalize_or(DVec3::X);
        let up = (rotation * Vec3::Y).as_dvec3().normalize_or(DVec3::Y);
        let half_w = width.max(1) as f64 * 0.5 * self.pixel;
        let half_h = height.max(1) as f64 * 0.5 * self.pixel;
        let snap = |v: f64, half: f64| {
            let edge = v - half;
            (edge / self.step).round() * self.step - edge
        };
        origin + right * snap(origin.dot(right), half_w) + up * snap(origin.dot(up), half_h)
    }
}

/// The pixel grid of camera `entity` when it is an orthographic [`Camera`] with
/// [`PixelPerfect`], for a `height`-pixel-tall viewport.
pub(crate) fn camera_pixel_grid(world: &World, entity: Entity, height: u32) -> Option<PixelGrid> {
    let pixel_perfect = world.get::<PixelPerfect>(entity)?;
    let camera = world.get::<Camera>(entity)?;
    match camera.projection {
        Projection::Orthographic { height: h, .. } if camera.active => {
            Some(pixel_perfect.grid(h, height))
        }
        _ => None,
    }
}

/// Unit vector of `v` in f64 with a fallback.
pub(crate) fn unit_d(v: Vec3, fallback: DVec3) -> DVec3 {
    let d = v.as_dvec3();
    let len = d.length();
    if len > 1e-9 { d / len } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_atlas_numbers_cells_left_to_right_then_down() {
        let atlas = SpriteAtlas::grid(4, 2);
        assert_eq!(atlas.frame_count(), 8);
        assert_eq!(atlas.frame_rect(0, [64, 32]), Some([0, 0, 16, 16]));
        assert_eq!(atlas.frame_rect(5, [64, 32]), Some([16, 16, 16, 16]));
        // Frames past the end wrap.
        assert_eq!(atlas.frame_rect(9, [64, 32]), atlas.frame_rect(1, [64, 32]));
        let packed = SpriteAtlas::grid_cells(3, 3, [16, 16], 1, 2);
        assert_eq!(packed.frame_rect(4, [100, 100]), Some([19, 19, 16, 16]));
        let rects = SpriteAtlas::Rects(vec![[2, 3, 4, 5]]);
        assert_eq!(rects.frame_uv(0, [10, 10]), Some([0.2, 0.3, 0.6, 0.8]));
        assert_eq!(SpriteAtlas::Rects(Vec::new()).frame_rect(0, [4, 4]), None);
    }

    #[test]
    fn animation_modes_step_through_frames() {
        let looped = SpriteAnimation::new([10, 11, 12], 10.0);
        let at = |a: &SpriteAnimation, t: f32| a.frame_at(t, 1);
        assert_eq!(
            [0.0, 0.1, 0.2, 0.3, 0.45].map(|t| at(&looped, t)),
            [10, 11, 12, 10, 11]
        );
        let once = looped.clone().with_mode(SpriteAnimationMode::Once);
        assert_eq!([0.05, 0.25, 9.0].map(|t| at(&once, t)), [10, 12, 12]);
        let ping = looped.clone().with_mode(SpriteAnimationMode::PingPong);
        assert_eq!(
            [0.0, 0.1, 0.2, 0.3, 0.4, 0.5].map(|t| at(&ping, t)),
            [10, 11, 12, 11, 10, 11]
        );
        // Empty frame lists play the whole atlas.
        let all = SpriteAnimation::default();
        assert_eq!(all.frame_at(13.0 / 12.0 + 0.01, 4), 1);
    }

    #[test]
    fn animation_advances_and_wraps_its_clock() {
        let mut a = SpriteAnimation::new(0..4, 4.0);
        for _ in 0..1000 {
            a.advance(0.37, 4);
        }
        assert!(a.time >= 0.0 && a.time < 1.0, "{}", a.time);
        let mut once = SpriteAnimation::new(0..4, 4.0).with_mode(SpriteAnimationMode::Once);
        once.advance(10.0, 4);
        assert_eq!(once.frame_at(once.time, 4), 3);
        assert!(once.finished(4));
        let mut back = SpriteAnimation::new(0..4, 4.0).with_speed(-1.0);
        back.advance(0.3, 4);
        assert_eq!(back.frame_at(back.time, 4), 2);
    }

    #[test]
    fn pixel_grid_scales_by_whole_numbers() {
        let pp = PixelPerfect::new(16.0);
        // 180 rows, 2.8 m asked: 4 px per texel (2.8 m × 16 × 4 = 179.2 ≤ 180).
        let g = pp.grid(2.8, 180);
        assert_eq!(g.scale, 4);
        assert!((g.height - 180.0 / 64.0).abs() < 1e-6);
        assert_eq!(pp.with_scale(3).grid(2.8, 180).scale, 3);
        // Too tall to fit even at 1: scale 1.
        assert_eq!(pp.grid(100.0, 180).scale, 1);
    }

    #[test]
    fn snapped_camera_puts_texel_corners_on_pixel_corners() {
        let g = PixelPerfect::new(16.0).grid(2.8, 181);
        for (x, w) in [(0.37, 320u32), (-5.031, 321), (1234.5678, 64)] {
            let o = g.snap_camera(DVec3::new(x, x * 0.3, 7.0), Quat::IDENTITY, w, 181);
            // The screen's left edge lies on the texel grid.
            let left = o.x - w as f64 * 0.5 * g.pixel;
            let bottom = o.y - 181.0 * 0.5 * g.pixel;
            for e in [left, bottom] {
                let t = e / g.texel;
                assert!((t - t.round()).abs() < 1e-9, "{x} {w}: {t}");
            }
            assert!((o.x - x).abs() <= g.texel * 0.5 + 1e-12);
            assert_eq!(o.z, 7.0);
        }
        let p = g.snap_point(DVec3::new(0.51, -0.26, 3.0), DVec3::X, DVec3::Y);
        assert!((p.x * 16.0 - (p.x * 16.0).round()).abs() < 1e-9);
        assert!((p.y * 16.0 - (p.y * 16.0).round()).abs() < 1e-9);
    }

    #[test]
    fn tile_maps_index_rows_from_the_top() {
        let mut map = TileMap {
            size: [3, 2],
            tiles: vec![0; 6],
            ..Default::default()
        };
        map.set(2, 1, 7 | TILE_FLIP_X);
        assert_eq!(map.tiles[5], 7 | TILE_FLIP_X);
        assert_eq!(map.get(2, 1) & TILE_FRAME_MASK, 7);
        assert_eq!(map.get(5, 0), 0);
        assert_eq!(map.extent(), [3.0, 2.0]);
    }

    #[test]
    fn sprites_round_trip_through_json() {
        let sprite = Sprite::pixel_art(Handle::from_index(3), [32, 16], 16.0)
            .with_atlas(SpriteAtlas::grid(8, 4))
            .with_billboard(Billboard::Upright)
            .lit();
        let v = serde_json::to_value(&sprite).unwrap();
        assert_eq!(
            serde_json::from_value::<Sprite>(v.clone()).unwrap(),
            sprite,
            "{v}"
        );
        let partial: Sprite = serde_json::from_value(serde_json::json!({"size": [2, 3]})).unwrap();
        assert_eq!(partial.size, [2.0, 3.0]);
        assert_eq!(partial.anchor, [0.5, 0.5]);
    }
}

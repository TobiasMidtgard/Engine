//! Baked smoke flipbooks: simulated smoke rendered offline under six lights, drawn by the smoke,
//! dust and fireball sprites in place of their procedural noise.
//!
//! Each sheet is a grid of frames of one puff over its life, made by `scripts/vfx-bake.py` (a
//! small stable-fluids smoke simulation, Stam 1999, with vorticity confinement and billow detail)
//! and fetched from the asset mirror (`assets/manifests/vfx.toml`):
//!
//! * `<name>_a.png`: lit fraction under a light from the right, top and back; alpha.
//! * `<name>_b.png`: the same from the left, bottom and front; emission.
//! * `<name>_mv.png`: where each pixel's smoke moves by the next frame, for blending frames
//!   without ghosting (motion-vector flipbooks, as in Unreal's and Unity's smoke).
//! * `<name>.json`: grid, frame count and cell size.
//!
//! This is the six-way lighting of Unity's VFX Graph and of Battlefront II's effects: the sun
//! direction in the sprite's own frame weighs the six lightmaps, so a puff is lit from wherever
//! the sun is, with its real self-shadowing (engine-design/particles-flipbook-research.md).
//! When the files are missing (not fetched yet), the sprites stay procedural.

use bevy_ecs::prelude::*;
use engine_assets::{AssetServer, Assets, Handle};
use engine_render::Image;

/// Where the sheets live under the asset root.
pub const SHEET_DIR: &str = "fetched/vfx/flipbooks";

/// The sheets and the sprite each one draws: smoke (kind 0), dust (kind 1) and the burning
/// billow of fireballs and explosions (kind 2, whose emission channel carries its glow).
pub const SHEETS: [(&str, u32); KINDS] = [("smoke_puff", 2), ("dust_burst", 9), ("billow", 18)];

/// Number of sheet kinds.
pub const KINDS: usize = 3;

/// Maps per sheet: lightmaps A, lightmaps B, motion.
pub const MAPS: usize = 3;

const SUFFIX: [&str; MAPS] = ["_a.png", "_b.png", "_mv.png"];

/// Layout of one sheet, from its `.json` sidecar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetMeta {
    /// Frames across and down.
    pub grid: [u32; 2],
    /// Frames used (row by row from the top left).
    pub frames: u32,
}

impl Default for SheetMeta {
    fn default() -> Self {
        Self {
            grid: [8, 8],
            frames: 64,
        }
    }
}

impl SheetMeta {
    /// Read from a sidecar's JSON; missing or odd fields keep the defaults.
    pub fn parse(json: &str) -> Self {
        let mut meta = Self::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            return meta;
        };
        if let Some([x, y]) = v.get("grid").and_then(|g| {
            let g = g.as_array()?;
            Some([g.first()?.as_u64()?, g.get(1)?.as_u64()?])
        }) && (1..=64).contains(&x)
            && (1..=64).contains(&y)
        {
            meta.grid = [x as u32, y as u32];
        }
        let cells = meta.grid[0] * meta.grid[1];
        meta.frames = v
            .get("frames")
            .and_then(|f| f.as_u64())
            .map_or(cells, |f| (f as u32).clamp(1, cells));
        meta
    }
}

/// One sheet's maps (A, B, motion) found on disk, and its layout.
pub type Sheet = ([Option<Handle<Image>>; MAPS], SheetMeta);

/// The sheets' image handles, asked for once the renderer first prepares particles.
#[derive(Default)]
pub struct SheetHandles {
    asked: bool,
    /// Per kind: the sheet, when its lightmaps are on disk.
    pub kinds: [Option<Sheet>; KINDS],
}

impl SheetHandles {
    /// Ask the asset server for every sheet whose lightmaps are on disk (once). A missing sheet
    /// is logged once and its sprite stays procedural.
    pub fn request(&mut self, world: &mut World) {
        if self.asked {
            return;
        }
        self.asked = true;
        if !world.contains_resource::<AssetServer>() || !world.contains_resource::<Assets<Image>>()
        {
            return;
        }
        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            let server = world.resource::<AssetServer>();
            let dir = server.root().join(SHEET_DIR);
            for (kind, (name, _)) in SHEETS.iter().enumerate() {
                let file = |suffix: &str| dir.join(format!("{name}{suffix}"));
                if !file(SUFFIX[0]).is_file() || !file(SUFFIX[1]).is_file() {
                    tracing::info!(
                        "vfx: no baked {name} sheet in {} (run `cargo run -p fetch-assets -- vfx`); \
                         its sprite stays procedural",
                        dir.display()
                    );
                    continue;
                }
                let maps = std::array::from_fn(|m| {
                    file(SUFFIX[m])
                        .is_file()
                        .then(|| server.load(&mut images, &format!("{SHEET_DIR}/{name}{}", SUFFIX[m])))
                });
                let meta = std::fs::read_to_string(dir.join(format!("{name}.json")))
                    .map(|j| SheetMeta::parse(&j))
                    .unwrap_or_default();
                self.kinds[kind] = Some((maps, meta));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sidecar_gives_the_grid_and_frames_and_odd_ones_keep_defaults() {
        let m = SheetMeta::parse(r#"{"grid": [4, 2], "frames": 7, "fps": 24}"#);
        assert_eq!(
            m,
            SheetMeta {
                grid: [4, 2],
                frames: 7
            }
        );
        // More frames than cells are clamped; a broken file keeps the defaults.
        assert_eq!(
            SheetMeta::parse(r#"{"grid": [2, 2], "frames": 9}"#).frames,
            4
        );
        assert_eq!(SheetMeta::parse("not json"), SheetMeta::default());
        assert_eq!(
            SheetMeta::parse(r#"{"grid": [0, 3]}"#),
            SheetMeta::default()
        );
    }

    #[test]
    fn missing_sheets_leave_the_sprites_procedural() {
        let mut world = World::new();
        let dir = std::env::temp_dir().join(format!("vfx-sheets-{}", std::process::id()));
        world.insert_resource(AssetServer::new(&dir, false));
        world.insert_resource(Assets::<Image>::default());
        let mut sheets = SheetHandles::default();
        sheets.request(&mut world);
        assert!(sheets.kinds.iter().all(Option::is_none));
    }
}

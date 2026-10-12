//! [`IconLibrary`]: SVG icons and portraits, rasterized at the exact pixel size they are drawn
//! at (crisp at every UI scale).
//!
//! Built-in art lives in `crates/ui/assets/{icons,portraits}` and is embedded at build time.
//! Games and AI agents add more at runtime: `ui.add_icon {"name": "relic", "svg": "<svg ...>"}`.
//! Names starting with `ui_` are single-color glyphs tinted with the node's text color.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use resvg::{tiny_skia, usvg};

include!(concat!(env!("OUT_DIR"), "/builtin_svgs.rs"));

/// Parsed SVGs by name.
#[derive(Resource)]
pub struct IconLibrary {
    sources: HashMap<String, Arc<str>>,
    parsed: HashMap<String, Option<Arc<usvg::Tree>>>,
}

impl Default for IconLibrary {
    fn default() -> Self {
        let mut sources = HashMap::new();
        for (name, svg) in BUILTIN_SVGS {
            sources.insert(name.to_string(), Arc::<str>::from(*svg));
        }
        Self {
            sources,
            parsed: HashMap::new(),
        }
    }
}

impl IconLibrary {
    /// Add or replace an icon from SVG source. Fails if the SVG doesn't parse.
    pub fn add(&mut self, name: &str, svg: &str) -> Result<(), String> {
        usvg::Tree::from_str(svg, &usvg::Options::default())
            .map_err(|e| format!("The SVG for icon \"{name}\" doesn't parse: {e}."))?;
        self.sources.insert(name.to_string(), Arc::from(svg));
        self.parsed.remove(name);
        Ok(())
    }

    /// Whether an icon with this name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.sources.contains_key(name)
    }

    /// All icon names, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.sources.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Single-color glyphs (`ui_*`) take the node's text color.
    pub fn is_glyph(&self, name: &str) -> bool {
        name.starts_with("ui_")
    }

    fn tree(&mut self, name: &str) -> Option<Arc<usvg::Tree>> {
        if let Some(t) = self.parsed.get(name) {
            return t.clone();
        }
        let tree = self.sources.get(name).and_then(|src| {
            match usvg::Tree::from_str(src, &usvg::Options::default()) {
                Ok(t) => Some(Arc::new(t)),
                Err(e) => {
                    tracing::warn!("icon {name}: {e}");
                    None
                }
            }
        });
        self.parsed.insert(name.to_string(), tree.clone());
        tree
    }

    /// Aspect ratio (width / height) of an icon.
    pub fn aspect(&mut self, name: &str) -> Option<f32> {
        let t = self.tree(name)?;
        let s = t.size();
        Some(s.width() / s.height())
    }

    /// Rasterize `name` to fit `w`×`h` (aspect kept). Returns the bitmap size and straight-alpha
    /// sRGB RGBA8 pixels.
    pub fn rasterize(&mut self, name: &str, w: u32, h: u32, flip: bool) -> Option<(u32, u32, Vec<u8>)> {
        let tree = self.tree(name)?;
        let size = tree.size();
        let s = (w as f32 / size.width()).min(h as f32 / size.height());
        let (pw, ph) = (
            (size.width() * s).round().max(1.0) as u32,
            (size.height() * s).round().max(1.0) as u32,
        );
        let mut pixmap = tiny_skia::Pixmap::new(pw, ph)?;
        let mut transform = tiny_skia::Transform::from_scale(s, s);
        if flip {
            transform = tiny_skia::Transform::from_row(-s, 0.0, 0.0, s, pw as f32, 0.0);
        }
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let mut data = pixmap.take();
        for p in data.chunks_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Some((pw, ph, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#ff0000"/></svg>"##;

    #[test]
    fn runtime_icons_rasterize_at_requested_size() {
        let mut lib = IconLibrary::default();
        assert!(lib.add("bad", "<svg").is_err());
        lib.add("dot", DOT).unwrap();
        let (w, h, px) = lib.rasterize("dot", 40, 40, false).unwrap();
        assert_eq!((w, h), (40, 40));
        let center = ((20 * 40 + 20) * 4) as usize;
        assert_eq!(&px[center..center + 4], &[255, 0, 0, 255]);
        assert_eq!(px[3], 0, "corner is transparent");
        assert!(lib.rasterize("missing", 10, 10, false).is_none());
    }

    #[test]
    fn every_builtin_svg_parses() {
        let mut lib = IconLibrary::default();
        let names: Vec<String> = lib.names().iter().map(|s| s.to_string()).collect();
        for name in names {
            assert!(lib.rasterize(&name, 32, 32, false).is_some(), "{name} failed");
        }
    }
}

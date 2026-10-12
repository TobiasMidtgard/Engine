//! The Spell court's props. Each function returns a [`Prop`] in its own space (origin on the
//! ground at the prop's center, +Y up); `spawn_demo` places them.

use engine_core::math::Vec2;

use super::meshes::lathe;
use super::{Prop, Surface};

/// A round stone plinth, `radius` wide and `height` tall, with a molded rim.
pub fn plinth(radius: f32, height: f32) -> Prop {
    let r = radius;
    let h = height;
    let profile = [
        Vec2::new(0.0, 0.0),
        Vec2::new(r, 0.0),
        Vec2::new(r, h * 0.15),
        Vec2::new(r * 0.9, h * 0.25),
        Vec2::new(r * 0.85, h * 0.8),
        Vec2::new(r * 0.95, h * 0.9),
        Vec2::new(r * 0.95, h),
        Vec2::new(0.0, h),
    ];
    Prop::new().with(Surface::CarvedStone, lathe(&profile, 48))
}

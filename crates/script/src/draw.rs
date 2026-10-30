//! `draw`: debug shapes from scripts (Unreal's DrawDebug functions, Unity's `Gizmos` and
//! `Debug.DrawLine`), drawn by [`engine_debug`] over the scene in every view.
//!
//! ```lua
//! draw.line(a, b)                                   -- white, this frame
//! draw.sphere(target, 0.5, "orange")                -- a color by name, or a vec3
//! draw.arrow(from, to, { color = "cyan", seconds = 2, on_top = true })
//! draw.circle(vec3.zero, self.props.range, { normal = vec3.up })
//! draw.cone(vec3(0, 1.2, 0), vec3.forward * 8, math.rad(30), "orange")
//! draw.space(self.entity)                           -- what follows is in its own space
//! ```
//!
//! A shape lasts one frame unless `seconds` says longer (real seconds, so they still count while
//! the game is paused or slowed). Positions are world meters, or relative to the entity
//! `draw.space` names (its position, rotation and scale) until the hook returns. The gizmo
//! hooks (`draw_gizmos`, `draw_gizmos_selected`) start in their own entity's space.
//!
//! Scripts only record shapes here, so `draw` works in `parallel_update` too; the runtime hands
//! them to [`DebugDraw`] after the frame ([`flush`]), placing each in its space then.

use bevy_ecs::prelude::*;
use engine_core::prelude::{DAffine3, DVec3, GlobalTransform, Quat as GQuat, Vec3 as GVec3};
use engine_debug::{Color, DebugDraw, DebugShape, DepthMode, ShapeStyle};
use mlua::{FromLua, Lua, Table, Value};

use crate::access::shared;
use crate::entity::EntityRef;
use crate::math::{Quat, Vec3, kind};

/// Shapes a VM keeps for one frame at most (a script drawing in a loop stops there).
pub(crate) const MAX_SHAPES: usize = 20_000;
/// The longest `seconds` a shape may ask for.
const MAX_SECONDS: f32 = 600.0;
/// Segments of the arcs and cones built here.
const ARC_SEGMENTS: usize = 32;

/// A shape a script drew, waiting for [`flush`].
#[derive(Debug, Clone)]
pub(crate) struct Drawn {
    pub shape: DebugShape,
    pub style: ShapeStyle,
    /// The entity whose space its positions are in (`None`: the world).
    pub space: Option<Entity>,
    /// The script that drew it.
    pub owner: Option<Entity>,
}

/// What `draw` keeps between calls: the frame's shapes and the space they go in.
#[derive(Debug, Default)]
pub(crate) struct DrawState {
    pub shapes: Vec<Drawn>,
    /// `draw.space(e)`: (the script that set it, the entity). It holds while that script's code
    /// runs; another script's hook starts in the world.
    pub space: Option<(Option<Entity>, Entity)>,
    /// Shapes left out this frame because of [`MAX_SHAPES`].
    pub dropped: usize,
}

/// The color of a style value: a name, a `vec3` (linear RGB) or `{r, g, b, a}`.
fn color_of(lua: &Lua, value: &Value) -> mlua::Result<Option<Color>> {
    match value {
        Value::Nil => Ok(None),
        Value::String(s) => {
            let name = s.to_str()?;
            engine_debug::color::named(&name).map(Some).ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "draw: no color named \"{}\" (one of {}, or a vec3)",
                    &*name,
                    engine_debug::color::NAMES.join(", ")
                ))
            })
        }
        Value::Table(t) if kind(t).is_some() => {
            let v = Vec3::from_lua(value.clone(), lua)?.f32();
            Ok(Some([v.x, v.y, v.z, 1.0]))
        }
        Value::Table(t) => {
            let at = |k: usize, default: f32| -> mlua::Result<f32> {
                Ok(t.raw_get::<Option<f64>>(k)?.map_or(default, |v| v as f32))
            };
            Ok(Some([at(1, 1.0)?, at(2, 1.0)?, at(3, 1.0)?, at(4, 1.0)?]))
        }
        other => Err(mlua::Error::runtime(format!(
            "draw: a color is a name or a vec3, not a {}",
            other.type_name()
        ))),
    }
}

/// A shape's style argument read: a color alone, or `{ color, seconds, on_top, ... }` (the
/// shape's own options, like `normal`, stay in the table for the caller).
struct Style {
    style: ShapeStyle,
    table: Option<Table>,
    lua: Lua,
}

impl Style {
    fn read(lua: &Lua, value: Value) -> mlua::Result<Self> {
        let mut style = ShapeStyle {
            color: engine_debug::color::WHITE,
            ..Default::default()
        };
        match &value {
            Value::Table(t) if kind(t).is_none() && t.raw_len() == 0 => {
                if let Some(c) = color_of(lua, &t.raw_get::<Value>("color")?)? {
                    style.color = c;
                }
                if let Some(secs) = t.raw_get::<Option<f64>>("seconds")? {
                    style.duration = (secs as f32).clamp(0.0, MAX_SECONDS);
                }
                if t.raw_get::<Option<bool>>("on_top")?.unwrap_or(false) {
                    style.depth = DepthMode::OnTop;
                }
                if let Some(alpha) = t.raw_get::<Option<f64>>("alpha")? {
                    style.color[3] = (alpha as f32).clamp(0.0, 1.0);
                }
                Ok(Self {
                    style,
                    table: Some(t.clone()),
                    lua: lua.clone(),
                })
            }
            _ => {
                if let Some(c) = color_of(lua, &value)? {
                    style.color = c;
                }
                Ok(Self {
                    style,
                    table: None,
                    lua: lua.clone(),
                })
            }
        }
    }

    fn vec3(&self, key: &str) -> mlua::Result<Option<GVec3>> {
        match &self.table {
            Some(t) => match t.raw_get::<Value>(key)? {
                Value::Nil => Ok(None),
                v => Ok(Some(Vec3::from_lua(v, &self.lua)?.f32())),
            },
            None => Ok(None),
        }
    }

    fn quat(&self, key: &str) -> mlua::Result<Option<GQuat>> {
        match &self.table {
            Some(t) => match t.raw_get::<Value>(key)? {
                Value::Nil => Ok(None),
                v => Ok(Some(Quat::from_lua(v, &self.lua)?.0)),
            },
            None => Ok(None),
        }
    }

    fn flag(&self, key: &str) -> mlua::Result<bool> {
        match &self.table {
            Some(t) => Ok(t.raw_get::<Option<bool>>(key)?.unwrap_or(false)),
            None => Ok(false),
        }
    }
}

/// Records a shape for the script running now (in the space it set, if it still holds).
fn record(lua: &Lua, shape: DebugShape, style: ShapeStyle) {
    let mut s = shared(lua);
    let owner = s.owner;
    let space = match s.draw.space {
        Some((by, e)) if by == owner => Some(e),
        _ => None,
    };
    if s.draw.shapes.len() >= MAX_SHAPES {
        s.draw.dropped += 1;
        return;
    }
    s.draw.shapes.push(Drawn {
        shape,
        style,
        space,
        owner,
    });
}

/// Points of an arc around `center` in the plane across `normal`, from direction `from`,
/// turning `angle` radians (counterclockwise seen from the side `normal` points to).
fn arc_points(center: DVec3, normal: GVec3, from: GVec3, radius: f64, angle: f64) -> Vec<DVec3> {
    let n = normal.as_dvec3().try_normalize().unwrap_or(DVec3::Y);
    let start = from.as_dvec3() - n * from.as_dvec3().dot(n);
    let u = start
        .try_normalize()
        .unwrap_or_else(|| n.any_orthonormal_vector());
    let v = n.cross(u);
    let steps = ((ARC_SEGMENTS as f64 * angle.abs() / std::f64::consts::TAU).ceil() as usize)
        .clamp(2, ARC_SEGMENTS);
    (0..=steps)
        .map(|i| {
            let t = angle * i as f64 / steps as f64;
            center + (u * t.cos() + v * t.sin()) * radius
        })
        .collect()
}

/// A cone from `apex` along `dir` (its length the cone's), `half` radians wide: the rim and
/// four lines out to it.
fn cone_shapes(apex: DVec3, dir: GVec3, half: f64) -> Vec<DebugShape> {
    let length = dir.length() as f64;
    let axis = dir.as_dvec3().try_normalize().unwrap_or(DVec3::NEG_Z);
    let radius = length * half.tan().abs();
    let base = apex + axis * length;
    let (u, v) = axis.any_orthonormal_pair();
    let mut shapes: Vec<DebugShape> = [u, v, -u, -v]
        .into_iter()
        .map(|side| DebugShape::Line {
            a: apex,
            b: base + side * radius,
        })
        .collect();
    shapes.push(DebugShape::Circle {
        center: base,
        normal: axis.as_vec3(),
        radius: radius as f32,
    });
    shapes
}

/// Installs the `draw` table.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let t = lua.create_table()?;
    t.set(
        "line",
        lua.create_function(|lua, (a, b, style): (Vec3, Vec3, Value)| {
            let s = Style::read(lua, style)?;
            record(lua, DebugShape::Line { a: a.0, b: b.0 }, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "ray",
        lua.create_function(|lua, (origin, dir, style): (Vec3, Vec3, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Ray {
                origin: origin.0,
                dir: dir.f32(),
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "arrow",
        lua.create_function(|lua, (from, to, style): (Vec3, Vec3, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Arrow {
                from: from.0,
                to: to.0,
                head: 0.0,
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "sphere",
        lua.create_function(|lua, (center, radius, style): (Vec3, f64, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Sphere {
                center: center.0,
                radius: radius as f32,
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "box",
        lua.create_function(|lua, (center, size, style): (Vec3, Vec3, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::OrientedBox {
                center: center.0,
                half_extents: size.f32() * 0.5,
                rotation: s.quat("rotation")?.unwrap_or(GQuat::IDENTITY),
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "circle",
        lua.create_function(|lua, (center, radius, style): (Vec3, f64, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Circle {
                center: center.0,
                normal: s.vec3("normal")?.unwrap_or(GVec3::Y),
                radius: radius as f32,
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "arc",
        lua.create_function(
            |lua, (center, radius, from, angle, style): (Vec3, f64, Vec3, f64, Value)| {
                let s = Style::read(lua, style)?;
                let normal = s.vec3("normal")?.unwrap_or(GVec3::Y);
                let points = arc_points(center.0, normal, from.f32(), radius, angle);
                record(
                    lua,
                    DebugShape::Path {
                        points,
                        closed: false,
                    },
                    s.style,
                );
                Ok(())
            },
        )?,
    )?;
    t.set(
        "cone",
        lua.create_function(|lua, (apex, dir, half, style): (Vec3, Vec3, f64, Value)| {
            let s = Style::read(lua, style)?;
            for shape in cone_shapes(apex.0, dir.f32(), half) {
                record(lua, shape, s.style);
            }
            Ok(())
        })?,
    )?;
    t.set(
        "point",
        lua.create_function(|lua, (at, style): (Vec3, Value)| {
            let s = Style::read(lua, style)?;
            let size = match &s.table {
                Some(t) => t.raw_get::<Option<f64>>("size")?.map(|v| v as f32),
                None => None,
            };
            let shape = DebugShape::Cross {
                at: at.0,
                size: size.unwrap_or(engine_debug::POINT_SIZE),
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "path",
        lua.create_function(|lua, (points, style): (Vec<Vec3>, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Path {
                points: points.into_iter().map(|p| p.0).collect(),
                closed: s.flag("closed")?,
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "capsule",
        lua.create_function(|lua, (a, b, radius, style): (Vec3, Vec3, f64, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Capsule {
                a: a.0,
                b: b.0,
                radius: radius as f32,
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "cylinder",
        lua.create_function(|lua, (a, b, radius, style): (Vec3, Vec3, f64, Value)| {
            let s = Style::read(lua, style)?;
            let shape = DebugShape::Cylinder {
                a: a.0,
                b: b.0,
                radius: radius as f32,
            };
            record(lua, shape, s.style);
            Ok(())
        })?,
    )?;
    t.set(
        "axes",
        lua.create_function(
            |lua, (at, rotation, size): (Vec3, Option<Quat>, Option<f64>)| {
                let shape = DebugShape::Axes {
                    position: at.0,
                    rotation: rotation.map_or(GQuat::IDENTITY, |q| q.0),
                    size: size.unwrap_or(1.0) as f32,
                };
                record(lua, shape, ShapeStyle::default());
                Ok(())
            },
        )?,
    )?;
    t.set(
        "space",
        lua.create_function(|lua, entity: Option<EntityRef>| {
            let mut s = shared(lua);
            s.draw.space = entity.map(|e| (s.owner, e.0));
            Ok(())
        })?,
    )?;
    t.set_readonly(true);
    lua.globals().set("draw", t)
}

/// Starts a gizmo hook of `e`: its shapes go in `e`'s space.
pub(crate) fn begin_gizmos(lua: &Lua, e: Entity) {
    shared(lua).draw.space = Some((Some(e), e));
}

/// A gizmo hook returned: back to the world.
pub(crate) fn end_gizmos(lua: &Lua) {
    shared(lua).draw.space = None;
}

/// `shape` moved from an entity's space into the world by `to` (its global transform).
pub(crate) fn placed(shape: &DebugShape, to: &DAffine3) -> DebugShape {
    let point = |p: DVec3| to.transform_point3(p);
    let dir = |v: GVec3| to.transform_vector3(v.as_dvec3()).as_vec3();
    let (scale, rotation, _) = to.to_scale_rotation_translation();
    let rotation = rotation.as_quat();
    // Radii grow with the largest scale (a sphere stays round).
    let r = |radius: f32| radius * scale.abs().max_element() as f32;
    match shape {
        DebugShape::Line { a, b } => DebugShape::Line {
            a: point(*a),
            b: point(*b),
        },
        DebugShape::Ray { origin, dir: d } => DebugShape::Ray {
            origin: point(*origin),
            dir: dir(*d),
        },
        DebugShape::Arrow {
            from,
            to: end,
            head,
        } => DebugShape::Arrow {
            from: point(*from),
            to: point(*end),
            head: *head,
        },
        DebugShape::Aabb { min, max } => DebugShape::OrientedBox {
            center: point((*min + *max) * 0.5),
            half_extents: ((*max - *min) * 0.5 * scale).as_vec3(),
            rotation,
        },
        DebugShape::OrientedBox {
            center,
            half_extents,
            rotation: q,
        } => DebugShape::OrientedBox {
            center: point(*center),
            half_extents: (half_extents.as_dvec3() * scale).as_vec3(),
            rotation: rotation * *q,
        },
        DebugShape::Sphere { center, radius } => DebugShape::Sphere {
            center: point(*center),
            radius: r(*radius),
        },
        DebugShape::Circle {
            center,
            normal,
            radius,
        } => DebugShape::Circle {
            center: point(*center),
            normal: rotation * *normal,
            radius: r(*radius),
        },
        DebugShape::Cross { at, size } => DebugShape::Cross {
            at: point(*at),
            size: *size,
        },
        DebugShape::Axes {
            position,
            rotation: q,
            size,
        } => DebugShape::Axes {
            position: point(*position),
            rotation: rotation * *q,
            size: r(*size),
        },
        DebugShape::Capsule { a, b, radius } => DebugShape::Capsule {
            a: point(*a),
            b: point(*b),
            radius: r(*radius),
        },
        DebugShape::Cylinder { a, b, radius } => DebugShape::Cylinder {
            a: point(*a),
            b: point(*b),
            radius: r(*radius),
        },
        DebugShape::Grid {
            center,
            normal,
            cell,
            cells,
        } => DebugShape::Grid {
            center: point(*center),
            normal: rotation * *normal,
            cell: r(*cell),
            cells: *cells,
        },
        DebugShape::Frustum { .. } => shape.clone(),
        DebugShape::Path { points, closed } => DebugShape::Path {
            points: points.iter().map(|p| point(*p)).collect(),
            closed: *closed,
        },
    }
}

/// Hands the shapes `lua`'s scripts drew to [`DebugDraw`], each placed in its space (a shape
/// whose space entity is gone is dropped). Without a `DebugDraw` they are dropped. While the
/// rewind recorder records ([`crate::rewind`]), returns them placed, with the script that drew
/// each.
pub(crate) fn flush(world: &mut World, lua: &Lua) -> Vec<(Entity, DebugShape, ShapeStyle)> {
    let (shapes, keep) = {
        let mut s = shared(lua);
        s.draw.space = None;
        s.draw.dropped = 0;
        (std::mem::take(&mut s.draw.shapes), s.trace.is_some())
    };
    if shapes.is_empty() {
        return Vec::new();
    }
    let placed_shapes: Vec<(Option<Entity>, DebugShape, ShapeStyle)> = shapes
        .into_iter()
        .filter_map(|d| match d.space {
            None => Some((d.owner, d.shape, d.style)),
            Some(e) => world
                .get::<GlobalTransform>(e)
                .map(|g| (d.owner, placed(&d.shape, &g.0), d.style)),
        })
        .collect();
    if let Some(mut debug) = world.get_resource_mut::<DebugDraw>() {
        for (_, shape, style) in &placed_shapes {
            debug.styled(shape, *style);
        }
    }
    // The rewind recorder keeps them, by the script that drew them.
    if !keep {
        return Vec::new();
    }
    placed_shapes
        .into_iter()
        .filter_map(|(owner, shape, style)| owner.map(|e| (e, shape, style)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_in_an_entitys_space_land_where_it_is() {
        let to = DAffine3::from_scale_rotation_translation(
            DVec3::splat(2.0),
            engine_core::prelude::DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
            DVec3::new(10.0, 0.0, 0.0),
        );
        let DebugShape::Sphere { center, radius } = placed(
            &DebugShape::Sphere {
                center: DVec3::new(1.0, 0.0, 0.0),
                radius: 0.5,
            },
            &to,
        ) else {
            panic!("a sphere stays a sphere");
        };
        // +x turned a quarter around y is -z, twice as far.
        assert!(
            (center - DVec3::new(10.0, 0.0, -2.0)).length() < 1e-9,
            "{center}"
        );
        assert_eq!(radius, 1.0);
    }

    #[test]
    fn arcs_turn_counterclockwise_seen_from_the_normal() {
        let points = arc_points(
            DVec3::ZERO,
            GVec3::Y,
            GVec3::X,
            2.0,
            std::f64::consts::FRAC_PI_2,
        );
        let (first, last) = (points[0], *points.last().expect("points"));
        assert!((first - DVec3::new(2.0, 0.0, 0.0)).length() < 1e-9);
        // Seen from above (+y), x turns toward -z.
        assert!(
            (last - DVec3::new(0.0, 0.0, -2.0)).length() < 1e-9,
            "{last}"
        );
    }
}

//! `vec3` and `quat` for scripts.
//!
//! World positions in this engine are `f64` (planets and orbits), and Luau's built-in `vector`
//! is `f32`, so scripts get their own `vec3` with `f64` components. Both types are immutable
//! values: operations return new ones, so `a = b; a = a + c` never changes `b`.
//!
//! ```lua
//! local v = vec3(1, 2, 3) + vec3.up * 2      -- operators: + - * / unary -, ==
//! print(v.x, v:length(), v:unit(), v:dot(vec3.right))
//! local q = quat.axis_angle(vec3.up, math.pi / 2)
//! local turned = q * vec3.forward             -- rotate a vector
//! ```
//!
//! Functions that take a `vec3` also take a table `{x, y, z}` or `{x = .., y = .., z = ..}`.

use engine_core::math::{DVec3, EulerRot, Quat as GQuat, Vec3 as GVec3};
use mlua::{
    FromLua, Lua, MetaMethod, Table, UserData, UserDataFields, UserDataMethods, Value, Variadic,
};

/// A 3D vector with `f64` components (meters for positions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3(pub DVec3);

/// A rotation (unit quaternion, `f32` like the engine's `Transform::rotation`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat(pub GQuat);

impl Vec3 {
    pub fn f32(&self) -> GVec3 {
        self.0.as_vec3()
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

impl FromLua for Vec3 {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        match &value {
            Value::UserData(ud) => {
                if let Ok(v) = ud.borrow::<Vec3>() {
                    return Ok(*v);
                }
            }
            Value::Table(t) => {
                let get = |k1: &str, k2: usize| -> mlua::Result<f64> {
                    let v: Value = t.raw_get(k1)?;
                    let v = if v.is_nil() { t.raw_get(k2)? } else { v };
                    Ok(number(&v).unwrap_or(0.0))
                };
                return Ok(Vec3(DVec3::new(get("x", 1)?, get("y", 2)?, get("z", 3)?)));
            }
            Value::Vector(v) => {
                return Ok(Vec3(DVec3::new(v.x() as f64, v.y() as f64, v.z() as f64)));
            }
            _ => {}
        }
        Err(mlua::Error::runtime(format!(
            "expected a vec3 (or a table {{x, y, z}}), got {}",
            value.type_name()
        )))
    }
}

impl FromLua for Quat {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        match &value {
            Value::UserData(ud) => {
                if let Ok(q) = ud.borrow::<Quat>() {
                    return Ok(*q);
                }
            }
            Value::Table(t) => {
                let get = |i: usize| -> mlua::Result<f32> {
                    Ok(number(&t.raw_get(i)?).unwrap_or(0.0) as f32)
                };
                return Ok(Quat(
                    GQuat::from_xyzw(get(1)?, get(2)?, get(3)?, get(4)?).normalize(),
                ));
            }
            _ => {}
        }
        Err(mlua::Error::runtime(format!(
            "expected a quat, got {}",
            value.type_name()
        )))
    }
}

impl UserData for Vec3 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("x", |_, v| Ok(v.0.x));
        fields.add_field_method_get("y", |_, v| Ok(v.0.y));
        fields.add_field_method_get("z", |_, v| Ok(v.0.z));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("length", |_, v, ()| Ok(v.0.length()));
        methods.add_method("length_squared", |_, v, ()| Ok(v.0.length_squared()));
        methods.add_method("unit", |_, v, ()| Ok(Vec3(v.0.normalize_or_zero())));
        methods.add_method("dot", |_, v, b: Vec3| Ok(v.0.dot(b.0)));
        methods.add_method("cross", |_, v, b: Vec3| Ok(Vec3(v.0.cross(b.0))));
        methods.add_method("distance", |_, v, b: Vec3| Ok(v.0.distance(b.0)));
        methods.add_method("lerp", |_, v, (b, t): (Vec3, f64)| {
            Ok(Vec3(v.0.lerp(b.0, t)))
        });
        methods.add_method("abs", |_, v, ()| Ok(Vec3(v.0.abs())));
        methods.add_method("min", |_, v, b: Vec3| Ok(Vec3(v.0.min(b.0))));
        methods.add_method("max", |_, v, b: Vec3| Ok(Vec3(v.0.max(b.0))));
        methods.add_method("flat", |_, v, ()| Ok(Vec3(DVec3::new(v.0.x, 0.0, v.0.z))));
        methods.add_method("with_x", |_, v, x: f64| {
            Ok(Vec3(DVec3::new(x, v.0.y, v.0.z)))
        });
        methods.add_method("with_y", |_, v, y: f64| {
            Ok(Vec3(DVec3::new(v.0.x, y, v.0.z)))
        });
        methods.add_method("with_z", |_, v, z: f64| {
            Ok(Vec3(DVec3::new(v.0.x, v.0.y, z)))
        });
        methods.add_method("rotate_y", |_, v, angle: f64| {
            let (s, c) = angle.sin_cos();
            Ok(Vec3(DVec3::new(
                v.0.x * c + v.0.z * s,
                v.0.y,
                -v.0.x * s + v.0.z * c,
            )))
        });
        methods.add_method("unpack", |_, v, ()| Ok((v.0.x, v.0.y, v.0.z)));

        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (Vec3, Vec3)| {
            Ok(Vec3(a.0 + b.0))
        });
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (Vec3, Vec3)| {
            Ok(Vec3(a.0 - b.0))
        });
        methods.add_meta_function(MetaMethod::Mul, |lua, (a, b): (Value, Value)| {
            match (number(&a), number(&b)) {
                (Some(s), _) => Ok(Vec3(Vec3::from_lua(b, lua)?.0 * s)),
                (_, Some(s)) => Ok(Vec3(Vec3::from_lua(a, lua)?.0 * s)),
                _ => Ok(Vec3(Vec3::from_lua(a, lua)?.0 * Vec3::from_lua(b, lua)?.0)),
            }
        });
        methods.add_meta_function(
            MetaMethod::Div,
            |lua, (a, b): (Value, Value)| match number(&b) {
                Some(s) => Ok(Vec3(Vec3::from_lua(a, lua)?.0 / s)),
                None => Ok(Vec3(Vec3::from_lua(a, lua)?.0 / Vec3::from_lua(b, lua)?.0)),
            },
        );
        methods.add_meta_method(MetaMethod::Unm, |_, v, ()| Ok(Vec3(-v.0)));
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Vec3, Vec3)| Ok(a.0 == b.0));
        methods.add_meta_method(MetaMethod::ToString, |_, v, ()| {
            Ok(format!("vec3({}, {}, {})", v.0.x, v.0.y, v.0.z))
        });
    }
}

impl UserData for Quat {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("x", |_, q| Ok(q.0.x));
        fields.add_field_method_get("y", |_, q| Ok(q.0.y));
        fields.add_field_method_get("z", |_, q| Ok(q.0.z));
        fields.add_field_method_get("w", |_, q| Ok(q.0.w));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("inverse", |_, q, ()| Ok(Quat(q.0.inverse())));
        methods.add_method("slerp", |_, q, (b, t): (Quat, f32)| {
            Ok(Quat(q.0.slerp(b.0, t)))
        });
        methods.add_method("rotate", |_, q, v: Vec3| {
            Ok(Vec3((q.0 * v.f32()).as_dvec3()))
        });
        methods.add_method("euler", |_, q, ()| Ok(q.0.to_euler(EulerRot::YXZ)));
        methods.add_method("angle_to", |_, q, b: Quat| Ok(q.0.angle_between(b.0)));
        methods.add_meta_function(MetaMethod::Mul, |lua, (a, b): (Quat, Value)| match &b {
            Value::UserData(ud) if ud.is::<Quat>() => Ok(Value::UserData(
                lua.create_userdata(Quat(a.0 * ud.borrow::<Quat>()?.0))?,
            )),
            _ => {
                let v = Vec3::from_lua(b, lua)?;
                Ok(Value::UserData(
                    lua.create_userdata(Vec3((a.0 * v.f32()).as_dvec3()))?,
                ))
            }
        });
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Quat, Quat)| Ok(a.0 == b.0));
        methods.add_meta_method(MetaMethod::ToString, |_, q, ()| {
            Ok(format!("quat({}, {}, {}, {})", q.0.x, q.0.y, q.0.z, q.0.w))
        });
    }
}

/// A [`engine_world::WorldShift`] as scripts see it in `on_world_shift(shift)`: where kept
/// positions, directions and rotations go in the new frame.
#[derive(Clone, Copy)]
pub struct Shift(pub engine_world::WorldShift);

impl UserData for Shift {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("translation", |_, s| Ok(Vec3(s.0.translation)));
        fields.add_field_method_get("rotation", |_, s| Ok(Quat(s.0.rotation.as_quat())));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("point", |_, s, p: Vec3| Ok(Vec3(s.0.point(p.0))));
        methods.add_method("direction", |_, s, v: Vec3| {
            Ok(Vec3(s.0.direction(v.f32()).as_dvec3()))
        });
        methods.add_method("orientation", |_, s, q: Quat| {
            Ok(Quat(s.0.orientation(q.0)))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, s, ()| {
            Ok(format!("world shift by {}", s.0.translation))
        });
    }
}

/// Adds the `vec3` and `quat` globals (callable tables with constants and constructors).
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    let vec3 = lua.create_table()?;
    let constants = [
        ("zero", DVec3::ZERO),
        ("one", DVec3::ONE),
        ("up", DVec3::Y),
        ("down", DVec3::NEG_Y),
        ("right", DVec3::X),
        ("left", DVec3::NEG_X),
        // Cameras and lights look along -Z (AGENTS.md, "Units and coordinates").
        ("forward", DVec3::NEG_Z),
        ("back", DVec3::Z),
    ];
    for (name, v) in constants {
        vec3.set(name, Vec3(v))?;
    }
    vec3.set(
        "lerp",
        lua.create_function(|_, (a, b, t): (Vec3, Vec3, f64)| Ok(Vec3(a.0.lerp(b.0, t))))?,
    )?;
    vec3.set(
        "from_angle",
        lua.create_function(|_, (angle, radius): (f64, Option<f64>)| {
            let r = radius.unwrap_or(1.0);
            Ok(Vec3(DVec3::new(angle.cos() * r, 0.0, angle.sin() * r)))
        })?,
    )?;
    let vec3_meta = lua.create_table()?;
    vec3_meta.set(
        "__call",
        lua.create_function(|lua, (_, args): (Table, Variadic<Value>)| {
            if args.len() == 1 && !matches!(args[0], Value::Integer(_) | Value::Number(_)) {
                return Vec3::from_lua(args[0].clone(), lua);
            }
            let f = |i: usize| args.get(i).and_then(number).unwrap_or(0.0);
            Ok(Vec3(DVec3::new(f(0), f(1), f(2))))
        })?,
    )?;
    vec3.set_metatable(Some(vec3_meta))?;
    g.set("vec3", vec3)?;

    let quat = lua.create_table()?;
    quat.set("identity", Quat(GQuat::IDENTITY))?;
    quat.set(
        "axis_angle",
        lua.create_function(|_, (axis, angle): (Vec3, f32)| {
            Ok(Quat(GQuat::from_axis_angle(
                axis.f32().normalize_or(GVec3::Y),
                angle,
            )))
        })?,
    )?;
    quat.set(
        "euler",
        lua.create_function(|_, (yaw, pitch, roll): (f32, Option<f32>, Option<f32>)| {
            Ok(Quat(GQuat::from_euler(
                EulerRot::YXZ,
                yaw,
                pitch.unwrap_or(0.0),
                roll.unwrap_or(0.0),
            )))
        })?,
    )?;
    quat.set(
        "look",
        lua.create_function(|_, (dir, up): (Vec3, Option<Vec3>)| {
            let up = up.map(|u| u.f32()).unwrap_or(GVec3::Y);
            let f = dir.f32().normalize_or(GVec3::NEG_Z);
            Ok(Quat(look_rotation(f, up)))
        })?,
    )?;
    quat.set(
        "between",
        lua.create_function(|_, (a, b): (Vec3, Vec3)| {
            Ok(Quat(GQuat::from_rotation_arc(
                a.f32().normalize_or(GVec3::Y),
                b.f32().normalize_or(GVec3::Y),
            )))
        })?,
    )?;
    let quat_meta = lua.create_table()?;
    quat_meta.set(
        "__call",
        lua.create_function(|_, (_, x, y, z, w): (Table, f32, f32, f32, f32)| {
            Ok(Quat(GQuat::from_xyzw(x, y, z, w).normalize()))
        })?,
    )?;
    quat.set_metatable(Some(quat_meta))?;
    g.set("quat", quat)?;
    Ok(())
}

/// The rotation that turns -Z to `forward` with Y as close to `up` as it can.
pub fn look_rotation(forward: GVec3, up: GVec3) -> GQuat {
    let back = -forward;
    let right = up.cross(back).try_normalize().unwrap_or(GVec3::X);
    let up = back.cross(right);
    GQuat::from_mat3(&engine_core::math::Mat3::from_cols(right, up, back))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        install(&lua).unwrap();
        lua
    }

    #[test]
    fn vectors_do_arithmetic_in_f64() {
        let lua = lua();
        let (x, y, z): (f64, f64, f64) = lua
            .load("local v = vec3(1, 2, 3) + vec3.up * 2 - vec3(0.5, 0, 0); return (2 * v / 2):unpack()")
            .eval()
            .unwrap();
        assert_eq!((x, y, z), (0.5, 4.0, 3.0));
        let far: f64 = lua
            .load("return (vec3(1e7, 0, 0) + vec3(0.001, 0, 0)).x - 1e7")
            .eval()
            .unwrap();
        // A millimeter 10 000 km out (f32 would lose it entirely).
        assert!(
            (far - 0.001).abs() < 1e-8,
            "f64 precision far from the origin: {far}"
        );
        let ok: bool = lua
            .load("return vec3({1, 2, 3}) == vec3(1, 2, 3) and vec3{x = 1}.x == 1")
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn quaternions_rotate_vectors() {
        let lua = lua();
        let (x, _, z): (f64, f64, f64) = lua
            .load("return (quat.axis_angle(vec3.up, math.pi / 2) * vec3(1, 0, 0)):unpack()")
            .eval()
            .unwrap();
        assert!(x.abs() < 1e-6 && (z + 1.0).abs() < 1e-6, "{x} {z}");
        let fwd: f64 = lua
            .load("return (quat.look(vec3(1, 0, 0)) * vec3.forward).x")
            .eval()
            .unwrap();
        assert!((fwd - 1.0).abs() < 1e-5);
    }
}

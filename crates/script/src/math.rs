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
//!
//! Both are written in Luau (`math.luau`) as frozen tables with a shared metatable, the way
//! Luau's performance guide shapes objects: fields in the table, methods behind a table
//! `__index`. Reading `v.x` is then a plain table read the VM caches, and `a + b` runs a Luau
//! function, where Rust userdata paid a trip through mlua for every field and operator.
//! Measured in release (`math_speed`, ns each, userdata then tables): `p.x` 197 / 17,
//! `p + q` 787 / 367, `vec3(x, y, z)` 695 / 257, `q * v` 789 / 572. What is left is mostly making
//! the new table; Luau's own `vector` (f32, no allocation) does `p + q` in 11. Rust makes and
//! reads the same tables ([`Vec3`] and [`Quat`] convert both ways).

use std::ffi::c_int;

use engine_core::math::{DVec3, Quat as GQuat, Vec3 as GVec3};
use mlua::{
    FromLua, Function, IntoLua, Lua, MetaMethod, Table, UserData, UserDataFields, UserDataMethods,
    Value, ffi,
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

/// The metatables of a VM's `vec3` and `quat` values (from `math.luau`).
struct Types {
    vec3: Table,
    quat: Table,
}

/// Which of the two a table is, if either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Vec3,
    Quat,
}

/// Whether `t` is a `vec3` or `quat` value (by its metatable's `__name`).
pub(crate) fn kind(t: &Table) -> Option<Kind> {
    let name: Option<mlua::LuaString> = t.metatable()?.raw_get("__name").ok()?;
    match name?.as_bytes().as_ref() {
        b"vec3" => Some(Kind::Vec3),
        b"quat" => Some(Kind::Quat),
        _ => None,
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

/// A frozen table with `fields` and the metatable `which` picks.
fn make(lua: &Lua, fields: &[(&str, f64)], which: fn(&Types) -> &Table) -> mlua::Result<Value> {
    let types = lua
        .app_data_ref::<Types>()
        .ok_or_else(|| mlua::Error::runtime("vec3 and quat aren't installed in this VM"))?;
    let t = lua.create_table_with_capacity(0, fields.len())?;
    for (k, v) in fields {
        t.raw_set(*k, *v)?;
    }
    t.set_metatable(Some(which(&types).clone()))?;
    t.set_readonly(true);
    Ok(Value::Table(t))
}

impl IntoLua for Vec3 {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        let v = self.0;
        make(lua, &[("x", v.x), ("y", v.y), ("z", v.z)], |t| &t.vec3)
    }
}

impl IntoLua for Quat {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        let q = self.0;
        let f = |c: f32| c as f64;
        make(
            lua,
            &[("x", f(q.x)), ("y", f(q.y)), ("z", f(q.z)), ("w", f(q.w))],
            |t| &t.quat,
        )
    }
}

impl FromLua for Vec3 {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        match &value {
            Value::Table(t) if kind(t) != Some(Kind::Quat) => {
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
            type_name(&value)
        )))
    }
}

impl FromLua for Quat {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        if let Value::Table(t) = &value {
            match kind(t) {
                Some(Kind::Quat) => {
                    let get = |k: &str| -> mlua::Result<f32> {
                        Ok(number(&t.raw_get(k)?).unwrap_or(0.0) as f32)
                    };
                    return Ok(Quat(GQuat::from_xyzw(
                        get("x")?,
                        get("y")?,
                        get("z")?,
                        get("w")?,
                    )));
                }
                None => {
                    let get = |k1: &str, k2: usize| -> mlua::Result<f32> {
                        let v: Value = t.raw_get(k1)?;
                        let v = if v.is_nil() { t.raw_get(k2)? } else { v };
                        Ok(number(&v).unwrap_or(0.0) as f32)
                    };
                    let q =
                        GQuat::from_xyzw(get("x", 1)?, get("y", 2)?, get("z", 3)?, get("w", 4)?);
                    let ok = q.length_squared() > 0.0 && q.is_finite();
                    return Ok(Quat(if ok { q.normalize() } else { GQuat::IDENTITY }));
                }
                Some(Kind::Vec3) => {}
            }
        }
        Err(mlua::Error::runtime(format!(
            "expected a quat, got {}",
            type_name(&value)
        )))
    }
}

/// `value`'s type as scripts would name it (`vec3` and `quat` rather than `table`).
pub(crate) fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Table(t) => match kind(t) {
            Some(Kind::Vec3) => "vec3",
            Some(Kind::Quat) => "quat",
            None => "table",
        },
        other => other.type_name(),
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

/// Adds the `vec3` and `quat` globals (callable tables with constants and constructors) from
/// `math.luau`, and keeps their metatables for [`Vec3`] and [`Quat`] conversions.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: `seal_table` checks its argument and touches nothing else.
    let seal: Function = unsafe { lua.create_c_function(seal_table)? };
    let (vec3, quat, vec3_meta, quat_meta): (Table, Table, Table, Table) = lua
        .load(include_str!("math.luau"))
        .set_name("@engine/math.luau")
        .call(seal)?;
    let g = lua.globals();
    g.set("vec3", vec3)?;
    g.set("quat", quat)?;
    lua.set_app_data(Types {
        vec3: vec3_meta,
        quat: quat_meta,
    });
    Ok(())
}

/// `seal(t)` for `math.luau`: makes `t` read-only and returns it. `table.freeze` does the same
/// but first checks the table isn't frozen and has no `__metatable` (by name, every call),
/// which costs more than making the table.
unsafe extern "C-unwind" fn seal_table(l: *mut ffi::lua_State) -> c_int {
    // SAFETY: reads and changes only the value in slot 1, and only when it is a table.
    unsafe {
        ffi::lua_settop(l, 1);
        if ffi::lua_type(l, 1) == ffi::LUA_TTABLE {
            ffi::lua_setreadonly(l, 1, 1);
        }
    }
    1
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

    /// A fixed stream of numbers in [-1, 1) (no rand dependency; the same every run).
    struct Numbers(u64);

    impl Numbers {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        }
        fn vec(&mut self, scale: f64) -> DVec3 {
            DVec3::new(self.next(), self.next(), self.next()) * scale
        }
    }

    fn close(a: DVec3, b: DVec3, tol: f64) -> bool {
        (a - b).length() <= tol * (1.0 + b.length())
    }

    fn vec_of(values: (f64, f64, f64)) -> DVec3 {
        DVec3::new(values.0, values.1, values.2)
    }

    /// The Luau math gives what glam (the engine's math) gives, over a few hundred random
    /// rotations and vectors.
    #[test]
    fn luau_math_matches_glam() {
        use engine_core::math::{DQuat, EulerRot};
        let lua = lua();
        let f: mlua::Function = lua
            .load(
                "return function(what, a, b, c, d, e, f, g)
                    if what == 'rotate' then
                        return (quat.axis_angle(vec3(a, b, c), d) * vec3(e, f, g)):unpack()
                    elseif what == 'euler' then
                        local q = quat.euler(a, b, c)
                        local x, y, z = (q * vec3(d, e, f)):unpack()
                        local yaw, pitch, roll = q:euler()
                        return x, y, z, yaw, pitch, roll
                    elseif what == 'compose' then
                        local q = quat.axis_angle(vec3(a, b, 0.3), c) * quat.euler(d, e)
                        return (q * vec3(f, g, 1)):unpack()
                    elseif what == 'look' then
                        local q = quat.look(vec3(a, b, c), vec3(d, e, f))
                        local x, y, z = (q * vec3.forward):unpack()
                        local ux, uy, uz = (q * vec3.up):unpack()
                        return x, y, z, ux, uy, uz
                    elseif what == 'between' then
                        return (quat.between(vec3(a, b, c), vec3(d, e, f)) * vec3(a, b, c):unit()):unpack()
                    elseif what == 'slerp' then
                        local p, q = quat.euler(a, b, c), quat.euler(d, e, f)
                        local x, y, z = (p:slerp(q, g) * vec3(1, 2, 3)):unpack()
                        return x, y, z, p:angle_to(q), (p:inverse() * (p * vec3(g, 1, 2))):unpack()
                    end
                end",
            )
            .eval()
            .unwrap();
        let mut n = Numbers(7);
        let pi = std::f64::consts::PI;
        for _ in 0..300 {
            let (axis, angle, v) = (n.vec(1.0), n.next() * pi * 2.0, n.vec(50.0));
            let got: (f64, f64, f64) = f
                .call(("rotate", axis.x, axis.y, axis.z, angle, v.x, v.y, v.z))
                .unwrap();
            let want = if axis.length() > 0.0 {
                DQuat::from_axis_angle(axis.normalize(), angle) * v
            } else {
                v
            };
            assert!(close(vec_of(got), want, 1e-9), "rotate {got:?} {want}");

            let (yaw, pitch, roll) = (n.next() * pi, n.next() * 1.4, n.next() * pi);
            let got: (f64, f64, f64, f64, f64, f64) =
                f.call(("euler", yaw, pitch, roll, v.x, v.y, v.z)).unwrap();
            let q = DQuat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
            assert!(
                close(DVec3::new(got.0, got.1, got.2), q * v, 1e-9),
                "euler {got:?} {}",
                q * v
            );
            let back = (got.3, got.4, got.5);
            assert!(
                (back.0 - yaw).abs() < 1e-9
                    && (back.1 - pitch).abs() < 1e-9
                    && (back.2 - roll).abs() < 1e-9,
                "q:euler() {back:?} for {yaw} {pitch} {roll}"
            );

            let got: (f64, f64, f64) = f
                .call(("compose", axis.x, axis.y, angle, yaw, pitch, v.x, v.y))
                .unwrap();
            let q = DQuat::from_axis_angle(DVec3::new(axis.x, axis.y, 0.3).normalize(), angle)
                * DQuat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            assert!(
                close(vec_of(got), q * DVec3::new(v.x, v.y, 1.0), 1e-9),
                "compose"
            );

            let (dir, up) = (n.vec(3.0), n.vec(1.0));
            if dir.length() > 1e-3
                && up.length() > 1e-3
                && dir.normalize().cross(up.normalize()).length() > 1e-2
            {
                let got: (f64, f64, f64, f64, f64, f64) = f
                    .call(("look", dir.x, dir.y, dir.z, up.x, up.y, up.z))
                    .unwrap();
                let forward = DVec3::new(got.0, got.1, got.2);
                assert!(
                    close(forward, dir.normalize(), 1e-9),
                    "look {forward} {dir}"
                );
                let top = DVec3::new(got.3, got.4, got.5);
                assert!(
                    top.dot(up) > 0.0 && top.dot(dir).abs() < 1e-9,
                    "look up {top} {up}"
                );
                let glam = look_rotation(dir.normalize().as_vec3(), up.as_vec3()).as_dquat();
                let (gf, gu) = (glam * DVec3::NEG_Z, glam * DVec3::Y);
                assert!(
                    close(gf, forward, 1e-5) && close(gu, top, 1e-5),
                    "look vs glam: {forward} {top} | {gf} {gu} for {dir} {up}"
                );
            }

            let (a, b) = (n.vec(2.0), n.vec(2.0));
            let got: (f64, f64, f64) = f.call(("between", a.x, a.y, a.z, b.x, b.y, b.z)).unwrap();
            assert!(
                close(vec_of(got), b.normalize(), 1e-9),
                "between {got:?} {b}"
            );

            let (p, q) = (
                DQuat::from_euler(EulerRot::YXZ, yaw, pitch, roll),
                DQuat::from_euler(EulerRot::YXZ, roll, -pitch * 0.5, yaw),
            );
            let t = n.next() * 0.5 + 0.5;
            let got: (f64, f64, f64, f64, f64, f64, f64) = f
                .call(("slerp", yaw, pitch, roll, roll, -pitch * 0.5, yaw, t))
                .unwrap();
            let want = p.slerp(q, t) * DVec3::new(1.0, 2.0, 3.0);
            assert!(
                close(DVec3::new(got.0, got.1, got.2), want, 1e-7),
                "slerp {got:?} {want}"
            );
            assert!(
                (got.3 - p.angle_between(q)).abs() < 1e-7,
                "angle_to {} {}",
                got.3,
                p.angle_between(q)
            );
            assert!(
                close(
                    DVec3::new(got.4, got.5, got.6),
                    DVec3::new(t, 1.0, 2.0),
                    1e-9
                ),
                "inverse"
            );
        }
        // Half turns and straight on, where the formulas have their edge cases.
        let flip: (f64, f64, f64) = f.call(("between", 0.0, 1.0, 0.0, 0.0, -2.0, 0.0)).unwrap();
        assert!(close(vec_of(flip), DVec3::NEG_Y, 1e-12), "{flip:?}");
        let same: (f64, f64, f64) = f.call(("between", 1.0, 0.0, 0.0, 3.0, 0.0, 0.0)).unwrap();
        assert!(close(vec_of(same), DVec3::X, 1e-12), "{same:?}");
    }

    /// Values are frozen, mix with plain tables, and say clearly what went wrong where.
    #[test]
    fn values_are_immutable_and_errors_name_the_line() {
        let lua = lua();
        let err = |code: &str| {
            lua.load(code)
                .set_name("@test.luau")
                .exec()
                .unwrap_err()
                .to_string()
        };
        assert!(err("local v = vec3(1, 2, 3)\nv.x = 5").contains("readonly"));
        assert!(err("local v = vec3(1, 2, 3)\nsetmetatable(v, nil)").contains("readonly"));
        assert!(err("getmetatable(vec3.zero).__add = print").contains("readonly"));
        assert!(err("getmetatable(vec3).__call = print").contains("readonly"));
        let e = err("local v = vec3(1, 2, 3)\nlocal w = v + 5");
        assert!(
            e.contains("test.luau:2:")
                && e.contains("vec3 + vec3: expected a vec3")
                && e.contains("got number"),
            "{e}"
        );
        let e = err("local w = vec3.up * quat.identity");
        assert!(e.contains("test.luau:1:") && e.contains("got quat"), "{e}");
        let e = err("local w = quat.identity * 2");
        assert!(e.contains("quat * vec3") && e.contains("got number"), "{e}");
        let ok: bool = lua
            .load(
                "local v = vec3(1, 2, 3)
                 return v + {1, 1, 1} == vec3(2, 3, 4) and v:dot({x = 1}) == 1
                    and vec3.is(v) and not vec3.is({1, 2, 3}) and quat.is(quat.identity)
                    and tostring(v) == 'vec3(1, 2, 3)' and v ~= vec3(1, 2, 3.5)
                    and vec3(vector.create(1, 2, 3)) == v and #tostring(quat.identity) > 0
                    and vec3.zero:unit() == vec3.zero and vec3(3, 0, 4):length() == 5",
            )
            .eval()
            .unwrap();
        assert!(ok);
        // Rust makes the same values and reads them back.
        let v: Value = Vec3(DVec3::new(1.0, 2.5, -3.0)).into_lua(&lua).unwrap();
        let t = v.as_table().unwrap();
        assert_eq!(kind(t), Some(Kind::Vec3));
        assert_eq!(t.raw_get::<f64>("y").unwrap(), 2.5);
        lua.globals().set("from_rust", v.clone()).unwrap();
        let s: String = lua.load("return tostring(from_rust * 2)").eval().unwrap();
        assert_eq!(s, "vec3(2, 5, -6)");
        assert_eq!(
            Vec3::from_lua(v, &lua).unwrap().0,
            DVec3::new(1.0, 2.5, -3.0)
        );
        let q: Value = lua.load("return quat.euler(1, 0.5)").eval().unwrap();
        assert!(
            Vec3::from_lua(q.clone(), &lua).is_err(),
            "a quat isn't a vec3"
        );
        let back = Quat::from_lua(q, &lua).unwrap().0;
        let want = GQuat::from_euler(engine_core::math::EulerRot::YXZ, 1.0, 0.5, 0.0);
        assert!(back.abs_diff_eq(want, 1e-6), "{back} {want}");
    }
}

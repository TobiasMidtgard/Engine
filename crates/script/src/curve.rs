//! Curves: a value over time drawn as keys, for tweens' easing and timelines' tracks (Unreal's
//! curve assets and FRichCurve, Unity's AnimationCurve). A `.curve` file in the scripts
//! folder is JSON:
//!
//! ```json
//! { "keys": [ { "t": 0, "v": 0 }, { "t": 1.2, "v": 1.08 }, { "t": 1.6, "v": 1 } ] }
//! ```
//!
//! Each key has a time `t` and value `v`; how the curve leaves it (`interp`: `cubic`, the
//! default, `linear` or `constant`), and its slope (`tangent`: `auto`, the default, smooth
//! without overshooting its neighbors; `flat`; `user`, one slope `out` both sides; or `break`,
//! its own `in` and `out`). Before the first key and after the last the curve holds its end
//! value, or repeats (`"after": "loop"`, `"pingpong"`). Edited in the editor's Curves tab;
//! [`ScriptCurves`] keeps them loaded and reloads a file when it changes, so a tween or
//! timeline playing it picks up the change the next frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

/// How the curve goes from a key to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    #[default]
    Cubic,
    Linear,
    Constant,
}

/// How a key's slope is chosen (Unreal's tangent modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tangent {
    /// Smooth through its neighbors, flat at a peak or dip, never overshooting (monotone
    /// cubic: Fritsch and Carlson; Unreal's smart auto).
    #[default]
    Auto,
    Flat,
    /// One slope both sides (`out`).
    User,
    /// Its own slope each side (`in`, `out`).
    Break,
}

/// What the curve does outside its keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extend {
    #[default]
    Hold,
    Loop,
    Pingpong,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A key: a value at a time, and how the curve passes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub t: f64,
    pub v: f64,
    #[serde(default, skip_serializing_if = "is_default")]
    pub interp: Interp,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tangent: Tangent,
    /// Slopes (value per second) for `user` and `break`.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub slope_in: Option<f64>,
    #[serde(default, rename = "out", skip_serializing_if = "Option::is_none")]
    pub slope_out: Option<f64>,
}

impl Key {
    pub fn new(t: f64, v: f64) -> Self {
        Self {
            t,
            v,
            interp: Interp::Cubic,
            tangent: Tangent::Auto,
            slope_in: None,
            slope_out: None,
        }
    }
}

/// A curve (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub keys: Vec<Key>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub before: Extend,
    #[serde(default, skip_serializing_if = "is_default")]
    pub after: Extend,
}

impl Curve {
    pub fn new(keys: Vec<Key>) -> Self {
        let mut c = Self {
            keys,
            ..Self::default()
        };
        c.sort();
        c
    }

    /// From a `.curve` file's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut c: Curve = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if c.keys.iter().any(|k| !k.t.is_finite() || !k.v.is_finite()) {
            return Err("a key's t or v is not a number".into());
        }
        c.sort();
        Ok(c)
    }

    /// As a `.curve` file: one key a line.
    pub fn to_text(&self) -> String {
        let mut out = String::from("{\n  \"keys\": [\n");
        for (i, k) in self.keys.iter().enumerate() {
            let key = serde_json::to_string(k)
                .unwrap_or_default()
                .replace(",\"", ", \"");
            let key = key
                .replace("{\"", "{ \"")
                .replace('}', " }")
                .replace("\":", "\": ");
            out.push_str("    ");
            out.push_str(&key);
            out.push_str(if i + 1 < self.keys.len() { ",\n" } else { "\n" });
        }
        out.push_str("  ]");
        if self.before != Extend::Hold {
            out.push_str(&format!(
                ",\n  \"before\": {}",
                serde_json::to_string(&self.before).unwrap_or_default()
            ));
        }
        if self.after != Extend::Hold {
            out.push_str(&format!(
                ",\n  \"after\": {}",
                serde_json::to_string(&self.after).unwrap_or_default()
            ));
        }
        out.push_str("\n}\n");
        out
    }

    /// Keeps the keys in time order.
    pub fn sort(&mut self) {
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    }

    /// The first and last keys' times (0, 0 without keys).
    pub fn span(&self) -> (f64, f64) {
        match (self.keys.first(), self.keys.last()) {
            (Some(a), Some(b)) => (a.t, b.t),
            _ => (0.0, 0.0),
        }
    }

    /// The lowest and highest the curve goes between its first and last keys (sampled), for
    /// fitting it in view.
    pub fn range(&self) -> (f64, f64) {
        let (a, b) = self.span();
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        for k in &self.keys {
            lo = lo.min(k.v);
            hi = hi.max(k.v);
        }
        if b > a {
            for i in 0..=200 {
                let v = self.eval(a + (b - a) * i as f64 / 200.0);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        if lo > hi { (0.0, 1.0) } else { (lo, hi) }
    }

    /// Key `i`'s slopes into and out of it (value per second), as the curve uses them.
    pub fn slopes(&self, i: usize) -> (f64, f64) {
        let k = &self.keys[i];
        match k.tangent {
            Tangent::Flat => (0.0, 0.0),
            Tangent::User => {
                let s = k.slope_out.or(k.slope_in).unwrap_or(0.0);
                (s, s)
            }
            Tangent::Break => (k.slope_in.unwrap_or(0.0), k.slope_out.unwrap_or(0.0)),
            Tangent::Auto => {
                let s = self.auto_slope(i);
                (s, s)
            }
        }
    }

    /// Fritsch and Carlson's monotone slope (the weighted harmonic mean of the neighbors'
    /// slopes, as PCHIP): flat at a peak or dip and at the ends, never overshooting.
    fn auto_slope(&self, i: usize) -> f64 {
        if i == 0 || i + 1 >= self.keys.len() {
            return 0.0;
        }
        let (a, k, b) = (&self.keys[i - 1], &self.keys[i], &self.keys[i + 1]);
        let (h0, h1) = (k.t - a.t, b.t - k.t);
        if h0 <= 1e-12 || h1 <= 1e-12 {
            return 0.0;
        }
        let (d0, d1) = ((k.v - a.v) / h0, (b.v - k.v) / h1);
        if d0 * d1 <= 0.0 {
            return 0.0;
        }
        let (w0, w1) = (2.0 * h1 + h0, h1 + 2.0 * h0);
        (w0 + w1) / (w0 / d0 + w1 / d1)
    }

    /// The value at `t`.
    pub fn eval(&self, t: f64) -> f64 {
        let n = self.keys.len();
        if n == 0 {
            return 0.0;
        }
        if n == 1 {
            return self.keys[0].v;
        }
        let (a, b) = self.span();
        let len = b - a;
        let t = if t < a {
            extend(self.before, t, a, len)
        } else if t > b {
            extend(self.after, t, a, len)
        } else {
            t
        };
        let i = self.keys.partition_point(|k| k.t <= t);
        if i == 0 {
            return self.keys[0].v;
        }
        if i >= n {
            return self.keys[n - 1].v;
        }
        let (k0, k1) = (&self.keys[i - 1], &self.keys[i]);
        let h = k1.t - k0.t;
        if h <= 1e-12 {
            return k1.v;
        }
        let s = (t - k0.t) / h;
        match k0.interp {
            Interp::Constant => k0.v,
            Interp::Linear => k0.v + (k1.v - k0.v) * s,
            Interp::Cubic => {
                let m0 = self.slopes(i - 1).1;
                let m1 = self.slopes(i).0;
                let (s2, s3) = (s * s, s * s * s);
                (2.0 * s3 - 3.0 * s2 + 1.0) * k0.v
                    + (s3 - 2.0 * s2 + s) * h * m0
                    + (-2.0 * s3 + 3.0 * s2) * k1.v
                    + (s3 - s2) * h * m1
            }
        }
    }

    /// Adds a key at `t` on the curve as it is (keeping its shape there). Returns its index.
    pub fn add_key(&mut self, t: f64) -> usize {
        let v = self.eval(t);
        self.keys.push(Key::new(t, v));
        self.sort();
        self.keys
            .iter()
            .position(|k| k.t == t && k.v == v)
            .unwrap_or(0)
    }

    /// A few shapes to start from (the editor's presets), over 0..1.
    pub fn preset(name: &str) -> Option<Self> {
        let k = |t: f64, v: f64| Key::new(t, v);
        let keys = match name {
            "linear" => {
                let mut a = k(0.0, 0.0);
                a.interp = Interp::Linear;
                vec![a, k(1.0, 1.0)]
            }
            "ease in out" => vec![k(0.0, 0.0), k(1.0, 1.0)],
            "ease in" => {
                let mut b = k(1.0, 1.0);
                b.tangent = Tangent::User;
                b.slope_out = Some(2.0);
                vec![k(0.0, 0.0), b]
            }
            "ease out" => {
                let mut a = k(0.0, 0.0);
                a.tangent = Tangent::User;
                a.slope_out = Some(2.0);
                vec![a, k(1.0, 1.0)]
            }
            "overshoot" => vec![k(0.0, 0.0), k(0.62, 1.12), k(0.84, 0.97), k(1.0, 1.0)],
            "bounce" => vec![
                k(0.0, 0.0),
                k(0.45, 1.0),
                k(0.62, 0.82),
                k(0.78, 1.0),
                k(0.89, 0.94),
                k(1.0, 1.0),
            ],
            "pulse" => vec![k(0.0, 0.0), k(0.25, 1.0), k(1.0, 0.0)],
            _ => return None,
        };
        Some(Self::new(keys))
    }

    pub const PRESETS: [&'static str; 7] = [
        "linear",
        "ease in out",
        "ease in",
        "ease out",
        "overshoot",
        "bounce",
        "pulse",
    ];
}

/// `t` outside the keys (`a` to `a + len`) brought back in.
fn extend(mode: Extend, t: f64, a: f64, len: f64) -> f64 {
    if len <= 1e-12 {
        return a;
    }
    match mode {
        Extend::Hold => t.clamp(a, a + len),
        Extend::Loop => a + (t - a).rem_euclid(len),
        Extend::Pingpong => {
            let u = (t - a).rem_euclid(2.0 * len);
            a + if u > len { 2.0 * len - u } else { u }
        }
    }
}

/// The loaded `.curve` files (a resource the runtime and the editor share): read once, reloaded
/// when the file changes ([`ScriptCurves::poll`]), replaced at once when the editor changes one
/// ([`ScriptCurves::set`]).
#[derive(Resource, Default)]
pub struct ScriptCurves {
    map: HashMap<String, Loaded>,
    /// Changes whenever a curve does.
    pub version: u64,
}

struct Loaded {
    curve: Result<Arc<Curve>, String>,
    full: PathBuf,
    stamp: Option<(SystemTime, u64)>,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// A curve file's path as used to name it: forward slashes, no leading `./`, `.curve` added.
pub fn normalize(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches("./").trim_start_matches('/');
    if p.ends_with(".curve") {
        p.to_string()
    } else {
        format!("{p}.curve")
    }
}

impl ScriptCurves {
    /// The curve at `path` (in `root`, the scripts folder), loading it the first time.
    pub fn get(&mut self, root: &Path, path: &str) -> Result<Arc<Curve>, String> {
        let path = normalize(path);
        if let Some(l) = self.map.get(&path) {
            return l.curve.clone();
        }
        let full = root.join(&path);
        let curve = std::fs::read_to_string(&full)
            .map_err(|e| format!("can't read {path}: {e}"))
            .and_then(|text| Curve::parse(&text).map_err(|e| format!("{path}: {e}")))
            .map(Arc::new);
        let stamp = stamp(&full);
        self.map.insert(
            path.clone(),
            Loaded {
                curve: curve.clone(),
                full,
                stamp,
            },
        );
        curve
    }

    /// The curve at `path` if it is loaded (no file access).
    pub fn loaded(&self, path: &str) -> Option<Arc<Curve>> {
        self.map
            .get(&normalize(path))
            .and_then(|l| l.curve.as_ref().ok().cloned())
    }

    /// Replaces a curve (the editor, as it edits; it writes the file itself).
    pub fn set(&mut self, root: &Path, path: &str, curve: Curve) {
        let path = normalize(path);
        let full = root.join(&path);
        let stamp = stamp(&full);
        self.map.insert(
            path,
            Loaded {
                curve: Ok(Arc::new(curve)),
                full,
                stamp,
            },
        );
        self.version += 1;
    }

    /// Reloads the curves whose files changed. Returns the paths reloaded.
    pub fn poll(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        for (path, l) in self.map.iter_mut() {
            let now = stamp(&l.full);
            if now == l.stamp {
                continue;
            }
            l.stamp = now;
            l.curve = std::fs::read_to_string(&l.full)
                .map_err(|e| format!("can't read {path}: {e}"))
                .and_then(|text| Curve::parse(&text).map_err(|e| format!("{path}: {e}")))
                .map(Arc::new);
            changed.push(path.clone());
        }
        if !changed.is_empty() {
            self.version += 1;
        }
        changed
    }

    /// The loaded curves' paths.
    pub fn paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self.map.keys().cloned().collect();
        out.sort();
        out
    }
}

/// Every `.curve` file under `root`, as paths relative to it.
pub fn find_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>, depth: u32) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 6 {
                walk(&p, root, out, depth + 1);
            } else if p.extension().is_some_and(|x| x == "curve")
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out, 0);
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curve_passes_its_keys_and_never_overshoots_on_auto() {
        let c = Curve::parse(
            r#"{ "keys": [ { "t": 0, "v": 0 }, { "t": 1, "v": 1 }, { "t": 2, "v": 1.5 }, { "t": 3, "v": 0 } ] }"#,
        )
        .unwrap();
        for k in &c.keys {
            assert!((c.eval(k.t) - k.v).abs() < 1e-9);
        }
        // Monotone between 0 and 2, peak at 2, nothing above it.
        let mut last = -1.0;
        for i in 0..=200 {
            let v = c.eval(i as f64 / 100.0);
            assert!(v >= last - 1e-12, "rises to the peak");
            last = v;
        }
        assert!((0..=300).all(|i| c.eval(i as f64 / 100.0) <= 1.5 + 1e-9));
        // Holds outside; loops when asked.
        assert_eq!(c.eval(-1.0), 0.0);
        assert_eq!(c.eval(9.0), 0.0);
        let mut l = c.clone();
        l.after = Extend::Loop;
        assert!((l.eval(4.0) - c.eval(1.0)).abs() < 1e-9);
        l.after = Extend::Pingpong;
        assert!((l.eval(4.0) - c.eval(2.0)).abs() < 1e-9);
        // Linear and constant segments; written and read back the same.
        let mut s = c.clone();
        s.keys[0].interp = Interp::Linear;
        s.keys[1].interp = Interp::Constant;
        assert!((s.eval(0.5) - 0.5).abs() < 1e-9);
        assert_eq!(s.eval(1.5), 1.0);
        s.keys[2].tangent = Tangent::Break;
        s.keys[2].slope_in = Some(2.0);
        s.keys[2].slope_out = Some(-1.0);
        assert_eq!(Curve::parse(&s.to_text()).unwrap(), s);
        for name in Curve::PRESETS {
            let p = Curve::preset(name).unwrap();
            assert!(p.eval(0.0).abs() < 1e-9, "{name}");
        }
    }
}

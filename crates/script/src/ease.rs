//! Easing: how a tween's 0..1 progress maps to how far along it is. Robert Penner's families
//! (sine to bounce, each `in`, `out`, `in_out` and `out_in`, as on easings.net and in Godot's
//! Tween and DOTween), CSS's `cubic-bezier` and `steps`, smoothstep, and a springy settle.
//! engine-design/scripting-research.md §25.

use std::f64::consts::PI;

/// An easing function of progress (0..1); 0 maps to 0 and 1 to 1, and between them it may
/// overshoot (`back`, `elastic`, `spring`).
pub type EaseFn = fn(f64) -> f64;

/// The families with an `in` shape; `out_x` is `in_x` turned around, `in_out_x` is `in_x` then
/// `out_x`, `out_in_x` the other way.
const FAMILIES: [(&str, EaseFn); 10] = [
    ("sine", in_sine),
    ("quad", |x| x * x),
    ("cubic", |x| x * x * x),
    ("quart", |x| x * x * x * x),
    ("quint", |x| x * x * x * x * x),
    ("expo", in_expo),
    ("circ", |x| 1.0 - (1.0 - x * x).max(0.0).sqrt()),
    ("back", in_back),
    ("elastic", in_elastic),
    ("bounce", |x| 1.0 - out_bounce(1.0 - x)),
];

/// The other named ones.
const PLAIN: [(&str, EaseFn); 5] = [
    ("linear", |x| x),
    // Smoothstep and Perlin's smootherstep: gentle at both ends.
    ("smooth", |x| x * x * (3.0 - 2.0 * x)),
    ("smoother", |x| x * x * x * (x * (6.0 * x - 15.0) + 10.0)),
    // A spring let go: past the end and back, settling (Godot's TRANS_SPRING).
    ("spring", out_spring),
    // Older name of in_out_quad.
    ("in_out", |x| in_out(x, |x| x * x)),
];

fn in_sine(x: f64) -> f64 {
    1.0 - (x * PI / 2.0).cos()
}

fn in_expo(x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else {
        2f64.powf(10.0 * x - 10.0)
    }
}

fn in_back(x: f64) -> f64 {
    const C1: f64 = 1.70158;
    (C1 + 1.0) * x * x * x - C1 * x * x
}

fn in_elastic(x: f64) -> f64 {
    if x <= 0.0 || x >= 1.0 {
        return x.clamp(0.0, 1.0);
    }
    let c4 = 2.0 * PI / 3.0;
    -(2f64.powf(10.0 * x - 10.0)) * ((10.0 * x - 10.75) * c4).sin()
}

fn out_bounce(x: f64) -> f64 {
    const N1: f64 = 7.5625;
    const D1: f64 = 2.75;
    if x < 1.0 / D1 {
        N1 * x * x
    } else if x < 2.0 / D1 {
        let x = x - 1.5 / D1;
        N1 * x * x + 0.75
    } else if x < 2.5 / D1 {
        let x = x - 2.25 / D1;
        N1 * x * x + 0.9375
    } else {
        let x = x - 2.625 / D1;
        N1 * x * x + 0.984375
    }
}

fn out_spring(x: f64) -> f64 {
    // Ends at exactly 1: cos(4.5π) = 0.
    1.0 - (-6.0 * x).exp() * (4.5 * PI * x).cos()
}

fn out(x: f64, f: EaseFn) -> f64 {
    1.0 - f(1.0 - x)
}

fn in_out(x: f64, f: EaseFn) -> f64 {
    if x < 0.5 {
        f(2.0 * x) / 2.0
    } else {
        1.0 - f(2.0 - 2.0 * x) / 2.0
    }
}

fn out_in(x: f64, f: EaseFn) -> f64 {
    if x < 0.5 {
        out(2.0 * x, f) / 2.0
    } else {
        0.5 + f(2.0 * x - 1.0) / 2.0
    }
}

/// How an ease shapes its family's curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    In,
    Out,
    InOut,
    OutIn,
}

/// A named ease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    Family(usize, Mode),
    Plain(usize),
}

impl Named {
    /// `in_cubic`, `out_back`, `in_out_sine`, `out_in_expo`, `linear`, `smooth`, ...
    pub fn parse(name: &str) -> Option<Self> {
        if let Some(i) = PLAIN.iter().position(|(n, _)| *n == name) {
            return Some(Named::Plain(i));
        }
        let (mode, family) = if let Some(f) = name.strip_prefix("in_out_") {
            (Mode::InOut, f)
        } else if let Some(f) = name.strip_prefix("out_in_") {
            (Mode::OutIn, f)
        } else if let Some(f) = name.strip_prefix("in_") {
            (Mode::In, f)
        } else {
            (Mode::Out, name.strip_prefix("out_")?)
        };
        let i = FAMILIES.iter().position(|(n, _)| *n == family)?;
        Some(Named::Family(i, mode))
    }

    pub fn name(self) -> String {
        match self {
            Named::Plain(i) => PLAIN[i].0.to_string(),
            Named::Family(i, mode) => {
                let m = match mode {
                    Mode::In => "in",
                    Mode::Out => "out",
                    Mode::InOut => "in_out",
                    Mode::OutIn => "out_in",
                };
                format!("{m}_{}", FAMILIES[i].0)
            }
        }
    }

    pub fn eval(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Named::Plain(i) => PLAIN[i].1(x),
            Named::Family(i, mode) => {
                let f = FAMILIES[i].1;
                match mode {
                    Mode::In => f(x),
                    Mode::Out => out(x, f),
                    Mode::InOut => in_out(x, f),
                    Mode::OutIn => out_in(x, f),
                }
            }
        }
    }
}

/// Every named ease, in the order the editor shows them: the plain ones, then each family's
/// in, out, in_out and out_in.
pub fn names() -> Vec<String> {
    let mut out: Vec<String> = PLAIN
        .iter()
        .filter(|(n, _)| *n != "in_out")
        .map(|(n, _)| n.to_string())
        .collect();
    for (family, _) in FAMILIES {
        for m in ["in", "out", "in_out", "out_in"] {
            out.push(format!("{m}_{family}"));
        }
    }
    out
}

/// The family names (`sine` .. `bounce`).
pub fn families() -> impl Iterator<Item = &'static str> {
    FAMILIES.iter().map(|(n, _)| *n)
}

/// A named ease by name (None: no such name).
pub fn by_name(name: &str, x: f64) -> Option<f64> {
    Named::parse(name).map(|n| n.eval(x))
}

/// CSS's `cubic-bezier(x1, y1, x2, y2)`: the curve from (0, 0) to (1, 1) through those two
/// control points, read as y at x. x1 and x2 are kept to 0..1 (as CSS requires), so x always
/// grows and each x has one y.
pub fn bezier([x1, y1, x2, y2]: [f64; 4], x: f64) -> f64 {
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let x = x.clamp(0.0, 1.0);
    let cubic = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    // Newton's method from x, then bisection if it wanders (flat spots).
    let mut s = x;
    for _ in 0..8 {
        let err = cubic(x1, x2, s) - x;
        if err.abs() < 1e-7 {
            return cubic(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s -= err / d;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    s = x;
    for _ in 0..40 {
        let v = cubic(x1, x2, s);
        if (v - x).abs() < 1e-7 {
            break;
        }
        if v < x {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    cubic(y1, y2, s)
}

/// CSS's `steps(n)` (jump-end): holds each of n levels for 1/n, reaching 1 at the end.
pub fn steps(n: u32, x: f64) -> f64 {
    let n = n.max(1) as f64;
    if x >= 1.0 {
        1.0
    } else {
        ((x.max(0.0) * n).floor() / n).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_ease_starts_at_0_and_ends_at_1() {
        for name in names() {
            let n = Named::parse(&name).unwrap_or_else(|| panic!("{name} parses"));
            assert_eq!(n.name(), name);
            assert!(n.eval(0.0).abs() < 1e-9, "{name} at 0: {}", n.eval(0.0));
            assert!(
                (n.eval(1.0) - 1.0).abs() < 1e-9,
                "{name} at 1: {}",
                n.eval(1.0)
            );
        }
        assert!(Named::parse("in_out").is_some());
        assert!(Named::parse("in_wobble").is_none());
        // Out is in turned around; back overshoots.
        assert!((by_name("out_cubic", 0.5).unwrap() - 0.875).abs() < 1e-9);
        assert!((0..=100).any(|i| by_name("out_back", i as f64 / 100.0).unwrap() > 1.05));
        assert!((by_name("in_out_quad", 0.25).unwrap() - 0.125).abs() < 1e-9);
    }

    #[test]
    fn bezier_matches_css() {
        // CSS `ease-in-out` is symmetric about the middle; `linear` as a bezier is linear.
        let io = [0.42, 0.0, 0.58, 1.0];
        assert!((bezier(io, 0.5) - 0.5).abs() < 1e-5);
        assert!((bezier(io, 0.25) + bezier(io, 0.75) - 1.0).abs() < 1e-5);
        let lin = [0.25, 0.25, 0.75, 0.75];
        for i in 0..=10 {
            let x = i as f64 / 10.0;
            assert!((bezier(lin, x) - x).abs() < 1e-5);
        }
        assert_eq!(steps(4, 0.3), 0.25);
        assert_eq!(steps(4, 1.0), 1.0);
    }
}

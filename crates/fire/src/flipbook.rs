//! The flame flipbook: a small two-dimensional fire simulation, run once when the renderer
//! starts and recorded into a looping 3D texture (across, up, time) that every flame sprite
//! plays back at its own pace. Simulated flames neck in at the base, lick, curl and shed
//! pockets of burning gas the way real ones do, which noise alone doesn't get right. A little
//! turbulence and a puffing burner (a real flame swells and pinches off a bubble of burning gas
//! a couple of times a second) keep it breaking into tongues rather than settling into one
//! smooth plume with a thin ribbon above it.
//!
//! Red is the flame (glowing gas, 0 cold .. 1 hottest), green the soot it leaves above its
//! tips. Row 0 is the base of the flame. The last frames fade into the first, so it loops.

use std::sync::OnceLock;

use engine_core::math::noise::perlin2;

/// Flipbook size: texels across, texels up, frames in the loop.
pub const FLIPBOOK_SIZE: [u32; 3] = [64, 128, 96];

/// Simulation steps between recorded frames.
const STEPS_PER_FRAME: usize = 2;

pub struct Flipbook {
    pub size: [u32; 3],
    /// `[flame, soot]` per texel, x fastest, then y (up), then frame.
    pub texels: Vec<[u8; 2]>,
}

/// The engine's flame flipbook, simulated on first use (well under a second in release).
pub fn flipbook() -> &'static Flipbook {
    static BOOK: OnceLock<Flipbook> = OnceLock::new();
    BOOK.get_or_init(|| simulate(FLIPBOOK_SIZE, 7))
}

/// Tuning of the little fire, in cells and steps.
#[derive(Debug, Clone, Copy)]
struct Params {
    /// Upward acceleration per unit of temperature, cells/step².
    buoyancy: f32,
    /// Downward pull of soot.
    soot_weight: f32,
    /// Vorticity confinement (curls and licks).
    swirl: f32,
    /// Share of the fuel that burns each step once lit.
    burn_rate: f32,
    /// Temperature released per unit of fuel burned.
    heat: f32,
    /// Soot made per unit of fuel burned.
    soot_yield: f32,
    /// Share of the heat lost each step.
    cooling: f32,
    /// Share of the soot that thins away each step.
    soot_fade: f32,
    /// Burner half-width as a share of the grid width.
    burner: f32,
    /// Upward speed of the gas leaving the burner, cells/step.
    inflow: f32,
    /// Turbulent buffeting of hot gas, cells/step².
    turbulence: f32,
    /// Size of the turbulent eddies, cells.
    eddy: f32,
    /// How strongly the burner puffs (the flame swells and pinches off a bubble of burning gas).
    puff: f32,
    /// Steps between puffs.
    puff_period: f32,
}

const PARAMS: Params = Params {
    buoyancy: 0.024,
    soot_weight: 0.01,
    swirl: 0.3,
    burn_rate: 0.009,
    heat: 1.7,
    soot_yield: 0.25,
    cooling: 0.055,
    soot_fade: 0.03,
    burner: 0.34,
    inflow: 0.7,
    turbulence: 0.12,
    eddy: 5.0,
    puff: 0.6,
    puff_period: 64.0,
};

struct Sim {
    w: usize,
    h: usize,
    p: Params,
    seed: u64,
    u: Vec<f32>,
    v: Vec<f32>,
    temp: Vec<f32>,
    fuel: Vec<f32>,
    soot: Vec<f32>,
    pressure: Vec<f32>,
    div: Vec<f32>,
    curl: Vec<f32>,
    /// Turbulence stream function.
    psi: Vec<f32>,
    /// Where each cell's gas was a step ago, and where it goes next.
    from: Vec<(f32, f32)>,
    to: Vec<(f32, f32)>,
    scratch: [(Vec<f32>, Vec<f32>); 3],
}

impl Sim {
    fn new(w: usize, h: usize, p: Params, seed: u64) -> Self {
        let n = w * h;
        Self {
            w,
            h,
            p,
            seed,
            u: vec![0.0; n],
            v: vec![0.0; n],
            temp: vec![0.0; n],
            fuel: vec![0.0; n],
            soot: vec![0.0; n],
            pressure: vec![0.0; n],
            div: vec![0.0; n],
            curl: vec![0.0; n],
            psi: vec![0.0; n],
            from: vec![(0.0, 0.0); n],
            to: vec![(0.0, 0.0); n],
            scratch: std::array::from_fn(|_| (vec![0.0; n], vec![0.0; n])),
        }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    /// Bilinear sample at cell coordinates (cell centers on integers), clamped to the grid.
    fn sample(f: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (w - 1) as f32);
        let y = y.clamp(0.0, (h - 1) as f32);
        let x0 = (x.floor() as usize).min(w - 2);
        let y0 = (y.floor() as usize).min(h - 2);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let i = y0 * w + x0;
        let a = f[i] + (f[i + 1] - f[i]) * fx;
        let b = f[i + w] + (f[i + w + 1] - f[i + w]) * fx;
        a + (b - a) * fy
    }

    /// Fuel, a little heat and upward flow along the burner, flickering in space and time.
    fn feed(&mut self, t: f32) {
        let (w, p) = (self.w, self.p);
        let cx = (w as f32 - 1.0) * 0.5;
        let half = w as f32 * p.burner;
        let s = self.seed;
        for x in 0..w {
            let d = (x as f32 - cx) / half;
            if d.abs() >= 1.0 {
                continue;
            }
            let shape = (1.0 - d * d).powf(0.7);
            let flicker = 0.55
                + 0.45 * perlin2(x as f64 * 0.11, t as f64 * 0.045, s)
                + 0.25 * perlin2(x as f64 * 0.31, t as f64 * 0.11, s + 1);
            let beat = (t / p.puff_period * std::f32::consts::TAU).sin();
            let amount = (shape * flicker * (1.0 + p.puff * beat)).clamp(0.0, 1.0);
            for y in 1..4 {
                let i = self.at(x, y);
                self.fuel[i] = self.fuel[i].max(amount);
                self.temp[i] = self.temp[i].max(0.25 * amount);
                self.v[i] = self.v[i].max(p.inflow * amount);
            }
        }
    }

    fn combust(&mut self) {
        let p = self.p;
        for i in 0..self.temp.len() {
            let f = self.fuel[i];
            if f <= 1e-4 || self.temp[i] < 0.05 {
                continue;
            }
            let r = f * p.burn_rate;
            self.fuel[i] = f - r;
            self.temp[i] = (self.temp[i] + r * p.heat).min(2.0);
            self.soot[i] += r * p.soot_yield;
        }
    }

    fn forces(&mut self, t: f32) {
        let (w, h, p) = (self.w, self.h, self.p);
        for i in 0..self.v.len() {
            self.v[i] += p.buoyancy * self.temp[i] - p.soot_weight * self.soot[i];
        }
        if p.turbulence > 0.0 {
            // Eddies rise with the gas and churn as they go. The push is the curl of a noise
            // stream function, so it stirs without squeezing (a squeezing push leaves a comb of
            // stripes after the pressure solve).
            let f = 1.0 / p.eddy as f64;
            let (drift, rise) = (t as f64 * 0.01, t as f64 * 0.04);
            for y in 0..h {
                for x in 0..w {
                    self.psi[y * w + x] =
                        perlin2(x as f64 * f + drift, y as f64 * f - rise, self.seed + 11);
                }
            }
            // Scaled to the eddy size, so the push is about ±1 across whatever their size.
            let scale = 0.25 * p.eddy;
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let i = y * w + x;
                    let k = p.turbulence * self.temp[i].min(1.0) * scale;
                    self.u[i] += k * (self.psi[i + w] - self.psi[i - w]);
                    self.v[i] -= k * (self.psi[i + 1] - self.psi[i - 1]);
                }
            }
        }
        // Vorticity confinement: spin small eddies back up after the grid smears them.
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = self.at(x, y);
                self.curl[i] =
                    0.5 * ((self.v[i + 1] - self.v[i - 1]) - (self.u[i + w] - self.u[i - w]));
            }
        }
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                let i = self.at(x, y);
                let gx = 0.5 * (self.curl[i + 1].abs() - self.curl[i - 1].abs());
                let gy = 0.5 * (self.curl[i + w].abs() - self.curl[i - w].abs());
                let len = (gx * gx + gy * gy).sqrt() + 1e-5;
                let c = self.curl[i];
                // Only where it's hot: still air around the flame stays still.
                let k = p.swirl * self.temp[i].clamp(0.15, 1.0);
                self.u[i] += k * (gy / len) * c;
                self.v[i] -= k * (gx / len) * c;
            }
        }
    }

    /// Velocity at the edges: the floor is a wall (except the burner, set by `feed`); the sides
    /// and top are open.
    fn velocity_bounds(&mut self) {
        let (w, h) = (self.w, self.h);
        for x in 0..w {
            let i = self.at(x, 0);
            self.u[i] = 0.0;
            self.v[i] = 0.0;
            let top = self.at(x, h - 1);
            self.u[top] = self.u[top - w];
            self.v[top] = self.v[top - w].max(0.0);
        }
        for y in 0..h {
            let l = self.at(0, y);
            let r = self.at(w - 1, y);
            self.u[l] = self.u[l + 1];
            self.v[l] = self.v[l + 1];
            self.u[r] = self.u[r - 1];
            self.v[r] = self.v[r - 1];
        }
    }

    /// Make the flow incompressible (red-black Gauss-Seidel with over-relaxation). Pressure is
    /// zero at the open sides and top, so air is drawn in at the base of the flame.
    fn project(&mut self) {
        let (w, h) = (self.w, self.h);
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = self.at(x, y);
                self.div[i] = 0.5 * (self.u[i + 1] - self.u[i - 1] + self.v[i + w] - self.v[i - w]);
            }
        }
        for _ in 0..40 {
            for color in 0..2 {
                for y in 1..h - 1 {
                    let start = 1 + (y + color) % 2;
                    let mut x = start;
                    while x < w - 1 {
                        let i = y * w + x;
                        // Floor: no flow through it (the cell below mirrors this one).
                        let below = if y == 1 {
                            self.pressure[i]
                        } else {
                            self.pressure[i - w]
                        };
                        let target = (self.pressure[i - 1]
                            + self.pressure[i + 1]
                            + below
                            + self.pressure[i + w]
                            - self.div[i])
                            * 0.25;
                        self.pressure[i] += 1.75 * (target - self.pressure[i]);
                        x += 2;
                    }
                }
            }
        }
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = self.at(x, y);
                let below = if y == 1 {
                    self.pressure[i]
                } else {
                    self.pressure[i - w]
                };
                self.u[i] -= 0.5 * (self.pressure[i + 1] - self.pressure[i - 1]);
                self.v[i] -= 0.5 * (self.pressure[i + w] - below);
            }
        }
    }

    /// Where the gas in cell `(x, y)` came from one step ago (midpoint rule).
    fn trace(&self, x: usize, y: usize, sign: f32) -> (f32, f32) {
        let (w, h) = (self.w, self.h);
        let i = self.at(x, y);
        let (fx, fy) = (x as f32, y as f32);
        let mx = fx - 0.5 * sign * self.u[i];
        let my = fy - 0.5 * sign * self.v[i];
        let u = Self::sample(&self.u, w, h, mx, my);
        let v = Self::sample(&self.v, w, h, mx, my);
        (fx - sign * u, fy - sign * v)
    }

    fn advect_velocity(&mut self) {
        let (w, h) = (self.w, self.h);
        let (nu, nv) = &mut self.scratch[0];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let (sx, sy) = {
                    let mx = x as f32 - 0.5 * self.u[i];
                    let my = y as f32 - 0.5 * self.v[i];
                    (
                        x as f32 - Self::sample(&self.u, w, h, mx, my),
                        y as f32 - Self::sample(&self.v, w, h, mx, my),
                    )
                };
                nu[i] = Self::sample(&self.u, w, h, sx, sy);
                nv[i] = Self::sample(&self.v, w, h, sx, sy);
            }
        }
        std::mem::swap(&mut self.u, nu);
        std::mem::swap(&mut self.v, nv);
    }

    fn step(&mut self, t: f32) {
        self.feed(t);
        self.combust();
        self.forces(t);
        self.velocity_bounds();
        self.project();
        self.velocity_bounds();
        self.advect_velocity();
        // Every scalar moves with the same flow: trace it once, then carry the three at once.
        let (w, h) = (self.w, self.h);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                self.from[i] = self.trace(x, y, 1.0);
                self.to[i] = self.trace(x, y, -1.0);
            }
        }
        let (from, to) = (&self.from, &self.to);
        let [s0, s1, s2] = &mut self.scratch;
        std::thread::scope(|scope| {
            for (field, scratch) in [
                (&mut self.temp, s0),
                (&mut self.fuel, s1),
                (&mut self.soot, s2),
            ] {
                scope.spawn(move || maccormack(field, scratch, from, to, w, h));
            }
        });
        let p = self.p;
        for i in 0..self.temp.len() {
            let t = self.temp[i];
            // Hot gas radiates its heat away fast; it fades faster the hotter it is.
            self.temp[i] = (t - p.cooling * t * (0.6 + 0.4 * t)).max(0.0);
            self.soot[i] *= 1.0 - p.soot_fade;
            // Damp the flow a touch so stray eddies in the still air die out.
            self.u[i] *= 0.995;
            self.v[i] *= 0.995;
        }
    }
}

/// MacCormack advection of a scalar: a semi-Lagrangian step corrected by the error of tracing
/// it back, clamped to its neighbors so it stays stable. Keeps flame edges crisp.
fn maccormack(
    field: &mut [f32],
    (fwd, back): &mut (Vec<f32>, Vec<f32>),
    from: &[(f32, f32)],
    to: &[(f32, f32)],
    w: usize,
    h: usize,
) {
    for (i, &(x, y)) in from.iter().enumerate() {
        fwd[i] = Sim::sample(field, w, h, x, y);
    }
    for (i, &(x, y)) in to.iter().enumerate() {
        back[i] = Sim::sample(fwd, w, h, x, y);
    }
    // The result goes into `back` (each entry is read only here) so `field` stays intact for
    // the clamps.
    for (i, &(sx, sy)) in from.iter().enumerate() {
        let cx = (sx.clamp(0.0, (w - 1) as f32) as usize).min(w - 2);
        let cy = (sy.clamp(0.0, (h - 1) as f32) as usize).min(h - 2);
        let j = cy * w + cx;
        let (a, b, c, d) = (field[j], field[j + 1], field[j + w], field[j + w + 1]);
        let corrected = fwd[i] + 0.5 * (field[i] - back[i]);
        back[i] = corrected
            .clamp(a.min(b).min(c).min(d), a.max(b).max(c).max(d))
            .max(0.0);
    }
    field.copy_from_slice(back);
    // Fresh air at the open sides and top.
    for x in 0..w {
        field[(h - 1) * w + x] = 0.0;
    }
    for y in 0..h {
        field[y * w] = 0.0;
        field[y * w + w - 1] = 0.0;
    }
}

/// Run the fire and record a looping flipbook of `size` (across, up, frames); `seed` picks how
/// the burner flickers.
pub fn simulate(size: [u32; 3], seed: u64) -> Flipbook {
    let [w, h, frames] = size.map(|s| s as usize);
    let blend = frames / 4;
    let mut sim = Sim::new(w, h, PARAMS, seed);
    let mut t = 0.0f32;
    // Let the flame grow and settle first.
    for _ in 0..h * 2 {
        sim.step(t);
        t += 1.0;
    }
    let mut raw: Vec<(Vec<f32>, Vec<f32>)> = Vec::with_capacity(frames + blend);
    for _ in 0..frames + blend {
        for _ in 0..STEPS_PER_FRAME {
            sim.step(t);
            t += 1.0;
        }
        raw.push((sim.temp.clone(), sim.soot.clone()));
    }
    // Loop: the first `blend` frames fade in from the frames recorded after the last one.
    let mixed = |f: usize, k: usize| -> (f32, f32) {
        let (a, b) = (&raw[f], k);
        if f < blend {
            let late = &raw[frames + f];
            let wgt = (f as f32 + 0.5) / blend as f32;
            (
                late.0[b] + (a.0[b] - late.0[b]) * wgt,
                late.1[b] + (a.1[b] - late.1[b]) * wgt,
            )
        } else {
            (a.0[b], a.1[b])
        }
    };
    // Scale so the hottest gas (ignoring a few outliers) is 1.
    let reference = |pick: fn((f32, f32)) -> f32| -> f32 {
        let mut values: Vec<f32> = (0..frames)
            .flat_map(|f| (0..w * h).map(move |k| (f, k)))
            .map(|(f, k)| pick(mixed(f, k)))
            .filter(|v| *v > 0.01)
            .collect();
        if values.is_empty() {
            return 1.0;
        }
        let k = (values.len() as f32 * 0.995) as usize;
        let k = k.min(values.len() - 1);
        let (_, v, _) = values.select_nth_unstable_by(k, f32::total_cmp);
        v.max(1e-3)
    };
    let t_ref = reference(|(t, _)| t);
    let s_ref = reference(|(_, s)| s);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut texels = Vec::with_capacity(w * h * frames);
    // Temperature is stored compressed: the base of a flame is far hotter than its tongues,
    // and the tongues need the precision.
    let compress = |t: f32| (1.0 - (-2.5 * t).exp()) / (1.0 - (-2.5f32).exp());
    for f in 0..frames {
        for k in 0..w * h {
            let (t, s) = mixed(f, k);
            texels.push([byte(compress(t / t_ref)), byte(s / s_ref)]);
        }
    }
    Flipbook { size, texels }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_flipbook_burns_rises_and_loops() {
        let size = [24, 48, 16];
        let book = simulate(size, 3);
        let [w, h, frames] = size.map(|s| s as usize);
        assert_eq!(book.texels.len(), w * h * frames);
        // Flame low down in the middle, cooler air at the top corners.
        let mean = |x0: usize, x1: usize, y0: usize, y1: usize| {
            let mut sum = 0.0;
            let mut n = 0.0;
            for f in 0..frames {
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += book.texels[(f * h + y) * w + x][0] as f32;
                        n += 1.0;
                    }
                }
            }
            sum / n
        };
        let core = mean(w / 2 - 3, w / 2 + 3, 4, h / 3);
        let corner = mean(0, 3, h - 6, h - 1);
        assert!(core > 60.0, "flame core {core}");
        assert!(corner < core * 0.25, "corner {corner} vs core {core}");
        // It moves: consecutive frames differ.
        let frame = |f: usize| &book.texels[f * w * h..(f + 1) * w * h];
        let changed = frame(4)
            .iter()
            .zip(frame(8))
            .filter(|(a, b)| a[0].abs_diff(b[0]) > 20)
            .count();
        assert!(changed > w * h / 50, "only {changed} texels changed");
    }
}

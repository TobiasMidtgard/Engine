//! Rocky planets of other stars without a living surface, painted from their
//! [`SurfaceRecipe`]: airless cratered rock, lava worlds, hot deserts, frozen deserts and ice
//! shells.
//!
//! What each looks like follows the Solar System's own examples: airless planets like
//! Mercury and the Moon (dark young plains among bright cratered highlands); lava worlds like
//! the dark basalt crust of Io's paterae and the predicted dayside magma oceans of planets
//! such as 55 Cancri e and K2-141b (Léger et al. 2011; Demory et al. 2016), where exposed melt
//! glows through cracks at about 1400 K; deserts like Mars (bright dust and dark basaltic
//! provinces, dune fields, few fresh craters under air); frozen deserts with polar caps; and
//! ice shells like Europa's, almost crater-free and crossed by reddish-brown lineae.
//!
//! The glow channel (0..1) says where melt shows; the renderer turns it into light from the
//! melt's temperature.

use std::f32::consts::PI;

use engine_core::math::Vec3;

use super::bake::{Paint, Painter, Texel};
use super::color::{Rgb, mix, scale, srgb};
use super::noise::{Grid, Noise, smooth};
use super::rock::{CraterMap, Craters, grid_width, ragged};
use crate::nearby::{SurfaceRecipe, WorldClass};

const DEG: f32 = PI / 180.0;

/// A barren world: everything but the living and the cloud-wrapped classes.
pub(crate) struct Barren {
    class: WorldClass,
    base: Rgb,
    n: Noise,
    n2: Noise,
    /// Dark provinces (plains, basalt), mottling, dune mask.
    coarse: Grid<3>,
    craters: CraterMap,
    /// Polar caps reach this far from the poles, degrees (0: none).
    caps: f32,
    /// Shows one face to its star: a lava world's magma ocean sits under it.
    locked: bool,
    /// Mean surface temperature, K.
    kelvin: f32,
    /// Under air: the craters are softened and partly buried.
    air: f32,
}

impl Barren {
    /// Whether `class` is drawn by this painter.
    pub(crate) fn draws(class: WorldClass) -> bool {
        matches!(
            class,
            WorldClass::Airless
                | WorldClass::Lava
                | WorldClass::Desert
                | WorldClass::ColdDesert
                | WorldClass::Ice
        )
    }

    pub(crate) fn new(
        r: &SurfaceRecipe,
        color: [f32; 3],
        locked: bool,
        seed: u64,
        width: u32,
    ) -> Self {
        let n = Noise::new(seed);
        let n2 = Noise::new(seed ^ 0x5e);
        let gw = grid_width(width);
        let air = smooth(0.0, 0.5, r.pressure_bar as f32);
        let coarse = Grid::bake(gw, gw / 2, |d| {
            // Large provinces: lowland plains flooded by lava (dark) or bright dusty uplands.
            let warp = Vec3::new(
                n2.fbm(d * 1.7 + Vec3::new(3.0, 0.0, 0.0), 3, 0.5),
                n2.fbm(d * 1.7 + Vec3::new(0.0, 5.0, 0.0), 3, 0.5),
                n2.fbm(d * 1.7 + Vec3::new(0.0, 0.0, 7.0), 3, 0.5),
            );
            let plains = smooth(0.05, 0.35, n.fbm(d * 1.4 + warp * 0.5, 4, 0.55));
            let mottle = n.fbm(d * 5.0 + warp, 3, 0.6);
            let dunes = smooth(0.1, 0.4, n2.fbm(d * 2.5 + Vec3::splat(11.0), 3, 0.5));
            [plains, mottle, dunes]
        });
        // Cratering: the recipe's density, fewer where plains are young, softened by air.
        let density = (0.15 + 0.55 * r.crater_density as f32).min(0.9);
        let fresh = 0.05 * (1.0 - air);
        let craters = Craters::new(16.0 * DEG, density, fresh, width, seed as u32)
            .bake(width, |c| {
                (1.0 - 0.7 * coarse.at_dir(c)[0]) * (1.0 - 0.6 * air)
            });
        let caps = match r.class {
            WorldClass::ColdDesert => 18.0 + 30.0 * smooth(240.0, 160.0, r.temperature_k as f32),
            WorldClass::Desert if r.temperature_k < 330.0 => 8.0,
            _ => 0.0,
        };
        Self {
            class: r.class,
            base: color,
            n,
            n2,
            coarse,
            craters,
            caps,
            locked,
            kelvin: r.temperature_k as f32,
            air,
        }
    }

    fn rock(&self, t: &Texel, plains: f32, mottle: f32) -> (Rgb, f32) {
        let fine = self.n.fbm(t.d * 48.0, 2, 0.5);
        let plains = ragged(plains, fine, 0.5);
        let cr = self.craters.at(t);
        let mottle = mottle + 0.35 * fine;
        let mut c = scale(self.base, (0.85 + 0.12 * mottle) * (1.0 - 0.35 * plains));
        c = scale(c, 1.0 + 0.4 * cr.bright);
        (c, cr.height * (1.0 - 0.5 * self.air) - 0.001 * plains)
    }
}

impl Painter for Barren {
    fn paint(&self, t: &Texel) -> Paint {
        let d = t.d;
        let [plains, mottle, dunes] = self.coarse.at(t.lat, t.lon);
        let (mut c, mut height) = self.rock(t, plains, mottle);
        let mut glow = 0.0;
        match self.class {
            WorldClass::Airless => {}
            WorldClass::Lava => {
                // Black basalt crust broken by a network of glowing cracks, with lakes and
                // flows of open melt; on a locked world a magma ocean under the star.
                let crust = mix(srgb!(0x3d3936), srgb!(0x524a43), smooth(-0.3, 0.4, mottle));
                c = scale(crust, 1.0 + 0.6 * (c[0] / self.base[0].max(1e-3) - 1.0));
                let cracks = self.n2.ridged(d * 14.0, 4);
                let fine = self.n2.ridged(d * 60.0 + Vec3::splat(4.0), 3);
                let lakes = smooth(0.3, 0.42, self.n.fbm(d * 3.5 + Vec3::splat(2.0), 4, 0.55));
                // Local temperature: a locked world without much air keeps its heat under the
                // star (T = T_eq √2 cos^¼ θ there, the night side frozen; Koll & Abbot 2015);
                // a spinning one spreads it by latitude.
                let local = if self.locked {
                    let mu = d.dot(Vec3::Z).max(0.0);
                    self.kelvin * 2f32.sqrt() * mu.powf(0.25)
                } else {
                    self.kelvin * (1.1 * t.lat.cos()).max(0.2).powf(0.25)
                };
                let hot = smooth(950.0, 1300.0, local + 60.0 * mottle);
                let ocean = smooth(1450.0, 1600.0, local + 80.0 * mottle);
                let melt = ((smooth(0.62, 0.8, cracks) * 0.7
                    + smooth(0.7, 0.85, fine) * 0.35 * plains
                    + lakes * smooth(1150.0, 1350.0, local))
                    * hot)
                    .max(ocean)
                    .min(1.0);
                glow = melt;
                // Freshly crusted melt is darker still; the glow carries the light.
                c = mix(c, srgb!(0x2a1610), melt);
                height *= 1.0 - melt;
            }
            WorldClass::Desert | WorldClass::ColdDesert => {
                // Bright dust, dark basaltic provinces, dune seas streaked by the winds.
                let dark = scale(mul_tint(self.base, [0.55, 0.5, 0.48]), 0.75);
                c = mix(c, dark, plains * 0.55);
                let streak = self
                    .n2
                    .fbm(Vec3::new(d.x * 60.0, d.y * 220.0, d.z * 60.0), 3, 0.5);
                let dune_field = ragged(dunes, self.n.fbm(d * 30.0, 2, 0.5), 0.6);
                let dune = mix(
                    scale(self.base, 1.1),
                    scale(self.base, 0.8),
                    smooth(-0.3, 0.3, streak),
                );
                c = mix(c, dune, dune_field * 0.7);
                height *= 1.0 - 0.6 * dune_field;
                c = scale(c, 1.0 + 0.08 * self.n.fbm(d * 120.0, 2, 0.5));
            }
            WorldClass::Ice => {
                // Clean water ice crossed by lineae: double ridges and bands tinted by salts.
                let lineae = self.n2.ridged(d * 6.0 + Vec3::splat(1.0), 5);
                let fine = self.n2.ridged(d * 30.0 + Vec3::splat(8.0), 3);
                let stain = srgb!(0x9a6d4f);
                let ice = mix(
                    scale(self.base, 0.95),
                    scale(self.base, 1.05),
                    smooth(-0.4, 0.4, mottle),
                );
                c = mix(ice, stain, smooth(0.55, 0.85, lineae) * 0.55);
                c = mix(c, stain, smooth(0.7, 0.9, fine) * 0.25);
                // Chaos terrain: blocky darker patches.
                let chaos = smooth(0.45, 0.6, self.n.fbm(d * 4.0 + Vec3::splat(6.0), 3, 0.5));
                c = mix(c, mix(stain, ice, 0.5), chaos * 0.5);
                height = height * 0.3 + 0.0008 * lineae;
            }
            _ => {}
        }
        // Polar caps of frost, ragged at their edges.
        if self.caps > 0.0 {
            let from_pole = 90.0 - t.lat.abs() / DEG;
            let edge = self.caps * (1.0 + 0.25 * self.n.fbm(d * 8.0, 3, 0.55));
            let cap = smooth(edge + 2.0, edge - 2.0, from_pole);
            c = mix(c, srgb!(0xeceff2), cap * 0.9);
        }
        Paint {
            color: c,
            height,
            glow,
            ..Default::default()
        }
    }

    fn relief(&self) -> bool {
        true
    }
}

fn mul_tint(a: Rgb, b: Rgb) -> Rgb {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

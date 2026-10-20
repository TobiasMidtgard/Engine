//! Gravity, ground, water and obscurance queries, and surface materials.

use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

/// A body that pulls (planet, moon, star).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GravityBody {
    pub center: DVec3,
    /// Standard gravitational parameter μ = G·M, m³/s². Earth ≈ 3.986e14.
    pub mu: f64,
    /// Surface radius, meters (inside it gravity falls off linearly to zero at the center).
    pub radius: f64,
}

/// Gravity at any point. Flat levels use `uniform`; planetary scenes add `bodies`
/// (the space module keeps them in sync with orbits).
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Gravity {
    /// Constant acceleration, m/s² (flat worlds). Set to zero when using bodies.
    pub uniform: Vec3,
    pub bodies: Vec<GravityBody>,
}

impl Default for Gravity {
    fn default() -> Self {
        Self {
            uniform: Vec3::new(0.0, -9.81, 0.0),
            bodies: Vec::new(),
        }
    }
}

impl Gravity {
    /// Acceleration at `p`, m/s².
    pub fn at(&self, p: DVec3) -> Vec3 {
        let mut a = self.uniform.as_dvec3();
        for b in &self.bodies {
            let d = b.center - p;
            let r = d.length();
            if r < 1e-6 {
                continue;
            }
            let g = if r >= b.radius {
                b.mu / (r * r)
            } else {
                b.mu / (b.radius * b.radius) * (r / b.radius)
            };
            a += d / r * g;
        }
        a.as_vec3()
    }

    /// Local "up" (opposite gravity). Falls back to +Y in zero-g.
    pub fn up_at(&self, p: DVec3) -> Vec3 {
        (-self.at(p)).try_normalize().unwrap_or(Vec3::Y)
    }
}

/// Identifies a [`SurfaceMaterial`] in [`SurfaceMaterials`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct SurfaceId(pub u16);

impl SurfaceId {
    pub const DEFAULT: SurfaceId = SurfaceId(0);
}

/// How a surface deforms under feet, wheels and bodies.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Deformation {
    /// Rock, metal, concrete.
    #[default]
    None,
    /// Sinks and stays (mud, wet dirt). Depth in meters for a typical footstep.
    Plastic { depth_m: f32 },
    /// Compacts (snow): leaves deep prints that slowly fill with new snow.
    Snow,
    /// Shifts and partially refills (sand).
    Granular { depth_m: f32 },
    /// Blades bend and recover over `recover_secs` (grass, crops).
    Flatten { recover_secs: f32 },
}

/// Physical properties of a surface type. One table shared by physics (friction), audio
/// (footsteps), VFX (impact particles), fire (fuel), terrain (deformation) and AI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceMaterial {
    pub name: String,
    pub friction: f32,
    pub restitution: f32,
    /// kg/m³.
    pub density: f32,
    /// 0 never burns .. 1 tinder.
    pub flammability: f32,
    /// °C at which it ignites.
    pub ignition_c: f32,
    /// kg/m² of fuel for fire spread.
    pub fuel_kg_m2: f32,
    /// How much wetness it soaks up (0 rock .. 1 soil).
    pub absorbency: f32,
    pub deformation: Deformation,
    /// Audio event for footsteps on this surface.
    pub footstep_sound: String,
    /// VFX effect for impacts on this surface.
    pub impact_effect: String,
}

impl Default for SurfaceMaterial {
    fn default() -> Self {
        Self {
            name: "default".into(),
            friction: 0.7,
            restitution: 0.1,
            density: 2000.0,
            flammability: 0.0,
            ignition_c: 600.0,
            fuel_kg_m2: 0.0,
            absorbency: 0.3,
            deformation: Deformation::None,
            footstep_sound: "footstep.default".into(),
            impact_effect: "impact.dust".into(),
        }
    }
}

/// Registry of surface materials. Look up by id or name; modules add their own.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceMaterials {
    pub materials: Vec<SurfaceMaterial>,
}

impl SurfaceMaterials {
    pub fn get(&self, id: SurfaceId) -> &SurfaceMaterial {
        self.materials
            .get(id.0 as usize)
            .unwrap_or(&self.materials[0])
    }

    pub fn id(&self, name: &str) -> Option<SurfaceId> {
        self.materials
            .iter()
            .position(|m| m.name == name)
            .map(|i| SurfaceId(i as u16))
    }

    /// Id for `name`, or the default surface if unknown.
    pub fn id_or_default(&self, name: &str) -> SurfaceId {
        self.id(name).unwrap_or_default()
    }

    /// Add (or replace by name) a material and return its id.
    pub fn add(&mut self, material: SurfaceMaterial) -> SurfaceId {
        if let Some(id) = self.id(&material.name) {
            self.materials[id.0 as usize] = material;
            id
        } else {
            self.materials.push(material);
            SurfaceId(self.materials.len() as u16 - 1)
        }
    }
}

impl Default for SurfaceMaterials {
    fn default() -> Self {
        let m = |name: &str,
                 friction: f32,
                 density: f32,
                 flammability: f32,
                 fuel: f32,
                 absorbency: f32,
                 deformation: Deformation| {
            SurfaceMaterial {
                name: name.into(),
                friction,
                density,
                flammability,
                fuel_kg_m2: fuel,
                absorbency,
                deformation,
                ignition_c: if flammability > 0.0 { 300.0 } else { 900.0 },
                footstep_sound: format!("footstep.{name}"),
                impact_effect: format!("impact.{name}"),
                ..Default::default()
            }
        };
        use Deformation::*;
        Self {
            materials: vec![
                m("default", 0.7, 2000.0, 0.0, 0.0, 0.3, None),
                m("rock", 0.8, 2600.0, 0.0, 0.0, 0.05, None),
                m(
                    "dirt",
                    0.75,
                    1500.0,
                    0.0,
                    0.0,
                    0.7,
                    Plastic { depth_m: 0.01 },
                ),
                m(
                    "grass",
                    0.65,
                    1300.0,
                    0.6,
                    0.6,
                    0.6,
                    Flatten { recover_secs: 20.0 },
                ),
                m(
                    "mud",
                    0.45,
                    1700.0,
                    0.0,
                    0.0,
                    1.0,
                    Plastic { depth_m: 0.06 },
                ),
                m(
                    "sand",
                    0.6,
                    1600.0,
                    0.0,
                    0.0,
                    0.4,
                    Granular { depth_m: 0.04 },
                ),
                m("snow", 0.35, 300.0, 0.0, 0.0, 0.2, Snow),
                m("ice", 0.05, 917.0, 0.0, 0.0, 0.0, None),
                m("water", 0.1, 1000.0, 0.0, 0.0, 1.0, None),
                m("wood", 0.6, 600.0, 0.8, 8.0, 0.3, None),
                m(
                    "leaves",
                    0.55,
                    200.0,
                    0.9,
                    1.0,
                    0.5,
                    Flatten { recover_secs: 60.0 },
                ),
                m("metal", 0.5, 7800.0, 0.0, 0.0, 0.0, None),
                m("concrete", 0.8, 2400.0, 0.0, 0.0, 0.1, None),
                m("glass", 0.3, 2500.0, 0.0, 0.0, 0.0, None),
                m("fabric", 0.7, 300.0, 0.7, 0.5, 0.8, None),
                m("flesh", 0.6, 1050.0, 0.1, 0.0, 0.2, None),
            ],
        }
    }
}

/// What the ground looks like at one spot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundSample {
    /// World height of the surface, meters.
    pub height: f64,
    pub normal: Vec3,
    pub surface: SurfaceId,
}

/// Implemented by the terrain module (heightmaps, voxels, planets). Lets foliage, fire, AI
/// and footsteps ask about the ground without depending on how terrain works.
pub trait GroundQuery: Send + Sync + 'static {
    /// Ground directly below/above world X/Z (`p.y` is a hint for multi-level terrain such as
    /// caves: return the first surface at or below it).
    fn sample(&self, p: DVec3) -> Option<GroundSample>;
}

/// The active ground provider. Defaults to a flat grass plane at y = 0.
#[derive(Resource, Clone)]
pub struct Ground(pub Arc<dyn GroundQuery>);

struct FlatGround;
impl GroundQuery for FlatGround {
    fn sample(&self, _p: DVec3) -> Option<GroundSample> {
        Some(GroundSample {
            height: 0.0,
            normal: Vec3::Y,
            surface: SurfaceId(3),
        })
    }
}

impl Default for Ground {
    fn default() -> Self {
        Self(Arc::new(FlatGround))
    }
}

impl Ground {
    pub fn sample(&self, p: DVec3) -> Option<GroundSample> {
        self.0.sample(p)
    }

    pub fn height(&self, p: DVec3) -> Option<f64> {
        self.0.sample(p).map(|s| s.height)
    }
}

/// How wooded the land is at one spot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForestSample {
    /// Canopy cover, 0 (open) to 1 (closed wood).
    pub density: f32,
    /// Share of needle trees, 0 (all broadleaf) to 1 (all conifers).
    pub conifer: f32,
}

/// Implemented by the terrain module (its painted far woods and climate belts). Lets foliage
/// grow trees where the terrain paints woods, without depending on how terrain works.
pub trait ForestQuery: Send + Sync + 'static {
    /// Woods at world position `p` (the ground below it), `None` where none are painted.
    fn sample(&self, p: DVec3) -> Option<ForestSample>;
}

/// The active forest provider. Defaults to no woods anywhere.
#[derive(Resource, Clone)]
pub struct ForestCover(pub Arc<dyn ForestQuery>);

struct NoForest;
impl ForestQuery for NoForest {
    fn sample(&self, _p: DVec3) -> Option<ForestSample> {
        None
    }
}

impl Default for ForestCover {
    fn default() -> Self {
        Self(Arc::new(NoForest))
    }
}

impl ForestCover {
    pub fn sample(&self, p: DVec3) -> Option<ForestSample> {
        self.0.sample(p)
    }
}

/// Implemented by the terrain module (its wear map, fed by `GroundContact`s). Lets foliage thin
/// out on paths, AI prefer them and footsteps sound like bare dirt, without depending on how
/// terrain keeps it.
pub trait WearQuery: Send + Sync + 'static {
    /// How worn the ground below world position `p` is: 0 untouched .. 1 a well-worn path.
    fn wear(&self, p: DVec3) -> f32;
}

/// The active ground-wear provider. Defaults to untouched ground everywhere.
#[derive(Resource, Clone)]
pub struct GroundWear(pub Arc<dyn WearQuery>);

struct NoWear;
impl WearQuery for NoWear {
    fn wear(&self, _p: DVec3) -> f32 {
        0.0
    }
}

impl Default for GroundWear {
    fn default() -> Self {
        Self(Arc::new(NoWear))
    }
}

impl GroundWear {
    /// Wear at `p`, clamped to 0..1.
    pub fn wear(&self, p: DVec3) -> f32 {
        self.0.wear(p).clamp(0.0, 1.0)
    }
}

/// Water at one spot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterSample {
    /// World height of the water surface (including waves), meters.
    pub surface_height: f64,
    /// Water depth below the surface at this X/Z, meters.
    pub depth: f32,
    /// Surface current, m/s.
    pub velocity: Vec3,
    pub normal: Vec3,
}

/// Implemented by the water module. Used for buoyancy, swimming, splashes, AI pathing.
pub trait WaterQuery: Send + Sync + 'static {
    fn sample(&self, p: DVec3) -> Option<WaterSample>;
}

/// The active water provider. Defaults to no water anywhere.
#[derive(Resource, Clone)]
pub struct Water(pub Arc<dyn WaterQuery>);

struct NoWater;
impl WaterQuery for NoWater {
    fn sample(&self, _p: DVec3) -> Option<WaterSample> {
        None
    }
}

impl Default for Water {
    fn default() -> Self {
        Self(Arc::new(NoWater))
    }
}

impl Water {
    pub fn sample(&self, p: DVec3) -> Option<WaterSample> {
        self.0.sample(p)
    }

    /// How far below the water surface `p` is (positive = submerged).
    pub fn submersion(&self, p: DVec3) -> f32 {
        self.sample(p)
            .map(|w| (w.surface_height - p.y) as f32)
            .unwrap_or(f32::NEG_INFINITY)
    }
}

/// Implemented by anything that dims the air: smoke (`engine_smoke`), and later weather fog or
/// dust from destruction. Lets NPC AI, gameplay and UI ask "can I see through this" without
/// depending on the module that simulates it.
///
/// The two methods must agree: `transmittance(a, b)` is `exp(-∫ extinction ds)` along the
/// segment. [`check_obscurance`] tests that, so a replacement module behaves the same.
pub trait ObscuranceQuery: Send + Sync + 'static {
    /// Fraction of light that gets from `from` to `to` through this source: 1 = clear air,
    /// 0 = opaque. The same in both directions.
    fn transmittance(&self, from: DVec3, to: DVec3) -> f32;
    /// Extinction coefficient at `p`, 1/m (0 in clear air). Over `d` meters of it, light falls
    /// to `exp(-extinction * d)`; you can see about `3 / extinction` meters.
    fn extinction(&self, p: DVec3) -> f32;
}

/// Everything that obscures sight, combined. Each module registers its own source by name, so
/// smoke, fog and dust stack and any one of them can be swapped out. Clear air by default.
///
/// ```ignore
/// // The smoke module, once per simulation step:
/// obscurance.set("smoke", Arc::new(smoke_snapshot));
/// // NPC AI:
/// let can_see = obscurance.transmittance(eye, target) > 0.3;
/// ```
#[derive(Resource, Clone, Default)]
pub struct Obscurance {
    sources: Vec<(String, Arc<dyn ObscuranceQuery>)>,
}

impl Obscurance {
    /// Adds a source, or replaces the one already registered under `name`.
    pub fn set(&mut self, name: impl Into<String>, source: Arc<dyn ObscuranceQuery>) {
        let name = name.into();
        match self.sources.iter_mut().find(|(n, _)| *n == name) {
            Some((_, s)) => *s = source,
            None => self.sources.push((name, source)),
        }
    }

    /// Removes the source registered under `name`. Returns whether there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.sources.len();
        self.sources.retain(|(n, _)| n != name);
        self.sources.len() != before
    }

    /// Names of the registered sources.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|(n, _)| n.as_str())
    }

    /// Fraction of light that gets from `from` to `to` through every source: 1 = clear air,
    /// 0 = opaque. A source's values outside 0..1 are clamped; NaN counts as clear air.
    pub fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        self.sources
            .iter()
            .map(|(_, s)| {
                let t = s.transmittance(from, to);
                if t.is_nan() { 1.0 } else { t.clamp(0.0, 1.0) }
            })
            .product()
    }

    /// Extinction coefficient at `p` from every source, 1/m. Negative values and NaN from a
    /// source count as clear air.
    pub fn extinction(&self, p: DVec3) -> f32 {
        // Folded from +0.0 so clear air gives +0.0 (an empty float sum is -0.0, and
        // `3 / extinction` would then be -inf instead of +inf).
        self.sources.iter().fold(0.0, |sum, (_, s)| {
            let e = s.extinction(p);
            if e > 0.0 { sum + e } else { sum }
        })
    }
}

/// Distance between the extinction samples [`check_obscurance`] takes along a segment, meters.
/// Segments longer than about 10 km are sampled more coarsely.
pub const OBSCURANCE_CHECK_STEP_M: f64 = 0.01;

/// Checks that an [`ObscuranceQuery`] keeps the contract along the segment `from`..`to`:
/// values in range, clear over zero distance, the same both ways (within 0.02), and a
/// transmittance whose optical depth `-ln(t)` matches the extinction integrated every
/// [`OBSCURANCE_CHECK_STEP_M`] (within 0.03 + 10 %; anything below 1e-6 counts as opaque).
/// Call it from the implementing module's tests with segments through clear air, thin smoke,
/// thick smoke and long sight lines.
pub fn check_obscurance(
    source: &dyn ObscuranceQuery,
    from: DVec3,
    to: DVec3,
) -> Result<(), String> {
    let t = source.transmittance(from, to);
    if !(0.0..=1.0).contains(&t) {
        return Err(format!("transmittance {t} is outside 0..1"));
    }
    let back = source.transmittance(to, from);
    if !within(t.into(), back.into(), 0.02) {
        return Err(format!("transmittance differs by direction: {t} vs {back}"));
    }
    let here = source.transmittance(from, from);
    if !within(here.into(), 1.0, 1e-3) {
        return Err(format!("transmittance over zero distance is {here}, not 1"));
    }
    let length = from.distance(to);
    let steps = ((length / OBSCURANCE_CHECK_STEP_M).ceil() as usize).clamp(1, 1 << 20);
    let step = length / steps as f64;
    let mut optical_depth = 0.0;
    for i in 0..steps {
        let p = from.lerp(to, (i as f64 + 0.5) / steps as f64);
        let e = source.extinction(p);
        if !(e >= 0.0 && e.is_finite()) {
            return Err(format!("extinction at {p} is {e}"));
        }
        optical_depth += e as f64 * step;
    }
    // Compare optical depths, so a mismatch shows in thick smoke too, where both
    // transmittances are near zero.
    let opaque = -(1e-6f64).ln();
    let claimed = (-(t as f64).ln()).min(opaque);
    let integrated = optical_depth.min(opaque);
    if !within(claimed, integrated, 0.03 + 0.1 * integrated) {
        return Err(format!(
            "transmittance {t} (optical depth {claimed:.3}) doesn't match its extinction \
             (optical depth {integrated:.3})"
        ));
    }
    Ok(())
}

/// `a` and `b` differ by at most `tolerance`; false if either is NaN.
fn within(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A uniform ball of smoke.
    struct Ball {
        center: DVec3,
        radius: f64,
        extinction: f32,
    }

    impl ObscuranceQuery for Ball {
        fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
            // Length of the segment inside the ball.
            let d = to - from;
            let len = d.length();
            if len == 0.0 {
                return 1.0;
            }
            let dir = d / len;
            let oc = from - self.center;
            let b = oc.dot(dir);
            let disc = b * b - (oc.length_squared() - self.radius * self.radius);
            if disc <= 0.0 {
                return 1.0;
            }
            let (t0, t1) = (-b - disc.sqrt(), -b + disc.sqrt());
            let inside = (t1.min(len) - t0.max(0.0)).max(0.0);
            (-(self.extinction as f64) * inside).exp() as f32
        }

        fn extinction(&self, p: DVec3) -> f32 {
            if p.distance(self.center) < self.radius {
                self.extinction
            } else {
                0.0
            }
        }
    }

    fn ball(x: f64, extinction: f32) -> Arc<dyn ObscuranceQuery> {
        Arc::new(Ball {
            center: DVec3::new(x, 0.0, 0.0),
            radius: 1.0,
            extinction,
        })
    }

    const A: DVec3 = DVec3::new(-10.0, 0.0, 0.0);
    const B: DVec3 = DVec3::new(10.0, 0.0, 0.0);

    #[test]
    fn air_is_clear_without_sources() {
        let o = Obscurance::default();
        assert_eq!(o.transmittance(A, B), 1.0);
        let e = o.extinction(DVec3::ZERO);
        assert_eq!(e, 0.0);
        // Visibility distance is +inf, not -inf.
        assert_eq!(3.0 / e, f32::INFINITY);
    }

    #[test]
    fn sources_stack_and_replace_by_name() {
        let mut o = Obscurance::default();
        o.set("smoke", ball(0.0, 0.5));
        let one = o.transmittance(A, B);
        assert!((one - (-1.0f32).exp()).abs() < 1e-4, "{one}");
        o.set("dust", ball(5.0, 0.5));
        let two = o.transmittance(A, B);
        assert!((two - one * one).abs() < 1e-4, "{two}");
        assert_eq!(o.extinction(DVec3::ZERO), 0.5);
        // A new snapshot replaces the old one instead of stacking on it.
        o.set("smoke", ball(0.0, 0.0));
        assert!((o.transmittance(A, B) - one).abs() < 1e-4);
        assert_eq!(o.names().collect::<Vec<_>>(), ["smoke", "dust"]);
        assert!(o.remove("dust"));
        assert!(!o.remove("dust"));
        assert_eq!(o.transmittance(A, B), 1.0);
    }

    /// Returns the same values everywhere.
    struct Constant(f32, f32);
    impl ObscuranceQuery for Constant {
        fn transmittance(&self, _: DVec3, _: DVec3) -> f32 {
            self.0
        }
        fn extinction(&self, _: DVec3) -> f32 {
            self.1
        }
    }

    #[test]
    fn broken_sources_count_as_clear_air() {
        for broken in [Constant(1.5, -1.0), Constant(f32::NAN, f32::NAN)] {
            let mut o = Obscurance::default();
            o.set("smoke", ball(0.0, 0.5));
            o.set("broken", Arc::new(broken));
            assert!((o.transmittance(A, B) - (-1.0f32).exp()).abs() < 1e-4);
            assert_eq!(o.extinction(DVec3::ZERO), 0.5);
            assert_eq!(o.extinction(A), 0.0);
        }
        assert!(check_obscurance(&Constant(1.5, -1.0), A, B).is_err());
        assert!(check_obscurance(&Constant(f32::NAN, 0.0), A, B).is_err());
        assert!(check_obscurance(&Constant(1.0, f32::NAN), A, B).is_err());
    }

    #[test]
    fn the_check_accepts_consistent_sources() {
        let thick = Ball {
            center: DVec3::ZERO,
            radius: 1.0,
            extinction: 2.0,
        };
        let thin = Ball {
            extinction: 0.05,
            ..thick
        };
        let medium = Ball {
            extinction: 0.5,
            ..thick
        };
        for source in [&thick, &thin, &medium] {
            for (from, to) in [
                (A, B),
                (A, A),
                // Off center: a 1.7 m chord.
                (DVec3::new(-10.0, 0.5, 0.0), DVec3::new(10.0, 0.5, 0.0)),
                // Ends inside the ball.
                (DVec3::new(0.2, 0.1, 0.0), B),
                // Misses it.
                (DVec3::new(0.0, 3.0, -3.0), B),
            ] {
                check_obscurance(source, from, to).unwrap();
            }
        }
        // A small, dense puff off-center on a 300 m sight line.
        let puff = Ball {
            center: DVec3::new(0.37, 0.0, 0.0),
            radius: 0.3,
            extinction: 3.0,
        };
        let (far_a, far_b) = (DVec3::new(-150.0, 0.0, 0.0), DVec3::new(150.0, 0.0, 0.0));
        check_obscurance(&puff, far_a, far_b).unwrap();
        check_obscurance(&medium, far_a, far_b).unwrap();
    }

    #[test]
    fn the_check_rejects_inconsistent_sources() {
        struct OneWay;
        impl ObscuranceQuery for OneWay {
            fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
                if from.x < to.x { 0.5 } else { 1.0 }
            }
            fn extinction(&self, _: DVec3) -> f32 {
                0.0
            }
        }
        assert!(check_obscurance(&OneWay, A, B).is_err());
        // Says the air is clear but reports thick smoke along the way.
        assert!(check_obscurance(&Constant(1.0, 1.0), A, B).is_err());
        // Thick smoke, but 50 % thicker than its extinction says: both transmittances are near
        // zero, so only the optical depths show it.
        struct TooThick(Ball);
        impl ObscuranceQuery for TooThick {
            fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
                self.0.transmittance(from, to).powf(1.5)
            }
            fn extinction(&self, p: DVec3) -> f32 {
                self.0.extinction(p)
            }
        }
        let thick = TooThick(Ball {
            center: DVec3::ZERO,
            radius: 1.0,
            extinction: 2.0,
        });
        assert!(check_obscurance(&thick, A, B).is_err());
        // Not clear over zero distance.
        struct NanHere;
        impl ObscuranceQuery for NanHere {
            fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
                if from == to { f32::NAN } else { 1.0 }
            }
            fn extinction(&self, _: DVec3) -> f32 {
                0.0
            }
        }
        assert!(check_obscurance(&NanHere, A, B).is_err());
    }
}

//! Moving parts: wheels that steer, spin and ride the suspension, landing gear that folds
//! away, control surfaces that deflect, propellers that spin, exhaust plumes that grow with
//! thrust, brake lights, hulls that glow with re-entry heat and the glowing air around them. All are child entities of the
//! vehicle, updated in `Update` from the simulation state, so they work with any physics
//! backend.

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, StandardMaterial};
use engine_world::{ExhaustEffects, VehicleControls};

use crate::aero::{Aircraft, Propulsion};
use crate::drivetrain::Drivetrain;
use crate::ship::{HeatShield, Spacecraft, blackbody};
use crate::wheels::Wheels;

/// Draws wheel `index` of the parent vehicle at its suspension position, steering and spin.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WheelVisual {
    pub index: usize,
    /// Hide when the aircraft retracts its gear (gear that folds away carries a [`GearFold`]
    /// instead).
    pub retracts: bool,
    /// Farthest the wheel is drawn below its mount, m: the end of a landing-gear leg, so a
    /// wheel hanging free in flight stays on its axle. `f32::MAX` follows the suspension.
    pub max_travel: f32,
}

/// A landing-gear part (leg, door or wheel) that folds with the parent [`Aircraft`]'s gear:
/// turned about `axis` through `pivot` (the vehicle's frame) by `angles.0` radians when
/// stowed and `angles.1` when down, moving while the gear position crosses `span`. Doors
/// take the first part of the travel and legs the rest, so the doors open before the legs
/// swing down and close after they are in.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GearFold {
    pub pivot: Vec3,
    pub axis: Vec3,
    pub angles: (f32, f32),
    pub span: (f32, f32),
}

impl GearFold {
    /// The part's turn at gear position `gear` (0 stowed .. 1 down).
    pub fn rotation(&self, gear: f32) -> Quat {
        let (a, b) = self.span;
        let t = ((gear - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        Quat::from_axis_angle(
            self.axis,
            self.angles.0 + (self.angles.1 - self.angles.0) * t,
        )
    }

    /// Where point `p` of the part as built moves at gear position `gear`.
    pub fn apply(&self, gear: f32, p: Vec3) -> Vec3 {
        self.pivot + self.rotation(gear) * (p - self.pivot)
    }

    /// The same part on the other side (mirrored across x = 0).
    pub fn mirrored(&self) -> Self {
        Self {
            pivot: self.pivot * Vec3::new(-1.0, 1.0, 1.0),
            axis: self.axis * Vec3::new(-1.0, 1.0, 1.0),
            angles: (-self.angles.0, -self.angles.1),
            span: self.span,
        }
    }
}

/// Rotates with control surface `index` of the parent [`Aircraft`] about local `hinge`.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SurfaceVisual {
    pub index: usize,
    pub hinge: Vec3,
    pub base: Quat,
}

/// Spins with engine `index` of the parent [`Propulsion`] about local −Z.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PropellerVisual {
    pub index: usize,
    pub base: Quat,
}

/// The blades (`blur: false`) or the blur disc (`blur: true`) of engine `index`'s propeller,
/// a child of its [`PropellerVisual`]: the disc shows in place of the blades at speed.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PropellerPart {
    pub index: usize,
    pub blur: bool,
}

/// Engine level above which a propeller turns too fast to see its blades.
const PROP_BLUR_LEVEL: f32 = 0.12;

/// An exhaust plume for thruster `index` of the parent [`Spacecraft`]: scaled along its
/// local +Z by the thruster's level, hidden when off.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PlumeVisual {
    pub index: usize,
    /// Plume length at full thrust and nozzle radius, meters.
    pub length: f32,
    pub radius: f32,
}

/// A lamp that glows brighter while braking (or reversing, `reverse: true`).
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BrakeLight {
    pub off: StandardMaterial,
    pub on: StandardMaterial,
    pub reverse: bool,
}

/// A surface that glows with the parent's [`HeatShield`] temperature.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HeatGlow {
    pub base: StandardMaterial,
    /// Quantized glow level currently shown.
    pub level: u8,
    /// How much of the shield's heating above 288 K this part gets: 1 on the windward belly,
    /// nose and leading edges; far less in the lee, which the hot flow skips over.
    pub share: f32,
}

/// Glowing air of a re-entry, brightening with the parent's [`HeatShield`] heat flux: the
/// shock layer hugging the windward skin (`wake: None`), or the wake streaming behind the
/// ship along the airflow.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlasmaVisual {
    /// Material at full glow ([`PLASMA_FULL_FLUX`]).
    pub base: StandardMaterial,
    /// Quantized glow level currently shown (0 = hidden).
    pub level: u8,
    pub wake: Option<PlasmaWake>,
}

/// How a [`PlasmaVisual`] wake sits: a unit cone along +Z (see the vehicle model) scaled
/// to the airflow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlasmaWake {
    /// Where it leaves the ship (body axes), such as a wingtip; `None` for the main wake,
    /// which leaves from the far side of the ship's skin.
    pub from: Option<Vec3>,
    /// Half extents of the ship's skin (body axes), for the main wake.
    pub skin: Vec3,
    /// Wake radius where it leaves the ship, m.
    pub radius: f32,
    /// Wake length at full glow, m.
    pub length: f32,
}

/// Nose heat flux at which re-entry plasma glows fully, W/m² (about the peak of a return
/// from low orbit: 7 km/s at 70 km). Air starts to glow faintly at about 5% of it.
pub const PLASMA_FULL_FLUX: f32 = 3.0e5;

/// Wheels: position along the suspension, steering, spin; retract with aircraft gear.
pub fn wheel_visuals(
    vehicles: Query<(&Wheels, Option<&Aircraft>)>,
    mut q: Query<(&ChildOf, &WheelVisual, Option<&GearFold>, &mut Transform)>,
) {
    for (parent, vis, fold, mut t) in &mut q {
        let Ok((wheels, aircraft)) = vehicles.get(parent.parent()) else {
            continue;
        };
        let Some(w) = wheels.wheels.get(vis.index) else {
            continue;
        };
        let st = &w.state;
        // On the ground the wheel sits where the spring puts it; in the air it hangs at full
        // travel, which the model's leg may not reach.
        let travel = if st.grounded {
            st.spring_length
        } else {
            w.suspension.travel_m.min(vis.max_travel)
        };
        // Gear that doesn't fold retracts by shrinking away (with everything attached to it).
        let shown = !vis.retracts || aircraft.is_none_or(|a| a.gear_down);
        let gear = aircraft.map_or(1.0, |a| a.gear);
        // Folding gear steers only when fully down.
        let steer = if fold.is_some() && gear < 1.0 {
            0.0
        } else {
            st.steer
        };
        let mut at = w.mount - Vec3::Y * travel;
        let mut rotation = Quat::from_rotation_y(-steer) * Quat::from_rotation_x(-st.angle);
        if let Some(fold) = fold {
            let turn = fold.rotation(gear);
            at = fold.pivot + turn * (at - fold.pivot);
            rotation = turn * rotation;
        }
        let new = Transform {
            translation: at.as_dvec3(),
            rotation,
            scale: if shown { Vec3::ONE } else { Vec3::splat(1e-3) },
        };
        if *t != new {
            *t = new;
        }
    }
}

/// Landing-gear legs and doors: turn with the aircraft's gear position (wheels follow in
/// [`wheel_visuals`]).
pub fn gear_visuals(
    aircraft: Query<&Aircraft>,
    mut q: Query<(&ChildOf, &GearFold, &mut Transform), Without<WheelVisual>>,
) {
    for (parent, fold, mut t) in &mut q {
        let Ok(a) = aircraft.get(parent.parent()) else {
            continue;
        };
        let rotation = fold.rotation(a.gear);
        let translation = (fold.pivot - rotation * fold.pivot).as_dvec3();
        if t.rotation != rotation || t.translation != translation {
            t.rotation = rotation;
            t.translation = translation;
        }
    }
}

/// Control surfaces and propellers.
#[allow(clippy::type_complexity)]
pub fn aircraft_visuals(
    aircraft: Query<&Aircraft>,
    engines: Query<&Propulsion>,
    pivots: Query<&ChildOf, With<PropellerVisual>>,
    mut surfaces: Query<
        (&ChildOf, &SurfaceVisual, &mut Transform),
        (Without<PropellerVisual>, Without<PropellerPart>),
    >,
    mut props: Query<
        (&ChildOf, &PropellerVisual, &mut Transform),
        (Without<SurfaceVisual>, Without<PropellerPart>),
    >,
    mut parts: Query<
        (&ChildOf, &PropellerPart, &mut Transform),
        (Without<SurfaceVisual>, Without<PropellerVisual>),
    >,
) {
    for (parent, vis, mut t) in &mut surfaces {
        let Ok(a) = aircraft.get(parent.parent()) else {
            continue;
        };
        let Some(s) = a.surfaces.get(vis.index) else {
            continue;
        };
        // + deflection = trailing edge toward the surface's −up: rotate about the hinge.
        let rot = vis.base * Quat::from_axis_angle(vis.hinge, -s.state.deflection);
        if t.rotation != rot {
            t.rotation = rot;
        }
    }
    for (parent, vis, mut t) in &mut props {
        let Ok(p) = engines.get(parent.parent()) else {
            continue;
        };
        let Some(e) = p.engines.get(vis.index) else {
            continue;
        };
        t.rotation = vis.base * Quat::from_rotation_z(e.spin_angle);
    }
    for (pivot, part, mut t) in &mut parts {
        let Ok(body) = pivots.get(pivot.parent()) else {
            continue;
        };
        let Some(e) = engines
            .get(body.parent())
            .ok()
            .and_then(|p| p.engines.get(part.index))
        else {
            continue;
        };
        let shown = (e.level > PROP_BLUR_LEVEL) == part.blur;
        let scale = if shown { Vec3::ONE } else { Vec3::splat(1e-3) };
        if t.scale != scale {
            t.scale = scale;
        }
    }
}

/// Exhaust plumes: length follows thrust, with a little flicker. Hidden while an effects
/// module draws the [`engine_world::Exhausts`] instead.
pub fn plume_visuals(
    time: Res<Time>,
    effects: Option<Res<ExhaustEffects>>,
    ships: Query<&Spacecraft>,
    mut q: Query<(&ChildOf, &PlumeVisual, &mut Transform, &mut MeshRenderer)>,
) {
    let flicker_t = time.elapsed_secs();
    for (parent, vis, mut t, mut r) in &mut q {
        if effects.is_some() {
            if r.visible {
                r.visible = false;
            }
            continue;
        }
        let Ok(ship) = ships.get(parent.parent()) else {
            continue;
        };
        let level = ship.thrusters.get(vis.index).map_or(0.0, |t| t.level);
        let on = level > 0.02;
        if r.visible != on {
            r.visible = on;
        }
        if on {
            let flicker = 1.0 + 0.06 * (flicker_t * 37.0 + vis.index as f32 * 1.7).sin();
            let len = vis.length * level.sqrt() * flicker;
            t.scale = Vec3::new(vis.radius, vis.radius, len.max(0.01));
        }
    }
}

/// Brake and reverse lights.
pub fn light_visuals(
    vehicles: Query<(&VehicleControls, Option<&Drivetrain>)>,
    mut q: Query<(&ChildOf, &BrakeLight, &mut MeshRenderer)>,
) {
    for (parent, light, mut r) in &mut q {
        let Ok((controls, drivetrain)) = vehicles.get(parent.parent()) else {
            continue;
        };
        let reversing = drivetrain.is_some_and(|d| d.state.gear < 0);
        let lit = if light.reverse {
            reversing
        } else if reversing {
            controls.throttle > 0.1
        } else {
            controls.brake > 0.1 || controls.handbrake
        };
        let mat = if lit { light.on } else { light.off };
        let mat = engine_render::MaterialRef::from(mat);
        if r.material != mat {
            r.material = mat;
        }
    }
}

/// Hulls glowing dull red to white-hot with heat-shield temperature.
pub fn heat_visuals(
    shields: Query<&HeatShield>,
    mut q: Query<(&ChildOf, &mut HeatGlow, &mut MeshRenderer)>,
) {
    for (parent, mut glow, mut r) in &mut q {
        let Ok(shield) = shields.get(parent.parent()) else {
            continue;
        };
        let t = 288.0 + (shield.temperature_k - 288.0) * glow.share;
        let (color, strength) = blackbody(t);
        // 24 steps on a log scale keep the number of distinct materials small.
        const STEPS: f32 = 24.0;
        const MAX_LOG: f32 = 6.0;
        let level = ((strength + 1.0).log10() / MAX_LOG * STEPS).clamp(0.0, STEPS) as u8;
        if level == glow.level {
            continue;
        }
        glow.level = level;
        let mut m = glow.base;
        if level > 0 {
            let s = 10f32.powf(level as f32 / STEPS * MAX_LOG) - 1.0;
            m = m.with_emissive(color, s);
        }
        r.material = m.into();
    }
}

/// Re-entry plasma: shown once the heat flux makes the air glow, brighter (and the wake
/// longer) as it grows; the wake trails along the airflow.
pub fn plasma_visuals(
    shields: Query<&HeatShield>,
    mut q: Query<(
        &ChildOf,
        &mut PlasmaVisual,
        &mut MeshRenderer,
        &mut Transform,
    )>,
) {
    for (parent, mut plasma, mut r, mut t) in &mut q {
        let Ok(shield) = shields.get(parent.parent()) else {
            continue;
        };
        let glow = plasma_glow(shield.heat_flux_w_m2);
        let flow = shield.flow_local;
        // 16 steps on a log scale, from 1% to 150% of full, keep materials few.
        const STEPS: f32 = 16.0;
        let level = if glow > 0.0 && flow != Vec3::ZERO {
            1 + ((glow / 0.01).log10() / 2.18 * STEPS).clamp(0.0, STEPS) as u8
        } else {
            0
        };
        if let Some(wake) = plasma.wake
            && level > 0
        {
            // The main wake leaves from the ship's far side along the flow (on an ellipsoid
            // around its skin), streamers from their edge; both grow with the glow.
            let from = wake
                .from
                .unwrap_or_else(|| flow * (wake.skin * flow).length() * 0.45);
            let new = Transform {
                translation: from.as_dvec3(),
                rotation: Quat::from_rotation_arc(Vec3::Z, flow),
                scale: Vec3::new(wake.radius, wake.radius, wake.length * glow.sqrt()),
            };
            if *t != new {
                *t = new;
            }
        }
        if level == plasma.level {
            continue;
        }
        plasma.level = level;
        r.visible = level > 0;
        if level > 0 {
            let s = 0.01 * 10f32.powf((level - 1) as f32 / STEPS * 2.18);
            let mut m = plasma.base;
            m.emissive_strength *= s;
            r.material = m.into();
        }
    }
}

/// How brightly the air glows at a nose heat flux, 0 (none) to 1.5 (relative to
/// [`PLASMA_FULL_FLUX`]): nothing below 5% of full, then rising with the flux.
pub fn plasma_glow(heat_flux_w_m2: f32) -> f32 {
    let x = heat_flux_w_m2 / PLASMA_FULL_FLUX;
    if x < 0.05 {
        0.0
    } else {
        ((x - 0.05) / 0.95).clamp(0.0, 1.5)
    }
}

//! Moving parts: wheels that steer, spin and ride the suspension, control surfaces that
//! deflect, propellers that spin, exhaust plumes that grow with thrust, brake lights, and hulls
//! that glow with re-entry heat. All are child entities of the vehicle, updated in `Update`
//! from the simulation state, so they work with any physics backend.

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, StandardMaterial};
use engine_world::VehicleControls;

use crate::aero::{Aircraft, Propulsion};
use crate::drivetrain::Drivetrain;
use crate::ship::{HeatShield, Spacecraft, blackbody};
use crate::wheels::Wheels;

/// Draws wheel `index` of the parent vehicle at its suspension position, steering and spin.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WheelVisual {
    pub index: usize,
    /// Hide when the aircraft retracts its gear.
    pub retracts: bool,
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
}

/// Wheels: position along the suspension, steering, spin; retract with aircraft gear.
pub fn wheel_visuals(
    vehicles: Query<(&Wheels, Option<&Aircraft>)>,
    mut q: Query<(&ChildOf, &WheelVisual, &mut Transform)>,
) {
    for (parent, vis, mut t) in &mut q {
        let Ok((wheels, aircraft)) = vehicles.get(parent.parent()) else {
            continue;
        };
        let Some(w) = wheels.wheels.get(vis.index) else {
            continue;
        };
        let st = &w.state;
        let travel = if st.grounded {
            st.spring_length
        } else {
            w.suspension.travel_m
        };
        // Retracted gear folds away (scaled down with everything attached to it).
        let shown = !vis.retracts || aircraft.is_none_or(|a| a.gear_down);
        let new = Transform {
            translation: (w.mount - Vec3::Y * travel).as_dvec3(),
            rotation: Quat::from_rotation_y(-st.steer) * Quat::from_rotation_x(-st.angle),
            scale: if shown { Vec3::ONE } else { Vec3::splat(1e-3) },
        };
        if *t != new {
            *t = new;
        }
    }
}

/// Control surfaces and propellers.
pub fn aircraft_visuals(
    aircraft: Query<&Aircraft>,
    engines: Query<&Propulsion>,
    mut surfaces: Query<(&ChildOf, &SurfaceVisual, &mut Transform), Without<PropellerVisual>>,
    mut props: Query<(&ChildOf, &PropellerVisual, &mut Transform), Without<SurfaceVisual>>,
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
}

/// Exhaust plumes: length follows thrust, with a little flicker.
pub fn plume_visuals(
    time: Res<Time>,
    ships: Query<&Spacecraft>,
    mut q: Query<(&ChildOf, &PlumeVisual, &mut Transform, &mut MeshRenderer)>,
) {
    let flicker_t = time.elapsed_secs();
    for (parent, vis, mut t, mut r) in &mut q {
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
        let (color, strength) = blackbody(shield.temperature_k);
        // 24 steps on a log scale keep the number of distinct materials small.
        const STEPS: f32 = 24.0;
        const MAX_LOG: f32 = 4.8;
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

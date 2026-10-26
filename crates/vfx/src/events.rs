//! Effects that play by themselves when something happens in the world.
//!
//! The vfx module never talks to physics, animation or weapons directly. It reads the shared
//! messages in `engine_world` instead:
//!
//! - [`GroundContact`]: footsteps and wheels kick up dust, mud, snow or water depending on the
//!   surface under them, puddles and snow cover (`step.*`).
//! - [`ProjectileTrace`]: shots splash where they cross water and chip the ground where they
//!   land, using the surface material's `impact_effect` (`impact.*`).
//! - [`WorldImpulse`]: explosions and heavy landings raise a dust ring or a water burst.
//! - [`WaterSplash`]: anything the water module sees breaking the surface throws spray,
//!   sized by the water it displaced.
//!
//! [`VfxReactions`] tunes or switches each of these off. A per-frame budget keeps a crowd
//! running across gravel from spawning hundreds of effects at once.

use std::collections::HashMap;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::{
    ContactKind, Env, FocusPoints, GroundContact, LightningStrike, ProjectileTrace,
    SimulationRange, WaterSplash, WorldImpulse,
};

use crate::emitter::ParticleEffect;
use crate::library::EffectLibrary;

/// Which world events play effects automatically, and how many per frame.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VfxReactions {
    /// Footsteps, paws, hooves and wheels (`GroundContact`).
    pub footsteps: bool,
    /// Bullet and arrow hits and water crossings (`ProjectileTrace`).
    pub projectiles: bool,
    /// Explosions and shockwaves (`WorldImpulse`).
    pub impulses: bool,
    /// Things breaking the water surface (`WaterSplash`).
    pub splashes: bool,
    /// Where cloud-to-ground lightning lands (`LightningStrike`).
    pub lightning: bool,
    /// Most reaction effects started per frame.
    pub max_per_frame: u32,
    /// Shortest time between two footstep effects from the same entity, seconds.
    pub step_interval: f32,
    /// Footstep effect size multiplier.
    pub step_scale: f32,
}

impl Default for VfxReactions {
    fn default() -> Self {
        Self {
            footsteps: true,
            projectiles: true,
            impulses: true,
            splashes: true,
            lightning: true,
            max_per_frame: 32,
            step_interval: 0.12,
            step_scale: 1.0,
        }
    }
}

/// Spawn a one-shot effect at `position`, its up axis along `up`. The entity despawns itself
/// once the effect has finished.
pub fn spawn_effect_at(
    commands: &mut Commands,
    effect: &str,
    position: DVec3,
    up: Vec3,
    scale: f32,
) -> Entity {
    let rotation = Quat::from_rotation_arc(Vec3::Y, up.normalize_or(Vec3::Y));
    commands
        .spawn((
            Transform::from_translation(position).with_rotation(rotation),
            ParticleEffect::once(effect).with_scale(scale),
        ))
        .id()
}

/// The first name in `names` the library has.
fn pick<'a>(library: &EffectLibrary, names: &[&'a str]) -> Option<&'a str> {
    names.iter().copied().find(|n| library.contains(n))
}

/// The footstep effect for a contact, from puddles, snow, water and the ground's material.
pub fn footstep_effect(
    env: &Env,
    library: &EffectLibrary,
    contact: &GroundContact,
) -> Option<String> {
    let p = contact.position;
    if let Some(w) = env.water(p)
        && w.surface_height > p.y + 0.02
    {
        return pick(library, &["step.water", "water.splash_small"]).map(str::to_string);
    }
    let state = env.surface_state(p);
    if state.puddle_m > 0.005 {
        return pick(library, &["step.puddle", "step.water"]).map(str::to_string);
    }
    if state.snow_m > 0.02 {
        return pick(library, &["step.snow"]).map(str::to_string);
    }
    let surface = contact
        .surface
        .or_else(|| env.ground(p).map(|g| g.surface))
        .unwrap_or_default();
    let name = &env.material(surface).name;
    let own = format!("step.{name}");
    if library.contains(&own) {
        return Some(own);
    }
    match name.as_str() {
        "grass" => pick(library, &["step.leaves"]),
        "rock" | "concrete" | "metal" | "wood" | "glass" | "ice" | "fabric" | "flesh" => None,
        _ => pick(library, &["step.default", "step.dirt"]),
    }
    .map(str::to_string)
}

/// The impact effect for a hit on `surface`'s material.
fn impact_effect(
    env: &Env,
    library: &EffectLibrary,
    surface: engine_world::SurfaceId,
) -> Option<String> {
    let material = env.material(surface);
    if library.contains(&material.impact_effect) {
        return Some(material.impact_effect.clone());
    }
    let own = format!("impact.{}", material.name);
    if library.contains(&own) {
        return Some(own);
    }
    pick(library, &["impact.default", "impact.dirt"]).map(str::to_string)
}

/// Play effects for this frame's footsteps, projectile hits and shockwaves.
#[allow(clippy::too_many_arguments)]
pub fn react_to_world(
    mut commands: Commands,
    time: Res<Time>,
    env: Env,
    library: Res<EffectLibrary>,
    reactions: Res<VfxReactions>,
    mut contacts: MessageReader<GroundContact>,
    mut traces: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut last_step: Local<HashMap<Entity, f32>>,
    range: Option<Res<SimulationRange>>,
) {
    let now = time.elapsed_secs();
    let mut budget = reactions.max_per_frame;
    // Nothing plays beyond the simulation range of every focus: no one is there to see it,
    // and looking up the ground for it is not free.
    let far = |p: DVec3| {
        range
            .as_deref()
            .is_some_and(|r| r.is_resting(&env.focus, p))
    };

    for impulse in impulses.read() {
        if !reactions.impulses || budget == 0 || impulse.strength < 2.0 || far(impulse.center) {
            continue;
        }
        let c = impulse.center;
        let scale = (impulse.radius / 4.0).clamp(0.4, 4.0);
        // Over water the water module reports the splash (see `react_to_splashes`).
        if let Some(w) = env.water(c)
            && (c.y - w.surface_height).abs() < impulse.radius as f64 * 0.5
        {
            continue;
        }
        if let Some(g) = env.ground(c)
            && c.y - g.height < impulse.radius as f64 * 0.6
            && let Some(name) = pick(&library, &["impulse.dust"])
        {
            let at = DVec3::new(c.x, g.height, c.z);
            spawn_effect_at(&mut commands, name, at, g.normal, scale);
            budget -= 1;
        }
    }

    for trace in traces.read() {
        if !reactions.projectiles || budget == 0 || far(trace.to) {
            continue;
        }
        // Into water: the water module reports the splash (see `react_to_splashes`).
        if let Some(w) = env.water(trace.to)
            && trace.from.y > w.surface_height
            && trace.to.y < w.surface_height
        {
            continue;
        }
        // Landing on the ground.
        if let Some(g) = env.ground(trace.to)
            && (trace.to.y - g.height).abs() < 0.35
            && let Some(name) = impact_effect(&env, &library, g.surface)
        {
            let at = DVec3::new(trace.to.x, g.height, trace.to.z);
            let scale = (trace.radius * 4.0).clamp(0.4, 2.5);
            spawn_effect_at(&mut commands, &name, at, g.normal, scale);
            budget -= 1;
        }
    }

    for contact in contacts.read() {
        if !reactions.footsteps || budget == 0 || far(contact.position) {
            continue;
        }
        if let Some(e) = contact.entity {
            let last = last_step.get(&e).copied().unwrap_or(f32::NEG_INFINITY);
            if now - last < reactions.step_interval {
                continue;
            }
            last_step.insert(e, now);
        }
        let rolling = matches!(contact.kind, ContactKind::Wheel | ContactKind::Track);
        if rolling && contact.speed < 3.0 {
            continue;
        }
        let Some(name) = footstep_effect(&env, &library, contact) else {
            continue;
        };
        let length = contact.size[0].max(contact.size[1]).max(0.05);
        let weight = (contact.load_kg / 80.0).clamp(0.3, 4.0).sqrt();
        let scale = ((length / 0.28) * weight).clamp(0.4, 3.0) * reactions.step_scale.max(0.0);
        let rate = (0.6 + contact.speed / 5.0).clamp(0.5, 2.5);
        let rotation = Quat::from_rotation_arc(Vec3::Y, contact.normal.normalize_or(Vec3::Y))
            * Quat::from_rotation_y(-contact.yaw);
        commands.spawn((
            Transform::from_translation(contact.position).with_rotation(rotation),
            ParticleEffect::once(name).with_scale(scale).with_rate(rate),
        ));
        budget -= 1;
    }

    // Forget entities that have not stepped for a while.
    if last_step.len() > 512 {
        last_step.retain(|_, t| now - *t < 5.0);
    }
}

/// Which effect a splash plays (first that exists), its scale and its up direction.
///
/// Water thrown up hard and in bulk is a blast's column; thrown sideways it is spray off a
/// bow; thrown gently up it is mist (a waterfall's foot). Anything going in splashes: a
/// small splash for drops and bullets, the big one with its crown for bodies.
fn splash_look(s: &WaterSplash) -> (&'static [&'static str], f32, Vec3) {
    let rising = s.velocity.y > 0.5;
    let sideways = s.velocity.with_y(0.0).length();
    if rising && s.mass_kg >= 50.0 {
        // The column's height is the throw's (v²/2g); the effect is 10 m tall at scale 1
        // and its speeds scale with it, so the height goes with scale².
        let height = (s.velocity.y * s.velocity.y / 19.62).clamp(2.0, 60.0);
        let names: &[&str] = &["water.column", "water.splash"];
        return (names, (height / 10.0).sqrt(), Vec3::Y);
    }
    if rising && sideways > 3.0 {
        let names: &[&str] = &["water.bow_spray", "water.splash_small"];
        let scale =
            (s.velocity.length() / 6.0).clamp(0.5, 2.5) * s.mass_kg.clamp(0.5, 8.0).sqrt() / 1.4;
        return (names, scale, s.velocity.normalize());
    }
    if rising {
        let names: &[&str] = &["water.mist"];
        let scale = (s.radius / 0.75).clamp(0.3, 4.0);
        return (names, scale, Vec3::Y + s.velocity.with_y(0.0) * 0.1);
    }
    let names: &[&str] = if s.mass_kg < 2.0 {
        &["water.splash_small", "water.splash"]
    } else {
        &["water.splash", "water.splash_small"]
    };
    let scale = (s.radius / 0.5).clamp(0.3, 5.0) * (s.mass_kg / 20.0).clamp(0.5, 2.0).sqrt();
    (names, scale, Vec3::Y - s.velocity.with_y(0.0) * 0.05)
}

/// Flash, sparks and blasted soil where a cloud-to-ground strike lands, at the end of the
/// same channel the renderer draws and audio rolls thunder along (`LightningStrike::path`).
pub fn react_to_lightning(
    mut commands: Commands,
    env: Env,
    library: Res<EffectLibrary>,
    reactions: Res<VfxReactions>,
    mut strikes: MessageReader<LightningStrike>,
) {
    for strike in strikes.read() {
        if !reactions.lightning || !strike.cloud_to_ground {
            continue;
        }
        let Some(name) = pick(
            &library,
            &["lightning.ground_strike", "spell.lightning_hit"],
        ) else {
            continue;
        };
        // The channel's last point is the strike point; a coarse path is enough for it.
        let at = strike.path(2).last().copied().unwrap_or(strike.to);
        let up = env.ground(at).map_or(Vec3::Y, |g| g.normal);
        let scale = (strike.peak_current_ka.max(1.0) / 30.0)
            .sqrt()
            .clamp(0.5, 2.5);
        spawn_effect_at(&mut commands, name, at, up, scale);
    }
}

/// Spray where the water module reports something broke the surface.
pub fn react_to_splashes(
    mut commands: Commands,
    library: Res<EffectLibrary>,
    reactions: Res<VfxReactions>,
    mut splashes: MessageReader<WaterSplash>,
    (range, focus): (Option<Res<SimulationRange>>, Option<Res<FocusPoints>>),
) {
    let mut budget = reactions.max_per_frame / 2;
    for s in splashes.read() {
        let far = match (range.as_deref(), focus.as_deref()) {
            (Some(r), Some(f)) => r.is_resting(f, s.position),
            _ => false,
        };
        if !reactions.splashes || budget == 0 || s.mass_kg <= 0.0 || far {
            continue;
        }
        let (names, scale, up) = splash_look(s);
        let Some(name) = pick(&library, names) else {
            continue;
        };
        spawn_effect_at(&mut commands, name, s.position, up, scale);
        budget -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::{SurfaceCell, SurfaceLayers, SurfaceMaterials, WorldPlugin};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(WorldPlugin)
            .insert_resource(EffectLibrary::with_builtins())
            .init_resource::<VfxReactions>()
            .add_systems(
                Update,
                (react_to_world, react_to_splashes, react_to_lightning),
            );
        app
    }

    fn effects(app: &mut App) -> Vec<String> {
        let mut q = app.world.query::<&ParticleEffect>();
        let mut v: Vec<String> = q.iter(&app.world).map(|e| e.effect.clone()).collect();
        v.sort();
        v
    }

    fn step(app: &mut App, surface: &str, x: f64, entity: Option<Entity>) {
        let surface = app.world.resource::<SurfaceMaterials>().id(surface);
        app.world.write_message(GroundContact {
            position: DVec3::new(x, 0.0, 0.0),
            normal: Vec3::Y,
            size: [0.12, 0.28],
            yaw: 0.0,
            load_kg: 80.0,
            speed: 3.0,
            kind: ContactKind::Foot,
            surface,
            entity,
        });
    }

    #[test]
    fn footsteps_pick_effects_by_surface() {
        let mut app = app();
        step(&mut app, "mud", 0.0, None);
        step(&mut app, "snow", 1.0, None);
        step(&mut app, "rock", 2.0, None);
        app.update();
        assert_eq!(effects(&mut app), vec!["step.mud", "step.snow"]);
    }

    #[test]
    fn puddles_and_snow_cover_win_over_the_material() {
        let mut app = app();
        for x in -1..=1 {
            for z in -1..=1 {
                let p = DVec3::new(x as f64, 0.0, z as f64);
                app.world
                    .resource_mut::<SurfaceLayers>()
                    .modify(p, |c: &mut SurfaceCell| c.puddle_m = 0.03);
            }
        }
        step(&mut app, "dirt", 0.0, None);
        app.update();
        assert_eq!(effects(&mut app), vec!["step.puddle"]);
    }

    #[test]
    fn one_entity_steps_at_most_every_interval() {
        let mut app = app();
        let walker = app.world.spawn_empty().id();
        step(&mut app, "dirt", 0.0, Some(walker));
        step(&mut app, "dirt", 0.1, Some(walker));
        app.update();
        assert_eq!(effects(&mut app).len(), 1);
    }

    #[test]
    fn the_frame_budget_caps_reactions() {
        let mut app = app();
        app.world.resource_mut::<VfxReactions>().max_per_frame = 3;
        for i in 0..10 {
            step(&mut app, "sand", i as f64, None);
        }
        app.update();
        assert_eq!(effects(&mut app).len(), 3);
    }

    #[test]
    fn water_splashes_throw_spray() {
        let mut app = app();
        app.world.write_message(WaterSplash {
            position: DVec3::new(0.0, 0.0, 0.0),
            velocity: Vec3::new(0.0, -5.0, 0.0),
            radius: 0.4,
            mass_kg: 30.0,
            source: None,
        });
        app.update();
        assert_eq!(effects(&mut app), vec!["water.splash"]);
    }

    #[test]
    fn shots_chip_the_ground_with_its_material() {
        let mut app = app();
        app.world.write_message(ProjectileTrace {
            from: DVec3::new(0.0, 1.5, -10.0),
            to: DVec3::new(0.0, 0.05, 0.0),
            radius: 0.05,
            speed: 400.0,
            shooter: None,
        });
        app.update();
        let names = effects(&mut app);
        assert_eq!(names.len(), 1);
        assert!(names[0].starts_with("impact."), "{names:?}");
    }

    #[test]
    fn each_kind_of_splash_gets_its_own_look() {
        let splash = |velocity: Vec3, radius: f32, mass_kg: f32| WaterSplash {
            position: DVec3::ZERO,
            velocity,
            radius,
            mass_kg,
            source: None,
        };
        let first = |s: WaterSplash| splash_look(&s).0[0];
        // What the water module sends: a body falling in, a bullet, an underwater charge,
        // spray off a boat's bow and mist at a waterfall's foot.
        assert_eq!(
            first(splash(Vec3::new(0.5, -6.0, 0.0), 0.4, 48.0)),
            "water.splash"
        );
        assert_eq!(
            first(splash(Vec3::new(300.0, -40.0, 0.0), 0.05, 0.4)),
            "water.splash_small"
        );
        assert_eq!(first(splash(Vec3::Y * 18.0, 3.0, 2000.0)), "water.column");
        assert_eq!(
            first(splash(Vec3::new(9.0, 2.5, 3.0), 0.6, 2.0)),
            "water.bow_spray"
        );
        assert_eq!(
            first(splash(Vec3::new(0.6, 3.0, 0.0), 2.0, 0.8)),
            "water.mist"
        );
        // Taller throws make taller columns.
        let (_, low, _) = splash_look(&splash(Vec3::Y * 10.0, 3.0, 500.0));
        let (_, high, _) = splash_look(&splash(Vec3::Y * 25.0, 3.0, 500.0));
        assert!(high > low * 2.0, "{low} vs {high}");
        for name in ["water.column", "water.bow_spray", "water.mist"] {
            assert!(EffectLibrary::with_builtins().contains(name), "{name}");
        }
    }

    #[test]
    fn lightning_that_reaches_the_ground_strikes_sparks_where_it_lands() {
        let mut app = app();
        let strike = |cloud_to_ground| LightningStrike {
            from: DVec3::new(30.0, 1500.0, -20.0),
            to: DVec3::new(4.0, 0.0, 7.0),
            peak_current_ka: 30.0,
            cloud_to_ground,
            seed: 7,
        };
        app.world.write_message(strike(false));
        app.update();
        assert!(
            effects(&mut app).is_empty(),
            "cloud-to-cloud stays in the sky"
        );
        app.world.write_message(strike(true));
        app.update();
        assert_eq!(effects(&mut app), ["lightning.ground_strike"]);
        let mut q = app.world.query::<(&ParticleEffect, &Transform)>();
        let (_, t) = q.iter(&app.world).next().unwrap();
        assert_eq!(t.translation, DVec3::new(4.0, 0.0, 7.0));
    }
}

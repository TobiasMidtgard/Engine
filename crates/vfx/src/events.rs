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
use engine_world::{ContactKind, Env, GroundContact, ProjectileTrace, WaterSplash, WorldImpulse};

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
) {
    let now = time.elapsed_secs();
    let mut budget = reactions.max_per_frame;

    for impulse in impulses.read() {
        if !reactions.impulses || budget == 0 || impulse.strength < 2.0 {
            continue;
        }
        let c = impulse.center;
        let scale = (impulse.radius / 4.0).clamp(0.4, 4.0);
        if let Some(w) = env.water(c)
            && (c.y - w.surface_height).abs() < impulse.radius as f64 * 0.5
        {
            if let Some(name) = pick(&library, &["water.splash"]) {
                let at = DVec3::new(c.x, w.surface_height, c.z);
                spawn_effect_at(&mut commands, name, at, w.normal, scale);
                budget -= 1;
            }
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
        if !reactions.projectiles || budget == 0 {
            continue;
        }
        // Crossing a water surface on the way.
        if let Some(w) = env.water(trace.to)
            && trace.from.y > w.surface_height
            && trace.to.y < w.surface_height
            && let Some(name) = pick(&library, &["water.splash_small", "water.splash"])
        {
            let t = (trace.from.y - w.surface_height) / (trace.from.y - trace.to.y);
            let at = trace.from.lerp(trace.to, t);
            spawn_effect_at(
                &mut commands,
                name,
                DVec3::new(at.x, w.surface_height, at.z),
                w.normal,
                0.6,
            );
            budget -= 1;
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
        if !reactions.footsteps || budget == 0 {
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

/// Spray where the water module reports something broke the surface.
pub fn react_to_splashes(
    mut commands: Commands,
    library: Res<EffectLibrary>,
    reactions: Res<VfxReactions>,
    mut splashes: MessageReader<WaterSplash>,
) {
    let mut budget = reactions.max_per_frame / 2;
    for s in splashes.read() {
        if !reactions.splashes || budget == 0 || s.mass_kg <= 0.0 {
            continue;
        }
        // Small things (drops, bullets, feet) get the small splash; bodies the big one.
        let names: &[&str] = if s.mass_kg < 2.0 {
            &["water.splash_small", "water.splash"]
        } else {
            &["water.splash", "water.splash_small"]
        };
        let Some(name) = pick(&library, names) else {
            continue;
        };
        let scale = (s.radius / 0.5).clamp(0.3, 5.0) * (s.mass_kg / 20.0).clamp(0.5, 2.0).sqrt();
        let up = Vec3::Y - s.velocity.with_y(0.0) * 0.05;
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
            .add_systems(Update, (react_to_world, react_to_splashes));
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
}

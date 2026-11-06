//! Spells drawn from the magic contracts (`engine_world::magic`): whatever writes a
//! [`SpellLook`] or sends a [`SpellCue`] gets the matching `spell.<look>.<role>` effect, with no
//! dependency on the module that cast it.
//!
//! - [`SpellLook`] on a carrier plays `spell.<look>.<role>` on it while the component lasts
//!   (bolt, field, aura, wall piece, status as `spell.status.<look>`); a `Charge` plays on a
//!   child at the caster's hand. Changing the look or scale restyles it; removing the component
//!   (or the entity) lets its particles fade.
//! - [`SpellCue`]: `Impact` plays `spell.<look>.impact` facing `normal`; `Cone` four gouts of
//!   `spell.<look>.cone` along `dir`, growing with the range; `Beam` a lightning, energy or
//!   smoke ribbon colored for the look; `Fizzle` `spell.<look>.fizzle`; `Mark` the mark.
//! - [`Statuses`] is left to the looks magic writes for it (status entities with role
//!   `Status`), so a body never burns twice.
//!
//! A look the library has no effect for (a weave without art yet) borrows the nearest
//! element's effect, recolored to the look's color, and defines it under the asked-for name
//! once, so later casts find it directly. Roles fall back too: aura to field, wall to field.
//!
//! Magic asks `vfx.spell_contracts` once and silences its JSON stand-ins when it answers.

use std::collections::HashMap;

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::magic::{SpellCue, SpellLook, SpellMark, SpellRole};

use crate::emitter::ParticleEffect;
use crate::library::EffectLibrary;
use crate::marks::{Mark, MarkKind, Marks};
use crate::trail::{Beam, RibbonStyle};
use crate::{VfxLifetime, spawn_effect_at};

/// Where a caster's hands gather a charge, in its own frame (x right, y up, −z forward): the
/// magic module's default hand.
pub const HAND: Vec3 = Vec3::new(0.25, 1.35, -0.45);

/// The elements, their signature colors (magic's `Element::color`) and whether they glow.
pub const ELEMENTS: [(&str, [f32; 3]); 6] = [
    ("fire", [1.0, 0.55, 0.2]),
    ("frost", [0.55, 0.78, 1.0]),
    ("storm", [0.62, 0.74, 1.0]),
    ("blood", [1.0, 0.07, 0.04]),
    ("blight", [0.38, 1.0, 0.16]),
    ("shadow", [0.55, 0.22, 1.0]),
];

/// The name a role is asked for by.
pub fn role_key(role: SpellRole) -> &'static str {
    match role {
        SpellRole::Bolt => "bolt",
        SpellRole::Field => "field",
        SpellRole::Wall => "wall",
        SpellRole::Aura => "aura",
        SpellRole::Charge => "charge",
        SpellRole::Status => "status",
    }
}

/// The effect name for a look in a role: `spell.<look>.<role>`, or `spell.status.<look>`.
pub fn look_name(look: &str, role: &str) -> String {
    if role == "status" {
        format!("spell.status.{look}")
    } else {
        format!("spell.{look}.{role}")
    }
}

/// The element whose effects a look without its own borrows: the nearest in hue, with dark
/// colors going to shadow and blood.
pub fn nearest_element(color: [f32; 3]) -> &'static str {
    let norm = |c: [f32; 3]| {
        let m = c[0].max(c[1]).max(c[2]).max(1e-3);
        [c[0] / m, c[1] / m, c[2] / m]
    };
    let c = norm(color);
    let mut best = ("shadow", f32::MAX);
    for (name, rgb) in ELEMENTS {
        let e = norm(rgb);
        let d = (0..3).map(|i| (c[i] - e[i]).powi(2)).sum::<f32>();
        if d < best.1 {
            best = (name, d);
        }
    }
    best.0
}

/// Effect names resolved so far: (look, role) → the effect to play.
#[derive(Resource, Debug, Default)]
pub struct SpellLooks {
    resolved: HashMap<(String, &'static str), Option<String>>,
}

impl SpellLooks {
    /// The effect to play for `look` in `role`, defining a recolored borrow when the library
    /// has none. None when not even the nearest element has the role.
    pub fn resolve(
        &mut self,
        library: &mut EffectLibrary,
        look: &str,
        role: &'static str,
        color: [f32; 3],
    ) -> Option<String> {
        let key = (look.to_string(), role);
        if let Some(hit) = self.resolved.get(&key) {
            return hit.clone();
        }
        let found = Self::find(library, look, role, color);
        self.resolved.insert(key, found.clone());
        found
    }

    fn find(
        library: &mut EffectLibrary,
        look: &str,
        role: &str,
        color: [f32; 3],
    ) -> Option<String> {
        let roles: &[&str] = match role {
            "aura" | "wall" => &[role, "field"],
            _ => std::slice::from_ref(&role),
        };
        for r in roles {
            let name = look_name(look, r);
            if library.contains(&name) {
                return Some(name);
            }
        }
        // Borrow the nearest element's, recolored, under the asked-for name.
        let element = nearest_element(color);
        for r in roles {
            let from = look_name(element, r);
            if let Some(base) = library.get(&from) {
                let mut desc = (**base).clone();
                desc.recolor(color, 0.7);
                desc.name = look_name(look, role);
                desc.description = format!(
                    "The {look} {role} look, borrowed from {from} and recolored until it has \
                     its own."
                );
                let name = desc.name.clone();
                return match library.insert(desc) {
                    Ok(()) => Some(name),
                    Err(e) => {
                        tracing::warn!("vfx: borrowed look {name} refused: {e}");
                        Some(from)
                    }
                };
            }
        }
        None
    }
}

/// On a carrier whose [`SpellLook`] vfx draws: what is playing and where (the carrier itself,
/// or a hand child for a charge).
#[derive(Component, Debug, Clone)]
pub struct DrawnSpellLook {
    pub effect: String,
    pub on: Entity,
}

/// Start, restyle and stop the looks on spell carriers.
pub fn draw_spell_looks(
    mut commands: Commands,
    mut library: ResMut<EffectLibrary>,
    mut looks: ResMut<SpellLooks>,
    changed: Query<(Entity, &SpellLook, Option<&DrawnSpellLook>), Changed<SpellLook>>,
    mut effects: Query<&mut ParticleEffect>,
    mut removed: RemovedComponents<SpellLook>,
    drawn: Query<&DrawnSpellLook>,
) {
    for e in removed.read() {
        let Ok(d) = drawn.get(e) else {
            continue;
        };
        stop(&mut commands, &mut effects, e, d);
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.try_remove::<DrawnSpellLook>();
        }
    }
    for (e, look, was) in &changed {
        let role = role_key(look.role);
        let Some(name) = looks.resolve(&mut library, &look.look, role, look.color) else {
            continue;
        };
        let scale = look.scale.clamp(0.05, 30.0);
        if let Some(d) = was {
            if d.effect == name
                && let Ok(mut fx) = effects.get_mut(d.on)
            {
                if fx.scale != scale {
                    fx.scale = scale;
                }
                fx.playing = true;
                continue;
            }
            stop(&mut commands, &mut effects, e, d);
        }
        let effect = ParticleEffect::new(name.clone()).with_scale(scale);
        let charge = look.role == SpellRole::Charge;
        // Magic may despawn a look the same frame it changes it (a bolt landing, a charge
        // released): then there is nothing to draw, so the command quietly does nothing.
        commands
            .entity(e)
            .queue_silenced(move |mut carrier: EntityWorldMut| {
                let on = if charge {
                    let parent = carrier.id();
                    carrier.world_scope(|w| {
                        w.spawn((
                            Name::new("Spell charge"),
                            Transform::from_translation(HAND.as_dvec3()),
                            effect,
                            ChildOf(parent),
                        ))
                        .id()
                    })
                } else {
                    carrier.insert(effect);
                    carrier.id()
                };
                carrier.insert(DrawnSpellLook { effect: name, on });
            });
    }
}

fn stop(
    commands: &mut Commands,
    effects: &mut Query<&mut ParticleEffect>,
    carrier: Entity,
    d: &DrawnSpellLook,
) {
    if let Ok(mut fx) = effects.get_mut(d.on) {
        fx.playing = false;
        if d.on != carrier {
            // A hand child: let its particles fade, then go.
            fx.despawn_when_done = true;
        }
    } else if d.on != carrier
        && let Ok(mut ec) = commands.get_entity(d.on)
    {
        ec.try_despawn();
    }
}

/// The ribbon a beam of `look` is drawn as, and its color.
pub fn beam_style(look: &str, color: [f32; 3]) -> (RibbonStyle, [f32; 3]) {
    match look {
        "storm" | "tempest" | "plasma" | "aether" | "blood_lightning" | "dark_thunder"
        | "acid_storm" | "thunderclap" => (RibbonStyle::Lightning, color),
        "shadow" | "blood" | "blight" | "plague" | "nightfeed" | "wither" | "doom" | "steam" => {
            (RibbonStyle::Smoke, color)
        }
        _ => (RibbonStyle::Energy, color),
    }
}

/// The mark kind a cue asks for.
pub fn mark_kind(m: SpellMark) -> MarkKind {
    match m {
        SpellMark::Scorch => MarkKind::Scorch,
        SpellMark::Frost => MarkKind::Frost,
        SpellMark::Wet => MarkKind::Wet,
        SpellMark::Blood => MarkKind::Blood,
        SpellMark::Rot => MarkKind::Rot,
        SpellMark::Void => MarkKind::Void,
        SpellMark::Burn => MarkKind::Burn,
    }
}

/// The color a cue's look is tinted to when it borrows (cues carry no color but Impact's).
fn look_color(look: &str) -> [f32; 3] {
    ELEMENTS
        .iter()
        .find(|(n, _)| *n == look)
        .map_or([0.8, 0.8, 1.0], |(_, c)| *c)
}

/// Play one-shot spell moments.
pub fn play_spell_cues(
    mut commands: Commands,
    mut cues: MessageReader<SpellCue>,
    mut library: ResMut<EffectLibrary>,
    mut looks: ResMut<SpellLooks>,
    mut marks: ResMut<Marks>,
) {
    for cue in cues.read() {
        match cue {
            SpellCue::Impact {
                at,
                normal,
                look,
                scale,
                color,
            } => {
                if let Some(name) = looks.resolve(&mut library, look, "impact", *color) {
                    // Magic sends its own Mark cue for ground hits; a hit on a body leaves none.
                    let rotation = Quat::from_rotation_arc(Vec3::Y, normal.normalize_or(Vec3::Y));
                    commands.spawn((
                        Transform::from_translation(*at).with_rotation(rotation),
                        ParticleEffect {
                            leave_mark: false,
                            ..ParticleEffect::once(name).with_scale(scale.clamp(0.05, 30.0))
                        },
                    ));
                }
            }
            SpellCue::Cone {
                at,
                dir,
                look,
                range_m,
            } => {
                let Some(name) = looks.resolve(&mut library, look, "cone", look_color(look)) else {
                    continue;
                };
                // Four gouts along the cone, growing as they go (one gout covers ~4 m at 1).
                let dir = dir.normalize_or(Vec3::Z);
                let range = range_m.max(0.5);
                for k in 1..=4 {
                    let f = k as f32 / 4.0;
                    let p = *at + (dir * range * f * 0.8).as_dvec3();
                    let s = (0.35 + 0.45 * f * (range / 4.0).max(1.0)).min(6.0);
                    spawn_effect_at(&mut commands, &name, p, dir, s);
                }
            }
            SpellCue::Beam {
                from,
                to,
                look,
                width_m,
                secs,
            } => {
                let (style, color) = beam_style(look, look_color(look));
                let emissive = if style == RibbonStyle::Smoke {
                    2000.0
                } else {
                    80_000.0
                };
                commands.spawn((
                    Transform::from_translation(*from),
                    Beam {
                        target: *to,
                        width: width_m.max(0.01),
                        color,
                        emissive,
                        style,
                        ..Default::default()
                    },
                    VfxLifetime(secs.max(0.05)),
                ));
            }
            SpellCue::Fizzle { at, look } => {
                let name = looks
                    .resolve(&mut library, look, "fizzle", look_color(look))
                    .unwrap_or_else(|| "spell.fizzle".into());
                spawn_effect_at(&mut commands, &name, *at, Vec3::Y, 0.6);
            }
            SpellCue::Mark {
                at,
                normal,
                kind,
                radius_m,
            } => {
                let kind = mark_kind(*kind);
                let mut mark = Mark::round(kind, *at, *normal, radius_m.max(0.05));
                mark.life_secs = 90.0;
                mark.glow_secs = if matches!(kind, MarkKind::Scorch | MarkKind::Burn) {
                    4.0
                } else {
                    0.0
                };
                marks.add(mark);
            }
        }
    }
}

/// What `vfx.spell_contracts` answers: vfx draws spells from the contracts.
pub fn contracts_answer() -> serde_json::Value {
    serde_json::json!({ "looks": true, "cues": true, "statuses": false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VfxPlugin;
    use engine_core::math::DVec3;
    use engine_core::registry::call_api;
    use engine_world::SpellLookDrawer;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(VfxPlugin);
        app
    }

    fn look(look: &str, role: SpellRole, scale: f32, color: [f32; 3]) -> SpellLook {
        SpellLook {
            look: look.into(),
            role,
            scale,
            color,
            power_j: 1000.0,
        }
    }

    #[test]
    fn vfx_says_it_draws_the_contracts() {
        let mut app = app();
        assert_eq!(
            app.world.resource::<SpellLookDrawer>().drawn_by,
            "vfx",
            "magic silences its stand-ins while this exists"
        );
        let r = call_api(&mut app.world, "vfx.spell_contracts", serde_json::json!({})).unwrap();
        assert_eq!(r["looks"], true);
        assert_eq!(r["cues"], true);
    }

    /// Magic may despawn a look in the same frame it appears (a bolt landing on its first
    /// frame, a charge released at once): drawing it must not crash the game.
    #[test]
    fn a_look_despawned_the_frame_it_appears_is_skipped() {
        fn land_at_once(mut commands: Commands, new: Query<Entity, Added<SpellLook>>) {
            for e in &new {
                commands.entity(e).despawn();
            }
        }
        let mut app = app();
        app.add_systems(
            Update,
            land_at_once.before_ignore_deferred(draw_spell_looks),
        );
        let caster = app
            .world
            .spawn((
                Transform::default(),
                look("fire", SpellRole::Charge, 1.0, [1.0, 0.5, 0.2]),
            ))
            .id();
        app.world.spawn((
            Transform::default(),
            look("frost", SpellRole::Bolt, 1.0, [0.5, 0.8, 1.0]),
        ));
        app.update();
        app.update();
        assert!(app.world.get_entity(caster).is_err());
        assert_eq!(
            app.world
                .query::<&ParticleEffect>()
                .iter(&app.world)
                .count(),
            0
        );
    }

    #[test]
    fn a_carrier_plays_its_look_restyles_and_lets_go() {
        let mut app = app();
        let bolt = app
            .world
            .spawn((
                Transform::default(),
                look("fire", SpellRole::Bolt, 1.5, [1.0, 0.5, 0.2]),
            ))
            .id();
        app.update();
        let fx = app
            .world
            .get::<ParticleEffect>(bolt)
            .expect("the bolt plays");
        assert_eq!(fx.effect, "spell.fire.bolt");
        assert_eq!(fx.scale, 1.5);

        app.world.get_mut::<SpellLook>(bolt).unwrap().look = "steam".into();
        app.update();
        assert_eq!(
            app.world.get::<ParticleEffect>(bolt).unwrap().effect,
            "spell.steam.bolt"
        );

        app.world.entity_mut(bolt).remove::<SpellLook>();
        app.update();
        assert!(!app.world.get::<ParticleEffect>(bolt).unwrap().playing);
    }

    #[test]
    fn a_charge_gathers_at_the_hand_and_walls_and_statuses_find_their_looks() {
        let mut app = app();
        let caster = app
            .world
            .spawn((
                Transform::default(),
                look("storm", SpellRole::Charge, 0.8, [0.6, 0.7, 1.0]),
            ))
            .id();
        let wall = app
            .world
            .spawn((
                Transform::default(),
                look("frost", SpellRole::Wall, 1.0, [0.5, 0.8, 1.0]),
            ))
            .id();
        let status = app
            .world
            .spawn((
                Transform::default(),
                look("burning", SpellRole::Status, 1.0, [1.0, 0.45, 0.1]),
            ))
            .id();
        app.update();
        let hand = app.world.get::<DrawnSpellLook>(caster).unwrap().on;
        assert_ne!(hand, caster);
        assert_eq!(
            app.world.get::<ParticleEffect>(hand).unwrap().effect,
            "spell.storm.charge"
        );
        assert_eq!(app.world.get::<ChildOf>(hand).unwrap().parent(), caster);
        assert_eq!(
            app.world.get::<ParticleEffect>(wall).unwrap().effect,
            "spell.frost.wall"
        );
        assert_eq!(
            app.world.get::<ParticleEffect>(status).unwrap().effect,
            "spell.status.burning"
        );

        app.world.entity_mut(caster).remove::<SpellLook>();
        app.update();
        let fx = app.world.get::<ParticleEffect>(hand).unwrap();
        assert!(
            !fx.playing && fx.despawn_when_done,
            "the charge fades and goes"
        );
    }

    #[test]
    fn a_weave_without_its_own_art_borrows_the_nearest_element_recolored() {
        let mut app = app();
        assert_eq!(nearest_element([0.65, 0.85, 0.1]), "blight");
        assert_eq!(nearest_element([1.0, 0.15, 0.05]), "blood");
        let e = app
            .world
            .spawn((
                Transform::default(),
                look("hellfire", SpellRole::Field, 1.0, [0.65, 0.85, 0.1]),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world.get::<ParticleEffect>(e).unwrap().effect,
            "spell.hellfire.field"
        );
        let lib = app.world.resource::<EffectLibrary>();
        let borrowed = lib.get("spell.hellfire.field").expect("defined once");
        assert!(borrowed.description.contains("spell.blight.field"));
    }

    #[test]
    fn cues_play_impacts_cones_beams_fizzles_and_marks() {
        let mut app = app();
        let before = app
            .world
            .query::<&ParticleEffect>()
            .iter(&app.world)
            .count();
        let at = DVec3::new(1.0, 0.0, 2.0);
        let msgs = [
            SpellCue::Impact {
                at,
                normal: Vec3::Y,
                look: "plasma".into(),
                scale: 1.0,
                color: [1.0, 0.6, 1.0],
            },
            SpellCue::Cone {
                at,
                dir: Vec3::X,
                look: "fire".into(),
                range_m: 6.0,
            },
            SpellCue::Beam {
                from: at,
                to: at + DVec3::X * 5.0,
                look: "storm".into(),
                width_m: 0.1,
                secs: 0.5,
            },
            SpellCue::Fizzle {
                at,
                look: "frost".into(),
            },
            SpellCue::Mark {
                at,
                normal: Vec3::Y,
                kind: SpellMark::Rot,
                radius_m: 1.0,
            },
        ];
        for m in msgs {
            app.world.write_message(m);
        }
        app.update();
        let names: Vec<String> = app
            .world
            .query::<&ParticleEffect>()
            .iter(&app.world)
            .map(|f| f.effect.clone())
            .collect();
        assert_eq!(names.len() - before, 1 + 4 + 1, "{names:?}");
        assert!(names.iter().any(|n| n == "spell.plasma.impact"));
        assert_eq!(names.iter().filter(|n| *n == "spell.fire.cone").count(), 4);
        assert!(names.iter().any(|n| n == "spell.frost.fizzle"));
        assert_eq!(app.world.query::<&Beam>().iter(&app.world).count(), 1);
        assert_eq!(
            app.world.resource::<Marks>().len(),
            1,
            "only the Mark cue marks: the impact may have hit a body"
        );
    }
}

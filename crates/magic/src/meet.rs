//! Spells that meet (Spellbreak, Magicka; research-games.md): a projectile flying through a
//! field of another element bursts into the pair's recipe and turns the field into it. A fire
//! bolt through a frost field bursts into steam and leaves a steam cloud; a storm bolt into
//! that steam makes Aether. Each field takes a few meetings at most, half a second apart, and
//! what the meeting makes never reacts again.

use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::SmokeEmission;

use crate::combine::{Recipe, recipe_for, weave};
use crate::hits::reaction_spell;
use crate::looks::{LookCmd, Role};
use crate::runtime::{
    CastInfo, Field, FieldKind, MAX_STAGE_DEPTH, MagicQueue, Missile, Spawn, SpellPos, field,
};

/// A field takes at most this many meetings.
pub const MAX_MEETS: u32 = 4;

/// Two spells met.
#[derive(Message, Debug, Clone, Copy)]
pub struct SpellsMet {
    pub at: DVec3,
    /// What they made (`steam`).
    pub recipe: &'static str,
    pub missile: Entity,
    pub field: Entity,
}

/// The recipe two looks make together, nesting (steam + storm = aether).
pub fn meeting(a: &str, b: &str) -> Option<&'static Recipe> {
    if a == b {
        return None;
    }
    let w = weave(&[a.to_string(), b.to_string()])?;
    if w.elements.len() < 2 {
        return None;
    }
    recipe_for(&w.ingredients)
}

/// Find missiles inside fields of another element and let them meet.
#[allow(clippy::type_complexity)]
pub fn meet_spells(
    mut missiles: Query<(Entity, &mut Missile, &SpellPos)>,
    mut fields: Query<(Entity, &mut Field, &SpellPos), Without<Missile>>,
    mut queue: ResMut<MagicQueue>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut met: MessageWriter<SpellsMet>,
) {
    if fields.is_empty() {
        return;
    }
    for (me, mut m, mp) in &mut missiles {
        if m.spent.is_some() || m.age < 0.0 || m.orbit.is_some() || m.cast.reaction {
            continue;
        }
        let Some(m_el) = m.cast.spell.element() else {
            continue;
        };
        let _ = m_el;
        for (fe, mut f, fp) in &mut fields {
            if f.spent.is_some()
                || f.kind != FieldKind::Lasting
                || f.cone.is_some()
                || f.meets >= MAX_MEETS
                || f.age - f.met_at < 0.5
                || f.cast.spell.element().is_none()
            {
                continue;
            }
            let d = mp.pos - fp.pos;
            let flat = DVec3::new(d.x, 0.0, d.z).length() as f32;
            let reach = match f.wall {
                Some((along, half)) => {
                    let v = d.as_vec3();
                    let a = v.dot(along).clamp(-half, half);
                    let off = v - along * a;
                    Vec3::new(off.x, 0.0, off.z).length() - f.radius
                }
                None => flat - f.radius,
            };
            if reach > 0.0 || d.y < -0.5 || d.y > 3.5 {
                continue;
            }
            let Some(r) = meeting(&f.cast.spell.weave.key, &m.cast.spell.weave.key) else {
                continue;
            };
            let at = mp.pos;
            let ground = DVec3::new(at.x, fp.pos.y, at.z);
            let child = |s: Arc<crate::compile::Spell>, cast: &CastInfo| CastInfo {
                spell: s,
                caster: cast.caster,
                side: cast.side,
                faction: cast.faction,
                depth: MAX_STAGE_DEPTH,
                scale: 1.0,
                seed: cast.seed.wrapping_add(4243),
                reaction: true,
            };
            // The burst where they met.
            let burst_r = (f.radius * 1.1).clamp(2.5, 8.0);
            if let Some(s) = reaction_spell(r, m.cast.spell.power, burst_r, 0.0, 0.9) {
                queue.spawns.push(Spawn::Field(
                    field(&child(s, &m.cast), FieldKind::Burst, burst_r, 0.0),
                    ground,
                ));
            }
            // The field turns into the recipe.
            let left = (f.duration - f.age).max(4.0);
            if let Some(s) = reaction_spell(r, f.cast.spell.power, f.radius, left, 0.35) {
                f.cast = child(s, &f.cast);
                f.duration = f.age + left;
                queue.looks.push(LookCmd::Attach {
                    entity: fe,
                    look: r.key.into(),
                    role: Role::Field,
                    scale: (f.radius / 2.0).clamp(0.2, 30.0),
                    color: r.color,
                    power_j: f.cast.spell.power.energy_j as f32,
                });
            }
            f.met_at = f.age;
            f.meets += 1;
            if r.reaction.cloud_m > 0.0 {
                smoke.write(SmokeEmission {
                    position: at,
                    velocity: Vec3::Y * 2.0,
                    radius: f.radius * 0.7,
                    mass_kg: 0.6 * f.radius,
                    temperature_c: if r.reaction.cloud_dark < 0.3 {
                        100.0
                    } else {
                        40.0
                    },
                    darkness: r.reaction.cloud_dark,
                });
            }
            met.write(SpellsMet {
                at,
                recipe: r.key,
                missile: me,
                field: fe,
            });
            m.spent = Some(0.0);
            queue.looks.push(LookCmd::Stop { entity: me });
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meetings_follow_the_table_and_nest() {
        assert_eq!(meeting("frost", "fire").map(|r| r.key), Some("steam"));
        assert_eq!(meeting("fire", "frost").map(|r| r.key), Some("steam"));
        assert_eq!(meeting("steam", "storm").map(|r| r.key), Some("aether"));
        assert_eq!(
            meeting("blood", "storm").map(|r| r.key),
            Some("blood_lightning")
        );
        assert!(meeting("fire", "fire").is_none());
    }
}

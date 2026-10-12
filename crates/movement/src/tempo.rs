//! Time perception: how fast characters act and how slowly a perceptive player sees the world.
//!
//! * [`Tempo::speed`] is how fast a character acts. It becomes the character's
//!   `LocalTimeScale` factor, so an elite with speed 2.5 runs, dashes, winds up and recovers
//!   2.5 times faster in world time. Nothing else needs to know.
//! * [`Tempo::reflexes`] is how fast the [`Perceiver`] (the player) perceives. When something
//!   attacks the perceiver, the world slows to `1 / reflexes` (**focus**) while the perceiver
//!   keeps moving at full speed (`LocalTimeScale::ignore_world_dilation`). An untrained player
//!   (reflexes 1) gets no help: an elite with speed 2.5 crosses ten meters and cuts them down
//!   in under a quarter of a second, faster than human reaction. At reflexes 5.5 the same
//!   dash plays out over a full second and can be dodged and countered.
//! * [`Cinematic`] adds short slow-motion beats when the player's blade lands, when an enemy
//!   falls, when the player is hit, and when an enemy the player outclasses swings at them.
//!
//! Everything is data: [`TimePerception`] and [`Tempo`] are patchable at runtime, and the
//! `movement.set_tempo` / `movement.perception` APIs change them from scripts or AI agents.

use engine_core::prelude::*;
use engine_world::{AttackEvent, DamageEvent};

use crate::combat::CombatState;

/// `TimeDilation` key used by focus (reflex slow motion).
pub const FOCUS_KEY: &str = "perception";
/// `TimeDilation` key used by cinematic slow-motion beats.
pub const CINEMATIC_KEY: &str = "cinematic";

/// How fast a character acts (`speed`) and perceives (`reflexes`). 1 = an ordinary person.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(LocalTimeScale)]
#[serde(default)]
pub struct Tempo {
    /// Multiplies the character's whole clock (movement, attacks, recovery). Written into
    /// its `LocalTimeScale::factor` every step.
    pub speed: f32,
    /// Perception. On the [`Perceiver`], attacks slow the world to `1 / reflexes`.
    pub reflexes: f32,
}

impl Default for Tempo {
    fn default() -> Self {
        Self {
            speed: 1.0,
            reflexes: 1.0,
        }
    }
}

impl Tempo {
    /// Stats for a character level (1 = untrained): every level adds 5% speed and half a
    /// point of reflexes, so level 10 has speed 1.45 and reflexes 5.5.
    pub fn level(level: u32) -> Self {
        let l = level.max(1) as f32 - 1.0;
        Self {
            speed: 1.0 + 0.05 * l,
            reflexes: 1.0 + 0.5 * l,
        }
    }

    /// The world time scale focus uses for this observer.
    pub fn focus_scale(&self, min: f32) -> f32 {
        (1.0 / self.reflexes.max(1e-3)).clamp(min.clamp(0.0, 1.0), 1.0)
    }

    /// How fast `attacker` looks to this observer while focused (1 = like an ordinary
    /// person at normal speed).
    pub fn apparent_speed(&self, attacker: &Tempo, min: f32) -> f32 {
        attacker.speed * self.focus_scale(min)
    }
}

/// The character whose senses drive world slow motion: normally the player. If several
/// entities have it, the first one found is used.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Perceiver;

/// A short slow-motion beat. `hold_secs` = 0 turns it off.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SlowMoPulse {
    /// World time scale at full strength.
    pub scale: f32,
    /// Real seconds at full strength.
    pub hold_secs: f32,
}

impl SlowMoPulse {
    pub const OFF: Self = Self {
        scale: 1.0,
        hold_secs: 0.0,
    };

    pub fn new(scale: f32, hold_secs: f32) -> Self {
        Self { scale, hold_secs }
    }

    fn active(&self) -> bool {
        self.hold_secs > 0.0 && self.scale < 1.0
    }
}

/// Cinematic slow-motion beats ("every swing looks like slow motion").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cinematic {
    pub enabled: bool,
    /// The perceiver's strike lands.
    pub player_hit: SlowMoPulse,
    /// The perceiver's strike defeats an enemy.
    pub player_kill: SlowMoPulse,
    /// The perceiver is hit.
    pub player_hurt: SlowMoPulse,
    /// An enemy the perceiver can read (its speed no higher than the perceiver's reflexes)
    /// starts an attack on the perceiver while focus is off.
    pub enemy_attack: SlowMoPulse,
}

impl Default for Cinematic {
    fn default() -> Self {
        Self {
            enabled: true,
            player_hit: SlowMoPulse::new(0.3, 0.15),
            player_kill: SlowMoPulse::new(0.12, 0.5),
            player_hurt: SlowMoPulse::new(0.35, 0.15),
            enemy_attack: SlowMoPulse::new(0.4, 0.2),
        }
    }
}

/// Settings for reflex slow motion and cinematic beats.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimePerception {
    pub enabled: bool,
    /// The slowest focus gets, however high the reflexes.
    pub min_scale: f32,
    /// Focus only engages when it would slow the world below this.
    pub engage_below: f32,
    /// Real seconds to ease into focus.
    pub blend_in_secs: f32,
    /// Real seconds focus stays on after the last threat is over.
    pub linger_secs: f32,
    /// Attacks without a target threaten the perceiver within this distance, m.
    pub threat_radius: f32,
    pub cinematic: Cinematic,
}

impl Default for TimePerception {
    fn default() -> Self {
        Self {
            enabled: true,
            min_scale: 0.1,
            engage_below: 0.95,
            blend_in_secs: 0.06,
            linger_secs: 0.25,
            threat_radius: 12.0,
            cinematic: Cinematic::default(),
        }
    }
}

/// What perception is doing right now (read-only; for UI, tests and AI agents).
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerceptionStatus {
    /// Entity bits of the perceiver.
    pub perceiver: Option<u64>,
    /// Focus (reflex slow motion) is on.
    pub focused: bool,
    /// World time scale focus asks for.
    pub focus_scale: f32,
    /// Attacks threatening the perceiver right now.
    pub threats: u32,
    /// How many times focus has engaged.
    pub focus_count: u32,
    /// How many cinematic beats have played, and the last one's name.
    pub cinematic_count: u32,
    pub last_cinematic: String,
    #[serde(skip)]
    linger: f32,
    /// (attacker, world time its hit lands)
    #[serde(skip)]
    pending: Vec<(Entity, f64)>,
}

/// Write each character's clock from its [`Tempo`]. The perceiver ignores world slow motion
/// while focused.
pub(crate) fn apply_tempo(
    dilation: Res<TimeDilation>,
    settings: Res<TimePerception>,
    mut q: Query<(&Tempo, &mut LocalTimeScale, Has<Perceiver>)>,
) {
    let focused = settings.enabled && dilation.is_active(FOCUS_KEY);
    for (tempo, mut lts, perceiver) in &mut q {
        let want = LocalTimeScale {
            factor: tempo.speed.max(0.0),
            ignore_world_dilation: perceiver && focused,
        };
        if *lts != want {
            *lts = want;
        }
    }
}

/// React to attacks on the perceiver (focus) and to hits (cinematic beats).
#[allow(clippy::too_many_arguments)]
pub(crate) fn perceive(
    time: Res<Time>,
    settings: Res<TimePerception>,
    mut dilation: ResMut<TimeDilation>,
    mut status: ResMut<PerceptionStatus>,
    mut diagnostics: Option<ResMut<Diagnostics>>,
    mut attacks: MessageReader<AttackEvent>,
    mut damage: MessageReader<DamageEvent>,
    perceivers: Query<(Entity, &Transform, Option<&Tempo>), With<Perceiver>>,
    others: Query<(&Transform, Option<&Tempo>, Option<&CombatState>)>,
) {
    let Some((me, my_t, my_tempo)) = perceivers.iter().next() else {
        attacks.clear();
        damage.clear();
        if status.focused {
            dilation.release(FOCUS_KEY);
            status.focused = false;
        }
        status.perceiver = None;
        status.pending.clear();
        status.threats = 0;
        return;
    };
    let my_tempo = my_tempo.copied().unwrap_or_default();
    let scale = my_tempo.focus_scale(settings.min_scale);
    let can_focus = settings.enabled && scale < settings.engage_below;
    let now = time.elapsed_secs_f64();
    let cine = settings.cinematic;
    status.perceiver = Some(me.to_bits());
    status.focus_scale = scale;

    // Cinematic beats, played after focus is updated.
    let mut beats: Vec<(&str, SlowMoPulse)> = Vec::new();

    for ev in attacks.read() {
        if ev.attacker == me {
            continue;
        }
        let attacker = others.get(ev.attacker).ok();
        let near = attacker.is_some_and(|(t, _, _)| {
            t.translation.distance(my_t.translation) <= settings.threat_radius as f64
        });
        if ev.target != Some(me) && !(ev.target.is_none() && near) {
            continue;
        }
        if can_focus {
            status
                .pending
                .push((ev.attacker, now + ev.windup_secs as f64));
        } else {
            let speed = attacker
                .and_then(|(_, t, _)| t.copied())
                .unwrap_or_default()
                .speed;
            if speed <= my_tempo.reflexes {
                beats.push(("enemy_attack", cine.enemy_attack));
            }
        }
    }

    // A threat lasts until its hit lands, and while its attacker is still mid-attack on the
    // perceiver (including the recovery, so counters play out in slow motion too).
    status.pending.retain(|(attacker, hit_at)| {
        let fighting = others.get(*attacker).ok().and_then(|(_, _, cs)| cs);
        if fighting.is_some_and(|cs| cs.defeated) {
            return false;
        }
        now < *hit_at + 0.05
            || fighting.is_some_and(|cs| cs.attack.is_some_and(|a| a.target == Some(me)))
    });
    status.pending.sort_by_key(|(e, _)| *e);
    status.pending.dedup_by_key(|(e, _)| *e);
    status.threats = status.pending.len() as u32;

    if status.threats > 0 && can_focus {
        if !status.focused {
            status.focus_count += 1;
        }
        dilation.hold(FOCUS_KEY, scale, settings.blend_in_secs);
        status.focused = true;
        status.linger = settings.linger_secs;
    } else if status.focused {
        status.linger -= time.real_delta_secs();
        if status.linger <= 0.0 || !can_focus {
            dilation.release(FOCUS_KEY);
            status.focused = false;
        }
    }

    for ev in damage.read() {
        if ev.source == Some(me) && ev.target != me && ev.amount > 0.0 {
            let killed = others
                .get(ev.target)
                .ok()
                .and_then(|(_, _, cs)| cs)
                .is_some_and(|cs| cs.defeated);
            if killed {
                beats.push(("player_kill", cine.player_kill));
            } else {
                beats.push(("player_hit", cine.player_hit));
            }
        } else if ev.target == me && ev.amount > 0.0 {
            beats.push(("player_hurt", cine.player_hurt));
        }
    }

    for (name, p) in beats {
        if cine.enabled && p.active() {
            dilation.pulse(CINEMATIC_KEY, p.scale, p.hold_secs);
            status.cinematic_count += 1;
            status.last_cinematic = name.to_string();
        }
    }

    if let Some(d) = diagnostics.as_deref_mut() {
        d.set("movement.time_scale", time.scale() as f64);
        d.set(
            "movement.focus",
            if status.focused { scale as f64 } else { 1.0 },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_raise_reflexes_and_speed() {
        assert_eq!(Tempo::level(1), Tempo::default());
        let ten = Tempo::level(10);
        assert!((ten.reflexes - 5.5).abs() < 1e-6);
        assert!((ten.speed - 1.45).abs() < 1e-6);
        assert!((ten.focus_scale(0.1) - 1.0 / 5.5).abs() < 1e-6);
        assert_eq!(Tempo::level(1).focus_scale(0.1), 1.0);
        // Reflexes beyond 1/min_scale don't slow the world further.
        assert_eq!(Tempo::level(100).focus_scale(0.1), 0.1);
    }

    #[test]
    fn an_elite_looks_slow_only_to_a_trained_eye() {
        let elite = Tempo {
            speed: 2.5,
            reflexes: 1.0,
        };
        assert_eq!(Tempo::level(1).apparent_speed(&elite, 0.1), 2.5);
        assert!(Tempo::level(10).apparent_speed(&elite, 0.1) < 0.5);
    }

    #[test]
    fn settings_round_trip() {
        let s = TimePerception::default();
        let json = serde_json::to_value(&s).unwrap();
        let back: TimePerception = serde_json::from_value(json).unwrap();
        assert_eq!(s, back);
    }
}

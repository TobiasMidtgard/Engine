//! `spells_craft`: the Spellcrafter at the hall's altar (round-7-plan.md, section 5).
//!
//! An Expert weaver stands at the altar on the dais and builds the design's example, a
//! returning frost glaive, through the same screen a player gets: frost (fire and frost weave
//! into steam in the title, then fire comes off before the crystal's first cast, so no steam
//! cloud hides the dummy), a bolt, boomerang, pierce 3, stacking, safeguard, a
//! split of three and speed, which fills an Expert's budget of 8. Seek is refused in red. The
//! crystal casts each change at the dummy in front of the altar a moment later. The spell is
//! learned, put on key 3 (the HUD's slot changes), the screen closes and the weaver casts it
//! from the bar.
//!
//! Each step is pressed through the UI (`ui.click` on the button's id, after `ui.focus` shows
//! which), so the film shows exactly what a player's clicks do.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use serde_json::json;

use super::art::{ALTAR_DUMMY, DAIS_TOP, MAGE_Z};
use super::player::Weaver;
use super::{Hall, Program, VIEWS};
use crate::caster::MagicCaster;
use crate::crafter::{self, Altar, id_of};
use crate::player::{CastInput, SpellBar, starter_spells};
use crate::schools::Mastery;

/// Seconds the script takes before it starts over.
pub const PERIOD: f32 = 21.0;
/// Where the weaver stands: at the altar's back left, clear of its crystal's line.
pub const STAND: [f64; 3] = [-0.7, DAIS_TOP, MAGE_Z + 0.85];
/// Seconds a button shows focus before it is pressed.
pub const FOCUS_SECS: f32 = 0.45;

/// One step of the script.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Do {
    /// A fresh weaver: the starter bar, nothing learned, the screen shut.
    Reset,
    /// Open the Spellcrafter at the altar.
    Open,
    /// Press a crafter button (`mod:boomerang`).
    Press(&'static str),
    /// Press (true) or release a spell bar slot, aimed at the altar's dummy.
    Slot(usize, bool),
}

/// The script, by seconds into the period.
pub const SCRIPT: [(f32, Do); 20] = [
    (0.0, Do::Reset),
    (0.3, Do::Open),
    // Frost: woven with the fire the screen starts with, the title shows steam...
    (1.2, Do::Press("effect:frost")),
    // ...then fire comes off, before the crystal's first cast.
    (2.0, Do::Press("effect:fire")),
    (3.0, Do::Press("step:cast")),
    (3.9, Do::Press("step:modifiers")),
    (4.8, Do::Press("mod:boomerang")),
    (5.7, Do::Press("mod:pierce")),
    (6.5, Do::Press("mod:stacking")),
    (7.3, Do::Press("mod:safeguard")),
    (8.1, Do::Press("mod:split")),
    (9.0, Do::Press("mod:speed")),
    // 10 against 8: refused in red.
    (10.3, Do::Press("mod:seek")),
    (12.2, Do::Press("learn")),
    (13.4, Do::Press("key:3")),
    (14.8, Do::Press("close")),
    (15.6, Do::Slot(2, true)),
    (15.7, Do::Slot(2, false)),
    (18.2, Do::Slot(2, true)),
    (18.3, Do::Slot(2, false)),
];

/// The weaver: an Expert in every subclass (a budget of 8) with the starter bar.
pub fn weaver_caster() -> MagicCaster {
    let mut caster = MagicCaster {
        learns: false,
        side: 1,
        seed: 4243,
        ..MagicCaster::new(60.0, Mastery::Expert)
    };
    for (d, g) in starter_spells() {
        caster = caster.learn(d, g);
    }
    caster.max_mana = 400.0;
    caster.mana = 400.0;
    caster.mana_regen = 6.0;
    caster
}

/// The stop's camera eye, from the hall's center.
pub fn eye() -> DVec3 {
    VIEWS
        .iter()
        .find(|v| v.key == Program::Craft.key())
        .map(|v| DVec3::from_array(v.eye))
        .unwrap_or(DVec3::new(2.2, 2.9, 14.2))
}

/// Play the script from the hall's clock.
#[allow(clippy::type_complexity)]
pub fn script_crafter(
    hall: Option<Res<Hall>>,
    mut weavers: Query<(
        Entity,
        &mut Weaver,
        &mut CastInput,
        &mut Transform,
        &mut MagicCaster,
        &mut SpellBar,
    )>,
    altars: Query<Entity, With<Altar>>,
    mut commands: Commands,
) {
    let Some(hall) = hall else {
        return;
    };
    if hall.program != Program::Craft || !hall.near || hall.restart {
        return;
    }
    let Some(altar) = altars.iter().next() else {
        return;
    };
    let origin = hall.origin;
    for (e, mut w, mut input, mut tf, mut caster, mut bar) in &mut weavers {
        if w.program != Program::Craft {
            continue;
        }
        // The hall's clock (it starts again with the program).
        let mut t0 = w.t;
        let t1 = hall.t;
        if t1 < t0 {
            t0 = -1e-3;
        }
        w.t = t1;
        let cycle = |t: f32| (t / PERIOD).floor();
        let local = |t: f32| t - cycle(t) * PERIOD;
        let (a, b) = (local(t0.max(0.0)), local(t1));
        let wrapped = cycle(t0.max(0.0)) != cycle(t1);
        let crossed = |at: f32| {
            if t0 < 0.0 && at == 0.0 {
                return true;
            }
            if wrapped {
                at >= a || at < b
            } else {
                at > a && at <= b
            }
        };
        for (at, step) in SCRIPT {
            // Buttons light up a moment before they are pressed.
            if let Do::Press(action) = step
                && crossed(at - FOCUS_SECS)
            {
                let id = id_of(action);
                commands.queue(move |world: &mut World| {
                    let _ = call_api(world, "ui.focus", json!({ "id": id }));
                });
            }
            if !crossed(at) {
                continue;
            }
            match step {
                Do::Reset => {
                    *caster = weaver_caster();
                    *bar = SpellBar::default();
                    *input = CastInput::default();
                    commands.queue(|world: &mut World| {
                        crafter::act_now(world, "close");
                    });
                }
                Do::Open => {
                    commands.queue(move |world: &mut World| {
                        crafter::request_open(world, e, Some(altar));
                    });
                }
                Do::Press(action) => {
                    commands.queue(move |world: &mut World| crafter::press(world, action));
                }
                Do::Slot(slot, true) => input.press(slot),
                Do::Slot(slot, false) => input.release(slot),
            }
        }
        // Aim along the camera at the altar's dummy; face where the spell goes.
        let target = origin + DVec3::from_array(ALTAR_DUMMY) + DVec3::Y * 1.2;
        let from = origin + eye();
        input.ray = Some((
            from,
            (target - from)
                .as_vec3()
                .try_normalize()
                .unwrap_or(Vec3::NEG_Z),
        ));
        let flat = (target - tf.translation).as_vec3();
        if flat.x * flat.x + flat.z * flat.z > 0.01 {
            let want = Quat::from_rotation_y((-flat.x).atan2(-flat.z));
            tf.rotation = tf.rotation.slerp(want, 0.2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crafter::{Crafter, apply};

    #[test]
    fn the_script_builds_the_glaive_to_the_budget_and_is_refused_once() {
        let mut caster = weaver_caster();
        let mut bar = SpellBar::default();
        let mut c = Crafter {
            design: crafter::starting_design(&caster),
            ..Default::default()
        };
        let mut refused = 0;
        for (_, step) in SCRIPT {
            if let Do::Press(a) = step {
                apply(&mut c, &mut caster, Some(&mut bar), a);
                if c.note.as_ref().is_some_and(|n| !n.ok) {
                    refused += 1;
                }
            }
        }
        assert_eq!(refused, 1, "only seek is refused");
        assert_eq!(c.design.complexity(), 8);
        let g = bar.slots[2].expect("on key 3");
        assert_eq!(caster.grimoire[g].design.name, "Frost glaives");
        assert!(SCRIPT.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(SCRIPT.last().unwrap().0 < PERIOD);
        // Each press has its focus moment after the step before.
        for w in SCRIPT.windows(2) {
            if matches!(w[1].1, Do::Press(_)) {
                assert!(w[1].0 - FOCUS_SECS > w[0].0, "{:?}", w[1]);
            }
        }
    }
}

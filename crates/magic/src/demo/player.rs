//! `spells_player`: the player's spell bar, seen over the shoulder (round-4-plan.md).
//!
//! A robed weaver stands on the hall floor with the starter bar and works through it against
//! the lane dummies: a charged fireball, a pillar placed with its landing ring, chain
//! lightning, a frost wall, a channelled flame jet, a stone grip, a steam flask and a mend.
//! A script presses and releases the slots and aims along the stop's camera ray, through the
//! same [`CastInput`] the keys write, so the picture shows exactly what a player gets: the HUD
//! hotbar with its cooldown sweeps and the mana bar going down and coming back.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::animation::BodyRequest;
use serde_json::json;

use super::art::{DUMMY_Z, FLOOR, LANES, mage_look};
use super::{Hall, Program, VIEWS};
use crate::caster::{MagicCaster, Vitality};
use crate::player::{CastInput, HudBar, SLOTS, ScriptedInput, SpellBar, hud_data, starter_spells};
use crate::schools::Mastery;

/// Where the weaver stands, from the hall's center.
pub const STAND: [f64; 3] = [0.0, FLOOR, 4.0];
/// Seconds the script takes before it starts over.
pub const PERIOD: f32 = 12.0;

/// The scripted weaver, its clock, and the program it is scripted for.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Weaver {
    pub t: f32,
    pub program: Program,
}

/// One step of the script: at `at` s, press (or release) `slot`, aiming at `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beat {
    pub at: f32,
    pub slot: usize,
    pub press: bool,
    /// From the hall's center.
    pub target: [f64; 3],
}

const fn beat(at: f32, slot: usize, press: bool, target: [f64; 3]) -> Beat {
    Beat {
        at,
        slot,
        press,
        target,
    }
}

const DUMMY_Y: f64 = FLOOR + 1.2;

/// The script, in slot order of the starter bar (0 Fireball, 1 Frost wall, 2 Pillar, 3 Chain
/// lightning, 4 Steam flask, 5 Flame jet, 6 Stone grip, 7 Mend).
pub const SCRIPT: [Beat; 16] = [
    // Hold a fireball for a full charge, then let it go at the dummy under the crosshair.
    beat(0.0, 0, true, [LANES[2], DUMMY_Y, DUMMY_Z]),
    beat(1.25, 0, false, [LANES[2], DUMMY_Y, DUMMY_Z]),
    // Place a pillar: the ring shows where while the key is held.
    beat(1.5, 2, true, [-6.5, FLOOR, -3.5]),
    beat(2.3, 2, false, [-6.5, FLOOR, -3.5]),
    // Chain lightning at the right dummy.
    beat(2.6, 3, true, [LANES[3], DUMMY_Y, DUMMY_Z]),
    beat(2.7, 3, false, [LANES[3], DUMMY_Y, DUMMY_Z]),
    // A frost wall across the right of the floor.
    beat(3.4, 1, true, [5.0, FLOOR, -2.5]),
    beat(4.6, 1, false, [5.0, FLOOR, -2.5]),
    // A flame jet held on the dummy under the crosshair.
    beat(5.3, 5, true, [LANES[2], DUMMY_Y, DUMMY_Z]),
    beat(7.4, 5, false, [LANES[2], DUMMY_Y, DUMMY_Z]),
    // Stone closes on the far-left dummy's feet.
    beat(7.8, 6, true, [LANES[1], DUMMY_Y, DUMMY_Z]),
    beat(7.9, 6, false, [LANES[1], DUMMY_Y, DUMMY_Z]),
    // A steam flask lobbed at the right dummy.
    beat(8.6, 4, true, [LANES[3], DUMMY_Y, DUMMY_Z]),
    beat(8.7, 4, false, [LANES[3], DUMMY_Y, DUMMY_Z]),
    // Mend.
    beat(9.6, 7, true, [0.0, DUMMY_Y, DUMMY_Z]),
    beat(9.7, 7, false, [0.0, DUMMY_Y, DUMMY_Z]),
];

/// The stop's camera eye, from the hall's center.
pub fn eye() -> DVec3 {
    VIEWS
        .iter()
        .find(|v| v.key == Program::Player.key())
        .map(|v| DVec3::from_array(v.eye))
        .unwrap_or(DVec3::new(0.9, 2.2, 7.0))
}

/// The weaver's caster: an Adept with the starter bar and mana enough for the script, filling
/// slowly enough that the HUD's mana bar shows each cast.
pub fn weaver_caster() -> MagicCaster {
    let mut caster = MagicCaster {
        learns: false,
        side: 1,
        seed: 4242,
        ..MagicCaster::new(40.0, Mastery::Adept)
    };
    for (d, g) in starter_spells() {
        caster = caster.learn(d, g);
    }
    caster.max_mana = 400.0;
    caster.mana = 400.0;
    caster.mana_regen = 6.0;
    caster
}

/// Spawn the weaver with the starter bar and a HUD bar: an Adept on the floor for the spell
/// bar, an Expert at the altar for the Spellcrafter.
pub fn spawn_weaver(commands: &mut Commands, origin: DVec3, program: Program) -> Entity {
    let (caster, stand, name) = if program == Program::Craft {
        (
            super::craft::weaver_caster(),
            super::craft::STAND,
            "Weaver (Spellcrafter)",
        )
    } else {
        (weaver_caster(), STAND, "Weaver (spell bar)")
    };
    commands
        .spawn((
            Name::new(name),
            Transform::from_translation(origin + DVec3::from_array(stand)),
            caster,
            Vitality::new(300.0),
            BodyRequest::human(4242),
            mage_look(7, Mastery::Adept, Some([0.7, 0.12, 0.12])),
            SpellBar::default(),
            CastInput::default(),
            ScriptedInput,
            HudBar,
            Weaver { t: 0.0, program },
        ))
        .id()
}

/// Spawn or remove the weaver as the program comes and goes, and show the HUD with it.
pub fn keep_weaver(
    hall: Option<Res<Hall>>,
    weavers: Query<(Entity, &Weaver)>,
    mut shown: Local<Option<Program>>,
    mut commands: Commands,
) {
    let Some(hall) = hall else {
        return;
    };
    let program = hall.program;
    let want = matches!(program, Program::Player | Program::Craft);
    let mut have = false;
    for (e, w) in &weavers {
        if want && w.program == program {
            have = true;
        } else {
            commands.entity(e).despawn();
        }
    }
    if want && !have {
        spawn_weaver(&mut commands, hall.origin, program);
    }
    let show = want.then_some(program);
    if show != *shown {
        *shown = show;
        commands.queue(move |world: &mut World| {
            let call = |world: &mut World, api: &str, args: serde_json::Value| {
                let _ = engine_core::registry::call_api(world, api, args);
            };
            let Some(program) = show else {
                call(world, "ui.hide", json!({"screen": "hud", "instant": true}));
                return;
            };
            let craft = program == Program::Craft;
            let caster = if craft {
                super::craft::weaver_caster()
            } else {
                weaver_caster()
            };
            // The hotbar takes its slot count from the data when the HUD is built, so the
            // bar goes in first.
            let mut values = hud_data(&SpellBar::default(), &caster);
            if let Some(v) = values.as_object_mut() {
                for (k, x) in [
                    ("player.name", json!("Weaver")),
                    ("player.level", json!(caster.level)),
                    ("player.health", json!(300)),
                    ("player.health_max", json!(300)),
                    ("player.shield", json!(0)),
                    ("player.shield_max", json!(0)),
                    ("player.speed", json!(0)),
                    (
                        "quest.title",
                        json!(if craft {
                            "The Spellcrafter"
                        } else {
                            "The spell bar"
                        }),
                    ),
                    (
                        "quest.step",
                        json!(if craft {
                            "Weave a returning frost glaive"
                        } else {
                            "Cast each spell once"
                        }),
                    ),
                    ("quest.progress", json!(0)),
                    ("quest.progress_max", json!(if craft { 8 } else { SLOTS })),
                ] {
                    v.insert(k.into(), x);
                }
            }
            call(world, "ui.set_data", json!({ "values": values }));
            call(
                world,
                "ui.show",
                json!({"screen": "hud", "portrait": "spellbook", "instant": true}),
            );
            // No gun and no speed readout for a weaver.
            for id in ["hud_weapon", "hud_time"] {
                call(world, "ui.despawn", json!({ "id": id }));
            }
        });
    }
}

/// The HUD's quest line counts the spells cast so far this round of the script.
/// At the altar it counts the spell's complexity against the budget of 8.
pub fn weaver_quest(
    hall: Option<Res<Hall>>,
    weavers: Query<&Weaver>,
    crafter: Option<Res<crate::crafter::Crafter>>,
    mut last: Local<Option<usize>>,
    mut commands: Commands,
) {
    let Some(program) = hall
        .map(|h| h.program)
        .filter(|p| matches!(p, Program::Player | Program::Craft))
    else {
        *last = None;
        return;
    };
    let Some(w) = weavers.iter().find(|w| w.program == program) else {
        return;
    };
    let n = if program == Program::Craft {
        match crafter.filter(|c| c.open) {
            Some(c) => c.design.complexity() as usize,
            None => last.unwrap_or(0),
        }
    } else {
        let t = w.t - (w.t / PERIOD).floor() * PERIOD;
        SCRIPT.iter().filter(|b| !b.press && b.at <= t).count()
    };
    if *last == Some(n) {
        return;
    }
    *last = Some(n);
    commands.queue(move |world: &mut World| {
        let _ = engine_core::registry::call_api(
            world,
            "ui.set_data",
            json!({"path": "quest.progress", "value": n}),
        );
    });
}

/// Play the script: press and release slots on time, aim along the camera, turn to the aim.
pub fn script_weaver(
    time: Res<Time>,
    hall: Option<Res<Hall>>,
    mut weavers: Query<(&mut Weaver, &mut CastInput, &mut Transform)>,
) {
    let Some(hall) = hall else {
        return;
    };
    if hall.program != Program::Player || !hall.near || hall.restart {
        return;
    }
    let dt = time.delta_secs();
    let eye = hall.origin + eye();
    for (mut w, mut input, mut tf) in &mut weavers {
        let t0 = w.t;
        w.t += dt;
        let t1 = w.t;
        let cycle = |t: f32| (t / PERIOD).floor();
        let local = |t: f32| t - cycle(t) * PERIOD;
        let (a, b) = (local(t0), local(t1));
        let wrapped = cycle(t0) != cycle(t1);
        for s in SCRIPT {
            let crossed = if wrapped {
                s.at >= a || s.at < b
            } else {
                s.at >= a && s.at < b
            };
            if !crossed {
                continue;
            }
            if s.press {
                input.press(s.slot);
            } else {
                input.release(s.slot);
            }
        }
        // Aim at the target of the latest beat begun.
        let now = local(t1);
        let target = SCRIPT
            .iter()
            .rev()
            .find(|s| s.at <= now)
            .unwrap_or(&SCRIPT[0])
            .target;
        let target = hall.origin + DVec3::from_array(target);
        let dir = (target - eye)
            .as_vec3()
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z);
        input.ray = Some((eye, dir));
        // The body turns toward where it casts.
        let flat = (target - tf.translation).as_vec3();
        if flat.x * flat.x + flat.z * flat.z > 0.01 {
            let want = Quat::from_rotation_y((-flat.x).atan2(-flat.z));
            tf.rotation = tf.rotation.slerp(want, (8.0 * dt).min(1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Hold;

    #[test]
    fn the_script_presses_then_releases_every_slot_once() {
        for slot in 0..SLOTS {
            let beats: Vec<&Beat> = SCRIPT.iter().filter(|b| b.slot == slot).collect();
            assert_eq!(beats.len(), 2, "slot {slot}");
            assert!(beats[0].press && !beats[1].press, "slot {slot}");
            assert!(beats[0].at < beats[1].at, "slot {slot}");
        }
        assert!(SCRIPT.windows(2).all(|w| w[0].at <= w[1].at));
        assert!(SCRIPT.last().unwrap().at < PERIOD);
        // The charge is held long enough to be full.
        let s = starter_spells();
        assert_eq!(Hold::of(&s[0].0), Hold::Charge);
        assert!(SCRIPT[1].at - SCRIPT[0].at >= crate::player::FULL_CHARGE_SECS);
    }
}

//! # engine_combat
//!
//! One melee rule set for the player and every NPC (engine-design/characters-week/aimed-melee.md,
//! round plan combat-k1.md). The animation module swings the weapons and finds where a blade
//! crosses a body; this module decides what that blow *means*:
//!
//! * **Aim where you look** ([`look`]): a fighter with [`LookAim`] (or the player's
//!   [`Interactor`](engine_world::Interactor)) aims at the body part under the camera's centre:
//!   [`StrikeTargets`](engine_world::animation::StrikeTargets) `zone` and `point` come from the
//!   camera ray against the target's zone capsules ([`body`]), near misses snapping to the
//!   nearest part, the pick held a moment so it doesn't flicker between thigh and knee.
//! * **Rules on every blow** ([`resolve`]): each melee `DamageEvent` (Slash, Pierce, Blunt from
//!   a body with a [`Loadout`](engine_world::combat::Loadout)) is judged on the zone it struck
//!   ([`rules`]): the attacker's weapon skill decides whether an aimed blow lands on its part
//!   and how clean the edge is, the defender's skill how often it covers its vital parts (head
//!   and neck most), the zone, the side it came from and the armour how much it hurts. The
//!   judged amount replaces the event's own, so every module that applies health agrees.
//! * **Wounds stay** ([`BodyInjuries`](engine_world::combat::BodyInjuries)): wounds build up
//!   per part (keep cutting the same knee and it gives out), edged blows bleed, blunt ones lock
//!   knees and elbows; a cut from behind at the knee is a hamstring. A clean, strong enough cut
//!   severs at the joint (neck, shoulder, elbow, wrist, hip, knee, ankle), by
//!   [`CombatSettings::gore`]; a severed neck kills. A ruined weapon arm drops the weapon
//!   ([`CombatSettings::disarm`]: the [`Loadout`](engine_world::combat::Loadout) hand empties
//!   and the animation module drops its props).
//! * **Bleeding** ([`bleed`]): open wounds keep costing health as `DamageKind::Bleed` (no
//!   flinch, no alarm) and clot slowly; a cut-off limb bleeds hardest and clots slowest.
//! * **Judged before anyone reads it**: the plugin is the world's melee judge
//!   (`engine_world::judges_melee`), so the animation module sends its blows as `MeleeContact`
//!   and every reader of `DamageEvent` sees only the judged amount. Blows still sent straight
//!   as `DamageEvent` are judged in place.
//! * Every judged blow goes out as a [`BodyHit`](engine_world::combat::BodyHit) and into the
//!   [`CombatLog`] (`combat.log`).
//! * **Pictures** ([`hint`]): a thin outline on the part a look-aimed strike would hit, and
//!   (off by default) every body's wounds drawn on its parts.
//!
//! Systems run in `Update` after `UpdateSet::Camera`: after the animation module has landed
//! this frame's blows, before its hit reactions (PostUpdate) and before the health of NPCs and
//! the player is taken (next fixed step).

pub mod bleed;
pub mod body;
pub mod demo;
pub mod hint;
pub mod look;
pub mod resolve;
pub mod rules;

use engine_core::prelude::*;
use serde_json::{Value, json};

pub use body::{BodyCapsules, ZoneCapsule};
pub use look::LookAim;
pub use resolve::{CombatLog, CombatRecord};
pub use rules::{Gore, Verdict};

/// Tuning for the melee rules. A resource (`combat.settings` reads and writes it).
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatSettings {
    /// Judge melee blows: scale their damage by zone, skill and armour, keep wounds, sever.
    /// Off, blows keep the amount their swing gave them and only `BodyHit`s are sent.
    pub rules: bool,
    /// How far dismemberment goes: `Off` never, `Limbs` arms and legs, `Full` heads too.
    pub gore: Gore,
    /// A ruined or severed weapon arm empties that hand of the body's `Loadout`.
    pub disarm: bool,
    /// Outline the part a look-aimed strike would hit, on the target.
    pub hint: bool,
    /// Draw every body's wounds on its parts (yellow to red, severed black), for pictures.
    pub show_wounds: bool,
    /// Health a fully bleeding part costs per second (as `DamageKind::Bleed`); 0 turns
    /// bleeding off.
    pub bleed_hp_per_s: f32,
    /// Time constant of clotting, s (a cut-off limb clots three times slower).
    pub bleed_clot_secs: f32,
    /// Seconds a look-picked zone is held before another may replace it.
    pub look_hold_secs: f32,
    /// A camera ray passing this close to a part (m) still picks it.
    pub look_snap_m: f32,
    /// Farthest target a look pick reaches, m.
    pub look_range_m: f32,
}

impl Default for CombatSettings {
    fn default() -> Self {
        Self {
            rules: true,
            gore: Gore::Full,
            disarm: true,
            hint: true,
            show_wounds: false,
            bleed_hp_per_s: 1.5,
            bleed_clot_secs: 25.0,
            look_hold_secs: 0.12,
            look_snap_m: 0.25,
            look_range_m: 4.0,
        }
    }
}

/// The combat module: look-to-aim, the melee rules, wounds and their pictures.
pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.world.init_resource::<CombatSettings>();
        app.world.init_resource::<CombatLog>();
        app.world.init_resource::<resolve::Dice>();
        app.world.init_resource::<resolve::OwnDamage>();
        // Animation sends its blows to us as MeleeContact, and we write their damage.
        engine_world::judges_melee(app, "engine_combat");
        app.add_message::<engine_world::DamageEvent>()
            .add_message::<engine_world::combat::BodyHit>()
            .add_message::<engine_world::MeleeContact>();
        app.register_resource::<CombatSettings>(
            "Melee rules: rules on/off, gore (Off, Limbs, Full), disarm, hint (outline the \
             look-aimed part), show_wounds, bleed_hp_per_s, bleed_clot_secs, look_hold_secs, look_snap_m, \
             look_range_m.",
        )
        .register_component::<LookAim>(
            "Aim this fighter's strikes where the camera looks: the body part under the screen \
             centre becomes StrikeTargets.zone/point (the player's Interactor does this too).",
        )
        .register_component::<rules::Armor>(
            "Armour per body zone, 0 bare .. 1 plate, in BodyZone order (16): cuts lose most \
             to it, stabs less, blunt blows least; it also keeps limbs on.",
        );
        app.add_systems(
            Update,
            (
                look::look_aim,
                resolve::judge_contacts,
                resolve::judge_blows,
                bleed::bleed,
                resolve::disarm,
                hint::draw_hint,
                hint::draw_blows,
                hint::draw_wounds,
            )
                // In order, without sync points: what they spawn or insert (a player's
                // LookAim, a body's first BodyInjuries) lands at the end of Update.
                .chain_ignore_deferred()
                .after(UpdateSet::Camera),
        );
        demo::register(app);
        register_api(app);
    }
}

fn register_api(app: &mut App) {
    app.register_api(
        "combat.settings",
        "Read the melee rules' settings, or change some: {\"gore\": \"Limbs\", \"hint\": false}.",
        r#"{"show_wounds": true}"#,
        |world, args| {
            let mut s = world.resource::<CombatSettings>().clone();
            if let Value::Object(map) = &args
                && !map.is_empty()
            {
                let mut cur = serde_json::to_value(&s).map_err(|e| e.to_string())?;
                for (k, v) in map {
                    if cur.get(k).is_none() {
                        return Err(format!(
                            "'{k}' is not a combat setting; call combat.settings with {{}} to \
                             see them all"
                        ));
                    }
                    cur[k] = v.clone();
                }
                s = serde_json::from_value(cur).map_err(|e| {
                    format!("a combat setting has the wrong type ({e}); see combat.settings {{}}")
                })?;
                *world.resource_mut::<CombatSettings>() = s.clone();
            }
            serde_json::to_value(&s).map_err(|e| e.to_string())
        },
    )
    .register_api(
        "combat.log",
        "The last judged melee blows: aimed and struck zone, approach, covered, glanced, \
         cut quality, amount before and after, severed, killed. {\"last\": 20}.",
        r#"{"last": 10}"#,
        |world, args| {
            let n = args.get("last").and_then(Value::as_u64).unwrap_or(20) as usize;
            let log = world.resource::<CombatLog>();
            let rows: Vec<Value> = log
                .records
                .iter()
                .rev()
                .take(n)
                .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                .collect();
            Ok(json!({ "blows": log.total, "last": rows }))
        },
    );
}

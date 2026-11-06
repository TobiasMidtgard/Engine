//! The player's spell bar (magic round 4, engine-design/magic/round-4-plan.md): eight saved
//! spells on keys 1-8, aimed from the camera, held to charge, channel or place.
//!
//! * [`SpellBar`] maps slots to grimoire spells and keeps each slot's cooldown. The UI module's
//!   HUD hotbar draws it from `ui.set_data` (`abilities[i].{icon, key, cooldown, total}`,
//!   `player.{mana, mana_max}`), so no UI code knows about magic.
//! * [`CastInput`] is what the bar reads each step: which slots are down and the aiming ray.
//!   Keys write it ([`read_spell_keys`]) unless the entity is [`ScriptedInput`] (a film, a test
//!   or an AI driving the same path).
//! * Aiming follows "Better third-person projectiles" (tinygame.dev): sweep a 0.2 m sphere from
//!   the camera through the crosshair out to 60 m; the hit (or the point 60 m out) is the
//!   target, and the spell leaves the hand toward it, so it lands where the crosshair shows.
//! * How a slot is held depends on the spell ([`Hold`]): a bolt fires on press; a spell with a
//!   Charge modifier grows while held and fires on release (Forspoken's hold tiers); a beam or
//!   aura lasts while held and drains mana; an area, wall or rune shows a ring where it will
//!   land and goes there on release.
//! * Commit on release (Unreal's Gameplay Ability System): mana is paid and the cooldown starts
//!   only when the spell leaves the hand, so a cancelled charge costs nothing.
//!
//! [`PlayerMagic`] equips the play-mode character (the first with a `CharacterController` and
//! a `SimulationFocus`) with a caster and the starter bar.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_input::{ActionState, Binding, InputMap, KeyCode};
use engine_render::{Camera, MeshRenderer, Shape, StandardMaterial};
use engine_world::SimulationFocus;
use engine_world::physics::{CharacterController, ColliderShape, PhysicsQueries, QueryFilter};
use serde_json::json;

use crate::caster::{Aim, CastOutcome, CastRequest, Casting, MagicCaster, SpellCast, SpellChoice};
use crate::compile::Spell;
use crate::design::{CastType, Modifier, SpellDesign};
use crate::growth::Growth;
use crate::looks::LookCmd;
use crate::runtime::{BeamCast, MagicQueue};
use crate::schools::Mastery;

/// Slots on the bar (keys 1-8).
pub const SLOTS: usize = 8;
/// How far the crosshair reaches, m.
pub const AIM_RANGE_M: f32 = 60.0;
/// The aiming sweep's radius, m (tinygame.dev's 20 cm sphere).
pub const AIM_RADIUS_M: f32 = 0.2;
/// Seconds of holding for a full charge.
pub const FULL_CHARGE_SECS: f32 = 1.2;
/// A channel lasts this long past the last step it was held, s.
pub const CHANNEL_GRACE_SECS: f32 = 0.15;
/// Mana a channel drains per second, as a share of the spell's cost.
pub const CHANNEL_DRAIN: f32 = 0.35;
/// Seconds a slot flashes when it can't cast (cooling down, out of mana).
pub const FLASH_SECS: f32 = 0.35;
/// A press in the last this-many seconds of the cast before it waits for the hands and goes
/// the moment that cast leaves (a spell queue window, as in World of Warcraft's 400 ms); an
/// earlier press flashes.
pub const BUFFER_SECS: f32 = 0.45;

/// Equip the player and draw the bar on the HUD.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerMagic {
    /// Give the play-mode character a caster and the starter bar.
    pub equip: bool,
    /// The level and mastery a starter caster gets.
    pub level: f32,
    pub mastery: Mastery,
    /// Write the bar to the UI's HUD data.
    pub hud: bool,
}

impl Default for PlayerMagic {
    fn default() -> Self {
        Self {
            equip: true,
            level: 40.0,
            mastery: Mastery::Adept,
            hud: true,
        }
    }
}

/// How a slot is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Fires on press (bolts, lobs, chains, cones, touch, self).
    Tap,
    /// Grows while held, fires on release (a Charge modifier).
    Charge,
    /// Fires on press and lasts while held (beams, auras, orbits, tethers).
    Channel,
    /// Shows where it will land while held, goes there on release (areas, walls, runes, rain,
    /// waves, summons).
    Place,
}

impl Hold {
    pub fn of(design: &SpellDesign) -> Hold {
        if design.has(|m| matches!(m, Modifier::Charge { .. })) {
            return Hold::Charge;
        }
        match design.cast {
            CastType::Beam | CastType::Aura | CastType::Orbit | CastType::Tether => Hold::Channel,
            CastType::Area
            | CastType::Wall
            | CastType::Rune
            | CastType::Rain
            | CastType::Wave
            | CastType::Summon => Hold::Place,
            _ => Hold::Tap,
        }
    }
}

/// The slot being held now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub slot: usize,
    pub secs: f32,
    pub hold: Hold,
    /// A channel's spell is out (it ends when the key comes up).
    pub fired: bool,
}

/// Eight slots of grimoire spells and their cooldowns.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellBar {
    /// The grimoire spell on each slot.
    pub slots: Vec<Option<usize>>,
    /// Seconds before each slot casts again.
    pub cooldown: Vec<f32>,
    /// The cooldown each slot last started, s (the HUD's sweep).
    pub cooldown_total: Vec<f32>,
    /// Seconds left of each slot's "can't" flash.
    pub flash: Vec<f32>,
    #[serde(skip)]
    pub held: Option<Held>,
    /// A press waiting for the hands to be free: slot and seconds it still waits.
    #[serde(skip)]
    pub buffered: Option<(usize, f32)>,
    /// Where the crosshair points, updated every step (for the HUD and the placing ring).
    #[serde(skip)]
    pub aim_point: Option<DVec3>,
}

impl Default for SpellBar {
    fn default() -> Self {
        Self {
            slots: (0..SLOTS).map(Some).collect(),
            cooldown: vec![0.0; SLOTS],
            cooldown_total: vec![0.0; SLOTS],
            flash: vec![0.0; SLOTS],
            held: None,
            buffered: None,
            aim_point: None,
        }
    }
}

impl SpellBar {
    fn ensure_len(&mut self) {
        for v in [
            &mut self.cooldown,
            &mut self.cooldown_total,
            &mut self.flash,
        ] {
            v.resize(SLOTS, 0.0);
        }
        self.slots.resize(SLOTS, None);
    }
}

/// What the bar reads each step: slots down, presses since the last step, and the aiming ray.
#[derive(Component, Debug, Clone, PartialEq, Default)]
pub struct CastInput {
    pub down: [bool; SLOTS],
    /// Pressed since the last fixed step (a tap shorter than a step still counts).
    pub pressed: [bool; SLOTS],
    /// The aiming ray: origin and unit direction (the camera through the crosshair).
    pub ray: Option<(DVec3, Vec3)>,
    /// Drop a held charge or placement without casting (a dodge, a block).
    pub cancel: bool,
}

impl CastInput {
    pub fn press(&mut self, slot: usize) {
        if slot < SLOTS {
            self.down[slot] = true;
            self.pressed[slot] = true;
        }
    }

    pub fn release(&mut self, slot: usize) {
        if slot < SLOTS {
            self.down[slot] = false;
        }
    }
}

/// The entity's [`CastInput`] comes from a script, not the keys.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ScriptedInput;

/// This bar is the one the HUD hotbar shows.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudBar;

/// The ring that shows where a placed spell will land.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlaceRing {
    pub owner: Entity,
}

/// The spell actions, `spell_1`..`spell_8` on the number keys, unless the game binds them.
pub fn bind_actions(map: Option<ResMut<InputMap>>) {
    let Some(mut map) = map else {
        return;
    };
    let keys = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
    ];
    for (i, k) in keys.into_iter().enumerate() {
        let action = action_name(i);
        if !map.actions.contains_key(&action) {
            map.bind(&action, Binding::Key(k));
        }
    }
}

pub fn action_name(slot: usize) -> String {
    format!("spell_{}", slot + 1)
}

/// The starter bar: one spell of each way of holding, across the schools the hall shows.
pub fn starter_spells() -> Vec<(SpellDesign, Option<Growth>)> {
    let d = SpellDesign::new;
    vec![
        (
            d(&["fire"], CastType::Bolt)
                .with(Modifier::Charge { max_secs: 1.0 })
                .named("Fireball"),
            None,
        ),
        (
            d(&["frost"], CastType::Wall)
                .with(Modifier::Size { factor: 0.7 })
                .named("Frost wall"),
            None,
        ),
        (
            d(&["lift", "stone"], CastType::Area)
                .with(Modifier::Duration { factor: 0.5 })
                .named("Pillar"),
            None,
        ),
        (
            d(&["storm"], CastType::Chain).named("Chain lightning"),
            None,
        ),
        (
            d(&["fire", "frost"], CastType::Lob).named("Steam flask"),
            None,
        ),
        (d(&["fire"], CastType::Beam).named("Flame jet"), None),
        (d(&["stone"], CastType::Bolt).named("Stone grip"), None),
        (d(&["heal"], CastType::OnSelf).named("Mend"), None),
    ]
}

/// The HUD icon for a spell (the UI module's ability icons; a spellbook where none fits).
pub fn icon_of(design: &SpellDesign) -> &'static str {
    let has = |k: &str| design.all_effects().iter().any(|e| e == k);
    if has("heal") {
        "ability_heal"
    } else if has("storm") {
        "ability_lightning"
    } else if has("frost") && !has("fire") {
        "ability_frost"
    } else if has("fire") {
        "ability_fireball"
    } else if has("ward") {
        "ability_shield_bubble"
    } else if has("blink") {
        "ability_dash"
    } else {
        "spellbook"
    }
}

/// A slot's cooldown, s: a breath for a plain bolt, longer for complex and heavy spells.
pub fn cooldown_secs(spell: &Spell, hold: Hold) -> f32 {
    let base = match hold {
        Hold::Tap => 0.45,
        Hold::Charge => 0.8,
        Hold::Channel => 1.5,
        Hold::Place => 2.5,
    };
    (base + 0.18 * spell.complexity as f32 + 0.004 * spell.cost.mana).clamp(0.3, 12.0)
}

/// Give the play-mode character a caster and the starter bar.
#[allow(clippy::type_complexity)]
pub fn equip_player(
    settings: Option<Res<PlayerMagic>>,
    players: Query<
        (Entity, Option<&MagicCaster>),
        (
            With<CharacterController>,
            With<SimulationFocus>,
            Without<SpellBar>,
        ),
    >,
    mut commands: Commands,
) {
    let Some(settings) = settings else {
        return;
    };
    if !settings.equip {
        return;
    }
    for (e, caster) in &players {
        let caster = match caster {
            Some(c) if !c.grimoire.is_empty() => None,
            _ => {
                let mut c = MagicCaster::new(settings.level, settings.mastery).with_seed(7);
                for (design, growth) in starter_spells() {
                    c = c.learn(design, growth);
                }
                Some(c)
            }
        };
        let mut ec = commands.entity(e);
        if let Some(c) = caster {
            ec.insert(c);
        }
        ec.insert((SpellBar::default(), CastInput::default(), HudBar));
        tracing::info!("magic: the player can cast (keys 1-8)");
    }
}

/// The keys and the camera into [`CastInput`] (every frame, so no press is missed). A
/// character without its controller (the pilot parked when play mode ends, while the number
/// keys jump the free camera around) hears nothing.
#[allow(clippy::type_complexity)]
pub fn read_spell_keys(
    actions: Option<Res<ActionState>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut players: Query<
        (&mut CastInput, Has<CharacterController>),
        (With<SpellBar>, Without<ScriptedInput>),
    >,
) {
    let Some(actions) = actions else {
        return;
    };
    let ray = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| (t.translation(), (t.rotation() * Vec3::NEG_Z).normalize()));
    for (mut input, playing) in &mut players {
        if !playing {
            if *input != CastInput::default() {
                *input = CastInput::default();
            }
            continue;
        }
        for i in 0..SLOTS {
            let a = action_name(i);
            input.down[i] = actions.pressed(&a);
            if actions.just_pressed(&a) {
                input.pressed[i] = true;
            }
        }
        input.ray = ray;
    }
}

/// Where a ray from `origin` along `dir` meets the world (a sphere sweep), or the point at full
/// range.
pub fn aim_point(
    queries: Option<&PhysicsQueries>,
    origin: DVec3,
    dir: Vec3,
    caster: Entity,
) -> DVec3 {
    let far = origin + (dir * AIM_RANGE_M).as_dvec3();
    let Some(q) = queries else {
        return far;
    };
    q.shapecast(
        &ColliderShape::Sphere {
            radius: AIM_RADIUS_M,
        },
        origin,
        Quat::IDENTITY,
        dir,
        AIM_RANGE_M,
        &QueryFilter::excluding(caster),
    )
    .map(|h| origin + (dir * h.distance).as_dvec3())
    .unwrap_or(far)
}

/// Run each bar: cooldowns, holds, and the casts they release.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn drive_spell_bars(
    time: Res<Time>,
    queries: Option<Res<PhysicsQueries>>,
    mut bars: Query<(
        Entity,
        &mut SpellBar,
        &mut CastInput,
        &mut MagicCaster,
        &GlobalTransform,
        Option<&Casting>,
    )>,
    mut beams: Query<&mut BeamCast>,
    mut requests: MessageWriter<CastRequest>,
    mut queue: ResMut<MagicQueue>,
) {
    let dt = time.delta_secs();
    for (e, mut bar, mut input, mut caster, t, casting) in &mut bars {
        let busy = casting.is_some();
        // Seconds until the cast in the hands leaves.
        let left = casting.map_or(0.0, |c| (c.release_at - c.t).max(0.0));
        bar.ensure_len();
        for i in 0..SLOTS {
            bar.cooldown[i] = (bar.cooldown[i] - dt).max(0.0);
            bar.flash[i] = (bar.flash[i] - dt).max(0.0);
        }
        bar.buffered = bar
            .buffered
            .map(|(slot, left)| (slot, left - dt))
            .filter(|b| b.1 > 0.0);
        let target = input
            .ray
            .map(|(o, d)| aim_point(queries.as_deref(), o, d, e));
        bar.aim_point = target;
        let aim = || match target {
            Some(p) => Aim::Point(p),
            None => Aim::Direction((t.rotation() * Vec3::NEG_Z).normalize()),
        };

        // The held slot: grow, keep a channel going, or release.
        if let Some(mut held) = bar.held {
            held.secs += dt;
            let up = !input.down[held.slot];
            let design = bar.slots[held.slot].and_then(|g| caster.grown(g));
            if input.cancel {
                queue.looks.push(LookCmd::EndCharge { caster: e });
                bar.held = None;
            } else if let Some(design) = design {
                match held.hold {
                    Hold::Channel => {
                        // Keep the beam alive while held; drain mana; stop when let go or dry.
                        let drain = caster
                            .compile(&design, 0.0)
                            .map(|s| s.cost.mana * CHANNEL_DRAIN)
                            .unwrap_or(0.0);
                        let dry = caster.mana <= 0.0;
                        let going = !up && !dry;
                        if going {
                            caster.mana = (caster.mana - drain * dt).max(0.0);
                        }
                        for mut b in &mut beams {
                            if b.cast.caster != Some(e) {
                                continue;
                            }
                            let end = b.age + CHANNEL_GRACE_SECS;
                            if going {
                                b.duration = b.duration.max(end);
                                if let Some(p) = target {
                                    b.aim = Aim::Point(p);
                                }
                            } else {
                                // Let go (or out of mana): it ends now, however long it had.
                                b.duration = b.duration.min(end);
                            }
                        }
                        bar.held = going.then_some(held);
                    }
                    Hold::Charge | Hold::Place if up => {
                        let charge = if held.hold == Hold::Charge {
                            (held.secs / FULL_CHARGE_SECS).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        if !busy {
                            requests.write(CastRequest {
                                caster: e,
                                spell: SpellChoice::Slot(bar.slots[held.slot].unwrap_or(0)),
                                aim: aim(),
                                charge,
                                gathered: held.hold == Hold::Charge,
                            });
                            start_cooldown(&mut bar, held.slot, &caster, &design, charge);
                        }
                        queue.looks.push(LookCmd::EndCharge { caster: e });
                        bar.held = None;
                    }
                    _ => bar.held = Some(held),
                }
            } else {
                bar.held = None;
            }
        }

        // A new press.
        if bar.held.is_none() {
            let pressed = (0..SLOTS)
                .find(|&i| input.pressed[i])
                .or_else(|| bar.buffered.filter(|_| !busy).map(|b| b.0));
            if let Some(slot) = pressed {
                bar.buffered = None;
                let design = bar.slots[slot].and_then(|g| caster.grown(g));
                let spell = design.as_ref().and_then(|d| caster.compile(d, 0.0).ok());
                match (design, spell) {
                    (Some(design), Some(spell)) => {
                        if busy && bar.cooldown[slot] <= 0.0 && left <= BUFFER_SECS {
                            bar.buffered = Some((slot, left + 0.1));
                        } else if busy || bar.cooldown[slot] > 0.0 || caster.mana < spell.cost.mana
                        {
                            bar.flash[slot] = FLASH_SECS;
                        } else {
                            let hold = Hold::of(&design);
                            match hold {
                                Hold::Tap | Hold::Channel => {
                                    requests.write(CastRequest {
                                        caster: e,
                                        spell: SpellChoice::Slot(bar.slots[slot].unwrap_or(0)),
                                        aim: aim(),
                                        charge: 0.0,
                                        gathered: false,
                                    });
                                    start_cooldown(&mut bar, slot, &caster, &design, 0.0);
                                    if hold == Hold::Channel && input.down[slot] {
                                        bar.held = Some(Held {
                                            slot,
                                            secs: 0.0,
                                            hold,
                                            fired: true,
                                        });
                                    }
                                }
                                Hold::Charge | Hold::Place => {
                                    if hold == Hold::Charge {
                                        queue.looks.push(LookCmd::Charge {
                                            caster: e,
                                            look: spell.weave.key.clone(),
                                            secs: FULL_CHARGE_SECS,
                                            scale: (0.4 + 0.25 * spell.power.size()).min(2.0),
                                        });
                                    }
                                    // A tap shorter than a step is released on the next.
                                    bar.held = Some(Held {
                                        slot,
                                        secs: 0.0,
                                        hold,
                                        fired: false,
                                    });
                                }
                            }
                        }
                    }
                    _ => bar.flash[slot] = FLASH_SECS,
                }
            }
        }
        input.pressed = [false; SLOTS];
        input.cancel = false;
    }
}

fn start_cooldown(
    bar: &mut SpellBar,
    slot: usize,
    caster: &MagicCaster,
    design: &SpellDesign,
    charge: f32,
) {
    if let Ok(spell) = caster.compile(design, charge) {
        let secs = cooldown_secs(&spell, Hold::of(design));
        bar.cooldown[slot] = secs;
        bar.cooldown_total[slot] = secs;
    }
}

/// A slot that was refused (out of mana at release) flashes.
pub fn flash_refused(
    mut casts: MessageReader<SpellCast>,
    mut bars: Query<(&mut SpellBar, &MagicCaster)>,
) {
    for c in casts.read() {
        if c.outcome != CastOutcome::Spent {
            continue;
        }
        let Ok((mut bar, caster)) = bars.get_mut(c.caster) else {
            continue;
        };
        // The slot whose spell this was.
        let slot = (0..bar.slots.len()).find(|&i| {
            bar.slots[i]
                .and_then(|g| caster.grown(g))
                .is_some_and(|d| d.name == c.spell.name)
        });
        if let Some(i) = slot {
            bar.flash[i] = FLASH_SECS;
            bar.cooldown[i] = 0.0;
        }
    }
}

/// The ring under a placed spell's landing point while its slot is held.
#[allow(clippy::type_complexity)]
pub fn show_place_rings(
    bars: Query<(Entity, &SpellBar, &MagicCaster)>,
    mut rings: Query<(Entity, &PlaceRing, &mut Transform)>,
    queries: Option<Res<PhysicsQueries>>,
    mut commands: Commands,
) {
    let mut wanted: Vec<(Entity, DVec3, f32, [f32; 3])> = Vec::new();
    for (e, bar, caster) in &bars {
        let Some(held) = bar.held else { continue };
        if held.hold != Hold::Place {
            continue;
        }
        let (Some(p), Some(design)) = (
            bar.aim_point,
            bar.slots[held.slot].and_then(|g| caster.grown(g)),
        ) else {
            continue;
        };
        let Ok(spell) = caster.compile(&design, 0.0) else {
            continue;
        };
        let ground = match queries.as_deref() {
            Some(q) => {
                crate::runtime::ground_below(q, p + DVec3::Y * 0.5, &QueryFilter::excluding(e))
            }
            None => p,
        };
        wanted.push((
            e,
            ground,
            spell.carrier.radius.clamp(0.8, 6.0),
            spell.weave.color,
        ));
    }
    let mut placed = Vec::new();
    for (ring_e, ring, mut tf) in &mut rings {
        match wanted.iter().find(|w| w.0 == ring.owner) {
            Some(w) => {
                tf.translation = w.1 + DVec3::Y * 0.04;
                placed.push(ring.owner);
            }
            None => commands.entity(ring_e).despawn(),
        }
    }
    for (owner, at, radius, color) in wanted {
        if placed.contains(&owner) {
            continue;
        }
        commands.spawn((
            Name::new("Spell landing ring"),
            PlaceRing { owner },
            Transform::from_translation(at + DVec3::Y * 0.04),
            MeshRenderer::new(
                Shape::Torus {
                    radius,
                    tube: 0.04,
                    segments: 64,
                },
                StandardMaterial::default()
                    .with_tint(color[0], color[1], color[2])
                    .with_emissive(color, 4.0),
            ),
        ));
    }
}

/// The bar on the UI's HUD hotbar, and the mana bar (only when something changed).
pub fn write_hud(
    settings: Option<Res<PlayerMagic>>,
    bars: Query<(&SpellBar, &MagicCaster), With<HudBar>>,
    mut last: Local<Option<serde_json::Value>>,
    mut commands: Commands,
) {
    if !settings.is_some_and(|s| s.hud) {
        return;
    }
    let Some((bar, caster)) = bars.iter().next() else {
        return;
    };
    let value = hud_data(bar, caster);
    if last.as_ref() == Some(&value) {
        return;
    }
    *last = Some(value.clone());
    commands.queue(move |world: &mut World| {
        let _ = engine_core::registry::call_api(world, "ui.set_data", json!({ "values": value }));
    });
}

/// The HUD data for a bar: abilities[i] and the mana pool, rounded so it changes ten times a
/// second at most.
pub fn hud_data(bar: &SpellBar, caster: &MagicCaster) -> serde_json::Value {
    let round = |x: f32| (x as f64 * 10.0).round() / 10.0;
    let abilities: Vec<serde_json::Value> = (0..bar.slots.len().min(SLOTS))
        .map(|i| {
            let icon = bar.slots[i]
                .and_then(|g| caster.grown(g))
                .map(|d| icon_of(&d))
                .unwrap_or("");
            json!({
                "icon": icon,
                "key": (i + 1).to_string(),
                "cooldown": round(bar.cooldown.get(i).copied().unwrap_or(0.0)),
                "total": round(bar.cooldown_total.get(i).copied().unwrap_or(0.0)),
                "flash": bar.flash.get(i).copied().unwrap_or(0.0) > 0.0,
            })
        })
        .collect();
    json!({
        "abilities": abilities,
        "player.mana": caster.mana.round(),
        "player.mana_max": caster.max_mana.round(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_spell_is_held_its_own_way() {
        let s = starter_spells();
        let holds: Vec<Hold> = s.iter().map(|(d, _)| Hold::of(d)).collect();
        assert_eq!(holds[0], Hold::Charge, "fireball charges");
        assert_eq!(holds[1], Hold::Place, "a wall is placed");
        assert_eq!(holds[2], Hold::Place, "a pillar is placed");
        assert_eq!(holds[3], Hold::Tap, "chain lightning fires at once");
        assert_eq!(holds[5], Hold::Channel, "a flame jet is channelled");
        assert_eq!(holds[7], Hold::Tap, "a self heal fires at once");
    }

    #[test]
    fn the_starter_bar_compiles_for_an_adept() {
        let mut c = MagicCaster::new(40.0, Mastery::Adept);
        for (d, g) in starter_spells() {
            c = c.learn(d, g);
        }
        for i in 0..SLOTS {
            let d = c.grown(i).unwrap();
            let s = c
                .compile(&d, 0.0)
                .unwrap_or_else(|e| panic!("{}: {e}", d.name));
            let cd = cooldown_secs(&s, Hold::of(&d));
            assert!((0.3..=12.0).contains(&cd), "{}: {cd}", d.name);
            assert!(s.cost.mana < c.max_mana, "{} costs {}", d.name, s.cost.mana);
        }
    }

    #[test]
    fn the_hud_gets_icons_keys_and_mana() {
        let mut c = MagicCaster::new(40.0, Mastery::Adept);
        for (d, g) in starter_spells() {
            c = c.learn(d, g);
        }
        let mut bar = SpellBar::default();
        bar.cooldown[2] = 1.234;
        bar.cooldown_total[2] = 2.5;
        let v = hud_data(&bar, &c);
        assert_eq!(v["abilities"][0]["icon"], "ability_fireball");
        assert_eq!(v["abilities"][3]["icon"], "ability_lightning");
        assert_eq!(v["abilities"][7]["icon"], "ability_heal");
        assert_eq!(v["abilities"][2]["key"], "3");
        assert_eq!(v["abilities"][2]["cooldown"], 1.2);
        assert_eq!(v["player.mana_max"], c.max_mana.round());
    }
}

//! Gear and captains aboard ships, airships and planes: items of [`Host::Ship`] in a
//! vessel's own slots. Slot 0 is the captain's (a specialist); the rest take devices and
//! charters. What they add up to is the vessel's [`Kit`], which the fleet reads: speed, a
//! quicker turnround, hold slots, how far she clears the fog, flotsam, postage.
//!
//! Orders aboard (crowd on all sail, full steam ahead, throw cargo overboard) are pressed
//! from the vessel's card or order bar; consumables (a coal bunker) are used up; a charged
//! item (the ship's cat) fills with voyages.

use engine::prelude::*;
use serde::Serialize;

use crate::build::Changed;
use crate::fleet::{Doing, Vessel};
use crate::items::{Burst, Effect, Fill, Held, Host, ITEMS, ItemView, Items, SHELF, Sort, Way};
use crate::vessels::Craft;

/// Captain plus gear: the most slots any vessel has.
pub const MAX_GEAR: usize = 4;

/// Gear slots a craft has besides the captain's.
pub fn gear_slots(c: Craft) -> usize {
    match c {
        Craft::Smack | Craft::Biplane | Craft::Seaplane | Craft::Cutter | Craft::Gunboat => 1,
        Craft::Schooner | Craft::Barque | Craft::PaddleSteamer | Craft::Brig => 2,
        Craft::FlyingBoat => 2,
        Craft::Clipper | Craft::Steamer | Craft::Airship | Craft::Frigate | Craft::Liner => 3,
        Craft::Dreadnought | Craft::Leviathan | Craft::SkyCarrier => 3,
    }
}

/// A vessel's slots: `[0]` the captain, then gear.
#[derive(Component, Clone, Debug, Default)]
pub struct Gear {
    pub slots: [Option<Held>; MAX_GEAR],
    /// Voyages finished, for charged items.
    pub voyages: u32,
}

/// What a vessel's gear adds up to. The fleet reads only this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Kit {
    pub speed: f32,
    pub loads: f32,
    pub hold: u32,
    pub sight: f32,
    pub luck: f32,
    pub postage: f32,
    pub longer: f32,
    pub recharge: f32,
    /// Share of every hold the fittings take (see `fittings`).
    pub room: f32,
}

impl Kit {
    fn add(&mut self, e: Effect, k: f32) {
        match e {
            Effect::Speed(x) => self.speed = (self.speed + x * k).clamp(-0.5, 1.5),
            Effect::Loads(x) => self.loads = (self.loads + x * k).min(0.8),
            Effect::Hold(n) => self.hold += n as u32,
            Effect::Sight(x) => self.sight += x * k,
            Effect::Luck(x) => self.luck += x * k,
            Effect::Postage(c) => self.postage += c * k,
            Effect::Longer(x) => self.longer += x * k,
            Effect::Recharge(x) => self.recharge = (self.recharge + x * k).min(0.7),
            _ => {}
        }
    }
}

/// Whether `item` may go in slot `slot` of a vessel of this craft.
fn fits(item: usize, slot: usize, craft: Craft) -> Result<(), String> {
    let d = &ITEMS[item];
    if d.host != Host::Ship {
        return Err(format!(
            "{} belongs in a {}.",
            d.name,
            d.host.kind().def().name
        ));
    }
    if slot > gear_slots(craft) {
        return Err(format!(
            "A {} has {} gear slots.",
            craft.name(),
            gear_slots(craft)
        ));
    }
    let captain = d.sort == Sort::Specialist;
    if slot == 0 && !captain {
        return Err("The first slot is the captain's.".into());
    }
    if slot > 0 && captain {
        return Err(format!("{} goes in the captain's slot.", d.name));
    }
    if let Some(dr) = d.drive
        && !dr.fits(craft)
    {
        return Err(format!("{} does not fit a {}.", d.name, craft.name()));
    }
    Ok(())
}

/// Work out every vessel's [`Kit`] from the gear acting aboard.
pub fn apply(world: &mut World) {
    let mut q = world.query::<(&Gear, &mut Vessel)>();
    for (gear, mut v) in q.iter_mut(world) {
        let mut kit = Kit::default();
        for h in gear.slots.iter().flatten().filter(|h| h.acting()) {
            let k = 1.0 + 0.2 * h.stars() as f32;
            for e in h.def().effects {
                kit.add(*e, k);
            }
        }
        crate::fittings::add_to_kit(&v.fit, &mut kit);
        crate::flagships::innate(v.craft, &mut kit);
        if v.kit != kit {
            v.kit = kit;
        }
    }
}

/// The vessel whose entity index is `id`.
pub fn by_id(world: &mut World, id: u32) -> Option<Entity> {
    let mut q = world.query_filtered::<Entity, With<Vessel>>();
    q.iter(world).find(|e| e.index_u32() == id)
}

/// Put the shelf item `uid` aboard, in slot `slot` (what was there goes back on the shelf).
pub fn socket(world: &mut World, vessel: u32, slot: usize, uid: u32) -> Result<(), String> {
    let e = by_id(world, vessel).ok_or("No such vessel.")?;
    let craft = world.get::<Vessel>(e).ok_or("No such vessel.")?.craft;
    let at = world
        .resource::<Items>()
        .shelf
        .iter()
        .position(|h| h.uid == uid)
        .ok_or("That item is not on the shelf.")?;
    fits(world.resource::<Items>().shelf[at].item, slot, craft)?;
    let item = world.resource_mut::<Items>().shelf.remove(at);
    let old = world
        .get_mut::<Gear>(e)
        .and_then(|mut g| g.slots[slot].replace(item));
    if let Some(old) = old {
        world.resource_mut::<Items>().shelf.push(old);
    }
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Take an item from aboard, back to the shelf.
pub fn unsocket(world: &mut World, vessel: u32, slot: usize) -> Result<(), String> {
    let e = by_id(world, vessel).ok_or("No such vessel.")?;
    if world.resource::<Items>().shelf.len() >= SHELF {
        return Err("The shelf is full.".into());
    }
    let item = world
        .get_mut::<Gear>(e)
        .and_then(|mut g| g.slots.get_mut(slot).and_then(Option::take))
        .ok_or("That slot is empty.")?;
    world.resource_mut::<Items>().shelf.push(item);
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Start an item aboard: its effects act for its time, and its burst happens now.
fn start(world: &mut World, e: Entity, slot: usize) {
    let (longer, recharge) = world
        .get::<Vessel>(e)
        .map_or((0.0, 0.0), |v| (v.kit.longer, v.kit.recharge));
    let bursts = {
        let Some(mut gear) = world.get_mut::<Gear>(e) else {
            return;
        };
        let Some(h) = gear.slots[slot].as_mut() else {
            return;
        };
        let d = h.def();
        h.left_s = d.way.lasts() * (1.0 + longer);
        match d.way {
            Way::Order { cooldown_s, .. } => h.cool_s = h.left_s + cooldown_s * (1.0 - recharge),
            Way::Consumable { .. } => h.charges = h.charges.saturating_sub(1),
            _ => {}
        }
        d.burst
    };
    for b in bursts {
        match b {
            Burst::Overboard => {
                if let Some(mut v) = world.get_mut::<Vessel>(e) {
                    for c in v.cargo.iter_mut() {
                        c.1 -= c.1 / 3;
                    }
                }
            }
            Burst::Flotsam => {
                let ship = world
                    .get::<Vessel>(e)
                    .map(|v| v.name.clone())
                    .unwrap_or_default();
                crate::items::flotsam_sure(world, &ship);
            }
            _ => {}
        }
    }
    world.resource_mut::<Changed>().any = true;
}

/// Press an order or use a consumable aboard.
pub fn use_item(world: &mut World, vessel: u32, slot: usize) -> Result<&'static str, String> {
    let e = by_id(world, vessel).ok_or("No such vessel.")?;
    let h = world
        .get::<Gear>(e)
        .and_then(|g| g.slots.get(slot).cloned().flatten())
        .ok_or("That slot is empty.")?;
    let d = h.def();
    if d.later > 0 {
        return Err(format!("{} works from round {}.", d.name, d.later));
    }
    if !h.ready() {
        return Err(match d.way {
            Way::Order { .. } if h.left_s > 0.0 => format!("{} is already under way.", d.name),
            Way::Order { .. } => format!("{} is ready again in {:.0} s.", d.name, h.cool_s),
            Way::Consumable { .. } => format!("{} is used up.", d.name),
            _ => format!("{} acts by itself.", d.name),
        });
    }
    start(world, e, slot);
    Ok(d.name)
}

/// Each frame: timers aboard run down, captains gain experience at sea, used-up
/// consumables go, and charged items fill with voyages.
pub fn tick(world: &mut World, dt: f32) {
    let mut changed = false;
    let mut fire: Vec<(Entity, usize)> = Vec::new();
    let mut q = world.query::<(Entity, &Vessel, &mut Gear)>();
    for (e, v, mut gear) in q.iter_mut(world) {
        let at_sea = v.doing == Doing::Sailing;
        let voyages = gear.voyages;
        for (i, slot) in gear.slots.iter_mut().enumerate() {
            let Some(h) = slot.as_mut() else { continue };
            if h.left_s > 0.0 {
                h.left_s -= dt;
                if h.left_s <= 0.0 {
                    h.left_s = 0.0;
                    changed = true;
                    if matches!(h.def().way, Way::Consumable { .. }) && h.charges == 0 {
                        *slot = None;
                        continue;
                    }
                }
            }
            h.cool_s = (h.cool_s - dt).max(0.0);
            // Captains learn the ship at sea.
            if at_sea && h.def().sort == Sort::Specialist {
                let before = h.stars();
                h.xp += dt;
                if h.stars() > before {
                    changed = true;
                }
            }
            if let Way::Charged {
                fill: Fill::Voyages,
                need,
                ..
            } = h.def().way
                && h.def().later == 0
            {
                if h.seen < 0.0 {
                    h.seen = voyages as f64;
                }
                h.fill += (voyages as f64 - h.seen) as f32;
                h.seen = voyages as f64;
                if h.fill >= need && h.left_s <= 0.0 {
                    h.fill = 0.0;
                    fire.push((e, i));
                }
            }
        }
    }
    for (e, i) in fire {
        start(world, e, i);
    }
    if changed {
        world.resource_mut::<Changed>().any = true;
    }
}

/// A vessel's slots as the interface shows them.
#[derive(Clone, Debug, Serialize)]
pub struct GearSlot {
    pub slot: usize,
    /// "captain" or "gear".
    pub kind: &'static str,
    pub item: Option<ItemView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GearPanel {
    /// Pass this back in `isle.gear.*` actions.
    pub vessel: u32,
    pub name: String,
    pub craft: &'static str,
    pub slots: Vec<GearSlot>,
    pub kit: Kit,
    /// Shelf items that fit a free or taken slot here, by uid.
    pub fits: Vec<(usize, u32)>,
}

/// One vessel's captain and gear.
pub fn panel(world: &mut World, e: Entity) -> Option<GearPanel> {
    let v = world.get::<Vessel>(e)?.clone();
    let gear = world.get::<Gear>(e)?.clone();
    let n = gear_slots(v.craft) + 1;
    let slots = (0..n)
        .map(|i| GearSlot {
            slot: i,
            kind: if i == 0 { "captain" } else { "gear" },
            item: gear.slots[i]
                .as_ref()
                .map(|h| crate::items::view_of(h, now(h, &v))),
        })
        .collect();
    let mut fits_list = Vec::new();
    for h in &world.resource::<Items>().shelf {
        for i in 0..n {
            if fits(h.item, i, v.craft).is_ok() {
                fits_list.push((i, h.uid));
            }
        }
    }
    Some(GearPanel {
        vessel: e.index_u32(),
        name: v.name.clone(),
        craft: v.craft.name(),
        slots,
        kit: v.kit,
        fits: fits_list,
    })
}

/// What an item aboard is doing right now.
fn now(h: &Held, v: &Vessel) -> Vec<String> {
    if !h.acting() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for e in h.def().effects {
        match *e {
            Effect::Speed(_) => out.push(format!(
                "{:.1} knots now (craft's own {:.1})",
                v.craft.speed() * (1.0 + v.haste + v.kit.speed) * 1.944,
                v.craft.speed() * 1.944
            )),
            Effect::Hold(_) => out.push(format!(
                "{} hold slots aboard",
                v.craft.hold().0 + v.kit.hold
            )),
            Effect::Loads(_) => out.push(format!(
                "Turns round in {:.0} s",
                18.0 * (1.0 - v.kit.loads)
            )),
            _ => {}
        }
    }
    out
}

/// Every vessel with slots: for the fleet list.
pub fn fleet(world: &mut World) -> Vec<GearPanel> {
    let es: Vec<Entity> = world
        .query_filtered::<Entity, With<Gear>>()
        .iter(world)
        .collect();
    let mut out: Vec<GearPanel> = es.into_iter().filter_map(|e| panel(world, e)).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `isle.gear.<verb>:<args>`: `socket:<vessel>:<slot>:<uid>`, `unsocket:<vessel>:<slot>`,
/// `use:<vessel>:<slot>`.
pub fn action(world: &mut World, a: &str) -> Result<(), String> {
    let mut parts = a.split(':');
    let verb = parts.next().unwrap_or("");
    let nums: Vec<u32> = parts
        .map(|s| s.parse::<u32>().map_err(|_| format!("{s} is not a number")))
        .collect::<Result<_, _>>()?;
    let arg = |k: usize| nums.get(k).copied().ok_or("missing a number".to_string());
    match verb {
        "socket" => socket(world, arg(0)?, arg(1)? as usize, arg(2)?),
        "unsocket" => unsocket(world, arg(0)?, arg(1)? as usize),
        "use" => use_item(world, arg(0)?, arg(1)? as usize).map(|_| ()),
        _ => Err(format!("unknown gear action {verb}")),
    }
}

/// Gear given to the start's fleet, by vessel name prefix: (vessel, item names).
pub fn fit_out(world: &mut World, plan: &[(&str, &[&str])]) {
    let es: Vec<(Entity, String, Craft)> = world
        .query::<(Entity, &Vessel)>()
        .iter(world)
        .map(|(e, v)| (e, v.name.clone(), v.craft))
        .collect();
    for (prefix, names) in plan {
        let Some((e, _, craft)) = es.iter().find(|x| x.1.starts_with(prefix)).cloned() else {
            continue;
        };
        let mut gear = world.get::<Gear>(e).cloned().unwrap_or_default();
        let mut next = 1;
        for name in *names {
            let Some(i) = ITEMS.iter().position(|d| d.name == *name) else {
                continue;
            };
            let slot = if ITEMS[i].sort == Sort::Specialist {
                0
            } else {
                next += 1;
                next - 1
            };
            if fits(i, slot, craft).is_ok() {
                let h = world.resource_mut::<Items>().mint(i);
                gear.slots[slot] = Some(h);
            }
        }
        world.entity_mut(e).insert(gear);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> usize {
        ITEMS.iter().position(|d| d.name == name).expect(name)
    }

    #[test]
    fn gear_fits_its_craft_and_slot() {
        let captain = row("Captain Maren Holt (bold)");
        let sails = row("Studding sails");
        let gas = row("Gas cell lining");
        assert!(fits(captain, 0, Craft::Clipper).is_ok());
        assert!(fits(captain, 1, Craft::Clipper).is_err());
        assert!(fits(sails, 0, Craft::Clipper).is_err());
        assert!(fits(sails, 1, Craft::Clipper).is_ok());
        assert!(fits(gas, 1, Craft::Clipper).is_err());
        assert!(fits(gas, 1, Craft::Airship).is_ok());
        assert!(fits(sails, 2, Craft::Smack).is_err());
        // A hall's item never goes aboard.
        assert!(fits(row("Town band"), 1, Craft::Clipper).is_err());
        for c in [
            Craft::Schooner,
            Craft::Steamer,
            Craft::Smack,
            Craft::Barque,
            Craft::Airship,
            Craft::Biplane,
            Craft::Clipper,
            Craft::PaddleSteamer,
            Craft::Seaplane,
        ] {
            assert!(gear_slots(c) >= 1 && gear_slots(c) < MAX_GEAR, "{c:?}");
        }
    }

    #[test]
    fn the_start_fleet_sails_with_its_gear() {
        let mut w = crate::town::tests::bare_world();
        crate::town::build_start(&mut w);
        let kit = |w: &mut World, name: &str| {
            w.query::<&Vessel>()
                .iter(w)
                .find(|v| v.name.starts_with(name))
                .map(|v| v.kit)
                .expect(name)
        };
        assert!(kit(&mut w, "Morning Star").speed > 0.0);
        assert_eq!(kit(&mut w, "Albatross").hold, 1);
        let f = fleet(&mut w);
        assert!(f.iter().any(|p| p.slots.iter().any(|s| s.item.is_some())));
    }
}

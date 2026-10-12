//! Items for the inventory and shop screens: [`ItemDatabase`] (what items exist),
//! [`Inventory`] (what a character carries and wears) and [`Vendor`] (what a merchant sells).
//!
//! All plain data: add items with `resource.patch ItemDatabase {"items": {"relic": {...}}}`
//! or `inventory.give {"item": "potion_health", "count": 3}`. Games with their own item
//! systems can skip these and feed the screens their own data instead.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

/// Broad item category (inventory and shop tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemCategory {
    Weapon,
    Armor,
    Consumable,
    #[default]
    Material,
    Gadget,
    Quest,
}

impl ItemCategory {
    pub const ALL: [ItemCategory; 6] = [
        ItemCategory::Weapon,
        ItemCategory::Armor,
        ItemCategory::Consumable,
        ItemCategory::Material,
        ItemCategory::Gadget,
        ItemCategory::Quest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ItemCategory::Weapon => "Weapon",
            ItemCategory::Armor => "Armor",
            ItemCategory::Consumable => "Consumable",
            ItemCategory::Material => "Material",
            ItemCategory::Gadget => "Gadget",
            ItemCategory::Quest => "Quest item",
        }
    }
}

/// Where an item is worn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipSlot {
    Head,
    Chest,
    Hands,
    Feet,
    Back,
    MainHand,
    OffHand,
    Ring,
    Amulet,
}

impl EquipSlot {
    pub const ALL: [EquipSlot; 9] = [
        EquipSlot::Head,
        EquipSlot::Chest,
        EquipSlot::Hands,
        EquipSlot::Feet,
        EquipSlot::Back,
        EquipSlot::MainHand,
        EquipSlot::OffHand,
        EquipSlot::Ring,
        EquipSlot::Amulet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EquipSlot::Head => "Head",
            EquipSlot::Chest => "Chest",
            EquipSlot::Hands => "Hands",
            EquipSlot::Feet => "Feet",
            EquipSlot::Back => "Back",
            EquipSlot::MainHand => "Main hand",
            EquipSlot::OffHand => "Off hand",
            EquipSlot::Ring => "Ring",
            EquipSlot::Amulet => "Amulet",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            EquipSlot::Head => "head",
            EquipSlot::Chest => "chest",
            EquipSlot::Hands => "hands",
            EquipSlot::Feet => "feet",
            EquipSlot::Back => "back",
            EquipSlot::MainHand => "main_hand",
            EquipSlot::OffHand => "off_hand",
            EquipSlot::Ring => "ring",
            EquipSlot::Amulet => "amulet",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == k)
    }

    /// Icon shown in an empty equipment slot.
    pub fn placeholder_icon(self) -> &'static str {
        match self {
            EquipSlot::Head => "helmet",
            EquipSlot::Chest => "chest_armor",
            EquipSlot::Hands => "gloves",
            EquipSlot::Feet => "boots",
            EquipSlot::Back => "cloak",
            EquipSlot::MainHand => "sword",
            EquipSlot::OffHand => "shield_small",
            EquipSlot::Ring => "ring",
            EquipSlot::Amulet => "amulet",
        }
    }
}

/// One kind of item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemDef {
    pub name: String,
    /// Icon name (`ui.icons`).
    pub icon: String,
    /// 0 common, 1 uncommon, 2 rare, 3 epic, 4 legendary.
    pub rarity: u8,
    pub category: ItemCategory,
    pub description: String,
    /// Base price in gold.
    pub value: u32,
    /// Most per slot (1 = doesn't stack).
    pub stack: u32,
    /// Where it is worn, if equippable.
    pub slot: Option<EquipSlot>,
    /// Shown as stat rows and compared with what is equipped: `{"damage": 24, "speed": 1.1}`.
    pub stats: BTreeMap<String, f32>,
    /// kg.
    pub weight: f32,
}

impl Default for ItemDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            icon: String::new(),
            rarity: 0,
            category: ItemCategory::Material,
            description: String::new(),
            value: 1,
            stack: 1,
            slot: None,
            stats: BTreeMap::new(),
            weight: 0.1,
        }
    }
}

/// Every item that exists, by id.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemDatabase {
    pub items: BTreeMap<String, ItemDef>,
}

impl ItemDatabase {
    pub fn get(&self, id: &str) -> Option<&ItemDef> {
        self.items.get(id)
    }

    /// Add (or replace) an item.
    pub fn add(&mut self, id: &str, def: ItemDef) -> &mut Self {
        self.items.insert(id.to_string(), def);
        self
    }
}

#[allow(clippy::too_many_arguments)]
fn item(
    name: &str,
    icon: &str,
    rarity: u8,
    category: ItemCategory,
    value: u32,
    stack: u32,
    slot: Option<EquipSlot>,
    stats: &[(&str, f32)],
    weight: f32,
    description: &str,
) -> ItemDef {
    ItemDef {
        name: name.into(),
        icon: icon.into(),
        rarity,
        category,
        description: description.into(),
        value,
        stack,
        slot,
        stats: stats.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        weight,
    }
}

impl Default for ItemDatabase {
    /// A starter set covering every category and rarity (used by the showcase).
    fn default() -> Self {
        use EquipSlot::*;
        use ItemCategory::*;
        let mut db = ItemDatabase {
            items: BTreeMap::new(),
        };
        let list = [
            ("sword", item("Ranger's Sword", "sword", 1, Weapon, 120, 1, Some(MainHand), &[("damage", 24.0), ("speed", 1.1)], 3.2, "A well-balanced blade with a worn leather grip.")),
            ("greatsword", item("Sunforged Greatsword", "greatsword", 3, Weapon, 940, 1, Some(MainHand), &[("damage", 58.0), ("speed", 0.8)], 7.5, "Still warm from the heart of a dying star.")),
            ("dagger", item("Whisper Dagger", "dagger", 2, Weapon, 310, 1, Some(MainHand), &[("damage", 16.0), ("speed", 1.6), ("crit", 12.0)], 1.1, "Strikes before the target hears it.")),
            ("axe", item("Woodsman's Axe", "axe", 0, Weapon, 60, 1, Some(MainHand), &[("damage", 20.0), ("speed", 0.9)], 3.8, "Fells trees and the occasional bandit.")),
            ("hammer", item("Quake Hammer", "hammer", 2, Weapon, 420, 1, Some(MainHand), &[("damage", 42.0), ("speed", 0.7)], 9.0, "Each blow sends tremors through the ground.")),
            ("bow", item("Hunter's Longbow", "bow", 1, Weapon, 180, 1, Some(MainHand), &[("damage", 28.0), ("range", 60.0)], 1.9, "Yew and sinew, strung tight.")),
            ("staff", item("Staff of the Tides", "staff", 3, Weapon, 780, 1, Some(MainHand), &[("damage", 30.0), ("mana", 40.0)], 2.4, "Ripples with the pull of distant moons.")),
            ("pistol", item("Ion Pistol", "pistol", 2, Weapon, 520, 1, Some(MainHand), &[("damage", 34.0), ("range", 35.0)], 1.4, "Standard issue for frontier marshals.")),
            ("rifle", item("Longshot Rail Rifle", "rifle", 4, Weapon, 2400, 1, Some(MainHand), &[("damage", 96.0), ("range", 400.0), ("speed", 0.5)], 6.0, "Accelerates a tungsten slug to a tenth of light speed. Mind the recoil.")),
            ("shield", item("Bulwark Shield", "shield", 1, Armor, 150, 1, Some(OffHand), &[("armor", 18.0)], 5.5, "Dented, but never broken.")),
            ("shield_small", item("Buckler", "shield_small", 0, Armor, 45, 1, Some(OffHand), &[("armor", 7.0)], 2.0, "Light enough to parry with.")),
            ("helmet", item("Pathfinder Helm", "helmet", 1, Armor, 95, 1, Some(Head), &[("armor", 9.0)], 1.8, "A visor tuned to three suns.")),
            ("chest_armor", item("Scaled Cuirass", "chest_armor", 2, Armor, 380, 1, Some(Chest), &[("armor", 26.0)], 9.5, "Overlapping plates of drake scale.")),
            ("gloves", item("Grip Gloves", "gloves", 0, Armor, 30, 1, Some(Hands), &[("armor", 3.0)], 0.4, "Never drop your tools again.")),
            ("boots", item("Strider Boots", "boots", 2, Armor, 260, 1, Some(Feet), &[("armor", 6.0), ("speed", 1.08)], 1.2, "Light as air. You'll outrun your own echo.")),
            ("cloak", item("Nightveil Cloak", "cloak", 3, Armor, 650, 1, Some(Back), &[("armor", 8.0), ("stealth", 25.0)], 0.9, "Woven from the dark between stars.")),
            ("ring", item("Ring of Embers", "ring", 3, Armor, 820, 1, Some(Ring), &[("fire", 15.0)], 0.05, "A tiny flame dances inside the stone.")),
            ("amulet", item("Navigator's Amulet", "amulet", 4, Armor, 3100, 1, Some(Amulet), &[("luck", 20.0), ("mana", 25.0)], 0.1, "Points, always, toward home.")),
            ("potion_health", item("Health Potion", "potion_health", 0, Consumable, 25, 10, None, &[("heal", 40.0)], 0.3, "Restores 40 health over 3 seconds.")),
            ("potion_mana", item("Mana Draught", "potion_mana", 1, Consumable, 40, 10, None, &[("mana", 50.0)], 0.3, "Tastes of starlight and copper.")),
            ("potion_stamina", item("Stamina Tonic", "potion_stamina", 0, Consumable, 20, 10, None, &[("stamina", 60.0)], 0.3, "For when the legs say no.")),
            ("elixir", item("Elixir of Ages", "elixir", 4, Consumable, 1800, 3, None, &[("heal", 100.0), ("mana", 100.0)], 0.2, "One sip, and time forgets you for a while.")),
            ("medkit", item("Field Medkit", "medkit", 1, Consumable, 75, 5, None, &[("heal", 80.0)], 1.0, "Bandages, stims and a stern note from the medic.")),
            ("bread", item("Hearth Bread", "bread", 0, Consumable, 4, 20, None, &[("heal", 8.0)], 0.2, "Still warm. Someone's grandmother made this.")),
            ("meat", item("Grilled Meat", "meat", 0, Consumable, 9, 20, None, &[("heal", 15.0)], 0.4, "Don't ask what it was.")),
            ("fish", item("River Trout", "fish", 0, Consumable, 6, 20, None, &[("heal", 10.0)], 0.5, "Caught this morning.")),
            ("herb", item("Silverleaf", "herb", 0, Material, 3, 50, None, &[], 0.05, "A common herb used in potions.")),
            ("mushroom_glow", item("Glowcap", "mushroom_glow", 2, Material, 35, 50, None, &[], 0.05, "Glows faintly in the dark. Alchemists love it.")),
            ("crystal_glow", item("Resonant Crystal", "crystal_glow", 3, Material, 220, 20, None, &[], 0.3, "Hums when held near a ley line.")),
            ("ore_iron", item("Iron Ore", "ore_iron", 0, Material, 5, 50, None, &[], 1.0, "Rough and heavy.")),
            ("ore_gold", item("Gold Ore", "ore_gold", 2, Material, 60, 50, None, &[], 1.2, "Worth its weight, almost.")),
            ("gem_ruby", item("Ruby", "gem_ruby", 2, Material, 150, 20, None, &[], 0.02, "Deep red, flawless.")),
            ("gem_sapphire", item("Sapphire", "gem_sapphire", 2, Material, 160, 20, None, &[], 0.02, "Blue as an ice moon's sky.")),
            ("gem_emerald", item("Emerald", "gem_emerald", 3, Material, 240, 20, None, &[], 0.02, "Green fire caught in stone.")),
            ("wood_log", item("Oak Log", "wood_log", 0, Material, 2, 50, None, &[], 2.0, "Good for building and burning.")),
            ("circuit", item("Quantum Circuit", "circuit", 3, Material, 480, 20, None, &[], 0.1, "Exists in two states until you look at the price.")),
            ("fuel_cell", item("Fusion Fuel Cell", "fuel_cell", 2, Gadget, 300, 5, None, &[("fuel", 25.0)], 2.5, "Powers a ship's drive for one jump.")),
            ("grenade", item("Pulse Grenade", "grenade", 1, Gadget, 90, 10, None, &[("damage", 70.0)], 0.6, "Pull, throw, count to three. Or two.")),
            ("lockpick", item("Lockpick Set", "lockpick", 0, Gadget, 15, 10, None, &[], 0.1, "Every lock is a puzzle.")),
            ("torch", item("Torch", "torch", 0, Gadget, 3, 10, None, &[], 0.6, "Lights the way for an hour.")),
            ("rope", item("Climbing Rope", "rope", 0, Gadget, 12, 5, None, &[], 1.5, "Twenty meters of braided fibre.")),
            ("compass", item("Star Compass", "compass", 2, Gadget, 210, 1, None, &[], 0.3, "Points to the nearest jump gate.")),
            ("key", item("Vault Key", "key", 3, Quest, 0, 1, None, &[], 0.1, "Opens the vault beneath the old observatory.")),
            ("map_scroll", item("Old Star Chart", "map_scroll", 2, Quest, 0, 1, None, &[], 0.1, "Shows a system that isn't on any modern map.")),
            ("scroll", item("Sealed Letter", "scroll", 1, Quest, 0, 1, None, &[], 0.05, "Addressed to the merchant at Hollow Reach.")),
            ("spellbook", item("Tome of Gravity", "spellbook", 4, Quest, 0, 1, None, &[], 1.5, "Its pages turn themselves toward the nearest planet.")),
        ];
        for (id, def) in list {
            db.items.insert(id.to_string(), def);
        }
        db
    }
}

/// A stack of one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: String,
    pub count: u32,
}

impl ItemStack {
    pub fn new(item: &str, count: u32) -> Self {
        Self {
            item: item.into(),
            count,
        }
    }
}

/// What a character carries (slots) and wears (equipment), and their gold.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Inventory {
    /// Bag slots (the length is the capacity); `None` is empty.
    pub slots: Vec<Option<ItemStack>>,
    pub equipment: BTreeMap<EquipSlot, ItemStack>,
    pub gold: u64,
    /// Carry limit, kg.
    pub max_weight: f32,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::with_capacity(40)
    }
}

impl Inventory {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            slots: vec![None; n],
            equipment: BTreeMap::new(),
            gold: 0,
            max_weight: 80.0,
        }
    }

    /// Add items, filling existing stacks first. Returns how many didn't fit.
    pub fn add(&mut self, db: &ItemDatabase, item: &str, mut count: u32) -> u32 {
        let max = db.get(item).map(|d| d.stack.max(1)).unwrap_or(1);
        for s in self.slots.iter_mut().flatten() {
            if s.item == item && s.count < max {
                let n = (max - s.count).min(count);
                s.count += n;
                count -= n;
                if count == 0 {
                    return 0;
                }
            }
        }
        for s in self.slots.iter_mut() {
            if s.is_none() {
                let n = max.min(count);
                *s = Some(ItemStack::new(item, n));
                count -= n;
                if count == 0 {
                    return 0;
                }
            }
        }
        count
    }

    /// Remove up to `count` from slot `index`; returns the removed stack.
    pub fn take(&mut self, index: usize, count: u32) -> Option<ItemStack> {
        let slot = self.slots.get_mut(index)?;
        let s = slot.as_mut()?;
        let n = count.min(s.count);
        s.count -= n;
        let out = ItemStack::new(&s.item, n);
        if s.count == 0 {
            *slot = None;
        }
        Some(out)
    }

    /// How many of `item` are in the bag.
    pub fn count_of(&self, item: &str) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count)
            .sum()
    }

    /// Move slot `from` onto slot `to`: merges same items up to the stack size, otherwise
    /// swaps.
    pub fn move_slot(&mut self, db: &ItemDatabase, from: usize, to: usize) {
        if from == to || from >= self.slots.len() || to >= self.slots.len() {
            return;
        }
        let (a, b) = (self.slots[from].clone(), self.slots[to].clone());
        if let (Some(a), Some(b)) = (&a, &b)
            && a.item == b.item
        {
            let max = db.get(&a.item).map(|d| d.stack.max(1)).unwrap_or(1);
            let n = (max - b.count.min(max)).min(a.count);
            if n > 0 {
                self.slots[to].as_mut().unwrap().count += n;
                let left = a.count - n;
                self.slots[from] = (left > 0).then(|| ItemStack::new(&a.item, left));
                return;
            }
        }
        self.slots.swap(from, to);
    }

    /// Equip the item in slot `index` (swapping out what was worn there).
    pub fn equip(&mut self, db: &ItemDatabase, index: usize) -> Result<EquipSlot, String> {
        let stack = self
            .slots
            .get(index)
            .cloned()
            .flatten()
            .ok_or("That slot is empty.")?;
        let def = db
            .get(&stack.item)
            .ok_or_else(|| format!("Unknown item \"{}\".", stack.item))?;
        let slot = def
            .slot
            .ok_or_else(|| format!("{} can't be equipped.", def.name))?;
        let worn = self.equipment.insert(slot, ItemStack::new(&stack.item, 1));
        self.take(index, 1);
        if let Some(w) = worn {
            if self.slots[index].is_none() {
                self.slots[index] = Some(w);
            } else if self.add(db, &w.item, w.count) > 0 {
                // No room: put it back on.
                let back = self.equipment.insert(slot, w).unwrap();
                self.add(db, &back.item, 1);
                return Err("The bag is full.".into());
            }
        }
        Ok(slot)
    }

    /// Take off what is worn in `slot` into the bag (into `to` when given and empty).
    pub fn unequip(&mut self, db: &ItemDatabase, slot: EquipSlot, to: Option<usize>) -> Result<(), String> {
        let worn = self
            .equipment
            .get(&slot)
            .cloned()
            .ok_or("Nothing is worn there.")?;
        match to {
            Some(i) if self.slots.get(i).is_some_and(Option::is_none) => {
                self.slots[i] = Some(worn);
            }
            _ => {
                if self.add(db, &worn.item, worn.count) > 0 {
                    return Err("The bag is full.".into());
                }
            }
        }
        self.equipment.remove(&slot);
        Ok(())
    }

    /// Total carried weight (bag and equipment), kg.
    pub fn weight(&self, db: &ItemDatabase) -> f32 {
        self.slots
            .iter()
            .flatten()
            .chain(self.equipment.values())
            .map(|s| db.get(&s.item).map(|d| d.weight).unwrap_or(0.0) * s.count as f32)
            .sum()
    }

    /// Used slots.
    pub fn used(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Sort by category, then rarity (highest first), then name; merges stacks.
    pub fn sort(&mut self, db: &ItemDatabase) {
        let mut stacks: Vec<ItemStack> = self.slots.iter_mut().filter_map(Option::take).collect();
        let cap = self.slots.len();
        stacks.sort_by(|a, b| {
            let (da, dbb) = (db.get(&a.item), db.get(&b.item));
            let key = |d: Option<&ItemDef>, id: &str| {
                d.map(|d| (d.category, 4 - d.rarity.min(4), d.name.clone()))
                    .unwrap_or((ItemCategory::Quest, 9, id.to_string()))
            };
            key(da, &a.item).cmp(&key(dbb, &b.item))
        });
        self.slots = vec![None; cap];
        for s in stacks {
            self.add(db, &s.item, s.count);
        }
    }

    /// A stat summed over everything worn (e.g. total "armor").
    pub fn equipped_stat(&self, db: &ItemDatabase, stat: &str) -> f32 {
        self.equipment
            .values()
            .filter_map(|s| db.get(&s.item))
            .filter_map(|d| d.stats.get(stat))
            .sum()
    }
}

/// A merchant: what they sell and how they price it.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vendor {
    pub name: String,
    /// Portrait icon (`portrait:merchant`).
    pub portrait: String,
    pub greeting: String,
    /// Items for sale; a count of 0 means unlimited.
    pub stock: Vec<ItemStack>,
    /// Price multiplier when the player buys.
    pub markup: f32,
    /// Fraction of the value the vendor pays when the player sells.
    pub buy_rate: f32,
    pub gold: u64,
}

impl Default for Vendor {
    fn default() -> Self {
        Self {
            name: "Merchant".into(),
            portrait: "portrait:merchant".into(),
            greeting: "Take a look. Everything's genuine, mostly.".into(),
            stock: Vec::new(),
            markup: 1.0,
            buy_rate: 0.4,
            gold: 5000,
        }
    }
}

impl Vendor {
    /// Price the player pays for one `item`.
    pub fn price(&self, def: &ItemDef) -> u64 {
        ((def.value as f32 * self.markup).round() as u64).max(1)
    }

    /// What the vendor pays the player for one `item`.
    pub fn offer(&self, def: &ItemDef) -> u64 {
        (def.value as f32 * self.buy_rate).floor() as u64
    }
}

/// Buy `count` of stock entry `index` from `vendor` into `inv`. Returns the gold spent.
pub fn buy(db: &ItemDatabase, vendor: &mut Vendor, inv: &mut Inventory, index: usize, count: u32) -> Result<u64, String> {
    let entry = vendor
        .stock
        .get(index)
        .cloned()
        .ok_or("That item isn't for sale any more.")?;
    let def = db
        .get(&entry.item)
        .ok_or_else(|| format!("Unknown item \"{}\".", entry.item))?;
    let count = if entry.count > 0 { count.min(entry.count) } else { count };
    if count == 0 {
        return Err(format!("{} is sold out.", def.name));
    }
    let cost = vendor.price(def) * count as u64;
    if inv.gold < cost {
        return Err(format!("You need {} more gold.", cost - inv.gold));
    }
    let mut trial = inv.clone();
    if trial.add(db, &entry.item, count) > 0 {
        return Err("Your bag is full.".into());
    }
    *inv = trial;
    inv.gold -= cost;
    vendor.gold += cost;
    if entry.count > 0 {
        vendor.stock[index].count -= count;
        if vendor.stock[index].count == 0 {
            vendor.stock.remove(index);
        }
    }
    Ok(cost)
}

/// Sell `count` from bag slot `index` to `vendor`. Returns the gold earned.
pub fn sell(db: &ItemDatabase, vendor: &mut Vendor, inv: &mut Inventory, index: usize, count: u32) -> Result<u64, String> {
    let stack = inv
        .slots
        .get(index)
        .cloned()
        .flatten()
        .ok_or("That slot is empty.")?;
    let def = db
        .get(&stack.item)
        .ok_or_else(|| format!("Unknown item \"{}\".", stack.item))?;
    if def.category == ItemCategory::Quest {
        return Err(format!("{} is too important to sell.", def.name));
    }
    let count = count.min(stack.count);
    let earned = vendor.offer(def) * count as u64;
    if vendor.gold < earned {
        return Err(format!("{} can't afford that.", vendor.name));
    }
    inv.take(index, count);
    inv.gold += earned;
    vendor.gold -= earned;
    match vendor.stock.iter_mut().find(|s| s.item == stack.item && s.count > 0) {
        Some(s) => s.count += count,
        None => vendor.stock.push(ItemStack::new(&stack.item, count)),
    }
    Ok(earned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacking_moving_and_equipping() {
        let db = ItemDatabase::default();
        let mut inv = Inventory::with_capacity(4);
        assert_eq!(inv.add(&db, "potion_health", 13), 0);
        assert_eq!(inv.slots[0].as_ref().unwrap().count, 10);
        assert_eq!(inv.slots[1].as_ref().unwrap().count, 3);
        inv.move_slot(&db, 1, 0);
        assert_eq!(inv.count_of("potion_health"), 13);
        inv.add(&db, "sword", 1);
        inv.add(&db, "dagger", 1);
        let sword = inv.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.item == "sword")).unwrap();
        assert_eq!(inv.equip(&db, sword), Ok(EquipSlot::MainHand));
        let dagger = inv.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.item == "dagger")).unwrap();
        inv.equip(&db, dagger).unwrap();
        assert_eq!(inv.equipment[&EquipSlot::MainHand].item, "dagger");
        assert_eq!(inv.count_of("sword"), 1, "the sword went back into the bag");
        assert!(inv.equip(&db, inv.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.item == "potion_health")).unwrap()).is_err());
    }

    #[test]
    fn buying_and_selling() {
        let db = ItemDatabase::default();
        let mut v = Vendor {
            stock: vec![ItemStack::new("sword", 1), ItemStack::new("potion_health", 0)],
            ..Default::default()
        };
        let mut inv = Inventory::with_capacity(10);
        inv.gold = 100;
        assert!(buy(&db, &mut v, &mut inv, 0, 1).unwrap_err().contains("more gold"));
        inv.gold = 200;
        assert_eq!(buy(&db, &mut v, &mut inv, 0, 1), Ok(120));
        assert_eq!(v.stock.len(), 1, "the only sword is gone");
        assert_eq!(buy(&db, &mut v, &mut inv, 0, 3), Ok(75));
        assert_eq!(inv.gold, 5);
        let slot = inv.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.item == "sword")).unwrap();
        assert_eq!(sell(&db, &mut v, &mut inv, slot, 1), Ok(48));
        assert_eq!(inv.gold, 53);
        assert!(v.stock.iter().any(|s| s.item == "sword"));
    }

    #[test]
    fn sorting_groups_by_category_and_rarity() {
        let db = ItemDatabase::default();
        let mut inv = Inventory::with_capacity(8);
        for id in ["herb", "potion_health", "rifle", "sword", "herb"] {
            inv.add(&db, id, 1);
        }
        inv.sort(&db);
        let order: Vec<&str> = inv.slots.iter().flatten().map(|s| s.item.as_str()).collect();
        assert_eq!(order, ["rifle", "sword", "potion_health", "herb"]);
        assert_eq!(inv.count_of("herb"), 2);
    }
}

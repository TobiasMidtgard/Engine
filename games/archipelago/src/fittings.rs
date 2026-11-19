//! Fittings: what a hull is built with, chosen at the yard before the keel is laid.
//!
//! Every craft has a few fitting slots (none on a smack, up to four on a frigate or a liner).
//! Each slot takes one fitting: more canvas or a bigger engine for speed, oak or iron for a
//! stronger hull, guns of four kinds, an extra hold, marines, and for aircraft gas cells,
//! engines, a spotter's seat or a mail room. Fittings cost coins and goods on top of the
//! hull, add to its upkeep, and (all but the holds) take room from every hold. What a design
//! adds up to is its [`Stats`]: speed, hull, firepower and range, holds, upkeep. Speed and
//! holds act now; hull and guns count from the round with pirates and warships.
//!
//! Gear (`gear`) is different: items put aboard later and moved between ships. Fittings are
//! built in and stay with the hull.

use engine::prelude::*;
use serde::Serialize;

use crate::defs::Good;
use crate::items::Drive;
use crate::vessels::Craft;

/// Most fitting slots any craft has.
pub const MAX_FIT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fitting {
    /// Royals and studding sails: faster with the wind.
    Canvas,
    /// Copper over the bottom: no weed, a little faster, a little stronger.
    Copper,
    /// A compound engine and a bigger bunker.
    Compound,
    /// Oak frames doubled: a much stronger hull, a little slower.
    Oak,
    /// Iron plate over the waterline (steam only).
    Plate,
    /// Swivel guns on the rails: short range, all round.
    Swivels,
    /// Carronades: heavy short guns on the broadside.
    Carronades,
    /// Long guns: far-reaching, fore and aft.
    LongGuns,
    /// A mortar: the longest reach and the heaviest shell, slow and wild.
    Mortar,
    /// Another hold.
    Hold,
    /// Marines: a boarding party and guards for the cargo.
    Marines,
    /// Another gas cell (airships): one more hold.
    GasCell,
    /// Bigger engines (aircraft).
    Engines,
    /// A spotter's seat and glass: sees further through the fog.
    Spotter,
    /// A mail room: postage on every landing.
    MailRoom,
}

/// One fitting's row.
#[derive(Clone, Copy, Debug)]
pub struct FittingDef {
    pub fitting: Fitting,
    pub name: &'static str,
    pub blurb: &'static str,
    /// What it can go on.
    pub drives: &'static [Drive],
    pub coins: u32,
    pub cost: &'static [(Good, u32)],
    /// Coins a minute more to keep.
    pub upkeep: f32,
    /// Share faster (or slower).
    pub speed: f32,
    /// Share more hull.
    pub hull: f32,
    /// Damage a minute and reach in meters (guns).
    pub guns: (f32, f32),
    /// Extra holds.
    pub holds: u32,
    /// Share of every hold it takes.
    pub room: f32,
    /// Share further through the fog.
    pub sight: f32,
    /// Coins on every landing.
    pub postage: f32,
    /// Most of this fitting on one craft.
    pub most: usize,
}

const fn row(fitting: Fitting, name: &'static str, drives: &'static [Drive]) -> FittingDef {
    FittingDef {
        fitting,
        name,
        blurb: "",
        drives,
        coins: 0,
        cost: &[],
        upkeep: 0.0,
        speed: 0.0,
        hull: 0.0,
        guns: (0.0, 0.0),
        holds: 0,
        room: 0.12,
        sight: 0.0,
        postage: 0.0,
        most: 1,
    }
}

const SEA: &[Drive] = &[Drive::Sea];

pub const FITTINGS: &[FittingDef] = &[
    FittingDef {
        blurb: "Royals, skysails and studding sails: 15 % faster under sail.",
        coins: 300,
        cost: &[(Good::Clothes, 10)],
        upkeep: 1.0,
        speed: 0.15,
        room: 0.05,
        ..row(Fitting::Canvas, "More canvas", &[Drive::Sail])
    },
    FittingDef {
        blurb: "Copper sheathing keeps the weed off: 6 % faster and a tenth stronger.",
        coins: 600,
        cost: &[(Good::Planks, 4)],
        upkeep: 1.0,
        speed: 0.06,
        hull: 0.1,
        room: 0.0,
        ..row(Fitting::Copper, "Copper bottom", &[Drive::Sail])
    },
    FittingDef {
        blurb: "A compound engine and a bigger bunker: 18 % faster, and it eats more coal.",
        coins: 900,
        cost: &[(Good::Bricks, 6), (Good::Oil, 4)],
        upkeep: 4.0,
        speed: 0.18,
        room: 0.1,
        ..row(Fitting::Compound, "Compound engine", &[Drive::Steam])
    },
    FittingDef {
        blurb: "Doubled oak frames: half again the hull, 4 % slower.",
        coins: 400,
        cost: &[(Good::Planks, 16)],
        upkeep: 1.0,
        speed: -0.04,
        hull: 0.5,
        room: 0.08,
        ..row(Fitting::Oak, "Oak frames", SEA)
    },
    FittingDef {
        blurb: "Iron plate over the waterline: twice the hull, 8 % slower.",
        coins: 900,
        cost: &[(Good::Bricks, 10), (Good::Planks, 6)],
        upkeep: 3.0,
        speed: -0.08,
        hull: 1.0,
        room: 0.1,
        ..row(Fitting::Plate, "Iron plate", &[Drive::Steam])
    },
    FittingDef {
        blurb: "Swivel guns on the rails: 60 m, all round. Keeps sloops off.",
        coins: 300,
        cost: &[(Good::Planks, 4)],
        upkeep: 1.5,
        guns: (6.0, 60.0),
        room: 0.06,
        most: 2,
        ..row(Fitting::Swivels, "Swivel guns", SEA)
    },
    FittingDef {
        blurb: "Carronades on the broadside: heavy shot at 120 m.",
        coins: 600,
        cost: &[(Good::Planks, 6), (Good::Bricks, 4)],
        upkeep: 3.0,
        guns: (18.0, 120.0),
        room: 0.15,
        most: 2,
        ..row(Fitting::Carronades, "Carronades", SEA)
    },
    FittingDef {
        blurb: "Long guns fore and aft: lighter shot, out to 260 m.",
        coins: 800,
        cost: &[(Good::Planks, 6), (Good::Bricks, 6)],
        upkeep: 3.0,
        guns: (10.0, 260.0),
        room: 0.12,
        most: 2,
        ..row(Fitting::LongGuns, "Long guns", SEA)
    },
    FittingDef {
        blurb: "A mortar amidships: the heaviest shell, out to 400 m, slow and wild. For \
                batteries and coves.",
        coins: 1_200,
        cost: &[(Good::Bricks, 12)],
        upkeep: 5.0,
        speed: -0.03,
        guns: (24.0, 400.0),
        room: 0.2,
        ..row(Fitting::Mortar, "Mortar", SEA)
    },
    FittingDef {
        blurb: "Another hold, built in where guns would go.",
        coins: 400,
        cost: &[(Good::Planks, 10)],
        upkeep: 1.0,
        speed: -0.03,
        holds: 1,
        room: 0.0,
        most: 2,
        ..row(Fitting::Hold, "Extra hold", SEA)
    },
    FittingDef {
        blurb: "A boarding party and guards for the cargo: half again the hull in a fight.",
        coins: 500,
        cost: &[(Good::Clothes, 6)],
        upkeep: 3.0,
        hull: 0.3,
        guns: (4.0, 20.0),
        room: 0.1,
        ..row(Fitting::Marines, "Marines", SEA)
    },
    FittingDef {
        blurb: "Another gas cell: one more hold aloft.",
        coins: 700,
        cost: &[(Good::Clothes, 12), (Good::Oil, 4)],
        upkeep: 2.0,
        holds: 1,
        room: 0.0,
        ..row(Fitting::GasCell, "Gas cell", &[Drive::Air])
    },
    FittingDef {
        blurb: "Bigger engines: 15 % faster through the air.",
        coins: 500,
        cost: &[(Good::Oil, 6)],
        upkeep: 2.0,
        speed: 0.15,
        room: 0.05,
        ..row(Fitting::Engines, "Bigger engines", &[Drive::Air])
    },
    FittingDef {
        blurb: "A spotter's seat and a long glass: clears the fog half as far again.",
        coins: 300,
        cost: &[(Good::Glass, 2)],
        upkeep: 1.0,
        sight: 0.5,
        room: 0.05,
        ..row(
            Fitting::Spotter,
            "Spotter's seat",
            &[Drive::Air, Drive::Sail, Drive::Steam],
        )
    },
    FittingDef {
        blurb: "A mail room: 15 coins on every landing.",
        coins: 400,
        cost: &[(Good::Planks, 4)],
        upkeep: 1.0,
        postage: 15.0,
        room: 0.1,
        ..row(Fitting::MailRoom, "Mail room", &[Drive::Air, Drive::Steam])
    },
];

impl Fitting {
    pub fn def(self) -> &'static FittingDef {
        FITTINGS
            .iter()
            .find(|d| d.fitting == self)
            .expect("every fitting has a row")
    }

    pub fn from_name(s: &str) -> Option<Fitting> {
        let s = s.replace(['-', ' '], "_").to_ascii_lowercase();
        FITTINGS.iter().map(|d| d.fitting).find(|f| {
            serde_json::to_value(f)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                == Some(s.clone())
        })
    }

    pub fn fits(self, c: Craft) -> bool {
        self.def().drives.iter().any(|d| d.fits(c))
    }
}

/// Fitting slots a craft has.
pub fn slots(c: Craft) -> usize {
    match c {
        Craft::Smack => 0,
        Craft::Biplane | Craft::Seaplane => 1,
        Craft::Cutter | Craft::Schooner | Craft::PaddleSteamer | Craft::FlyingBoat => 2,
        Craft::Brig | Craft::Barque | Craft::Gunboat | Craft::Steamer | Craft::Clipper => 3,
        Craft::Airship => 3,
        Craft::Frigate | Craft::Liner => 4,
    }
}

/// Hull points of a bare craft.
pub fn base_hull(c: Craft) -> f32 {
    match c {
        Craft::Smack => 40.0,
        Craft::Cutter => 80.0,
        Craft::Schooner => 100.0,
        Craft::Brig => 150.0,
        Craft::Barque => 170.0,
        Craft::Clipper => 160.0,
        Craft::Frigate => 320.0,
        Craft::PaddleSteamer => 150.0,
        Craft::Gunboat => 220.0,
        Craft::Steamer => 240.0,
        Craft::Liner => 300.0,
        Craft::Biplane | Craft::Seaplane => 30.0,
        Craft::FlyingBoat => 60.0,
        Craft::Airship => 120.0,
    }
}

/// What a hull is built with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Fit(pub [Option<Fitting>; MAX_FIT]);

impl Fit {
    pub fn of(list: &[Fitting]) -> Fit {
        let mut f = Fit::default();
        for (i, x) in list.iter().take(MAX_FIT).enumerate() {
            f.0[i] = Some(*x);
        }
        f
    }

    pub fn iter(&self) -> impl Iterator<Item = Fitting> + '_ {
        self.0.iter().flatten().copied()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.iter().map(|f| f.def().name).collect()
    }
}

/// Whether a craft can be built with `fit`.
pub fn check(c: Craft, fit: &Fit) -> Result<(), String> {
    let n = fit.iter().count();
    if n > slots(c) {
        return Err(format!("A {} has {} fitting slots.", c.name(), slots(c)));
    }
    for f in fit.iter() {
        let d = f.def();
        if !f.fits(c) {
            return Err(format!("{} does not go on a {}.", d.name, c.name()));
        }
        if fit.iter().filter(|x| *x == f).count() > d.most {
            return Err(format!("At most {} of {} on one craft.", d.most, d.name));
        }
    }
    Ok(())
}

/// What the fittings cost on top of the hull.
pub fn price(fit: &Fit) -> (u32, Vec<(Good, u32)>) {
    let mut coins = 0;
    let mut goods: Vec<(Good, u32)> = Vec::new();
    for f in fit.iter() {
        let d = f.def();
        coins += d.coins;
        for (g, n) in d.cost {
            match goods.iter_mut().find(|x| x.0 == *g) {
                Some(x) => x.1 += n,
                None => goods.push((*g, *n)),
            }
        }
    }
    (coins, goods)
}

pub fn upkeep(fit: &Fit) -> f32 {
    fit.iter().map(|f| f.def().upkeep).sum()
}

/// What a design adds up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Stats {
    /// Cruising speed, m/s.
    pub speed: f32,
    pub hull: f32,
    /// Damage a minute of all its guns, and the reach of the longest.
    pub firepower: f32,
    pub range: f32,
    pub holds: u32,
    /// Goods per hold.
    pub per_hold: u32,
    /// Coins a minute, hull and fittings.
    pub upkeep: f32,
    pub sight: f32,
    pub postage: f32,
}

/// The share of each hold left once the fittings have taken their room (never under half).
pub fn room_left(fit: &Fit) -> f32 {
    (1.0 - fit.iter().map(|f| f.def().room).sum::<f32>()).max(0.5)
}

pub fn stats(c: Craft, fit: &Fit) -> Stats {
    let (holds, per) = c.hold();
    let mut s = Stats {
        speed: c.speed(),
        hull: base_hull(c),
        holds,
        per_hold: per,
        upkeep: crate::shipyard::upkeep_of(c) + upkeep(fit),
        ..Default::default()
    };
    let (mut fast, mut strong) = (0.0, 0.0);
    for f in fit.iter() {
        let d = f.def();
        fast += d.speed;
        strong += d.hull;
        s.firepower += d.guns.0;
        s.range = s.range.max(d.guns.1);
        s.holds += d.holds;
        s.sight += d.sight;
        s.postage += d.postage;
    }
    s.speed *= 1.0 + fast;
    s.hull *= 1.0 + strong;
    s.per_hold = (per as f32 * room_left(fit)).round() as u32;
    if holds == 0 && s.holds > 0 {
        s.per_hold = 20;
    }
    s
}

/// How each craft comes out of the yard if no one chooses: the fittings it is known for.
pub fn usual(c: Craft) -> Fit {
    use Fitting::*;
    Fit::of(match c {
        Craft::Cutter => &[Swivels],
        Craft::Brig => &[Swivels, Hold],
        Craft::Frigate => &[LongGuns, Carronades, Carronades, Oak],
        Craft::Gunboat => &[LongGuns, Plate],
        Craft::Liner => &[Compound, MailRoom, Hold],
        Craft::FlyingBoat => &[Spotter],
        Craft::Clipper => &[Canvas, Copper],
        _ => &[],
    })
}

/// A design saved under a name, to order again.
#[derive(Clone, Debug, Serialize)]
pub struct Design {
    pub name: String,
    pub craft: Craft,
    pub fit: Fit,
}

#[derive(Resource, Default, Debug)]
pub struct Designs(pub Vec<Design>);

/// Goods per hold once the fittings have taken their room. A craft with no holds of its own
/// (a fishing smack, a plane) gets 20 a hold from an extra hold or gas cell.
pub fn per_hold(per: u32, holds: u32, room: f32) -> u32 {
    if per == 0 {
        return if holds > 0 { 20 } else { 0 };
    }
    (per as f32 * (1.0 - room)).round() as u32
}

/// What fittings add to a vessel's [`crate::gear::Kit`]: speed, holds, room, sight, postage.
pub fn add_to_kit(fit: &Fit, kit: &mut crate::gear::Kit) {
    for f in fit.iter() {
        let d = f.def();
        kit.speed += d.speed;
        kit.hold += d.holds;
        kit.sight += d.sight;
        kit.postage += d.postage;
    }
    kit.room = 1.0 - room_left(fit);
}

// ------------------------------------------------------------------ on the hull

/// Deck height, half beam and half length of a hull, for putting fittings on it.
fn deck_of(c: Craft) -> Option<(f32, f32, f32)> {
    Some(match c {
        Craft::Smack => (0.85, 1.8, 5.5),
        Craft::Schooner => (1.5, 3.3, 15.0),
        Craft::Steamer => (2.4, 4.6, 28.0),
        Craft::Barque => (2.1, 4.3, 22.0),
        Craft::Clipper => (2.2, 5.0, 31.0),
        Craft::PaddleSteamer => (1.9, 3.5, 21.0),
        Craft::Cutter => (1.3, 2.6, 10.0),
        Craft::Brig => (1.8, 3.8, 17.0),
        Craft::Frigate => (3.4, 5.7, 26.0),
        Craft::Gunboat => (1.4, 3.3, 15.0),
        Craft::Liner => (3.8, 5.7, 40.0),
        _ => return None,
    })
}

/// A gun on a little carriage at `at`, its barrel `len` long pointing along `dir`.
fn gun(p: &mut crate::art::Parts, at: Vec3, dir: Vec3, len: f32, bore: f32) {
    use crate::art::{Mat, hexc, v3};
    p.box_at(
        Mat::Timber,
        at + v3(0.0, 0.18, 0.0),
        v3(0.35, 0.18, 0.3),
        dir.z.atan2(dir.x),
        hexc(0x5a3a22),
    );
    p.rod(
        Mat::Iron,
        at + v3(0.0, 0.45, 0.0) - dir * len * 0.3,
        at + v3(0.0, 0.5, 0.0) + dir * len * 0.7,
        bore,
        8,
        hexc(0x1c1c1e),
    );
}

/// What a fitting looks like on a craft's deck, in the craft's own space (bow +X), if it
/// shows at all.
pub fn visual(c: Craft, f: Fitting) -> Option<crate::art::Parts> {
    use crate::art::{Mat, Parts, hexc, v3};
    let (deck, hb, hl) = deck_of(c)?;
    if !f.fits(c) {
        return None;
    }
    let mut p = Parts::default();
    match f {
        Fitting::Swivels => {
            for x in [hl * 0.45, -hl * 0.55] {
                for z in [-1.0f32, 1.0] {
                    let at = v3(x, deck, z * (hb * 0.8));
                    p.rod(
                        Mat::Timber,
                        at,
                        at + v3(0.0, 0.9, 0.0),
                        0.06,
                        6,
                        hexc(0x4a3020),
                    );
                    p.rod(
                        Mat::Iron,
                        at + v3(-0.3, 1.0, 0.0),
                        at + v3(0.6, 1.05, z * 0.4),
                        0.07,
                        6,
                        hexc(0x2a2a2a),
                    );
                }
            }
        }
        Fitting::Carronades => {
            let n = ((hl * 0.9) / 3.5).clamp(2.0, 7.0) as i32;
            for i in 0..n {
                let x = -hl * 0.4 + hl * 0.8 * i as f32 / (n - 1).max(1) as f32;
                for z in [-1.0f32, 1.0] {
                    gun(
                        &mut p,
                        v3(x, deck, z * (hb * 0.86 - 0.6)),
                        v3(0.0, 0.0, z),
                        1.4,
                        0.2,
                    );
                }
            }
        }
        Fitting::LongGuns => {
            gun(&mut p, v3(hl * 0.62, deck, 0.0), Vec3::X, 2.8, 0.13);
            gun(&mut p, v3(-hl * 0.7, deck, 0.0), -Vec3::X, 2.8, 0.13);
        }
        Fitting::Mortar => {
            p.cylinder(
                Mat::Timber,
                v3(-hl * 0.1, deck, 0.0),
                1.2,
                1.1,
                0.4,
                12,
                hexc(0x5a3a22),
                true,
            );
            let mut m = Parts::default();
            m.cylinder(
                Mat::Copper,
                Vec3::ZERO,
                0.55,
                0.45,
                1.3,
                12,
                hexc(0x9a7a3a),
                false,
            );
            p.add_parts(
                &m,
                Mat4::from_rotation_translation(
                    Quat::from_rotation_z(-0.5),
                    v3(-hl * 0.1, deck + 0.4, 0.0),
                ),
            );
        }
        Fitting::Hold => {
            // Deck cargo: crates and barrels lashed down aft of the main hatch.
            let x0 = -hl * 0.25;
            for (k, (dx, dz)) in [(0.0f32, -0.8f32), (1.3, 0.7), (-1.2, 0.6), (0.4, 0.0)]
                .into_iter()
                .enumerate()
            {
                let h = 0.5 + (k % 2) as f32 * 0.25;
                p.box_at(
                    Mat::Planks,
                    v3(x0 + dx, deck + h, dz * hb * 0.45),
                    v3(0.6, h, 0.55),
                    0.1 * k as f32,
                    hexc(0x9a7a4a),
                );
            }
            for dz in [-1.0f32, 1.0] {
                p.cylinder(
                    Mat::Planks,
                    v3(x0 + 2.5, deck, dz * hb * 0.4),
                    0.4,
                    0.4,
                    0.95,
                    10,
                    hexc(0x7a5430),
                    true,
                );
            }
        }
        Fitting::Plate => {
            for z in [-1.0f32, 1.0] {
                p.cuboid(
                    Mat::Iron,
                    v3(-hl * 0.55, 0.2, z * (hb + 0.03) - 0.06),
                    v3(hl * 0.5, deck - 0.2, z * (hb + 0.03) + 0.06),
                    hexc(0x50565c),
                    false,
                );
            }
        }
        Fitting::Copper => {
            for z in [-1.0f32, 1.0] {
                p.cuboid(
                    Mat::Copper,
                    v3(-hl * 0.6, -0.15, z * (hb * 0.98) - 0.05),
                    v3(hl * 0.55, 0.12, z * (hb * 0.98) + 0.05),
                    hexc(0xc07840),
                    false,
                );
            }
        }
        Fitting::Marines => {
            for k in 0..5 {
                let x = hl * 0.25 + k as f32 * 0.9;
                let z = if k % 2 == 0 { -0.6 } else { 0.6 };
                crate::incidents::figure(
                    &mut p,
                    v3(x, deck, z),
                    hexc(0xb02020),
                    hexc(0x101010),
                    1.8,
                );
                p.rod(
                    Mat::Iron,
                    v3(x + 0.1, deck + 0.9, z + 0.25),
                    v3(x + 0.2, deck + 2.3, z + 0.3),
                    0.03,
                    4,
                    hexc(0x2a2a2a),
                );
            }
        }
        Fitting::Spotter => {
            // A raised lookout platform forward with a long glass on a stand.
            let at = v3(hl * 0.5, deck, 0.0);
            for (dx, dz) in [(-0.8f32, -0.8f32), (0.8, -0.8), (-0.8, 0.8), (0.8, 0.8)] {
                p.beam(
                    Mat::Timber,
                    at + v3(dx, 0.0, dz),
                    at + v3(dx, 3.0, dz),
                    0.12,
                    hexc(0x5a3a22),
                );
            }
            p.cuboid(
                Mat::Planks,
                at + v3(-1.0, 3.0, -1.0),
                at + v3(1.0, 3.2, 1.0),
                hexc(0x8a6a40),
                true,
            );
            p.rod(
                Mat::Copper,
                at + v3(-0.2, 3.9, 0.0),
                at + v3(1.0, 4.1, 0.0),
                0.08,
                8,
                hexc(0xc8a040),
            );
        }
        _ => return None,
    }
    Some(p.finish())
}

/// Put the fittings that show on vessel `e`'s hull, as parts of its body.
pub fn dress(world: &mut World, e: Entity) {
    let Some(art) = world.get_resource::<crate::draw::Art>().cloned() else {
        return;
    };
    let Some(v) = world.get::<crate::fleet::Vessel>(e).cloned() else {
        return;
    };
    let t = world.get::<Transform>(e).copied().unwrap_or_default();
    let mut seen = Vec::new();
    for f in v.fit.iter() {
        if seen.contains(&f) {
            continue;
        }
        seen.push(f);
        let Some(model) = art.fitted.get(&(v.craft, f)).cloned() else {
            continue;
        };
        let parts = crate::draw::spawn_model(world, &art, &model, t, true);
        if let Some(mut body) = world.get_mut::<crate::draw::Body>(e) {
            body.parts.extend(parts);
        }
    }
}

fn parse(craft: Craft, list: &serde_json::Value) -> Result<Fit, String> {
    let mut out = Vec::new();
    for v in list.as_array().ok_or("fittings wants a list of names")? {
        let name = v.as_str().ok_or("a fitting is a name")?;
        out.push(Fitting::from_name(name).ok_or(format!("no fitting called {name}"))?);
    }
    if out.len() > MAX_FIT {
        return Err(format!("At most {MAX_FIT} fittings."));
    }
    let fit = Fit::of(&out);
    check(craft, &fit)?;
    Ok(fit)
}

/// A fitting list from the interface's action text: names joined by commas.
pub fn parse_list(craft: Craft, s: &str) -> Result<Fit, String> {
    let names: Vec<serde_json::Value> = s
        .split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| serde_json::Value::String(x.trim().to_string()))
        .collect();
    parse(craft, &serde_json::Value::Array(names))
}

/// Save (or replace) a design under `name`.
pub fn save(world: &mut World, name: &str, craft: Craft, fit: Fit) -> Result<(), String> {
    check(craft, &fit)?;
    let mut d = world.resource_mut::<Designs>();
    d.0.retain(|x| x.name != name);
    d.0.push(Design {
        name: name.to_string(),
        craft,
        fit,
    });
    Ok(())
}

/// `isle.fittings`, `isle.yard.design`, `isle.yard.designs`, `isle.yard.order_fitted`,
/// `isle.ship.fit`: fittings for scripts, the playtest and the interface.
pub fn register(app: &mut App) {
    use serde_json::{Value, json};
    fn fitting_json(d: &FittingDef) -> Value {
        json!({
            "key": d.fitting, "name": d.name, "blurb": d.blurb,
            "for": d.drives.iter().map(|x| x.words_pub()).collect::<Vec<_>>(),
            "coins": d.coins, "cost": d.cost.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>(),
            "upkeep": d.upkeep, "speed": d.speed, "hull": d.hull, "damage": d.guns.0, "range": d.guns.1,
            "holds": d.holds, "room": d.room, "most": d.most,
        })
    }
    app.register_api(
        "isle.fittings",
        "Every fitting a hull can be built with, and for each craft its slots, bare stats and usual fittings.",
        "{}",
        |_, _| {
            let fittings: Vec<Value> = FITTINGS.iter().map(fitting_json).collect();
            let crafts: Vec<Value> = Craft::ALL
                .iter()
                .map(|c| {
                    let usual = usual(*c);
                    json!({
                        "craft": c, "name": c.name(), "slots": slots(*c),
                        "fits": FITTINGS.iter().filter(|d| d.fitting.fits(*c)).map(|d| d.fitting).collect::<Vec<_>>(),
                        "bare": stats(*c, &Fit::default()),
                        "usual": usual.iter().collect::<Vec<_>>(), "usual_stats": stats(*c, &usual),
                    })
                })
                .collect();
            Ok(json!({"fittings": fittings, "crafts": crafts}))
        },
    )
    .register_api(
        "isle.yard.design",
        "Work out (and with a name, save) a design: a craft and its fittings. Returns its price and stats.",
        r#"{"craft": "frigate", "fittings": ["long_guns", "carronades", "carronades", "oak"], "name": "Escort"}"#,
        |world, args| {
            let craft = args["craft"].as_str().and_then(Craft::from_name).ok_or("unknown craft")?;
            let fit = parse(craft, &args["fittings"])?;
            if let Some(name) = args["name"].as_str() {
                save(world, name, craft, fit)?;
            }
            let (coins, goods) = price(&fit);
            let plan = crate::shipyard::plan(craft).ok_or("that craft is not built")?;
            Ok(json!({
                "craft": craft, "fittings": fit.names(), "stats": stats(craft, &fit),
                "coins": plan.coins + coins,
                "fittings_cost": goods.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>(),
            }))
        },
    )
    .register_api(
        "isle.yard.designs",
        "The designs saved so far.",
        "{}",
        |world, _| {
            let d: Vec<Value> = world
                .resource::<Designs>()
                .0
                .iter()
                .map(|d| json!({"name": d.name, "craft": d.craft, "fittings": d.fit.names(), "stats": stats(d.craft, &d.fit)}))
                .collect();
            Ok(json!({"designs": d}))
        },
    )
    .register_api(
        "isle.yard.order_fitted",
        "Order a craft built with these fittings (or a saved design by name) at a shipyard or airship mast.",
        r#"{"yard": 12, "craft": "brig", "fittings": ["swivels", "hold"]}"#,
        |world, args| {
            let id = args["yard"].as_u64().ok_or("yard wants a building id")? as u32;
            let e = crate::build::by_id(world, id).ok_or(format!("no building {id}"))?;
            let (craft, fit) = if let Some(name) = args["design"].as_str() {
                let d = world
                    .resource::<Designs>()
                    .0
                    .iter()
                    .find(|d| d.name == name)
                    .cloned()
                    .ok_or(format!("no design called {name}"))?;
                (d.craft, d.fit)
            } else {
                let craft = args["craft"].as_str().and_then(Craft::from_name).ok_or("unknown craft")?;
                (craft, parse(craft, &args["fittings"])?)
            };
            crate::shipyard::order_fitted(world, e, craft, fit)?;
            Ok(json!({"ordered": craft.name(), "fittings": fit.names()}))
        },
    )
    .register_api(
        "isle.ship.fit",
        "A craft's fittings and what they add up to: speed, hull, firepower and range, holds, upkeep.",
        r#"{"vessel": "Whimbrel"}"#,
        |world, args| {
            let key = match &args["vessel"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return Err("vessel wants a name or an id".into()),
            };
            let e = crate::orders::find_vessel(world, &key).ok_or(format!("no vessel {key}"))?;
            let v = world.get::<crate::fleet::Vessel>(e).ok_or("no such vessel")?;
            Ok(json!({"name": v.name, "craft": v.craft, "slots": slots(v.craft), "fittings": v.fit.names(), "stats": stats(v.craft, &v.fit)}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_usual_fit_is_allowed_and_guns_take_room() {
        for c in Craft::ALL {
            check(c, &usual(c)).unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
        let f = stats(Craft::Frigate, &usual(Craft::Frigate));
        assert!(f.firepower > 40.0 && f.range >= 260.0, "{f:?}");
        assert!(f.hull > base_hull(Craft::Frigate));
        let bare = stats(Craft::Brig, &Fit::default());
        let gunned = stats(
            Craft::Brig,
            &Fit::of(&[Fitting::Carronades, Fitting::Carronades]),
        );
        for c in Craft::ALL {
            for d in FITTINGS {
                if let Some(p) = visual(c, d.fitting) {
                    assert!(p.triangles() < 6_000, "{c:?} {:?}", d.fitting);
                }
            }
        }
        assert!(gunned.per_hold < bare.per_hold);
        assert!(
            check(
                Craft::Cutter,
                &Fit::of(&[Fitting::Swivels, Fitting::Swivels, Fitting::Hold])
            )
            .is_err()
        );
        assert!(check(Craft::Schooner, &Fit::of(&[Fitting::Plate])).is_err());
        assert_eq!(Fitting::from_name("long guns"), Some(Fitting::LongGuns));
    }
}

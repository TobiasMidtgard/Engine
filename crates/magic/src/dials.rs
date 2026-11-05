//! The seven domains: what every spell turns, underneath its player-facing name.
//!
//! Every spell turns one or more dials of reality up or down on a target
//! (engine-design/spell-combination.md, section 2). Each [`Domain`] is one physical quantity the
//! engine already simulates, with two ends ([`Pole`]s): Energy is heat and cold, Force is push
//! and pull, Life is grow and wither, Matter is form and unform, Mind is reveal (and bind) and
//! veil (and free), Space is near and far, Time is fast and slow. A spell's [`Dials`] are the
//! signed amount on each, so a weave of two spells is simply both sets of dials at once, and the
//! world's reaction (fire, physics, foliage, AI) reads only the dials it cares about.

use serde::{Deserialize, Serialize};

/// One of the seven dials of reality a spell can turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    /// Heat ↔ cold: burn, freeze, light, lightning.
    Energy,
    /// Push ↔ pull, with a direction: knock back, lift, fly, crush, shields.
    Force,
    /// Grow ↔ wither: heal, regenerate, poison, rot, raise the dead.
    Life,
    /// Form ↔ unform: conjure, reshape, transmute, dissolve.
    Matter,
    /// Reveal and bind ↔ veil and free: perception and will.
    Mind,
    /// Near ↔ far: blink, portals, pull across a room, banish.
    Space,
    /// Fast ↔ slow: haste, slow, stasis, rewind.
    Time,
}

impl Domain {
    pub const ALL: [Domain; 7] = [
        Domain::Energy,
        Domain::Force,
        Domain::Life,
        Domain::Matter,
        Domain::Mind,
        Domain::Space,
        Domain::Time,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Domain::Energy => "Energy",
            Domain::Force => "Force",
            Domain::Life => "Life",
            Domain::Matter => "Matter",
            Domain::Mind => "Mind",
            Domain::Space => "Space",
            Domain::Time => "Time",
        }
    }

    /// The positive and the negative end of this dial.
    pub fn poles(self) -> (Pole, Pole) {
        match self {
            Domain::Energy => (Pole::Heat, Pole::Cold),
            Domain::Force => (Pole::Push, Pole::Pull),
            Domain::Life => (Pole::Grow, Pole::Wither),
            Domain::Matter => (Pole::Form, Pole::Unform),
            Domain::Mind => (Pole::Reveal, Pole::Veil),
            Domain::Space => (Pole::Far, Pole::Near),
            Domain::Time => (Pole::Fast, Pole::Slow),
        }
    }

    /// Learnable from the start of the game; Space and Time open mid-game (around level 40).
    pub fn open_from_start(self) -> bool {
        !matches!(self, Domain::Space | Domain::Time)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One end of a [`Domain`]: 14 in all. A weave is built from poles, so heat with push is a
/// blast while cold with push is a shard storm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pole {
    Heat,
    Cold,
    Push,
    Pull,
    Grow,
    Wither,
    Form,
    Unform,
    Reveal,
    Veil,
    Far,
    Near,
    Fast,
    Slow,
}

impl Pole {
    pub const ALL: [Pole; 14] = [
        Pole::Heat,
        Pole::Cold,
        Pole::Push,
        Pole::Pull,
        Pole::Grow,
        Pole::Wither,
        Pole::Form,
        Pole::Unform,
        Pole::Reveal,
        Pole::Veil,
        Pole::Far,
        Pole::Near,
        Pole::Fast,
        Pole::Slow,
    ];

    pub fn domain(self) -> Domain {
        match self {
            Pole::Heat | Pole::Cold => Domain::Energy,
            Pole::Push | Pole::Pull => Domain::Force,
            Pole::Grow | Pole::Wither => Domain::Life,
            Pole::Form | Pole::Unform => Domain::Matter,
            Pole::Reveal | Pole::Veil => Domain::Mind,
            Pole::Far | Pole::Near => Domain::Space,
            Pole::Fast | Pole::Slow => Domain::Time,
        }
    }

    /// +1 for the first end of its domain, -1 for the second.
    pub fn sign(self) -> f32 {
        if self.domain().poles().0 == self {
            1.0
        } else {
            -1.0
        }
    }

    pub fn opposite(self) -> Pole {
        let (a, b) = self.domain().poles();
        if a == self { b } else { a }
    }

    pub fn name(self) -> &'static str {
        match self {
            Pole::Heat => "Heat",
            Pole::Cold => "Cold",
            Pole::Push => "Push",
            Pole::Pull => "Pull",
            Pole::Grow => "Grow",
            Pole::Wither => "Wither",
            Pole::Form => "Form",
            Pole::Unform => "Unform",
            Pole::Reveal => "Reveal",
            Pole::Veil => "Veil",
            Pole::Far => "Far",
            Pole::Near => "Near",
            Pole::Fast => "Fast",
            Pole::Slow => "Slow",
        }
    }
}

/// How far a spell turns each of the seven dials, signed, about -1..1 per unit of power
/// (heat +, cold −; push +, pull −; grow +, wither −; form +, unform −; reveal +, veil −;
/// far +, near −; fast +, slow −). The payload every world reaction reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dials {
    pub energy: f32,
    pub force: f32,
    pub life: f32,
    pub matter: f32,
    pub mind: f32,
    pub space: f32,
    pub time: f32,
}

/// Below this a dial counts as untouched.
pub const DIAL_EPSILON: f32 = 0.05;

impl Dials {
    pub const ZERO: Dials = Dials {
        energy: 0.0,
        force: 0.0,
        life: 0.0,
        matter: 0.0,
        mind: 0.0,
        space: 0.0,
        time: 0.0,
    };

    pub fn get(&self, d: Domain) -> f32 {
        self.as_array()[d.index()]
    }

    pub fn set(&mut self, d: Domain, v: f32) {
        let mut a = self.as_array();
        a[d.index()] = v;
        *self = Self::from_array(a);
    }

    /// One pole turned by `amount` (its sign comes from the pole).
    pub fn pole(p: Pole, amount: f32) -> Dials {
        let mut d = Dials::ZERO;
        d.set(p.domain(), p.sign() * amount);
        d
    }

    /// Several poles turned fully (a weave written as poles).
    pub fn from_poles(poles: &[Pole]) -> Dials {
        poles
            .iter()
            .fold(Dials::ZERO, |acc, p| acc + Dials::pole(*p, 1.0))
    }

    pub fn as_array(&self) -> [f32; 7] {
        [
            self.energy,
            self.force,
            self.life,
            self.matter,
            self.mind,
            self.space,
            self.time,
        ]
    }

    pub fn from_array(a: [f32; 7]) -> Dials {
        Dials {
            energy: a[0],
            force: a[1],
            life: a[2],
            matter: a[3],
            mind: a[4],
            space: a[5],
            time: a[6],
        }
    }

    pub fn scaled(&self, k: f32) -> Dials {
        Dials::from_array(self.as_array().map(|v| v * k))
    }

    /// How many domains this turns (its depth: 1 for a plain spell, 2 for a pair weave).
    pub fn depth(&self) -> usize {
        self.as_array()
            .iter()
            .filter(|v| v.abs() > DIAL_EPSILON)
            .count()
    }

    /// The turned poles, strongest first.
    pub fn poles(&self) -> Vec<Pole> {
        let mut out: Vec<(Pole, f32)> = Domain::ALL
            .iter()
            .filter_map(|d| {
                let v = self.get(*d);
                (v.abs() > DIAL_EPSILON).then(|| {
                    let (pos, neg) = d.poles();
                    (if v > 0.0 { pos } else { neg }, v.abs())
                })
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out.into_iter().map(|(p, _)| p).collect()
    }

    /// The strongest pole, if any dial is turned.
    pub fn dominant(&self) -> Option<Pole> {
        self.poles().first().copied()
    }

    /// The sum of every dial's size: how much reality this changes per unit of power.
    pub fn magnitude(&self) -> f32 {
        self.as_array().iter().map(|v| v.abs()).sum()
    }

    /// How hard two sets of dials pull against each other, 0..1: the share of their combined
    /// size that cancels (fire against frost is 1, fire with push is 0). Weaves that pull hard in
    /// opposite directions are unstable: stronger, with a chance to backfire.
    pub fn tension(&self, other: &Dials) -> f32 {
        let (a, b) = (self.as_array(), other.as_array());
        let total: f32 = a.iter().zip(&b).map(|(x, y)| x.abs() + y.abs()).sum();
        if total <= 0.0 {
            return 0.0;
        }
        let cancel: f32 = a
            .iter()
            .zip(&b)
            .filter(|(x, y)| x.signum() != y.signum() && x.abs() > 0.0 && y.abs() > 0.0)
            .map(|(x, y)| 2.0 * x.abs().min(y.abs()))
            .sum();
        (cancel / total).clamp(0.0, 1.0)
    }

    /// Both sets of dials at once, the way two spells woven together turn reality: each domain
    /// keeps the larger push of the two in each direction, so fire with frost keeps both heat and
    /// cold (an unstable steam weave) rather than cancelling to nothing.
    pub fn weave(&self, other: &Dials) -> Dials {
        let (a, b) = (self.as_array(), other.as_array());
        let mut out = [0.0; 7];
        for i in 0..7 {
            let (x, y) = (a[i], b[i]);
            out[i] = if x.signum() == y.signum() || x == 0.0 || y == 0.0 {
                // Same way: they add, but no more than either at full strength would push.
                (x + y).clamp(-1.5, 1.5)
            } else {
                // Opposed: whichever is stronger wins, the other is kept as tension.
                if x.abs() >= y.abs() { x } else { y }
            };
        }
        Dials::from_array(out)
    }
}

impl std::ops::Add for Dials {
    type Output = Dials;
    fn add(self, rhs: Dials) -> Dials {
        let (a, b) = (self.as_array(), rhs.as_array());
        Dials::from_array(std::array::from_fn(|i| a[i] + b[i]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pole_belongs_to_one_domain_and_has_an_opposite() {
        for p in Pole::ALL {
            assert_eq!(p.opposite().opposite(), p);
            assert_eq!(p.opposite().domain(), p.domain());
            assert_eq!(p.sign(), -p.opposite().sign());
            let d = Dials::pole(p, 1.0);
            assert_eq!(d.dominant(), Some(p), "{p:?}");
            assert_eq!(d.depth(), 1);
        }
        assert_eq!(Pole::ALL.len(), 2 * Domain::ALL.len());
    }

    #[test]
    fn opposite_poles_are_tense_and_unrelated_ones_are_not() {
        let fire = Dials::pole(Pole::Heat, 1.0);
        let frost = Dials::pole(Pole::Cold, 1.0);
        let push = Dials::pole(Pole::Push, 1.0);
        assert!((fire.tension(&frost) - 1.0).abs() < 1e-6);
        assert_eq!(fire.tension(&push), 0.0);
        // A weave keeps the stronger of opposed dials and adds dials that agree.
        let steam = fire.weave(&frost.scaled(0.8));
        assert_eq!(steam.energy, 1.0);
        let blast = fire.weave(&push);
        assert_eq!(blast.depth(), 2);
        assert_eq!(blast.poles(), vec![Pole::Heat, Pole::Push]);
    }
}

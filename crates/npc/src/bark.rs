//! Barks: what NPCs shout to each other in a fight, so the player hears a plan coming (one of
//! the fairness floors every tier keeps, engine-design/npc-difficulty.md 4.4; F.E.A.R.'s
//! callouts). Each bark is a [`NoiseEvent`] (enemies near enough hear it, as they hear a
//! callout) and an entry in [`Barks`], which game code reads for voice lines and subtitles.

use engine_core::prelude::*;
use engine_world::NoiseEvent;

/// Barks kept in [`Barks::said`] this long (s).
const KEEP: f64 = 10.0;
/// How far a bark carries (m).
pub const BARK_RADIUS: f32 = 30.0;

/// What was shouted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BarkLine {
    /// "Flush them out!": an archer calls a mate round the foe's cover.
    Flush,
    /// "Going round!": the mate sent round answers.
    GoingRound,
    /// "On my mark... loose!": the archers loose together.
    Volley,
    /// "Arrow over!": an arrow lobbed over cover.
    Lob,
    /// "Fall back!": a push that failed pulls back to try again.
    FallBack,
}

impl BarkLine {
    pub fn name(self) -> &'static str {
        match self {
            BarkLine::Flush => "flush",
            BarkLine::GoingRound => "going_round",
            BarkLine::Volley => "volley",
            BarkLine::Lob => "lob",
            BarkLine::FallBack => "fall_back",
        }
    }
}

/// One bark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bark {
    /// World clock (s).
    pub time: f64,
    pub speaker: Entity,
    pub line: BarkLine,
    /// Where it was shouted from (head height).
    pub at: DVec3,
}

/// The barks of the last few seconds, newest last.
#[derive(Resource, Debug, Default, Clone)]
pub struct Barks {
    pub said: Vec<Bark>,
}

impl Barks {
    /// Shout `line` from `speaker` at `at` (feet): kept here, and a noise for whoever is near.
    pub(crate) fn say(
        &mut self,
        now: f64,
        speaker: Entity,
        line: BarkLine,
        at: DVec3,
        noise: &mut MessageWriter<NoiseEvent>,
    ) {
        self.said.retain(|b| now - b.time < KEEP);
        let at = at + DVec3::Y * 1.6;
        self.said.push(Bark {
            time: now,
            speaker,
            line,
            at,
        });
        noise.write(NoiseEvent {
            position: at,
            radius: BARK_RADIUS,
            source: Some(speaker),
        });
    }

    /// The barks `speaker` gave since `since` (world clock).
    pub fn by(&self, speaker: Entity, since: f64) -> impl Iterator<Item = &Bark> {
        self.said
            .iter()
            .filter(move |b| b.speaker == speaker && b.time >= since)
    }
}

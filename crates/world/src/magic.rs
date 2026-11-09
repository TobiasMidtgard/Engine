//! Spells as other modules see them: what a spell looks and sounds like ([`SpellLook`],
//! [`SpellCue`]) and what a body is suffering ([`Statuses`]). Magic writes them; vfx draws
//! them, audio sounds them, and anim and NPC AI react to statuses. Proposals and reasons:
//! engine-design/magic-contracts.md.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

/// What part a spell's carrier plays, so vfx and audio pick the right effect for a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SpellRole {
    /// A projectile in flight.
    #[default]
    Bolt,
    /// An area on the ground or in the air.
    Field,
    /// A piece of a wall.
    Wall,
    /// Around a caster or a target.
    Aura,
    /// Gathering in the caster's hands before it is cast.
    Charge,
    /// A status shown on a body (see [`Statuses`]).
    Status,
}

/// On a spell's carrier (bolt, field, wall piece, aura): what it looks like while it lasts.
/// vfx attaches an effect that follows the entity, audio loops its sound; removing the
/// component (or the entity) stops both.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellLook {
    /// "fire", "frost", "storm", ..., or a weave of several ("steam", "plasma", "doom").
    pub look: String,
    pub role: SpellRole,
    /// Size against the look's default: a bolt's radius / 0.16 m, a field's radius / 2 m.
    pub scale: f32,
    /// Linear RGB tint, for weaves vfx has no look of its own for yet.
    pub color: [f32; 3],
    /// The spell's energy, J, so one look can grow from a spark to a sun.
    pub power_j: f32,
}

impl Default for SpellLook {
    fn default() -> Self {
        Self {
            look: String::new(),
            role: SpellRole::Bolt,
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
            power_j: 0.0,
        }
    }
}

/// What a spell leaves on a surface it hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SpellMark {
    #[default]
    Scorch,
    Frost,
    Wet,
    Blood,
    Rot,
    Void,
    Burn,
}

/// One-shot spell moments for vfx and audio.
#[derive(Message, Debug, Clone, PartialEq)]
pub enum SpellCue {
    /// A spell struck at `at`, on a surface facing `normal`.
    Impact {
        at: DVec3,
        normal: Vec3,
        look: String,
        scale: f32,
        color: [f32; 3],
    },
    /// A cone breathed from `at` along `dir`.
    Cone {
        at: DVec3,
        dir: Vec3,
        look: String,
        range_m: f32,
    },
    /// A beam or arc from `from` to `to` for `secs`.
    Beam {
        from: DVec3,
        to: DVec3,
        look: String,
        width_m: f32,
        secs: f32,
    },
    /// A cast that failed (no mana, interrupted).
    Fizzle { at: DVec3, look: String },
    /// A mark left on a surface.
    Mark {
        at: DVec3,
        normal: Vec3,
        kind: SpellMark,
        radius_m: f32,
    },
}

/// What a body is suffering, written by magic each tick; read-only for everyone else (vfx:
/// flames, frost crust, sparks; anim: a frozen body holds its pose, a shocked one twitches, a
/// rooted one keeps its feet planted, dread cowers; NPC AI: a burning NPC runs, a frightened
/// one flees, a rooted one can't move off; audio: sizzle, crackle).
/// Magic also slows frozen and shocked bodies through `LocalTimeScale`. Objects burning in the
/// fire simulation use `Combustible`'s burn state, not this.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Statuses {
    /// Burning intensity, 0..1.
    pub burning: f32,
    /// Chill stacks; `frozen` once they reach magic's threshold.
    pub chill: u8,
    pub frozen: bool,
    pub shocked: bool,
    /// Bleeding, 0..1.
    pub bleeding: f32,
    /// Rot (blight), 0..1.
    pub rot: f32,
    /// Dread (fear), 0..1.
    pub dread: f32,
    /// Held fast (a stone grip on the feet): anim keeps the feet planted, AI can't move off.
    pub rooted: bool,
    /// A held shield just broke in this body's hands (0.6 s): anim plays a stagger; magic slows
    /// the body's time scale meanwhile.
    pub staggered: bool,
    /// Holding a shield up: anim raises the off hand, movement walks 30% slower.
    pub shielding: bool,
}

/// Present while a module draws and sounds spells from [`SpellLook`], [`SpellCue`] and
/// [`Statuses`] (see [`draws_spell_looks`]); magic then stops its own stand-ins, so nothing is
/// drawn twice. Check for it when systems run, not in `Plugin::build`, since the drawing plugin
/// may be added later.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct SpellLookDrawer {
    pub drawn_by: String,
}

/// Call from an effects plugin's `build` once it draws [`SpellLook`], [`SpellCue`] and
/// [`Statuses`].
pub fn draws_spell_looks(app: &mut App, name: &str) {
    if app.world.get_resource::<SpellLookDrawer>().is_none() {
        app.insert_resource(SpellLookDrawer {
            drawn_by: name.to_string(),
        });
    }
}

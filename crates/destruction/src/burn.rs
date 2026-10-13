//! Wooden building pieces burn. Each wooden [`StructuralPiece`] gets an
//! `engine_world::Combustible` holding its wood as fuel; the fire module (when the game
//! installs one) heats, ignites and burns it. Here a burning piece chars (scorched, then
//! glowing char, smoldering embers, cold char) and loses strength with its fuel (see
//! `collapse::burn_strength`), so a burning building sags piece by piece and comes down. A
//! fallen piece keeps burning where it lands.

use engine_core::prelude::*;
use engine_render::MeshRenderer;
use engine_world::{BurnState, Combustible};

use crate::DestructionSettings;
use crate::assets::DestructionAssets;
use crate::dress::PropMesh;
use crate::material::{DestructionMaterials, MaterialProps};
use crate::structure::{StructuralPiece, StructureGraph};

/// How charred something looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CharLevel {
    /// Browned and blackened, the grain still showing.
    Scorched,
    /// Checked black char with embers glowing in the cracks (flaming).
    Glowing,
    /// Char with a dull glow (smoldering).
    Smoldering,
    /// Cold char and ash.
    Cold,
}

impl CharLevel {
    /// Index into `DestructionAssets::char`.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The look of a combustible that started with `fuel0_kg`, or `None` while unburnt.
    pub fn of(c: &Combustible, fuel0_kg: f32) -> Option<Self> {
        let left = if fuel0_kg > 0.0 {
            (c.fuel_kg / fuel0_kg).clamp(0.0, 1.0)
        } else {
            1.0
        };
        match c.state {
            BurnState::Burning if left > 0.85 => Some(CharLevel::Scorched),
            BurnState::Burning => Some(CharLevel::Glowing),
            BurnState::Smoldering => Some(CharLevel::Smoldering),
            BurnState::Burnt => Some(CharLevel::Cold),
            // Put out, or not lit yet: show what the fire already did.
            BurnState::Intact if left < 0.85 => Some(CharLevel::Cold),
            BurnState::Intact if left < 0.999 => Some(CharLevel::Scorched),
            BurnState::Intact => None,
        }
    }
}

/// Chars an entity's look as its `Combustible` burns: the entity's own mesh and its prop
/// meshes ([`crate::dress::PropModel`]) take the char materials.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Charring {
    /// Fuel when it was whole, kg.
    pub fuel0_kg: f32,
    /// The look it has now.
    pub level: Option<CharLevel>,
    /// Strength left when support was last solved (1 = intact).
    pub strength: f32,
}

impl Default for Charring {
    fn default() -> Self {
        Self {
            fuel0_kg: 1.0,
            level: None,
            strength: 1.0,
        }
    }
}

/// Moisture of seasoned building timber (fraction of dry mass).
pub const TIMBER_MOISTURE: f32 = 0.12;
/// Ignition temperature of wood, °C.
pub const WOOD_IGNITION_C: f32 = 300.0;

/// Give each new wooden building piece its fuel (Update).
pub fn make_wood_combustible_system(
    mut commands: Commands,
    settings: Res<DestructionSettings>,
    table: Res<DestructionMaterials>,
    pieces: Query<
        (Entity, &StructuralPiece, Option<&Combustible>),
        (Added<StructuralPiece>, Without<Charring>),
    >,
) {
    for (e, piece, fire) in &pieces {
        let props = table.get(piece.material);
        if let Some(c) = fire {
            // The owner set its own fuel; just char it.
            commands.entity(e).insert(Charring {
                fuel0_kg: c.fuel_kg,
                ..Charring::default()
            });
            continue;
        }
        if !settings.wood_burns || props.fuel_kg_m3 <= 0.0 {
            continue;
        }
        let h = piece.half_extents;
        let fuel = 8.0 * h.x * h.y * h.z * props.fuel_kg_m3;
        commands.entity(e).insert((
            Combustible {
                fuel_kg: fuel,
                moisture: TIMBER_MOISTURE,
                ignition_c: WOOD_IGNITION_C,
                ..Combustible::default()
            },
            Charring {
                fuel0_kg: fuel,
                ..Charring::default()
            },
        ));
    }
}

/// Fraction of a burning wooden piece's fuel left (1 = intact, 0 = burnt): of what it
/// started with ([`Charring`]) or else of what solid wood of its size holds.
pub fn fuel_left(
    piece: &StructuralPiece,
    props: &MaterialProps,
    fire: Option<&Combustible>,
    charring: Option<&Charring>,
) -> f32 {
    let Some(c) = fire else {
        return 1.0;
    };
    match c.state {
        BurnState::Intact => 1.0,
        BurnState::Burnt => 0.0,
        BurnState::Smoldering | BurnState::Burning => {
            let full = charring.map(|ch| ch.fuel0_kg).unwrap_or_else(|| {
                let v = 8.0 * piece.half_extents.x * piece.half_extents.y * piece.half_extents.z;
                v * props.fuel_kg_m3
            });
            (c.fuel_kg / full.max(1e-3)).clamp(0.0, 1.0)
        }
    }
}

/// How much of its material's strength a burning wooden piece has left (1 = intact): the
/// square of its [`fuel_left`]. Strength goes faster than mass: the char layer carries
/// nothing, the wood just under it is weakened by the heat, and a beam's bending strength
/// falls with its shrinking core faster than the core's area does.
pub fn burn_strength(
    piece: &StructuralPiece,
    props: &MaterialProps,
    fire: Option<&Combustible>,
    charring: Option<&Charring>,
) -> f32 {
    fuel_left(piece, props, fire, charring).powi(2)
}

/// Re-solve support when a burning piece has lost a noticeable part of its strength (fixed
/// step, before `collapse::structure_system`); the fire module changes every `Combustible`
/// every step, so reacting to each change would rebuild the graph all the time.
pub fn burn_weaken_system(
    table: Res<DestructionMaterials>,
    mut graph: ResMut<StructureGraph>,
    mut pieces: Query<(&StructuralPiece, &Combustible, &mut Charring), Changed<Combustible>>,
) {
    for (piece, fire, mut charring) in &mut pieces {
        let k = burn_strength(
            piece,
            &table.get(piece.material),
            Some(fire),
            Some(&charring),
        );
        let crossed_zero = (k <= 0.0) != (charring.strength <= 0.0);
        if (k - charring.strength).abs() >= 0.04 || crossed_zero {
            charring.strength = k;
            graph.dirty = true;
        }
    }
}

/// Swap charring things to the char material for their level (Update, after the support
/// tint, so a burning piece shows its char even with `ShowSupport` on).
pub fn char_system(
    assets: Option<Res<DestructionAssets>>,
    mut burning: Query<(Entity, &Combustible, &mut Charring, Option<&Children>)>,
    prop_meshes: Query<(), With<PropMesh>>,
    mut renderers: Query<&mut MeshRenderer>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (e, fire, mut charring, children) in &mut burning {
        let Some(level) = CharLevel::of(fire, charring.fuel0_kg) else {
            continue;
        };
        if charring.level != Some(level) {
            charring.level = Some(level);
        }
        let material = engine_render::MaterialRef::Handle(assets.char[level.index()]);
        let own = std::iter::once(e);
        let kids = children
            .into_iter()
            .flat_map(|c| c.iter())
            .filter(|c| prop_meshes.contains(*c));
        for t in own.chain(kids) {
            if let Ok(mut r) = renderers.get_mut(t)
                && r.material != material
            {
                r.material = material.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fire(state: BurnState, fuel: f32) -> Combustible {
        Combustible {
            fuel_kg: fuel,
            state,
            ..Combustible::default()
        }
    }

    #[test]
    fn char_level_follows_the_burn() {
        assert_eq!(CharLevel::of(&fire(BurnState::Intact, 10.0), 10.0), None);
        assert_eq!(
            CharLevel::of(&fire(BurnState::Burning, 9.5), 10.0),
            Some(CharLevel::Scorched)
        );
        assert_eq!(
            CharLevel::of(&fire(BurnState::Burning, 5.0), 10.0),
            Some(CharLevel::Glowing)
        );
        assert_eq!(
            CharLevel::of(&fire(BurnState::Smoldering, 1.0), 10.0),
            Some(CharLevel::Smoldering)
        );
        assert_eq!(
            CharLevel::of(&fire(BurnState::Burnt, 0.0), 10.0),
            Some(CharLevel::Cold)
        );
        // Put out halfway: it stays charred.
        assert_eq!(
            CharLevel::of(&fire(BurnState::Intact, 5.0), 10.0),
            Some(CharLevel::Cold)
        );
    }
}
